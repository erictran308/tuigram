//! An Opus decoder (RFC 6716, RFC 8251), for voice messages: the parts of
//! the `opus-decoder` crate 0.1.1 (<https://github.com/TadeuszWolfGang/Rusopus>,
//! MIT OR Apache-2.0, ported from libopus, see `COPYING`) that decoding one
//! stream takes. Copied rather than depended on because that release writes
//! debug logs to fixed paths on every frame; those, its tests and what
//! tuigram doesn't use were left out. It has no unsafe code, and decodes
//! packets other people send, so a panic in it is caught (`voice::play`).

#![forbid(unsafe_code)]
#![allow(
    clippy::erasing_op,
    clippy::identity_op,
    clippy::precedence,
    clippy::int_plus_one,
    clippy::too_many_arguments,
    clippy::needless_range_loop,
    clippy::excessive_precision
)]

mod celt;
mod entropy;
mod error;
mod packet;
mod silk;

use std::fmt;

use crate::opus::entropy::EcDec;
pub(crate) use error::Error;

/// Decodes one Opus stream to 16-bit samples.
pub struct OpusDecoder {
    decoder: Decoder,
}

impl OpusDecoder {
    /// Most samples a packet decodes to, per channel, at 48 kHz.
    pub const MAX_FRAME_SIZE_48K: usize = Decoder::MAX_FRAME_SIZE_48K;

    /// `sample_rate` is 8000, 12000, 16000, 24000 or 48000, and `channels`
    /// 1 or 2; a stream with the other number of channels is mixed to fit.
    pub fn new(sample_rate: u32, channels: u8) -> Result<Self, OpusError> {
        let decoder = Decoder::new(sample_rate, channels)?;
        Ok(Self { decoder })
    }

    /// Decodes `packet` into `pcm` (interleaved if stereo), returning the
    /// samples per channel; an empty `packet` conceals a lost one.
    pub fn decode(&mut self, packet: &[u8], pcm: &mut [i16]) -> Result<usize, OpusError> {
        let packet = (!packet.is_empty()).then_some(packet);
        Ok(self.decoder.decode(packet, pcm)?)
    }
}

#[derive(Debug)]
pub enum OpusError {
    /// The packet is malformed or inconsistent.
    InvalidPacket,
    /// The decoder hit a state it doesn't handle.
    InternalError,
    BufferTooSmall,
    InvalidArgument(&'static str),
}

impl fmt::Display for OpusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpusError::InvalidPacket => write!(f, "invalid packet"),
            OpusError::InternalError => write!(f, "internal error"),
            OpusError::BufferTooSmall => write!(f, "buffer too small"),
            OpusError::InvalidArgument(what) => write!(f, "invalid argument: {what}"),
        }
    }
}

impl std::error::Error for OpusError {}

