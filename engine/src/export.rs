//! Export: tagged FLAC per track, raw captures as WAV, cover art, report.

use crate::audio::{self, SourceAudio};
use crate::dsp::loudness;
use crate::project::{expand_home, Project, SourceKind, Track};
use crate::render;
use crate::{Error, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub type Sources = HashMap<u64, Arc<SourceAudio>>;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ExportResult {
    pub album_dir: PathBuf,
    pub files: Vec<PathBuf>,
    pub report: PathBuf,
    pub warnings: Vec<String>,
}

/// Characters that cannot appear in a file name.
pub fn safe_name(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let t = t.trim().trim_matches('.').to_string();
    if t.is_empty() {
        "Untitled".into()
    } else {
        t
    }
}

pub fn album_dir(p: &Project) -> PathBuf {
    let artist = if !p.album.album_artist.trim().is_empty() { &p.album.album_artist } else { &p.album.artist };
    let name = format!("{} - {}", if artist.trim().is_empty() { "Unknown Artist" } else { artist.trim() }, if p.album.album.trim().is_empty() { "Unknown Album" } else { p.album.album.trim() });
    expand_home(&p.export.root).join(safe_name(&name))
}

pub fn track_file_name(t: &Track) -> String {
    if let Some(f) = t.file_name.as_ref().filter(|f| !f.trim().is_empty()) {
        return safe_name(f);
    }
    match t.number {
        Some(n) => safe_name(&format!("{:02}. {}", n, t.title.trim())),
        None => safe_name(t.title.trim()),
    }
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }
}

/// Float to integer with TPDF dither.
pub fn to_int_dithered(ch: &[Vec<f32>], bits: u16, seed: u64) -> Vec<i32> {
    let n = ch.first().map(|c| c.len()).unwrap_or(0);
    let scale = (1i64 << (bits - 1)) as f32;
    let (lo, hi) = (-scale, scale - 1.0);
    let mut rng = Lcg(seed | 1);
    let mut out = Vec::with_capacity(n * ch.len());
    for i in 0..n {
        for c in ch {
            let d = rng.next() - rng.next();
            out.push((c[i] * scale + d).round().clamp(lo, hi) as i32);
        }
    }
    out
}

fn ints_to_planar(v: &[i32], chs: usize, bits: u16) -> Vec<Vec<f32>> {
    let scale = 1.0 / (1i64 << (bits - 1)) as f32;
    let n = v.len() / chs;
    (0..chs).map(|c| (0..n).map(|i| v[i * chs + c] as f32 * scale).collect()).collect()
}

pub fn encode_flac(path: &Path, samples: &[i32], channels: usize, bits: u16, rate: u32) -> Result<()> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;
    let mut cfg = flacenc::config::Encoder::default();
    cfg.block_size = 4096;
    let cfg = cfg.into_verified().map_err(|e| Error::Encode(format!("{e:?}")))?;
    let src = flacenc::source::MemSource::from_samples(samples, channels, bits as usize, rate as usize);
    let stream = flacenc::encode_with_fixed_block_size(&cfg, src, cfg.block_size).map_err(|e| Error::Encode(format!("{e:?}")))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).map_err(|e| Error::Encode(format!("{e:?}")))?;
    let mut bytes = sink.as_slice().to_vec();
    // flacenc records the short final block as the minimum block size. For a
    // fixed-blocksize stream the spec says min = max (the last block may be
    // shorter), and some decoders reject the file otherwise.
    if bytes.len() > 12 && &bytes[0..4] == b"fLaC" && bytes[4] & 0x7f == 0 {
        let (a, b) = (bytes[10], bytes[11]);
        bytes[8] = a;
        bytes[9] = b;
    }
    std::fs::write(path, bytes).map_err(|e| Error::Io(e.to_string()))
}

pub struct TagSet {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub year: String,
    pub genre: String,
    pub track: Option<u32>,
    pub track_total: Option<u32>,
    pub disc: Option<u32>,
}

