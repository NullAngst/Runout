//! Synthetic test material and the end-to-end self-test used in CI.
//!
//! Vinyl: a log sine sweep with 60 Hz hum, a little hiss, and clicks at known
//! positions. It goes through the full chain and the result is checked: clicks
//! found and repaired, hum gone, loudness on target, true peak under the
//! ceiling after the 16-bit step. This part is exactly what it always was.
//!
//! Tape: a 96 kHz stereo clip as a worn deck would play it: a 1 kHz reference
//! tone, a sine sweep, hiss at -50 dBFS, the deck 3.0% fast, 0.3% wow at
//! 0.5 Hz carried on 60 Hz hum recorded onto the tape, the right channel
//! 40 us late, 12 dropouts from 5 to 200 ms, and a B-type encoded segment.
//!
//! Digital: a 16-bit fixture through the digital chain, the low-bit check, and
//! error repair on injected mutes and holds.

use crate::audio::SourceAudio;
use crate::chain::{Chain, SourceFx};
use crate::dsp::loudness;
use crate::export::to_int_dithered;
use crate::project::{Project, Source, SourceKind, Track};
use crate::render;

pub struct TestSignal {
    pub clean: Vec<Vec<f32>>,
    pub dirty: Vec<Vec<f32>>,
    pub click_pos: Vec<usize>,
    pub rate: u32,
}

struct Rng(u64);
impl Rng {
    fn f(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32 - 0.5
    }
}

/// `secs` of stereo audio at 48 kHz. Sweep from 80 Hz to 8 kHz at about
/// -24 dBFS, hum at -46 dBFS, hiss at -66 dBFS, `n_clicks` clicks.
pub fn signal(secs: f32, hum: bool, n_clicks: usize) -> TestSignal {
    let rate = 48000u32;
    let n = (secs * rate as f32) as usize;
    let mut rng = Rng(0x5eed);
    let (f0, f1) = (80.0f64, 8000.0f64);
    let k = (f1 / f0).ln() / secs as f64;
    let mut clean = vec![vec![0.0f32; n]; 2];
    let mut dirty = vec![vec![0.0f32; n]; 2];
    for i in 0..n {
        let t = i as f64 / rate as f64;
        // log sweep phase: 2*pi*f0*(e^{kt}-1)/k, with a slow amplitude wobble like music
        let ph = 2.0 * std::f64::consts::PI * f0 * ((k * t).exp() - 1.0) / k;
        let amp = 0.08 * (1.0 + 0.5 * (2.0 * std::f64::consts::PI * 0.7 * t).sin());
        let fade = (t / 0.3).min(1.0) * ((secs as f64 - t) / 0.3).clamp(0.0, 1.0);
        let s = (amp * fade * ph.sin()) as f32;
        let s2 = (amp * fade * (ph * 1.002 + 0.3).sin()) as f32;
        clean[0][i] = s;
        clean[1][i] = s2;
        let h = if hum { (0.005 * (2.0 * std::f64::consts::PI * 60.0 * t).sin() + 0.002 * (2.0 * std::f64::consts::PI * 180.0 * t).sin()) as f32 } else { 0.0 };
        dirty[0][i] = s + h + 0.0007 * rng.f();
        dirty[1][i] = s2 + h + 0.0007 * rng.f();
    }
    let mut click_pos = Vec::new();
    if n_clicks > 0 {
        let span = n - rate as usize;
        for j in 0..n_clicks {
            let p = rate as usize / 2 + span * j / n_clicks + (rng.f().abs() * 1000.0) as usize;
            let len = 3 + (j * 7) % 40;
            let amp = 0.3 + 0.4 * rng.f().abs();
            for q in 0..len {
                // a sharp spike with a decaying ring, on both channels
                let v = amp * (-(q as f32) / (len as f32 / 3.0)).exp() * if q % 2 == 0 { 1.0 } else { -0.6 };
                dirty[0][p + q] += v;
                dirty[1][p + q] += v * 0.8;
            }
            click_pos.push(p);
        }
    }
    TestSignal { clean, dirty, click_pos, rate }
}

