//! Terminal emulation: replays a cast's output bytes through
//! `alacritty_terminal` and produces a list of [`Snapshot`]s.
//!
//! Two invariants live here:
//!
//! 1. **Colors stay abstract.** Cells carry [`ColorRef`]s (named/indexed
//!    references), never resolved pixels. Resolving through a theme happens at
//!    raster time, which is what makes re-theming a render-only operation.
//! 2. **No torn frames.** Events closer together than the coalescing window
//!    are folded into one snapshot, and synchronized-output (`?2026`) blocks
//!    are atomic because the parser buffers them until ESU.

pub mod redact;
pub mod typing;
pub use typing::{smooth_typing, KeyPress};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Point;
use alacritty_terminal::term::cell::Flags as TermFlags;
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::vte::ansi::{
    Color as AnsiColor, CursorShape as AnsiCursorShape, NamedColor, Processor,
};
use reel_cast::{Cast, EventKind};

/// Events closer together than this are treated as one burst and produce a
/// single snapshot. Chosen to merge PTY write bursts without eating real
/// animation frames.
const COALESCE_WINDOW: f64 = 0.0015;

#[derive(Debug, thiserror::Error)]
pub enum TermError {
    #[error("cast declares {0}x{1} which exceeds the 1000x500 emulation limit")]
    TooLarge(u16, u16),
}

