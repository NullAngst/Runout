//! Export: tagged FLAC per track, raw captures as WAV, cover art, report.

use crate::audio::{self, SourceAudio};
use crate::dsp::loudness;
use crate::project::{expand_home, ExportKind, Project, SourceKind, Track};
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

/// Where an export goes:
///
/// - Album: {root}/{artist}/{album artist} - {album}/ where the artist folder
///   is the album's artist, falling back to the album artist, then Unknown
///   Artist.
/// - Mixtape: {root}/Mixtapes/{title}/
/// - Recording: {root}/Recordings/{date} {title}/ (date from the year field,
///   left out when empty).
pub fn album_dir(p: &Project) -> PathBuf {
    let root = expand_home(&p.export.root);
    let title = || if p.album.album.trim().is_empty() { p.name.trim().to_string() } else { p.album.album.trim().to_string() };
    match p.export.kind {
        ExportKind::Album => {
            let aa = if !p.album.album_artist.trim().is_empty() { &p.album.album_artist } else { &p.album.artist };
            let name = format!("{} - {}", if aa.trim().is_empty() { "Unknown Artist" } else { aa.trim() }, if p.album.album.trim().is_empty() { "Unknown Album" } else { p.album.album.trim() });
            let artist = [&p.album.artist, &p.album.album_artist].into_iter().map(|s| s.trim()).find(|s| !s.is_empty()).unwrap_or("Unknown Artist");
            root.join(safe_name(artist)).join(safe_name(&name))
        }
        ExportKind::Mixtape => root.join("Mixtapes").join(safe_name(&title())),
        ExportKind::Recording => {
            let date = p.album.year.trim();
            let name = if date.is_empty() { title() } else { format!("{date} {}", title()) };
            root.join("Recordings").join(safe_name(&name))
        }
    }
}

/// A track's artist: its own tag, else the album's.
fn track_artist(p: &Project, t: &Track) -> String {
    t.tags.artist.clone().filter(|a| !a.trim().is_empty()).unwrap_or_else(|| if p.album.artist.trim().is_empty() { "Unknown Artist".into() } else { p.album.artist.clone() })
}

/// A track's title, with Recording's automatic "Part N" when it has none.
pub fn track_title(p: &Project, t: &Track) -> String {
    if t.title.trim().is_empty() && p.export.kind == ExportKind::Recording {
        format!("Part {}", t.number.unwrap_or(1))
    } else {
        t.title.trim().to_string()
    }
}

