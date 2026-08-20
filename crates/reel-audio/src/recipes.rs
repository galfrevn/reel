//! The sound palette: recipe types and the built-in recipes, ported from
//! cuelume (https://github.com/Danilaa1/cuelume, MIT © Daniel Belyi) with
//! reel-specific additions for keyboards and the agent-thinking bed.
//!
//! A recipe is declarative data. Adding a sound means adding an entry here —
//! no synthesis code changes.

/// Oscillator shape for tone layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waveform {
    Sine,
    Triangle,
    Square,
    Saw,
}

/// Biquad filter shape for noise layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterKind {
    Lowpass,
    Bandpass,
    Highpass,
}

/// A single note — the building block for chimes, arpeggios, and pads.
/// Times are seconds, `peak` is linear gain.
#[derive(Debug, Clone, Copy)]
pub struct ToneLayer {
    pub offset: f32,
    pub attack: f32,
    pub decay: f32,
    pub peak: f32,
    pub waveform: Waveform,
    pub frequency: f32,
    /// Detune in cents, for a gentle chorus/beating effect between layers.
    pub detune: f32,
    /// If set, pitch glides exponentially from `frequency` to this value.
    pub glide_to: Option<f32>,
    /// Glide duration; `None` means attack + decay.
    pub glide_time: Option<f32>,
}

/// A filtered white-noise burst — used for breathy or percussive layers.
#[derive(Debug, Clone, Copy)]
pub struct NoiseLayer {
    pub offset: f32,
    pub attack: f32,
    pub decay: f32,
    pub peak: f32,
    pub filter: FilterKind,
    pub frequency: f32,
    pub q: f32,
}

#[derive(Debug, Clone, Copy)]
pub enum Layer {
    Tone(ToneLayer),
    Noise(NoiseLayer),
}

/// A soft, spacious echo tail applied to the whole sound.
#[derive(Debug, Clone, Copy)]
pub struct Shimmer {
    pub delay: f32,
    pub feedback: f32,
    pub wet: f32,
    pub lowpass: f32,
}

#[derive(Debug, Clone)]
pub struct Recipe {
    pub master_gain: f32,
    /// Borrowed for the built-in table; owned for recipes loaded from TOML.
    pub layers: std::borrow::Cow<'static, [Layer]>,
    pub shimmer: Option<Shimmer>,
}

const fn tone(
    offset: f32,
    attack: f32,
    decay: f32,
    peak: f32,
    waveform: Waveform,
    frequency: f32,
) -> Layer {
    Layer::Tone(ToneLayer {
        offset,
        attack,
        decay,
        peak,
        waveform,
        frequency,
        detune: 0.0,
        glide_to: None,
        glide_time: None,
    })
}

#[expect(clippy::too_many_arguments, reason = "mirrors the recipe table's column order")]
const fn glide(
    offset: f32,
    attack: f32,
    decay: f32,
    peak: f32,
    waveform: Waveform,
    frequency: f32,
    to: f32,
    time: f32,
) -> Layer {
    Layer::Tone(ToneLayer {
        offset,
        attack,
        decay,
        peak,
        waveform,
        frequency,
        detune: 0.0,
        glide_to: Some(to),
        glide_time: Some(time),
    })
}

const fn noise(
    offset: f32,
    attack: f32,
    decay: f32,
    peak: f32,
    filter: FilterKind,
    frequency: f32,
    q: f32,
) -> Layer {
    Layer::Noise(NoiseLayer { offset, attack, decay, peak, filter, frequency, q })
}

use FilterKind::{Bandpass, Lowpass};
use Waveform::{Sine, Square, Triangle};

macro_rules! recipes {
    ($( $name:literal => { gain: $gain:expr, layers: $layers:expr, shimmer: $shimmer:expr } ),+ $(,)?) => {
        static RECIPE_NAMES: &[&str] = &[$($name),+];

        /// Looks up a built-in recipe by name.
        pub fn recipe(name: &str) -> Option<Recipe> {
            match name {
                $($name => {
                    // A const item promotes the layer array to 'static.
                    const LAYERS: &[Layer] = $layers;
                    Some(Recipe {
                        master_gain: $gain,
                        layers: std::borrow::Cow::Borrowed(LAYERS),
                        shimmer: $shimmer,
                    })
                })+
                _ => None,
            }
        }
    };
}

