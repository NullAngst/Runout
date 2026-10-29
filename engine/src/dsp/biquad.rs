//! Biquad filters (RBJ cookbook), transposed direct form II with f64 state.

use std::f64::consts::PI;

#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    fn norm(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        Biquad { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0, z1: 0.0, z2: 0.0 }
    }

    /// A section from normalized coefficients (a0 = 1).
    pub fn from_coeffs(b0: f64, b1: f64, b2: f64, a1: f64, a2: f64) -> Self {
        Biquad { b0, b1, b2, a1, a2, z1: 0.0, z2: 0.0 }
    }

    pub fn highpass(fs: f64, f: f64, q: f64) -> Self {
        let w = 2.0 * PI * f / fs;
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * q);
        Self::norm((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    pub fn lowpass(fs: f64, f: f64, q: f64) -> Self {
        let w = 2.0 * PI * f / fs;
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * q);
        Self::norm((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    /// First-order highpass (bilinear), stored as a biquad.
    pub fn highpass1(fs: f64, f: f64) -> Self {
        let k = (PI * f / fs).tan();
        let a0 = 1.0 + k;
        Self::norm(1.0, -1.0, 0.0, a0, k - 1.0, 0.0)
    }

    pub fn notch(fs: f64, f: f64, q: f64) -> Self {
        let w = 2.0 * PI * f / fs;
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * q);
        Self::norm(1.0, -2.0 * c, 1.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    pub fn peaking(fs: f64, f: f64, q: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w = 2.0 * PI * f / fs;
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * q);
        Self::norm(1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a)
    }

    pub fn low_shelf(fs: f64, f: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w = 2.0 * PI * f / fs;
        let (s, c) = w.sin_cos();
        let alpha = s / 2.0 * std::f64::consts::SQRT_2; // shelf slope S = 1
        let sa = 2.0 * a.sqrt() * alpha;
        Self::norm(
            a * ((a + 1.0) - (a - 1.0) * c + sa),
            2.0 * a * ((a - 1.0) - (a + 1.0) * c),
            a * ((a + 1.0) - (a - 1.0) * c - sa),
            (a + 1.0) + (a - 1.0) * c + sa,
            -2.0 * ((a - 1.0) + (a + 1.0) * c),
            (a + 1.0) + (a - 1.0) * c - sa,
        )
    }

    pub fn high_shelf(fs: f64, f: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w = 2.0 * PI * f / fs;
        let (s, c) = w.sin_cos();
        let alpha = s / 2.0 * std::f64::consts::SQRT_2;
        let sa = 2.0 * a.sqrt() * alpha;
        Self::norm(
            a * ((a + 1.0) + (a - 1.0) * c + sa),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - sa),
            (a + 1.0) - (a - 1.0) * c + sa,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - sa,
        )
    }

    #[inline]
    pub fn tick(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    pub fn run(&mut self, buf: &mut [f32]) {
        for v in buf.iter_mut() {
            *v = self.tick(*v as f64) as f32;
        }
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    /// Group delay at DC, samples.
    pub fn group_delay_dc(&self) -> f64 {
        let nb = self.b0 + self.b1 + self.b2;
        let na = 1.0 + self.a1 + self.a2;
        let tb = if nb.abs() > 1e-18 { (self.b1 + 2.0 * self.b2) / nb } else { 0.0 };
        let ta = if na.abs() > 1e-18 { (self.a1 + 2.0 * self.a2) / na } else { 0.0 };
        tb - ta
    }

    /// Magnitude response at frequency f.
    pub fn magnitude(&self, fs: f64, f: f64) -> f64 {
        let w = 2.0 * PI * f / fs;
        let (c1, s1) = (w.cos(), -w.sin());
        let (c2, s2) = ((2.0 * w).cos(), -(2.0 * w).sin());
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = self.b1 * s1 + self.b2 * s2;
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = self.a1 * s1 + self.a2 * s2;
        ((nr * nr + ni * ni) / (dr * dr + di * di)).sqrt()
    }
}

/// Butterworth highpass of the given order as a cascade of sections.
pub fn butter_highpass(fs: f64, f: f64, order: usize) -> Vec<Biquad> {
    let mut out = Vec::new();
    let n = order.max(1);
    for k in 0..n / 2 {
        let theta = PI * (2 * k + 1) as f64 / (2 * n) as f64;
        let q = 1.0 / (2.0 * theta.cos());
        out.push(Biquad::highpass(fs, f, q));
    }
    if n % 2 == 1 {
        out.push(Biquad::highpass1(fs, f));
    }
    out
}

/// Butterworth lowpass of the given order as a cascade of sections.
pub fn butter_lowpass(fs: f64, f: f64, order: usize) -> Vec<Biquad> {
    let mut out = Vec::new();
    let n = order.max(1);
    for k in 0..n / 2 {
        let theta = PI * (2 * k + 1) as f64 / (2 * n) as f64;
        let q = 1.0 / (2.0 * theta.cos());
        out.push(Biquad::lowpass(fs, f, q));
    }
    if n % 2 == 1 {
        let k = (PI * f / fs).tan();
        let a0 = 1.0 + k;
        out.push(Biquad::norm(k, k, 0.0, a0, k - 1.0, 0.0));
    }
    out
}

/// Magnitude of a cascade at frequency f.
pub fn cascade_magnitude(sections: &[Biquad], fs: f64, f: f64) -> f64 {
    sections.iter().map(|s| s.magnitude(fs, f)).product()
}

pub fn run_cascade(sections: &mut [Biquad], buf: &mut [f32]) {
    for s in sections.iter_mut() {
        s.run(buf);
    }
}

/// Zero-phase run: forward then backward through fresh copies of the sections.
pub fn run_cascade_zero_phase(sections: &[Biquad], buf: &mut [f32]) {
    let mut fwd: Vec<Biquad> = sections.to_vec();
    run_cascade(&mut fwd, buf);
    buf.reverse();
    let mut bwd: Vec<Biquad> = sections.to_vec();
    for s in bwd.iter_mut() {
        s.reset();
    }
    run_cascade(&mut bwd, buf);
    buf.reverse();
}
