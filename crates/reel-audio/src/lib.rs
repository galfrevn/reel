//! Procedural audio for reel.
//!
//! Everything here follows the spec's core rule (§8.1): **audio is an event
//! list with timestamps, mixed after the timeline is resolved — never a
//! pre-rendered waveform.** Speeding up a region drops events instead of
//! pitch-shifting them; keys still sound like keys, there are just fewer.
//!
//! There are no audio files anywhere. Every sound is a *recipe* — tone and
//! filtered-noise layers with envelopes plus an optional shimmer tail —
//! synthesized offline into the mix buffer. The recipe model and the built-in
//! palette are ported from [cuelume](https://github.com/Danilaa1/cuelume)
//! (MIT, © Daniel Belyi), which proved the approach in the browser with the
//! Web Audio API. Determinism is a feature: same input, same bytes, on every
//! machine.

mod dsp;
mod events;
mod mix;
mod recipes;
mod soundfile;

pub use events::{plan_events, AudioEvent, AudioPlan, GridChange, KeyKind, KeyInput, PlanConfig};
pub use mix::mix;
pub use recipes::{
    keyboard_profile, keyboard_profile_names, recipe, recipe_names, KeyboardProfile, Recipe,
};
pub use soundfile::{sound_from_toml, sound_to_toml, SoundFile, SOUND_SCHEMA};

/// Everything is rendered at this rate, mono.
pub const SAMPLE_RATE: u32 = 48_000;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("unknown sound `{0}` (available: {1})")]
    UnknownSound(String, String),
    #[error("unknown keyboard profile `{0}` (available: {1})")]
    UnknownProfile(String, String),
}

/// Renders a single named recipe to samples — the building block `mix`
/// composes, exposed for tests and for auditioning sounds.
pub fn render_sound(name: &str, pitch: f32, gain: f32, seed: u64) -> Result<Vec<f32>, AudioError> {
    let r = recipes::recipe(name).ok_or_else(|| {
        AudioError::UnknownSound(name.to_string(), recipes::recipe_names().join(", "))
    })?;
    Ok(dsp::render_recipe(&r, pitch, gain, seed))
}

/// Renders any recipe (builtin or loaded from TOML) to samples.
pub fn render_recipe(recipe: &Recipe, pitch: f32, gain: f32, seed: u64) -> Vec<f32> {
    dsp::render_recipe(recipe, pitch, gain, seed)
}
