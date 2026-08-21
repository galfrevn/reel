//! Grid → pixels: draws one snapshot into a tiny-skia Pixmap.

use crate::font::{GlyphPixels, Rasterizer, Variant};
use crate::theme::{Rgba, Theme};
use reel_term::{CellAttrs, CursorShape, Snapshot};
use tiny_skia::{Pixmap, PremultipliedColorU8};

pub struct GridStyle<'a> {
    pub theme: &'a Theme,
    pub font_size: f32,
    pub line_height: f32,
    /// Draw the cursor (false during a blink's off phase).
    pub cursor_visible: bool,
    /// Fractional cell position override — mid-slide cursor animation.
    pub cursor_pos: Option<(f32, f32)>,
    /// Template-forced cursor shape (recordings keep theirs otherwise).
    pub cursor_style: Option<CursorShape>,
    /// Cursor color override (default: the theme's).
    pub cursor_color: Option<Rgba>,
    /// Terminal background alpha (1 = opaque); below 1 the window glass
    /// shows through cells that use the default background.
    pub bg_alpha: f32,
}

impl<'a> GridStyle<'a> {
    pub fn new(theme: &'a Theme, font_size: f32, line_height: f32, cursor_visible: bool) -> Self {
        GridStyle {
            theme,
            font_size,
            line_height,
            cursor_visible,
            cursor_pos: None,
            cursor_style: None,
            cursor_color: None,
            bg_alpha: 1.0,
        }
    }
}

/// A rectangle inside a cell, in cell fractions: (x0, y0, x1, y1).
type BlockPart = (f32, f32, f32, f32);

/// The sub-rectangles making up a Block Elements character (U+2580..U+259F),
/// plus the alpha the shade characters ask for.
///
/// These are drawn as geometry rather than blitted from the font. A glyph
/// bitmap has a fixed pixel width while the cell advance is fractional, so
/// wherever the advance rounds up the blit leaves a 1px seam — visible as
/// gaps between columns, and as a chopped-up logo in any TUI that draws with
/// blocks. Deriving them from the cell rect instead makes neighbours share
/// edges exactly, which is what real terminals do.
fn block_parts(ch: char) -> Option<(&'static [BlockPart], f32)> {
    const T: f32 = 1.0 / 8.0;
    let parts: (&'static [BlockPart], f32) = match ch {
        // Full block and the shades: the whole cell at varying alpha.
        '\u{2588}' => (&[(0.0, 0.0, 1.0, 1.0)], 1.0),
        '\u{2591}' => (&[(0.0, 0.0, 1.0, 1.0)], 0.25),
        '\u{2592}' => (&[(0.0, 0.0, 1.0, 1.0)], 0.5),
        '\u{2593}' => (&[(0.0, 0.0, 1.0, 1.0)], 0.75),
        // Halves.
        '\u{2580}' => (&[(0.0, 0.0, 1.0, 0.5)], 1.0),
        '\u{2584}' => (&[(0.0, 0.5, 1.0, 1.0)], 1.0),
        '\u{258C}' => (&[(0.0, 0.0, 0.5, 1.0)], 1.0),
        '\u{2590}' => (&[(0.5, 0.0, 1.0, 1.0)], 1.0),
        // Lower eighths, growing upward.
        '\u{2581}' => (&[(0.0, 1.0 - T, 1.0, 1.0)], 1.0),
        '\u{2582}' => (&[(0.0, 1.0 - 2.0 * T, 1.0, 1.0)], 1.0),
        '\u{2583}' => (&[(0.0, 1.0 - 3.0 * T, 1.0, 1.0)], 1.0),
        '\u{2585}' => (&[(0.0, 1.0 - 5.0 * T, 1.0, 1.0)], 1.0),
        '\u{2586}' => (&[(0.0, 1.0 - 6.0 * T, 1.0, 1.0)], 1.0),
        '\u{2587}' => (&[(0.0, 1.0 - 7.0 * T, 1.0, 1.0)], 1.0),
        // Left eighths, growing rightward.
        '\u{258F}' => (&[(0.0, 0.0, T, 1.0)], 1.0),
        '\u{258E}' => (&[(0.0, 0.0, 2.0 * T, 1.0)], 1.0),
        '\u{258D}' => (&[(0.0, 0.0, 3.0 * T, 1.0)], 1.0),
        '\u{258B}' => (&[(0.0, 0.0, 5.0 * T, 1.0)], 1.0),
        '\u{258A}' => (&[(0.0, 0.0, 6.0 * T, 1.0)], 1.0),
        '\u{2589}' => (&[(0.0, 0.0, 7.0 * T, 1.0)], 1.0),
        // Single upper/right eighths.
        '\u{2594}' => (&[(0.0, 0.0, 1.0, T)], 1.0),
        '\u{2595}' => (&[(1.0 - T, 0.0, 1.0, 1.0)], 1.0),
        // Quadrants. UL = (0,0,.5,.5), UR = (.5,0,1,.5),
        //            LL = (0,.5,.5,1),  LR = (.5,.5,1,1).
        '\u{2596}' => (&[(0.0, 0.5, 0.5, 1.0)], 1.0),
        '\u{2597}' => (&[(0.5, 0.5, 1.0, 1.0)], 1.0),
        '\u{2598}' => (&[(0.0, 0.0, 0.5, 0.5)], 1.0),
        '\u{259D}' => (&[(0.5, 0.0, 1.0, 0.5)], 1.0),
        '\u{2599}' => (&[(0.0, 0.0, 0.5, 0.5), (0.0, 0.5, 1.0, 1.0)], 1.0),
        '\u{259A}' => (&[(0.0, 0.0, 0.5, 0.5), (0.5, 0.5, 1.0, 1.0)], 1.0),
        '\u{259B}' => (&[(0.0, 0.0, 1.0, 0.5), (0.0, 0.5, 0.5, 1.0)], 1.0),
        '\u{259C}' => (&[(0.0, 0.0, 1.0, 0.5), (0.5, 0.5, 1.0, 1.0)], 1.0),
        '\u{259E}' => (&[(0.5, 0.0, 1.0, 0.5), (0.0, 0.5, 0.5, 1.0)], 1.0),
        '\u{259F}' => (&[(0.5, 0.0, 1.0, 0.5), (0.0, 0.5, 1.0, 1.0)], 1.0),
        _ => return None,
    };
    Some(parts)
}

