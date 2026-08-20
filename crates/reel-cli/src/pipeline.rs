//! The render pipeline: .reel → cast → snapshots → timeline → frames →
//! encoder, plus the greedy budget ladder.

use anyhow::{anyhow, bail, Context, Result};
use reel_cast::{Cast, EventKind, ReelMeta};
use reel_encode::{GifOptions, PaletteMode, RgbaFrame, WebmOptions};
use reel_format::{parse_budget, ReelConfig, ReelFile, TimeExpr};
use reel_render::{pixmap_to_rgba, plan_with, settings_from_config, Renderer};
use reel_term::Snapshot;
use reel_timeline::{AudioOp, Timeline, VisualOp};
use std::path::{Path, PathBuf};

struct Loaded {
    file: ReelFile,
    cast: Cast,
    snapshots: Vec<Snapshot>,
    timeline: Timeline,
    visuals: Vec<VisualOp>,
    audio_ops: Vec<AudioOp>,
    base_dir: PathBuf,
    cast_path: PathBuf,
}

/// State kept between `reel watch` renders: replayed snapshots keyed by cast
/// mtime, and a renderer whose glyph cache stays warm across edits.
#[derive(Default)]
pub struct WatchCache {
    cast: Option<(PathBuf, std::time::SystemTime, Cast, Vec<Snapshot>)>,
    renderer: Option<Renderer>,
}

pub struct WatchRender {
    pub bytes: Vec<u8>,
    pub out_path: PathBuf,
    pub cast_path: PathBuf,
    pub extension: String,
}

fn load(path: &Path, template_override: Option<String>, quiet: bool) -> Result<Loaded> {
    let base_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut file = if path.extension().is_some_and(|e| e == "cast") {
        // Quick path: default styling straight from a recording.
        let text = format!(
            "---\n[source]\ncast = \"{}\"\n---\n",
            path.file_name().unwrap().to_string_lossy()
        );
        ReelFile::parse(&text)?
    } else {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        ReelFile::parse(&text).with_context(|| format!("parsing {}", path.display()))?
    };
    if let Some(t) = template_override {
        file.config.template.name = t;
    }

    let cast_rel = &file.config.source.as_ref().expect("edit mode guaranteed by parser").cast;
    let cast_path = base_dir.join(cast_rel);
    let cast = Cast::load(&cast_path)
        .with_context(|| format!("loading cast {}", cast_path.display()))?;

    let mut snapshots = reel_term::replay(&cast)?;
    if !uniform_dims(&snapshots) {
        bail!(
            "this cast resizes mid-session, which reel can't render yet — \
             re-record at a fixed size"
        );
    }
    // Rebuild letter-by-letter typing from the recorded keys: TUIs batch
    // their repaints; the demo shouldn't.
    reel_term::smooth_typing(&mut snapshots, &printable_keys(&cast, &cast_path));

    let program = file.resolve(cast.duration())?;
    let (timeline, warnings) = Timeline::compile(&program.edits, cast.duration())?;
    if !quiet {
        for w in &warnings {
            eprintln!("warning: {w}");
        }
    }
    // Redactions run before anything renders, and cover .txt dumps too.
    for pattern in &program.redactions {
        let re = regex_lite::Regex::new(pattern)
            .map_err(|e| anyhow!("redact `{pattern}`: {e}"))?;
        reel_term::redact::apply(&mut snapshots, &re);
    }
    if !quiet {
        for (kind, sample) in reel_term::redact::scan_sensitive(&snapshots, 4) {
            eprintln!(
                "warning: possible {kind} on screen: \"{sample}\" — mask it with: redact \"…\""
            );
        }
    }

    let visuals = program.visuals;
    let audio_ops = program.audio;
    Ok(Loaded { file, cast, snapshots, timeline, visuals, audio_ops, base_dir, cast_path })
}

/// Renders each planned frame into the renderer's reused buffer and hands
/// the encoder a borrowed slice — flat memory, zero per-frame allocations.
fn render_each(
    renderer: &mut Renderer,
    plans: &[reel_render::FramePlan],
    snapshots: &[Snapshot],
    mut sink: impl FnMut(&[u8], u32, u32, f64) -> Result<()>,
) -> Result<()> {
    for f in plans {
        let (w, h, rgba) = renderer.render_frame_rgba(&snapshots[f.snapshot], f);
        sink(rgba, w, h, f.dur)?;
    }
    Ok(())
}

