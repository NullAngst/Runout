//! True-peak lookahead limiter.
//!
//! Peak detection runs 4x oversampled so peaks that land between samples are
//! caught. The gain curve is a sliding minimum over the lookahead window,
//! followed by a moving average of the same length. Together they guarantee the
//! gain applied to a sample never exceeds the gain that sample needs, and the
//! gain changes smoothly. Release is exponential.

pub struct LimiterParams {
    pub ceiling_db: f32,
    pub lookahead_ms: f32,
    pub release_ms: f32,
}

/// 4x oversampled true-peak estimate per sample: max of |x[n]| and the three
/// interpolated points between n and n+1.
pub fn tp_envelope(x: &[f32]) -> Vec<f32> {
    const HALF: i64 = 6;
    let mut kern = [[0.0f32; 12]; 3];
    for (p, row) in kern.iter_mut().enumerate() {
        let frac = (p + 1) as f64 * 0.25;
        let mut sum = 0.0;
        for (t, v) in row.iter_mut().enumerate() {
            let d = (t as f64 - (HALF - 1) as f64) - frac;
            let sinc = (std::f64::consts::PI * d).sin() / (std::f64::consts::PI * d);
            let r = d / HALF as f64;
            let w = if r.abs() < 1.0 { (std::f64::consts::PI * r).cos() * 0.5 + 0.5 } else { 0.0 };
            *v = (sinc * w) as f32;
            sum += *v as f64;
        }
        for v in row.iter_mut() {
            *v /= sum as f32;
        }
    }
    let n = x.len() as i64;
    let mut env = vec![0.0f32; x.len()];
    for i in 0..n {
        let mut m = x[i as usize].abs();
        let base = i - (HALF - 1);
        for row in &kern {
            let mut acc = 0.0f32;
            if base >= 0 && base + 12 <= n {
                let s = &x[base as usize..base as usize + 12];
                for t in 0..12 {
                    acc += s[t] * row[t];
                }
            } else {
                for t in 0..12 {
                    let j = base + t as i64;
                    if j >= 0 && j < n {
                        acc += x[j as usize] * row[t];
                    }
                }
            }
            m = m.max(acc.abs());
        }
        env[i as usize] = m;
    }
    env
}

/// Apply the limiter in place across all channels (linked). Returns the largest
/// gain reduction applied, in dB (positive number).
pub fn limit(ch: &mut [Vec<f32>], rate: u32, prm: &LimiterParams) -> f32 {
    if ch.is_empty() || ch[0].is_empty() {
        return 0.0;
    }
    let n = ch[0].len();
    let ceil = 10f32.powf(prm.ceiling_db / 20.0) * 0.9995;
    let w = ((prm.lookahead_ms / 1000.0 * rate as f32) as usize).max(1);

    // required gain per sample
    let mut req = vec![1.0f32; n];
    for c in ch.iter() {
        let env = tp_envelope(c);
        for i in 0..n {
            if env[i] > ceil {
                req[i] = req[i].min(ceil / env[i]);
            }
        }
    }
    if req.iter().all(|&g| g >= 1.0) {
        return 0.0;
    }
    // m[i] = min(req[i - w + 1 ..= i]) via monotonic deque
    let mut m = vec![1.0f32; n + w];
    {
        let mut dq: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
        for i in 0..n + w {
            let v = if i < n { req[i] } else { 1.0 };
            let vals = |j: usize| if j < n { req[j] } else { 1.0 };
            while let Some(&b) = dq.back() {
                if vals(b) >= v {
                    dq.pop_back();
                } else {
                    break;
                }
            }
            dq.push_back(i);
            while let Some(&f) = dq.front() {
                if f + w <= i {
                    dq.pop_front();
                } else {
                    break;
                }
            }
            m[i] = vals(*dq.front().unwrap());
        }
    }
    // release: allow the held curve to rise only slowly
    let rel = (-1.0 / (prm.release_ms / 1000.0 * rate as f32)).exp();
    let mut env = 1.0f32;
    for v in m.iter_mut() {
        let rising = env + (1.0 - env) * (1.0 - rel);
        env = v.min(rising);
        *v = env;
    }
    // boxcar of length w; gain for sample i is avg(m[i .. i + w))
    let mut sum: f64 = m[..w].iter().map(|&v| v as f64).sum();
    let mut maxgr = 1.0f32;
    for i in 0..n {
        let g = (sum / w as f64) as f32;
        maxgr = maxgr.min(g);
        for c in ch.iter_mut() {
            c[i] *= g;
        }
        sum += m[i + w] as f64 - m[i] as f64;
    }
    -20.0 * maxgr.log10()
}
