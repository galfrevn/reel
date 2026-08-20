//! The `.reel` file format: TOML front-matter between `---` fences, followed
//! by a newline-delimited script body.
//!
//! If `[source] cast = "..."` is present the file is in **edit mode** and the
//! body may only contain timeline/audio operations. Script mode (input ops)
//! is a later phase; the parser recognizes those ops and reports a clear
//! error rather than a generic parse failure.

mod time;

pub use time::TimeExpr;

use reel_timeline::{AudioOp, CaptionPos, EditOps, VisualOp};
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("missing front-matter: a .reel file starts with `---`, TOML config, then `---`")]
    MissingFrontMatter,
    #[error("unclosed front-matter: no closing `---` fence found")]
    UnclosedFrontMatter,
    #[error("invalid front-matter TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("line {line}: {msg}")]
    Script { line: usize, msg: String },
    #[error("line {line}: `{op}` is a script-mode input op, but this file is in edit mode ([source].cast is set) — edit files may only shape the timeline")]
    InputOpInEditMode { line: usize, op: String },
    #[error("script mode (no [source].cast) is not implemented yet — record with `reel record` or asciinema, then set [source] cast = \"...\"")]
    ScriptModeUnsupported,
}

fn err(line: usize, msg: impl Into<String>) -> FormatError {
    FormatError::Script { line, msg: msg.into() }
}

