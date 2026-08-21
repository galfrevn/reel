//! Drawn-on overlays: callout notes, title cards, the speed chip, and the
//! progress bar.
//!
//! These all composite onto the finished canvas (chrome included), except
//! where a cell anchor is involved — those go through
//! [`OverlayStyle::cell_to_canvas`] so they keep pointing at the right cell
//! under `zoom`/`pan`.

use crate::chrome::{self, Layout};
use crate::font::{Rasterizer, Variant};
use crate::plan::{CardDraw, NoteDraw};
use crate::raster;
use crate::template::{Overlay, Template};
use crate::theme::{Rgba, Theme};
use reel_timeline::{NoteSide, NoteStyle};
use tiny_skia::{Pixmap, Rect};

/// The overlay palette, with every template/theme fallback already applied.
#[derive(Debug, Clone, Copy)]
pub struct OverlayStyle {
    pub bg: Rgba,
    pub fg: Rgba,
    pub accent: Rgba,
    /// Corner radius in logical px (multiply by the supersampling scale).
    pub radius: f32,
}

impl OverlayStyle {
    pub fn resolve(tpl: &Template, theme: &Theme) -> OverlayStyle {
        let o: Overlay = tpl.overlay;
        // Near-opaque on purpose: a note exists to be read, and terminal
        // text ghosting through it is exactly what makes it hard to.
        let opacity = o.opacity.unwrap_or(0.98).clamp(0.0, 1.0);
        let bg = o.bg.unwrap_or_else(|| surface(theme.bg));
        OverlayStyle {
            bg: Rgba { a: (opacity * 255.0) as u8, ..bg },
            fg: o.fg.unwrap_or(theme.fg),
            accent: o.accent.unwrap_or(theme.cursor),
            radius: o.radius.unwrap_or(8.0),
        }
    }

    fn with_alpha(c: Rgba, alpha: f32) -> Rgba {
        Rgba { a: (c.a as f32 * alpha.clamp(0.0, 1.0)) as u8, ..c }
    }
}

/// A card color that reads as a surface floating over `bg`: lifted on dark
/// themes, dropped on light ones. Darkening unconditionally would make a
/// card on a near-black terminal invisible except for its outline.
fn surface(bg: Rgba) -> Rgba {
    let lum = (0.299 * bg.r as f32 + 0.587 * bg.g as f32 + 0.114 * bg.b as f32) / 255.0;
    let mix = |c: u8, toward: f32, k: f32| (c as f32 + (toward - c as f32) * k) as u8;
    let (toward, k) = if lum < 0.5 { (255.0, 0.16) } else { (0.0, 0.10) };
    Rgba { r: mix(bg.r, toward, k), g: mix(bg.g, toward, k), b: mix(bg.b, toward, k), a: 255 }
}

/// Where a cell sits on the finished canvas, given the camera's crop.
///
/// `view_off` is the zoom viewport origin in zoomed pixels and `cell` the
/// current cell size — the same pair the highlight math uses, which is what
/// keeps anchors glued to their cell while the camera moves.
pub fn cell_to_canvas(
    l: &Layout,
    view_off: (i32, i32),
    cell: (f32, f32),
    col: u16,
    row: u16,
) -> (f32, f32) {
    (
        l.term_x + (col as f32 + 0.5) * cell.0 - view_off.0 as f32,
        l.term_y + (row as f32 + 0.5) * cell.1 - view_off.1 as f32,
    )
}

/// Greedy wrap at `max_chars`, breaking on spaces and keeping explicit
/// newlines. Monospace, so character count is width.
pub fn wrap_text(text: &str, max_chars: usize) -> Vec<String> {
    let max = max_chars.max(1);
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        let mut len = 0;
        for word in para.split_whitespace() {
            let wl = word.chars().count();
            if len > 0 && len + 1 + wl > max {
                out.push(std::mem::take(&mut line));
                len = 0;
            }
            if len > 0 {
                line.push(' ');
                len += 1;
            }
            // A single word longer than the line gets hard-split.
            if wl > max {
                for ch in word.chars() {
                    if len == max {
                        out.push(std::mem::take(&mut line));
                        len = 0;
                    }
                    line.push(ch);
                    len += 1;
                }
            } else {
                line.push_str(word);
                len += wl;
            }
        }
        out.push(line);
    }
    out
}

