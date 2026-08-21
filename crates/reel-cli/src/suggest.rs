//! `reel suggest`: read a recording and draft the edit a human — or their
//! agent — would write.
//!
//! The point is not to be clever. It is to close the gap between "I have a
//! cast" and "I have a demo worth showing", because that gap is where most
//! terminal recordings die. So the draft is deliberately conservative:
//! everything written live into the file is *safe*, meaning a render of the
//! draft is never worse than a render of the raw cast. Anything speculative
//! — cutting a typo, mainly — goes in commented out, with the reason on the
//! line above it, so uncommenting is a one-line decision.
//!
//! What it reads: the grid diff between snapshots (where and how much
//! changed, and when nothing did), the recorded keystrokes (corrections and
//! the trailing `exit`), the cast's own markers, and reel's existing secret
//! scanner.

use crate::json;
use anyhow::{anyhow, Context, Result};
use reel_cast::Cast;
use reel_term::Snapshot;
use std::path::Path;

/// Source-time gap that counts as dead air worth compressing.
const DEAD_AIR: f64 = 2.5;
/// What a compressed gap should feel like in the output.
const TARGET_GAP: f64 = 1.4;
/// Change smaller than this fraction of the grid is "noise", not activity.
const ACTIVITY_FRACTION: f64 = 0.002;
/// Activity closer together than this belongs to the same burst.
const BURST_GAP: f64 = 0.4;
/// Backspaces in a row before it reads as fixing a mistake rather than
/// ordinary editing.
const TYPO_RUN: usize = 3;
/// A zoom has to actually magnify to be worth the frames it costs. Below
/// this, the region already fills the screen and zooming would only crop.
const ZOOM_MIN_FIT: f64 = 1.4;
/// Beyond this much output, GIF stops being the right container.
const GIF_CEILING_S: f64 = 20.0;

// ---------------------------------------------------------------------------
// Analysis
// ---------------------------------------------------------------------------

/// A contiguous run of screen activity: when it happened, how much moved,
/// and the region it moved in.
struct Burst {
    start: f64,
    end: f64,
    changed: u64,
    /// Inclusive cell bounds: (col0, row0, col1, row1).
    bbox: (u16, u16, u16, u16),
}

impl Burst {
    fn cols(&self) -> u16 {
        self.bbox.2 - self.bbox.0 + 1
    }
    fn rows(&self) -> u16 {
        self.bbox.3 - self.bbox.1 + 1
    }
    fn center(&self) -> (u16, u16) {
        ((self.bbox.0 + self.bbox.2) / 2, (self.bbox.1 + self.bbox.3) / 2)
    }
}

/// Everything read off the recording, before any of it becomes an op.
struct Analysis {
    duration: f64,
    cols: f64,
    rows: f64,
    bursts: Vec<Burst>,
    /// Dead air worth compressing: (start, end) on the source clock.
    gaps: Vec<(f64, f64)>,
    /// Backspace corrections: (start, end).
    typos: Vec<(f64, f64)>,
    /// Secrets on screen: (kind, the shortest form reel classifies as one).
    secrets: Vec<(String, String)>,
    markers: Vec<(String, f64)>,
    /// Full-screen repaints rather than a scrolling command line.
    tui: bool,
    /// When the session was closed by typing `exit` (or Ctrl-D).
    exit_at: Option<f64>,
}

impl Analysis {
    fn first_activity(&self) -> f64 {
        self.bursts.first().map(|b| b.start).unwrap_or(0.0)
    }

    fn last_activity(&self) -> f64 {
        self.bursts.last().map(|b| b.end).unwrap_or(self.duration)
    }

    /// The payoff: the biggest burst that arrives *after* a wait. That
    /// pairing — a pause, then a lot of screen — is what a result looks
    /// like, whether it's a test suite finishing or a model answering.
    /// With no dead air anywhere, fall back to the biggest burst in the
    /// back half, which is where a demo's conclusion lives.
    fn payoff(&self) -> Option<&Burst> {
        let after_wait = self
            .bursts
            .iter()
            .filter(|b| self.gaps.iter().any(|(_, ge)| (b.start - ge).abs() < BURST_GAP * 2.0))
            .max_by_key(|b| b.changed);
        after_wait.or_else(|| {
            let half = self.duration / 2.0;
            self.bursts.iter().filter(|b| b.start >= half).max_by_key(|b| b.changed)
        })
    }
}

