//! Playback EQ curves as analog time constants, the filters that convert audio
//! played through one curve into what another curve would have given, and the
//! 50/15 us de-emphasis for digital sources.
//!
//! Every curve is a playback response of the form
//!
//!   P(s) = (1 + s*zero) / ((1 + s*lf_pole) * (1 + s*hf_pole))
//!
//! with a missing time constant meaning that factor is absent. Tape curves use
//! the zero (120, 70, 50, 90 or 35 us) and the 3180 us bass pole; disc curves
//! use all three, as RIAA does (3180, 318, 75 us). A correction from curve A
//! (what the playback chain applied) to curve B (what the recording needs) is
//! P_B / P_A, normalized to 0 dB at 1 kHz.
//!
//! The analog prototypes go to digital one first-order section at a time. Plain
//! bilinear mapping squeezes the top octave enough at 48 kHz to miss a 10 kHz
//! target by over 1 dB, so each section keeps its pole exactly (matched z) and
//! places its zero so the gain matches the analog section at DC and at one
//! match frequency: 10 kHz for disc curves, whose test range ends there, and
//! 16 kHz for tape and de-emphasis. Every built-in disc correction then holds
//! 0.36 dB from 50 Hz to 10 kHz at 44.1 kHz (0.07 dB at 96 kHz), and every tape
//! conversion 0.05 dB to 20 kHz at 48 kHz. The sections are plain first-order
//! `Biquad`s.

use super::biquad::Biquad;
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// Bass pole used where a curve's bass boost has no shelf (the "N" curves and
/// IEC at 7.5 and 15 ips). Without it the correction would boost DC without
/// limit; 10 Hz keeps the error at 50 Hz under 0.2 dB.
pub const N_LIMIT_US: f32 = 15915.494;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Curve {
    /// Bass pole, microseconds (0 = none: flat bass).
    pub lf_pole_us: f32,
    /// Turnover zero, microseconds (0 = none).
    pub zero_us: f32,
    /// Treble pole, microseconds (0 = none: no rolloff).
    pub hf_pole_us: f32,
}

impl Curve {
    pub const fn new(lf_pole_us: f32, zero_us: f32, hf_pole_us: f32) -> Self {
        Curve { lf_pole_us, zero_us, hf_pole_us }
    }

    pub fn is_flat(&self) -> bool {
        self.lf_pole_us <= 0.0 && self.zero_us <= 0.0 && self.hf_pole_us <= 0.0
    }

    /// A disc curve from a bass turnover (Hz, 0 = none) and a treble rolloff at
    /// 10 kHz (dB, positive or negative, 0 = flat), with the bass boost limited
    /// at 10 Hz. This is how 78 curves are usually written down.
    pub fn from_turnover(turnover_hz: f32, rolloff_db_10k: f32) -> Self {
        let zero_us = if turnover_hz > 0.0 { (1e6 / (2.0 * std::f64::consts::PI * turnover_hz as f64)) as f32 } else { 0.0 };
        let r = rolloff_db_10k.abs() as f64;
        let hf_pole_us = if r > 0.01 { ((10f64.powf(r / 10.0) - 1.0).sqrt() / (2.0 * PI * 10_000.0) * 1e6) as f32 } else { 0.0 };
        Curve { lf_pole_us: if zero_us > 0.0 { N_LIMIT_US } else { 0.0 }, zero_us, hf_pole_us }
    }

    /// Bass turnover in Hz, 0 when there is none.
    pub fn turnover_hz(&self) -> f32 {
        if self.zero_us > 0.0 {
            (1e6 / (2.0 * PI * self.zero_us as f64)) as f32
        } else {
            0.0
        }
    }

    /// Treble rolloff at 10 kHz in dB (negative), 0 when flat.
    pub fn rolloff_db_10k(&self) -> f32 {
        if self.hf_pole_us > 0.0 {
            let w = 2.0 * PI * 10_000.0 * self.hf_pole_us as f64 * 1e-6;
            (-10.0 * (1.0 + w * w).log10()) as f32
        } else {
            0.0
        }
    }

