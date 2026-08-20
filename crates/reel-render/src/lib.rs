//! reel-render: snapshots + frame plans in, composited RGBA frames out.

pub mod chrome;
pub mod font;
pub mod fx;
pub mod paths;
pub mod plan;
pub mod raster;
pub mod template;
pub mod theme;

pub use plan::{plan, Camera, FramePlan};
pub use template::{Template, WindowStyle};
pub use theme::{Rgba, Theme};
pub use tiny_skia::Pixmap;

use font::{GlyphPixels, Rasterizer, Variant};
use raster::GridStyle;
use reel_format::ReelConfig;
use reel_term::Snapshot;
use reel_timeline::CaptionPos;

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("unknown template `{0}` (built-ins: {1})")]
    UnknownTemplate(String, String),
    #[error("unknown theme `{0}` (built-ins: {1})")]
    UnknownTheme(String, String),
    #[error("unknown window style `{0}` (expected macos, rounded, plain, or none)")]
    UnknownWindow(String),
    #[error("{0}")]
    Font(String),
    #[error("invalid aspect `{0}` (try 16:9, 4:3, or 1.78)")]
    BadAspect(String),
}

#[derive(Debug, Clone)]
pub struct RenderSettings {
    pub template: Template,
    pub theme: Theme,
    /// Supersampling factor: output pixels per logical pixel.
    pub scale: f32,
    pub fps: u32,
    /// Minimum canvas width/height ratio; the canvas pads out to reach it.
    pub aspect: Option<f32>,
}

/// Layering per the spec: built-in defaults → template → `[style]` overrides.
/// (CLI flags are applied by the caller before this.)
pub fn settings_from_config(cfg: &ReelConfig) -> Result<(RenderSettings, Vec<String>), RenderError> {
    let warnings = Vec::new();
    let mut tpl = template::lookup(&cfg.template.name).ok_or_else(|| {
        let mut names: Vec<String> =
            template::template_names().iter().map(|s| s.to_string()).collect();
        names.extend(template::user_template_names());
        RenderError::UnknownTemplate(cfg.template.name.clone(), names.join(", "))
    })?;

    let style = &cfg.style;
    if let Some(fs) = style.font_size {
        tpl.font_size = fs;
    }
    if let Some(lh) = style.line_height {
        tpl.line_height = lh;
    }
    if let Some(p) = style.padding {
        tpl.padding = p as f32;
    }
    if let Some(w) = &style.window {
        tpl.window =
            template::parse_window_style(w).ok_or_else(|| RenderError::UnknownWindow(w.clone()))?;
    }
    if let Some(f) = &style.font {
        // Resolved against installed fonts when the renderer is built.
        tpl.font = Some(f.clone());
    }

    let theme_name = style.theme.as_deref().unwrap_or(tpl.theme.as_str());
    let theme = theme::lookup(theme_name).ok_or_else(|| {
        let mut names: Vec<String> =
            theme::theme_names().iter().map(|s| s.to_string()).collect();
        names.extend(theme::user_theme_names());
        RenderError::UnknownTheme(theme_name.to_string(), names.join(", "))
    })?;

    let aspect = match &cfg.output.aspect {
        Some(a) => Some(
            reel_format::parse_aspect(a).ok_or_else(|| RenderError::BadAspect(a.clone()))?,
        ),
        None => None,
    };
    Ok((
        RenderSettings {
            template: tpl,
            theme,
            scale: cfg.output.scale.clamp(1, 4) as f32,
            fps: cfg.output.fps.unwrap_or(30).clamp(1, 120),
            aspect,
        },
        warnings,
    ))
}

pub struct Renderer {
    pub settings: RenderSettings,
    raster: Rasterizer,
    /// Cached static chrome (canvas bg + shadow + window) keyed by term size.
    chrome_base: Option<((u32, u32), Pixmap)>,
}

impl Renderer {
    /// Builds a renderer against the installed system fonts. The warnings
    /// (e.g. the template's preferred font missing) are for the caller to
    /// surface.
    pub fn new(settings: RenderSettings) -> Result<(Self, Vec<String>), RenderError> {
        let (raster, warning) = Rasterizer::new(settings.template.font.as_deref())
            .map_err(RenderError::Font)?;
        Ok((
            Renderer { settings, raster, chrome_base: None },
            warning.into_iter().collect(),
        ))
    }

    /// Swaps settings while keeping the font database and glyph cache warm
    /// (faces persist; the primary family re-resolves). Used by `reel watch`.
    pub fn set_settings(&mut self, settings: RenderSettings) -> Result<Vec<String>, RenderError> {
        let warning = self
            .raster
            .fonts
            .set_primary(settings.template.font.as_deref())
            .map_err(RenderError::Font)?;
        self.settings = settings;
        self.chrome_base = None;
        Ok(warning.into_iter().collect())
    }

