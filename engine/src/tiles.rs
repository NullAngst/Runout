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

/// Colors for the pictures drawn here. One per theme; Verdigris is the
/// palette Runout always had.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Palette {
    pub wave_raw: [u8; 3],
    pub wave_proc: [u8; 3],
    pub wave_mid: [u8; 3],
    /// Spectrogram ramp at RAMP_STOPS.
    pub ramp: [[u8; 3]; 5],
    pub tile_bg: [u8; 3],
    pub accent: [u8; 3],
    pub attention: [u8; 3],
}

pub const RAMP_STOPS: [f32; 5] = [0.0, 0.35, 0.65, 0.85, 1.0];
pub const THEMES: [&str; 5] = ["verdigris", "light", "dark", "kawaii-light", "kawaii-dark"];

fn rgb(h: u32) -> [u8; 3] {
    [(h >> 16) as u8, (h >> 8) as u8, h as u8]
}

impl Default for Palette {
    fn default() -> Self {
        Palette::verdigris()
    }
}

impl Palette {
    /// Today's colors, byte for byte.
    pub fn verdigris() -> Self {
        Palette {
            wave_raw: [0x4a, 0x5f, 0x5a],
            wave_proc: [0x5f, 0xb8, 0xa5],
            wave_mid: [0x2a, 0x3d, 0x39],
            ramp: [[0x0e, 0x19, 0x17], [0x1d, 0x4a, 0x42], [0x5f, 0xb8, 0xa5], [0xd4, 0x88, 0x4a], [0xf3, 0xd9, 0xbd]],
            tile_bg: [0x0e, 0x19, 0x17],
            accent: [0x5f, 0xb8, 0xa5],
            attention: [0xd4, 0x88, 0x4a],
        }
    }

    pub fn named(theme: &str) -> Self {
        let p = |raw: u32, proc: u32, mid: u32, ramp: [u32; 5], bg: u32, att: u32| Palette {
            wave_raw: rgb(raw),
            wave_proc: rgb(proc),
            wave_mid: rgb(mid),
            ramp: ramp.map(rgb),
            tile_bg: rgb(bg),
            accent: rgb(proc),
            attention: rgb(att),
        };
        match theme {
            "light" => p(0xa9b1bb, 0x2563d0, 0xd4d8dd, [0xf6f7f8, 0xc9d8f2, 0x2f6fdb, 0xc2541a, 0x4a1a05], 0xf6f7f8, 0xb2410f),
            "dark" => p(0x4b535e, 0x5b9cff, 0x333a43, [0x15181c, 0x1f3a63, 0x5b9cff, 0xff9a52, 0xffe1c4], 0x15181c, 0xff9a52),
            "kawaii-light" => p(0xd9b8cb, 0xb8367a, 0xf5c6dc, [0xfff5fa, 0xf8c9df, 0xe0569b, 0x8b7ae6, 0x3b2a6e], 0xfff5fa, 0xb03e22),
            "kawaii-dark" => p(0x5d5078, 0xff8cc6, 0x463b63, [0x1f1a2e, 0x3d2f63, 0x9f86e0, 0xff8cc6, 0xffe3f1], 0x1f1a2e, 0xffd27a),
            _ => Palette::verdigris(),
        }
    }
}

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
    waveform_with(segs, t0, t1, w, h, vzoom, &Palette::verdigris())
}

pub fn waveform_with(segs: &[Seg], t0: f64, t1: f64, w: u32, h: u32, vzoom: f32, pal: &Palette) -> Vec<u8> {
    let (raw_c, proc_c, mid_c) = (pal.wave_raw, pal.wave_proc, pal.wave_mid);
    let w = w.clamp(1, 8192);
    let h = h.clamp(8, 2048);
    let mut img = image::RgbaImage::new(w, h);
    let mid = h as f32 / 2.0;
    for x in 0..w {
        img.put_pixel(x, mid as u32, image::Rgba([mid_c[0], mid_c[1], mid_c[2], 255]));
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
            draw(r, raw_c);
        }
        if let Some(p) = pr {
            draw(p, proc_c);
        }
    }
    png(&img)
}

/// The palette's ramp (Verdigris: dark -> verdigris -> pale copper)
fn ramp(pal: &Palette, v: f32) -> [u8; 3] {
    let f3 = |c: [u8; 3]| [c[0] as f32, c[1] as f32, c[2] as f32];
    let stops: [(f32, [f32; 3]); 5] = [
        (RAMP_STOPS[0], f3(pal.ramp[0])),
        (RAMP_STOPS[1], f3(pal.ramp[1])),
        (RAMP_STOPS[2], f3(pal.ramp[2])),
        (RAMP_STOPS[3], f3(pal.ramp[3])),
        (RAMP_STOPS[4], f3(pal.ramp[4])),
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
    pal.ramp[4]
}

/// Frequency (Hz) at a row of the spectrogram (log scale, 20 Hz at the bottom).
pub fn row_freq(y: u32, h: u32, nyq: f32) -> f32 {
    let fmin = 20.0f32;
    let frac = 1.0 - (y as f32 + 0.5) / h as f32;
    fmin * (nyq / fmin).powf(frac)
}

/// Spectrogram of the raw source audio over [t0, t1) global seconds.
pub fn spectrogram(segs: &[Seg], t0: f64, t1: f64, w: u32, h: u32, floor_db: f32) -> Vec<u8> {
    spectrogram_with(segs, t0, t1, w, h, floor_db, &Palette::verdigris())
}

pub fn spectrogram_with(segs: &[Seg], t0: f64, t1: f64, w: u32, h: u32, floor_db: f32, pal: &Palette) -> Vec<u8> {
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
                    let c = ramp(pal, v);
                    img.put_pixel(x as u32, y as u32, image::Rgba([c[0], c[1], c[2], 255]));
                }
            }
            None => {
                let bg = pal.tile_bg;
                for y in 0..h {
                    img.put_pixel(x as u32, y, image::Rgba([bg[0], bg[1], bg[2], 255]));
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

/// Which overlay to draw under the waveform.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Overlay {
    Speed,
    Azimuth,
    Dropouts,
    Errors,
}

/// One source's tape analysis, for the overlays.
pub struct OverlaySeg<'a> {
    pub place: &'a Placement,
    pub analysis: &'a crate::analysis::TapeAnalysis,
}