pub fn write_tags(path: &Path, tags: &TagSet, cover: Option<&[u8]>, keep_from: Option<&Path>) -> Result<()> {
    use lofty::config::WriteOptions;
    use lofty::picture::{MimeType, Picture, PictureType};
    use lofty::prelude::*;
    use lofty::tag::{ItemKey, Tag, TagType};

    let mut tag = Tag::new(TagType::VorbisComments);
    let mut old_pictures = Vec::new();
    if let Some(orig) = keep_from {
        if let Ok(f) = lofty::read_from_path(orig) {
            if let Some(t) = f.primary_tag().or_else(|| f.first_tag()) {
                for item in t.items() {
                    tag.insert(item.clone());
                }
                old_pictures = t.pictures().to_vec();
            }
        }
    }
    let mut set = |k: ItemKey, v: &str| {
        if !v.trim().is_empty() {
            tag.remove_key(k.clone());
            tag.insert_text(k, v.trim().to_string());
        }
    };
    set(ItemKey::TrackTitle, &tags.title);
    set(ItemKey::TrackArtist, &tags.artist);
    set(ItemKey::AlbumTitle, &tags.album);
    set(ItemKey::AlbumArtist, &tags.album_artist);
    set(ItemKey::RecordingDate, &tags.year);
    set(ItemKey::Genre, &tags.genre);
    if let Some(n) = tags.track {
        tag.set_track(n);
    }
    if let Some(n) = tags.track_total {
        tag.set_track_total(n);
    }
    if let Some(d) = tags.disc {
        tag.set_disk(d);
    }
    if let Some(c) = cover {
        tag.push_picture(Picture::unchecked(c.to_vec()).pic_type(PictureType::CoverFront).mime_type(MimeType::Jpeg).build());
    } else {
        for p in old_pictures {
            tag.push_picture(p);
        }
    }
    tag.save_to_path(path, WriteOptions::default()).map_err(|e| Error::Encode(format!("tags: {e}")))
}

/// Read a cover image; returns JPEG bytes (converting PNG and others).
pub fn load_cover(path: &Path) -> Result<Vec<u8>> {
    let bytes = std::fs::read(path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
    if bytes.len() > 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 {
        return Ok(bytes);
    }
    let img = image::load_from_memory(&bytes).map_err(|e| Error::Decode(format!("cover: {e}")))?;
    let mut out = Vec::new();
    let rgb = img.to_rgb8();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 92);
    enc.encode_image(&rgb).map_err(|e| Error::Encode(format!("cover: {e}")))?;
    Ok(out)
}

fn fmt_time(s: f64) -> String {
    let m = (s / 60.0).floor();
    format!("{}:{:06.3}", m as u64, s - m * 60.0)
}

/// Make sure every track has a fresh analysis (needed for leveling gains).
pub fn ensure_analysis(p: &mut Project, sources: &Sources, progress: &(dyn Fn(f32, String) + Sync), cancel: &AtomicBool) -> Result<()> {
    let stale: Vec<u64> = p.ordered_tracks().iter().filter(|t| !p.analysis_fresh(t)).map(|t| t.id).collect();
    let n = stale.len().max(1);
    for (i, id) in stale.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let t = p.track(*id).cloned().ok_or(Error::Project("track vanished".into()))?;
        progress(i as f32 / n as f32, format!("Analyzing {}", if t.title.is_empty() { "track" } else { &t.title }));
        let src = sources.get(&t.source_id).ok_or(Error::Project("source not loaded".into()))?;
        if let Some(a) = render::analyze_track(p, &t, src) {
            if let Some(tt) = p.tracks.iter_mut().find(|x| x.id == *id) {
                tt.analysis = Some(a.analysis);
            }
        }
    }
    Ok(())
}

