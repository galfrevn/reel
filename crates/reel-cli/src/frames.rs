//! `--frames-out`: hand an edited timeline to someone else's compositor.
//!
//! reel renders video, not motion graphics — a launch video with animated
//! titles, product shots and a soundtrack belongs in Remotion, Motion Canvas,
//! After Effects or an NLE. What those tools want is a constant-rate image
//! sequence, and what they *can't* reconstruct is where the interesting
//! moments are. So this mode writes both: `NNNN.png` at a fixed fps, and a
//! `frames.json` manifest carrying the edit itself — every marker, caption,
//! note, card and speed ramp, in output seconds *and* in frame numbers.
//!
//! That manifest is the point. A recorder that only exports footage leaves
//! the compositor guessing at sync points; reel already knows them, because
//! it cut the timeline.
//!
//! It also solves MP4 without reel shipping an H.264 encoder: the printed
//! `ffmpeg` line turns the sequence into one, on the user's own ffmpeg.

use crate::json;
use crate::pipeline::{
    build_audio, human_size, progress_ticks, render_each_parallel, write_atomic, Loaded,
};
use anyhow::{anyhow, Context, Result};
use reel_format::ReelConfig;
use reel_render::{plan_frames, settings_from_config, Renderer};
use reel_timeline::{Segment, VisualOp};
use std::path::{Path, PathBuf};

/// Manifest schema version. Bump when a field changes meaning; consumers
/// pin on it the same way template packs do.
const SCHEMA: u32 = 1;

pub struct FramesReport {
    pub dir: PathBuf,
    pub manifest: PathBuf,
    pub audio: Option<PathBuf>,
    pub frames: usize,
    pub unique_frames: usize,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub duration_s: f64,
    pub bytes: u64,
    pub ffmpeg: String,
    pub warnings: Vec<String>,
}

/// Frame index for an output timestamp, clamped to the sequence.
fn frame_at(t: f64, fps: f64, total: usize) -> usize {
    ((t * fps).round().max(0.0) as usize).min(total.saturating_sub(1))
}

/// How many constant-rate ticks a video of this length holds. Always at
/// least one: a single-frame render is still a render.
fn tick_count(out_dur: f64, fps: f64) -> usize {
    ((out_dur * fps).round() as usize).max(1)
}

/// How many times each planned frame is written, given where each one
/// starts on the output clock. The invariant that matters: the counts sum
/// to exactly `tick_count`, so the sequence is neither short nor long, and
/// no planned frame is skipped except by rounding two of them onto the same
/// tick (which is what a 0 means).
fn hold_counts(starts: &[f64], out_dur: f64, fps: f64) -> Vec<usize> {
    let total = tick_count(out_dur, fps);
    let mut counts = vec![0usize; starts.len()];
    if starts.is_empty() {
        return counts;
    }
    let mut placed = 0usize;
    for (i, count) in counts.iter_mut().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(out_dur);
        let upto = ((end * fps).round() as usize).min(total);
        *count = upto.saturating_sub(placed);
        placed = upto;
    }
    // Rounding can leave the tail a frame or two short; the last planned
    // frame absorbs it rather than the sequence ending early.
    if placed < total {
        *counts.last_mut().unwrap() += total - placed;
    }
    counts
}

