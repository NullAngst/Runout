//! Software decode for tapes recorded with noise reduction: a B-type sliding
//! band compander (for tapes recorded with Dolby B) and a dbx II style 1:2
//! expander.
//!
//! B-type. The encoder adds a side signal to the audio: a first-order high-pass
//! whose corner slides upward as the side signal gets louder, scaled for 10 dB
//! of treble boost at most. Quiet treble gets the full boost; loud treble pushes
//! the corner up and out of the way, so it gets little. The shape follows the
//! published B-type encode curves: about +10 dB at 10 kHz at -40 dB re the
//! reference level, falling to about +1 dB at the reference level, with the
//! boost reaching down to a few hundred hertz only on quiet material. The
//! decoder is the exact inverse: it subtracts the same side signal, computed
//! from its own output (the feedback form a B-type decoder uses), with the
//! control driven by past samples only so each sample solves in closed form.
//! Decoding audio encoded here gives the input back to float precision.
//!
//! This is a model of B-type behaviour, not Dolby's circuit. A tape recorded on
//! a real deck decodes well when the reference level is right; a 2 dB error is
//! audible as dull or spitty treble.
//!
//! dbx II. A 2:1 compressor on record and 1:2 expander on playback around a
//! fixed pivot, with an RMS detector, a band-limited control path and a
//! treble pre-emphasis that the decoder takes back out. Level independent: no
//! calibration. The emphasis corners and detector time constant here are
//! tuning values chosen to behave like a dbx II decoder, not figures from a dbx
//! document, since none could be found.

use super::biquad::{self, Biquad};
use super::tape_eq::shelf;
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum NrKind {
    #[default]
    Btype,
    Dbx2,
}

/// Most boost B-type adds, dB.
const B_MAX_DB: f64 = 10.0;
/// Corner of the sliding filter at low levels, Hz.
const B_F0: f64 = 1200.0;
/// Side-signal level, relative to the reference tone's RMS, where the corner
/// starts to slide.
const B_THRESH_DB: f64 = -27.0;
/// How hard the corner slides above the threshold.
const B_SLIDE: f64 = 1.6;
const B_ATTACK_MS: f64 = 2.0;
const B_RELEASE_MS: f64 = 60.0;

struct BState {
    k: f64,
    thr: f64,
    att: f64,
    rel: f64,
    fs: f64,
    x1: f64,
    s1: f64,
    env: f64,
}

impl BState {
    fn new(fs: f64, ref_dbfs: f32) -> Self {
        let ref_rms = 10f64.powf(ref_dbfs as f64 / 20.0);
        BState {
            k: 10f64.powf(B_MAX_DB / 20.0) - 1.0,
            thr: ref_rms * 10f64.powf(B_THRESH_DB / 20.0),
            att: 1.0 - (-1.0 / (B_ATTACK_MS * 1e-3 * fs)).exp(),
            rel: 1.0 - (-1.0 / (B_RELEASE_MS * 1e-3 * fs)).exp(),
            fs,
            x1: 0.0,
            s1: 0.0,
            env: 0.0,
        }
    }

    /// Sliding filter coefficient for this sample, from the envelope so far.
    #[inline]
    fn coeff(&self) -> f64 {
        let r = self.env / self.thr;
        let fc = if r > 1.0 { B_F0 * r.powf(B_SLIDE) } else { B_F0 };
        let fc = fc.min(0.45 * self.fs);
        (-2.0 * PI * fc / self.fs).exp()
    }

    #[inline]
    fn track(&mut self, s: f64) {
        let a = s.abs();
        let c = if a > self.env { self.att } else { self.rel };
        self.env += (a - self.env) * c;
    }
}

/// B-type encode, used by the tests and the self-test.
pub fn btype_encode(x: &mut [f32], fs: f64, ref_dbfs: f32) {
    let mut st = BState::new(fs, ref_dbfs);
    for v in x.iter_mut() {
        let a = st.coeff();
        let xn = *v as f64;
        let s = a * (st.s1 + xn - st.x1);
        st.x1 = xn;
        st.s1 = s;
        st.track(s);
        *v = (xn + st.k * s) as f32;
    }
}

/// B-type decode: the inverse of `btype_encode`.
pub fn btype_decode(y: &mut [f32], fs: f64, ref_dbfs: f32) {
    let mut st = BState::new(fs, ref_dbfs);
    for v in y.iter_mut() {
        let a = st.coeff();
        let yn = *v as f64;
        // y = x + k*a*(s1 - x1) + k*a*x  ->  solve for x
        let x = (yn - st.k * a * (st.s1 - st.x1)) / (1.0 + st.k * a);
        let s = a * (st.s1 + x - st.x1);
        st.x1 = x;
        st.s1 = s;
        st.track(s);
        *v = x as f32;
    }
}

/// Pivot of the dbx II compander, dBFS RMS: levels here pass unchanged.
const DBX_PIVOT_DB: f64 = -20.0;
/// Treble pre-emphasis: a shelf with these time constants, us (about +16 dB).
const DBX_EMPH_ZERO_US: f64 = 106.0;
const DBX_EMPH_POLE_US: f64 = 16.0;
/// RMS detector time constant, ms.
const DBX_RMS_MS: f64 = 30.0;
/// Control path band limits, Hz.
const DBX_CTRL_LO: f64 = 40.0;
const DBX_CTRL_HI: f64 = 10_000.0;

struct Detector {
    hp: Vec<Biquad>,
    lp: Vec<Biquad>,
    ms: f64,
    c: f64,
}

