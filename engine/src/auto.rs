//! "Auto" buttons: analyze the audio and pick starting values. Also silence
//! splitting and run-out groove detection.

use crate::audio::SourceAudio;
use crate::chain::{Chain, SourceFx};
use crate::dsp::{declick, denoise};
use crate::project::SplitSettings;

fn rms_db(x: &[f32]) -> f32 {
    if x.is_empty() {
        return -120.0;
    }
    let s: f64 = x.iter().map(|v| (*v as f64) * (*v as f64)).sum();
    let r = (s / x.len() as f64).sqrt();
    if r <= 1e-9 {
        -120.0
    } else {
        (20.0 * r.log10()) as f32
    }
}

fn goertzel_power(x: &[f32], rate: u32, f: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * f / rate as f64;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &v in x {
        let s0 = v as f64 + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - coeff * s1 * s2) / (x.len() as f64).powi(2)
}

/// Mains hum: Some(50) or Some(60) if a hum line stands out, else None.
/// Looks at the quiet stretches (lead-in, gaps) where hum is easiest to hear.
pub fn detect_hum(src: &SourceAudio) -> Option<f32> {
    let rate = src.rate;
    let len = (rate as usize * 4).min(src.frames);
    if len < rate as usize {
        return None;
    }
    // Check a few 4 s windows spread across the side; use the quietest three.
    let mut wins: Vec<(f32, Vec<f32>)> = Vec::new();
    let n = 12;
    for i in 0..n {
        let start = (src.frames.saturating_sub(len)) * i / (n - 1).max(1);
        let x = src.read_mono(start as i64, (start + len) as i64);
        wins.push((rms_db(&x), x));
    }
    wins.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let mut score50 = 0.0;
    let mut score60 = 0.0;
    for (_, x) in wins.iter().take(3) {
        for (base, score) in [(50.0, &mut score50), (60.0, &mut score60)] {
            for k in 1..=3 {
                let f = base * k as f64;
                let p = goertzel_power(x, rate, f);
                let side = (goertzel_power(x, rate, f - 4.0) + goertzel_power(x, rate, f + 4.0)) / 2.0;
                if side > 0.0 {
                    *score += (p / side).log10().max(0.0);
                }
            }
        }
    }
    // Sum of log power ratios over three harmonics in three windows. 6.0 means
    // the hum lines average about 7 dB above their neighbours.
    let need = 6.0;
    if score60 >= score50 && score60 > need {
        Some(60.0)
    } else if score50 > need {
        Some(50.0)
    } else {
        None
    }
}

/// Find the quietest non-silent 1.5 s stretch in the lead-in or run-out and
/// learn a noise profile from it.
pub fn learn_noise(src: &SourceAudio) -> Option<denoise::NoiseProfile> {
    let rate = src.rate as usize;
    let win = rate * 3 / 2;
    if src.frames < win * 2 {
        return None;
    }
    let head = (rate * 25).min(src.frames);
    let tail_from = src.frames.saturating_sub(rate * 40);
    let mut best: Option<(f32, usize)> = None;
    let step = rate / 4;
    let mut consider = |s: usize| {
        if s + win > src.frames {
            return;
        }
        let x = src.read_mono(s as i64, (s + win) as i64);
        let db = rms_db(&x);
        // skip digital silence before the needle drops
        let zeros = x.iter().filter(|v| v.abs() < 1e-6).count();
        if db < -100.0 || zeros > win / 10 {
            return;
        }
        if best.map(|b| db < b.0).unwrap_or(true) {
            best = Some((db, s));
        }
    };
    let mut s = 0;
    while s + win <= head {
        consider(s);
        s += step;
    }
    let mut s = tail_from;
    while s + win <= src.frames {
        consider(s);
        s += step;
    }
    let (_, s) = best?;
    learn_noise_at(src, s, s + win)
}

pub fn learn_noise_at(src: &SourceAudio, a: usize, b: usize) -> Option<denoise::NoiseProfile> {
    let x = src.read_mono(a as i64, b as i64);
    denoise::learn_profile(&x, src.rate, a as f64 / src.rate as f64, b as f64 / src.rate as f64)
}

