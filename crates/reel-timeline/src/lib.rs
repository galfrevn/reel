//! The timeline model.
//!
//! Every timestamp in a `.reel` file refers to the **source clock** — the
//! cast's own time axis. Ops are declarative and order-independent: they are
//! compiled here into a single piecewise mapping from output time to source
//! time. Conflicts (overlapping speed regions) are compile errors, not
//! silently order-dependent behavior.
//!
//! Convention: *anchors and ranges* (`A..B`, `from A to B`, `at T`) are source
//! time; *bare durations* (`for D`, `hold 2s`, `freeze last 1s`) are output
//! time — they describe what the viewer experiences.

const EPS: f64 = 1e-9;

#[derive(Debug, thiserror::Error)]
pub enum TimelineError {
    #[error("speed regions overlap: {0:.3}s..{1:.3}s and {2:.3}s..{3:.3}s")]
    SpeedOverlap(f64, f64, f64, f64),
    #[error("speed factor must be > 0, got {0}")]
    BadSpeed(f64),
    #[error("range is empty or reversed: {0:.3}s..{1:.3}s")]
    EmptyRange(f64, f64),
    #[error("trim range {0:.3}s..{1:.3}s is outside the recording (duration {2:.3}s)")]
    TrimOutside(f64, f64, f64),
    #[error("nothing left after edits (trim/cut removed the whole recording)")]
    NothingLeft,
}

/// Time-shaping ops, all in source time except durations (see module docs).
#[derive(Debug, Clone, Default)]
pub struct EditOps {
    /// At most one; keeps only this source range.
    pub trim: Option<(f64, f64)>,
    /// Removed source ranges. Overlaps are merged.
    pub cuts: Vec<(f64, f64)>,
    /// (factor, src_start, src_end). Factor 5.0 = 5x faster.
    pub speeds: Vec<(f64, f64, f64)>,
    /// (output duration, src_time): insert a still pause.
    pub holds: Vec<(f64, f64)>,
    /// (src_time, output duration, text): a title card. Inserts a still like
    /// a hold does, and the still's own output window is what the card is
    /// drawn over — see [`Timeline::cards`].
    pub cards: Vec<(f64, f64, String)>,
    /// Hold the final frame for this many output seconds before looping.
    pub freeze_last: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    /// Plays source range [src_start, src_end) at `rate` (2.0 = 2x fast).
    Play { out_start: f64, src_start: f64, src_end: f64, rate: f64 },
    /// Holds the source state at `src_at` for `dur` output seconds.
    Still { out_start: f64, src_at: f64, dur: f64 },
}

impl Segment {
    pub fn out_start(&self) -> f64 {
        match *self {
            Segment::Play { out_start, .. } | Segment::Still { out_start, .. } => out_start,
        }
    }

