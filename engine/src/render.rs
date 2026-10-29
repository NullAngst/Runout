//! Track rendering for analysis and export, and block rendering for preview.

use crate::audio::SourceAudio;
use crate::chain::{self, Chain, ClickHit, RenderReq, StageEvent};
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
    pub events: Vec<StageEvent>,
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
    let chs = parts.first().map(|p| p.out.len()).unwrap_or(src.channels);
    let pre_chs = parts.first().and_then(|p| p.pre.as_ref().map(|v| v.len())).unwrap_or(chs);
    let mut out = vec![Vec::with_capacity(len); chs];
    let mut pre = if want_pre { Some(vec![Vec::with_capacity(len); pre_chs]) } else { None };
    let mut clicks = Vec::new();
    let mut events = Vec::new();
    for p in parts {
        for c in 0..chs {
            out[c].extend_from_slice(&p.out[c]);
        }
        if let (Some(pre), Some(pp)) = (pre.as_mut(), p.pre.as_ref()) {
            for c in 0..pre_chs {
                pre[c].extend_from_slice(&pp[c]);
            }
        }
        clicks.extend(p.clicks);
        events.extend(p.events);
    }
    TrackAudio { out, pre, clicks, events }
}

/// Cut a track's joins out of its rendered audio, crossfading across each
/// cut. `start` is the processed-time start of the buffer.
pub fn apply_joins(p: &Project, t: &Track, buf: &mut Vec<Vec<f32>>, start: i64) {
    if t.joins.is_empty() {
        return;
    }
    let Some(s) = p.source(t.source_id) else { return };
    let chain = p.effective_chain(t);
    let rate = s.rate as f64;
    let mut cuts: Vec<(i64, i64, usize)> = t
        .joins
        .iter()
        .map(|j| {
            let a = s.fx.to_processed(chain, j.start_s * rate).round() as i64 - start;
            let b = s.fx.to_processed(chain, j.end_s * rate).round() as i64 - start;
            (a, b, ((j.xfade_ms.max(1.0) / 1000.0) as f64 * rate) as usize)
        })
        .filter(|(a, b, _)| b > a)
        .collect();
    // last first, so earlier positions stay valid
    cuts.sort_by_key(|c| std::cmp::Reverse(c.0));
    for (a, b, xf) in cuts {
        for c in buf.iter_mut() {
            let n = c.len() as i64;
            let a = a.clamp(0, n) as usize;
            let b = b.clamp(0, n) as usize;
            if b <= a {
                continue;
            }
            let xf = xf.min(a).min(c.len() - b);
            // crossfade the xf samples before the cut with the xf after it
            for k in 0..xf {
                let g = (k as f32 + 0.5) / xf as f32;
                let i = a - xf + k;
                c[i] = c[i] * (1.0 - g) + c[b + k] * g;
            }
            c.drain(a..b + xf);
        }
    }
}

/// Make a buffer have `n` channels: a mono buffer is copied out, a stereo one
/// summed down.
pub fn match_channels(v: Vec<Vec<f32>>, n: usize) -> Vec<Vec<f32>> {
    if v.len() == n || v.is_empty() {
        return v;
    }
    if v.len() == 1 {
        return (0..n).map(|_| v[0].clone()).collect();
    }
    let len = v[0].len();
    let k = 1.0 / v.len() as f32;
    let m: Vec<f32> = (0..len).map(|i| v.iter().map(|c| c[i]).sum::<f32>() * k).collect();
    (0..n).map(|_| m.clone()).collect()
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
    /// Dropouts and digital errors the chain found, processed time.
    pub events: Vec<StageEvent>,
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
    apply_joins(p, t, &mut ta.out, start);
    if let Some(pre) = ta.pre.as_mut() {
        apply_joins(p, t, pre, start);
    }
    let len = ta.out.first().map(|c| c.len()).unwrap_or(len);
    if !pass {
        level::apply_fades(&mut ta.out, 0, len, ms(rate, t.fade_in_ms), ms(rate, t.fade_out_ms));
    }
    let (before, _) = loudness::measure(&ta.out, rate);
    let noise_before = ta.pre.as_ref().map(|x| loudness::noise_floor_db(x, rate)).unwrap_or(-120.0);
    let noise_after = loudness::noise_floor_db(&ta.out, rate);
    let clicks = dedupe_clicks(std::mem::take(&mut ta.clicks), rate);
    let events = dedupe_events(std::mem::take(&mut ta.events), rate);
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
        dropouts: events.iter().filter(|e| e.kind == crate::analysis::EventKind::Dropout).count() as u32,
        dropouts_flagged: events.iter().filter(|e| e.kind == crate::analysis::EventKind::DropoutFlagged).count() as u32,
        errors: events.iter().filter(|e| e.kind == crate::analysis::EventKind::Error).count() as u32,
        errors_flagged: events.iter().filter(|e| e.kind == crate::analysis::EventKind::ErrorFlagged).count() as u32,
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
    Some(Analysis { analysis: an, clicks, events, start, peaks })
}