/// Fills a Block Elements character into the exact cell rect. Returns false
/// for anything that isn't one, so the caller falls back to the font.
#[allow(clippy::too_many_arguments)]
fn draw_block(
    pix: &mut Pixmap,
    ch: char,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    fg: Rgba,
) -> bool {
    let Some((parts, alpha)) = block_parts(ch) else { return false };
    let (w, h) = ((x1 - x0) as f32, (y1 - y0) as f32);
    let color = Rgba { a: (fg.a as f32 * alpha) as u8, ..fg };
    for &(fx0, fy0, fx1, fy1) in parts {
        // Snap to the cell's own integer edges so a full block covers it
        // exactly and abutting cells leave nothing between them.
        let px0 = x0 + (w * fx0).round() as i32;
        let py0 = y0 + (h * fy0).round() as i32;
        let px1 = x0 + (w * fx1).round() as i32;
        let py1 = y0 + (h * fy1).round() as i32;
        fill_rect(pix, px0, py0, px1 - px0, py1 - py0, color);
    }
    true
}

/// Renders the full grid at the given font size. The same routine serves the
/// base view and the zoom view (at a larger size), so zoomed text is
/// re-rasterized, never upscaled.
pub fn raster_grid(r: &mut Rasterizer, snap: &Snapshot, style: &GridStyle) -> Pixmap {
    let mut pix = Pixmap::new(1, 1).expect("pixmap");
    raster_grid_into(r, snap, style, &mut pix);
    pix
}