fn uniform_dims(snaps: &[Snapshot]) -> bool {
    snaps.windows(2).all(|w| w[0].cols == w[1].cols && w[0].rows == w[1].rows)
}

fn render_frames(loaded: &Loaded, cfg: &ReelConfig, quiet: bool) -> Result<(Vec<RgbaFrame>, Vec<String>)> {
    let (settings, mut warnings) = settings_from_config(cfg)?;
    let fps = settings.fps;
    let settings_cursor_blink = settings.cursor_blink;
    let (mut renderer, font_warnings) = Renderer::new(settings)?;
    renderer.fit_exact(loaded.cast.cols(), loaded.cast.rows());
    warnings.extend(font_warnings);
    let frames = plan_with(&loaded.timeline, &loaded.snapshots, &loaded.visuals, fps, settings_cursor_blink);
    if !quiet {
        eprintln!(
            "rendering {} frames ({:.1}s output from {:.1}s recording)…",
            frames.len(),
            loaded.timeline.out_duration(),
            loaded.cast.duration()
        );
    }
    let mut out = Vec::with_capacity(frames.len());
    for f in &frames {
        let pix = renderer.render_frame(&loaded.snapshots[f.snapshot], f);
        out.push(RgbaFrame {
            width: pix.width(),
            height: pix.height(),
            data: pixmap_to_rgba(&pix),
            duration_s: f.dur,
        });
    }
    Ok((out, warnings))
}

pub fn render(
    path: &Path,
    out: Option<PathBuf>,
    template: Option<String>,
    budget: Option<String>,
    scale: Option<u32>,
    aspect: Option<String>,
    size: Option<String>,
    no_audio: bool,
    quiet: bool,
) -> Result<()> {
    let loaded = load(path, template, quiet)?;
    let mut cfg = loaded.file.config.clone();
    if let Some(s) = scale {
        cfg.output.scale = s;
    }
    if aspect.is_some() {
        cfg.output.aspect = aspect;
    }
    if size.is_some() {
        cfg.output.size = size;
    }
    if let Some(b) = budget {
        cfg.output.budget = Some(b);
    }
    if no_audio {
        cfg.audio.enabled = Some(false);
    }

    let out_path = out
        .or_else(|| cfg.output.file.as_ref().map(|f| loaded.base_dir.join(f)))
        .unwrap_or_else(|| path.with_extension("gif"));
    let ext = out_path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    let result = match ext.as_str() {
        "gif" => render_gif(&loaded, cfg.clone(), &out_path, quiet),
        "png" => {
            let (frames, warns) = render_frames(&loaded, &cfg, quiet)?;
            print_warnings(&warns, quiet);
            let f = frames.first().ok_or_else(|| anyhow!("no frames"))?;
            std::fs::write(&out_path, reel_encode::encode_png(f.width, f.height, &f.data)?)?;
            done(&out_path, quiet)
        }
        "txt" => {
            let mut text = String::new();
            for s in &loaded.snapshots {
                text.push_str(&format!("--- t={:.3}s\n", s.src_time));
                text.push_str(&s.to_text());
            }
            std::fs::write(&out_path, text)?;
            done(&out_path, quiet)
        }
        "apng" => render_apng(&loaded, cfg.clone(), &out_path, quiet),
        "webm" => render_webm(&loaded, cfg.clone(), &out_path, quiet),
        "mp4" => bail!(
            "mp4 is deferred (H.264 licensing) — render a .webm, or use .gif for READMEs"
        ),
        other => bail!(
            "unsupported output extension `.{other}` (use .gif, .webm, .apng, .png, or .txt)"
        ),
    };
    if result.is_ok() && cfg.output.subtitles {
        let vtt_path = out_path.with_extension("vtt");
        std::fs::write(&vtt_path, render_vtt(&caption_cues(&loaded)))?;
        if !quiet {
            eprintln!("{}: WebVTT captions sidecar", vtt_path.display());
        }
    }
    result
}

