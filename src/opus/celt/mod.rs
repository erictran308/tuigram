//! CELT decoder for Opus CELT-only and hybrid packets.
//!
//! The implementation stays structurally close to libopus to preserve
//! conformance while keeping the decode path self-contained in Rust.

use crate::opus::Error;
use crate::opus::entropy::EcDec;

mod bands;
mod cwrs;
mod laplace;
mod mdct;
mod modes;
mod quant_bands;
mod rate;
mod vq;

const NBANDS_48K_20MS: usize = 21;
const LOG_ENERGY_FLOOR_DB: f32 = -28.0;
const BITRES: i32 = 3;
const COMBFILTER_MINPERIOD: usize = 15;
const DECODE_BUFFER_SIZE: usize = 2048;
const COMB_FILTER_GAINS: [[f32; 3]; 3] = [
    [0.306_640_62, 0.217_041_02, 0.129_638_67],
    [0.463_867_2, 0.268_066_4, 0.0],
    [0.799_804_7, 0.100_097_66, 0.0],
];
const TRIM_ICDF: [u8; 11] = [126, 124, 119, 109, 87, 41, 19, 9, 4, 2, 0];
const SPREAD_ICDF: [u8; 4] = [25, 23, 2, 0];
const TAPSET_ICDF: [u8; 3] = [2, 1, 0];
const TF_SELECT_TABLE: [[i8; 8]; 4] = [
    [0, -1, 0, -1, 0, -1, 0, -1],
    [0, -1, 0, -2, 1, 0, 1, -1],
    [0, -2, 0, -3, 2, 0, 1, -1],
    [0, -2, 0, -3, 3, 0, 1, -1],
];

/// Result of decoding one CELT frame.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CeltFrameDecode {
    /// Number of decoded samples per channel.
    pub samples_per_channel: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct CeltDecoder {
    fs_hz: u32,
    channels: u8,
    start_band: usize,
    mode: &'static modes::CeltMode,
    mdct: mdct::MdctBackward,
    mdct_480: mdct::MdctBackward,
    mdct_240: mdct::MdctBackward,
    mdct_short: mdct::MdctBackward,
    decode_mem: Vec<f32>,
    decode_mem_right: Vec<f32>,
    prev_energy: Vec<f32>,
    old_log_energy: Vec<f32>,
    old_log_energy2: Vec<f32>,
    background_log_energy: Vec<f32>,
    deemph_mem: Vec<f32>,
    end_band_override: Option<usize>,
    postfilter_period: i32,
    postfilter_period_old: i32,
    postfilter_gain: f32,
    postfilter_gain_old: f32,
    postfilter_tapset: usize,
    postfilter_tapset_old: usize,
    rng_seed: u32,
    loss_count: u32,
}

impl CeltDecoder {
    /// Create CELT decoder state.
    ///
    /// Params: `fs_hz` is output sample rate and `channels` is output channels.
    /// Returns: initialized CELT decoder state.
    pub fn new(fs_hz: u32, channels: u8) -> Self {
        let mode = modes::mode48000_960_120();
        let state_ch = 2usize;
        Self {
            fs_hz,
            channels,
            start_band: 0,
            mode,
            mdct: mdct::MdctBackward::new(1920, mode.overlap),
            mdct_480: mdct::MdctBackward::new(960, mode.overlap),
            mdct_240: mdct::MdctBackward::new(480, mode.overlap),
            mdct_short: mdct::MdctBackward::new(240, mode.overlap),
            decode_mem: vec![0.0; DECODE_BUFFER_SIZE + mode.overlap],
            decode_mem_right: vec![0.0; DECODE_BUFFER_SIZE + mode.overlap],
            prev_energy: vec![0.0; NBANDS_48K_20MS * state_ch],
            old_log_energy: vec![LOG_ENERGY_FLOOR_DB; NBANDS_48K_20MS * state_ch],
            old_log_energy2: vec![LOG_ENERGY_FLOOR_DB; NBANDS_48K_20MS * state_ch],
            background_log_energy: vec![0.0; NBANDS_48K_20MS * state_ch],
            deemph_mem: vec![0.0; state_ch],
            end_band_override: None,
            postfilter_period: 0,
            postfilter_period_old: 0,
            postfilter_gain: 0.0,
            postfilter_gain_old: 0.0,
            postfilter_tapset: 0,
            postfilter_tapset_old: 0,
            rng_seed: 0,
            loss_count: 0,
        }
    }

    /// Reset decoder state between streams.
    ///
    /// Params: none.
    /// Returns: nothing.
    pub fn reset(&mut self) {
        self.decode_mem.fill(0.0);
        self.decode_mem_right.fill(0.0);
        self.prev_energy.fill(0.0);
        self.old_log_energy.fill(LOG_ENERGY_FLOOR_DB);
        self.old_log_energy2.fill(LOG_ENERGY_FLOOR_DB);
        self.background_log_energy.fill(0.0);
        self.deemph_mem.fill(0.0);
        self.end_band_override = None;
        self.postfilter_period = 0;
        self.postfilter_period_old = 0;
        self.postfilter_gain = 0.0;
        self.postfilter_gain_old = 0.0;
        self.postfilter_tapset = 0;
        self.postfilter_tapset_old = 0;
        self.rng_seed = 0;
        self.start_band = 0;
        self.loss_count = 0;
    }

