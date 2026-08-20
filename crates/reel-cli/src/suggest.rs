//! `reel suggest`: analyze a recording and draft the edit script a human
//! (or their agent) would write — trims, speed-ramps over dead air, and a
//! freeze — ready to paste or write straight into a .reel file.

use anyhow::{anyhow, Context, Result};
use reel_cast::Cast;
use std::path::Path;

/// Source-time gap that counts as dead air worth compressing.
const DEAD_AIR: f64 = 2.5;
/// What a compressed gap should feel like in the output.
const TARGET_GAP: f64 = 1.4;
/// Change smaller than this fraction of the grid is "noise", not activity.
const ACTIVITY_FRACTION: f64 = 0.002;

pub fn suggest(path: &Path, write: Option<&Path>, template: &str) -> Result<()> {
    let cast = Cast::load(path)?;
    let snapshots = reel_term::replay(&cast).map_err(|e| anyhow!("{e}"))?;
    let duration = cast.duration();
    let total_cells = (cast.cols() as u32 * cast.rows() as u32).max(1);

    // Activity timeline: times where a meaningful fraction of cells changed.
    let mut activity: Vec<f64> = Vec::new();
    for w in snapshots.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        let changed = a
            .cells
            .iter()
            .zip(&b.cells)
            .filter(|(x, y)| x != y)
            .count() as f64;
        // Typing echoes are single-cell changes but very much activity.
        let typing = b.cursor.row == a.cursor.row && b.cursor.col > a.cursor.col && changed >= 1.0;
        if typing || changed / total_cells as f64 >= ACTIVITY_FRACTION {
            activity.push(b.src_time);
        }
    }
    if activity.is_empty() {
        return Err(anyhow!("no visible activity in this recording"));
    }

    let mut ops: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    // Lead-in and tail.
    let first = activity.first().copied().unwrap_or(0.0);
    let last = activity.last().copied().unwrap_or(duration);
    let start = if first > 1.0 { (first - 0.5).max(0.0) } else { 0.0 };
    let end = if duration - last > 2.0 { last + 1.5 } else { duration };
    if start > 0.0 || end < duration {
        let end_str = if end < duration { format!("{end:.1}s") } else { "end".into() };
        ops.push(format!("trim    {start:.1}s..{end_str}"));
        notes.push(format!(
            "lead-in and tail: activity spans {first:.1}s–{last:.1}s of {duration:.1}s"
        ));
    }

    // Dead air between activity → speed ramps.
    let mut saved = 0.0;
    for w in activity.windows(2) {
        let gap = w[1] - w[0];
        if gap >= DEAD_AIR {
            let a = (w[0] + 0.4).max(start);
            let b = (w[1] - 0.2).min(end);
            let factor = ((b - a) / TARGET_GAP).clamp(2.0, 12.0).round();
            ops.push(format!("speed   {factor:.0}x from {a:.1}s to {b:.1}s"));
            notes.push(format!("{:.1}s of dead air at {a:.1}s → ~{TARGET_GAP}s", b - a));
            saved += (b - a) * (1.0 - 1.0 / factor);
        }
    }

    ops.push("freeze  last 1.5s".into());

    let out_estimate = (end - start) - saved + 1.5;
    eprintln!(
        "recording {duration:.1}s → suggested output ≈ {out_estimate:.1}s ({} ops)",
        ops.len()
    );
    for n in &notes {
        eprintln!("  · {n}");
    }

    let body = ops.join("\n");
    match write {
        Some(dest) => {
            if dest.exists() {
                return Err(anyhow!("{} already exists", dest.display()));
            }
            let cast_name = path
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            let content = format!(
                "---\n[source]\ncast = \"{cast_name}\"\n\n[template]\nname = \"{template}\"\n\n[output]\nfile = \"demo.gif\"\n---\n\n{body}\n"
            );
            std::fs::write(dest, content)
                .with_context(|| format!("writing {}", dest.display()))?;
            println!("wrote {} — review the ops, then: reel render {}", dest.display(), dest.display());
        }
        None => println!("{body}"),
    }
    Ok(())
}
