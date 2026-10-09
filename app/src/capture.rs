//! Capture from the turntable (or any input) straight into a WAV in the
//! project folder. The header is rewritten every second, so a crash loses
//! seconds, not a side.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use runout_engine::auto::RunoutDetector;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Serialize, Clone)]
pub struct InputDevice {
    pub id: String,
    pub name: String,
    pub default: bool,
}

pub fn list_inputs() -> Vec<InputDevice> {
    let host = cpal::default_host();
    let def = host.default_input_device().and_then(|d| d.id().ok()).map(|i| i.to_string());
    let mut out = Vec::new();
    if let Ok(devs) = host.input_devices() {
        for d in devs {
            let Ok(id) = d.id() else { continue };
            let id = id.to_string();
            let name = d.description().map(|x| x.name().to_string()).unwrap_or_else(|_| id.clone());
            out.push(InputDevice { default: Some(&id) == def.as_ref(), id, name });
        }
    }
    out
}

/// Remembered device, else something that looks like a USB turntable, else the default.
pub fn pick_default(devs: &[InputDevice], remembered: Option<&str>) -> Option<String> {
    if let Some(r) = remembered {
        if devs.iter().any(|d| d.id == r) {
            return Some(r.to_string());
        }
    }
    let usb = devs.iter().find(|d| {
        let n = d.name.to_lowercase();
        n.contains("lp120") || n.contains("turntable") || n.contains("usb audio codec") || n.contains("usb")
    });
    usb.or_else(|| devs.iter().find(|d| d.default)).or(devs.first()).map(|d| d.id.clone())
}

pub struct Level {
    peak: [AtomicU32; 2],
    hold: [AtomicU32; 2],
    pub clipped: AtomicBool,
    pub frames: AtomicU64,
    pub rate: AtomicU32,
    pub auto_stopped: AtomicBool,
}

#[derive(Serialize, Clone, Copy)]
pub struct LevelReading {
    pub l: f32,
    pub r: f32,
    pub hold_l: f32,
    pub hold_r: f32,
    pub clipped: bool,
    pub seconds: f64,
}

fn db(v: f32) -> f32 {
    if v <= 1e-6 {
        -120.0
    } else {
        20.0 * v.log10()
    }
}

impl Level {
    pub fn read(&self) -> LevelReading {
        let l = f32::from_bits(self.peak[0].swap(0, Ordering::Relaxed));
        let r = f32::from_bits(self.peak[1].swap(0, Ordering::Relaxed));
        let rate = self.rate.load(Ordering::Relaxed).max(1) as f64;
        LevelReading {
            l: db(l),
            r: db(r),
            hold_l: db(f32::from_bits(self.hold[0].load(Ordering::Relaxed))),
            hold_r: db(f32::from_bits(self.hold[1].load(Ordering::Relaxed))),
            clipped: self.clipped.load(Ordering::Relaxed),
            seconds: self.frames.load(Ordering::Relaxed) as f64 / rate,
        }
    }
}

pub struct Capture {
    pub stop: Arc<AtomicBool>,
    pub level: Arc<Level>,
    pub thread: Option<JoinHandle<Result<Done, String>>>,
    pub path: Option<PathBuf>,
    pub name: String,
}

pub struct Done {
    pub path: Option<PathBuf>,
    pub frames: u64,
    pub rate: u32,
    pub peak_db: f32,
    pub auto_stopped: bool,
    pub clipped: bool,
}

pub struct Opts {
    pub device: Option<String>,
    /// None = level check only, nothing written.
    pub path: Option<PathBuf>,
    pub auto_stop_s: Option<f32>,
    pub max_s: Option<f32>,
}

fn find_device(id: Option<&str>) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    if let Some(id) = id {
        if let Ok(devs) = host.input_devices() {
            for d in devs {
                if d.id().map(|x| x.to_string()).ok().as_deref() == Some(id) {
                    return Ok(d);
                }
            }
        }
    }
    host.default_input_device().ok_or_else(|| "no input device found".into())
}

/// 16-bit at 48 kHz if the device offers it (the AT-LP120XUSB does), else the
/// closest thing it has.
fn pick_config(dev: &cpal::Device) -> Result<(cpal::StreamConfig, cpal::SampleFormat), String> {
    let mut best: Option<(i32, cpal::SupportedStreamConfig)> = None;
    for c in dev.supported_input_configs().map_err(|e| e.to_string())? {
        for rate in [48000u32, 44100] {
            let Some(s) = c.try_with_sample_rate(rate) else { continue };
            let fmt = s.sample_format();
            if !matches!(fmt, cpal::SampleFormat::I16 | cpal::SampleFormat::F32 | cpal::SampleFormat::I32) {
                continue;
            }
            let score = (rate == 48000) as i32 * 8 + (s.channels() == 2) as i32 * 4 + (fmt == cpal::SampleFormat::I16) as i32 * 2 + (fmt == cpal::SampleFormat::F32) as i32;
            if best.as_ref().map(|b| score > b.0).unwrap_or(true) {
                best = Some((score, s));
            }
        }
    }
    let s = match best {
        Some((_, s)) => s,
        None => dev.default_input_config().map_err(|e| e.to_string())?,
    };
    let fmt = s.sample_format();
    Ok((s.into(), fmt))
}

