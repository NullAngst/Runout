//! "Auto" buttons: analyze the audio and pick starting values. Also splitting
//! (silence, noise floor, program changes, fixed length), the capture stop
//! detectors, reference tones, best channel, and the tape measurements
//! (wow curve, azimuth curve) Analyze stores.

use crate::analysis::ToneInfo;
use crate::audio::SourceAudio;
use crate::chain::{Chain, SourceFx, WowRef};
use crate::dsp::{azimuth, declick, denoise, wow};
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

/// Something that watches a capture and says when to stop.
pub trait StopDetector: Send {
    /// Feed mono samples. Returns true once it is time to stop.
    fn feed(&mut self, x: &[f32]) -> bool;
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
    /// One revolution, in 10 ms hops, for each speed it listens for.
    lags: Vec<usize>,
}

/// One revolution at `rpm`, in 10 ms envelope hops: 180 at 33 1/3, 133 at 45,
/// 77 at 78.26.
pub fn revolution_hops(rpm: f32) -> usize {
    (6000.0 / rpm.max(1.0) as f64).round() as usize
}

impl RunoutDetector {
    /// Listens for 33 1/3 and 45.
    pub fn new(rate: u32, hold_s: f32) -> Self {
        Self::for_speeds(rate, hold_s, &[100.0 / 3.0, 45.0])
    }

    pub fn for_speeds(rate: u32, hold_s: f32, rpms: &[f32]) -> Self {
        let mut lags: Vec<usize> = rpms.iter().map(|r| revolution_hops(*r)).collect();
        lags.dedup();
        RunoutDetector { rate, hold_s, env: Vec::new(), acc: 0.0, acc_n: 0, hop: rate as usize / 100, loudest: -120.0, quiet_hops: 0, lags }
    }

    pub fn lags(&self) -> &[usize] {
        &self.lags
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
        self.lags.iter().any(|&lag| {
            let c: f32 = (lag..d.len()).map(|i| d[i] * d[i - lag]).sum();
            c / var > 0.5
        })
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }
}

impl StopDetector for RunoutDetector {
    fn feed(&mut self, x: &[f32]) -> bool {
        RunoutDetector::feed(self, x)
    }
}

/// Stops when the signal sits below the tape's hiss floor for `hold_s`: a
/// stopped deck is quieter than blank tape. The floor is the quietest level
/// heard while the tape was running.
pub struct TapeEndDetector {
    hold_s: f32,
    hop: usize,
    acc: f64,
    acc_n: usize,
    env: Vec<f32>,
    floor: f32,
    heard_s: f32,
    below: usize,
}

impl TapeEndDetector {
    pub fn new(rate: u32, hold_s: f32) -> Self {
        TapeEndDetector { hold_s, hop: rate as usize / 100, acc: 0.0, acc_n: 0, env: Vec::new(), floor: 0.0, heard_s: 0.0, below: 0 }
    }
}

impl StopDetector for TapeEndDetector {
    fn feed(&mut self, x: &[f32]) -> bool {
        let mut hit = false;
        for &v in x {
            self.acc += (v as f64) * (v as f64);
            self.acc_n += 1;
            if self.acc_n < self.hop {
                continue;
            }
            let db = (10.0 * (self.acc / self.hop as f64).max(1e-14).log10()) as f32;
            self.acc = 0.0;
            self.acc_n = 0;
            self.env.push(db);
            if self.env.len() > 100 {
                self.env.remove(0);
            }
            // half-second median
            let mut t = self.env[self.env.len().saturating_sub(50)..].to_vec();
            t.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let med = t[t.len() / 2];
            self.heard_s += 0.01;
            // the floor follows the quietest running level, slowly
            if self.heard_s < 2.0 {
                self.floor = med;
            } else if med > -100.0 {
                self.floor = self.floor.min(med + 0.5);
            }
            if self.heard_s > 5.0 && med < self.floor - 4.0 {
                self.below += 1;
            } else {
                self.below = 0;
            }
            if self.below as f32 / 100.0 >= self.hold_s {
                hit = true;
            }
        }
        hit
    }
}

/// Stops after `hold_s` of exact digital silence.
pub struct DigitalSilenceDetector {
    rate: u32,
    hold_s: f32,
    zeros: usize,
    heard: bool,
}