pub fn export(p: &mut Project, project_dir: &Path, sources: &Sources, progress: &(dyn Fn(f32, String) + Sync), cancel: &AtomicBool) -> Result<ExportResult> {
    if p.tracks.is_empty() {
        return Err(Error::Project("there are no tracks to export".into()));
    }
    if let Some(t) = p.tracks.iter().find(|t| t.title.trim().is_empty()) {
        return Err(Error::Project(format!("track {} has no title", t.number.map(|n| n.to_string()).unwrap_or_else(|| "?".into()))));
    }
    progress(0.0, "Analyzing".into());
    ensure_analysis(p, sources, &|f, m| progress(f * 0.3, m), cancel)?;

    let dir = album_dir(p);
    std::fs::create_dir_all(&dir).map_err(|e| Error::Io(format!("{}: {e}", dir.display())))?;
    let cover = match p.album.cover_path.as_ref().filter(|c| !c.is_empty()) {
        Some(c) => {
            let jpg = load_cover(Path::new(c))?;
            std::fs::write(dir.join("cover.jpg"), &jpg).map_err(|e| Error::Io(e.to_string()))?;
            Some(jpg)
        }
        None => None,
    };

    let mut files = Vec::new();
    let mut warnings = Vec::new();
    let order: Vec<Track> = p.ordered_tracks().into_iter().cloned().collect();
    let total = order.len();
    let mut lines: Vec<String> = Vec::new();
    for (i, t) in order.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        progress(0.3 + 0.6 * i as f32 / total as f32, format!("Exporting {}", t.title));
        let src = sources.get(&t.source_id).ok_or(Error::Project("source not loaded".into()))?;
        let srcmeta = p.source(t.source_id).cloned().ok_or(Error::Project("source missing".into()))?;
        let chain = p.effective_chain(t).clone();
        let pass = p.is_passthrough(t);
        let name = track_file_name(t);
        let path = dir.join(format!("{name}.flac"));

        let (ints, bits, after, gr) = if pass {
            let bits = if src.bits <= 16 { 16 } else { 24 };
            let (start, len) = p.track_range(t).unwrap_or((0, src.frames));
            let ints = src.read_int_interleaved(start as usize, start as usize + len, bits);
            let after = loudness::measure(&ints_to_planar(&ints, src.channels, bits), src.rate).0;
            (ints, bits, after, 0.0)
        } else {
            let bits = if p.export.bit_depth >= 24 { 24 } else { 16 };
            let mut fix = t.analysis.as_ref().map(|a| a.ceiling_fix_db).unwrap_or(0.0);
            let mut tries = 0;
            loop {
                let (audio, gr) = render::render_final(p, t, src, fix).ok_or(Error::Project("render failed".into()))?;
                let ints = to_int_dithered(&audio, bits, t.id.wrapping_mul(7919) + tries);
                let after = loudness::measure(&ints_to_planar(&ints, src.channels, bits), src.rate).0;
                // Re-check: the integer file must not read over the ceiling.
                if chain.level.enabled && after.true_peak > chain.level.ceiling_dbtp && tries < 3 {
                    fix += after.true_peak - chain.level.ceiling_dbtp + 0.05;
                    tries += 1;
                    continue;
                }
                if chain.level.enabled && after.true_peak > chain.level.ceiling_dbtp {
                    warnings.push(format!("{}: true peak {:.2} dBTP still over the ceiling after 3 tries", t.title, after.true_peak));
                }
                if let Some(tt) = p.tracks.iter_mut().find(|x| x.id == t.id) {
                    if let Some(a) = tt.analysis.as_mut() {
                        a.ceiling_fix_db = fix;
                        a.after = Some(after);
                        a.limiter_gr_db = gr;
                    }
                }
                break (ints, bits, after, gr);
            }
        };
        encode_flac(&path, &ints, src.channels, bits, src.rate)?;
        let album_artist = if p.album.album_artist.trim().is_empty() { p.album.artist.clone() } else { p.album.album_artist.clone() };
        let tags = TagSet {
            title: t.title.clone(),
            artist: if p.album.artist.trim().is_empty() { t.tags.artist.clone().unwrap_or_default() } else { p.album.artist.clone() },
            album: if p.album.album.trim().is_empty() { t.tags.album.clone().unwrap_or_default() } else { p.album.album.clone() },
            album_artist,
            year: if p.album.year.trim().is_empty() { t.tags.year.clone().unwrap_or_default() } else { p.album.year.clone() },
            genre: if p.album.genre.trim().is_empty() { t.tags.genre.clone().unwrap_or_default() } else { p.album.genre.clone() },
            track: t.number,
            track_total: Some(order.iter().filter(|x| x.disc.unwrap_or(1) == t.disc.unwrap_or(1)).count() as u32),
            disc: t.disc,
        };
        let keep = if srcmeta.kind == SourceKind::Folder { Some(p.resolve(project_dir, &srcmeta.path)) } else { None };
        write_tags(&path, &tags, cover.as_deref(), keep.as_deref())?;
        files.push(path);

        // report lines
        let an = p.track(t.id).and_then(|x| x.analysis.clone()).unwrap_or_default();
        let gain = p.gain_for(t);
        let mut l = vec![format!("{} {}", t.number.map(|n| format!("{n:02}.")).unwrap_or_else(|| "--".into()), t.title)];
        l.push(format!("  File: {name}.flac ({bits}-bit, {} Hz)", src.rate));
        l.push(format!("  Source: {}, {} to {}", srcmeta.name, fmt_time(t.start_s), fmt_time(t.end_s)));
        if pass {
            l.push("  Cleaned import: passed through untouched, tags only".into());
        } else {
            l.push(format!("  Settings: {}", if t.chain.is_some() { "own settings for this track" } else { "project settings" }));
            l.push(format!("  Chain: {}", describe_chain(&chain, &srcmeta.fx)));
            l.push(format!("  Clicks repaired: {}, crackle ticks: {}", an.clicks, an.crackles));
            l.push(format!(
                "  Noise floor: {:.1} -> {:.1} dBFS ({:.1} dB lower)",
                an.noise_before_db,
                an.noise_after_db,
                an.noise_before_db - an.noise_after_db
            ));
            l.push(format!("  Loudness: {:.1} LUFS -> {:.1} LUFS (gain {:+.1} dB), LRA {:.1} LU", an.before.integrated, after.integrated, gain.gain_db, after.lra));
            l.push(format!("  True peak: {:.2} dBTP -> {:.2} dBTP, limiter at most {:.1} dB", an.before.true_peak, after.true_peak, gr));
            if gain.short_db > 0.05 {
                l.push(format!("  NOTE: stopped {:.1} dB short of the {:.1} LUFS target; reaching it needs more than {:.1} dB of limiting", gain.short_db, chain.level.target_lufs, chain.level.max_gr_db));
                warnings.push(format!("{}: {:.1} dB short of target", t.title, gain.short_db));
            }
        }
        lines.push(l.join("\n"));
    }

    // Raw captures
    progress(0.92, "Writing raw captures".into());
    let raw_dir = dir.join("Raw");
    for s in p.sources.iter().filter(|s| s.raw) {
        std::fs::create_dir_all(&raw_dir).map_err(|e| Error::Io(e.to_string()))?;
        let from = p.resolve(project_dir, &s.path);
        let is_wav = from.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("wav")).unwrap_or(false);
        let dest = match s.kind {
            SourceKind::Folder => raw_dir.join(from.file_name().unwrap_or_default()),
            _ => raw_dir.join(format!("{}.wav", safe_name(&s.name))),
        };
        if s.kind == SourceKind::Folder || is_wav {
            std::fs::copy(&from, &dest).map_err(|e| Error::Io(format!("{}: {e}", from.display())))?;
        } else if let Some(a) = sources.get(&s.id) {
            audio::write_source_wav(&dest, a)?;
        }
        files.push(dest);
    }

    // Report
    let album_lufs = p.album_lufs();
    let mut rep = String::new();
    rep.push_str("Runout processing report\n");
    rep.push_str(&format!("Album: {} - {}\n", p.album.album_artist.trim().is_empty().then(|| p.album.artist.clone()).unwrap_or(p.album.album_artist.clone()), p.album.album));
    rep.push_str(&format!("Exported: {} UTC\n", utc_now()));
    let lv = &p.chain.level;
    rep.push_str(&format!(
        "Leveling: {} mode, target {:.1} LUFS, ceiling {:.1} dBTP, limiter cap {:.1} dB{}\n",
        if lv.mode == crate::chain::LevelMode::Album { "album" } else { "track" },
        lv.target_lufs,
        lv.ceiling_dbtp,
        lv.max_gr_db,
        album_lufs.map(|a| format!(", album loudness {:.1} LUFS (duration weighted)", a)).unwrap_or_default()
    ));
    rep.push_str(&format!("Output: FLAC, {}-bit{}\n\n", p.export.bit_depth, if p.export.bit_depth == 16 { " with TPDF dither" } else { "" }));
    rep.push_str(&lines.join("\n\n"));
    rep.push('\n');
    if !warnings.is_empty() {
        rep.push_str("\nWarnings:\n");
        for w in &warnings {
            rep.push_str(&format!("  {w}\n"));
        }
    }
    let report = dir.join("Runout report.txt");
    std::fs::write(&report, rep).map_err(|e| Error::Io(e.to_string()))?;
    progress(1.0, "Done".into());
    Ok(ExportResult { album_dir: dir, files, report, warnings })
}

