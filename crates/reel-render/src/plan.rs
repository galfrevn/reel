//! Frame planning: turns (timeline, snapshots, visual ops, fps cap) into an
//! explicit list of frames to render. Frames are emitted on *change* — a grid
//! update, a camera movement tick, an overlay appearing — never on a bare
//! clock, which is where the file-size win comes from.

use reel_term::Snapshot;
use reel_timeline::{
    CaptionPos, HighlightStyle, NoteSide, NoteStyle, Segment, Timeline, VisualOp,
};

/// Camera state for one frame. `zoom == 1.0` means the base view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub zoom: f64,
    /// Zoom center in fractional cell coordinates.
    pub center: (f64, f64),
}

impl Camera {
    pub const BASE: Camera = Camera { zoom: 1.0, center: (0.0, 0.0) };
}

#[derive(Debug, Clone)]
pub struct CaptionDraw {
    pub text: String,
    pub pos: CaptionPos,
}

/// A highlight rect in cell coords, with its entrance state.
#[derive(Debug, Clone, PartialEq)]
pub struct HighlightDraw {
    /// (col, row, w, h) in cells.
    pub rect: (u16, u16, u16, u16),
    pub style: HighlightStyle,
    pub anim: Anim,
}

/// A callout anchored to a cell.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteDraw {
    pub text: String,
    /// The cell the note points at.
    pub anchor: (u16, u16),
    pub style: NoteStyle,
    pub side: NoteSide,
    pub anim: Anim,
}

/// The speed chip shown while a ramp plays.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateBadge {
    pub rate: f64,
    pub anim: Anim,
}

/// A full-frame title card, drawn over the still it inserted.
#[derive(Debug, Clone, PartialEq)]
pub struct CardDraw {
    pub text: String,
    pub anim: Anim,
}

#[derive(Debug, Clone)]
pub struct FramePlan {
    pub out_t: f64,
    /// Display duration in output seconds.
    pub dur: f64,
    /// Index into the snapshot list.
    pub snapshot: usize,
    pub camera: Camera,
    pub captions: Vec<CaptionDraw>,
    /// Highlight rects in cell coords.
    pub highlights: Vec<HighlightDraw>,
    /// Callouts anchored to cells.
    pub notes: Vec<NoteDraw>,
    /// The title card covering this frame, if any.
    pub card: Option<CardDraw>,
    /// Playback rate to announce as a chip (None = don't, or 1x).
    pub rate_badge: Option<RateBadge>,
    /// Progress through the video, 0..1, quantized (None = no bar).
    pub progress: Option<f32>,
    /// Keystroke-overlay chips currently visible, oldest first.
    pub keys: Vec<String>,
    /// Whether the cursor is drawn (false during a blink's off phase).
    pub cursor_on: bool,
    /// Fractional cell position mid cursor-slide (None = the snapshot's).
    pub cursor_pos: Option<(f32, f32)>,
    /// Freshly changed cells still glowing: (col, row, intensity 0..1).
    pub glow: Vec<(u16, u16, f32)>,
}

/// Knobs for [`plan_frames`] beyond the raw fps.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PlanOptions {
    /// Blink the cursor during long stills.
    pub cursor_blink: bool,
    /// Slide the cursor between cells over this many ms (0 = off).
    pub cursor_slide_ms: f32,
    /// Typing-glow strength 0..1 (0 = off).
    pub typing_glow: f32,
    /// Announce speed ramps with a `▸▸ 5×` chip.
    pub speed_badge: bool,
    /// Burn a progress bar into the frame. Where the notches go is the
    /// renderer's business (`RenderSettings::progress_ticks`); the planner
    /// only needs to know the bar exists, because it emits frames.
    pub progress: bool,
}

/// A zoom/pan op projected into output time.
struct ZoomWindow {
    factor: f64,
    center: (f64, f64),
    /// Output-time extent; None = whole video.
    range: Option<(f64, f64)>,
    ramp: f64,
    pans: Vec<PanWindow>,
}

struct PanWindow {
    to: (f64, f64),
    range: (f64, f64),
}

struct CaptionWindow {
    text: String,
    pos: CaptionPos,
    start: f64,
    end: f64,
}

struct HighlightWindow {
    rect: (u16, u16, u16, u16),
    style: HighlightStyle,
    start: f64,
    end: f64,
}

struct NoteWindow {
    text: String,
    anchor: (u16, u16),
    style: NoteStyle,
    side: NoteSide,
    start: f64,
    end: f64,
}

struct KeyWindow {
    label: String,
    start: f64,
    end: f64,
}

const RAMP_MAX: f64 = 0.45;

/// Entrance and exit durations for a note, highlight, or card, in output
/// seconds. The entrance is the longer of the two on purpose: arriving is
/// what the viewer watches, leaving is what they stop watching.
const ENTER: f64 = 0.38;
const EXIT: f64 = 0.26;

/// Steps the progress bar is quantized to. A continuous bar would change on
/// every frame and defeat the change-driven frame economy; this caps the
/// extra frames a bar can add to the whole video.
const PROGRESS_STEPS: usize = 120;

/// Entrance/exit state of one overlay, for the renderer to transform with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anim {
    /// Eased presence. Rises 0→1 with a spring overshoot (peaks ~1.07) on
    /// the way in, eases 1→0 on the way out. Scale, offset, and draw-on
    /// fractions all read from this.
    pub t: f32,
    /// Opacity, always 0..1 — a separate curve so the overshoot in `t`
    /// never prints as a flash.
    pub alpha: f32,
}

impl Anim {
    /// Fully settled — what a still frame in the middle of a window gets.
    pub const SETTLED: Anim = Anim { t: 1.0, alpha: 1.0 };

    /// Whether there is anything to draw at all.
    pub fn visible(&self) -> bool {
        self.alpha > 0.004
    }
}