    /// Complex analog response at f (unnormalized), as (re, im).
    fn response(&self, f: f64) -> (f64, f64) {
        let w = 2.0 * PI * f;
        let mul = |a: (f64, f64), b: (f64, f64)| (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0);
        let div = |a: (f64, f64), b: (f64, f64)| {
            let d = b.0 * b.0 + b.1 * b.1;
            ((a.0 * b.0 + a.1 * b.1) / d, (a.1 * b.0 - a.0 * b.1) / d)
        };
        let term = |tau_us: f32| if tau_us > 0.0 { (1.0, w * tau_us as f64 * 1e-6) } else { (1.0, 0.0) };
        let num = term(self.zero_us);
        let den = mul(term(self.lf_pole_us), term(self.hf_pole_us));
        div(num, den)
    }

    /// Magnitude in dB relative to 1 kHz.
    pub fn db_re_1k(&self, f: f64) -> f64 {
        let m = |f: f64| {
            let (r, i) = self.response(f);
            (r * r + i * i).sqrt()
        };
        20.0 * (m(f) / m(1000.0)).log10()
    }
}

/// Target response of the correction from `from` to `to` in dB, 0 at 1 kHz.
pub fn target_db(from: &Curve, to: &Curve, f: f64) -> f64 {
    to.db_re_1k(f) - from.db_re_1k(f)
}

/// First-order section (1 + s*tz) / (1 + s*tp), either time constant may be 0.
/// The pole is exact, and the zero sits where the gain matches the analog
/// section at DC and at `fm` Hz.
fn section(fs: f64, tz_us: f64, tp_us: f64, fm: f64) -> Biquad {
    let tz = tz_us * 1e-6;
    let tp = tp_us * 1e-6;
    let a1 = if tp > 0.0 { -(-1.0 / (tp * fs)).exp() } else { 0.0 };
    let wa = 2.0 * PI * fm;
    let m2 = (1.0 + (wa * tz).powi(2)) / (1.0 + (wa * tp).powi(2));
    let c = (2.0 * PI * fm / fs).cos();
    let d = 1.0 + 2.0 * a1 * c + a1 * a1;
    let k = m2 * d / (1.0 + a1).powi(2);
    // (1-k) b^2 + 2 (c-k) b + (1-k) = 0, root inside the unit circle
    let (qa, qb) = (1.0 - k, 2.0 * (c - k));
    let b1 = if qa.abs() < 1e-15 {
        0.0
    } else {
        let disc = (qb * qb - 4.0 * qa * qa).max(0.0).sqrt();
        let r1 = (-qb + disc) / (2.0 * qa);
        let r2 = (-qb - disc) / (2.0 * qa);
        if r1.abs() < 1.0 {
            r1
        } else {
            r2
        }
    };
    let g = (1.0 + a1) / (1.0 + b1);
    Biquad::from_coeffs(g, g * b1, 0.0, a1, 0.0)
}

/// One first-order shelf (1 + s*zero) / (1 + s*pole), matched at DC and at
/// 16 kHz (or 0.45 fs). Used by the noise reduction emphasis networks.
pub fn shelf(fs: f64, zero_us: f64, pole_us: f64) -> Biquad {
    section(fs, zero_us, pole_us, 16_000f64.min(0.45 * fs))
}

