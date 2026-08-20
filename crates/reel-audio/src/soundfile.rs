//! The sound TOML format: one recipe per file, the shape `reel audio
//! add/publish` trade in. Mirrors the template TOML conventions (schema
//! stamp, name from the file stem, description for search/gallery).

use crate::recipes::{FilterKind, Layer, NoiseLayer, Recipe, Shimmer, ToneLayer, Waveform};
use std::borrow::Cow;

/// The sound TOML schema this reel reads and writes.
pub const SOUND_SCHEMA: u32 = 1;

/// A parsed sound file: the recipe plus its registry-facing metadata.
#[derive(Debug, Clone)]
pub struct SoundFile {
    pub name: String,
    pub description: String,
    pub recipe: Recipe,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRepr {
    /// Checked against SOUND_SCHEMA before deserialization; kept here so
    /// `deny_unknown_fields` accepts the stamp.
    #[allow(dead_code)]
    schema: Option<u32>,
    name: Option<String>,
    #[serde(default)]
    description: String,
    gain: f32,
    layers: Vec<LayerRepr>,
    shimmer: Option<ShimmerRepr>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct LayerRepr {
    /// "tone" | "noise"
    kind: String,
    #[serde(default)]
    offset: f32,
    attack: f32,
    decay: f32,
    peak: f32,
    frequency: f32,
    /// Tone only: "sine" | "triangle" | "square" | "saw".
    waveform: Option<String>,
    /// Tone only: detune in cents.
    detune: Option<f32>,
    /// Tone only: glide the pitch to this frequency…
    glide_to: Option<f32>,
    /// …over this many seconds (defaults to attack + decay).
    glide_time: Option<f32>,
    /// Noise only: "lowpass" | "bandpass" | "highpass".
    filter: Option<String>,
    /// Noise only: filter resonance.
    q: Option<f32>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ShimmerRepr {
    delay: f32,
    feedback: f32,
    wet: f32,
    lowpass: f32,
}

fn check(cond: bool, what: &str) -> Result<(), String> {
    if cond {
        Ok(())
    } else {
        Err(what.to_string())
    }
}

/// Bounds that keep a community recipe a *cue* rather than a weapon: short,
/// mixable, and within audible/encodable ranges.
fn validate_layer(i: usize, l: &LayerRepr) -> Result<(), String> {
    let ctx = |m: &str| format!("layer {}: {m}", i + 1);
    check(l.offset >= 0.0 && l.offset <= 10.0, &ctx("`offset` must be 0..=10 seconds"))?;
    check(l.attack >= 0.0 && l.attack <= 10.0, &ctx("`attack` must be 0..=10 seconds"))?;
    check(l.decay > 0.0 && l.decay <= 10.0, &ctx("`decay` must be >0 and <=10 seconds"))?;
    check(l.peak > 0.0 && l.peak <= 1.0, &ctx("`peak` must be >0 and <=1"))?;
    check(
        (20.0..=20_000.0).contains(&l.frequency),
        &ctx("`frequency` must be 20..=20000 Hz"),
    )?;
    Ok(())
}

/// Parses and validates a sound TOML.
pub fn sound_from_toml(text: &str, fallback_name: &str) -> Result<SoundFile, String> {
    // Schema gate first: "upgrade reel" beats "unknown field" for files
    // written against a newer schema.
    let value: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
    if let Some(s) = value.get("schema").and_then(|v| v.as_integer()) {
        if s > SOUND_SCHEMA as i64 {
            return Err(format!(
                "sound schema {s} is newer than this reel understands \
                 (schema {SOUND_SCHEMA}) — upgrade reel"
            ));
        }
    }
    let f: FileRepr = value.try_into().map_err(|e: toml::de::Error| e.to_string())?;
    check(f.gain > 0.0 && f.gain <= 2.0, "`gain` must be >0 and <=2")?;
    check(
        !f.layers.is_empty() && f.layers.len() <= 32,
        "a sound needs 1..=32 `[[layers]]`",
    )?;

    let mut layers = Vec::with_capacity(f.layers.len());
    for (i, l) in f.layers.iter().enumerate() {
        validate_layer(i, l)?;
        let layer = match l.kind.as_str() {
            "tone" => {
                let waveform = match l.waveform.as_deref().unwrap_or("sine") {
                    "sine" => Waveform::Sine,
                    "triangle" => Waveform::Triangle,
                    "square" => Waveform::Square,
                    "saw" => Waveform::Saw,
                    other => return Err(format!("layer {}: unknown waveform `{other}`", i + 1)),
                };
                if let Some(g) = l.glide_to {
                    check(
                        (20.0..=20_000.0).contains(&g),
                        &format!("layer {}: `glide_to` must be 20..=20000 Hz", i + 1),
                    )?;
                }
                Layer::Tone(ToneLayer {
                    offset: l.offset,
                    attack: l.attack,
                    decay: l.decay,
                    peak: l.peak,
                    waveform,
                    frequency: l.frequency,
                    detune: l.detune.unwrap_or(0.0),
                    glide_to: l.glide_to,
                    glide_time: l.glide_time,
                })
            }
            "noise" => {
                let filter = match l.filter.as_deref().unwrap_or("lowpass") {
                    "lowpass" => FilterKind::Lowpass,
                    "bandpass" => FilterKind::Bandpass,
                    "highpass" => FilterKind::Highpass,
                    other => return Err(format!("layer {}: unknown filter `{other}`", i + 1)),
                };
                Layer::Noise(NoiseLayer {
                    offset: l.offset,
                    attack: l.attack,
                    decay: l.decay,
                    peak: l.peak,
                    filter,
                    frequency: l.frequency,
                    q: l.q.unwrap_or(1.0).clamp(0.1, 12.0),
                })
            }
            other => return Err(format!("layer {}: `kind` must be tone|noise, got `{other}`", i + 1)),
        };
        layers.push(layer);
    }

    let shimmer = f.shimmer.map(|s| Shimmer {
        delay: s.delay.clamp(0.0, 1.0),
        feedback: s.feedback.clamp(0.0, 0.9),
        wet: s.wet.clamp(0.0, 1.0),
        lowpass: s.lowpass.clamp(100.0, 20_000.0),
    });

    Ok(SoundFile {
        name: f.name.unwrap_or_else(|| fallback_name.to_string()),
        description: f.description,
        recipe: Recipe { master_gain: f.gain, layers: Cow::Owned(layers), shimmer },
    })
}

/// Serializes a recipe as the TOML `sound_from_toml` reads — so any builtin
/// doubles as a starting point (`reel audio show chime > mine.toml`).
pub fn sound_to_toml(name: &str, description: &str, recipe: &Recipe) -> String {
    let mut out = String::new();
    out.push_str(&format!("schema = {SOUND_SCHEMA}\n"));
    out.push_str(&format!("name = {name:?}\n"));
    out.push_str(&format!("description = {description:?}\n"));
    out.push_str(&format!("gain = {}\n", recipe.master_gain));
    for l in recipe.layers.iter() {
        out.push_str("\n[[layers]]\n");
        match l {
            Layer::Tone(t) => {
                out.push_str("kind = \"tone\"\n");
                out.push_str(&format!("offset = {}\n", t.offset));
                out.push_str(&format!("attack = {}\n", t.attack));
                out.push_str(&format!("decay = {}\n", t.decay));
                out.push_str(&format!("peak = {}\n", t.peak));
                let wf = match t.waveform {
                    Waveform::Sine => "sine",
                    Waveform::Triangle => "triangle",
                    Waveform::Square => "square",
                    Waveform::Saw => "saw",
                };
                out.push_str(&format!("waveform = \"{wf}\"\n"));
                out.push_str(&format!("frequency = {}\n", t.frequency));
                if t.detune != 0.0 {
                    out.push_str(&format!("detune = {}\n", t.detune));
                }
                if let Some(g) = t.glide_to {
                    out.push_str(&format!("glide_to = {g}\n"));
                }
                if let Some(g) = t.glide_time {
                    out.push_str(&format!("glide_time = {g}\n"));
                }
            }
            Layer::Noise(n) => {
                out.push_str("kind = \"noise\"\n");
                out.push_str(&format!("offset = {}\n", n.offset));
                out.push_str(&format!("attack = {}\n", n.attack));
                out.push_str(&format!("decay = {}\n", n.decay));
                out.push_str(&format!("peak = {}\n", n.peak));
                let filter = match n.filter {
                    FilterKind::Lowpass => "lowpass",
                    FilterKind::Bandpass => "bandpass",
                    FilterKind::Highpass => "highpass",
                };
                out.push_str(&format!("filter = \"{filter}\"\n"));
                out.push_str(&format!("frequency = {}\n", n.frequency));
                out.push_str(&format!("q = {}\n", n.q));
            }
        }
    }
    if let Some(s) = &recipe.shimmer {
        out.push_str("\n[shimmer]\n");
        out.push_str(&format!("delay = {}\n", s.delay));
        out.push_str(&format!("feedback = {}\n", s.feedback));
        out.push_str(&format!("wet = {}\n", s.wet));
        out.push_str(&format!("lowpass = {}\n", s.lowpass));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_roundtrips_through_toml() {
        for name in crate::recipes::recipe_names() {
            let r = crate::recipes::recipe(name).unwrap();
            let text = sound_to_toml(name, "a test sound", &r);
            let back = sound_from_toml(&text, "x").unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(back.name, name);
            assert_eq!(back.recipe.layers.len(), r.layers.len(), "{name}");
            assert_eq!(back.recipe.master_gain, r.master_gain, "{name}");
            assert_eq!(back.recipe.shimmer.is_some(), r.shimmer.is_some(), "{name}");
        }
    }

    #[test]
    fn validation_catches_out_of_range_values() {
        let bad_gain = "gain = 9\n[[layers]]\nkind = \"tone\"\nattack = 0.01\ndecay = 0.1\npeak = 0.5\nfrequency = 440\n";
        assert!(sound_from_toml(bad_gain, "x").unwrap_err().contains("gain"));

        let bad_freq = "gain = 0.5\n[[layers]]\nkind = \"tone\"\nattack = 0.01\ndecay = 0.1\npeak = 0.5\nfrequency = 5\n";
        assert!(sound_from_toml(bad_freq, "x").unwrap_err().contains("frequency"));

        let no_layers = "gain = 0.5\nlayers = []\n";
        assert!(sound_from_toml(no_layers, "x").unwrap_err().contains("layers"));
    }

    #[test]
    fn newer_schema_says_upgrade() {
        let err = sound_from_toml("schema = 99\n", "x").unwrap_err();
        assert!(err.contains("upgrade reel"), "got: {err}");
    }

    #[test]
    fn name_falls_back_to_the_file_stem() {
        let text = "gain = 0.5\n[[layers]]\nkind = \"noise\"\nattack = 0.001\ndecay = 0.02\npeak = 0.1\nfrequency = 2000\n";
        let s = sound_from_toml(text, "thud").unwrap();
        assert_eq!(s.name, "thud");
        assert!(matches!(s.recipe.layers[0], Layer::Noise(_)));
    }
}
