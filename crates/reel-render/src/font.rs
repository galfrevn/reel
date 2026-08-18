//! Embedded font handling and glyph rasterization.
//!
//! reel deliberately skips paragraph-layout shaping: a terminal is a grid,
//! every glyph goes at its cell origin and advances exactly one (or two, for
//! wide chars) cell widths. We map chars through the font's charmap and
//! rasterize with swash, with a cache keyed by (glyph, variant, size) —
//! terminal content is repetitive enough that rasterization is ~free after
//! the first frame.

use std::collections::HashMap;
use swash::scale::image::{Content, Image};
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::FontRef;

// The Nerd Font build ships PUA icon glyphs, which is a hard requirement:
// tofu where the user's TUI shows icons would kill the output.
static FONT_REGULAR: &[u8] =
    include_bytes!("../../../assets/fonts/JetBrainsMonoNLNerdFont-Regular.ttf");
static FONT_BOLD: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMonoNLNerdFont-Bold.ttf");
static FONT_ITALIC: &[u8] =
    include_bytes!("../../../assets/fonts/JetBrainsMonoNLNerdFont-Italic.ttf");
static FONT_BOLD_ITALIC: &[u8] =
    include_bytes!("../../../assets/fonts/JetBrainsMonoNLNerdFont-BoldItalic.ttf");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variant {
    Regular = 0,
    Bold = 1,
    Italic = 2,
    BoldItalic = 3,
}

impl Variant {
    pub fn select(bold: bool, italic: bool) -> Self {
        match (bold, italic) {
            (false, false) => Variant::Regular,
            (true, false) => Variant::Bold,
            (false, true) => Variant::Italic,
            (true, true) => Variant::BoldItalic,
        }
    }
}

pub struct FontSet {
    fonts: [FontRef<'static>; 4],
}

#[derive(Debug, Clone, Copy)]
pub struct CellMetrics {
    pub cell_w: f32,
    pub cell_h: f32,
    /// Distance from cell top to the text baseline.
    pub baseline: f32,
}

impl FontSet {
    pub fn embedded() -> Self {
        let load = |data: &'static [u8]| {
            FontRef::from_index(data, 0).expect("embedded font is valid")
        };
        FontSet {
            fonts: [
                load(FONT_REGULAR),
                load(FONT_BOLD),
                load(FONT_ITALIC),
                load(FONT_BOLD_ITALIC),
            ],
        }
    }

    pub fn font(&self, v: Variant) -> FontRef<'static> {
        self.fonts[v as usize]
    }

    /// Maps a char to (variant-adjusted) glyph id; falls back to Regular when
    /// a styled variant lacks the codepoint (common for PUA icons in Bold).
    pub fn glyph(&self, ch: char, v: Variant) -> Option<(Variant, u16)> {
        let id = self.font(v).charmap().map(ch);
        if id != 0 {
            return Some((v, id));
        }
        let id = self.font(Variant::Regular).charmap().map(ch);
        (id != 0).then_some((Variant::Regular, id))
    }

    /// Grid metrics for a given font size in pixels.
    pub fn cell_metrics(&self, font_size: f32, line_height: f32) -> CellMetrics {
        let font = self.font(Variant::Regular);
        let m = font.metrics(&[]).scale(font_size);
        let gm = font.glyph_metrics(&[]).scale(font_size);
        let m_id = font.charmap().map('M');
        let cell_w = gm.advance_width(m_id);
        let natural = m.ascent + m.descent;
        let cell_h = (font_size * line_height).max(natural).round();
        // Center the natural line box inside the (usually taller) cell.
        let baseline = ((cell_h - natural) / 2.0 + m.ascent).round();
        CellMetrics { cell_w, cell_h, baseline }
    }
}

/// A rasterized glyph ready to blit.
pub struct CachedGlyph {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub pixels: GlyphPixels,
}

