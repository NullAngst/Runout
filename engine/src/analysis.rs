//! Per-source tape analysis: the speed (wow) curve, the azimuth curve, and the
//! dropout and digital error maps. Analyze computes them; preview and export
//! only apply them.
//!
//! The curves and maps live as small binary files in the project's `analysis/`
//! folder, referenced by name from the source's settings, so project.json stays
//! readable and autosaves stay small. Each name carries a hash of its content,
//! so a new measurement is a new name and anything keyed on the settings sees
//! the change. If the folder goes missing, Analyze writes it again.

use crate::dsp::azimuth::AzimuthCurve;
use crate::dsp::wow::{FreqTrack, WowMap};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

pub const DIR: &str = "analysis";
const MAGIC: &[u8; 4] = b"RNOA";
const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    Dropout,
    /// Too long to repair automatically.
    DropoutFlagged,
    Error,
    ErrorFlagged,
    ProgramChange,
}

/// One event on a map, in source seconds.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct MapEvent {
    pub start_s: f64,
    pub len_s: f64,
    pub kind: EventKind,
    pub depth_db: f32,
}

/// A reference tone found at the head of a tape.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToneInfo {
    pub freq_hz: f32,
    /// The standard tone it is closest to, if any.
    pub nominal_hz: Option<f32>,
    pub level_dbfs: f32,
    pub start_s: f64,
    pub len_s: f64,
    /// Speed error it implies, percent (positive: the deck ran fast).
    pub speed_err_pct: Option<f32>,
}

/// Loaded curve data. Not serialized; compared as equal so it never makes an
/// edit look like a change (the file names already say that).
#[derive(Clone, Default)]
pub struct Loaded(pub Option<Arc<Data>>);

impl PartialEq for Loaded {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for Loaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Loaded({})", if self.0.is_some() { "yes" } else { "no" })
    }
}

#[derive(Clone, Debug, Default)]
pub struct Data {
    pub wow: Option<(FreqTrack, WowMap)>,
    pub azimuth: Option<AzimuthCurve>,
    pub dropouts: Vec<MapEvent>,
    pub errors: Vec<MapEvent>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct TapeAnalysis {
    pub speed_curve: Option<String>,
    pub azimuth_curve: Option<String>,
    pub dropout_map: Option<String>,
    pub error_map: Option<String>,
    /// Best channel auto: 0 left, 1 right, 2 sum.
    pub best_channel: Option<u8>,
    pub tone: Option<ToneInfo>,
    /// Plain-language note from the last wow measurement.
    pub wow_note: Option<String>,
    /// Peak wow of the reference before correction, percent.
    pub wow_peak_pct: Option<f32>,
    /// Weighted wow and flutter (3150 Hz reference only), percent.
    pub wow_weighted_pct: Option<f32>,
    #[serde(skip)]
    pub data: Loaded,
}

impl TapeAnalysis {
    pub fn is_empty(&self) -> bool {
        self.speed_curve.is_none() && self.azimuth_curve.is_none() && self.dropout_map.is_none() && self.error_map.is_none() && self.best_channel.is_none() && self.tone.is_none()
    }

    fn data(&self) -> Option<&Data> {
        self.data.0.as_deref()
    }

    pub fn wow_map(&self) -> Option<&WowMap> {
        self.data().and_then(|d| d.wow.as_ref()).map(|w| &w.1).filter(|m| !m.is_empty())
    }

    pub fn wow_track(&self) -> Option<&FreqTrack> {
        self.data().and_then(|d| d.wow.as_ref()).map(|w| &w.0)
    }

    pub fn azimuth(&self) -> Option<&AzimuthCurve> {
        self.data().and_then(|d| d.azimuth.as_ref()).filter(|c| !c.is_empty())
    }

    pub fn dropouts(&self) -> &[MapEvent] {
        self.data().map(|d| d.dropouts.as_slice()).unwrap_or(&[])
    }

    pub fn errors(&self) -> &[MapEvent] {
        self.data().map(|d| d.errors.as_slice()).unwrap_or(&[])
    }

    /// Named files whose data is not loaded: Analyze must redo them.
    pub fn missing(&self) -> bool {
        let d = self.data();
        (self.speed_curve.is_some() && d.map(|d| d.wow.is_none()).unwrap_or(true)) || (self.azimuth_curve.is_some() && d.map(|d| d.azimuth.is_none()).unwrap_or(true))
    }

    fn edit(&mut self) -> &mut Data {
        let cur = self.data.0.take().map(|a| (*a).clone()).unwrap_or_default();
        self.data.0 = Some(Arc::new(cur));
        Arc::get_mut(self.data.0.as_mut().unwrap()).unwrap()
    }