/// A strip under the waveform: the speed curve (relative speed, +-0.5% full
/// height or the curve's own range if bigger), the azimuth curve (+-the
/// largest offset), or the dropout or error map as bars. Transparent where
/// there is nothing.
pub fn overlay(segs: &[OverlaySeg], kind: Overlay, t0: f64, t1: f64, w: u32, h: u32, pal: &Palette) -> Vec<u8> {
    let w = w.clamp(1, 8192);
    let h = h.clamp(4, 512);
    let mut img = image::RgbaImage::new(w, h);
    let dt = (t1 - t0) / w as f64;
    let col = match kind {
        Overlay::Speed | Overlay::Azimuth => pal.accent,
        Overlay::Dropouts | Overlay::Errors => pal.attention,
    };
    let mid = (h / 2) as i64;
    let px = |img: &mut image::RgbaImage, x: u32, y: i64, a: u8| {
        if y >= 0 && (y as u32) < h {
            img.put_pixel(x, y as u32, image::Rgba([col[0], col[1], col[2], a]));
        }
    };
    for s in segs {
        let ps = s.place.start;
        let pe = ps + s.place.duration;
        // source seconds for a timeline time
        let to_src = |g: f64| (g - ps) * s.place.ratio;
        match kind {
            Overlay::Speed | Overlay::Azimuth => {
                let value = |src_s: f64| -> Option<f32> {
                    match kind {
                        Overlay::Speed => {
                            let t = s.analysis.wow_track()?;
                            let k = ((src_s - t.t0) / crate::dsp::wow::HOP_S).round();
                            if k < 0.0 || k as usize >= t.freq.len() {
                                return None;
                            }
                            Some(((t.freq[k as usize] as f64 / t.mean()) - 1.0) as f32 * 100.0)
                        }
                        _ => {
                            let c = s.analysis.azimuth()?;
                            if (src_s as f32) < c.t[0] - 1.0 || (src_s as f32) > c.t[c.t.len() - 1] + 1.0 {
                                return None;
                            }
                            Some(c.at(src_s))
                        }
                    }
                };
                let range = match kind {
                    Overlay::Speed => s.analysis.wow_track().map(|t| (t.peak_dev_pct() as f32).max(0.5)).unwrap_or(0.5),
                    _ => s.analysis.azimuth().map(|c| c.us.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(10.0)).unwrap_or(10.0),
                };
                let mut last: Option<i64> = None;
                for x in 0..w {
                    let g = t0 + (x as f64 + 0.5) * dt;
                    if g < ps || g >= pe {
                        last = None;
                        continue;
                    }
                    let Some(v) = value(to_src(g)) else {
                        last = None;
                        continue;
                    };
                    let y = mid - ((v / range).clamp(-1.0, 1.0) * (h as f32 / 2.0 - 1.0)) as i64;
                    px(&mut img, x, mid, 60);
                    let (a, b) = match last {
                        Some(l) => (l.min(y), l.max(y)),
                        None => (y, y),
                    };
                    for yy in a..=b {
                        px(&mut img, x, yy, 255);
                    }
                    last = Some(y);
                }
            }
            Overlay::Dropouts | Overlay::Errors => {
                let evs = if kind == Overlay::Dropouts { s.analysis.dropouts() } else { s.analysis.errors() };
                for e in evs {
                    let a = ps + e.start_s / s.place.ratio;
                    let b = ps + (e.start_s + e.len_s) / s.place.ratio;
                    if b < t0 || a > t1 {
                        continue;
                    }
                    let x0 = ((a - t0) / dt).floor().max(0.0) as u32;
                    let x1 = (((b - t0) / dt).ceil() as u32).clamp(x0 + 1, w);
                    let flagged = matches!(e.kind, crate::analysis::EventKind::DropoutFlagged | crate::analysis::EventKind::ErrorFlagged);
                    let hh = if flagged { h as i64 } else { ((e.depth_db.max(3.0) / 30.0).min(1.0) * h as f32) as i64 };
                    for x in x0..x1.min(w) {
                        for y in (h as i64 - hh.max(2))..h as i64 {
                            px(&mut img, x, y, if flagged { 255 } else { 200 });
                        }
                    }
                }
            }
        }
    }
    png(&img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdigris_is_todays_palette() {
        let p = Palette::verdigris();
        assert_eq!(p.wave_raw, [0x4a, 0x5f, 0x5a]);
        assert_eq!(p.wave_proc, [0x5f, 0xb8, 0xa5]);
        assert_eq!(p.wave_mid, [0x2a, 0x3d, 0x39]);
        assert_eq!(p.tile_bg, [0x0e, 0x19, 0x17]);
        assert_eq!(Palette::named("anything else"), p);
        for t in THEMES {
            let _ = Palette::named(t);
        }
    }
}
