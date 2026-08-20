//! Offline synthesis: recipe → f32 samples at [`SAMPLE_RATE`](crate::SAMPLE_RATE).
//!
//! This mirrors what cuelume builds live with Web Audio nodes: oscillators
//! and white noise through biquad filters, exponential envelopes, and a
//! lowpass feedback delay for the shimmer tail. Everything is deterministic —
//! noise comes from a seeded PCG32, never the system RNG.

use crate::recipes::{FilterKind, Layer, NoiseLayer, Recipe, Shimmer, ToneLayer, Waveform};
use crate::SAMPLE_RATE;

/// Envelope floor, matching Web Audio's exponential-ramp convention of
/// ramping to/from a near-zero value instead of true zero.
const ENV_FLOOR: f32 = 1e-4;

/// cuelume boosts its recipe output before a limiter; folded in here so the
/// ported peak values land at a useful level.
const OUTPUT_GAIN: f32 = 4.0;

// ---------------------------------------------------------------------------
// Deterministic RNG (PCG32)
// ---------------------------------------------------------------------------

pub struct Pcg32 {
    state: u64,
}

impl Pcg32 {
    pub fn new(seed: u64) -> Self {
        let mut rng = Pcg32 { state: seed.wrapping_add(0x853c_49e6_748f_ea9b) };
        rng.next_u32();
        rng
    }

    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Uniform in [-1, 1).
    pub fn bipolar(&mut self) -> f32 {
        (self.next_u32() as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// Uniform in [lo, hi).
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (self.next_u32() as f32 / u32::MAX as f32) * (hi - lo)
    }
}

// ---------------------------------------------------------------------------
// Biquad filter (RBJ audio EQ cookbook)
// ---------------------------------------------------------------------------

struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    fn new(kind: FilterKind, freq: f32, q: f32) -> Self {
        let sr = SAMPLE_RATE as f32;
        let freq = freq.clamp(10.0, sr * 0.49);
        let q = q.max(0.05);
        let w0 = 2.0 * std::f32::consts::PI * freq / sr;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let (b0, b1, b2) = match kind {
            FilterKind::Lowpass => {
                let b1 = 1.0 - cos;
                (b1 / 2.0, b1, b1 / 2.0)
            }
            FilterKind::Highpass => {
                let b1 = -(1.0 + cos);
                ((1.0 + cos) / 2.0, b1, (1.0 + cos) / 2.0)
            }
            FilterKind::Bandpass => (alpha, 0.0, -alpha),
        };
        let a0 = 1.0 + alpha;
        Biquad {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: (-2.0 * cos) / a0,
            a2: (1.0 - alpha) / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

// ---------------------------------------------------------------------------
// Envelope
// ---------------------------------------------------------------------------

/// Exponential attack/decay envelope value at time `t` (seconds from layer
/// start): floor → peak over `attack`, peak → floor over `decay`.
fn envelope(t: f32, attack: f32, decay: f32, peak: f32) -> f32 {
    let peak = peak.max(ENV_FLOOR);
    if t < 0.0 {
        0.0
    } else if t < attack {
        ENV_FLOOR * (peak / ENV_FLOOR).powf(t / attack.max(1e-6))
    } else if t < attack + decay {
        peak * (ENV_FLOOR / peak).powf((t - attack) / decay.max(1e-6))
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// Layer rendering
// ---------------------------------------------------------------------------

fn render_tone(out: &mut [f32], layer: &ToneLayer, pitch: f32) {
    let sr = SAMPLE_RATE as f32;
    let start = (layer.offset * sr) as usize;
    let len = ((layer.attack + layer.decay) * sr).ceil() as usize;
    let detune = 2f32.powf(layer.detune / 1200.0);
    let f0 = layer.frequency * detune * pitch;
    let f1 = layer.glide_to.map(|f| f * detune * pitch);
    let glide_time = layer.glide_time.unwrap_or(layer.attack + layer.decay).max(1e-6);

    let mut phase = 0f32;
    for i in 0..len {
        let idx = start + i;
        if idx >= out.len() {
            break;
        }
        let t = i as f32 / sr;
        let freq = match f1 {
            // Exponential glide, like Web Audio's exponentialRampToValueAtTime.
            Some(f1) if t < glide_time => f0 * (f1 / f0).powf(t / glide_time),
            Some(f1) => f1,
            None => f0,
        };
        phase += 2.0 * std::f32::consts::PI * freq / sr;
        if phase > 2.0 * std::f32::consts::PI {
            phase -= 2.0 * std::f32::consts::PI;
        }
        let wave = match layer.waveform {
            Waveform::Sine => phase.sin(),
            Waveform::Triangle => (2.0 / std::f32::consts::PI) * phase.sin().asin(),
            Waveform::Square => {
                if phase.sin() >= 0.0 {
                    1.0
                } else {
                    -1.0
                }
            }
            Waveform::Saw => (phase / std::f32::consts::PI) - 1.0,
        };
        out[idx] += wave * envelope(t, layer.attack, layer.decay, layer.peak);
    }
}

fn render_noise(out: &mut [f32], layer: &NoiseLayer, pitch: f32, rng: &mut Pcg32) {
    let sr = SAMPLE_RATE as f32;
    let start = (layer.offset * sr) as usize;
    let len = ((layer.attack + layer.decay) * sr).ceil() as usize;
    let mut filter = Biquad::new(layer.filter, layer.frequency * pitch, layer.q);
    for i in 0..len {
        let idx = start + i;
        if idx >= out.len() {
            break;
        }
        let t = i as f32 / sr;
        let x = filter.process(rng.bipolar());
        out[idx] += x * envelope(t, layer.attack, layer.decay, layer.peak);
    }
}

/// Number of trailing samples the shimmer echo needs to fade below the floor.
fn shimmer_tail(shimmer: Option<Shimmer>) -> usize {
    let Some(s) = shimmer else { return 0 };
    if s.feedback <= 0.0 {
        return 0;
    }
    let repeats = if s.feedback >= 1.0 {
        1.0
    } else {
        1.0 + (ENV_FLOOR.ln() / s.feedback.ln()).ceil()
    };
    (s.delay * repeats * SAMPLE_RATE as f32).ceil() as usize
}

/// Renders a full recipe to mono samples. `pitch` scales every frequency
/// (1.0 = as written); `gain` scales the result; `seed` drives the noise so
/// two renders of the same event are bit-identical.
pub fn render_recipe(recipe: Recipe, pitch: f32, gain: f32, seed: u64) -> Vec<f32> {
    let sr = SAMPLE_RATE as f32;
    let source_len = recipe
        .layers
        .iter()
        .map(|l| {
            let (offset, attack, decay) = match l {
                Layer::Tone(t) => (t.offset, t.attack, t.decay),
                Layer::Noise(n) => (n.offset, n.attack, n.decay),
            };
            ((offset + attack + decay) * sr).ceil() as usize
        })
        .max()
        .unwrap_or(0);

    let mut dry = vec![0f32; source_len];
    let mut rng = Pcg32::new(seed);
    for layer in recipe.layers {
        match layer {
            Layer::Tone(t) => render_tone(&mut dry, t, pitch),
            Layer::Noise(n) => render_noise(&mut dry, n, pitch, &mut rng),
        }
    }

    let scale = recipe.master_gain * gain * OUTPUT_GAIN;
    match recipe.shimmer {
        None => {
            for s in &mut dry {
                *s *= scale;
            }
            dry
        }
        Some(sh) => {
            // cuelume's graph: dry → delay → lowpass → feedback back into the
            // delay, with the filtered signal also tapped to the output.
            let tail = shimmer_tail(Some(sh));
            let delay_len = ((sh.delay * sr) as usize).max(1);
            let mut ring = vec![0f32; delay_len];
            let mut lp = Biquad::new(FilterKind::Lowpass, sh.lowpass, 0.707);
            let mut out = vec![0f32; source_len + tail];
            let mut idx = 0usize;
            for (n, o) in out.iter_mut().enumerate() {
                let d = if n < source_len { dry[n] } else { 0.0 };
                let filtered = lp.process(ring[idx]);
                ring[idx] = d + filtered * sh.feedback;
                idx = (idx + 1) % delay_len;
                *o = (d + filtered * sh.wet) * scale;
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipes::recipe;

    #[test]
    fn render_is_deterministic() {
        let r = recipe("tick").unwrap();
        let a = render_recipe(r, 1.0, 1.0, 42);
        let b = render_recipe(r, 1.0, 1.0, 42);
        assert_eq!(a, b);
    }

    #[test]
    fn different_seeds_differ_for_noise() {
        let r = recipe("press").unwrap();
        let a = render_recipe(r, 1.0, 1.0, 1);
        let b = render_recipe(r, 1.0, 1.0, 2);
        assert_ne!(a, b);
    }

    #[test]
    fn shimmer_extends_the_tail() {
        let plain = render_recipe(recipe("press").unwrap(), 1.0, 1.0, 0);
        let shimmered = render_recipe(recipe("chime").unwrap(), 1.0, 1.0, 0);
        // chime's source is ~0.35s; its shimmer tail pushes well past that.
        assert!(shimmered.len() as f32 / (SAMPLE_RATE as f32) > 0.8);
        assert!(plain.len() as f32 / (SAMPLE_RATE as f32) < 0.1);
    }

    #[test]
    fn output_is_audible_but_not_clipping() {
        for name in crate::recipes::recipe_names() {
            let samples = render_recipe(recipe(name).unwrap(), 1.0, 1.0, 7);
            let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
            assert!(peak > 0.01, "`{name}` is inaudible (peak {peak})");
            assert!(peak < 1.0, "`{name}` clips (peak {peak})");
        }
    }

    #[test]
    fn pitch_shifts_the_spectrum() {
        let r = recipe("chime").unwrap();
        let lo = render_recipe(r, 0.5, 1.0, 0);
        let hi = render_recipe(r, 2.0, 1.0, 0);
        // Rough spectral proxy: zero crossings per second.
        let crossings = |s: &[f32]| s.windows(2).filter(|w| w[0].signum() != w[1].signum()).count();
        assert!(crossings(&hi) > crossings(&lo) * 2);
    }

    #[test]
    fn envelope_shape_rises_then_falls() {
        assert!(envelope(0.0, 0.01, 0.1, 0.5) < 0.001);
        let at_peak = envelope(0.01, 0.01, 0.1, 0.5);
        assert!((at_peak - 0.5).abs() < 0.01, "peak {at_peak}");
        assert!(envelope(0.06, 0.01, 0.1, 0.5) < at_peak);
        assert_eq!(envelope(0.2, 0.01, 0.1, 0.5), 0.0);
    }
}
