//! `reel theme import`: converts base16 YAML, iTerm2 `.itermcolors`, and
//! Alacritty theme files into reel's native theme TOML in the user themes
//! dir. Zero-switching-cost adoption for whatever palette someone already
//! loves.

use anyhow::{anyhow, bail, Context, Result};
use reel_render::theme::{self, Theme};
use reel_render::Rgba;
use std::path::Path;

/// A parsed theme, its default name, and an optional (font family, size) hint.
type ThemeImport = (Theme, String, Option<(String, f32)>);

/// Imports the theme straight from an installed terminal's own settings.
pub fn import_from_terminal(which: &str, name: Option<String>) -> Result<()> {
    let home = std::env::var("HOME").map(std::path::PathBuf::from).ok();
    let (theme, default_name, font_hint) = match which {
        "iterm" | "iterm2" => {
            let plist_path = home
                .as_ref()
                .map(|h| h.join("Library/Preferences/com.googlecode.iterm2.plist"))
                .filter(|p| p.exists())
                .ok_or_else(|| anyhow!("iTerm2 preferences not found (is iTerm2 installed?)"))?;
            from_iterm_profile(&plist_path)?
        }
        "kitty" => {
            let conf = home
                .as_ref()
                .map(|h| h.join(".config/kitty/kitty.conf"))
                .filter(|p| p.exists())
                .ok_or_else(|| anyhow!("~/.config/kitty/kitty.conf not found"))?;
            from_keyvalue_config(&conf, KittyDialect)?
        }
        "ghostty" => {
            let conf = home
                .as_ref()
                .and_then(|h| {
                    [
                        h.join(".config/ghostty/config"),
                        h.join("Library/Application Support/com.mitchellh.ghostty/config"),
                    ]
                    .into_iter()
                    .find(|p| p.exists())
                })
                .ok_or_else(|| anyhow!("Ghostty config not found"))?;
            from_keyvalue_config(&conf, GhosttyDialect)?
        }
        other => bail!("unknown terminal `{other}` (supported: iterm, kitty, ghostty)"),
    };
    let mut theme = theme;
    theme.name = sanitize(&name.unwrap_or(default_name));
    let installed = install_theme(&theme)?;
    if let Some((family, size)) = font_hint {
        println!("your terminal font: [style] font = \"{family}\", font_size = {size}");
    }
    println!("use it: [style] theme = \"{installed}\"");
    Ok(())
}

fn install_theme(theme: &Theme) -> Result<String> {
    let dir = reel_render::paths::themes_dir()
        .ok_or_else(|| anyhow!("cannot determine the reel config directory"))?;
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(format!("{}.toml", theme.name));
    std::fs::write(&dest, theme::to_toml(theme))?;
    println!("imported `{}` → {}", theme.name, dest.display());
    Ok(theme.name.clone())
}