    /// Clear CELT PLC loss history before a good decode.
    ///
    /// Params: none.
    /// Returns: nothing.
    pub fn reset_loss_count(&mut self) {
        self.loss_count = 0;
    }

    /// Return the CELT output decimation factor for the configured sample rate.
    ///
    /// Params: none.
    /// Returns: integer 48 kHz downsample factor for the public output rate.
    fn downsample_factor(&self) -> Result<usize, Error> {
        match self.fs_hz {
            48_000 => Ok(1),
            24_000 => Ok(2),
            16_000 => Ok(3),
            12_000 => Ok(4),
            8_000 => Ok(6),
            _ => Err(Error::NotImplemented),
        }
    }

    /// Convert a 48 kHz CELT frame size to output samples per channel.
    ///
    /// Params: internal `frame_size_48k`.
    /// Returns: per-channel sample count at the configured public output rate.
    fn output_frame_size(&self, frame_size_48k: usize) -> Result<usize, Error> {
        let downsample = self.downsample_factor()?;
        if !frame_size_48k.is_multiple_of(downsample) {
            return Err(Error::NotImplemented);
        }
        Ok(frame_size_48k / downsample)
    }

    /// Write one deemphasized CELT channel into the interleaved i16 output buffer.
    ///
    /// Params: mutable interleaved `out`, per-channel synthesized `ch_synth`,
    /// channel index `ch`, and additive flag `accum`.
    /// Returns: `Ok(())` on success or `Error::NotImplemented` for unsupported
    /// output rates.
    fn write_output_channel_i16(
        &self,
        out: &mut [i16],
        ch_synth: &[f32],
        ch: usize,
        accum: bool,
    ) -> Result<(), Error> {
        let downsample = self.downsample_factor()?;
        let output_samples = self.output_frame_size(ch_synth.len())?;
        for i in 0..output_samples {
            let sample = Self::float_to_i16(ch_synth[i * downsample]);
            let out_idx = i * self.channels as usize + ch;
            out[out_idx] = if accum {
                out[out_idx].saturating_add(sample)
            } else {
                sample
            };
        }
        Ok(())
    }

    /// Derive the highest active CELT band from saved energy history.
    ///
    /// Params: active output `channels`.
    /// Returns: exclusive upper band index for PLC shaping.
    fn plc_end_band(&self, channels: usize) -> usize {
        let active_channels = channels.clamp(1, 2);
        let nb_ebands = self.mode.nb_ebands;
        for band in (0..nb_ebands).rev() {
            for ch in 0..active_channels {
                if self.prev_energy[ch * nb_ebands + band] > LOG_ENERGY_FLOOR_DB {
                    return band + 1;
                }
            }
        }
        0
    }

