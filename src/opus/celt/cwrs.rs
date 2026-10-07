//! CELT combinatorial PVQ pulse decoding (`cwrs.c`) for decode path.

use crate::opus::entropy::EcDec;

/// Compute U(n, k) row up to `k+1` and return V(n, k).
///
/// Params: `n` dimensions, `k` pulses.
/// Returns: `(u_row, v_nk)` where `u_row[i] = U(n, i)`.
fn ncwrs_urow(n: usize, k: usize) -> (Vec<u32>, u32) {
    let len = k + 2;
    let mut u = vec![0u32; len];
    u[0] = 0;
    u[1] = 1;
    for (idx, item) in u.iter_mut().enumerate().skip(2) {
        *item = ((idx as u32) << 1) - 1;
    }
    for _ in 2..n {
        unext(&mut u[1..], 1);
    }
    let v = u[k].wrapping_add(u[k + 1]);
    (u, v)
}

/// Advance one recurrence row for U-table generation.
///
/// Params: mutable row slice and first base value.
/// Returns: nothing.
fn unext(u: &mut [u32], mut u0: u32) {
    if u.len() < 2 {
        return;
    }
    for j in 1..u.len() {
        let u1 = u[j].wrapping_add(u[j - 1]).wrapping_add(u0);
        u[j - 1] = u0;
        u0 = u1;
    }
    let last = u.len() - 1;
    u[last] = u0;
}

/// Step recurrence backwards for one dimension removal.
///
/// Params: mutable row slice and first base value.
/// Returns: nothing.
fn uprev(u: &mut [u32], mut u0: u32) {
    if u.len() < 2 {
        return;
    }
    for j in 1..u.len() {
        let u1 = u[j].wrapping_sub(u[j - 1]).wrapping_sub(u0);
        u[j - 1] = u0;
        u0 = u1;
    }
    let last = u.len() - 1;
    u[last] = u0;
}

/// Decode CWRS index into signed pulse vector.
///
/// Params: `n` dimensions, `k` pulses, index `idx`, and mutable U-row scratch.
/// Returns: decoded vector.
fn cwrsi(n: usize, mut k: usize, mut idx: u32, u: &mut [u32]) -> Vec<i32> {
    let mut y = vec![0i32; n];
    for yj in y.iter_mut().take(n) {
        let p = u[k + 1];
        let neg = idx >= p;
        if neg {
            idx = idx.wrapping_sub(p);
        }
        let yk = k;
        let mut cur = u[k];
        while cur > idx {
            k -= 1;
            cur = u[k];
        }
        idx = idx.wrapping_sub(cur);
        let mut val = (yk - k) as i32;
        if neg {
            val = -val;
        }
        *yj = val;
        uprev(&mut u[..=k + 1], 0);
    }
    y
}

/// Decode PVQ pulses from range coder.
///
/// Params: range decoder, `n` dimensions and `k` pulses.
/// Returns: decoded signed pulses.
pub(crate) fn decode_pulses(dec: &mut EcDec<'_>, n: usize, k: usize) -> Vec<i32> {
    if n == 0 || k == 0 {
        return vec![0; n];
    }
    let (mut u, nc) = ncwrs_urow(n, k);
    let idx = dec.dec_uint(nc.max(2));
    cwrsi(n, k, idx, &mut u)
}