/// Animated PNG: lossless truecolor, streamed in one pass (no palette to
/// negotiate and no budget ladder — sizes are what the content costs).
fn render_apng(loaded: &Loaded, cfg: ReelConfig, out_path: &Path, quiet: bool) -> Result<()> {
    if cfg.output.budget.is_some() && !quiet {
        eprintln!("note: budget is ignored for .apng (lossless format)");
    }
    let (settings, warns) = settings_from_config(&cfg)?;
    let fps = settings.fps;
    let blink = settings.cursor_blink;
    let (mut renderer, font_warns) = Renderer::new(settings)?;
    renderer.fit_exact(loaded.cast.cols(), loaded.cast.rows());
    print_warnings(&warns, quiet);
    print_warnings(&font_warns, quiet);

    let plans = plan_with(&loaded.timeline, &loaded.snapshots, &loaded.visuals, fps, blink);
    if !quiet {
        eprintln!(
            "rendering {} frames ({:.1}s output from {:.1}s recording)…",
            plans.len(),
            loaded.timeline.out_duration(),
            loaded.cast.duration()
        );
    }
    let (cw, ch) = renderer.canvas_size(loaded.cast.cols(), loaded.cast.rows());
    let mut bytes = Vec::new();
    let frames = {
        let mut stream = reel_encode::ApngStream::new(
            &mut bytes,
            cw,
            ch,
            plans.len() as u32,
            cfg.output.looping,
        )?;
        render_each(&mut renderer, &plans, &loaded.snapshots, |rgba, _, _, dur| {
            stream.push(rgba, dur).map_err(Into::into)
        })?;
        stream.finish()?
    };
    std::fs::write(out_path, &bytes)?;
    if !quiet {
        eprintln!(
            "{}: {} — {} frames, lossless truecolor",
            out_path.display(),
            human_size(bytes.len() as u64),
            frames
        );
    }
    Ok(())
}

/// Caption windows in output time: (start, end, text).
fn caption_cues(loaded: &Loaded) -> Vec<(f64, f64, String)> {
    loaded
        .visuals
        .iter()
        .filter_map(|v| match v {
            VisualOp::Caption { text, at, dur, .. } => {
                let start = loaded.timeline.project_snapped(*at);
                let end = (start + dur).min(loaded.timeline.out_duration());
                (end > start).then(|| (start, end, text.clone()))
            }
            _ => None,
        })
        .collect()
}

