//! reel-render: snapshots + frame plans in, composited RGBA frames out.

pub mod chrome;
pub mod font;
pub mod fx;
pub mod image;
pub mod overlay;
pub mod paths;
pub mod plan;
pub mod raster;
pub mod template;
pub mod theme;

pub use plan::{plan, plan_frames, plan_with, Camera, FramePlan, PlanOptions};
pub use template::{Template, WindowStyle};
pub use theme::{Rgba, Theme};
pub use tiny_skia::Pixmap;

use font::{GlyphPixels, Rasterizer, Variant};
use raster::GridStyle;
use reel_format::ReelConfig;
use reel_term::Snapshot;
use reel_timeline::{CaptionPos, HighlightStyle};

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
    #[error("invalid size `{0}` (try 1920x1080)")]
    BadSize(String),
    #[error("[style] {0} = {1} is out of range (allowed {2})")]
    BadStyle(&'static str, f64, &'static str),
}

#[derive(Debug, Clone)]
pub struct RenderSettings {
    pub template: Template,
    pub theme: Theme,
    /// Supersampling factor: output pixels per logical pixel.
    pub scale: f32,
    pub fps: u32,
    /// Aspect/exact-size canvas padding.
    pub fit: chrome::CanvasFit,
    /// Blink the cursor during long stills.
    pub cursor_blink: bool,
    /// Announce speed ramps with a chip.
    pub speed_badge: bool,
    /// Burn a progress bar into the frame.
    pub progress: bool,
    /// Marker positions as fractions of the output duration, notched onto
    /// that bar. Lives here rather than on the `Renderer` because parallel
    /// rendering rebuilds workers from the settings alone.
    pub progress_ticks: Vec<f64>,
}