/// Right-channel time offset relative to left, microseconds, from cross-correlation
/// over the loudest stretch.
pub fn detect_azimuth(src: &SourceAudio) -> Option<f32> {
    if src.channels != 2 {
        return None;
    }
    let rate = src.rate as usize;
    let len = (rate * 5).min(src.frames);
    // loudest 5 s window among a few candidates
    let mut best = (f32::MIN, 0usize);
    for i in 0..10 {
        let s = src.frames.saturating_sub(len) * i / 9;
        let db = rms_db(&src.read_mono(s as i64, (s + len) as i64));
        if db > best.0 {
            best = (db, s);
        }
    }
    let l = src.read_channel(0, best.1 as i64, (best.1 + len) as i64);
    let r = src.read_channel(1, best.1 as i64, (best.1 + len) as i64);
    let maxlag = (rate / 1000) as i64; // 1 ms
    let corr = |lag: i64| -> f64 {
        let mut s = 0.0f64;
        let n = l.len() as i64;
        for i in maxlag..n - maxlag {
            s += l[i as usize] as f64 * r[(i + lag) as usize] as f64;
        }
        s
    };
    let vals: Vec<f64> = (-maxlag..=maxlag).map(corr).collect();
    let (mut bi, mut bv) = (0usize, f64::MIN);
    for (i, v) in vals.iter().enumerate() {
        if *v > bv {
            bv = *v;
            bi = i;
        }
    }
    if bv <= 0.0 {
        return None;
    }
    let mut lag = bi as f64 - maxlag as f64;
    if bi > 0 && bi + 1 < vals.len() {
        let (a, b, c) = (vals[bi - 1], vals[bi], vals[bi + 1]);
        let d = a - 2.0 * b + c;
        if d.abs() > 1e-12 {
            lag += 0.5 * (a - c) / d;
        }
    }
    // R lags L by `lag` samples; correcting means delaying R by -lag
    Some((-lag / rate as f64 * 1e6) as f32)
}

pub fn detect_clipping(src: &SourceAudio) -> Option<f32> {
    let x = src.read_mono(0, src.frames as i64);
    declick::detect_clipping(&x).map(|thr| 20.0 * thr.log10())
}

/// Click density per second at a default threshold, on a 20 s sample.
pub fn click_density(src: &SourceAudio) -> f32 {
    let rate = src.rate as usize;
    let len = (rate * 20).min(src.frames);
    let s = src.frames.saturating_sub(len) / 2;
    let mut x = src.read_channel(0, s as i64, (s + len) as i64);
    let order = (rate / 2000).clamp(16, 48);
    let clicks = declick::process(&mut x, s as i64, &declick::ClickParams { threshold_db: 13.0, max_len: rate / 500, order, passes: 1 });
    clicks.len() as f32 / (len as f32 / rate as f32)
}

/// Sub-sonic energy relative to the low bass, dB. High values mean rumble.
pub fn rumble_ratio(src: &SourceAudio) -> f32 {
    let rate = src.rate;
    let len = (rate as usize * 10).min(src.frames);
    let s = src.frames.saturating_sub(len) / 2;
    let x = src.read_mono(s as i64, (s + len) as i64);
    let sub: f64 = [8.0, 12.0, 16.0].iter().map(|f| goertzel_power(&x, rate, *f)).sum();
    let bass: f64 = [50.0, 70.0, 90.0].iter().map(|f| goertzel_power(&x, rate, *f)).sum();
    if bass <= 0.0 {
        return 0.0;
    }
    (10.0 * (sub / bass).log10()) as f32
}

/// Fill in starting values for a freshly added raw source.
pub fn auto_source(src: &SourceAudio, fx: &mut SourceFx, chain: &mut Chain) {
    if fx.noise.is_none() {
        fx.noise = learn_noise(src);
    }
    match detect_hum(src) {
        Some(f) => {
            chain.hum.enabled = true;
            chain.hum.mains_hz = f;
        }
        None => chain.hum.enabled = false,
    }
    if let Some(level) = detect_clipping(src) {
        chain.declip.enabled = true;
        chain.declip.level_db = level;
    }
}

pub fn auto_declick(src: &SourceAudio, chain: &mut Chain) {
    let d = click_density(src);
    chain.declick.enabled = true;
    // Busy records get a slightly lower threshold and longer clicks allowed.
    if d > 20.0 {
        chain.declick.threshold_db = 11.5;
        chain.declick.max_len_ms = 3.0;
    } else if d > 5.0 {
        chain.declick.threshold_db = 12.5;
        chain.declick.max_len_ms = 2.5;
    } else {
        chain.declick.threshold_db = 13.5;
        chain.declick.max_len_ms = 2.0;
    }
}