impl Detector {
    fn new(fs: f64) -> Self {
        Detector {
            hp: biquad::butter_highpass(fs, DBX_CTRL_LO, 2),
            lp: biquad::butter_lowpass(fs, DBX_CTRL_HI.min(0.45 * fs), 2),
            // start at the pivot so the first samples are not slammed
            ms: 10f64.powf(DBX_PIVOT_DB / 10.0),
            c: 1.0 - (-1.0 / (DBX_RMS_MS * 1e-3 * fs)).exp(),
        }
    }

    /// Feed one sample, return the RMS level in dB.
    #[inline]
    fn feed(&mut self, x: f64) -> f64 {
        let mut v = x;
        for s in self.hp.iter_mut() {
            v = s.tick(v);
        }
        for s in self.lp.iter_mut() {
            v = s.tick(v);
        }
        self.ms += (v * v - self.ms) * self.c;
        10.0 * self.ms.max(1e-14).log10()
    }
}

/// dbx II style encode (pre-emphasis, then 2:1 around the pivot). Tests only.
pub fn dbx2_encode(x: &mut [f32], fs: f64) {
    let mut pre = shelf(fs, DBX_EMPH_ZERO_US, DBX_EMPH_POLE_US);
    let mut det = Detector::new(fs);
    for v in x.iter_mut() {
        let e = pre.tick(*v as f64);
        let l = det.feed(e);
        let g = 10f64.powf((DBX_PIVOT_DB - l) / 2.0 / 20.0);
        *v = (e * g) as f32;
    }
}

/// dbx II style decode: 1:2 expansion around the pivot, then de-emphasis.
pub fn dbx2_decode(y: &mut [f32], fs: f64) {
    let mut de = shelf(fs, DBX_EMPH_POLE_US, DBX_EMPH_ZERO_US);
    let mut det = Detector::new(fs);
    for v in y.iter_mut() {
        let yn = *v as f64;
        let l = det.feed(yn);
        // the tape carries (L_in + pivot)/2; expand back by the same distance
        let g = 10f64.powf(((l - DBX_PIVOT_DB).clamp(-60.0, 30.0)) / 20.0);
        *v = de.tick(yn * g) as f32;
    }
}

/// Decode one channel with the chosen system.
pub fn decode(x: &mut [f32], fs: f64, kind: NrKind, ref_dbfs: f32) {
    match kind {
        NrKind::Btype => btype_decode(x, fs, ref_dbfs),
        NrKind::Dbx2 => dbx2_decode(x, fs),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(fs: f64, f: f64, db_re: f64, ref_dbfs: f32, secs: f64) -> Vec<f32> {
        let a = 10f64.powf((ref_dbfs as f64 + db_re) / 20.0) * std::f64::consts::SQRT_2;
        (0..(fs * secs) as usize).map(|i| (a * (2.0 * PI * f * i as f64 / fs).sin()) as f32).collect()
    }

    fn rms_db(x: &[f32]) -> f64 {
        let s: f64 = x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64;
        10.0 * s.max(1e-30).log10()
    }

    /// Steady-state encode boost for a tone, dB.
    fn boost(fs: f64, f: f64, lvl: f64) -> f64 {
        let mut x = tone(fs, f, lvl, -20.0, 1.0);
        let before = rms_db(&x[(fs * 0.5) as usize..]);
        btype_encode(&mut x, fs, -20.0);
        rms_db(&x[(fs * 0.5) as usize..]) - before
    }

    #[test]
    fn btype_follows_published_shape() {
        let fs = 96000.0;
        // treble: full boost when quiet, little at the reference level
        let q = boost(fs, 10_000.0, -40.0);
        let m = boost(fs, 10_000.0, -20.0);
        let l = boost(fs, 10_000.0, 0.0);
        assert!(q > 9.0 && q <= 10.05, "-40 dB at 10 kHz: {q:.2}");
        assert!(m > 4.0 && m < 9.0, "-20 dB at 10 kHz: {m:.2}");
        assert!(l > 0.0 && l < 3.0, "0 dB at 10 kHz: {l:.2}");
        // bass is left alone
        assert!(boost(fs, 100.0, -40.0) < 0.7);
        // quiet 1 kHz gets some, less than the treble
        let k = boost(fs, 1000.0, -40.0);
        assert!(k > 3.0 && k < q, "-40 dB at 1 kHz: {k:.2}");
    }

    #[test]
    fn btype_round_trip_is_exact() {
        let fs = 48000.0;
        let mut x: Vec<f32> = (0..48000).map(|i| ((i as f32 * 0.013).sin() * 0.2 + (i as f32 * 0.9).sin() * 0.05) * (1.0 + (i as f32 * 0.0003).sin())).collect();
        let orig = x.clone();
        btype_encode(&mut x, fs, -18.0);
        btype_decode(&mut x, fs, -18.0);
        let err = orig.iter().zip(&x).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(err < 1e-4, "round trip error {err}");
    }

    #[test]
    fn dbx2_round_trip_close() {
        let fs = 48000.0;
        for lvl in [-50.0, -30.0, -10.0] {
            let mut x = tone(fs, 1000.0, lvl, 0.0, 2.0);
            let before = rms_db(&x[48000..]);
            dbx2_encode(&mut x, fs);
            let mid = rms_db(&x[48000..]);
            dbx2_decode(&mut x, fs);
            let after = rms_db(&x[48000..]);
            assert!((after - before).abs() < 0.5, "{lvl}: {before:.2} -> {mid:.2} -> {after:.2}");
        }
    }
}