impl RenderSettings {
    /// The frame-planning options this template implies.
    pub fn plan_options(&self) -> PlanOptions {
        PlanOptions {
            cursor_blink: self.cursor_blink,
            cursor_slide_ms: self.template.motion.cursor_slide_ms,
            typing_glow: self.template.motion.typing_glow,
            speed_badge: self.speed_badge,
            progress: self.progress,
        }
    }
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
    // Style numbers size the canvas; unchecked they reach Pixmap::new as an
    // unrepresentable dimension and abort instead of erroring.
    if let Some(fs) = style.font_size {
        if !(4.0..=200.0).contains(&fs) {
            return Err(RenderError::BadStyle("font_size", fs as f64, "4-200"));
        }
        tpl.font_size = fs;
    }
    if let Some(lh) = style.line_height {
        if !(0.5..=4.0).contains(&lh) {
            return Err(RenderError::BadStyle("line_height", lh as f64, "0.5-4"));
        }
        tpl.line_height = lh;
    }
    if let Some(p) = style.padding {
        if p > 1000 {
            return Err(RenderError::BadStyle("padding", p as f64, "0-1000"));
        }
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

    // `[style] theme` (an explicit override) beats the template's palette;
    // a palette embedded in the template beats its `theme` name reference.
    let theme = match (&style.theme, &tpl.theme_colors) {
        (None, Some(inline)) => inline.clone(),
        (named, _) => {
            let theme_name = named.as_deref().unwrap_or(tpl.theme.as_str());
            theme::lookup(theme_name).ok_or_else(|| {
                let mut names: Vec<String> =
                    theme::theme_names().iter().map(|s| s.to_string()).collect();
                names.extend(theme::user_theme_names());
                RenderError::UnknownTheme(theme_name.to_string(), names.join(", "))
            })?
        }
    };

    let aspect = match &cfg.output.aspect {
        Some(a) => Some(
            reel_format::parse_aspect(a).ok_or_else(|| RenderError::BadAspect(a.clone()))?,
        ),
        None => None,
    };
    let exact = match &cfg.output.size {
        Some(v) => Some(
            reel_format::parse_size(v).ok_or_else(|| RenderError::BadSize(v.clone()))?,
        ),
        None => None,
    };
    Ok((
        RenderSettings {
            template: tpl,
            theme,
            scale: cfg.output.scale.clamp(1, 4) as f32,
            fps: cfg.output.fps.unwrap_or(30).clamp(1, 120),
            fit: chrome::CanvasFit { aspect, exact },
            cursor_blink: cfg.style.cursor_blink.unwrap_or(true),
            speed_badge: cfg.style.speed_badge.unwrap_or(false),
            progress: cfg.style.progress.unwrap_or(false),
            progress_ticks: Vec::new(),
        },
        warnings,
    ))
}

/// Everything frame-specific that changes what `raster_grid_into` paints.
/// Snapshots are identified by index (stable within a render pass); floats
/// are keyed by bit pattern.
type GridKey = (usize, u32, bool, Option<(u32, u32)>);

pub struct Renderer {
    pub settings: RenderSettings,
    raster: Rasterizer,
    /// Cached static chrome (canvas bg + shadow + window) keyed by term size.
    chrome_base: Option<((u32, u32), Pixmap)>,
    /// Rasterized-grid cache. Stills dominate terminal demos, and the
    /// planner multiplies them into blink phases and progress ticks that
    /// differ only in cursor state — two entries (blink on/off) turn nearly
    /// all of that re-rasterization into memcpys.
    grid_cache: Vec<(GridKey, Pixmap)>,
    // Reused per-frame buffers: allocating ~25MB per frame made macOS's
    // large-alloc cache balloon multi-gigabyte on long renders.
    term_scratch: Pixmap,
    zoom_scratch: Pixmap,
    canvas_scratch: Pixmap,
    rgba_scratch: Vec<u8>,
}

impl Renderer {
    /// Builds a renderer against the installed system fonts. The warnings
    /// (e.g. the template's preferred font missing) are for the caller to
    /// surface.
    pub fn new(settings: RenderSettings) -> Result<(Self, Vec<String>), RenderError> {
        let (raster, warning) = Rasterizer::new(settings.template.font.as_deref())
            .map_err(RenderError::Font)?;
        let px = || Pixmap::new(1, 1).expect("pixmap");
        Ok((
            Renderer {
                settings,
                raster,
                chrome_base: None,
                grid_cache: Vec::new(),
                term_scratch: px(),
                zoom_scratch: px(),
                canvas_scratch: px(),
                rgba_scratch: Vec::new(),
            },
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
        // The key doesn't cover theme/template, so a settings swap must
        // drop cached grids (watch mode also swaps snapshot sets here).
        self.grid_cache.clear();
        Ok(warning.into_iter().collect())
    }

    /// When an exact canvas size is requested, solve the font size so the
    /// grid fills it as closely as possible without overflowing — no more
    /// hand-tuning font_size to land on 1920x1080.
    pub fn fit_exact(&mut self, cols: u16, rows: u16) {
        let Some((tw, th)) = self.settings.fit.exact else { return };
        let s = self.settings.scale;
        // Chrome overhead at zero terminal size.
        let l0 = chrome::layout(&self.settings.template, 0, 0, s, chrome::CanvasFit::default());
        let (ow, oh) = (l0.canvas_w as f32, l0.canvas_h as f32);
        // Cell metrics scale linearly with font size; measure at 100px.
        let m = self.raster.fonts.cell_metrics(100.0, self.settings.template.line_height);
        let (kw, kh) = (m.cell_w / 100.0, m.cell_h / 100.0);
        let fs_w = (tw as f32 - ow) / (cols as f32 * kw * s);
        let fs_h = (th as f32 - oh) / (rows as f32 * kh * s);
        // Tiny margin absorbs per-cell rounding so we never overflow.
        let fs = fs_w.min(fs_h) * 0.995;
        if fs.is_finite() && fs >= 4.0 {
            self.settings.template.font_size = fs;
            self.chrome_base = None;
        }
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
        let l = chrome::layout(&self.settings.template, tw, th, self.settings.scale, self.settings.fit);
        (l.canvas_w, l.canvas_h)
    }

    /// Renders one planned frame to a composited canvas.
    pub fn render_frame(&mut self, snap: &Snapshot, frame: &FramePlan) -> Pixmap {
        self.render_to_scratch(snap, frame);
        self.canvas_scratch.clone()
    }

    /// Renders one planned frame and returns straight RGBA bytes from an
    /// internal reused buffer — the zero-churn path encoders should use.
    pub fn render_frame_rgba(&mut self, snap: &Snapshot, frame: &FramePlan) -> (u32, u32, &[u8]) {
        self.render_to_scratch(snap, frame);
        pixmap_to_rgba_into(&self.canvas_scratch, &mut self.rgba_scratch);
        (self.canvas_scratch.width(), self.canvas_scratch.height(), &self.rgba_scratch)
    }

    /// Like [`render_frame_rgba`](Self::render_frame_rgba), converting
    /// straight into `out` — a worker fills its recycled channel buffer
    /// without an extra full-canvas copy in between.
    pub fn render_frame_rgba_into(
        &mut self,
        snap: &Snapshot,
        frame: &FramePlan,
        out: &mut Vec<u8>,
    ) -> (u32, u32) {
        self.render_to_scratch(snap, frame);
        pixmap_to_rgba_into(&self.canvas_scratch, out);
        (self.canvas_scratch.width(), self.canvas_scratch.height())
    }

    fn render_to_scratch(&mut self, snap: &Snapshot, frame: &FramePlan) {
        let s = self.settings.scale;
        let tpl = self.settings.template.clone();
        let theme = self.settings.theme.clone();
        let base_px = self.base_font_px();
        let base_m = self.raster.fonts.cell_metrics(base_px, tpl.line_height);
        let term_w = (snap.cols as f32 * base_m.cell_w).ceil() as u32;
        let term_h = (snap.rows as f32 * base_m.cell_h).ceil() as u32;

        let grid_style = |font_size: f32| {
            let mut gs = GridStyle::new(&theme, font_size, tpl.line_height, frame.cursor_on);
            gs.cursor_pos = frame.cursor_pos;
            gs.cursor_style = tpl.cursor_style;
            gs.cursor_color = tpl.cursor_color;
            // Glass windows: the translucent window body already carries the
            // background tint, so the grid leaves its default-bg cells fully
            // transparent — otherwise the content area double-stacks alpha
            // and reads more opaque than the padding. CRT keeps an opaque
            // grid (its passes assume solid pixels). Bare terminals carry
            // the opacity themselves.
            gs.bg_alpha = match tpl.window {
                WindowStyle::None => tpl.window_opacity,
                _ if tpl.window_opacity < 1.0 && tpl.crt.is_none() => 0.0,
                _ => 1.0,
            };
            gs
        };

        let z = frame.camera.zoom.max(1.0);
        // (viewport origin in zoomed px, current cell size) for overlay math.
        let (view_off, cur_cell) = if z > 1.0001 {
            // Re-rasterize at the zoomed size so text stays sharp — never
            // upscale pixels.
            let zoom_px = base_px * z as f32;
            let zm = self.raster.fonts.cell_metrics(zoom_px, tpl.line_height);
            raster::raster_grid_into(
                &mut self.raster,
                snap,
                &grid_style(zoom_px),
                &mut self.zoom_scratch,
            );
            let big = &self.zoom_scratch;
            let cx = (frame.camera.center.0 as f32 + 0.5) * zm.cell_w;
            let cy = (frame.camera.center.1 as f32 + 0.5) * zm.cell_h;
            let vx = (cx - term_w as f32 / 2.0)
                .clamp(0.0, (big.width() as f32 - term_w as f32).max(0.0))
                .round() as i32;
            let vy = (cy - term_h as f32 / 2.0)
                .clamp(0.0, (big.height() as f32 - term_h as f32).max(0.0))
                .round() as i32;
            crop_into(big, vx, vy, term_w, term_h, &mut self.term_scratch);
            ((vx, vy), (zm.cell_w, zm.cell_h))
        } else {
            let key: GridKey = (
                frame.snapshot,
                base_px.to_bits(),
                frame.cursor_on,
                frame.cursor_pos.map(|(x, y)| (x.to_bits(), y.to_bits())),
            );
            Self::raster_grid_cached(
                &mut self.raster,
                &mut self.grid_cache,
                snap,
                &grid_style(base_px),
                key,
                &mut self.term_scratch,
            );
            ((0, 0), (base_m.cell_w, base_m.cell_h))
        };
        let term = &mut self.term_scratch;

        if let Some(&(_, _, intensity)) = frame.glow.first() {
            let cells: Vec<(f32, f32, f32, f32, Rgba)> = frame
                .glow
                .iter()
                .filter(|&&(c, r, _)| c < snap.cols && r < snap.rows)
                .filter_map(|&(c, r, _)| {
                    let cell = snap.cell(c, r);
                    if cell.ch == ' ' || cell.ch == '\0' {
                        return None;
                    }
                    let color = theme.resolve(cell.fg, &snap.palette_overrides);
                    Some((
                        c as f32 * cur_cell.0 - view_off.0 as f32,
                        r as f32 * cur_cell.1 - view_off.1 as f32,
                        cur_cell.0,
                        cur_cell.1,
                        color,
                    ))
                })
                .collect();
            fx::glow_cells(term, &cells, intensity);
        }

        // Highlights draw on the terminal image, before the chrome, so the
        // camera carries them.
        let ov = overlay::OverlayStyle::resolve(&tpl, &theme);
        for hl in &frame.highlights {
            let (c, r, w, h) = hl.rect;
            let x = (c as f32 * cur_cell.0) as i32 - view_off.0;
            let y = (r as f32 * cur_cell.1) as i32 - view_off.1;
            let w = (w as f32 * cur_cell.0).ceil() as i32;
            let h = (h as f32 * cur_cell.1).ceil() as i32;
            let a = hl.anim.alpha.clamp(0.0, 1.0);
            let t = hl.anim.t.clamp(0.0, 1.0);
            match hl.style {
                HighlightStyle::Spotlight => {
                    // The pool of light widens as it brightens.
                    chrome::dim_except(term, (x, y, w, h), 0.55 * a, (4.0 + 8.0 * t) * s)
                }
                HighlightStyle::Box => {
                    // Breathing room, so the box frames the text instead of
                    // clipping its ascenders — clamped to the grid, so a
                    // full-width highlight keeps a closed outline instead of
                    // losing its side strokes off-canvas.
                    let pad = 4.0 * s;
                    let (tw, th) = (term.width() as f32, term.height() as f32);
                    let bx = (x as f32 - pad).max(1.0);
                    let by = (y as f32 - pad).max(1.0);
                    let bw = (x as f32 + w as f32 + pad).min(tw - 1.0) - bx;
                    let bh = (y as f32 + h as f32 + pad).min(th - 1.0) - by;
                    if let Some(rect) = tiny_skia::Rect::from_xywh(bx, by, bw, bh) {
                        let radius = 6.0 * s;
                        // A wash inside so it reads as *highlighted*, not
                        // merely outlined — the fill is what a marker pen
                        // does, the stroke is what a form field does.
                        let wash = Rgba { a: (30.0 * a) as u8, ..ov.accent };
                        chrome::fill_rounded(term, rect, radius, wash);
                        // …and the outline draws itself around the rect.
                        let color = Rgba { a: (255.0 * a) as u8, ..ov.accent };
                        chrome::stroke_rounded_partial(term, rect, radius, color, 2.0 * s, t);
                    }
                }
                HighlightStyle::Underline => {
                    // Wipes out from the left.
                    let color = Rgba { a: (255.0 * a) as u8, ..ov.accent };
                    let th = (2.0 * s).max(1.0);
                    raster::fill_rect(term, x, y + h, (w as f32 * t) as i32, th.ceil() as i32, color);
                }
            }
        }

        if let Some(crt) = &tpl.crt {
            fx::apply_crt(term, crt, s);
        }

        let key = (term.width(), term.height());
        let l = chrome::layout(&tpl, key.0, key.1, s, self.settings.fit);
        if self.chrome_base.as_ref().map(|(k, _)| *k) != Some(key) {
            let mut base = chrome::compose_base(&tpl, &theme, key.0, key.1, s, self.settings.fit);
            Self::decorate_base(&mut self.raster, &mut base, &tpl, &theme, &l, s);
            self.chrome_base = Some((key, base));
        }
        let base = &self.chrome_base.as_ref().unwrap().1;
        chrome::compose_over_into(
            base,
            &tpl,
            &self.term_scratch,
            s,
            self.settings.fit,
            &mut self.canvas_scratch,
        );

        for note in &frame.notes {
            let anchor =
                overlay::cell_to_canvas(&l, view_off, cur_cell, note.anchor.0, note.anchor.1);
            overlay::draw_note(
                &mut self.raster,
                &mut self.canvas_scratch,
                note,
                anchor,
                &ov,
                tpl.font_size,
                s,
            );
        }

        for cap in &frame.captions {
            Self::draw_caption(
                &mut self.raster,
                self.settings.template.font_size,
                &mut self.canvas_scratch,
                &cap.text,
                cap.pos,
                s,
            );
        }

        if !frame.keys.is_empty() {
            let over_caption =
                frame.captions.iter().any(|c| c.pos == CaptionPos::Bottom);
            Self::draw_keys(
                &mut self.raster,
                self.settings.template.font_size,
                &mut self.canvas_scratch,
                &frame.keys,
                over_caption,
                s,
            );
        }

        if let Some(badge) = frame.rate_badge {
            let taken = tpl.badge.as_ref().is_some_and(|b| b.corner == template::Corner::TopRight);
            overlay::draw_rate_badge(
                &mut self.raster,
                &mut self.canvas_scratch,
                badge.rate,
                &l,
                taken,
                &ov,
                tpl.font_size,
                badge.anim.alpha,
                s,
            );
        }

        // A card covers the frame, so it goes over every other overlay; the
        // progress bar goes over even that — it's the video's own furniture.
        if let Some(card) = &frame.card {
            overlay::draw_card(
                &mut self.raster,
                &mut self.canvas_scratch,
                card,
                canvas_scrim(&tpl, &theme),
                &ov,
                tpl.font_size,
                s,
            );
        }

        if let Some(p) = frame.progress {
            overlay::draw_progress(
                &mut self.canvas_scratch,
                p,
                &self.settings.progress_ticks,
                &ov,
                s,
            );
        }
    }

    /// `raster_grid_into` behind the two-entry grid cache. On a hit the
    /// raster is a memcpy; effects (glow, highlights, CRT) draw on the
    /// scratch afterwards, so the cached copy stays pristine.
    fn raster_grid_cached(
        raster: &mut Rasterizer,
        cache: &mut Vec<(GridKey, Pixmap)>,
        snap: &Snapshot,
        style: &GridStyle,
        key: GridKey,
        out: &mut Pixmap,
    ) {
        if let Some(pos) = cache.iter().position(|(k, _)| *k == key) {
            let pix = &cache[pos].1;
            if out.width() == pix.width() && out.height() == pix.height() {
                out.data_mut().copy_from_slice(pix.data());
            } else {
                *out = pix.clone();
            }
            return;
        }
        raster::raster_grid_into(raster, snap, style, out);
        // Bound the cache's memory: two entries, none absurdly large.
        const MAX_CACHED_BYTES: usize = 64 << 20;
        if out.data().len() <= MAX_CACHED_BYTES {
            if cache.len() >= 2 {
                cache.remove(0);
            }
            cache.push((key, out.clone()));
        }
    }

    /// Static decorations drawn once onto the cached chrome: titlebar text
    /// and the corner badge.
    fn decorate_base(
        raster: &mut Rasterizer,
        canvas: &mut Pixmap,
        tpl: &Template,
        theme: &Theme,
        l: &chrome::Layout,
        s: f32,
    ) {
        if let Some(title) = &tpl.title {
            if l.titlebar_h > 0.0 {
                let size = (12.5 * s).max(8.0);
                let m = raster.fonts.cell_metrics(size, 1.0);
                let text_w = title.chars().count() as f32 * m.cell_w;
                let x = l.win_x + (l.win_w - text_w) / 2.0;
                let baseline = l.win_y + (l.titlebar_h - m.cell_h) / 2.0 + m.baseline;
                let color = Rgba { a: 150, ..theme.fg };
                draw_text(raster, canvas, title, x, baseline, size, color, Variant::Regular);
            }
        }
        if let Some(badge) = &tpl.badge {
            let margin = 16.0 * s;
            let opacity = badge.opacity.clamp(0.0, 1.0);
            let text_size = (11.5 * s).max(8.0);
            let m = raster.fonts.cell_metrics(text_size, 1.0);
            let img_h = 20.0 * s;
            let (img_w, gap) = match &badge.image {
                Some(img) => {
                    let scale = img_h / img.pix.height().max(1) as f32;
                    (img.pix.width() as f32 * scale, if badge.text.is_some() { 7.0 * s } else { 0.0 })
                }
                None => (0.0, 0.0),
            };
            let text_w = badge
                .text
                .as_ref()
                .map(|t| t.chars().count() as f32 * m.cell_w)
                .unwrap_or(0.0);
            let total_w = img_w + gap + text_w;
            let total_h = if badge.image.is_some() { img_h } else { m.cell_h };
            let cw = canvas.width() as f32;
            let ch = canvas.height() as f32;
            let x = match badge.corner {
                template::Corner::TopLeft | template::Corner::BottomLeft => margin,
                _ => cw - total_w - margin,
            };
            let y = match badge.corner {
                template::Corner::TopLeft | template::Corner::TopRight => margin,
                _ => ch - total_h - margin,
            };
            if let Some(img) = &badge.image {
                let scale = img_h / img.pix.height().max(1) as f32;
                canvas.draw_pixmap(
                    0,
                    0,
                    (*img.pix).as_ref(),
                    &tiny_skia::PixmapPaint {
                        opacity,
                        quality: tiny_skia::FilterQuality::Bilinear,
                        ..Default::default()
                    },
                    tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, x, y),
                    None,
                );
            }
            if let Some(text) = &badge.text {
                let color = Rgba { a: (opacity * 255.0) as u8, ..theme.fg };
                let baseline = y + (total_h - m.cell_h) / 2.0 + m.baseline;
                draw_text(
                    raster,
                    canvas,
                    text,
                    x + img_w + gap,
                    baseline,
                    text_size,
                    color,
                    Variant::Bold,
                );
            }
        }
    }

    fn draw_caption(
        raster: &mut Rasterizer,
        template_font_size: f32,
        canvas: &mut Pixmap,
        text: &str,
        pos: CaptionPos,
        s: f32,
    ) {
        let size = (template_font_size * 0.95 * s).max(10.0);
        let m = raster.fonts.cell_metrics(size, 1.0);
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
            if let Some(g) = raster.glyph(chr, Variant::Bold, size) {
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

    /// Keystroke chips, bottom-centered — one small pill per key, oldest on
    /// the left. When a bottom caption is showing, the row lifts above it.
    fn draw_keys(
        raster: &mut Rasterizer,
        template_font_size: f32,
        canvas: &mut Pixmap,
        keys: &[String],
        over_caption: bool,
        s: f32,
    ) {
        let size = (template_font_size * 0.85 * s).max(9.0);
        let m = raster.fonts.cell_metrics(size, 1.0);
        let pad_x = 8.0 * s;
        let pad_y = 5.0 * s;
        let gap = 6.0 * s;
        let chip_h = m.cell_h + pad_y * 2.0;
        let widths: Vec<f32> = keys
            .iter()
            .map(|k| k.chars().count() as f32 * m.cell_w + pad_x * 2.0)
            .collect();
        let total_w: f32 = widths.iter().sum::<f32>() + gap * (keys.len() - 1) as f32;
        let cw = canvas.width() as f32;
        let ch = canvas.height() as f32;
        let margin = 18.0 * s;
        // A bottom caption's pill height (same math as draw_caption).
        let lift = if over_caption {
            let cm = raster.fonts.cell_metrics((template_font_size * 0.95 * s).max(10.0), 1.0);
            cm.cell_h + 16.0 * s + 10.0 * s
        } else {
            0.0
        };
        let y = ch - chip_h - margin - lift;
        let mut x = (cw - total_w) / 2.0;
        let text_color = Rgba::rgb(0xf2, 0xf2, 0xf5);
        for (key, w) in keys.iter().zip(&widths) {
            raster::fill_rect(
                canvas,
                x as i32,
                y as i32,
                w.ceil() as i32,
                chip_h.ceil() as i32,
                Rgba { r: 8, g: 8, b: 10, a: 190 },
            );
            let mut pen_x = x + pad_x;
            let baseline_y = y + pad_y + m.baseline;
            for chr in key.chars() {
                if let Some(g) = raster.glyph(chr, Variant::Bold, size) {
                    let gx = pen_x.round() as i32 + g.left;
                    let gy = baseline_y.round() as i32 - g.top;
                    match &g.pixels {
                        GlyphPixels::Mask(mask) => {
                            raster::blit_mask(canvas, gx, gy, g.width, g.height, mask, text_color)
                        }
                        GlyphPixels::Color(rgba) => {
                            raster::blit_rgba(canvas, gx, gy, g.width, g.height, rgba)
                        }
                    }
                }
                pen_x += m.cell_w;
            }
            x += w + gap;
        }
    }
}

/// The canvas's base color — what a title card scrims the frame with, so a
/// card reads as part of the template rather than as a black flash.
fn canvas_scrim(tpl: &Template, theme: &Theme) -> Rgba {
    match &tpl.canvas {
        template::CanvasBg::Solid(c) => *c,
        template::CanvasBg::Linear { stops, .. } | template::CanvasBg::Radial { stops } => {
            stops.first().map(|(_, c)| *c).unwrap_or(theme.bg)
        }
        template::CanvasBg::Image { .. } => theme.bg,
    }
}

/// Draws a single line of text at a baseline; returns the advance width.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_text(
    raster: &mut Rasterizer,
    canvas: &mut Pixmap,
    text: &str,
    x: f32,
    baseline: f32,
    size: f32,
    color: Rgba,
    variant: Variant,
) -> f32 {
    let m = raster.fonts.cell_metrics(size, 1.0);
    let mut pen = x;
    for ch in text.chars() {
        if let Some(g) = raster.glyph(ch, variant, size) {
            let gx = pen.round() as i32 + g.left;
            let gy = baseline.round() as i32 - g.top;
            match &g.pixels {
                GlyphPixels::Mask(mask) => {
                    raster::blit_mask(canvas, gx, gy, g.width, g.height, mask, color)
                }
                GlyphPixels::Color(rgba) => raster::blit_rgba(canvas, gx, gy, g.width, g.height, rgba),
            }
        }
        pen += m.cell_w;
    }
    pen - x
}

/// Premultiplied pixmap → straight RGBA bytes (what encoders expect).
pub fn pixmap_to_rgba(pix: &Pixmap) -> Vec<u8> {
    let mut out = Vec::new();
    pixmap_to_rgba_into(pix, &mut out);
    out
}

/// Buffer-reusing variant of [`pixmap_to_rgba`].
pub fn pixmap_to_rgba_into(pix: &Pixmap, out: &mut Vec<u8>) {
    let px = pix.pixels();
    out.clear();
    out.resize(px.len() * 4, 0);
    for (p, o) in px.iter().zip(out.chunks_exact_mut(4)) {
        // Opaque pixels — virtually the whole canvas — need no demultiply.
        if p.alpha() == 255 {
            o.copy_from_slice(&[p.red(), p.green(), p.blue(), 255]);
        } else {
            let c = p.demultiply();
            o.copy_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
        }
    }
}

fn crop_into(src: &Pixmap, x: i32, y: i32, w: u32, h: u32, out: &mut Pixmap) {
    let (w, h) = (w.max(1), h.max(1));
    if out.width() != w || out.height() != h {
        *out = Pixmap::new(w, h).expect("crop pixmap");
    } else {
        out.data_mut().fill(0);
    }
    let sw = src.width() as i32;
    let sh = src.height() as i32;
    let col0 = (-x).max(0);
    let col1 = (sw - x).min(w as i32);
    if col0 >= col1 {
        return;
    }
    let n = (col1 - col0) as usize;
    let src_px = src.pixels();
    let out_w = out.width() as usize;
    let dst = out.pixels_mut();
    for row in 0..h as i32 {
        let sy = y + row;
        if sy < 0 || sy >= sh {
            continue;
        }
        let s0 = sy as usize * sw as usize + (x + col0) as usize;
        let d0 = row as usize * out_w + col0 as usize;
        dst[d0..d0 + n].copy_from_slice(&src_px[s0..s0 + n]);
    }
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
            notes: vec![],
            card: None,
            rate_badge: None,
            progress: None,
            keys: vec![],
            cursor_on: true,
            cursor_pos: None,
            glow: vec![],
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
    fn translucent_window_differs_from_opaque() {
        let s = snap(r#"[0.1, "o", "$ glass"]"#);
        let glassy = settings("aurora");
        let mut opaque = settings("aurora");
        opaque.template.window_opacity = 1.0;
        opaque.template.window_blur = 0.0;
        let a = Renderer::new(glassy).unwrap().0.render_frame(&s, &base_frame());
        let b = Renderer::new(opaque).unwrap().0.render_frame(&s, &base_frame());
        assert_ne!(a.data(), b.data());
    }

    #[test]
    fn title_and_badge_draw_on_the_chrome() {
        let s = snap(r#"[0.1, "o", "hi"]"#);
        let plain = settings("glass");
        let mut decorated = settings("glass");
        decorated.template.title = Some("~/app — zsh".into());
        decorated.template.badge = Some(template::Badge {
            text: Some("reel".into()),
            image: None,
            corner: template::Corner::BottomRight,
            opacity: 0.6,
        });
        let a = Renderer::new(plain).unwrap().0.render_frame(&s, &base_frame());
        let b = Renderer::new(decorated).unwrap().0.render_frame(&s, &base_frame());
        assert_ne!(a.data(), b.data());
    }

    #[test]
    fn glow_and_slide_frames_render() {
        let mut r = Renderer::new(settings("aurora")).unwrap().0;
        let s = snap(r#"[0.1, "o", "typing"]"#);
        let mut f = base_frame();
        f.cursor_pos = Some((2.5, 0.0));
        f.glow = vec![(0, 0, 0.6), (1, 0, 0.6)];
        let animated = r.render_frame(&s, &f);
        let still = r.render_frame(&s, &base_frame());
        assert_eq!(animated.width(), still.width());
        assert_ne!(animated.data(), still.data());
    }

    #[test]
    fn key_chips_draw() {
        let mut r = Renderer::new(settings("minimal")).unwrap().0;
        let s = snap(r#"[0.1, "o", "hi"]"#);
        let mut f = base_frame();
        f.keys = vec!["cargo test".into(), "⏎".into()];
        let with = r.render_frame(&s, &f);
        let without = r.render_frame(&s, &base_frame());
        assert_ne!(with.data(), without.data());
    }

    #[test]
    fn note_draws_and_follows_its_anchor() {
        let mut r = Renderer::new(settings("minimal")).unwrap().0;
        let s = snap(r#"[0.1, "o", "cache hit"]"#);
        let note = |col: u16| plan::NoteDraw {
            text: "look here".into(),
            anchor: (col, 0),
            style: reel_timeline::NoteStyle::Card,
            side: reel_timeline::NoteSide::Down,
            anim: plan::Anim::SETTLED,
        };
        let mut f = base_frame();
        f.notes = vec![note(2)];
        let left = r.render_frame(&s, &f).data().to_vec();
        let plain = r.render_frame(&s, &base_frame()).data().to_vec();
        assert_ne!(left, plain, "note drew nothing");
        // The anchor moves with the cell, so the pixels differ.
        f.notes = vec![note(30)];
        assert_ne!(left, r.render_frame(&s, &f).data());
        // A fully faded-out note draws nothing at all.
        f.notes = vec![plan::NoteDraw {
            anim: plan::Anim { t: 0.0, alpha: 0.0 },
            ..note(2)
        }];
        assert_eq!(plain, r.render_frame(&s, &f).data());
    }

    #[test]
    fn highlight_styles_differ_from_each_other() {
        use reel_timeline::HighlightStyle::*;
        let s = snap(r#"[0.1, "o", "highlight me"]"#);
        let mut r = Renderer::new(settings("minimal")).unwrap().0;
        let shot = |r: &mut Renderer, style| {
            let mut f = base_frame();
            f.highlights =
                vec![plan::HighlightDraw { rect: (2, 0, 5, 1), style, anim: plan::Anim::SETTLED }];
            r.render_frame(&s, &f).data().to_vec()
        };
        let spot = shot(&mut r, Spotlight);
        let boxed = shot(&mut r, Box);
        let under = shot(&mut r, Underline);
        let plain = r.render_frame(&s, &base_frame()).data().to_vec();
        for (name, pixels) in [("spotlight", &spot), ("box", &boxed), ("underline", &under)] {
            assert_ne!(&plain, pixels, "{name} drew nothing");
        }
        assert_ne!(spot, boxed);
        assert_ne!(boxed, under);
    }

    #[test]
    fn a_card_scrims_the_whole_frame() {
        let mut r = Renderer::new(settings("glass")).unwrap().0;
        let s = snap(r#"[0.1, "o", "$ cargo build"]"#);
        let mut f = base_frame();
        f.card = Some(plan::CardDraw { text: "1 · Install".into(), anim: plan::Anim::SETTLED });
        let carded = r.render_frame(&s, &f).data().to_vec();
        let plain = r.render_frame(&s, &base_frame()).data().to_vec();
        // A card covers the canvas rather than a region: most of the frame
        // moves, not a band of it. (Pixels already the scrim's color stay
        // put, so this is a majority test, not an every-pixel one.)
        let moved = carded
            .chunks_exact(4)
            .zip(plain.chunks_exact(4))
            .filter(|(a, b)| a != b)
            .count();
        let total = carded.len() / 4;
        assert!(moved * 10 > total * 6, "card only moved {moved}/{total} pixels");
    }

    #[test]
    fn progress_bar_fills_from_the_left_in_the_accent_color() {
        let set = settings("minimal");
        let accent = set.theme.cursor;
        let mut r = Renderer::new(set).unwrap().0;
        let s = snap(r#"[0.1, "o", "hi"]"#);
        let mut f = base_frame();
        f.progress = Some(0.5);
        let pix = r.render_frame(&s, &f);
        let y = pix.height() - 2;
        let filled = pix.pixel(pix.width() / 4, y).unwrap().demultiply();
        assert_eq!((filled.red(), filled.green(), filled.blue()), (accent.r, accent.g, accent.b));
        // Past the halfway mark it's the unfilled track, not the accent.
        let empty = pix.pixel(pix.width() * 3 / 4, y).unwrap().demultiply();
        assert_ne!((empty.red(), empty.green(), empty.blue()), (accent.r, accent.g, accent.b));
    }

    #[test]
    fn the_speed_badge_draws_a_chip() {
        let mut r = Renderer::new(settings("minimal")).unwrap().0;
        let s = snap(r#"[0.1, "o", "hi"]"#);
        let mut f = base_frame();
        f.rate_badge = Some(plan::RateBadge { rate: 5.0, anim: plan::Anim::SETTLED });
        assert_ne!(
            r.render_frame(&s, &f).data(),
            r.render_frame(&s, &base_frame()).data()
        );
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