pub fn auto_decrackle(src: &SourceAudio, chain: &mut Chain) {
    let d = click_density(src);
    chain.decrackle.enabled = d > 2.0;
    chain.decrackle.threshold_db = if d > 30.0 { 9.5 } else { 10.5 };
    chain.decrackle.max_len_ms = 0.25;
}

pub fn auto_rumble(src: &SourceAudio, chain: &mut Chain) {
    let r = rumble_ratio(src);
    chain.rumble.enabled = true;
    chain.rumble.freq_hz = if r > 0.0 { 30.0 } else { 20.0 };
    chain.rumble.slope_db_oct = if r > 0.0 { 36 } else { 24 };
}

pub fn auto_denoise(fx: &SourceFx, chain: &mut Chain) {
    chain.denoise.enabled = fx.noise.is_some();
    if let Some(p) = &fx.noise {
        // Gentle by default: more reduction only for a clearly noisy surface.
        let lvl = p.level_db();
        chain.denoise.reduction_db = ((lvl + 78.0) / 2.5).clamp(3.0, 10.0).round();
        chain.denoise.smoothing_pct = 50.0;
    }
}

/// Split threshold: 10 dB above the surface noise between songs.
pub fn auto_split_threshold(src: &SourceAudio) -> f32 {
    let rate = src.rate as usize;
    let w = rate / 20;
    let mut vals = Vec::new();
    let mut i = 0;
    while i + w <= src.frames {
        let x = src.read_mono(i as i64, (i + w) as i64);
        let db = rms_db(&x);
        if db > -100.0 {
            vals.push(db);
        }
        i += w * 4;
    }
    if vals.is_empty() {
        return -45.0;
    }
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (vals[vals.len() / 20] + 10.0).clamp(-70.0, -25.0)
}

/// Auto-split on silence. Returns (start_s, end_s) in source seconds.
pub fn split_on_silence(src: &SourceAudio, s: &SplitSettings) -> Vec<(f64, f64)> {
    let rate = src.rate as usize;
    let w = rate / 20; // 50 ms windows
    let nw = src.frames / w;
    if nw < 4 {
        return vec![(0.0, src.duration())];
    }
    let loud: Vec<bool> = (0..nw)
        .map(|i| {
            let x = src.read_mono((i * w) as i64, ((i + 1) * w) as i64);
            // drop isolated clicks from the level measure: use the 90th percentile of |x|
            let mut a: Vec<f32> = x.iter().map(|v| v.abs()).collect();
            let k = a.len() * 9 / 10;
            a.select_nth_unstable_by(k, |p, q| p.partial_cmp(q).unwrap());
            let p90 = a[k] * 0.9; // roughly RMS-like for noise
            20.0 * p90.max(1e-9).log10() > s.threshold_db
        })
        .collect();
    let min_gap = ((s.min_gap_s.max(0.2) as f64 * 20.0).round() as usize).max(1);
    // music runs, ignoring short dips
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < nw {
        if loud[i] {
            let st = i;
            let mut en = i;
            let mut j = i;
            while j < nw {
                if loud[j] {
                    en = j;
                    j += 1;
                } else {
                    let mut k = j;
                    while k < nw && !loud[k] {
                        k += 1;
                    }
                    if k - j >= min_gap || k == nw {
                        break;
                    }
                    j = k;
                }
            }
            runs.push((st, en + 1));
            i = en + 1;
        } else {
            i += 1;
        }
    }
    // drop runs shorter than 5 s (a stray thump in the run-out)
    runs.retain(|(a, b)| (b - a) * w >= rate * 5);
    if runs.is_empty() {
        return vec![(0.0, src.duration())];
    }
    let to_s = |win: usize| (win * w) as f64 / rate as f64;
    let mut out = Vec::new();
    for (idx, &(a, b)) in runs.iter().enumerate() {
        let start = if idx == 0 {
            (to_s(a) - 0.3).max(0.0)
        } else {
            let prev_end = runs[idx - 1].1;
            (to_s(prev_end) + to_s(a)) / 2.0
        };
        let end = if idx + 1 == runs.len() {
            (to_s(b) + 2.0).min(src.duration())
        } else {
            (to_s(b) + to_s(runs[idx + 1].0)) / 2.0
        };
        out.push((start, end));
    }
    out
}