// ---------------------------------------------------------------------------
// Front-matter config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReelConfig {
    #[serde(default)]
    pub source: Option<SourceCfg>,
    #[serde(default)]
    pub output: OutputCfg,
    #[serde(default)]
    pub template: TemplateCfg,
    #[serde(default)]
    pub terminal: Option<toml::Value>,
    #[serde(default)]
    pub env: Option<toml::Value>,
    #[serde(default)]
    pub typing: Option<toml::Value>,
    #[serde(default)]
    pub style: StyleCfg,
    #[serde(default)]
    pub audio: AudioCfg,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceCfg {
    pub cast: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OutputCfg {
    pub file: Option<String>,
    #[serde(rename = "loop")]
    pub looping: bool,
    /// Target size like "800kb" / "2mb"; the encoder degrades to fit.
    pub budget: Option<String>,
    /// Frame-rate cap / video frame rate. Defaults per format: 30 for GIF
    /// (size), 60 for WebM (played at constant frame rate).
    pub fps: Option<u32>,
    /// Supersampling factor for crisp text.
    pub scale: u32,
    /// Canvas aspect ratio like "16:9"; the canvas grows (never crops) to fit.
    pub aspect: Option<String>,
}

impl Default for OutputCfg {
    fn default() -> Self {
        OutputCfg { file: None, looping: true, budget: None, fps: None, scale: 2, aspect: None }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TemplateCfg {
    pub name: String,
}

impl Default for TemplateCfg {
    fn default() -> Self {
        TemplateCfg { name: "minimal".into() }
    }
}

/// `[style]` overrides applied on top of the template. All optional — absent
/// means "template decides".
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StyleCfg {
    pub theme: Option<String>,
    pub font: Option<String>,
    pub font_size: Option<f32>,
    pub line_height: Option<f32>,
    /// macos | rounded | plain | none
    pub window: Option<String>,
    pub padding: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AudioCfg {
    /// `None` = auto: audio is active when anything audible is configured
    /// (a keyboard/thinking/bed setting or a `sound` op in the script).
    pub enabled: Option<bool>,
    pub keyboard: Option<String>,
    pub volume: f32,
    pub ui_sounds: bool,
    pub thinking: Option<String>,
    pub bed: Option<String>,
}

impl Default for AudioCfg {
    fn default() -> Self {
        AudioCfg { enabled: None, keyboard: None, volume: 0.35, ui_sounds: true, thinking: None, bed: None }
    }
}

impl AudioCfg {
    /// Whether audio should be produced at all (only WebM output carries it).
    pub fn active(&self, has_sound_ops: bool) -> bool {
        match self.enabled {
            Some(v) => v,
            None => {
                has_sound_ops
                    || self.keyboard.is_some()
                    || self.thinking.is_some()
                    || self.bed.is_some()
            }
        }
    }
}

/// Parses an aspect ratio like "16:9", "4:3", or "1.78" into width/height.
pub fn parse_aspect(s: &str) -> Option<f32> {
    let s = s.trim();
    let v = if let Some((w, h)) = s.split_once(':') {
        w.trim().parse::<f32>().ok()? / h.trim().parse::<f32>().ok()?
    } else {
        s.parse::<f32>().ok()?
    };
    (v.is_finite() && v > 0.1 && v < 10.0).then_some(v)
}

/// Parses a size budget like "800kb", "2mb", "1.5MB" into bytes.
pub fn parse_budget(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_lowercase();
    let (num, mult) = if let Some(n) = s.strip_suffix("kb") {
        (n, 1_000f64)
    } else if let Some(n) = s.strip_suffix("mb") {
        (n, 1_000_000f64)
    } else if let Some(n) = s.strip_suffix('b') {
        (n, 1f64)
    } else {
        (s.as_str(), 1f64)
    };
    let v: f64 = num.trim().parse().ok()?;
    (v > 0.0).then_some((v * mult) as u64)
}

// ---------------------------------------------------------------------------
// Script body ops (edit mode)
// ---------------------------------------------------------------------------

/// A parsed op with unresolved time expressions (`end-2s` needs the cast
/// duration, which the parser doesn't have).
#[derive(Debug, Clone)]
pub enum RawOp {
    Trim { range: (TimeExpr, TimeExpr) },
    Cut { range: (TimeExpr, TimeExpr) },
    Speed { factor: f64, range: (TimeExpr, TimeExpr) },
    Hold { dur: f64, at: TimeExpr },
    FreezeLast { dur: f64 },
    Zoom { factor: f64, center: (u16, u16), range: Option<(TimeExpr, TimeExpr)> },
    Pan { to: (u16, u16), range: (TimeExpr, TimeExpr) },
    Caption { text: String, at: TimeExpr, dur: f64, pos: CaptionPos },
    Highlight { rect: (u16, u16, u16, u16), at: TimeExpr, dur: f64 },
    Marker { label: String, at: TimeExpr },
    Sound { name: String, at: TimeExpr },
    Mute { range: (TimeExpr, TimeExpr) },
    Volume { level: f64, range: (TimeExpr, TimeExpr) },
}

const INPUT_OPS: &[&str] = &[
    "run", "type", "paste", "key", "enter", "mouse", "resize", "capture_live", "wait_idle",
    "wait_text", "wait_gone", "sleep",
];

#[derive(Debug)]
pub struct ReelFile {
    pub config: ReelConfig,
    pub ops: Vec<RawOp>,
}

/// The fully resolved edit program, produced once the cast duration is known.
#[derive(Debug, Default)]
pub struct EditProgram {
    pub edits: EditOps,
    pub visuals: Vec<VisualOp>,
    pub audio: Vec<AudioOp>,
}

impl ReelFile {
    pub fn parse(text: &str) -> Result<Self, FormatError> {
        let (front, body, body_first_line) = split_front_matter(text)?;
        let config: ReelConfig = toml::from_str(front)?;
        if config.source.is_none() {
            return Err(FormatError::ScriptModeUnsupported);
        }

        let mut ops = Vec::new();
        for (i, raw_line) in body.lines().enumerate() {
            let line_no = body_first_line + i;
            let line = strip_comment(raw_line);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let tokens = tokenize(line).map_err(|m| err(line_no, m))?;
            let op_name = tokens[0].word().ok_or_else(|| {
                err(line_no, "expected an operation name at the start of the line")
            })?;
            if INPUT_OPS.contains(&op_name) {
                return Err(FormatError::InputOpInEditMode {
                    line: line_no,
                    op: op_name.to_string(),
                });
            }
            ops.push(parse_op(op_name, &tokens[1..], line_no)?);
        }
        Ok(ReelFile { config, ops })
    }

    /// Resolves all time expressions against the recording duration and
    /// buckets ops for the timeline compiler.
    pub fn resolve(&self, src_duration: f64) -> Result<EditProgram, FormatError> {
        let mut p = EditProgram::default();
        let d = src_duration;
        let range = |r: &(TimeExpr, TimeExpr)| (r.0.resolve(d), r.1.resolve(d));
        for op in &self.ops {
            match op {
                RawOp::Trim { range: r } => p.edits.trim = Some(range(r)),
                RawOp::Cut { range: r } => p.edits.cuts.push(range(r)),
                RawOp::Speed { factor, range: r } => {
                    let (a, b) = range(r);
                    p.edits.speeds.push((*factor, a, b));
                }
                RawOp::Hold { dur, at } => p.edits.holds.push((*dur, at.resolve(d))),
                RawOp::FreezeLast { dur } => p.edits.freeze_last = Some(*dur),
                RawOp::Zoom { factor, center, range: r } => p.visuals.push(VisualOp::Zoom {
                    factor: *factor,
                    center: *center,
                    range: r.as_ref().map(&range),
                }),
                RawOp::Pan { to, range: r } => {
                    p.visuals.push(VisualOp::Pan { to: *to, range: range(r) })
                }
                RawOp::Caption { text, at, dur, pos } => p.visuals.push(VisualOp::Caption {
                    text: text.clone(),
                    at: at.resolve(d),
                    dur: *dur,
                    pos: *pos,
                }),
                RawOp::Highlight { rect, at, dur } => p.visuals.push(VisualOp::Highlight {
                    rect: *rect,
                    at: at.resolve(d),
                    dur: *dur,
                }),
                RawOp::Marker { label, at } => {
                    p.visuals.push(VisualOp::Marker { label: label.clone(), at: at.resolve(d) })
                }
                RawOp::Sound { name, at } => {
                    p.audio.push(AudioOp::Sound { name: name.clone(), at: at.resolve(d) })
                }
                RawOp::Mute { range: r } => p.audio.push(AudioOp::Mute { range: range(r) }),
                RawOp::Volume { level, range: r } => {
                    p.audio.push(AudioOp::Volume { level: *level, range: range(r) })
                }
            }
        }
        Ok(p)
    }
}

fn split_front_matter(text: &str) -> Result<(&str, &str, usize), FormatError> {
    let text = text.trim_start_matches('\u{feff}');
    let mut rest = text;
    // Skip leading blank lines before the opening fence.
    let mut line_no = 1;
    loop {
        match rest.split_once('\n') {
            Some((first, tail)) if first.trim().is_empty() => {
                rest = tail;
                line_no += 1;
            }
            _ => break,
        }
    }
    let after_open = rest.strip_prefix("---").ok_or(FormatError::MissingFrontMatter)?;
    let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);
    line_no += 1;

    let mut offset = 0;
    for l in after_open.lines() {
        if l.trim() == "---" {
            let front = &after_open[..offset];
            let body_start = offset + l.len();
            let body = after_open[body_start..].strip_prefix('\n').unwrap_or(&after_open[body_start..]);
            let front_lines = front.lines().count();
            return Ok((front, body, line_no + front_lines + 1));
        }
        offset += l.len() + 1;
    }
    Err(FormatError::UnclosedFrontMatter)
}

fn strip_comment(line: &str) -> &str {
    let mut in_str = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_str = !in_str,
            '#' if !in_str => return &line[..i],
            _ => {}
        }
    }
    line
}

// ---------------------------------------------------------------------------
// Tokenizer: words, "quoted strings", (tuples)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    Str(String),
    /// Comma-separated numbers inside parentheses: `(30,10)` or `(2,3,10,4)`.
    Tuple(Vec<i64>),
}

