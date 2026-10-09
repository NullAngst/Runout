//! Click and crackle detection and repair.
//!
//! Detection: the audio is modeled as an AR process per frame. Samples the model
//! cannot explain from either side (both the forward and backward prediction
//! errors are large) are flagged. Repair: LSAR interpolation across each flagged
//! run, using the good audio on both sides.

use super::lpc;

#[derive(Clone, Copy, Debug)]
pub struct ClickParams {
    /// Detection threshold above the residual noise level, in dB.
    pub threshold_db: f32,
    /// Longest run treated as a click, in samples.
    pub max_len: usize,
    /// AR model order.
    pub order: usize,
    /// Detection/repair passes.
    pub passes: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct Click {
    /// Start index in the buffer.
    pub pos: usize,
    pub len: usize,
    /// Largest sample change made by the repair (linear).
    pub size: f32,
}

const FRAME: usize = 4096;
const CONTEXT: usize = 512;

/// `abs0` is the absolute sample index of x[0]; analysis frames sit on a fixed
/// grid so a block render makes the same decisions as a full render.
pub fn process(x: &mut [f32], abs0: i64, params: &ClickParams) -> Vec<Click> {
    let mut all = Vec::new();
    for _ in 0..params.passes.max(1) {
        let found = one_pass(x, abs0, params);
        if found.is_empty() {
            break;
        }
        all.extend(found);
    }
    let merged = merge_events(all);
    refine(x, &merged, params.order);
    merged
}

/// Redo each repair with an AR model fit locally around the gap, now that the
/// surrounding clicks are gone. The first fit saw click energy; this one doesn't.
fn refine(x: &mut [f32], clicks: &[Click], p: usize) {
    let n = x.len();
    for c in clicks {
        let (g0, g1) = (c.pos, c.pos + c.len);
        if g0 < p || g1 + p > n {
            continue;
        }
        let a = g0.saturating_sub(1024);
        let b = (g1 + 1024).min(n);
        let mut fit: Vec<f32> = x[a..g0].to_vec();
        fit.extend_from_slice(&x[g1..b]);
        if let Some(coef) = lpc::estimate(&fit, p) {
            let keep: Vec<f32> = x[g0..g1].to_vec();
            if !lpc::lsar_fill(x, &coef, g0, g1) {
                x[g0..g1].copy_from_slice(&keep);
            }
        }
    }
}

fn merge_events(mut v: Vec<Click>) -> Vec<Click> {
    v.sort_by_key(|c| c.pos);
    let mut out: Vec<Click> = Vec::with_capacity(v.len());
    for c in v {
        if let Some(last) = out.last_mut() {
            if c.pos <= last.pos + last.len + 2 {
                let end = (c.pos + c.len).max(last.pos + last.len);
                last.len = end - last.pos;
                last.size = last.size.max(c.size);
                continue;
            }
        }
        out.push(c);
    }
    out
}

fn one_pass(x: &mut [f32], abs0: i64, prm: &ClickParams) -> Vec<Click> {
    let n = x.len();
    let p = prm.order;
    if n < FRAME / 2 + 2 * p {
        return Vec::new();
    }
    let k = 10f64.powf(prm.threshold_db as f64 / 20.0);
    // frame grid on absolute positions: first frame starts at -off
    let off = abs0.rem_euclid(FRAME as i64) as usize;
    let nframes = (n + off).div_ceil(FRAME);
    let mut coeffs: Vec<Option<Vec<f64>>> = Vec::with_capacity(nframes);
    // score = min(|forward error|, |backward error|) / residual level
    let mut score = vec![0.0f32; n];

    for f in 0..nframes {
        let f0 = (f * FRAME).saturating_sub(off);
        let f1 = ((f + 1) * FRAME).saturating_sub(off).min(n);
        if f1 <= f0 {
            coeffs.push(None);
            continue;
        }
        let w0 = f0.saturating_sub(CONTEXT);
        let w1 = (f1 + CONTEXT).min(n);
        let c = clean_fit(&x[w0..w1], p, k);
        if let Some(c) = &c {
            let lo = f0.max(p);
            let hi = f1.min(n - p);
            if hi > lo {
                let ef: Vec<f64> = (lo..hi).map(|i| lpc::fwd_err(x, c, i)).collect();
                let sigma = 1.4826 * lpc::median_abs(&ef);
                if sigma > 1e-7 {
                    let thr = k * sigma;
                    for (j, i) in (lo..hi).enumerate() {
                        if ef[j].abs() > thr {
                            let b = lpc::bwd_err(x, c, i).abs();
                            score[i] = (ef[j].abs().min(b) / sigma) as f32;
                        }
                    }
                }
            }
        }
        coeffs.push(c);
    }

    // Group flagged samples. Interior samples of a click can be predictable from
    // the click itself, so nearby flags are merged. A run longer than the max
    // length is re-examined at higher thresholds to find the click core inside
    // it; if even that is too long it is left alone (likely a music transient).
    let merge_gap = (p / 3).max(3);
    let mut targets: Vec<(usize, usize)> = Vec::new();
    for (s, e) in groups(&score, 0, n, k as f32, merge_gap) {
        if e - s + 2 <= prm.max_len {
            targets.push((s, e));
            continue;
        }
        for mult in [2.0f32, 4.0] {
            let sub = groups(&score, s, e, k as f32 * mult, merge_gap);
            if !sub.is_empty() && sub.iter().all(|(a, b)| b - a + 2 <= prm.max_len) {
                targets.extend(sub);
                break;
            }
        }
    }

    let mut out = Vec::new();
    for (s, e) in targets {
        let g0 = s.saturating_sub(1);
        let g1 = (e + 1).min(n);
        let len = g1 - g0;
        if len > prm.max_len || g0 < p || g1 + p > n {
            continue;
        }
        let fi = ((g0 + off) / FRAME).min(coeffs.len() - 1);
        let Some(c) = coeffs[fi].as_ref() else { continue };
        let before: Vec<f32> = x[g0..g1].to_vec();
        if lpc::lsar_fill(x, c, g0, g1) {
            let mut size = 0.0f32;
            for (a, b) in before.iter().zip(&x[g0..g1]) {
                size = size.max((a - b).abs());
            }
            out.push(Click { pos: g0, len, size });
        }
    }
    out
}

/// Runs of score > thr within [from, to), merging gaps up to `gap`.
fn groups(score: &[f32], from: usize, to: usize, thr: f32, gap: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = from;
    while i < to {
        if score[i] > thr {
            let s = i;
            let mut e = i;
            let mut j = i + 1;
            while j < to && j <= e + gap {
                if score[j] > thr {
                    e = j;
                }
                j += 1;
            }
            out.push((s, e + 1));
            i = e + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// AR estimate that is not thrown off by the clicks in the frame: fit, patch the
/// samples the fit cannot explain in a scratch copy, and fit again.
fn clean_fit(w: &[f32], p: usize, k: f64) -> Option<Vec<f64>> {
    let mut c = lpc::estimate(w, p)?;
    let n = w.len();
    if n < 4 * p {
        return Some(c);
    }
    let mut copy = w.to_vec();
    for _ in 0..2 {
        let ef: Vec<f64> = (p..n).map(|i| lpc::fwd_err(&copy, &c, i)).collect();
        let sigma = 1.4826 * lpc::median_abs(&ef);
        if sigma <= 1e-9 {
            return Some(c);
        }
        let thr = k * sigma;
        let mut flagged = vec![false; n];
        let mut any = false;
        for (j, e) in ef.iter().enumerate() {
            if e.abs() > thr {
                // the forward error smears over the next p samples; flag the onset area
                let i = j + p;
                for f in flagged.iter_mut().take((i + 2).min(n)).skip(i.saturating_sub(1)) {
                    *f = true;
                }
                any = true;
            }
        }
        if !any {
            break;
        }
        let mut i = p;
        while i + p < n {
            if flagged[i] {
                let s = i;
                while i + p < n && flagged[i] {
                    i += 1;
                }
                lpc::lsar_fill(&mut copy, &c, s, i);
            } else {
                i += 1;
            }
        }
        c = lpc::estimate(&copy, p)?;
    }
    Some(c)
}

/// Interpolate a selected region with a high-order AR model fit to the audio on
/// both sides. For short selections (up to ~10 ms).
pub fn repair_region(x: &mut [f32], g0: usize, g1: usize, order: usize) -> bool {
    let n = x.len();
    let ctx = 2048;
    if g0 < order || g1 + order > n || g1 <= g0 {
        return false;
    }
    let a0 = g0.saturating_sub(ctx);
    let b1 = (g1 + ctx).min(n);
    // Fit on the good audio either side, concatenated.
    let mut fit: Vec<f32> = x[a0..g0].to_vec();
    fit.extend_from_slice(&x[g1..b1]);
    let Some(c) = lpc::estimate(&fit, order) else { return false };
    lpc::lsar_fill(x, &c, g0, g1)
}

/// Declip: rebuild runs of samples stuck at or above `thr` (linear).
pub fn declip(x: &mut [f32], thr: f32, max_len: usize, order: usize) -> usize {
    let n = x.len();
    let mut runs = Vec::new();
    let mut i = 0;
    while i < n {
        if x[i].abs() >= thr {
            let s = i;
            while i < n && x[i].abs() >= thr {
                i += 1;
            }
            if i - s >= 2 {
                runs.push((s, i));
            }
        } else {
            i += 1;
        }
    }
    let mut fixed = 0;
    for (s, e) in runs {
        let len = e - s;
        if len > max_len || s < order + 1 || e + order + 1 > n {
            continue;
        }
        let sign: Vec<f32> = x[s..e].iter().map(|v| v.signum()).collect();
        let a0 = s.saturating_sub(1024);
        let b1 = (e + 1024).min(n);
        let mut fit: Vec<f32> = x[a0..s].to_vec();
        fit.extend_from_slice(&x[e..b1]);
        let Some(c) = lpc::estimate(&fit, order) else { continue };
        if lpc::lsar_fill(x, &c, s, e) {
            for (j, sg) in sign.iter().enumerate() {
                let v = x[s + j];
                // The rebuilt peak must be at least as large as the clip level.
                x[s + j] = sg * v.abs().max(thr);
            }
            fixed += 1;
        }
    }
    fixed
}

/// Detect clipping: returns the clip level if there are repeated runs of
/// samples pinned at the maximum.
pub fn detect_clipping(x: &[f32]) -> Option<f32> {
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if peak < 0.25 {
        return None;
    }
    let thr = peak * 0.999;
    let mut runs = 0;
    let mut i = 0;
    while i < x.len() {
        if x[i].abs() >= thr {
            let s = i;
            while i < x.len() && x[i].abs() >= thr {
                i += 1;
            }
            if i - s >= 3 {
                runs += 1;
            }
        } else {
            i += 1;
        }
    }
    if runs >= 3 {
        Some(thr)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_and_fixes_clicks() {
        let n = 48000;
        let mut seed = 1u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        let clean: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f64 / 48000.0;
                (0.3 * (2.0 * std::f64::consts::PI * 330.0 * t).sin()
                    + 0.1 * (2.0 * std::f64::consts::PI * 2900.0 * t).sin()) as f32
                    + 0.001 * rnd()
            })
            .collect();
        let mut x = clean.clone();
        let positions = [5000, 12000, 20000, 33000, 41000];
        for (j, &p) in positions.iter().enumerate() {
            for k in 0..(4 + j * 3) {
                x[p + k] += if k % 2 == 0 { 0.5 } else { -0.3 };
            }
        }
        let clicks = process(&mut x, 0, &ClickParams { threshold_db: 13.0, max_len: 144, order: 24, passes: 2 });
        assert!(clicks.len() >= positions.len(), "found {}", clicks.len());
        for &p in &positions {
            let err = (p - 5..p + 30).map(|i| (x[i] - clean[i]).abs()).fold(0.0f32, f32::max);
            assert!(err < 0.05, "residual {err} at {p}");
        }
    }
}
