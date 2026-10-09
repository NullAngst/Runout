//! Waveform and spectrogram pictures, rendered here so the web view only shows
//! images.

use crate::audio::{SourceAudio, PEAK_BUCKET};
use crate::dsp::denoise::{fft_pair, hann};
use crate::project::Placement;
use rayon::prelude::*;
use realfft::num_complex::Complex;

pub struct Seg<'a> {
    pub place: &'a Placement,
    pub src: &'a SourceAudio,
    /// Processed peaks per PEAK_BUCKET processed samples, if analysed.
    pub processed: Option<&'a [(f32, f32)]>,
}

const RAW: [u8; 3] = [0x4a, 0x5f, 0x5a];
const PROC: [u8; 3] = [0x5f, 0xb8, 0xa5];
const MID: [u8; 3] = [0x2a, 0x3d, 0x39];

fn png(img: &image::RgbaImage) -> Vec<u8> {
    use image::ImageEncoder;
    let mut out = Vec::new();
    let enc = image::codecs::png::PngEncoder::new_with_quality(&mut out, image::codecs::png::CompressionType::Fast, image::codecs::png::FilterType::NoFilter);
    let _ = enc.write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8);
    out
}

fn minmax_raw(src: &SourceAudio, f0: i64, f1: i64) -> Option<(f32, f32)> {
    let f0 = f0.max(0) as usize;
    let f1 = (f1.max(0) as usize).min(src.frames);
    if f1 <= f0 {
        return None;
    }
    if f1 - f0 < PEAK_BUCKET * 2 {
        let mut lo = 0.0f32;
        let mut hi = 0.0f32;
        for f in f0..f1 {
            for c in 0..src.channels {
                let v = src.sample(f, c);
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
        return Some((lo, hi));
    }
    let b0 = f0 / PEAK_BUCKET;
    let b1 = f1.div_ceil(PEAK_BUCKET).min(src.peaks.len());
    let mut lo = 0.0f32;
    let mut hi = 0.0f32;
    for p in &src.peaks[b0..b1] {
        lo = lo.min(p.0);
        hi = hi.max(p.1);
    }
    Some((lo, hi))
}

fn minmax_proc(p: &[(f32, f32)], s0: i64, s1: i64) -> Option<(f32, f32)> {
    let b0 = (s0.max(0) as usize) / PEAK_BUCKET;
    let b1 = ((s1.max(0) as usize).div_ceil(PEAK_BUCKET)).min(p.len());
    if b1 <= b0 {
        return None;
    }
    let mut lo = 0.0f32;
    let mut hi = 0.0f32;
    for v in &p[b0..b1] {
        lo = lo.min(v.0);
        hi = hi.max(v.1);
    }
    Some((lo, hi))
}

/// Waveform for the global timeline range [t0, t1) seconds.
pub fn waveform(segs: &[Seg], t0: f64, t1: f64, w: u32, h: u32, vzoom: f32) -> Vec<u8> {
    let w = w.clamp(1, 8192);
    let h = h.clamp(8, 2048);
    let mut img = image::RgbaImage::new(w, h);
    let mid = h as f32 / 2.0;
    for x in 0..w {
        img.put_pixel(x, mid as u32, image::Rgba([MID[0], MID[1], MID[2], 255]));
    }
    let dt = (t1 - t0) / w as f64;
    let cols: Vec<(u32, Option<(f32, f32)>, Option<(f32, f32)>)> = (0..w)
        .into_par_iter()
        .map(|x| {
            let a = t0 + x as f64 * dt;
            let b = a + dt;
            for s in segs {
                let ps = s.place.start;
                let pe = ps + s.place.duration;
                if b <= ps || a >= pe {
                    continue;
                }
                let rate = s.place.rate as f64;
                let pa = ((a - ps) * rate).floor() as i64;
                let pb = ((b - ps) * rate).ceil() as i64;
                let raw = minmax_raw(s.src, (pa as f64 * s.place.ratio) as i64, (pb as f64 * s.place.ratio) as i64 + 1);
                let pr = s.processed.and_then(|p| minmax_proc(p, pa, pb.max(pa + 1)));
                return (x, raw, pr);
            }
            (x, None, None)
        })
        .collect();
    let to_y = |v: f32| -> f32 { (mid - v * vzoom * (mid - 1.0)).clamp(0.0, h as f32 - 1.0) };
    for (x, raw, pr) in cols {
        let mut draw = |mm: (f32, f32), col: [u8; 3]| {
            let y0 = to_y(mm.1) as u32;
            let y1 = to_y(mm.0) as u32;
            for y in y0.min(y1)..=y1.max(y0) {
                img.put_pixel(x, y, image::Rgba([col[0], col[1], col[2], 255]));
            }
        };
        if let Some(r) = raw {
            draw(r, RAW);
        }
        if let Some(p) = pr {
            draw(p, PROC);
        }
    }
    png(&img)
}

/// dark -> verdigris -> pale copper
fn ramp(v: f32) -> [u8; 3] {
    let stops: [(f32, [f32; 3]); 5] = [
        (0.0, [0x0e as f32, 0x19 as f32, 0x17 as f32]),
        (0.35, [0x1d as f32, 0x4a as f32, 0x42 as f32]),
        (0.65, [0x5f as f32, 0xb8 as f32, 0xa5 as f32]),
        (0.85, [0xd4 as f32, 0x88 as f32, 0x4a as f32]),
        (1.0, [0xf3 as f32, 0xd9 as f32, 0xbd as f32]),
    ];
    let v = v.clamp(0.0, 1.0);
    for i in 0..stops.len() - 1 {
        let (a, ca) = stops[i];
        let (b, cb) = stops[i + 1];
        if v <= b {
            let f = (v - a) / (b - a);
            return [
                (ca[0] + (cb[0] - ca[0]) * f) as u8,
                (ca[1] + (cb[1] - ca[1]) * f) as u8,
                (ca[2] + (cb[2] - ca[2]) * f) as u8,
            ];
        }
    }
    [0xf3, 0xd9, 0xbd]
}

/// Frequency (Hz) at a row of the spectrogram (log scale, 20 Hz at the bottom).
pub fn row_freq(y: u32, h: u32, nyq: f32) -> f32 {
    let fmin = 20.0f32;
    let frac = 1.0 - (y as f32 + 0.5) / h as f32;
    fmin * (nyq / fmin).powf(frac)
}

/// Spectrogram of the raw source audio over [t0, t1) global seconds.
pub fn spectrogram(segs: &[Seg], t0: f64, t1: f64, w: u32, h: u32, floor_db: f32) -> Vec<u8> {
    let w = w.clamp(1, 8192);
    let h = h.clamp(8, 2048);
    const N: usize = 2048;
    let dt = (t1 - t0) / w as f64;
    let win = hann(N);
    let wsum: f32 = win.iter().sum();
    let cols: Vec<Option<Vec<f32>>> = (0..w)
        .into_par_iter()
        .map(|x| {
            let tc = t0 + (x as f64 + 0.5) * dt;
            let s = segs.iter().find(|s| tc >= s.place.start && tc < s.place.start + s.place.duration)?;
            let rate = s.place.rate as f64;
            let centre = ((tc - s.place.start) * rate * s.place.ratio) as i64;
            let a = centre - N as i64 / 2;
            let x = s.src.read_mono(a, a + N as i64);
            let (fwd, _) = fft_pair(N);
            let mut inb: Vec<f32> = x.iter().zip(&win).map(|(v, w)| v * w).collect();
            let mut spec = vec![Complex::new(0.0f32, 0.0); N / 2 + 1];
            fwd.process(&mut inb, &mut spec).ok()?;
            let nyq = s.place.rate as f32 / 2.0;
            let binhz = s.place.rate as f32 / N as f32;
            let col: Vec<f32> = (0..h)
                .map(|y| {
                    let f = row_freq(y, h, nyq);
                    let fb = f / binhz;
                    // average the bins this row covers so high rows don't alias
                    let f_hi = row_freq(y.saturating_sub(1), h, nyq) / binhz;
                    let k0 = fb.floor() as usize;
                    let k1 = (f_hi.ceil() as usize).max(k0 + 1).min(N / 2);
                    let mut m = 0.0f32;
                    for k in k0.min(N / 2)..=k1 {
                        m = m.max(spec[k].norm());
                    }
                    let db = 20.0 * (2.0 * m / wsum).max(1e-12).log10();
                    ((db - floor_db) / (-floor_db - 6.0)).clamp(0.0, 1.0)
                })
                .collect();
            Some(col)
        })
        .collect();
    let mut img = image::RgbaImage::new(w, h);
    for (x, c) in cols.into_iter().enumerate() {
        match c {
            Some(col) => {
                for (y, v) in col.into_iter().enumerate() {
                    let c = ramp(v);
                    img.put_pixel(x as u32, y as u32, image::Rgba([c[0], c[1], c[2], 255]));
                }
            }
            None => {
                for y in 0..h {
                    img.put_pixel(x as u32, y, image::Rgba([0x0e, 0x19, 0x17, 255]));
                }
            }
        }
    }
    png(&img)
}

/// Processed peaks from a rendered buffer (all channels combined).
pub fn peaks_of(ch: &[Vec<f32>]) -> Vec<(f32, f32)> {
    let n = ch.first().map(|c| c.len()).unwrap_or(0);
    (0..n.div_ceil(PEAK_BUCKET))
        .map(|b| {
            let a = b * PEAK_BUCKET;
            let e = (a + PEAK_BUCKET).min(n);
            let mut lo = 0.0f32;
            let mut hi = 0.0f32;
            for c in ch {
                for v in &c[a..e] {
                    lo = lo.min(*v);
                    hi = hi.max(*v);
                }
            }
            (lo, hi)
        })
        .collect()
}
