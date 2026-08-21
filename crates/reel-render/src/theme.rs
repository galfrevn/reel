//! Color themes and the ColorRef → RGBA resolution rules.

use reel_term::ColorRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Rgba { r, g, b, a: 255 }
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        let h = s.trim().strip_prefix('#')?;
        let parse = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        match h.len() {
            6 => Some(Rgba { r: parse(0)?, g: parse(2)?, b: parse(4)?, a: 255 }),
            8 => Some(Rgba { r: parse(0)?, g: parse(2)?, b: parse(4)?, a: parse(6)? }),
            _ => None,
        }
    }

    pub fn scaled(self, f: f32) -> Self {
        Rgba {
            r: (self.r as f32 * f).clamp(0.0, 255.0) as u8,
            g: (self.g as f32 * f).clamp(0.0, 255.0) as u8,
            b: (self.b as f32 * f).clamp(0.0, 255.0) as u8,
            a: self.a,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub fg: Rgba,
    pub bg: Rgba,
    pub cursor: Rgba,
    /// ANSI 0-15.
    pub ansi: [Rgba; 16],
}

impl Theme {
    /// A copy of the theme with a snapshot's OSC 10/11/12 dynamic default
    /// colors applied (fg, bg, cursor) — a vim colorscheme that sets the
    /// terminal background must render on it, not on the template theme's.
    pub fn with_defaults(&self, defaults: &[Option<(u8, u8, u8)>; 3]) -> Theme {
        let mut t = self.clone();
        if let Some((r, g, b)) = defaults[0] {
            t.fg = Rgba::rgb(r, g, b);
        }
        if let Some((r, g, b)) = defaults[1] {
            t.bg = Rgba::rgb(r, g, b);
        }
        if let Some((r, g, b)) = defaults[2] {
            t.cursor = Rgba::rgb(r, g, b);
        }
        t
    }

    /// Resolves an abstract cell color. `overrides` are the snapshot's OSC 4
    /// palette redefinitions.
    pub fn resolve(&self, c: ColorRef, overrides: &[(u8, (u8, u8, u8))]) -> Rgba {
        match c {
            ColorRef::Fg => self.fg,
            ColorRef::Bg => self.bg,
            ColorRef::Cursor => self.cursor,
            ColorRef::Rgb(r, g, b) => Rgba::rgb(r, g, b),
            ColorRef::Indexed(i) => {
                if let Some(&(_, (r, g, b))) = overrides.iter().find(|(idx, _)| *idx == i) {
                    return Rgba::rgb(r, g, b);
                }
                if i < 16 {
                    self.ansi[i as usize]
                } else {
                    xterm_256(i)
                }
            }
        }
    }
}

/// Standard xterm 256-color cube and grayscale ramp for indices 16-255.
fn xterm_256(i: u8) -> Rgba {
    if i < 16 {
        unreachable!("handled by the theme");
    }
    if i < 232 {
        let i = i - 16;
        let comp = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
        Rgba::rgb(comp(i / 36), comp((i / 6) % 6), comp(i % 6))
    } else {
        let v = 8 + (i - 232) * 10;
        Rgba::rgb(v, v, v)
    }
}

fn hex(s: &str) -> Rgba {
    Rgba::from_hex(s).expect("built-in theme hex")
}

pub fn builtin(name: &str) -> Option<Theme> {
    let t = match name {
        "reel-dark" => Theme {
            name: "reel-dark".to_string(),
            fg: hex("#e6e6eb"),
            bg: hex("#101014"),
            cursor: hex("#8ab4f8"),
            ansi: [
                hex("#1c1c22"), hex("#f28b82"), hex("#81c995"), hex("#fdd663"),
                hex("#8ab4f8"), hex("#d7aefb"), hex("#78d9ec"), hex("#e6e6eb"),
                hex("#5f6368"), hex("#f6aea9"), hex("#a8dab5"), hex("#fde293"),
                hex("#aecbfa"), hex("#e9d2fd"), hex("#a1e4f2"), hex("#ffffff"),
            ],
        },
        "catppuccin-mocha" => Theme {
            name: "catppuccin-mocha".to_string(),
            fg: hex("#cdd6f4"),
            bg: hex("#1e1e2e"),
            cursor: hex("#f5e0dc"),
            ansi: [
                hex("#45475a"), hex("#f38ba8"), hex("#a6e3a1"), hex("#f9e2af"),
                hex("#89b4fa"), hex("#f5c2e7"), hex("#94e2d5"), hex("#bac2de"),
                hex("#585b70"), hex("#f38ba8"), hex("#a6e3a1"), hex("#f9e2af"),
                hex("#89b4fa"), hex("#f5c2e7"), hex("#94e2d5"), hex("#a6adc8"),
            ],
        },
        "tokyo-night" => Theme {
            name: "tokyo-night".to_string(),
            fg: hex("#c0caf5"),
            bg: hex("#1a1b26"),
            cursor: hex("#c0caf5"),
            ansi: [
                hex("#15161e"), hex("#f7768e"), hex("#9ece6a"), hex("#e0af68"),
                hex("#7aa2f7"), hex("#bb9af7"), hex("#7dcfff"), hex("#a9b1d6"),
                hex("#414868"), hex("#f7768e"), hex("#9ece6a"), hex("#e0af68"),
                hex("#7aa2f7"), hex("#bb9af7"), hex("#7dcfff"), hex("#c0caf5"),
            ],
        },
        "geist-dark" => Theme {
            name: "geist-dark".to_string(),
            fg: hex("#ededed"),
            bg: hex("#000000"),
            cursor: hex("#ededed"),
            ansi: [
                hex("#1a1a1a"), hex("#e5484d"), hex("#45a557"), hex("#f5a623"),
                hex("#0070f3"), hex("#8e4ec6"), hex("#12a594"), hex("#ededed"),
                hex("#505050"), hex("#ff6166"), hex("#63c174"), hex("#ffb224"),
                hex("#52a8ff"), hex("#bf7af0"), hex("#0ac5b3"), hex("#ffffff"),
            ],
        },
        "paper-light" => Theme {
            name: "paper-light".to_string(),
            fg: hex("#2d2d2d"),
            bg: hex("#f7f7f2"),
            cursor: hex("#0f62fe"),
            ansi: [
                hex("#2d2d2d"), hex("#c4331d"), hex("#237a3b"), hex("#a1740c"),
                hex("#0f62fe"), hex("#8a3ffc"), hex("#007d79"), hex("#6f6f6f"),
                hex("#525252"), hex("#e05243"), hex("#3fa860"), hex("#c9a03a"),
                hex("#4589ff"), hex("#a56eff"), hex("#08bdba"), hex("#161616"),
            ],
        },
        "phosphor" => Theme {
            name: "phosphor".to_string(),
            fg: hex("#33ff66"),
            bg: hex("#0a0f0a"),
            cursor: hex("#66ffa0"),
            ansi: [
                hex("#0e140e"), hex("#2ee65c"), hex("#33ff66"), hex("#7dffa3"),
                hex("#1fb84a"), hex("#57f584"), hex("#45ec74"), hex("#a4ffbf"),
                hex("#1d7a3c"), hex("#49f277"), hex("#5cff85"), hex("#a0ffba"),
                hex("#2ecb5d"), hex("#7affa1"), hex("#68f792"), hex("#ccffdb"),
            ],
        },
        _ => return None,
    };
    Some(t)
}

pub fn theme_names() -> &'static [&'static str] {
    &["reel-dark", "catppuccin-mocha", "tokyo-night", "geist-dark", "paper-light", "phosphor"]
}