/// Snap boundaries to a list of track lengths (from a tracklist lookup): each
/// boundary moves to the quietest point within +-6 s of where the lengths put it.
pub fn snap_to_lengths(src: &SourceAudio, first_start: f64, lengths: &[f64]) -> Vec<(f64, f64)> {
    let rate = src.rate as usize;
    let w = rate / 20;
    let level = |t: f64| -> f32 {
        let a = (t * rate as f64) as i64;
        rms_db(&src.read_mono(a, a + w as i64))
    };
    let mut bounds = vec![first_start];
    let mut t = first_start;
    for (i, l) in lengths.iter().enumerate() {
        t += l;
        if i + 1 == lengths.len() {
            bounds.push((t + 1.0).min(src.duration()));
            break;
        }
        let mut best = (f32::MAX, t);
        let mut c = (t - 6.0).max(*bounds.last().unwrap() + 1.0);
        while c <= (t + 6.0).min(src.duration()) {
            let v = level(c);
            if v < best.0 {
                best = (v, c);
            }
            c += 0.05;
        }
        t = best.1;
        bounds.push(t);
    }
    bounds.windows(2).map(|p| (p[0], p[1])).collect()
}

/// Watches a capture for the run-out groove: a quiet, repeating loop once per
/// revolution, or plain quiet, lasting `hold_s` seconds.
pub struct RunoutDetector {
    rate: u32,
    hold_s: f32,
    env: Vec<f32>,
    acc: f64,
    acc_n: usize,
    hop: usize,
    loudest: f32,
    quiet_hops: usize,
}

impl RunoutDetector {
    pub fn new(rate: u32, hold_s: f32) -> Self {
        RunoutDetector { rate, hold_s, env: Vec::new(), acc: 0.0, acc_n: 0, hop: rate as usize / 100, loudest: -120.0, quiet_hops: 0 }
    }

    /// Feed mono samples. Returns true once the run-out has been heard for long enough.
    pub fn feed(&mut self, x: &[f32]) -> bool {
        let mut hit = false;
        for &v in x {
            self.acc += (v as f64) * (v as f64);
            self.acc_n += 1;
            if self.acc_n == self.hop {
                let db = (10.0 * (self.acc / self.hop as f64).max(1e-12).log10()) as f32;
                self.acc = 0.0;
                self.acc_n = 0;
                self.env.push(db);
                if self.env.len() > 1500 {
                    self.env.drain(..500);
                }
                // music level estimate: slow peak follower
                self.loudest = if db > self.loudest { db } else { self.loudest - 0.0005 };
                // "quiet" = 30 dB below the music, measured on the median of the last 0.5 s
                let tail = &self.env[self.env.len().saturating_sub(50)..];
                let mut t: Vec<f32> = tail.to_vec();
                t.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let med = t[t.len() / 2];
                if self.loudest > -50.0 && med < self.loudest - 30.0 {
                    self.quiet_hops += 1;
                } else {
                    self.quiet_hops = 0;
                }
                let quiet_s = self.quiet_hops as f32 / 100.0;
                if quiet_s >= self.hold_s {
                    hit = true;
                } else if quiet_s >= self.hold_s.min(6.0) && self.loops() {
                    // a clear once-per-revolution pattern ends it sooner
                    hit = true;
                }
            }
        }
        hit
    }

    fn loops(&self) -> bool {
        // correlate the envelope with itself at one revolution (33 1/3: 1.8 s, 45: 1.333 s)
        let e = &self.env;
        if e.len() < 500 {
            return false;
        }
        let tail = &e[e.len() - 500..];
        let mean = tail.iter().sum::<f32>() / tail.len() as f32;
        let d: Vec<f32> = tail.iter().map(|v| v - mean).collect();
        let var: f32 = d.iter().map(|v| v * v).sum();
        if var <= 1e-6 {
            return false;
        }
        [180usize, 133].iter().any(|&lag| {
            let c: f32 = (lag..d.len()).map(|i| d[i] * d[i - lag]).sum();
            c / var > 0.5
        })
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }
}