impl Token {
    fn word(&self) -> Option<&str> {
        match self {
            Token::Word(w) => Some(w),
            _ => None,
        }
    }
}

fn tokenize(line: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut chars = line.char_indices().peekable();
    while let Some(&(i, c)) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '"' => {
                chars.next();
                let mut s = String::new();
                let mut closed = false;
                for (_, c) in chars.by_ref() {
                    if c == '"' {
                        closed = true;
                        break;
                    }
                    s.push(c);
                }
                if !closed {
                    return Err("unclosed string literal".into());
                }
                tokens.push(Token::Str(s));
            }
            '(' => {
                chars.next();
                let mut inner = String::new();
                let mut closed = false;
                for (_, c) in chars.by_ref() {
                    if c == ')' {
                        closed = true;
                        break;
                    }
                    inner.push(c);
                }
                if !closed {
                    return Err("unclosed parenthesis".into());
                }
                let nums: Result<Vec<i64>, _> =
                    inner.split(',').map(|p| p.trim().parse::<i64>()).collect();
                tokens.push(Token::Tuple(nums.map_err(|_| {
                    format!("expected comma-separated integers inside parentheses, got `({inner})`")
                })?));
            }
            _ => {
                let start = i;
                let mut end = i;
                while let Some(&(j, c)) = chars.peek() {
                    if c.is_whitespace() || c == '"' || c == '(' {
                        break;
                    }
                    end = j + c.len_utf8();
                    chars.next();
                }
                tokens.push(Token::Word(line[start..end].to_string()));
            }
        }
    }
    Ok(tokens)
}