/// Overshooting ease for the entrance — the spring feel, without a spring
/// solver: back-out with the classic 1.70158 tension.
fn ease_out_back(x: f32) -> f32 {
    const C1: f32 = 1.70158;
    const C3: f32 = C1 + 1.0;
    let x = x.clamp(0.0, 1.0) - 1.0;
    1.0 + C3 * x * x * x + C1 * x * x
}

fn ease_out_cubic(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    1.0 - (1.0 - x).powi(3)
}

fn ease_in_cubic(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * x
}

/// The animation state of a window at output time `t`. Values are quantized
/// so frames that land on the same state still dedupe.
fn anim_at(t: f64, start: f64, end: f64) -> Anim {
    let span = end - start;
    if span <= 1e-9 {
        return Anim::SETTLED;
    }
    // A window too short for both ramps splits what it has between them.
    let scale = (span / (ENTER + EXIT)).min(1.0);
    let (enter, exit) = (ENTER * scale, EXIT * scale);
    let since = t - start;
    let until = end - t;

    let q = |v: f32| (v * 128.0).round() / 128.0;
    if since < enter {
        let x = (since / enter) as f32;
        Anim { t: q(ease_out_back(x)), alpha: q(ease_out_cubic(x)) }
    } else if until < exit {
        let x = 1.0 - (until / exit) as f32;
        Anim { t: q(1.0 - ease_in_cubic(x)), alpha: q(1.0 - ease_in_cubic(x)) }
    } else {
        Anim::SETTLED
    }
}

/// Emission times covering both ramps of a window, so the motion is drawn
/// at the full frame rate instead of jumping between change frames.
fn anim_ticks(start: f64, end: f64, step: f64, out: &mut Vec<f64>) {
    let span = end - start;
    if span <= 1e-9 {
        return;
    }
    let scale = (span / (ENTER + EXIT)).min(1.0);
    let (enter, exit) = (ENTER * scale, EXIT * scale);
    let mut t = start;
    while t < start + enter {
        out.push(t);
        t += step;
    }
    let mut t = end - exit;
    while t < end {
        out.push(t);
        t += step;
    }
}

/// Output-time windows where playback is not 1x, one per `Play` segment.
/// Modelled as windows rather than sampled per frame so the chip gets the
/// same entrance treatment as every other overlay.
fn badge_windows(timeline: &Timeline, out_dur: f64) -> Vec<(f64, f64, f64)> {
    let segs = timeline.segments();
    let mut out = Vec::new();
    for (i, seg) in segs.iter().enumerate() {
        let Segment::Play { rate, .. } = *seg else { continue };
        if (rate - 1.0).abs() <= 0.05 {
            continue;
        }
        let start = seg.out_start();
        let end = segs.get(i + 1).map(|n| n.out_start()).unwrap_or(out_dur);
        if end - start > 1e-6 {
            out.push((rate, start, end));
        }
    }
    out
}

/// How long a keystroke chip stays on screen, in output seconds.
const KEY_HOLD: f64 = 1.2;
/// Most chips shown at once; older ones drop off first.
const MAX_KEY_CHIPS: usize = 6;

/// Half-period of the synthetic cursor blink (the classic ~530ms).
const BLINK_HALF: f64 = 0.53;

pub fn plan(
    timeline: &Timeline,
    snapshots: &[Snapshot],
    visuals: &[VisualOp],
    fps: u32,
) -> Vec<FramePlan> {
    plan_frames(timeline, snapshots, visuals, fps, &PlanOptions::default())
}

/// Like [`plan`], with a synthetic cursor blink during long stills — real
/// terminals blink, and a frozen block cursor is what makes long pauses in
/// a demo read as "the video hung".
pub fn plan_with(
    timeline: &Timeline,
    snapshots: &[Snapshot],
    visuals: &[VisualOp],
    fps: u32,
    cursor_blink: bool,
) -> Vec<FramePlan> {
    plan_frames(
        timeline,
        snapshots,
        visuals,
        fps,
        &PlanOptions { cursor_blink, ..Default::default() },
    )
}

