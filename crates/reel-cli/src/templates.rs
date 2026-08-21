//! `reel template …`: list, show, and install templates.
//!
//! The "registry" is deliberately just GitHub: any repo with a `templates/`
//! directory of reel template TOML files is a template pack. `reel template
//! add owner/repo` installs them all; `owner/repo/name` picks one. No hosted
//! infrastructure, nothing to run.

use crate::json;
use crate::net;
use anyhow::{anyhow, bail, Context, Result};
use reel_render::template;
use std::path::{Path, PathBuf};

pub fn list() -> Result<()> {
    if json::on() {
        let mut out = Vec::new();
        for name in template::template_names() {
            let t = template::builtin(name).unwrap();
            out.push(serde_json::json!({
                "name": name, "description": t.description, "source": "builtin",
            }));
        }
        for name in template::user_template_names() {
            let desc = template::lookup(&name).map(|t| t.description).unwrap_or_default();
            out.push(serde_json::json!({
                "name": name, "description": desc, "source": "installed",
            }));
        }
        return json::emit(serde_json::json!({ "templates": out }));
    }
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
    Ok(())
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
        let installed = install_at(&text, stem, path.parent())?;
        println!("installed `{installed}` from {}", path.display());
        return Ok(());
    }

    // owner/repo[/name] on GitHub.
    match crate::packs::parse_source(source) {
        Some((owner, repo, None)) => {
            let mut installed = Vec::new();
            for (stem, text) in crate::packs::fetch_all(owner, repo, "templates")? {
                installed.push(install(&text, &stem)?);
            }
            println!("installed from {owner}/{repo}: {}", installed.join(", "));
            Ok(())
        }
        Some((owner, repo, Some(name))) => {
            let text = crate::packs::fetch_one(owner, repo, "templates", name)?;
            let installed = install(&text, name)?;
            println!("installed `{installed}` from {owner}/{repo}");
            Ok(())
        }
        None => bail!(
            "`{source}` is neither a local .toml file nor owner/repo[/name] — \
             try `reel template add galfrevn/reel-templates`"
        ),
    }
}

/// Validates and writes one template; returns the installed name.
fn install(text: &str, fallback_name: &str) -> Result<String> {
    install_at(text, fallback_name, None)
}

/// [`install`] with the source file's directory, so a local template's
/// relative image assets (wallpaper, badge logo) validate and get copied
/// next to the installed TOML — installed templates resolve images against
/// the templates dir.
fn install_at(text: &str, fallback_name: &str, base_dir: Option<&Path>) -> Result<String> {
    let t = template::from_toml_at(text, fallback_name, base_dir)
        .map_err(|e| anyhow!("invalid template: {e}"))?;
    let dir = reel_render::paths::templates_dir()
        .ok_or_else(|| anyhow!("cannot determine the reel config directory"))?;
    std::fs::create_dir_all(&dir)?;
    for image in template::referenced_images(&t) {
        let src = Path::new(&image);
        if src.is_absolute() {
            continue; // resolves the same from anywhere
        }
        if src.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            bail!("image path `{image}` escapes the template directory (`..`)");
        }
        let Some(base) = base_dir else {
            bail!("template references image `{image}` but has no source directory");
        };
        let dst = dir.join(src);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(base.join(src), &dst)
            .with_context(|| format!("copying image asset `{image}`"))?;
    }
    std::fs::write(dir.join(format!("{}.toml", t.name)), text)?;
    Ok(t.name)
}

/// The canonical demo recording: every registry preview and `template try`
/// renders this same cast, so looks stay comparable across templates.
const DEMO_CAST: &str = include_str!("../assets/demo.cast");

/// `reel template try <source>`: preview a template against the bundled demo
/// cast without installing anything. `source` is a template name, a local
/// .toml, or `owner/repo/name` on GitHub (downloaded to a temp file).
pub fn try_template(source: &str) -> Result<()> {
    let dir = preview_dir()?;
    let (template, label) = resolve_try_source(source, &dir)?;
    let out = render_demo_preview(&template, &label)?;
    if json::on() {
        return json::emit(serde_json::json!({
            "template": label,
            "preview": out.display().to_string(),
            "source": source,
        }));
    }
    println!("previewing `{label}` → {}", out.display());
    open_preview(&out);
    Ok(())
}

fn preview_dir() -> Result<PathBuf> {
    let dir = std::env::temp_dir().join("reel-try");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Renders the embedded demo cast with a template (name or .toml path) into
/// the preview dir. Shared by `try` and `publish`.
pub fn render_demo_preview(template_ref: &str, label: &str) -> Result<PathBuf> {
    let dir = preview_dir()?;
    let cast = dir.join("demo.cast");
    std::fs::write(&cast, DEMO_CAST)?;
    let out = dir.join(format!("{label}.webm"));
    crate::pipeline::render(
        &cast,
        crate::pipeline::RenderArgs {
            out: Some(out.clone()),
            template: Some(template_ref.to_string()),
            ..Default::default()
        },
    )?;
    Ok(out)
}

/// Turns a try source into something `template::lookup` resolves (a name or
/// a .toml path) plus a display label.
fn resolve_try_source(source: &str, dir: &Path) -> Result<(String, String)> {
    // Local .toml file?
    let path = Path::new(source);
    if path.extension().is_some_and(|e| e == "toml") && path.exists() {
        let text = std::fs::read_to_string(path)?;
        let t = template::from_toml_at(&text, "preview", path.parent())
            .map_err(|e| anyhow!("invalid template {}: {e}", path.display()))?;
        return Ok((source.to_string(), t.name));
    }

    // Installed or builtin name?
    if template::lookup(source).is_some() && !source.contains('/') {
        return Ok((source.to_string(), source.to_string()));
    }

    // owner/repo/name on GitHub, fetched to a temp file — not installed.
    let parts: Vec<&str> = source.split('/').filter(|p| !p.is_empty()).collect();
    let [owner, repo, name] = parts.as_slice() else {
        bail!(
            "`{source}` is not a template name, a local .toml, or owner/repo/name — \
             try `reel template search`"
        );
    };
    let url =
        format!("https://raw.githubusercontent.com/{owner}/{repo}/HEAD/templates/{name}.toml");
    let text = net::fetch(&url, &format!("downloading {name} from {owner}/{repo}…"))
        .with_context(|| format!("downloading templates/{name}.toml from {owner}/{repo}"))?;
    let t = template::from_toml(&text, name)
        .map_err(|e| anyhow!("invalid template from {owner}/{repo}: {e}"))?;
    let tmp = dir.join(format!("{}.toml", t.name));
    std::fs::write(&tmp, &text)?;
    Ok((tmp.to_string_lossy().into_owned(), t.name))
}

/// Best-effort: pop the rendered preview open in the platform viewer.
/// `--json` never opens anything — an agent-driven run must not spawn a
/// window on the user's desktop, and the path is in the document anyway.
pub fn open_preview(path: &PathBuf) {
    if json::on() {
        return;
    }
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let opener = "xdg-open";
    #[cfg(windows)]
    let opener = "explorer";
    let _ = std::process::Command::new(opener).arg(path).status();
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
