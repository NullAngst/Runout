//! Steady hiss reduction by spectral subtraction, plus noise profile learning
//! and a band-limited spectral repair used by manual repair.

use realfft::num_complex::Complex;
use realfft::RealFftPlanner;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const FFT: usize = 2048;
pub const HOP: usize = 512;
pub const BINS: usize = FFT / 2 + 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NoiseProfile {
    pub rate: u32,
    /// Mean magnitude per bin for an FFT of size 2048 (Hann window).
    pub mag: Vec<f32>,
    /// Where it was learned from, in source seconds.
    pub from_s: f64,
    pub to_s: f64,
}

impl NoiseProfile {
    /// Level of the profile in dBFS RMS (approximate).
    pub fn level_db(&self) -> f32 {
        let wsum: f32 = hann(FFT).iter().map(|w| w * w).sum();
        let p: f32 = self.mag.iter().map(|m| m * m).sum::<f32>() * 2.0 / (FFT as f32 * wsum);
        10.0 * p.max(1e-20).log10()
    }
}

pub fn hann(n: usize) -> Vec<f32> {
    (0..n).map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()) as f32).collect()
}

thread_local! {
    static PLANNER: std::cell::RefCell<RealFftPlanner<f32>> = std::cell::RefCell::new(RealFftPlanner::new());
}

pub fn fft_pair(n: usize) -> (Arc<dyn realfft::RealToComplex<f32>>, Arc<dyn realfft::ComplexToReal<f32>>) {
    PLANNER.with(|p| {
        let mut p = p.borrow_mut();
        (p.plan_fft_forward(n), p.plan_fft_inverse(n))
    })
}

/// Learn a profile from a stretch of noise (one channel or a mono mix).
pub fn learn_profile(x: &[f32], rate: u32, from_s: f64, to_s: f64) -> Option<NoiseProfile> {
    if x.len() < FFT * 2 {
        return None;
    }
    let (fwd, _) = fft_pair(FFT);
    let win = hann(FFT);
    let mut acc = vec![0.0f64; BINS];
    let mut frames = 0;
    let mut inbuf = vec![0.0f32; FFT];
    let mut spec = vec![Complex::new(0.0f32, 0.0); BINS];
    let mut pos = 0;
    while pos + FFT <= x.len() {
        for i in 0..FFT {
            inbuf[i] = x[pos + i] * win[i];
        }
        fwd.process(&mut inbuf, &mut spec).ok()?;
        for (a, s) in acc.iter_mut().zip(&spec) {
            *a += s.norm_sqr() as f64;
        }
        frames += 1;
        pos += HOP;
    }
    let mag = acc.iter().map(|a| ((a / frames as f64).sqrt()) as f32).collect();
    Some(NoiseProfile { rate, mag, from_s, to_s })
}