/// [`plan`] with the full option set (blink, cursor slide, typing glow).
pub fn plan_frames(
    timeline: &Timeline,
    snapshots: &[Snapshot],
    visuals: &[VisualOp],
    fps: u32,
    opts: &PlanOptions,
) -> Vec<FramePlan> {
    let cursor_blink = opts.cursor_blink;
    let fps = fps.clamp(1, 120) as f64;
    let step = 1.0 / fps;
    let out_dur = timeline.out_duration();

    // --- Project visual ops into output time -----------------------------
    let mut zooms: Vec<ZoomWindow> = Vec::new();
    let mut captions: Vec<CaptionWindow> = Vec::new();
    let mut highlights: Vec<HighlightWindow> = Vec::new();
    let mut notes: Vec<NoteWindow> = Vec::new();
    let mut key_windows: Vec<KeyWindow> = Vec::new();
    for v in visuals {
        match v {
            VisualOp::Zoom { factor, center, range } => {
                let range = range.map(|(a, b)| {
                    (timeline.project_snapped(a), timeline.project_snapped(b))
                });
                let ramp = match range {
                    Some((a, b)) => RAMP_MAX.min((b - a) * 0.25),
                    None => 0.0,
                };
                zooms.push(ZoomWindow {
                    factor: *factor,
                    center: (center.0 as f64, center.1 as f64),
                    range,
                    ramp,
                    pans: Vec::new(),
                });
            }
            VisualOp::Pan { to, range } => {
                let range = (timeline.project_snapped(range.0), timeline.project_snapped(range.1));
                // Attach to the zoom whose window contains the pan start;
                // a pan with no active zoom has nothing to move.
                if let Some(z) = zooms.iter_mut().find(|z| match z.range {
                    Some((a, b)) => range.0 >= a - 1e-9 && range.0 <= b + 1e-9,
                    None => true,
                }) {
                    z.pans.push(PanWindow { to: (to.0 as f64, to.1 as f64), range });
                }
            }
            VisualOp::Caption { text, at, dur, pos } => {
                let start = timeline.project_snapped(*at);
                captions.push(CaptionWindow {
                    text: text.clone(),
                    pos: *pos,
                    start,
                    end: (start + dur).min(out_dur),
                });
            }
            VisualOp::Highlight { rect, at, dur, style } => {
                let start = timeline.project_snapped(*at);
                highlights.push(HighlightWindow {
                    rect: *rect,
                    style: *style,
                    start,
                    end: (start + dur).min(out_dur),
                });
            }
            VisualOp::Note { text, anchor, at, dur, style, side } => {
                // Authored like a caption, so it snaps rather than vanishing
                // with the footage under it.
                let start = timeline.project_snapped(*at);
                notes.push(NoteWindow {
                    text: text.clone(),
                    anchor: *anchor,
                    style: *style,
                    side: *side,
                    start,
                    end: (start + dur).min(out_dur),
                });
            }
            VisualOp::Key { label, at } => {
                // A key cut out of the timeline vanishes with its footage
                // (unlike captions, which snap: they're authored, keys are
                // recorded).
                if let Some(start) = timeline.project(*at) {
                    key_windows.push(KeyWindow {
                        label: label.clone(),
                        start,
                        end: (start + KEY_HOLD).min(out_dur),
                    });
                }
            }
        }
    }
    key_windows.sort_by(|a, b| a.start.total_cmp(&b.start));

    // --- Collect emission times (exact, never snapped to a grid) ---------
    // Snapping change times to an fps grid aliases against sources with
    // their own cadence (a 25Hz spinner on a 30fps grid judders 33/67ms);
    // exact timestamps keep motion even. The fps cap only *coalesces*
    // changes that land inside the same 1/fps window.
    let mut raw: Vec<f64> = Vec::new();
    raw.push(0.0);
    raw.push(out_dur);

    for s in snapshots {
        if let Some(t) = timeline.project(s.src_time) {
            raw.push(t);
        }
    }
    // Seam states: the frame right at each segment boundary.
    for seg in timeline.segments() {
        raw.push(seg.out_start());
    }

    // Camera ramps need continuous ticks.
    for z in &zooms {
        if let Some((a, b)) = z.range {
            let mut t = a;
            while t <= a + z.ramp + 1e-9 {
                raw.push(t);
                t += step;
            }
            let mut t = (b - z.ramp).max(a);
            while t <= b + 1e-9 {
                raw.push(t);
                t += step;
            }
            // Land exactly on the ramp ends so full zoom is reached on time.
            raw.push(a + z.ramp);
            raw.push(b - z.ramp);
            for p in &z.pans {
                let mut t = p.range.0;
                while t <= p.range.1 + 1e-9 {
                    raw.push(t);
                    t += step;
                }
            }
        }
    }
    for c in &captions {
        raw.push(c.start);
        raw.push(c.end);
    }
    for hl in &highlights {
        raw.push(hl.start);
        raw.push(hl.end);
        anim_ticks(hl.start, hl.end, step, &mut raw);
    }
    for n in &notes {
        raw.push(n.start);
        raw.push(n.end);
        anim_ticks(n.start, n.end, step, &mut raw);
    }
    for (start, end, _) in timeline.cards() {
        raw.push(*start);
        raw.push(*end);
        anim_ticks(*start, *end, step, &mut raw);
    }
    for k in &key_windows {
        raw.push(k.start);
        raw.push(k.end);
    }
    let badges = if opts.speed_badge { badge_windows(timeline, out_dur) } else { Vec::new() };
    for (_, start, end) in &badges {
        anim_ticks(*start, *end, step, &mut raw);
    }
    if opts.progress && out_dur > 0.0 {
        for i in 1..PROGRESS_STEPS {
            raw.push(out_dur * i as f64 / PROGRESS_STEPS as f64);
        }
    }

    for t in &mut raw {
        *t = t.clamp(0.0, out_dur);
    }
    raw.sort_by(|a, b| a.total_cmp(b));

    // Coalesce: one frame per 1/fps window, keeping the window's *last*
    // event time so the frame's sample includes every change in the burst.
    let mut times: Vec<f64> = vec![0.0];
    let mut last_window = -1i64;
    for &t in &raw {
        if t <= 1e-9 {
            continue;
        }
        let w = (t / step).floor() as i64;
        if w == last_window {
            *times.last_mut().unwrap() = t;
        } else {
            times.push(t);
            last_window = w;
        }
    }
    let mut frames = Vec::with_capacity(times.len());
    for (i, &t) in times.iter().enumerate() {
        let next = times.get(i + 1).copied().unwrap_or(out_dur);
        let dur = next - t;
        if dur <= 1e-9 && i + 1 < times.len() {
            continue;
        }
        let src_t = timeline.sample(t);
        let snapshot = snapshot_at(snapshots, src_t);
        let camera = camera_at(&zooms, t);
        let caps = captions
            .iter()
            .filter(|c| t >= c.start - 1e-9 && t < c.end - 1e-9)
            .map(|c| CaptionDraw { text: c.text.clone(), pos: c.pos })
            .collect();
        let hls = highlights
            .iter()
            .filter(|h| t >= h.start - 1e-9 && t < h.end - 1e-9)
            .map(|h| HighlightDraw {
                rect: h.rect,
                style: h.style,
                anim: anim_at(t, h.start, h.end),
            })
            .collect();
        let note_draws = notes
            .iter()
            .filter(|n| t >= n.start - 1e-9 && t < n.end - 1e-9)
            .map(|n| NoteDraw {
                text: n.text.clone(),
                anchor: n.anchor,
                style: n.style,
                side: n.side,
                anim: anim_at(t, n.start, n.end),
            })
            .collect();
        let card = timeline
            .cards()
            .iter()
            .find(|(a, b, _)| t >= a - 1e-9 && t < b - 1e-9)
            .map(|(a, b, text)| CardDraw {
                text: text.clone(),
                anim: anim_at(t, *a, *b),
            });
        let rate_badge = badges
            .iter()
            .find(|(_, a, b)| t >= a - 1e-9 && t < b - 1e-9)
            .map(|(rate, a, b)| RateBadge { rate: *rate, anim: anim_at(t, *a, *b) });
        let progress = (opts.progress && out_dur > 0.0).then(|| {
            let steps = PROGRESS_STEPS as f64;
            (((t / out_dur) * steps).floor() / steps) as f32
        });
        let active_keys: Vec<String> = key_windows
            .iter()
            .filter(|k| t >= k.start - 1e-9 && t < k.end - 1e-9)
            .map(|k| k.label.clone())
            .collect();
        let keys = if active_keys.len() > MAX_KEY_CHIPS {
            active_keys[active_keys.len() - MAX_KEY_CHIPS..].to_vec()
        } else {
            active_keys
        };
        frames.push(FramePlan {
            out_t: t,
            dur: dur.max(1.0 / fps / 2.0),
            snapshot,
            camera,
            captions: caps,
            highlights: hls,
            notes: note_draws,
            card,
            rate_badge,
            progress,
            keys,
            cursor_on: true,
            cursor_pos: None,
            glow: Vec::new(),
        });
    }

    // Merge consecutive frames that ended up identical (same snapshot,
    // camera, overlays) — happens when a projected change and a segment
    // boundary quantize adjacently.
    let mut merged: Vec<FramePlan> = Vec::with_capacity(frames.len());
    for f in frames {
        if let Some(last) = merged.last_mut() {
            let same = last.snapshot == f.snapshot
                && last.camera == f.camera
                && last.highlights == f.highlights
                && last.notes == f.notes
                && last.card == f.card
                && last.rate_badge == f.rate_badge
                && last.progress == f.progress
                && last.keys == f.keys
                && last.captions.len() == f.captions.len()
                && last
                    .captions
                    .iter()
                    .zip(&f.captions)
                    .all(|(a, b)| a.text == b.text && a.pos == b.pos);
            if same {
                last.dur += f.dur;
                continue;
            }
        }
        merged.push(f);
    }

    if opts.cursor_slide_ms > 0.0 || opts.typing_glow > 0.0 {
        merged = animate(merged, snapshots, fps, opts);
    }
    if cursor_blink {
        merged = blink(merged, snapshots);
    }
    merged
}