    pub fn out_dur(&self) -> f64 {
        match *self {
            Segment::Play { src_start, src_end, rate, .. } => (src_end - src_start) / rate,
            Segment::Still { dur, .. } => dur,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Timeline {
    segments: Vec<Segment>,
    out_duration: f64,
    /// Kept source intervals (post trim/cut), for anchor snapping.
    kept: Vec<(f64, f64)>,
    /// (out_start, out_end, text) per title card, in output order.
    cards: Vec<(f64, f64, String)>,
}

impl Timeline {
    /// Compiles declarative ops against a recording of `src_duration` seconds.
    /// Returns the timeline plus human-readable warnings (clamped ranges,
    /// anchors that fell inside cuts, ...).
    pub fn compile(ops: &EditOps, src_duration: f64) -> Result<(Timeline, Vec<String>), TimelineError> {
        let mut warnings = Vec::new();

        // 1. Kept range from trim.
        let (t0, t1) = match ops.trim {
            Some((a, b)) => {
                if b - a < EPS {
                    return Err(TimelineError::EmptyRange(a, b));
                }
                if a >= src_duration {
                    return Err(TimelineError::TrimOutside(a, b, src_duration));
                }
                if b > src_duration + EPS {
                    warnings.push(format!(
                        "trim end {:.3}s clamped to recording end {:.3}s",
                        b, src_duration
                    ));
                }
                (a.max(0.0), b.min(src_duration))
            }
            None => (0.0, src_duration),
        };

        // 2. Subtract cuts (merged, clamped to the trimmed range).
        let mut cuts: Vec<(f64, f64)> = Vec::new();
        for &(a, b) in &ops.cuts {
            if b - a < EPS {
                return Err(TimelineError::EmptyRange(a, b));
            }
            let (a, b) = (a.max(t0), b.min(t1));
            if b - a < EPS {
                warnings.push(format!("cut {:.3}s..{:.3}s lies outside the kept range; ignored", a, b));
                continue;
            }
            cuts.push((a, b));
        }
        cuts.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for c in cuts {
            match merged.last_mut() {
                Some(last) if c.0 <= last.1 + EPS => last.1 = last.1.max(c.1),
                _ => merged.push(c),
            }
        }

        let mut kept: Vec<(f64, f64)> = Vec::new();
        let mut pos = t0;
        for (a, b) in &merged {
            if *a - pos > EPS {
                kept.push((pos, *a));
            }
            pos = *b;
        }
        if t1 - pos > EPS {
            kept.push((pos, t1));
        }
        if kept.is_empty() {
            return Err(TimelineError::NothingLeft);
        }

        // 3. Validate speed regions (non-overlapping among themselves).
        let mut speeds = ops.speeds.clone();
        for &(n, a, b) in &speeds {
            if n <= 0.0 {
                return Err(TimelineError::BadSpeed(n));
            }
            if b - a < EPS {
                return Err(TimelineError::EmptyRange(a, b));
            }
        }
        speeds.sort_by(|x, y| x.1.total_cmp(&y.1));
        for w in speeds.windows(2) {
            if w[1].1 < w[0].2 - EPS {
                return Err(TimelineError::SpeedOverlap(w[0].1, w[0].2, w[1].1, w[1].2));
            }
        }

        // 4. Split kept intervals at speed boundaries → Play segments.
        let mut plays: Vec<(f64, f64, f64)> = Vec::new(); // (src_start, src_end, rate)
        for &(ka, kb) in &kept {
            let mut cursor = ka;
            for &(n, sa, sb) in &speeds {
                let (oa, ob) = (sa.max(cursor), sb.min(kb));
                if ob - oa < EPS || oa >= kb {
                    continue;
                }
                if oa - cursor > EPS {
                    plays.push((cursor, oa, 1.0));
                }
                plays.push((oa, ob, n));
                cursor = ob;
            }
            if kb - cursor > EPS {
                plays.push((cursor, kb, 1.0));
            }
        }

        // 5. Interleave stills (holds and cards) and assemble output offsets.
        // Cards ride the same queue as holds so the two stay deterministic
        // against each other; a card additionally records the output window
        // of the still it produced, which is what gets drawn over.
        let mut stills: Vec<Still> = ops
            .holds
            .iter()
            .map(|&(dur, at)| Still { at, dur, text: None })
            .collect();
        stills.extend(
            ops.cards
                .iter()
                .map(|(at, dur, text)| Still { at: *at, dur: *dur, text: Some(text.clone()) }),
        );
        stills.sort_by(|x, y| x.at.total_cmp(&y.at));
        // Anchors before the first or after the last kept instant mean "at
        // the start" / "at the end" — `card "…" at end` is the documented
        // outro form. Only an anchor swallowed by an *interior* cut is a
        // surprise worth warning about.
        let (first_kept, last_kept) = (kept[0].0, kept.last().unwrap().1);
        for s in &stills {
            let interior = s.at > first_kept + EPS && s.at < last_kept - EPS;
            if interior && !kept.iter().any(|&(a, b)| s.at >= a - EPS && s.at <= b + EPS) {
                let what = if s.text.is_some() { "card" } else { "hold" };
                warnings.push(format!(
                    "{what} at {:.3}s falls inside a cut; it will snap to the seam",
                    s.at
                ));
            }
        }

        let mut segments = Vec::new();
        let mut cards: Vec<(f64, f64, String)> = Vec::new();
        let mut out = 0.0f64;
        let mut still_iter = stills.into_iter().peekable();
        for (sa, sb, rate) in plays {
            // Stills anchored before/at this play's start fire first.
            while let Some(s) = still_iter.peek() {
                if s.at <= sa + EPS {
                    let s = still_iter.next().unwrap();
                    push_still(&mut segments, &mut cards, &mut out, sa, s.dur, s.text);
                } else {
                    break;
                }
            }
            // Stills inside this play split it.
            let mut cursor = sa;
            while let Some(s) = still_iter.peek() {
                if s.at < sb - EPS {
                    let s = still_iter.next().unwrap();
                    if s.at - cursor > EPS {
                        segments.push(Segment::Play { out_start: out, src_start: cursor, src_end: s.at, rate });
                        out += (s.at - cursor) / rate;
                    }
                    cursor = s.at;
                    push_still(&mut segments, &mut cards, &mut out, s.at, s.dur, s.text);
                } else {
                    break;
                }
            }
            if sb - cursor > EPS {
                segments.push(Segment::Play { out_start: out, src_start: cursor, src_end: sb, rate });
                out += (sb - cursor) / rate;
            }
        }
        // Trailing stills (anchored at/after the last kept time). An outro
        // card lands here, before `freeze last` appends its own still.
        let last_src = kept.last().unwrap().1;
        for s in still_iter {
            push_still(&mut segments, &mut cards, &mut out, last_src, s.dur, s.text);
        }

        if let Some(dur) = ops.freeze_last {
            if dur > EPS {
                segments.push(Segment::Still { out_start: out, src_at: last_src, dur });
                out += dur;
            }
        }

        Ok((Timeline { segments, out_duration: out, kept, cards }, warnings))
    }

    pub fn out_duration(&self) -> f64 {
        self.out_duration
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Output time → source time. Clamps outside [0, out_duration].
    pub fn sample(&self, out_t: f64) -> f64 {
        let out_t = out_t.clamp(0.0, self.out_duration);
        let idx = match self
            .segments
            .binary_search_by(|s| s.out_start().total_cmp(&out_t))
        {
            Ok(i) => i,
            Err(0) => 0,
            Err(i) => i - 1,
        };
        match self.segments[idx] {
            Segment::Play { out_start, src_start, src_end, rate } => {
                (src_start + (out_t - out_start) * rate).min(src_end)
            }
            Segment::Still { src_at, .. } => src_at,
        }
    }

    /// Source time → output time, or `None` if the instant was cut/trimmmed
    /// away. Anchors for captions/zooms/sounds go through here.
    pub fn project(&self, src_t: f64) -> Option<f64> {
        for seg in &self.segments {
            if let Segment::Play { out_start, src_start, src_end, rate } = *seg {
                if src_t >= src_start - EPS && src_t <= src_end + EPS {
                    let clamped = src_t.clamp(src_start, src_end);
                    return Some(out_start + (clamped - src_start) / rate);
                }
            }
        }
        None
    }

    /// Like [`project`](Self::project), but an anchor that fell into a cut
    /// snaps forward to the seam (or backward if it was after the last kept
    /// instant). Never fails on a non-empty timeline.
    pub fn project_snapped(&self, src_t: f64) -> f64 {
        if let Some(t) = self.project(src_t) {
            return t;
        }
        // Snap to the start of the first kept interval after src_t.
        for &(a, _) in &self.kept {
            if a >= src_t {
                if let Some(t) = self.project(a) {
                    return t;
                }
            }
        }
        // Everything kept is before src_t: snap to the very end.
        self.project(self.kept.last().unwrap().1)
            .unwrap_or(self.out_duration)
    }

    pub fn kept_ranges(&self) -> &[(f64, f64)] {
        &self.kept
    }

    /// Title cards as (out_start, out_end, text), in output order.
    ///
    /// These come from the timeline rather than from projecting an anchor:
    /// [`project`](Self::project) only maps `Play` segments, so the output
    /// window a card *created* is not recoverable after the fact.
    pub fn cards(&self) -> &[(f64, f64, String)] {
        &self.cards
    }
}

/// One pending still during compilation: a `hold` (no text) or a `card`.
struct Still {
    at: f64,
    dur: f64,
    text: Option<String>,
}

fn push_still(
    segments: &mut Vec<Segment>,
    cards: &mut Vec<(f64, f64, String)>,
    out: &mut f64,
    src_at: f64,
    dur: f64,
    text: Option<String>,
) {
    segments.push(Segment::Still { out_start: *out, src_at, dur });
    if let Some(text) = text {
        cards.push((*out, *out + dur, text));
    }
    *out += dur;
}

/// Where a caption sits on the canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaptionPos {
    #[default]
    Bottom,
    Top,
    Center,
}

/// How a `highlight` marks its rect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HighlightStyle {
    /// Dim everything outside the rect.
    #[default]
    Spotlight,
    /// Stroke a box around the rect, leaving the rest untouched.
    Box,
    /// Underline the rect's last row.
    Underline,
}

/// How a `note` connects its card to the cell it points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoteStyle {
    /// Rectangular card with a leader line and a dot on the anchor.
    #[default]
    Card,
    /// Speech bubble with a tail pointing at the anchor.
    Bubble,
}

/// Which way a `note` sits from its anchor (`Auto` picks the roomiest side).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoteSide {
    #[default]
    Auto,
    Up,
    Down,
    Left,
    Right,
}

