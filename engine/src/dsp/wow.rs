//! Wow and flutter: tracking a stable reference in the recording, turning it
//! into a speed curve, and the time map the speed stage reads through.
//!
//! The reference is either a named tone (test tapes, alignment tones) or mains
//! hum that went onto the tape when it was recorded. The signal is mixed down
//! by the nominal frequency, low-passed and decimated, the exact offset found
//! by an FFT, then a narrow zero-phase low-pass leaves just the reference as a
//! slowly turning phasor. Its phase rate is the instantaneous frequency: a
//! narrow phase-locked tracker, done offline so it has no lag.
//!
//! Hum added at capture by a ground loop is rock steady at the mains frequency
//! and says nothing about the tape; `classify` spots that.

use super::biquad;
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

/// Curve resolution in seconds.
pub const HOP_S: f64 = 0.01;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FreqTrack {
    /// Source seconds of the first point; points are HOP_S apart.
    pub t0: f64,
    /// Frequency of the reference, Hz.
    pub freq: Vec<f32>,
    /// Its level, dBFS RMS.
    pub level_db: Vec<f32>,
    pub nominal: f64,
}

impl FreqTrack {
    pub fn mean(&self) -> f64 {
        if self.freq.is_empty() {
            return self.nominal;
        }
        self.freq.iter().map(|f| *f as f64).sum::<f64>() / self.freq.len() as f64
    }

    /// Peak deviation from the mean, percent.
    pub fn peak_dev_pct(&self) -> f64 {
        let m = self.mean();
        self.freq.iter().map(|f| ((*f as f64 / m) - 1.0).abs()).fold(0.0, f64::max) * 100.0
    }

    pub fn std_dev_pct(&self) -> f64 {
        let m = self.mean();
        let n = self.freq.len().max(1) as f64;
        (self.freq.iter().map(|f| ((*f as f64 / m) - 1.0).powi(2)).sum::<f64>() / n).sqrt() * 100.0
    }

    /// Mean level, dBFS.
    pub fn level(&self) -> f32 {
        if self.level_db.is_empty() {
            -120.0
        } else {
            self.level_db.iter().sum::<f32>() / self.level_db.len() as f32
        }
    }
}

/// Track a reference near `nominal` Hz within +-`search` (fraction, e.g. 0.06),
/// following frequency changes up to `bw` Hz. None if nothing usable is there.
pub fn track(x: &[f32], rate: u32, nominal: f64, search: f64, bw: f64) -> Option<FreqTrack> {
    track_blocks(x.len(), &mut |a, b| x[a..b].to_vec(), rate, nominal, search, bw)
}