/// Stroke weight for leader lines and highlight boxes, in logical px.
const STROKE: f32 = 1.75;

/// A callout card pointing at `anchor_px` on the canvas.
///
/// The card arrives the way a tooltip should: the leader line draws itself
/// from the anchor outward, the dot lands with a ring, and the card rises
/// into place from the anchor's direction with a spring overshoot. What sits
/// still at the end is a plain elevated surface — a shadow and a hairline,
/// no coloured outline boxing the text in.
#[allow(clippy::too_many_arguments)]
pub fn draw_note(
    raster: &mut Rasterizer,
    canvas: &mut Pixmap,
    note: &NoteDraw,
    anchor_px: (f32, f32),
    style: &OverlayStyle,
    font_size: f32,
    s: f32,
) {
    if !note.anim.visible() {
        return;
    }
    let size = (font_size * 0.85 * s).max(9.0);
    let m = raster.fonts.cell_metrics(size, 1.0);
    let (cw, ch) = (canvas.width() as f32, canvas.height() as f32);
    let max_chars = ((cw * 0.4 / m.cell_w) as usize).clamp(12, 38);
    let lines = wrap_text(&note.text, max_chars);
    let widest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);

    let pad_x = 15.0 * s;
    let pad_y = 11.0 * s;
    let line_h = m.cell_h * 1.3;
    let box_w = widest as f32 * m.cell_w + pad_x * 2.0;
    let box_h = lines.len() as f32 * line_h + pad_y * 2.0;
    // How far the card floats off the cell it points at.
    let lead = 30.0 * s;

    let side = resolve_side(note.side, anchor_px, (cw, ch), (box_w, box_h), lead);
    let (mut x, mut y) = match side {
        NoteSide::Up => (anchor_px.0 - box_w / 2.0, anchor_px.1 - lead - box_h),
        NoteSide::Down => (anchor_px.0 - box_w / 2.0, anchor_px.1 + lead),
        NoteSide::Left => (anchor_px.0 - lead - box_w, anchor_px.1 - box_h / 2.0),
        // Auto resolved above; Right is the remaining case.
        _ => (anchor_px.0 + lead, anchor_px.1 - box_h / 2.0),
    };
    let margin = 10.0 * s;
    x = x.clamp(margin, (cw - box_w - margin).max(margin));
    y = y.clamp(margin, (ch - box_h - margin).max(margin));

    // --- Motion -----------------------------------------------------------
    // The card slides in *from* the anchor and settles, so the eye is led
    // outward from the thing being pointed at rather than ambushed.
    let t = note.anim.t;
    let a = note.anim.alpha;
    let slip = (1.0 - t) * 14.0 * s;
    let (dx, dy) = match side {
        NoteSide::Up => (0.0, slip),
        NoteSide::Down => (0.0, -slip),
        NoteSide::Left => (slip, 0.0),
        _ => (-slip, 0.0),
    };
    // Scale about the card's own center, riding the same spring.
    let scale = 0.94 + 0.06 * t;
    let sw = box_w * scale;
    let sh = box_h * scale;
    let sx = x + dx + (box_w - sw) / 2.0;
    let sy = y + dy + (box_h - sh) / 2.0;

    let bg = OverlayStyle::with_alpha(style.bg, a);
    let fg = OverlayStyle::with_alpha(style.fg, a);
    let accent = OverlayStyle::with_alpha(style.accent, a);
    let Some(rect) = Rect::from_xywh(sx, sy, sw, sh) else { return };
    let radius = style.radius * s * scale;

    // The leader draws from the anchor toward the card, reaching it as the
    // card settles — the line is what earns the card its place on screen.
    let reach = ease_out(t.clamp(0.0, 1.0));
    let edge = edge_point(rect, anchor_px);
    let tip = (
        anchor_px.0 + (edge.0 - anchor_px.0) * reach,
        anchor_px.1 + (edge.1 - anchor_px.1) * reach,
    );

    match note.style {
        NoteStyle::Card => {
            chrome::stroke_line(canvas, anchor_px, tip, accent, STROKE * s);
            // A ring that expands and fades as the dot lands.
            let ring = 1.0 - t.clamp(0.0, 1.0);
            if ring > 0.02 {
                let halo = Rgba { a: (a * ring * 90.0) as u8, ..style.accent };
                chrome::fill_circle(canvas, anchor_px.0, anchor_px.1, (4.0 + 10.0 * ring) * s, halo);
            }
            shadow_rounded(canvas, rect, radius, a, s);
            chrome::fill_rounded(canvas, rect, radius, bg);
            hairline(canvas, rect, radius, style, a, s);
            chrome::fill_circle(canvas, anchor_px.0, anchor_px.1, 3.5 * s, accent);
        }
        NoteStyle::Bubble => {
            shadow_rounded(canvas, rect, radius, a, s);
            // Tail first, body over it: the two share a translucent color,
            // and blending them twice would print the overlap as a seam.
            // It grows with the card rather than reaching for the anchor.
            let len = 10.0 * s * reach.max(0.001);
            chrome::fill_tri(canvas, tail_tip(rect, anchor_px, len, radius), bg);
            chrome::fill_rounded(canvas, rect, radius, bg);
            hairline(canvas, rect, radius, style, a, s);
        }
    }

    // Text fades a beat behind the surface it sits on.
    let text_a = ease_out((a - 0.35).max(0.0) / 0.65);
    let fg = OverlayStyle::with_alpha(fg, text_a);
    let mut baseline = sy + pad_y * scale + m.baseline;
    for line in &lines {
        crate::draw_text(raster, canvas, line, sx + pad_x * scale, baseline, size, fg, Variant::Regular);
        baseline += line_h;
    }
}