/// Typing-glow decay window in output seconds.
const GLOW_DUR: f64 = 0.28;
/// A change touching more of the grid than this is a repaint, not typing —
/// no glow.
const GLOW_MAX_CELLS: usize = 64;

/// Splits the head of each post-change frame into short animation ticks:
/// the cursor slides from its previous cell and freshly changed cells carry
/// a decaying glow. Only frames right after a snapshot change inflate, so
/// the change-driven frame economy survives.
fn animate(
    frames: Vec<FramePlan>,
    snapshots: &[Snapshot],
    fps: f64,
    opts: &PlanOptions,
) -> Vec<FramePlan> {
    let step = 1.0 / fps;
    let slide_dur = (opts.cursor_slide_ms as f64 / 1000.0).max(0.0);
    let mut out: Vec<FramePlan> = Vec::with_capacity(frames.len());
    let mut prev_snap: Option<usize> = None;
    for f in frames {
        let changed = prev_snap.is_some_and(|p| p != f.snapshot);
        let from = prev_snap.and_then(|p| snapshots.get(p));
        prev_snap = Some(f.snapshot);
        let (Some(a), Some(b), true) = (from, snapshots.get(f.snapshot), changed) else {
            out.push(f);
            continue;
        };

        let slide = (slide_dur > 0.0
            && a.cursor.shape != reel_term::CursorShape::Hidden
            && b.cursor.shape != reel_term::CursorShape::Hidden
            && (a.cursor.col, a.cursor.row) != (b.cursor.col, b.cursor.row))
            .then_some(((a.cursor.col as f32, a.cursor.row as f32), (b.cursor.col as f32, b.cursor.row as f32)));

        let glow_cells: Vec<(u16, u16)> = if opts.typing_glow > 0.0 {
            // Cap relative to the grid so small grids still detect repaints.
            let cap = ((a.cols as usize * a.rows as usize) / 3).clamp(1, GLOW_MAX_CELLS);
            diff_cells(a, b, cap).unwrap_or_default()
        } else {
            Vec::new()
        };

        let anim_dur = match (slide.is_some(), glow_cells.is_empty()) {
            (false, true) => 0.0,
            (true, true) => slide_dur,
            (false, false) => GLOW_DUR,
            (true, false) => slide_dur.max(GLOW_DUR),
        }
        .min(f.dur);
        if anim_dur < step * 1.5 {
            out.push(f);
            continue;
        }

        let mut t = 0.0;
        while t < anim_dur - 1e-9 {
            let dur = step.min(anim_dur - t);
            let mut tick = f.clone();
            tick.out_t = f.out_t + t;
            tick.dur = dur;
            tick.cursor_pos = slide.filter(|_| t < slide_dur).map(|(from, to)| {
                let k = ease_in_out_cubic(t / slide_dur) as f32;
                (from.0 + (to.0 - from.0) * k, from.1 + (to.1 - from.1) * k)
            });
            if !glow_cells.is_empty() && t < GLOW_DUR {
                let decay = (1.0 - t / GLOW_DUR) as f32;
                tick.glow = glow_cells
                    .iter()
                    .map(|&(c, r)| (c, r, opts.typing_glow * decay))
                    .collect();
            }
            out.push(tick);
            t += dur;
        }
        if anim_dur < f.dur - 1e-9 {
            let mut rest = f.clone();
            rest.out_t = f.out_t + anim_dur;
            rest.dur = f.dur - anim_dur;
            out.push(rest);
        }
    }
    out
}

