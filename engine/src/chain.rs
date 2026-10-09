//! Restoration chain settings and the block renderer.
//!
//! The chain runs in a fixed order: DC, speed, rumble, hum, declick, decrackle,
//! manual repair, declip, denoise, channel tools, EQ. Leveling is applied per
//! track afterwards (see `level`), since it depends on the whole track.

use crate::audio::SourceAudio;
use crate::dsp::{biquad, declick, denoise, resample};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Rumble {
    pub enabled: bool,
    pub freq_hz: f32,
    /// 12, 24, 36 or 48 dB/octave.
    pub slope_db_oct: u32,
    pub linear_phase: bool,
}
impl Default for Rumble {
    fn default() -> Self {
        Rumble { enabled: true, freq_hz: 20.0, slope_db_oct: 24, linear_phase: false }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Hum {
    pub enabled: bool,
    /// Mains fundamental, 50 or 60.
    pub mains_hz: f32,
    pub harmonics: u32,
    /// Notch width, Hz.
    pub width_hz: f32,
}
impl Default for Hum {
    fn default() -> Self {
        Hum { enabled: false, mains_hz: 60.0, harmonics: 4, width_hz: 1.0 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Declick {
    pub enabled: bool,
    /// How far above the residual noise a sample must be to count as a click. Lower catches more.
    pub threshold_db: f32,
    pub max_len_ms: f32,
}
impl Default for Declick {
    fn default() -> Self {
        Declick { enabled: true, threshold_db: 13.0, max_len_ms: 2.0 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Decrackle {
    pub enabled: bool,
    pub threshold_db: f32,
    pub max_len_ms: f32,
}
impl Default for Decrackle {
    fn default() -> Self {
        Decrackle { enabled: true, threshold_db: 10.5, max_len_ms: 0.25 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Declip {
    pub enabled: bool,
    /// Clip level, dBFS. Samples at or above it in runs are rebuilt.
    pub level_db: f32,
    pub max_len_ms: f32,
}
impl Default for Declip {
    fn default() -> Self {
        Declip { enabled: false, level_db: -0.1, max_len_ms: 2.0 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Denoise {
    pub enabled: bool,
    pub reduction_db: f32,
    /// 0 to 100 percent.
    pub smoothing_pct: f32,
}
impl Default for Denoise {
    fn default() -> Self {
        Denoise { enabled: true, reduction_db: 6.0, smoothing_pct: 50.0 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ChannelTools {
    pub enabled: bool,
    /// Positive moves the image right.
    pub balance_db: f32,
    pub swap: bool,
    /// Right channel delay relative to left, microseconds (negative advances it).
    pub azimuth_us: f32,
}
impl Default for ChannelTools {
    fn default() -> Self {
        ChannelTools { enabled: false, balance_db: 0.0, swap: false, azimuth_us: 0.0 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EqBand {
    pub freq_hz: f32,
    pub gain_db: f32,
    pub q: f32,
}
impl Default for EqBand {
    fn default() -> Self {
        EqBand { freq_hz: 1000.0, gain_db: 0.0, q: 1.0 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Eq {
    pub enabled: bool,
    pub low_shelf_hz: f32,
    pub low_shelf_db: f32,
    pub high_shelf_hz: f32,
    pub high_shelf_db: f32,
    pub bands: Vec<EqBand>,
}
impl Default for Eq {
    fn default() -> Self {
        Eq { enabled: false, low_shelf_hz: 100.0, low_shelf_db: 0.0, high_shelf_hz: 8000.0, high_shelf_db: 0.0, bands: Vec::new() }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LevelMode {
    Track,
    Album,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Level {
    pub enabled: bool,
    pub mode: LevelMode,
    pub target_lufs: f32,
    pub ceiling_dbtp: f32,
    /// Most gain reduction the limiter may apply before leveling stops short of target.
    pub max_gr_db: f32,
}
impl Default for Level {
    fn default() -> Self {
        Level { enabled: true, mode: LevelMode::Track, target_lufs: -14.0, ceiling_dbtp: -1.0, max_gr_db: 3.0 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Chain {
    pub rumble: Rumble,
    pub hum: Hum,
    pub declick: Declick,
    pub decrackle: Decrackle,
    pub declip: Declip,
    pub denoise: Denoise,
    pub channel: ChannelTools,
    pub eq: Eq,
    pub level: Level,
}

impl Chain {
    /// Every stage off: the audio passes through untouched (Cleaned imports).
    pub fn passthrough() -> Self {
        let mut c = Chain::default();
        c.rumble.enabled = false;
        c.hum.enabled = false;
        c.declick.enabled = false;
        c.decrackle.enabled = false;
        c.declip.enabled = false;
        c.denoise.enabled = false;
        c.channel.enabled = false;
        c.eq.enabled = false;
        c.level.enabled = false;
        c
    }

    /// True when no restoration stage is on (leveling is checked separately).
    pub fn restoration_off(&self) -> bool {
        !(self.rumble.enabled
            || self.hum.enabled
            || self.declick.enabled
            || self.decrackle.enabled
            || self.declip.enabled
            || self.denoise.enabled
            || self.channel.enabled
            || self.eq.enabled)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Speed {
    /// The speed the record was played at during capture.
    pub captured_rpm: f32,
    /// The speed it should have been played at.
    pub correct_rpm: f32,
    /// Fine adjustment, cents. Positive raises pitch.
    pub cents: f32,
}
impl Default for Speed {
    fn default() -> Self {
        Speed { captured_rpm: 33.333, correct_rpm: 33.333, cents: 0.0 }
    }
}
impl Speed {
    /// Input samples consumed per output sample.
    pub fn ratio(&self) -> f64 {
        let r = (self.correct_rpm as f64 / self.captured_rpm.max(1.0) as f64) * 2f64.powf(self.cents as f64 / 1200.0);
        if (r - 1.0).abs() < 1e-9 {
            1.0
        } else {
            r
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Repair {
    pub id: u64,
    /// Source seconds (as captured, before speed correction).
    pub start_s: f64,
    pub end_s: f64,
    /// Frequency band painted on the spectrogram; None for a plain time selection.
    #[serde(default)]
    pub band: Option<(f32, f32)>,
}

/// Per-source settings that belong to the recording rather than the chain.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct SourceFx {
    pub speed: Speed,
    pub noise: Option<denoise::NoiseProfile>,
    pub repairs: Vec<Repair>,
}

#[derive(Clone, Copy, Debug)]
pub struct ClickHit {
    /// Processed-time sample index within the source.
    pub pos: i64,
    pub len: u32,
    /// Largest correction, linear.
    pub size: f32,
    pub crackle: bool,
}

pub struct RenderOut {
    pub out: Vec<Vec<f32>>,
    pub pre: Option<Vec<Vec<f32>>>,
    pub removed: Option<Vec<Vec<f32>>>,
    pub clicks: Vec<ClickHit>,
}

pub struct RenderReq<'a> {
    pub src: &'a SourceAudio,
    pub fx: &'a SourceFx,
    pub chain: &'a Chain,
    /// Processed-time start sample (may be negative).
    pub start: i64,
    pub len: usize,
    pub want_pre: bool,
    pub want_removed: bool,
    pub want_clicks: bool,
    /// Skip DC removal and every stage: bit-exact pass-through.
    pub passthrough: bool,
}

/// Context kept on both sides of a block so filters settle and the declicker
/// and denoiser always see audio on both sides of a cut.
pub fn pad_for(rate: u32) -> usize {
    (rate as usize * 3) / 10
}

/// Processed-time length of a source.
pub fn processed_frames(src: &SourceAudio, fx: &SourceFx) -> usize {
    (src.frames as f64 / fx.speed.ratio()).floor() as usize
}

fn ms(rate: u32, v: f32) -> usize {
    ((v / 1000.0) * rate as f32).round().max(1.0) as usize
}

pub fn render(req: &RenderReq) -> RenderOut {
    let src = req.src;
    let rate = src.rate;
    let fs = rate as f64;
    let pad = if req.passthrough { 0 } else { pad_for(rate) };
    let ratio = req.fx.speed.ratio();
    // Narrow hum notches ring for a long time; give the cheap IIR stages a longer
    // run-up than the expensive ones so block renders match full renders.
    let warm = if !req.passthrough && req.chain.hum.enabled {
        let w = req.chain.hum.width_hz.clamp(0.2, 10.0) as f64;
        ((4.5 / (std::f64::consts::PI * w)) * fs) as usize
    } else {
        0
    };
    let a = req.start - pad as i64 - warm as i64;
    let total = req.len + 2 * pad + warm;

    // 2. speed (read with resampling), done first so everything downstream sees corrected time
    let mut buf: Vec<Vec<f32>> = if ratio == 1.0 || req.passthrough {
        src.read(a, a + total as i64)
    } else {
        let it = resample::Interp::new(ratio);
        let reach = it.reach() as i64 + 2;
        let r0 = (a as f64 * ratio).floor() as i64 - reach;
        let r1 = ((a + total as i64) as f64 * ratio).ceil() as i64 + reach;
        (0..src.channels)
            .into_par_iter()
            .map(|c| {
                let raw = src.read_channel(c, r0, r1);
                (0..total).map(|n| it.at(&raw, (a + n as i64) as f64 * ratio - r0 as f64)).collect()
            })
            .collect()
    };

    if req.passthrough {
        return RenderOut {
            pre: if req.want_pre { Some(buf.clone()) } else { None },
            removed: if req.want_removed { Some(vec![vec![0.0; total]; buf.len()]) } else { None },
            out: buf,
            clicks: Vec::new(),
        };
    }

    let ch = req.chain;
    // 1. DC offset (5 Hz highpass), always on
    buf.par_iter_mut().for_each(|c| {
        let mut f = biquad::butter_highpass(fs, 5.0, 2);
        biquad::run_cascade(&mut f, c);
    });

    let pre = if req.want_pre || req.want_removed { Some(buf.clone()) } else { None };

    // 3. rumble
    if ch.rumble.enabled {
        let order = ((ch.rumble.slope_db_oct.clamp(12, 48) / 6) as usize).max(2);
        let lp = ch.rumble.linear_phase;
        let f = ch.rumble.freq_hz.clamp(5.0, 200.0) as f64;
        buf.par_iter_mut().for_each(|c| {
            if lp {
                let s = biquad::butter_highpass(fs, f, (order / 2).max(1));
                biquad::run_cascade_zero_phase(&s, c);
            } else {
                let mut s = biquad::butter_highpass(fs, f, order);
                biquad::run_cascade(&mut s, c);
            }
        });
    }

    // 4. hum
    if ch.hum.enabled {
        let f0 = ch.hum.mains_hz.clamp(40.0, 70.0) as f64;
        let w = ch.hum.width_hz.clamp(0.2, 10.0) as f64;
        let h = ch.hum.harmonics.clamp(1, 12) as usize;
        buf.par_iter_mut().for_each(|c| {
            for k in 1..=h {
                let f = f0 * k as f64;
                if f >= fs * 0.45 {
                    break;
                }
                let mut n = biquad::Biquad::notch(fs, f, f / w);
                n.run(c);
            }
        });
    }

    // drop the run-up: from here on the buffer starts at start - pad
    let (a, total, pre) = if warm > 0 {
        buf.iter_mut().for_each(|c| {
            c.drain(..warm);
        });
        let pre = pre.map(|mut p| {
            p.iter_mut().for_each(|c| {
                c.drain(..warm);
            });
            p
        });
        (a + warm as i64, total - warm, pre)
    } else {
        (a, total, pre)
    };

    // 5-8 per channel, in parallel
    let order = ((rate as usize) / 2000).clamp(16, 48);
    let repairs: Vec<(i64, i64, Option<(f32, f32)>)> = req
        .fx
        .repairs
        .iter()
        .map(|r| {
            let s = (r.start_s * fs / ratio).round() as i64 - a;
            let e = (r.end_s * fs / ratio).round() as i64 - a;
            (s, e, r.band)
        })
        .filter(|(s, e, _)| *e > 0 && *s < total as i64)
        .collect();
    let dc = ch.declick.clone();
    let dk = ch.decrackle.clone();
    let dl = ch.declip.clone();
    let dn = ch.denoise.clone();
    let profile = req.fx.noise.clone();
    let clicks_per_ch: Vec<Vec<ClickHit>> = buf
        .par_iter_mut()
        .map(|c| {
            let mut hits = Vec::new();
            if dc.enabled {
                let p = declick::ClickParams {
                    threshold_db: dc.threshold_db.clamp(3.0, 40.0),
                    max_len: ms(rate, dc.max_len_ms.clamp(0.1, 10.0)),
                    order,
                    passes: 2,
                };
                for k in declick::process(c, a, &p) {
                    hits.push(ClickHit { pos: a + k.pos as i64, len: k.len as u32, size: k.size, crackle: false });
                }
            }
            if dk.enabled {
                let p = declick::ClickParams {
                    threshold_db: dk.threshold_db.clamp(3.0, 40.0),
                    max_len: ms(rate, dk.max_len_ms.clamp(0.02, 1.0)),
                    order: (order * 2 / 3).max(12),
                    passes: 1,
                };
                for k in declick::process(c, a, &p) {
                    hits.push(ClickHit { pos: a + k.pos as i64, len: k.len as u32, size: k.size, crackle: true });
                }
            }
            // 7. manual repair
            for &(s, e, band) in &repairs {
                let s = s.max(0) as usize;
                let e = (e.max(0) as usize).min(c.len());
                if e <= s {
                    continue;
                }
                match band {
                    Some((lo, hi)) => denoise::spectral_repair(c, rate, s, e, lo, hi),
                    None => {
                        if e - s <= ms(rate, 10.0) {
                            declick::repair_region(c, s, e, (order * 2).min(64));
                        } else {
                            denoise::spectral_repair(c, rate, s, e, 0.0, rate as f32 / 2.0);
                        }
                    }
                }
            }
            // 8. declip
            if dl.enabled {
                let thr = 10f32.powf(dl.level_db.min(0.0) / 20.0);
                declick::declip(c, thr, ms(rate, dl.max_len_ms.clamp(0.1, 5.0)), order);
            }
            // 9. denoise
            if dn.enabled {
                if let Some(p) = &profile {
                    let prm = denoise::DenoiseParams { reduction_db: dn.reduction_db.clamp(0.0, 30.0), smoothing: dn.smoothing_pct / 100.0 };
                    denoise::denoise(c, a, rate, p, &prm);
                }
            }
            hits
        })
        .collect();

    let removed = if req.want_removed {
        pre.as_ref().map(|p| p.iter().zip(&buf).map(|(a, b)| a.iter().zip(b).map(|(x, y)| x - y).collect()).collect())
    } else {
        None
    };

    // 10. channel tools
    if ch.channel.enabled && buf.len() == 2 {
        if ch.channel.swap {
            buf.swap(0, 1);
        }
        let d = ch.channel.azimuth_us as f64 * 1e-6 * fs;
        if d.abs() > 1e-3 {
            buf[1] = resample::frac_delay(&buf[1], d);
        }
        let b = ch.channel.balance_db;
        if b != 0.0 {
            let gl = 10f32.powf(-b / 40.0);
            let gr = 10f32.powf(b / 40.0);
            buf[0].iter_mut().for_each(|v| *v *= gl);
            buf[1].iter_mut().for_each(|v| *v *= gr);
        }
    }

    // 11. EQ
    if ch.eq.enabled {
        let eq = ch.eq.clone();
        buf.par_iter_mut().for_each(|c| {
            if eq.low_shelf_db.abs() > 0.01 {
                biquad::Biquad::low_shelf(fs, eq.low_shelf_hz as f64, eq.low_shelf_db as f64).run(c);
            }
            if eq.high_shelf_db.abs() > 0.01 {
                biquad::Biquad::high_shelf(fs, (eq.high_shelf_hz as f64).min(fs * 0.45), eq.high_shelf_db as f64).run(c);
            }
            for b in &eq.bands {
                if b.gain_db.abs() > 0.01 {
                    biquad::Biquad::peaking(fs, (b.freq_hz as f64).min(fs * 0.45), b.q.clamp(0.1, 20.0) as f64, b.gain_db as f64).run(c);
                }
            }
        });
    }

    let crop = |v: Vec<Vec<f32>>| -> Vec<Vec<f32>> { v.into_iter().map(|c| c[pad..pad + req.len].to_vec()).collect() };
    let lo = req.start;
    let hi = req.start + req.len as i64;
    let mut clicks: Vec<ClickHit> = if req.want_clicks {
        clicks_per_ch.into_iter().flatten().filter(|h| h.pos >= lo && h.pos < hi).collect()
    } else {
        Vec::new()
    };
    clicks.sort_by_key(|h| h.pos);
    RenderOut {
        out: crop(buf),
        pre: if req.want_pre { pre.map(crop) } else { None },
        removed: removed.map(crop),
        clicks,
    }
}
