//! Frame planning: turns (timeline, snapshots, visual ops, fps cap) into an
//! explicit list of frames to render. Frames are emitted on *change* — a grid
//! update, a camera movement tick, an overlay appearing — never on a bare
//! clock, which is where the file-size win comes from.

use reel_term::Snapshot;
use reel_timeline::{CaptionPos, Timeline, VisualOp};

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

#[derive(Debug, Clone)]
pub struct FramePlan {
    pub out_t: f64,
    /// Display duration in output seconds.
    pub dur: f64,
    /// Index into the snapshot list.
    pub snapshot: usize,
    pub camera: Camera,
    pub captions: Vec<CaptionDraw>,
    /// Highlight rects in cell coords (col, row, w, h).
    pub highlights: Vec<(u16, u16, u16, u16)>,
    /// Whether the cursor is drawn (false during a blink's off phase).
    pub cursor_on: bool,
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
    start: f64,
    end: f64,
}

const RAMP_MAX: f64 = 0.45;

/// Half-period of the synthetic cursor blink (the classic ~530ms).
const BLINK_HALF: f64 = 0.53;

pub fn plan(
    timeline: &Timeline,
    snapshots: &[Snapshot],
    visuals: &[VisualOp],
    fps: u32,
) -> Vec<FramePlan> {
    plan_with(timeline, snapshots, visuals, fps, false)
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
    let fps = fps.clamp(1, 120) as f64;
    let step = 1.0 / fps;
    let out_dur = timeline.out_duration();

    // --- Project visual ops into output time -----------------------------
    let mut zooms: Vec<ZoomWindow> = Vec::new();
    let mut captions: Vec<CaptionWindow> = Vec::new();
    let mut highlights: Vec<HighlightWindow> = Vec::new();
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
            VisualOp::Highlight { rect, at, dur } => {
                let start = timeline.project_snapped(*at);
                highlights.push(HighlightWindow { rect: *rect, start, end: (start + dur).min(out_dur) });
            }
            VisualOp::Marker { .. } => {}
        }
    }

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
            .map(|h| h.rect)
            .collect();
        frames.push(FramePlan {
            out_t: t,
            dur: dur.max(1.0 / fps / 2.0),
            snapshot,
            camera,
            captions: caps,
            highlights: hls,
            cursor_on: true,
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

    if cursor_blink {
        merged = blink(merged, snapshots);
    }
    merged
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
