//! CELT inverse MDCT primitives.
//!
//! Current scope is a decoder-oriented path for 48 kHz / 20 ms (`LM=3`), with
//! explicit overlap/window processing kept close to libopus flow.

use core::f32::consts::PI;

/// Backward MDCT helper for CELT decode path.
#[derive(Debug, Clone)]
pub(crate) struct MdctBackward {
    n: usize,
    overlap: usize,
    trig: Vec<f32>,
}

impl MdctBackward {
    /// Create backward MDCT state for fixed frame size.
    ///
    /// Params: `n` is MDCT size (e.g. 960), `overlap` is overlap length
    /// (e.g. 120).
    /// Returns: initialized state with runtime trig tables.
    pub fn new(n: usize, overlap: usize) -> Self {
        let n2 = n / 2;
        let mut trig = Vec::with_capacity(n2);
        for j in 0..n2 {
            let phase = 2.0_f64 * core::f64::consts::PI * (j as f64 + 0.125_f64) / n as f64;
            trig.push(phase.cos() as f32);
        }
        Self { n, overlap, trig }
    }

    /// Run `clt_mdct_backward`-style transform.
    ///
    /// Params: `input` contains `N/2` MDCT bins, `window` contains `overlap`
    /// samples, `out` has length at least `N/2 + overlap`.
    /// Returns: `Ok(())` on success, otherwise a static validation error.
    pub fn backward(
        &self,
        input: &[f32],
        window: &[f32],
        out: &mut [f32],
    ) -> Result<(), &'static str> {
        let n = self.n;
        let n2 = n >> 1;
        let n4 = n >> 2;
        let overlap = self.overlap;
        let required = n2 + overlap;
        if input.len() != n2 || out.len() < required {
            return Err("mdct backward length mismatch");
        }
        if window.len() != overlap {
            return Err("mdct backward window length mismatch");
        }

        // Flat interleaved buffer: [re0, im0, re1, im1, ...]
        let mut f2 = vec![0.0f32; n2];
        let t = &self.trig;
        for i in 0..n4 {
            let xp1 = input[2 * i];
            let xp2 = input[n2 - 1 - 2 * i];
            // Match libopus clt_mdct_backward pre-rotation for float path.
            let yr = xp2 * t[i] + xp1 * t[n4 + i];
            let yi = xp1 * t[i] - xp2 * t[n4 + i];
            // Swap slots like libopus FFT-vs-IFFT trick.
            f2[2 * i] = yi;
            f2[2 * i + 1] = yr;
        }

        let mut f2_out = vec![0.0f32; n2];
        flat_fft_forward(&f2, &mut f2_out, n4);

        let ov2 = overlap >> 1;
        out[ov2..ov2 + n2].copy_from_slice(&f2_out[..n2]);
        let mut p0 = ov2;
        let mut p1 = ov2 + n2 - 2;
        for i in 0..((n4 + 1) >> 1) {
            // We swap real/imag in reads because we use FFT instead of IFFT.
            let re = out[p0 + 1];
            let im = out[p0];
            let t0 = t[i];
            let t1 = t[n4 + i];
            let yr = re * t0 + im * t1;
            let yi = re * t1 - im * t0;

            // Read yp1 from the original buffer state, like libopus.
            let re2 = out[p1 + 1];
            let im2 = out[p1];
            out[p0] = yr;
            out[p1 + 1] = yi;

            let t2 = t[n4 - i - 1];
            let t3 = t[n2 - i - 1];
            let yr2 = re2 * t2 + im2 * t3;
            let yi2 = re2 * t3 - im2 * t2;
            out[p1] = yr2;
            out[p0 + 1] = yi2;

            p0 += 2;
            p1 -= 2;
        }

        for i in 0..(overlap / 2) {
            let x1 = out[overlap - 1 - i];
            let x2 = out[i];
            out[i] = x2 * window[overlap - 1 - i] - x1 * window[i];
            out[overlap - 1 - i] = x2 * window[i] + x1 * window[overlap - 1 - i];
        }
        Ok(())
    }
}

/// Compute forward DFT on flat interleaved complex buffers.
///
/// Params: `input`/`output` are `[re0, im0, re1, im1, ...]` and `n` is the
/// number of complex samples.
/// Returns: nothing; writes the forward transform to `output`.
fn flat_fft_forward(input: &[f32], output: &mut [f32], n: usize) {
    assert!(input.len() >= 2 * n && output.len() >= 2 * n);
    for k in 0..n {
        let mut sum_re = 0.0f64;
        let mut sum_im = 0.0f64;
        for j in 0..n {
            let angle = -2.0 * PI as f64 * (k as f64) * (j as f64) / (n as f64);
            let (sin_a, cos_a) = angle.sin_cos();
            let re = input[2 * j] as f64;
            let im = input[2 * j + 1] as f64;
            sum_re += re * cos_a - im * sin_a;
            sum_im += re * sin_a + im * cos_a;
        }
        output[2 * k] = sum_re as f32;
        output[2 * k + 1] = sum_im as f32;
    }
}