pub fn write_frames(
    loaded: &Loaded,
    cfg: &ReelConfig,
    dir: &Path,
    quiet: bool,
) -> Result<FramesReport> {
    let (mut settings, mut warnings) = settings_from_config(cfg)?;
    settings.progress_ticks = progress_ticks(&loaded.timeline, &loaded.markers);
    let fps = settings.fps;
    let plan_opts = settings.plan_options();
    let (mut renderer, font_warnings) = Renderer::new(settings)?;
    renderer.fit_exact(loaded.cast.cols(), loaded.cast.rows());
    warnings.extend(font_warnings);

    let plans = plan_frames(&loaded.timeline, &loaded.snapshots, &loaded.visuals, fps, &plan_opts);
    if plans.is_empty() {
        return Err(anyhow!("no frames to write"));
    }
    let out_dur = loaded.timeline.out_duration();
    let fps_f = fps as f64;

    // reel's planner emits variable-duration frames (identical frames
    // collapse into one long one — that's what keeps GIFs small). Every
    // compositor downstream wants constant rate, so expand back out: render
    // each distinct frame once, write it as many times as it is held.
    let starts: Vec<f64> = plans.iter().map(|f| f.out_t).collect();
    let total = tick_count(out_dur, fps_f);
    let counts = hold_counts(&starts, out_dur, fps_f);

    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating {}", dir.display()))?;

    if !quiet {
        eprintln!(
            "writing {} frames at {}fps ({:.1}s output from {:.1}s recording)…",
            total,
            fps,
            out_dur,
            loaded.cast.duration()
        );
    }

    let width_digits = total.to_string().len().max(4);
    let (mut w_px, mut h_px) = (0u32, 0u32);
    let mut written = 0usize;
    let mut unique = 0usize;
    let mut bytes = 0u64;
    let mut index = 0usize;
    let mut err: Option<anyhow::Error> = None;
    render_each_parallel(&mut renderer, &plans, &loaded.snapshots, |rgba, w, h, _| {
        if err.is_some() {
            return Ok(());
        }
        let repeats = counts[index];
        index += 1;
        if repeats == 0 {
            return Ok(()); // collapsed away by rounding
        }
        let png = reel_encode::encode_png(w, h, rgba)?;
        unique += 1;
        (w_px, h_px) = (w, h);
        for _ in 0..repeats {
            let path = dir.join(format!("{:0width$}.png", written, width = width_digits));
            if let Err(e) = write_atomic(&path, &png) {
                err = Some(anyhow::Error::new(e).context(format!("writing {}", path.display())));
                return Ok(());
            }
            bytes += png.len() as u64;
            written += 1;
        }
        Ok(())
    })?;
    if let Some(e) = err {
        return Err(e);
    }

    // Audio rides along as a WAV: a soundtrack the compositor can drop on
    // the timeline. reel's is synthesized from the recorded keystrokes, so
    // it is already in sync with the frames beside it.
    let audio_path = match build_audio(
        &loaded.cast,
        &loaded.cast_path,
        &loaded.snapshots,
        &loaded.timeline,
        &loaded.audio_ops,
        cfg,
        quiet,
    )? {
        Some(samples) if !samples.is_empty() => {
            let path = dir.join("audio.wav");
            write_atomic(&path, &crate::audio::wav_bytes(&samples))
                .with_context(|| format!("writing {}", path.display()))?;
            Some(path)
        }
        _ => None,
    };

    let pattern = format!("%0{width_digits}d.png");
    let ffmpeg = ffmpeg_line(dir, &pattern, fps, audio_path.as_deref());
    let manifest = manifest_json(
        loaded,
        cfg,
        fps,
        total,
        w_px,
        h_px,
        &pattern,
        audio_path.as_deref(),
        &ffmpeg,
    );
    let manifest_path = dir.join("frames.json");
    write_atomic(&manifest_path, serde_json::to_string_pretty(&manifest)?.as_bytes())
        .with_context(|| format!("writing {}", manifest_path.display()))?;

    Ok(FramesReport {
        dir: dir.to_path_buf(),
        manifest: manifest_path,
        audio: audio_path,
        frames: written,
        unique_frames: unique,
        fps,
        width: w_px,
        height: h_px,
        duration_s: out_dur,
        bytes,
        ffmpeg,
        warnings,
    })
}

/// The one-liner that turns the sequence into the MP4 reel deliberately
/// doesn't encode itself (H.264 licensing). yuv420p and the `scale` filter
/// are there because odd dimensions are rejected by most H.264 profiles.
fn ffmpeg_line(dir: &Path, pattern: &str, fps: u32, audio: Option<&Path>) -> String {
    let input = dir.join(pattern);
    let audio_in = audio.map(|a| format!(" -i {}", a.display())).unwrap_or_default();
    let audio_out = if audio.is_some() { " -c:a aac -shortest" } else { "" };
    format!(
        "ffmpeg -framerate {fps} -i {}{audio_in} -c:v libx264 -crf 18 \
         -pix_fmt yuv420p -vf \"scale=trunc(iw/2)*2:trunc(ih/2)*2\"{audio_out} out.mp4",
        input.display()
    )
}

