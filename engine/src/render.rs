//! Track rendering for analysis and export, and block rendering for preview.

use crate::audio::SourceAudio;
use crate::chain::{self, Chain, ClickHit, RenderReq};
use crate::dsp::loudness;
use crate::level;
use crate::project::{Project, Track, TrackAnalysis};
use rayon::prelude::*;

/// Chunk length for parallel track rendering.
pub const CHUNK_S: usize = 8;

pub struct TrackAudio {
    pub out: Vec<Vec<f32>>,
    pub pre: Option<Vec<Vec<f32>>>,
    pub clicks: Vec<ClickHit>,
}

fn ms(rate: u32, v: f32) -> usize {
    ((v.max(0.0) / 1000.0) * rate as f32).round() as usize
}

/// Cleaned audio (stages 1 to 11) for a processed-time range, rendered in
/// parallel chunks. Each chunk carries its own padding, so the result matches a
/// single long render to within the settling of the filters.
pub fn render_range(src: &SourceAudio, fx: &chain::SourceFx, chain: &Chain, start: i64, len: usize, passthrough: bool, want_pre: bool, want_clicks: bool) -> TrackAudio {
    let rate = src.rate as usize;
    let chunk = rate * CHUNK_S;
    let n = len.div_ceil(chunk).max(1);
    let parts: Vec<chain::RenderOut> = (0..n)
        .into_par_iter()
        .map(|i| {
            let s = start + (i * chunk) as i64;
            let l = chunk.min(len - (i * chunk).min(len));
            chain::render(&RenderReq { src, fx, chain, start: s, len: l, want_pre, want_removed: false, want_clicks, passthrough })
        })
        .collect();
    let chs = src.channels;
    let mut out = vec![Vec::with_capacity(len); chs];
    let mut pre = if want_pre { Some(vec![Vec::with_capacity(len); chs]) } else { None };
    let mut clicks = Vec::new();
    for p in parts {
        for c in 0..chs {
            out[c].extend_from_slice(&p.out[c]);
            if let (Some(pre), Some(pp)) = (pre.as_mut(), p.pre.as_ref()) {
                pre[c].extend_from_slice(&pp[c]);
            }
        }
        clicks.extend(p.clicks);
    }
    TrackAudio { out, pre, clicks }
}

/// Merge hits from different channels that refer to the same click.
pub fn dedupe_clicks(mut v: Vec<ClickHit>, rate: u32) -> Vec<ClickHit> {
    v.sort_by_key(|h| (h.crackle, h.pos));
    let tol = (rate / 1000) as i64;
    let mut out: Vec<ClickHit> = Vec::with_capacity(v.len());
    for h in v {
        if let Some(l) = out.last_mut() {
            if l.crackle == h.crackle && h.pos - l.pos <= tol {
                l.size = l.size.max(h.size);
                continue;
            }
        }
        out.push(h);
    }
    out.sort_by_key(|h| h.pos);
    out
}

pub struct Analysis {
    pub analysis: TrackAnalysis,
    pub clicks: Vec<ClickHit>,
    /// Processed-time start of the track and peaks of the final (leveled) audio.
    pub start: i64,
    pub peaks: Vec<(f32, f32)>,
}

/// Render a track, measure it before and after leveling.
pub fn analyze_track(p: &Project, t: &Track, src: &SourceAudio) -> Option<Analysis> {
    let s = p.source(t.source_id)?;
    let (start, len) = p.track_range(t)?;
    let chain = p.effective_chain(t);
    let pass = p.is_passthrough(t);
    let mut ta = render_range(src, &s.fx, chain, start, len, pass, true, true);
    let rate = src.rate;
    if !pass {
        level::apply_fades(&mut ta.out, 0, len, ms(rate, t.fade_in_ms), ms(rate, t.fade_out_ms));
    }
    let (before, _) = loudness::measure(&ta.out, rate);
    let noise_before = ta.pre.as_ref().map(|x| loudness::noise_floor_db(x, rate)).unwrap_or(-120.0);
    let noise_after = loudness::noise_floor_db(&ta.out, rate);
    let clicks = dedupe_clicks(std::mem::take(&mut ta.clicks), rate);
    let mut an = TrackAnalysis {
        key: p.track_key(t),
        before,
        after: None,
        gain: Default::default(),
        limiter_gr_db: 0.0,
        clicks: clicks.iter().filter(|c| !c.crackle).count() as u32,
        crackles: clicks.iter().filter(|c| c.crackle).count() as u32,
        noise_before_db: noise_before,
        noise_after_db: noise_after,
        duration_s: len as f64 / rate as f64,
        ceiling_fix_db: 0.0,
    };
    // Measure what leveling would do with track-mode or current album gain.
    let mut tmp = p.clone();
    if let Some(tt) = tmp.tracks.iter_mut().find(|x| x.id == t.id) {
        tt.analysis = Some(an.clone());
    }
    let tref = tmp.track(t.id)?.clone();
    let gain = tmp.gain_for(&tref);
    an.gain = gain;
    let mut out = ta.out;
    if chain.level.enabled && !pass {
        an.limiter_gr_db = level::apply(&mut out, rate, &chain.level, gain.gain_db, 0.0);
        an.after = Some(loudness::measure(&out, rate).0);
    } else {
        an.after = Some(before);
    }
    let peaks = crate::tiles::peaks_of(&out);
    Some(Analysis { analysis: an, clicks, start, peaks })
}

