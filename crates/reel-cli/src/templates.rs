//! `reel template …`: list, show, and install templates.
//!
//! The "registry" is deliberately just GitHub: any repo with a `templates/`
//! directory of reel template TOML files is a template pack. `reel template
//! add owner/repo` installs them all; `owner/repo/name` picks one. No hosted
//! infrastructure, nothing to run.

use anyhow::{anyhow, bail, Context, Result};
use reel_render::template;
use std::io::Read;
use std::path::Path;

pub fn list() {
    for name in template::template_names() {
        let t = template::builtin(name).unwrap();
        println!("{name:<10} {}", t.description);
    }
    for name in template::user_template_names() {
        let desc = template::lookup(&name)
            .map(|t| t.description)
            .unwrap_or_default();
        println!("{name:<10} {desc} (installed)");
    }
}

/// Prints a template as TOML — any builtin doubles as a starting point:
/// `reel template show glass > my-glass.toml`.
pub fn show(name: &str) -> Result<()> {
    let t = template::lookup(name).ok_or_else(|| {
        anyhow!(
            "unknown template `{name}` (available: {})",
            all_names().join(", ")
        )
    })?;
    print!("{}", template::to_toml(&t));
    Ok(())
}

pub fn add(source: &str) -> Result<()> {
    // Local file?
    let path = Path::new(source);
    if path.exists() {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("template");
        let installed = install(&text, stem)?;
        println!("installed `{installed}` from {}", path.display());
        return Ok(());
    }

    // owner/repo[/name] on GitHub.
    let parts: Vec<&str> = source.split('/').filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        [owner, repo] => {
            let listing = fetch(&format!(
                "https://api.github.com/repos/{owner}/{repo}/contents/templates"
            ))
            .with_context(|| format!("listing templates in {owner}/{repo}"))?;
            let entries: Vec<serde_json::Value> =
                serde_json::from_str(&listing).context("parsing GitHub response")?;
            let mut installed = Vec::new();
            for e in &entries {
                let name = e["name"].as_str().unwrap_or_default();
                let Some(stem) = name.strip_suffix(".toml") else { continue };
                let url = e["download_url"]
                    .as_str()
                    .ok_or_else(|| anyhow!("no download_url for {name}"))?;
                let text = fetch(url).with_context(|| format!("downloading {name}"))?;
                installed.push(install(&text, stem)?);
            }
            if installed.is_empty() {
                bail!("{owner}/{repo} has no templates/*.toml files");
            }
            println!("installed from {owner}/{repo}: {}", installed.join(", "));
            Ok(())
        }
        [owner, repo, name] => {
            let url = format!(
                "https://raw.githubusercontent.com/{owner}/{repo}/HEAD/templates/{name}.toml"
            );
            let text = fetch(&url)
                .with_context(|| format!("downloading templates/{name}.toml from {owner}/{repo}"))?;
            let installed = install(&text, name)?;
            println!("installed `{installed}` from {owner}/{repo}");
            Ok(())
        }
        _ => bail!(
            "`{source}` is neither a local .toml file nor owner/repo[/name] — \
             try `reel template add galfrevn/reel-templates`"
        ),
    }
}

/// Validates and writes one template; returns the installed name.
fn install(text: &str, fallback_name: &str) -> Result<String> {
    let t = template::from_toml(text, fallback_name)
        .map_err(|e| anyhow!("invalid template: {e}"))?;
    let dir = reel_render::paths::templates_dir()
        .ok_or_else(|| anyhow!("cannot determine the reel config directory"))?;
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(format!("{}.toml", t.name)), text)?;
    Ok(t.name)
}

fn fetch(url: &str) -> Result<String> {
    let mut response = ureq::get(url)
        .header("User-Agent", "reel-cli")
        .call()
        .map_err(|e| anyhow!("GET {url}: {e}"))?;
    let mut body = String::new();
    response
        .body_mut()
        .as_reader()
        .take(4 << 20)
        .read_to_string(&mut body)?;
    Ok(body)
}

fn all_names() -> Vec<String> {
    let mut names: Vec<String> =
        template::template_names().iter().map(|s| s.to_string()).collect();
    names.extend(template::user_template_names());
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_validates_and_uses_declared_name() {
        let dir = std::env::temp_dir().join(format!("reel-tpl-test-{}", std::process::id()));
        std::env::set_var("REEL_CONFIG_DIR", &dir);
        let name = install("name = \"neon\"\ntheme = \"tokyo-night\"\n", "file-stem").unwrap();
        assert_eq!(name, "neon");
        assert!(dir.join("templates/neon.toml").exists());
        assert!(install("wobble = true\n", "bad").is_err());
        std::env::remove_var("REEL_CONFIG_DIR");
        let _ = std::fs::remove_dir_all(dir);
    }
}