    /// Called when a CELT packet is lost (packet=[] or fec path).
    ///
    /// Params: per-channel `frame_size` and output `channels`.
    /// Returns: interleaved concealed floating PCM.
    pub fn decode_lost(&mut self, frame_size: usize, channels: usize) -> Vec<f32> {
        let downsample = match self.downsample_factor() {
            Ok(value) => value,
            Err(_) => return Vec::new(),
        };
        let active_channels = channels.clamp(1, self.channels as usize).min(2);
        let mut pcm = vec![0.0f32; frame_size * active_channels];
        let Some(frame_size_48k) = frame_size.checked_mul(downsample) else {
            return pcm;
        };
        let lm = match frame_size_48k {
            120 => 0,
            240 => 1,
            480 => 2,
            960 => 3,
            _ => return pcm,
        };

        let nb_ebands = self.mode.nb_ebands;
        let end = self.plc_end_band(active_channels);
        if end == 0 {
            self.loss_count = self.loss_count.saturating_add(1);
            return pcm;
        }

        let mut left_energy = self.prev_energy[..nb_ebands].to_vec();
        let mut right_energy = if active_channels == 2 {
            self.prev_energy[nb_ebands..2 * nb_ebands].to_vec()
        } else {
            left_energy.clone()
        };
        if self.loss_count > 0 {
            for band in 0..end {
                left_energy[band] = (left_energy[band] - 6.0).max(self.background_log_energy[band]);
                if active_channels == 2 {
                    right_energy[band] = (right_energy[band] - 6.0)
                        .max(self.background_log_energy[nb_ebands + band]);
                }
            }
        }
        for band in end..nb_ebands {
            left_energy[band] = LOG_ENERGY_FLOOR_DB;
            right_energy[band] = LOG_ENERGY_FLOOR_DB;
        }

        let mut mdct_left = vec![0.0f32; frame_size_48k];
        for band in 0..end {
            let start = (self.mode.e_bands[band] as usize) << lm;
            let band_len =
                (self.mode.e_bands[band + 1] as usize - self.mode.e_bands[band] as usize) << lm;
            for value in &mut mdct_left[start..start + band_len] {
                self.rng_seed = bands::celt_lcg_rand(self.rng_seed);
                *value = ((self.rng_seed >> 16) as i16) as f32;
            }
            vq::renormalise_vector(&mut mdct_left[start..start + band_len], 1.0);
        }
        let mut denorm_left = vec![0.0f32; frame_size_48k];
        bands::denormalise_bands(
            self.mode,
            &mdct_left,
            &mut denorm_left,
            &left_energy,
            0,
            end,
            lm,
            false,
        );
        if Self::synthesise_channel_to_mem(
            &denorm_left,
            frame_size_48k,
            lm,
            false,
            self.mode.window,
            self.mode.overlap,
            &self.mdct,
            &self.mdct_480,
            &self.mdct_240,
            &self.mdct_short,
            &mut self.decode_mem,
        )
        .is_err()
        {
            return pcm;
        }

        if active_channels == 2 {
            let mut mdct_right = vec![0.0f32; frame_size_48k];
            for band in 0..end {
                let start = (self.mode.e_bands[band] as usize) << lm;
                let band_len =
                    (self.mode.e_bands[band + 1] as usize - self.mode.e_bands[band] as usize) << lm;
                for value in &mut mdct_right[start..start + band_len] {
                    self.rng_seed = bands::celt_lcg_rand(self.rng_seed);
                    *value = ((self.rng_seed >> 16) as i16) as f32;
                }
                vq::renormalise_vector(&mut mdct_right[start..start + band_len], 1.0);
            }
            let mut denorm_right = vec![0.0f32; frame_size_48k];
            bands::denormalise_bands(
                self.mode,
                &mdct_right,
                &mut denorm_right,
                &right_energy,
                0,
                end,
                lm,
                false,
            );
            if Self::synthesise_channel_to_mem(
                &denorm_right,
                frame_size_48k,
                lm,
                false,
                self.mode.window,
                self.mode.overlap,
                &self.mdct,
                &self.mdct_480,
                &self.mdct_240,
                &self.mdct_short,
                &mut self.decode_mem_right,
            )
            .is_err()
            {
                return pcm;
            }
        }

        self.postfilter_period = 0;
        self.postfilter_period_old = 0;
        self.postfilter_gain = 0.0;
        self.postfilter_gain_old = 0.0;
        self.postfilter_tapset = 0;
        self.postfilter_tapset_old = 0;
        self.old_log_energy2[..nb_ebands].copy_from_slice(&self.old_log_energy[..nb_ebands]);
        self.old_log_energy[..nb_ebands].copy_from_slice(&left_energy);
        self.prev_energy[..nb_ebands].copy_from_slice(&left_energy);
        self.old_log_energy2[nb_ebands..2 * nb_ebands]
            .copy_from_slice(&self.old_log_energy[nb_ebands..2 * nb_ebands]);
        if active_channels == 2 {
            self.old_log_energy[nb_ebands..2 * nb_ebands].copy_from_slice(&right_energy);
            self.prev_energy[nb_ebands..2 * nb_ebands].copy_from_slice(&right_energy);
        } else {
            self.old_log_energy[nb_ebands..2 * nb_ebands].copy_from_slice(&left_energy);
            self.prev_energy[nb_ebands..2 * nb_ebands].copy_from_slice(&left_energy);
        }

        let out_start = Self::out_start(frame_size_48k);
        let mut left = self.decode_mem[out_start..out_start + frame_size_48k].to_vec();
        self.apply_deemph(0, &mut left);
        for i in 0..frame_size {
            pcm[i * active_channels] = left[i * downsample];
        }
        if active_channels == 2 {
            let mut right = self.decode_mem_right[out_start..out_start + frame_size_48k].to_vec();
            self.apply_deemph(1, &mut right);
            for i in 0..frame_size {
                pcm[i * 2 + 1] = right[i * downsample];
            }
        }

        self.loss_count = self.loss_count.saturating_add(1);
        pcm
    }

    /// Return the MDCT overlap window used for transition fades.
    ///
    /// Params: none.
    /// Returns: immutable CELT synthesis window coefficients.
    pub(crate) fn window(&self) -> &[f32] {
        self.mode.window
    }

    /// Override the lowest coded band for the next CELT decode.
    ///
    /// Params: `band` lower band index to decode from.
    /// Returns: nothing; the index is clamped to the valid mode range.
    pub fn set_start_band(&mut self, band: usize) {
        self.start_band = band.min(self.mode.nb_ebands.saturating_sub(1));
    }

    /// Override the highest coded band for the next CELT decode.
    ///
    /// Params: `band` exclusive upper band index.
    /// Returns: nothing; the index is clamped to the valid mode range.
    pub fn set_end_band(&mut self, band: usize) {
        self.end_band_override = Some(band.clamp(1, self.mode.nb_ebands));
    }

    /// Clear any explicit CELT end-band override.
    ///
    /// Params: none.
    /// Returns: nothing; future decodes use config-derived band end.
    pub fn clear_end_band(&mut self) {
        self.end_band_override = None;
    }

