//! Templates: the complete visual package (font, theme, chrome, canvas).
//! Built-ins are code for now; the community-template TOML loader arrives
//! with the registry (Phase 3).

use crate::fx::{CrtEffect, CRT_DEFAULT};
use crate::image::{ImageFit, LoadedImage};
use crate::theme::Rgba;
use reel_term::CursorShape;
use std::path::Path;

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

#[derive(Debug, Clone, PartialEq)]
pub enum CanvasBg {
    Solid(Rgba),
    /// Multi-stop linear gradient with an angle in degrees (CSS convention:
    /// 0 = up, 90 = right). Stops are (position 0..1, color), sorted.
    Linear { angle_deg: f32, stops: Vec<(f32, Rgba)> },
    /// Multi-stop radial gradient from the canvas center outward.
    Radial { stops: Vec<(f32, Rgba)> },
    /// Wallpaper image with optional darkening and blur for text contrast.
    Image { img: LoadedImage, fit: ImageFit, dim: f32, blur: f32 },
}

impl CanvasBg {
    /// Convenience for the common two-stop linear case.
    pub fn linear(angle_deg: f32, from: Rgba, to: Rgba) -> Self {
        CanvasBg::Linear { angle_deg, stops: vec![(0.0, from), (1.0, to)] }
    }
}

/// Corner anchor for badges/watermarks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "top-left" => Corner::TopLeft,
            "top-right" => Corner::TopRight,
            "bottom-left" => Corner::BottomLeft,
            "bottom-right" => Corner::BottomRight,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Corner::TopLeft => "top-left",
            Corner::TopRight => "top-right",
            Corner::BottomLeft => "bottom-left",
            Corner::BottomRight => "bottom-right",
        }
    }
}

/// A small watermark drawn on the canvas corner — brand text or a logo.
#[derive(Debug, Clone, PartialEq)]
pub struct Badge {
    pub text: Option<String>,
    pub image: Option<LoadedImage>,
    pub corner: Corner,
    pub opacity: f32,
}

/// How the terminal path shows in an injected prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptPath {
    None,
    /// Just the current directory name (`~`, `app`).
    Short,
    /// The full working directory.
    Full,
}

/// A branded shell prompt: `reel run` injects it into the shell it spawns
/// (recordings of your real terminal keep your real prompt).
#[derive(Debug, Clone, PartialEq)]
pub struct Prompt {
    /// The leading glyph — "▲", "❯", "$", "λ"…
    pub symbol: String,
    pub color: Option<Rgba>,
    pub path: PromptPath,
}

/// Motion effects that make renders feel alive. All off by default: they
/// add frames, and change-driven frame planning is reel's size advantage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    /// Cursor slides between cells over this many ms (0 = jump like a real
    /// terminal).
    pub cursor_slide_ms: f32,
    /// Freshly typed cells glow and decay; 0..1 strength, 0 = off.
    pub typing_glow: f32,
}

impl Motion {
    pub const OFF: Motion = Motion { cursor_slide_ms: 0.0, typing_glow: 0.0 };

    pub fn is_off(&self) -> bool {
        *self == Motion::OFF
    }
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
    /// Palette embedded in the template file itself. When set it wins over
    /// the `theme` name — this is what makes a published template
    /// self-contained: installers don't need the author's theme installed.
    pub theme_colors: Option<crate::theme::Theme>,
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
    /// Window body alpha (1 = opaque). Below 1 the canvas shows through the
    /// terminal background — glassmorphism.
    pub window_opacity: f32,
    /// Backdrop blur radius in logical px applied to the canvas behind the
    /// window. Pairs with `window_opacity` for the frosted-glass look.
    pub window_blur: f32,
    /// Text centered in the titlebar.
    pub title: Option<String>,
    /// Film-grain strength (0..1) over the canvas background.
    pub grain: f32,
    /// Forces the cursor shape regardless of what the recording set.
    pub cursor_style: Option<CursorShape>,
    /// Cursor color override (default: the theme's).
    pub cursor_color: Option<Rgba>,
    pub badge: Option<Badge>,
    pub prompt: Option<Prompt>,
    pub motion: Motion,
}

fn hex(s: &str) -> Rgba {
    Rgba::from_hex(s).expect("builtin template hex")
}