fn ease_out(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    1.0 - (1.0 - x).powi(3)
}

/// A soft drop shadow under an overlay surface: concentric rounded rects at
/// low alpha. Cheap next to a real blur, and at these sizes indistinguishable.
fn shadow_rounded(canvas: &mut Pixmap, rect: Rect, radius: f32, alpha: f32, s: f32) {
    let steps = 6;
    for i in (1..=steps).rev() {
        let grow = i as f32 * 1.6 * s;
        let a = (alpha * 16.0 * (1.0 - i as f32 / (steps as f32 + 1.0))) as u8;
        if a == 0 {
            continue;
        }
        if let Some(r) = Rect::from_xywh(
            rect.x() - grow,
            rect.y() - grow + 2.0 * s,
            rect.width() + grow * 2.0,
            rect.height() + grow * 2.0,
        ) {
            chrome::fill_rounded(canvas, r, radius + grow, Rgba { r: 0, g: 0, b: 0, a });
        }
    }
}

/// The 1px light edge that makes a surface read as raised rather than as a
/// hole punched in the frame.
fn hairline(canvas: &mut Pixmap, rect: Rect, radius: f32, style: &OverlayStyle, alpha: f32, s: f32) {
    let edge = Rgba { a: (alpha * 38.0) as u8, ..style.fg };
    chrome::stroke_rounded(canvas, rect, radius, edge, 1.0 * s);
}

/// Picks the side of the anchor with the most room for the card.
fn resolve_side(
    want: NoteSide,
    anchor: (f32, f32),
    canvas: (f32, f32),
    box_size: (f32, f32),
    lead: f32,
) -> NoteSide {
    if want != NoteSide::Auto {
        return want;
    }
    let room = [
        (NoteSide::Up, anchor.1 - lead - box_size.1),
        (NoteSide::Down, canvas.1 - anchor.1 - lead - box_size.1),
        (NoteSide::Right, canvas.0 - anchor.0 - lead - box_size.0),
        (NoteSide::Left, anchor.0 - lead - box_size.0),
    ];
    room.iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(side, _)| *side)
        .unwrap_or(NoteSide::Down)
}

