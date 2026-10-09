//! Windowed-sinc interpolation for speed correction and fractional delays.
//!
//! Output sample n is read from input position `pos0 + n * ratio`. With
//! ratio > 1 (speeding up) the kernel is widened to low-pass below the new
//! Nyquist, so nothing aliases.

const PHASES: usize = 512;
const HALF: usize = 32;

pub struct Interp {
    /// Kernel table: (PHASES + 1) rows of 2*half taps.
    table: Vec<f32>,
    half: usize,
    taps: usize,
}

fn kaiser_i0(x: f64) -> f64 {
    let mut s = 1.0;
    let mut t = 1.0;
    let y = x * x / 4.0;
    for k in 1..30 {
        t *= y / (k * k) as f64;
        s += t;
        if t < 1e-12 * s {
            break;
        }
    }
    s
}

impl Interp {
    /// `ratio` input samples per output sample.
    pub fn new(ratio: f64) -> Self {
        let cut = if ratio > 1.0 { 0.97 / ratio } else { 0.97 };
        let half = ((HALF as f64) / cut).ceil() as usize;
        let taps = half * 2;
        let beta = 9.0;
        let i0b = kaiser_i0(beta);
        let mut table = vec![0.0f32; (PHASES + 1) * taps];
        for ph in 0..=PHASES {
            let frac = ph as f64 / PHASES as f64;
            let mut sum = 0.0;
            for t in 0..taps {
                // tap t sits at input offset (t - half + 1) relative to floor(pos)
                let d = (t as f64 - half as f64 + 1.0) - frac;
                let xs = d * cut;
                let sinc = if xs.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * xs).sin() / (std::f64::consts::PI * xs) };
                let r = d / half as f64;
                let w = if r.abs() >= 1.0 { 0.0 } else { kaiser_i0(beta * (1.0 - r * r).sqrt()) / i0b };
                let v = cut * sinc * w;
                table[ph * taps + t] = v as f32;
                sum += v;
            }
            // normalize DC gain
            if sum.abs() > 1e-9 {
                for t in 0..taps {
                    table[ph * taps + t] /= sum as f32;
                }
            }
        }
        Interp { table, half, taps }
    }

    /// Input samples needed on each side of a read position.
    pub fn reach(&self) -> usize {
        self.half + 1
    }

    #[inline]
    pub fn at(&self, x: &[f32], pos: f64) -> f32 {
        let ip = pos.floor();
        let frac = pos - ip;
        let base = ip as i64 - self.half as i64 + 1;
        let phf = frac * PHASES as f64;
        let ph = phf.floor() as usize;
        let pf = (phf - ph as f64) as f32;
        let r0 = &self.table[ph * self.taps..(ph + 1) * self.taps];
        let r1 = &self.table[(ph + 1) * self.taps..(ph + 2) * self.taps];
        let mut acc = 0.0f32;
        let n = x.len() as i64;
        if base >= 0 && base + self.taps as i64 <= n {
            let s = &x[base as usize..base as usize + self.taps];
            for t in 0..self.taps {
                acc += s[t] * (r0[t] + (r1[t] - r0[t]) * pf);
            }
        } else {
            for t in 0..self.taps {
                let i = base + t as i64;
                if i >= 0 && i < n {
                    acc += x[i as usize] * (r0[t] + (r1[t] - r0[t]) * pf);
                }
            }
        }
        acc
    }
}

/// Produce `n_out` samples reading from `x` at `pos0 + n * ratio`.
pub fn resample(x: &[f32], pos0: f64, ratio: f64, n_out: usize) -> Vec<f32> {
    let it = Interp::new(ratio);
    (0..n_out).map(|n| it.at(x, pos0 + n as f64 * ratio)).collect()
}

/// Delay a signal by `d` samples (fractional, may be negative).
pub fn frac_delay(x: &[f32], d: f64) -> Vec<f32> {
    if d.abs() < 1e-6 {
        return x.to_vec();
    }
    resample(x, -d, 1.0, x.len())
}