/// Like [`raster_grid`], but reuses `pix` when the size already matches —
/// per-frame buffer churn is what used to balloon renders to gigabytes.
pub fn raster_grid_into(r: &mut Rasterizer, snap: &Snapshot, style: &GridStyle, pix: &mut Pixmap) {
    let m = r.fonts.cell_metrics(style.font_size, style.line_height);
    let w = ((snap.cols as f32 * m.cell_w).ceil() as u32).max(1);
    let h = ((snap.rows as f32 * m.cell_h).ceil() as u32).max(1);
    if pix.width() != w || pix.height() != h {
        *pix = Pixmap::new(w, h).expect("grid pixmap");
    }

    let base_bg = if style.bg_alpha >= 1.0 {
        style.theme.bg
    } else {
        let a = (style.theme.bg.a as f32 * style.bg_alpha.clamp(0.0, 1.0)) as u8;
        Rgba { a, ..style.theme.bg }
    };
    fill(pix, base_bg);

    let ov = &snap.palette_overrides;
    // Backgrounds first (a glyph may overhang its cell).
    for row in 0..snap.rows {
        for col in 0..snap.cols {
            let cell = snap.cell(col, row);
            let (_, bg) = cell_colors(cell, style.theme, ov);
            if bg != style.theme.bg {
                let x0 = (col as f32 * m.cell_w).round() as i32;
                let x1 = ((col + 1) as f32 * m.cell_w).round() as i32;
                let y0 = (row as f32 * m.cell_h).round() as i32;
                let y1 = ((row + 1) as f32 * m.cell_h).round() as i32;
                fill_rect(pix, x0, y0, x1 - x0, y1 - y0, bg);
            }
        }
    }

    // Cursor under the glyph so the char stays readable on top.
    let cursor_cell_fg = if style.cursor_visible {
        draw_cursor(pix, snap, style, m.cell_w, m.cell_h)
    } else {
        None
    };

    for row in 0..snap.rows {
        for col in 0..snap.cols {
            let cell = snap.cell(col, row);
            if cell.attrs.contains(CellAttrs::WIDE_SPACER) || cell.ch == ' ' || cell.ch == '\0' {
                continue;
            }
            let (mut fg, bg) = cell_colors(cell, style.theme, ov);
            if cell.attrs.contains(CellAttrs::HIDDEN) {
                continue;
            }
            if cell.attrs.contains(CellAttrs::DIM) {
                fg = fg.scaled(0.62);
            }
            if let Some((ccol, crow, ccolor)) = cursor_cell_fg {
                if col == ccol && row == crow {
                    fg = ccolor;
                }
            }
            let x = col as f32 * m.cell_w;
            let y = row as f32 * m.cell_h;

            // Block elements are geometry, not glyphs — same cell edges the
            // background fill used, so runs of them tile without seams.
            let span = if cell.attrs.contains(CellAttrs::WIDE) { 2 } else { 1 };
            let bx0 = (col as f32 * m.cell_w).round() as i32;
            let bx1 = ((col + span) as f32 * m.cell_w).round() as i32;
            let by0 = (row as f32 * m.cell_h).round() as i32;
            let by1 = ((row + 1) as f32 * m.cell_h).round() as i32;
            if draw_block(pix, cell.ch, bx0, by0, bx1, by1, fg) {
                continue;
            }

            let variant = Variant::select(
                cell.attrs.contains(CellAttrs::BOLD),
                cell.attrs.contains(CellAttrs::ITALIC),
            );
            if let Some(g) = r.glyph(cell.ch, variant, style.font_size) {
                let gx = x.round() as i32 + g.left;
                let gy = y.round() as i32 + m.baseline as i32 - g.top;
                match &g.pixels {
                    GlyphPixels::Mask(mask) => {
                        blit_mask(pix, gx, gy, g.width, g.height, mask, fg)
                    }
                    GlyphPixels::Color(rgba) => {
                        blit_rgba(pix, gx, gy, g.width, g.height, rgba)
                    }
                }
            }

            let span_w = if cell.attrs.contains(CellAttrs::WIDE) { m.cell_w * 2.0 } else { m.cell_w };
            if cell.attrs.contains(CellAttrs::UNDERLINE) {
                let ly = (y + m.baseline + (style.font_size * 0.11).max(1.5)).round() as i32;
                let lh = (style.font_size / 14.0).max(1.0).round() as i32;
                fill_rect(pix, x.round() as i32, ly, span_w.round() as i32, lh, fg);
            }
            if cell.attrs.contains(CellAttrs::STRIKEOUT) {
                let ly = (y + m.baseline - style.font_size * 0.3).round() as i32;
                let lh = (style.font_size / 14.0).max(1.0).round() as i32;
                fill_rect(pix, x.round() as i32, ly, span_w.round() as i32, lh, fg);
            }
            let _ = bg;
        }
    }

    composite_images(pix, snap, m.cell_w, m.cell_h);
}