/// Sections that turn audio played through `from` into audio played through
/// `to`, 0 dB at 1 kHz. Empty when the two are the same.
pub fn correction(fs: f64, from: &Curve, to: &Curve) -> Vec<Biquad> {
    // zeros of the correction: to's zero and from's poles; poles: to's poles and from's zero
    let mut zeros: Vec<f64> = Vec::new();
    let mut poles: Vec<f64> = Vec::new();
    let push = |v: &mut Vec<f64>, t: f32| {
        if t > 0.0 {
            v.push(t as f64)
        }
    };
    push(&mut zeros, to.zero_us);
    push(&mut zeros, from.lf_pole_us);
    push(&mut zeros, from.hf_pole_us);
    push(&mut poles, to.lf_pole_us);
    push(&mut poles, to.hf_pole_us);
    push(&mut poles, from.zero_us);
    // cancel identical time constants
    let mut i = 0;
    while i < zeros.len() {
        if let Some(j) = poles.iter().position(|p| (p - zeros[i]).abs() < 1e-6 * p.max(1.0)) {
            poles.remove(j);
            zeros.remove(i);
        } else {
            i += 1;
        }
    }
    if zeros.is_empty() && poles.is_empty() {
        return Vec::new();
    }
    zeros.sort_by(|a, b| b.partial_cmp(a).unwrap());
    poles.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let disc = from.hf_pole_us > 0.0 || to.hf_pole_us > 0.0;
    let fm = if disc { 10_000f64.min(0.4 * fs) } else { 16_000f64.min(0.45 * fs) };
    let n = zeros.len().max(poles.len());
    let mut out: Vec<Biquad> = (0..n).map(|k| section(fs, zeros.get(k).copied().unwrap_or(0.0), poles.get(k).copied().unwrap_or(0.0), fm)).collect();
    // normalize to 0 dB at 1 kHz
    let g: f64 = out.iter().map(|s| s.magnitude(fs, 1000.0)).product();
    if g > 0.0 {
        out.push(Biquad::from_coeffs(1.0 / g, 0.0, 0.0, 0.0, 0.0));
    }
    out
}

/// De-emphasis for 50/15 us pre-emphasized digital audio (DAT, CD-style flag).
pub fn deemphasis_50_15(fs: f64) -> Vec<Biquad> {
    vec![section(fs, 15.0, 50.0, 16_000f64.min(0.45 * fs))]
}

/// A named curve for the pickers.
#[derive(Clone, Debug, Serialize)]
pub struct NamedCurve {
    pub id: &'static str,
    pub name: &'static str,
    /// "tape", "disc" or "any".
    pub kind: &'static str,
    pub curve: Curve,
    pub note: &'static str,
}

/// Tape curves, from the time constants the build spec lists: cassette 120 and
/// 70 us (both with 3180 us), reel NAB 3180/50 us at 7.5 and 15 ips and
/// 3180/90 us at 3.75 and 1.875, reel IEC 35 us at 15 ips, 70 us at 7.5, and
/// 90 us plus 3180 us at 3.75.
pub fn tape_curves() -> Vec<NamedCurve> {
    vec![
        NamedCurve { id: "cassette-120", name: "Cassette 120 us", kind: "tape", curve: Curve::new(3180.0, 120.0, 0.0), note: "Type I (normal)" },
        NamedCurve { id: "cassette-70", name: "Cassette 70 us", kind: "tape", curve: Curve::new(3180.0, 70.0, 0.0), note: "Type II and IV" },
        NamedCurve { id: "nab-50", name: "NAB 3180/50 us", kind: "tape", curve: Curve::new(3180.0, 50.0, 0.0), note: "7.5 and 15 ips" },
        NamedCurve { id: "nab-90", name: "NAB 3180/90 us", kind: "tape", curve: Curve::new(3180.0, 90.0, 0.0), note: "3.75 and 1.875 ips" },
        NamedCurve { id: "iec-35", name: "IEC 35 us", kind: "tape", curve: Curve::new(N_LIMIT_US, 35.0, 0.0), note: "15 ips" },
        NamedCurve { id: "iec-70", name: "IEC 70 us", kind: "tape", curve: Curve::new(N_LIMIT_US, 70.0, 0.0), note: "7.5 ips" },
        NamedCurve { id: "iec-90", name: "IEC 3180/90 us", kind: "tape", curve: Curve::new(3180.0, 90.0, 0.0), note: "3.75 ips" },
    ]
}

