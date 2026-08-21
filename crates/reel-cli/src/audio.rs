//! `reel audio …`: list, audition, install, and search sound recipes.
//!
//! Sounds are the third registry citizen next to templates (and the themes
//! they embed): a pack repo may carry a `sounds/` directory of recipe TOML
//! files. Installed recipes land in the user sounds dir and resolve by name
//! anywhere a built-in does (`sound "name"`, `thinking`, `bed`).

use anyhow::{anyhow, bail, Context, Result};
use reel_audio::{sound_from_toml, sound_to_toml, Recipe, SoundFile};
use std::collections::HashMap;
use std::path::Path;

/// Loads every installed sound (user sounds dir). Invalid files are skipped
/// with a warning on stderr rather than failing the render — a broken
/// community sound must not brick every demo on the machine.
pub fn installed_sounds() -> HashMap<String, Recipe> {
    let mut out = HashMap::new();
    let Some(dir) = reel_render::paths::sounds_dir() else { return out };
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "toml") {
            continue;
        }
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("sound");
        match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|t| sound_from_toml(&t, stem)) {
            Ok(s) => {
                out.insert(s.name, s.recipe);
            }
            Err(err) => eprintln!("warning: skipping sound {}: {err}", p.display()),
        }
    }
    out
}

pub fn list() -> Result<()> {
    let mut installed: Vec<String> = installed_sounds().into_keys().collect();
    installed.sort();
    if crate::json::on() {
        let mut out: Vec<serde_json::Value> = reel_audio::recipe_names()
            .iter()
            .map(|n| serde_json::json!({ "name": n, "source": "builtin" }))
            .collect();
        out.extend(
            installed
                .iter()
                .map(|n| serde_json::json!({ "name": n, "source": "installed" })),
        );
        return crate::json::emit(serde_json::json!({ "sounds": out }));
    }
    for name in reel_audio::recipe_names() {
        println!("{name}");
    }
    for name in installed {
        println!("{name} (installed)");
    }
    Ok(())
}

/// Prints a recipe as TOML — any builtin doubles as a starting point:
/// `reel audio show chime > my-sound.toml`.
pub fn show(name: &str) -> Result<()> {
    if let Some(r) = reel_audio::recipe(name) {
        print!("{}", sound_to_toml(name, "", &r));
        return Ok(());
    }
    if let Some(r) = installed_sounds().get(name) {
        print!("{}", sound_to_toml(name, "", r));
        return Ok(());
    }
    bail!("unknown sound `{name}` — see `reel audio list`");
}

/// Resolves a name, an installed sound, or a local .toml file to a recipe.
fn resolve(source: &str) -> Result<SoundFile> {
    let path = Path::new(source);
    if path.extension().is_some_and(|e| e == "toml") && path.exists() {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("sound");
        return sound_from_toml(&text, stem).map_err(|e| anyhow!("invalid sound: {e}"));
    }
    if let Some(r) = reel_audio::recipe(source) {
        return Ok(SoundFile { name: source.into(), description: String::new(), recipe: r });
    }
    if let Some(r) = installed_sounds().remove(source) {
        return Ok(SoundFile { name: source.into(), description: String::new(), recipe: r });
    }
    bail!("`{source}` is neither a sound name nor a .toml file — see `reel audio list`");
}

/// `reel audio try <name|file>`: synthesize the sound to a WAV and open it.
/// With `out`, just write the file (what the gallery build uses).
pub fn try_sound(source: &str, out: Option<std::path::PathBuf>) -> Result<()> {
    let s = resolve(source)?;
    let samples = reel_audio::render_recipe(&s.recipe, 1.0, 1.0, 42);
    if samples.is_empty() {
        bail!("`{}` rendered zero samples", s.name);
    }
    let audition = out.is_none();
    let path = match out {
        Some(p) => p,
        None => {
            let dir = std::env::temp_dir().join("reel-audio-try");
            std::fs::create_dir_all(&dir)?;
            dir.join(format!("{}.wav", s.name))
        }
    };
    std::fs::write(&path, wav_bytes(&samples))?;
    if crate::json::on() {
        return crate::json::emit(serde_json::json!({
            "sound": s.name,
            "output": path.display().to_string(),
            "samples": samples.len(),
            "sample_rate": reel_audio::SAMPLE_RATE,
        }));
    }
    println!("{}", path.display());
    if audition {
        crate::templates::open_preview(&path);
    }
    Ok(())
}

/// Mono 16-bit PCM WAV at the synthesis rate — small enough to write by hand.
pub fn wav_bytes(samples: &[f32]) -> Vec<u8> {
    let sr = reel_audio::SAMPLE_RATE;
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sr.to_le_bytes());
    out.extend_from_slice(&(sr * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    out
}

pub fn add(source: &str) -> Result<()> {
    let path = Path::new(source);
    if path.exists() {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("sound");
        let installed = install(&text, stem)?;
        println!("installed `{installed}` from {}", path.display());
        return Ok(());
    }
    match crate::packs::parse_source(source) {
        Some((owner, repo, None)) => {
            let mut installed = Vec::new();
            for (stem, text) in crate::packs::fetch_all(owner, repo, "sounds")? {
                installed.push(install(&text, &stem)?);
            }
            println!("installed from {owner}/{repo}: {}", installed.join(", "));
            Ok(())
        }
        Some((owner, repo, Some(name))) => {
            let text = crate::packs::fetch_one(owner, repo, "sounds", name)?;
            let installed = install(&text, name)?;
            println!("installed `{installed}` from {owner}/{repo}");
            Ok(())
        }
        None => bail!(
            "`{source}` is neither a local .toml file nor owner/repo[/name] — \
             try `reel audio search` to find sounds"
        ),
    }
}

/// Validates and writes one sound; returns the installed name. Built-in
/// names are reserved: they always win at resolution, so installing under
/// one would only create confusion.
fn install(text: &str, fallback_name: &str) -> Result<String> {
    let s = sound_from_toml(text, fallback_name).map_err(|e| anyhow!("invalid sound: {e}"))?;
    if reel_audio::recipe(&s.name).is_some() {
        bail!("`{}` is a built-in sound name — rename yours to install it", s.name);
    }
    let dir = reel_render::paths::sounds_dir()
        .ok_or_else(|| anyhow!("cannot determine the reel config directory"))?;
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(format!("{}.toml", s.name)), text)?;
    Ok(s.name)
}