    fn base_font_px(&self) -> f32 {
        self.settings.template.font_size * self.settings.scale
    }

    /// Terminal image size in pixels for a given grid.
    pub fn term_size(&mut self, cols: u16, rows: u16) -> (u32, u32) {
        let m = self.raster.fonts.cell_metrics(
            self.base_font_px(),
            self.settings.template.line_height,
        );
        (
            (cols as f32 * m.cell_w).ceil() as u32,
            (rows as f32 * m.cell_h).ceil() as u32,
        )
    }

    /// Full output frame size (canvas including chrome).
    pub fn canvas_size(&mut self, cols: u16, rows: u16) -> (u32, u32) {
        let (tw, th) = self.term_size(cols, rows);
        let l = chrome::layout(&self.settings.template, tw, th, self.settings.scale, self.settings.aspect);
        (l.canvas_w, l.canvas_h)
    }

    /// Renders one planned frame to a composited canvas.
    pub fn render_frame(&mut self, snap: &Snapshot, frame: &FramePlan) -> Pixmap {
        let s = self.settings.scale;
        let tpl = self.settings.template.clone();
        let theme = self.settings.theme.clone();
        let base_px = self.base_font_px();
        let base_m = self.raster.fonts.cell_metrics(base_px, tpl.line_height);
        let term_w = (snap.cols as f32 * base_m.cell_w).ceil() as u32;
        let term_h = (snap.rows as f32 * base_m.cell_h).ceil() as u32;

        let z = frame.camera.zoom.max(1.0);
        // (viewport origin in zoomed px, current cell size) for overlay math.
        let (mut term, view_off, cur_cell) = if z > 1.0001 {
            // Re-rasterize at the zoomed size so text stays sharp — never
            // upscale pixels.
            let zoom_px = base_px * z as f32;
            let zm = self.raster.fonts.cell_metrics(zoom_px, tpl.line_height);
            let big = raster::raster_grid(
                &mut self.raster,
                snap,
                &GridStyle { theme: &theme, font_size: zoom_px, line_height: tpl.line_height },
            );
            let cx = (frame.camera.center.0 as f32 + 0.5) * zm.cell_w;
            let cy = (frame.camera.center.1 as f32 + 0.5) * zm.cell_h;
            let vx = (cx - term_w as f32 / 2.0)
                .clamp(0.0, (big.width() as f32 - term_w as f32).max(0.0))
                .round() as i32;
            let vy = (cy - term_h as f32 / 2.0)
                .clamp(0.0, (big.height() as f32 - term_h as f32).max(0.0))
                .round() as i32;
            (crop(&big, vx, vy, term_w, term_h), (vx, vy), (zm.cell_w, zm.cell_h))
        } else {
            let pix = raster::raster_grid(
                &mut self.raster,
                snap,
                &GridStyle { theme: &theme, font_size: base_px, line_height: tpl.line_height },
            );
            (pix, (0, 0), (base_m.cell_w, base_m.cell_h))
        };

        for &(c, r, w, h) in &frame.highlights {
            let rect = (
                (c as f32 * cur_cell.0) as i32 - view_off.0,
                (r as f32 * cur_cell.1) as i32 - view_off.1,
                (w as f32 * cur_cell.0).ceil() as i32,
                (h as f32 * cur_cell.1).ceil() as i32,
            );
            chrome::dim_except(&mut term, rect, 0.55);
        }

        if let Some(crt) = &tpl.crt {
            fx::apply_crt(&mut term, crt, s);
        }

        let key = (term.width(), term.height());
        if self.chrome_base.as_ref().map(|(k, _)| *k) != Some(key) {
            let base = chrome::compose_base(&tpl, &theme, term.width(), term.height(), s, self.settings.aspect);
            self.chrome_base = Some((key, base));
        }
        let base = &self.chrome_base.as_ref().unwrap().1;
        let mut canvas = chrome::compose_over(base, &tpl, &term, s, self.settings.aspect);

        for cap in &frame.captions {
            self.draw_caption(&mut canvas, &cap.text, cap.pos, s);
        }
        canvas
    }