pub fn start(opts: Opts, name: String) -> Result<Capture, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let level = Arc::new(Level {
        peak: [AtomicU32::new(0), AtomicU32::new(0)],
        hold: [AtomicU32::new(0), AtomicU32::new(0)],
        clipped: AtomicBool::new(false),
        frames: AtomicU64::new(0),
        rate: AtomicU32::new(48000),
        auto_stopped: AtomicBool::new(false),
    });
    let (ready_tx, ready_rx) = channel::<Result<(), String>>();
    let (st, lv) = (stop.clone(), level.clone());
    let path = opts.path.clone();
    let thread = std::thread::Builder::new()
        .name("runout-capture".into())
        .spawn(move || run(opts, st, lv, ready_tx))
        .map_err(|e| e.to_string())?;
    match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(Capture { stop, level, thread: Some(thread), path, name }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            stop.store(true, Ordering::Relaxed);
            Err("the input device did not start".into())
        }
    }
}

fn run(opts: Opts, stop: Arc<AtomicBool>, level: Arc<Level>, ready: std::sync::mpsc::Sender<Result<(), String>>) -> Result<Done, String> {
    let fail = |e: String, ready: &std::sync::mpsc::Sender<Result<(), String>>| {
        let _ = ready.send(Err(e.clone()));
        Err(e)
    };
    let dev = match find_device(opts.device.as_deref()) {
        Ok(d) => d,
        Err(e) => return fail(e, &ready),
    };
    let (cfg, fmt) = match pick_config(&dev) {
        Ok(c) => c,
        Err(e) => return fail(e, &ready),
    };
    let chans = cfg.channels as usize;
    let rate = cfg.sample_rate;
    level.rate.store(rate, Ordering::Relaxed);
    let (tx, rx) = channel::<Vec<i16>>();
    let lv = level.clone();
    let push = move |v: Vec<i16>| {
        let mut p = [0i32; 2];
        let mut clip = false;
        for (i, s) in v.iter().enumerate() {
            let c = (i % chans).min(1);
            let a = (*s as i32).abs();
            p[c] = p[c].max(a);
            if a >= 32767 {
                clip = true;
            }
        }
        if chans == 1 {
            p[1] = p[0];
        }
        for c in 0..2 {
            let v = p[c] as f32 / 32768.0;
            if v > f32::from_bits(lv.peak[c].load(Ordering::Relaxed)) {
                lv.peak[c].store(v.to_bits(), Ordering::Relaxed);
            }
            if v > f32::from_bits(lv.hold[c].load(Ordering::Relaxed)) {
                lv.hold[c].store(v.to_bits(), Ordering::Relaxed);
            }
        }
        if clip {
            lv.clipped.store(true, Ordering::Relaxed);
        }
        let _ = tx.send(v);
    };
    let err = |e: cpal::Error| eprintln!("capture: {e}");
    let stream = match fmt {
        cpal::SampleFormat::I16 => dev.build_input_stream(cfg.clone(), move |d: &[i16], _: &cpal::InputCallbackInfo| push(d.to_vec()), err, None),
        cpal::SampleFormat::I32 => dev.build_input_stream(cfg.clone(), move |d: &[i32], _: &cpal::InputCallbackInfo| push(d.iter().map(|s| (s >> 16) as i16).collect()), err, None),
        _ => dev.build_input_stream(
            cfg.clone(),
            move |d: &[f32], _: &cpal::InputCallbackInfo| push(d.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0).round() as i16).collect()),
            err,
            None,
        ),
    };
    let stream = match stream {
        Ok(s) => s,
        Err(e) => return fail(e.to_string(), &ready),
    };
    if let Err(e) = stream.play() {
        return fail(e.to_string(), &ready);
    }

    let mut writer = match &opts.path {
        Some(p) => {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let spec = hound::WavSpec { channels: chans as u16, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
            match hound::WavWriter::create(p, spec) {
                Ok(w) => Some(w),
                Err(e) => return fail(e.to_string(), &ready),
            }
        }
        None => None,
    };
    let _ = ready.send(Ok(()));

    let mut det = opts.auto_stop_s.map(|s| RunoutDetector::new(rate, s));
    let mut last_flush = Instant::now();
    let mut frames = 0u64;
    let mut peak = 0i32;
    let mut auto_stopped = false;
    let max_frames = opts.max_s.map(|s| (s * rate as f32) as u64);
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(v) => {
                if let Some(w) = writer.as_mut() {
                    for s in &v {
                        if w.write_sample(*s).is_err() {
                            stop.store(true, Ordering::Relaxed);
                            break;
                        }
                    }
                }
                peak = peak.max(v.iter().map(|s| (*s as i32).abs()).max().unwrap_or(0));
                let n = (v.len() / chans) as u64;
                frames += n;
                level.frames.store(frames, Ordering::Relaxed);
                if let Some(d) = det.as_mut() {
                    let mono: Vec<f32> = v.chunks(chans).map(|c| c.iter().map(|s| *s as f32).sum::<f32>() / (chans as f32 * 32768.0)).collect();
                    if d.feed(&mono) {
                        auto_stopped = true;
                        level.auto_stopped.store(true, Ordering::Relaxed);
                        break;
                    }
                }
                if max_frames.map(|m| frames >= m).unwrap_or(false) {
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if last_flush.elapsed() >= Duration::from_secs(1) {
            if let Some(w) = writer.as_mut() {
                let _ = w.flush();
            }
            last_flush = Instant::now();
        }
    }
    drop(stream);
    // drain what the callback already handed over
    while let Ok(v) = rx.try_recv() {
        if let Some(w) = writer.as_mut() {
            for s in &v {
                let _ = w.write_sample(*s);
            }
        }
        frames += (v.len() / chans) as u64;
    }
    if let Some(w) = writer {
        w.finalize().map_err(|e| e.to_string())?;
    }
    Ok(Done {
        path: opts.path,
        frames,
        rate,
        peak_db: db(peak as f32 / 32768.0),
        auto_stopped,
        clipped: level.clipped.load(Ordering::Relaxed),
    })
}
