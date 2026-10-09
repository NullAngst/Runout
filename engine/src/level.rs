//! Loudness leveling: gain to a LUFS target, then a true-peak limiter with a hard
//! ceiling and a cap on how hard it may work.

use crate::chain::{Level, LevelMode};
use crate::dsp::limiter;
use crate::dsp::loudness::Loudness;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Gain {
    /// Gain applied before the limiter, dB.
    pub gain_db: f32,
    /// How far short of the target this track stops because of the limiter cap, dB.
    pub short_db: f32,
}

/// Gain for one track. `album_lufs` is used in album mode.
pub fn gain_for(level: &Level, before: &Loudness, album_lufs: Option<f32>) -> Gain {
    if !level.enabled || before.integrated <= -70.0 {
        return Gain::default();
    }
    let reference = match (level.mode, album_lufs) {
        (LevelMode::Album, Some(a)) if a > -70.0 => a,
        _ => before.integrated,
    };
    let want = level.target_lufs - reference;
    // The limiter may take off at most max_gr_db from the highest true peak.
    let allowed = level.ceiling_dbtp + level.max_gr_db.max(0.0) - before.true_peak;
    let g = want.min(allowed);
    Gain { gain_db: g, short_db: (want - g).max(0.0) }
}

/// Album loudness as the duration-weighted energy mean of the tracks'
/// integrated loudness.
pub fn album_lufs(tracks: &[(Loudness, f64)]) -> Option<f32> {
    let mut e = 0.0f64;
    let mut d = 0.0f64;
    for (l, dur) in tracks {
        if l.integrated > -70.0 {
            e += dur * 10f64.powf(l.integrated as f64 / 10.0);
            d += dur;
        }
    }
    if d <= 0.0 {
        None
    } else {
        Some((10.0 * (e / d).log10()) as f32)
    }
}

/// Fade in over the first `fade_in` frames and out over the last `fade_out`
/// frames of [0, len) inside the buffer, where the track starts at `start`.
/// Samples before the start or past the end are silenced.
pub fn apply_fades(ch: &mut [Vec<f32>], start: i64, len: usize, fade_in: usize, fade_out: usize) {
    for c in ch.iter_mut() {
        let n = c.len() as i64;
        for i in 0..n {
            let t = i - start;
            let g = if t < 0 || t >= len as i64 {
                0.0
            } else {
                let t = t as usize;
                let mut g = 1.0f32;
                if fade_in > 0 && t < fade_in {
                    g = g.min(fade_curve(t as f32 / fade_in as f32));
                }
                let left = len - 1 - t;
                if fade_out > 0 && left < fade_out {
                    g = g.min(fade_curve(left as f32 / fade_out as f32));
                }
                g
            };
            if g != 1.0 {
                c[i as usize] *= g;
            }
        }
    }
}

/// Equal-power-ish fade shape.
fn fade_curve(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    (x * std::f32::consts::FRAC_PI_2).sin().powi(2)
}

/// Apply gain then the limiter. Returns limiter gain reduction in dB.
pub fn apply(ch: &mut [Vec<f32>], rate: u32, level: &Level, gain_db: f32, ceiling_offset_db: f32) -> f32 {
    if !level.enabled {
        return 0.0;
    }
    let g = 10f32.powf(gain_db / 20.0);
    if (g - 1.0).abs() > 1e-7 {
        for c in ch.iter_mut() {
            c.iter_mut().for_each(|v| *v *= g);
        }
    }
    limiter::limit(
        ch,
        rate,
        &limiter::LimiterParams { ceiling_db: level.ceiling_dbtp - ceiling_offset_db, lookahead_ms: 5.0, release_ms: 80.0 },
    )
}
