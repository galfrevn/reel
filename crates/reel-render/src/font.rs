//! System font discovery and glyph rasterization.
//!
//! reel uses the fonts installed on the machine (no fonts ship in the
//! binary). The primary family comes from the template/`[style] font` name,
//! or from a Nerd-Font-first preference chain; any glyph the primary lacks
//! (TUI icons, box drawing, braille spinners, emoji) is resolved by lazily
//! scanning the rest of the installed faces, symbol-ish families first.
//!
//! reel deliberately skips paragraph-layout shaping: a terminal is a grid,
//! every glyph goes at its cell origin and advances exactly one (or two, for
//! wide chars) cell widths. Rasterization goes through swash with a cache
//! keyed by (face, glyph, size). Output is deterministic for a given set of
//! installed fonts — install the same fonts to get the same bytes.

use std::collections::HashMap;
use swash::scale::image::{Content, Image};
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::FontRef;

/// Families tried in order when no font is named. Nerd Font builds come
/// first — their PUA icon coverage is what TUI demos need most.
const DEFAULT_CHAIN: &[&str] = &[
    "JetBrainsMono Nerd Font Mono",
    "JetBrainsMono NL Nerd Font Mono",
    "JetBrainsMono Nerd Font",
    "FiraCode Nerd Font Mono",
    "FiraCode Nerd Font",
    "Hack Nerd Font Mono",
    "MesloLGS NF",
    "JetBrains Mono",
    "Fira Code",
    "Cascadia Mono",
    "SF Mono",
    "Menlo",
    "Monaco",
    "Consolas",
    "DejaVu Sans Mono",
    "Ubuntu Mono",
    "Liberation Mono",
    "Courier New",
];

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

/// Index into the loaded face list. Face 0-3 are the primary family's
/// variants; higher slots are lazily loaded fallbacks.
pub type FaceSlot = u16;

struct LoadedFace {
    data: Vec<u8>,
    index: u32,
}

impl LoadedFace {
    fn font(&self) -> Option<FontRef<'_>> {
        FontRef::from_index(&self.data, self.index as usize)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CellMetrics {
    pub cell_w: f32,
    pub cell_h: f32,
    /// Distance from cell top to the text baseline.
    pub baseline: f32,
}

pub struct FontSet {
    db: fontdb::Database,
    faces: Vec<LoadedFace>,
    /// Face slot per primary variant (may repeat when styles are missing).
    primary: [FaceSlot; 4],
    /// char → face that has it, discovered lazily; None = tofu everywhere.
    char_slots: HashMap<char, Option<FaceSlot>>,
    /// fontdb faces already loaded (or rejected), so scans don't repeat work.
    scanned: HashMap<fontdb::ID, Option<FaceSlot>>,
    /// How many db faces the lazy scan has visited so far.
    scan_pos: usize,
    /// Scan order: symbol-ish and monospace faces first.
    scan_order: Vec<fontdb::ID>,
}

impl FontSet {
    /// Loads the system font database and resolves the primary family.
    /// Returns the set plus a warning when `preferred` wasn't found.
    pub fn system(preferred: Option<&str>) -> Result<(Self, Option<String>), String> {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();

        let mut set = FontSet {
            db,
            faces: Vec::new(),
            primary: [0; 4],
            char_slots: HashMap::new(),
            scanned: HashMap::new(),
            scan_pos: 0,
            scan_order: Vec::new(),
        };

        // Fallback scan order: Nerd Font / symbol / emoji families first,
        // then monospace, then everything else.
        let mut ranked: Vec<(u8, fontdb::ID)> = set
            .db
            .faces()
            .map(|f| {
                let name = f
                    .families
                    .first()
                    .map(|(n, _)| n.as_str())
                    .unwrap_or_default();
                let lower = name.to_ascii_lowercase();
                let rank = if lower.contains("nerd") || lower.contains("symbol") || lower.contains("powerline") {
                    0
                } else if lower.contains("emoji") || lower.contains("braille") {
                    1
                } else if f.monospaced {
                    2
                } else {
                    3
                };
                (rank, f.id)
            })
            .collect();
        ranked.sort_by_key(|(rank, _)| *rank);
        set.scan_order = ranked.into_iter().map(|(_, id)| id).collect();

        let warning = set.set_primary(preferred)?;
        Ok((set, warning))
    }

