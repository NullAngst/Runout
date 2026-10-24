//! Playback: renders the chain in blocks ahead of the playhead on worker threads
//! and feeds the audio device through a lock-free ring. The audio callback only
//! copies finished samples.

use crate::ring::{Ring, FRAME};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use runout_engine::audio::SourceAudio;
use runout_engine::project::{Placement, Project};
use runout_engine::render;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct RenderCtx {
    pub project: Project,
    pub audio: HashMap<u64, Arc<SourceAudio>>,
    pub layout: Vec<Placement>,
    pub gen: u64,
}

pub enum Cmd {
    Play(f64),
    Pause,
    Seek(f64),
    Update(Arc<RenderCtx>),
    Loop(Option<(f64, f64)>),
}

#[derive(Clone, Copy, Serialize)]
pub struct Meters {
    pub m: f32,
    pub s: f32,
    pub i: f32,
    pub tp: f32,
    pub lra: f32,
    pub pre_m: f32,
    pub pre_s: f32,
    pub pre_i: f32,
    pub pre_tp: f32,
    pub pre_lra: f32,
    pub peak_l: f32,
    pub peak_r: f32,
}

impl Default for Meters {
    fn default() -> Self {
        Meters { m: -70.0, s: -70.0, i: -70.0, tp: -70.0, lra: 0.0, pre_m: -70.0, pre_s: -70.0, pre_i: -70.0, pre_tp: -70.0, pre_lra: 0.0, peak_l: -120.0, peak_r: -120.0 }
    }
}

pub struct Shared {
    pub playing: AtomicBool,
    pos_bits: AtomicU64,
    /// 0 processed, 1 original, 2 removed
    pub monitor: AtomicU8,
    pub warn: Mutex<Option<String>>,
    peak: [AtomicU32; 2],
    pub meters: Mutex<Meters>,
    underrun: AtomicBool,
    grace: AtomicBool,
    consumed: AtomicU64,
    pub ring: Ring,
}

impl Shared {
    pub fn pos(&self) -> f64 {
        f64::from_bits(self.pos_bits.load(Ordering::Relaxed))
    }
    fn set_pos(&self, v: f64) {
        self.pos_bits.store(v.to_bits(), Ordering::Relaxed);
    }
    /// Sample peaks heard since the last call, linear.
    pub fn take_peaks(&self) -> (f32, f32) {
        let l = f32::from_bits(self.peak[0].swap(0, Ordering::Relaxed));
        let r = f32::from_bits(self.peak[1].swap(0, Ordering::Relaxed));
        (l, r)
    }
}

pub struct Player {
    pub shared: Arc<Shared>,
    tx: Sender<Cmd>,
}

