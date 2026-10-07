//! SILK decoder scaffolding (Phase 3a).
//!
//! This module decodes SILK packet headers, side information, and synthesis
//! state while keeping the implementation close to the reference decoder.

use crate::opus::Error;
use crate::opus::entropy::EcDec;

mod decode_core;
mod entropy_tables;
mod gain;
mod lbrr;
mod lpc;
mod ltp;
mod nlsf;
mod pitch;
mod plc;
mod resampler;
mod resampler_private;
mod resampler_rom;
mod stereo;
mod tables;

const MAX_INTERNAL_FRAMES: usize = 3;
const MAX_LPC_HALVES: usize = 2;
const MAX_API_FRAME_SAMPLES_48K: usize = 960 * MAX_INTERNAL_FRAMES;
const TYPE_VOICED: i32 = 2;

/// Result of decoding one SILK Opus frame.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SilkFrameDecode {
    /// Number of decoded samples per output channel.
    pub samples_per_channel: usize,
    /// Whether a top-level SILK redundancy trailer was consumed.
    pub consumed_redundancy: bool,
    /// Number of trailing redundancy bytes reserved for CELT.
    pub redundancy_bytes: usize,
    /// Transition direction flag from the top-level redundancy trailer.
    pub celt_to_silk: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct ChannelHeader {
    vad_flags: [bool; MAX_INTERNAL_FRAMES],
    lbrr_flags: [bool; MAX_INTERNAL_FRAMES],
}

#[derive(Debug, Clone, Copy, Default)]
struct ParsedHeader {
    channels: [ChannelHeader; 2],
}

/// Stateful SILK decoder shell used by top-level Opus decoder.
#[derive(Debug, Clone)]
pub(crate) struct SilkDecoder {
    fs_hz: u32,
    channels: u8,
    lbrr_state: [lbrr::ChannelState; 2],
    core_state: [decode_core::SilkChannelState; 2],
    resampler_state: [Option<resampler::SilkResampler>; 2],
    stereo_state: stereo::StereoState,
    prev_nlsf_q15: [[i16; tables::MAX_LPC_ORDER]; 2],
    pred_coef_q12: [[[[i16; tables::MAX_LPC_ORDER]; MAX_LPC_HALVES]; MAX_INTERNAL_FRAMES]; 2],
    first_frame_after_reset: [bool; 2],
    prev_decode_only_middle: bool,
    prev_packet_channels: u8,
}

impl SilkDecoder {
    /// Create SILK decoder state.
    ///
    /// Params: `fs_hz` is Opus API output sample rate, `channels` is output channels.
    /// Returns: initialized SILK decoder state with cleared range state.
    pub fn new(fs_hz: u32, channels: u8) -> Self {
        Self {
            fs_hz,
            channels,
            lbrr_state: [lbrr::ChannelState::default(), lbrr::ChannelState::default()],
            core_state: [
                decode_core::SilkChannelState::default(),
                decode_core::SilkChannelState::default(),
            ],
            resampler_state: [None, None],
            stereo_state: stereo::StereoState::default(),
            prev_nlsf_q15: [[0; tables::MAX_LPC_ORDER]; 2],
            pred_coef_q12: [[[[0; tables::MAX_LPC_ORDER]; MAX_LPC_HALVES]; MAX_INTERNAL_FRAMES]; 2],
            first_frame_after_reset: [true; 2],
            prev_decode_only_middle: false,
            prev_packet_channels: 1,
        }
    }

    /// Reset SILK decoder persistent state.
    ///
    /// Params: none.
    /// Returns: nothing.
    pub fn reset(&mut self) {
        self.lbrr_state = [lbrr::ChannelState::default(), lbrr::ChannelState::default()];
        self.core_state = [
            decode_core::SilkChannelState::default(),
            decode_core::SilkChannelState::default(),
        ];
        self.resampler_state = [None, None];
        self.stereo_state = stereo::StereoState::default();
        self.prev_nlsf_q15 = [[0; tables::MAX_LPC_ORDER]; 2];
        self.pred_coef_q12 =
            [[[[0; tables::MAX_LPC_ORDER]; MAX_LPC_HALVES]; MAX_INTERNAL_FRAMES]; 2];
        self.first_frame_after_reset = [true; 2];
        self.prev_decode_only_middle = false;
        self.prev_packet_channels = 1;
    }