/// Visual overlay ops. Anchors/ranges in source time, durations in output time.
#[derive(Debug, Clone)]
pub enum VisualOp {
    Zoom { factor: f64, center: (u16, u16), range: Option<(f64, f64)> },
    Pan { to: (u16, u16), range: (f64, f64) },
    Caption { text: String, at: f64, dur: f64, pos: CaptionPos },
    Highlight { rect: (u16, u16, u16, u16), at: f64, dur: f64, style: HighlightStyle },
    /// A callout anchored to a cell, pointing at it from `side`.
    Note {
        text: String,
        anchor: (u16, u16),
        at: f64,
        dur: f64,
        style: NoteStyle,
        side: NoteSide,
    },
    /// One keystroke-overlay chip (a key label from the recorded input).
    Key { label: String, at: f64 },
}

/// Audio ops — parsed and carried through since Phase 1, mixed in Phase 1.5.
#[derive(Debug, Clone)]
pub enum AudioOp {
    Sound { name: String, at: f64 },
    Mute { range: (f64, f64) },
    Volume { level: f64, range: (f64, f64) },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(ops: EditOps, dur: f64) -> Timeline {
        Timeline::compile(&ops, dur).unwrap().0
    }

    #[test]
    fn identity_when_no_ops() {
        let tl = compile(EditOps::default(), 10.0);
        assert!((tl.out_duration() - 10.0).abs() < EPS);
        assert!((tl.sample(3.5) - 3.5).abs() < EPS);
        assert!((tl.project(7.0).unwrap() - 7.0).abs() < EPS);
    }

