//! The medium layer: what a recording is, and how that sets up a project.
//!
//! Three layers, applied in order:
//!
//! 1. Medium preset: what the thing is (Vinyl LP, Cassette Type II, DAT...).
//!    It sets the capture path, the speed units, which stages exist and their
//!    defaults, the split strategy and the export.
//! 2. Condition preset: how worn it is (`presets`), scoped to the medium's family.
//! 3. Deck profile: what the playback machine does wrong (speed error, azimuth
//!    offset, channel balance, EQ trim). A turntable is a deck too.
//!
//! A project stores the resolved values, so editing a preset later never
//! changes an existing project.
//!
//! Built-in medium presets live in code. User medium presets live as JSON in
//! $XDG_CONFIG_HOME/runout/media/, each holding a base id and only the fields
//! that differ from it, so a fix to a built-in reaches presets built on it.
//! Deck profiles live in $XDG_CONFIG_HOME/runout/decks/.

use crate::chain::{BandLimit, Chain, ChannelModeKind, CurveSel, SpeedUnit, WowRef};
use crate::dsp::nr::NrKind;
use crate::presets::{self, Preset};
use crate::project::{ExportKind, Project, SplitMode, SplitSettings};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    #[default]
    Vinyl,
    Cassette,
    Cartridge,
    Reel,
    Video,
    Digital,
    Generic,
}