    /// Update the per-channel rewhitening history after PLC synthesis.
    ///
    /// Params: mutable channel `state`, concealed internal `frame`, and internal `fs_khz`.
    /// Returns: nothing; the history buffers are refreshed in place.
    fn refresh_conceal_history(
        out_buf: &mut [i16],
        lag_prev: &mut i32,
        pitch_lag: i32,
        frame: &[i16],
        fs_khz: u32,
    ) {
        let ltp_mem_length = fs_khz as usize * tables::LTP_MEM_LENGTH_MS;
        let frame_len = frame.len();
        let mv_len = ltp_mem_length.saturating_sub(frame_len);
        if mv_len > 0 {
            out_buf.copy_within(frame_len..frame_len + mv_len, 0);
        }
        out_buf[mv_len..mv_len + frame_len].copy_from_slice(frame);
        if pitch_lag > 0 {
            *lag_prev = pitch_lag;
        }
    }

    /// Conceal one lost SILK or Hybrid packet using the stored PLC state.
    ///
    /// Params: target `samples_per_channel`, mutable interleaved `out`, and packet `loss_count`.
    /// Returns: concealed sample count per output channel.
    pub(crate) fn decode_lost(
        &mut self,
        samples_per_channel: usize,
        out: &mut [i16],
        loss_count: u32,
    ) -> Result<usize, Error> {
        let packet_channels = self.prev_packet_channels.clamp(1, 2) as usize;
        let channels = self.channels as usize;
        let needed = samples_per_channel * channels;
        if out.len() < needed {
            return Err(Error::OutputTooSmall);
        }

        let plc0 = &self.core_state[0].plc;
        let internal_frame_samples = plc0.subfr_length.saturating_mul(plc0.nb_subfr);
        if internal_frame_samples == 0 || plc0.fs_khz <= 0 {
            out[..needed].fill(0);
            return Ok(samples_per_channel);
        }

        let frame_output_samples = if self.fs_hz == 48_000 {
            self.resampler_state[0]
                .as_ref()
                .map(|resampler| resampler.output_len(internal_frame_samples))
                .unwrap_or(0)
        } else {
            internal_frame_samples
        };
        if frame_output_samples == 0 {
            out[..needed].fill(0);
            return Ok(samples_per_channel);
        }

        out[..needed].fill(0);
        let conceal_frames = samples_per_channel.div_ceil(frame_output_samples);
        let mut mono_resampler_history = self.stereo_state.s_mid;
        let mut written_total = 0usize;

        for _ in 0..conceal_frames {
            let mut internal_pcm = [[0i16; tables::MAX_FRAME_LENGTH]; 2];
            for (ch, frame_pcm) in internal_pcm.iter_mut().enumerate().take(packet_channels) {
                let ch_state = &mut self.core_state[ch];
                let frame_pcm = &mut frame_pcm[..internal_frame_samples];
                let (plc_state, lpc_state) = (&mut ch_state.plc, &mut ch_state.s_lpc_q14_buf[..]);
                plc::plc_conceal(plc_state, lpc_state, frame_pcm, loss_count);
                let fs_khz = plc_state.fs_khz as u32;
                let pitch_lag = plc_state.pitch_lag;
                Self::refresh_conceal_history(
                    &mut ch_state.out_buf,
                    &mut ch_state.lag_prev,
                    pitch_lag,
                    frame_pcm,
                    fs_khz,
                );
            }

            if packet_channels == 1 {
                self.stereo_state.s_mid.copy_from_slice(
                    &internal_pcm[0][internal_frame_samples - 2..internal_frame_samples],
                );
            }

            let frame_write =
                frame_output_samples.min(samples_per_channel.saturating_sub(written_total));
            if self.fs_hz == 48_000 {
                let mut resampled_pcm = [[0i16; MAX_API_FRAME_SAMPLES_48K]; 2];
                for ch in 0..packet_channels {
                    if let Some(resampler) = self.resampler_state[ch].as_mut() {
                        let frame_pcm = &internal_pcm[ch][..internal_frame_samples];
                        let expected = resampler.output_len(internal_frame_samples);
                        let written = if packet_channels == 2 {
                            resampler.process(frame_pcm, &mut resampled_pcm[ch][..expected])
                        } else {
                            let mut resamp_input = vec![0i16; internal_frame_samples + 2];
                            resamp_input[..2].copy_from_slice(&mono_resampler_history);
                            resamp_input[2..2 + internal_frame_samples].copy_from_slice(frame_pcm);
                            let written = resampler.process(
                                &resamp_input[1..1 + internal_frame_samples],
                                &mut resampled_pcm[ch][..expected],
                            );
                            mono_resampler_history.copy_from_slice(
                                &frame_pcm[internal_frame_samples - 2..internal_frame_samples],
                            );
                            written
                        };
                        debug_assert_eq!(written, expected);
                    }
                }

                for n in 0..frame_write {
                    let left = resampled_pcm[0][n];
                    match channels {
                        1 => out[written_total + n] = left,
                        2 => {
                            let base = 2 * (written_total + n);
                            out[base] = left;
                            out[base + 1] = if packet_channels == 2 {
                                resampled_pcm[1][n]
                            } else {
                                left
                            };
                        }
                        _ => unreachable!(),
                    }
                }
            } else {
                let copy_len = internal_frame_samples.min(frame_write);
                for n in 0..copy_len {
                    let left = internal_pcm[0][n];
                    match channels {
                        1 => out[written_total + n] = left,
                        2 => {
                            let base = 2 * (written_total + n);
                            out[base] = left;
                            out[base + 1] = if packet_channels == 2 {
                                internal_pcm[1][n]
                            } else {
                                left
                            };
                        }
                        _ => unreachable!(),
                    }
                }
            }

            written_total += frame_write;
            if written_total >= samples_per_channel {
                break;
            }
        }

        Ok(samples_per_channel)
    }