/// The neutral field set every builtin starts from — also the inheritance
/// base for sparse user TOML templates.
fn neutral() -> Template {
    Template {
        name: String::new(),
        description: String::new(),
        theme: "reel-dark".into(),
        theme_colors: None,
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
        border: None,
        crt: None,
        window_opacity: 1.0,
        window_blur: 0.0,
        title: None,
        grain: 0.0,
        cursor_style: None,
        cursor_color: None,
        badge: None,
        prompt: None,
        motion: Motion::OFF,
    }
}

pub fn builtin(name: &str) -> Option<Template> {
    let t = match name {
        "minimal" => Template {
            name: "minimal".into(),
            description: "High contrast, square corners, no chrome noise".into(),
            border: Some(hex("#ffffff22")),
            ..neutral()
        },
        "glass" => Template {
            name: "glass".into(),
            description: "Soft gradient, rounded chrome, generous air".into(),
            theme: "catppuccin-mocha".into(),
            font_size: 17.0,
            line_height: 1.45,
            window: WindowStyle::MacOs,
            titlebar: Titlebar::TrafficLights,
            corner_radius: 14.0,
            padding: 28.0,
            inset: 48.0,
            canvas: CanvasBg::linear(135.0, hex("#1a1a2e"), hex("#16213e")),
            shadow: Some(Shadow { blur: 42.0, opacity: 0.45, offset_y: 14.0 }),
            border: Some(hex("#ffffff12")),
            ..neutral()
        },
        "classic" => Template {
            name: "classic".into(),
            description: "Bare terminal, no chrome — for purists and docs embeds".into(),
            line_height: 1.3,
            window: WindowStyle::None,
            padding: 12.0,
            inset: 0.0,
            canvas: CanvasBg::Solid(hex("#101014")),
            ..neutral()
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
            border: Some(hex("#ffffff2e")),
            prompt: Some(Prompt {
                symbol: "▲".into(),
                color: None,
                path: PromptPath::Short,
            }),
            ..neutral()
        },
        "paper" => Template {
            name: "paper".into(),
            description: "Light background, for daytime documentation".into(),
            theme: "paper-light".into(),
            line_height: 1.4,
            window: WindowStyle::Rounded,
            corner_radius: 10.0,
            inset: 40.0,
            canvas: CanvasBg::Solid(hex("#e8e6df")),
            shadow: Some(Shadow { blur: 26.0, opacity: 0.18, offset_y: 8.0 }),
            border: Some(hex("#00000014")),
            ..neutral()
        },
        "crt" => Template {
            name: "crt".into(),
            description: "Phosphor glow, scanlines, vignette — the shareable one".into(),
            theme: "phosphor".into(),
            font_size: 17.0,
            line_height: 1.3,
            window: WindowStyle::Rounded,
            corner_radius: 16.0,
            padding: 30.0,
            inset: 34.0,
            canvas: CanvasBg::Solid(hex("#0b0b09")),
            shadow: Some(Shadow { blur: 34.0, opacity: 0.6, offset_y: 10.0 }),
            border: Some(hex("#2c2c22aa")),
            crt: Some(CRT_DEFAULT),
            ..neutral()
        },
        "aurora" => Template {
            name: "aurora".into(),
            description: "Frosted glass over a radial glow — the showcase template".into(),
            theme: "tokyo-night".into(),
            font_size: 17.0,
            line_height: 1.45,
            window: WindowStyle::MacOs,
            titlebar: Titlebar::TrafficLights,
            corner_radius: 16.0,
            padding: 30.0,
            inset: 52.0,
            canvas: CanvasBg::Radial {
                stops: vec![
                    (0.0, hex("#2b2350")),
                    (0.55, hex("#171233")),
                    (1.0, hex("#0a0817")),
                ],
            },
            shadow: Some(Shadow { blur: 48.0, opacity: 0.5, offset_y: 16.0 }),
            border: Some(hex("#ffffff1c")),
            window_opacity: 0.86,
            window_blur: 16.0,
            grain: 0.05,
            motion: Motion { cursor_slide_ms: 90.0, typing_glow: 0.5 },
            prompt: Some(Prompt {
                symbol: "❯".into(),
                color: Some(hex("#7aa2f7")),
                path: PromptPath::Short,
            }),
            ..neutral()
        },
        _ => return None,
    };
    Some(t)
}

