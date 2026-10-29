//! Tape azimuth: the right channel's time offset against the left, measured
//! over the recording and corrected as a fixed delay or one that follows it.
//!
//! Measurement: both channels band-passed to 2-10 kHz (where head tilt shows),
//! cross-correlated over 2 s windows every 0.5 s within +-1 ms, the peak refined
//! by parabolic interpolation. Windows where the channels hardly correlate
//! (wide stereo, silence) give no usable lag and are skipped. When the lag
//! barely moves, a single static value is used; when it drifts (a tape that
//! wanders, a splice), the curve is used.

use super::biquad;
use super::resample::Interp;

pub const WIN_S: f64 = 2.0;
pub const HOP_S: f64 = 0.5;
pub const MAX_US: f64 = 1000.0;
/// Normalized correlation a window needs to count.
const MIN_COHERENCE: f64 = 0.35;
/// Spread (max - min, us) under which the curve is called static.
const STATIC_SPREAD_US: f32 = 4.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AzimuthCurve {
    /// Source seconds of each point (window centres).
    pub t: Vec<f32>,
    /// Right-channel correction, microseconds (positive delays R).
    pub us: Vec<f32>,
}

impl AzimuthCurve {
    pub fn is_empty(&self) -> bool {
        self.t.is_empty()
    }

    /// Correction at source time `s` (held at the ends, linear between points).
    pub fn at(&self, s: f64) -> f32 {
        let n = self.t.len();
        if n == 0 {
            return 0.0;
        }
        let s = s as f32;
        if s <= self.t[0] {
            return self.us[0];
        }
        if s >= self.t[n - 1] {
            return self.us[n - 1];
        }
        let k = self.t.partition_point(|&x| x <= s).max(1) - 1;
        let f = (s - self.t[k]) / (self.t[k + 1] - self.t[k]).max(1e-6);
        self.us[k] + (self.us[k + 1] - self.us[k]) * f
    }

    pub fn mean(&self) -> f32 {
        if self.us.is_empty() {
            0.0
        } else {
            self.us.iter().sum::<f32>() / self.us.len() as f32
        }
    }

    pub fn spread(&self) -> f32 {
        let mx = self.us.iter().cloned().fold(f32::MIN, f32::max);
        let mn = self.us.iter().cloned().fold(f32::MAX, f32::min);
        if self.us.is_empty() {
            0.0
        } else {
            mx - mn
        }
    }

    /// True when a single value describes it well enough.
    pub fn is_static(&self) -> bool {
        self.spread() <= STATIC_SPREAD_US
    }
}

fn band(x: &[f32], rate: u32) -> Vec<f32> {
    let mut y = x.to_vec();
    let fs = rate as f64;
    let mut hp = biquad::butter_highpass(fs, 2000.0, 4);
    biquad::run_cascade(&mut hp, &mut y);
    let mut lp = biquad::butter_lowpass(fs, 10_000f64.min(0.45 * fs), 4);
    biquad::run_cascade(&mut lp, &mut y);
    y
}