/// RIAA: 3180, 318 and 75 us.
pub const RIAA: Curve = Curve::new(3180.0, 318.0, 75.0);

/// Disc curves for 78s and early LPs.
///
/// Source: Audacity, "Playback equalization for 78 rpm shellacs and early 33 1/3
/// LPs", Table 1 (https://plugins.audacityteam.org/additional-resources/eq-curves/playback-equalization-for-78-rpm-shellacs-and-early-33-lps),
/// which is mostly based on Tab. 2a of Heinz O. Graumann, "Schallplatten-
/// Schneidkennlinien und ihre Entzerrung", FUNKSCHAU 1958, Heft 15. The time
/// constants are that table's tau1/tau2/tau3; the two American 78 rows use the
/// table's computed treble corners (5900 and 4400 Hz). Curves the table writes
/// without a bass shelf ("N") get the 10 Hz limit in N_LIMIT_US.
///
/// `check_*` hold that table's own boost at 50 Hz and treble at 10 kHz, both
/// relative to 1 kHz, so the tests can prove the transcription. The European 78
/// and Blumlein 300 rows print a 50 Hz boost their own time constants do not
/// give (14.4 and 16 dB, where 636 and 531 us give 13.9 and 15.3 dB, as the
/// table's American 78 and Decca rows with the same time constants show), so
/// their 50 Hz check is left out and the time constants rule.
pub struct DiscCurve {
    pub named: NamedCurve,
    pub check_50_db: Option<f32>,
    pub check_10k_db: f32,
}

pub fn disc_curves() -> Vec<DiscCurve> {
    let n = N_LIMIT_US;
    let us = |f: f64| (1e6 / (2.0 * PI * f)) as f32;
    let d = |id, name, note, c: Curve, b50: Option<f32>, t10: f32| DiscCurve { named: NamedCurve { id, name, kind: "disc", curve: c, note }, check_50_db: b50, check_10k_db: t10 };
    vec![
        d("riaa", "RIAA", "Every LP after about 1956", RIAA, Some(16.9), -13.7),
        d("european-78", "European 78 (250N-0)", "Europe and Britain 1926-1946, EMI", Curve::new(n, 636.0, 0.0), None, 0.0),
        d("blumlein-300", "Blumlein 300 (300N-0)", "Europe and Britain 1927-1946", Curve::new(n, 531.0, 0.0), None, 0.0),
        d("flat-500", "500-FLAT (500N-0)", "Britain and America 1926-1950", Curve::new(n, 318.0, 0.0), Some(19.0), 0.0),
        d("american-78-6", "American 78 (250N-6)", "America 1926-1951", Curve::new(n, 636.0, us(5900.0)), Some(14.0), -6.0),
        d("american-78-8", "American 78 (250N-8)", "America 1926-1951", Curve::new(n, 636.0, us(4400.0)), Some(14.0), -8.0),
        d("columbia-78", "Columbia 78 (300N-16)", "Columbia (CBS) 1938-1948", Curve::new(n, 530.0, 100.0), Some(16.7), -15.0),
        d("old-rca", "Old RCA (800N-8)", "RCA 1941-1947", Curve::new(n, 199.0, 36.7), Some(22.2), -9.9),
        d("decca-78", "Decca 78 ffrr (300N-5.5)", "Decca and London 1944-1956", Curve::new(n, 531.0, 25.0), Some(15.4), -5.7),
        d("ccir-78", "CCIR 78 (350N-10.5)", "Germany 1952-1955", Curve::new(n, 450.0, 50.0), Some(17.0), -10.5),
        d("iec-n78", "IEC N78 (350A-10.5)", "Europe and Britain 1955 on", Curve::new(3180.0, 450.0, 50.0), Some(14.0), -10.5),
    ]
}

