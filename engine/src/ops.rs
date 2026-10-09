//! Project edits shared by the app and the CLI: adding sources, folder import,
//! splitting, numbering.

use crate::audio::SourceAudio;
use crate::auto;
use crate::chain::{Chain, SourceFx};
use crate::project::{FileTags, ImportItem, Project, Source, SourceKind, Track};

pub fn add_source(p: &mut Project, path: &str, kind: SourceKind, raw: bool, name: &str, a: &SourceAudio) -> u64 {
    let id = p.new_id();
    p.sources.push(Source {
        id,
        name: name.to_string(),
        path: path.to_string(),
        kind,
        raw,
        rate: a.rate,
        channels: a.channels,
        bits: a.bits,
        frames: a.frames,
        fx: SourceFx::default(),
    });
    id
}

/// Analyze a newly added raw source: noise profile, hum, clipping.
pub fn auto_new_source(p: &mut Project, id: u64, a: &SourceAudio) {
    let mut chain = p.chain.clone();
    if let Some(s) = p.source_mut(id) {
        auto::auto_source(a, &mut s.fx, &mut chain);
    }
    // Only let the first raw source steer the project-wide hum/declip choice,
    // unless it found something the current settings lack.
    if chain.hum.enabled && !p.chain.hum.enabled {
        p.chain.hum = chain.hum;
    } else if !p.sources.iter().any(|s| s.id != id && s.raw) {
        p.chain.hum = chain.hum;
    }
    if chain.declip.enabled {
        p.chain.declip = chain.declip;
    }
}

pub fn new_track(p: &mut Project, source_id: u64, start_s: f64, end_s: f64) -> u64 {
    let id = p.new_id();
    p.tracks.push(Track {
        id,
        source_id,
        start_s,
        end_s,
        title: String::new(),
        number: None,
        disc: None,
        fade_in_ms: 50.0,
        fade_out_ms: 500.0,
        chain: None,
        cleaned: false,
        file_name: None,
        tags: FileTags::default(),
        analysis: None,
    });
    id
}

/// Folder import: one source and one track per file, in the given order.
/// `load` decodes a file. Returns the new source ids.
pub fn import_items(p: &mut Project, items: &[ImportItem], load: &mut dyn FnMut(&str) -> crate::Result<std::sync::Arc<SourceAudio>>) -> crate::Result<Vec<(u64, std::sync::Arc<SourceAudio>)>> {
    let mut out = Vec::new();
    for it in items {
        let a = load(&it.path)?;
        let sid = add_source(p, &it.path, SourceKind::Folder, it.raw, &it.name, &a);
        if it.raw {
            if let Some(s) = p.source_mut(sid) {
                s.fx.noise = auto::learn_noise(&a);
            }
        }
        let tid = new_track(p, sid, 0.0, a.frames as f64 / a.rate as f64);
        if let Some(t) = p.tracks.iter_mut().find(|t| t.id == tid) {
            t.title = it.tags.title.clone().unwrap_or_else(|| strip_number(&it.name));
            t.number = it.tags.track;
            t.disc = Some(it.disc);
            // already split and trimmed files: no automatic fades
            t.fade_in_ms = 0.0;
            t.fade_out_ms = 0.0;
            t.file_name = Some(it.name.clone());
            t.tags = it.tags.clone();
            set_cleaned(t, !it.raw);
        }
        out.push((sid, a));
    }
    fill_album_from_tags(p);
    Ok(out)
}

/// Mark a track Raw or Cleaned. Cleaned starts with every stage and leveling off.
pub fn set_cleaned(t: &mut Track, cleaned: bool) {
    t.cleaned = cleaned;
    t.chain = if cleaned { Some(Chain::passthrough()) } else { None };
}

/// "02 - Song Name" -> "Song Name"
pub fn strip_number(name: &str) -> String {
    let s = name.trim_start_matches(|c: char| c.is_ascii_digit());
    let s = s.trim_start_matches([' ', '-', '.', '_', ')']);
    if s.trim().is_empty() {
        name.to_string()
    } else {
        s.trim().to_string()
    }
}

