//! Capture into a WAV in the project folder (RF64 past 4 GB). The header is
//! rewritten every second, so a crash loses seconds, not a side.
//!
//! Two paths, picked by the medium:
//!
//! - Analog: any input cpal sees (PipeWire, PulseAudio, ALSA). Rate and depth
//!   come from the medium: vinyl takes 48 kHz / 16-bit as it always has; tape
//!   and video take 96 kHz / 24-bit when the device offers it.
//! - S/PDIF: an ALSA hw: device opened directly, bypassing PipeWire, which may
//!   resample or apply volume. Captured at 24 bits; a 16-bit source over
//!   S/PDIF leaves the low 8 bits at zero, so any set bit means something in
//!   the path is processing the audio. When the check holds for the whole
//!   capture, the raw is stored as 16-bit.

use crate::iec958;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use runout_engine::audio::WavWriter;
use runout_engine::auto::{stop_detector, StopDetector};
use runout_engine::media::{CapturePath, CaptureSpec};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Serialize, Clone)]
pub struct InputDevice {
    pub id: String,
    pub name: String,
    pub default: bool,
    /// An ALSA hw: device for the S/PDIF path.
    pub digital: bool,
}

/// Inputs for a capture path: everything cpal sees for analog, ALSA hw:
/// capture devices for S/PDIF.
pub fn list_inputs(path: CapturePath) -> Vec<InputDevice> {
    match path {
        CapturePath::Analog => list_analog(),
        CapturePath::Spdif => list_hw(),
    }
}

fn list_analog() -> Vec<InputDevice> {
    let host = cpal::default_host();
    let def = host.default_input_device().and_then(|d| d.id().ok()).map(|i| i.to_string());
    let mut out = Vec::new();
    if let Ok(devs) = host.input_devices() {
        for d in devs {
            let Ok(id) = d.id() else { continue };
            let id = id.to_string();
            let name = d.description().map(|x| x.name().to_string()).unwrap_or_else(|_| id.clone());
            out.push(InputDevice { default: Some(&id) == def.as_ref(), id, name, digital: false });
        }
    }
    out
}

fn looks_digital(n: &str) -> bool {
    let n = n.to_lowercase();
    ["iec958", "s/pdif", "spdif", "digital", "optical", "coax", "toslink", "aes"].iter().any(|k| n.contains(k))
}

fn list_hw() -> Vec<InputDevice> {
    use alsa::ctl::Ctl;
    use alsa::Direction;
    let mut out = Vec::new();
    for card in alsa::card::Iter::new().flatten() {
        let idx = card.get_index();
        let cname = card.get_name().unwrap_or_else(|_| format!("card {idx}"));
        let Ok(ctl) = Ctl::from_card(&card, false) else { continue };
        let cid = ctl.card_info().ok().and_then(|i| i.get_id().ok().map(|s| s.to_string())).unwrap_or_else(|| idx.to_string());
        for dev in alsa::ctl::DeviceIter::new(&ctl) {
            let Ok(info) = ctl.pcm_info(dev as u32, 0, Direction::Capture) else { continue };
            let pname = info.get_name().unwrap_or("").to_string();
            let name = format!("{cname}: {pname} (hw:{idx},{dev})");
            out.push(InputDevice { id: format!("hw:CARD={cid},DEV={dev}"), default: looks_digital(&name), name, digital: true });
        }
    }
    out
}

/// Remembered device, else something that looks right for the path, else the default.
pub fn pick_default(devs: &[InputDevice], remembered: Option<&str>) -> Option<String> {
    if let Some(r) = remembered {
        if devs.iter().any(|d| d.id == r) {
            return Some(r.to_string());
        }
    }
    let likely = devs.iter().find(|d| {
        let n = d.name.to_lowercase();
        if d.digital {
            looks_digital(&n)
        } else {
            n.contains("lp120") || n.contains("turntable") || n.contains("usb audio codec") || n.contains("usb")
        }
    });
    likely.or_else(|| devs.iter().find(|d| d.default)).or(devs.first()).map(|d| d.id.clone())
}