/// Cells that differ between two snapshots, or None when the change is too
/// large to be typing (full repaints shouldn't flash the whole screen).
fn diff_cells(a: &Snapshot, b: &Snapshot, max: usize) -> Option<Vec<(u16, u16)>> {
    if a.cols != b.cols || a.rows != b.rows {
        return None;
    }
    let cols = b.cols as usize;
    let mut out = Vec::new();
    for row in 0..b.rows as usize {
        for col in 0..cols {
            let i = row * cols + col;
            if a.cells[i] != b.cells[i] {
                out.push((col as u16, row as u16));
                if out.len() > max {
                    return None;
                }
            }
        }
    }
    Some(out)
}

/// Splits frames longer than a blink period into on/off phases when the
/// snapshot's cursor is visible.
fn blink(frames: Vec<FramePlan>, snapshots: &[Snapshot]) -> Vec<FramePlan> {
    let mut out = Vec::with_capacity(frames.len());
    for f in frames {
        let visible = snapshots
            .get(f.snapshot)
            .map(|s| s.cursor.shape != reel_term::CursorShape::Hidden)
            .unwrap_or(false);
        if !visible || f.dur < BLINK_HALF * 1.6 {
            out.push(f);
            continue;
        }
        let mut t = f.out_t;
        let end = f.out_t + f.dur;
        let mut on = true;
        while t < end - 1e-9 {
            let dur = BLINK_HALF.min(end - t);
            let mut phase = f.clone();
            phase.out_t = t;
            phase.dur = dur;
            phase.cursor_on = on;
            out.push(phase);
            t += dur;
            on = !on;
        }
    }
    out
}

/// Index of the last snapshot at or before `src_t`.
fn snapshot_at(snapshots: &[Snapshot], src_t: f64) -> usize {
    match snapshots.binary_search_by(|s| s.src_time.total_cmp(&(src_t + 1e-9))) {
        Ok(i) => i,
        Err(0) => 0,
        Err(i) => i - 1,
    }
}

fn ease_in_out_cubic(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    if x < 0.5 {
        4.0 * x * x * x
    } else {
        1.0 - (-2.0 * x + 2.0).powi(3) / 2.0
    }
}

fn camera_at(zooms: &[ZoomWindow], t: f64) -> Camera {
    for z in zooms {
        let (progress, in_window) = match z.range {
            None => (1.0, true),
            Some((a, b)) => {
                if t < a - 1e-9 || t > b + 1e-9 {
                    (0.0, false)
                } else if z.ramp <= 0.0 {
                    (1.0, true)
                } else if t < a + z.ramp {
                    (ease_in_out_cubic((t - a) / z.ramp), true)
                } else if t > b - z.ramp {
                    (ease_in_out_cubic((b - t) / z.ramp), true)
                } else {
                    (1.0, true)
                }
            }
        };
        if !in_window || progress <= 0.0 {
            continue;
        }
        let mut center = z.center;
        for p in &z.pans {
            let (pa, pb) = p.range;
            if t >= pb {
                center = p.to;
            } else if t > pa {
                let k = ease_in_out_cubic((t - pa) / (pb - pa).max(1e-9));
                center = (center.0 + (p.to.0 - center.0) * k, center.1 + (p.to.1 - center.1) * k);
            }
        }
        let zoom = 1.0 + (z.factor - 1.0) * progress;
        return Camera { zoom, center };
    }
    Camera::BASE
}

#[cfg(test)]
mod tests {
    use super::*;
    use reel_timeline::EditOps;

    fn snapshots(times: &[f64]) -> Vec<Snapshot> {
        times
            .iter()
            .map(|&t| Snapshot {
                src_time: t,
                cols: 2,
                rows: 1,
                cells: vec![Default::default(); 2],
                cursor: reel_term::Cursor { col: 0, row: 0, shape: reel_term::CursorShape::Block },
                palette_overrides: vec![],
            images: vec![],
            })
            .collect()
    }

    fn typed_snapshots(times: &[f64]) -> Vec<Snapshot> {
        // Each snapshot advances the cursor one cell and types one char.
        times
            .iter()
            .enumerate()
            .map(|(i, &t)| {
                let mut cells: Vec<reel_term::Cell> = vec![Default::default(); 10];
                for cell in cells.iter_mut().take(i) {
                    cell.ch = 'x';
                }
                Snapshot {
                    src_time: t,
                    cols: 10,
                    rows: 1,
                    cells,
                    cursor: reel_term::Cursor {
                        col: i as u16,
                        row: 0,
                        shape: reel_term::CursorShape::Block,
                    },
                    palette_overrides: vec![],
                    images: vec![],
                }
            })
            .collect()
    }

    #[test]
    fn cursor_slide_emits_interpolated_ticks() {
        let (tl, _) = Timeline::compile(&EditOps::default(), 4.0).unwrap();
        let snaps = typed_snapshots(&[0.0, 1.0]);
        let opts = PlanOptions { cursor_slide_ms: 200.0, ..Default::default() };
        let frames = plan_frames(&tl, &snaps, &[], 30, &opts);
        let sliding: Vec<&FramePlan> =
            frames.iter().filter(|f| f.cursor_pos.is_some()).collect();
        assert!(sliding.len() >= 3, "expected slide ticks, got {}", sliding.len());
        // Positions move monotonically from the old cell toward the new one.
        let xs: Vec<f32> = sliding.iter().map(|f| f.cursor_pos.unwrap().0).collect();
        assert!(xs.windows(2).all(|w| w[1] >= w[0]), "not monotonic: {xs:?}");
        assert!(xs[0] < 1.0 && *xs.last().unwrap() <= 1.0);
        // Durations still tile the timeline.
        let total: f64 = frames.iter().map(|f| f.dur).sum();
        assert!((total - 4.0).abs() < 0.05, "total {total}");
    }