impl Player {
    pub fn new() -> Self {
        let shared = Arc::new(Shared {
            playing: AtomicBool::new(false),
            pos_bits: AtomicU64::new(0f64.to_bits()),
            monitor: AtomicU8::new(0),
            warn: Mutex::new(None),
            peak: [AtomicU32::new(0), AtomicU32::new(0)],
            meters: Mutex::new(Meters::default()),
            underrun: AtomicBool::new(false),
            grace: AtomicBool::new(false),
            consumed: AtomicU64::new(0),
            ring: Ring::new(48000 / 2),
        });
        let (tx, rx) = channel();
        let s2 = shared.clone();
        std::thread::Builder::new().name("runout-feeder".into()).spawn(move || Feeder::new(s2, rx).run()).expect("feeder thread");
        Player { shared, tx }
    }

    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Key {
    gen: u64,
    seg: usize,
    start: i64,
    len: usize,
}

enum Item {
    Block { key: Key, global: f64, rate: u32 },
    Silence { frames: usize, global: f64, rate: u32 },
}

struct OutStream {
    stream: cpal::Stream,
    rate: u32,
}

struct Feeder {
    sh: Arc<Shared>,
    rx: Receiver<Cmd>,
    ctx: Option<Arc<RenderCtx>>,
    loop_range: Option<(f64, f64)>,
    queue: VecDeque<Item>,
    offset: usize,
    next_global: f64,
    first: bool,
    ready: HashMap<Key, Arc<render::Block>>,
    pending: HashMap<Key, Instant>,
    res_tx: Sender<(Key, Option<render::Block>)>,
    res_rx: Receiver<(Key, Option<render::Block>)>,
    stream: Option<OutStream>,
    marks: Vec<(u64, f64, u32)>,
    pushed: u64,
    meter_out: Option<ebur128::EbuR128>,
    meter_pre: Option<ebur128::EbuR128>,
    meter_rate: u32,
    timeline_end: f64,
    ended: bool,
    /// The last block scheduled ended exactly at the loop end, so the next one
    /// starts back at the loop start. Only playback that runs into the loop end
    /// wraps; a playhead placed past the loop plays straight on.
    wrap: bool,
}

/// How far ahead of the device we keep audio requested, in playback seconds.
const AHEAD_S: f64 = 3.0;
/// Hard cap on items scheduled per pass, so a bad loop range can never spin
/// the feeder thread.
const MAX_SCHEDULE_PER_PASS: usize = 512;

impl Feeder {
    fn new(sh: Arc<Shared>, rx: Receiver<Cmd>) -> Self {
        let (res_tx, res_rx) = channel();
        Feeder {
            sh,
            rx,
            ctx: None,
            loop_range: None,
            queue: VecDeque::new(),
            offset: 0,
            next_global: 0.0,
            first: true,
            ready: HashMap::new(),
            pending: HashMap::new(),
            res_tx,
            res_rx,
            stream: None,
            marks: Vec::new(),
            pushed: 0,
            meter_out: None,
            meter_pre: None,
            meter_rate: 0,
            timeline_end: 0.0,
            ended: false,
            wrap: false,
        }
    }

