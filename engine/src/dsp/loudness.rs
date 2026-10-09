//! BS.1770 loudness measurement via the ebur128 crate.

use ebur128::{EbuR128, Mode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Loudness {
    /// Integrated loudness, LUFS. -inf for silence (stored as -120).
    pub integrated: f32,
    /// Loudness range, LU.
    pub lra: f32,
    /// True peak, dBTP.
    pub true_peak: f32,
    /// Sample peak, dBFS.
    pub sample_peak: f32,
}

pub fn meter(channels: u32, rate: u32) -> Option<EbuR128> {
    EbuR128::new(channels, rate, Mode::I | Mode::LRA | Mode::TRUE_PEAK | Mode::SAMPLE_PEAK | Mode::M | Mode::S).ok()
}

pub fn feed(m: &mut EbuR128, ch: &[Vec<f32>], from: usize, to: usize) {
    let refs: Vec<&[f32]> = ch.iter().map(|c| &c[from..to]).collect();
    let _ = m.add_frames_planar_f32(&refs);
}

fn db(v: f64) -> f32 {
    if v <= 0.0 {
        -120.0
    } else {
        (20.0 * v.log10()) as f32
    }
}

fn fin(v: f64) -> f32 {
    if v.is_finite() {
        v as f32
    } else {
        -120.0
    }
}

pub fn read(m: &EbuR128) -> Loudness {
    let ch = m.channels();
    let mut tp = 0.0f64;
    let mut sp = 0.0f64;
    for c in 0..ch {
        tp = tp.max(m.true_peak(c).unwrap_or(0.0));
        sp = sp.max(m.sample_peak(c).unwrap_or(0.0));
    }
    Loudness {
        integrated: fin(m.loudness_global().unwrap_or(f64::NEG_INFINITY)),
        lra: fin(m.loudness_range().unwrap_or(0.0)).max(0.0),
        true_peak: db(tp),
        sample_peak: db(sp),
    }
}

pub fn measure(ch: &[Vec<f32>], rate: u32) -> (Loudness, Option<EbuR128>) {
    if ch.is_empty() || ch[0].is_empty() {
        return (Loudness { integrated: -120.0, lra: 0.0, true_peak: -120.0, sample_peak: -120.0 }, None);
    }
    let Some(mut m) = meter(ch.len() as u32, rate) else {
        return (Loudness::default(), None);
    };
    let n = ch[0].len();
    let step = rate as usize * 10;
    let mut i = 0;
    while i < n {
        let e = (i + step).min(n);
        feed(&mut m, ch, i, e);
        i = e;
    }
    (read(&m), Some(m))
}

/// Integrated loudness over several tracks as one programme (album mode).
pub fn album_integrated(meters: &[&EbuR128]) -> f32 {
    fin(EbuR128::loudness_global_multiple(meters.iter().copied()).unwrap_or(f64::NEG_INFINITY))
}

/// Noise floor estimate: 10th percentile of 400 ms RMS windows, dBFS.
pub fn noise_floor_db(ch: &[Vec<f32>], rate: u32) -> f32 {
    if ch.is_empty() {
        return -120.0;
    }
    let w = (rate as usize * 4) / 10;
    let n = ch[0].len();
    if n < w {
        return -120.0;
    }
    let mut vals = Vec::new();
    let mut i = 0;
    while i + w <= n {
        let mut s = 0.0f64;
        for c in ch {
            for v in &c[i..i + w] {
                s += (*v as f64) * (*v as f64);
            }
        }
        let rms = (s / (w * ch.len()) as f64).sqrt();
        if rms > 1e-7 {
            vals.push(rms);
        }
        i += w / 2;
    }
    if vals.is_empty() {
        return -120.0;
    }
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    db(vals[vals.len() / 10])
}