    /// Apply first-order deemphasis to one channel buffer in-place.
    ///
    /// Params: `ch` is channel index, `samples` are per-channel samples.
    /// Returns: nothing.
    fn apply_deemph(&mut self, ch: usize, samples: &mut [f32]) {
        let coef = self.mode.preemph[0];
        let mut mem = self.deemph_mem[ch];
        for x in samples.iter_mut() {
            // libopus recurrence:
            // tmp = x + m; m = coef * tmp; output = tmp
            let tmp = *x + mem;
            mem = coef * tmp;
            *x = tmp;
        }
        self.deemph_mem[ch] = mem;
    }

    /// Apply CELT pitch comb postfilter in-place on decoder memory.
    ///
    /// Params: `buf` is decode memory, `start` is frame start in `buf`, `t0`/`t1`
    /// are old/new periods, `n` is frame segment length, `g0`/`g1` are old/new
    /// gains, `tapset0`/`tapset1` select tap weights, `window` is overlap window,
    /// `overlap` is overlap length.
    /// Returns: nothing.
    fn comb_filter_in_place(
        buf: &mut [f32],
        start: usize,
        t0: usize,
        t1: usize,
        n: usize,
        g0: f32,
        g1: f32,
        tapset0: usize,
        tapset1: usize,
        window: &[f32],
        overlap: usize,
    ) {
        if n == 0 || (g0 == 0.0 && g1 == 0.0) {
            return;
        }
        let period0 = t0.max(COMBFILTER_MINPERIOD);
        let period1 = t1.max(COMBFILTER_MINPERIOD);
        let ts0 = tapset0.min(2);
        let ts1 = tapset1.min(2);
        let g00 = g0 * COMB_FILTER_GAINS[ts0][0];
        let g01 = g0 * COMB_FILTER_GAINS[ts0][1];
        let g02 = g0 * COMB_FILTER_GAINS[ts0][2];
        let g10 = g1 * COMB_FILTER_GAINS[ts1][0];
        let g11 = g1 * COMB_FILTER_GAINS[ts1][1];
        let g12 = g1 * COMB_FILTER_GAINS[ts1][2];
        let mut overlap_len = overlap.min(n);
        if g0 == g1 && period0 == period1 && ts0 == ts1 {
            overlap_len = 0;
        }

        let mut x1 = buf[start - period1 + 1];
        let mut x2 = buf[start - period1];
        let mut x3 = buf[start - period1 - 1];
        let mut x4 = buf[start - period1 - 2];

        for i in 0..overlap_len {
            let x0 = buf[start + i - period1 + 2];
            let f = window[i] * window[i];
            let one_minus_f = 1.0 - f;
            let mut y = buf[start + i];
            y += (one_minus_f * g00) * buf[start + i - period0];
            y +=
                (one_minus_f * g01) * (buf[start + i - period0 + 1] + buf[start + i - period0 - 1]);
            y +=
                (one_minus_f * g02) * (buf[start + i - period0 + 2] + buf[start + i - period0 - 2]);
            y += (f * g10) * x2;
            y += (f * g11) * (x1 + x3);
            y += (f * g12) * (x0 + x4);
            buf[start + i] = y;
            x4 = x3;
            x3 = x2;
            x2 = x1;
            x1 = x0;
        }

        if g1 == 0.0 {
            return;
        }
        for i in overlap_len..n {
            let x0 = buf[start + i - period1 + 2];
            let y = buf[start + i] + g10 * x2 + g11 * (x1 + x3) + g12 * (x0 + x4);
            buf[start + i] = y;
            x4 = x3;
            x3 = x2;
            x2 = x1;
            x1 = x0;
        }
    }

    /// Convert one floating sample to i16 with saturation.
    ///
    /// Params: `x` is floating PCM sample.
    /// Returns: saturated i16 PCM.
    fn float_to_i16(x: f32) -> i16 {
        let v = x.round().clamp(i16::MIN as f32, i16::MAX as f32);
        v as i16
    }

    /// Return frame output start offset inside `decode_mem`.
    ///
    /// Params: `frame_size_48k` is frame size in 48 kHz samples.
    /// Returns: start index for current synthesis output in `decode_mem`.
    fn out_start(frame_size_48k: usize) -> usize {
        DECODE_BUFFER_SIZE - frame_size_48k
    }