    /// Decode one SILK Opus frame payload.
    ///
    /// Params: `frame` payload bytes, `frame_samples_48k` frame duration in 48 kHz samples,
    /// `config` TOC config, `packet_channels` coded channels, and `out` interleaved output slice.
    /// Returns: decoded sample count per output channel at the API output rate.
    pub fn decode_frame(
        &mut self,
        frame: &[u8],
        frame_samples_48k: usize,
        config: u8,
        packet_channels: u8,
        out: &mut [i16],
    ) -> Result<SilkFrameDecode, Error> {
        let mut dec = EcDec::new(frame);
        self.decode_frame_with_ec(
            frame,
            &mut dec,
            frame_samples_48k,
            config,
            packet_channels,
            false,
            out,
        )
    }

    /// Decode one SILK Opus frame payload using a shared entropy decoder.
    ///
    /// Params: original `frame`, shared mutable `dec`, `frame_samples_48k`, TOC `config`,
    /// coded `packet_channels`, `is_hybrid` mode flag, and interleaved `out`.
    /// Returns: decoded sample count per output channel at the API output rate.
    pub(crate) fn decode_frame_with_ec(
        &mut self,
        frame: &[u8],
        dec: &mut EcDec<'_>,
        frame_samples_48k: usize,
        config: u8,
        packet_channels: u8,
        is_hybrid: bool,
        out: &mut [i16],
    ) -> Result<SilkFrameDecode, Error> {
        if !matches!(packet_channels, 1 | 2) {
            return Err(Error::BadPacket);
        }

        let internal_fs_hz = silk_internal_fs_hz(config)?;
        let (internal_frames, nb_subfr, frame_length) =
            internal_frame_shape(frame_samples_48k, internal_fs_hz)?;
        let mut mono_resampler_history = self.stereo_state.s_mid;
        let samples_per_channel = (frame_samples_48k * self.fs_hz as usize) / 48_000;
        let needed = samples_per_channel * self.channels as usize;
        if out.len() < needed {
            return Err(Error::OutputTooSmall);
        }

        let mut internal_pcm = [[0i16; MAX_INTERNAL_FRAMES * tables::MAX_FRAME_LENGTH]; 2];
        let mut resampled_pcm = [[0i16; MAX_API_FRAME_SAMPLES_48K]; 2];
        for ch in 0..packet_channels as usize {
            let prev_fs_khz = self.lbrr_state[ch].fs_khz;
            let new_fs_khz = internal_fs_hz / 1000;
            if prev_fs_khz != 0 && prev_fs_khz != new_fs_khz {
                self.core_state[ch] = decode_core::SilkChannelState::default();
                self.first_frame_after_reset[ch] = true;
            }
            lbrr::configure_channel_state(&mut self.lbrr_state[ch], new_fs_khz, nb_subfr);
        }
        if self.fs_hz == 48_000 {
            for ch in 0..packet_channels as usize {
                let needs_reset = self.resampler_state[ch]
                    .as_ref()
                    .is_none_or(|state| !state.matches(internal_fs_hz, self.fs_hz));
                if needs_reset {
                    self.resampler_state[ch] =
                        Some(resampler::SilkResampler::new(internal_fs_hz, self.fs_hz)?);
                }
            }
            if self.channels == 2 && packet_channels == 2 && self.prev_packet_channels == 1 {
                self.stereo_state.pred_prev_q13 = [0; 2];
                self.stereo_state.s_side = [0; 2];
                if let Some(left_resampler) = self.resampler_state[0].clone() {
                    self.resampler_state[1] = Some(left_resampler);
                }
            }
        }
        let parsed = parse_header(dec, packet_channels as usize, internal_frames)?;
        consume_lbrr_payload(
            dec,
            &parsed,
            packet_channels as usize,
            internal_frames,
            &mut self.lbrr_state,
            frame_length,
        )?;
        consume_main_payload(
            dec,
            &parsed,
            packet_channels as usize,
            internal_frames,
            &mut self.lbrr_state,
            frame_length,
            &mut self.prev_nlsf_q15,
            &mut self.pred_coef_q12,
            &mut self.first_frame_after_reset,
            &mut self.core_state,
            &mut internal_pcm,
            &mut self.stereo_state,
            &mut self.prev_decode_only_middle,
        )?;
        let mut consumed_redundancy = false;
        let mut redundancy_bytes = 0usize;
        let mut celt_to_silk = false;
        if !is_hybrid && dec.tell() + 17 <= (8 * frame.len() as i32) {
            celt_to_silk = dec.dec_bit_logp(1);
            redundancy_bytes = frame
                .len()
                .saturating_sub(((dec.tell() + 7).max(0) as usize) >> 3);
            consumed_redundancy = redundancy_bytes > 0;
        }
        out[..needed].fill(0);
        let internal_samples_per_channel = internal_frames * frame_length;
        if self.fs_hz == 48_000 {
            for ch in 0..packet_channels as usize {
                if let Some(resampler) = self.resampler_state[ch].as_mut() {
                    let mut written_total = 0usize;
                    for iframe_idx in 0..internal_frames {
                        let pcm_range = iframe_idx * frame_length..(iframe_idx + 1) * frame_length;
                        let frame_pcm = &internal_pcm[ch][pcm_range];
                        let expected = resampler.output_len(frame_length);
                        let written = if packet_channels == 2 {
                            resampler.process(
                                frame_pcm,
                                &mut resampled_pcm[ch][written_total..written_total + expected],
                            )
                        } else {
                            let mut resamp_input = vec![0i16; frame_length + 2];
                            resamp_input[..2].copy_from_slice(&mono_resampler_history);
                            resamp_input[2..2 + frame_length].copy_from_slice(frame_pcm);
                            let written = resampler.process(
                                &resamp_input[1..1 + frame_length],
                                &mut resampled_pcm[ch][written_total..written_total + expected],
                            );
                            mono_resampler_history
                                .copy_from_slice(&frame_pcm[frame_length - 2..frame_length]);
                            written
                        };
                        debug_assert_eq!(written, expected);
                        written_total += written;
                    }
                    debug_assert_eq!(written_total, samples_per_channel);
                }
            }
        }
        if self.fs_hz == 48_000 {
            for n in 0..samples_per_channel {
                let left = resampled_pcm[0][n];
                match self.channels {
                    1 => out[n] = left,
                    2 => {
                        out[2 * n] = left;
                        out[2 * n + 1] = if packet_channels == 2 {
                            resampled_pcm[1][n]
                        } else {
                            left
                        };
                    }
                    _ => unreachable!(),
                }
            }
        } else {
            for n in 0..internal_samples_per_channel.min(samples_per_channel) {
                let left = internal_pcm[0][n];
                match self.channels {
                    1 => out[n] = left,
                    2 => {
                        out[2 * n] = left;
                        out[2 * n + 1] = if packet_channels == 2 {
                            internal_pcm[1][n]
                        } else {
                            left
                        };
                    }
                    _ => unreachable!(),
                }
            }
        }

        self.prev_packet_channels = packet_channels;
        if dec.is_error() {
            return Err(Error::BadPacket);
        }

        Ok(SilkFrameDecode {
            samples_per_channel,
            consumed_redundancy,
            redundancy_bytes,
            celt_to_silk,
        })
    }
}

