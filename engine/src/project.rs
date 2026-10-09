//! The project model: sources, tracks, settings. Saved as JSON inside a
//! `<name>.runout/` folder next to the captures it owns.

use crate::chain::{Chain, SourceFx};
use crate::dsp::loudness::Loudness;
use crate::level::Gain;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

pub const PROJECT_FILE: &str = "project.json";
pub const AUTOSAVE_FILE: &str = "project.autosave.json";
pub const TIMELINE_GAP_S: f64 = 2.0;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Capture,
    File,
    Folder,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Source {
    pub id: u64,
    /// "Side A", or the file name for imports.
    pub name: String,
    /// Path to the audio. Relative paths are inside the project folder.
    pub path: String,
    pub kind: SourceKind,
    /// Copy into the export's Raw/ folder.
    #[serde(default = "yes")]
    pub raw: bool,
    pub rate: u32,
    pub channels: usize,
    pub bits: u16,
    pub frames: usize,
    #[serde(default)]
    pub fx: SourceFx,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct FileTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub track: Option<u32>,
    pub disc: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct TrackAnalysis {
    /// Hash of everything that changes the cleaned audio. Stale when it differs.
    pub key: u64,
    pub before: Loudness,
    /// Measured after leveling (analysis or export).
    pub after: Option<Loudness>,
    pub gain: Gain,
    pub limiter_gr_db: f32,
    pub clicks: u32,
    pub crackles: u32,
    pub noise_before_db: f32,
    pub noise_after_db: f32,
    pub duration_s: f64,
    /// Ceiling correction needed at export (dB), from the post-export check.
    pub ceiling_fix_db: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Track {
    pub id: u64,
    pub source_id: u64,
    /// Bounds in source seconds (as captured, before speed correction).
    pub start_s: f64,
    pub end_s: f64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub number: Option<u32>,
    #[serde(default)]
    pub disc: Option<u32>,
    #[serde(default = "fade_in_default")]
    pub fade_in_ms: f32,
    #[serde(default = "fade_out_default")]
    pub fade_out_ms: f32,
    /// Per-track chain override. None uses the project chain.
    #[serde(default)]
    pub chain: Option<Chain>,
    /// Folder import: already cleaned, pass through untouched.
    #[serde(default)]
    pub cleaned: bool,
    /// Output file name without extension (folder imports keep theirs).
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub tags: FileTags,
    #[serde(default)]
    pub analysis: Option<TrackAnalysis>,
}

fn fade_in_default() -> f32 {
    50.0
}
fn fade_out_default() -> f32 {
    500.0
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct AlbumInfo {
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub year: String,
    pub genre: String,
    pub cover_path: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SplitSettings {
    pub threshold_db: f32,
    pub min_gap_s: f32,
}
impl Default for SplitSettings {
    fn default() -> Self {
        SplitSettings { threshold_db: -45.0, min_gap_s: 1.2 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ExportSettings {
    pub bit_depth: u16,
    /// Export root; ~ expands to the home folder.
    pub root: String,
}
impl Default for ExportSettings {
    fn default() -> Self {
        ExportSettings { bit_depth: 16, root: "~/Music/Runout/Artists".into() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Project {
    pub version: u32,
    pub name: String,
    pub chain: Chain,
    pub sources: Vec<Source>,
    pub tracks: Vec<Track>,
    pub album: AlbumInfo,
    pub split: SplitSettings,
    pub export: ExportSettings,
    pub next_id: u64,
}

impl Default for Project {
    fn default() -> Self {
        Project {
            version: 1,
            name: "Untitled".into(),
            chain: Chain::default(),
            sources: Vec::new(),
            tracks: Vec::new(),
            album: AlbumInfo::default(),
            split: SplitSettings::default(),
            export: ExportSettings::default(),
            next_id: 1,
        }
    }
}

/// Where one source sits on the shared timeline (processed seconds).
#[derive(Clone, Debug, Serialize)]
pub struct Placement {
    pub source_id: u64,
    pub start: f64,
    pub duration: f64,
    pub ratio: f64,
    pub rate: u32,
}

impl Project {
    pub fn new_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn source(&self, id: u64) -> Option<&Source> {
        self.sources.iter().find(|s| s.id == id)
    }

    pub fn source_mut(&mut self, id: u64) -> Option<&mut Source> {
        self.sources.iter_mut().find(|s| s.id == id)
    }

    pub fn track(&self, id: u64) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn effective_chain<'a>(&'a self, t: &'a Track) -> &'a Chain {
        t.chain.as_ref().unwrap_or(&self.chain)
    }

    /// A track passes through untouched: cleaned import with every stage still off.
    pub fn is_passthrough(&self, t: &Track) -> bool {
        let c = self.effective_chain(t);
        t.cleaned
            && c.restoration_off()
            && !c.level.enabled
            && self.source(t.source_id).map(|s| s.fx.speed.ratio() == 1.0 && s.fx.repairs.is_empty()).unwrap_or(true)
    }

    pub fn layout(&self) -> Vec<Placement> {
        let mut t = 0.0;
        let mut out = Vec::new();
        for s in &self.sources {
            let ratio = s.fx.speed.ratio();
            let dur = (s.frames as f64 / ratio) / s.rate as f64;
            out.push(Placement { source_id: s.id, start: t, duration: dur, ratio, rate: s.rate });
            t += dur + TIMELINE_GAP_S;
        }
        out
    }

    /// Tracks in play order: by source order, then start time.
    pub fn ordered_tracks(&self) -> Vec<&Track> {
        let order: Vec<u64> = self.sources.iter().map(|s| s.id).collect();
        let mut v: Vec<&Track> = self.tracks.iter().collect();
        v.sort_by(|a, b| {
            let ia = order.iter().position(|&x| x == a.source_id).unwrap_or(usize::MAX);
            let ib = order.iter().position(|&x| x == b.source_id).unwrap_or(usize::MAX);
            ia.cmp(&ib).then(a.start_s.partial_cmp(&b.start_s).unwrap_or(Ordering::Equal))
        });
        v
    }

    /// Track sample range in processed time: (start, len).
    pub fn track_range(&self, t: &Track) -> Option<(i64, usize)> {
        let s = self.source(t.source_id)?;
        let r = s.fx.speed.ratio();
        let a = (t.start_s * s.rate as f64 / r).round() as i64;
        let b = (t.end_s * s.rate as f64 / r).round() as i64;
        let max = (s.frames as f64 / r).floor() as i64;
        let a = a.clamp(0, max);
        let b = b.clamp(a, max);
        Some((a, (b - a) as usize))
    }

    /// Everything that changes a track's cleaned audio, hashed.
    pub fn track_key(&self, t: &Track) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut chain = self.effective_chain(t).clone();
        chain.level = Default::default();
        let src = self.source(t.source_id);
        let s = serde_json::to_string(&(chain, src.map(|s| &s.fx), t.start_s, t.end_s, t.fade_in_ms, t.fade_out_ms, t.cleaned)).unwrap_or_default();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        s.hash(&mut h);
        h.finish()
    }

    pub fn analysis_fresh(&self, t: &Track) -> bool {
        t.analysis.as_ref().map(|a| a.key == self.track_key(t)).unwrap_or(false)
    }

    /// Current leveling gain for a track from its stored analysis.
    pub fn gain_for(&self, t: &Track) -> crate::level::Gain {
        let chain = self.effective_chain(t);
        let Some(an) = &t.analysis else { return Default::default() };
        let album = if chain.level.mode == crate::chain::LevelMode::Album { self.album_lufs() } else { None };
        crate::level::gain_for(&chain.level, &an.before, album)
    }

    pub fn album_lufs(&self) -> Option<f32> {
        let v: Vec<(Loudness, f64)> = self.tracks.iter().filter_map(|t| t.analysis.as_ref().map(|a| (a.before, a.duration_s))).collect();
        crate::level::album_lufs(&v)
    }

    pub fn resolve(&self, dir: &Path, p: &str) -> PathBuf {
        let pb = PathBuf::from(p);
        if pb.is_absolute() {
            pb
        } else {
            dir.join(pb)
        }
    }

    pub fn load(dir: &Path) -> Result<Project> {
        let txt = std::fs::read_to_string(dir.join(PROJECT_FILE)).map_err(|e| Error::Io(format!("{}: {e}", dir.display())))?;
        serde_json::from_str(&txt).map_err(|e| Error::Project(e.to_string()))
    }

    pub fn load_file(path: &Path) -> Result<Project> {
        let txt = std::fs::read_to_string(path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
        serde_json::from_str(&txt).map_err(|e| Error::Project(e.to_string()))
    }

    /// Atomic save: write a temp file, then rename over the target.
    pub fn save_as(&self, dir: &Path, file: &str) -> Result<()> {
        std::fs::create_dir_all(dir).map_err(|e| Error::Io(e.to_string()))?;
        let txt = serde_json::to_string_pretty(self).map_err(|e| Error::Project(e.to_string()))?;
        let tmp = dir.join(format!(".{file}.tmp"));
        std::fs::write(&tmp, txt).map_err(|e| Error::Io(e.to_string()))?;
        std::fs::rename(&tmp, dir.join(file)).map_err(|e| Error::Io(e.to_string()))?;
        Ok(())
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        self.save_as(dir, PROJECT_FILE)?;
        let _ = std::fs::remove_file(dir.join(AUTOSAVE_FILE));
        Ok(())
    }
}

/// Expand a leading ~ to the home folder.
pub fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(h) = std::env::var_os("HOME") {
            return PathBuf::from(h).join(rest);
        }
    }
    PathBuf::from(p)
}

/// Natural order: "2 - Song" before "10 - Song".
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = ai.peek().copied().filter(|c| c.is_ascii_digit()) {
                    na.push(c);
                    ai.next();
                }
                let mut nb = String::new();
                while let Some(c) = bi.peek().copied().filter(|c| c.is_ascii_digit()) {
                    nb.push(c);
                    bi.next();
                }
                let ta = na.trim_start_matches('0');
                let tb = nb.trim_start_matches('0');
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb)).then_with(|| na.len().cmp(&nb.len()));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

pub const AUDIO_EXT: &[&str] = &["wav", "flac", "aif", "aiff", "ogg", "oga", "mp3"];

pub fn is_audio(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|e| AUDIO_EXT.contains(&e.to_ascii_lowercase().as_str())).unwrap_or(false)
}

/// One file in a folder import, before committing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportItem {
    pub path: String,
    pub name: String,
    /// Disc from the tag, or from the subfolder order.
    pub disc: u32,
    pub track: Option<u32>,
    pub tags: FileTags,
    /// Raw (restore it) or already cleaned.
    pub raw: bool,
}

/// Read tags from an audio file.
pub fn read_tags(path: &Path) -> FileTags {
    use lofty::prelude::*;
    use lofty::tag::ItemKey;
    let Ok(f) = lofty::read_from_path(path) else { return FileTags::default() };
    let Some(t) = f.primary_tag().or_else(|| f.first_tag()) else { return FileTags::default() };
    let s = |k: ItemKey| t.get_string(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    FileTags {
        title: s(ItemKey::TrackTitle),
        artist: s(ItemKey::TrackArtist),
        album: s(ItemKey::AlbumTitle),
        album_artist: s(ItemKey::AlbumArtist),
        year: s(ItemKey::Year).or_else(|| s(ItemKey::RecordingDate)).map(|y| y.chars().take(4).collect()),
        genre: s(ItemKey::Genre),
        track: t.track(),
        disc: t.disk(),
    }
}

/// Read a folder (and subfolders as discs) into an ordered import list:
/// disc number, then track number, then natural filename order for untagged files.
pub fn plan_folder_import(dir: &Path) -> Result<Vec<ImportItem>> {
    let mut groups: Vec<(PathBuf, u32)> = vec![(dir.to_path_buf(), 0)];
    let mut subs: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| Error::Io(format!("{}: {e}", dir.display())))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        // Runout's own raw-capture folder inside an exported album is not a disc
        .filter(|p| !p.file_name().map(|n| n.to_string_lossy().eq_ignore_ascii_case("raw")).unwrap_or(false))
        .collect();
    subs.sort_by(|a, b| natural_cmp(&a.file_name().unwrap_or_default().to_string_lossy(), &b.file_name().unwrap_or_default().to_string_lossy()));
    for (i, s) in subs.into_iter().enumerate() {
        groups.push((s, i as u32 + 1));
    }
    let mut items = Vec::new();
    for (g, folder_disc) in groups {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&g)
            .map_err(|e| Error::Io(e.to_string()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && is_audio(p))
            .collect();
        files.sort_by(|a, b| natural_cmp(&a.file_name().unwrap_or_default().to_string_lossy(), &b.file_name().unwrap_or_default().to_string_lossy()));
        for p in files {
            let tags = read_tags(&p);
            let name = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
            items.push(ImportItem {
                path: p.to_string_lossy().to_string(),
                name,
                disc: tags.disc.unwrap_or(folder_disc.max(1)),
                track: tags.track,
                tags,
                raw: true,
            });
        }
    }
    sort_import(&mut items);
    Ok(items)
}

pub fn sort_import(items: &mut [ImportItem]) {
    items.sort_by(|a, b| {
        a.disc.cmp(&b.disc).then_with(|| match (a.track, b.track) {
            (Some(x), Some(y)) => x.cmp(&y).then_with(|| natural_cmp(&a.name, &b.name)),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => natural_cmp(&a.name, &b.name),
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn natural() {
        assert_eq!(natural_cmp("2 - Song.flac", "10 - Song.flac"), Ordering::Less);
        assert_eq!(natural_cmp("Disc 2", "Disc 10"), Ordering::Less);
        assert_eq!(natural_cmp("a", "B"), Ordering::Less);
    }
}