fn most_common(v: Vec<String>) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for s in v.into_iter().filter(|s| !s.trim().is_empty()) {
        if let Some(c) = counts.iter_mut().find(|c| c.0 == s) {
            c.1 += 1;
        } else {
            counts.push((s, 1));
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    counts.into_iter().next().map(|c| c.0).unwrap_or_default()
}

pub fn fill_album_from_tags(p: &mut Project) {
    let tags: Vec<FileTags> = p.tracks.iter().map(|t| t.tags.clone()).collect();
    let pick = |f: fn(&FileTags) -> Option<String>| most_common(tags.iter().filter_map(f).collect());
    if p.album.artist.is_empty() {
        p.album.artist = pick(|t| t.artist.clone());
    }
    if p.album.album.is_empty() {
        p.album.album = pick(|t| t.album.clone());
    }
    if p.album.album_artist.is_empty() {
        p.album.album_artist = pick(|t| t.album_artist.clone());
    }
    if p.album.year.is_empty() {
        p.album.year = pick(|t| t.year.clone());
    }
    if p.album.genre.is_empty() {
        p.album.genre = pick(|t| t.genre.clone());
    }
}

/// Replace a source's tracks with an automatic split on silence.
pub fn split_source(p: &mut Project, source_id: u64, a: &SourceAudio) {
    let settings = p.split.clone();
    let regions = auto::split_on_silence(a, &settings);
    p.tracks.retain(|t| t.source_id != source_id);
    for (s, e) in regions {
        new_track(p, source_id, s, e);
    }
    renumber(p);
}

/// Number capture tracks 1..N in play order. Folder imports keep their numbers.
pub fn renumber(p: &mut Project) {
    let order: Vec<u64> = p.ordered_tracks().iter().map(|t| t.id).collect();
    let kinds: Vec<(u64, SourceKind)> = p.sources.iter().map(|s| (s.id, s.kind)).collect();
    let mut n = 0;
    for id in order {
        if let Some(t) = p.tracks.iter_mut().find(|t| t.id == id) {
            let folder = kinds.iter().any(|(sid, k)| *sid == t.source_id && *k == SourceKind::Folder);
            n += 1;
            if !folder || t.number.is_none() {
                t.number = Some(n);
            }
        }
    }
}

/// Split the track under `at_s` (source seconds) into two.
pub fn mark_boundary(p: &mut Project, source_id: u64, at_s: f64) -> Option<u64> {
    let idx = p.tracks.iter().position(|t| t.source_id == source_id && t.start_s < at_s && t.end_s > at_s)?;
    let old = p.tracks[idx].clone();
    if at_s - old.start_s < 0.5 || old.end_s - at_s < 0.5 {
        return None;
    }
    p.tracks[idx].end_s = at_s;
    p.tracks[idx].analysis = None;
    let id = p.new_id();
    let mut t = old.clone();
    t.id = id;
    t.start_s = at_s;
    t.title = String::new();
    t.analysis = None;
    t.file_name = None;
    p.tracks.push(t);
    renumber(p);
    Some(id)
}

/// Merge a track with the next one on the same source.
pub fn merge_with_next(p: &mut Project, track_id: u64) -> bool {
    let order: Vec<u64> = p.ordered_tracks().iter().map(|t| t.id).collect();
    let Some(i) = order.iter().position(|&x| x == track_id) else { return false };
    let Some(&next) = order.get(i + 1) else { return false };
    let (a, b) = (p.track(track_id).cloned(), p.track(next).cloned());
    let (Some(a), Some(b)) = (a, b) else { return false };
    if a.source_id != b.source_id {
        return false;
    }
    if let Some(t) = p.tracks.iter_mut().find(|t| t.id == track_id) {
        t.end_s = b.end_s;
        t.analysis = None;
    }
    p.tracks.retain(|t| t.id != next);
    renumber(p);
    true
}