/// The point on `rect`'s edge closest to `to` — where a leader line starts.
fn edge_point(rect: Rect, to: (f32, f32)) -> (f32, f32) {
    (
        to.0.clamp(rect.left(), rect.right()),
        to.1.clamp(rect.top(), rect.bottom()),
    )
}

/// Three points making a bubble tail from `rect` toward `to`.
/// A speech-bubble tail: a short stub sitting flush on whichever edge faces
/// `to`, leaning toward it.
///
/// It deliberately does *not* reach the anchor. A triangle stretched across
/// the whole gap reads as a spike stuck to the card; a stub the height of a
/// text line reads as a speech bubble, and the anchor is already identified
/// by where the bubble sits.
fn tail_tip(rect: Rect, to: (f32, f32), len: f32, radius: f32) -> [(f32, f32); 3] {
    let half = len * 0.85;
    // Keep the base on the flat part of the edge, clear of the corners.
    let inset = radius + half;
    if to.1 < rect.top() {
        let cx = to.0.clamp(rect.left() + inset, (rect.right() - inset).max(rect.left() + inset));
        let tip_x = to.0.clamp(cx - half, cx + half);
        [(cx - half, rect.top()), (cx + half, rect.top()), (tip_x, rect.top() - len)]
    } else if to.1 > rect.bottom() {
        let cx = to.0.clamp(rect.left() + inset, (rect.right() - inset).max(rect.left() + inset));
        let tip_x = to.0.clamp(cx - half, cx + half);
        [(cx - half, rect.bottom()), (cx + half, rect.bottom()), (tip_x, rect.bottom() + len)]
    } else if to.0 < rect.left() {
        let cy = to.1.clamp(rect.top() + inset, (rect.bottom() - inset).max(rect.top() + inset));
        let tip_y = to.1.clamp(cy - half, cy + half);
        [(rect.left(), cy - half), (rect.left(), cy + half), (rect.left() - len, tip_y)]
    } else {
        let cy = to.1.clamp(rect.top() + inset, (rect.bottom() - inset).max(rect.top() + inset));
        let tip_y = to.1.clamp(cy - half, cy + half);
        [(rect.right(), cy - half), (rect.right(), cy + half), (rect.right() + len, tip_y)]
    }
}

/// A full-frame title card: a scrim in the template's canvas color with the
/// text centered on it. The frozen frame ghosts through.
pub fn draw_card(
    raster: &mut Rasterizer,
    canvas: &mut Pixmap,
    card: &CardDraw,
    scrim: Rgba,
    style: &OverlayStyle,
    font_size: f32,
    s: f32,
) {
    if !card.anim.visible() {
        return;
    }
    let a = card.anim.alpha;
    let t = card.anim.t;
    let (cw, ch) = (canvas.width() as f32, canvas.height() as f32);
    // The scrim closes first so the title never lands on live content.
    let scrim_a = ease_out((a * 1.35).min(1.0));
    raster::fill_rect(
        canvas,
        0,
        0,
        cw as i32,
        ch as i32,
        Rgba { a: (235.0 * scrim_a) as u8, ..scrim },
    );

    let size = (font_size * 1.8 * s).max(14.0);
    let m = raster.fonts.cell_metrics(size, 1.0);
    let max_chars = ((cw * 0.8 / m.cell_w) as usize).max(8);
    let lines = wrap_text(&card.text, max_chars);
    let line_h = m.cell_h * 1.3;
    let block_h = lines.len() as f32 * line_h;

    // The title rises into place and the lines stagger, ~70ms apart.
    let text_a = ease_out(((a - 0.25) / 0.75).max(0.0));
    let fg = OverlayStyle::with_alpha(Rgba { a: 255, ..style.fg }, text_a);
    let mut baseline = (ch - block_h) / 2.0 + m.baseline;
    for (i, line) in lines.iter().enumerate() {
        let stagger = (t - i as f32 * 0.12).clamp(0.0, 1.0);
        let rise = (1.0 - ease_out(stagger)) * 26.0 * s;
        let w = line.chars().count() as f32 * m.cell_w;
        crate::draw_text(
            raster,
            canvas,
            line,
            (cw - w) / 2.0,
            baseline + rise,
            size,
            OverlayStyle::with_alpha(fg, ease_out(stagger)),
            Variant::Bold,
        );
        baseline += line_h;
    }

    // A hairline rule that wipes out from the center under the title —
    // the one flourish, and it doubles as a progress-free "this is a card"
    // signal.
    let rule_w = (cw * 0.14 * ease_out(t)).max(0.0);
    if rule_w > 1.0 {
        let rule = Rgba { a: (text_a * 90.0) as u8, ..style.accent };
        raster::fill_rect(
            canvas,
            ((cw - rule_w) / 2.0) as i32,
            (baseline + 10.0 * s) as i32,
            rule_w as i32,
            (2.0 * s).max(1.0) as i32,
            rule,
        );
    }
}