/// Profile resampled onto this rate's bin grid.
fn profile_for_rate(p: &NoiseProfile, rate: u32) -> Vec<f32> {
    if p.rate == rate || p.mag.len() != BINS {
        return p.mag.clone();
    }
    let scale = rate as f64 / p.rate as f64;
    (0..BINS)
        .map(|k| {
            let src = k as f64 * scale;
            let i = src.floor() as usize;
            if i + 1 >= BINS {
                p.mag[BINS - 1]
            } else {
                let f = (src - i as f64) as f32;
                p.mag[i] * (1.0 - f) + p.mag[i + 1] * f
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
pub struct DenoiseParams {
    pub reduction_db: f32,
    /// 0..1, temporal/frequency smoothing of the gains.
    pub smoothing: f32,
}

/// Denoise one channel. `abs0` is the absolute sample index of x[0] so frames
/// line up the same way no matter where a block starts.
pub fn denoise(x: &mut [f32], abs0: i64, rate: u32, profile: &NoiseProfile, prm: &DenoiseParams) {
    let n = x.len();
    if n < FFT {
        return;
    }
    let noise = profile_for_rate(profile, rate);
    let noise_p: Vec<f32> = noise.iter().map(|m| m * m).collect();
    let floor = 10f32.powf(-prm.reduction_db.max(0.0) / 20.0);
    let over = 2.0f32;
    let s = (prm.smoothing.clamp(0.0, 1.0)) * 0.8;
    let (fwd, inv) = fft_pair(FFT);
    let win = hann(FFT);
    // Hann analysis + Hann synthesis at 75% overlap sums to 1.5.
    let norm = 1.0 / (FFT as f32 * 1.5);

    let first = -((abs0.rem_euclid(HOP as i64)) as i64) - (FFT - HOP) as i64;
    let mut out = vec![0.0f32; n];
    let mut inbuf = vec![0.0f32; FFT];
    let mut spec = vec![Complex::new(0.0f32, 0.0); BINS];
    let mut gprev = vec![1.0f32; BINS];
    let mut g = vec![1.0f32; BINS];
    let mut start = first;
    while start < n as i64 {
        for i in 0..FFT {
            let idx = start + i as i64;
            inbuf[i] = if idx >= 0 && (idx as usize) < n { x[idx as usize] * win[i] } else { 0.0 };
        }
        if fwd.process(&mut inbuf, &mut spec).is_err() {
            return;
        }
        for k in 0..BINS {
            let px = spec[k].norm_sqr().max(1e-24);
            let raw = (1.0 - over * noise_p[k] / px).max(0.0).sqrt().max(floor);
            let gp = gprev[k];
            let a = if raw > gp { s * 0.25 } else { s };
            g[k] = a * gp + (1.0 - a) * raw;
        }
        if s > 0.0 {
            // light smoothing across frequency
            let wgt = s * 0.5;
            let mut tmp = g.clone();
            for k in 1..BINS - 1 {
                tmp[k] = (1.0 - wgt) * g[k] + wgt * 0.5 * (g[k - 1] + g[k + 1]);
            }
            g.copy_from_slice(&tmp);
        }
        gprev.copy_from_slice(&g);
        for k in 0..BINS {
            spec[k] *= g[k];
        }
        spec[0].im = 0.0;
        spec[BINS - 1].im = 0.0;
        if inv.process(&mut spec, &mut inbuf).is_err() {
            return;
        }
        for i in 0..FFT {
            let idx = start + i as i64;
            if idx >= 0 && (idx as usize) < n {
                out[idx as usize] += inbuf[i] * win[i] * norm;
            }
        }
        start += HOP as i64;
    }
    x.copy_from_slice(&out);
}

/// Band-limited spectral repair: inside [t0, t1) and bins for [lo_hz, hi_hz],
/// pull magnitudes down to the level of the audio just outside the selection,
/// keeping phase.
pub fn spectral_repair(x: &mut [f32], rate: u32, t0: usize, t1: usize, lo_hz: f32, hi_hz: f32) {
    let n = x.len();
    if t1 <= t0 || n < FFT * 2 {
        return;
    }
    let (fwd, inv) = fft_pair(FFT);
    let win = hann(FFT);
    let norm = 1.0 / (FFT as f32 * 1.5);
    let binhz = rate as f32 / FFT as f32;
    let k0 = ((lo_hz / binhz).floor() as usize).min(BINS - 1);
    let k1 = ((hi_hz / binhz).ceil() as usize).min(BINS - 1).max(k0);

    // Reference magnitudes from frames just before and after the selection.
    let mut inbuf = vec![0.0f32; FFT];
    let mut spec = vec![Complex::new(0.0f32, 0.0); BINS];
    let mut ref_mag = vec![f32::MAX; BINS];
    let mut take_ref = |pos: i64, ref_mag: &mut Vec<f32>| {
        if pos < 0 || pos as usize + FFT > n {
            return;
        }
        for i in 0..FFT {
            inbuf[i] = x[pos as usize + i] * win[i];
        }
        if fwd.process(&mut inbuf, &mut spec).is_ok() {
            for k in 0..BINS {
                ref_mag[k] = ref_mag[k].min(spec[k].norm());
            }
        }
    };
    for j in 1..=3 {
        take_ref(t0 as i64 - (FFT + j * HOP) as i64, &mut ref_mag);
        take_ref((t1 + j * HOP) as i64, &mut ref_mag);
    }
    if ref_mag[0] == f32::MAX {
        return;
    }

    let a0 = (t0 as i64 - FFT as i64).max(0) as usize;
    let a1 = (t1 + FFT).min(n);
    let mut out = vec![0.0f32; a1 - a0];
    let mut wsum = vec![0.0f32; a1 - a0];
    let mut start = a0 as i64 - (FFT - HOP) as i64;
    let mut inbuf = vec![0.0f32; FFT];
    let mut spec = vec![Complex::new(0.0f32, 0.0); BINS];
    while start < a1 as i64 {
        for i in 0..FFT {
            let idx = start + i as i64;
            inbuf[i] = if idx >= 0 && (idx as usize) < n { x[idx as usize] * win[i] } else { 0.0 };
        }
        let centre = start + (FFT / 2) as i64;
        let inside = centre >= t0 as i64 - (FFT / 4) as i64 && centre < t1 as i64 + (FFT / 4) as i64;
        if fwd.process(&mut inbuf, &mut spec).is_err() {
            return;
        }
        if inside {
            for k in k0..=k1 {
                let m = spec[k].norm();
                if m > ref_mag[k] && m > 0.0 {
                    spec[k] *= ref_mag[k] / m;
                }
            }
        }
        spec[0].im = 0.0;
        spec[BINS - 1].im = 0.0;
        if inv.process(&mut spec, &mut inbuf).is_err() {
            return;
        }
        for i in 0..FFT {
            let idx = start + i as i64;
            if idx >= a0 as i64 && (idx as usize) < a1 {
                let j = idx as usize - a0;
                out[j] += inbuf[i] * win[i] * norm;
                wsum[j] += win[i] * win[i] / 1.5;
            }
        }
        start += HOP as i64;
    }
    // Only replace where the overlap-add is complete; cross-fade at the edges of
    // the touched span so untouched audio stays bit-identical outside it.
    let fade = FFT / 2;
    let len = a1 - a0;
    for j in 0..len {
        if wsum[j] < 0.99 {
            continue;
        }
        let edge = j.min(len - 1 - j);
        let w = if edge >= fade { 1.0 } else { edge as f32 / fade as f32 };
        x[a0 + j] = x[a0 + j] * (1.0 - w) + out[j] * w;
    }
}