// ---------------------------------------------------------------------------
// User themes: TOML files in the themes dir, written by `reel theme import`
// ---------------------------------------------------------------------------

/// Parses reel's native theme TOML (`fg`/`bg`/`cursor` + 16 `ansi` colors).
pub fn from_toml(text: &str, fallback_name: &str) -> Result<Theme, String> {
    let value: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
    from_value(value, fallback_name)
}

/// Parses a theme from an already-parsed TOML value — the same shape as a
/// standalone theme file, which is how templates embed a palette inline.
pub fn from_value(value: toml::Value, fallback_name: &str) -> Result<Theme, String> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ThemeFile {
        name: Option<String>,
        fg: String,
        bg: String,
        cursor: Option<String>,
        ansi: Vec<String>,
    }
    let f: ThemeFile = value.try_into().map_err(|e: toml::de::Error| e.to_string())?;
    if f.ansi.len() != 16 {
        return Err(format!("`ansi` needs exactly 16 colors, got {}", f.ansi.len()));
    }
    let color = |s: &String| {
        Rgba::from_hex(s).ok_or_else(|| format!("bad color `{s}` (expected #rrggbb)"))
    };
    let fg = color(&f.fg)?;
    let mut ansi = [Rgba::rgb(0, 0, 0); 16];
    for (i, s) in f.ansi.iter().enumerate() {
        ansi[i] = color(s)?;
    }
    Ok(Theme {
        name: f.name.unwrap_or_else(|| fallback_name.to_string()),
        fg,
        bg: color(&f.bg)?,
        cursor: f.cursor.as_ref().map(&color).transpose()?.unwrap_or(fg),
        ansi,
    })
}