/// File name for a track in this project's layout.
pub fn track_file_name_in(p: &Project, t: &Track) -> String {
    match p.export.kind {
        ExportKind::Album => track_file_name(t),
        ExportKind::Mixtape => {
            if let Some(f) = t.file_name.as_ref().filter(|f| !f.trim().is_empty()) {
                return safe_name(f);
            }
            safe_name(&format!("{:02}. {} - {}", t.number.unwrap_or(0), track_artist(p, t).trim(), t.title.trim()))
        }
        ExportKind::Recording => safe_name(&format!("{:02}. {}", t.number.unwrap_or(0), track_title(p, t))),
    }
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

/// Set extra Vorbis comment fields (MEDIA, RUNOUT_*) on a FLAC, replacing any
/// already there.
pub fn write_fields(path: &Path, fields: &[(String, String)]) -> Result<()> {
    use lofty::config::{ParseOptions, WriteOptions};
    use lofty::file::AudioFile;
    use lofty::ogg::tag::VorbisComments;
    if fields.is_empty() {
        return Ok(());
    }
    let mut f = std::fs::File::open(path).map_err(|e| Error::Io(e.to_string()))?;
    let mut ff = lofty::flac::FlacFile::read_from(&mut f, ParseOptions::new()).map_err(|e| Error::Encode(format!("tags: {e}")))?;
    drop(f);
    if ff.vorbis_comments().is_none() {
        ff.set_vorbis_comments(VorbisComments::default());
    }
    let vc = ff.vorbis_comments_mut().ok_or_else(|| Error::Encode("tags: no vorbis comments".into()))?;
    for (k, v) in fields {
        vc.insert(k.clone(), v.clone());
    }
    ff.save_to_path(path, WriteOptions::default()).map_err(|e| Error::Encode(format!("tags: {e}")))
}

/// The MEDIA and RUNOUT_* fields for this project, only the ones that say
/// something: no NR line on vinyl, no deck line without a deck.
pub fn media_fields(p: &Project, t: &Track) -> Vec<(String, String)> {
    let mut v = Vec::new();
    let m = &p.medium.preset;
    if !m.media_tag.trim().is_empty() {
        v.push(("MEDIA".to_string(), m.media_tag.clone()));
    }
    if p.medium.id != "vinyl-lp" {
        v.push(("RUNOUT_MEDIUM".to_string(), m.name.clone()));
    }
    let chain = p.effective_chain(t);
    if let Some(s) = p.source(t.source_id) {
        let r = s.fx.speed.ratio();
        let wow = s.fx.wow_map(chain).is_some();
        if r != 1.0 || wow {
            let unit = match s.fx.speed.unit {
                crate::chain::SpeedUnit::Rpm => " rpm",
                crate::chain::SpeedUnit::Cms => " cm/s",
                crate::chain::SpeedUnit::Ips => " ips",
                crate::chain::SpeedUnit::None => "",
            };
            let mut txt = if s.fx.speed.unit == crate::chain::SpeedUnit::None { format!("x{r:.4}") } else { format!("{}{unit}, x{r:.4}", trim_num(s.fx.speed.correct)) };
            if wow {
                txt.push_str(&format!(", wow corrected from the {}", if chain.wow.reference == crate::chain::WowRef::Hum { "hum" } else { "tone" }));
            }
            v.push(("RUNOUT_SPEED".to_string(), txt));
        }
    }
    if chain.nr.enabled && !chain.digital {
        v.push(("RUNOUT_NR".to_string(), match chain.nr.kind {
            crate::dsp::nr::NrKind::Btype => format!("B-type decode, reference {:.1} dBFS", chain.nr.ref_dbfs),
            crate::dsp::nr::NrKind::Dbx2 => "dbx II decode".into(),
        }));
    }
    if let Some(d) = p.deck.as_deref() {
        let name = crate::media::find_deck(d).map(|d| d.name).unwrap_or_else(|| d.to_string());
        v.push(("RUNOUT_DECK".to_string(), name));
    }
    v
}

fn trim_num(v: f32) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
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
    if p.export.kind != ExportKind::Recording {
        if let Some(t) = p.tracks.iter().find(|t| t.title.trim().is_empty()) {
            return Err(Error::Project(format!("track {} has no title", t.number.map(|n| n.to_string()).unwrap_or_else(|| "?".into()))));
        }
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
        let name = track_file_name_in(p, t);
        let path = dir.join(format!("{name}.flac"));
        let title = track_title(p, t);

        let (ints, bits, after, gr, out_rate, out_ch) = if pass {
            let bits = if src.bits <= 16 { 16 } else { 24 };
            let (start, len) = p.track_range(t).unwrap_or((0, src.frames));
            let ints = src.read_int_interleaved(start as usize, start as usize + len, bits);
            let after = loudness::measure(&ints_to_planar(&ints, src.channels, bits), src.rate).0;
            (ints, bits, after, 0.0, src.rate, src.channels)
        } else {
            // digital sources keep their depth and are rounded, never dithered,
            // so samples no stage touched come out exactly as they went in
            let bits = if chain.digital { if src.bits <= 16 { 16 } else { 24 } } else if p.export.bit_depth >= 24 { 24 } else { 16 };
            let dither = p.export.dither && !chain.digital;
            let mut fix = t.analysis.as_ref().map(|a| a.ceiling_fix_db).unwrap_or(0.0);
            let mut tries = 0;
            loop {
                let (audio, gr, rate) = render::render_final_at(p, t, src, fix).ok_or(Error::Project("render failed".into()))?;
                let chs = audio.len();
                let ints = if dither { to_int_dithered(&audio, bits, t.id.wrapping_mul(7919) + tries) } else { to_int_rounded(&audio, bits) };
                let after = loudness::measure(&ints_to_planar(&ints, chs, bits), rate).0;
                // Re-check: the integer file must not read over the ceiling.
                if chain.level.enabled && after.true_peak > chain.level.ceiling_dbtp && tries < 3 {
                    fix += after.true_peak - chain.level.ceiling_dbtp + 0.05;
                    tries += 1;
                    continue;
                }
                if chain.level.enabled && after.true_peak > chain.level.ceiling_dbtp {
                    warnings.push(format!("{}: true peak {:.2} dBTP still over the ceiling after 3 tries", title, after.true_peak));
                }
                if let Some(tt) = p.tracks.iter_mut().find(|x| x.id == t.id) {
                    if let Some(a) = tt.analysis.as_mut() {
                        a.ceiling_fix_db = fix;
                        a.after = Some(after);
                        a.limiter_gr_db = gr;
                    }
                }
                break (ints, bits, after, gr, rate, chs);
            }
        };
        encode_flac(&path, &ints, out_ch, bits, out_rate)?;
        let mixtape = p.export.kind == ExportKind::Mixtape;
        let album_artist = if mixtape { "Various Artists".to_string() } else if p.album.album_artist.trim().is_empty() { p.album.artist.clone() } else { p.album.album_artist.clone() };
        let tags = TagSet {
            title: title.clone(),
            artist: if mixtape { track_artist(p, t) } else if p.album.artist.trim().is_empty() { t.tags.artist.clone().unwrap_or_default() } else { p.album.artist.clone() },
            album: if mixtape {
                if p.album.album.trim().is_empty() { p.name.clone() } else { p.album.album.clone() }
            } else if p.album.album.trim().is_empty() {
                t.tags.album.clone().unwrap_or_default()
            } else {
                p.album.album.clone()
            },
            album_artist,
            year: if p.album.year.trim().is_empty() { t.tags.year.clone().unwrap_or_default() } else { p.album.year.clone() },
            genre: if p.album.genre.trim().is_empty() { t.tags.genre.clone().unwrap_or_default() } else { p.album.genre.clone() },
            track: t.number,
            track_total: Some(order.iter().filter(|x| x.disc.unwrap_or(1) == t.disc.unwrap_or(1)).count() as u32),
            disc: t.disc,
        };
        let keep = if srcmeta.kind == SourceKind::Folder { Some(p.resolve(project_dir, &srcmeta.path)) } else { None };
        write_tags(&path, &tags, cover.as_deref(), keep.as_deref())?;
        write_fields(&path, &media_fields(p, t))?;
        files.push(path);

        // report lines
        let an = p.track(t.id).and_then(|x| x.analysis.clone()).unwrap_or_default();
        let gain = p.gain_for(t);
        let mut l = vec![format!("{} {}", t.number.map(|n| format!("{n:02}.")).unwrap_or_else(|| "--".into()), title)];
        l.push(format!("  File: {name}.flac ({bits}-bit, {out_rate} Hz{})", if out_ch == 1 { ", mono" } else { "" }));
        l.push(format!("  Source: {}, {} to {}", srcmeta.name, fmt_time(t.start_s), fmt_time(t.end_s)));
        if pass {
            l.push("  Cleaned import: passed through untouched, tags only".into());
        } else {
            l.push(format!("  Settings: {}", if t.chain.is_some() { "own settings for this track" } else { "project settings" }));
            l.push(format!("  Chain: {}", describe_chain(&chain, &srcmeta.fx)));
            l.push(format!("  Clicks repaired: {}, crackle ticks: {}", an.clicks, an.crackles));
            for extra in tape_report_lines(&chain, &srcmeta.fx, &an) {
                l.push(extra);
            }
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
                warnings.push(format!("{}: {:.1} dB short of target", title, gain.short_db));
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
    rep.push_str(&format!(
        "Medium: {}{}{}\n",
        p.medium.preset.name,
        p.medium.condition.as_ref().and_then(|c| crate::presets::find(c)).map(|c| format!(", condition: {}", c.name)).unwrap_or_default(),
        p.deck.as_ref().map(|d| format!(", deck: {}", crate::media::find_deck(d).map(|x| x.name).unwrap_or_else(|| d.clone()))).unwrap_or_default()
    ));
    let lv = &p.chain.level;
    rep.push_str(&format!(
        "Leveling: {} mode, target {:.1} LUFS, ceiling {:.1} dBTP, limiter cap {:.1} dB{}\n",
        if lv.mode == crate::chain::LevelMode::Album { "album" } else { "track" },
        lv.target_lufs,
        lv.ceiling_dbtp,
        lv.max_gr_db,
        album_lufs.map(|a| format!(", album loudness {:.1} LUFS (duration weighted)", a)).unwrap_or_default()
    ));
    if p.chain.digital {
        rep.push_str("Output: FLAC at the source rate and depth, rounded without dither (untouched samples are bit-identical)\n\n");
    } else if p.export.rate > 0 || p.export.channels > 0 || !p.export.dither {
        rep.push_str(&format!(
            "Output: FLAC, {}-bit{}{}\n\n",
            p.export.bit_depth,
            if p.export.dither { " with TPDF dither" } else { "" },
            if p.export.rate > 0 { format!(", {} Hz", p.export.rate) } else { String::new() }
        ));
    } else {
        rep.push_str(&format!("Output: FLAC, {}-bit{}\n\n", p.export.bit_depth, if p.export.bit_depth == 16 { " with TPDF dither" } else { "" }));
    }
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
    // the fully resolved medium, condition and deck, next to the report
    let resolved = serde_json::json!({
        "medium": p.medium,
        "condition": p.medium.condition.as_ref().and_then(|c| crate::presets::find(c)),
        "deck": p.deck.as_ref().and_then(|d| crate::media::find_deck(d)),
        "chain": p.chain,
        "split": p.split,
        "export": p.export,
    });
    std::fs::write(dir.join("preset.json"), serde_json::to_string_pretty(&resolved).unwrap_or_default()).map_err(|e| Error::Io(e.to_string()))?;
    progress(1.0, "Done".into());
    Ok(ExportResult { album_dir: dir, files, report, warnings })
}

/// Float to integer by plain rounding (digital sources: no dither).
pub fn to_int_rounded(ch: &[Vec<f32>], bits: u16) -> Vec<i32> {
    let n = ch.first().map(|c| c.len()).unwrap_or(0);
    let scale = (1i64 << (bits - 1)) as f64;
    let mut out = Vec::with_capacity(n * ch.len());
    for i in 0..n {
        for c in ch {
            out.push((c[i] as f64 * scale).round().clamp(-scale, scale - 1.0) as i32);
        }
    }
    out
}

fn summarize(v: &[f32]) -> (f32, f32, f32) {
    let mn = v.iter().cloned().fold(f32::MAX, f32::min);
    let mx = v.iter().cloned().fold(f32::MIN, f32::max);
    let mean = v.iter().sum::<f32>() / v.len().max(1) as f32;
    (mn, mx, mean)
}

/// Report lines for the tape and digital stages, only when they did something.
pub fn tape_report_lines(c: &crate::chain::Chain, fx: &crate::chain::SourceFx, an: &crate::project::TrackAnalysis) -> Vec<String> {
    let mut l = Vec::new();
    if let (true, Some(t)) = (fx.wow_map(c).is_some(), fx.analysis.wow_track()) {
        let m = t.mean() as f32;
        let speeds: Vec<f32> = t.freq.iter().map(|f| f / m).collect();
        let (a, b, mean) = summarize(&speeds);
        l.push(format!("  Speed curve: x{a:.4} to x{b:.4}, mean x{mean:.4}{}", fx.analysis.wow_weighted_pct.map(|w| format!(", weighted wow and flutter before {w:.3}%")).unwrap_or_default()));
    }
    if c.azimuth.enabled {
        match fx.analysis.azimuth() {
            Some(curve) => {
                let (a, b, mean) = summarize(&curve.us);
                l.push(format!("  Azimuth: {a:.1} to {b:.1} us, mean {mean:.1} us ({})", if c.azimuth.mode == crate::chain::AzimuthMode::Static || (c.azimuth.mode == crate::chain::AzimuthMode::Auto && curve.is_static()) { "static" } else { "tracked" }));
            }
            None => l.push(format!("  Azimuth: {:.1} us static", c.azimuth.static_us)),
        }
    }
    if c.dropout.enabled {
        l.push(format!("  Dropouts repaired: {}{}", an.dropouts, if an.dropouts_flagged > 0 { format!(", {} flagged for manual repair", an.dropouts_flagged) } else { String::new() }));
    }
    if c.digital && c.error_repair.enabled {
        l.push(format!("  Digital errors repaired: {}{}", an.errors, if an.errors_flagged > 0 { format!(", {} flagged for manual repair", an.errors_flagged) } else { String::new() }));
    }
    if c.nr.enabled && !c.digital {
        l.push(match c.nr.kind {
            crate::dsp::nr::NrKind::Btype => format!("  NR: B-type decode, reference {:.1} dBFS{}", c.nr.ref_dbfs, if c.nr.ref_measured { " (from the tape's level tone)" } else { "" }),
            crate::dsp::nr::NrKind::Dbx2 => "  NR: dbx II decode".into(),
        });
    }
    l
}

pub fn describe_chain(c: &crate::chain::Chain, fx: &crate::chain::SourceFx) -> String {
    let mut v = Vec::new();
    if c.digital {
        if c.error_repair.enabled {
            v.push("error repair".to_string());
        }
        if c.deemphasis.enabled {
            v.push("de-emphasis 50/15 us".to_string());
        }
        if !fx.repairs.is_empty() {
            v.push(format!("{} manual repairs on this side", fx.repairs.len()));
        }
        return if v.is_empty() { "nothing: bit-identical".into() } else { v.join(", ") };
    }
    let r = fx.speed.ratio();
    if r != 1.0 {
        v.push(format!("speed x{r:.4}"));
    }
    if fx.wow_map(c).is_some() {
        v.push("wow corrected".into());
    }
    if c.azimuth.enabled {
        v.push("azimuth".into());
    }
    if c.channel_mode.enabled && c.channel_mode.mode != crate::chain::ChannelModeKind::Stereo {
        v.push(
            match c.channel_mode.mode {
                crate::chain::ChannelModeKind::MonoSum => "mono sum",
                crate::chain::ChannelModeKind::MonoLeft => "mono, left channel",
                crate::chain::ChannelModeKind::MonoRight => "mono, right channel",
                _ => "mono, best channel",
            }
            .to_string(),
        );
    }
    if c.playback_eq.enabled && c.playback_eq.from.curve != c.playback_eq.to.curve {
        v.push(format!("playback EQ {} to {}", c.playback_eq.from.id, c.playback_eq.to.id));
    }
    if c.nr.enabled {
        v.push(match c.nr.kind {
            crate::dsp::nr::NrKind::Btype => format!("B-type decode at {:.1} dBFS", c.nr.ref_dbfs),
            crate::dsp::nr::NrKind::Dbx2 => "dbx II decode".into(),
        });
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
    if c.dropout.enabled {
        v.push(format!("dropout repair at {:.0} dB", c.dropout.depth_db));
    }
    if c.denoise.enabled {
        v.push(format!("denoise {:.1} dB, smoothing {:.0}%{}", c.denoise.reduction_db, c.denoise.smoothing_pct, if fx.noise.is_none() { " (no profile, skipped)" } else { "" }));
    }
    if c.band_limit.enabled {
        v.push(format!("band limit {:.0} Hz to {:.0} Hz", c.band_limit.low_hz, c.band_limit.high_hz));
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