    /// Run CELT synthesis for one channel into the provided decode-state buffer.
    ///
    /// Params: denormalized spectrum, frame shape context and mutable channel state.
    /// Returns: `Ok(())` on success, `Error::NotImplemented` for unsupported sizes.
    fn synthesise_channel_to_mem(
        denorm: &[f32],
        frame_size_48k: usize,
        lm: usize,
        is_transient: bool,
        window: &[f32],
        overlap: usize,
        mdct: &mdct::MdctBackward,
        mdct_480: &mdct::MdctBackward,
        mdct_240: &mdct::MdctBackward,
        mdct_short: &mdct::MdctBackward,
        decode_mem: &mut [f32],
    ) -> Result<(), Error> {
        let decode_len = DECODE_BUFFER_SIZE + overlap;
        decode_mem.copy_within(frame_size_48k..decode_len, 0);
        let out_start = Self::out_start(frame_size_48k);
        if is_transient {
            let m_blocks = 1usize << lm;
            let short_len = frame_size_48k / m_blocks;
            let mut short_coeffs = vec![0.0f32; short_len];
            for b in 0..m_blocks {
                let out_offset = out_start + b * short_len;
                for j in 0..short_len {
                    short_coeffs[j] = denorm[j * m_blocks + b];
                }
                mdct_short
                    .backward(
                        &short_coeffs,
                        window,
                        &mut decode_mem[out_offset..out_offset + short_len + overlap],
                    )
                    .map_err(|_| Error::NotImplemented)?;
            }
        } else {
            let mdct_impl = match frame_size_48k {
                120 => mdct_short,
                240 => mdct_240,
                480 => mdct_480,
                960 => mdct,
                _ => return Err(Error::NotImplemented),
            };
            mdct_impl
                .backward(
                    denorm,
                    window,
                    &mut decode_mem[out_start..out_start + frame_size_48k + overlap],
                )
                .map_err(|_| Error::NotImplemented)?;
        }
        Ok(())
    }

    /// Decode a single CELT frame.
    ///
    /// `frame_size_48k` is samples-per-channel at 48 kHz (120/240/480/960).
    pub fn decode_frame(
        &mut self,
        frame: &[u8],
        frame_size_48k: usize,
        config: u8,
        packet_channels: u8,
        out: &mut [i16],
    ) -> Result<CeltFrameDecode, Error> {
        let mut ec = EcDec::new(frame);
        self.decode_frame_with_ec(&mut ec, frame_size_48k, config, packet_channels, out, false)
    }