/// Parse SILK packet header signaling used before core frame decode.
///
/// Params: `dec` shared entropy decoder, `packet_channels` coded channels, and
/// `internal_frames` count of 20 ms SILK-internal frames (1/2/3).
/// Returns: parsed VAD/LBRR signaling snapshot for the frame.
fn parse_header(
    dec: &mut EcDec<'_>,
    packet_channels: usize,
    internal_frames: usize,
) -> Result<ParsedHeader, Error> {
    let mut parsed = ParsedHeader::default();

    for ch in 0..packet_channels {
        for i in 0..internal_frames {
            parsed.channels[ch].vad_flags[i] = dec.dec_bit_logp(1);
        }

        let has_lbrr = dec.dec_bit_logp(1);
        parsed.channels[ch].lbrr_flags = unpack_lbrr_flags(dec, has_lbrr, internal_frames)?;
    }

    Ok(parsed)
}

/// Consume LBRR payload side information for all flagged internal frames.
///
/// Params: entropy decoder `dec`, parsed `header`, coded channel count,
/// `internal_frames`, mutable per-channel `states`, and internal `frame_length`.
/// Returns: nothing on success, `BadPacket` on malformed entropy payload.
fn consume_lbrr_payload(
    dec: &mut EcDec<'_>,
    header: &ParsedHeader,
    packet_channels: usize,
    internal_frames: usize,
    states: &mut [lbrr::ChannelState; 2],
    frame_length: usize,
) -> Result<(), Error> {
    for i in 0..internal_frames {
        for (ch, state) in states.iter_mut().enumerate().take(packet_channels) {
            if !header.channels[ch].lbrr_flags[i] {
                continue;
            }
            if packet_channels == 2 && ch == 0 {
                let _ = stereo::decode_stereo_pred(dec);
                if !header.channels[1].lbrr_flags[i] {
                    let _ = stereo::decode_mid_only(dec);
                }
            }
            let cond = if i > 0 && header.channels[ch].lbrr_flags[i - 1] {
                lbrr::CondCoding::Conditionally
            } else {
                lbrr::CondCoding::Independently
            };
            let side =
                lbrr::decode_indices(state, dec, header.channels[ch].vad_flags[i], true, cond)?;
            let _ =
                lbrr::decode_pulses(dec, side.signal_type, side.quant_offset_type, frame_length)?;
        }
    }
    Ok(())
}

