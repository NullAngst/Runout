//! Restoration chain settings and the block renderer.
//!
//! The analog chain runs in a fixed order: DC, speed (with wow), azimuth,
//! channel mode, playback EQ correction, NR decode, rumble, hum, dropout
//! repair, declick, decrackle, manual repair, declip, denoise, band limit,
//! channel tools, EQ. Leveling is applied per track afterwards (see `level`),
//! since it depends on the whole track. Every stage added for tape defaults to
//! off, so a vinyl chain renders exactly as it always has.
//!
//! The digital chain (DAT, MiniDisc, DCC) is error repair, de-emphasis, manual
//! repair, then leveling. No DC filter: with every stage off it is the Cleaned
//! passthrough and the export is bit-identical to the source.

use crate::analysis::{EventKind, TapeAnalysis};
use crate::audio::SourceAudio;
use crate::dsp::nr::NrKind;
use crate::dsp::tape_eq::{self, Curve};
use crate::dsp::{azimuth, biquad, declick, denoise, dropout, nr, resample};
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
    /// Fundamental: mains (50 or 60) for hum, the video field rate (59.94 or 50)
    /// for head-switching buzz. Saved as fundamental_hz; older projects call it
    /// mains_hz, which is still the name in code so nothing built on it breaks.
    #[serde(rename = "fundamental_hz", alias = "mains_hz")]
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

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum WowRef {
    /// Mains hum recorded onto the tape.
    #[default]
    Hum,
    /// A named tone (test tape, alignment tone).
    Tone,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Wow {
    pub enabled: bool,
    pub reference: WowRef,
    /// Mains frequency the hum reference sits near.
    pub hum_hz: f32,
    pub tone_hz: f32,
}
impl Default for Wow {
    fn default() -> Self {
        Wow { enabled: false, reference: WowRef::Hum, hum_hz: 60.0, tone_hz: 3150.0 }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AzimuthMode {
    /// From the measured curve: static when it barely moves, tracked when it drifts.
    #[default]
    Auto,
    Static,
    Tracked,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Azimuth {
    pub enabled: bool,
    pub mode: AzimuthMode,
    /// Right-channel delay for static correction, microseconds (negative advances it).
    pub static_us: f32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChannelModeKind {
    #[default]
    Stereo,
    MonoSum,
    MonoLeft,
    MonoRight,
    /// Analyze picks: the clearly better channel, or the sum when they are close.
    BestAuto,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct ChannelMode {
    pub enabled: bool,
    pub mode: ChannelModeKind,
}

impl ChannelMode {
    /// Output channel count for a source with `src_ch` channels.
    pub fn out_channels(&self, src_ch: usize) -> usize {
        if self.enabled && src_ch == 2 && self.mode != ChannelModeKind::Stereo {
            1
        } else {
            src_ch
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CurveSel {
    /// Picker id ("cassette-120", "riaa", "columbia-78", "custom", ...).
    pub id: String,
    pub curve: Curve,
}
impl Default for CurveSel {
    fn default() -> Self {
        CurveSel { id: "flat".into(), curve: Curve::default() }
    }
}
impl CurveSel {
    pub fn named(id: &str) -> Self {
        CurveSel { id: id.into(), curve: tape_eq::find_curve(id).unwrap_or_default() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct PlaybackEq {
    pub enabled: bool,
    /// The curve the recording was played through.
    pub from: CurveSel,
    /// The curve it should have been played through.
    pub to: CurveSel,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Nr {
    pub enabled: bool,
    pub kind: NrKind,
    /// B-type reference level: where the NR level tone sits, dBFS RMS.
    pub ref_dbfs: f32,
    /// The reference level came from a tone on the tape (kept across presets).
    pub ref_measured: bool,
}
impl Default for Nr {
    fn default() -> Self {
        Nr { enabled: false, kind: NrKind::Btype, ref_dbfs: -18.0, ref_measured: false }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Dropout {
    pub enabled: bool,
    /// How far below the local level a dip must go, dB.
    pub depth_db: f32,
    /// Longest dip repaired; longer ones are flagged for manual repair.
    pub max_ms: f32,
    /// Longest dip rebuilt by interpolation; longer ones get gain compensation.
    pub interp_ms: f32,
}
impl Default for Dropout {
    fn default() -> Self {
        Dropout { enabled: false, depth_db: 6.0, max_ms: 300.0, interp_ms: 10.0 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BandLimit {
    pub enabled: bool,
    pub low_hz: f32,
    pub high_hz: f32,
    /// 12, 24, 36 or 48 dB/octave.
    pub slope_db_oct: u32,
}
impl Default for BandLimit {
    fn default() -> Self {
        BandLimit { enabled: false, low_hz: 50.0, high_hz: 15000.0, slope_db_oct: 24 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ErrorRepair {
    pub enabled: bool,
    /// Shortest run of identical samples counted as an error.
    pub min_run: u32,
    /// Longest error rebuilt; longer ones are flagged.
    pub max_ms: f32,
}
impl Default for ErrorRepair {
    fn default() -> Self {
        ErrorRepair { enabled: false, min_run: 4, max_ms: 10.0 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Deemphasis {
    pub enabled: bool,
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
    pub wow: Wow,
    pub azimuth: Azimuth,
    pub channel_mode: ChannelMode,
    pub playback_eq: PlaybackEq,
    pub nr: Nr,
    pub dropout: Dropout,
    pub band_limit: BandLimit,
    pub error_repair: ErrorRepair,
    pub deemphasis: Deemphasis,
    /// Digital source (DAT, MiniDisc, DCC): the digital chain runs instead.
    pub digital: bool,
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
        c.wow.enabled = false;
        c.azimuth.enabled = false;
        c.channel_mode.enabled = false;
        c.playback_eq.enabled = false;
        c.nr.enabled = false;
        c.dropout.enabled = false;
        c.band_limit.enabled = false;
        c.error_repair.enabled = false;
        c.deemphasis.enabled = false;
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
            || self.eq.enabled
            || self.wow.enabled
            || self.azimuth.enabled
            || self.channel_mode.enabled
            || self.playback_eq.enabled
            || self.nr.enabled
            || self.dropout.enabled
            || self.band_limit.enabled
            || self.error_repair.enabled
            || self.deemphasis.enabled)
    }

    /// Every stage added for tape and digital media is at its default (off).
    /// Such a chain is a version 1 chain.
    pub fn new_stages_default(&self) -> bool {
        self.wow == Wow::default()
            && self.azimuth == Azimuth::default()
            && self.channel_mode == ChannelMode::default()
            && self.playback_eq == PlaybackEq::default()
            && self.nr == Nr::default()
            && self.dropout == Dropout::default()
            && self.band_limit == BandLimit::default()
            && self.error_repair == ErrorRepair::default()
            && self.deemphasis == Deemphasis::default()
            && !self.digital
    }

    /// Channels this chain puts out for a source with `src_ch` channels.
    pub fn out_channels(&self, src_ch: usize) -> usize {
        if self.digital {
            src_ch
        } else {
            self.channel_mode.out_channels(src_ch)
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SpeedUnit {
    #[default]
    Rpm,
    /// Centimetres per second (cassette, microcassette, 8-track).
    Cms,
    /// Inches per second (reel to reel).
    Ips,
    /// No speed (video, digital).
    None,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Speed {
    /// The speed it was played at during capture. Older projects call it captured_rpm.
    #[serde(alias = "captured_rpm")]
    pub captured: f32,
    /// The speed it should have been played at. Older projects call it correct_rpm.
    #[serde(alias = "correct_rpm")]
    pub correct: f32,
    pub unit: SpeedUnit,
    /// Fine adjustment, cents. Positive raises pitch.
    pub cents: f32,
    /// Measured speed error of the playback, percent, positive when it ran fast:
    /// from a deck profile, a reference tone, or a known track length.
    pub trim_pct: f32,
}
impl Default for Speed {
    fn default() -> Self {
        Speed { captured: 33.333, correct: 33.333, unit: SpeedUnit::Rpm, cents: 0.0, trim_pct: 0.0 }
    }
}
impl Speed {
    /// Input samples consumed per output sample.
    pub fn ratio(&self) -> f64 {
        let mut r = (self.correct as f64 / self.captured.max(0.01) as f64) * 2f64.powf(self.cents as f64 / 1200.0);
        if self.trim_pct != 0.0 {
            r /= 1.0 + self.trim_pct as f64 / 100.0;
        }
        if (r - 1.0).abs() < 1e-9 {
            1.0
        } else {
            r
        }
    }

    /// trim_pct that makes something now `have_s` long come out `want_s` long.
    pub fn solve_trim_for_length(&self, have_s: f64, want_s: f64) -> f32 {
        if have_s <= 0.0 || want_s <= 0.0 {
            return self.trim_pct;
        }
        // length scales as 1/ratio; ratio scales as 1/(1 + trim/100)
        let cur = 1.0 + self.trim_pct as f64 / 100.0;
        ((cur * want_s / have_s - 1.0) * 100.0) as f32
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
    /// Wow and azimuth curves, dropout and error maps (see `analysis`).
    pub analysis: TapeAnalysis,
}

impl SourceFx {
    /// True when nothing added for tape is set: a version 1 SourceFx.
    pub fn new_fields_default(&self) -> bool {
        self.speed.unit == SpeedUnit::Rpm && self.speed.trim_pct == 0.0 && self.analysis.is_empty()
    }

    /// Wow map in effect for this chain, if any.
    pub fn wow_map<'a>(&'a self, chain: &Chain) -> Option<&'a crate::dsp::wow::WowMap> {
        if chain.wow.enabled && !chain.digital {
            self.analysis.wow_map()
        } else {
            None
        }
    }

    /// Source sample position -> processed sample position, through the static
    /// speed and the wow map when one is in effect.
    pub fn to_processed(&self, chain: &Chain, src_pos: f64) -> f64 {
        let r = self.speed.ratio();
        match self.wow_map(chain) {
            Some(m) => m.inverse(src_pos) / r,
            None => src_pos / r,
        }
    }
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

/// A dropout or digital error the chain found, in processed time.
#[derive(Clone, Copy, Debug)]
pub struct StageEvent {
    pub pos: i64,
    pub len: u32,
    pub kind: EventKind,
    pub depth_db: f32,
}

pub struct RenderOut {
    pub out: Vec<Vec<f32>>,
    pub pre: Option<Vec<Vec<f32>>>,
    pub removed: Option<Vec<Vec<f32>>>,
    pub clicks: Vec<ClickHit>,
    pub events: Vec<StageEvent>,
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

/// Padding for this chain: the base amount, or more when a stage needs a
/// longer look either side (dropout decisions, NR settling).
pub fn pad_for_chain(rate: u32, ch: &Chain) -> usize {
    let mut p = pad_for(rate);
    if ch.dropout.enabled {
        p = p.max((dropout::context_s() * rate as f64) as usize);
    }
    if ch.nr.enabled {
        p = p.max(rate as usize / 2);
    }
    p
}

/// Processed-time length of a source.
pub fn processed_frames(src: &SourceAudio, fx: &SourceFx) -> usize {
    (src.frames as f64 / fx.speed.ratio()).floor() as usize
}

fn mono_of(buf: &[Vec<f32>], mode: ChannelModeKind, best: Option<u8>) -> Vec<f32> {
    let pick = match mode {
        ChannelModeKind::MonoLeft => 0,
        ChannelModeKind::MonoRight => 1,
        ChannelModeKind::BestAuto => best.unwrap_or(2),
        _ => 2,
    };
    match pick {
        0 => buf[0].clone(),
        1 => buf[1].clone(),
        _ => buf[0].iter().zip(&buf[1]).map(|(a, b)| 0.5 * (a + b)).collect(),
    }
}

fn manual_repairs(c: &mut [f32], repairs: &[(i64, i64, Option<(f32, f32)>)], rate: u32, order: usize) {
    for &(s, e, band) in repairs {
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
}

fn repair_ranges(fx: &SourceFx, chain: &Chain, fs: f64, a: i64, total: usize) -> Vec<(i64, i64, Option<(f32, f32)>)> {
    let ratio = fx.speed.ratio();
    let wow = fx.wow_map(chain);
    fx.repairs
        .iter()
        .map(|r| {
            let (s, e) = match wow {
                None => ((r.start_s * fs / ratio).round() as i64 - a, (r.end_s * fs / ratio).round() as i64 - a),
                Some(_) => (fx.to_processed(chain, r.start_s * fs).round() as i64 - a, fx.to_processed(chain, r.end_s * fs).round() as i64 - a),
            };
            (s, e, r.band)
        })
        .filter(|(s, e, _)| *e > 0 && *s < total as i64)
        .collect()
}

fn band_limit_sections(fs: f64, b: &BandLimit) -> Vec<biquad::Biquad> {
    let order = ((b.slope_db_oct.clamp(12, 48) / 6) as usize).max(2);
    let mut v = Vec::new();
    if b.low_hz > 1.0 {
        v.extend(biquad::butter_highpass(fs, b.low_hz.clamp(5.0, 2000.0) as f64, order));
    }
    if b.high_hz > 0.0 && (b.high_hz as f64) < fs * 0.45 {
        v.extend(biquad::butter_lowpass(fs, (b.high_hz as f64).max(1000.0), order));
    }
    v
}

/// The digital chain: error repair, de-emphasis, manual repair. No DC filter.
fn render_digital(req: &RenderReq) -> RenderOut {
    let src = req.src;
    let rate = src.rate;
    let fs = rate as f64;
    let ch = req.chain;
    let pad = pad_for(rate);
    let a = req.start - pad as i64;
    let total = req.len + 2 * pad;
    let mut buf = src.read(a, a + total as i64);
    let pre = if req.want_pre || req.want_removed { Some(buf.clone()) } else { None };
    let order = ((rate as usize) / 2000).clamp(16, 48);
    let repairs = repair_ranges(req.fx, ch, fs, a, total);
    let er = ch.error_repair.clone();
    let de = ch.deemphasis.enabled;
    let events: Vec<Vec<StageEvent>> = buf
        .par_iter_mut()
        .map(|c| {
            let mut ev = Vec::new();
            if er.enabled {
                let found = dropout::find_errors(c, rate, er.min_run.max(2) as usize, er.max_ms.clamp(0.1, 100.0));
                dropout::repair_errors(c, &found, order);
                for e in found {
                    let kind = if e.fix == dropout::Fix::Flagged { EventKind::ErrorFlagged } else { EventKind::Error };
                    ev.push(StageEvent { pos: a + e.pos as i64, len: e.len as u32, kind, depth_db: 0.0 });
                }
            }
            if de {
                let mut s = tape_eq::deemphasis_50_15(fs);
                biquad::run_cascade(&mut s, c);
            }
            manual_repairs(c, &repairs, rate, order);
            ev
        })
        .collect();
    let removed = if req.want_removed {
        pre.as_ref().map(|p| p.iter().zip(&buf).map(|(a, b)| a.iter().zip(b).map(|(x, y)| x - y).collect()).collect())
    } else {
        None
    };
    let crop = |v: Vec<Vec<f32>>| -> Vec<Vec<f32>> { v.into_iter().map(|c| c[pad..pad + req.len].to_vec()).collect() };
    let (lo, hi) = (req.start, req.start + req.len as i64);
    let mut events: Vec<StageEvent> = if req.want_clicks { events.into_iter().flatten().filter(|e| e.pos >= lo && e.pos < hi).collect() } else { Vec::new() };
    events.sort_by_key(|e| e.pos);
    RenderOut { out: crop(buf), pre: if req.want_pre { pre.map(crop) } else { None }, removed: removed.map(crop), clicks: Vec::new(), events }
}

fn ms(rate: u32, v: f32) -> usize {
    ((v / 1000.0) * rate as f32).round().max(1.0) as usize
}

pub fn render(req: &RenderReq) -> RenderOut {
    if req.chain.digital && !req.passthrough {
        return render_digital(req);
    }
    let src = req.src;
    let rate = src.rate;
    let fs = rate as f64;
    let pad = if req.passthrough { 0 } else { pad_for_chain(rate, req.chain) };
    let ratio = req.fx.speed.ratio();
    let wow = if req.passthrough { None } else { req.fx.wow_map(req.chain) };
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
    let mut buf: Vec<Vec<f32>> = if req.passthrough || (ratio == 1.0 && wow.is_none()) {
        src.read(a, a + total as i64)
    } else if let Some(map) = wow {
        // wow: the read position follows the measured speed curve
        let it = resample::Interp::new(ratio);
        let reach = it.reach() as i64 + 2;
        let p0 = map.at(a as f64 * ratio);
        let p1 = map.at((a + total as i64) as f64 * ratio);
        let r0 = p0.min(p1).floor() as i64 - reach;
        let r1 = p0.max(p1).ceil() as i64 + reach;
        let pos: Vec<f64> = (0..total).map(|n| map.at((a + n as i64) as f64 * ratio) - r0 as f64).collect();
        (0..src.channels)
            .into_par_iter()
            .map(|c| {
                let raw = src.read_channel(c, r0, r1);
                pos.iter().map(|p| it.at(&raw, *p)).collect()
            })
            .collect()
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
            events: Vec::new(),
        };
    }

    let ch = req.chain;
    // 1. DC offset (5 Hz highpass), always on
    buf.par_iter_mut().for_each(|c| {
        let mut f = biquad::butter_highpass(fs, 5.0, 2);
        biquad::run_cascade(&mut f, c);
    });

    // 3. azimuth (tape): line the right channel up with the left
    if ch.azimuth.enabled && buf.len() == 2 {
        let curve = req.fx.analysis.azimuth();
        let tracked = match ch.azimuth.mode {
            AzimuthMode::Static => None,
            AzimuthMode::Tracked => curve,
            AzimuthMode::Auto => curve.filter(|c| !c.is_static()),
        };
        match tracked {
            Some(c) => {
                let r = &buf[1];
                let out = azimuth::tracked_delay(r, |n| c.at((a + n as i64) as f64 * ratio / fs) as f64 * 1e-6 * fs);
                buf[1] = out;
            }
            None => {
                let us = match (ch.azimuth.mode, curve) {
                    (AzimuthMode::Auto, Some(c)) => c.mean(),
                    _ => ch.azimuth.static_us,
                };
                let d = us.clamp(-1000.0, 1000.0) as f64 * 1e-6 * fs;
                if d.abs() > 1e-3 {
                    buf[1] = resample::frac_delay(&buf[1], d);
                }
            }
        }
    }

    // 4. channel mode: mono sum, one channel, or the better one
    if ch.channel_mode.enabled && buf.len() == 2 && ch.channel_mode.mode != ChannelModeKind::Stereo {
        let m = mono_of(&buf, ch.channel_mode.mode, req.fx.analysis.best_channel);
        buf = vec![m];
    }

    let pre = if req.want_pre || req.want_removed { Some(buf.clone()) } else { None };

    // 5. playback EQ correction
    if ch.playback_eq.enabled {
        let sections = tape_eq::correction(fs, &ch.playback_eq.from.curve, &ch.playback_eq.to.curve);
        if !sections.is_empty() {
            buf.par_iter_mut().for_each(|c| {
                let mut s = sections.clone();
                biquad::run_cascade(&mut s, c);
            });
        }
    }

    // Dropouts are found before NR decode when it is on: the decoder's release
    // smears a dropout's edges, and its sharp edges are how it is told from music.
    let dropout_params = dropout::DropoutParams { depth_db: ch.dropout.depth_db.clamp(2.0, 40.0), min_ms: 2.0, max_ms: ch.dropout.max_ms.clamp(10.0, 2000.0), interp_ms: ch.dropout.interp_ms.clamp(0.0, 50.0) };
    let early_dropouts: Option<Vec<Vec<dropout::Event>>> = if ch.dropout.enabled && ch.nr.enabled {
        Some(buf.par_iter().map(|c| dropout::find_dropouts(c, rate, a, &dropout_params)).collect())
    } else {
        None
    };

    // 6. NR decode
    if ch.nr.enabled {
        let (kind, r) = (ch.nr.kind, ch.nr.ref_dbfs.clamp(-60.0, 0.0));
        buf.par_iter_mut().for_each(|c| nr::decode(c, fs, kind, r));
    }

    // 7. rumble
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

    // 8. hum and buzz
    if ch.hum.enabled {
        let f0 = ch.hum.mains_hz.clamp(20.0, 1000.0) as f64;
        let w = ch.hum.width_hz.clamp(0.2, 10.0) as f64;
        let h = ch.hum.harmonics.clamp(1, 256) as usize;
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

    // 9-14 per channel, in parallel
    let order = ((rate as usize) / 2000).clamp(16, 48);
    let repairs = repair_ranges(req.fx, ch, fs, a, total);
    let dc = ch.declick.clone();
    let dk = ch.decrackle.clone();
    let dl = ch.declip.clone();
    let dn = ch.denoise.clone();
    let dr = ch.dropout.clone();
    let profile = req.fx.noise.clone();
    // early detections were made before the run-up was dropped
    let early: Option<Vec<Vec<dropout::Event>>> = early_dropouts.map(|v| {
        v.into_iter()
            .map(|evs| {
                evs.into_iter()
                    .filter(|e| e.pos >= warm)
                    .map(|mut e| {
                        e.pos -= warm;
                        e
                    })
                    .collect()
            })
            .collect()
    });
    let per_ch: Vec<(Vec<ClickHit>, Vec<StageEvent>)> = buf
        .par_iter_mut()
        .enumerate()
        .map(|(ci, c)| {
            let mut hits = Vec::new();
            let mut events = Vec::new();
            // 9. dropout repair
            if dr.enabled {
                let found = match &early {
                    Some(v) => v[ci].clone(),
                    None => dropout::find_dropouts(c, rate, a, &dropout_params),
                };
                dropout::repair_dropouts(c, rate, &found, order);
                for e in found {
                    let kind = if e.fix == dropout::Fix::Flagged { EventKind::DropoutFlagged } else { EventKind::Dropout };
                    events.push(StageEvent { pos: a + e.pos as i64, len: e.len as u32, kind, depth_db: e.depth_db });
                }
            }
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
            // 12. manual repair
            manual_repairs(c, &repairs, rate, order);
            // 13. declip
            if dl.enabled {
                let thr = 10f32.powf(dl.level_db.min(0.0) / 20.0);
                declick::declip(c, thr, ms(rate, dl.max_len_ms.clamp(0.1, 5.0)), order);
            }
            // 14. denoise
            if dn.enabled {
                if let Some(p) = &profile {
                    let prm = denoise::DenoiseParams { reduction_db: dn.reduction_db.clamp(0.0, 30.0), smoothing: dn.smoothing_pct / 100.0 };
                    denoise::denoise(c, a, rate, p, &prm);
                }
            }
            (hits, events)
        })
        .collect();

    // 15. band limit
    if ch.band_limit.enabled {
        let sections = band_limit_sections(fs, &ch.band_limit);
        buf.par_iter_mut().for_each(|c| {
            let mut s = sections.clone();
            biquad::run_cascade(&mut s, c);
        });
    }

    let removed = if req.want_removed {
        pre.as_ref().map(|p| p.iter().zip(&buf).map(|(a, b)| a.iter().zip(b).map(|(x, y)| x - y).collect()).collect())
    } else {
        None
    };

    // 16. channel tools
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

    // 17. EQ
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
    let (clicks_per_ch, events_per_ch): (Vec<Vec<ClickHit>>, Vec<Vec<StageEvent>>) = per_ch.into_iter().unzip();
    let mut clicks: Vec<ClickHit> = if req.want_clicks {
        clicks_per_ch.into_iter().flatten().filter(|h| h.pos >= lo && h.pos < hi).collect()
    } else {
        Vec::new()
    };
    clicks.sort_by_key(|h| h.pos);
    let mut events: Vec<StageEvent> = if req.want_clicks { events_per_ch.into_iter().flatten().filter(|e| e.pos >= lo && e.pos < hi).collect() } else { Vec::new() };
    events.sort_by_key(|e| e.pos);
    RenderOut {
        out: crop(buf),
        pre: if req.want_pre { pre.map(crop) } else { None },
        removed: removed.map(crop),
        clicks,
        events,
    }
}