/// The *active* iTerm2 profile from its preferences plist.
fn from_iterm_profile(path: &Path) -> Result<ThemeImport> {
    let value = plist::Value::from_file(path)
        .with_context(|| format!("parsing {}", path.display()))?;
    let root = value
        .as_dictionary()
        .ok_or_else(|| anyhow!("iTerm2 plist root is not a dictionary"))?;
    let bookmarks = root
        .get("New Bookmarks")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("no profiles in the iTerm2 plist"))?;
    let default_guid = root.get("Default Bookmark Guid").and_then(|v| v.as_string());
    let profile = bookmarks
        .iter()
        .filter_map(|b| b.as_dictionary())
        .find(|b| {
            default_guid.is_some()
                && b.get("Guid").and_then(|g| g.as_string()) == default_guid
        })
        .or_else(|| bookmarks.first().and_then(|b| b.as_dictionary()))
        .ok_or_else(|| anyhow!("no usable iTerm2 profile found"))?;

    let color = |key: &str| -> Result<Rgba> {
        let d = profile
            .get(key)
            .and_then(|v| v.as_dictionary())
            .ok_or_else(|| anyhow!("profile missing `{key}`"))?;
        let comp = |k: &str| -> Result<u8> {
            let v = d
                .get(k)
                .and_then(|v| v.as_real().or_else(|| v.as_signed_integer().map(|i| i as f64)))
                .ok_or_else(|| anyhow!("`{key}` missing `{k}`"))?;
            Ok((v.clamp(0.0, 1.0) * 255.0).round() as u8)
        };
        Ok(Rgba::rgb(comp("Red Component")?, comp("Green Component")?, comp("Blue Component")?))
    };
    let fg = color("Foreground Color")?;
    let mut ansi = [Rgba::rgb(0, 0, 0); 16];
    for (i, slot) in ansi.iter_mut().enumerate() {
        *slot = color(&format!("Ansi {i} Color"))?;
    }
    let name = profile
        .get("Name")
        .and_then(|v| v.as_string())
        .unwrap_or("iterm")
        .to_string();
    // "GeistMono-Regular 14" → family + size hint for [style].
    let font_hint = profile
        .get("Normal Font")
        .and_then(|v| v.as_string())
        .and_then(|f| {
            let (family, size) = f.rsplit_once(' ')?;
            Some((family.replace('-', " "), size.parse::<f32>().ok()?))
        });
    Ok((
        Theme {
            name: format!("iterm-{name}"),
            fg,
            bg: color("Background Color")?,
            cursor: color("Cursor Color").unwrap_or(fg),
            ansi,
        },
        format!("iterm-{name}"),
        font_hint,
    ))
}

/// kitty.conf / Ghostty config are both simple key-value lines.
struct KittyDialect;
struct GhosttyDialect;

trait TermDialect {
    /// Returns (ansi_index, color) / named slot for one config line.
    fn parse(&self, key: &str, val: &str) -> Option<(Slot, String)>;
    fn name(&self) -> &'static str;
}

enum Slot {
    Ansi(usize),
    Fg,
    Bg,
    Cursor,
    FontFamily,
    FontSize,
}

impl TermDialect for KittyDialect {
    fn parse(&self, key: &str, val: &str) -> Option<(Slot, String)> {
        let slot = match key {
            k if k.starts_with("color") => Slot::Ansi(k[5..].parse().ok()?),
            "foreground" => Slot::Fg,
            "background" => Slot::Bg,
            "cursor" => Slot::Cursor,
            "font_family" => Slot::FontFamily,
            "font_size" => Slot::FontSize,
            _ => return None,
        };
        Some((slot, val.to_string()))
    }
    fn name(&self) -> &'static str {
        "kitty"
    }
}

impl TermDialect for GhosttyDialect {
    fn parse(&self, key: &str, val: &str) -> Option<(Slot, String)> {
        let slot = match key {
            "palette" => {
                let (idx, color) = val.split_once('=')?;
                return Some((Slot::Ansi(idx.trim().parse().ok()?), color.trim().to_string()));
            }
            "foreground" => Slot::Fg,
            "background" => Slot::Bg,
            "cursor-color" => Slot::Cursor,
            "font-family" => Slot::FontFamily,
            "font-size" => Slot::FontSize,
            _ => return None,
        };
        Some((slot, val.to_string()))
    }
    fn name(&self) -> &'static str {
        "ghostty"
    }
}