/// A color reference, resolved against a theme at raster time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorRef {
    /// Theme foreground.
    Fg,
    /// Theme background.
    Bg,
    /// Theme cursor color.
    Cursor,
    /// 256-color palette index (0-15 come from the theme, 16-255 from the
    /// standard cube/grayscale unless overridden by OSC 4).
    Indexed(u8),
    /// Direct 24-bit color from the application.
    Rgb(u8, u8, u8),
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct CellAttrs: u16 {
        const BOLD          = 1 << 0;
        const ITALIC        = 1 << 1;
        const DIM           = 1 << 2;
        const INVERSE       = 1 << 3;
        const HIDDEN        = 1 << 4;
        const STRIKEOUT     = 1 << 5;
        const UNDERLINE     = 1 << 6;
        /// First cell of a double-width character.
        const WIDE          = 1 << 7;
        /// Spacer cell following a double-width character; never drawn.
        const WIDE_SPACER   = 1 << 8;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cell {
    pub ch: char,
    pub fg: ColorRef,
    pub bg: ColorRef,
    pub attrs: CellAttrs,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { ch: ' ', fg: ColorRef::Fg, bg: ColorRef::Bg, attrs: CellAttrs::empty() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CursorShape {
    Block,
    Underline,
    Beam,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cursor {
    pub col: u16,
    pub row: u16,
    pub shape: CursorShape,
}

/// The full visible terminal state at one source-time instant.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Seconds on the source clock (the cast's clock).
    pub src_time: f64,
    pub cols: u16,
    pub rows: u16,
    /// Row-major, `cols * rows` cells.
    pub cells: Vec<Cell>,
    pub cursor: Cursor,
    /// Palette slots the application overrode via OSC 4, as (index, rgb).
    pub palette_overrides: Vec<(u8, (u8, u8, u8))>,
}

impl Snapshot {
    pub fn cell(&self, col: u16, row: u16) -> &Cell {
        &self.cells[row as usize * self.cols as usize + col as usize]
    }

    /// Plain-text dump of the grid — the `.txt` output format and the
    /// debugging tool for everything downstream.
    pub fn to_text(&self) -> String {
        let mut out = String::with_capacity((self.cols as usize + 1) * self.rows as usize);
        for row in 0..self.rows {
            let mut line = String::new();
            for col in 0..self.cols {
                let cell = self.cell(col, row);
                if cell.attrs.contains(CellAttrs::WIDE_SPACER) {
                    continue;
                }
                line.push(cell.ch);
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }

    /// Content hash used for change detection (frame dedup).
    pub fn content_hash(&self) -> u64 {
        // FNV-1a over cells + cursor; collision odds are irrelevant here
        // because a false merge just drops one duplicate-looking frame.
        let mut h: u64 = 0xcbf29ce484222325;
        let mut mix = |v: u64| {
            h ^= v;
            h = h.wrapping_mul(0x100000001b3);
        };
        for c in &self.cells {
            mix(c.ch as u64);
            mix(color_key(c.fg));
            mix(color_key(c.bg));
            mix(c.attrs.bits() as u64);
        }
        mix(self.cursor.col as u64 | (self.cursor.row as u64) << 16);
        mix(self.cursor.shape as u64);
        for (i, (r, g, b)) in &self.palette_overrides {
            mix(*i as u64 | (*r as u64) << 8 | (*g as u64) << 16 | (*b as u64) << 24);
        }
        h
    }
}

fn color_key(c: ColorRef) -> u64 {
    match c {
        ColorRef::Fg => 1 << 32,
        ColorRef::Bg => 2 << 32,
        ColorRef::Cursor => 3 << 32,
        ColorRef::Indexed(i) => (4 << 32) | i as u64,
        ColorRef::Rgb(r, g, b) => (5 << 32) | (r as u64) << 16 | (g as u64) << 8 | b as u64,
    }
}

struct NullListener;
impl EventListener for NullListener {
    fn send_event(&self, _event: Event) {}
}

struct GridSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Replays a cast and returns one snapshot per *visible change*, in source
/// time. The first snapshot is at t=0 (the empty grid), so the timeline can
/// always sample a state before the first output.
pub fn replay(cast: &Cast) -> Result<Vec<Snapshot>, TermError> {
    let (cols, rows) = (cast.cols(), cast.rows());
    if cols > 1000 || rows > 500 || cols == 0 || rows == 0 {
        return Err(TermError::TooLarge(cols, rows));
    }

    let config = TermConfig { scrolling_history: 0, ..Default::default() };
    let mut term = Term::new(
        config,
        &GridSize { cols: cols as usize, rows: rows as usize },
        NullListener,
    );
    let mut parser: Processor = Processor::new();

    let mut snapshots: Vec<Snapshot> = vec![take_snapshot(&term, 0.0)];
    let mut last_hash = snapshots[0].content_hash();

    let outputs: Vec<_> = cast
        .events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::Output | EventKind::Resize))
        .collect();

    for (i, ev) in outputs.iter().enumerate() {
        match ev.kind {
            EventKind::Output => parser.advance(&mut term, ev.data.as_bytes()),
            EventKind::Resize => {
                if let Some((c, r)) = parse_resize(&ev.data) {
                    term.resize(GridSize { cols: c as usize, rows: r as usize });
                }
            }
            _ => unreachable!(),
        }

        // Mid-burst: the next event lands inside the coalescing window, so
        // fold this state into the burst's final snapshot.
        if let Some(next) = outputs.get(i + 1) {
            if next.time - ev.time < COALESCE_WINDOW {
                continue;
            }
        }
        // Inside a synchronized-output block the parser is buffering; the
        // grid hasn't changed, so skip (dedup would drop it anyway).
        if parser.sync_bytes_count() > 0 {
            continue;
        }

        let snap = take_snapshot(&term, ev.time);
        let hash = snap.content_hash();
        if hash != last_hash {
            last_hash = hash;
            snapshots.push(snap);
        }
    }

    // A recording can end mid-synchronized-update; flush so the final state
    // is captured.
    if parser.sync_bytes_count() > 0 {
        parser.stop_sync(&mut term);
        let t = outputs.last().map(|e| e.time).unwrap_or(0.0);
        let snap = take_snapshot(&term, t);
        if snap.content_hash() != last_hash {
            snapshots.push(snap);
        }
    }

    Ok(snapshots)
}

fn parse_resize(data: &str) -> Option<(u16, u16)> {
    let (c, r) = data.split_once('x')?;
    Some((c.trim().parse().ok()?, r.trim().parse().ok()?))
}

fn take_snapshot<L: EventListener>(term: &Term<L>, src_time: f64) -> Snapshot {
    let grid = term.grid();
    let cols = grid.columns() as u16;
    let rows = grid.screen_lines() as u16;
    let mut cells = vec![Cell::default(); cols as usize * rows as usize];

    for indexed in grid.display_iter() {
        let Point { line, column } = indexed.point;
        if line.0 < 0 {
            continue;
        }
        let (row, col) = (line.0 as usize, column.0);
        if row >= rows as usize || col >= cols as usize {
            continue;
        }
        let src = &indexed.cell;
        cells[row * cols as usize + col] = Cell {
            ch: src.c,
            fg: convert_color(src.fg),
            bg: convert_color(src.bg),
            attrs: convert_flags(src.flags),
        };
    }

    let cursor = cursor_state(term);

    let mut palette_overrides = Vec::new();
    let colors = term.colors();
    for i in 0..=255usize {
        if let Some(rgb) = colors[i] {
            palette_overrides.push((i as u8, (rgb.r, rgb.g, rgb.b)));
        }
    }

    Snapshot { src_time, cols, rows, cells, cursor, palette_overrides }
}

fn cursor_state<L: EventListener>(term: &Term<L>) -> Cursor {
    let point = term.grid().cursor.point;
    let visible = term.mode().contains(TermMode::SHOW_CURSOR);
    let shape = if !visible {
        CursorShape::Hidden
    } else {
        match term.cursor_style().shape {
            AnsiCursorShape::Block => CursorShape::Block,
            AnsiCursorShape::Underline => CursorShape::Underline,
            AnsiCursorShape::Beam => CursorShape::Beam,
            AnsiCursorShape::HollowBlock => CursorShape::Block,
            AnsiCursorShape::Hidden => CursorShape::Hidden,
        }
    };
    Cursor {
        col: point.column.0.min(u16::MAX as usize) as u16,
        row: point.line.0.max(0).min(u16::MAX as i32) as u16,
        shape,
    }
}

fn convert_color(c: AnsiColor) -> ColorRef {
    match c {
        AnsiColor::Spec(rgb) => ColorRef::Rgb(rgb.r, rgb.g, rgb.b),
        AnsiColor::Indexed(i) => ColorRef::Indexed(i),
        AnsiColor::Named(named) => match named {
            NamedColor::Foreground | NamedColor::BrightForeground => ColorRef::Fg,
            NamedColor::Background => ColorRef::Bg,
            NamedColor::Cursor => ColorRef::Cursor,
            // Dim variants keep their base index; the DIM attr darkens at
            // raster time.
            NamedColor::DimBlack => ColorRef::Indexed(0),
            NamedColor::DimRed => ColorRef::Indexed(1),
            NamedColor::DimGreen => ColorRef::Indexed(2),
            NamedColor::DimYellow => ColorRef::Indexed(3),
            NamedColor::DimBlue => ColorRef::Indexed(4),
            NamedColor::DimMagenta => ColorRef::Indexed(5),
            NamedColor::DimCyan => ColorRef::Indexed(6),
            NamedColor::DimWhite => ColorRef::Indexed(7),
            NamedColor::DimForeground => ColorRef::Fg,
            other => {
                let idx = other as usize;
                if idx < 16 {
                    ColorRef::Indexed(idx as u8)
                } else {
                    ColorRef::Fg
                }
            }
        },
    }
}

fn convert_flags(f: TermFlags) -> CellAttrs {
    let mut a = CellAttrs::empty();
    let map = [
        (TermFlags::BOLD, CellAttrs::BOLD),
        (TermFlags::ITALIC, CellAttrs::ITALIC),
        (TermFlags::DIM, CellAttrs::DIM),
        (TermFlags::INVERSE, CellAttrs::INVERSE),
        (TermFlags::HIDDEN, CellAttrs::HIDDEN),
        (TermFlags::STRIKEOUT, CellAttrs::STRIKEOUT),
        (TermFlags::UNDERLINE, CellAttrs::UNDERLINE),
        (TermFlags::DOUBLE_UNDERLINE, CellAttrs::UNDERLINE),
        (TermFlags::UNDERCURL, CellAttrs::UNDERLINE),
        (TermFlags::DOTTED_UNDERLINE, CellAttrs::UNDERLINE),
        (TermFlags::DASHED_UNDERLINE, CellAttrs::UNDERLINE),
        (TermFlags::WIDE_CHAR, CellAttrs::WIDE),
        (TermFlags::WIDE_CHAR_SPACER, CellAttrs::WIDE_SPACER),
        (TermFlags::LEADING_WIDE_CHAR_SPACER, CellAttrs::WIDE_SPACER),
    ];
    for (from, to) in map {
        if f.contains(from) {
            a |= to;
        }
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cast(body: &str) -> Cast {
        let text = format!("{}\n{}", r#"{"version": 2, "width": 20, "height": 4}"#, body);
        Cast::parse(&text).unwrap()
    }

    #[test]
    fn plain_text_lands_on_grid() {
        let c = cast(r#"[0.1, "o", "hello"]"#);
        let snaps = replay(&c).unwrap();
        assert_eq!(snaps.len(), 2); // empty grid + one change
        assert_eq!(snaps[1].to_text().lines().next().unwrap(), "hello");
        assert_eq!(snaps[1].cursor.col, 5);
    }

    #[test]
    fn colors_stay_abstract() {
        let c = cast(r#"[0.1, "o", "\u001b[31mred\u001b[0m \u001b[38;2;1;2;3mrgb"]"#);
        let snaps = replay(&c).unwrap();
        let s = &snaps[1];
        assert_eq!(s.cell(0, 0).fg, ColorRef::Indexed(1));
        assert_eq!(s.cell(3, 0).fg, ColorRef::Fg);
        assert_eq!(s.cell(4, 0).fg, ColorRef::Rgb(1, 2, 3));
    }

    #[test]
    fn identical_output_dedupes() {
        let c = cast("[0.1, \"o\", \"x\"]\n[0.5, \"o\", \"\"]\n[0.9, \"o\", \"\\u001b[s\\u001b[u\"]");
        let snaps = replay(&c).unwrap();
        assert_eq!(snaps.len(), 2);
    }

    #[test]
    fn burst_coalesces_to_one_snapshot() {
        let c = cast("[0.1, \"o\", \"a\"]\n[0.1005, \"o\", \"b\"]\n[0.101, \"o\", \"c\"]");
        let snaps = replay(&c).unwrap();
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[1].to_text().lines().next().unwrap(), "abc");
    }

    #[test]
    fn alt_screen_and_clear() {
        let c = cast(
            "[0.1, \"o\", \"before\"]\n[0.5, \"o\", \"\\u001b[?1049h\\u001b[2J\\u001b[H\\u001b[1;1Halt!\"]",
        );
        let snaps = replay(&c).unwrap();
        let last = snaps.last().unwrap();
        assert_eq!(last.to_text().lines().next().unwrap(), "alt!");
        assert!(!last.to_text().contains("before"));
    }

    #[test]
    fn wide_chars_take_two_cells() {
        let c = cast(r#"[0.1, "o", "你a"]"#);
        let snaps = replay(&c).unwrap();
        let s = &snaps[1];
        assert!(s.cell(0, 0).attrs.contains(CellAttrs::WIDE));
        assert!(s.cell(1, 0).attrs.contains(CellAttrs::WIDE_SPACER));
        assert_eq!(s.cell(2, 0).ch, 'a');
        assert_eq!(s.to_text().lines().next().unwrap(), "你a");
    }

    #[test]
    fn synchronized_update_is_atomic() {
        // BSU, partial draw, more draw, ESU — only the post-ESU state may
        // appear, never the partial one.
        let c = cast(
            "[0.1, \"o\", \"\\u001b[?2026htorn\"]\n[0.5, \"o\", \" whole\\u001b[?2026l\"]",
        );
        let snaps = replay(&c).unwrap();
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[1].to_text().lines().next().unwrap(), "torn whole");
    }

    #[test]
    fn cursor_hidden_is_reported() {
        let c = cast(r#"[0.1, "o", "\u001b[?25lx"]"#);
        let snaps = replay(&c).unwrap();
        assert_eq!(snaps[1].cursor.shape, CursorShape::Hidden);
    }
}
