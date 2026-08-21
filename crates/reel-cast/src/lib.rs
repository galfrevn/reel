//! Parsing for the asciinema v2 cast format and reel's `.reelmeta` sidecar.
//!
//! A cast is the frozen boundary between capture and everything downstream:
//! once a session is recorded, reel never re-executes the program. All later
//! stages are pure functions of the cast (plus the `.reel` edit file).

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum CastError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("cast is empty (no header line)")]
    Empty,
    #[error("invalid cast header: {0}")]
    Header(serde_json::Error),
    #[error("unsupported cast version {0} (only v2 is supported)")]
    Version(u32),
    #[error("invalid event on line {line}: {source}")]
    Event {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("event on line {line} has non-monotonic time {t} (previous {prev})")]
    NonMonotonic { line: usize, t: f64, prev: f64 },
    #[error("event on line {line} has invalid time {t} (must be finite, 0..{MAX_TIME_S}s)")]
    InvalidTime { line: usize, t: f64 },
    #[error("header duration {0} is invalid (must be finite, 0..{MAX_TIME_S}s)")]
    InvalidDuration(f64),
    #[error("invalid sidecar {path}: {source}")]
    Sidecar {
        path: String,
        #[source]
        source: serde_json::Error,
    },
}

/// Upper bound on event times and header duration. Times from a cast flow
/// into buffer allocations and frame counts downstream, so an absurd value
/// must die here rather than as an OOM in the mixer or encoder.
pub const MAX_TIME_S: f64 = 86_400.0;

/// Recorders occasionally emit sub-frame backwards jitter; clamp it instead
/// of rejecting the file. Anything larger is treated as real corruption.
const JITTER_CLAMP_S: f64 = 0.010;

/// The asciinema v2 header (first line of the file). Fields we don't use are
/// preserved so a cast can round-trip through reel unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CastHeader {
    pub version: u32,
    pub width: u16,
    pub height: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<std::collections::HashMap<String, String>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// Terminal output ("o") — the only kind that drives emulation.
    Output,
    /// Recorded input ("i"). Kept for audio inference when no sidecar exists.
    Input,
    /// Resize ("r"), payload "COLSxROWS".
    Resize,
    /// Marker ("m").
    Marker,
    /// Anything else — preserved, ignored downstream.
    Other,
}

#[derive(Debug, Clone)]
pub struct CastEvent {
    /// Seconds since session start (the *source clock* every reel timestamp
    /// refers to).
    pub time: f64,
    pub kind: EventKind,
    pub data: String,
}

#[derive(Debug, Clone)]
pub struct Cast {
    pub header: CastHeader,
    pub events: Vec<CastEvent>,
}

impl Cast {
    pub fn load(path: &Path) -> Result<Self, CastError> {
        let text = std::fs::read_to_string(path).map_err(|source| CastError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, CastError> {
        let mut lines = text.lines().enumerate();
        let (_, header_line) = lines
            .by_ref()
            .find(|(_, l)| !l.trim().is_empty())
            .ok_or(CastError::Empty)?;
        let header: CastHeader =
            serde_json::from_str(header_line).map_err(CastError::Header)?;
        if header.version != 2 {
            return Err(CastError::Version(header.version));
        }
        if let Some(d) = header.duration {
            if !d.is_finite() || !(0.0..=MAX_TIME_S).contains(&d) {
                return Err(CastError::InvalidDuration(d));
            }
        }

        let mut events = Vec::new();
        let mut prev_t = 0.0f64;
        for (idx, line) in lines {
            if line.trim().is_empty() {
                continue;
            }
            let raw: (f64, String, String) = serde_json::from_str(line)
                .map_err(|source| CastError::Event { line: idx + 1, source })?;
            let (time, code, data) = raw;
            if !time.is_finite() || !(0.0..=MAX_TIME_S).contains(&time) {
                return Err(CastError::InvalidTime { line: idx + 1, t: time });
            }
            let time = if time < prev_t {
                if prev_t - time <= JITTER_CLAMP_S {
                    prev_t
                } else {
                    return Err(CastError::NonMonotonic { line: idx + 1, t: time, prev: prev_t });
                }
            } else {
                time
            };
            prev_t = time;
            let kind = match code.as_str() {
                "o" => EventKind::Output,
                "i" => EventKind::Input,
                "r" => EventKind::Resize,
                "m" => EventKind::Marker,
                _ => EventKind::Other,
            };
            events.push(CastEvent { time, kind, data });
        }
        Ok(Cast { header, events })
    }

    /// Duration of the recording: header value if present, else last event time.
    pub fn duration(&self) -> f64 {
        self.header
            .duration
            .unwrap_or_else(|| self.events.last().map(|e| e.time).unwrap_or(0.0))
    }

    pub fn cols(&self) -> u16 {
        self.header.width
    }

    pub fn rows(&self) -> u16 {
        self.header.height
    }
}

/// The `.reelmeta` sidecar: data the cast format has no place for.
/// Written by `reel record` (Phase 2); optional on imported casts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReelMeta {
    pub version: u32,
    #[serde(default)]
    pub input_events: Vec<InputEvent>,
    #[serde(default)]
    pub term_env: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub cols: u16,
    #[serde(default)]
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputEvent {
    pub t: f64,
    pub kind: String,
    pub value: String,
}

impl ReelMeta {
    /// Loads `<cast>.reelmeta` next to a cast file if it exists.
    pub fn load_sidecar(cast_path: &Path) -> Option<Self> {
        Self::load_sidecar_checked(cast_path).ok().flatten()
    }

    /// Like [`load_sidecar`](Self::load_sidecar), but a sidecar that exists
    /// and fails to parse is an error rather than a silent `None` — callers
    /// can warn instead of quietly dropping typing/audio reconstruction.
    pub fn load_sidecar_checked(cast_path: &Path) -> Result<Option<Self>, CastError> {
        let path = sidecar_path(cast_path);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(CastError::Io { path: path.display().to_string(), source })
            }
        };
        serde_json::from_str(&text).map(Some).map_err(|source| CastError::Sidecar {
            path: path.display().to_string(),
            source,
        })
    }