    /// (Re)resolves the primary family — used at load and when `reel watch`
    /// swaps templates. Loaded faces, caches, and the scan survive.
    pub fn set_primary(&mut self, preferred: Option<&str>) -> Result<Option<String>, String> {
        let mut warning = None;
        let mut chain: Vec<&str> = Vec::with_capacity(1 + DEFAULT_CHAIN.len());
        if let Some(p) = preferred {
            chain.push(p);
        }
        chain.extend_from_slice(DEFAULT_CHAIN);

        let mut resolved_family: Option<String> = None;
        for name in &chain {
            let base = fontdb::Query {
                families: &[fontdb::Family::Name(name)],
                weight: fontdb::Weight::NORMAL,
                stretch: fontdb::Stretch::Normal,
                style: fontdb::Style::Normal,
            };
            if self.db.query(&base).is_some() {
                resolved_family = Some(name.to_string());
                break;
            }
        }
        // Last resorts: whatever fontdb considers monospace, then nothing.
        if resolved_family.is_none() {
            let q = fontdb::Query {
                families: &[fontdb::Family::Monospace],
                weight: fontdb::Weight::NORMAL,
                stretch: fontdb::Stretch::Normal,
                style: fontdb::Style::Normal,
            };
            if let Some(id) = self.db.query(&q) {
                if let Some(info) = self.db.face(id) {
                    resolved_family = info.families.first().map(|(name, _)| name.clone());
                }
            }
        }
        let family = resolved_family.ok_or_else(|| {
            "no usable monospace font found on this system — install one (any Nerd Font \
             build is ideal for TUI icons)"
                .to_string()
        })?;
        if let Some(p) = preferred {
            if !family.eq_ignore_ascii_case(p) {
                warning = Some(format!(
                    "font `{p}` is not installed — using `{family}` \
                     (see `fc-list`/Font Book for names)"
                ));
            }
        }

        // Load the four variants; missing styles fall back to regular.
        let query = |db: &fontdb::Database, weight, style| {
            db.query(&fontdb::Query {
                families: &[fontdb::Family::Name(&family)],
                weight,
                stretch: fontdb::Stretch::Normal,
                style,
            })
        };
        let regular_id = query(&self.db, fontdb::Weight::NORMAL, fontdb::Style::Normal)
            .ok_or_else(|| format!("font family `{family}` disappeared mid-load"))?;
        let regular = self
            .load_face(regular_id)
            .ok_or_else(|| format!("could not read the font file for `{family}`"))?;
        let slot_for = |set: &mut Self, weight, style| -> FaceSlot {
            query(&set.db, weight, style)
                .and_then(|id| set.load_face(id))
                .unwrap_or(regular)
        };
        let bold = slot_for(self, fontdb::Weight::BOLD, fontdb::Style::Normal);
        let italic = slot_for(self, fontdb::Weight::NORMAL, fontdb::Style::Italic);
        let bold_italic = slot_for(self, fontdb::Weight::BOLD, fontdb::Style::Italic);
        self.primary = [regular, bold, italic, bold_italic];
        Ok(warning)
    }

    fn load_face(&mut self, id: fontdb::ID) -> Option<FaceSlot> {
        if let Some(slot) = self.scanned.get(&id) {
            return *slot;
        }
        let slot = self.db.face(id).and_then(|info| {
            let data: Vec<u8> = match &info.source {
                fontdb::Source::File(path) => std::fs::read(path).ok()?,
                fontdb::Source::Binary(bin) => bin.as_ref().as_ref().to_vec(),
                fontdb::Source::SharedFile(path, _) => std::fs::read(path).ok()?,
            };
            let face = LoadedFace { data, index: info.index };
            face.font()?; // reject files swash can't parse
            let slot = self.faces.len() as FaceSlot;
            self.faces.push(face);
            Some(slot)
        });
        self.scanned.insert(id, slot);
        slot
    }

    pub fn font(&self, slot: FaceSlot) -> FontRef<'_> {
        self.faces[slot as usize].font().expect("validated at load")
    }

    fn primary_slot(&self, v: Variant) -> FaceSlot {
        self.primary[v as usize]
    }

    /// Maps a char to (face, glyph): primary variant → primary regular →
    /// already-loaded fallbacks → lazy scan of remaining installed faces.
    pub fn glyph(&mut self, ch: char, v: Variant) -> Option<(FaceSlot, u16)> {
        for slot in [self.primary_slot(v), self.primary_slot(Variant::Regular)] {
            let id = self.font(slot).charmap().map(ch);
            if id != 0 {
                return Some((slot, id));
            }
        }
        if let Some(&cached) = self.char_slots.get(&ch) {
            return cached.map(|slot| (slot, self.font(slot).charmap().map(ch)));
        }
        // Check faces previous scans already loaded.
        for slot in 0..self.faces.len() as FaceSlot {
            let id = self.font(slot).charmap().map(ch);
            if id != 0 {
                self.char_slots.insert(ch, Some(slot));
                return Some((slot, id));
            }
        }
        // Walk the remaining installed faces until someone claims the char.
        while self.scan_pos < self.scan_order.len() {
            let face_id = self.scan_order[self.scan_pos];
            self.scan_pos += 1;
            let Some(slot) = self.load_face(face_id) else { continue };
            let id = self.font(slot).charmap().map(ch);
            if id != 0 {
                self.char_slots.insert(ch, Some(slot));
                return Some((slot, id));
            }
        }
        self.char_slots.insert(ch, None);
        None
    }