/// Consume main SILK frame side information and pulses for all internal frames.
///
/// Params: entropy decoder `dec`, parsed packet `header`, coded channel count,
/// `internal_frames`, mutable per-channel `states`, internal `frame_length`,
/// and packet-level `prev_decode_only_middle`.
/// Returns: nothing on success, `BadPacket` on malformed entropy payload.
fn consume_main_payload(
    dec: &mut EcDec<'_>,
    header: &ParsedHeader,
    packet_channels: usize,
    internal_frames: usize,
    states: &mut [lbrr::ChannelState; 2],
    frame_length: usize,
    prev_nlsf_q15: &mut [[i16; tables::MAX_LPC_ORDER]; 2],
    pred_coef_q12: &mut [[[[i16; tables::MAX_LPC_ORDER]; MAX_LPC_HALVES]; MAX_INTERNAL_FRAMES]; 2],
    first_frame_after_reset: &mut [bool; 2],
    core_state: &mut [decode_core::SilkChannelState; 2],
    internal_pcm: &mut [[i16; MAX_INTERNAL_FRAMES * tables::MAX_FRAME_LENGTH]; 2],
    stereo_state: &mut stereo::StereoState,
    prev_decode_only_middle: &mut bool,
) -> Result<(), Error> {
    for i in 0..internal_frames {
        let mut decode_only_middle = false;
        let mut pred_q13 = [0i32; 2];
        let mut decoded_frames: [Option<plc::SilkDecodedFrame>; 2] = [None, None];
        let mut plc_lpc_q12 = [[0i32; tables::MAX_LPC_ORDER]; 2];
        let mut plc_nb_coefs = [0usize; 2];
        if packet_channels == 2 {
            pred_q13 = stereo::decode_stereo_pred(dec);
            decode_only_middle = !header.channels[1].vad_flags[i] && stereo::decode_mid_only(dec);
        }

        for ch in 0..packet_channels {
            if ch == 1 && decode_only_middle {
                continue;
            }
            let cond = if i == 0 {
                if ch > 0 && *prev_decode_only_middle {
                    lbrr::CondCoding::IndependentlyNoLtpScaling
                } else {
                    lbrr::CondCoding::Independently
                }
            } else if ch > 0 && *prev_decode_only_middle {
                lbrr::CondCoding::IndependentlyNoLtpScaling
            } else {
                lbrr::CondCoding::Conditionally
            };
            let side = lbrr::decode_indices(
                &mut states[ch],
                dec,
                header.channels[ch].vad_flags[i],
                false,
                cond,
            )?;
            update_lpc_from_nlsf(
                states[ch].fs_khz,
                &side.nlsf,
                &mut prev_nlsf_q15[ch],
                &mut pred_coef_q12[ch][i],
                &mut first_frame_after_reset[ch],
            );
            let pulses =
                lbrr::decode_pulses(dec, side.signal_type, side.quant_offset_type, frame_length)?;
            let frame_params = decode_core::SilkFrameParams {
                signal_type: side.signal_type,
                quant_offset_type: side.quant_offset_type,
                gain_indices: side.gain_indices,
                lag_index: side.lag_index,
                contour_index: side.contour_index,
                per_index: side.per_index,
                ltp_indices: side.ltp_indices,
                ltp_scale_index: side.ltp_scale_index,
                seed: side.seed,
                pulses,
                conditional: matches!(cond, lbrr::CondCoding::Conditionally),
                frame_length,
                nb_subfr: states[ch].nb_subfr,
                fs_khz: states[ch].fs_khz,
                lpc_order: tables::nlsf_codebook(states[ch].fs_khz).order,
            };
            let pcm_range = i * frame_length..(i + 1) * frame_length;
            decode_core::decode_core(
                &frame_params,
                &pred_coef_q12[ch][i],
                &mut internal_pcm[ch][pcm_range],
                &mut core_state[ch],
            );
            let mut ltp_gains = [0i32; tables::MAX_NB_SUBFR];
            let lag = if frame_params.signal_type == TYPE_VOICED {
                let pitch_lags = pitch::decode_pitch_lags(
                    frame_params.lag_index as i32,
                    frame_params.contour_index as i32,
                    frame_params.fs_khz as i32,
                    frame_params.nb_subfr,
                );
                let (ltp_coeffs_q14, _) = ltp::decode_ltp_coeffs(
                    frame_params.per_index,
                    &frame_params.ltp_indices,
                    frame_params.nb_subfr,
                    frame_params.ltp_scale_index,
                );
                for (subframe_idx, gain) in
                    ltp_gains.iter_mut().enumerate().take(frame_params.nb_subfr)
                {
                    let summed_q14 = ltp_coeffs_q14[subframe_idx]
                        .iter()
                        .map(|coef| i32::from(*coef))
                        .sum::<i32>()
                        .max(0);
                    *gain = summed_q14.clamp(0, 1 << 16);
                }
                pitch_lags[frame_params.nb_subfr - 1]
            } else {
                0
            };
            for (dst, src) in plc_lpc_q12[ch]
                .iter_mut()
                .zip(pred_coef_q12[ch][i][1].iter())
                .take(frame_params.lpc_order)
            {
                *dst = i32::from(*src);
            }
            plc_nb_coefs[ch] = frame_params.lpc_order;
            decoded_frames[ch] = Some(plc::SilkDecodedFrame {
                lag,
                ltp_gains,
                output: Vec::new(),
            });
        }

        if packet_channels == 2 {
            let pcm_range = i * frame_length..(i + 1) * frame_length;
            let (left_channels, right_channels) = internal_pcm.split_at_mut(1);
            stereo::ms_to_lr(
                stereo_state,
                &mut left_channels[0][pcm_range.clone()],
                &mut right_channels[0][pcm_range.clone()],
                pred_q13,
                states[0].fs_khz,
            );
        } else {
            let pcm_range = i * frame_length..(i + 1) * frame_length;
            let frame = &internal_pcm[0][pcm_range.clone()];
            stereo_state
                .s_mid
                .copy_from_slice(&frame[frame_length - 2..frame_length]);
        }

        let pcm_range = i * frame_length..(i + 1) * frame_length;
        for ch in 0..packet_channels {
            let Some(mut decoded_frame) = decoded_frames[ch].take() else {
                continue;
            };
            decoded_frame
                .output
                .extend_from_slice(&internal_pcm[ch][pcm_range.clone()]);
            plc::plc_update(
                &mut core_state[ch].plc,
                &decoded_frame,
                &plc_lpc_q12[ch][..plc_nb_coefs[ch]],
                states[ch].nb_subfr,
                frame_length / states[ch].nb_subfr,
                states[ch].fs_khz as i32,
                plc_nb_coefs[ch],
            );
        }

        *prev_decode_only_middle = decode_only_middle;
    }
    Ok(())
}