    fn run(mut self) {
        loop {
            match self.rx.recv_timeout(Duration::from_millis(4)) {
                Ok(c) => self.handle(c),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            while let Ok(c) = self.rx.try_recv() {
                self.handle(c);
            }
            while let Ok((k, b)) = self.res_rx.try_recv() {
                self.pending.remove(&k);
                if let Some(b) = b {
                    if self.ctx.as_ref().map(|c| c.gen == k.gen).unwrap_or(false) {
                        self.ready.insert(k, Arc::new(b));
                    }
                }
            }
            if self.sh.playing.load(Ordering::Relaxed) {
                self.schedule();
                self.feed();
                self.update_pos();
                if self.sh.underrun.swap(false, Ordering::Relaxed) && !self.ended {
                    *self.sh.warn.lock().unwrap() = Some("Rendering fell behind, so playback paused. Turn off a stage or two, or play a shorter loop.".into());
                    self.pause();
                }
                if self.ended && self.sh.ring.len() == 0 && self.queue.is_empty() {
                    self.pause();
                    self.sh.set_pos(self.timeline_end);
                }
            }
        }
    }

    fn handle(&mut self, c: Cmd) {
        match c {
            Cmd::Update(ctx) => {
                let changed = self.ctx.as_ref().map(|c| c.gen != ctx.gen).unwrap_or(true);
                self.timeline_end = ctx.layout.last().map(|p| p.start + p.duration).unwrap_or(0.0);
                self.ctx = Some(ctx);
                if changed {
                    self.ready.clear();
                    if self.sh.playing.load(Ordering::Relaxed) {
                        let p = self.sh.pos();
                        self.restart(p);
                    }
                }
            }
            Cmd::Play(t) => {
                *self.sh.warn.lock().unwrap() = None;
                self.meter_out = None;
                self.meter_pre = None;
                *self.sh.meters.lock().unwrap() = Meters::default();
                self.restart(t);
                self.sh.playing.store(true, Ordering::Relaxed);
                if let Some(s) = &self.stream {
                    let _ = s.stream.play();
                }
            }
            Cmd::Pause => self.pause(),
            Cmd::Seek(t) => {
                if self.sh.playing.load(Ordering::Relaxed) {
                    self.restart(t);
                } else {
                    self.sh.set_pos(t);
                }
            }
            Cmd::Loop(l) => {
                self.loop_range = l.filter(|(a, b)| b - a > 0.05);
                self.wrap = false;
            }
        }
    }

    fn pause(&mut self) {
        self.sh.playing.store(false, Ordering::Relaxed);
        if let Some(s) = &self.stream {
            let _ = s.stream.pause();
        }
        self.update_pos();
        self.sh.ring.clear_from_consumer();
        self.queue.clear();
    }

    /// Drop queued audio and start rendering again from `t`.
    fn restart(&mut self, t: f64) {
        let t = t.clamp(0.0, self.timeline_end.max(0.0));
        let running = self.stream.is_some() && self.sh.playing.load(Ordering::Relaxed);
        if running {
            self.sh.ring.flush.store(true, Ordering::Release);
            let t0 = Instant::now();
            while self.sh.ring.flush.load(Ordering::Acquire) && t0.elapsed() < Duration::from_millis(100) {
                std::thread::sleep(Duration::from_millis(1));
            }
            if self.sh.ring.flush.swap(false, Ordering::AcqRel) {
                // callback never ran; clearing ourselves is safe
                self.sh.ring.clear_from_consumer();
            }
        } else {
            self.sh.ring.clear_from_consumer();
        }
        self.sh.consumed.store(0, Ordering::Relaxed);
        self.pushed = 0;
        self.marks.clear();
        self.queue.clear();
        self.offset = 0;
        self.next_global = t;
        self.first = true;
        self.ended = false;
        self.wrap = false;
        self.sh.grace.store(true, Ordering::Relaxed);
        self.sh.set_pos(t);
    }

    fn layout_at(&self, t: f64) -> Option<(usize, &Placement)> {
        let ctx = self.ctx.as_ref()?;
        ctx.layout.iter().enumerate().find(|(_, p)| t >= p.start - 1e-9 && t < p.start + p.duration)
    }

    /// Seconds of audio queued ahead of what the device has played: what sits
    /// in the ring plus every block and silence still waiting in the queue.
    ///
    /// This is measured in playback time, not timeline position. A loop wraps
    /// the timeline position back to the loop start, so comparing timeline
    /// positions against the playhead never catches up once the playhead is
    /// within AHEAD_S of the loop end, or past it. That was the lockup.
    fn queued_s(&self) -> f64 {
        let out_rate = self.stream.as_ref().map(|s| s.rate).unwrap_or(48000) as f64;
        let mut s = self.sh.ring.len() as f64 / out_rate;
        for (i, it) in self.queue.iter().enumerate() {
            let done = if i == 0 { self.offset } else { 0 };
            s += match it {
                Item::Block { key, rate, .. } => key.len.saturating_sub(done) as f64 / *rate as f64,
                Item::Silence { frames, rate, .. } => frames.saturating_sub(done) as f64 / *rate as f64,
            };
        }
        s
    }

    /// Queue blocks so about AHEAD_S seconds of playback are requested ahead
    /// of the device.
    fn schedule(&mut self) {
        let Some(ctx) = self.ctx.clone() else { return };
        if ctx.layout.is_empty() {
            self.ended = true;
            return;
        }
        let mut budget = MAX_SCHEDULE_PER_PASS;
        while !self.ended && budget > 0 && self.queued_s() < AHEAD_S {
            budget -= 1;
            let mut g = self.next_global;
            if self.wrap {
                self.wrap = false;
                if let Some((a, _)) = self.loop_range {
                    g = a;
                }
            }
            if g >= self.timeline_end - 1e-6 {
                self.ended = true;
                break;
            }
            match self.layout_at(g).map(|(i, p)| (i, p.clone())) {
                Some((seg, p)) => {
                    let rate = p.rate;
                    let start = ((g - p.start) * rate as f64).round() as i64;
                    let seg_frames = (p.duration * rate as f64).floor() as i64;
                    let want = if self.first { rate as usize / 4 } else { rate as usize };
                    self.first = false;
                    let mut len = want.min((seg_frames - start).max(0) as usize);
                    if len == 0 {
                        self.next_global = p.start + p.duration + 1e-6;
                        continue;
                    }
                    if let Some((_, b)) = self.loop_range {
                        if g < b {
                            let to_b = ((b - g) * rate as f64).ceil().max(1.0) as usize;
                            if to_b <= len {
                                len = to_b;
                                self.wrap = true;
                            }
                        }
                    }
                    let key = Key { gen: ctx.gen, seg, start, len };
                    self.request(&ctx, key, p.source_id);
                    self.queue.push_back(Item::Block { key, global: g, rate });
                    self.next_global = g + len as f64 / rate as f64;
                }
                None => {
                    // gap between sources: silence until the next one starts
                    let next = ctx.layout.iter().find(|p| p.start > g).map(|p| p.start).unwrap_or(self.timeline_end);
                    let rate = self.stream.as_ref().map(|s| s.rate).unwrap_or(48000);
                    let mut frames = ((next - g) * rate as f64).ceil().max(1.0) as usize;
                    if let Some((_, b)) = self.loop_range {
                        if g < b {
                            let to_b = ((b - g) * rate as f64).ceil().max(1.0) as usize;
                            if to_b <= frames {
                                frames = to_b;
                                self.wrap = true;
                            }
                        }
                    }
                    self.queue.push_back(Item::Silence { frames, global: g, rate });
                    self.next_global = g + frames as f64 / rate as f64;
                }
            }
        }
    }

    fn request(&mut self, ctx: &Arc<RenderCtx>, key: Key, source_id: u64) {
        if self.ready.contains_key(&key) || self.pending.contains_key(&key) {
            return;
        }
        self.pending.insert(key, Instant::now());
        let ctx = ctx.clone();
        let tx = self.res_tx.clone();
        rayon::spawn(move || {
            let b = ctx.audio.get(&source_id).and_then(|a| render::render_play_block(&ctx.project, source_id, a, key.start, key.len));
            let _ = tx.send((key, b));
        });
    }

    fn ensure_stream(&mut self, rate: u32) -> bool {
        if let Some(s) = &self.stream {
            if s.rate == rate {
                return true;
            }
            if self.sh.ring.len() > 0 {
                return false; // let the old rate drain first
            }
        }
        self.stream = None;
        match open_output(rate, self.sh.clone()) {
            Ok(s) => {
                let _ = s.stream.play();
                self.stream = Some(s);
                true
            }
            Err(e) => {
                *self.sh.warn.lock().unwrap() = Some(format!("Could not open the audio output: {e}"));
                self.sh.playing.store(false, Ordering::Relaxed);
                false
            }
        }
    }

    fn feed(&mut self) {
        loop {
            let Some(front) = self.queue.front() else { return };
            let (rate, global) = match front {
                Item::Block { rate, global, .. } => (*rate, *global),
                Item::Silence { rate, global, .. } => (*rate, *global),
            };
            if !self.ensure_stream(rate) {
                return;
            }
            let out_rate = self.stream.as_ref().map(|s| s.rate).unwrap_or(rate);
            let free = self.sh.ring.free();
            if free == 0 {
                return;
            }
            if self.offset == 0 {
                self.marks.push((self.pushed, global, out_rate));
            }
            match self.queue.front().unwrap() {
                Item::Silence { frames, .. } => {
                    let n = (*frames - self.offset).min(free);
                    for _ in 0..n {
                        self.sh.ring.push(&[0.0; FRAME]);
                    }
                    self.offset += n;
                    self.pushed += n as u64;
                    if self.offset >= *frames {
                        self.queue.pop_front();
                        self.offset = 0;
                    }
                }
                Item::Block { key, .. } => {
                    let key = *key;
                    let Some(b) = self.ready.get(&key).cloned() else {
                        return; // still rendering
                    };
                    self.sh.grace.store(false, Ordering::Relaxed);
                    let n = (key.len - self.offset).min(free);
                    let st = b.out.len() > 1;
                    let pick = |v: &Vec<Vec<f32>>, c: usize, i: usize| if st { v[c][i] } else { v[0][i] };
                    for i in self.offset..self.offset + n {
                        let f = [
                            pick(&b.pre, 0, i),
                            pick(&b.pre, 1, i),
                            pick(&b.out, 0, i),
                            pick(&b.out, 1, i),
                            pick(&b.removed, 0, i),
                            pick(&b.removed, 1, i),
                        ];
                        self.sh.ring.push(&f);
                    }
                    self.meter(&b, self.offset, self.offset + n, key_rate(&self.ctx, key.seg));
                    self.offset += n;
                    self.pushed += n as u64;
                    if self.offset >= key.len {
                        self.queue.pop_front();
                        self.offset = 0;
                        // keep blocks inside the loop for the next pass
                        let keep = self.loop_range.is_some();
                        if !keep {
                            self.ready.remove(&key);
                        }
                        if self.ready.len() > 64 {
                            self.ready.clear();
                        }
                    }
                }
            }
        }
    }

    fn meter(&mut self, b: &render::Block, from: usize, to: usize, rate: u32) {
        let chs = b.out.len() as u32;
        if self.meter_out.is_none() || self.meter_rate != rate {
            self.meter_out = runout_engine::dsp::loudness::meter(chs, rate);
            self.meter_pre = runout_engine::dsp::loudness::meter(chs, rate);
            self.meter_rate = rate;
        }
        if let (Some(mo), Some(mp)) = (self.meter_out.as_mut(), self.meter_pre.as_mut()) {
            runout_engine::dsp::loudness::feed(mo, &b.out, from, to);
            runout_engine::dsp::loudness::feed(mp, &b.pre, from, to);
            let f = |v: Result<f64, ebur128::Error>| v.ok().filter(|x| x.is_finite()).map(|x| x as f32).unwrap_or(-70.0).max(-70.0);
            let tp = |m: &ebur128::EbuR128| {
                let mut p = 0.0f64;
                for c in 0..m.channels() {
                    p = p.max(m.true_peak(c).unwrap_or(0.0));
                }
                if p > 0.0 {
                    (20.0 * p.log10()) as f32
                } else {
                    -70.0
                }
            };
            let mut mt = self.sh.meters.lock().unwrap();
            mt.m = f(mo.loudness_momentary());
            mt.s = f(mo.loudness_shortterm());
            mt.i = f(mo.loudness_global());
            mt.lra = mo.loudness_range().unwrap_or(0.0) as f32;
            mt.tp = tp(mo);
            mt.pre_m = f(mp.loudness_momentary());
            mt.pre_s = f(mp.loudness_shortterm());
            mt.pre_i = f(mp.loudness_global());
            mt.pre_lra = mp.loudness_range().unwrap_or(0.0) as f32;
            mt.pre_tp = tp(mp);
        }
    }

    fn update_pos(&mut self) {
        let consumed = self.sh.consumed.load(Ordering::Relaxed);
        if let Some(&(at, g, rate)) = self.marks.iter().rev().find(|m| m.0 <= consumed) {
            self.sh.set_pos(g + (consumed - at) as f64 / rate as f64);
        }
        // forget marks that are fully played
        while self.marks.len() > 2 && self.marks[1].0 <= consumed {
            self.marks.remove(0);
        }
    }
}

fn key_rate(ctx: &Option<Arc<RenderCtx>>, seg: usize) -> u32 {
    ctx.as_ref().and_then(|c| c.layout.get(seg)).map(|p| p.rate).unwrap_or(48000)
}

fn pick_config(dev: &cpal::Device, rate: u32) -> Result<(cpal::StreamConfig, cpal::SampleFormat), String> {
    let mut best: Option<cpal::SupportedStreamConfig> = None;
    if let Ok(cfgs) = dev.supported_output_configs() {
        for c in cfgs {
            let fmt_ok = matches!(c.sample_format(), cpal::SampleFormat::F32 | cpal::SampleFormat::I16 | cpal::SampleFormat::I32);
            if !fmt_ok || c.channels() < 1 {
                continue;
            }
            if let Some(s) = c.try_with_sample_rate(rate) {
                let score = |s: &cpal::SupportedStreamConfig| (s.channels() == 2) as i32 * 4 + (s.sample_format() == cpal::SampleFormat::F32) as i32 * 2;
                if best.as_ref().map(|b| score(&s) > score(b)).unwrap_or(true) {
                    best = Some(s);
                }
            }
        }
    }
    let s = match best {
        Some(s) => s,
        None => {
            return Err(match dev.default_output_config() {
                Ok(d) => format!("the output device runs at {} Hz and will not take {rate} Hz. Use the PipeWire or PulseAudio default device, which converts", d.sample_rate()),
                Err(e) => format!("the output device has no usable format ({e})"),
            })
        }
    };
    let fmt = s.sample_format();
    let mut cfg: cpal::StreamConfig = s.into();
    cfg.buffer_size = cpal::BufferSize::Default;
    Ok((cfg, fmt))
}

fn open_output(rate: u32, sh: Arc<Shared>) -> Result<OutStream, String> {
    let host = cpal::default_host();
    let dev = host.default_output_device().ok_or("no audio output device")?;
    let (cfg, fmt) = pick_config(&dev, rate)?;
    let chans = cfg.channels as usize;
    let err = |e: cpal::Error| eprintln!("audio output: {e}");
    let fill = move |out: &mut dyn FnMut(usize, f32), frames: usize, sh: &Shared| {
        if sh.ring.flush.load(Ordering::Acquire) {
            sh.ring.clear_from_consumer();
            sh.ring.flush.store(false, Ordering::Release);
        }
        let mon = sh.monitor.load(Ordering::Relaxed) as usize;
        let mut pl = 0.0f32;
        let mut pr = 0.0f32;
        let mut got = 0u64;
        let mut starved = false;
        for i in 0..frames {
            let (l, r) = match sh.ring.pop() {
                Some(f) => {
                    got += 1;
                    let o = match mon {
                        1 => 0,
                        2 => 4,
                        _ => 2,
                    };
                    (f[o], f[o + 1])
                }
                None => {
                    starved = true;
                    (0.0, 0.0)
                }
            };
            pl = pl.max(l.abs());
            pr = pr.max(r.abs());
            if chans == 1 {
                out(i, (l + r) * 0.5);
            } else {
                out(i * chans, l);
                out(i * chans + 1, r);
                for c in 2..chans {
                    out(i * chans + c, 0.0);
                }
            }
        }
        sh.consumed.fetch_add(got, Ordering::Relaxed);
        if starved && sh.playing.load(Ordering::Relaxed) && !sh.grace.load(Ordering::Relaxed) && got > 0 {
            sh.underrun.store(true, Ordering::Relaxed);
        }
        let upd = |a: &AtomicU32, v: f32| {
            let cur = f32::from_bits(a.load(Ordering::Relaxed));
            if v > cur {
                a.store(v.to_bits(), Ordering::Relaxed);
            }
        };
        upd(&sh.peak[0], pl);
        upd(&sh.peak[1], pr);
    };
    let stream = match fmt {
        cpal::SampleFormat::F32 => {
            let sh = sh.clone();
            dev.build_output_stream(
                cfg.clone(),
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let frames = data.len() / chans;
                    fill(&mut |i, v| data[i] = v, frames, &sh);
                },
                err,
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let sh = sh.clone();
            dev.build_output_stream(
                cfg.clone(),
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    let frames = data.len() / chans;
                    fill(&mut |i, v| data[i] = (v.clamp(-1.0, 1.0) * 32767.0) as i16, frames, &sh);
                },
                err,
                None,
            )
        }
        _ => {
            let sh = sh.clone();
            dev.build_output_stream(
                cfg.clone(),
                move |data: &mut [i32], _: &cpal::OutputCallbackInfo| {
                    let frames = data.len() / chans;
                    fill(&mut |i, v| data[i] = (v.clamp(-1.0, 1.0) as f64 * 2147483647.0) as i32, frames, &sh);
                },
                err,
                None,
            )
        }
    }
    .map_err(|e| e.to_string())?;
    Ok(OutStream { stream, rate })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A feeder with one 10-minute 48 kHz source on the timeline and no audio
    /// device. Blocks are requested but never rendered (no audio in the
    /// context), which is all the scheduler needs.
    fn feeder() -> Feeder {
        let sh = Arc::new(Shared {
            playing: AtomicBool::new(false),
            pos_bits: AtomicU64::new(0f64.to_bits()),
            monitor: AtomicU8::new(0),
            warn: Mutex::new(None),
            peak: [AtomicU32::new(0), AtomicU32::new(0)],
            meters: Mutex::new(Meters::default()),
            underrun: AtomicBool::new(false),
            grace: AtomicBool::new(false),
            consumed: AtomicU64::new(0),
            ring: Ring::new(48000 / 2),
        });
        let (_tx, rx) = channel();
        let mut f = Feeder::new(sh, rx);
        f.handle(Cmd::Update(Arc::new(RenderCtx {
            project: Project::default(),
            audio: HashMap::new(),
            layout: vec![Placement { source_id: 1, start: 0.0, duration: 600.0, ratio: 1.0, rate: 48000 }],
            gen: 1,
        })));
        f
    }