/// The theme as a TOML value — the shape templates embed under `[theme]`.
pub fn to_value(t: &Theme) -> toml::Value {
    let hex = |c: Rgba| toml::Value::String(format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b));
    let mut table = toml::map::Map::new();
    table.insert("name".into(), toml::Value::String(t.name.clone()));
    table.insert("fg".into(), hex(t.fg));
    table.insert("bg".into(), hex(t.bg));
    table.insert("cursor".into(), hex(t.cursor));
    table.insert("ansi".into(), toml::Value::Array(t.ansi.iter().map(|&c| hex(c)).collect()));
    toml::Value::Table(table)
}

/// Serializes a theme as the native TOML format `from_toml` reads.
pub fn to_toml(t: &Theme) -> String {
    let hex = |c: Rgba| format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b);
    let mut out = String::new();
    out.push_str(&format!("name = {:?}\n", t.name));
    out.push_str(&format!("fg = \"{}\"\n", hex(t.fg)));
    out.push_str(&format!("bg = \"{}\"\n", hex(t.bg)));
    out.push_str(&format!("cursor = \"{}\"\n", hex(t.cursor)));
    out.push_str("ansi = [\n");
    for row in t.ansi.chunks(4) {
        let cells: Vec<String> = row.iter().map(|&c| format!("\"{}\"", hex(c))).collect();
        out.push_str(&format!("    {},\n", cells.join(", ")));
    }
    out.push_str("]\n");
    out
}

/// Resolves a theme name: built-ins first, then `<themes_dir>/<name>.toml`.
pub fn lookup(name: &str) -> Option<Theme> {
    if let Some(t) = builtin(name) {
        return Some(t);
    }
    let path = crate::paths::themes_dir()?.join(format!("{name}.toml"));
    let text = std::fs::read_to_string(path).ok()?;
    from_toml(&text, name).ok()
}

/// Names of themes installed in the user themes dir.
pub fn user_theme_names() -> Vec<String> {
    let Some(dir) = crate::paths::themes_dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension()? == "toml").then(|| p.file_stem()?.to_str().map(str::to_owned))?
        })
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_matches_xterm_reference() {
        assert_eq!(xterm_256(16), Rgba::rgb(0, 0, 0));
        assert_eq!(xterm_256(21), Rgba::rgb(0, 0, 255));
        assert_eq!(xterm_256(196), Rgba::rgb(255, 0, 0));
        assert_eq!(xterm_256(231), Rgba::rgb(255, 255, 255));
        assert_eq!(xterm_256(232), Rgba::rgb(8, 8, 8));
        assert_eq!(xterm_256(255), Rgba::rgb(238, 238, 238));
    }

    #[test]
    fn toml_roundtrip_preserves_every_color() {
        let t = builtin("tokyo-night").unwrap();
        let text = to_toml(&t);
        let back = from_toml(&text, "ignored").unwrap();
        assert_eq!(back.name, t.name);
        assert_eq!(back.fg, t.fg);
        assert_eq!(back.bg, t.bg);
        assert_eq!(back.cursor, t.cursor);
        assert_eq!(back.ansi, t.ansi);
    }

    #[test]
    fn from_toml_rejects_short_palettes() {
        let err = from_toml("fg = \"#ffffff\"\nbg = \"#000000\"\nansi = [\"#111111\"]\n", "x")
            .unwrap_err();
        assert!(err.contains("16 colors"));
    }

    #[test]
    fn overrides_win() {
        let t = builtin("reel-dark").unwrap();
        let c = t.resolve(ColorRef::Indexed(1), &[(1, (9, 9, 9))]);
        assert_eq!(c, Rgba::rgb(9, 9, 9));
    }
}
