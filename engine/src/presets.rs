//! Chain presets by record condition. Built-ins plus user presets saved as JSON
//! in ~/.config/runout/presets/.

use crate::chain::{Chain, LevelMode};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub builtin: bool,
    pub chain: Chain,
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

    vec![
        Preset { id: "clean-modern".into(), name: "Clean modern pressing".into(), builtin: true, chain: clean },
        Preset { id: "light-wear".into(), name: "Light wear".into(), builtin: true, chain: light },
        Preset { id: "worn-70s".into(), name: "Played-to-death 70s LP".into(), builtin: true, chain: worn },
    ]
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
    let dir = user_dir();
    std::fs::create_dir_all(&dir).map_err(|e| Error::Io(e.to_string()))?;
    let p = Preset { id: slug(name), name: name.trim().to_string(), builtin: false, chain: chain.clone() };
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
/// recording, so they are kept.
pub fn apply(current: &Chain, preset: &Preset) -> Chain {
    let mut c = preset.chain.clone();
    c.hum = current.hum.clone();
    c.declip = current.declip.clone();
    c
}
