//! Time syntax: `3s`, `1200ms`, `1:24` (mm:ss), `7` (seconds), `end`,
//! `end-2s`.

/// A timestamp that may reference the end of the recording, resolved once the
/// cast duration is known.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimeExpr {
    Abs(f64),
    /// `end` minus an offset in seconds (`end` itself is offset 0).
    FromEnd(f64),
}

impl TimeExpr {
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if let Some(rest) = s.strip_prefix("end") {
            let rest = rest.trim();
            if rest.is_empty() {
                return Ok(TimeExpr::FromEnd(0.0));
            }
            let off = rest
                .strip_prefix('-')
                .ok_or_else(|| format!("expected `end` or `end-<duration>`, got `{s}`"))?;
            return Ok(TimeExpr::FromEnd(parse_duration(off)?));
        }
        Ok(TimeExpr::Abs(parse_duration(s)?))
    }

    pub fn resolve(&self, duration: f64) -> f64 {
        match *self {
            TimeExpr::Abs(t) => t,
            TimeExpr::FromEnd(off) => (duration - off).max(0.0),
        }
    }
}

/// A concrete span of time: `3s`, `1200ms`, `1:24`, or a bare number of
/// seconds.
pub fn parse_duration(s: &str) -> Result<f64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty time value".into());
    }
    if let Some((m, sec)) = s.split_once(':') {
        let m: f64 = m
            .parse()
            .map_err(|_| format!("bad minutes in `{s}` (expected mm:ss)"))?;
        let sec: f64 = sec
            .parse()
            .map_err(|_| format!("bad seconds in `{s}` (expected mm:ss)"))?;
        if !(0.0..60.0).contains(&sec) {
            return Err(format!("seconds out of range in `{s}` (expected 0-59.999)"));
        }
        return Ok(m * 60.0 + sec);
    }
    if let Some(n) = s.strip_suffix("ms") {
        let v: f64 = n.trim().parse().map_err(|_| format!("bad duration `{s}`"))?;
        return Ok(v / 1000.0);
    }
    if let Some(n) = s.strip_suffix('s') {
        let v: f64 = n.trim().parse().map_err(|_| format!("bad duration `{s}`"))?;
        return Ok(v);
    }
    s.parse::<f64>()
        .map_err(|_| format!("bad time `{s}` (expected 3s, 1200ms, 1:24, or seconds)"))
}