/// `track` over audio read a block at a time, so an hour of tape never sits
/// in memory: `read(a, b)` returns samples [a, b) of a mono signal `len` long.
pub fn track_blocks(len: usize, read: &mut dyn FnMut(usize, usize) -> Vec<f32>, rate: u32, nominal: f64, search: f64, bw: f64) -> Option<FreqTrack> {
    let fs = rate as f64;
    let span = nominal * search + bw;
    let drate = (8.0 * span).max(200.0).min(fs / 4.0);
    let dec = ((fs / drate).floor() as usize).max(1);
    let drate = fs / dec as f64;
    if len < dec * 160 {
        return None;
    }
    // 1. mix down and low-pass at the full rate, keep every dec-th sample
    let w0 = 2.0 * std::f64::consts::PI * nominal / fs;
    let mut lpi = biquad::butter_lowpass(fs, span.min(drate * 0.4), 4);
    let mut lpq = lpi.clone();
    // this first low-pass runs forwards only; its delay is taken off the times
    let lag_s = lpi.iter().map(|s| s.group_delay_dc()).sum::<f64>() / fs;
    let mut z: Vec<Complex<f64>> = Vec::with_capacity(len / dec + 1);
    let (mut c, mut s) = (1.0f64, 0.0f64);
    let (cw, sw) = (w0.cos(), w0.sin());
    let block = (rate as usize * 10).max(dec);
    let mut at = 0usize;
    let mut chunk: Vec<f32> = Vec::new();
    for n in 0..len {
        if n == at + chunk.len() {
            at = n;
            chunk = read(n, (n + block).min(len));
        }
        let v = chunk[n - at] as f64;
        let mut i = v * c;
        let mut q = -v * s;
        for f in lpi.iter_mut() {
            i = f.tick(i);
        }
        for f in lpq.iter_mut() {
            q = f.tick(q);
        }
        if n % dec == 0 {
            z.push(Complex::new(i, q));
        }
        // rotate the oscillator, renormalizing now and then
        let nc = c * cw - s * sw;
        s = s * cw + c * sw;
        c = nc;
        if n % 4096 == 0 {
            let m = (c * c + s * s).sqrt();
            c /= m;
            s /= m;
        }
    }
    // 2. find the offset: FFT of the decimated phasor
    // filter settling: the narrow tracking filter needs a few of its periods
    let skip = ((drate * (3.0 / bw).max(0.5)) as usize).min(z.len() / 4);
    if z.len() < skip * 2 + 64 {
        return None;
    }
    let body = &z[skip..];
    let nfft = body.len().next_power_of_two().min(1 << 20);
    let mut buf: Vec<Complex<f64>> = vec![Complex::new(0.0, 0.0); nfft];
    let take = body.len().min(nfft);
    for i in 0..take {
        let h = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / take as f64).cos();
        buf[i] = body[i] * h;
    }
    FftPlanner::new().plan_fft_forward(nfft).process(&mut buf);
    let lim = nominal * search;
    let mut best = (0.0f64, 0.0f64);
    for (k, v) in buf.iter().enumerate() {
        let f = if k <= nfft / 2 { k as f64 } else { k as f64 - nfft as f64 } * drate / nfft as f64;
        if f.abs() <= lim && v.norm() > best.1 {
            best = (f, v.norm());
        }
    }
    let off = best.0;
    // 3. shift to the found frequency, narrow zero-phase low-pass
    let wd = -2.0 * std::f64::consts::PI * off / drate;
    let mut zi: Vec<f32> = Vec::with_capacity(z.len());
    let mut zq: Vec<f32> = Vec::with_capacity(z.len());
    for (n, v) in z.iter().enumerate() {
        let r = Complex::from_polar(1.0, wd * n as f64);
        let u = v * r;
        zi.push(u.re as f32);
        zq.push(u.im as f32);
    }
    // steep, so a loud tone a few hertz away (bass near the hum) cannot wobble
    // the phase
    let nlp = biquad::butter_lowpass(drate, bw.min(drate * 0.4), 8);
    let wide: Vec<f32> = zi.iter().zip(&zq).map(|(a, b)| a * a + b * b).collect();
    biquad::run_cascade_zero_phase(&nlp, &mut zi);
    biquad::run_cascade_zero_phase(&nlp, &mut zq);
    // 4. phase rate -> frequency, averaged onto the HOP_S grid
    let hop = ((HOP_S * drate).round() as usize).max(1);
    let n = zi.len();
    let mut freq = Vec::new();
    let mut level = Vec::new();
    let mut prev = (zq[skip] as f64).atan2(zi[skip] as f64);
    let mut acc_f = 0.0f64;
    let mut acc_l = 0.0f64;
    let mut acc_w = 0.0f64;
    let mut cnt = 0usize;
    // per hop: how much of what came through the first low-pass is not the
    // reference (a loud tone near it, splatter from a transient)
    let mut interference: Vec<f32> = Vec::new();
    let end = n.saturating_sub(skip);
    for k in skip + 1..end {
        let ph = (zq[k] as f64).atan2(zi[k] as f64);
        let mut d = ph - prev;
        while d > std::f64::consts::PI {
            d -= 2.0 * std::f64::consts::PI;
        }
        while d < -std::f64::consts::PI {
            d += 2.0 * std::f64::consts::PI;
        }
        prev = ph;
        acc_f += d;
        acc_l += (zi[k] as f64).powi(2) + (zq[k] as f64).powi(2);
        acc_w += wide[k] as f64;
        cnt += 1;
        if cnt == hop {
            interference.push(((acc_w - acc_l).max(0.0) / acc_l.max(1e-30)) as f32);
            acc_w = 0.0;
            freq.push((nominal + off + acc_f / cnt as f64 * drate / (2.0 * std::f64::consts::PI)) as f32);
            // a real tone of amplitude A mixes down to a phasor of amplitude A/2
            let a = 2.0 * (acc_l / cnt as f64).sqrt();
            level.push((20.0 * (a / std::f64::consts::SQRT_2).max(1e-9).log10()) as f32);
            acc_f = 0.0;
            acc_l = 0.0;
            cnt = 0;
        }
    }
    if freq.len() < 10 {
        return None;
    }
    clean_track(&mut freq, &level, &interference);
    Some(FreqTrack { t0: (skip + 1) as f64 / drate + hop as f64 / drate / 2.0 - lag_s, freq, level_db: level, nominal })
}