    pub fn set_wow(&mut self, source_id: u64, t: Option<(FreqTrack, WowMap)>) {
        self.speed_curve = t.as_ref().map(|(tr, _)| name(source_id, "speed", &encode_track(tr)));
        self.edit().wow = t;
    }

    pub fn set_azimuth(&mut self, source_id: u64, c: Option<AzimuthCurve>) {
        self.azimuth_curve = c.as_ref().map(|c| name(source_id, "azimuth", &encode_azimuth(c)));
        self.edit().azimuth = c;
    }

    pub fn set_dropouts(&mut self, source_id: u64, v: Vec<MapEvent>) {
        self.dropout_map = if v.is_empty() { None } else { Some(name(source_id, "dropouts", &encode_events(&v))) };
        self.edit().dropouts = v;
    }

    pub fn set_errors(&mut self, source_id: u64, v: Vec<MapEvent>) {
        self.error_map = if v.is_empty() { None } else { Some(name(source_id, "errors", &encode_events(&v))) };
        self.edit().errors = v;
    }

    /// Write every loaded curve and map into `dir/analysis/`.
    pub fn write(&self, project_dir: &Path, rate: u32) -> Result<()> {
        let Some(d) = self.data() else { return Ok(()) };
        let dir = project_dir.join(DIR);
        let put = |n: &Option<String>, bytes: Vec<u8>| -> Result<()> {
            if let Some(n) = n {
                std::fs::create_dir_all(&dir).map_err(|e| Error::Io(e.to_string()))?;
                let p = dir.join(n);
                if !p.exists() {
                    std::fs::write(&p, bytes).map_err(|e| Error::Io(format!("{}: {e}", p.display())))?;
                }
            }
            Ok(())
        };
        if let Some((t, _)) = &d.wow {
            put(&self.speed_curve, encode_track(t))?;
        }
        if let Some(c) = &d.azimuth {
            put(&self.azimuth_curve, encode_azimuth(c))?;
        }
        if !d.dropouts.is_empty() {
            put(&self.dropout_map, encode_events(&d.dropouts))?;
        }
        if !d.errors.is_empty() {
            put(&self.error_map, encode_events(&d.errors))?;
        }
        let _ = rate;
        Ok(())
    }

    /// Load whatever named files exist. Missing ones stay unloaded.
    pub fn load(&mut self, project_dir: &Path, rate: u32) {
        let dir = project_dir.join(DIR);
        let read = |n: &Option<String>| n.as_ref().and_then(|n| std::fs::read(dir.join(n)).ok());
        let mut d = Data::default();
        if let Some(t) = read(&self.speed_curve).and_then(|b| decode_track(&b)) {
            let m = WowMap::from_track(&t, rate);
            d.wow = Some((t, m));
        }
        d.azimuth = read(&self.azimuth_curve).and_then(|b| decode_azimuth(&b));
        d.dropouts = read(&self.dropout_map).and_then(|b| decode_events(&b)).unwrap_or_default();
        d.errors = read(&self.error_map).and_then(|b| decode_events(&b)).unwrap_or_default();
        self.data = Loaded(Some(Arc::new(d)));
    }
}

fn name(source_id: u64, kind: &str, bytes: &[u8]) -> String {
    // FNV-1a, stable across builds
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("s{source_id}-{kind}-{h:016x}.bin")
}

fn header(kind: u32, out: &mut Vec<u8>) {
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&kind.to_le_bytes());
}

fn check(b: &[u8], kind: u32) -> Option<&[u8]> {
    if b.len() < 12 || &b[0..4] != MAGIC || u32::from_le_bytes(b[4..8].try_into().ok()?) != VERSION || u32::from_le_bytes(b[8..12].try_into().ok()?) != kind {
        return None;
    }
    Some(&b[12..])
}

fn f64s(b: &[u8], n: usize) -> Option<Vec<f64>> {
    (0..n).map(|i| b.get(i * 8..i * 8 + 8).map(|s| f64::from_le_bytes(s.try_into().unwrap()))).collect()
}

fn f32s(b: &[u8], n: usize) -> Option<Vec<f32>> {
    (0..n).map(|i| b.get(i * 4..i * 4 + 4).map(|s| f32::from_le_bytes(s.try_into().unwrap()))).collect()
}