// ---------------------------------------------------------------------------
// Per-op parsing
// ---------------------------------------------------------------------------

struct Args<'a> {
    toks: &'a [Token],
    pos: usize,
    line: usize,
    op: &'a str,
}

impl<'a> Args<'a> {
    fn next(&mut self) -> Result<&'a Token, FormatError> {
        let t = self.toks.get(self.pos).ok_or_else(|| {
            err(self.line, format!("`{}`: missing argument", self.op))
        })?;
        self.pos += 1;
        Ok(t)
    }

    fn word(&mut self) -> Result<&'a str, FormatError> {
        let line = self.line;
        let op = self.op;
        self.next()?.word().ok_or_else(|| err(line, format!("`{op}`: expected a word argument")))
    }

    fn string(&mut self) -> Result<String, FormatError> {
        match self.next()? {
            Token::Str(s) => Ok(s.clone()),
            other => Err(err(self.line, format!("`{}`: expected a \"quoted string\", got {:?}", self.op, other))),
        }
    }

    fn keyword(&mut self, kw: &str) -> Result<(), FormatError> {
        let w = self.word()?;
        if w == kw {
            Ok(())
        } else {
            Err(err(self.line, format!("`{}`: expected `{kw}`, got `{w}`", self.op)))
        }
    }

    fn time(&mut self) -> Result<TimeExpr, FormatError> {
        let w = self.word()?;
        TimeExpr::parse(w).map_err(|m| err(self.line, format!("`{}`: {m}", self.op)))
    }

    /// A `A..B` range in one token.
    fn time_range(&mut self) -> Result<(TimeExpr, TimeExpr), FormatError> {
        let w = self.word()?;
        let (a, b) = w
            .split_once("..")
            .ok_or_else(|| err(self.line, format!("`{}`: expected a range like `2s..8s`, got `{w}`", self.op)))?;
        let a = TimeExpr::parse(a).map_err(|m| err(self.line, format!("`{}`: {m}", self.op)))?;
        let b = TimeExpr::parse(b).map_err(|m| err(self.line, format!("`{}`: {m}", self.op)))?;
        Ok((a, b))
    }

    /// A `from A to B` range.
    fn from_to(&mut self) -> Result<(TimeExpr, TimeExpr), FormatError> {
        self.keyword("from")?;
        let a = self.time()?;
        self.keyword("to")?;
        let b = self.time()?;
        Ok((a, b))
    }

    fn duration(&mut self) -> Result<f64, FormatError> {
        let w = self.word()?;
        time::parse_duration(w).map_err(|m| err(self.line, format!("`{}`: {m}", self.op)))
    }

    fn factor(&mut self) -> Result<f64, FormatError> {
        let w = self.word()?;
        let n = w.strip_suffix('x').unwrap_or(w);
        n.parse::<f64>()
            .map_err(|_| err(self.line, format!("`{}`: expected a factor like `5x`, got `{w}`", self.op)))
    }

    fn tuple(&mut self, arity: usize) -> Result<Vec<i64>, FormatError> {
        match self.next()? {
            Token::Tuple(v) if v.len() == arity => Ok(v.clone()),
            Token::Tuple(v) => Err(err(
                self.line,
                format!("`{}`: expected {arity} numbers in parentheses, got {}", self.op, v.len()),
            )),
            other => Err(err(self.line, format!("`{}`: expected `(...)`, got {:?}", self.op, other))),
        }
    }

    fn done(&self) -> Result<(), FormatError> {
        if self.pos < self.toks.len() {
            return Err(err(
                self.line,
                format!("`{}`: unexpected trailing arguments: {:?}", self.op, &self.toks[self.pos..]),
            ));
        }
        Ok(())
    }

    fn peek_word(&self) -> Option<&str> {
        self.toks.get(self.pos).and_then(|t| t.word())
    }
}