    /// Grid metrics for a given font size in pixels.
    pub fn cell_metrics(&self, font_size: f32, line_height: f32) -> CellMetrics {
        let font = self.font(self.primary_slot(Variant::Regular));
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

    /// The family reel actually resolved (for logs and error messages).
    pub fn primary_family(&self) -> String {
        self.font(self.primary_slot(Variant::Regular))
            .localized_strings()
            .find_by_id(swash::StringId::Family, None)
            .map(|s| s.chars().collect())
            .unwrap_or_default()
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
    slot: FaceSlot,
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
    /// Discovers system fonts, preferring `font` when given. The warning
    /// (preferred font missing) is for the caller to surface.
    pub fn new(font: Option<&str>) -> Result<(Self, Option<String>), String> {
        let (fonts, warning) = FontSet::system(font)?;
        Ok((
            Rasterizer { fonts, cache: GlyphCache::default(), scale_ctx: ScaleContext::new() },
            warning,
        ))
    }

    /// Rasterizes (or fetches) a glyph at `size` px.
    pub fn glyph(&mut self, ch: char, variant: Variant, size: f32) -> Option<&CachedGlyph> {
        let (slot, glyph) = self.fonts.glyph(ch, variant)?;
        let size_q = (size * 16.0).round() as u32;
        let key = GlyphKey { slot, glyph, size_q };
        let entry = self.cache.map.entry(key).or_insert_with(|| {
            let font = self.fonts.font(slot);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn set() -> FontSet {
        FontSet::system(None).expect("some monospace font exists").0
    }

    #[test]
    fn system_discovery_finds_a_monospace_primary() {
        let s = set();
        let m = s.cell_metrics(17.0, 1.4);
        assert!(m.cell_w > 4.0 && m.cell_w < 20.0, "cell_w {}", m.cell_w);
        assert!(m.cell_h >= 17.0);
        assert!(m.baseline > 0.0 && m.baseline < m.cell_h);
        assert!(!s.primary_family().is_empty());
    }

    #[test]
    fn ascii_variants_resolve_in_primary() {
        let mut s = set();
        let (reg, _) = s.glyph('M', Variant::Regular).expect("regular M");
        assert!(s.glyph('M', Variant::Bold).is_some());
        assert!(s.glyph('M', Variant::Italic).is_some());
        // Regular ASCII must come from the primary family, not a fallback.
        assert!(reg < 4, "M resolved via fallback slot {reg}");
    }

    #[test]
    fn common_terminal_glyphs_resolve_somewhere() {
        // Box drawing and common status symbols appear in almost every TUI;
        // the fallback scan must find them even when the primary lacks them.
        let mut s = set();
        for ch in ['─', '│', '╭', '✓', '→'] {
            assert!(s.glyph(ch, Variant::Regular).is_some(), "no face for {ch:?}");
        }
    }

    #[test]
    fn preferred_font_miss_warns_and_falls_back() {
        let (s, warning) = FontSet::system(Some("Definitely Not A Font 9000")).unwrap();
        assert!(warning.unwrap().contains("Definitely Not A Font 9000"));
        assert!(!s.primary_family().is_empty());
    }

    #[test]
    fn unknown_char_is_negative_cached_not_fatal() {
        let mut s = set();
        // Plane-16 private use: no sane font covers it.
        assert!(s.glyph('\u{10FF00}', Variant::Regular).is_none());
        assert!(s.glyph('\u{10FF00}', Variant::Regular).is_none());
    }

    #[test]
    fn glyphs_rasterize_and_cache() {
        let (mut r, _) = Rasterizer::new(None).unwrap();
        {
            let g = r.glyph('A', Variant::Regular, 17.0).expect("glyph A");
            assert!(g.width > 0 && g.height > 0);
            assert!(matches!(g.pixels, GlyphPixels::Mask(_)));
        }
        // Space typically produces no image.
        assert!(r.glyph(' ', Variant::Regular, 17.0).is_none());
    }
}