    #[test]
    fn typing_glow_decays_and_skips_repaints() {
        let (tl, _) = Timeline::compile(&EditOps::default(), 4.0).unwrap();
        let snaps = typed_snapshots(&[0.0, 1.0]);
        let opts = PlanOptions { typing_glow: 0.8, ..Default::default() };
        let frames = plan_frames(&tl, &snaps, &[], 30, &opts);
        let glowing: Vec<&FramePlan> = frames.iter().filter(|f| !f.glow.is_empty()).collect();
        assert!(glowing.len() >= 3, "expected glow ticks, got {}", glowing.len());
        let peaks: Vec<f32> = glowing.iter().map(|f| f.glow[0].2).collect();
        assert!(peaks.windows(2).all(|w| w[1] <= w[0]), "not decaying: {peaks:?}");
        assert!(peaks[0] <= 0.8 && peaks[0] > 0.5);

        // A full-grid repaint (every cell differs) must not glow.
        let mut b = snaps[1].clone();
        for c in &mut b.cells {
            c.ch = 'Z';
        }
        let repaint = vec![snaps[0].clone(), b];
        let frames = plan_frames(&tl, &repaint, &[], 30, &opts);
        assert!(frames.iter().all(|f| f.glow.is_empty()), "repaint glowed");
    }

    #[test]
    fn motion_off_adds_no_frames() {
        let (tl, _) = Timeline::compile(&EditOps::default(), 4.0).unwrap();
        let snaps = typed_snapshots(&[0.0, 1.0]);
        let plain = plan_frames(&tl, &snaps, &[], 30, &PlanOptions::default());
        assert!(plain.iter().all(|f| f.cursor_pos.is_none() && f.glow.is_empty()));
    }

    #[test]
    fn blink_splits_long_stills_only() {
        let (tl, _) = Timeline::compile(&EditOps::default(), 10.0).unwrap();
        let snaps = snapshots(&[0.0, 1.0, 1.2]); // long still after 1.2s
        let frames = plan_with(&tl, &snaps, &[], 30, true);
        let phases: Vec<&FramePlan> = frames.iter().filter(|f| f.out_t >= 1.2).collect();
        assert!(phases.len() > 10, "long still should blink, got {}", phases.len());
        assert!(phases.iter().any(|f| !f.cursor_on));
        assert!(phases.iter().any(|f| f.cursor_on));
        // Short frames stay whole (the 0.2s frame between the changes).
        let short: Vec<&FramePlan> = frames
            .iter()
            .filter(|f| f.out_t >= 1.0 - 1e-9 && f.out_t < 1.19)
            .collect();
        assert_eq!(short.len(), 1);
        assert!(short[0].cursor_on);
        // Durations still tile the timeline.
        let total: f64 = frames.iter().map(|f| f.dur).sum();
        assert!((total - 10.0).abs() < 0.05, "total {total}");
    }

    #[test]
    fn frames_follow_changes_not_clock() {
        let (tl, _) = Timeline::compile(&EditOps::default(), 10.0).unwrap();
        let snaps = snapshots(&[0.0, 1.0, 2.0, 7.0]);
        let frames = plan(&tl, &snaps, &[], 30);
        // 4 change frames + terminal frame at end (merged if identical).
        assert!(frames.len() <= 5, "got {} frames", frames.len());
        assert_eq!(frames[0].snapshot, 0);
        assert_eq!(frames[1].snapshot, 1);
        // Long still gap: one frame lasting ~5s, not 150 frames.
        let long = frames.iter().find(|f| f.dur > 4.0).expect("long still frame");
        assert_eq!(long.snapshot, 2);
    }

    #[test]
    fn zoom_ramp_generates_ticks() {
        let (tl, _) = Timeline::compile(&EditOps::default(), 10.0).unwrap();
        let snaps = snapshots(&[0.0]);
        let visuals = vec![VisualOp::Zoom { factor: 2.0, center: (1, 0), range: Some((4.0, 8.0)) }];
        let frames = plan(&tl, &snaps, &visuals, 30);
        // Ramp ticks exist and reach full zoom mid-window.
        let mid = frames
            .iter()
            .find(|f| f.out_t <= 6.0 && 6.0 < f.out_t + f.dur)
            .unwrap();
        assert!((mid.camera.zoom - 2.0).abs() < 1e-6);
        let before = frames.iter().find(|f| f.out_t < 3.9).unwrap();
        assert_eq!(before.camera, Camera::BASE);
        let ramping = frames
            .iter()
            .filter(|f| f.out_t > 4.0 && f.out_t < 4.45 && f.camera.zoom > 1.0 && f.camera.zoom < 2.0)
            .count();
        assert!(ramping >= 2, "expected easing ticks, got {ramping}");
    }

    #[test]
    fn key_chips_appear_and_expire() {
        let (tl, _) = Timeline::compile(&EditOps::default(), 10.0).unwrap();
        let snaps = snapshots(&[0.0]);
        let visuals = vec![
            VisualOp::Key { label: "ls".into(), at: 2.0 },
            VisualOp::Key { label: "⏎".into(), at: 2.3 },
        ];
        let frames = plan(&tl, &snaps, &visuals, 30);
        let both = frames
            .iter()
            .find(|f| f.keys.len() == 2)
            .expect("frame with both chips");
        assert_eq!(both.keys, vec!["ls", "⏎"]);
        // After the hold window both chips are gone (the post-expiry frame
        // merges with the tail still).
        let late = frames.last().unwrap();
        assert!(late.out_t >= 3.4 && late.keys.is_empty(), "{late:?}");
    }

    #[test]
    fn cut_keys_vanish_instead_of_snapping() {
        let ops = EditOps { cuts: vec![(1.0, 3.0)], ..Default::default() };
        let (tl, _) = Timeline::compile(&ops, 10.0).unwrap();
        let snaps = snapshots(&[0.0]);
        let visuals = vec![VisualOp::Key { label: "^C".into(), at: 2.0 }];
        let frames = plan(&tl, &snaps, &visuals, 30);
        assert!(frames.iter().all(|f| f.keys.is_empty()), "cut key survived");
    }

