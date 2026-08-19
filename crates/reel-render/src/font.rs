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
static GEIST_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/geist/GeistMono-Regular.ttf");
static GEIST_BOLD: &[u8] = include_bytes!("../../../assets/fonts/geist/GeistMono-Bold.ttf");

/// Embedded font family. JetBrains Mono NL Nerd Font is the universal
/// fallback: any glyph a family lacks (PUA icons especially) resolves there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Family {
    #[default]
    JetBrainsMono = 0,
    GeistMono = 1,
}

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
    /// Geist Mono ships Regular and Bold only; italic styles fall back.
    geist: [FontRef<'static>; 2],
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
            geist: [load(GEIST_REGULAR), load(GEIST_BOLD)],
        }
    }

    pub fn font(&self, family: Family, v: Variant) -> FontRef<'static> {
        match family {
            Family::JetBrainsMono => self.fonts[v as usize],
            // Geist has no italics: Italic → Regular, BoldItalic → Bold.
            Family::GeistMono => {
                let bold = matches!(v, Variant::Bold | Variant::BoldItalic);
                self.geist[bold as usize]
            }
        }
    }

    /// Maps a char to a glyph, walking the fallback chain: requested
    /// family+variant → family regular → JetBrains Mono NF variant →
    /// JetBrains Mono NF regular (which carries the PUA icon set).
    pub fn glyph(&self, ch: char, family: Family, v: Variant) -> Option<(Family, Variant, u16)> {
        let chain: [(Family, Variant); 4] = [
            (family, v),
            (family, Variant::Regular),
            (Family::JetBrainsMono, v),
            (Family::JetBrainsMono, Variant::Regular),
        ];
        for (f, var) in chain {
            let id = self.font(f, var).charmap().map(ch);
            if id != 0 {
                return Some((f, var, id));
            }
        }
        None
    }

    /// Grid metrics for a given font size in pixels.
    pub fn cell_metrics(&self, family: Family, font_size: f32, line_height: f32) -> CellMetrics {
        let font = self.font(family, Variant::Regular);
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
    family: Family,
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
    pub fn glyph(
        &mut self,
        ch: char,
        family: Family,
        variant: Variant,
        size: f32,
    ) -> Option<&CachedGlyph> {
        let (family, variant, glyph) = self.fonts.glyph(ch, family, variant)?;
        let size_q = (size * 16.0).round() as u32;
        let key = GlyphKey { family, variant, glyph, size_q };
        let entry = self.cache.map.entry(key).or_insert_with(|| {
            let font = self.fonts.font(family, variant);
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
        let m = fonts.cell_metrics(Family::JetBrainsMono, 17.0, 1.4);
        assert!(m.cell_w > 5.0 && m.cell_w < 17.0);
        assert!(m.cell_h >= 17.0);
        assert!(m.baseline > 0.0 && m.baseline < m.cell_h);
    }

    #[test]
    fn ascii_and_nerd_font_icons_have_glyphs() {
        let fonts = FontSet::embedded();
        let jb = Family::JetBrainsMono;
        assert!(fonts.glyph('M', jb, Variant::Regular).is_some());
        assert!(fonts.glyph('M', jb, Variant::Bold).is_some());
        // Nerd Font PUA: git branch icon and folder icon.
        assert!(fonts.glyph('\u{e0a0}', jb, Variant::Regular).is_some(), "PUA e0a0 missing");
        assert!(fonts.glyph('\u{f07b}', jb, Variant::Regular).is_some(), "PUA f07b missing");
        // Box drawing.
        assert!(fonts.glyph('─', jb, Variant::Regular).is_some());
        assert!(fonts.glyph('╭', jb, Variant::Regular).is_some());
    }

    #[test]
    fn geist_falls_back_to_nerd_font_for_icons() {
        let fonts = FontSet::embedded();
        let g = Family::GeistMono;
        // Native Geist glyph stays in-family.
        let (fam, _, _) = fonts.glyph('M', g, Variant::Regular).unwrap();
        assert_eq!(fam, Family::GeistMono);
        // PUA icon must resolve through the JetBrains Mono NF fallback.
        let (fam, _, _) = fonts.glyph('\u{e0a0}', g, Variant::Regular).unwrap();
        assert_eq!(fam, Family::JetBrainsMono);
        // Italic silently maps to Regular (Geist has no italics).
        assert!(fonts.glyph('M', g, Variant::Italic).is_some());
    }

    #[test]
    fn glyphs_rasterize_and_cache() {
        let mut r = Rasterizer::new();
        {
            let g = r.glyph('A', Family::JetBrainsMono, Variant::Regular, 17.0).expect("glyph A");
            assert!(g.width > 0 && g.height > 0);
            assert!(matches!(g.pixels, GlyphPixels::Mask(_)));
        }
        // Space typically produces no image.
        assert!(r.glyph(' ', Family::JetBrainsMono, Variant::Regular, 17.0).is_none());
    }
}