/// Merge events from different channels that refer to the same dropout.
pub fn dedupe_events(mut v: Vec<StageEvent>, rate: u32) -> Vec<StageEvent> {
    v.sort_by_key(|e| e.pos);
    let tol = (rate / 200) as i64;
    let mut out: Vec<StageEvent> = Vec::with_capacity(v.len());
    for e in v {
        if let Some(l) = out.last_mut() {
            if l.kind == e.kind && e.pos - (l.pos + l.len as i64) <= tol {
                let end = (l.pos + l.len as i64).max(e.pos + e.len as i64);
                l.len = (end - l.pos) as u32;
                l.depth_db = l.depth_db.max(e.depth_db);
                continue;
            }
        }
        out.push(e);
    }
    out
}

/// Final audio for one track: cleaned, faded, resampled to the export rate,
/// leveled. Requires fresh analysis for the gain. Returns the audio, the
/// limiter's gain reduction and the rate the audio is at.
pub fn render_final(p: &Project, t: &Track, src: &SourceAudio, ceiling_fix_db: f32) -> Option<(Vec<Vec<f32>>, f32)> {
    render_final_at(p, t, src, ceiling_fix_db).map(|(a, g, _)| (a, g))
}

/// Export rate for a source: the project's, or the source's own. A reel
/// played at 7.5 ips or faster keeps its capture rate (real content above
/// 20 kHz), unless the export rate was changed by hand.
pub fn export_rate(p: &Project, src_rate: u32) -> u32 {
    if p.export.rate > 0 {
        p.export.rate
    } else {
        src_rate
    }
}

pub fn export_rate_for(p: &Project, s: &crate::project::Source) -> u32 {
    let m = &p.medium.preset;
    if m.export.full_rate_from_speed > 0.0 && p.export.rate == m.export.rate {
        return if s.fx.speed.correct >= m.export.full_rate_from_speed { s.rate } else { m.export.rate.max(1) };
    }
    export_rate(p, s.rate)
}

pub fn render_final_at(p: &Project, t: &Track, src: &SourceAudio, ceiling_fix_db: f32) -> Option<(Vec<Vec<f32>>, f32, u32)> {
    let s = p.source(t.source_id)?;
    let (start, len) = p.track_range(t)?;
    let chain = p.effective_chain(t);
    let pass = p.is_passthrough(t);
    let mut ta = render_range(src, &s.fx, chain, start, len, pass, false, false);
    if pass {
        return Some((ta.out, 0.0, src.rate));
    }
    let rate = src.rate;
    apply_joins(p, t, &mut ta.out, start);
    let len = ta.out.first().map(|c| c.len()).unwrap_or(len);
    level::apply_fades(&mut ta.out, 0, len, ms(rate, t.fade_in_ms), ms(rate, t.fade_out_ms));
    let out_rate = export_rate_for(p, s);
    let mut audio = if out_rate != rate { resample_to(&ta.out, rate, out_rate) } else { ta.out };
    if p.export.channels > 0 {
        audio = match_channels(audio, p.export.channels as usize);
    }
    let gain = p.gain_for(t);
    let gr = level::apply(&mut audio, out_rate, &chain.level, gain.gain_db, ceiling_fix_db);
    Some((audio, gr, out_rate))
}

/// Resample whole buffers with the windowed-sinc interpolator.
pub fn resample_to(ch: &[Vec<f32>], from: u32, to: u32) -> Vec<Vec<f32>> {
    let ratio = from as f64 / to as f64;
    let n = ch.first().map(|c| ((c.len() as f64) / ratio).floor() as usize).unwrap_or(0);
    let it = crate::dsp::resample::Interp::new(ratio);
    ch.par_iter().map(|c| (0..n).map(|i| it.at(c, i as f64 * ratio)).collect()).collect()
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
    let chs = base.out.len();
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
        let mut seg: Vec<Vec<f32>> = match_channels(buf_src.clone(), chs);
        if !pass {
            level::apply_fades(&mut seg, ts - a, tl, ms(rate, t.fade_in_ms), ms(rate, t.fade_out_ms));
            let gain = p.gain_for(t);
            level::apply(&mut seg, rate, &chain.level, gain.gain_db, t.analysis.as_ref().map(|x| x.ceiling_fix_db).unwrap_or(0.0));
        }
        let lo = (ts - start).max(0) as usize;
        let hi = ((ts + tl as i64 - start).min(len as i64)).max(0) as usize;
        let rm = r.as_ref().and_then(|x| x.removed.clone()).map(|v| match_channels(v, removed.len()));
        for c in 0..chs {
            out[c][lo..hi].copy_from_slice(&seg[c][head + lo..head + hi]);
        }
        if let Some(rm) = rm {
            for c in 0..removed.len() {
                removed[c][lo..hi].copy_from_slice(&rm[c][head + lo..head + hi]);
            }
        }
    }
    Some(Block { pre, out, removed })
}
