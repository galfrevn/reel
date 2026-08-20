//! Templates: the complete visual package (font, theme, chrome, canvas).
//! Built-ins are code for now; the community-template TOML loader arrives
//! with the registry (Phase 3).

use crate::fx::{CrtEffect, CRT_DEFAULT};
use crate::theme::Rgba;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowStyle {
    /// Rounded window with traffic-light buttons.
    MacOs,
    /// Rounded window, no titlebar buttons.
    Rounded,
    /// Square window box.
    Plain,
    /// No chrome at all — bare terminal.
    None,
}

/// Titlebar decoration, independent of the window shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Titlebar {
    None,
    /// macOS red/amber/green buttons.
    TrafficLights,
    /// Monochrome gray dots (the Vercel-docs look).
    Dots,
}

#[derive(Debug, Clone, Copy)]
pub enum CanvasBg {
    Solid(Rgba),
    /// Two-stop linear gradient with an angle in degrees (CSS convention:
    /// 0 = up, 90 = right).
    Linear { angle_deg: f32, from: Rgba, to: Rgba },
}

#[derive(Debug, Clone)]
pub struct Shadow {
    pub blur: f32,
    pub opacity: f32,
    pub offset_y: f32,
}

#[derive(Debug, Clone)]
pub struct Template {
    pub name: String,
    pub description: String,
    pub theme: String,
    /// Preferred font family name; `None` = the system monospace chain.
    pub font: Option<String>,
    pub font_size: f32,
    pub line_height: f32,
    pub window: WindowStyle,
    pub titlebar: Titlebar,
    /// Hairline rule between the titlebar and the content.
    pub titlebar_rule: bool,
    pub corner_radius: f32,
    /// Space between the window edge and the terminal content.
    pub padding: f32,
    /// Space between the window and the canvas edge.
    pub inset: f32,
    pub canvas: CanvasBg,
    pub shadow: Option<Shadow>,
    /// 1px-ish border color (alpha carries the subtlety).
    pub border: Option<Rgba>,
    /// Post-effects applied to the terminal image (scanlines, glow…).
    pub crt: Option<CrtEffect>,
}

fn hex(s: &str) -> Rgba {
    Rgba::from_hex(s).expect("builtin template hex")
}

pub fn builtin(name: &str) -> Option<Template> {
    let t = match name {
        "minimal" => Template {
            name: "minimal".into(),
            description: "High contrast, square corners, no chrome noise".into(),
            theme: "reel-dark".into(),
            font: None,
            font_size: 16.0,
            line_height: 1.35,
            window: WindowStyle::Plain,
            titlebar: Titlebar::None,
            titlebar_rule: false,
            corner_radius: 0.0,
            padding: 24.0,
            inset: 24.0,
            canvas: CanvasBg::Solid(hex("#000000")),
            shadow: None,
            border: Some(hex("#ffffff22")),
            crt: None,
        },
        "glass" => Template {
            name: "glass".into(),
            description: "Soft gradient, rounded chrome, generous air".into(),
            theme: "catppuccin-mocha".into(),
            font: None,
            font_size: 17.0,
            line_height: 1.45,
            window: WindowStyle::MacOs,
            titlebar: Titlebar::TrafficLights,
            titlebar_rule: false,
            corner_radius: 14.0,
            padding: 28.0,
            inset: 48.0,
            canvas: CanvasBg::Linear { angle_deg: 135.0, from: hex("#1a1a2e"), to: hex("#16213e") },
            shadow: Some(Shadow { blur: 42.0, opacity: 0.45, offset_y: 14.0 }),
            border: Some(hex("#ffffff12")),
            crt: None,
        },
        "classic" => Template {
            name: "classic".into(),
            description: "Bare terminal, no chrome — for purists and docs embeds".into(),
            theme: "reel-dark".into(),
            font: None,
            font_size: 16.0,
            line_height: 1.3,
            window: WindowStyle::None,
            titlebar: Titlebar::None,
            titlebar_rule: false,
            corner_radius: 0.0,
            padding: 12.0,
            inset: 0.0,
            canvas: CanvasBg::Solid(hex("#101014")),
            shadow: None,
            border: None,
            crt: None,
        },
        "geist" => Template {
            name: "geist".into(),
            description: "Pure black, Geist Mono, hairline border — deploy-preview energy".into(),
            theme: "geist-dark".into(),
            font: Some("Geist Mono".into()),
            font_size: 15.0,
            line_height: 1.55,
            window: WindowStyle::Rounded,
            titlebar: Titlebar::Dots,
            titlebar_rule: true,
            corner_radius: 12.0,
            padding: 34.0,
            inset: 26.0,
            canvas: CanvasBg::Solid(hex("#000000")),
            shadow: None,
            border: Some(hex("#ffffff2e")),
            crt: None,
        },
        "paper" => Template {
            name: "paper".into(),
            description: "Light background, for daytime documentation".into(),
            theme: "paper-light".into(),
            font: None,
            font_size: 16.0,
            line_height: 1.4,
            window: WindowStyle::Rounded,
            titlebar: Titlebar::None,
            titlebar_rule: false,
            corner_radius: 10.0,
            padding: 24.0,
            inset: 40.0,
            canvas: CanvasBg::Solid(hex("#e8e6df")),
            shadow: Some(Shadow { blur: 26.0, opacity: 0.18, offset_y: 8.0 }),
            border: Some(hex("#00000014")),
            crt: None,
        },
        "crt" => Template {
            name: "crt".into(),
            description: "Phosphor glow, scanlines, vignette — the shareable one".into(),
            theme: "phosphor".into(),
            font: None,
            font_size: 17.0,
            line_height: 1.3,
            window: WindowStyle::Rounded,
            titlebar: Titlebar::None,
            titlebar_rule: false,
            corner_radius: 16.0,
            padding: 30.0,
            inset: 34.0,
            canvas: CanvasBg::Solid(hex("#0b0b09")),
            shadow: Some(Shadow { blur: 34.0, opacity: 0.6, offset_y: 10.0 }),
            border: Some(hex("#2c2c22aa")),
            crt: Some(CRT_DEFAULT),
        },
        _ => return None,
    };
    Some(t)
}