impl Family {
    pub fn all() -> [Family; 7] {
        [Family::Vinyl, Family::Cassette, Family::Cartridge, Family::Reel, Family::Video, Family::Digital, Family::Generic]
    }
    pub fn label(&self) -> &'static str {
        match self {
            Family::Vinyl => "Vinyl",
            Family::Cassette => "Cassette",
            Family::Cartridge => "Cartridge",
            Family::Reel => "Reel",
            Family::Video => "Video",
            Family::Digital => "Digital",
            Family::Generic => "Generic",
        }
    }
    /// Tape-like families share the Clean / Worn / Rough condition presets.
    pub fn is_tape(&self) -> bool {
        matches!(self, Family::Cassette | Family::Cartridge | Family::Reel | Family::Video)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SideNaming {
    /// Side A, Side B
    #[default]
    SideAb,
    /// Program 1 to 4
    Program,
    /// Reel 1, Reel 2
    Reel,
    /// Tape, Tape 2
    Tape,
}

impl SideNaming {
    /// Name of the n-th side or part (0-based).
    pub fn name(&self, n: usize) -> String {
        match self {
            SideNaming::SideAb => format!("Side {}", (b'A' + (n as u8).min(25)) as char),
            SideNaming::Program => format!("Program {}", n + 1),
            SideNaming::Reel => format!("Reel {}", n + 1),
            SideNaming::Tape => {
                if n == 0 {
                    "Tape".into()
                } else {
                    format!("Tape {}", n + 1)
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CapturePath {
    #[default]
    Analog,
    /// Coax or optical into an interface with a digital input.
    Spdif,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StopRule {
    /// The run-out groove (vinyl).
    #[default]
    Runout,
    /// Signal under the hiss floor: the tape ran out.
    TapeEnd,
    /// Exact digital silence.
    DigitalSilence,
    /// Never stop on its own.
    Never,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SpeedSpec {
    pub unit: SpeedUnit,
    pub nominal: f32,
    /// What the Speed card offers.
    pub choices: Vec<f32>,
}
impl Default for SpeedSpec {
    fn default() -> Self {
        SpeedSpec { unit: SpeedUnit::Rpm, nominal: 33.333, choices: vec![33.333, 45.0] }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CaptureSpec {
    pub path: CapturePath,
    /// Preferred rate and depth; the device's best otherwise.
    pub rate: u32,
    pub bits: u16,
    /// Next choice when the device does not offer `rate`.
    pub fallback_rate: u32,
    pub stop: StopRule,
}
impl Default for CaptureSpec {
    fn default() -> Self {
        CaptureSpec { path: CapturePath::Analog, rate: 48000, bits: 16, fallback_rate: 44100, stop: StopRule::Runout }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ExportSpec {
    pub kind: ExportKind,
    /// Output rate; 0 keeps the capture rate.
    pub rate: u32,
    /// Keep the capture rate when the tape ran at least this fast (ips): reel
    /// at 7.5 and 15 ips has content above 20 kHz worth keeping. 0 = never.
    pub full_rate_from_speed: f32,
    pub bits: u16,
    pub dither: bool,
}
impl Default for ExportSpec {
    fn default() -> Self {
        ExportSpec { kind: ExportKind::Album, rate: 0, full_rate_from_speed: 0.0, bits: 16, dither: true }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MediumPreset {
    pub id: String,
    pub name: String,
    pub family: Family,
    pub builtin: bool,
    /// User presets: the built-in this one is built on.
    pub base: Option<String>,
    pub side_naming: SideNaming,
    pub default_condition: Option<String>,
    pub speed: SpeedSpec,
    pub capture: CaptureSpec,
    /// Every stage's default on/off and settings, channel mode, playback EQ,
    /// DNR, wow reference, band limit corners and leveling.
    pub chain: Chain,
    /// Stage cards shown in the Clean panel, in chain order.
    pub visible: Vec<String>,
    pub split: SplitSettings,
    pub export: ExportSpec,
    /// MEDIA tag value (the names MusicBrainz and Picard use). Empty: no tag.
    pub media_tag: String,
    /// MusicBrainz format for the tracklist lookup. Empty: no filter.
    pub mb_format: String,
    pub notes: String,
}

impl Default for MediumPreset {
    fn default() -> Self {
        vinyl_lp()
    }
}

/// The medium a project uses: its preset id and the values resolved from it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MediumRef {
    pub id: String,
    pub preset: MediumPreset,
    /// Condition preset applied over it, if any.
    pub condition: Option<String>,
}

impl Default for MediumRef {
    fn default() -> Self {
        MediumRef { id: "vinyl-lp".into(), preset: vinyl_lp(), condition: None }
    }
}

/// Stage ids, in chain order, for `visible`.
pub const STAGES: &[&str] = &[
    "speed", "wow", "azimuth", "channel_mode", "playback_eq", "nr", "rumble", "hum", "dropout", "declick", "decrackle", "declip", "denoise", "band_limit", "channel", "eq", "error_repair", "deemphasis", "level",
];

fn vis(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

const VINYL_STAGES: &[&str] = &["speed", "rumble", "hum", "declick", "decrackle", "declip", "denoise", "channel", "eq", "level"];
const TAPE_STAGES: &[&str] = &["speed", "wow", "azimuth", "channel_mode", "playback_eq", "nr", "rumble", "hum", "dropout", "declick", "declip", "denoise", "band_limit", "channel", "eq", "level"];
const DIGITAL_STAGES: &[&str] = &["error_repair", "deemphasis", "level"];

fn vinyl_base(id: &str, name: &str, rpm: f32, choices: Vec<f32>) -> MediumPreset {
    MediumPreset {
        id: id.into(),
        name: name.into(),
        family: Family::Vinyl,
        builtin: true,
        base: None,
        side_naming: SideNaming::SideAb,
        default_condition: None,
        speed: SpeedSpec { unit: SpeedUnit::Rpm, nominal: rpm, choices },
        capture: CaptureSpec::default(),
        chain: Chain::default(),
        visible: vis(VINYL_STAGES),
        split: SplitSettings::default(),
        export: ExportSpec::default(),
        media_tag: "Vinyl".into(),
        mb_format: "Vinyl".into(),
        notes: String::new(),
    }
}

/// Today's Runout: the first-run default.
pub fn vinyl_lp() -> MediumPreset {
    vinyl_base("vinyl-lp", "Vinyl LP 33 1/3", 33.333, vec![33.333, 45.0])
}

/// Tape chain defaults: hiss reduction and dropout repair on, the vinyl-only
/// stages off, the rumble filter acting as a plain high-pass.
fn tape_chain() -> Chain {
    let mut c = Chain::default();
    c.declick.enabled = false;
    c.decrackle.enabled = false;
    c.rumble.freq_hz = 20.0;
    c.denoise.enabled = true;
    c.denoise.reduction_db = 6.0;
    c.dropout.enabled = true;
    c.wow.reference = WowRef::Hum;
    c
}

fn tape_base(id: &str, name: &str, family: Family, unit: SpeedUnit, nominal: f32, choices: Vec<f32>) -> MediumPreset {
    MediumPreset {
        id: id.into(),
        name: name.into(),
        family,
        builtin: true,
        base: None,
        side_naming: SideNaming::SideAb,
        default_condition: Some(format!("{}-clean", family.label().to_lowercase())),
        speed: SpeedSpec { unit, nominal, choices },
        capture: CaptureSpec { path: CapturePath::Analog, rate: 96000, bits: 24, fallback_rate: 48000, stop: StopRule::TapeEnd },
        chain: tape_chain(),
        visible: vis(TAPE_STAGES),
        split: SplitSettings { mode: SplitMode::NoiseFloor, min_gap_s: 1.5, ..Default::default() },
        export: ExportSpec { kind: ExportKind::Album, rate: 48000, full_rate_from_speed: 0.0, bits: 24, dither: false },
        media_tag: "Cassette".into(),
        mb_format: "Cassette".into(),
        notes: String::new(),
    }
}

fn mono(c: &mut Chain, mode: ChannelModeKind) {
    c.channel_mode.enabled = true;
    c.channel_mode.mode = mode;
}

fn eq_pair(c: &mut Chain, id: &str) {
    // From and to the same: off, but the card is set up for a one-click change.
    c.playback_eq.from = CurveSel::named(id);
    c.playback_eq.to = CurveSel::named(id);
    c.playback_eq.enabled = false;
}

fn band(c: &mut Chain, lo: f32, hi: f32) {
    c.band_limit = BandLimit { enabled: true, low_hz: lo, high_hz: hi, slope_db_oct: 24 };
}

fn digital_base(id: &str, name: &str, tag: &str, rate: u32) -> MediumPreset {
    let mut c = Chain::passthrough();
    c.digital = true;
    MediumPreset {
        id: id.into(),
        name: name.into(),
        family: Family::Digital,
        builtin: true,
        base: None,
        side_naming: SideNaming::Tape,
        default_condition: None,
        speed: SpeedSpec { unit: SpeedUnit::None, nominal: 1.0, choices: vec![] },
        capture: CaptureSpec { path: CapturePath::Spdif, rate, bits: 24, fallback_rate: 0, stop: StopRule::DigitalSilence },
        chain: c,
        visible: vis(DIGITAL_STAGES),
        split: SplitSettings { mode: SplitMode::Absolute, threshold_db: -70.0, min_gap_s: 1.0, ..Default::default() },
        export: ExportSpec { kind: ExportKind::Album, rate: 0, full_rate_from_speed: 0.0, bits: 0, dither: false },
        media_tag: tag.into(),
        mb_format: tag.into(),
        notes: "Bit-perfect over S/PDIF. With every stage off the FLAC decodes to exactly what the deck sent.".into(),
    }
}

/// Every built-in medium preset, in picker order.
pub fn builtin() -> Vec<MediumPreset> {
    let mut v = Vec::new();

    // Vinyl
    v.push(vinyl_lp());
    v.push(vinyl_base("vinyl-45", "Vinyl Single 45", 45.0, vec![45.0, 33.333]));
    let mut m = vinyl_base("vinyl-mono-lp", "Vinyl Mono LP", 33.333, vec![33.333, 45.0]);
    mono(&mut m.chain, ChannelModeKind::MonoSum);
    m.visible = vis(&["speed", "channel_mode", "rumble", "hum", "declick", "decrackle", "declip", "denoise", "channel", "eq", "level"]);
    m.notes = "Summing the channels cancels the vertical groove noise a mono groove carries. Best channel is there for a record with one worn wall.".into();
    v.push(m);
    let mut m = vinyl_base("vinyl-78", "Vinyl 78 rpm", 78.26, vec![78.26, 77.92, 78.0, 80.0]);
    mono(&mut m.chain, ChannelModeKind::BestAuto);
    m.chain.playback_eq.enabled = true;
    m.chain.playback_eq.from = CurveSel::named("riaa");
    m.chain.playback_eq.to = CurveSel::named("american-78-6");
    m.chain.declick.threshold_db = 11.0;
    m.chain.declick.max_len_ms = 3.0;
    m.chain.decrackle.threshold_db = 9.0;
    m.chain.rumble.freq_hz = 30.0;
    m.chain.denoise.reduction_db = 8.0;
    band(&mut m.chain, 50.0, 8000.0);
    m.capture = CaptureSpec { path: CapturePath::Analog, rate: 96000, bits: 24, fallback_rate: 48000, stop: StopRule::Runout };
    m.export = ExportSpec { kind: ExportKind::Album, rate: 48000, full_rate_from_speed: 0.0, bits: 24, dither: false };
    m.visible = vis(&["speed", "channel_mode", "playback_eq", "rumble", "hum", "declick", "decrackle", "declip", "denoise", "band_limit", "channel", "eq", "level"]);
    m.notes = "78.26 rpm in 60 Hz countries, 77.92 in 50 Hz ones. Pick the label's EQ curve. 78s need a wider stylus than LPs.".into();
    v.push(m);

    // Cassette
    let mut m = tape_base("cassette-type-i", "Cassette Type I", Family::Cassette, SpeedUnit::Cms, 4.76, vec![4.76, 9.53]);
    eq_pair(&mut m.chain, "cassette-120");
    v.push(m);
    let mut m = tape_base("cassette-type-ii", "Cassette Type II / IV", Family::Cassette, SpeedUnit::Cms, 4.76, vec![4.76, 9.53]);
    eq_pair(&mut m.chain, "cassette-70");
    v.push(m);
    let mut m = tape_base("cassette-spoken", "Cassette Spoken word", Family::Cassette, SpeedUnit::Cms, 4.76, vec![4.76, 2.38]);
    eq_pair(&mut m.chain, "cassette-120");
    mono(&mut m.chain, ChannelModeKind::BestAuto);
    band(&mut m.chain, 80.0, 10000.0);
    m.chain.level.target_lufs = -16.0;
    m.visible.retain(|s| s != "nr");
    m.export.kind = ExportKind::Recording;
    v.push(m);
    let mut m = tape_base("microcassette", "Microcassette", Family::Cassette, SpeedUnit::Cms, 2.4, vec![2.4, 1.2]);
    mono(&mut m.chain, ChannelModeKind::BestAuto);
    band(&mut m.chain, 150.0, 6000.0);
    m.chain.level.target_lufs = -16.0;
    m.visible.retain(|s| s != "nr" && s != "playback_eq");
    m.export.kind = ExportKind::Recording;
    m.media_tag = "Microcassette".into();
    m.mb_format = "Microcassette".into();
    v.push(m);

    // Cartridge
    let mut m = tape_base("8-track", "8-track", Family::Cartridge, SpeedUnit::Cms, 9.5, vec![9.5]);
    m.side_naming = SideNaming::Program;
    m.split.mode = SplitMode::ProgramChange;
    m.visible.retain(|s| s != "nr" && s != "playback_eq");
    m.media_tag = "8-Track Cartridge".into();
    m.mb_format = "8-Track Cartridge".into();
    m.notes = "9.5 cm/s (3 3/4 ips). Find program changes in the Split panel finds the track switches.".into();
    v.push(m);

    // Reel
    for (id, name, choices, stereo, dnr) in [
        ("reel-half-track", "Reel 1/4\" half-track", vec![1.875, 3.75, 7.5, 15.0], true, true),
        ("reel-quarter-track", "Reel 1/4\" quarter-track", vec![1.875, 3.75, 7.5], true, true),
        ("reel-full-track", "Reel full-track mono", vec![1.875, 3.75, 7.5, 15.0], false, false),
    ] {
        let mut m = tape_base(id, name, Family::Reel, SpeedUnit::Ips, 7.5, choices);
        eq_pair(&mut m.chain, "nab-50");
        m.chain.nr.kind = NrKind::Dbx2;
        if !stereo {
            mono(&mut m.chain, ChannelModeKind::MonoSum);
        }
        if !dnr {
            m.visible.retain(|s| s != "nr");
        }
        m.side_naming = SideNaming::Reel;
        m.export.full_rate_from_speed = 7.5;
        m.media_tag = "Reel-to-reel".into();
        m.mb_format = "Reel-to-reel".into();
        m.notes = "NAB or IEC per speed: NAB 3180/50 us at 7.5 and 15 ips, 3180/90 at 3.75 and 1.875; IEC 35 us at 15, 70 at 7.5, 3180/90 at 3.75.".into();
        v.push(m);
    }

    // Video
    let mut m = tape_base("video-hifi", "VHS / Beta Hi-Fi", Family::Video, SpeedUnit::None, 1.0, vec![]);
    m.chain.denoise.enabled = false;
    m.chain.dropout.enabled = false;
    m.chain.hum.enabled = true;
    m.chain.hum.mains_hz = 59.94;
    m.chain.hum.harmonics = 166; // to 10 kHz
    m.chain.hum.width_hz = 1.0;
    m.visible = vis(&["hum", "dropout", "declick", "declip", "denoise", "band_limit", "channel", "eq", "level"]);
    m.side_naming = SideNaming::Tape;
    m.export.kind = ExportKind::Recording;
    m.media_tag = "VHS".into();
    m.mb_format = "VHS".into();
    m.split.mode = SplitMode::FixedLength;
    m.notes = "Head-switching buzz is the video field rate and its harmonics: 59.94 Hz for NTSC, 50 Hz for PAL.".into();
    v.push(m);
    let mut m = tape_base("video-linear", "VHS linear track", Family::Video, SpeedUnit::None, 1.0, vec![]);
    mono(&mut m.chain, ChannelModeKind::MonoSum);
    band(&mut m.chain, 100.0, 10000.0);
    m.visible = vis(&["channel_mode", "hum", "dropout", "declick", "declip", "denoise", "band_limit", "channel", "eq", "level"]);
    m.side_naming = SideNaming::Tape;
    m.export.kind = ExportKind::Recording;
    m.media_tag = "VHS".into();
    m.mb_format = "VHS".into();
    m.split.mode = SplitMode::FixedLength;
    v.push(m);
    let mut m = tape_base("laserdisc", "LaserDisc analog", Family::Video, SpeedUnit::None, 1.0, vec![]);
    m.chain.denoise.reduction_db = 3.0;
    m.chain.dropout.enabled = false;
    m.visible = vis(&["hum", "dropout", "declick", "declip", "denoise", "band_limit", "channel", "eq", "level"]);
    m.side_naming = SideNaming::SideAb;
    m.media_tag = "LaserDisc".into();
    m.mb_format = "LaserDisc".into();
    v.push(m);

    // Digital
    v.push(digital_base("dat", "DAT", "DAT", 48000));
    v.push(digital_base("minidisc", "MiniDisc", "MiniDisc", 44100));
    v.push(digital_base("dcc", "DCC", "DCC", 48000));

    // Generic
    let mut c = Chain::passthrough();
    c.level.enabled = true;
    v.push(MediumPreset {
        id: "generic-analog".into(),
        name: "Analog line-in".into(),
        family: Family::Generic,
        builtin: true,
        base: None,
        side_naming: SideNaming::Tape,
        default_condition: None,
        speed: SpeedSpec { unit: SpeedUnit::None, nominal: 1.0, choices: vec![] },
        capture: CaptureSpec { path: CapturePath::Analog, rate: 96000, bits: 24, fallback_rate: 48000, stop: StopRule::Never },
        chain: c,
        visible: STAGES.iter().filter(|s| **s != "error_repair" && **s != "deemphasis").map(|s| s.to_string()).collect(),
        split: SplitSettings { mode: SplitMode::NoiseFloor, min_gap_s: 1.5, ..Default::default() },
        export: ExportSpec { kind: ExportKind::Recording, rate: 48000, full_rate_from_speed: 0.0, bits: 24, dither: false },
        media_tag: String::new(),
        mb_format: String::new(),
        notes: "Wire recordings, dictation belts, radio airchecks, anything with a line out. Everything off but DC removal and leveling.".into(),
    });
    let mut d = digital_base("generic-digital", "Digital S/PDIF", "", 0);
    d.family = Family::Generic;
    d.capture.rate = 0;
    d.notes = "Any rate. Everything off.".into();
    v.push(d);
    v
}

fn config_dir() -> PathBuf {
    presets::user_dir().parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."))
}

pub fn user_media_dir() -> PathBuf {
    config_dir().join("media")
}

pub fn decks_dir() -> PathBuf {
    config_dir().join("decks")
}

/// What a user medium preset file holds.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct UserFile {
    id: String,
    name: String,
    base: String,
    /// Only the fields that differ from the base.
    #[serde(default)]
    diff: Value,
}

/// Fields of `b` that differ from `a`, recursively through objects.
pub fn json_diff(a: &Value, b: &Value) -> Option<Value> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let mut m = Map::new();
            for (k, vb) in y {
                match x.get(k) {
                    Some(va) => {
                        if let Some(d) = json_diff(va, vb) {
                            m.insert(k.clone(), d);
                        }
                    }
                    None => {
                        m.insert(k.clone(), vb.clone());
                    }
                }
            }
            if m.is_empty() {
                None
            } else {
                Some(Value::Object(m))
            }
        }
        _ => {
            if a == b {
                None
            } else {
                Some(b.clone())
            }
        }
    }
}

/// `base` with `diff` laid over it.
pub fn json_merge(base: &mut Value, diff: &Value) {
    match (base, diff) {
        (Value::Object(b), Value::Object(d)) => {
            for (k, v) in d {
                match b.get_mut(k) {
                    Some(bv) if bv.is_object() && v.is_object() => json_merge(bv, v),
                    _ => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, d) => *b = d.clone(),
    }
}

fn from_user(u: &UserFile) -> Option<MediumPreset> {
    let base = builtin().into_iter().find(|b| b.id == u.base)?;
    let mut v = serde_json::to_value(&base).ok()?;
    json_merge(&mut v, &u.diff);
    let mut p: MediumPreset = serde_json::from_value(v).ok()?;
    p.id = u.id.clone();
    p.name = u.name.clone();
    p.base = Some(u.base.clone());
    p.builtin = false;
    Some(p)
}

/// Built-ins first, then the user's own, sorted by name.
pub fn list() -> Vec<MediumPreset> {
    let mut v = builtin();
    if let Ok(rd) = std::fs::read_dir(user_media_dir()) {
        let mut user: Vec<MediumPreset> = rd
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|s| serde_json::from_str::<UserFile>(&s).ok())
            .filter_map(|u| from_user(&u))
            .collect();
        user.sort_by(|a, b| a.name.cmp(&b.name));
        v.extend(user);
    }
    v
}

pub fn find(id: &str) -> Option<MediumPreset> {
    let key = presets::slug(id);
    list().into_iter().find(|m| m.id == id || m.id == key || presets::slug(&m.name) == key)
}

/// The built-in a preset ultimately rests on.
pub fn base_of(p: &MediumPreset) -> Option<MediumPreset> {
    let id = p.base.clone().unwrap_or_else(|| p.id.clone());
    builtin().into_iter().find(|b| b.id == id)
}

/// Save `p` under `name` as a user preset: its base and only what differs.
pub fn save(name: &str, p: &MediumPreset) -> Result<MediumPreset> {
    let base = base_of(p).ok_or_else(|| Error::Project("this medium has no built-in base".into()))?;
    let id = presets::slug(name);
    if builtin().iter().any(|b| b.id == id) {
        return Err(Error::Project("that name belongs to a built-in medium".into()));
    }
    let mut mine = p.clone();
    mine.id = base.id.clone();
    mine.name = base.name.clone();
    mine.base = None;
    mine.builtin = true;
    let a = serde_json::to_value(&base).map_err(|e| Error::Project(e.to_string()))?;
    let b = serde_json::to_value(&mine).map_err(|e| Error::Project(e.to_string()))?;
    let diff = json_diff(&a, &b).unwrap_or(Value::Object(Map::new()));
    let u = UserFile { id: id.clone(), name: name.trim().to_string(), base: base.id.clone(), diff };
    let dir = user_media_dir();
    std::fs::create_dir_all(&dir).map_err(|e| Error::Io(e.to_string()))?;
    let txt = serde_json::to_string_pretty(&u).map_err(|e| Error::Project(e.to_string()))?;
    std::fs::write(dir.join(format!("{id}.json")), txt).map_err(|e| Error::Io(e.to_string()))?;
    from_user(&u).ok_or_else(|| Error::Project("could not read the preset back".into()))
}

pub fn delete(id: &str) -> Result<()> {
    std::fs::remove_file(user_media_dir().join(format!("{}.json", presets::slug(id)))).map_err(|e| Error::Io(e.to_string()))
}

/// A playback machine's known faults, applied whenever it is picked.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct DeckProfile {
    pub id: String,
    pub name: String,
    pub family: Family,
    /// How fast it runs, percent (positive: fast).
    pub speed_pct: f32,
    /// Its right-channel azimuth offset, microseconds.
    pub azimuth_us: f32,
    /// Balance correction, dB (positive moves the image right).
    pub balance_db: f32,
    /// EQ trim, dB.
    pub eq_low_db: f32,
    pub eq_high_db: f32,
}

pub fn decks() -> Vec<DeckProfile> {
    let mut v: Vec<DeckProfile> = std::fs::read_dir(decks_dir())
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .filter_map(|s| serde_json::from_str(&s).ok())
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

pub fn find_deck(id: &str) -> Option<DeckProfile> {
    decks().into_iter().find(|d| d.id == id || presets::slug(&d.name) == presets::slug(id))
}

pub fn save_deck(d: &DeckProfile) -> Result<DeckProfile> {
    let mut d = d.clone();
    if d.name.trim().is_empty() {
        return Err(Error::Project("give the deck a name".into()));
    }
    d.id = presets::slug(&d.name);
    let dir = decks_dir();
    std::fs::create_dir_all(&dir).map_err(|e| Error::Io(e.to_string()))?;
    let txt = serde_json::to_string_pretty(&d).map_err(|e| Error::Project(e.to_string()))?;
    std::fs::write(dir.join(format!("{}.json", d.id)), txt).map_err(|e| Error::Io(e.to_string()))?;
    Ok(d)
}

pub fn delete_deck(id: &str) -> Result<()> {
    std::fs::remove_file(decks_dir().join(format!("{}.json", presets::slug(id)))).map_err(|e| Error::Io(e.to_string()))
}

/// Resolve medium + condition + deck into a chain.
pub fn resolve_chain(m: &MediumPreset, condition: Option<&Preset>, deck: Option<&DeckProfile>) -> Chain {
    let mut c = m.chain.clone();
    if let Some(pr) = condition {
        c = presets::apply(&c, pr);
    }
    if let Some(d) = deck {
        apply_deck_chain(&mut c, m.family, d);
    }
    c
}

fn apply_deck_chain(c: &mut Chain, family: Family, d: &DeckProfile) {
    if d.balance_db != 0.0 {
        c.channel.enabled = true;
        c.channel.balance_db = d.balance_db;
    }
    if d.azimuth_us != 0.0 {
        if family == Family::Vinyl {
            c.channel.enabled = true;
            c.channel.azimuth_us = d.azimuth_us;
        } else {
            c.azimuth.enabled = true;
            c.azimuth.static_us = d.azimuth_us;
        }
    }
    if d.eq_low_db != 0.0 || d.eq_high_db != 0.0 {
        c.eq.enabled = true;
        c.eq.low_shelf_db = d.eq_low_db;
        c.eq.high_shelf_db = d.eq_high_db;
    }
}

/// Speed settings a new source of this medium starts with.
pub fn source_speed(m: &MediumPreset, deck: Option<&DeckProfile>) -> crate::chain::Speed {
    let mut s = crate::chain::Speed { captured: m.speed.nominal, correct: m.speed.nominal, unit: m.speed.unit, cents: 0.0, trim_pct: 0.0 };
    if m.speed.unit == SpeedUnit::None {
        s.captured = 1.0;
        s.correct = 1.0;
    }
    if let Some(d) = deck {
        s.trim_pct = d.speed_pct;
    }
    s
}

/// Export rate for a project of this medium at this tape speed (ips or cm/s).
pub fn export_rate(m: &MediumPreset, speed: f32) -> u32 {
    if m.export.full_rate_from_speed > 0.0 && speed >= m.export.full_rate_from_speed {
        0
    } else {
        m.export.rate
    }
}

/// Set a project up for a medium (with optional condition preset and deck).
/// Captures, tracks and manual repairs stay; settings that came from
/// analysing the audio (hum, declip, NR reference, wow reference, azimuth
/// curve, noise profiles) stay too.
pub fn apply_to_project(p: &mut Project, m: &MediumPreset, condition: Option<&Preset>, deck: Option<&DeckProfile>) {
    let old = p.chain.clone();
    let mut c = resolve_chain(m, condition, deck);
    // analysis-driven settings survive
    if old.hum.enabled && m.family == p.medium.preset.family {
        c.hum = old.hum.clone();
    }
    if old.declip.enabled {
        c.declip = old.declip.clone();
    }
    if old.nr.ref_measured {
        c.nr.ref_dbfs = old.nr.ref_dbfs;
        c.nr.ref_measured = true;
    }
    c.wow.reference = if m.family == p.medium.preset.family { old.wow.reference } else { c.wow.reference };
    if old.wow.enabled && m.family == p.medium.preset.family {
        c.wow = old.wow.clone();
    }
    p.chain = c;
    let speed_changed_unit = p.medium.preset.speed.unit != m.speed.unit;
    p.medium = MediumRef { id: m.id.clone(), preset: m.clone(), condition: condition.map(|c| c.id.clone()) };
    p.deck = deck.map(|d| d.id.clone());
    let thr = p.split.threshold_db;
    p.split = m.split.clone();
    if m.split.mode == SplitMode::Absolute && thr != SplitSettings::default().threshold_db {
        p.split.threshold_db = thr;
    }
    p.export.kind = m.export.kind;
    p.export.rate = m.export.rate;
    if m.export.bits > 0 {
        p.export.bit_depth = m.export.bits;
    }
    p.export.dither = m.export.dither;
    let fresh = source_speed(m, deck);
    for s in p.sources.iter_mut() {
        if speed_changed_unit {
            s.fx.speed.captured = fresh.captured;
            s.fx.speed.correct = fresh.correct;
            s.fx.speed.unit = fresh.unit;
        }
        if deck.is_some() || p.deck.is_none() {
            s.fx.speed.trim_pct = fresh.trim_pct;
        }
    }
    for t in p.tracks.iter_mut() {
        if t.chain.is_some() && !t.cleaned {
            t.chain = Some(p.chain.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vinyl_lp_is_todays_runout() {
        let m = vinyl_lp();
        assert_eq!(m.chain, Chain::default());
        assert_eq!(resolve_chain(&m, None, None), Chain::default());
        let mut p = Project::default();
        apply_to_project(&mut p, &m, None, None);
        assert_eq!(p.chain, Chain::default());
        assert_eq!(p.export.kind, ExportKind::Album);
        assert_eq!(p.export.rate, 0);
        assert_eq!(p.export.bit_depth, 16);
    }

    #[test]
    fn every_builtin_round_trips() {
        let ids: Vec<String> = builtin().iter().map(|m| m.id.clone()).collect();
        let mut seen = std::collections::HashSet::new();
        for m in builtin() {
            assert!(seen.insert(m.id.clone()), "duplicate id {}", m.id);
            let txt = serde_json::to_string(&m).unwrap();
            let back: MediumPreset = serde_json::from_str(&txt).unwrap();
            assert_eq!(back, m, "{} changed on a round trip", m.id);
            for s in &m.visible {
                assert!(STAGES.contains(&s.as_str()), "{}: unknown stage {s}", m.id);
            }
        }
        assert!(ids.len() >= 20);
    }

    #[test]
    fn user_preset_stores_only_the_diff() {
        let base = builtin().into_iter().find(|m| m.id == "cassette-type-ii").unwrap();
        let mut mine = base.clone();
        mine.chain.denoise.reduction_db = 9.5;
        let a = serde_json::to_value(&base).unwrap();
        let mut b_m = mine.clone();
        b_m.name = base.name.clone();
        let b = serde_json::to_value(&b_m).unwrap();
        let d = json_diff(&a, &b).unwrap();
        assert_eq!(d, serde_json::json!({"chain": {"denoise": {"reduction_db": 9.5}}}));
        let mut back = a.clone();
        json_merge(&mut back, &d);
        assert_eq!(back, b);
    }
}