fn analyze(cast: &Cast, snapshots: &[Snapshot], inputs: &[(f64, String)]) -> Analysis {
    let grid_cells = (cast.cols() as f64 * cast.rows() as f64).max(1.0);
    let cols = cast.cols() as usize;

    // --- Activity, as bursts with a region ------------------------------
    let mut bursts: Vec<Burst> = Vec::new();
    for w in snapshots.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        let mut changed = 0u64;
        let (mut c0, mut r0, mut c1, mut r1) = (u16::MAX, u16::MAX, 0u16, 0u16);
        for (i, (x, y)) in a.cells.iter().zip(&b.cells).enumerate() {
            if x != y {
                changed += 1;
                let (c, r) = ((i % cols) as u16, (i / cols) as u16);
                c0 = c0.min(c);
                r0 = r0.min(r);
                c1 = c1.max(c);
                r1 = r1.max(r);
            }
        }
        if changed == 0 {
            continue;
        }
        // Typing echoes are single-cell changes but very much activity.
        let typing = b.cursor.row == a.cursor.row && b.cursor.col > a.cursor.col;
        if !typing && (changed as f64) / grid_cells < ACTIVITY_FRACTION {
            continue;
        }
        let t = b.src_time;
        match bursts.last_mut() {
            Some(last) if t - last.end < BURST_GAP => {
                last.end = t;
                last.changed += changed;
                last.bbox.0 = last.bbox.0.min(c0);
                last.bbox.1 = last.bbox.1.min(r0);
                last.bbox.2 = last.bbox.2.max(c1);
                last.bbox.3 = last.bbox.3.max(r1);
            }
            _ => bursts.push(Burst { start: t, end: t, changed, bbox: (c0, r0, c1, r1) }),
        }
    }

    // --- Dead air between bursts ----------------------------------------
    let gaps = bursts
        .windows(2)
        .filter(|w| w[1].start - w[0].end >= DEAD_AIR)
        .map(|w| (w[0].end, w[1].start))
        .collect();

    Analysis {
        duration: cast.duration(),
        cols: (cast.cols() as f64).max(1.0),
        rows: (cast.rows() as f64).max(1.0),
        bursts,
        gaps,
        typos: corrections(inputs),
        secrets: reel_term::redact::scan_sensitive(snapshots, 4),
        markers: cast_markers(cast),
        tui: uses_alt_screen(cast),
        exit_at: exit_time(inputs),
    }
}