impl DigitalSilenceDetector {
    pub fn new(rate: u32, hold_s: f32) -> Self {
        DigitalSilenceDetector { rate, hold_s, zeros: 0, heard: false }
    }
}

impl StopDetector for DigitalSilenceDetector {
    fn feed(&mut self, x: &[f32]) -> bool {
        for &v in x {
            if v == 0.0 {
                self.zeros += 1;
            } else {
                self.zeros = 0;
                self.heard = true;
            }
        }
        self.heard && self.zeros as f32 >= self.hold_s * self.rate as f32
    }
}

/// The stop detector a medium's capture uses.
pub fn stop_detector(rule: crate::media::StopRule, rate: u32, hold_s: f32, rpms: &[f32]) -> Option<Box<dyn StopDetector>> {
    use crate::media::StopRule;
    match rule {
        StopRule::Runout => Some(Box::new(if rpms.is_empty() { RunoutDetector::new(rate, hold_s) } else { RunoutDetector::for_speeds(rate, hold_s, rpms) })),
        StopRule::TapeEnd => Some(Box::new(TapeEndDetector::new(rate, hold_s))),
        StopRule::DigitalSilence => Some(Box::new(DigitalSilenceDetector::new(rate, hold_s))),
        StopRule::Never => None,
    }
}

/// The tape's hiss floor, dBFS: the quiet end of the 50 ms levels, ignoring
/// digital silence.
pub fn hiss_floor_db(src: &SourceAudio) -> f32 {
    let rate = src.rate as usize;
    let w = rate / 20;
    let mut vals = Vec::new();
    let mut i = 0;
    while i + w <= src.frames {
        let x = src.read_mono(i as i64, (i + w) as i64);
        let db = rms_db(&x);
        if db > -110.0 {
            vals.push(db);
        }
        i += w * 2;
    }
    if vals.is_empty() {
        return -90.0;
    }
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    vals[vals.len() / 20]
}

/// Split on silence measured from the hiss floor, since a fixed level fails on
/// tape: the floor moves from tape to tape.
pub fn split_noise_floor(src: &SourceAudio, s: &SplitSettings) -> Vec<(f64, f64)> {
    let floor = hiss_floor_db(src);
    let settings = SplitSettings { threshold_db: floor + s.floor_margin_db.max(1.0), min_gap_s: s.min_gap_s, ..s.clone() };
    split_on_silence(src, &settings)
}

/// Fixed-length chapters.
pub fn split_fixed(src: &SourceAudio, minutes: f32) -> Vec<(f64, f64)> {
    let len = (minutes.max(1.0) as f64) * 60.0;
    let dur = src.duration();
    let mut out = Vec::new();
    let mut t = 0.0;
    while t < dur - 0.5 {
        out.push((t, (t + len).min(dur)));
        t += len;
    }
    if out.is_empty() {
        out.push((0.0, dur));
    }
    out
}

/// An 8-track program change: a short mute with a mechanical transient in it.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ProgramChange {
    /// Source seconds: where the mute starts and ends.
    pub start_s: f64,
    pub end_s: f64,
    /// Music runs right up to it and straight on after: a song spans it.
    pub spans_song: bool,
}