/// Draws sixel/kitty images over the rendered grid (experimental).
fn composite_images(pix: &mut Pixmap, snap: &Snapshot, cell_w: f32, cell_h: f32) {
    use tiny_skia::{FilterQuality, PixmapPaint, Transform};
    for img in &snap.images {
        let Some(mut src) = Pixmap::new(img.width, img.height) else { continue };
        for (i, px) in img.rgba.chunks_exact(4).enumerate() {
            let a = px[3] as u16;
            let pm = |v: u8| ((v as u16 * a) / 255) as u8;
            src.pixels_mut()[i] =
                PremultipliedColorU8::from_rgba(pm(px[0]), pm(px[1]), pm(px[2]), px[3])
                    .unwrap_or(PremultipliedColorU8::TRANSPARENT);
        }
        let sx = (img.cols as f32 * cell_w) / img.width as f32;
        let sy = (img.rows as f32 * cell_h) / img.height as f32;
        pix.draw_pixmap(
            0,
            0,
            src.as_ref(),
            &PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() },
            Transform::from_row(sx, 0.0, 0.0, sy, img.col as f32 * cell_w, img.row as f32 * cell_h),
            None,
        );
    }
}

fn cell_colors(
    cell: &reel_term::Cell,
    theme: &Theme,
    overrides: &[(u8, (u8, u8, u8))],
) -> (Rgba, Rgba) {
    let mut fg = theme.resolve(cell.fg, overrides);
    let mut bg = theme.resolve(cell.bg, overrides);
    if cell.attrs.contains(CellAttrs::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    (fg, bg)
}

/// Draws the cursor; for a block cursor returns the cell whose glyph should
/// flip to the background color for contrast.
fn draw_cursor(
    pix: &mut Pixmap,
    snap: &Snapshot,
    style: &GridStyle,
    cell_w: f32,
    cell_h: f32,
) -> Option<(u16, u16, Rgba)> {
    let cur = snap.cursor;
    if cur.shape == CursorShape::Hidden || cur.col >= snap.cols || cur.row >= snap.rows {
        return None;
    }
    let shape = style.cursor_style.unwrap_or(cur.shape);
    let color = style.cursor_color.unwrap_or(style.theme.cursor);
    let (fcol, frow) = style.cursor_pos.unwrap_or((cur.col as f32, cur.row as f32));
    let at_rest = style.cursor_pos.is_none();
    let x = (fcol * cell_w).round() as i32;
    let y = (frow * cell_h).round() as i32;
    let w = cell_w.round() as i32;
    let h = cell_h.round() as i32;
    match shape {
        CursorShape::Block => {
            fill_rect(pix, x, y, w, h, color);
            // Mid-slide the block isn't cell-aligned, so no glyph to flip.
            at_rest.then_some((cur.col, cur.row, style.theme.bg))
        }
        CursorShape::Beam => {
            fill_rect(pix, x, y, (cell_w * 0.15).max(2.0).round() as i32, h, color);
            None
        }
        CursorShape::Underline => {
            let bar = (cell_h * 0.12).max(2.0).round() as i32;
            fill_rect(pix, x, y + h - bar, w, bar, color);
            None
        }
        CursorShape::Hidden => None,
    }
}

// ---------------------------------------------------------------------------
// Pixel helpers (premultiplied-alpha compositing over a Pixmap)
// ---------------------------------------------------------------------------

pub fn fill(pix: &mut Pixmap, c: Rgba) {
    let px = premul(c);
    pix.pixels_mut().fill(px);
}

pub fn fill_rect(pix: &mut Pixmap, x: i32, y: i32, w: i32, h: i32, c: Rgba) {
    if w <= 0 || h <= 0 {
        return;
    }
    let (pw, ph) = (pix.width() as i32, pix.height() as i32);
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(pw);
    let y1 = (y + h).min(ph);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let src = premul(c);
    let opaque = c.a == 255;
    let data = pix.pixels_mut();
    for yy in y0..y1 {
        let row = yy as usize * pw as usize;
        if opaque {
            data[row + x0 as usize..row + x1 as usize].fill(src);
        } else {
            for d in &mut data[row + x0 as usize..row + x1 as usize] {
                *d = blend(src, *d);
            }
        }
    }
}

pub fn blit_mask(pix: &mut Pixmap, x: i32, y: i32, w: u32, h: u32, mask: &[u8], color: Rgba) {
    let (pw, ph) = (pix.width() as i32, pix.height() as i32);
    let gx0 = (-x).max(0);
    let gy0 = (-y).max(0);
    let gx1 = (pw - x).min(w as i32);
    let gy1 = (ph - y).min(h as i32);
    if gx0 >= gx1 || gy0 >= gy1 {
        return;
    }
    let solid = premul(color);
    let data = pix.pixels_mut();
    for gy in gy0..gy1 {
        let srow = gy as usize * w as usize;
        let drow = (y + gy) as isize * pw as isize + x as isize;
        for gx in gx0..gx1 {
            let a = mask[srow + gx as usize] as u32;
            if a == 0 {
                continue;
            }
            let d = &mut data[(drow + gx as isize) as usize];
            // Fully-covered pixels (glyph interiors) skip the blend math.
            if a == 255 && color.a == 255 {
                *d = solid;
                continue;
            }
            let a = a * color.a as u32 / 255;
            let src = PremultipliedColorU8::from_rgba(
                (color.r as u32 * a / 255) as u8,
                (color.g as u32 * a / 255) as u8,
                (color.b as u32 * a / 255) as u8,
                a as u8,
            )
            .unwrap();
            *d = blend(src, *d);
        }
    }
}

pub fn blit_rgba(pix: &mut Pixmap, x: i32, y: i32, w: u32, h: u32, rgba: &[u8]) {
    let (pw, ph) = (pix.width() as i32, pix.height() as i32);
    let gx0 = (-x).max(0);
    let gy0 = (-y).max(0);
    let gx1 = (pw - x).min(w as i32);
    let gy1 = (ph - y).min(h as i32);
    if gx0 >= gx1 || gy0 >= gy1 {
        return;
    }
    let data = pix.pixels_mut();
    for gy in gy0..gy1 {
        let srow = gy as usize * w as usize;
        let drow = (y + gy) as isize * pw as isize + x as isize;
        for gx in gx0..gx1 {
            let i = (srow + gx as usize) * 4;
            let a = rgba[i + 3] as u32;
            if a == 0 {
                continue;
            }
            let src = PremultipliedColorU8::from_rgba(
                (rgba[i] as u32 * a / 255) as u8,
                (rgba[i + 1] as u32 * a / 255) as u8,
                (rgba[i + 2] as u32 * a / 255) as u8,
                a as u8,
            )
            .unwrap();
            let d = &mut data[(drow + gx as isize) as usize];
            *d = blend(src, *d);
        }
    }
}

pub fn premul(c: Rgba) -> PremultipliedColorU8 {
    let a = c.a as u32;
    PremultipliedColorU8::from_rgba(
        (c.r as u32 * a / 255) as u8,
        (c.g as u32 * a / 255) as u8,
        (c.b as u32 * a / 255) as u8,
        c.a,
    )
    .unwrap()
}

fn blend(src: PremultipliedColorU8, dst: PremultipliedColorU8) -> PremultipliedColorU8 {
    let ia = 255 - src.alpha() as u32;
    PremultipliedColorU8::from_rgba(
        (src.red() as u32 + dst.red() as u32 * ia / 255).min(255) as u8,
        (src.green() as u32 + dst.green() as u32 * ia / 255).min(255) as u8,
        (src.blue() as u32 + dst.blue() as u32 * ia / 255).min(255) as u8,
        (src.alpha() as u32 + dst.alpha() as u32 * ia / 255).min(255) as u8,
    )
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::builtin;
    use reel_cast::Cast;

    fn snap(body: &str, cols: u16, rows: u16) -> Snapshot {
        let text = format!(
            "{{\"version\": 2, \"width\": {cols}, \"height\": {rows}}}\n{body}"
        );
        let cast = Cast::parse(&text).unwrap();
        reel_term::replay(&cast).unwrap().pop().unwrap()
    }

    #[test]
    fn renders_text_pixels() {
        let s = snap(r#"[0.1, "o", "\u001b[31mhello\u001b[0m"]"#, 20, 4);
        let theme = builtin("reel-dark").unwrap();
        let mut r = Rasterizer::new(None).unwrap().0;
        let pix = raster_grid(&mut r, &s, &GridStyle::new(&theme, 17.0, 1.4, true));
        assert!(pix.width() > 100 && pix.height() > 40);
        // Some pixel in the first row band should be red-ish (fg Indexed(1)).
        let red = theme.ansi[1];
        let found = pix.pixels().iter().any(|p| {
            p.alpha() == 255 && p.red() as i32 > red.r as i32 - 60 && p.red() > p.blue() && p.red() > 120
        });
        assert!(found, "no red-ish glyph pixels found");
    }

    /// A run of full blocks must paint one solid bar. Font glyphs are
    /// rasterized at a fixed bitmap width while the cell advance is
    /// fractional, so blitting them leaves a 1px seam wherever the advance
    /// rounds up — visible as gaps between columns and as a chopped-up
    /// logo in any TUI that draws with block elements.
    #[test]
    fn a_run_of_full_blocks_has_no_seams() {
        let s = snap(r#"[0.1, "o", "\u2588\u2588\u2588\u2588\u2588\u2588\u2588\u2588"]"#, 12, 2);
        let theme = builtin("reel-dark").unwrap();
        let mut r = Rasterizer::new(None).unwrap().0;
        let pix = raster_grid(&mut r, &s, &GridStyle::new(&theme, 17.0, 1.4, false));
        let m = r.fonts.cell_metrics(17.0, 1.4);
        let y = (m.cell_h * 0.5) as u32;
        // Sample across the middle of the run, a pixel inside either end.
        let x0 = 1;
        let x1 = (8.0 * m.cell_w) as u32 - 2;
        let mut gaps = Vec::new();
        for x in x0..x1 {
            let p = pix.pixel(x, y).unwrap();
            if (p.red() as i32 - theme.fg.r as i32).abs() > 40 {
                gaps.push(x);
            }
        }
        assert!(gaps.is_empty(), "seams between block columns at x={gaps:?}");
    }

    /// The same for a horizontal box-drawing rule: one unbroken line.
    #[test]
    fn a_box_drawing_rule_is_unbroken() {
        let s = snap(r#"[0.1, "o", "\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500"]"#, 12, 2);
        let theme = builtin("reel-dark").unwrap();
        let mut r = Rasterizer::new(None).unwrap().0;
        let pix = raster_grid(&mut r, &s, &GridStyle::new(&theme, 17.0, 1.4, false));
        let m = r.fonts.cell_metrics(17.0, 1.4);
        let x1 = (8.0 * m.cell_w) as u32 - 2;
        // Find the row the rule sits on, then check it runs uninterrupted.
        let row = (0..(m.cell_h as u32))
            .max_by_key(|&y| {
                (1..x1).filter(|&x| pix.pixel(x, y).unwrap().alpha() > 200).count()
            })
            .unwrap();
        let gaps: Vec<u32> = (1..x1)
            .filter(|&x| pix.pixel(x, row).unwrap().alpha() < 128)
            .collect();
        assert!(gaps.is_empty(), "rule broken at x={gaps:?} (row {row})");
    }

    #[test]
    fn block_cursor_is_visible() {
        let s = snap(r#"[0.1, "o", "x"]"#, 10, 2);
        let theme = builtin("reel-dark").unwrap();
        let mut r = Rasterizer::new(None).unwrap().0;
        let pix = raster_grid(&mut r, &s, &GridStyle::new(&theme, 16.0, 1.2, true));
        let cur = theme.cursor;
        let found = pix.pixels().iter().any(|p| {
            (p.red() as i32 - cur.r as i32).abs() < 12
                && (p.green() as i32 - cur.g as i32).abs() < 12
                && (p.blue() as i32 - cur.b as i32).abs() < 12
        });
        assert!(found, "cursor color not present");
    }
}
