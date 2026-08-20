//! Audio event planning: derive an event list in *output time* from the
//! resolved timeline plus whatever evidence of activity exists — recorded
//! input events when we have them, grid-diff inference when we don't.
//!
//! Layer order per the spec (§8.2): keyboard, UI response cues, the
//! agent-thinking bed, and explicit `sound` ops from the edit script.

use crate::recipes::{self, KeyboardProfile, Recipe};
use crate::{dsp::Pcg32, AudioError};
use reel_timeline::{AudioOp, Timeline};

/// One scheduled sound: recipe + when (output seconds) + per-event shaping.
#[derive(Debug, Clone)]
pub struct AudioEvent {
    pub t: f64,
    pub name: String,
    pub recipe: Recipe,
    pub gain: f32,
    pub pitch: f32,
}

/// What changed between two consecutive grid snapshots — the caller computes
/// this from `reel-term` snapshots so this crate stays render-agnostic.
#[derive(Debug, Clone, Copy)]
pub struct GridChange {
    pub src_time: f64,
    pub changed_cells: u32,
    pub total_cells: u32,
    pub rows_touched: u16,
    /// The cursor advanced within the same row — the classic typing echo.
    pub cursor_advanced: bool,
}

/// A recorded (or inferred) keypress in source time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Char,
    Enter,
    Space,
    Backspace,
    Other,
}

