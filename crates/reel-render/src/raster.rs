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

    fill(pix, style.theme.bg);

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
    let color = style.theme.cursor;
    let x = (cur.col as f32 * cell_w).round() as i32;
    let y = (cur.row as f32 * cell_h).round() as i32;
    let w = cell_w.round() as i32;
    let h = cell_h.round() as i32;
    match cur.shape {
        CursorShape::Block => {
            fill_rect(pix, x, y, w, h, color);
            Some((cur.col, cur.row, style.theme.bg))
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
    for p in pix.pixels_mut() {
        *p = px;
    }
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
        for xx in x0..x1 {
            let d = &mut data[row + xx as usize];
            *d = if opaque { src } else { blend(src, *d) };
        }
    }
}

pub fn blit_mask(pix: &mut Pixmap, x: i32, y: i32, w: u32, h: u32, mask: &[u8], color: Rgba) {
    let (pw, ph) = (pix.width() as i32, pix.height() as i32);
    let data = pix.pixels_mut();
    for gy in 0..h as i32 {
        let py = y + gy;
        if py < 0 || py >= ph {
            continue;
        }
        for gx in 0..w as i32 {
            let px = x + gx;
            if px < 0 || px >= pw {
                continue;
            }
            let a = mask[(gy as u32 * w + gx as u32) as usize] as u32;
            if a == 0 {
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
            let d = &mut data[py as usize * pw as usize + px as usize];
            *d = blend(src, *d);
        }
    }
}

pub fn blit_rgba(pix: &mut Pixmap, x: i32, y: i32, w: u32, h: u32, rgba: &[u8]) {
    let (pw, ph) = (pix.width() as i32, pix.height() as i32);
    let data = pix.pixels_mut();
    for gy in 0..h as i32 {
        let py = y + gy;
        if py < 0 || py >= ph {
            continue;
        }
        for gx in 0..w as i32 {
            let px = x + gx;
            if px < 0 || px >= pw {
                continue;
            }
            let i = ((gy as u32 * w + gx as u32) * 4) as usize;
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
            let d = &mut data[py as usize * pw as usize + px as usize];
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
        let pix = raster_grid(&mut r, &s, &GridStyle { theme: &theme, font_size: 17.0, line_height: 1.4, cursor_visible: true });
        assert!(pix.width() > 100 && pix.height() > 40);
        // Some pixel in the first row band should be red-ish (fg Indexed(1)).
        let red = theme.ansi[1];
        let found = pix.pixels().iter().any(|p| {
            p.alpha() == 255 && p.red() as i32 > red.r as i32 - 60 && p.red() > p.blue() && p.red() > 120
        });
        assert!(found, "no red-ish glyph pixels found");
    }

    #[test]
    fn block_cursor_is_visible() {
        let s = snap(r#"[0.1, "o", "x"]"#, 10, 2);
        let theme = builtin("reel-dark").unwrap();
        let mut r = Rasterizer::new(None).unwrap().0;
        let pix = raster_grid(&mut r, &s, &GridStyle { theme: &theme, font_size: 16.0, line_height: 1.2, cursor_visible: true });
        let cur = theme.cursor;
        let found = pix.pixels().iter().any(|p| {
            (p.red() as i32 - cur.r as i32).abs() < 12
                && (p.green() as i32 - cur.g as i32).abs() < 12
                && (p.blue() as i32 - cur.b as i32).abs() < 12
        });
        assert!(found, "cursor color not present");
    }
}
