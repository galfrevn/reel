//! Typing reconstruction: make batched echoes look like real typing.
//!
//! TUIs repaint on their own tick, so fast keystrokes often land on screen
//! in clumps ("hel" at once) or only inside a later full repaint. The
//! recording knows better: the `.reelmeta` sidecar has every key with its
//! true timestamp. This pass rebuilds the letter-by-letter reveal the user
//! actually performed, inserting synthetic snapshots timed by the keys —
//! so sound and pixels agree by construction.
//!
//! Both rules are deliberately conservative: they only touch snapshots
//! whose diff is exactly a typed run at the cursor, and leave everything
//! else (spinners, streams, full repaints) untouched.

use crate::{CellAttrs, Snapshot};

/// One printable keypress from the recording, in source time.
#[derive(Debug, Clone, Copy)]
pub struct KeyPress {
    pub t: f64,
    pub ch: char,
}

pub fn smooth_typing(snapshots: &mut Vec<Snapshot>, keys: &[KeyPress]) {
    if keys.is_empty() || snapshots.len() < 2 {
        return;
    }
    let mut consumed = vec![false; keys.len()];
    let mut out: Vec<Snapshot> = Vec::with_capacity(snapshots.len() + keys.len());
    out.push(snapshots[0].clone());

    for snap in snapshots.iter().skip(1) {
        let a = out.last().unwrap().clone();
        let mut b = snap.clone();
        if a.cols != b.cols || a.rows != b.rows {
            out.push(b);
            continue;
        }

        match classify(&a, &b) {
            Diff::TypedRun { row, c0, c1 } if c1 - c0 >= 2 => {
                // Rule A: one repaint echoed several keys — split it.
                let span: Vec<char> = (c0..c1)
                    .map(|c| b.cell(c, row).ch)
                    .collect();
                let times = key_times(keys, &mut consumed, &span, a.src_time, b.src_time);
                let k = (c1 - c0) as usize;
                for (j, &t) in times.iter().enumerate().take(k - 1) {
                    let mut s = b.clone();
                    let reveal_to = c0 + j as u16 + 1;
                    for c in reveal_to..c1 {
                        let idx = row as usize * s.cols as usize + c as usize;
                        s.cells[idx] = a.cells[idx];
                    }
                    s.cursor.col = reveal_to;
                    s.src_time = t;
                    out.push(s);
                }
                b.src_time = times[k - 1];
                out.push(b);
            }
            Diff::Other { rows_changed } if rows_changed >= 2 => {
                // Rule B: keys typed in this gap whose echo only shows up
                // inside a bigger repaint (e.g. submit) — synthesize the
                // echo they should have had, at their true times.
                let pending: Vec<usize> = (0..keys.len())
                    .filter(|&i| {
                        !consumed[i]
                            && keys[i].t > a.src_time
                            && keys[i].t < b.src_time - 0.05
                    })
                    .collect();
                let fits = a.cursor.col as usize + pending.len() < a.cols as usize;
                if !pending.is_empty() && fits && a.cursor.shape != crate::CursorShape::Hidden {
                    let mut s = a.clone();
                    for &i in &pending {
                        consumed[i] = true;
                        let idx =
                            s.cursor.row as usize * s.cols as usize + s.cursor.col as usize;
                        // Inherit the style already at the echo position.
                        s.cells[idx].ch = keys[i].ch;
                        s.cursor.col += 1;
                        let mut frame = s.clone();
                        frame.src_time = keys[i].t;
                        out.push(frame);
                    }
                }
                out.push(b);
            }
            _ => out.push(b),
        }
    }

    // Times must stay monotonic for everything downstream.
    for i in 1..out.len() {
        if out[i].src_time < out[i - 1].src_time {
            out[i].src_time = out[i - 1].src_time;
        }
    }
    *snapshots = out;
}

enum Diff {
    /// Exactly one row changed, only in [c0, c1), matching a cursor advance.
    TypedRun { row: u16, c0: u16, c1: u16 },
    Other { rows_changed: u16 },
}

fn classify(a: &Snapshot, b: &Snapshot) -> Diff {
    let cols = a.cols as usize;
    let mut changed_row = None;
    let mut rows_changed = 0u16;
    for row in 0..a.rows as usize {
        if a.cells[row * cols..(row + 1) * cols] != b.cells[row * cols..(row + 1) * cols] {
            rows_changed += 1;
            changed_row = Some(row as u16);
        }
    }
    let (Some(row), 1) = (changed_row, rows_changed) else {
        return Diff::Other { rows_changed };
    };
    // Typed run: cursor advanced within this row and the changes stay
    // inside [old cursor, new cursor), all printable, no wide cells.
    if b.cursor.row != row || a.cursor.row != row || b.cursor.col <= a.cursor.col {
        return Diff::Other { rows_changed };
    }
    let (c0, c1) = (a.cursor.col, b.cursor.col);
    let base = row as usize * cols;
    for c in 0..cols {
        let differs = a.cells[base + c] != b.cells[base + c];
        let inside = (c0 as usize..c1 as usize).contains(&c);
        if differs && !inside {
            return Diff::Other { rows_changed };
        }
        if inside {
            let cell = b.cells[base + c];
            if cell.attrs.intersects(CellAttrs::WIDE | CellAttrs::WIDE_SPACER)
                || cell.ch.is_control()
            {
                return Diff::Other { rows_changed };
            }
        }
    }
    Diff::TypedRun { row, c0, c1 }
}