/// Decode, optionally interpolate, and convert one NLSF vector to LPC halves.
///
/// Params: internal `fs_khz`, decoded `nlsf_indices`, mutable `prev_nlsf_q15`,
/// mutable `pred_coef_q12`, and per-channel `first_frame_after_reset`.
/// Returns: nothing; previous NLSF and both LPC halves are updated in place.
fn update_lpc_from_nlsf(
    fs_khz: u32,
    nlsf_indices: &lbrr::DecodedNlsfIndices,
    prev_nlsf_q15: &mut [i16; tables::MAX_LPC_ORDER],
    pred_coef_q12: &mut [[i16; tables::MAX_LPC_ORDER]; MAX_LPC_HALVES],
    first_frame_after_reset: &mut bool,
) {
    let codebook = tables::nlsf_codebook(fs_khz);
    let order = codebook.order;
    let mut curr_nlsf_q15 = [0i16; tables::MAX_LPC_ORDER];
    nlsf::nlsf_decode(
        &mut curr_nlsf_q15[..order],
        &nlsf_indices.values[..order + 1],
        codebook,
    );
    lpc::nlsf2a(
        &mut pred_coef_q12[1][..order],
        &curr_nlsf_q15[..order],
        order,
    );

    let interp_coef_q2 = if *first_frame_after_reset {
        4
    } else {
        nlsf_indices.interp_coef_q2
    };
    if interp_coef_q2 < 4 {
        let mut interp_nlsf_q15 = [0i16; tables::MAX_LPC_ORDER];
        for i in 0..order {
            let delta = curr_nlsf_q15[i] as i32 - prev_nlsf_q15[i] as i32;
            interp_nlsf_q15[i] =
                (prev_nlsf_q15[i] as i32 + ((interp_coef_q2 as i32 * delta) >> 2)) as i16;
        }
        lpc::nlsf2a(
            &mut pred_coef_q12[0][..order],
            &interp_nlsf_q15[..order],
            order,
        );
    } else {
        let pred1 = pred_coef_q12[1];
        pred_coef_q12[0][..order].copy_from_slice(&pred1[..order]);
    }

    prev_nlsf_q15[..order].copy_from_slice(&curr_nlsf_q15[..order]);
    *first_frame_after_reset = false;
}