/// Measure the azimuth curve of a stereo recording.
pub fn measure(l: &[f32], r: &[f32], rate: u32) -> AzimuthCurve {
    let fs = rate as f64;
    let lb = band(l, rate);
    let rb = band(r, rate);
    let win = (WIN_S * fs) as usize;
    let hop = (HOP_S * fs) as usize;
    let maxlag = ((MAX_US * 1e-6) * fs).ceil() as i64;
    let n = lb.len().min(rb.len());
    let mut curve = AzimuthCurve::default();
    if n < win + 2 * maxlag as usize {
        return curve;
    }
    let mut s = maxlag as usize;
    let mut idx: Vec<usize> = Vec::new();
    while s + win + maxlag as usize <= n {
        idx.push(s);
        s += hop;
    }
    // Coarse search on every `dec`-th sample (the band is already under 10 kHz),
    // then the fine peak at the full rate over a few lags around it.
    let dec = ((rate / 24000).max(1)) as usize;
    let coarse = (maxlag as usize).div_ceil(dec) as i64;
    use rayon::prelude::*;
    let pts: Vec<Option<(f32, f32)>> = idx
        .par_iter()
        .map(|&s| {
            let a = &lb[s..s + win];
            let el: f64 = a.iter().map(|v| (*v as f64).powi(2)).sum();
            let er: f64 = rb[s..s + win].iter().map(|v| (*v as f64).powi(2)).sum();
            if el < 1e-9 || er < 1e-9 {
                return None;
            }
            let corr = |lag: i64, step: usize| -> f64 {
                let off = (s as i64 + lag) as usize;
                let b = &rb[off..off + win];
                let mut acc = 0.0f64;
                let mut i = 0;
                while i < win {
                    acc += a[i] as f64 * b[i] as f64;
                    i += step;
                }
                acc * step as f64
            };
            let mut best = (0i64, f64::MIN);
            for k in -coarse..=coarse {
                let lag = (k * dec as i64).clamp(-maxlag, maxlag);
                let v = corr(lag, dec);
                if v > best.1 {
                    best = (lag, v);
                }
            }
            let lo = (best.0 - dec as i64 - 1).max(-maxlag);
            let hi = (best.0 + dec as i64 + 1).min(maxlag);
            let vals: Vec<(i64, f64)> = (lo..=hi).map(|lag| (lag, corr(lag, 1))).collect();
            let (mut bi, mut bv) = (0usize, f64::MIN);
            for (i, (_, v)) in vals.iter().enumerate() {
                if *v > bv {
                    bv = *v;
                    bi = i;
                }
            }
            if bv / (el * er).sqrt() < MIN_COHERENCE || bi == 0 || bi + 1 == vals.len() {
                return None;
            }
            let (y0, y1, y2) = (vals[bi - 1].1, vals[bi].1, vals[bi + 1].1);
            let d = y0 - 2.0 * y1 + y2;
            let mut lag = vals[bi].0 as f64;
            if d.abs() > 1e-18 {
                lag += 0.5 * (y0 - y2) / d;
            }
            // R lags L by `lag` samples, so R must move earlier: correction = -lag
            let us = (-lag / fs * 1e6) as f32;
            Some(((s + win / 2) as f32 / rate as f32, us))
        })
        .collect();
    for (t, us) in pts.into_iter().flatten() {
        curve.t.push(t);
        curve.us.push(us);
    }
    // light smoothing (3-point median) to drop single bad windows
    if curve.us.len() >= 3 {
        let u = curve.us.clone();
        for i in 1..u.len() - 1 {
            let mut w = [u[i - 1], u[i], u[i + 1]];
            w.sort_by(|a, b| a.partial_cmp(b).unwrap());
            curve.us[i] = w[1];
        }
    }
    curve
}

/// Delay `x` by a varying amount: out[n] = x[n - d(n)], d in samples.
pub fn tracked_delay(x: &[f32], d: impl Fn(usize) -> f64) -> Vec<f32> {
    let it = Interp::new(1.0);
    (0..x.len()).map(|n| it.at(x, n as f64 - d(n))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_static_offset() {
        let rate = 96000u32;
        let n = rate as usize * 8;
        let mut seed = 11u64;
        let x: Vec<f32> = (0..n)
            .map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((seed >> 40) as f32 / (1u64 << 24) as f32) - 0.5
            })
            .collect();
        // R arrives 40 us late
        let d = 40e-6 * rate as f64;
        let r = crate::dsp::resample::frac_delay(&x, d);
        let c = measure(&x, &r, rate);
        assert!(!c.is_empty());
        assert!(c.is_static(), "spread {}", c.spread());
        assert!((c.mean() + 40.0).abs() < 2.0, "mean {}", c.mean());
    }
}