/// Times for each revealed char: the matching keys' timestamps when the
/// window contains them, else an even spread across (ta, tb].
fn key_times(
    keys: &[KeyPress],
    consumed: &mut [bool],
    span: &[char],
    ta: f64,
    tb: f64,
) -> Vec<f64> {
    let window: Vec<usize> = (0..keys.len())
        .filter(|&i| !consumed[i] && keys[i].t > ta && keys[i].t <= tb + 0.05)
        .collect();
    // Match the span against the tail of the window (earlier unconsumed keys
    // may belong to an app that echoed them elsewhere).
    if window.len() >= span.len() {
        let tail = &window[window.len() - span.len()..];
        if tail.iter().zip(span).all(|(&i, &ch)| keys[i].ch == ch) {
            let mut times = Vec::with_capacity(span.len());
            let mut prev = ta;
            for &i in tail {
                consumed[i] = true;
                let t = keys[i].t.clamp(prev + 1e-4, tb);
                times.push(t);
                prev = t;
            }
            return times;
        }
    }
    // Fallback: spread evenly.
    let k = span.len();
    (1..=k)
        .map(|j| ta + (tb - ta) * j as f64 / k as f64)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cell, Cursor, CursorShape};

    fn snap(t: f64, text_rows: &[&str], cursor: (u16, u16)) -> Snapshot {
        let cols = 20u16;
        let rows = text_rows.len() as u16;
        let mut cells = vec![Cell::default(); cols as usize * rows as usize];
        for (r, line) in text_rows.iter().enumerate() {
            for (c, ch) in line.chars().enumerate() {
                cells[r * cols as usize + c].ch = ch;
            }
        }
        Snapshot {
            src_time: t,
            cols,
            rows,
            cells,
            cursor: Cursor { col: cursor.0, row: cursor.1, shape: CursorShape::Block },
            palette_overrides: vec![],
            default_overrides: [None; 3],
            images: vec![],
        }
    }

    fn key(t: f64, ch: char) -> KeyPress {
        KeyPress { t, ch }
    }

    fn row_text(s: &Snapshot, row: u16) -> String {
        (0..s.cols).map(|c| s.cell(c, row).ch).collect::<String>().trim_end().to_string()
    }

    #[test]
    fn batched_echo_splits_into_per_key_frames() {
        let mut snaps = vec![
            snap(1.0, &["> h", ""], (3, 0)),
            snap(2.0, &["> hello", ""], (7, 0)), // "ello" painted at once
        ];
        let keys = [key(1.2, 'e'), key(1.4, 'l'), key(1.6, 'l'), key(1.8, 'o')];
        smooth_typing(&mut snaps, &keys);
        assert_eq!(snaps.len(), 5);
        let texts: Vec<String> = snaps.iter().map(|s| row_text(s, 0)).collect();
        assert_eq!(texts, ["> h", "> he", "> hel", "> hell", "> hello"]);
        let times: Vec<f64> = snaps.iter().map(|s| s.src_time).collect();
        assert_eq!(times, [1.0, 1.2, 1.4, 1.6, 1.8]);
        assert_eq!(snaps[2].cursor.col, 5);
    }

    #[test]
    fn split_without_matching_keys_spreads_evenly() {
        let mut snaps = vec![
            snap(1.0, &["> a"], (3, 0)),
            snap(2.0, &["> abc"], (5, 0)),
        ];
        smooth_typing(&mut snaps, &[key(0.1, 'x')]);
        assert_eq!(snaps.len(), 3);
        assert!((snaps[1].src_time - 1.5).abs() < 1e-9);
        assert!((snaps[2].src_time - 2.0).abs() < 1e-9);
    }

    #[test]
    fn unechoed_tail_is_synthesized_before_a_repaint() {
        // "hell" visible; 'o' typed but the next snapshot is a full submit
        // repaint where the input box is gone.
        let mut snaps = vec![
            snap(1.0, &["> hell", "", ""], (6, 0)),
            snap(3.0, &["", "you said hello", "ok"], (0, 2)),
        ];
        let keys = [key(1.3, 'o')];
        smooth_typing(&mut snaps, &keys);
        assert_eq!(snaps.len(), 3);
        assert_eq!(row_text(&snaps[1], 0), "> hello");
        assert!((snaps[1].src_time - 1.3).abs() < 1e-9);
        assert_eq!(snaps[1].cursor.col, 7);
    }

    #[test]
    fn spinners_and_streams_are_untouched() {
        // Multi-row change with no pending keys: passes through unchanged.
        let mut snaps = vec![
            snap(1.0, &["working ⠋", "line"], (0, 0)),
            snap(1.1, &["working ⠙", "line two"], (0, 0)),
        ];
        smooth_typing(&mut snaps, &[key(0.5, 'x')]);
        assert_eq!(snaps.len(), 2);
        assert_eq!(row_text(&snaps[1], 0), "working ⠙");
    }

    #[test]
    fn single_char_echo_needs_no_help() {
        let mut snaps = vec![
            snap(1.0, &["> a"], (3, 0)),
            snap(1.2, &["> ab"], (4, 0)),
        ];
        let keys = [key(1.15, 'b')];
        smooth_typing(&mut snaps, &keys);
        assert_eq!(snaps.len(), 2);
    }

    #[test]
    fn times_stay_monotonic() {
        let mut snaps = vec![
            snap(1.0, &["> h"], (3, 0)),
            snap(1.4, &["> hey"], (5, 0)),
            snap(1.5, &["> hey!"], (6, 0)),
        ];
        // Keys with awkward times near the boundary.
        let keys = [key(1.38, 'e'), key(1.39, 'y'), key(1.45, '!')];
        smooth_typing(&mut snaps, &keys);
        for w in snaps.windows(2) {
            assert!(w[1].src_time >= w[0].src_time);
        }
    }
}