/// Decode and expand LBRR per-internal-frame flags for one coded channel.
///
/// Params: `dec` entropy decoder, `has_lbrr` packet-level LBRR present bit, and
/// `internal_frames` number of SILK-internal frames.
/// Returns: fixed-size LBRR flag array with active entries in `[0..internal_frames)`.
fn unpack_lbrr_flags(
    dec: &mut EcDec<'_>,
    has_lbrr: bool,
    internal_frames: usize,
) -> Result<[bool; MAX_INTERNAL_FRAMES], Error> {
    if !has_lbrr {
        return Ok([false; MAX_INTERNAL_FRAMES]);
    }

    if internal_frames == 1 {
        return Ok([true, false, false]);
    }

    let icdf = tables::lbrr_flags_icdf(internal_frames).ok_or(Error::BadPacket)?;
    let symbol = dec.dec_icdf(icdf, 8) as u32 + 1;
    Ok(unpack_lbrr_symbol(symbol, internal_frames))
}

/// Expand packed LBRR symbol bits to boolean frame flags.
///
/// Params: `symbol` packed bit mask and `internal_frames` active frame count.
/// Returns: fixed-size LBRR flag array with decoded bits in low indices.
fn unpack_lbrr_symbol(symbol: u32, internal_frames: usize) -> [bool; MAX_INTERNAL_FRAMES] {
    let mut out = [false; MAX_INTERNAL_FRAMES];
    for (i, flag) in out.iter_mut().take(internal_frames).enumerate() {
        *flag = ((symbol >> i) & 1) != 0;
    }
    out
}