    fn draw_caption(&mut self, canvas: &mut Pixmap, text: &str, pos: CaptionPos, s: f32) {
        let size = (self.settings.template.font_size * 0.95 * s).max(10.0);
        let m = self.raster.fonts.cell_metrics(size, 1.0);
        let text_w: f32 = text.chars().count() as f32 * m.cell_w;
        let pad_x = 14.0 * s;
        let pad_y = 8.0 * s;
        let pill_w = text_w + pad_x * 2.0;
        let pill_h = m.cell_h + pad_y * 2.0;
        let cw = canvas.width() as f32;
        let ch = canvas.height() as f32;
        let x = (cw - pill_w) / 2.0;
        let margin = 18.0 * s;
        let y = match pos {
            CaptionPos::Bottom => ch - pill_h - margin,
            CaptionPos::Top => margin,
            CaptionPos::Center => (ch - pill_h) / 2.0,
        };

        raster::fill_rect(
            canvas,
            x as i32,
            y as i32,
            pill_w.ceil() as i32,
            pill_h.ceil() as i32,
            Rgba { r: 8, g: 8, b: 10, a: 208 },
        );
        let color = Rgba::rgb(0xf2, 0xf2, 0xf5);
        let mut pen_x = x + pad_x;
        let baseline_y = y + pad_y + m.baseline;
        for chr in text.chars() {
            if let Some(g) = self.raster.glyph(chr, Variant::Bold, size) {
                let gx = pen_x.round() as i32 + g.left;
                let gy = baseline_y.round() as i32 - g.top;
                match &g.pixels {
                    GlyphPixels::Mask(mask) => {
                        raster::blit_mask(canvas, gx, gy, g.width, g.height, mask, color)
                    }
                    GlyphPixels::Color(rgba) => {
                        raster::blit_rgba(canvas, gx, gy, g.width, g.height, rgba)
                    }
                }
            }
            pen_x += m.cell_w;
        }
    }
}

/// Premultiplied pixmap → straight RGBA bytes (what encoders expect).
pub fn pixmap_to_rgba(pix: &Pixmap) -> Vec<u8> {
    let mut out = Vec::with_capacity(pix.pixels().len() * 4);
    for p in pix.pixels() {
        let c = p.demultiply();
        out.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    out
}

fn crop(src: &Pixmap, x: i32, y: i32, w: u32, h: u32) -> Pixmap {
    let mut out = Pixmap::new(w.max(1), h.max(1)).expect("crop pixmap");
    let sw = src.width() as i32;
    let sh = src.height() as i32;
    let src_px = src.pixels();
    let out_w = out.width() as usize;
    let dst = out.pixels_mut();
    for row in 0..h as i32 {
        let sy = y + row;
        if sy < 0 || sy >= sh {
            continue;
        }
        for col in 0..w as i32 {
            let sx = x + col;
            if sx < 0 || sx >= sw {
                continue;
            }
            dst[row as usize * out_w + col as usize] = src_px[sy as usize * sw as usize + sx as usize];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use reel_cast::Cast;

    fn snap(body: &str) -> Snapshot {
        let text = format!("{}\n{}", r#"{"version": 2, "width": 40, "height": 10}"#, body);
        let cast = Cast::parse(&text).unwrap();
        reel_term::replay(&cast).unwrap().pop().unwrap()
    }

    fn settings(template: &str) -> RenderSettings {
        let cfg_text = format!(
            "---\n[source]\ncast = \"x.cast\"\n[template]\nname = \"{template}\"\n---\n"
        );
        let f = reel_format::ReelFile::parse(&cfg_text).unwrap();
        settings_from_config(&f.config).unwrap().0
    }

    fn base_frame() -> FramePlan {
        FramePlan {
            out_t: 0.0,
            dur: 1.0,
            snapshot: 0,
            camera: Camera::BASE,
            captions: vec![],
            highlights: vec![],
        }
    }

    #[test]
    fn glass_frame_composites() {
        let mut r = Renderer::new(settings("glass")).unwrap().0;
        let s = snap(r#"[0.1, "o", "$ reel render demo.reel"]"#);
        let pix = r.render_frame(&s, &base_frame());
        let (cw, chh) = r.canvas_size(40, 10);
        assert_eq!((pix.width(), pix.height()), (cw, chh));
        // Corners show the gradient canvas, not the terminal bg.
        let corner = pix.pixel(2, 2).unwrap();
        assert!(corner.red() > 0 || corner.blue() > 0);
    }

    #[test]
    fn zoom_magnifies_content() {
        let mut r = Renderer::new(settings("minimal")).unwrap().0;
        let s = snap(r#"[0.1, "o", "XYZXYZXYZ"]"#);
        let base = r.render_frame(&s, &base_frame());
        let mut zframe = base_frame();
        zframe.camera = Camera { zoom: 2.0, center: (2.0, 0.0) };
        let zoomed = r.render_frame(&s, &zframe);
        // Same canvas size, different pixels.
        assert_eq!(base.width(), zoomed.width());
        assert_ne!(base.data(), zoomed.data());
    }

    #[test]
    fn caption_draws_a_pill() {
        let mut r = Renderer::new(settings("minimal")).unwrap().0;
        let s = snap(r#"[0.1, "o", "hi"]"#);
        let mut f = base_frame();
        f.captions.push(plan::CaptionDraw { text: "Look here".into(), pos: CaptionPos::Bottom });
        let with = r.render_frame(&s, &f);
        let without = r.render_frame(&s, &base_frame());
        assert_ne!(with.data(), without.data());
    }
}