    /// Decode a single CELT frame using a shared entropy decoder.
    ///
    /// Params: shared mutable `ec`, `frame_size_48k`, TOC `config`,
    /// coded `packet_channels`, mutable `out`, and `accum`
    /// which adds decoded PCM on top of existing samples for hybrid mode.
    /// Returns: decoded CELT frame metadata.
    pub(crate) fn decode_frame_with_ec(
        &mut self,
        ec: &mut EcDec<'_>,
        frame_size_48k: usize,
        config: u8,
        packet_channels: u8,
        out: &mut [i16],
        accum: bool,
    ) -> Result<CeltFrameDecode, Error> {
        let output_samples = self.output_frame_size(frame_size_48k)?;
        if !matches!(self.channels, 1 | 2) {
            return Err(Error::NotImplemented);
        }
        if !matches!(frame_size_48k, 120 | 240 | 480 | 960) {
            return Err(Error::NotImplemented);
        }
        let needed = output_samples * self.channels as usize;
        if out.len() < needed {
            return Err(Error::OutputTooSmall);
        }

        let coded_channels = packet_channels.clamp(1, 2) as usize;
        if coded_channels == 1 {
            let nb = self.mode.nb_ebands;
            for i in 0..nb {
                self.prev_energy[i] = self.prev_energy[i].max(self.prev_energy[nb + i]);
            }
        }
        let start = self.start_band.min(self.mode.nb_ebands.saturating_sub(1));
        let end = self
            .end_band_override
            .unwrap_or_else(|| bandwidth_end(config));
        let active_len = ec.storage();
        let total_bits = (active_len * 8) as i32;
        let mut tell = ec.tell();
        let silence_flag = if tell >= total_bits {
            true
        } else if tell == 1 {
            ec.dec_bit_logp(15)
        } else {
            false
        };
        if silence_flag {
            tell = total_bits;
        }

        let mut postfilter_pitch = 0i32;
        let mut postfilter_tapset = 0i32;
        let mut postfilter_gain = 0.0f32;
        if start == 0 && tell + 16 <= total_bits && ec.dec_bit_logp(1) {
            let octave = ec.dec_uint(6) as i32;
            postfilter_pitch = ((16 << octave) + ec.dec_bits((4 + octave) as u32) as i32) - 1;
            let postfilter_qg = ec.dec_bits(3) as i32;
            if ec.tell() + 2 <= total_bits {
                postfilter_tapset = ec.dec_icdf(&TAPSET_ICDF, 2);
            }
            postfilter_gain = 0.09375 * (postfilter_qg + 1) as f32;
        }
        tell = ec.tell();
        let lm = match frame_size_48k {
            120 => 0,
            240 => 1,
            480 => 2,
            960 => 3,
            _ => unreachable!(),
        };
        let is_transient = if lm > 0 && tell + 3 <= total_bits {
            ec.dec_bit_logp(3)
        } else {
            false
        };
        tell = ec.tell();
        let intra_ener = tell + 3 <= total_bits && ec.dec_bit_logp(3);

        quant_bands::unquant_coarse_energy(
            self.mode,
            start,
            end,
            &mut self.prev_energy,
            intra_ener,
            ec,
            coded_channels,
            lm,
            total_bits,
        );
        let mut tf_res = vec![0i32; self.mode.nb_ebands];
        tf_decode(start, end, is_transient, &mut tf_res, lm, total_bits, ec);
        let spread_decision = if ec.tell() + 4 <= total_bits {
            ec.dec_icdf(&SPREAD_ICDF, 5)
        } else {
            0
        };
        let cap = rate::init_caps(self.mode, lm, coded_channels);
        let mut offsets = vec![0i32; self.mode.nb_ebands];
        let mut dynalloc_logp = 6i32;
        let mut total_bits_q = total_bits << BITRES;
        tell = ec.tell_frac() as i32;
        for i in start..end {
            let width = (coded_channels as i32)
                * (((self.mode.e_bands[i + 1] - self.mode.e_bands[i]) as i32) << lm);
            let quanta = ((width << BITRES).min((6 << BITRES).max(width))).max(0);
            let mut dynalloc_loop_logp = dynalloc_logp;
            let mut boost = 0i32;
            while tell + (dynalloc_loop_logp << BITRES) < total_bits_q && boost < cap[i] {
                let flag = ec.dec_bit_logp(dynalloc_loop_logp as u32);
                tell = ec.tell_frac() as i32;
                if !flag {
                    break;
                }
                boost += quanta;
                total_bits_q -= quanta;
                dynalloc_loop_logp = 1;
            }
            offsets[i] = boost;
            if boost > 0 {
                dynalloc_logp = (dynalloc_logp - 1).max(2);
            }
        }
        let alloc_trim = if tell + (6 << BITRES) <= total_bits_q {
            ec.dec_icdf(&TRIM_ICDF, 7)
        } else {
            5
        };
        let mut bits = ((active_len as i32 * 8) << BITRES) - ec.tell_frac() as i32 - 1;
        let anti_collapse_rsv = if is_transient && lm >= 2 && bits >= ((lm as i32 + 2) << BITRES) {
            1 << BITRES
        } else {
            0
        };
        bits -= anti_collapse_rsv;
        let alloc = rate::clt_compute_allocation(
            self.mode,
            start,
            end,
            &offsets,
            &cap,
            alloc_trim,
            bits,
            coded_channels,
            lm,
            ec,
        );
        quant_bands::unquant_fine_energy(
            self.mode,
            start,
            end,
            &mut self.prev_energy,
            &alloc.fine_quant,
            ec,
            coded_channels,
        );
        let mut mdct_in = vec![0.0f32; frame_size_48k];
        let mut mdct_side = if coded_channels == 2 {
            Some(vec![0.0f32; frame_size_48k])
        } else {
            None
        };
        let collapse_masks = if let Some(ref mut side) = mdct_side {
            bands::quant_all_bands_stereo(
                self.mode,
                start,
                end,
                &mut mdct_in,
                side,
                &alloc.pulses,
                is_transient,
                spread_decision,
                &tf_res,
                (active_len as i32 * 8 << BITRES) - anti_collapse_rsv,
                alloc.balance,
                alloc.coded_bands,
                lm,
                ec,
                &mut self.rng_seed,
                alloc.dual_stereo,
                alloc.intensity,
                coded_channels == 2 && self.channels == 1,
            )
        } else {
            bands::quant_all_bands_mono(
                self.mode,
                start,
                end,
                &mut mdct_in,
                &alloc.pulses,
                is_transient,
                spread_decision,
                &tf_res,
                (active_len as i32 * 8 << BITRES) - anti_collapse_rsv,
                alloc.balance,
                alloc.coded_bands,
                lm,
                ec,
                &mut self.rng_seed,
            )
        };
        // Anti-collapse bit (consumed from range coder for transient frames).
        let mut anti_collapse_on = 0u32;
        if anti_collapse_rsv > 0 {
            anti_collapse_on = ec.dec_bits(1);
        }
        quant_bands::unquant_energy_finalise(
            self.mode,
            start,
            end,
            &mut self.prev_energy,
            &alloc.fine_quant,
            &alloc.fine_priority,
            active_len as i32 * 8 - ec.tell(),
            ec,
            coded_channels,
        );
        if anti_collapse_on != 0 {
            if let Some(side) = mdct_side.as_mut() {
                bands::anti_collapse(
                    self.mode,
                    &mut mdct_in,
                    Some(side),
                    &collapse_masks,
                    lm,
                    coded_channels,
                    start,
                    end,
                    &self.prev_energy,
                    &self.old_log_energy,
                    &self.old_log_energy2,
                    &alloc.pulses,
                    self.rng_seed,
                );
            } else {
                bands::anti_collapse(
                    self.mode,
                    &mut mdct_in,
                    None,
                    &collapse_masks,
                    lm,
                    coded_channels,
                    start,
                    end,
                    &self.prev_energy,
                    &self.old_log_energy,
                    &self.old_log_energy2,
                    &alloc.pulses,
                    self.rng_seed,
                );
            }
        }
        if silence_flag {
            let state_channels = coded_channels.min(2);
            for c in 0..state_channels {
                let base = c * self.mode.nb_ebands;
                for i in 0..self.mode.nb_ebands {
                    self.prev_energy[base + i] = LOG_ENERGY_FLOOR_DB;
                }
            }
        }
        let mut denorm = vec![0.0f32; frame_size_48k];
        bands::denormalise_bands(
            self.mode,
            &mdct_in,
            &mut denorm,
            &self.prev_energy[..self.mode.nb_ebands],
            start,
            end,
            lm,
            silence_flag,
        );
        let mut denorm_right_for_stereo: Option<Vec<f32>> = None;
        if coded_channels == 2 {
            if let Some(side) = mdct_side.as_ref() {
                let mut denorm_side = vec![0.0f32; frame_size_48k];
                bands::denormalise_bands(
                    self.mode,
                    side,
                    &mut denorm_side,
                    &self.prev_energy[self.mode.nb_ebands..self.mode.nb_ebands * 2],
                    start,
                    end,
                    lm,
                    silence_flag,
                );
                if self.channels == 1 {
                    for i in 0..frame_size_48k {
                        denorm[i] = 0.5 * (denorm[i] + denorm_side[i]);
                    }
                } else if self.channels == 2 {
                    denorm_right_for_stereo = Some(denorm_side);
                }
            }
        } else if self.channels == 2 {
            // Mono-coded packet routed to stereo output: identical spectrum on both channels.
            denorm_right_for_stereo = Some(denorm.clone());
        }

        // Match libopus buffer flow: shift history before new synthesis.
        let decode_len = DECODE_BUFFER_SIZE + self.mode.overlap;
        self.decode_mem.copy_within(frame_size_48k..decode_len, 0);
        let out_start = Self::out_start(frame_size_48k);

        if is_transient {
            // Transient CELT frames are composed of 120-sample short blocks.
            let m_blocks = 1usize << lm;
            let short_len = frame_size_48k / m_blocks;
            let mut short_coeffs = vec![0.0f32; short_len];

            for b in 0..m_blocks {
                let out_offset = out_start + b * short_len;
                for j in 0..short_len {
                    short_coeffs[j] = denorm[j * m_blocks + b];
                }
                self.mdct_short
                    .backward(
                        &short_coeffs,
                        self.mode.window,
                        &mut self.decode_mem
                            [out_offset..out_offset + short_len + self.mode.overlap],
                    )
                    .map_err(|_| Error::NotImplemented)?;
            }
        } else {
            // Long block path for all supported frame sizes.
            let mdct_impl = match frame_size_48k {
                120 => &self.mdct_short,
                240 => &self.mdct_240,
                480 => &self.mdct_480,
                960 => &self.mdct,
                _ => return Err(Error::NotImplemented),
            };
            mdct_impl
                .backward(
                    &denorm,
                    self.mode.window,
                    &mut self.decode_mem[out_start..out_start + frame_size_48k + self.mode.overlap],
                )
                .map_err(|_| Error::NotImplemented)?;
        }
        if self.channels == 2 {
            let window = self.mode.window;
            let overlap = self.mode.overlap;
            if let Some(denorm_right) = denorm_right_for_stereo.as_deref() {
                Self::synthesise_channel_to_mem(
                    denorm_right,
                    frame_size_48k,
                    lm,
                    is_transient,
                    window,
                    overlap,
                    &self.mdct,
                    &self.mdct_480,
                    &self.mdct_240,
                    &self.mdct_short,
                    &mut self.decode_mem_right,
                )?;
            } else {
                Self::synthesise_channel_to_mem(
                    &denorm,
                    frame_size_48k,
                    lm,
                    is_transient,
                    window,
                    overlap,
                    &self.mdct,
                    &self.mdct_480,
                    &self.mdct_240,
                    &self.mdct_short,
                    &mut self.decode_mem_right,
                )?;
            }
        }

        let pf_period = self.postfilter_period.max(COMBFILTER_MINPERIOD as i32) as usize;
        let pf_period_old = self.postfilter_period_old.max(COMBFILTER_MINPERIOD as i32) as usize;
        let decoded_tapset = postfilter_tapset.clamp(0, 2) as usize;
        let short_n = self.mode.short_mdct_size.min(frame_size_48k);
        Self::comb_filter_in_place(
            &mut self.decode_mem,
            out_start,
            pf_period_old,
            pf_period,
            short_n,
            self.postfilter_gain_old,
            self.postfilter_gain,
            self.postfilter_tapset_old,
            self.postfilter_tapset,
            self.mode.window,
            self.mode.overlap,
        );
        if lm != 0 && frame_size_48k > short_n {
            Self::comb_filter_in_place(
                &mut self.decode_mem,
                out_start + short_n,
                pf_period,
                postfilter_pitch.max(0) as usize,
                frame_size_48k - short_n,
                self.postfilter_gain,
                postfilter_gain,
                self.postfilter_tapset,
                decoded_tapset,
                self.mode.window,
                self.mode.overlap,
            );
        }
        if self.channels == 2 {
            Self::comb_filter_in_place(
                &mut self.decode_mem_right,
                out_start,
                pf_period_old,
                pf_period,
                short_n,
                self.postfilter_gain_old,
                self.postfilter_gain,
                self.postfilter_tapset_old,
                self.postfilter_tapset,
                self.mode.window,
                self.mode.overlap,
            );
            if lm != 0 && frame_size_48k > short_n {
                Self::comb_filter_in_place(
                    &mut self.decode_mem_right,
                    out_start + short_n,
                    pf_period,
                    postfilter_pitch.max(0) as usize,
                    frame_size_48k - short_n,
                    self.postfilter_gain,
                    postfilter_gain,
                    self.postfilter_tapset,
                    decoded_tapset,
                    self.mode.window,
                    self.mode.overlap,
                );
            }
        }
        self.postfilter_period_old = self.postfilter_period;
        self.postfilter_gain_old = self.postfilter_gain;
        self.postfilter_tapset_old = self.postfilter_tapset;
        self.postfilter_period = postfilter_pitch;
        self.postfilter_gain = postfilter_gain;
        self.postfilter_tapset = decoded_tapset;
        if lm != 0 {
            self.postfilter_period_old = self.postfilter_period;
            self.postfilter_gain_old = self.postfilter_gain;
            self.postfilter_tapset_old = self.postfilter_tapset;
        }

        let nb_ebands = self.mode.nb_ebands;
        let state_len = 2 * nb_ebands;
        if coded_channels == 1 {
            let (left, right) = self.prev_energy.split_at_mut(nb_ebands);
            right.copy_from_slice(left);
        }
        if !is_transient {
            self.old_log_energy2[..state_len].copy_from_slice(&self.old_log_energy[..state_len]);
            self.old_log_energy[..state_len].copy_from_slice(&self.prev_energy[..state_len]);
        } else {
            for i in 0..state_len {
                self.old_log_energy[i] = self.old_log_energy[i].min(self.prev_energy[i]);
            }
        }
        let m = 1usize << lm;
        let max_background_increase = (m.min(160) as f32) * 0.001;
        for i in 0..state_len {
            self.background_log_energy[i] =
                (self.background_log_energy[i] + max_background_increase).min(self.prev_energy[i]);
        }
        for c in 0..2usize {
            let base = c * nb_ebands;
            for i in 0..start.min(nb_ebands) {
                self.prev_energy[base + i] = 0.0;
                self.old_log_energy[base + i] = LOG_ENERGY_FLOOR_DB;
                self.old_log_energy2[base + i] = LOG_ENERGY_FLOOR_DB;
            }
            for i in end.min(nb_ebands)..nb_ebands {
                self.prev_energy[base + i] = 0.0;
                self.old_log_energy[base + i] = LOG_ENERGY_FLOOR_DB;
                self.old_log_energy2[base + i] = LOG_ENERGY_FLOOR_DB;
            }
        }

        for ch in 0..self.channels as usize {
            let decode_mem_ch = if ch == 0 {
                &self.decode_mem
            } else {
                &self.decode_mem_right
            };
            let mut ch_synth = decode_mem_ch[out_start..out_start + frame_size_48k].to_vec();
            self.apply_deemph(ch, &mut ch_synth);
            self.write_output_channel_i16(out, &ch_synth, ch, accum)?;
        }

        self.rng_seed = ec.rng();
        Ok(CeltFrameDecode {
            samples_per_channel: output_samples,
        })
    }
}

