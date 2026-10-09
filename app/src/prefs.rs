//! App settings (~/.config/runout/settings.json) and the session marker used for
//! crash recovery (~/.local/share/runout/session.json).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config")).join("runout")
}

pub fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share")).join("runout")
}

pub fn projects_root() -> PathBuf {
    home().join("Music/Runout/Projects")
}

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)]
pub struct Prefs {
    pub input_device: Option<String>,
    pub recent: Vec<String>,
    pub auto_stop_s: Option<f32>,
}

impl Prefs {
    pub fn load() -> Self {
        std::fs::read_to_string(config_dir().join("settings.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    pub fn save(&self) {
        let d = config_dir();
        let _ = std::fs::create_dir_all(&d);
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(d.join("settings.json"), s);
        }
    }
    pub fn add_recent(&mut self, dir: &str) {
        self.recent.retain(|r| r != dir);
        self.recent.insert(0, dir.to_string());
        self.recent.truncate(10);
        self.save();
    }
}

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)]
pub struct Session {
    /// Project open when the app last ran; cleared on a clean exit.
    pub open_project: Option<String>,
    /// Capture file being written; cleared when the capture is added.
    pub capture: Option<String>,
    pub capture_name: Option<String>,
}

impl Session {
    pub fn load() -> Self {
        std::fs::read_to_string(data_dir().join("session.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    pub fn save(&self) {
        let d = data_dir();
        let _ = std::fs::create_dir_all(&d);
        if let Ok(s) = serde_json::to_string(self) {
            let _ = std::fs::write(d.join("session.json"), s);
        }
    }
}