    /// Writes the sidecar next to its cast.
    pub fn save_sidecar(&self, cast_path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self).expect("sidecar serializes");
        std::fs::write(sidecar_path(cast_path), json)
    }
}

fn sidecar_path(cast_path: &Path) -> std::path::PathBuf {
    let mut p = cast_path.as_os_str().to_owned();
    p.push(".reelmeta");
    std::path::PathBuf::from(p)
}

/// Streaming asciinema-v2 writer for `reel record`. Events go to disk as
/// they happen, so a crashed session still leaves a playable cast. The
/// header carries no duration; readers fall back to the last event time.
pub struct CastWriter<W: std::io::Write> {
    out: W,
    prev_t: f64,
}

impl<W: std::io::Write> CastWriter<W> {
    pub fn new(mut out: W, header: &CastHeader) -> std::io::Result<Self> {
        let json = serde_json::to_string(header).expect("header serializes");
        writeln!(out, "{json}")?;
        Ok(CastWriter { out, prev_t: 0.0 })
    }

    /// Appends one event. Time is clamped monotonic — wall-clock hiccups
    /// must never produce a cast our own parser rejects.
    pub fn event(&mut self, time: f64, code: &str, data: &str) -> std::io::Result<()> {
        let time = if time < self.prev_t { self.prev_t } else { time };
        self.prev_t = time;
        let line =
            serde_json::to_string(&(time, code, data)).expect("event serializes");
        writeln!(self.out, "{line}")?;
        self.out.flush()
    }

    /// Flushes without consuming — for writers shared behind an `Arc` where
    /// detached reader threads still hold clones at shutdown.
    pub fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }

    pub fn finish(mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"version": 2, "width": 80, "height": 24, "timestamp": 1700000000, "env": {"SHELL": "/bin/zsh", "TERM": "xterm-256color"}}