/// Find 8-track program changes: the level drops sharply for 0.1 to 3 s, a
/// mechanical transient (the head moving) sits inside or at the edges, and
/// the music resumes.
pub fn find_program_changes(src: &SourceAudio) -> Vec<ProgramChange> {
    let rate = src.rate as usize;
    let hop = rate / 100;
    let n = src.frames / hop;
    if n < 400 {
        return Vec::new();
    }
    let env: Vec<f32> = (0..n).map(|i| rms_db(&src.read_mono((i * hop) as i64, ((i + 1) * hop) as i64))).collect();
    let peak: Vec<f32> = (0..n)
        .map(|i| {
            let x = src.read_mono((i * hop) as i64, ((i + 1) * hop) as i64);
            20.0 * x.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-9).log10()
        })
        .collect();
    let floor = {
        let mut v: Vec<f32> = env.iter().copied().filter(|d| *d > -110.0).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v.get(v.len() / 50).copied().unwrap_or(-90.0)
    };
    let mut out = Vec::new();
    let mut i = 100;
    while i + 100 < n {
        // level in the 2 s before
        let before: f32 = env[i - 100..i].iter().sum::<f32>() / 100.0;
        if env[i] < floor + 3.0 && before > floor + 15.0 {
            let mut j = i;
            while j < n && env[j] < floor + 6.0 {
                j += 1;
            }
            let len = j - i;
            if (10..=300).contains(&len) && j + 100 < n {
                let after: f32 = env[j..j + 100].iter().sum::<f32>() / 100.0;
                // the transient: a sample peak far above the RMS around the mute
                let a = i.saturating_sub(5);
                let b = (j + 5).min(n);
                let thump = (a..b).any(|k| peak[k] - env[k] > 18.0 && peak[k] > floor + 20.0);
                if after > floor + 15.0 && thump {
                    // a song spans it when the music is loud right at both edges
                    let edge_b = env[i.saturating_sub(15)..i.saturating_sub(5)].iter().sum::<f32>() / 10.0;
                    let edge_a = env[(j + 5).min(n - 1)..(j + 15).min(n)].iter().sum::<f32>() / 10.0;
                    let spans = edge_b > before - 6.0 && edge_a > after - 6.0;
                    out.push(ProgramChange { start_s: (i * hop) as f64 / rate as f64, end_s: (j * hop) as f64 / rate as f64, spans_song: spans });
                }
                i = j + 100;
                continue;
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    out
}

/// Tracks between program changes, in source seconds.
pub fn split_program_changes(src: &SourceAudio) -> Vec<(f64, f64)> {
    let ch = find_program_changes(src);
    if ch.is_empty() {
        return vec![(0.0, src.duration())];
    }
    let mut out = Vec::new();
    let mut t = 0.0;
    for c in &ch {
        out.push((t, (c.start_s + c.end_s) / 2.0));
        t = (c.start_s + c.end_s) / 2.0;
    }
    out.push((t, src.duration()));
    out
}

/// Standard alignment and test tone frequencies, Hz.
const STANDARD_TONES: &[f32] = &[315.0, 333.0, 400.0, 1000.0, 3000.0, 3150.0, 10000.0];

/// A steady tone in the first minute: its frequency (speed), level (NR
/// reference) and where it is.
pub fn detect_tone(src: &SourceAudio) -> Option<ToneInfo> {
    let rate = src.rate as usize;
    let win = rate; // 1 s
    let max_s = 60.min(src.frames / rate);
    if max_s < 3 {
        return None;
    }
    // dominant frequency per second, tonal if one line holds most of the energy
    let (fwd, _) = denoise::fft_pair(8192);
    let hann = denoise::hann(8192);
    let mut per_s: Vec<Option<(f32, f32)>> = Vec::new();
    for k in 0..max_s {
        let x = src.read_mono((k * win) as i64, (k * win + 8192) as i64);
        let mut inb: Vec<f32> = x.iter().zip(&hann).map(|(a, b)| a * b).collect();
        let mut spec = vec![realfft::num_complex::Complex::new(0.0f32, 0.0); 4097];
        if fwd.process(&mut inb, &mut spec).is_err() {
            per_s.push(None);
            continue;
        }
        let pw: Vec<f32> = spec.iter().map(|c| c.norm_sqr()).collect();
        let total: f32 = pw.iter().sum();
        let (bi, _) = pw.iter().enumerate().skip(4).fold((0, 0.0f32), |b, (i, v)| if *v > b.1 { (i, *v) } else { b });
        let near: f32 = pw[bi.saturating_sub(3)..(bi + 4).min(pw.len())].iter().sum();
        let lvl = rms_db(&src.read_mono((k * win) as i64, ((k + 1) * win) as i64));
        if total > 0.0 && near / total > 0.8 && lvl > -60.0 {
            per_s.push(Some((bi as f32 * rate as f32 / 8192.0, lvl)));
        } else {
            per_s.push(None);
        }
    }
    // the first run of 3+ seconds on the same line
    let mut i = 0;
    while i < per_s.len() {
        if let Some((f, _)) = per_s[i] {
            let mut j = i + 1;
            while j < per_s.len() && per_s[j].map(|(g, _)| (g - f).abs() < f * 0.01 + 3.0).unwrap_or(false) {
                j += 1;
            }
            if j - i >= 3 {
                let a = i * win + win / 5;
                let b = j * win - win / 5;
                let x = src.read_mono(a as i64, b as i64);
                let t = wow::track(&x, src.rate, f as f64, 0.02, 4.0)?;
                let freq = t.mean() as f32;
                let nominal = STANDARD_TONES.iter().copied().filter(|n| (freq / n - 1.0).abs() < 0.06).min_by(|a, b| (freq / a - 1.0).abs().partial_cmp(&(freq / b - 1.0).abs()).unwrap());
                let level = rms_db(&x);
                return Some(ToneInfo {
                    freq_hz: freq,
                    nominal_hz: nominal,
                    level_dbfs: level,
                    start_s: i as f64,
                    len_s: (j - i) as f64,
                    speed_err_pct: nominal.map(|n| (freq / n - 1.0) * 100.0),
                });
            }
            i = j;
        } else {
            i += 1;
        }
    }
    None
}

/// Best channel: 0 left, 1 right, 2 the sum. One channel clearly worse (more
/// noise or more dropouts) loses; close ones are summed for about 3 dB more
/// signal over hiss.
pub fn best_channel(src: &SourceAudio) -> u8 {
    if src.channels < 2 {
        return 0;
    }
    // noise floor and dropout count per channel, over 20 s stretches spread
    // across the source (all of it when it is short)
    let rate = src.rate as usize;
    let len = (rate * 20).min(src.frames);
    let n = (src.frames / len.max(1)).clamp(1, 12);
    let (mut fl, mut fr, mut dl, mut dr) = (Vec::new(), Vec::new(), 0usize, 0usize);
    for i in 0..n {
        let s = (src.frames - len) * i / (n - 1).max(1);
        let l = src.read_channel(0, s as i64, (s + len) as i64);
        let r = src.read_channel(1, s as i64, (s + len) as i64);
        fl.push(crate::dsp::loudness::noise_floor_db(&[l.clone()], src.rate));
        fr.push(crate::dsp::loudness::noise_floor_db(&[r.clone()], src.rate));
        dl += crate::dsp::dropout::find_dropouts(&l, src.rate, s as i64, &Default::default()).len();
        dr += crate::dsp::dropout::find_dropouts(&r, src.rate, s as i64, &Default::default()).len();
    }
    let med = |mut v: Vec<f32>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    };
    let (fl, fr) = (med(fl), med(fr));
    // 4 dB more noise, or three times the dropouts (and at least 5 more), loses
    let worse_l = fl - fr > 4.0 || (dl >= 3 * dr.max(1) && dl >= dr + 5);
    let worse_r = fr - fl > 4.0 || (dr >= 3 * dl.max(1) && dr >= dl + 5);
    match (worse_l, worse_r) {
        (true, false) => 1,
        (false, true) => 0,
        _ => 2,
    }
}

/// Measure the azimuth curve of a stereo source, a minute at a time so a long
/// tape is never held in memory.
pub fn measure_azimuth(src: &SourceAudio) -> Option<azimuth::AzimuthCurve> {
    if src.channels != 2 {
        return None;
    }
    let rate = src.rate as usize;
    let block = rate * 60;
    // windows need +-1 ms of lag room and the band filters a little run-up
    let lead = rate / 10;
    let mut curve = azimuth::AzimuthCurve::default();
    let mut f = 0usize;
    while f < src.frames {
        let a = f.saturating_sub(lead);
        let e = (f + block + (azimuth::WIN_S * rate as f64) as usize).min(src.frames);
        let l = src.read_channel(0, a as i64, e as i64);
        let r = src.read_channel(1, a as i64, e as i64);
        let c = azimuth::measure(&l, &r, src.rate);
        let off = a as f32 / rate as f32;
        let lo = f as f32 / rate as f32;
        let hi = (f + block) as f32 / rate as f32;
        for (t, us) in c.t.iter().zip(&c.us) {
            let t = t + off;
            if t >= lo && t < hi && curve.t.last().map(|l| t > *l + 0.1).unwrap_or(true) {
                curve.t.push(t);
                curve.us.push(*us);
            }
        }
        f += block;
    }
    if curve.is_empty() {
        None
    } else {
        Some(curve)
    }
}

/// What a wow measurement found.
pub struct WowResult {
    pub track: Option<(wow::FreqTrack, wow::WowMap)>,
    /// Plain-language note for the stage card.
    pub note: String,
    pub peak_pct: Option<f32>,
    pub weighted_pct: Option<f32>,
    /// Static speed error the reference shows, percent (positive: fast).
    pub static_err_pct: Option<f32>,
}

/// Track the wow reference (hum or a named tone) over a whole source.
pub fn measure_wow(src: &SourceAudio, w: &crate::chain::Wow) -> WowResult {
    let (nominal, search, bw) = match w.reference {
        WowRef::Hum => (w.hum_hz.clamp(40.0, 70.0) as f64, 0.06, 4.0),
        WowRef::Tone => (w.tone_hz.clamp(50.0, 20000.0) as f64, 0.06, 60.0),
    };
    let Some(t) = wow::track_blocks(src.frames, &mut |a, b| src.read_mono(a as i64, b as i64), src.rate, nominal, search, bw) else {
        return WowResult { track: None, note: "No stable reference found. Wow correction needs hum recorded onto the tape or a test tone.".into(), peak_pct: None, weighted_pct: None, static_err_pct: None };
    };
    match (w.reference, wow::classify(&t, nominal)) {
        (WowRef::Hum, wow::RefKind::CaptureChain) => WowResult {
            track: None,
            note: "This hum is from your capture chain, not the tape. It is rock steady, so it says nothing about the tape's speed.".into(),
            peak_pct: None,
            weighted_pct: None,
            static_err_pct: None,
        },
        (_, wow::RefKind::Unusable) => WowResult { track: None, note: "The reference is too weak or too noisy to track.".into(), peak_pct: None, weighted_pct: None, static_err_pct: None },
        _ => {
            let peak = t.peak_dev_pct() as f32;
            let weighted = if w.reference == WowRef::Tone && (w.tone_hz - 3150.0).abs() < 1.0 { Some(wow::weighted_wow_flutter(&t) as f32) } else { None };
            let err = ((t.mean() / nominal - 1.0) * 100.0) as f32;
            let map = wow::WowMap::from_track(&t, src.rate);
            let note = match weighted {
                Some(wf) => format!("Tracking the {nominal:.0} Hz reference: wow peaks at {peak:.2}%, weighted wow and flutter {wf:.3}% (2-sigma, approximate IEC 60386 weighting)."),
                None => format!("Tracking the {} reference: wow peaks at {peak:.2}%.", if w.reference == WowRef::Hum { format!("{nominal:.0} Hz hum") } else { format!("{nominal:.0} Hz tone") }),
            };
            WowResult { track: Some((t, map)), note, peak_pct: Some(peak), weighted_pct: weighted, static_err_pct: Some(err) }
        }
    }
}

/// Measure what a source's tape and digital stages need, store it in the
/// source's analysis, and return plain notes for the user. Runs as part of
/// Analyze (and headless jobs), only for the stages that are on:
///
/// - the reference tone at the head of a tape (always, for tape media)
/// - the wow curve when wow is on
/// - the azimuth curve when azimuth is on
/// - the best channel when channel mode is Best channel auto
/// - digital errors for digital media, turning error repair on when any are found
pub fn analyze_media(p: &mut crate::project::Project, sid: u64, a: &SourceAudio) -> Vec<String> {
    use crate::analysis::{EventKind, MapEvent};
    let fam = p.medium.preset.family;
    let chain = p.chain.clone();
    let mut notes = Vec::new();
    let Some(s) = p.source_mut(sid) else { return notes };
    let name = s.name.clone();
    if fam.is_tape() || fam == crate::media::Family::Generic {
        s.fx.analysis.tone = detect_tone(a);
        if let Some(t) = &s.fx.analysis.tone {
            notes.push(format!(
                "{name}: reference tone {:.1} Hz at {:.1} dBFS{}",
                t.freq_hz,
                t.level_dbfs,
                t.speed_err_pct.map(|e| format!(", the deck runs {:.2}% {}", e.abs(), if e >= 0.0 { "fast" } else { "slow" })).unwrap_or_default()
            ));
        }
    }
    if chain.wow.enabled && !chain.digital {
        let r = measure_wow(a, &chain.wow);
        s.fx.analysis.wow_note = Some(r.note.clone());
        s.fx.analysis.wow_peak_pct = r.peak_pct;
        s.fx.analysis.wow_weighted_pct = r.weighted_pct;
        s.fx.analysis.set_wow(sid, r.track);
        notes.push(format!("{name}: {}", r.note));
    }
    if chain.azimuth.enabled && a.channels == 2 {
        let c = measure_azimuth(a);
        if let Some(c) = &c {
            notes.push(format!("{name}: azimuth {:.1} us mean, {:.1} us spread ({})", c.mean(), c.spread(), if c.is_static() { "static" } else { "drifting, tracked" }));
        } else {
            notes.push(format!("{name}: no usable azimuth reading (the channels hardly correlate)"));
        }
        s.fx.analysis.set_azimuth(sid, c);
    }
    if chain.channel_mode.enabled && chain.channel_mode.mode == crate::chain::ChannelModeKind::BestAuto && a.channels == 2 {
        let b = best_channel(a);
        s.fx.analysis.best_channel = Some(b);
        notes.push(format!("{name}: best channel {}", ["left", "right", "both, summed"][b as usize]));
    }
    let mut enable_errors = false;
    if chain.digital {
        let er = &chain.error_repair;
        let mut ev = Vec::new();
        let step = a.rate as usize * 10;
        let mut f = 0usize;
        while f < a.frames {
            let e = (f + step).min(a.frames);
            let lead = 512.min(f);
            for c in 0..a.channels {
                let x = a.read_channel(c, (f - lead) as i64, e as i64);
                for e in crate::dsp::dropout::find_errors(&x, a.rate, er.min_run.max(2) as usize, er.max_ms.clamp(0.1, 100.0)) {
                    if e.pos < lead {
                        continue;
                    }
                    ev.push(MapEvent {
                        start_s: (f - lead + e.pos) as f64 / a.rate as f64,
                        len_s: e.len as f64 / a.rate as f64,
                        kind: if e.fix == crate::dsp::dropout::Fix::Flagged { EventKind::ErrorFlagged } else { EventKind::Error },
                        depth_db: 0.0,
                    });
                }
            }
            f = e;
        }
        ev.sort_by(|x, y| x.start_s.partial_cmp(&y.start_s).unwrap());
        ev.dedup_by(|x, y| (x.start_s - y.start_s).abs() < 1e-4);
        if !ev.is_empty() {
            notes.push(format!("{name}: {} digital error(s) found; error repair is on", ev.len()));
            enable_errors = true;
        }
        s.fx.analysis.set_errors(sid, ev);
    }
    if enable_errors {
        p.chain.error_repair.enabled = true;
    }
    notes
}

/// Store the dropouts a track analysis found on its source's map.
pub fn store_dropouts(p: &mut crate::project::Project, sid: u64, events: &[crate::chain::StageEvent], range: (i64, i64)) {
    use crate::analysis::{EventKind, MapEvent};
    let Some(s) = p.source_mut(sid) else { return };
    let rate = s.rate as f64;
    let ratio = s.fx.speed.ratio();
    let (lo, hi) = (range.0 as f64 * ratio / rate, range.1 as f64 * ratio / rate);
    let mut v: Vec<MapEvent> = s.fx.analysis.dropouts().iter().filter(|e| e.start_s < lo || e.start_s >= hi).copied().collect();
    for e in events.iter().filter(|e| matches!(e.kind, EventKind::Dropout | EventKind::DropoutFlagged)) {
        v.push(MapEvent { start_s: e.pos as f64 * ratio / rate, len_s: e.len as f64 * ratio / rate, kind: e.kind, depth_db: e.depth_db });
    }
    v.sort_by(|a, b| a.start_s.partial_cmp(&b.start_s).unwrap());
    s.fx.analysis.set_dropouts(sid, v);
}
