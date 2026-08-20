//! Mixing: sum rendered events into one mono f32 buffer. Mixing is buffer
//! addition (spec §8.1) — no audio engine, just a soft clipper at the end so
//! coincident events can't wrap.

use crate::dsp::render_recipe;
use crate::events::AudioEvent;
use crate::SAMPLE_RATE;

/// Where the soft clipper starts bending. Below this, the mix is untouched.
const KNEE: f32 = 0.8;
/// Fade applied to the last samples so a truncated tail can't click.
const END_FADE_S: f32 = 0.02;

/// Mixes events into `duration_s` seconds of mono audio. Event tails that
/// would run past the end are faded out — video length wins.
pub fn mix(events: &[AudioEvent], duration_s: f64, master_volume: f32) -> Vec<f32> {
    let len = (duration_s * SAMPLE_RATE as f64).ceil() as usize;
    let mut buf = vec![0f32; len];

    for (i, ev) in events.iter().enumerate() {
        // Seed from the event's index and anchor so noise is stable across
        // runs but distinct across events.
        let seed = (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ ev.t.to_bits();
        let samples = render_recipe(&ev.recipe, ev.pitch, ev.gain * master_volume, seed);
        let start = (ev.t * SAMPLE_RATE as f64).round() as i64;
        for (j, s) in samples.iter().enumerate() {
            let idx = start + j as i64;
            if idx < 0 {
                continue;
            }
            let Ok(idx) = usize::try_from(idx) else { continue };
            if idx >= len {
                break;
            }
            buf[idx] += s;
        }
    }

    // Soft clip: linear below the knee, smooth compression above it, bounded
    // by ±1. tanh keeps it continuous and deterministic.
    for s in &mut buf {
        let a = s.abs();
        if a > KNEE {
            *s = s.signum() * (KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh());
        }
    }

    let fade = ((END_FADE_S * SAMPLE_RATE as f32) as usize).min(len);
    for k in 0..fade {
        let idx = len - fade + k;
        buf[idx] *= 1.0 - (k + 1) as f32 / fade as f32;
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipes::recipe;

    fn event(t: f64, name: &str, gain: f32) -> AudioEvent {
        AudioEvent { t, name: name.into(), recipe: recipe(name).unwrap(), gain, pitch: 1.0 }
    }

    #[test]
    fn buffer_length_matches_duration() {
        let buf = mix(&[], 2.5, 1.0);
        assert_eq!(buf.len(), (2.5 * SAMPLE_RATE as f64) as usize);
        assert!(buf.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn event_lands_at_its_timestamp() {
        let buf = mix(&[event(1.0, "tick", 1.0)], 2.0, 1.0);
        let before: f32 = buf[..SAMPLE_RATE as usize - 100].iter().map(|s| s.abs()).sum();
        let after: f32 = buf[SAMPLE_RATE as usize..].iter().map(|s| s.abs()).sum();
        assert_eq!(before, 0.0);
        assert!(after > 0.0);
    }

    #[test]
    fn mix_is_deterministic() {
        let evs = [event(0.1, "press", 1.0), event(0.5, "chime", 1.0)];
        assert_eq!(mix(&evs, 1.5, 0.5), mix(&evs, 1.5, 0.5));
    }

    #[test]
    fn coincident_events_never_exceed_full_scale() {
        let evs: Vec<AudioEvent> = (0..40).map(|_| event(0.2, "success", 3.0)).collect();
        let buf = mix(&evs, 1.0, 1.0);
        assert!(buf.iter().all(|s| s.abs() <= 1.0));
        assert!(buf.iter().any(|s| s.abs() > KNEE), "clipper engaged");
    }

    #[test]
    fn tail_past_the_end_is_truncated_with_fade() {
        // chime's shimmer tail is ~1s; place it 50ms before the end.
        let buf = mix(&[event(0.95, "chime", 1.0)], 1.0, 1.0);
        assert_eq!(buf.len(), SAMPLE_RATE as usize);
        assert_eq!(*buf.last().unwrap(), 0.0, "final sample fully faded");
    }

    #[test]
    fn master_volume_scales_output() {
        let loud = mix(&[event(0.1, "tick", 1.0)], 0.5, 1.0);
        let quiet = mix(&[event(0.1, "tick", 1.0)], 0.5, 0.25);
        let peak = |b: &[f32]| b.iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!((peak(&quiet) - peak(&loud) * 0.25).abs() < 0.01);
    }
}
