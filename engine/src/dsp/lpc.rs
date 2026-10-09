//! Autoregressive modeling and least-squares AR interpolation (LSAR).
//!
//! Prediction-error filter convention: c[0] = 1, e[n] = sum_k c[k] * x[n-k].

/// Estimate an AR model of order `p` from `x` (Hann windowed autocorrelation,
/// Levinson-Durbin). Returns the prediction-error filter c (len p+1), or None
/// for silent or degenerate input.
pub fn estimate(x: &[f32], p: usize) -> Option<Vec<f64>> {
    let n = x.len();
    if n <= p * 2 {
        return None;
    }
    let mut w = Vec::with_capacity(n);
    let denom = (n - 1) as f64;
    for (i, &v) in x.iter().enumerate() {
        let h = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / denom).cos();
        w.push(v as f64 * h);
    }
    let mut r = vec![0.0f64; p + 1];
    for lag in 0..=p {
        let mut s = 0.0;
        for i in lag..n {
            s += w[i] * w[i - lag];
        }
        r[lag] = s;
    }
    if r[0] <= 1e-18 {
        return None;
    }
    // Slight lag windowing and white-noise correction for stability.
    r[0] *= 1.0 + 1e-6;
    for (lag, v) in r.iter_mut().enumerate().skip(1) {
        let g = (-0.5 * (0.004 * lag as f64).powi(2)).exp();
        *v *= g;
    }
    // Levinson-Durbin for a (x[n] ~ sum a_k x[n-k])
    let mut a = vec![0.0f64; p + 1];
    let mut err = r[0];
    for i in 1..=p {
        let mut acc = r[i];
        for j in 1..i {
            acc -= a[j] * r[i - j];
        }
        let k = acc / err;
        let prev = a.clone();
        a[i] = k;
        for j in 1..i {
            a[j] = prev[j] - k * prev[i - j];
        }
        err *= 1.0 - k * k;
        if err <= 0.0 {
            return None;
        }
    }
    let mut c = vec![0.0f64; p + 1];
    c[0] = 1.0;
    for k in 1..=p {
        c[k] = -a[k];
    }
    Some(c)
}

/// Forward prediction error at n (requires n >= p).
#[inline]
pub fn fwd_err(x: &[f32], c: &[f64], n: usize) -> f64 {
    let mut s = 0.0;
    for (k, &ck) in c.iter().enumerate() {
        s += ck * x[n - k] as f64;
    }
    s
}

/// Backward prediction error at n (requires n + p < len).
#[inline]
pub fn bwd_err(x: &[f32], c: &[f64], n: usize) -> f64 {
    let mut s = 0.0;
    for (k, &ck) in c.iter().enumerate() {
        s += ck * x[n + k] as f64;
    }
    s
}

/// Replace x[g0..g1) by the samples that minimize the AR prediction error energy
/// (LSAR). Needs p good samples on each side: g0 >= p, g1 + p <= x.len().
/// Returns false if the system could not be solved.
pub fn lsar_fill(x: &mut [f32], c: &[f64], g0: usize, g1: usize) -> bool {
    let p = c.len() - 1;
    let l = g1 - g0;
    if l == 0 || g0 < p || g1 + p > x.len() {
        return false;
    }
    // Autocorrelation of c: R[i][j] = rc[|i-j|]
    let mut rc = vec![0.0f64; p + 1];
    for lag in 0..=p {
        let mut s = 0.0;
        for k in 0..=(p - lag) {
            s += c[k] * c[k + lag];
        }
        rc[lag] = s;
    }
    // Known-part prediction error for n in g0..g1+p
    let span = l + p;
    let mut ek = vec![0.0f64; span];
    for (idx, n) in (g0..g1 + p).enumerate() {
        let mut s = 0.0;
        for (k, &ck) in c.iter().enumerate() {
            let m = n - k;
            if m < g0 || m >= g1 {
                s += ck * x[m] as f64;
            }
        }
        ek[idx] = s;
    }
    // b_i = -sum_{n=t}^{t+p} c[n-t] * ek[n]
    let mut b = vec![0.0f64; l];
    for i in 0..l {
        let mut s = 0.0;
        for k in 0..=p {
            s += c[k] * ek[i + k];
        }
        b[i] = -s;
    }
    // Banded symmetric matrix, solve with Cholesky (dense lower triangle within band).
    let bw = p.min(l - 1);
    // store L as l x (bw+1) band: lb[i][d] = L[i][i-d]
    let mut lb = vec![0.0f64; l * (bw + 1)];
    let idx = |i: usize, d: usize| i * (bw + 1) + d;
    for i in 0..l {
        let jmin = i.saturating_sub(bw);
        for j in jmin..=i {
            let d = i - j;
            let mut s = rc[d] + if d == 0 { 1e-9 * rc[0] } else { 0.0 };
            let kmin = jmin.max(j.saturating_sub(bw));
            for k in kmin..j {
                s -= lb[idx(i, i - k)] * lb[idx(j, j - k)];
            }
            if i == j {
                if s <= 0.0 {
                    return false;
                }
                lb[idx(i, 0)] = s.sqrt();
            } else {
                lb[idx(i, d)] = s / lb[idx(j, 0)];
            }
        }
    }
    // forward substitution L y = b
    let mut y = vec![0.0f64; l];
    for i in 0..l {
        let mut s = b[i];
        let jmin = i.saturating_sub(bw);
        for j in jmin..i {
            s -= lb[idx(i, i - j)] * y[j];
        }
        y[i] = s / lb[idx(i, 0)];
    }
    // back substitution L^T z = y
    let mut z = vec![0.0f64; l];
    for i in (0..l).rev() {
        let mut s = y[i];
        let jmax = (i + bw).min(l - 1);
        for j in i + 1..=jmax {
            s -= lb[idx(j, j - i)] * z[j];
        }
        z[i] = s / lb[idx(i, 0)];
    }
    for i in 0..l {
        if !z[i].is_finite() {
            return false;
        }
    }
    for i in 0..l {
        x[g0 + i] = z[i] as f32;
    }
    true
}

/// Median of absolute values (copies).
pub fn median_abs(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut a: Vec<f64> = v.iter().map(|x| x.abs()).collect();
    let mid = a.len() / 2;
    a.select_nth_unstable_by(mid, |p, q| p.partial_cmp(q).unwrap_or(std::cmp::Ordering::Equal));
    a[mid]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsar_restores_sine() {
        let n = 4000;
        let mut x: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f64 / 48000.0;
                (0.4 * (2.0 * std::f64::consts::PI * 440.0 * t).sin()
                    + 0.2 * (2.0 * std::f64::consts::PI * 1250.0 * t).sin()) as f32
            })
            .collect();
        let orig = x.clone();
        for i in 2000..2030 {
            x[i] = 0.9;
        }
        let c = estimate(&orig[1000..3000], 24).unwrap();
        assert!(lsar_fill(&mut x, &c, 2000, 2030));
        let err: f64 = (2000..2030).map(|i| (x[i] - orig[i]).abs() as f64).fold(0.0, f64::max);
        assert!(err < 0.01, "max err {err}");
    }
}