fn from_keyvalue_config(path: &Path, dialect: impl TermDialect) -> Result<ThemeImport> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let mut ansi: [Option<Rgba>; 16] = [None; 16];
    let (mut fg, mut bg, mut cursor) = (None, None, None);
    let (mut family, mut size) = (None, None);
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, val) = match line.split_once(['=', ' ', '\t']) {
            Some((k, v)) => (k.trim(), v.trim().trim_start_matches('=').trim()),
            None => continue,
        };
        let Some((slot, val)) = dialect.parse(key, val) else { continue };
        match slot {
            Slot::Ansi(i) if i < 16 => {
                if let Ok(c) = hex(&val) {
                    ansi[i] = Some(c);
                }
            }
            Slot::Ansi(_) => {}
            Slot::Fg => fg = hex(&val).ok(),
            Slot::Bg => bg = hex(&val).ok(),
            Slot::Cursor => cursor = hex(&val).ok(),
            Slot::FontFamily => family = Some(val.trim_matches('"').to_string()),
            Slot::FontSize => size = val.parse::<f32>().ok(),
        }
    }
    let fg = fg.ok_or_else(|| anyhow!("no foreground color in {}", path.display()))?;
    let bg = bg.ok_or_else(|| anyhow!("no background color in {}", path.display()))?;
    // Missing palette slots fall back to reel's defaults for that half.
    let defaults = theme::builtin("reel-dark").unwrap();
    let mut palette = defaults.ansi;
    for (i, c) in ansi.iter().enumerate() {
        if let Some(c) = c {
            palette[i] = *c;
        }
    }
    let name = dialect.name().to_string();
    Ok((
        Theme { name: name.clone(), fg, bg, cursor: cursor.unwrap_or(fg), ansi: palette },
        name,
        family.map(|f| (f, size.unwrap_or(14.0))),
    ))
}

pub fn import(path: &Path, name: Option<String>) -> Result<()> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("imported")
        .to_string();
    // Parsers fall back to the file stem; a scheme's own declared name wins,
    // and an explicit --name beats both.
    let fallback = sanitize(&stem);
    let name = name.map(|n| sanitize(&n));

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let theme = match ext.as_str() {
        "itermcolors" => from_iterm(path, &fallback)?,
        "yaml" | "yml" => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            from_base16(&text, &fallback)
                .or_else(|base16_err| {
                    from_alacritty_yaml(&text, &fallback).map_err(|ala_err| {
                        anyhow!("not base16 ({base16_err}) and not alacritty ({ala_err})")
                    })
                })?
        }
        "toml" => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            match from_alacritty_toml(&text, &fallback) {
                Ok(t) => t,
                // Maybe it's already a reel theme: install as-is.
                Err(ala_err) => theme::from_toml(&text, &fallback)
                    .map_err(|reel_err| {
                        anyhow!("not alacritty ({ala_err}) and not a reel theme ({reel_err})")
                    })?,
            }
        }
        other => bail!(
            "unrecognized theme format `.{other}` (supported: base16 .yaml, \
             alacritty .toml/.yml, iTerm2 .itermcolors, reel .toml)"
        ),
    };
    let mut theme = theme;
    theme.name = sanitize(&name.unwrap_or_else(|| theme.name.clone()));
    let name = theme.name.clone();

    let dir = reel_render::paths::themes_dir()
        .ok_or_else(|| anyhow!("cannot determine the reel config directory"))?;
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(format!("{name}.toml"));
    std::fs::write(&dest, theme::to_toml(&theme))?;
    println!("imported `{name}` → {}", dest.display());
    println!("use it: [style] theme = \"{name}\"");
    Ok(())
}

/// Theme names become file names: lowercase, dash-separated, nothing weird.
fn sanitize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.to_ascii_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

fn hex(s: &str) -> Result<Rgba> {
    let s = s.trim();
    let normalized = if let Some(h) = s.strip_prefix("0x") {
        format!("#{h}")
    } else if s.starts_with('#') {
        s.to_string()
    } else {
        format!("#{s}")
    };
    Rgba::from_hex(&normalized).ok_or_else(|| anyhow!("bad color `{s}`"))
}

// ---------------------------------------------------------------------------
// base16 (https://github.com/chriskempson/base16)
// ---------------------------------------------------------------------------

