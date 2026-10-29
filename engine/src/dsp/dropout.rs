//! Tape dropout repair and digital error repair.
//!
//! Dropouts: short losses of head contact. They show as a dip in level, worst in
//! the treble, with sharp edges and the same level on both sides. Detection
//! runs on the 2 kHz-and-up band (where dropouts are deepest and where tape
//! hiss keeps an envelope even in quiet passages): a dip more than `depth_db`
//! below the local median level, 2 ms to 300 ms long, that drops in and comes
//! back within a few milliseconds to roughly where it was. Music that fades or
//! stops does neither, so it is left alone. Up to 10 ms the gap is rebuilt by
//! LSAR interpolation, the same one declick uses. Longer dips get envelope gain
//! compensation, which restores the level but cannot bring back lost treble.
//! Over 300 ms the dip is only flagged, for manual repair.
//!
//! Digital errors: a DAT, MiniDisc or DCC deck that cannot correct an error
//! mutes (a run of exact zeros) or holds the last good sample (a run of
//! identical samples). Inside audio that is not silent and not sitting still,
//! those runs are marks of an error. Under 10 ms they are interpolated; longer
//! ones are flagged.

use super::biquad;
use super::declick;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fix {
    Interpolated,
    Gain,
    Flagged,
}

#[derive(Clone, Copy, Debug)]
pub struct Event {
    /// Start index in the buffer.
    pub pos: usize,
    pub len: usize,
    pub fix: Fix,
    /// Depth below the local level, dB (dropouts) or 0 (digital).
    pub depth_db: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct DropoutParams {
    pub depth_db: f32,
    pub min_ms: f32,
    pub max_ms: f32,
    /// Longest dropout rebuilt by interpolation.
    pub interp_ms: f32,
}

impl Default for DropoutParams {
    fn default() -> Self {
        DropoutParams { depth_db: 6.0, min_ms: 2.0, max_ms: 300.0, interp_ms: 10.0 }
    }
}

/// Context each side a block needs for dropout decisions to match a full render.
pub fn context_s() -> f64 {
    0.75
}

fn db(ms: f64) -> f32 {
    (10.0 * ms.max(1e-14).log10()) as f32
}

/// Treble-band envelope: 2 ms RMS every 1 ms, dB.
fn hf_env(x: &[f32], rate: u32) -> (Vec<f32>, usize) {
    let hop = (rate as usize / 1000).max(1);
    let mut y = x.to_vec();
    let mut hp = biquad::butter_highpass(rate as f64, 2000.0, 4);
    biquad::run_cascade(&mut hp, &mut y);
    let n = y.len() / hop;
    let sq: Vec<f64> = (0..n).map(|i| y[i * hop..(i + 1) * hop].iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / hop as f64).collect();
    let env = (0..n).map(|i| if i + 1 < n { db((sq[i] + sq[i + 1]) / 2.0) } else { db(sq[i]) }).collect();
    (env, hop)
}

/// Median of `env` over a +-`half` hop window around each hop, computed every
/// 10 hops on an absolute grid and held in between.
fn local_median(env: &[f32], half: usize, abs_hop0: i64) -> Vec<f32> {
    let n = env.len();
    let mut out = vec![-120.0f32; n];
    let step = 10i64;
    let mut i = 0usize;
    while i < n {
        // grid point for this hop
        let g = (abs_hop0 + i as i64).div_euclid(step) * step + step / 2 - abs_hop0;
        let c = g.clamp(0, n as i64 - 1) as usize;
        let a = c.saturating_sub(half);
        let b = (c + half).min(n);
        let mut w: Vec<f32> = env[a..b].to_vec();
        let k = w.len() / 2;
        let m = if w.is_empty() {
            -120.0
        } else {
            *w.select_nth_unstable_by(k, |p, q| p.partial_cmp(q).unwrap()).1
        };
        // fill until the next grid boundary
        let next = ((abs_hop0 + i as i64).div_euclid(step) + 1) * step - abs_hop0;
        let end = (next.max(i as i64 + 1) as usize).min(n);
        for v in &mut out[i..end] {
            *v = m;
        }
        i = end;
    }
    out
}

/// Find dropouts in one channel. `abs0` is the absolute index of x[0], so the
/// decisions line up the same in a block render and a full render.
pub fn find_dropouts(x: &[f32], rate: u32, abs0: i64, p: &DropoutParams) -> Vec<Event> {
    let (env, hop) = hf_env(x, rate);
    let n = env.len();
    if n < 50 {
        return Vec::new();
    }
    let max_hops = (p.max_ms as usize).max(2);
    let reference = local_median(&env, max_hops + 100, abs0.div_euclid(hop as i64));
    let depth = p.depth_db.max(1.0);
    let min_hops = (p.min_ms.round() as usize).max(1);
    let mut out = Vec::new();
    let mut i = 3;
    while i + 3 < n {
        if reference[i] > -75.0 && env[i] < reference[i] - depth {
            let mut j = i;
            while j < n && env[j] < reference[i] - depth {
                j += 1;
            }
            let len = j - i;
            // sharp in and out, level back where it was
            let before = env[i.saturating_sub(3)..i].iter().cloned().fold(f32::MIN, f32::max);
            let after = env[j.min(n)..(j + 3).min(n)].iter().cloned().fold(f32::MIN, f32::max);
            let near = |v: f32| v > reference[i] - 3.0;
            let flagged = len > max_hops;
            if len >= min_hops && j + 3 <= n && near(before) && near(after) && (before - after).abs() < 4.0 {
                let floor = env[i..j].iter().cloned().fold(f32::MAX, f32::min);
                let interp = (len as f32) <= p.interp_ms;
                out.push(Event {
                    pos: i * hop,
                    len: len * hop,
                    fix: if flagged {
                        Fix::Flagged
                    } else if interp {
                        Fix::Interpolated
                    } else {
                        Fix::Gain
                    },
                    depth_db: reference[i] - floor,
                });
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// Repair found dropouts in place. Flagged ones are left alone.
pub fn repair_dropouts(x: &mut [f32], rate: u32, events: &[Event], order: usize) {
    let hop = (rate as usize / 1000).max(1);
    for e in events {
        match e.fix {
            Fix::Flagged => {}
            Fix::Interpolated => {
                let a = e.pos.saturating_sub(hop);
                let b = (e.pos + e.len + hop).min(x.len());
                if !declick::repair_region(x, a, b, order) {
                    gain_fill(x, rate, a, b);
                }
            }
            Fix::Gain => {
                let a = e.pos.saturating_sub(hop);
                let b = (e.pos + e.len + hop).min(x.len());
                gain_fill(x, rate, a, b);
            }
        }
    }
}

/// Envelope gain compensation over [a, b): bring the broadband level back up to
/// the average of the 20 ms either side, with 2 ms ramps.
fn gain_fill(x: &mut [f32], rate: u32, a: usize, b: usize) {
    let w = (rate as usize / 50).max(1);
    let rms = |s: &[f32]| (s.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / s.len().max(1) as f64).sqrt();
    let pre = rms(&x[a.saturating_sub(w)..a]);
    let post = rms(&x[b..(b + w).min(x.len())]);
    let want = (pre + post) / 2.0;
    let seg = (rate as usize / 200).max(1); // 5 ms
    let ramp = (rate as usize / 500).max(1);
    let mut gains = Vec::new();
    let mut i = a;
    while i < b {
        let e = (i + seg).min(b);
        let have = rms(&x[i..e]);
        gains.push(if have > 1e-9 { (want / have).clamp(1.0, 10.0) } else { 1.0 });
        i = e;
    }
    for (k, i) in (a..b).enumerate() {
        let gi = k / seg;
        let f = (k % seg) as f64 / seg as f64;
        let g0 = gains[gi];
        let g1 = *gains.get(gi + 1).unwrap_or(&g0);
        let mut g = g0 + (g1 - g0) * f;
        let edge = (i - a).min(b - 1 - i);
        if edge < ramp {
            g = 1.0 + (g - 1.0) * edge as f64 / ramp as f64;
        }
        x[i] = (x[i] as f64 * g) as f32;
    }
}

/// Digital error runs: exact zeros or held samples, at least `min_run` long,
/// inside audio that is moving. Samples are compared exactly, which is right
/// for anything decoded from 16- or 24-bit PCM.
pub fn find_errors(x: &[f32], rate: u32, min_run: usize, max_ms: f32) -> Vec<Event> {
    let n = x.len();
    let min_run = min_run.max(2);
    let ctx = (rate as usize / 200).max(8); // 5 ms
    let lsb = 1.0 / 32768.0;
    let max_len = ((max_ms / 1000.0) * rate as f32) as usize;
    let mut out = Vec::new();
    let mut i = 1;
    while i < n {
        let v = x[i];
        let mut j = i + 1;
        while j < n && x[j] == v {
            j += 1;
        }
        let len = j - i;
        if len >= min_run && i >= 2 && j + 1 < n {
            let around: Vec<f32> = x[i.saturating_sub(ctx)..i].iter().chain(&x[j..(j + ctx).min(n)]).copied().collect();
            let rms = (around.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / around.len().max(1) as f64).sqrt();
            // what the audio would have done over the run, from the slopes either side
            let slope_in = (x[i - 1] - x[i - 2]).abs();
            let slope_out = (x[j + 1] - x[j]).abs();
            let moving = (slope_in.max(slope_out) as f64) * len as f64 > 2.0 * lsb as f64;
            let audible = rms > 10f64.powf(-50.0 / 20.0);
            if moving && audible {
                out.push(Event { pos: i, len, fix: if len <= max_len { Fix::Interpolated } else { Fix::Flagged }, depth_db: 0.0 });
            }
        }
        i = j;
    }
    out
}

pub fn repair_errors(x: &mut [f32], events: &[Event], order: usize) {
    for e in events.iter().filter(|e| e.fix == Fix::Interpolated) {
        declick::repair_region(x, e.pos, e.pos + e.len, order);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hissy_tone(rate: u32, secs: f32) -> Vec<f32> {
        let mut seed = 3u64;
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .map(|i| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let r = ((seed >> 40) as f32 / (1u64 << 24) as f32) - 0.5;
                0.2 * (2.0 * std::f32::consts::PI * 3000.0 * i as f32 / rate as f32).sin() + 0.01 * r
            })
            .collect()
    }

    #[test]
    fn finds_dips_ignores_clean() {
        let rate = 48000;
        let mut x = hissy_tone(rate, 4.0);
        let spots = [(30000usize, 5usize), (70000, 40), (120000, 150)];
        for (p, ms) in spots {
            let l = ms * 48;
            for v in &mut x[p..p + l] {
                *v *= 0.15;
            }
        }
        let ev = find_dropouts(&x, rate, 0, &DropoutParams::default());
        assert_eq!(ev.len(), spots.len(), "{ev:?}");
        for ((p, ms), e) in spots.iter().zip(&ev) {
            assert!((e.pos as i64 - *p as i64).abs() < 200, "{e:?} vs {p}");
            assert!((e.len as i64 - (*ms * 48) as i64).abs() < 300, "{e:?} vs {ms} ms");
        }
        let mut y = x.clone();
        repair_dropouts(&mut y, rate, &ev, 32);
        let r = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
        assert!(r(&y[120000 + 1000..120000 + 6000]) > 0.1, "gain fill restores level");
    }

    #[test]
    fn digital_errors() {
        let rate = 48000;
        let q = |v: f32| (v * 32768.0).round() / 32768.0;
        let mut x: Vec<f32> = (0..48000).map(|i| q(0.3 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / rate as f32).sin())).collect();
        let clean = x.clone();
        for v in &mut x[10000..10040] {
            *v = 0.0;
        }
        let held = x[30000];
        for v in &mut x[30000..30006] {
            *v = held;
        }
        let ev = find_errors(&x, rate, 4, 10.0);
        assert_eq!(ev.len(), 2, "{ev:?}");
        assert!(find_errors(&clean, rate, 4, 10.0).is_empty());
        repair_errors(&mut x, &ev, 32);
        let err = (10000..10040).map(|i| (x[i] - clean[i]).abs()).fold(0.0, f32::max);
        assert!(err < 0.01, "zero run rebuilt, error {err}");
    }
}