/// Map CELT or Hybrid Opus config to active end band.
///
/// Params: TOC config value `(toc >> 3) & 0x1f`.
/// Returns: exclusive end band index.
fn bandwidth_end(config: u8) -> usize {
    match config {
        12 | 13 => 19,
        14 | 15 => 21,
        16..=19 => 13,
        20..=23 => 17,
        24..=27 => 19,
        28..=31 => 21,
        _ => 21,
    }
}

/// Decode CELT TF flags for active bands.
///
/// Params: band range, transient flag, mutable tf array, LM, frame bit budget and range decoder.
/// Returns: nothing; `tf_res` updated in-place.
fn tf_decode(
    start: usize,
    end: usize,
    is_transient: bool,
    tf_res: &mut [i32],
    lm: usize,
    total_bits: i32,
    dec: &mut EcDec<'_>,
) {
    let mut budget = total_bits.max(0);
    let mut tell = dec.tell();
    let mut logp = if is_transient { 2 } else { 4 };
    let tf_select_rsv = lm > 0 && tell + logp + 1 <= budget;
    if tf_select_rsv {
        budget = budget.saturating_sub(1);
    }
    let mut tf_changed = 0i32;
    let mut curr = 0i32;
    for i in start..end {
        if tell + logp <= budget {
            curr ^= i32::from(dec.dec_bit_logp(logp as u32));
            tell = dec.tell();
            tf_changed |= curr;
        }
        tf_res[i] = curr;
        logp = if is_transient { 4 } else { 5 };
    }
    let mut tf_select = 0i32;
    let idx0 = 4 * usize::from(is_transient) + tf_changed as usize;
    let idx1 = 4 * usize::from(is_transient) + 2 + tf_changed as usize;
    if tf_select_rsv && TF_SELECT_TABLE[lm][idx0] != TF_SELECT_TABLE[lm][idx1] {
        tf_select = i32::from(dec.dec_bit_logp(1));
    }
    for t in tf_res.iter_mut().take(end).skip(start) {
        let idx = 4 * usize::from(is_transient) + 2 * tf_select as usize + *t as usize;
        *t = TF_SELECT_TABLE[lm][idx] as i32;
    }
}