fn from_base16(text: &str, fallback_name: &str) -> Result<Theme> {
    let map: std::collections::BTreeMap<String, serde_yaml_ng::Value> =
        serde_yaml_ng::from_str(text).context("parsing YAML")?;
    // New-style schemes nest colors under `palette`; classic ones are flat.
    let flat;
    let palette = match map.get("palette").and_then(|v| v.as_mapping()) {
        Some(p) => {
            flat = p
                .iter()
                .filter_map(|(k, v)| Some((k.as_str()?.to_string(), v.clone())))
                .collect();
            &flat
        }
        None => &map,
    };
    let base = |key: &str| -> Result<Rgba> {
        let v = palette
            .get(key)
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing `{key}`"))?;
        hex(v)
    };
    let b: Vec<Rgba> = (0..16)
        .map(|i| base(&format!("base{i:02X}")))
        .collect::<Result<_>>()?;

    let name = map
        .get("scheme")
        .or_else(|| map.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_ascii_lowercase().replace(' ', "-"))
        .unwrap_or_else(|| fallback_name.to_string());

    // The canonical base16 → ANSI-16 mapping (base16-shell's default).
    Ok(Theme {
        name,
        fg: b[0x5],
        bg: b[0x0],
        cursor: b[0x5],
        ansi: [
            b[0x0], b[0x8], b[0xB], b[0xA], b[0xD], b[0xE], b[0xC], b[0x5],
            b[0x3], b[0x8], b[0xB], b[0xA], b[0xD], b[0xE], b[0xC], b[0x7],
        ],
    })
}

// ---------------------------------------------------------------------------
// Alacritty (TOML since 0.13; YAML before)
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct AlacrittyFile {
    colors: AlacrittyColors,
}

#[derive(serde::Deserialize)]
struct AlacrittyColors {
    primary: AlacrittyPrimary,
    #[serde(default)]
    cursor: Option<AlacrittyCursor>,
    normal: AlacrittyAnsi,
    #[serde(default)]
    bright: Option<AlacrittyAnsi>,
}

#[derive(serde::Deserialize)]
struct AlacrittyPrimary {
    background: String,
    foreground: String,
}

#[derive(serde::Deserialize)]
struct AlacrittyCursor {
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(serde::Deserialize)]
struct AlacrittyAnsi {
    black: String,
    red: String,
    green: String,
    yellow: String,
    blue: String,
    magenta: String,
    cyan: String,
    white: String,
}

fn alacritty_theme(f: AlacrittyFile, name: &str) -> Result<Theme> {
    let row = |a: &AlacrittyAnsi| -> Result<[Rgba; 8]> {
        Ok([
            hex(&a.black)?, hex(&a.red)?, hex(&a.green)?, hex(&a.yellow)?,
            hex(&a.blue)?, hex(&a.magenta)?, hex(&a.cyan)?, hex(&a.white)?,
        ])
    };
    let normal = row(&f.colors.normal)?;
    let bright = match &f.colors.bright {
        Some(b) => row(b)?,
        None => normal,
    };
    let fg = hex(&f.colors.primary.foreground)?;
    let mut ansi = [Rgba::rgb(0, 0, 0); 16];
    ansi[..8].copy_from_slice(&normal);
    ansi[8..].copy_from_slice(&bright);
    Ok(Theme {
        name: name.to_string(),
        fg,
        bg: hex(&f.colors.primary.background)?,
        cursor: f
            .colors
            .cursor
            .and_then(|c| c.cursor)
            .map(|c| hex(&c))
            .transpose()?
            .unwrap_or(fg),
        ansi,
    })
}

fn from_alacritty_toml(text: &str, name: &str) -> Result<Theme> {
    alacritty_theme(toml::from_str(text).context("parsing TOML")?, name)
}

fn from_alacritty_yaml(text: &str, name: &str) -> Result<Theme> {
    alacritty_theme(serde_yaml_ng::from_str(text).context("parsing YAML")?, name)
}

// ---------------------------------------------------------------------------
// iTerm2 .itermcolors (XML plist)
// ---------------------------------------------------------------------------