    #[test]
    fn trim_shifts_output_origin() {
        let tl = compile(EditOps { trim: Some((2.0, 8.0)), ..Default::default() }, 10.0);
        assert!((tl.out_duration() - 6.0).abs() < EPS);
        assert!((tl.sample(0.0) - 2.0).abs() < EPS);
        assert!((tl.project(5.0).unwrap() - 3.0).abs() < EPS);
        assert!(tl.project(1.0).is_none());
        assert!(tl.project(9.0).is_none());
    }

    #[test]
    fn cut_joins_the_seam() {
        let tl = compile(EditOps { cuts: vec![(3.0, 5.0)], ..Default::default() }, 10.0);
        assert!((tl.out_duration() - 8.0).abs() < EPS);
        assert!((tl.sample(2.9) - 2.9).abs() < EPS);
        assert!((tl.sample(3.1) - 5.1).abs() < EPS);
        assert!(tl.project(4.0).is_none());
        // Snapped anchor lands at the seam.
        assert!((tl.project_snapped(4.0) - 3.0).abs() < EPS);
    }

    #[test]
    fn overlapping_cuts_merge() {
        let tl = compile(EditOps { cuts: vec![(1.0, 3.0), (2.0, 4.0)], ..Default::default() }, 10.0);
        assert!((tl.out_duration() - 7.0).abs() < EPS);
    }