    #[test]
    fn captions_toggle_overlays() {
        let (tl, _) = Timeline::compile(&EditOps::default(), 10.0).unwrap();
        let snaps = snapshots(&[0.0]);
        let visuals = vec![VisualOp::Caption {
            text: "hi".into(),
            at: 2.0,
            dur: 3.0,
            pos: CaptionPos::Bottom,
        }];
        let frames = plan(&tl, &snaps, &visuals, 30);
        assert!(frames.iter().any(|f| !f.captions.is_empty()));
        let with = frames.iter().find(|f| !f.captions.is_empty()).unwrap();
        assert!((with.out_t - 2.0).abs() < 0.05);
        assert!((with.dur - 3.0).abs() < 0.1, "caption frame lasts its duration");
    }

    #[test]
    fn spinner_cadence_stays_even_no_grid_aliasing() {
        // A 25Hz spinner (40ms) planned at 30fps used to alias into a
        // 33/67ms judder; exact timestamps must keep the spacing uniform.
        let (tl, _) = Timeline::compile(&EditOps::default(), 4.0).unwrap();
        let times: Vec<f64> = (0..75).map(|i| 0.2 + i as f64 * 0.04).collect();
        let snaps = snapshots(&times);
        let frames = plan(&tl, &snaps, &[], 60);
        let spin: Vec<&FramePlan> = frames
            .iter()
            .filter(|f| f.out_t > 0.21 && f.out_t < 3.1)
            .collect();
        for w in spin.windows(2) {
            let gap = w[1].out_t - w[0].out_t;
            assert!((gap - 0.04).abs() < 1e-6, "gap {gap} aliased");
        }
    }

    #[test]
    fn key_times_survive_exactly() {
        let (tl, _) = Timeline::compile(&EditOps::default(), 5.0).unwrap();
        let snaps = snapshots(&[0.0, 1.16, 1.27, 1.51, 1.64, 1.85]);
        let frames = plan(&tl, &snaps, &[], 60);
        for t in [1.16, 1.27, 1.51, 1.64, 1.85] {
            assert!(
                frames.iter().any(|f| (f.out_t - t).abs() < 1e-9),
                "exact time {t} missing"
            );
        }
    }

    #[test]
    fn notes_fade_in_and_out_within_their_window() {
        let (tl, _) = Timeline::compile(&EditOps::default(), 10.0).unwrap();
        let snaps = snapshots(&[0.0]);
        let visuals = vec![VisualOp::Note {
            text: "look".into(),
            anchor: (3, 1),
            at: 2.0,
            dur: 2.0,
            style: NoteStyle::Card,
            side: NoteSide::Auto,
        }];
        let frames = plan(&tl, &snaps, &visuals, 30);
        let shown: Vec<&FramePlan> = frames.iter().filter(|f| !f.notes.is_empty()).collect();
        assert!(!shown.is_empty(), "note never drew");
        // Confined to 2s..4s.
        assert!(shown.iter().all(|f| f.out_t >= 2.0 - 1e-9 && f.out_t < 4.0));
        // Ramps up from near zero and back down.
        let alphas: Vec<f32> = shown.iter().map(|f| f.notes[0].anim.alpha).collect();
        assert!(alphas[0] < 0.2, "no fade-in: {alphas:?}");
        assert!(alphas.iter().cloned().fold(0.0, f32::max) > 0.99, "never opaque: {alphas:?}");
        assert!(*alphas.last().unwrap() < 0.3, "no fade-out: {alphas:?}");
        assert!(shown.iter().all(|f| f.notes[0].anchor == (3, 1)));
    }

    #[test]
    fn the_entrance_overshoots_then_settles() {
        // The spring is the whole point of the motion: `t` must pass 1 and
        // come back, while alpha never exceeds 1.
        let (tl, _) = Timeline::compile(&EditOps::default(), 10.0).unwrap();
        let snaps = snapshots(&[0.0]);
        let visuals = vec![VisualOp::Note {
            text: "look".into(),
            anchor: (1, 0),
            at: 2.0,
            dur: 3.0,
            style: NoteStyle::Card,
            side: NoteSide::Auto,
        }];
        let frames = plan(&tl, &snaps, &visuals, 60);
        let anims: Vec<Anim> = frames
            .iter()
            .filter(|f| !f.notes.is_empty())
            .map(|f| f.notes[0].anim)
            .collect();
        assert!(anims.len() > 20, "entrance not drawn at frame rate: {}", anims.len());
        let peak = anims.iter().map(|a| a.t).fold(0.0, f32::max);
        assert!(peak > 1.0, "no overshoot, peak {peak}");
        assert!(peak < 1.2, "overshoot too violent, peak {peak}");
        assert!(anims.iter().all(|a| a.alpha <= 1.0), "alpha overshot with t");
        // It settles: somewhere in the middle it is exactly at rest.
        assert!(anims.iter().any(|a| *a == Anim::SETTLED));
    }

    #[test]
    fn a_short_window_still_gets_both_ramps() {
        // A note shorter than ENTER + EXIT must not skip its exit.
        let (tl, _) = Timeline::compile(&EditOps::default(), 10.0).unwrap();
        let snaps = snapshots(&[0.0]);
        let visuals = vec![VisualOp::Note {
            text: "brief".into(),
            anchor: (0, 0),
            at: 1.0,
            dur: 0.3,
            style: NoteStyle::Card,
            side: NoteSide::Auto,
        }];
        let frames = plan(&tl, &snaps, &visuals, 60);
        let alphas: Vec<f32> = frames
            .iter()
            .filter(|f| !f.notes.is_empty())
            .map(|f| f.notes[0].anim.alpha)
            .collect();
        assert!(alphas.len() > 4, "{alphas:?}");
        assert!(alphas[0] < 0.3, "no fade-in: {alphas:?}");
        assert!(*alphas.last().unwrap() < 0.5, "no fade-out: {alphas:?}");
    }