[0.128, "o", "hello "]
[0.712, "o", "\u001b[31mworld\u001b[0m\r\n"]
[1.5, "i", "q"]
[2.0, "r", "100x30"]
"#;

    #[test]
    fn parses_header_and_events() {
        let cast = Cast::parse(SAMPLE).unwrap();
        assert_eq!(cast.cols(), 80);
        assert_eq!(cast.rows(), 24);
        assert_eq!(cast.events.len(), 4);
        assert_eq!(cast.events[0].kind, EventKind::Output);
        assert_eq!(cast.events[1].data, "\u{1b}[31mworld\u{1b}[0m\r\n");
        assert_eq!(cast.events[2].kind, EventKind::Input);
        assert_eq!(cast.events[3].kind, EventKind::Resize);
        assert!((cast.duration() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn rejects_v1() {
        let err = Cast::parse(r#"{"version": 1, "width": 80, "height": 24}"#).unwrap_err();
        assert!(matches!(err, CastError::Version(1)));
    }

    #[test]
    fn rejects_time_going_backwards() {
        let text = r#"{"version": 2, "width": 80, "height": 24}
[1.0, "o", "a"]
[0.5, "o", "b"]
"#;
        assert!(matches!(Cast::parse(text).unwrap_err(), CastError::NonMonotonic { .. }));
    }

    #[test]
    fn clamps_subframe_jitter_but_rejects_real_regressions() {
        let jitter = r#"{"version": 2, "width": 80, "height": 24}
[1.0, "o", "a"]
[0.995, "o", "b"]
"#;
        let cast = Cast::parse(jitter).unwrap();
        assert!((cast.events[1].time - 1.0).abs() < 1e-9);
    }

    #[test]
    fn rejects_absurd_times_and_durations() {
        for bad in ["1e15", "-1.0", "NaN"] {
            let text = format!(
                "{{\"version\": 2, \"width\": 80, \"height\": 24}}\n[{bad}, \"o\", \"x\"]\n"
            );
            let err = Cast::parse(&text);
            assert!(
                matches!(err, Err(CastError::InvalidTime { .. }) | Err(CastError::Event { .. })),
                "time {bad} must not parse: {err:?}"
            );
        }
        let err = Cast::parse(r#"{"version": 2, "width": 80, "height": 24, "duration": 1e15}"#);
        assert!(matches!(err, Err(CastError::InvalidDuration(_))));
    }

    #[test]
    fn corrupt_sidecar_is_an_error_not_a_silent_none() {
        let dir = std::env::temp_dir().join("reel-cast-test-corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let cast_path = dir.join("s.cast");
        std::fs::write(dir.join("s.cast.reelmeta"), "{ not json").unwrap();
        assert!(matches!(
            ReelMeta::load_sidecar_checked(&cast_path),
            Err(CastError::Sidecar { .. })
        ));
        assert!(ReelMeta::load_sidecar(&cast_path).is_none());
        let missing = dir.join("missing.cast");
        assert!(ReelMeta::load_sidecar_checked(&missing).unwrap().is_none());
    }

    #[test]
    fn writer_roundtrips_through_the_parser() {
        let header = CastHeader {
            version: 2,
            width: 100,
            height: 30,
            timestamp: Some(1_700_000_000),
            duration: None,
            title: None,
            command: Some("zsh".into()),
            env: None,
            extra: Default::default(),
        };
        let mut buf = Vec::new();
        let mut w = CastWriter::new(&mut buf, &header).unwrap();
        w.event(0.1, "o", "hi \u{1b}[31mred\u{1b}[0m\r\n").unwrap();
        w.event(0.5, "r", "80x24").unwrap();
        w.event(0.4, "o", "clock went backwards").unwrap(); // clamped
        w.finish().unwrap();

        let cast = Cast::parse(std::str::from_utf8(&buf).unwrap()).unwrap();
        assert_eq!((cast.cols(), cast.rows()), (100, 30));
        assert_eq!(cast.events.len(), 3);
        assert_eq!(cast.events[0].data, "hi \u{1b}[31mred\u{1b}[0m\r\n");
        assert_eq!(cast.events[1].kind, EventKind::Resize);
        assert!((cast.events[2].time - 0.5).abs() < 1e-9, "monotonic clamp");
    }

    #[test]
    fn sidecar_saves_and_loads() {
        let dir = std::env::temp_dir().join("reel-cast-test");
        std::fs::create_dir_all(&dir).unwrap();
        let cast_path = dir.join("s.cast");
        let meta = ReelMeta {
            version: 1,
            input_events: vec![InputEvent { t: 1.5, kind: "key".into(), value: "a".into() }],
            term_env: [("TERM".to_string(), "xterm-256color".to_string())].into(),
            cols: 80,
            rows: 24,
        };
        meta.save_sidecar(&cast_path).unwrap();
        let loaded = ReelMeta::load_sidecar(&cast_path).unwrap();
        assert_eq!(loaded.input_events.len(), 1);
        assert_eq!(loaded.input_events[0].value, "a");
        assert_eq!(loaded.cols, 80);
    }

    #[test]
    fn header_extras_survive() {
        let cast = Cast::parse(r#"{"version": 2, "width": 10, "height": 5, "idle_time_limit": 2.5}"#).unwrap();
        assert!(cast.header.extra.contains_key("idle_time_limit"));
    }
}