/// Runs of backspaces in the recorded input, as the span from the first
/// backspace to the moment typing resumes — that whole stretch is the
/// mistake being unwound and retyped.
fn corrections(inputs: &[(f64, String)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let mut run_start: Option<f64> = None;
    let mut run = 0usize;
    for (t, data) in inputs {
        let backspaces = data.chars().filter(|c| *c == '\u{7f}' || *c == '\u{8}').count();
        if backspaces > 0 {
            run_start.get_or_insert(*t);
            run += backspaces;
            continue;
        }
        if let Some(start) = run_start.take() {
            if run >= TYPO_RUN {
                out.push((start, *t));
            }
            run = 0;
        }
    }
    out
}

/// When the shell was closed: `exit` typed, or a bare Ctrl-D.
fn exit_time(inputs: &[(f64, String)]) -> Option<f64> {
    let mut typed = String::new();
    for (t, data) in inputs {
        if data.contains('\u{4}') {
            return Some(*t);
        }
        for ch in data.chars() {
            match ch {
                '\r' | '\n' => {
                    if typed.trim() == "exit" {
                        return Some(*t);
                    }
                    typed.clear();
                }
                '\u{7f}' | '\u{8}' => {
                    typed.pop();
                }
                c if !c.is_control() => typed.push(c),
                _ => {}
            }
        }
    }
    None
}

/// Whether the session ran on the alternate screen buffer. That is the
/// one unambiguous "this is a TUI" signal: vim, lazygit, htop, fzf and
/// agentic TUIs all switch to it, and a scrolling command line never does.
/// Guessing from where cells changed doesn't work — a CLI that scrolls
/// repaints the whole grid too.
fn uses_alt_screen(cast: &Cast) -> bool {
    cast.events
        .iter()
        .filter(|e| e.kind == reel_cast::EventKind::Output)
        .any(|e| {
            e.data.contains("?1049h") || e.data.contains("?1047h") || e.data.contains("?47h")
        })
}

fn cast_markers(cast: &Cast) -> Vec<(String, f64)> {
    cast.events
        .iter()
        .filter(|e| e.kind == reel_cast::EventKind::Marker)
        .enumerate()
        .map(|(i, e)| {
            let label = e.data.trim();
            let label = if label.is_empty() { (i + 1).to_string() } else { label.to_string() };
            (label, e.time)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Drafting
// ---------------------------------------------------------------------------

/// One drafted operation. `applied` is the honest part: a draft that
/// renders worse than the raw cast is worse than no draft, so anything
/// speculative is written commented out with `why` above it.
struct Op {
    kind: &'static str,
    line: String,
    why: String,
    at_s: f64,
    span_s: f64,
    applied: bool,
}

/// The `[template]`/`[output]` half of the draft — chosen from what the
/// recording turned out to be, with the reasoning recorded alongside.
struct Recommendation {
    template: &'static str,
    format: &'static str,
    budget: Option<&'static str>,
    why: Vec<String>,
}

/// Escapes a literal so it can go in a `redact` op, which takes a regex.
fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The `redact` argument for one finding.
///
/// It can't be the literal reel's scanner handed over. That literal is the
/// *first* form that matched, and a secret being typed matches early: the
/// scanner sees `sk-live-9fA` several frames before the full
/// `sk-live-9fA3kQ2mZ7xB1nR4` exists. Redacting the literal would mask
/// eleven characters and leave the rest of the key on screen.
///
/// So anchor on that first form and let it run: every longer state the
/// token passes through is covered, which is exactly the set of frames reel
/// itself would flag. Shorter prefixes are left alone deliberately — reel
/// doesn't call them secrets either, and a greedier pattern starts eating
/// ordinary words.
fn redact_pattern(sample: &str) -> String {
    // Characters a key, token, URL or address can continue with.
    format!("{}[A-Za-z0-9_.:/@%+~-]*", regex_escape(sample))
}

/// Renders a source time as `@marker` when one sits on it. An edit written
/// in names survives retiming — and the user pressed Ctrl+] there on
/// purpose, which is the strongest signal in the whole recording.
fn at(a: &Analysis, t: f64) -> String {
    match a.markers.iter().find(|(_, m)| (m - t).abs() < 0.5) {
        Some((label, _)) => format!("@{label}"),
        None => format!("{t:.1}s"),
    }
}

fn draft(a: &Analysis) -> (Vec<Op>, Recommendation, f64) {
    let mut ops: Vec<Op> = Vec::new();

    // --- Secrets first: they must not survive into any render -----------
    for (kind, literal) in &a.secrets {
        // A quote would end the op's string argument; leave those to a
        // human rather than emitting something that won't parse.
        if literal.contains('"') {
            continue;
        }
        let article = if kind.starts_with(['a', 'e', 'i', 'o', 'u', 'A', 'E', 'I', 'O', 'U']) {
            "an"
        } else {
            "a"
        };
        ops.push(Op {
            kind: "redact",
            line: format!("redact  \"{}\"", redact_pattern(literal)),
            why: format!(
                "{article} {kind} is on screen — masked from the first character \
                 reel recognises, so check any frames where it was still being typed"
            ),
            at_s: 0.0,
            span_s: a.duration,
            applied: true,
        });
    }

    // --- Trim the lead-in and whatever trails the last thing worth seeing
    let first = a.first_activity();
    let last = a.last_activity();
    let start = if first > 1.0 { (first - 0.5).max(0.0) } else { 0.0 };
    // A typed `exit` ends the session; it isn't part of the demo.
    let (end, closed) = match a.exit_at {
        Some(t) if t > start + 1.0 => ((t - 0.3).min(a.duration), true),
        _ if a.duration - last > 2.0 => ((last + 1.5).min(a.duration), false),
        _ => (a.duration, false),
    };
    if start > 0.0 || end < a.duration {
        let end_str = if end < a.duration { at(a, end) } else { "end".into() };
        let why = if closed {
            format!("activity starts at {first:.1}s; the shell was closed at the end")
        } else {
            format!("activity spans {first:.1}s–{last:.1}s of {:.1}s", a.duration)
        };
        ops.push(Op {
            kind: "trim",
            line: format!("trim    {}..{}", at(a, start), end_str),
            why,
            at_s: start,
            span_s: end - start,
            applied: true,
        });
    }

    // --- Compress the waiting -------------------------------------------
    let mut saved = 0.0;
    for (gs, ge) in &a.gaps {
        let s = (gs + 0.4).max(start);
        let e = (ge - 0.2).min(end);
        if e <= s {
            continue;
        }
        let factor = ((e - s) / TARGET_GAP).clamp(2.0, 12.0).round();
        ops.push(Op {
            kind: "speed",
            line: format!("speed   {factor:.0}x from {} to {}", at(a, s), at(a, e)),
            why: format!("{:.1}s of dead air → ~{TARGET_GAP}s", e - s),
            at_s: s,
            span_s: e - s,
            applied: true,
        });
        saved += (e - s) * (1.0 - 1.0 / factor);
    }

    // --- The payoff ------------------------------------------------------
    if let Some(b) = a.payoff().filter(|b| b.start > start && b.start < end) {
        ops.push(Op {
            kind: "marker",
            line: format!("marker  \"payoff\" at {:.1}s", b.start),
            why: format!(
                "the biggest burst of output ({} cells) — reference it as @payoff",
                b.changed
            ),
            at_s: b.start,
            span_s: b.end - b.start,
            applied: true,
        });

        // Zoom only when the result is a compact region, and only as far
        // as keeps all of it on screen. The binding constraint is per axis,
        // not area: a 62-column band is 10% of an 80x24 grid by area but
        // can't survive even a 1.3x zoom without losing its ends.
        let fit = (a.cols / b.cols() as f64).min(a.rows / b.rows() as f64);
        if fit >= ZOOM_MIN_FIT && b.cols() >= 4 && b.rows() >= 2 {
            let (cx, cy) = b.center();
            // Back off from the exact fit so the region isn't flush against
            // the frame edge.
            let factor = (fit * 0.9).clamp(1.3, 2.2);
            let zs = (b.start - 0.3).max(start);
            let ze = (b.end + 2.0).min(end);
            if ze > zs {
                ops.push(Op {
                    kind: "zoom",
                    line: format!(
                        "zoom    {factor:.1}x at ({cx},{cy}) from {} to {}",
                        at(a, zs),
                        at(a, ze)
                    ),
                    why: format!("the result lands in a {}x{} region — magnify it", b.cols(), b.rows()),
                    at_s: zs,
                    span_s: ze - zs,
                    applied: true,
                });
            }
        }
    }

    // --- Corrections: real, but risky to cut automatically ---------------
    for (ts, te) in &a.typos {
        if *ts < start || *te > end {
            continue;
        }
        ops.push(Op {
            kind: "cut",
            line: format!("cut     {}..{}", at(a, *ts), at(a, *te)),
            why: format!(
                "a {:.1}s correction (backspaces) — check the seam before keeping this",
                te - ts
            ),
            at_s: *ts,
            span_s: te - ts,
            // Deliberately not live: a cut landing mid-word looks worse
            // than the typo it removes, and only an eye can tell.
            applied: false,
        });
    }

    ops.push(Op {
        kind: "freeze",
        line: "freeze  last 1.5s".into(),
        why: "a resting point before a looping GIF restarts".into(),
        at_s: end,
        span_s: 1.5,
        applied: true,
    });

    let estimate = (end - start) - saved + 1.5;
    (ops, recommend(a, estimate), estimate)
}

fn recommend(a: &Analysis, estimate: f64) -> Recommendation {
    let mut why = Vec::new();
    let template = if a.tui {
        why.push("full-screen repaints: a chrome-less template keeps the TUI the subject".into());
        "classic"
    } else {
        why.push("a scrolling command line: the decorated default suits it".into());
        "glass"
    };
    let (format, budget) = if estimate > GIF_CEILING_S {
        why.push(format!(
            "≈{estimate:.0}s of output is past what GIF encodes well — WebM also carries sound"
        ));
        ("webm", None)
    } else {
        why.push("short enough for a GIF; the budget keeps it embeddable in a README".into());
        ("gif", Some("1mb"))
    };
    Recommendation { template, format, budget, why }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

pub fn suggest(
    path: &Path,
    write: Option<&Path>,
    template: Option<&str>,
    inputs: &[(f64, String)],
) -> Result<()> {
    let cast = Cast::load(path)?;
    let snapshots = reel_term::replay(&cast).map_err(|e| anyhow!("{e}"))?;
    let a = analyze(&cast, &snapshots, inputs);
    if a.bursts.is_empty() {
        return Err(anyhow!("no visible activity in this recording"));
    }
    let (ops, rec, estimate) = draft(&a);
    let template = template.unwrap_or(rec.template);

    let body = script(&ops);
    let written = match write {
        Some(dest) => {
            if dest.exists() {
                return Err(anyhow!("{} already exists", dest.display()));
            }
            let cast_name = path
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            std::fs::write(dest, reel_file(&cast_name, template, &rec, &body))
                .with_context(|| format!("writing {}", dest.display()))?;
            Some(dest)
        }
        None => None,
    };

    if json::on() {
        return json::emit(serde_json::json!({
            "cast": path.display().to_string(),
            "source": {
                "duration_s": a.duration,
                "cols": cast.cols(),
                "rows": cast.rows(),
                "activity_start_s": a.first_activity(),
                "activity_end_s": a.last_activity(),
                "tui": a.tui,
                "closed_with_exit": a.exit_at.is_some(),
                "markers": a.markers.iter()
                    .map(|(l, t)| serde_json::json!({ "label": l, "src_t_s": t }))
                    .collect::<Vec<_>>(),
            },
            "estimate_s": estimate,
            "recommend": {
                "template": template,
                "format": rec.format,
                "budget": rec.budget,
                "why": rec.why,
            },
            "ops": ops.iter().map(|o| serde_json::json!({
                "kind": o.kind,
                "op": o.line,
                "why": o.why,
                "at_s": o.at_s,
                "span_s": o.span_s,
                // false = written commented out; it needs an eye on the
                // render before it belongs in the edit.
                "applied": o.applied,
            })).collect::<Vec<_>>(),
            "written": written.map(|d| d.display().to_string()),
            "script": body,
        }));
    }

    let review = ops.iter().filter(|o| !o.applied).count();
    eprintln!(
        "recording {:.1}s → suggested output ≈ {estimate:.1}s ({} ops{})",
        a.duration,
        ops.iter().filter(|o| o.applied).count(),
        if review > 0 { format!(", {review} to review") } else { String::new() },
    );
    for o in &ops {
        eprintln!("  {} {}", if o.applied { "·" } else { "?" }, o.why);
    }
    for w in &rec.why {
        eprintln!("  → {w}");
    }
    match written {
        Some(dest) => println!(
            "wrote {} — review the ops, then: reel render {}",
            dest.display(),
            dest.display()
        ),
        None => println!("{body}"),
    }
    Ok(())
}

/// The edit script: live ops as themselves, speculative ones commented out
/// under their reason.
fn script(ops: &[Op]) -> String {
    let mut out = String::new();
    for o in ops {
        if o.applied {
            out.push_str(&o.line);
        } else {
            out.push_str(&format!("# {}\n# {}", o.why, o.line));
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}

fn reel_file(cast: &str, template: &str, rec: &Recommendation, body: &str) -> String {
    let budget = match rec.budget {
        Some(b) => format!("budget = \"{b}\"\n"),
        None => String::new(),
    };
    let why: String = rec.why.iter().map(|w| format!("# {w}\n")).collect();
    format!(
        "---\n[source]\ncast = \"{cast}\"\n\n[template]\nname = \"{template}\"\n\n\
         [output]\nfile = \"demo.{}\"\n{budget}---\n\n\
         # Drafted by `reel suggest` — a starting point, not a finished edit.\n\
         {why}\n{body}\n",
        rec.format
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backspace_runs_read_as_corrections() {
        let inputs = vec![
            (1.0, "cargo tets".to_string()),
            (2.0, "\u{7f}\u{7f}\u{7f}".to_string()),
            (2.6, "est".to_string()),
        ];
        assert_eq!(corrections(&inputs), vec![(2.0, 2.6)]);
        // One backspace is ordinary typing, not a mistake worth an op.
        let light = vec![(1.0, "ls".into()), (2.0, "\u{7f}".into()), (2.2, "s".into())];
        assert!(corrections(&light).is_empty());
        // Backspaces spread over several events still add up to a run.
        let spread = vec![
            (1.0, "abc".into()),
            (2.0, "\u{7f}".into()),
            (2.1, "\u{7f}".into()),
            (2.2, "\u{7f}".into()),
            (3.0, "xyz".into()),
        ];
        assert_eq!(corrections(&spread), vec![(2.0, 3.0)]);
    }

    #[test]
    fn exit_is_found_however_it_was_typed() {
        assert_eq!(exit_time(&[(1.0, "exit\r".into())]), Some(1.0));
        assert_eq!(exit_time(&[(1.0, "exit".into()), (1.5, "\r".into())]), Some(1.5));
        assert_eq!(exit_time(&[(3.0, "\u{4}".into())]), Some(3.0));
        // A command that merely contains the word is not the shell closing.
        assert_eq!(exit_time(&[(1.0, "grep exit log\r".into())]), None);
        // Backspaced away before Enter.
        assert_eq!(exit_time(&[(1.0, "exitt\u{7f}\u{7f}\r".into())]), None);
        assert_eq!(exit_time(&[]), None);
    }

    #[test]
    fn redact_arguments_are_regex_safe() {
        assert_eq!(regex_escape("sk-live-a1.b2"), "sk-live-a1\\.b2");
        assert_eq!(regex_escape("a+b(c)"), "a\\+b\\(c\\)");
        assert_eq!(regex_escape("plain"), "plain");
    }

    #[test]
    fn redact_covers_the_whole_token_not_just_what_matched_first() {
        let pat = redact_pattern("sk-live-9fA");
        let re = regex_lite::Regex::new(&pat).unwrap();
        // Every state the key grows through, including the finished one.
        for s in ["sk-live-9fA", "sk-live-9fA3kQ", "sk-live-9fA3kQ2mZ7xB1nR4"] {
            let m = re.find(s).expect("no match for {s}");
            assert_eq!(m.as_str(), s, "did not cover all of {s}");
        }
        // And it stops at whitespace rather than eating the rest of the line.
        let line = "export KEY=sk-live-9fA3kQ2mZ7xB1nR4 && echo done";
        assert_eq!(re.find(line).unwrap().as_str(), "sk-live-9fA3kQ2mZ7xB1nR4");
    }

    fn op(kind: &'static str, line: &str, applied: bool) -> Op {
        Op {
            kind,
            line: line.into(),
            why: format!("because of {kind}"),
            at_s: 0.0,
            span_s: 1.0,
            applied,
        }
    }

    #[test]
    fn speculative_ops_are_written_commented_out() {
        let s = script(&[
            op("trim", "trim    0.5s..end", true),
            op("cut", "cut     2.0s..2.6s", false),
            op("freeze", "freeze  last 1.5s", true),
        ]);
        assert!(s.contains("# because of cut\n# cut     2.0s..2.6s"));
        // Nothing risky may reach the render uncommented.
        for line in s.lines() {
            if line.contains("cut ") {
                assert!(line.starts_with('#'), "cut leaked in live: {line}");
            }
        }
        // Live ops stay exactly as written.
        assert!(s.lines().any(|l| l == "trim    0.5s..end"));
        assert!(s.lines().any(|l| l == "freeze  last 1.5s"));
    }

    #[test]
    fn a_marker_on_the_moment_names_it() {
        let a = Analysis {
            duration: 20.0,
            cols: 80.0,
            rows: 24.0,
            bursts: Vec::new(),
            gaps: Vec::new(),
            typos: Vec::new(),
            secrets: Vec::new(),
            markers: vec![("done".into(), 12.0)],
            tui: false,
            exit_at: None,
        };
        assert_eq!(at(&a, 12.2), "@done");
        assert_eq!(at(&a, 15.0), "15.0s");
    }
}