/// Every curve the pickers offer, flat first.
pub fn all_curves() -> Vec<NamedCurve> {
    let mut v = vec![NamedCurve { id: "flat", name: "Flat (no EQ)", kind: "any", curve: Curve::default(), note: "" }];
    v.extend(tape_curves());
    v.extend(disc_curves().into_iter().map(|d| d.named));
    v
}

pub fn find_curve(id: &str) -> Option<Curve> {
    all_curves().into_iter().find(|c| c.id == id).map(|c| c.curve)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::biquad::cascade_magnitude;

    fn log_freqs() -> Vec<f64> {
        (0..=60).map(|i| 50.0 * (200f64).powf(i as f64 / 60.0)).collect()
    }

    #[test]
    fn table_transcription_matches_its_own_check_columns() {
        for d in disc_curves() {
            let c = d.named.curve;
            let b = c.db_re_1k(50.0);
            let t = c.db_re_1k(10_000.0);
            if let Some(want) = d.check_50_db {
                assert!((b - want as f64).abs() < 0.25, "{}: 50 Hz {b:.2} vs table {want}", d.named.id);
            }
            // a 0 in the table's treble column means "no rolloff", not 0.0 dB re 1 kHz
            assert!(d.check_10k_db == 0.0 || (t - d.check_10k_db as f64).abs() < 0.25, "{}: 10 kHz {t:.2} vs table {}", d.named.id, d.check_10k_db);
        }
    }

    #[test]
    fn riaa_to_every_disc_curve_within_half_db() {
        for fs in [44100.0, 48000.0, 96000.0] {
            for d in disc_curves() {
                let s = correction(fs, &RIAA, &d.named.curve);
                for f in log_freqs() {
                    let got = 20.0 * cascade_magnitude(&s, fs, f).log10();
                    let want = target_db(&RIAA, &d.named.curve, f);
                    assert!((got - want).abs() <= 0.5, "{} at {fs} Hz, {f:.0} Hz: {got:.2} vs {want:.2}", d.named.id);
                }
            }
        }
    }

    #[test]
    fn tape_curves_convert_both_ways() {
        let fs = 96000.0;
        for a in tape_curves() {
            for b in tape_curves() {
                let s = correction(fs, &a.curve, &b.curve);
                for f in log_freqs() {
                    let got = if s.is_empty() { 0.0 } else { 20.0 * cascade_magnitude(&s, fs, f).log10() };
                    let want = target_db(&a.curve, &b.curve, f);
                    assert!((got - want).abs() <= 0.2, "{} -> {} at {f:.0}: {got:.2} vs {want:.2}", a.id, b.id);
                }
            }
        }
    }

    #[test]
    fn type_ii_played_at_120_gets_darker() {
        let a = find_curve("cassette-120").unwrap();
        let b = find_curve("cassette-70").unwrap();
        assert!(target_db(&a, &b, 10_000.0) < -3.0);
        assert!(correction(48000.0, &a, &a).is_empty());
    }

    #[test]
    fn deemphasis_shape() {
        let s = deemphasis_50_15(48000.0);
        let d = |f: f64| 20.0 * cascade_magnitude(&s, 48000.0, f).log10();
        assert!(d(100.0).abs() < 0.05);
        // analog: (1 + jw15u)/(1 + jw50u) at 10 kHz is about -6.4 dB
        let w = 2.0 * PI * 10_000.0;
        let want = 10.0 * ((1.0 + (w * 15e-6).powi(2)) / (1.0 + (w * 50e-6).powi(2))).log10();
        assert!((d(10_000.0) - want).abs() < 0.1, "{} vs {want}", d(10_000.0));
    }

    #[test]
    fn turnover_round_trip() {
        let c = Curve::from_turnover(300.0, -16.0);
        assert!((c.turnover_hz() - 300.0).abs() < 0.5);
        assert!((c.rolloff_db_10k() + 16.0).abs() < 0.05);
    }
}