pub struct Level {
    peak: [AtomicU32; 2],
    hold: [AtomicU32; 2],
    pub clipped: AtomicBool,
    pub frames: AtomicU64,
    pub rate: AtomicU32,
    pub auto_stopped: AtomicBool,
    /// S/PDIF: 0 not checked, 1 bit-perfect so far, 2 low bits seen.
    pub bit_perfect: AtomicU8,
    pub low_bits: AtomicU64,
    pub note: Mutex<Option<String>>,
}

#[derive(Serialize, Clone)]
pub struct LevelReading {
    pub l: f32,
    pub r: f32,
    pub hold_l: f32,
    pub hold_r: f32,
    pub clipped: bool,
    pub seconds: f64,
    /// S/PDIF only: Some(true) while every sample is a clean 16-bit value.
    pub bit_perfect: Option<bool>,
    pub note: Option<String>,
}

fn db(v: f32) -> f32 {
    if v <= 1e-6 {
        -120.0
    } else {
        20.0 * v.log10()
    }
}

impl Level {
    fn new() -> Self {
        Level {
            peak: [AtomicU32::new(0), AtomicU32::new(0)],
            hold: [AtomicU32::new(0), AtomicU32::new(0)],
            clipped: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            rate: AtomicU32::new(48000),
            auto_stopped: AtomicBool::new(false),
            bit_perfect: AtomicU8::new(0),
            low_bits: AtomicU64::new(0),
            note: Mutex::new(None),
        }
    }

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
            bit_perfect: match self.bit_perfect.load(Ordering::Relaxed) {
                1 => Some(true),
                2 => Some(false),
                _ => None,
            },
            note: self.note.lock().unwrap().clone(),
        }
    }

    /// Peak and clip bookkeeping for a block of interleaved samples.
    fn meter(&self, v: &[i32], chans: usize, full: i32) {
        let mut p = [0i32; 2];
        let mut clip = false;
        for (i, s) in v.iter().enumerate() {
            let c = (i % chans).min(1);
            let a = s.saturating_abs();
            p[c] = p[c].max(a);
            if a >= full - 1 {
                clip = true;
            }
        }
        if chans == 1 {
            p[1] = p[0];
        }
        for c in 0..2 {
            let x = p[c] as f32 / full as f32;
            if x > f32::from_bits(self.peak[c].load(Ordering::Relaxed)) {
                self.peak[c].store(x.to_bits(), Ordering::Relaxed);
            }
            if x > f32::from_bits(self.hold[c].load(Ordering::Relaxed)) {
                self.hold[c].store(x.to_bits(), Ordering::Relaxed);
            }
        }
        if clip {
            self.clipped.store(true, Ordering::Relaxed);
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
    pub bits: u16,
    pub peak_db: f32,
    pub auto_stopped: bool,
    pub clipped: bool,
    /// S/PDIF: whether every sample was a clean 16-bit value.
    pub bit_perfect: Option<bool>,
    /// The card's IEC958 status said the audio is pre-emphasized.
    pub emphasis: Option<bool>,
    pub notes: Vec<String>,
}

pub struct Opts {
    pub device: Option<String>,
    /// None = level check only, nothing written.
    pub path: Option<PathBuf>,
    pub auto_stop_s: Option<f32>,
    pub max_s: Option<f32>,
    pub spec: CaptureSpec,
    /// Speeds the run-out detector listens for.
    pub rpms: Vec<f32>,
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

/// The medium's rate and depth if the device offers them, else its fallback
/// rate, else the closest thing it has. For vinyl (48 kHz / 16-bit) this
/// picks exactly what Runout always picked.
fn pick_config(dev: &cpal::Device, spec: &CaptureSpec) -> Result<(cpal::StreamConfig, cpal::SampleFormat), String> {
    let mut rates: Vec<u32> = vec![spec.rate, spec.fallback_rate, 48000, 44100].into_iter().filter(|r| *r > 0).collect();
    rates.dedup();
    let want24 = spec.bits > 16;
    let mut best: Option<(i32, cpal::SupportedStreamConfig)> = None;
    for c in dev.supported_input_configs().map_err(|e| e.to_string())? {
        for (ri, &rate) in rates.iter().enumerate() {
            let Some(s) = c.try_with_sample_rate(rate) else { continue };
            let fmt = s.sample_format();
            if !matches!(fmt, cpal::SampleFormat::I16 | cpal::SampleFormat::F32 | cpal::SampleFormat::I32) {
                continue;
            }
            let rate_score = match ri {
                0 => 8,
                1 => 6,
                _ => 0,
            };
            let fmt_score = if want24 {
                (fmt == cpal::SampleFormat::I32) as i32 * 2 + (fmt == cpal::SampleFormat::F32) as i32
            } else {
                (fmt == cpal::SampleFormat::I16) as i32 * 2 + (fmt == cpal::SampleFormat::F32) as i32
            };
            let score = rate_score + (s.channels() == 2) as i32 * 4 + fmt_score;
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
    let level = Arc::new(Level::new());
    let (ready_tx, ready_rx) = channel::<Result<(), String>>();
    let (st, lv) = (stop.clone(), level.clone());
    let path = opts.path.clone();
    let digital = opts.spec.path == CapturePath::Spdif;
    let thread = std::thread::Builder::new()
        .name("runout-capture".into())
        .spawn(move || if digital { run_digital(opts, st, lv, ready_tx) } else { run_analog(opts, st, lv, ready_tx) })
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

fn detector(opts: &Opts, rate: u32) -> Option<Box<dyn StopDetector>> {
    opts.auto_stop_s.and_then(|s| stop_detector(opts.spec.stop, rate, s, &opts.rpms))
}

fn open_writer(opts: &Opts, rate: u32, chans: usize, bits: u16) -> Result<Option<WavWriter>, String> {
    match &opts.path {
        Some(p) => {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            WavWriter::create(p, rate, chans as u16, bits).map(Some).map_err(|e| e.to_string())
        }
        None => Ok(None),
    }
}

fn run_analog(opts: Opts, stop: Arc<AtomicBool>, level: Arc<Level>, ready: std::sync::mpsc::Sender<Result<(), String>>) -> Result<Done, String> {
    let fail = |e: String, ready: &std::sync::mpsc::Sender<Result<(), String>>| {
        let _ = ready.send(Err(e.clone()));
        Err(e)
    };
    let dev = match find_device(opts.device.as_deref()) {
        Ok(d) => d,
        Err(e) => return fail(e, &ready),
    };
    let (cfg, fmt) = match pick_config(&dev, &opts.spec) {
        Ok(c) => c,
        Err(e) => return fail(e, &ready),
    };
    let chans = cfg.channels as usize;
    let rate = cfg.sample_rate;
    let bits: u16 = if opts.spec.bits > 16 { 24 } else { 16 };
    let full: i32 = if bits == 24 { 8_388_608 } else { 32768 };
    level.rate.store(rate, Ordering::Relaxed);
    let (tx, rx) = channel::<Vec<i32>>();
    let lv = level.clone();
    let push = move |v: Vec<i32>| {
        lv.meter(&v, chans, full);
        let _ = tx.send(v);
    };
    let err = |e: cpal::Error| eprintln!("capture: {e}");
    let stream = match (fmt, bits) {
        (cpal::SampleFormat::I16, 16) => dev.build_input_stream(cfg.clone(), move |d: &[i16], _: &cpal::InputCallbackInfo| push(d.iter().map(|s| *s as i32).collect()), err, None),
        (cpal::SampleFormat::I16, _) => dev.build_input_stream(cfg.clone(), move |d: &[i16], _: &cpal::InputCallbackInfo| push(d.iter().map(|s| (*s as i32) << 8).collect()), err, None),
        (cpal::SampleFormat::I32, 16) => dev.build_input_stream(cfg.clone(), move |d: &[i32], _: &cpal::InputCallbackInfo| push(d.iter().map(|s| ((s >> 16) as i16) as i32).collect()), err, None),
        (cpal::SampleFormat::I32, _) => dev.build_input_stream(cfg.clone(), move |d: &[i32], _: &cpal::InputCallbackInfo| push(d.iter().map(|s| s >> 8).collect()), err, None),
        (_, 16) => dev.build_input_stream(
            cfg.clone(),
            move |d: &[f32], _: &cpal::InputCallbackInfo| push(d.iter().map(|s| ((s.clamp(-1.0, 1.0) * 32767.0).round() as i16) as i32).collect()),
            err,
            None,
        ),
        _ => dev.build_input_stream(cfg.clone(), move |d: &[f32], _: &cpal::InputCallbackInfo| push(d.iter().map(|s| (s.clamp(-1.0, 1.0) as f64 * 8_388_607.0).round() as i32).collect()), err, None),
    };
    let stream = match stream {
        Ok(s) => s,
        Err(e) => return fail(e.to_string(), &ready),
    };
    if let Err(e) = stream.play() {
        return fail(e.to_string(), &ready);
    }
    let mut writer = match open_writer(&opts, rate, chans, bits) {
        Ok(w) => w,
        Err(e) => return fail(e, &ready),
    };
    let _ = ready.send(Ok(()));

    let mut det = detector(&opts, rate);
    let mut last_flush = Instant::now();
    let mut frames = 0u64;
    let mut peak = 0i32;
    let mut auto_stopped = false;
    let max_frames = opts.max_s.map(|s| (s * rate as f32) as u64);
    let scale = 1.0 / (chans as f32 * full as f32);
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(v) => {
                if let Some(w) = writer.as_mut() {
                    if w.write(&v).is_err() {
                        stop.store(true, Ordering::Relaxed);
                        break;
                    }
                }
                peak = peak.max(v.iter().map(|s| s.saturating_abs()).max().unwrap_or(0));
                let n = (v.len() / chans) as u64;
                frames += n;
                level.frames.store(frames, Ordering::Relaxed);
                if let Some(d) = det.as_mut() {
                    let mono: Vec<f32> = v.chunks(chans).map(|c| c.iter().map(|s| *s as f32).sum::<f32>() * scale).collect();
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
                let _ = w.flush_header();
            }
            last_flush = Instant::now();
        }
    }
    drop(stream);
    // drain what the callback already handed over
    while let Ok(v) = rx.try_recv() {
        if let Some(w) = writer.as_mut() {
            let _ = w.write(&v);
        }
        frames += (v.len() / chans) as u64;
    }
    if let Some(w) = writer {
        w.finish().map_err(|e| e.to_string())?;
    }
    Ok(Done {
        path: opts.path,
        frames,
        rate,
        bits,
        peak_db: db(peak as f32 / full as f32),
        auto_stopped,
        clipped: level.clipped.load(Ordering::Relaxed),
        bit_perfect: None,
        emphasis: None,
        notes: Vec::new(),
    })
}

/// The card index in "hw:CARD=x,DEV=n" (or "hw:1,0").
fn card_of(id: &str) -> Option<i32> {
    let rest = id.strip_prefix("hw:")?;
    let first = rest.split(',').next()?;
    let c = first.strip_prefix("CARD=").unwrap_or(first);
    if let Ok(n) = c.parse::<i32>() {
        return Some(n);
    }
    alsa::card::Iter::new().flatten().find(|k| alsa::Ctl::from_card(k, false).ok().and_then(|ctl| ctl.card_info().ok().and_then(|i| i.get_id().ok().map(|s| s == c))).unwrap_or(false)).map(|k| k.get_index())
}

/// errno for "device or resource busy".
const EBUSY: i32 = 16;

fn run_digital(opts: Opts, stop: Arc<AtomicBool>, level: Arc<Level>, ready: std::sync::mpsc::Sender<Result<(), String>>) -> Result<Done, String> {
    use alsa::pcm::{Access, Format, HwParams, PCM};
    use alsa::{Direction, ValueOr};
    let fail = |e: String, ready: &std::sync::mpsc::Sender<Result<(), String>>| {
        let _ = ready.send(Err(e.clone()));
        Err(e)
    };
    let Some(id) = opts.device.clone().filter(|d| d.starts_with("hw:")) else {
        return fail("pick an S/PDIF input (an ALSA hw: device) for a digital capture".into(), &ready);
    };
    let card = card_of(&id);
    let card_name = card.map(|c| alsa::card::Card::new(c).get_name().unwrap_or_else(|_| format!("card {c}"))).unwrap_or_else(|| id.clone());
    let pcm = match PCM::new(&id, Direction::Capture, false) {
        Ok(p) => p,
        Err(e) if e.errno() == EBUSY => {
            return fail(
                format!("{card_name} ({id}) is busy: something else holds it, usually PipeWire or PulseAudio. Free it (for example by setting that card's profile to Off in your sound settings) and try again."),
                &ready,
            )
        }
        Err(e) => return fail(format!("{card_name} ({id}): {e}"), &ready),
    };
    let want_rate = if opts.spec.rate > 0 { opts.spec.rate } else { 48000 };
    let (rate, fmt) = {
        let hwp = match HwParams::any(&pcm) {
            Ok(h) => h,
            Err(e) => return fail(e.to_string(), &ready),
        };
        let _ = hwp.set_rate_resample(false);
        if let Err(e) = hwp.set_access(Access::RWInterleaved).and_then(|_| hwp.set_channels(2)) {
            return fail(format!("{card_name}: {e}"), &ready);
        }
        let fmt = [Format::s32(), Format::s24(), Format::s16()].into_iter().find(|f| hwp.test_format(*f).is_ok());
        let Some(fmt) = fmt else { return fail(format!("{card_name}: no 16, 24 or 32-bit capture format"), &ready) };
        if let Err(e) = hwp.set_format(fmt) {
            return fail(format!("{card_name}: {e}"), &ready);
        }
        let rate = if opts.spec.rate > 0 {
            match hwp.set_rate(want_rate, ValueOr::Nearest) {
                Ok(()) => want_rate,
                Err(_) => return fail(format!("{card_name} will not capture at {want_rate} Hz. S/PDIF runs at the source's rate: check the deck's output and the medium's rate."), &ready),
            }
        } else {
            hwp.set_rate_near(want_rate, ValueOr::Nearest).unwrap_or(want_rate)
        };
        let _ = hwp.set_period_size_near(4096, ValueOr::Nearest);
        let _ = hwp.set_buffer_size_near(4096 * 8);
        if let Err(e) = pcm.hw_params(&hwp) {
            return fail(format!("{card_name}: {e}"), &ready);
        }
        (rate, fmt)
    };
    level.rate.store(rate, Ordering::Relaxed);
    let mut notes = Vec::new();
    let status = card.and_then(iec958::read_status);
    let mut emphasis = None;
    if let Some(st) = &status {
        emphasis = Some(st.emphasis);
        if let Some(r) = st.rate {
            if r != rate {
                notes.push(format!("The card's IEC958 status says {r} Hz but the medium is set to {rate} Hz."));
            }
        }
        if st.emphasis {
            notes.push("The IEC958 status says the audio is pre-emphasized: de-emphasis will be turned on.".into());
        }
    }
    *level.note.lock().unwrap() = notes.first().cloned();
    let check = fmt != Format::s16();
    if check {
        level.bit_perfect.store(1, Ordering::Relaxed);
    }
    let mut writer = match open_writer(&opts, rate, 2, 24) {
        Ok(w) => w,
        Err(e) => return fail(e, &ready),
    };
    if let Err(e) = pcm.prepare().and_then(|_| pcm.start()) {
        return fail(format!("{card_name}: {e}"), &ready);
    }
    let _ = ready.send(Ok(()));

    let full = 8_388_608i32;
    let mut det = detector(&opts, rate);
    let mut buf = vec![0i32; 4096 * 2];
    let mut buf16 = vec![0i16; 4096 * 2];
    let mut frames = 0u64;
    let mut peak = 0i32;
    let mut low = 0u64;
    let mut auto_stopped = false;
    let mut last_flush = Instant::now();
    let max_frames = opts.max_s.map(|s| (s * rate as f32) as u64);
    let scale = 1.0 / (2.0 * full as f32);
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let read = if fmt == Format::s16() {
            pcm.io_i16().and_then(|io| io.readi(&mut buf16)).map(|n| {
                for i in 0..n * 2 {
                    buf[i] = (buf16[i] as i32) << 8;
                }
                n
            })
        } else if fmt == Format::s24() {
            pcm.io_i32_s24().and_then(|io| io.readi(&mut buf))
        } else {
            pcm.io_i32().and_then(|io| io.readi(&mut buf)).map(|n| {
                for v in buf[..n * 2].iter_mut() {
                    *v >>= 8;
                }
                n
            })
        };
        let n = match read {
            Ok(n) => n,
            Err(e) => {
                if pcm.try_recover(e, true).is_err() {
                    notes.push(format!("capture stopped: {e}"));
                    break;
                }
                notes.push("an overrun dropped some samples; the capture went on".into());
                continue;
            }
        };
        let v = &buf[..n * 2];
        if check {
            let k = runout_engine::audio::low_bits_nonzero(v) as u64;
            if k > 0 {
                low += k;
                if level.bit_perfect.swap(2, Ordering::Relaxed) != 2 {
                    *level.note.lock().unwrap() = Some("NOT BIT-PERFECT: the low 8 bits are not zero. Something in the path is processing the audio (resampling, volume, dither), or the source is really 24-bit. Still recording.".into());
                }
                level.low_bits.store(low, Ordering::Relaxed);
            }
        }
        level.meter(v, 2, full);
        if let Some(w) = writer.as_mut() {
            if w.write(v).is_err() {
                break;
            }
        }
        peak = peak.max(v.iter().map(|s| s.saturating_abs()).max().unwrap_or(0));
        frames += n as u64;
        level.frames.store(frames, Ordering::Relaxed);
        if let Some(d) = det.as_mut() {
            let mono: Vec<f32> = v.chunks(2).map(|c| (c[0] + c[1]) as f32 * scale).collect();
            if d.feed(&mono) {
                auto_stopped = true;
                level.auto_stopped.store(true, Ordering::Relaxed);
                break;
            }
        }
        if max_frames.map(|m| frames >= m).unwrap_or(false) {
            break;
        }
        if last_flush.elapsed() >= Duration::from_secs(1) {
            if let Some(w) = writer.as_mut() {
                let _ = w.flush_header();
            }
            last_flush = Instant::now();
        }
    }
    let _ = pcm.drop();
    let mut bits = 24u16;
    let bit_perfect = if check { Some(low == 0) } else { None };
    if let Some(w) = writer {
        w.finish().map_err(|e| e.to_string())?;
        if bit_perfect == Some(true) {
            if let Some(p) = &opts.path {
                if runout_engine::audio::shrink_to_16(p).map_err(|e| e.to_string())? {
                    bits = 16;
                    notes.push("Bit-perfect for the whole capture: the raw is stored as 16-bit.".into());
                }
            }
        } else if bit_perfect == Some(false) {
            notes.push(format!("{low} samples had low bits set: the capture is not bit-perfect and stays 24-bit."));
        }
    }
    Ok(Done { path: opts.path, frames, rate, bits, peak_db: db(peak as f32 / full as f32), auto_stopped, clipped: level.clipped.load(Ordering::Relaxed), bit_perfect, emphasis, notes })
}