fn render_vtt(cues: &[(f64, f64, String)]) -> String {
    let ts = |t: f64| {
        let ms = (t * 1000.0).round() as u64;
        format!("{:02}:{:02}:{:02}.{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
    };
    let mut out = String::from("WEBVTT\n\n");
    for (start, end, text) in cues {
        out.push_str(&format!("{} --> {}\n{}\n\n", ts(*start), ts(*end), text));
    }
    out
}

// ---------------------------------------------------------------------------
// Audio
// ---------------------------------------------------------------------------

/// Builds the mixed 48kHz mono buffer for this render, or `None` when audio
/// is inactive. Only the WebM path calls this.
fn build_audio(
    cast: &Cast,
    cast_path: &Path,
    snapshots: &[Snapshot],
    timeline: &Timeline,
    audio_ops: &[AudioOp],
    cfg: &ReelConfig,
    quiet: bool,
) -> Result<Option<Vec<f32>>> {
    if !cfg.audio.active(!audio_ops.is_empty()) {
        return Ok(None);
    }
    let audio = &cfg.audio;

    let keyboard = match audio.keyboard.as_deref() {
        Some("none") => None,
        Some(name) => Some(reel_audio::keyboard_profile(name).ok_or_else(|| {
            anyhow!(
                "unknown keyboard profile `{name}` (available: {}, none)",
                reel_audio::keyboard_profile_names().join(", ")
            )
        })?),
        // Batteries included: audio on implies a keyboard unless opted out.
        None => reel_audio::keyboard_profile("mx-brown"),
    };
    let thinking = match audio.thinking.as_deref() {
        Some("none") => None,
        Some(name) => Some(name.to_string()),
        None => Some("soft-pulse".to_string()),
    };
    let bed = match audio.bed.as_deref() {
        Some("none") | None => None,
        Some(name) => Some(name.to_string()),
    };

    let plan_cfg = reel_audio::PlanConfig {
        keyboard,
        ui_sounds: audio.ui_sounds,
        thinking,
        bed,
    };
    let keys = key_inputs(cast, cast_path);
    let changes = grid_changes(snapshots);
    let plan = reel_audio::plan_events(timeline, audio_ops, &keys, &changes, &plan_cfg)
        .map_err(|e| anyhow!("{e}"))?;
    if !quiet {
        for w in &plan.warnings {
            eprintln!("warning: {w}");
        }
        eprintln!("audio: {} events, keyboard {}", plan.events.len(),
            plan_cfg.keyboard.map(|p| p.name).unwrap_or("none"));
    }
    let samples = reel_audio::mix(&plan.events, timeline.out_duration(), audio.volume);
    Ok(Some(samples))
}

/// Printable keypresses for typing reconstruction, in source time.
fn printable_keys(cast: &Cast, cast_path: &Path) -> Vec<reel_term::KeyPress> {
    let raw: Vec<(f64, String)> = match ReelMeta::load_sidecar(cast_path) {
        Some(meta) if !meta.input_events.is_empty() => meta
            .input_events
            .into_iter()
            .filter(|e| e.kind == "key")
            .map(|e| (e.t, e.value))
            .collect(),
        _ => cast
            .events
            .iter()
            .filter(|e| e.kind == EventKind::Input)
            .map(|e| (e.time, e.data.clone()))
            .collect(),
    };
    raw.iter()
        .flat_map(|(t, v)| {
            v.chars()
                .filter(|c| !c.is_control())
                .map(|ch| reel_term::KeyPress { t: *t, ch })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Keystrokes for the audio planner: the `.reelmeta` sidecar when `reel
/// record` wrote one, else any "i" events an asciinema recording kept.
fn key_inputs(cast: &Cast, cast_path: &Path) -> Vec<reel_audio::KeyInput> {
    if let Some(meta) = ReelMeta::load_sidecar(cast_path) {
        if !meta.input_events.is_empty() {
            return meta
                .input_events
                .iter()
                .filter(|e| e.kind == "key")
                .map(|e| reel_audio::KeyInput {
                    src_time: e.t,
                    kind: reel_audio::KeyKind::from_data(&e.value),
                })
                .collect();
        }
    }
    cast.events
        .iter()
        .filter(|e| e.kind == EventKind::Input)
        .map(|e| reel_audio::KeyInput {
            src_time: e.time,
            kind: reel_audio::KeyKind::from_data(&e.data),
        })
        .collect()
}

/// Per-snapshot change summaries for typing inference and UI cues.
fn grid_changes(snaps: &[Snapshot]) -> Vec<reel_audio::GridChange> {
    snaps
        .windows(2)
        .map(|w| {
            let (a, b) = (&w[0], &w[1]);
            let cols = b.cols as usize;
            let mut changed = 0u32;
            let mut rows_touched = 0u16;
            for row in 0..b.rows as usize {
                let ra = &a.cells[row * cols..(row + 1) * cols];
                let rb = &b.cells[row * cols..(row + 1) * cols];
                let row_changed: u32 = ra.iter().zip(rb).filter(|(x, y)| x != y).count() as u32;
                if row_changed > 0 {
                    rows_touched += 1;
                    changed += row_changed;
                }
            }
            reel_audio::GridChange {
                src_time: b.src_time,
                changed_cells: changed,
                total_cells: (b.cols as u32) * (b.rows as u32),
                rows_touched,
                cursor_advanced: b.cursor.row == a.cursor.row && b.cursor.col > a.cursor.col,
            }
        })
        .collect()
}

/// WebM budget ladder: walk the CQ level (and then fps/scale) down until the
/// file fits, reporting each step like the GIF path does.
///
/// Frames stream straight from the renderer into the encoder — nothing
/// holds the whole video in RAM (a 1080p render used to peak at ~3GB).
fn render_webm(loaded: &Loaded, cfg: ReelConfig, out_path: &Path, quiet: bool) -> Result<()> {
    let budget_bytes = match &cfg.output.budget {
        Some(b) => Some(
            parse_budget(b).ok_or_else(|| anyhow!("invalid budget `{b}` (try 800kb or 2mb)"))?,
        ),
        None => None,
    };
    let audio = build_audio(
        &loaded.cast,
        &loaded.cast_path,
        &loaded.snapshots,
        &loaded.timeline,
        &loaded.audio_ops,
        &cfg,
        quiet,
    )?;

    // (label, cq_level, fps, scale)
    let base_fps = cfg.output.fps.unwrap_or(60);
    let base_scale = cfg.output.scale;
    let ladder: Vec<(String, u32, u32, u32)> = vec![
        ("as configured".into(), 24, base_fps, base_scale),
        ("cq → 34".into(), 34, base_fps, base_scale),
        (format!("scale {base_scale} → 1"), 34, base_fps, 1),
        ("cq → 45, fps → 20".into(), 45, base_fps.min(20), 1),
        ("cq → 55, fps → 15".into(), 55, 15, 1),
    ];

    let mut renderer: Option<Renderer> = None;
    for (i, (label, cq, fps, scale)) in ladder.iter().enumerate() {
        if budget_bytes.is_none() && i > 0 {
            break;
        }
        let mut step_cfg = cfg.clone();
        step_cfg.output.fps = Some(*fps);
        step_cfg.output.scale = *scale;
        let (settings, warns) = settings_from_config(&step_cfg)?;
        let fps_used = settings.fps;
        let blink = settings.cursor_blink;
        // One renderer across rungs keeps the glyph cache warm.
        let font_warns = match renderer.as_mut() {
            Some(r) => r.set_settings(settings)?,
            None => {
                let (r, w) = Renderer::new(settings)?;
                renderer = Some(r);
                w
            }
        };
        if i == 0 {
            print_warnings(&warns, quiet);
            print_warnings(&font_warns, quiet);
        }
        let r = renderer.as_mut().unwrap();
        r.fit_exact(loaded.cast.cols(), loaded.cast.rows());
        let plans = plan_with(&loaded.timeline, &loaded.snapshots, &loaded.visuals, fps_used, blink);
        if i == 0 && !quiet {
            eprintln!(
                "rendering {} frames ({:.1}s output from {:.1}s recording)…",
                plans.len(),
                loaded.timeline.out_duration(),
                loaded.cast.duration()
            );
        }

        let (cw, ch) = r.canvas_size(loaded.cast.cols(), loaded.cast.rows());
        let mut encoder = reel_encode::WebmEncoder::new(
            cw,
            ch,
            &WebmOptions { cq_level: *cq, cfr_fps: Some(*fps), ..Default::default() },
        )?;
        render_each(r, &plans, &loaded.snapshots, |rgba, w, h, dur| {
            encoder.push(rgba, w, h, dur).map_err(Into::into)
        })?;
        let cues: Vec<reel_encode::webm::Cue> = if cfg.output.subtitles {
            caption_cues(loaded)
                .into_iter()
                .map(|(start, end, text)| reel_encode::webm::Cue {
                    start_ms: (start * 1000.0).round() as i64,
                    end_ms: (end * 1000.0).round() as i64,
                    text,
                })
                .collect()
        } else {
            Vec::new()
        };
        let report = encoder.finish_with_cues(audio.as_deref(), &cues)?;
        let size = report.bytes.len() as u64;
        let fits = budget_bytes.map(|b| size <= b).unwrap_or(true);
        if fits || i == ladder.len() - 1 {
            std::fs::write(out_path, &report.bytes)?;
            if !quiet {
                eprintln!(
                    "{}: {} — {} frames, vp9 cq {} (cap {} kbps), {}{}",
                    out_path.display(),
                    human_size(size),
                    report.frames,
                    report.cq_level,
                    report.bitrate_kbps,
                    if report.has_audio { "opus audio" } else { "no audio" },
                    if i > 0 { format!(" (budget ladder: {label})") } else { String::new() }
                );
                if let Some(b) = budget_bytes {
                    if size > b {
                        eprintln!(
                            "warning: could not reach budget {} even at lowest quality",
                            human_size(b)
                        );
                    }
                }
            }
            return Ok(());
        }
        if !quiet {
            eprintln!(
                "budget: {} at {} exceeds {}, degrading ({})…",
                human_size(size),
                label,
                human_size(budget_bytes.unwrap()),
                ladder[i + 1].0
            );
        }
    }
    unreachable!("ladder always writes on its last rung");
}

/// Greedy degradation ladder: try the configured quality first, then walk
/// down predictable steps until the budget fits. Each step is reported so
/// the result isn't a black box.
fn render_gif(loaded: &Loaded, cfg: ReelConfig, out_path: &Path, quiet: bool) -> Result<()> {
    let budget_bytes = match &cfg.output.budget {
        Some(b) => Some(
            parse_budget(b).ok_or_else(|| anyhow!("invalid budget `{b}` (try 800kb or 2mb)"))?,
        ),
        None => None,
    };

    if !quiet {
        if let Ok((settings, _)) = settings_from_config(&cfg) {
            let gradient =
                matches!(settings.template.canvas, reel_render::template::CanvasBg::Linear { .. });
            if gradient || settings.template.crt.is_some() {
                let why = if gradient { "a gradient canvas" } else { "glow effects" };
                eprintln!(
                    "note: template `{}` uses {why}, which pushes GIF output past \
                     the lossless 256-color palette — sizes grow and colors quantize. \
                     A solid-canvas template (minimal, classic, geist) encodes exactly, \
                     and .webm output handles gradients natively.",
                    settings.template.name
                );
            }
        }
    }

    // (label, fps, scale, max_colors)
    let base_fps = cfg.output.fps.unwrap_or(30);
    let base_scale = cfg.output.scale;
    let ladder: Vec<(String, u32, u32, u16)> = vec![
        ("as configured".into(), base_fps, base_scale, 256),
        (format!("fps {} → 20", base_fps), base_fps.min(20), base_scale, 256),
        (format!("scale {} → 1", base_scale), base_fps.min(20), 1, 256),
        ("fps → 15".into(), 15, 1, 256),
        ("palette → 128".into(), 15, 1, 128),
        ("fps → 10, palette → 64".into(), 10, 1, 64),
    ];

    // Two streaming passes per rung (palette scan, then write): twice the
    // render work of keeping every frame, but flat memory — a 1080p render
    // used to hold gigabytes of RGBA. The shared renderer keeps the glyph
    // cache warm, so the second pass is much cheaper than the first.
    let mut renderer: Option<Renderer> = None;
    let mut prev: Option<(u32, u32)> = None;
    for (i, (label, fps, scale, colors)) in ladder.iter().enumerate() {
        if budget_bytes.is_none() && i > 0 {
            break;
        }
        // Skip ladder rungs that don't change anything.
        if prev == Some((*fps, *scale)) && *colors == 256 && i > 0 {
            continue;
        }
        prev = Some((*fps, *scale));

        let mut step_cfg = cfg.clone();
        step_cfg.output.fps = Some(*fps);
        step_cfg.output.scale = *scale;
        let (settings, warns) = settings_from_config(&step_cfg)?;
        let fps_used = settings.fps;
        let blink = settings.cursor_blink;
        let font_warns = match renderer.as_mut() {
            Some(r) => r.set_settings(settings)?,
            None => {
                let (r, w) = Renderer::new(settings)?;
                renderer = Some(r);
                w
            }
        };
        if i == 0 {
            print_warnings(&warns, quiet);
            print_warnings(&font_warns, quiet);
        }
        let r = renderer.as_mut().unwrap();
        r.fit_exact(loaded.cast.cols(), loaded.cast.rows());
        let plans = plan_with(&loaded.timeline, &loaded.snapshots, &loaded.visuals, fps_used, blink);
        if i == 0 && !quiet {
            eprintln!(
                "rendering {} frames ({:.1}s output from {:.1}s recording)…",
                plans.len(),
                loaded.timeline.out_duration(),
                loaded.cast.duration()
            );
        }

        let mut builder = reel_encode::GifPaletteBuilder::new(*colors);
        render_each(r, &plans, &loaded.snapshots, |rgba, _, _, _| {
            builder.feed(rgba);
            Ok(())
        })?;
        let (cw, ch) = r.canvas_size(loaded.cast.cols(), loaded.cast.rows());
        let mut bytes = Vec::new();
        let (frames_written, palette) = {
            let mut stream = reel_encode::GifStream::new(
                &mut bytes,
                cw,
                ch,
                builder.finish(),
                step_cfg.output.looping,
            )?;
            render_each(r, &plans, &loaded.snapshots, |rgba, _, _, dur| {
                stream.push(rgba, dur).map_err(Into::into)
            })?;
            stream.finish()?
        };
        let size = bytes.len() as u64;

        let fits = budget_bytes.map(|b| size <= b).unwrap_or(true);
        if fits || i == ladder.len() - 1 {
            std::fs::write(out_path, &bytes)?;
            if !quiet {
                let palette = match palette {
                    PaletteMode::Exact(n) => format!("exact {n}-color palette (lossless)"),
                    PaletteMode::Quantized(n) => format!("quantized to {n} colors"),
                };
                eprintln!(
                    "{}: {} — {} frames, {}, fps cap {}, scale {}{}",
                    out_path.display(),
                    human_size(size),
                    frames_written,
                    palette,
                    fps,
                    scale,
                    if i > 0 { format!(" (budget ladder: {label})") } else { String::new() }
                );
                if let Some(b) = budget_bytes {
                    if size > b {
                        eprintln!(
                            "warning: could not reach budget {} even at lowest quality",
                            human_size(b)
                        );
                    }
                }
            }
            return Ok(());
        }
        if !quiet {
            eprintln!(
                "budget: {} at {} exceeds {}, degrading ({})…",
                human_size(size),
                label,
                human_size(budget_bytes.unwrap()),
                ladder[i + 1].0
            );
        }
    }
    Err(anyhow!("no ladder step produced output"))
}

pub fn shot(path: &Path, at: &str, out: Option<PathBuf>, template: Option<String>) -> Result<()> {
    let loaded = load(path, template, true)?;
    let t = TimeExpr::parse(at)
        .map_err(|e| anyhow!("--at: {e}"))?
        .resolve(loaded.timeline.out_duration());

    let cfg = loaded.file.config.clone();
    let (settings, _) = settings_from_config(&cfg)?;
    let fps = settings.fps;
    let settings_cursor_blink = settings.cursor_blink;
    let (mut renderer, _) = Renderer::new(settings)?;
    renderer.fit_exact(loaded.cast.cols(), loaded.cast.rows());
    let frames = plan_with(&loaded.timeline, &loaded.snapshots, &loaded.visuals, fps, settings_cursor_blink);
    let frame = frames
        .iter()
        .rev()
        .find(|f| f.out_t <= t + 1e-9)
        .or_else(|| frames.first())
        .ok_or_else(|| anyhow!("no frames"))?;

    let pix = renderer.render_frame(&loaded.snapshots[frame.snapshot], frame);
    let out_path = out.unwrap_or_else(|| path.with_extension("png"));
    std::fs::write(
        &out_path,
        reel_encode::encode_png(pix.width(), pix.height(), &pixmap_to_rgba(&pix))?,
    )?;
    println!("{} (frame at {:.2}s)", out_path.display(), frame.out_t);
    Ok(())
}

pub fn inspect(path: &Path) -> Result<()> {
    let loaded = load(path, None, true)?;
    let cast = &loaded.cast;
    println!("cast      {}", loaded.file.config.source.as_ref().unwrap().cast);
    println!("grid      {}x{}", cast.cols(), cast.rows());
    println!("recorded  {:.2}s, {} events", cast.duration(), cast.events.len());
    println!("snapshots {} visible changes", loaded.snapshots.len());
    println!("output    {:.2}s after edits", loaded.timeline.out_duration());

    println!("\ntimeline:");
    for seg in loaded.timeline.segments() {
        match *seg {
            reel_timeline::Segment::Play { out_start, src_start, src_end, rate } => {
                let speed = if (rate - 1.0).abs() < 1e-9 {
                    String::new()
                } else {
                    format!("  ({rate}x)")
                };
                println!(
                    "  {:>7.2}s  play {:.2}s..{:.2}s{}",
                    out_start, src_start, src_end, speed
                );
            }
            reel_timeline::Segment::Still { out_start, src_at, dur } => {
                println!("  {:>7.2}s  hold {:.2}s for {:.2}s", out_start, src_at, dur);
            }
        }
    }

    let markers: Vec<_> = loaded
        .visuals
        .iter()
        .filter_map(|v| match v {
            VisualOp::Marker { label, at } => Some((label, at)),
            _ => None,
        })
        .collect();
    if !markers.is_empty() {
        println!("\nmarkers:");
        for (label, at) in markers {
            let out_t = loaded.timeline.project_snapped(*at);
            println!("  {:>7.2}s  {} (source {:.2}s)", out_t, label, at);
        }
    }

    let overlays = loaded
        .visuals
        .iter()
        .filter(|v| !matches!(v, VisualOp::Marker { .. }))
        .count();
    if overlays > 0 {
        println!("\noverlays  {overlays} (zoom/pan/caption/highlight)");
    }
    Ok(())
}

/// Watch-mode render: returns the encoded bytes (for the preview server)
/// as well as writing the output file. Reuses cached snapshots when the cast
/// is unchanged and keeps the renderer's glyph cache warm across edits.
pub fn render_for_watch(
    path: &Path,
    out: Option<PathBuf>,
    template: Option<String>,
    cache: &mut WatchCache,
) -> Result<WatchRender> {
    let base_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut file = ReelFile::parse(&text).with_context(|| format!("parsing {}", path.display()))?;
    if let Some(t) = template {
        file.config.template.name = t;
    }

    let cast_path = base_dir.join(&file.config.source.as_ref().unwrap().cast);
    let mtime = std::fs::metadata(&cast_path)
        .and_then(|m| m.modified())
        .with_context(|| format!("stat {}", cast_path.display()))?;

    let reuse = matches!(&cache.cast, Some((p, t, _, _)) if *p == cast_path && *t == mtime);
    if !reuse {
        let cast = Cast::load(&cast_path)
            .with_context(|| format!("loading cast {}", cast_path.display()))?;
        let mut snapshots = reel_term::replay(&cast)?;
        if !uniform_dims(&snapshots) {
            bail!("this cast resizes mid-session, which reel can't render yet");
        }
        reel_term::smooth_typing(&mut snapshots, &printable_keys(&cast, &cast_path));
        cache.cast = Some((cast_path.clone(), mtime, cast, snapshots));
    }
    let (_, _, cast, snapshots) = cache.cast.as_ref().unwrap();

    let program = file.resolve(cast.duration())?;
    let (timeline, _) = Timeline::compile(&program.edits, cast.duration())?;

    let (settings, _) = settings_from_config(&file.config)?;
    let fps = settings.fps;
    let watch_blink = settings.cursor_blink;
    match cache.renderer.as_mut() {
        Some(r) => {
            r.set_settings(settings)?;
        }
        None => cache.renderer = Some(Renderer::new(settings)?.0),
    }
    let renderer = cache.renderer.as_mut().unwrap();

    let frames = plan_with(&timeline, snapshots, &program.visuals, fps, watch_blink);
    let mut rgba = Vec::with_capacity(frames.len());
    for f in &frames {
        let pix = renderer.render_frame(&snapshots[f.snapshot], f);
        rgba.push(RgbaFrame {
            width: pix.width(),
            height: pix.height(),
            data: pixmap_to_rgba(&pix),
            duration_s: f.dur,
        });
    }

    let out_path = out
        .or_else(|| file.config.output.file.as_ref().map(|f| base_dir.join(f)))
        .unwrap_or_else(|| path.with_extension("gif"));
    let extension = out_path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| "gif".into());

    let bytes = match extension.as_str() {
        "png" => {
            let f = rgba.first().ok_or_else(|| anyhow!("no frames"))?;
            reel_encode::encode_png(f.width, f.height, &f.data)?
        }
        "webm" => {
            let audio = build_audio(
                cast,
                &cast_path,
                snapshots,
                &timeline,
                &program.audio,
                &file.config,
                true,
            )?;
            reel_encode::encode_webm(&rgba, audio.as_deref(), &WebmOptions::default())?.bytes
        }
        _ => {
            reel_encode::encode_gif(
                &rgba,
                &GifOptions { looping: file.config.output.looping, max_colors: 256 },
            )?
            .bytes
        }
    };
    std::fs::write(&out_path, &bytes)?;
    Ok(WatchRender { bytes, out_path, cast_path: cast_path.clone(), extension })
}

fn print_warnings(warnings: &[String], quiet: bool) {
    if !quiet {
        for w in warnings {
            eprintln!("warning: {w}");
        }
    }
}

fn done(path: &Path, quiet: bool) -> Result<()> {
    if !quiet {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        eprintln!("{}: {}", path.display(), human_size(size));
    }
    Ok(())
}

pub fn human_size(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.2}MB", bytes as f64 / 1_000_000.0)
    } else {
        format!("{:.1}KB", bytes as f64 / 1_000.0)
    }
}