/// Everything a compositor would otherwise have to guess, in output seconds
/// and frame numbers both — seconds for humans reading the file, frames for
/// `<Sequence from={…}>` and friends, which count in frames.
#[allow(clippy::too_many_arguments)]
fn manifest_json(
    loaded: &Loaded,
    cfg: &ReelConfig,
    fps: u32,
    total: usize,
    width: u32,
    height: u32,
    pattern: &str,
    audio: Option<&Path>,
    ffmpeg: &str,
) -> serde_json::Value {
    let fps_f = fps as f64;
    let tl = &loaded.timeline;
    let at = |t: f64| frame_at(t, fps_f, total);

    let segments: Vec<_> = tl
        .segments()
        .iter()
        .map(|seg| match *seg {
            Segment::Play { out_start, src_start, src_end, rate } => serde_json::json!({
                "kind": "play",
                "out_start_s": out_start,
                "out_end_s": out_start + seg.out_dur(),
                "frame_start": at(out_start),
                "frame_end": at(out_start + seg.out_dur()),
                "src_start_s": src_start,
                "src_end_s": src_end,
                "rate": rate,
            }),
            Segment::Still { out_start, src_at, dur } => serde_json::json!({
                "kind": "still",
                "out_start_s": out_start,
                "out_end_s": out_start + dur,
                "frame_start": at(out_start),
                "frame_end": at(out_start + dur),
                "src_at_s": src_at,
            }),
        })
        .collect();

    // Speed ramps read as their own list too: "where did time compress" is
    // the question a compositor asks when scoring a cut.
    let ramps: Vec<_> = tl
        .segments()
        .iter()
        .filter_map(|seg| match *seg {
            Segment::Play { out_start, rate, .. } if (rate - 1.0).abs() > 1e-9 => {
                Some(serde_json::json!({
                    "rate": rate,
                    "out_start_s": out_start,
                    "out_end_s": out_start + seg.out_dur(),
                    "frame_start": at(out_start),
                    "frame_end": at(out_start + seg.out_dur()),
                }))
            }
            _ => None,
        })
        .collect();

    let markers: Vec<_> = loaded
        .markers
        .iter()
        .map(|(label, src_t)| {
            let out_t = tl.project_snapped(*src_t);
            serde_json::json!({
                "label": label,
                "out_t_s": out_t,
                "frame": at(out_t),
                "src_t_s": src_t,
            })
        })
        .collect();

    let cards: Vec<_> = tl
        .cards()
        .iter()
        .map(|(start, dur, text)| {
            serde_json::json!({
                "text": text,
                "start_s": start,
                "end_s": start + dur,
                "frame_start": at(*start),
                "frame_end": at(start + dur),
            })
        })
        .collect();

    let mut captions = Vec::new();
    let mut notes = Vec::new();
    let mut highlights = Vec::new();
    let mut zooms = Vec::new();
    for v in &loaded.visuals {
        match v {
            VisualOp::Caption { text, at: src_at, dur, .. } => {
                let start = tl.project_snapped(*src_at);
                captions.push(serde_json::json!({
                    "text": text,
                    "start_s": start,
                    "end_s": start + dur,
                    "frame_start": at(start),
                    "frame_end": at(start + dur),
                }));
            }
            VisualOp::Note { text, anchor, at: src_at, dur, .. } => {
                let start = tl.project_snapped(*src_at);
                notes.push(serde_json::json!({
                    "text": text,
                    "anchor": {"col": anchor.0, "row": anchor.1},
                    "start_s": start,
                    "end_s": start + dur,
                    "frame_start": at(start),
                    "frame_end": at(start + dur),
                }));
            }
            VisualOp::Highlight { rect, at: src_at, dur, .. } => {
                let start = tl.project_snapped(*src_at);
                highlights.push(serde_json::json!({
                    "rect": {"col": rect.0, "row": rect.1, "cols": rect.2, "rows": rect.3},
                    "start_s": start,
                    "end_s": start + dur,
                    "frame_start": at(start),
                    "frame_end": at(start + dur),
                }));
            }
            VisualOp::Zoom { factor, center, range } => {
                let (start, end) = match range {
                    Some((a, b)) => (tl.project_snapped(*a), tl.project_snapped(*b)),
                    None => (0.0, tl.out_duration()),
                };
                zooms.push(serde_json::json!({
                    "factor": factor,
                    "center": {"col": center.0, "row": center.1},
                    "start_s": start,
                    "end_s": end,
                    "frame_start": at(start),
                    "frame_end": at(end),
                }));
            }
            // Key chips are per-keystroke and already burned in; a
            // compositor has no use for hundreds of them.
            VisualOp::Key { .. } | VisualOp::Pan { .. } => {}
        }
    }

    serde_json::json!({
        "schema": SCHEMA,
        "generator": concat!("reel ", env!("CARGO_PKG_VERSION")),
        "pattern": pattern,
        "fps": fps,
        "frames": total,
        "duration_s": tl.out_duration(),
        "width": width,
        "height": height,
        "audio": audio.map(|a| a.file_name().map(|f| f.to_string_lossy().into_owned())),
        "template": cfg.template.name,
        "source": {
            "cast": loaded.cast_path.file_name().map(|f| f.to_string_lossy().into_owned()),
            "duration_s": loaded.cast.duration(),
            "cols": loaded.cast.cols(),
            "rows": loaded.cast.rows(),
        },
        "segments": segments,
        "speed_ramps": ramps,
        // "chapters" is the name video tools use for the same idea; reel
        // calls them markers, so publish both rather than make callers map it.
        "markers": markers,
        "chapters": markers,
        "cards": cards,
        "captions": captions,
        "notes": notes,
        "highlights": highlights,
        "zooms": zooms,
        "ffmpeg": ffmpeg,
    })
}