fn from_iterm(path: &Path, name: &str) -> Result<Theme> {
    let value = plist::Value::from_file(path)
        .with_context(|| format!("parsing {}", path.display()))?;
    let dict = value
        .as_dictionary()
        .ok_or_else(|| anyhow!("plist root is not a dictionary"))?;

    let color = |key: &str| -> Result<Rgba> {
        let d = dict
            .get(key)
            .and_then(|v| v.as_dictionary())
            .ok_or_else(|| anyhow!("missing `{key}`"))?;
        let comp = |k: &str| -> Result<u8> {
            let v = d
                .get(k)
                .and_then(|v| v.as_real().or_else(|| v.as_signed_integer().map(|i| i as f64)))
                .ok_or_else(|| anyhow!("`{key}` missing `{k}`"))?;
            Ok((v.clamp(0.0, 1.0) * 255.0).round() as u8)
        };
        Ok(Rgba::rgb(comp("Red Component")?, comp("Green Component")?, comp("Blue Component")?))
    };

    let fg = color("Foreground Color")?;
    let mut ansi = [Rgba::rgb(0, 0, 0); 16];
    for (i, slot) in ansi.iter_mut().enumerate() {
        *slot = color(&format!("Ansi {i} Color"))?;
    }
    Ok(Theme {
        name: name.to_string(),
        fg,
        bg: color("Background Color")?,
        cursor: color("Cursor Color").unwrap_or(fg),
        ansi,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE16_CLASSIC: &str = r#"
scheme: "Ocean Deep"
author: "someone"
base00: "181818"
base01: "282828"
base02: "383838"
base03: "585858"
base04: "b8b8b8"
base05: "d8d8d8"
base06: "e8e8e8"
base07: "f8f8f8"
base08: "ab4642"
base09: "dc9656"
base0A: "f7ca88"
base0B: "a1b56c"
base0C: "86c1b9"
base0D: "7cafc2"
base0E: "ba8baf"
base0F: "a16946"
"#;

    #[test]
    fn base16_classic_maps_the_conventional_slots() {
        let t = from_base16(BASE16_CLASSIC, "x").unwrap();
        assert_eq!(t.name, "ocean-deep");
        assert_eq!(t.bg, Rgba::from_hex("#181818").unwrap());
        assert_eq!(t.fg, Rgba::from_hex("#d8d8d8").unwrap());
        assert_eq!(t.ansi[1], Rgba::from_hex("#ab4642").unwrap()); // red = base08
        assert_eq!(t.ansi[4], Rgba::from_hex("#7cafc2").unwrap()); // blue = base0D
        assert_eq!(t.ansi[8], Rgba::from_hex("#585858").unwrap()); // bright black = base03
    }

    #[test]
    fn base16_new_style_palette_nesting() {
        let text = r##"
name: "Nested"
palette:
  base00: "#101010"
  base01: "#202020"
  base02: "#303030"
  base03: "#404040"
  base04: "#505050"
  base05: "#606060"
  base06: "#707070"
  base07: "#808080"
  base08: "#900000"
  base09: "#a00000"
  base0A: "#b00000"
  base0B: "#009000"
  base0C: "#00a0a0"
  base0D: "#0000d0"
  base0E: "#d000d0"
  base0F: "#803000"
"##;
        let t = from_base16(text, "x").unwrap();
        assert_eq!(t.name, "nested");
        assert_eq!(t.ansi[2], Rgba::from_hex("#009000").unwrap());
    }

    #[test]
    fn alacritty_toml_with_bright_and_0x_colors() {
        let text = r##"
[colors.primary]
background = "0x1a1b26"
foreground = "#c0caf5"

[colors.cursor]
cursor = "#ffffff"

[colors.normal]
black = "#15161e"
red = "#f7768e"
green = "#9ece6a"
yellow = "#e0af68"
blue = "#7aa2f7"
magenta = "#bb9af7"
cyan = "#7dcfff"
white = "#a9b1d6"

[colors.bright]
black = "#414868"
red = "#ff7a93"
green = "#b9f27c"
yellow = "#ff9e64"
blue = "#7da6ff"
magenta = "#bb9af7"
cyan = "#0db9d7"
white = "#c0caf5"
"##;
        let t = from_alacritty_toml(text, "tokyo").unwrap();
        assert_eq!(t.bg, Rgba::from_hex("#1a1b26").unwrap());
        assert_eq!(t.cursor, Rgba::from_hex("#ffffff").unwrap());
        assert_eq!(t.ansi[9], Rgba::from_hex("#ff7a93").unwrap());
    }

    #[test]
    fn alacritty_without_bright_duplicates_normal() {
        let text = r##"
colors:
  primary:
    background: "#000000"
    foreground: "#ffffff"
  normal:
    black: "#111111"
    red: "#ff0000"
    green: "#00ff00"
    yellow: "#ffff00"
    blue: "#0000ff"
    magenta: "#ff00ff"
    cyan: "#00ffff"
    white: "#eeeeee"
"##;
        let t = from_alacritty_yaml(text, "mini").unwrap();
        assert_eq!(t.ansi[1], t.ansi[9]);
        assert_eq!(t.cursor, t.fg, "cursor falls back to fg");
    }

    #[test]
    fn ghostty_keyvalue_config_parses() {
        let dir = std::env::temp_dir().join("reel-ghostty-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config");
        std::fs::write(&p, "\n# comment\nfont-family = Geist Mono\nfont-size = 13\nbackground = #101014\nforeground = #e6e6eb\ncursor-color = #8ab4f8\npalette = 0=#1c1c22\npalette = 1=#f28b82\npalette = 9=#f6aea9\n").unwrap();
        let (theme, name, font) = from_keyvalue_config(&p, GhosttyDialect).unwrap();
        assert_eq!(name, "ghostty");
        assert_eq!(theme.bg, Rgba::from_hex("#101014").unwrap());
        assert_eq!(theme.ansi[1], Rgba::from_hex("#f28b82").unwrap());
        assert_eq!(theme.ansi[9], Rgba::from_hex("#f6aea9").unwrap());
        let (family, size) = font.unwrap();
        assert_eq!(family, "Geist Mono");
        assert_eq!(size, 13.0);
    }

    #[test]
    fn kitty_keyvalue_config_parses() {
        let dir = std::env::temp_dir().join("reel-kitty-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("kitty.conf");
        std::fs::write(&p, "font_family MesloLGS NF\nforeground #dddddd\nbackground #000000\ncolor4 #7aa2f7\n").unwrap();
        let (theme, _, font) = from_keyvalue_config(&p, KittyDialect).unwrap();
        assert_eq!(theme.ansi[4], Rgba::from_hex("#7aa2f7").unwrap());
        assert_eq!(font.unwrap().0, "MesloLGS NF");
    }

    #[test]
    fn iterm_plist_parses_components() {
        let plist_text = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
{}
<key>Foreground Color</key><dict>
  <key>Red Component</key><real>1</real>
  <key>Green Component</key><real>1</real>
  <key>Blue Component</key><real>1</real>
</dict>
<key>Background Color</key><dict>
  <key>Red Component</key><real>0.06274509804</real>
  <key>Green Component</key><real>0.06274509804</real>
  <key>Blue Component</key><real>0.08235294118</real>
</dict>
</dict></plist>"#,
            (0..16)
                .map(|i| format!(
                    "<key>Ansi {i} Color</key><dict>\
                     <key>Red Component</key><real>{}</real>\
                     <key>Green Component</key><real>0.5</real>\
                     <key>Blue Component</key><real>0.25</real></dict>",
                    i as f64 / 15.0
                ))
                .collect::<String>()
        );
        let dir = std::env::temp_dir().join("reel-theme-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.itermcolors");
        std::fs::write(&p, plist_text).unwrap();
        let t = from_iterm(&p, "iterm").unwrap();
        assert_eq!(t.fg, Rgba::rgb(255, 255, 255));
        assert_eq!(t.bg, Rgba::rgb(16, 16, 21));
        assert_eq!(t.ansi[15], Rgba::rgb(255, 128, 64));
        assert_eq!(t.cursor, t.fg, "missing cursor falls back to fg");
    }
}