    #[test]
    fn speed_compresses_region() {
        let tl = compile(EditOps { speeds: vec![(5.0, 2.0, 7.0)], ..Default::default() }, 10.0);
        // 2s normal + 5s/5 + 3s normal = 6s.
        assert!((tl.out_duration() - 6.0).abs() < EPS);
        // Inside the fast region: out 2.5 → src 2 + 0.5*5 = 4.5.
        assert!((tl.sample(2.5) - 4.5).abs() < EPS);
        // After it: out 3.5 → src 7.5.
        assert!((tl.sample(3.5) - 7.5).abs() < EPS);
        // Projection inverts: src 4.5 → out 2.5.
        assert!((tl.project(4.5).unwrap() - 2.5).abs() < EPS);
    }

    #[test]
    fn speed_across_a_cut_applies_to_remainder() {
        let ops = EditOps {
            cuts: vec![(3.0, 5.0)],
            speeds: vec![(2.0, 2.0, 6.0)],
            ..Default::default()
        };
        let tl = compile(ops, 10.0);
        // 0..2 @1 (2s) + 2..3 @2 (0.5s) + 5..6 @2 (0.5s) + 6..10 @1 (4s) = 7s.
        assert!((tl.out_duration() - 7.0).abs() < EPS);
    }

    #[test]
    fn overlapping_speeds_error() {
        let ops = EditOps { speeds: vec![(2.0, 1.0, 5.0), (3.0, 4.0, 6.0)], ..Default::default() };
        assert!(matches!(
            Timeline::compile(&ops, 10.0).unwrap_err(),
            TimelineError::SpeedOverlap(..)
        ));
    }

    #[test]
    fn hold_inserts_still_time() {
        let tl = compile(EditOps { holds: vec![(2.0, 5.0)], ..Default::default() }, 10.0);
        assert!((tl.out_duration() - 12.0).abs() < EPS);
        assert!((tl.sample(4.9) - 4.9).abs() < EPS);
        assert!((tl.sample(6.0) - 5.0).abs() < EPS); // inside the hold
        assert!((tl.sample(8.0) - 6.0).abs() < EPS); // after it
    }

    #[test]
    fn card_inserts_a_still_and_reports_its_window() {
        let ops = EditOps {
            cards: vec![(5.0, 2.0, "Step 1".into())],
            ..Default::default()
        };
        let tl = compile(ops, 10.0);
        assert!((tl.out_duration() - 12.0).abs() < EPS);
        assert_eq!(tl.cards().len(), 1);
        let (a, b, text) = &tl.cards()[0];
        assert!((a - 5.0).abs() < EPS, "card starts where the still does");
        assert!((b - 7.0).abs() < EPS);
        assert_eq!(text, "Step 1");
        // The card window holds the source state at its anchor.
        assert!((tl.sample(6.0) - 5.0).abs() < EPS);
    }