impl From<Error> for OpusError {
    fn from(value: Error) -> Self {
        match value {
            Error::InvalidSampleRate => Self::InvalidArgument("sample_rate"),
            Error::InvalidChannels => Self::InvalidArgument("channels"),
            Error::PacketTooLarge | Error::BadPacket => Self::InvalidPacket,
            Error::OutputTooSmall => Self::BufferTooSmall,
            Error::NotImplemented => Self::InternalError,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Decoder {
    fs_hz: u32,
    channels: u8,
    celt: celt::CeltDecoder,
    silk: silk::SilkDecoder,
    prev_mode: Option<OpusMode>,
    prev_redundancy: bool,
    loss_count: u32,
    last_packet_duration: usize,
    last_output: Vec<i16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpusMode {
    SilkOnly,
    Hybrid,
    CeltOnly,
}

impl Decoder {
    pub(crate) const MAX_FRAME_SIZE_48K: usize = 5760; // 120 ms @ 48 kHz

    pub(crate) fn new(fs_hz: u32, channels: u8) -> Result<Self, Error> {
        if !matches!(fs_hz, 8000 | 12000 | 16000 | 24000 | 48000) {
            return Err(Error::InvalidSampleRate);
        }
        if !matches!(channels, 1 | 2) {
            return Err(Error::InvalidChannels);
        }
        Ok(Self {
            fs_hz,
            channels,
            celt: celt::CeltDecoder::new(fs_hz, channels),
            silk: silk::SilkDecoder::new(fs_hz, channels),
            prev_mode: None,
            prev_redundancy: false,
            loss_count: 0,
            last_packet_duration: 0,
            last_output: Vec::new(),
        })
    }

    /// Conceal a lost non-CELT frame by fading the previous PCM.
    ///
    /// Params: per-channel `frame_size`, consecutive `loss_count`, and mutable `out`.
    /// Returns: nothing; `out` receives interleaved concealed PCM.
    fn conceal_with_fade(&self, frame_size: usize, loss_count: u32, out: &mut [i16]) {
        let channels = self.channels as usize;
        let needed = frame_size * channels;
        if self.last_output.len() < needed {
            out[..needed].fill(0);
            return;
        }

        let fade = 0.9f32.powi((loss_count.min(10) + 1) as i32);
        for (dst, src) in out[..needed]
            .iter_mut()
            .zip(self.last_output[..needed].iter())
        {
            let sample = f32::from(*src) * fade;
            *dst = sample.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        }
    }

    /// Persist the most recent decoded interleaved PCM frame.
    ///
    /// Params: decoded `out` buffer and `samples_per_channel` written into it.
    /// Returns: nothing; the decoder keeps a copy for future PLC fallback.
    fn store_last_output(&mut self, out: &[i16], samples_per_channel: usize) {
        let written = samples_per_channel * self.channels as usize;
        self.last_output.clear();
        self.last_output.extend_from_slice(&out[..written]);
    }

    /// Decode a lost packet using CELT PLC or a safe fade fallback.
    ///
    /// Params: mutable interleaved `out` buffer.
    /// Returns: concealed sample count per channel.
    fn decode_lost_packet(&mut self, out: &mut [i16]) -> Result<usize, Error> {
        let samples_per_channel = self.last_packet_duration;
        if samples_per_channel == 0 {
            return Ok(0);
        }

        let needed = samples_per_channel * self.channels as usize;
        if out.len() < needed {
            return Err(Error::OutputTooSmall);
        }

        out[..needed].fill(0);
        match self.prev_mode {
            Some(OpusMode::CeltOnly) => {
                let concealed = self
                    .celt
                    .decode_lost(samples_per_channel, self.channels as usize);
                for (dst, src) in out[..needed].iter_mut().zip(concealed.iter()) {
                    *dst = src.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                }
            }
            Some(OpusMode::SilkOnly | OpusMode::Hybrid) => {
                self.silk
                    .decode_lost(samples_per_channel, out, self.loss_count)?;
            }
            _ => {
                self.conceal_with_fade(samples_per_channel, self.loss_count, out);
            }
        }

        self.loss_count = self.loss_count.saturating_add(1);
        self.prev_redundancy = false;
        self.store_last_output(out, samples_per_channel);
        Ok(samples_per_channel)
    }

    /// Decode one Opus packet to interleaved i16 PCM.
    ///
    /// - `packet=None` triggers PLC (packet loss concealment).
    /// - `out` must be large enough for the maximum frame size (120 ms).
    /// - Returns the number of samples per channel written.
    pub(crate) fn decode(
        &mut self,
        packet: Option<&[u8]>,
        out: &mut [i16],
    ) -> Result<usize, Error> {
        let (toc, samples_per_channel_needed) = match packet {
            Some(packet) => {
                let pp = packet::parse_packet(packet)?;
                (Some(pp.toc), pp.samples_per_channel(self.fs_hz))
            }
            None => (None, self.last_packet_duration),
        };

        let needed = samples_per_channel_needed * self.channels as usize;
        if out.len() < needed {
            return Err(Error::OutputTooSmall);
        }

        let Some(toc) = toc else {
            return self.decode_lost_packet(out);
        };

        self.celt.reset_loss_count();
        let config = (toc >> 3) & 0x1f;
        let mode = match config {
            0..=11 => OpusMode::SilkOnly,
            12..=15 => OpusMode::Hybrid,
            16..=31 => OpusMode::CeltOnly,
            _ => unreachable!(),
        };
        // Parse again to get per-frame slices.
        let pp = packet::parse_packet(packet.unwrap())?;

        let transition = self.prev_mode.is_some()
            && ((mode == OpusMode::CeltOnly
                && self.prev_mode != Some(OpusMode::CeltOnly)
                && !self.prev_redundancy)
                || (mode != OpusMode::CeltOnly && self.prev_mode == Some(OpusMode::CeltOnly)));
        let transition_samples = (self.fs_hz as usize) / 200;
        let transition_overlap = (self.fs_hz as usize) / 400;
        let channels = self.channels as usize;
        let celt_transition = if transition && mode == OpusMode::CeltOnly {
            vec![0i16; transition_samples * channels]
        } else {
            Vec::new()
        };
        let mut apply_celt_transition = transition && mode == OpusMode::CeltOnly;
        let mut reset_silk = transition && self.prev_mode == Some(OpusMode::CeltOnly);
        let mut had_redundancy = false;
        let mut written_per_channel = 0usize;
        for &frame in pp.frames().iter() {
            let frame_samples_48k = pp.samples_per_frame_48k;
            let out_frame = &mut out[written_per_channel * self.channels as usize..];
            match mode {
                OpusMode::CeltOnly => {
                    if apply_celt_transition {
                        self.celt.reset();
                    }
                    // CELT frame sizes are specified in 48 kHz samples; this is enough
                    // to drive the CELT-side LM selection.
                    let mut ec = EcDec::new(frame);
                    let celt_frame = self.celt.decode_frame_with_ec(
                        &mut ec,
                        frame_samples_48k,
                        config,
                        pp.packet_channels,
                        out_frame,
                        false,
                    )?;
                    if apply_celt_transition {
                        apply_transition_fade_i16(
                            &celt_transition,
                            &mut out_frame[..celt_frame.samples_per_channel * channels],
                            transition_overlap.min(celt_frame.samples_per_channel / 2),
                            channels,
                            self.celt.window(),
                            self.fs_hz,
                        );
                        apply_celt_transition = false;
                    }
                    written_per_channel += celt_frame.samples_per_channel;
                }
                OpusMode::SilkOnly => {
                    if reset_silk {
                        self.silk.reset();
                        reset_silk = false;
                    }
                    let packet_frame = frame;
                    let silk_frame = self.silk.decode_frame(
                        packet_frame,
                        frame_samples_48k,
                        config,
                        pp.packet_channels,
                        out_frame,
                    )?;
                    if silk_frame.consumed_redundancy {
                        let redundancy_data =
                            &packet_frame[packet_frame.len() - silk_frame.redundancy_bytes..];
                        let redundancy_frame_size_48k = 240usize;
                        let redundancy_samples = (self.fs_hz as usize) / 200;
                        let redundancy_end_band = silk_redundancy_end_band(config);
                        let mut redundancy_out =
                            vec![0i16; redundancy_samples * self.channels as usize];
                        if !silk_frame.celt_to_silk {
                            self.celt.reset();
                        }
                        self.celt.set_start_band(0);
                        self.celt.set_end_band(redundancy_end_band);
                        let redundancy_frame = self.celt.decode_frame(
                            redundancy_data,
                            redundancy_frame_size_48k,
                            config,
                            pp.packet_channels,
                            &mut redundancy_out,
                        )?;
                        self.celt.clear_end_band();
                        if !silk_frame.celt_to_silk {
                            let overlap = (self.fs_hz as usize) / 400;
                            let channels = self.channels as usize;
                            let silk_tail_start =
                                (silk_frame.samples_per_channel - overlap) * channels;
                            let silk_tail_end = silk_frame.samples_per_channel * channels;
                            let redundancy_start = overlap * channels;
                            let redundancy_end = redundancy_start + overlap * channels;
                            let silk_tail = out_frame[silk_tail_start..silk_tail_end].to_vec();
                            smooth_fade_i16(
                                &silk_tail,
                                &redundancy_out[redundancy_start..redundancy_end],
                                &mut out_frame[silk_tail_start..silk_tail_end],
                                overlap,
                                channels,
                                self.celt.window(),
                                self.fs_hz,
                            );
                        } else {
                            let overlap = (self.fs_hz as usize) / 400;
                            let channels = self.channels as usize;
                            let frame_len = silk_frame.samples_per_channel * channels;
                            apply_transition_fade_i16(
                                &redundancy_out,
                                &mut out_frame[..frame_len],
                                overlap.min(redundancy_frame.samples_per_channel / 2),
                                channels,
                                self.celt.window(),
                                self.fs_hz,
                            );
                        }
                    }
                    had_redundancy = silk_frame.consumed_redundancy && !silk_frame.celt_to_silk;
                    written_per_channel += silk_frame.samples_per_channel;
                }
                OpusMode::Hybrid => {
                    let mut ec = EcDec::new(frame);
                    let silk_frame = self.silk.decode_frame_with_ec(
                        frame,
                        &mut ec,
                        frame_samples_48k,
                        config,
                        pp.packet_channels,
                        true,
                        out_frame,
                    )?;
                    let redundancy = if ec.tell() + 17 + 20 <= (frame.len() as i32) * 8 {
                        ec.dec_bit_logp(12)
                    } else {
                        false
                    };
                    let mut celt_to_silk = false;
                    let mut redundancy_bytes = 0usize;
                    if redundancy {
                        celt_to_silk = ec.dec_bit_logp(1);
                        redundancy_bytes = ec.dec_uint(256) as usize + 2;
                        ec.shrink_storage(redundancy_bytes);
                    }
                    let mut celt_to_silk_audio = Vec::new();
                    let mut celt_to_silk_samples = 0usize;
                    let apply_celt_to_silk_audio = redundancy
                        && celt_to_silk
                        && (self.prev_mode != Some(OpusMode::SilkOnly) || self.prev_redundancy);
                    let reset_main_celt = self.prev_mode.is_some()
                        && self.prev_mode != Some(mode)
                        && !self.prev_redundancy;
                    if redundancy && celt_to_silk {
                        let redundancy_samples = (self.fs_hz as usize) / 200;
                        let redundancy_data = &frame[frame.len() - redundancy_bytes..];
                        self.celt.set_start_band(0);
                        celt_to_silk_audio = vec![0i16; redundancy_samples * channels];
                        let redundancy_frame = self.celt.decode_frame(
                            redundancy_data,
                            240,
                            config,
                            pp.packet_channels,
                            &mut celt_to_silk_audio,
                        )?;
                        celt_to_silk_samples = redundancy_frame.samples_per_channel;
                    }
                    if reset_main_celt {
                        self.celt.reset();
                    }
                    self.celt.set_start_band(17);
                    let celt_frame = match self.celt.decode_frame_with_ec(
                        &mut ec,
                        frame_samples_48k,
                        config,
                        pp.packet_channels,
                        out_frame,
                        true,
                    ) {
                        Ok(frame) => frame,
                        Err(err) => {
                            self.celt.set_start_band(0);
                            return Err(err);
                        }
                    };
                    self.celt.set_start_band(0);
                    debug_assert_eq!(
                        silk_frame.samples_per_channel,
                        celt_frame.samples_per_channel
                    );
                    if redundancy && !celt_to_silk {
                        let channels = self.channels as usize;
                        let redundancy_samples = (self.fs_hz as usize) / 200;
                        let overlap = (self.fs_hz as usize) / 400;
                        let frame_len = celt_frame.samples_per_channel * channels;
                        let redundancy_data = &frame[frame.len() - redundancy_bytes..];
                        let mut redundancy_out = vec![0i16; redundancy_samples * channels];
                        self.celt.reset();
                        self.celt.set_start_band(0);
                        let redundancy_frame = self.celt.decode_frame(
                            redundancy_data,
                            240,
                            config,
                            pp.packet_channels,
                            &mut redundancy_out,
                        )?;
                        let fade_len = overlap * channels;
                        let tail_start = frame_len.saturating_sub(fade_len);
                        let tail_end = tail_start + fade_len;
                        let redundancy_start = fade_len;
                        let redundancy_end = redundancy_start + fade_len;
                        if tail_end <= out_frame.len()
                            && redundancy_end <= redundancy_out.len()
                            && redundancy_frame.samples_per_channel == redundancy_samples
                        {
                            let celt_tail = out_frame[tail_start..tail_end].to_vec();
                            smooth_fade_i16(
                                &celt_tail,
                                &redundancy_out[redundancy_start..redundancy_end],
                                &mut out_frame[tail_start..tail_end],
                                overlap,
                                channels,
                                self.celt.window(),
                                self.fs_hz,
                            );
                        }
                    }
                    if apply_celt_to_silk_audio {
                        let overlap = (self.fs_hz as usize) / 400;
                        let frame_len = celt_frame.samples_per_channel * channels;
                        apply_transition_fade_i16(
                            &celt_to_silk_audio,
                            &mut out_frame[..frame_len],
                            overlap.min(celt_to_silk_samples / 2),
                            channels,
                            self.celt.window(),
                            self.fs_hz,
                        );
                    }
                    had_redundancy = redundancy && !celt_to_silk;
                    written_per_channel += silk_frame.samples_per_channel;
                }
            }
        }

        self.prev_mode = Some(mode);
        self.prev_redundancy = had_redundancy;
        self.loss_count = 0;
        self.last_packet_duration = written_per_channel;
        self.store_last_output(out, written_per_channel);
        Ok(written_per_channel)
    }
}

/// Crossfade SILK PCM with redundant CELT PCM using the CELT overlap window.
///
/// Params: previous `in1`, incoming `in2`, mutable `out`, overlap length,
/// interleaved `channels`, CELT `window`, and output sampling rate `fs_hz`.
/// Returns: nothing; `out` is updated in-place.
fn smooth_fade_i16(
    in1: &[i16],
    in2: &[i16],
    out: &mut [i16],
    overlap: usize,
    channels: usize,
    window: &[f32],
    fs_hz: u32,
) {
    let inc = (48_000 / fs_hz) as usize;
    for c in 0..channels {
        for i in 0..overlap {
            let w = window[i * inc] * window[i * inc];
            let idx = i * channels + c;
            let mixed = w * in2[idx] as f32 + (1.0 - w) * in1[idx] as f32;
            out[idx] = mixed.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        }
    }
}

/// Apply the SILK-to-CELT transition prefix and crossfade.
///
/// Params: previous-mode `transition` PCM, mutable decoded `pcm`, fade length
/// `overlap`, interleaved `channels`, CELT `window`, and output rate `fs_hz`.
/// Returns: nothing; `pcm` is updated in-place.
fn apply_transition_fade_i16(
    transition: &[i16],
    pcm: &mut [i16],
    overlap: usize,
    channels: usize,
    window: &[f32],
    fs_hz: u32,
) {
    if overlap == 0 || channels == 0 {
        return;
    }

    let prefix_len = overlap * channels;
    let copy_len = prefix_len.min(transition.len()).min(pcm.len());
    pcm[..copy_len].copy_from_slice(&transition[..copy_len]);

    let fade_available = (transition.len().saturating_sub(prefix_len))
        .min(pcm.len().saturating_sub(prefix_len))
        / channels;
    if fade_available == 0 {
        return;
    }

    let fade_samples = fade_available.min(overlap);
    let fade_len = fade_samples * channels;
    let fade_start = prefix_len;
    let fade_end = fade_start + fade_len;
    let incoming = pcm[fade_start..fade_end].to_vec();
    smooth_fade_i16(
        &transition[fade_start..fade_end],
        &incoming,
        &mut pcm[fade_start..fade_end],
        fade_samples,
        channels,
        window,
        fs_hz,
    );
}

/// Map SILK packet config to CELT redundancy end band.
///
/// Params: Opus TOC `config`.
/// Returns: exclusive CELT end band matching libopus packet bandwidth.
fn silk_redundancy_end_band(config: u8) -> usize {
    match config {
        0..=3 => 13,
        4..=11 => 17,
        12..=13 => 19,
        14..=15 => 21,
        _ => 21,
    }
}
