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
}

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

        let mut events = Vec::new();
        let mut prev_t = 0.0f64;
        for (idx, line) in lines {
            if line.trim().is_empty() {
                continue;
            }
            let raw: (f64, String, String) = serde_json::from_str(line)
                .map_err(|source| CastError::Event { line: idx + 1, source })?;
            let (time, code, data) = raw;
            if time < prev_t {
                return Err(CastError::NonMonotonic { line: idx + 1, t: time, prev: prev_t });
            }
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
        let mut p = cast_path.as_os_str().to_owned();
        p.push(".reelmeta");
        let text = std::fs::read_to_string(Path::new(&p)).ok()?;
        serde_json::from_str(&text).ok()
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
    fn header_extras_survive() {
        let cast = Cast::parse(r#"{"version": 2, "width": 10, "height": 5, "idle_time_limit": 2.5}"#).unwrap();
        assert!(cast.header.extra.contains_key("idle_time_limit"));
    }
}