recipes! {
    // A soft two-note ascending bell, like a macOS confirmation tink.
    "chime" => {
        gain: 0.5,
        layers: &[
            tone(0.0, 0.006, 0.22, 0.09, Sine, 1046.5),
            tone(0.09, 0.006, 0.26, 0.08, Sine, 1568.0),
        ],
        shimmer: Some(Shimmer { delay: 0.12, feedback: 0.25, wet: 0.18, lowpass: 4000.0 })
    },
    // A quick ascending twinkle of four notes — bright and playful.
    "sparkle" => {
        gain: 0.5,
        layers: &[
            tone(0.0, 0.003, 0.09, 0.045, Sine, 1760.0),
            tone(0.045, 0.003, 0.09, 0.04, Sine, 2217.0),
            tone(0.09, 0.003, 0.10, 0.038, Sine, 2637.0),
            tone(0.135, 0.003, 0.12, 0.032, Sine, 3520.0),
        ],
        shimmer: Some(Shimmer { delay: 0.07, feedback: 0.35, wet: 0.22, lowpass: 6000.0 })
    },
    // A single note gliding smoothly downward, like a drop of water.
    "droplet" => {
        gain: 0.55,
        layers: &[glide(0.0, 0.004, 0.2, 0.075, Sine, 1200.0, 550.0, 0.14)],
        shimmer: Some(Shimmer { delay: 0.09, feedback: 0.2, wet: 0.15, lowpass: 3000.0 })
    },
    // A warm, slow-swelling pad from two gently detuned sines.
    "bloom" => {
        gain: 0.5,
        layers: &[
            tone(0.0, 0.06, 0.32, 0.06, Sine, 528.0),
            Layer::Tone(ToneLayer {
                offset: 0.0, attack: 0.06, decay: 0.34, peak: 0.05,
                waveform: Sine, frequency: 528.0, detune: 12.0,
                glide_to: None, glide_time: None,
            }),
        ],
        shimmer: Some(Shimmer { delay: 0.15, feedback: 0.2, wet: 0.12, lowpass: 2500.0 })
    },
    // A soft hush with a falling tone — low-priority cues.
    "whisper" => {
        gain: 0.48,
        layers: &[
            noise(0.0, 0.025, 0.13, 0.04, Lowpass, 1600.0, 0.7),
            glide(0.01, 0.012, 0.14, 0.025, Sine, 880.0, 660.0, 0.14),
        ],
        shimmer: None
    },
    // A focused tick with a bright sine ping on top — crisp and instant.
    "tick" => {
        gain: 0.4,
        layers: &[
            noise(0.0, 0.001, 0.018, 0.14, Bandpass, 5400.0, 1.8),
            tone(0.0, 0.001, 0.012, 0.018, Sine, 2600.0),
        ],
        shimmer: None
    },
    // A dull, muted knock — a key bottoming out.
    "press" => {
        gain: 0.4,
        layers: &[noise(0.0, 0.001, 0.02, 0.13, Bandpass, 1700.0, 1.4)],
        shimmer: None
    },
    // A brighter, springier tick — a key returning.
    "release" => {
        gain: 0.4,
        layers: &[
            noise(0.0, 0.001, 0.016, 0.12, Bandpass, 4600.0, 1.8),
            tone(0.006, 0.001, 0.05, 0.02, Sine, 3200.0),
        ],
        shimmer: None
    },
    // A two-part click-clack, like a mechanical switch flipping.
    "toggle" => {
        gain: 0.4,
        layers: &[
            noise(0.0, 0.001, 0.016, 0.12, Bandpass, 2200.0, 1.6),
            noise(0.024, 0.001, 0.02, 0.1, Bandpass, 3800.0, 1.6),
        ],
        shimmer: None
    },
    // A short, warm three-note ascending confirmation — "done", not a fanfare.
    "success" => {
        gain: 0.5,
        layers: &[
            tone(0.0, 0.004, 0.09, 0.06, Sine, 880.0),
            tone(0.06, 0.004, 0.10, 0.06, Sine, 1108.73),
            tone(0.12, 0.004, 0.18, 0.07, Sine, 1318.51),
        ],
        shimmer: Some(Shimmer { delay: 0.1, feedback: 0.22, wet: 0.16, lowpass: 4500.0 })
    },
    // A muted knock followed by two descending tones — a calm refusal.
    "error" => {
        gain: 0.42,
        layers: &[
            noise(0.0, 0.001, 0.035, 0.13, Bandpass, 850.0, 1.1),
            tone(0.025, 0.004, 0.09, 0.045, Triangle, 440.0),
            tone(0.1, 0.004, 0.14, 0.04, Triangle, 349.23),
        ],
        shimmer: None
    },
    // A papery filtered flick with a tiny glass tick — pages and scrolls.
    "page" => {
        gain: 0.38,
        layers: &[
            noise(0.0, 0.006, 0.08, 0.11, Lowpass, 1800.0, 0.7),
            noise(0.04, 0.004, 0.065, 0.08, Bandpass, 4200.0, 1.2),
            tone(0.075, 0.002, 0.045, 0.02, Sine, 2400.0),
        ],
        shimmer: None
    },
    // A brief unresolved lift — user-initiated work has started.
    "loading" => {
        gain: 0.42,
        layers: &[
            noise(0.0, 0.035, 0.14, 0.035, Lowpass, 1400.0, 0.6),
            glide(0.0, 0.025, 0.18, 0.05, Sine, 420.0, 630.0, 0.18),
        ],
        shimmer: Some(Shimmer { delay: 0.11, feedback: 0.18, wet: 0.12, lowpass: 2800.0 })
    },
    // A quick lock-on sweep resolving to a clear tone — the system is ready.
    "ready" => {
        gain: 0.48,
        layers: &[
            noise(0.0, 0.001, 0.02, 0.11, Bandpass, 3600.0, 1.8),
            glide(0.012, 0.004, 0.16, 0.055, Triangle, 330.0, 660.0, 0.12),
            tone(0.13, 0.004, 0.22, 0.06, Sine, 990.0),
        ],
        shimmer: Some(Shimmer { delay: 0.1, feedback: 0.16, wet: 0.1, lowpass: 4200.0 })
    },
    // A compact synthetic chirp — crisp feedback for a UI response.
    "pulse" => {
        gain: 0.42,
        layers: &[
            noise(0.0, 0.001, 0.022, 0.08, Bandpass, 2600.0, 2.4),
            glide(0.0, 0.002, 0.085, 0.055, Triangle, 620.0, 1240.0, 0.07),
        ],
        shimmer: None
    },
    // A fast three-step locator signal — playful menu feedback.
    "scan" => {
        gain: 0.4,
        layers: &[
            tone(0.0, 0.002, 0.055, 0.05, Sine, 740.0),
            tone(0.045, 0.002, 0.055, 0.045, Sine, 1110.0),
            tone(0.09, 0.002, 0.07, 0.04, Sine, 1665.0),
        ],
        shimmer: Some(Shimmer { delay: 0.065, feedback: 0.16, wet: 0.1, lowpass: 4200.0 })
    },
    // A rising harmonic portal with a soft tail — arrivals.
    "arrival" => {
        gain: 0.44,
        layers: &[
            noise(0.0, 0.05, 0.24, 0.035, Lowpass, 900.0, 0.8),
            glide(0.0, 0.04, 0.34, 0.055, Sine, 220.0, 440.0, 0.32),
            tone(0.12, 0.045, 0.32, 0.04, Sine, 659.25),
            tone(0.19, 0.045, 0.34, 0.032, Sine, 987.77),
        ],
        shimmer: Some(Shimmer { delay: 0.16, feedback: 0.28, wet: 0.18, lowpass: 3200.0 })
    },
    // reel addition: one beat of the agent-thinking bed — a low, breathing
    // pulse that reads as "working" without demanding attention.
    "soft-pulse" => {
        gain: 0.4,
        layers: &[
            tone(0.0, 0.12, 0.5, 0.035, Sine, 110.0),
            tone(0.0, 0.14, 0.48, 0.02, Sine, 220.0),
            noise(0.0, 0.1, 0.4, 0.012, Lowpass, 600.0, 0.7),
        ],
        shimmer: None
    },
}