pub fn describe_chain(c: &crate::chain::Chain, fx: &crate::chain::SourceFx) -> String {
    let mut v = Vec::new();
    let r = fx.speed.ratio();
    if r != 1.0 {
        v.push(format!("speed x{r:.4}"));
    }
    if c.rumble.enabled {
        v.push(format!("rumble {} Hz {} dB/oct{}", c.rumble.freq_hz, c.rumble.slope_db_oct, if c.rumble.linear_phase { " linear phase" } else { "" }));
    }
    if c.hum.enabled {
        v.push(format!("hum {} Hz x{} {} Hz wide", c.hum.mains_hz, c.hum.harmonics, c.hum.width_hz));
    }
    if c.declick.enabled {
        v.push(format!("declick {:.1} dB/{:.1} ms", c.declick.threshold_db, c.declick.max_len_ms));
    }
    if c.decrackle.enabled {
        v.push(format!("decrackle {:.1} dB", c.decrackle.threshold_db));
    }
    if !fx.repairs.is_empty() {
        v.push(format!("{} manual repairs on this side", fx.repairs.len()));
    }
    if c.declip.enabled {
        v.push(format!("declip at {:.2} dBFS", c.declip.level_db));
    }
    if c.denoise.enabled {
        v.push(format!("denoise {:.1} dB, smoothing {:.0}%{}", c.denoise.reduction_db, c.denoise.smoothing_pct, if fx.noise.is_none() { " (no profile, skipped)" } else { "" }));
    }
    if c.channel.enabled {
        v.push(format!("balance {:+.1} dB{}, azimuth {:.0} us", c.channel.balance_db, if c.channel.swap { ", swapped" } else { "" }, c.channel.azimuth_us));
    }
    if c.eq.enabled {
        v.push(format!("EQ low {:+.1} dB high {:+.1} dB, {} bands", c.eq.low_shelf_db, c.eq.high_shelf_db, c.eq.bands.len()));
    }
    if v.is_empty() {
        "DC removal only".into()
    } else {
        v.join(", ")
    }
}

/// "YYYY-MM-DD HH:MM" in UTC without a date library.
pub fn utc_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // civil from days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, (rem % 3600) / 60)
}