pub fn template_names() -> &'static [&'static str] {
    &["minimal", "glass", "classic", "geist", "paper", "crt"]
}

pub fn parse_window_style(s: &str) -> Option<WindowStyle> {
    Some(match s {
        "macos" => WindowStyle::MacOs,
        "rounded" => WindowStyle::Rounded,
        "plain" => WindowStyle::Plain,
        "none" => WindowStyle::None,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// User templates: TOML files in the templates dir (`reel template add`)
// ---------------------------------------------------------------------------

/// The template TOML schema this reel reads and writes. Bump it when a
/// change would make older reels misrender a template (not merely ignore a
/// field) — third-party templates in the wild check against this.
pub const SCHEMA: u32 = 1;

#[derive(serde::Deserialize, serde::Serialize, Default)]
#[serde(deny_unknown_fields, default)]
struct TemplateFile {
    schema: Option<u32>,
    name: Option<String>,
    description: Option<String>,
    theme: Option<String>,
    /// Any installed font family name (resolved at render time).
    font: Option<String>,
    font_size: Option<f32>,
    line_height: Option<f32>,
    /// macos | rounded | plain | none
    window: Option<String>,
    /// none | traffic-lights | dots
    titlebar: Option<String>,
    titlebar_rule: Option<bool>,
    corner_radius: Option<f32>,
    padding: Option<f32>,
    inset: Option<f32>,
    border: Option<String>,
    canvas: Option<CanvasFile>,
    shadow: Option<ShadowFile>,
    crt: Option<CrtFile>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct CanvasFile {
    solid: Option<String>,
    gradient: Option<GradientFile>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct GradientFile {
    angle: f32,
    from: String,
    to: String,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ShadowFile {
    blur: f32,
    opacity: f32,
    offset_y: f32,
}

#[derive(serde::Deserialize, serde::Serialize, Default)]
#[serde(deny_unknown_fields, default)]
struct CrtFile {
    scanline: f32,
    glow: f32,
    vignette: f32,
}

/// Parses a user template TOML. Unset fields inherit from `minimal`.
pub fn from_toml(text: &str, fallback_name: &str) -> Result<Template, String> {
    // The schema gate must fire before field validation: a schema-2 template
    // will have fields `deny_unknown_fields` rejects, and "upgrade reel"
    // beats "unknown field `wobble`" as the error.
    let value: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
    if let Some(s) = value.get("schema").and_then(|v| v.as_integer()) {
        if s > SCHEMA as i64 {
            return Err(format!(
                "template schema {s} is newer than this reel understands \
                 (schema {SCHEMA}) — upgrade reel"
            ));
        }
    }
    let f: TemplateFile = value.try_into().map_err(|e: toml::de::Error| e.to_string())?;
    let mut t = builtin("minimal").expect("minimal exists");
    // Dimensions inherit sensible defaults; decorations are strictly opt-in
    // (an absent `border` must mean "no border", not "minimal's border").
    t.border = None;
    t.shadow = None;
    t.crt = None;
    t.name = f.name.unwrap_or_else(|| fallback_name.to_string());
    t.description = f.description.unwrap_or_default();
    if let Some(theme) = f.theme {
        t.theme = theme;
    }
    if f.font.is_some() {
        t.font = f.font;
    }
    if let Some(v) = f.font_size {
        t.font_size = v;
    }
    if let Some(v) = f.line_height {
        t.line_height = v;
    }
    if let Some(w) = &f.window {
        t.window = parse_window_style(w).ok_or_else(|| format!("unknown window `{w}`"))?;
    }
    if let Some(tb) = &f.titlebar {
        t.titlebar = match tb.as_str() {
            "none" => Titlebar::None,
            "traffic-lights" => Titlebar::TrafficLights,
            "dots" => Titlebar::Dots,
            other => return Err(format!("unknown titlebar `{other}`")),
        };
    }
    if let Some(v) = f.titlebar_rule {
        t.titlebar_rule = v;
    }
    if let Some(v) = f.corner_radius {
        t.corner_radius = v;
    }
    if let Some(v) = f.padding {
        t.padding = v;
    }
    if let Some(v) = f.inset {
        t.inset = v;
    }
    let color = |s: &str| Rgba::from_hex(s).ok_or_else(|| format!("bad color `{s}`"));
    if let Some(b) = &f.border {
        t.border = Some(color(b)?);
    }
    if let Some(c) = f.canvas {
        t.canvas = match (c.solid, c.gradient) {
            (Some(s), None) => CanvasBg::Solid(color(&s)?),
            (None, Some(g)) => CanvasBg::Linear {
                angle_deg: g.angle,
                from: color(&g.from)?,
                to: color(&g.to)?,
            },
            _ => return Err("canvas needs exactly one of `solid` or `gradient`".into()),
        };
    }
    if let Some(s) = f.shadow {
        t.shadow = Some(Shadow { blur: s.blur, opacity: s.opacity, offset_y: s.offset_y });
    }
    if let Some(c) = f.crt {
        t.crt = Some(CrtEffect { scanline: c.scanline, glow: c.glow, vignette: c.vignette });
    }
    Ok(t)
}

/// Serializes a template as the TOML `from_toml` reads — used by
/// `reel template show` so any builtin doubles as a starting point.
pub fn to_toml(t: &Template) -> String {
    let hex = |c: Rgba| {
        if c.a == 255 {
            format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a)
        }
    };
    let f = TemplateFile {
        schema: Some(SCHEMA),
        name: Some(t.name.clone()),
        description: (!t.description.is_empty()).then(|| t.description.clone()),
        theme: Some(t.theme.clone()),
        font: t.font.clone(),
        font_size: Some(t.font_size),
        line_height: Some(t.line_height),
        window: Some(
            match t.window {
                WindowStyle::MacOs => "macos",
                WindowStyle::Rounded => "rounded",
                WindowStyle::Plain => "plain",
                WindowStyle::None => "none",
            }
            .to_string(),
        ),
        titlebar: Some(
            match t.titlebar {
                Titlebar::None => "none",
                Titlebar::TrafficLights => "traffic-lights",
                Titlebar::Dots => "dots",
            }
            .to_string(),
        ),
        titlebar_rule: Some(t.titlebar_rule),
        corner_radius: Some(t.corner_radius),
        padding: Some(t.padding),
        inset: Some(t.inset),
        border: t.border.map(hex),
        canvas: Some(match t.canvas {
            CanvasBg::Solid(c) => CanvasFile { solid: Some(hex(c)), gradient: None },
            CanvasBg::Linear { angle_deg, from, to } => CanvasFile {
                solid: None,
                gradient: Some(GradientFile { angle: angle_deg, from: hex(from), to: hex(to) }),
            },
        }),
        shadow: t.shadow.as_ref().map(|s| ShadowFile {
            blur: s.blur,
            opacity: s.opacity,
            offset_y: s.offset_y,
        }),
        crt: t.crt.map(|c| CrtFile { scanline: c.scanline, glow: c.glow, vignette: c.vignette }),
    };
    toml::to_string_pretty(&f).expect("template serializes")
}

/// Resolves a template name: built-ins first, then the user templates dir.
/// Anything that looks like a path to a `.toml` file loads directly — so
/// `--template ./my.toml` works without installing, which is what
/// `reel template try` builds on.
pub fn lookup(name: &str) -> Option<Template> {
    if std::path::Path::new(name).extension().is_some_and(|e| e == "toml") {
        let path = std::path::Path::new(name);
        let stem = path.file_stem()?.to_str()?.to_string();
        let text = std::fs::read_to_string(path).ok()?;
        return from_toml(&text, &stem).ok();
    }
    if let Some(t) = builtin(name) {
        return Some(t);
    }
    let path = crate::paths::templates_dir()?.join(format!("{name}.toml"));
    let text = std::fs::read_to_string(path).ok()?;
    from_toml(&text, name).ok()
}

/// Names of templates installed in the user templates dir.
pub fn user_template_names() -> Vec<String> {
    let Some(dir) = crate::paths::templates_dir() else { return Vec::new() };
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
    fn every_builtin_roundtrips_through_toml() {
        for name in template_names() {
            let t = builtin(name).unwrap();
            let text = to_toml(&t);
            let back = from_toml(&text, "x").unwrap();
            assert_eq!(back.name, t.name);
            assert_eq!(back.theme, t.theme);
            assert_eq!(back.window, t.window);
            assert_eq!(back.titlebar, t.titlebar);
            assert_eq!(back.font_size, t.font_size);
            assert_eq!(back.padding, t.padding);
            assert_eq!(back.crt, t.crt, "{name} crt");
            assert_eq!(back.border, t.border);
        }
    }

    #[test]
    fn sparse_template_inherits_minimal_defaults() {
        let t = from_toml("theme = \"tokyo-night\"\n", "sparse").unwrap();
        let min = builtin("minimal").unwrap();
        assert_eq!(t.name, "sparse");
        assert_eq!(t.theme, "tokyo-night");
        assert_eq!(t.padding, min.padding);
        assert_eq!(t.window, min.window);
    }

    #[test]
    fn canvas_requires_exactly_one_kind() {
        let err = from_toml(
            "[canvas]\nsolid = \"#000000\"\ngradient = { angle = 90, from = \"#000000\", to = \"#ffffff\" }\n",
            "x",
        )
        .unwrap_err();
        assert!(err.contains("exactly one"));
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(from_toml("wobble = 3\n", "x").is_err());
    }

    #[test]
    fn current_and_older_schemas_are_accepted() {
        assert!(from_toml(&format!("schema = {SCHEMA}\n"), "x").is_ok());
        assert!(from_toml("theme = \"tokyo-night\"\n", "x").is_ok(), "absent schema means 1");
    }

    #[test]
    fn newer_schema_says_upgrade_even_with_unknown_fields() {
        let err = from_toml("schema = 99\nwobble = 3\n", "x").unwrap_err();
        assert!(err.contains("upgrade reel"), "got: {err}");
    }

    #[test]
    fn to_toml_stamps_the_schema() {
        let text = to_toml(&builtin("minimal").unwrap());
        assert!(text.contains(&format!("schema = {SCHEMA}")));
    }

    #[test]
    fn lookup_loads_a_toml_path_directly() {
        let dir = std::env::temp_dir().join("reel-template-path-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("neon.toml");
        std::fs::write(&path, "theme = \"phosphor\"\n").unwrap();
        let t = lookup(path.to_str().unwrap()).unwrap();
        assert_eq!(t.name, "neon");
        assert_eq!(t.theme, "phosphor");
    }
}
