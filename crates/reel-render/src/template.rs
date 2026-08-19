//! Templates: the complete visual package (font, theme, chrome, canvas).
//! Built-ins are code for now; the community-template TOML loader arrives
//! with the registry (Phase 3).

use crate::font::Family;
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
    pub name: &'static str,
    pub description: &'static str,
    pub theme: &'static str,
    pub family: Family,
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
}

fn hex(s: &str) -> Rgba {
    Rgba::from_hex(s).expect("builtin template hex")
}

pub fn builtin(name: &str) -> Option<Template> {
    let t = match name {
        "minimal" => Template {
            name: "minimal",
            description: "High contrast, square corners, no chrome noise",
            theme: "reel-dark",
            family: Family::JetBrainsMono,
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
        },
        "glass" => Template {
            name: "glass",
            description: "Soft gradient, rounded chrome, generous air",
            theme: "catppuccin-mocha",
            family: Family::JetBrainsMono,
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
        },
        "classic" => Template {
            name: "classic",
            description: "Bare terminal, no chrome — for purists and docs embeds",
            theme: "reel-dark",
            family: Family::JetBrainsMono,
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
        },
        "geist" => Template {
            name: "geist",
            description: "Pure black, Geist Mono, hairline border — deploy-preview energy",
            theme: "geist-dark",
            family: Family::GeistMono,
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
        },
        "paper" => Template {
            name: "paper",
            description: "Light background, for daytime documentation",
            theme: "paper-light",
            family: Family::JetBrainsMono,
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
        },
        _ => return None,
    };
    Some(t)
}

pub fn template_names() -> &'static [&'static str] {
    &["minimal", "glass", "classic", "geist", "paper"]
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