/// Map Opus TOC config (SILK-only) to SILK internal decode sample rate.
///
/// Params: `config` is TOC config value `(toc >> 3) & 0x1f`.
/// Returns: internal SILK sample rate in Hz (8000/12000/16000).
fn silk_internal_fs_hz(config: u8) -> Result<u32, Error> {
    match config >> 2 {
        0 => Ok(8_000),
        1 => Ok(12_000),
        2 | 3 => Ok(16_000),
        _ => Err(Error::BadPacket),
    }
}

/// Derive SILK internal frame shape from Opus frame duration.
///
/// Params: `frame_samples_48k` Opus frame size at 48 kHz and `internal_fs_hz`.
/// Returns: tuple `(internal_frames, nb_subfr, frame_length_samples)`.
fn internal_frame_shape(
    frame_samples_48k: usize,
    internal_fs_hz: u32,
) -> Result<(usize, usize, usize), Error> {
    let fs_khz = (internal_fs_hz / 1000) as usize;
    match frame_samples_48k {
        480 => Ok((1, 2, 10 * fs_khz)),
        960 => Ok((1, 4, 20 * fs_khz)),
        1_920 => Ok((2, 4, 20 * fs_khz)),
        2_880 => Ok((3, 4, 20 * fs_khz)),
        _ => Err(Error::BadPacket),
    }
}