    #[test]
    fn the_speed_badge_animates_with_its_segment() {
        let ops = EditOps { speeds: vec![(5.0, 4.0, 8.0)], ..Default::default() };
        let (tl, _) = Timeline::compile(&ops, 10.0).unwrap();
        let times: Vec<f64> = (0..100).map(|i| i as f64 * 0.1).collect();
        let snaps = snapshots(&times);
        let opts = PlanOptions { speed_badge: true, ..Default::default() };
        let frames = plan_frames(&tl, &snaps, &[], 30, &opts);
        let shown: Vec<RateBadge> = frames.iter().filter_map(|f| f.rate_badge).collect();
        assert!(!shown.is_empty());
        assert!(shown.iter().all(|b| b.rate == 5.0));
        assert!(shown[0].anim.alpha < 0.3, "chip popped instead of sliding in");
        assert!(shown.iter().any(|b| b.anim == Anim::SETTLED));
    }

    #[test]
    fn a_note_anchored_in_a_cut_snaps_forward() {
        let ops = EditOps { cuts: vec![(2.0, 6.0)], ..Default::default() };
        let (tl, _) = Timeline::compile(&ops, 10.0).unwrap();
        let snaps = snapshots(&[0.0, 7.0]);
        let visuals = vec![VisualOp::Note {
            text: "x".into(),
            anchor: (0, 0),
            at: 4.0, // inside the cut
            dur: 1.0,
            style: NoteStyle::Card,
            side: NoteSide::Auto,
        }];
        let frames = plan(&tl, &snaps, &visuals, 30);
        assert!(frames.iter().any(|f| !f.notes.is_empty()), "authored note vanished with the cut");
    }

    #[test]
    fn highlight_styles_ride_through_to_the_frame() {
        let (tl, _) = Timeline::compile(&EditOps::default(), 6.0).unwrap();
        let snaps = snapshots(&[0.0]);
        let visuals = vec![VisualOp::Highlight {
            rect: (1, 0, 2, 1),
            at: 1.0,
            dur: 2.0,
            style: HighlightStyle::Box,
        }];
        let frames = plan(&tl, &snaps, &visuals, 30);
        let shown: Vec<&FramePlan> = frames.iter().filter(|f| !f.highlights.is_empty()).collect();
        assert!(!shown.is_empty());
        assert!(shown.iter().all(|f| f.highlights[0].style == HighlightStyle::Box));
        let alphas: Vec<f32> = shown.iter().map(|f| f.highlights[0].anim.alpha).collect();
        assert!(alphas[0] < 0.2 && alphas.iter().cloned().fold(0.0, f32::max) > 0.99);
    }

    #[test]
    fn a_card_covers_exactly_the_still_it_inserted() {
        let ops = EditOps {
            cards: vec![(5.0, 2.0, "Step 1".into())],
            ..Default::default()
        };
        let (tl, _) = Timeline::compile(&ops, 10.0).unwrap();
        let snaps = snapshots(&[0.0, 5.0]);
        let frames = plan(&tl, &snaps, &[], 30);
        let carded: Vec<&FramePlan> = frames.iter().filter(|f| f.card.is_some()).collect();
        assert!(!carded.is_empty(), "card never drew");
        assert!(
            carded.iter().all(|f| f.out_t >= 5.0 - 1e-9 && f.out_t < 7.0),
            "card leaked outside its still"
        );
        assert!(carded.iter().all(|f| f.card.as_ref().unwrap().text == "Step 1"));
        let total: f64 = frames.iter().map(|f| f.dur).sum();
        assert!((total - 12.0).abs() < 0.05, "total {total}");
    }

    #[test]
    fn the_speed_badge_follows_the_segment_rate() {
        let ops = EditOps { speeds: vec![(5.0, 4.0, 8.0)], ..Default::default() };
        let (tl, _) = Timeline::compile(&ops, 10.0).unwrap();
        let times: Vec<f64> = (0..100).map(|i| i as f64 * 0.1).collect();
        let snaps = snapshots(&times);
        let opts = PlanOptions { speed_badge: true, ..Default::default() };
        let frames = plan_frames(&tl, &snaps, &[], 30, &opts);
        assert!(
            frames.iter().any(|f| f.rate_badge.is_some_and(|b| b.rate == 5.0)),
            "no badge during the ramp"
        );
        // 1x stretches announce nothing.
        assert!(frames.iter().filter(|f| f.out_t < 3.9).all(|f| f.rate_badge.is_none()));
        // Off by default.
        let plain = plan(&tl, &snaps, &[], 30);
        assert!(plain.iter().all(|f| f.rate_badge.is_none()));
    }

    #[test]
    fn the_progress_bar_costs_a_bounded_number_of_frames() {
        let (tl, _) = Timeline::compile(&EditOps::default(), 20.0).unwrap();
        let snaps = snapshots(&[0.0, 1.0, 2.0]);
        let plain = plan(&tl, &snaps, &[], 30);
        let opts = PlanOptions { progress: true, ..Default::default() };
        let barred = plan_frames(&tl, &snaps, &[], 30, &opts);
        assert!(barred.iter().all(|f| f.progress.is_some()));
        let extra = barred.len() - plain.len();
        assert!(extra <= PROGRESS_STEPS, "progress added {extra} frames");
        // Monotonic, and it reaches the end.
        let ps: Vec<f32> = barred.iter().map(|f| f.progress.unwrap()).collect();
        assert!(ps.windows(2).all(|w| w[1] >= w[0]), "not monotonic: {ps:?}");
        assert!(*ps.last().unwrap() > 0.9, "bar never filled: {:?}", ps.last());
        // Off by default: no bar, no extra frames.
        assert!(plain.iter().all(|f| f.progress.is_none()));
    }

    #[test]
    fn speed_region_changes_collapse_to_fps_grid() {
        let ops = EditOps { speeds: vec![(10.0, 0.0, 10.0)], ..Default::default() };
        let (tl, _) = Timeline::compile(&ops, 10.0).unwrap();
        // 100 source changes in 10s → 10x speed → 100 changes in 1s output.
        let times: Vec<f64> = (0..100).map(|i| i as f64 * 0.1).collect();
        let snaps = snapshots(&times);
        let frames = plan(&tl, &snaps, &[], 30);
        assert!(frames.len() <= 33, "fps cap not applied: {} frames", frames.len());
    }
}