fn encode_track(t: &FreqTrack) -> Vec<u8> {
    let mut o = Vec::new();
    header(1, &mut o);
    o.extend_from_slice(&(t.freq.len() as u32).to_le_bytes());
    o.extend_from_slice(&t.t0.to_le_bytes());
    o.extend_from_slice(&t.nominal.to_le_bytes());
    for v in &t.freq {
        o.extend_from_slice(&v.to_le_bytes());
    }
    for v in &t.level_db {
        o.extend_from_slice(&v.to_le_bytes());
    }
    o
}

fn decode_track(b: &[u8]) -> Option<FreqTrack> {
    let b = check(b, 1)?;
    let n = u32::from_le_bytes(b.get(0..4)?.try_into().ok()?) as usize;
    let h = f64s(&b[4..], 2)?;
    let freq = f32s(&b[20..], n)?;
    let level_db = f32s(b.get(20 + n * 4..)?, n)?;
    Some(FreqTrack { t0: h[0], nominal: h[1], freq, level_db })
}

fn encode_azimuth(c: &AzimuthCurve) -> Vec<u8> {
    let mut o = Vec::new();
    header(2, &mut o);
    o.extend_from_slice(&(c.t.len() as u32).to_le_bytes());
    for v in c.t.iter().chain(&c.us) {
        o.extend_from_slice(&v.to_le_bytes());
    }
    o
}

fn decode_azimuth(b: &[u8]) -> Option<AzimuthCurve> {
    let b = check(b, 2)?;
    let n = u32::from_le_bytes(b.get(0..4)?.try_into().ok()?) as usize;
    let t = f32s(&b[4..], n)?;
    let us = f32s(b.get(4 + n * 4..)?, n)?;
    Some(AzimuthCurve { t, us })
}

fn kind_code(k: EventKind) -> u32 {
    match k {
        EventKind::Dropout => 0,
        EventKind::DropoutFlagged => 1,
        EventKind::Error => 2,
        EventKind::ErrorFlagged => 3,
        EventKind::ProgramChange => 4,
    }
}

fn code_kind(c: u32) -> Option<EventKind> {
    Some(match c {
        0 => EventKind::Dropout,
        1 => EventKind::DropoutFlagged,
        2 => EventKind::Error,
        3 => EventKind::ErrorFlagged,
        4 => EventKind::ProgramChange,
        _ => return None,
    })
}

fn encode_events(v: &[MapEvent]) -> Vec<u8> {
    let mut o = Vec::new();
    header(3, &mut o);
    o.extend_from_slice(&(v.len() as u32).to_le_bytes());
    for e in v {
        o.extend_from_slice(&e.start_s.to_le_bytes());
        o.extend_from_slice(&e.len_s.to_le_bytes());
        o.extend_from_slice(&kind_code(e.kind).to_le_bytes());
        o.extend_from_slice(&e.depth_db.to_le_bytes());
    }
    o
}

fn decode_events(b: &[u8]) -> Option<Vec<MapEvent>> {
    let b = check(b, 3)?;
    let n = u32::from_le_bytes(b.get(0..4)?.try_into().ok()?) as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let r = b.get(4 + i * 24..4 + i * 24 + 24)?;
        out.push(MapEvent {
            start_s: f64::from_le_bytes(r[0..8].try_into().ok()?),
            len_s: f64::from_le_bytes(r[8..16].try_into().ok()?),
            kind: code_kind(u32::from_le_bytes(r[16..20].try_into().ok()?))?,
            depth_db: f32::from_le_bytes(r[20..24].try_into().ok()?),
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_files() {
        let dir = std::env::temp_dir().join(format!("runout-an-{}", std::process::id()));
        let mut a = TapeAnalysis::default();
        let t = FreqTrack { t0: 0.5, freq: vec![60.0, 60.1, 59.9], level_db: vec![-40.0; 3], nominal: 60.0 };
        a.set_wow(3, Some((t.clone(), WowMap::from_track(&t, 48000))));
        a.set_azimuth(3, Some(AzimuthCurve { t: vec![1.0, 1.5], us: vec![-40.0, -41.0] }));
        a.set_dropouts(3, vec![MapEvent { start_s: 1.0, len_s: 0.01, kind: EventKind::Dropout, depth_db: 12.0 }]);
        a.write(&dir, 48000).unwrap();
        let mut b = TapeAnalysis { speed_curve: a.speed_curve.clone(), azimuth_curve: a.azimuth_curve.clone(), dropout_map: a.dropout_map.clone(), ..Default::default() };
        b.load(&dir, 48000);
        assert_eq!(b.wow_track().unwrap().freq, t.freq);
        assert_eq!(b.azimuth().unwrap().us, vec![-40.0, -41.0]);
        assert_eq!(b.dropouts().len(), 1);
        assert!(!b.missing());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