fn goertzel_db(x: &[f32], rate: u32, f: f64) -> f32 {
    let w = 2.0 * std::f64::consts::PI * f / rate as f64;
    let c = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &v in x {
        let s0 = v as f64 + c * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let p = (s1 * s1 + s2 * s2 - c * s1 * s2).max(1e-30);
    (10.0 * (p / (x.len() as f64).powi(2)).log10()) as f32
}

pub struct Check {
    pub name: String,
    pub value: String,
    pub pass: bool,
}

fn check(name: &str, value: String, pass: bool) -> Check {
    Check { name: name.into(), value, pass }
}

/// Project holding one in-memory source and one track over all of it.
pub fn one_track_project(src: &SourceAudio, chain: Chain) -> Project {
    let mut p = Project { chain, ..Default::default() };
    p.sources.push(Source {
        id: 1,
        name: "Test".into(),
        path: "test.wav".into(),
        kind: SourceKind::File,
        raw: false,
        rate: src.rate,
        channels: src.channels,
        bits: 16,
        frames: src.frames,
        fx: SourceFx::default(),
    });
    p.tracks.push(Track {
        id: 2,
        source_id: 1,
        start_s: 0.0,
        end_s: src.duration(),
        title: "Test".into(),
        number: Some(1),
        disc: None,
        fade_in_ms: 0.0,
        fade_out_ms: 0.0,
        chain: None,
        cleaned: false,
        file_name: None,
        tags: Default::default(),
        analysis: None,
        joins: Vec::new(),
    });
    p.next_id = 3;
    p
}

pub fn run() -> (Vec<Check>, f32) {
    let t0 = std::time::Instant::now();
    let sig = signal(20.0, true, 40);
    let src = SourceAudio::from_planar(sig.rate, 16, &sig.dirty);
    let mut chain = Chain::default();
    chain.hum.enabled = true;
    chain.hum.mains_hz = 60.0;
    chain.denoise.enabled = false; // checked separately in unit tests
    chain.decrackle.enabled = false;
    let mut p = one_track_project(&src, chain);
    let mut checks = Vec::new();

    let t = p.tracks[0].clone();
    let an = render::analyze_track(&p, &t, &src).expect("analysis");
    p.tracks[0].analysis = Some(an.analysis.clone());

    // clicks: each inserted click must have a repair hit within 2 ms
    let tol = (sig.rate / 500) as i64;
    let found = sig.click_pos.iter().filter(|&&cp| an.clicks.iter().any(|h| (h.pos - cp as i64).abs() <= tol + 40)).count();
    let need = sig.click_pos.len() * 9 / 10;
    checks.push(Check { name: "clicks repaired".into(), value: format!("{found} of {}", sig.click_pos.len()), pass: found >= need });

    let t = p.tracks[0].clone();
    let (out, _) = render::render_final(&p, &t, &src, 0.0).expect("render");

    // residual click energy vs the clean signal around each click (hum and hiss removed first)
    let mut worst = 0.0f32;
    let mut per_click = Vec::new();
    {
        let mut ref_chain = Chain::default();
        ref_chain.hum.enabled = true;
        ref_chain.declick.enabled = false;
        ref_chain.decrackle.enabled = false;
        ref_chain.denoise.enabled = false;
        ref_chain.level.enabled = false;
        let clean_src = SourceAudio::from_planar(sig.rate, 16, &sig.clean);
        let rp = one_track_project(&clean_src, ref_chain.clone());
        let cleaned_ref = render::render_range(&clean_src, &SourceFx::default(), &rp.chain, 0, clean_src.frames, false, false, false).out;
        let mut c2 = p.chain.clone();
        c2.level.enabled = false;
        let ours = render::render_range(&src, &SourceFx::default(), &c2, 0, src.frames, false, false, false).out;
        for &cp in &sig.click_pos {
            let mut w = 0.0f32;
            for i in cp..cp + 48 {
                w = w.max((ours[0][i] - cleaned_ref[0][i]).abs());
            }
            worst = worst.max(w);
            per_click.push(w);
        }
    }
    per_click.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = per_click.get(per_click.len() / 2).copied().unwrap_or(0.0);
    // Clicks are 0.3 to 0.7 high. Typical leftover must be tiny; the worst (a
    // 42-sample ringing click) must still be cut by well over 10 dB.
    checks.push(Check { name: "click residual, median".into(), value: format!("{:.4}", median), pass: median < 0.01 });
    checks.push(Check { name: "click residual, worst".into(), value: format!("{:.4}", worst), pass: worst < 0.08 });

    // hum
    let mid = &out[0][sig.rate as usize * 2..sig.rate as usize * 18];
    let din = &sig.dirty[0][sig.rate as usize * 2..sig.rate as usize * 18];
    let gain = p.gain_for(&p.tracks[0]).gain_db;
    let h_in = goertzel_db(din, sig.rate, 60.0) + gain;
    let h_out = goertzel_db(mid, sig.rate, 60.0);
    checks.push(Check { name: "60 Hz hum reduction".into(), value: format!("{:.1} dB", h_in - h_out), pass: h_in - h_out > 25.0 });

    // loudness and true peak after the 16-bit step
    let ints = to_int_dithered(&out, 16, 99);
    let back: Vec<Vec<f32>> = (0..2).map(|c| (0..out[0].len()).map(|i| ints[i * 2 + c] as f32 / 32768.0).collect()).collect();
    let l = loudness::measure(&back, sig.rate).0;
    let g = p.gain_for(&p.tracks[0]);
    let target = p.chain.level.target_lufs - g.short_db;
    checks.push(Check { name: "integrated loudness".into(), value: format!("{:.2} LUFS (target {:.1})", l.integrated, target), pass: (l.integrated - target).abs() < 0.6 });
    checks.push(Check { name: "true peak".into(), value: format!("{:.2} dBTP (ceiling {:.1})", l.true_peak, p.chain.level.ceiling_dbtp), pass: l.true_peak <= p.chain.level.ceiling_dbtp + 0.05 });

    // block preview matches export render
    let blk = render::render_play_block(&p, 1, &src, sig.rate as i64 * 5, sig.rate as usize).expect("block");
    let mut diff = 0.0f32;
    for i in 0..sig.rate as usize {
        diff = diff.max((blk.out[0][i] - out[0][sig.rate as usize * 5 + i]).abs());
    }
    checks.push(Check { name: "preview block vs export".into(), value: format!("max diff {:.5}", diff), pass: diff < 0.01 });

    // Limiter under stress: a quiet tone with loud bursts must hit the target
    // only as far as the 3 dB cap allows, and never cross the ceiling.
    {
        let rate = 48000usize;
        let n = rate * 12;
        let mut x = vec![vec![0.0f32; n]; 2];
        for i in 0..n {
            let t = i as f64 / rate as f64;
            let base = 0.03 * (2.0 * std::f64::consts::PI * 220.0 * t).sin();
            let burst = if (i % (rate * 2)) < rate / 500 { 0.5 * (2.0 * std::f64::consts::PI * 3150.0 * t + 0.4).sin() } else { 0.0 };
            x[0][i] = (base + burst) as f32;
            x[1][i] = (base * 0.9 + burst) as f32;
        }
        let s2 = SourceAudio::from_planar(48000, 16, &x);
        let mut c = Chain::passthrough();
        c.level.enabled = true;
        let mut lp = one_track_project(&s2, c);
        let tt = lp.tracks[0].clone();
        let a = render::analyze_track(&lp, &tt, &s2).expect("analysis");
        lp.tracks[0].analysis = Some(a.analysis);
        let tt = lp.tracks[0].clone();
        let (o, gr) = render::render_final(&lp, &tt, &s2, 0.0).expect("render");
        let ints = to_int_dithered(&o, 16, 7);
        let back: Vec<Vec<f32>> = (0..2).map(|c| (0..o[0].len()).map(|i| ints[i * 2 + c] as f32 / 32768.0).collect()).collect();
        let l = loudness::measure(&back, 48000).0;
        let g = lp.gain_for(&tt);
        checks.push(Check {
            name: "limiter cap respected".into(),
            value: format!("{:.2} dB reduction, {:.1} dB short", gr, g.short_db),
            pass: gr <= lp.chain.level.max_gr_db + 0.3 && g.short_db > 0.0,
        });
        checks.push(Check { name: "true peak, hard material".into(), value: format!("{:.2} dBTP", l.true_peak), pass: l.true_peak <= lp.chain.level.ceiling_dbtp + 0.05 });
    }

    // Real-time factor of the full chain, denoise and decrackle included.
    {
        let x = vinyl_throughput();
        let threads = rayon::current_num_threads();
        checks.push(Check { name: "full chain speed".into(), value: format!("{x:.0}x real time on {threads} threads"), pass: x >= 10.0 });
    }

    let secs = t0.elapsed().as_secs_f32();
    (checks, secs)
}

/// Real-time factor of the full vinyl chain on 30 s of generated audio.
pub fn vinyl_throughput() -> f32 {
    let sig = signal(30.0, true, 60);
    let src = SourceAudio::from_planar(sig.rate, 16, &sig.dirty);
    let mut fx = SourceFx::default();
    fx.noise = crate::auto::learn_noise_at(&src, 0, sig.rate as usize * 2);
    let mut c = Chain::default();
    c.hum.enabled = true;
    let t = std::time::Instant::now();
    let _ = render::render_range(&src, &fx, &c, 0, src.frames, false, false, true);
    (30.0 / t.elapsed().as_secs_f64()) as f32
}

pub const TAPE_RATE: u32 = 96000;
pub const TAPE_CLIP_S: f32 = 24.0;
pub const TAPE_SPEED_ERR: f64 = 0.03;
pub const TAPE_WOW: f64 = 0.003;
pub const TAPE_WOW_HZ: f64 = 0.5;
pub const TAPE_AZIMUTH_US: f64 = 40.0;
pub const TAPE_NR_REF_DBFS: f32 = -18.0;
/// Where the B-type segment sits in content time, and its steps.
pub const BTYPE_FROM_S: f64 = 14.0;
pub const BTYPE_STEP_S: f64 = 0.15;
pub const BTYPE_FREQS: [f64; 8] = [50.0, 100.0, 400.0, 1000.0, 3000.0, 6000.0, 10000.0, 15000.0];
pub const BTYPE_LEVELS: [f64; 5] = [-40.0, -30.0, -20.0, -10.0, 0.0];

pub struct TapeSignal {
    /// As the deck played it: what a capture would hold.
    pub dirty: Vec<Vec<f32>>,
    pub rate: u32,
    /// Dropouts in capture seconds: (start, length).
    pub dropouts: Vec<(f64, f64)>,
    /// The B-type segment before encoding, and encoded, at content rate (mono).
    pub btype_plain: Vec<f32>,
    pub btype_encoded: Vec<f32>,
}

fn btype_steps(rate: u32) -> Vec<f32> {
    let n = (BTYPE_STEP_S * rate as f64) as usize;
    let mut out = Vec::new();
    for &f in &BTYPE_FREQS {
        for &l in &BTYPE_LEVELS {
            let a = 10f64.powf((TAPE_NR_REF_DBFS as f64 + l) / 20.0) * std::f64::consts::SQRT_2;
            for i in 0..n {
                let t = i as f64 / rate as f64;
                // 2 ms ramps so steps do not click
                let r = (t / 0.002).min(1.0).min((BTYPE_STEP_S - t) / 0.002).max(0.0);
                out.push((a * r * (2.0 * std::f64::consts::PI * f * t).sin()) as f32);
            }
        }
    }
    out
}

/// The tape clip. Content is made at the true speed, then played back through
/// a deck that runs fast and wobbles.
pub fn tape_signal(secs: f32) -> TapeSignal {
    let rate = TAPE_RATE;
    let fs = rate as f64;
    // content length with headroom: the deck plays it faster than real time
    let n_content = ((secs as f64 * (1.0 + TAPE_SPEED_ERR + 0.01)) * fs) as usize + rate as usize;
    let plain = btype_steps(rate);
    let mut enc = plain.clone();
    crate::dsp::nr::btype_encode(&mut enc, fs, TAPE_NR_REF_DBFS);
    let b0 = (BTYPE_FROM_S * fs) as usize;
    let b1 = b0 + enc.len();
    let sweep_from = 4.5f64;
    let sweep_to = BTYPE_FROM_S;
    let (f0, f1) = (150.0f64, 12000.0f64);
    let k = (f1 / f0).ln() / (sweep_to - sweep_from);
    let content: Vec<f32> = (0..n_content)
        .map(|i| {
            let u = i as f64 / fs;
            let music = if u < sweep_from {
                // reference tone at -12 dBFS
                let r = (u / 0.01).min(1.0).min((sweep_from - u) / 0.01);
                0.25 * 1.4142 * r * (2.0 * std::f64::consts::PI * 1000.0 * u).sin() * 0.5
            } else if u < sweep_to {
                let t = u - sweep_from;
                let ph = 2.0 * std::f64::consts::PI * f0 * ((k * t).exp() - 1.0) / k;
                let fade = (t / 0.05).min(1.0).min((sweep_to - u) / 0.05).max(0.0);
                let amp = 0.12 * fade * (1.0 + 0.3 * (2.0 * std::f64::consts::PI * 0.7 * t).sin());
                amp * ph.sin()
            } else if i >= b0 && i < b1 {
                enc[i - b0] as f64
            } else {
                let t = u - BTYPE_FROM_S;
                0.08 * (2.0 * std::f64::consts::PI * (700.0 + 300.0 * t) * t).sin()
            };
            // hum went onto the tape when it was recorded: steady in content time
            let hum = 0.01 * (2.0 * std::f64::consts::PI * 60.0 * u).sin();
            (music + hum) as f32
        })
        .collect();
    // play it back: content time advances at s(tau) per capture second
    let n = (secs as f64 * fs) as usize;
    let mut u = Vec::with_capacity(n);
    let mut acc = 0.5 * fs; // start half a second in, so the interpolator has context
    for i in 0..n {
        let tau = i as f64 / fs;
        u.push(acc);
        let s = (1.0 + TAPE_SPEED_ERR) * (1.0 + TAPE_WOW * (2.0 * std::f64::consts::PI * TAPE_WOW_HZ * tau).sin());
        acc += s;
    }
    let it = crate::dsp::resample::Interp::new(1.04);
    let d = TAPE_AZIMUTH_US * 1e-6 * fs * (1.0 + TAPE_SPEED_ERR);
    let mut rng = Rng(0x7a9e);
    let mut dirty = vec![vec![0.0f32; n]; 2];
    // capture-time dropouts, 5 to 200 ms, spread over the sweep and after it
    let lens = [5.0, 10.0, 20.0, 30.0, 45.0, 60.0, 80.0, 100.0, 120.0, 150.0, 175.0, 200.0];
    let dropouts: Vec<(f64, f64)> = lens.iter().enumerate().map(|(i, ms)| (4.6 + i as f64 * 0.75, ms / 1000.0)).collect();
    let hiss = 10f32.powf(-50.0 / 20.0) * 3.4641; // uniform noise at -50 dBFS RMS
    for i in 0..n {
        let tau = i as f64 / fs;
        let mut g = 1.0f32;
        for &(a, l) in &dropouts {
            let ramp = 0.001;
            if tau > a - ramp && tau < a + l + ramp {
                let e = ((tau - a + ramp) / ramp).min(1.0).min((a + l + ramp - tau) / ramp).clamp(0.0, 1.0);
                g = g.min(1.0 - 0.9 * e as f32);
            }
        }
        let l = it.at(&content, u[i]);
        let r = it.at(&content, u[i] - d);
        dirty[0][i] = (l + hiss * rng.f()) * g;
        dirty[1][i] = (r + hiss * rng.f()) * g;
    }
    TapeSignal { dirty, rate, dropouts, btype_plain: plain, btype_encoded: enc }
}

/// The tape chain the self-test runs at full stretch.
pub fn tape_chain() -> Chain {
    let mut c = Chain::default();
    c.declick.enabled = false;
    c.decrackle.enabled = false;
    c.wow.enabled = true;
    c.azimuth.enabled = true;
    c.dropout.enabled = true;
    c.denoise.enabled = true;
    c.band_limit.enabled = true;
    c.band_limit.low_hz = 40.0;
    c.band_limit.high_hz = 16000.0;
    c.playback_eq.enabled = true;
    c.playback_eq.from = crate::chain::CurveSel::named("cassette-120");
    c.playback_eq.to = crate::chain::CurveSel::named("cassette-70");
    c.nr.enabled = true;
    c.nr.ref_dbfs = TAPE_NR_REF_DBFS;
    c
}

fn tone_level(x: &[f32], rate: u32, f: f64) -> f32 {
    // Hann-windowed Goertzel, as an amplitude in dB
    let w = 2.0 * std::f64::consts::PI * f / rate as f64;
    let c = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    let n = x.len() as f64;
    for (i, &v) in x.iter().enumerate() {
        let h = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n).cos();
        let s0 = v as f64 * h + c * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (10.0 * ((s1 * s1 + s2 * s2 - c * s1 * s2) / (n * n)).max(1e-30).log10()) as f32
}

/// Tape checks: speed, wow, azimuth, dropouts, B-type, throughput, buzz.
pub fn run_tape() -> Vec<Check> {
    let mut checks = Vec::new();
    let sig = tape_signal(TAPE_CLIP_S);
    let rate = sig.rate;
    let src = SourceAudio::from_planar(rate, 24, &sig.dirty);

    // speed error: the hum's mean over the whole clip gives it (a short tone at
    // the head only sees part of a wow cycle, so it is reported alongside)
    let tone = crate::auto::detect_tone(&src);
    let tone_err = tone.as_ref().and_then(|t| t.speed_err_pct);
    let mut wc = Chain::passthrough();
    wc.wow.enabled = true;
    let w = crate::auto::measure_wow(&src, &wc.wow);
    let err = w.static_err_pct.unwrap_or(0.0);
    checks.push(check(
        "tape speed error",
        format!("{err:.3}% from the hum (true {:.3}%), tone says {}", TAPE_SPEED_ERR * 100.0, tone_err.map(|e| format!("{e:.2}%")).unwrap_or_else(|| "nothing".into())),
        (err as f64 - TAPE_SPEED_ERR * 100.0).abs() < 0.05 && tone_err.map(|e| (e as f64 - TAPE_SPEED_ERR * 100.0).abs() < 0.2).unwrap_or(false),
    ));

    // wow from the hum, then what is left of it in the corrected output
    let mut fx = SourceFx::default();
    fx.speed.unit = crate::chain::SpeedUnit::Cms;
    fx.speed.captured = 4.76;
    fx.speed.correct = 4.76;
    fx.speed.trim_pct = err;
    let before = w.peak_pct.unwrap_or(0.0);
    fx.analysis.set_wow(1, w.track);
    let frames = crate::chain::processed_frames(&src, &fx);
    let out = render::render_range(&src, &fx, &wc, 0, frames, false, false, false).out;
    let mono: Vec<f32> = out[0].iter().zip(&out[1]).map(|(a, b)| 0.5 * (a + b)).collect();
    // Measured on the corrected output where nothing else disturbs the hum: the
    // stretch before the first dropout. A dropout's edges splatter the music
    // over the hum for a fraction of a second, which would be measuring the
    // measurement, not the correction.
    let first_drop = sig.dropouts[0].0 / fx.speed.ratio();
    let (a, b) = ((0.8 * rate as f64) as usize, ((first_drop - 0.3) * rate as f64) as usize);
    let residual = crate::dsp::wow::track(&mono[a..b.min(mono.len())], rate, 60.0, 0.02, 4.0).map(|t| (t.peak_dev_pct(), t.mean())).unwrap_or((99.0, 0.0));
    checks.push(check("tape residual wow", format!("{:.4}% (was {before:.3}%), hum now {:.3} Hz", residual.0, residual.1), residual.0 < 0.05 && (residual.1 - 60.0).abs() < 0.05));

    // azimuth
    let az = crate::auto::measure_azimuth(&src);
    let mean = az.as_ref().map(|c| c.mean()).unwrap_or(0.0);
    // the deck runs fast, so the offset on the tape plays 3% shorter in time
    let want = -(TAPE_AZIMUTH_US) as f32;
    checks.push(check("tape azimuth", format!("{mean:.1} us (true {want:.1})"), (mean - want).abs() < 5.0));

    // dropouts, through the full chain
    let mut fx2 = fx.clone();
    fx2.analysis.set_azimuth(1, az.clone());
    fx2.noise = crate::auto::learn_noise_at(&src, (2.5 * rate as f64) as usize, (3.8 * rate as f64) as usize);
    let tc = tape_chain();
    let r = render::render_range(&src, &fx2, &tc, 0, frames, false, false, true);
    let events = render::dedupe_events(r.events, rate);
    let ratio = fx2.speed.ratio();
    let map = fx2.analysis.wow_map().cloned().unwrap_or_default();
    let to_src = |pos: i64| map.at(pos as f64 * ratio) / rate as f64;
    let found = sig.dropouts.iter().filter(|(a, _)| events.iter().any(|e| (to_src(e.pos) - a).abs() < 0.02)).count();
    // the clean section: everything before the B-type test pattern
    let clean_end = (BTYPE_FROM_S - 0.5) / (1.0 + TAPE_SPEED_ERR);
    let false_pos = events
        .iter()
        .map(|e| to_src(e.pos))
        .filter(|t| *t < clean_end && !sig.dropouts.iter().any(|(a, l)| *t > a - 0.03 && *t < a + l + 0.03))
        .count();
    checks.push(check("tape dropouts found", format!("{found} of {}, {false_pos} false", sig.dropouts.len()), found == sig.dropouts.len() && false_pos == 0));

    // B-type round trip through the NR stage
    {
        let seg = SourceAudio::from_planar(rate, 24, &[sig.btype_encoded.clone()]);
        let mut c = Chain::passthrough();
        c.nr.enabled = true;
        c.nr.ref_dbfs = TAPE_NR_REF_DBFS;
        // DC removal always runs; it is 5 Hz and does not touch 50 Hz by more than 0.05 dB
        let dec = render::render_range(&seg, &SourceFx::default(), &c, 0, seg.frames, false, false, false).out;
        let step = (BTYPE_STEP_S * rate as f64) as usize;
        let mut worst = 0.0f32;
        let mut k = 0;
        for &f in &BTYPE_FREQS {
            for _ in &BTYPE_LEVELS {
                let a = k * step + step / 2;
                let b = (k + 1) * step - step / 8;
                let d = tone_level(&dec[0][a..b], rate, f) - tone_level(&sig.btype_plain[a..b], rate, f);
                worst = worst.max(d.abs());
                k += 1;
            }
        }
        checks.push(check("B-type round trip", format!("worst {worst:.2} dB, 50 Hz to 15 kHz, -40 to 0 dB"), worst <= 1.0));
    }

    // full tape chain speed at 96 kHz
    {
        let threads = rayon::current_num_threads();
        let t = std::time::Instant::now();
        let _ = render::render_range(&src, &fx2, &tc, 0, frames, false, false, true);
        let x = TAPE_CLIP_S / t.elapsed().as_secs_f32();
        checks.push(check("tape chain speed", format!("{x:.0}x real time at 96 kHz on {threads} threads"), x >= 10.0));
    }

    // head-switching buzz: 59.94 Hz and harmonics to 10 kHz
    {
        let rate = 96000u32;
        let n = rate as usize * 10;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f64 / rate as f64;
                let mut v = 0.0;
                let mut k = 1;
                while 59.94 * k as f64 <= 10000.0 {
                    v += (2.0 * std::f64::consts::PI * 59.94 * k as f64 * t).sin() / k as f64;
                    k += 1;
                }
                (0.02 * v) as f32
            })
            .collect();
        let s = SourceAudio::from_planar(rate, 24, &[x.clone(), x]);
        let m = crate::media::builtin().into_iter().find(|m| m.id == "video-hifi").unwrap();
        let mut c = Chain::passthrough();
        c.hum = m.chain.hum.clone();
        let out = render::render_range(&s, &SourceFx::default(), &c, 0, s.frames, false, false, false).out;
        let rms = |v: &[f32]| (v.iter().map(|a| (*a as f64).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
        let a = rate as usize * 4;
        let drop = 20.0 * (rms(&s.read_channel(0, a as i64, n as i64)) / rms(&out[0][a..]).max(1e-12)).log10();
        checks.push(check("head-switching buzz", format!("{drop:.1} dB down"), drop >= 30.0));
    }
    checks
}

/// 16-bit stereo fixture for the digital checks.
pub fn digital_fixture(secs: f32) -> Vec<Vec<i32>> {
    let rate = 48000.0;
    let n = (secs as f64 * rate) as usize;
    let mut rng = Rng(0xd47);
    (0..2)
        .map(|c| {
            (0..n)
                .map(|i| {
                    let t = i as f64 / rate;
                    let v = 0.3 * (2.0 * std::f64::consts::PI * (440.0 + 110.0 * c as f64) * t).sin() + 0.05 * (2.0 * std::f64::consts::PI * 3100.0 * t).sin() + 0.002 * rng.f() as f64;
                    (v * 32767.0).round() as i32
                })
                .collect()
        })
        .collect()
}

/// Digital checks: bit-identical passthrough, the low-bit check, error repair.
pub fn run_digital() -> Vec<Check> {
    let mut checks = Vec::new();
    let ints = digital_fixture(3.0);
    let planar: Vec<Vec<f32>> = ints.iter().map(|c| c.iter().map(|v| *v as f32 / 32768.0).collect()).collect();
    let interleaved: Vec<i16> = (0..ints[0].len()).flat_map(|i| [ints[0][i] as i16, ints[1][i] as i16]).collect();
    let src = SourceAudio::from_interleaved_i16(48000, 2, interleaved.clone());

    // passthrough: DAT medium, every stage off
    {
        let dat = crate::media::builtin().into_iter().find(|m| m.id == "dat").unwrap();
        let mut p = one_track_project(&src, Chain::default());
        crate::media::apply_to_project(&mut p, &dat, None, None);
        let t = p.tracks[0].clone();
        let pass = p.is_passthrough(&t);
        let (audio, _, rate) = render::render_final_at(&p, &t, &src, 0.0).expect("render");
        let back = crate::export::to_int_rounded(&audio, 16);
        let orig: Vec<i32> = interleaved.iter().map(|v| *v as i32).collect();
        // and the float path, as when error repair is on but finds nothing
        let mut p2 = p.clone();
        p2.chain.error_repair.enabled = true;
        let (audio2, _, _) = render::render_final_at(&p2, &t, &src, 0.0).expect("render");
        let back2 = crate::export::to_int_rounded(&audio2, 16);
        checks.push(check("digital passthrough", format!("{} at {rate} Hz, float path {}", if back == orig { "bit-identical" } else { "CHANGED" }, if back2 == orig { "bit-identical" } else { "CHANGED" }), pass && back == orig && back2 == orig));
    }

    // low-bit check: a clean 16-bit source in a 24-bit stream, then a processed one
    {
        let clean: Vec<i32> = interleaved.iter().map(|v| (*v as i32) << 8).collect();
        let mut processed = clean.clone();
        for (i, v) in processed.iter_mut().enumerate().step_by(97) {
            *v += 1 + (i % 7) as i32;
        }
        let a = crate::audio::low_bits_nonzero(&clean);
        let b = crate::audio::low_bits_nonzero(&processed);
        checks.push(check("low-bit check", format!("clean {a} flagged, processed {b} flagged"), a == 0 && b > 0));
    }

    // error repair: injected mutes and holds
    {
        let mut x = planar[0].clone();
        let clean = x.clone();
        let spots: [(usize, usize, bool); 4] = [(20000, 48, true), (45000, 6, false), (70000, 200, true), (100000, 960, true)];
        for &(p, l, zero) in &spots {
            let v = if zero { 0.0 } else { x[p] };
            for s in &mut x[p..p + l] {
                *s = v;
            }
        }
        let found = crate::dsp::dropout::find_errors(&x, 48000, 4, 10.0);
        let mut y = x.clone();
        crate::dsp::dropout::repair_errors(&mut y, &found, 32);
        let fixed = spots.iter().filter(|(p, l, _)| *l <= 480 && (*p..*p + *l).map(|i| (y[i] - clean[i]).abs()).fold(0.0f32, f32::max) < 0.02).count();
        let flagged = found.iter().filter(|e| e.fix == crate::dsp::dropout::Fix::Flagged).count();
        checks.push(check("digital error repair", format!("{} found, {fixed} of 3 short ones repaired, {flagged} flagged", found.len()), found.len() == 4 && fixed == 3 && flagged == 1));
    }
    checks
}

/// Every check: vinyl as it always was, then tape, then digital.
pub fn run_all() -> (Vec<Check>, f32) {
    let t0 = std::time::Instant::now();
    let (mut v, _) = run();
    v.extend(run_tape());
    v.extend(run_digital());
    (v, t0.elapsed().as_secs_f32())
}

#[cfg(test)]
mod tests {
    #[test]
    fn tape_and_digital_self_tests_pass() {
        let mut checks = super::run_tape();
        checks.extend(super::run_digital());
        for c in &checks {
            eprintln!("{:<28} {:<48} {}", c.name, c.value, if c.pass { "ok" } else { "FAIL" });
        }
        assert!(checks.iter().all(|c| c.pass));
    }

    #[test]
    fn self_test_passes() {
        let (checks, secs) = super::run();
        for c in &checks {
            eprintln!("{:<28} {:<36} {}", c.name, c.value, if c.pass { "ok" } else { "FAIL" });
        }
        eprintln!("took {secs:.1} s");
        assert!(checks.iter().all(|c| c.pass));
    }
}
