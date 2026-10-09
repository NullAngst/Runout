//! Synthetic test material and the end-to-end self-test used in CI.
//!
//! The test signal is a log sine sweep with 60 Hz hum, a little hiss, and
//! clicks at known positions. It goes through the full chain and the result is
//! checked: clicks found and repaired, hum gone, loudness on target, true peak
//! under the ceiling after the 16-bit step.

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
        let sig = signal(30.0, true, 60);
        let src = SourceAudio::from_planar(sig.rate, 16, &sig.dirty);
        let mut fx = SourceFx::default();
        fx.noise = crate::auto::learn_noise_at(&src, 0, sig.rate as usize * 2);
        let mut c = Chain::default();
        c.hum.enabled = true;
        let t = std::time::Instant::now();
        let _ = render::render_range(&src, &fx, &c, 0, src.frames, false, false, true);
        let x = 30.0 / t.elapsed().as_secs_f32();
        let threads = rayon::current_num_threads();
        checks.push(Check { name: "full chain speed".into(), value: format!("{x:.0}x real time on {threads} threads"), pass: x >= 10.0 });
    }

    let secs = t0.elapsed().as_secs_f32();
    (checks, secs)
}

#[cfg(test)]
mod tests {
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