pub enum GlyphPixels {
    /// 8-bit alpha mask, tinted with the cell fg at blit time.
    Mask(Vec<u8>),
    /// Straight-alpha RGBA (color emoji / COLR glyphs).
    Color(Vec<u8>),
}

#[derive(PartialEq, Eq, Hash)]
struct GlyphKey {
    variant: Variant,
    glyph: u16,
    /// Size quantized to 1/16 px so animated zoom doesn't explode the cache.
    size_q: u32,
}

#[derive(Default)]
pub struct GlyphCache {
    map: HashMap<GlyphKey, Option<CachedGlyph>>,
}

pub struct Rasterizer {
    pub fonts: FontSet,
    cache: GlyphCache,
    scale_ctx: ScaleContext,
}

impl Rasterizer {
    pub fn new() -> Self {
        Rasterizer { fonts: FontSet::embedded(), cache: GlyphCache::default(), scale_ctx: ScaleContext::new() }
    }

    /// Rasterizes (or fetches) a glyph at `size` px.
    pub fn glyph(&mut self, ch: char, variant: Variant, size: f32) -> Option<&CachedGlyph> {
        let (variant, glyph) = self.fonts.glyph(ch, variant)?;
        let size_q = (size * 16.0).round() as u32;
        let key = GlyphKey { variant, glyph, size_q };
        let entry = self.cache.map.entry(key).or_insert_with(|| {
            let font = self.fonts.font(variant);
            let mut scaler = self
                .scale_ctx
                .builder(font)
                .size(size_q as f32 / 16.0)
                .hint(true)
                .build();
            let img: Option<Image> = Render::new(&[
                Source::ColorOutline(0),
                Source::ColorBitmap(StrikeWith::BestFit),
                Source::Outline,
            ])
            .render(&mut scaler, glyph);
            img.and_then(|img| {
                if img.placement.width == 0 || img.placement.height == 0 {
                    return None;
                }
                let pixels = match img.content {
                    Content::Mask => GlyphPixels::Mask(img.data),
                    Content::Color => GlyphPixels::Color(img.data),
                    // Subpixel output is never requested.
                    Content::SubpixelMask => return None,
                };
                Some(CachedGlyph {
                    left: img.placement.left,
                    top: img.placement.top,
                    width: img.placement.width,
                    height: img.placement.height,
                    pixels,
                })
            })
        });
        entry.as_ref()
    }
}

impl Default for Rasterizer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_fonts_load_and_are_monospace() {
        let fonts = FontSet::embedded();
        let m = fonts.cell_metrics(17.0, 1.4);
        assert!(m.cell_w > 5.0 && m.cell_w < 17.0);
        assert!(m.cell_h >= 17.0);
        assert!(m.baseline > 0.0 && m.baseline < m.cell_h);
    }

    #[test]
    fn ascii_and_nerd_font_icons_have_glyphs() {
        let fonts = FontSet::embedded();
        assert!(fonts.glyph('M', Variant::Regular).is_some());
        assert!(fonts.glyph('M', Variant::Bold).is_some());
        // Nerd Font PUA: git branch icon and folder icon.
        assert!(fonts.glyph('\u{e0a0}', Variant::Regular).is_some(), "PUA e0a0 missing");
        assert!(fonts.glyph('\u{f07b}', Variant::Regular).is_some(), "PUA f07b missing");
        // Box drawing.
        assert!(fonts.glyph('─', Variant::Regular).is_some());
        assert!(fonts.glyph('╭', Variant::Regular).is_some());
    }

    #[test]
    fn glyphs_rasterize_and_cache() {
        let mut r = Rasterizer::new();
        {
            let g = r.glyph('A', Variant::Regular, 17.0).expect("glyph A");
            assert!(g.width > 0 && g.height > 0);
            assert!(matches!(g.pixels, GlyphPixels::Mask(_)));
        }
        // Space typically produces no image.
        assert!(r.glyph(' ', Variant::Regular, 17.0).is_none());
    }
}