/// The `▸▸ 5×` chip that runs while a speed ramp plays, so compressed time
/// reads as deliberate rather than as a glitch.
#[allow(clippy::too_many_arguments)]
pub fn draw_rate_badge(
    raster: &mut Rasterizer,
    canvas: &mut Pixmap,
    rate: f64,
    l: &Layout,
    top_right_taken: bool,
    style: &OverlayStyle,
    font_size: f32,
    // 0..1 entrance progress; the chip slides in from off the right edge.
    enter: f32,
    s: f32,
) {
    let enter = enter.clamp(0.0, 1.0);
    let label = if rate >= 1.0 {
        format!("▸▸ {}×", trim_num(rate))
    } else {
        format!("◂ {}×", trim_num(rate))
    };
    let size = (font_size * 0.8 * s).max(9.0);
    let m = raster.fonts.cell_metrics(size, 1.0);
    let pad_x = 9.0 * s;
    let pad_y = 5.0 * s;
    let w = label.chars().count() as f32 * m.cell_w + pad_x * 2.0;
    let h = m.cell_h + pad_y * 2.0;
    let margin = 14.0 * s;
    // It belongs to the ramp, so it arrives with it instead of popping.
    let a = ease_out(enter);
    let slide = (1.0 - a) * (w + margin * 2.0);
    let x = l.win_x + l.win_w - w - margin + slide;
    let y = if top_right_taken {
        l.win_y + l.win_h - h - margin
    } else {
        l.win_y + l.titlebar_h.max(0.0) + margin
    };
    if let Some(rect) = Rect::from_xywh(x, y, w, h) {
        let radius = (h / 2.0).min(style.radius * s);
        shadow_rounded(canvas, rect, radius, a, s);
        chrome::fill_rounded(canvas, rect, radius, OverlayStyle::with_alpha(style.bg, a));
        hairline(canvas, rect, radius, style, a, s);
    }
    crate::draw_text(
        raster,
        canvas,
        &label,
        x + pad_x,
        y + pad_y + m.baseline,
        size,
        Rgba { a: (255.0 * a) as u8, ..style.fg },
        Variant::Bold,
    );
}

/// `5` rather than `5.0`, `1.5` rather than `1.50`.
fn trim_num(v: f64) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}