impl KeyKind {
    /// Classifies raw input bytes (a cast "i" event or sidecar value).
    pub fn from_data(data: &str) -> KeyKind {
        match data {
            "\r" | "\n" | "\r\n" => KeyKind::Enter,
            " " => KeyKind::Space,
            "\x7f" | "\x08" => KeyKind::Backspace,
            d if d.chars().count() == 1 && !d.chars().next().is_some_and(char::is_control) => {
                KeyKind::Char
            }
            _ => KeyKind::Other,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct KeyInput {
    pub src_time: f64,
    pub kind: KeyKind,
}

/// Planner configuration, mapped from the `.reel` `[audio]` table by the CLI.
#[derive(Debug, Clone)]
pub struct PlanConfig {
    pub keyboard: Option<KeyboardProfile>,
    pub ui_sounds: bool,
    /// Recipe for the thinking-bed pulse (e.g. "soft-pulse"); `None` disables.
    pub thinking: Option<String>,
    /// Ambient bed recipe looped softly under the whole demo; default off.
    pub bed: Option<String>,
}

#[derive(Debug)]
pub struct AudioPlan {
    pub events: Vec<AudioEvent>,
    pub warnings: Vec<String>,
}

/// Minimum output-time gap between keyboard events. Speeding a region up
/// compresses events together; past this density we *drop* keys rather than
/// let them smear into noise (spec §8.1).
const KEY_MIN_GAP: f64 = 0.03;
/// Keystroke sounds snap forward to the grid change they caused, when one
/// lands within this window. TUIs repaint on their own tick, often well
/// after the key event — and the viewer can only sync sound to what they
/// *see*. Keys whose echo never shows up keep their input timestamp.
const KEY_ALIGN_WINDOW: f64 = 0.35;
/// Minimum output-time gap between UI response cues.
const UI_MIN_GAP: f64 = 0.8;
/// Source-idle threshold that makes a grid change a "response" cue.
const UI_IDLE_BEFORE: f64 = 0.8;
/// Fraction of cells that must change to count as a UI response.
const UI_CHANGE_FRACTION: f64 = 0.12;
/// Source-time silence that counts as "the agent is thinking".
const THINKING_MIN_SRC_GAP: f64 = 3.0;
/// Output seconds between thinking-bed pulses.
const THINKING_PULSE_EVERY: f64 = 1.1;

pub fn plan_events(
    timeline: &Timeline,
    ops: &[AudioOp],
    keys: &[KeyInput],
    changes: &[GridChange],
    cfg: &PlanConfig,
) -> Result<AudioPlan, AudioError> {
    let mut warnings = Vec::new();
    let mut events: Vec<AudioEvent> = Vec::new();

    let mutes: Vec<(f64, f64)> = ops
        .iter()
        .filter_map(|op| match op {
            AudioOp::Mute { range } => Some(*range),
            _ => None,
        })
        .collect();
    let volumes: Vec<(f64, f64, f64)> = ops
        .iter()
        .filter_map(|op| match op {
            AudioOp::Volume { level, range } => Some((range.0, range.1, *level)),
            _ => None,
        })
        .collect();
    let muted = |src_t: f64| mutes.iter().any(|&(a, b)| src_t >= a && src_t <= b);
    let volume_at = |src_t: f64| -> f32 {
        volumes
            .iter()
            .find(|&&(a, b, _)| src_t >= a && src_t <= b)
            .map(|&(_, _, level)| level as f32)
            .unwrap_or(1.0)
    };

    // --- 1. Keyboard --------------------------------------------------------
    if let Some(profile) = cfg.keyboard {
        let inferred;
        let key_list: &[KeyInput] = if keys.is_empty() {
            inferred = infer_keys(changes);
            &inferred
        } else {
            keys
        };
        let mut rng = Pcg32::new(0x005e_ed4b_0a4d);
        let mut last_t = f64::NEG_INFINITY;
        for key in key_list {
            // Humanization is drawn per source event *before* any drop, so
            // editing the timeline doesn't reshuffle surviving keys' sounds.
            let hp = rng.range(-0.06, 0.06);
            let hg = rng.range(-0.18, 0.18);
            if key.kind == KeyKind::Other || muted(key.src_time) {
                continue;
            }
            // Sync to the repaint this key caused, not the raw input time.
            let src_t = changes
                .iter()
                .find(|c| {
                    c.src_time >= key.src_time - 0.02
                        && c.src_time <= key.src_time + KEY_ALIGN_WINDOW
                })
                .map(|c| c.src_time)
                .unwrap_or(key.src_time);
            let Some(t) = timeline.project(src_t) else {
                continue; // cut away
            };
            if t - last_t < KEY_MIN_GAP {
                continue; // sped-up region: drop, don't smear
            }
            last_t = t;

            let (pitch_k, gain_k) = match key.kind {
                KeyKind::Enter => (0.8, 1.15),
                KeyKind::Space => (0.7, 1.0),
                KeyKind::Backspace => (1.1, 0.9),
                _ => (1.0, 1.0),
            };
            let vol = volume_at(key.src_time);
            let pitch = profile.press_pitch * pitch_k * (1.0 + hp);
            let gain = profile.gain * gain_k * (1.0 + hg) * vol;
            push(&mut events, t, "press", pitch, gain)?;
            if profile.click {
                push(&mut events, t, "tick", pitch * 0.9, gain * 0.5)?;
            }
            if profile.release {
                push(
                    &mut events,
                    t + 0.055,
                    "release",
                    profile.release_pitch * pitch_k * (1.0 + hp),
                    gain * 0.6,
                )?;
            }
        }
    }

    // --- 2. UI response cues ------------------------------------------------
    if cfg.ui_sounds {
        let mut last_cue = f64::NEG_INFINITY;
        let mut prev_change_t = 0.0f64;
        for c in changes {
            let idle = c.src_time - prev_change_t;
            prev_change_t = c.src_time;
            let fraction = c.changed_cells as f64 / c.total_cells.max(1) as f64;
            if idle < UI_IDLE_BEFORE || fraction < UI_CHANGE_FRACTION || muted(c.src_time) {
                continue;
            }
            let Some(t) = timeline.project(c.src_time) else {
                continue;
            };
            if t - last_cue < UI_MIN_GAP {
                continue;
            }
            last_cue = t;
            push(&mut events, t, "pulse", 1.0, 0.35 * volume_at(c.src_time))?;
        }
    }

    // --- 3. Agent-thinking bed ----------------------------------------------
    if let Some(bed) = &cfg.thinking {
        if recipes::recipe(bed).is_none() {
            return Err(AudioError::UnknownSound(
                bed.clone(),
                recipes::recipe_names().join(", "),
            ));
        }
        // Only gaps *between* changes count: a trailing still stretch never
        // "resolves", so it gets no bed (and is usually trimmed or frozen).
        let mut gap_start = 0.0f64;
        for c in changes {
            let gap_end = c.src_time;
            if gap_end - gap_start >= THINKING_MIN_SRC_GAP {
                let a = timeline.project_snapped(gap_start);
                let b = timeline.project_snapped(gap_end);
                // Only score gaps that survive editing as a visible pause.
                if b - a >= 1.2 {
                    let mut t = a + 0.4;
                    while t < b - 0.3 {
                        if !muted(timeline.sample(t)) {
                            push(&mut events, t, bed, 1.0, 0.5 * volume_at(timeline.sample(t)))?;
                        }
                        t += THINKING_PULSE_EVERY;
                    }
                    // Resolve to a chime when output resumes.
                    if !muted(gap_end) {
                        push(&mut events, b, "chime", 1.0, 0.4 * volume_at(gap_end))?;
                    }
                }
            }
            gap_start = gap_end;
        }
    }

    // --- 4. Ambient bed -------------------------------------------------------
    if let Some(bed) = &cfg.bed {
        if recipes::recipe(bed).is_none() {
            return Err(AudioError::UnknownSound(
                bed.clone(),
                recipes::recipe_names().join(", "),
            ));
        }
        let mut t = 0.0;
        while t < timeline.out_duration() {
            if !muted(timeline.sample(t)) {
                push(&mut events, t, bed, 1.0, 0.25 * volume_at(timeline.sample(t)))?;
            }
            t += 2.0;
        }
    }

    // --- 5. Explicit `sound` ops ---------------------------------------------
    for op in ops {
        if let AudioOp::Sound { name, at } = op {
            if recipes::recipe(name).is_none() {
                return Err(AudioError::UnknownSound(
                    name.clone(),
                    recipes::recipe_names().join(", "),
                ));
            }
            // The user asked for this one — snap through cuts, ignore mutes.
            let t = timeline.project_snapped(*at);
            push(&mut events, t, name, 1.0, volume_at(*at))?;
        }
    }

    events.sort_by(|a, b| a.t.total_cmp(&b.t));
    if events.is_empty() {
        warnings.push(
            "audio is enabled but produced no events — no input activity detected and no `sound` ops"
                .into(),
        );
    }
    Ok(AudioPlan { events, warnings })
}

fn push(
    events: &mut Vec<AudioEvent>,
    t: f64,
    name: &str,
    pitch: f32,
    gain: f32,
) -> Result<(), AudioError> {
    let recipe = recipes::recipe(name).ok_or_else(|| {
        AudioError::UnknownSound(name.to_string(), recipes::recipe_names().join(", "))
    })?;
    events.push(AudioEvent { t, name: name.to_string(), recipe, gain, pitch });
    Ok(())
}

/// Typing inference for plain casts (no recorded input): printable characters
/// appearing one at a time at the cursor read as keystrokes; block updates
/// don't (spec §8.2).
fn infer_keys(changes: &[GridChange]) -> Vec<KeyInput> {
    changes
        .iter()
        .filter(|c| c.cursor_advanced && c.rows_touched <= 1 && c.changed_cells <= 3)
        .map(|c| KeyInput { src_time: c.src_time, kind: KeyKind::Char })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reel_timeline::EditOps;

    fn timeline(ops: EditOps, dur: f64) -> Timeline {
        Timeline::compile(&ops, dur).unwrap().0
    }

    fn cfg_keyboard() -> PlanConfig {
        PlanConfig {
            keyboard: recipes::keyboard_profile("mx-brown"),
            ui_sounds: false,
            thinking: None,
            bed: None,
        }
    }

    fn key(t: f64) -> KeyInput {
        KeyInput { src_time: t, kind: KeyKind::Char }
    }

    #[test]
    fn keys_project_into_output_time() {
        let tl = timeline(EditOps { trim: Some((2.0, 10.0)), ..Default::default() }, 10.0);
        let plan = plan_events(&tl, &[], &[key(3.0)], &[], &cfg_keyboard()).unwrap();
        // press + release for one key.
        assert_eq!(plan.events.len(), 2);
        assert!((plan.events[0].t - 1.0).abs() < 1e-9);
        assert_eq!(plan.events[0].name, "press");
        assert_eq!(plan.events[1].name, "release");
    }

    #[test]
    fn keys_in_cuts_are_dropped() {
        let tl = timeline(EditOps { cuts: vec![(2.0, 4.0)], ..Default::default() }, 10.0);
        let plan = plan_events(&tl, &[], &[key(3.0), key(5.0)], &[], &cfg_keyboard()).unwrap();
        assert_eq!(plan.events.iter().filter(|e| e.name == "press").count(), 1);
    }

    #[test]
    fn speed_thins_keys_instead_of_smearing() {
        let tl = timeline(EditOps { speeds: vec![(10.0, 0.0, 10.0)], ..Default::default() }, 10.0);
        // 100 keys in 10s source → 1s output. At most ~1/KEY_MIN_GAP survive.
        let keys: Vec<KeyInput> = (0..100).map(|i| key(i as f64 * 0.1)).collect();
        let plan = plan_events(&tl, &[], &keys, &[], &cfg_keyboard()).unwrap();
        let presses = plan.events.iter().filter(|e| e.name == "press").count();
        assert!(presses <= 34, "expected thinning, got {presses} presses");
        assert!(presses >= 10, "over-thinned: {presses} presses");
    }

    #[test]
    fn humanization_varies_but_is_deterministic() {
        let tl = timeline(EditOps::default(), 10.0);
        let keys = [key(1.0), key(2.0)];
        let a = plan_events(&tl, &[], &keys, &[], &cfg_keyboard()).unwrap();
        let b = plan_events(&tl, &[], &keys, &[], &cfg_keyboard()).unwrap();
        assert!((a.events[0].pitch - b.events[0].pitch).abs() < 1e-9);
        assert_ne!(a.events[0].pitch, a.events[2].pitch, "two keys share a pitch");
    }

    #[test]
    fn key_sounds_snap_to_the_repaint_they_caused() {
        let tl = timeline(EditOps::default(), 10.0);
        // Key at 1.0; the TUI paints the echo at 1.19.
        let changes = [GridChange {
            src_time: 1.19,
            changed_cells: 1,
            total_cells: 800,
            rows_touched: 1,
            cursor_advanced: true,
        }];
        let plan = plan_events(&tl, &[], &[key(1.0)], &changes, &cfg_keyboard()).unwrap();
        let press = plan.events.iter().find(|e| e.name == "press").unwrap();
        assert!((press.t - 1.19).abs() < 1e-9, "press at {}", press.t);
    }

    #[test]
    fn keys_without_a_nearby_repaint_keep_their_time() {
        let tl = timeline(EditOps::default(), 10.0);
        // Only a change far outside the alignment window.
        let changes = [GridChange {
            src_time: 3.0,
            changed_cells: 500,
            total_cells: 800,
            rows_touched: 10,
            cursor_advanced: false,
        }];
        let plan = plan_events(&tl, &[], &[key(1.0)], &changes, &cfg_keyboard()).unwrap();
        let press = plan.events.iter().find(|e| e.name == "press").unwrap();
        assert!((press.t - 1.0).abs() < 1e-9);
    }

    #[test]
    fn ui_cue_fires_after_idle_then_big_change() {
        let tl = timeline(EditOps::default(), 10.0);
        let changes = [
            GridChange { src_time: 0.5, changed_cells: 200, total_cells: 1000, rows_touched: 10, cursor_advanced: false },
            // Big change but no idle before it: not a cue.
            GridChange { src_time: 0.6, changed_cells: 300, total_cells: 1000, rows_touched: 10, cursor_advanced: false },
            // Idle then big change: cue.
            GridChange { src_time: 5.0, changed_cells: 300, total_cells: 1000, rows_touched: 10, cursor_advanced: false },
        ];
        let cfg = PlanConfig { keyboard: None, ui_sounds: true, thinking: None, bed: None };
        let plan = plan_events(&tl, &[], &[], &changes, &cfg).unwrap();
        assert_eq!(plan.events.len(), 1);
        assert_eq!(plan.events[0].name, "pulse");
        assert!((plan.events[0].t - 5.0).abs() < 1e-9);
    }

    #[test]
    fn thinking_bed_fills_long_gaps_and_resolves() {
        let tl = timeline(EditOps::default(), 20.0);
        let changes = [
            GridChange { src_time: 1.0, changed_cells: 10, total_cells: 1000, rows_touched: 1, cursor_advanced: false },
            GridChange { src_time: 11.0, changed_cells: 10, total_cells: 1000, rows_touched: 1, cursor_advanced: false },
        ];
        let cfg = PlanConfig { keyboard: None, ui_sounds: false, thinking: Some("soft-pulse".into()), bed: None };
        let plan = plan_events(&tl, &[], &[], &changes, &cfg).unwrap();
        let pulses = plan.events.iter().filter(|e| e.name == "soft-pulse").count();
        assert!(pulses >= 5, "10s gap should pulse repeatedly, got {pulses}");
        let chime = plan.events.iter().find(|e| e.name == "chime").expect("resolve chime");
        assert!((chime.t - 11.0).abs() < 1e-6);
    }

    #[test]
    fn thinking_bed_respects_speed_compression() {
        // Same 10s gap, but sped 5x: fewer pulses in the 2s that remain.
        let tl = timeline(EditOps { speeds: vec![(5.0, 1.0, 11.0)], ..Default::default() }, 20.0);
        let changes = [
            GridChange { src_time: 1.0, changed_cells: 10, total_cells: 1000, rows_touched: 1, cursor_advanced: false },
            GridChange { src_time: 11.0, changed_cells: 10, total_cells: 1000, rows_touched: 1, cursor_advanced: false },
        ];
        let cfg = PlanConfig { keyboard: None, ui_sounds: false, thinking: Some("soft-pulse".into()), bed: None };
        let plan = plan_events(&tl, &[], &[], &changes, &cfg).unwrap();
        let pulses = plan.events.iter().filter(|e| e.name == "soft-pulse").count();
        assert!((1..=2).contains(&pulses), "2s output gap → 1-2 pulses, got {pulses}");
    }

    #[test]
    fn explicit_sound_snaps_through_cuts() {
        let tl = timeline(EditOps { cuts: vec![(2.0, 4.0)], ..Default::default() }, 10.0);
        let ops = [AudioOp::Sound { name: "success".into(), at: 3.0 }];
        let plan = plan_events(&tl, &ops, &[], &[], &cfg_keyboard()).unwrap();
        let s = plan.events.iter().find(|e| e.name == "success").unwrap();
        assert!((s.t - 2.0).abs() < 1e-9, "snapped to the seam");
    }

    #[test]
    fn unknown_sound_errors() {
        let tl = timeline(EditOps::default(), 10.0);
        let ops = [AudioOp::Sound { name: "airhorn".into(), at: 1.0 }];
        let err = plan_events(&tl, &ops, &[], &[], &cfg_keyboard()).unwrap_err();
        assert!(matches!(err, AudioError::UnknownSound(..)));
    }

    #[test]
    fn mute_and_volume_shape_derived_events() {
        let tl = timeline(EditOps::default(), 10.0);
        let ops = [
            AudioOp::Mute { range: (0.0, 2.0) },
            AudioOp::Volume { level: 0.25, range: (4.0, 6.0) },
        ];
        let keys = [key(1.0), key(3.0), key(5.0)];
        let plan = plan_events(&tl, &ops, &keys, &[], &cfg_keyboard()).unwrap();
        let presses: Vec<_> = plan.events.iter().filter(|e| e.name == "press").collect();
        assert_eq!(presses.len(), 2, "muted key dropped");
        let quiet = presses.iter().find(|e| (e.t - 5.0).abs() < 1e-9).unwrap();
        let normal = presses.iter().find(|e| (e.t - 3.0).abs() < 1e-9).unwrap();
        assert!(quiet.gain < normal.gain * 0.4);
    }

    #[test]
    fn inferred_typing_from_grid_changes() {
        let tl = timeline(EditOps::default(), 10.0);
        let changes = [
            GridChange { src_time: 1.0, changed_cells: 1, total_cells: 800, rows_touched: 1, cursor_advanced: true },
            GridChange { src_time: 1.2, changed_cells: 2, total_cells: 800, rows_touched: 1, cursor_advanced: true },
            // A repaint is not typing.
            GridChange { src_time: 2.0, changed_cells: 700, total_cells: 800, rows_touched: 20, cursor_advanced: false },
        ];
        let plan = plan_events(&tl, &[], &[], &changes, &cfg_keyboard()).unwrap();
        assert_eq!(plan.events.iter().filter(|e| e.name == "press").count(), 2);
    }

    #[test]
    fn key_kind_classification() {
        assert_eq!(KeyKind::from_data("\r"), KeyKind::Enter);
        assert_eq!(KeyKind::from_data(" "), KeyKind::Space);
        assert_eq!(KeyKind::from_data("\x7f"), KeyKind::Backspace);
        assert_eq!(KeyKind::from_data("a"), KeyKind::Char);
        assert_eq!(KeyKind::from_data("\x1b[A"), KeyKind::Other);
    }
}