/// Final audio for one track: cleaned, faded, leveled. Requires fresh analysis
/// for the gain.
pub fn render_final(p: &Project, t: &Track, src: &SourceAudio, ceiling_fix_db: f32) -> Option<(Vec<Vec<f32>>, f32)> {
    let s = p.source(t.source_id)?;
    let (start, len) = p.track_range(t)?;
    let chain = p.effective_chain(t);
    let pass = p.is_passthrough(t);
    let mut ta = render_range(src, &s.fx, chain, start, len, pass, false, false);
    if pass {
        return Some((ta.out, 0.0));
    }
    let rate = src.rate;
    level::apply_fades(&mut ta.out, 0, len, ms(rate, t.fade_in_ms), ms(rate, t.fade_out_ms));
    let gain = p.gain_for(t);
    let gr = level::apply(&mut ta.out, rate, &chain.level, gain.gain_db, ceiling_fix_db);
    Some((ta.out, gr))
}

pub struct Block {
    pub pre: Vec<Vec<f32>>,
    pub out: Vec<Vec<f32>>,
    pub removed: Vec<Vec<f32>>,
}

/// One preview block for a source: what playback hears, with the per-track
/// chain, fades, gain and limiter applied the same way export does.
pub fn render_play_block(p: &Project, source_id: u64, src: &SourceAudio, start: i64, len: usize) -> Option<Block> {
    let s = p.source(source_id)?;
    let rate = src.rate;
    let head = ms(rate, 50.0);
    let tail = ms(rate, 10.0);
    let a = start - head as i64;
    let total = len + head + tail;
    let tracks: Vec<&Track> = p
        .tracks
        .iter()
        .filter(|t| t.source_id == source_id)
        .filter(|t| p.track_range(t).map(|(ts, tl)| ts < start + len as i64 && ts + tl as i64 > start).unwrap_or(false))
        .collect();

    let base = chain::render(&RenderReq {
        src,
        fx: &s.fx,
        chain: &p.chain,
        start: a,
        len: total,
        want_pre: true,
        want_removed: true,
        want_clicks: false,
        passthrough: false,
    });
    let chs = src.channels;
    let crop = |v: &Vec<Vec<f32>>| -> Vec<Vec<f32>> { v.iter().map(|c| c[head..head + len].to_vec()).collect() };
    let pre = crop(base.pre.as_ref()?);
    let mut out = crop(&base.out);
    let mut removed = crop(base.removed.as_ref()?);

    for t in tracks {
        let (ts, tl) = p.track_range(t)?;
        let chain = p.effective_chain(t);
        let pass = p.is_passthrough(t);
        let r = if chain == &p.chain && !pass {
            None
        } else {
            Some(chain::render(&RenderReq { src, fx: &s.fx, chain, start: a, len: total, want_pre: false, want_removed: true, want_clicks: false, passthrough: pass }))
        };
        let buf_src = r.as_ref().map(|x| &x.out).unwrap_or(&base.out);
        let mut seg: Vec<Vec<f32>> = buf_src.clone();
        if !pass {
            level::apply_fades(&mut seg, ts - a, tl, ms(rate, t.fade_in_ms), ms(rate, t.fade_out_ms));
            let gain = p.gain_for(t);
            level::apply(&mut seg, rate, &chain.level, gain.gain_db, t.analysis.as_ref().map(|x| x.ceiling_fix_db).unwrap_or(0.0));
        }
        let lo = (ts - start).max(0) as usize;
        let hi = ((ts + tl as i64 - start).min(len as i64)).max(0) as usize;
        for c in 0..chs {
            out[c][lo..hi].copy_from_slice(&seg[c][head + lo..head + hi]);
            if let Some(rm) = r.as_ref().and_then(|x| x.removed.as_ref()) {
                removed[c][lo..hi].copy_from_slice(&rm[c][head + lo..head + hi]);
            }
        }
    }
    Some(Block { pre, out, removed })
}