pub fn template_names() -> &'static [&'static str] {
    &["minimal", "glass", "classic", "geist", "paper", "crt", "aurora"]
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
///
/// Schema 2 added: inline `[theme]` palettes, multi-stop/radial gradients,
/// image canvases, grain, `window_opacity`/`window_blur`, `title`,
/// `[cursor]`, `[badge]`, `[prompt]`, and `[motion]`.
pub const SCHEMA: u32 = 2;

#[derive(serde::Deserialize, serde::Serialize, Default)]
#[serde(deny_unknown_fields, default)]
struct TemplateFile {
    schema: Option<u32>,
    name: Option<String>,
    description: Option<String>,
    /// A theme name (`theme = "tokyo-night"`) or an inline palette table
    /// (`[theme]` with fg/bg/cursor/ansi) — the latter makes the file
    /// self-contained for publishing.
    theme: Option<toml::Value>,
    /// Any installed font family name (resolved at render time).
    font: Option<String>,
    font_size: Option<f64>,
    line_height: Option<f64>,
    /// macos | rounded | plain | none
    window: Option<String>,
    /// none | traffic-lights | dots
    titlebar: Option<String>,
    titlebar_rule: Option<bool>,
    corner_radius: Option<f64>,
    padding: Option<f64>,
    inset: Option<f64>,
    border: Option<String>,
    /// Window body alpha, 0..1.
    window_opacity: Option<f64>,
    /// Backdrop blur (logical px) behind a translucent window.
    window_blur: Option<f64>,
    /// Titlebar text.
    title: Option<String>,
    canvas: Option<CanvasFile>,
    shadow: Option<ShadowFile>,
    crt: Option<CrtFile>,
    cursor: Option<CursorFile>,
    badge: Option<BadgeFile>,
    prompt: Option<PromptFile>,
    motion: Option<MotionFile>,
}

#[derive(serde::Deserialize, serde::Serialize, Default)]
#[serde(deny_unknown_fields, default)]
struct CanvasFile {
    solid: Option<String>,
    gradient: Option<GradientFile>,
    image: Option<ImageFile>,
    /// Film-grain strength 0..1; composes with any background kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    grain: Option<f64>,
}

#[derive(serde::Deserialize, serde::Serialize, Default)]
#[serde(deny_unknown_fields, default)]
struct GradientFile {
    /// linear (default) | radial
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    angle: Option<f64>,
    /// Two-stop shorthand…
    #[serde(skip_serializing_if = "Option::is_none")]
    from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to: Option<String>,
    /// …or explicit stops: [["#1a0b2e", 0.0], ["#3d1d6b", 1.0]].
    #[serde(skip_serializing_if = "Option::is_none")]
    stops: Option<Vec<(String, f64)>>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ImageFile {
    /// Relative paths resolve against the template's own directory.
    path: String,
    /// cover (default) | contain | tile
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dim: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    blur: Option<f64>,
}

#[derive(serde::Deserialize, serde::Serialize, Default)]
#[serde(deny_unknown_fields, default)]
struct CursorFile {
    /// block | beam | underline
    #[serde(skip_serializing_if = "Option::is_none")]
    style: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<String>,
}

#[derive(serde::Deserialize, serde::Serialize, Default)]
#[serde(deny_unknown_fields, default)]
struct BadgeFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<String>,
    /// top-left | top-right | bottom-left | bottom-right (default)
    #[serde(skip_serializing_if = "Option::is_none")]
    position: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    opacity: Option<f64>,
}

#[derive(serde::Deserialize, serde::Serialize, Default)]
#[serde(deny_unknown_fields, default)]
struct PromptFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    /// none | short (default) | full
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

#[derive(serde::Deserialize, serde::Serialize, Default)]
#[serde(deny_unknown_fields, default)]
struct MotionFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor_slide: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    slide_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    typing_glow: Option<f64>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ShadowFile {
    blur: f64,
    opacity: f64,
    offset_y: f64,
}

#[derive(serde::Deserialize, serde::Serialize, Default)]
#[serde(deny_unknown_fields, default)]
struct CrtFile {
    scanline: f64,
    glow: f64,
    vignette: f64,
}

/// Parses a user template TOML. Unset fields inherit from `minimal`.
/// Relative image paths resolve against the process's working directory —
/// use [`from_toml_at`] when the template file's location is known.
pub fn from_toml(text: &str, fallback_name: &str) -> Result<Template, String> {
    from_toml_at(text, fallback_name, None)
}

/// [`from_toml`] with a base directory for the template's image assets.
pub fn from_toml_at(
    text: &str,
    fallback_name: &str,
    base_dir: Option<&Path>,
) -> Result<Template, String> {
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
    match f.theme {
        Some(toml::Value::String(name)) => t.theme = name,
        Some(table @ toml::Value::Table(_)) => {
            let inline = crate::theme::from_value(table, &t.name)
                .map_err(|e| format!("inline [theme]: {e}"))?;
            t.theme = inline.name.clone();
            t.theme_colors = Some(inline);
        }
        Some(other) => {
            return Err(format!(
                "`theme` must be a name string or an inline palette table, got {}",
                other.type_str()
            ))
        }
        None => {}
    }
    if f.font.is_some() {
        t.font = f.font;
    }
    if let Some(v) = f.font_size {
        t.font_size = v as f32;
    }
    if let Some(v) = f.line_height {
        t.line_height = v as f32;
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
        t.corner_radius = v as f32;
    }
    if let Some(v) = f.padding {
        t.padding = v as f32;
    }
    if let Some(v) = f.inset {
        t.inset = v as f32;
    }
    let color = |s: &str| Rgba::from_hex(s).ok_or_else(|| format!("bad color `{s}`"));
    if let Some(b) = &f.border {
        t.border = Some(color(b)?);
    }
    if let Some(v) = f.window_opacity {
        if !(0.0..=1.0).contains(&v) {
            return Err(format!("window_opacity {v} out of range 0..1"));
        }
        t.window_opacity = v as f32;
    }
    if let Some(v) = f.window_blur {
        t.window_blur = v.max(0.0) as f32;
    }
    if f.title.is_some() {
        t.title = f.title;
    }
    if let Some(c) = f.canvas {
        if let Some(g) = c.grain {
            t.grain = g.clamp(0.0, 1.0) as f32;
        }
        t.canvas = match (c.solid, c.gradient, c.image) {
            (Some(s), None, None) => CanvasBg::Solid(color(&s)?),
            (None, Some(g), None) => {
                let stops = match (&g.stops, &g.from, &g.to) {
                    (Some(stops), None, None) => {
                        if stops.len() < 2 {
                            return Err("gradient needs at least 2 stops".into());
                        }
                        let mut out = Vec::with_capacity(stops.len());
                        for (c, pos) in stops {
                            if !(0.0..=1.0).contains(pos) {
                                return Err(format!("gradient stop {pos} out of range 0..1"));
                            }
                            out.push((*pos as f32, color(c)?));
                        }
                        out.sort_by(|a, b| a.0.total_cmp(&b.0));
                        out
                    }
                    (None, Some(from), Some(to)) => {
                        vec![(0.0, color(from)?), (1.0, color(to)?)]
                    }
                    _ => {
                        return Err(
                            "gradient needs either `from`+`to` or a `stops` list".into()
                        )
                    }
                };
                match g.kind.as_deref().unwrap_or("linear") {
                    "linear" => CanvasBg::Linear {
                        angle_deg: g.angle.unwrap_or(180.0) as f32,
                        stops,
                    },
                    "radial" => CanvasBg::Radial { stops },
                    other => return Err(format!("unknown gradient kind `{other}`")),
                }
            }
            (None, None, Some(img)) => {
                let fit = match img.fit.as_deref() {
                    None => ImageFit::Cover,
                    Some(s) => ImageFit::parse(s)
                        .ok_or_else(|| format!("unknown image fit `{s}`"))?,
                };
                CanvasBg::Image {
                    img: crate::image::load(&img.path, base_dir)?,
                    fit,
                    dim: img.dim.unwrap_or(0.0).clamp(0.0, 1.0) as f32,
                    blur: img.blur.unwrap_or(0.0).max(0.0) as f32,
                }
            }
            (None, None, None) => t.canvas,
            _ => {
                return Err(
                    "canvas needs exactly one of `solid`, `gradient`, or `image`".into()
                )
            }
        };
    }
    if let Some(s) = f.shadow {
        t.shadow = Some(Shadow {
            blur: s.blur as f32,
            opacity: s.opacity as f32,
            offset_y: s.offset_y as f32,
        });
    }
    if let Some(c) = f.crt {
        t.crt = Some(CrtEffect {
            scanline: c.scanline as f32,
            glow: c.glow as f32,
            vignette: c.vignette as f32,
        });
    }
    if let Some(c) = f.cursor {
        t.cursor_style = match c.style.as_deref() {
            None => None,
            Some("block") => Some(CursorShape::Block),
            Some("beam") => Some(CursorShape::Beam),
            Some("underline") => Some(CursorShape::Underline),
            Some(other) => return Err(format!("unknown cursor style `{other}`")),
        };
        if let Some(col) = &c.color {
            t.cursor_color = Some(color(col)?);
        }
    }
    if let Some(b) = f.badge {
        if b.text.is_none() && b.image.is_none() {
            return Err("badge needs `text` or `image`".into());
        }
        t.badge = Some(Badge {
            text: b.text,
            image: match &b.image {
                Some(path) => Some(crate::image::load(path, base_dir)?),
                None => None,
            },
            corner: match b.position.as_deref() {
                None => Corner::BottomRight,
                Some(s) => {
                    Corner::parse(s).ok_or_else(|| format!("unknown badge position `{s}`"))?
                }
            },
            opacity: b.opacity.unwrap_or(0.6).clamp(0.0, 1.0) as f32,
        });
    }
    if let Some(p) = f.prompt {
        t.prompt = Some(Prompt {
            symbol: p.symbol.unwrap_or_else(|| "❯".into()),
            color: match &p.color {
                Some(c) => Some(color(c)?),
                None => None,
            },
            path: match p.path.as_deref() {
                None | Some("short") => PromptPath::Short,
                Some("none") => PromptPath::None,
                Some("full") => PromptPath::Full,
                Some(other) => return Err(format!("unknown prompt path `{other}`")),
            },
        });
    }
    if let Some(m) = f.motion {
        let slide_on = m.cursor_slide.unwrap_or(m.slide_ms.is_some());
        t.motion = Motion {
            cursor_slide_ms: if slide_on {
                m.slide_ms.unwrap_or(90.0).max(0.0) as f32
            } else {
                0.0
            },
            typing_glow: m.typing_glow.unwrap_or(0.0).clamp(0.0, 1.0) as f32,
        };
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
    // f32 -> f64 for TOML without float-expansion noise (1.35f32 would
    // otherwise print as 1.350000023841858).
    let clean = |v: f32| (f64::from(v) * 10_000.0).round() / 10_000.0;
    let f = TemplateFile {
        schema: Some(SCHEMA),
        name: Some(t.name.clone()),
        description: (!t.description.is_empty()).then(|| t.description.clone()),
        // An inline palette is a TOML table, and tables must follow the
        // scalar keys — so it's appended as a `[theme]` section below.
        theme: t.theme_colors.is_none().then(|| toml::Value::String(t.theme.clone())),
        font: t.font.clone(),
        font_size: Some(clean(t.font_size)),
        line_height: Some(clean(t.line_height)),
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
        corner_radius: Some(clean(t.corner_radius)),
        padding: Some(clean(t.padding)),
        inset: Some(clean(t.inset)),
        border: t.border.map(hex),
        window_opacity: (t.window_opacity < 1.0).then_some(clean(t.window_opacity)),
        window_blur: (t.window_blur > 0.0).then_some(clean(t.window_blur)),
        title: t.title.clone(),
        canvas: Some({
            let grain = (t.grain > 0.0).then_some(clean(t.grain));
            let stops_file = |stops: &[(f32, Rgba)]| {
                stops.iter().map(|(p, c)| (hex(*c), clean(*p))).collect::<Vec<_>>()
            };
            match &t.canvas {
                CanvasBg::Solid(c) => {
                    CanvasFile { solid: Some(hex(*c)), grain, ..Default::default() }
                }
                CanvasBg::Linear { angle_deg, stops } => CanvasFile {
                    gradient: Some(GradientFile {
                        angle: Some(clean(*angle_deg)),
                        stops: Some(stops_file(stops)),
                        ..Default::default()
                    }),
                    grain,
                    ..Default::default()
                },
                CanvasBg::Radial { stops } => CanvasFile {
                    gradient: Some(GradientFile {
                        kind: Some("radial".into()),
                        stops: Some(stops_file(stops)),
                        ..Default::default()
                    }),
                    grain,
                    ..Default::default()
                },
                CanvasBg::Image { img, fit, dim, blur } => CanvasFile {
                    image: Some(ImageFile {
                        path: img.path.clone(),
                        fit: Some(fit.name().into()),
                        dim: (*dim > 0.0).then_some(clean(*dim)),
                        blur: (*blur > 0.0).then_some(clean(*blur)),
                    }),
                    grain,
                    ..Default::default()
                },
            }
        }),
        shadow: t.shadow.as_ref().map(|s| ShadowFile {
            blur: clean(s.blur),
            opacity: clean(s.opacity),
            offset_y: clean(s.offset_y),
        }),
        crt: t.crt.map(|c| CrtFile {
            scanline: clean(c.scanline),
            glow: clean(c.glow),
            vignette: clean(c.vignette),
        }),
        cursor: (t.cursor_style.is_some() || t.cursor_color.is_some()).then(|| CursorFile {
            style: t.cursor_style.map(|s| {
                match s {
                    CursorShape::Block => "block",
                    CursorShape::Beam => "beam",
                    CursorShape::Underline => "underline",
                    CursorShape::Hidden => "block",
                }
                .to_string()
            }),
            color: t.cursor_color.map(hex),
        }),
        badge: t.badge.as_ref().map(|b| BadgeFile {
            text: b.text.clone(),
            image: b.image.as_ref().map(|i| i.path.clone()),
            position: Some(b.corner.name().into()),
            opacity: Some(clean(b.opacity)),
        }),
        prompt: t.prompt.as_ref().map(|p| PromptFile {
            symbol: Some(p.symbol.clone()),
            color: p.color.map(hex),
            path: Some(
                match p.path {
                    PromptPath::None => "none",
                    PromptPath::Short => "short",
                    PromptPath::Full => "full",
                }
                .into(),
            ),
        }),
        motion: (!t.motion.is_off()).then(|| MotionFile {
            cursor_slide: Some(t.motion.cursor_slide_ms > 0.0),
            slide_ms: (t.motion.cursor_slide_ms > 0.0)
                .then_some(clean(t.motion.cursor_slide_ms)),
            typing_glow: (t.motion.typing_glow > 0.0).then_some(clean(t.motion.typing_glow)),
        }),
    };
    let mut out = toml::to_string_pretty(&f).expect("template serializes");
    if let Some(colors) = &t.theme_colors {
        out.push_str("\n[theme]\n");
        out.push_str(&crate::theme::to_toml(colors));
    }
    out
}

/// Relative paths of image files this template references (canvas
/// wallpaper, badge logo). Publish refuses these (registry packs are
/// TOML-only) and local installs copy them next to the installed TOML.
pub fn referenced_images(t: &Template) -> Vec<String> {
    let mut out = Vec::new();
    if let CanvasBg::Image { img, .. } = &t.canvas {
        out.push(img.path.clone());
    }
    if let Some(badge) = &t.badge {
        if let Some(img) = &badge.image {
            out.push(img.path.clone());
        }
    }
    out
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
        return from_toml_at(&text, &stem, path.parent()).ok();
    }
    if let Some(t) = builtin(name) {
        return Some(t);
    }
    let dir = crate::paths::templates_dir()?;
    let text = std::fs::read_to_string(dir.join(format!("{name}.toml"))).ok()?;
    from_toml_at(&text, name, Some(&dir)).ok()
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
    fn inline_theme_parses_and_roundtrips() {
        let toml = r##"
description = "self-contained"

[theme]
name = "custom-glow"
fg = "#e6e6eb"
bg = "#101014"
cursor = "#8ab4f8"
ansi = [
    "#111111", "#222222", "#333333", "#444444",
    "#555555", "#666666", "#777777", "#888888",
    "#999999", "#aaaaaa", "#bbbbbb", "#cccccc",
    "#dddddd", "#eeeeee", "#ffffff", "#000000",
]
"##;
        let t = from_toml(toml, "portable").unwrap();
        let colors = t.theme_colors.as_ref().expect("inline palette");
        assert_eq!(t.theme, "custom-glow");
        assert_eq!(colors.bg, Rgba::rgb(0x10, 0x10, 0x14));

        let back = from_toml(&to_toml(&t), "x").unwrap();
        let back_colors = back.theme_colors.expect("palette survives roundtrip");
        assert_eq!(back_colors.fg, colors.fg);
        assert_eq!(back_colors.ansi, colors.ansi);
    }

    #[test]
    fn theme_rejects_non_string_non_table() {
        let err = from_toml("theme = 3\n", "x").unwrap_err();
        assert!(err.contains("name string or an inline palette"), "got: {err}");
    }

    #[test]
    fn schema2_fields_parse() {
        let t = from_toml(
            r##"
schema = 2
theme = "tokyo-night"
window = "macos"
titlebar = "traffic-lights"
title = "~/app"
window_opacity = 0.85
window_blur = 12.0

[canvas]
grain = 0.06
[canvas.gradient]
kind = "radial"
stops = [["#2b2350", 0.0], ["#171233", 0.6], ["#0a0817", 1.0]]

[cursor]
style = "beam"
color = "#ff2d8d"

[badge]
text = "reel"
position = "top-left"
opacity = 0.4

[prompt]
symbol = "▲"
color = "#ffffff"
path = "short"

[motion]
cursor_slide = true
typing_glow = 0.5
"##,
            "x",
        )
        .unwrap();
        assert_eq!(t.window_opacity, 0.85);
        assert_eq!(t.window_blur, 12.0);
        assert_eq!(t.title.as_deref(), Some("~/app"));
        assert_eq!(t.grain, 0.06);
        assert!(matches!(&t.canvas, CanvasBg::Radial { stops } if stops.len() == 3));
        assert_eq!(t.cursor_style, Some(CursorShape::Beam));
        assert_eq!(t.cursor_color, Some(Rgba::from_hex("#ff2d8d").unwrap()));
        let b = t.badge.unwrap();
        assert_eq!(b.text.as_deref(), Some("reel"));
        assert_eq!(b.corner, Corner::TopLeft);
        let p = t.prompt.unwrap();
        assert_eq!(p.symbol, "▲");
        assert_eq!(p.path, PromptPath::Short);
        assert_eq!(t.motion, Motion { cursor_slide_ms: 90.0, typing_glow: 0.5 });
    }

    #[test]
    fn gradient_shorthand_and_stops_are_exclusive() {
        let err = from_toml(
            "[canvas.gradient]\nfrom = \"#000000\"\nto = \"#ffffff\"\nstops = [[\"#000000\", 0.0], [\"#ffffff\", 1.0]]\n",
            "x",
        )
        .unwrap_err();
        assert!(err.contains("either"), "got: {err}");
    }

    #[test]
    fn gradient_stops_sort_and_validate() {
        let t = from_toml(
            "[canvas.gradient]\nangle = 90\nstops = [[\"#ffffff\", 1.0], [\"#000000\", 0.0]]\n",
            "x",
        )
        .unwrap();
        let CanvasBg::Linear { stops, .. } = &t.canvas else { panic!("not linear") };
        assert_eq!(stops[0].0, 0.0);
        assert!(from_toml(
            "[canvas.gradient]\nstops = [[\"#000000\", 0.0], [\"#ffffff\", 1.5]]\n",
            "x"
        )
        .is_err());
        assert!(from_toml("[canvas.gradient]\nstops = [[\"#000000\", 0.0]]\n", "x").is_err());
    }

    #[test]
    fn missing_canvas_image_is_an_error() {
        let err = from_toml("[canvas.image]\npath = \"nope-does-not-exist.png\"\n", "x")
            .unwrap_err();
        assert!(err.contains("nope-does-not-exist"), "got: {err}");
    }

    #[test]
    fn badge_needs_content() {
        assert!(from_toml("[badge]\nopacity = 0.5\n", "x").is_err());
    }

    #[test]
    fn slide_ms_alone_enables_the_slide() {
        let t = from_toml("[motion]\nslide_ms = 120\n", "x").unwrap();
        assert_eq!(t.motion.cursor_slide_ms, 120.0);
        let off = from_toml("[motion]\ntyping_glow = 0.3\n", "x").unwrap();
        assert_eq!(off.motion.cursor_slide_ms, 0.0);
        assert_eq!(off.motion.typing_glow, 0.3);
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