/// All built-in sound names, for error messages and docs.
pub fn recipe_names() -> Vec<&'static str> {
    RECIPE_NAMES.to_vec()
}

// ---------------------------------------------------------------------------
// Keyboard profiles
// ---------------------------------------------------------------------------

/// A keyboard profile shapes the press/release pair per keystroke. The base
/// recipes are re-tuned per profile via these multipliers rather than
/// duplicating recipe data. Humanization (±3% pitch, ±15% gain, round-robin)
/// is applied per event by the planner, seeded so output stays deterministic.
#[derive(Debug, Clone, Copy)]
pub struct KeyboardProfile {
    pub name: &'static str,
    /// Frequency multiplier for the press knock (1.0 = mx-brown's 1700Hz).
    pub press_pitch: f32,
    /// Frequency multiplier for the release tick.
    pub release_pitch: f32,
    pub gain: f32,
    /// Whether the release tick plays at all (topre/laptop are single-shot).
    pub release: bool,
    /// Extra click layer on press (mx-blue's tactile click).
    pub click: bool,
}

static PROFILES: &[KeyboardProfile] = &[
    KeyboardProfile { name: "mx-brown", press_pitch: 1.0, release_pitch: 1.0, gain: 1.0, release: true, click: false },
    KeyboardProfile { name: "mx-red", press_pitch: 0.88, release_pitch: 0.95, gain: 0.8, release: true, click: false },
    KeyboardProfile { name: "mx-blue", press_pitch: 1.25, release_pitch: 1.15, gain: 1.05, release: true, click: true },
    KeyboardProfile { name: "topre", press_pitch: 0.62, release_pitch: 0.8, gain: 0.95, release: false, click: false },
    KeyboardProfile { name: "laptop", press_pitch: 1.45, release_pitch: 1.3, gain: 0.55, release: false, click: false },
    KeyboardProfile { name: "typewriter", press_pitch: 1.9, release_pitch: 2.2, gain: 1.25, release: true, click: true },
    KeyboardProfile { name: "buckling-spring", press_pitch: 1.55, release_pitch: 1.75, gain: 1.3, release: true, click: true },
];

pub fn keyboard_profile(name: &str) -> Option<KeyboardProfile> {
    PROFILES.iter().find(|p| p.name == name).copied()
}

pub fn keyboard_profile_names() -> Vec<&'static str> {
    PROFILES.iter().map(|p| p.name).collect()
}

// Silence the "unused" lint for waveforms recipes don't use yet: Square and
// Saw are part of the model so user themes/templates can reference them.
#[allow(dead_code)]
const _WAVEFORMS: &[Waveform] = &[Sine, Triangle, Square, Waveform::Saw];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_recipe_resolves() {
        for name in recipe_names() {
            assert!(recipe(name).is_some(), "recipe `{name}` missing");
        }
    }

    #[test]
    fn unknown_recipe_is_none() {
        assert!(recipe("nope").is_none());
    }

    #[test]
    fn spec_profiles_exist() {
        for name in ["mx-brown", "mx-red", "mx-blue", "topre", "laptop", "typewriter", "buckling-spring"] {
            assert!(keyboard_profile(name).is_some(), "profile `{name}` missing");
        }
        assert!(keyboard_profile("none").is_none(), "`none` is handled by the planner, not a profile");
    }
}