/// A progress bar burned into the bottom of the canvas, notched at each
/// marker. Playing in a loop, this is what tells a viewer where the video
/// starts.
pub fn draw_progress(
    canvas: &mut Pixmap,
    progress: f32,
    ticks: &[f64],
    style: &OverlayStyle,
    s: f32,
) {
    let (cw, ch) = (canvas.width() as f32, canvas.height() as f32);
    let bar_h = (3.0 * s).max(2.0);
    let y = ch - bar_h;
    raster::fill_rect(
        canvas,
        0,
        y as i32,
        cw as i32,
        bar_h.ceil() as i32,
        Rgba { a: 48, ..style.fg },
    );
    let filled = (cw * progress.clamp(0.0, 1.0)).round() as i32;
    raster::fill_rect(canvas, 0, y as i32, filled, bar_h.ceil() as i32, Rgba { a: 255, ..style.accent });
    let notch_w = (2.0 * s).max(1.0);
    let notch_h = bar_h * 2.0;
    for t in ticks {
        let x = (cw * t.clamp(0.0, 1.0) as f32) - notch_w / 2.0;
        raster::fill_rect(
            canvas,
            x as i32,
            (ch - notch_h) as i32,
            notch_w.ceil() as i32,
            notch_h.ceil() as i32,
            Rgba { a: 160, ..style.fg },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_on_spaces_and_keeps_newlines() {
        assert_eq!(wrap_text("hola mundo cruel", 11), vec!["hola mundo", "cruel"]);
        assert_eq!(wrap_text("a\nb", 20), vec!["a", "b"]);
        assert_eq!(wrap_text("", 10), vec![""]);
    }

    #[test]
    fn wrap_hard_splits_a_word_longer_than_the_line() {
        assert_eq!(wrap_text("abcdefgh", 3), vec!["abc", "def", "gh"]);
    }

    #[test]
    fn auto_side_picks_the_roomiest_direction() {
        let canvas = (1000.0, 600.0);
        let b = (200.0, 80.0);
        // Anchor near the top-left: the card goes down/right, never up/left.
        let side = resolve_side(NoteSide::Auto, (30.0, 20.0), canvas, b, 26.0);
        assert!(matches!(side, NoteSide::Down | NoteSide::Right), "{side:?}");
        // Anchor near the bottom-right: the opposite.
        let side = resolve_side(NoteSide::Auto, (970.0, 580.0), canvas, b, 26.0);
        assert!(matches!(side, NoteSide::Up | NoteSide::Left), "{side:?}");
        // An explicit side is never second-guessed.
        assert_eq!(resolve_side(NoteSide::Left, (30.0, 20.0), canvas, b, 26.0), NoteSide::Left);
    }

    #[test]
    fn the_bubble_tail_is_a_flush_stub_not_a_spike() {
        let rect = Rect::from_xywh(100.0, 100.0, 200.0, 60.0).unwrap();
        // Anchor well below the bubble: the tail leaves the bottom edge.
        let anchor = (180.0, 400.0);
        let t = tail_tip(rect, anchor, 10.0, 8.0);
        // Base flush on the edge, tip just past it — never reaching the
        // anchor 240px away, which is what read as a spike.
        assert_eq!(t[0].1, rect.bottom());
        assert_eq!(t[1].1, rect.bottom());
        assert!((t[2].1 - rect.bottom()) > 0.0 && (t[2].1 - rect.bottom()) <= 10.0, "{t:?}");
        assert!(t[2].1 < anchor.1, "tail reached the anchor: {t:?}");
        // Symmetric base, clear of the rounded corners.
        assert!(t[0].0 >= rect.left() + 8.0 && t[1].0 <= rect.right() - 8.0, "{t:?}");
        assert!((t[1].0 - t[0].0) > 0.0);
    }

    #[test]
    fn the_tail_picks_the_edge_facing_the_anchor() {
        let rect = Rect::from_xywh(100.0, 100.0, 200.0, 60.0).unwrap();
        let above = tail_tip(rect, (180.0, 10.0), 10.0, 8.0);
        assert!(above[2].1 < rect.top(), "should exit the top edge: {above:?}");
        let left = tail_tip(rect, (10.0, 130.0), 10.0, 8.0);
        assert!(left[2].0 < rect.left(), "should exit the left edge: {left:?}");
        let right = tail_tip(rect, (500.0, 130.0), 10.0, 8.0);
        assert!(right[2].0 > rect.right(), "should exit the right edge: {right:?}");
    }

    #[test]
    fn trim_num_drops_pointless_decimals() {
        assert_eq!(trim_num(5.0), "5");
        assert_eq!(trim_num(1.5), "1.5");
        assert_eq!(trim_num(0.5), "0.5");
    }
}
