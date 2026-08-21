//! Time syntax: `3s`, `1200ms`, `1:24` (mm:ss), `7` (seconds), `end`,
//! `end-2s`, `@marker` (a label dropped while recording or defined with a
//! `marker` op).

/// A timestamp that may reference the end of the recording or a named
/// marker, resolved once the cast is known.
#[derive(Debug, Clone, PartialEq)]
pub enum TimeExpr {
    Abs(f64),
    /// `end` minus an offset in seconds (`end` itself is offset 0).
    FromEnd(f64),
    /// `@label`: a marker's source time (from the cast or a `marker` op).
    Marker(String),
}

impl TimeExpr {
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if let Some(name) = s.strip_prefix('@') {
            if name.is_empty() {
                return Err("expected a marker name after `@` (like `@1` or `@intro`)".into());
            }
            return Ok(TimeExpr::Marker(name.to_string()));
        }
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

    /// Resolves against the recording duration and a marker table
    /// (label → source time). Errors on a marker reference not in the table.
    pub fn resolve_in(&self, duration: f64, markers: &[(String, f64)]) -> Result<f64, String> {
        match self {
            TimeExpr::Abs(t) => Ok(*t),
            TimeExpr::FromEnd(off) => Ok((duration - off).max(0.0)),
            TimeExpr::Marker(name) => markers
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, t)| *t)
                .ok_or_else(|| format!("unknown marker `@{name}`")),
        }
    }

    /// Marker-free resolution, for contexts with no recording in hand.
    /// Callers that may see `@marker` must use [`resolve_in`](Self::resolve_in).
    pub fn resolve(&self, duration: f64) -> f64 {
        self.resolve_in(duration, &[]).unwrap_or(0.0)
    }
}

/// A concrete span of time: `3s`, `1200ms`, `1:24`, or a bare number of
/// seconds. Always finite and non-negative — durations feed frame counts
/// and buffer sizes, so `hold -5s` or `hold 1e12` must fail here.
pub fn parse_duration(s: &str) -> Result<f64, String> {
    const MAX_S: f64 = 86_400.0;
    let s = s.trim();
    if s.is_empty() {
        return Err("empty time value".into());
    }
    let v = if let Some((m, sec)) = s.split_once(':') {
        let m: f64 = m
            .parse()
            .map_err(|_| format!("bad minutes in `{s}` (expected mm:ss)"))?;
        let sec: f64 = sec
            .parse()
            .map_err(|_| format!("bad seconds in `{s}` (expected mm:ss)"))?;
        if !(0.0..60.0).contains(&sec) {
            return Err(format!("seconds out of range in `{s}` (expected 0-59.999)"));
        }
        m * 60.0 + sec
    } else if let Some(n) = s.strip_suffix("ms") {
        let v: f64 = n.trim().parse().map_err(|_| format!("bad duration `{s}`"))?;
        v / 1000.0
    } else if let Some(n) = s.strip_suffix('s') {
        n.trim().parse().map_err(|_| format!("bad duration `{s}`"))?
    } else {
        s.parse::<f64>()
            .map_err(|_| format!("bad time `{s}` (expected 3s, 1200ms, 1:24, or seconds)"))?
    };
    if !v.is_finite() || v < 0.0 {
        return Err(format!("time `{s}` must be a non-negative number"));
    }
    if v > MAX_S {
        return Err(format!("time `{s}` is longer than the 24h limit"));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_refs_parse_and_resolve() {
        assert_eq!(TimeExpr::parse("@intro"), Ok(TimeExpr::Marker("intro".into())));
        assert_eq!(TimeExpr::parse("@1"), Ok(TimeExpr::Marker("1".into())));
        assert!(TimeExpr::parse("@").is_err());

        let markers = vec![("intro".to_string(), 4.5)];
        let te = TimeExpr::parse("@intro").unwrap();
        assert_eq!(te.resolve_in(100.0, &markers), Ok(4.5));
        assert!(te.resolve_in(100.0, &[]).is_err());
    }

    #[test]
    fn durations_must_be_finite_non_negative_and_sane() {
        for bad in ["-5s", "-5", "-1:30", "-200ms", "NaN", "inf", "1e12", "1e12s"] {
            assert!(parse_duration(bad).is_err(), "`{bad}` must be rejected");
        }
        assert_eq!(parse_duration("0s"), Ok(0.0));
        assert_eq!(parse_duration("1:24"), Ok(84.0));
    }
}
