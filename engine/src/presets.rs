//! Condition presets: how worn a recording is. Built-ins plus user presets saved
//! as JSON in ~/.config/runout/presets/. Each belongs to a medium family; one
//! saved before families existed has none and reads as vinyl.

use crate::chain::{Chain, LevelMode};
use crate::media::Family;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub builtin: bool,
    pub chain: Chain,
    #[serde(default)]
    pub family: Family,
}

pub fn builtin() -> Vec<Preset> {
    let mut clean = Chain::default();
    clean.declick.threshold_db = 14.0;
    clean.declick.max_len_ms = 1.5;
    clean.decrackle.enabled = false;
    clean.denoise.reduction_db = 4.0;

    let mut light = Chain::default();
    light.declick.threshold_db = 13.0;
    light.decrackle.threshold_db = 11.0;
    light.denoise.reduction_db = 6.0;

    let mut worn = Chain::default();
    worn.rumble.freq_hz = 25.0;
    worn.rumble.slope_db_oct = 36;
    worn.declick.threshold_db = 11.5;
    worn.declick.max_len_ms = 3.0;
    worn.decrackle.threshold_db = 9.5;
    worn.denoise.reduction_db = 9.0;
    worn.denoise.smoothing_pct = 65.0;
    worn.eq.enabled = true;
    worn.eq.high_shelf_hz = 7000.0;
    worn.eq.high_shelf_db = 1.5;
    worn.level.mode = LevelMode::Track;

    let mut v = vec![
        Preset { id: "clean-modern".into(), name: "Clean modern pressing".into(), builtin: true, chain: clean, family: Family::Vinyl },
        Preset { id: "light-wear".into(), name: "Light wear".into(), builtin: true, chain: light, family: Family::Vinyl },
        Preset { id: "worn-70s".into(), name: "Played-to-death 70s LP".into(), builtin: true, chain: worn, family: Family::Vinyl },
    ];
    for fam in Family::all().into_iter().filter(|f| f.is_tape()) {
        v.extend(tape_conditions(fam));
    }
    v
}

/// Clean, Worn and Rough for one tape-like family. They set only the
/// condition stages: hiss, dropouts, clicks, the high-pass. What the medium
/// owns (playback EQ, NR, channel mode, band limit, leveling) is left alone
/// by `apply`.
fn tape_conditions(fam: Family) -> Vec<Preset> {
    let base = |f: &dyn Fn(&mut Chain)| {
        let mut c = Chain::default();
        c.declick.enabled = false;
        c.decrackle.enabled = false;
        c.denoise.enabled = true;
        c.dropout.enabled = true;
        f(&mut c);
        c
    };
    let clean = base(&|c| {
        c.denoise.reduction_db = 4.0;
        c.denoise.smoothing_pct = 45.0;
    });
    let worn = base(&|c| {
        c.denoise.reduction_db = 7.0;
        c.denoise.smoothing_pct = 55.0;
        c.dropout.depth_db = 5.0;
        c.declick.enabled = true;
        c.declick.threshold_db = 15.0;
    });
    let rough = base(&|c| {
        c.denoise.reduction_db = 10.0;
        c.denoise.smoothing_pct = 65.0;
        c.dropout.depth_db = 4.0;
        c.dropout.max_ms = 400.0;
        c.declick.enabled = true;
        c.declick.threshold_db = 13.0;
        c.rumble.freq_hz = 40.0;
        c.rumble.slope_db_oct = 36;
    });
    let f = fam.label().to_lowercase();
    vec![
        Preset { id: format!("{f}-clean"), name: "Clean".into(), builtin: true, chain: clean, family: fam },
        Preset { id: format!("{f}-worn"), name: "Worn".into(), builtin: true, chain: worn, family: fam },
        Preset { id: format!("{f}-rough"), name: "Rough".into(), builtin: true, chain: rough, family: fam },
    ]
}

/// Condition presets for one family.
pub fn for_family(fam: Family) -> Vec<Preset> {
    list().into_iter().filter(|p| p.family == fam).collect()
}

pub fn user_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("runout").join("presets")
}

pub fn slug(name: &str) -> String {
    let s: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let mut out = String::new();
    for part in s.split('-').filter(|p| !p.is_empty()) {
        if !out.is_empty() {
            out.push('-');
        }
        out.push_str(part);
    }
    if out.is_empty() {
        "preset".into()
    } else {
        out
    }
}

pub fn list() -> Vec<Preset> {
    let mut v = builtin();
    if let Ok(rd) = std::fs::read_dir(user_dir()) {
        let mut user: Vec<Preset> = rd
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|s| serde_json::from_str::<Preset>(&s).ok())
            .map(|mut p| {
                p.builtin = false;
                p
            })
            .collect();
        user.sort_by(|a, b| a.name.cmp(&b.name));
        v.extend(user);
    }
    v
}

pub fn find(id_or_name: &str) -> Option<Preset> {
    let key = slug(id_or_name);
    list().into_iter().find(|p| p.id == id_or_name || p.id == key || slug(&p.name) == key)
}

pub fn save(name: &str, chain: &Chain) -> Result<Preset> {
    save_for(name, chain, Family::Vinyl)
}

pub fn save_for(name: &str, chain: &Chain, family: Family) -> Result<Preset> {
    let dir = user_dir();
    std::fs::create_dir_all(&dir).map_err(|e| Error::Io(e.to_string()))?;
    let p = Preset { id: slug(name), name: name.trim().to_string(), builtin: false, chain: chain.clone(), family };
    if builtin().iter().any(|b| b.id == p.id) {
        return Err(Error::Project("that name belongs to a built-in preset".into()));
    }
    let txt = serde_json::to_string_pretty(&p).map_err(|e| Error::Project(e.to_string()))?;
    std::fs::write(dir.join(format!("{}.json", p.id)), txt).map_err(|e| Error::Io(e.to_string()))?;
    Ok(p)
}

pub fn delete(id: &str) -> Result<()> {
    let path = user_dir().join(format!("{}.json", slug(id)));
    std::fs::remove_file(path).map_err(|e| Error::Io(e.to_string()))
}

/// Apply a preset to a chain. Hum and declip settings come from analysing the
/// recording, so they are kept, and so is everything the medium owns: wow and
/// its reference, azimuth, channel mode, playback EQ, NR with its measured
/// reference level, band limit, the digital stages. Vinyl presets set
/// leveling as they always have; tape presets leave leveling to the medium.
pub fn apply(current: &Chain, preset: &Preset) -> Chain {
    let mut c = preset.chain.clone();
    c.hum = current.hum.clone();
    c.declip = current.declip.clone();
    c.wow = current.wow.clone();
    c.azimuth = current.azimuth.clone();
    c.channel_mode = current.channel_mode.clone();
    c.playback_eq = current.playback_eq.clone();
    c.nr = current.nr.clone();
    c.band_limit = current.band_limit.clone();
    c.error_repair = current.error_repair.clone();
    c.deemphasis = current.deemphasis.clone();
    c.digital = current.digital;
    if preset.family != Family::Vinyl {
        c.level = current.level.clone();
        c.channel = current.channel.clone();
        c.eq = current.eq.clone();
    }
    c
}