fn to_cell(v: i64, line: usize, what: &str) -> Result<u16, FormatError> {
    u16::try_from(v).map_err(|_| err(line, format!("{what} must be a non-negative cell coordinate, got {v}")))
}

fn parse_op(name: &str, toks: &[Token], line: usize) -> Result<RawOp, FormatError> {
    let mut a = Args { toks, pos: 0, line, op: name };
    let op = match name {
        "trim" => RawOp::Trim { range: a.time_range()? },
        "cut" => RawOp::Cut { range: a.time_range()? },
        "speed" => {
            let factor = a.factor()?;
            let range = a.from_to()?;
            RawOp::Speed { factor, range }
        }
        "hold" => {
            let dur = a.duration()?;
            a.keyword("at")?;
            let at = a.time()?;
            RawOp::Hold { dur, at }
        }
        "freeze" => {
            a.keyword("last")?;
            RawOp::FreezeLast { dur: a.duration()? }
        }
        "zoom" => {
            let factor = a.factor()?;
            a.keyword("at")?;
            let t = a.tuple(2)?;
            let center = (to_cell(t[0], line, "zoom col")?, to_cell(t[1], line, "zoom row")?);
            let range = if a.peek_word() == Some("from") {
                a.keyword("from")?;
                let s = a.time()?;
                a.keyword("to")?;
                let e = a.time()?;
                Some((s, e))
            } else {
                None
            };
            RawOp::Zoom { factor, center, range }
        }
        "pan" => {
            a.keyword("to")?;
            let t = a.tuple(2)?;
            let to = (to_cell(t[0], line, "pan col")?, to_cell(t[1], line, "pan row")?);
            let range = a.from_to()?;
            RawOp::Pan { to, range }
        }
        "caption" => {
            let text = a.string()?;
            a.keyword("at")?;
            let at = a.time()?;
            a.keyword("for")?;
            let dur = a.duration()?;
            let pos = if let Some(w) = a.peek_word().map(str::to_owned) {
                let p = w.strip_prefix("pos=").ok_or_else(|| {
                    err(line, format!("`caption`: unexpected argument `{w}` (did you mean `pos=bottom`?)"))
                })?;
                a.pos += 1;
                match p {
                    "bottom" => CaptionPos::Bottom,
                    "top" => CaptionPos::Top,
                    "center" => CaptionPos::Center,
                    other => return Err(err(line, format!("`caption`: unknown pos `{other}`"))),
                }
            } else {
                CaptionPos::Bottom
            };
            RawOp::Caption { text, at, dur, pos }
        }
        "highlight" => {
            let t = a.tuple(4)?;
            let rect = (
                to_cell(t[0], line, "highlight col")?,
                to_cell(t[1], line, "highlight row")?,
                to_cell(t[2], line, "highlight width")?,
                to_cell(t[3], line, "highlight height")?,
            );
            a.keyword("at")?;
            let at = a.time()?;
            a.keyword("for")?;
            let dur = a.duration()?;
            RawOp::Highlight { rect, at, dur }
        }
        "marker" => {
            let label = a.string()?;
            a.keyword("at")?;
            RawOp::Marker { label, at: a.time()? }
        }
        "sound" => {
            let name = a.string()?;
            a.keyword("at")?;
            RawOp::Sound { name, at: a.time()? }
        }
        "mute" => RawOp::Mute { range: a.time_range()? },
        "volume" => {
            let level = a
                .word()?
                .parse::<f64>()
                .map_err(|_| err(line, "`volume`: expected a level like `0.15`"))?;
            let range = a.from_to()?;
            RawOp::Volume { level, range }
        }
        other => {
            return Err(err(line, format!("unknown operation `{other}`")));
        }
    };
    a.done()?;
    Ok(op)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC_EXAMPLE: &str = r#"---
[source]
cast = "opencode-session.cast"

[template]
name = "glass"

[output]
file   = "demo.webm"
budget = "2mb"

[audio]
keyboard = "mx-brown"
thinking = "soft-pulse"
---

trim    2s..end
cut     19s..23s                    # remove the typo
speed   5x from 8s to 34s           # LLM thinking -> compressed
volume  0.15 from 8s to 34s
caption "Refactor the auth module" at 4s for 2.5s
zoom    1.8x at (30,10) from 36s to 41s
sound   "success" at 41s
freeze  last 1.5s
"#;

    #[test]
    fn parses_the_spec_example() {
        let f = ReelFile::parse(SPEC_EXAMPLE).unwrap();
        assert_eq!(f.config.source.as_ref().unwrap().cast, "opencode-session.cast");
        assert_eq!(f.config.template.name, "glass");
        assert_eq!(f.config.output.budget.as_deref(), Some("2mb"));
        assert_eq!(f.ops.len(), 8);

        let p = f.resolve(45.0).unwrap();
        assert_eq!(p.edits.trim, Some((2.0, 45.0)));
        assert_eq!(p.edits.cuts, vec![(19.0, 23.0)]);
        assert_eq!(p.edits.speeds, vec![(5.0, 8.0, 34.0)]);
        assert_eq!(p.edits.freeze_last, Some(1.5));
        assert_eq!(p.visuals.len(), 2);
        assert_eq!(p.audio.len(), 2);
        match &p.visuals[1] {
            VisualOp::Zoom { factor, center, range } => {
                assert!((factor - 1.8).abs() < 1e-9);
                assert_eq!(*center, (30, 10));
                assert_eq!(*range, Some((36.0, 41.0)));
            }
            other => panic!("expected zoom, got {other:?}"),
        }
    }

    #[test]
    fn time_expressions() {
        let f = |s: &str| TimeExpr::parse(s).unwrap().resolve(100.0);
        assert_eq!(f("3s"), 3.0);
        assert_eq!(f("1200ms"), 1.2);
        assert_eq!(f("1:24"), 84.0);
        assert_eq!(f("end"), 100.0);
        assert_eq!(f("end-2s"), 98.0);
        assert_eq!(f("7"), 7.0);
    }

    #[test]
    fn input_op_in_edit_mode_is_a_clear_error() {
        let text = "---\n[source]\ncast = \"x.cast\"\n---\ntype \"hello\"\n";
        match ReelFile::parse(text).unwrap_err() {
            FormatError::InputOpInEditMode { op, .. } => assert_eq!(op, "type"),
            other => panic!("wrong error: {other}"),
        }
    }

    #[test]
    fn script_mode_reports_unsupported() {
        let text = "---\n[output]\nfile = \"x.gif\"\n---\ntype \"hi\"\n";
        assert!(matches!(ReelFile::parse(text).unwrap_err(), FormatError::ScriptModeUnsupported));
    }

    #[test]
    fn unknown_op_names_the_line() {
        let text = "---\n[source]\ncast = \"x.cast\"\n---\n\nwarp 3s\n";
        match ReelFile::parse(text).unwrap_err() {
            FormatError::Script { line, msg } => {
                assert_eq!(line, 6);
                assert!(msg.contains("warp"));
            }
            other => panic!("wrong error: {other}"),
        }
    }

    #[test]
    fn comments_inside_strings_survive() {
        let text = "---\n[source]\ncast = \"x.cast\"\n---\ncaption \"see #42\" at 1s for 2s\n";
        let f = ReelFile::parse(text).unwrap();
        match &f.ops[0] {
            RawOp::Caption { text, .. } => assert_eq!(text, "see #42"),
            other => panic!("wrong op: {other:?}"),
        }
    }

    #[test]
    fn budget_parsing() {
        assert_eq!(parse_budget("800kb"), Some(800_000));
        assert_eq!(parse_budget("2mb"), Some(2_000_000));
        assert_eq!(parse_budget("1.5MB"), Some(1_500_000));
        assert_eq!(parse_budget("oops"), None);
    }

    #[test]
    fn trailing_garbage_is_rejected() {
        let text = "---\n[source]\ncast = \"x.cast\"\n---\ntrim 2s..end whoops\n";
        assert!(matches!(ReelFile::parse(text).unwrap_err(), FormatError::Script { .. }));
    }
}