    fn globals(f: &Feeder) -> Vec<f64> {
        f.queue
            .iter()
            .map(|it| match it {
                Item::Block { global, .. } | Item::Silence { global, .. } => *global,
            })
            .collect()
    }

    #[test]
    fn seek_past_loop_end_plays_on_and_returns() {
        let mut f = feeder();
        f.handle(Cmd::Loop(Some((100.0, 120.0))));
        f.restart(150.0);
        f.schedule();
        let g = globals(&f);
        assert!(!g.is_empty());
        assert!(g.iter().all(|&t| t >= 150.0), "jumped back into the loop: {g:?}");
        assert!(f.queued_s() >= AHEAD_S && f.queued_s() < AHEAD_S + 1.5);
    }

    #[test]
    fn near_loop_end_wraps_and_returns() {
        let mut f = feeder();
        f.handle(Cmd::Loop(Some((100.0, 120.0))));
        f.restart(118.5);
        f.schedule();
        let g = globals(&f);
        assert!(g.iter().any(|&t| (t - 100.0).abs() < 1e-9), "never wrapped: {g:?}");
        assert!(f.queued_s() < AHEAD_S + 1.5);
    }

    #[test]
    fn seek_before_loop_runs_into_it() {
        let mut f = feeder();
        f.handle(Cmd::Loop(Some((100.0, 120.0))));
        f.restart(98.0);
        f.schedule();
        let g = globals(&f);
        assert!(g.windows(2).all(|w| w[1] > w[0]), "should not wrap yet: {g:?}");
    }

    #[test]
    fn tiny_loop_terminates() {
        let mut f = feeder();
        f.handle(Cmd::Loop(Some((10.0, 10.06))));
        f.restart(10.0);
        f.schedule();
        assert!(f.queue.len() <= MAX_SCHEDULE_PER_PASS);
        assert!(f.queued_s() >= AHEAD_S);
    }
}