    #[test]
    fn cards_and_holds_share_one_ordered_queue() {
        let ops = EditOps {
            holds: vec![(1.0, 5.0)],
            cards: vec![(2.0, 2.0, "intro".into()), (8.0, 1.0, "outro".into())],
            ..Default::default()
        };
        let tl = compile(ops, 10.0);
        assert!((tl.out_duration() - 14.0).abs() < EPS);
        let windows: Vec<(f64, f64)> = tl.cards().iter().map(|(a, b, _)| (*a, *b)).collect();
        // intro at 2s (out 2..4), then the hold at 5s (out 5..6), then the
        // outro card at 8s (out 9..10) — each shifted by what came before.
        assert_eq!(windows.len(), 2);
        assert!((windows[0].0 - 2.0).abs() < EPS, "{windows:?}");
        assert!((windows[0].1 - 4.0).abs() < EPS, "{windows:?}");
        assert!((windows[1].0 - 11.0).abs() < EPS, "{windows:?}");
        assert!((windows[1].1 - 12.0).abs() < EPS, "{windows:?}");
        // Card windows never overlap a Play segment's footage.
        for (a, b, _) in tl.cards() {
            assert!((tl.sample(*a) - tl.sample((a + b) / 2.0)).abs() < EPS);
        }
    }

    #[test]
    fn outro_card_lands_before_the_freeze() {
        let ops = EditOps {
            cards: vec![(10.0, 2.0, "github.com/x/reel".into())],
            freeze_last: Some(1.0),
            ..Default::default()
        };
        let tl = compile(ops, 10.0);
        assert!((tl.out_duration() - 13.0).abs() < EPS);
        let (a, b, _) = &tl.cards()[0];
        assert!((a - 10.0).abs() < EPS);
        assert!((b - 12.0).abs() < EPS, "the freeze appends after the card");
    }

    #[test]
    fn a_card_inside_a_cut_snaps_and_warns() {
        let ops = EditOps {
            cuts: vec![(4.0, 6.0)],
            cards: vec![(5.0, 1.0, "oops".into())],
            ..Default::default()
        };
        let (tl, warnings) = Timeline::compile(&ops, 10.0).unwrap();
        assert!(warnings.iter().any(|w| w.contains("card at")), "{warnings:?}");
        assert_eq!(tl.cards().len(), 1);
    }

    #[test]
    fn an_outro_card_past_the_trim_is_not_a_warning() {
        // `card "…" at end` resolves past the trimmed tail; that's the
        // documented outro form, not an anchor lost in a cut.
        let ops = EditOps {
            trim: Some((0.0, 6.0)),
            cards: vec![(10.0, 1.5, "fin".into())],
            ..Default::default()
        };
        let (tl, warnings) = Timeline::compile(&ops, 10.0).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let (a, b, _) = &tl.cards()[0];
        assert!((a - 6.0).abs() < EPS && (b - 7.5).abs() < EPS);
    }

    #[test]
    fn freeze_extends_the_end() {
        let tl = compile(EditOps { freeze_last: Some(1.5), ..Default::default() }, 10.0);
        assert!((tl.out_duration() - 11.5).abs() < EPS);
        assert!((tl.sample(11.0) - 10.0).abs() < EPS);
    }

    #[test]
    fn everything_composed() {
        // The spec's §5.7 example, roughly: trim, cut, speed inside.
        let ops = EditOps {
            trim: Some((2.0, 40.0)),
            cuts: vec![(19.0, 23.0)],
            speeds: vec![(5.0, 8.0, 34.0)],
            freeze_last: Some(1.5),
            ..Default::default()
        };
        let (tl, warnings) = Timeline::compile(&ops, 45.0).unwrap();
        assert!(warnings.is_empty());
        // Kept: 2..19, 23..40. Speed 5x covers 8..19 and 23..34.
        // 2..8 @1 =6, 8..19 @5 =2.2, 23..34 @5 =2.2, 34..40 @1 =6, +1.5 freeze.
        assert!((tl.out_duration() - 17.9).abs() < 1e-6);
        // Monotonic sanity sweep.
        let mut prev = tl.sample(0.0);
        for i in 1..=1790 {
            let s = tl.sample(i as f64 * 0.01);
            assert!(s >= prev - EPS, "sample not monotonic at {}", i);
            prev = s;
        }
    }

    #[test]
    fn nothing_left_errors() {
        let ops = EditOps { cuts: vec![(0.0, 10.0)], ..Default::default() };
        assert!(matches!(Timeline::compile(&ops, 10.0).unwrap_err(), TimelineError::NothingLeft));
    }
}