/// Human summary; the JSON document is emitted by the caller instead when
/// `--json` is on.
pub fn report(r: &FramesReport, quiet: bool) -> Result<()> {
    if json::on() {
        return json::emit(serde_json::json!({
            "frames_out": r.dir.display().to_string(),
            "manifest": r.manifest.display().to_string(),
            "audio": r.audio.as_ref().map(|p| p.display().to_string()),
            "frames": r.frames,
            "unique_frames": r.unique_frames,
            "fps": r.fps,
            "width": r.width,
            "height": r.height,
            "duration_s": r.duration_s,
            "bytes": r.bytes,
            "ffmpeg": r.ffmpeg,
            "warnings": r.warnings,
        }));
    }
    if quiet {
        return Ok(());
    }
    eprintln!(
        "{}: {} frames at {}fps, {}x{} — {} ({} distinct, the rest are held)",
        r.dir.display(),
        r.frames,
        r.fps,
        r.width,
        r.height,
        human_size(r.bytes),
        r.unique_frames,
    );
    eprintln!("{}: markers, captions, notes and speed ramps in frames", r.manifest.display());
    if let Some(a) = &r.audio {
        eprintln!("{}: procedural soundtrack, already in sync", a.display());
    }
    eprintln!("\nto MP4:\n  {}", r.ffmpeg);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_index_clamps_to_the_sequence() {
        assert_eq!(frame_at(0.0, 30.0, 90), 0);
        assert_eq!(frame_at(1.0, 30.0, 90), 30);
        // Past the end lands on the last frame, never out of bounds.
        assert_eq!(frame_at(99.0, 30.0, 90), 89);
        assert_eq!(frame_at(-1.0, 30.0, 90), 0);
        // A zero-length sequence has no valid index to return but must not
        // underflow.
        assert_eq!(frame_at(5.0, 30.0, 0), 0);
    }

    /// The property the whole constant-rate expansion rests on.
    fn sums_to_total(starts: &[f64], out_dur: f64, fps: f64) {
        let counts = hold_counts(starts, out_dur, fps);
        assert_eq!(
            counts.iter().sum::<usize>(),
            tick_count(out_dur, fps),
            "counts {counts:?} for starts {starts:?} at {fps}fps over {out_dur}s"
        );
    }

    #[test]
    fn holds_cover_the_sequence_exactly() {
        // Evenly spaced frames: one tick each.
        sums_to_total(&[0.0, 1.0, 2.0], 3.0, 1.0);
        // A long hold in the middle — the planner's usual output.
        sums_to_total(&[0.0, 0.5, 4.0, 4.1], 5.0, 30.0);
        // Durations that don't land on a tick boundary.
        sums_to_total(&[0.0, 0.33, 0.67], 1.0, 30.0);
        sums_to_total(&[0.0, 2.7], 11.5, 30.0);
        // One frame, and a degenerate zero-length render.
        sums_to_total(&[0.0], 0.04, 30.0);
        sums_to_total(&[0.0], 0.0, 30.0);
    }

    #[test]
    fn a_held_frame_repeats_and_the_rest_stay_single() {
        // 3s at 1fps: frame 0 covers [0,1), frame 1 covers [1,3).
        assert_eq!(hold_counts(&[0.0, 1.0], 3.0, 1.0), vec![1, 2]);
    }

    #[test]
    fn frames_closer_than_a_tick_collapse_rather_than_stretch() {
        // Two plans inside one 30fps tick: the first gets no tick of its
        // own, and the total is still right.
        let counts = hold_counts(&[0.0, 0.001], 1.0, 30.0);
        assert_eq!(counts.iter().sum::<usize>(), 30);
        assert_eq!(counts[0], 0);
    }

    #[test]
    fn no_plans_means_no_frames() {
        assert!(hold_counts(&[], 5.0, 30.0).is_empty());
    }

    #[test]
    fn ffmpeg_line_mentions_audio_only_when_there_is_some() {
        let silent = ffmpeg_line(Path::new("out"), "%04d.png", 30, None);
        assert!(silent.contains("-framerate 30"));
        assert!(!silent.contains("-c:a"));
        let scored = ffmpeg_line(Path::new("out"), "%04d.png", 30, Some(Path::new("out/audio.wav")));
        assert!(scored.contains("audio.wav"));
        assert!(scored.contains("-c:a aac -shortest"));
    }
}