/// Drop the points where the reference cannot be trusted and bridge them from
/// their neighbours: where its level falls 8 dB under its median (a dropout,
/// a mute), where something else sits close to it (a loud tone a few hertz
/// away, splatter from a transient), and where the frequency jumps away from
/// the local trend by more than a speed change could.
fn clean_track(freq: &mut [f32], level: &[f32], interference: &[f32]) {
    let n = freq.len();
    let mut lv = level.to_vec();
    lv.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med_level = lv[n / 2];
    // something as strong as the reference right next to it
    let mut good: Vec<bool> = level.iter().zip(interference).map(|(l, i)| *l > med_level - 8.0 && *i < 0.5).collect();
    // local quadratic fit over +-0.25 s of the good points; a point far off the
    // fit is a glitch, not a speed change (speed changes are smooth at this scale)
    let half = 25i64;
    let fit = |good: &[bool]| -> Vec<f32> {
        (0..n)
            .map(|i| {
                let (mut s0, mut s1, mut s2, mut s3, mut s4) = (0.0f64, 0.0, 0.0, 0.0, 0.0);
                let (mut t0, mut t1, mut t2) = (0.0f64, 0.0, 0.0);
                for k in (i as i64 - half).max(0)..(i as i64 + half + 1).min(n as i64) {
                    let k = k as usize;
                    if !good[k] {
                        continue;
                    }
                    let x = (k as f64 - i as f64) / half as f64;
                    let y = freq[k] as f64;
                    s0 += 1.0;
                    s1 += x;
                    s2 += x * x;
                    s3 += x * x * x;
                    s4 += x * x * x * x;
                    t0 += y;
                    t1 += x * y;
                    t2 += x * x * y;
                }
                if s0 < 6.0 {
                    return freq[i];
                }
                // solve the 3x3 normal equations for the value at x = 0
                let m = [[s0, s1, s2], [s1, s2, s3], [s2, s3, s4]];
                let det = |a: [[f64; 3]; 3]| a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
                let d = det(m);
                if d.abs() < 1e-12 {
                    return (t0 / s0) as f32;
                }
                let mut m0 = m;
                m0[0][0] = t0;
                m0[1][0] = t1;
                m0[2][0] = t2;
                (det(m0) / d) as f32
            })
            .collect()
    };
    for _ in 0..2 {
        let trend = fit(&good);
        let mut res: Vec<f32> = (0..n).filter(|&i| good[i]).map(|i| (freq[i] - trend[i]).abs()).collect();
        if res.is_empty() {
            return;
        }
        let k = res.len() / 2;
        let mad = *res.select_nth_unstable_by(k, |a, b| a.partial_cmp(b).unwrap()).1;
        let mean = freq.iter().sum::<f32>() / n as f32;
        let lim = (6.0 * mad).max(mean * 0.0002);
        for i in 0..n {
            if (freq[i] - trend[i]).abs() > lim {
                good[i] = false;
            }
        }
    }
    // widen every bad stretch by 2 points (filter edges)
    let bad: Vec<bool> = (0..n).map(|i| (i.saturating_sub(2)..(i + 3).min(n)).any(|k| !good[k])).collect();
    let mut i = 0;
    while i < n {
        if !bad[i] {
            i += 1;
            continue;
        }
        let a = i;
        while i < n && bad[i] {
            i += 1;
        }
        let left = if a > 0 { Some(freq[a - 1]) } else { None };
        let right = if i < n { Some(freq[i]) } else { None };
        let (l, r) = match (left, right) {
            (Some(l), Some(r)) => (l, r),
            (Some(l), None) => (l, l),
            (None, Some(r)) => (r, r),
            (None, None) => return,
        };
        let len = (i - a + 1) as f32;
        for (k, f) in freq[a..i].iter_mut().enumerate() {
            *f = l + (r - l) * (k + 1) as f32 / len;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RefKind {
    /// Moves with the tape: usable.
    Tape,
    /// Rock steady at mains: from the capture chain.
    CaptureChain,
    /// Too weak or too noisy to use.
    Unusable,
}

pub fn classify(t: &FreqTrack, mains_hz: f64) -> RefKind {
    if t.level() < -85.0 {
        return RefKind::Unusable;
    }
    let mean = t.mean();
    let sd = t.std_dev_pct();
    if (mean / mains_hz - 1.0).abs() < 1e-4 && sd < 0.002 {
        return RefKind::CaptureChain;
    }
    if sd > 3.0 {
        return RefKind::Unusable;
    }
    RefKind::Tape
}

/// The speed stage's view of a wow curve: where in the source each moment of
/// corrected time is read from.
///
/// With v the source position the static speed alone would read (processed
/// index times the static ratio), the actual read position is Phi(v), where
/// dPhi/dv = w(Phi) and w is the inverse of the relative tape speed,
/// normalized to a mean of 1 so the length stays what the static speed gives.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WowMap {
    /// Grid step in source samples.
    pub step: f64,
    /// Phi at v = k * step.
    pub phi: Vec<f64>,
}

impl WowMap {
    /// Build from a reference track (frequencies over source time).
    pub fn from_track(t: &FreqTrack, rate: u32) -> WowMap {
        let fs = rate as f64;
        let mean = t.mean();
        let inv: Vec<f64> = t.freq.iter().map(|f| mean / (*f as f64).max(1e-6)).collect();
        let m = inv.iter().sum::<f64>() / inv.len().max(1) as f64;
        let w: Vec<f64> = inv.iter().map(|v| v / m).collect();
        let step = HOP_S * fs;
        let t0 = t.t0 * fs;
        let t1 = t0 + w.len() as f64 * step;
        let w_at = |p: f64| -> f64 {
            if p < t0 || p >= t1 {
                return 1.0;
            }
            let k = (p - t0) / step;
            let i = k.floor() as usize;
            let f = k - i as f64;
            let a = w[i.min(w.len() - 1)];
            let b = w[(i + 1).min(w.len() - 1)];
            a + (b - a) * f
        };
        let n = ((t1 / step).ceil() as usize) + 2;
        let mut phi = Vec::with_capacity(n);
        let mut p = 0.0f64;
        for _ in 0..n {
            phi.push(p);
            // midpoint step
            let mid = p + 0.5 * step * w_at(p);
            p += step * w_at(mid);
        }
        WowMap { step, phi }
    }

    pub fn is_empty(&self) -> bool {
        self.phi.len() < 2
    }

    /// Read position for static position v (source samples).
    #[inline]
    pub fn at(&self, v: f64) -> f64 {
        let n = self.phi.len();
        if n < 2 || v <= 0.0 {
            return v + self.phi.first().copied().unwrap_or(0.0);
        }
        let k = v / self.step;
        let i = k.floor() as usize;
        if i + 1 >= n {
            return self.phi[n - 1] + (v - (n - 1) as f64 * self.step);
        }
        let f = k - i as f64;
        self.phi[i] + (self.phi[i + 1] - self.phi[i]) * f
    }

    /// Static position whose read position is p (the inverse of `at`).
    pub fn inverse(&self, p: f64) -> f64 {
        let n = self.phi.len();
        if n < 2 || p <= self.phi[0] {
            return p - self.phi.first().copied().unwrap_or(0.0);
        }
        if p >= self.phi[n - 1] {
            return (n - 1) as f64 * self.step + (p - self.phi[n - 1]);
        }
        let i = self.phi.partition_point(|&x| x <= p).max(1) - 1;
        let f = (p - self.phi[i]) / (self.phi[i + 1] - self.phi[i]).max(1e-12);
        (i as f64 + f) * self.step
    }
}

/// Weighted wow and flutter from a 3150 Hz reference, percent, two-sigma.
///
/// The weighting is a band-pass peaking at 4 Hz, where speed changes are most
/// audible, falling away below 0.5 Hz and above 50 Hz: an approximation of the
/// IEC 60386 curve, whose exact table was not available. Treat the figure as
/// close to, not certified as, an IEC 60386 reading.
pub fn weighted_wow_flutter(t: &FreqTrack) -> f64 {
    let fs = 1.0 / HOP_S;
    let m = t.mean();
    let mut d: Vec<f32> = t.freq.iter().map(|f| ((*f as f64 / m) - 1.0) as f32).collect();
    if d.len() < 64 {
        return 0.0;
    }
    let mut hp = biquad::butter_highpass(fs, 0.5, 2);
    biquad::run_cascade(&mut hp, &mut d);
    let mut pk = biquad::Biquad::peaking(fs, 4.0, 0.7, 0.0);
    let mut lp = biquad::butter_lowpass(fs, 40.0f64.min(fs * 0.45), 1);
    pk.run(&mut d);
    biquad::run_cascade(&mut lp, &mut d);
    let skip = d.len() / 10;
    let s = &d[skip..];
    let sd = (s.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / s.len() as f64).sqrt();
    2.0 * sd * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_hum_wow() {
        let rate = 48000u32;
        let secs = 12.0;
        let n = (rate as f64 * secs) as usize;
        // 61.8 Hz (3% fast) carrying 0.3% wow at 0.5 Hz
        let mut ph = 0.0f64;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f64 / rate as f64;
                let f = 61.8 * (1.0 + 0.003 * (2.0 * std::f64::consts::PI * 0.5 * t).sin());
                ph += 2.0 * std::f64::consts::PI * f / rate as f64;
                (0.01 * ph.sin()) as f32
            })
            .collect();
        let t = track(&x, rate, 60.0, 0.06, 4.0).expect("track");
        assert!((t.mean() - 61.8).abs() < 0.01, "mean {}", t.mean());
        assert!((t.peak_dev_pct() - 0.3).abs() < 0.03, "peak dev {}", t.peak_dev_pct());
        assert_eq!(classify(&t, 60.0), RefKind::Tape);
        let m = WowMap::from_track(&t, rate);
        let v = 5.0 * rate as f64;
        assert!((m.inverse(m.at(v)) - v).abs() < 1e-3);
    }

    #[test]
    fn steady_hum_is_capture_chain() {
        let rate = 48000u32;
        let x: Vec<f32> = (0..rate as usize * 8).map(|i| (0.01 * (2.0 * std::f64::consts::PI * 60.0 * i as f64 / rate as f64).sin()) as f32).collect();
        let t = track(&x, rate, 60.0, 0.06, 4.0).expect("track");
        assert_eq!(classify(&t, 60.0), RefKind::CaptureChain);
    }
}
