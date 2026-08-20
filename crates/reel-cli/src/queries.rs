//! Terminal query responder for script mode (SPEC §9).
//!
//! Headless captures have no real terminal to answer DA1/DSR/OSC probes, and
//! programs genuinely hang or quit without answers (fx exits after two
//! unanswered `ESC[c`). This scans the child's output and answers with a
//! credible xterm-256color feature set. Responses go only to the child —
//! they are terminal protocol, not keystrokes, so they never reach the
//! input sidecar or the keyboard audio.

/// Rolling scanner; keeps an unterminated tail across chunks.
#[derive(Default)]
pub struct QueryResponder {
    tail: Vec<u8>,
}

impl QueryResponder {
    /// Scans a chunk of program output; returns the responses to write back.
    /// `cursor` is the live grid's (col, row), zero-based.
    pub fn scan(&mut self, chunk: &[u8], cursor: (u16, u16)) -> Vec<Vec<u8>> {
        self.tail.extend_from_slice(chunk);
        let mut out = Vec::new();
        let mut i = 0usize;
        let buf = std::mem::take(&mut self.tail);
        while i < buf.len() {
            if buf[i] != 0x1b {
                i += 1;
                continue;
            }
            match parse_query(&buf[i..]) {
                Parsed::Query(len, q) => {
                    if let Some(resp) = respond(q, cursor) {
                        out.push(resp);
                    }
                    i += len;
                }
                Parsed::NotAQuery(len) => i += len,
                Parsed::Incomplete => break,
            }
        }
        // Keep at most a small unterminated tail; queries are short.
        self.tail = buf[i..].to_vec();
        if self.tail.len() > 128 {
            let cut = self.tail.len() - 128;
            self.tail.drain(..cut);
        }
        out
    }
}

enum Query {
    PrimaryDa,
    SecondaryDa,
    CursorPosition,
    DeviceStatus,
    KittyKeyboard,
    XtVersion,
    OscColor { code: u32, bel: bool },
    Other,
}

enum Parsed {
    Query(usize, Query),
    NotAQuery(usize),
    Incomplete,
}

fn parse_query(buf: &[u8]) -> Parsed {
    debug_assert_eq!(buf[0], 0x1b);
    let Some(&kind) = buf.get(1) else { return Parsed::Incomplete };
    match kind {
        b'[' => {
            // CSI: params then a final byte in 0x40..=0x7e.
            let mut j = 2;
            while j < buf.len() && !(0x40..=0x7e).contains(&buf[j]) {
                j += 1;
            }
            let Some(&fin) = buf.get(j) else { return Parsed::Incomplete };
            let params = &buf[2..j];
            let q = match (fin, params) {
                (b'c', b"") | (b'c', b"0") => Query::PrimaryDa,
                (b'c', p) if p.first() == Some(&b'>') => Query::SecondaryDa,
                (b'n', b"6") => Query::CursorPosition,
                (b'n', b"5") => Query::DeviceStatus,
                (b'u', b"?") => Query::KittyKeyboard,
                (b'q', p) if p.first() == Some(&b'>') => Query::XtVersion,
                _ => Query::Other,
            };
            Parsed::Query(j + 1, q)
        }
        b']' => {
            // OSC: "Ps;?" terminated by BEL or ST is a color query.
            for j in 2..buf.len() {
                let (done, bel, end) = match buf[j] {
                    0x07 => (true, true, j + 1),
                    0x9c => (true, false, j + 1),
                    0x1b if buf.get(j + 1) == Some(&b'\\') => (true, false, j + 2),
                    _ => (false, false, 0),
                };
                if done {
                    let body = &buf[2..j];
                    if let Some(code) = body
                        .strip_suffix(b";?")
                        .and_then(|c| std::str::from_utf8(c).ok())
                        .and_then(|c| c.parse::<u32>().ok())
                    {
                        return Parsed::Query(end, Query::OscColor { code, bel });
                    }
                    return Parsed::NotAQuery(end);
                }
            }
            Parsed::Incomplete
        }
        _ => Parsed::NotAQuery(1),
    }
}

fn respond(q: Query, cursor: (u16, u16)) -> Option<Vec<u8>> {
    Some(match q {
        // VT220-class with color — what a modern xterm-alike advertises.
        Query::PrimaryDa => b"\x1b[?62;22c".to_vec(),
        Query::SecondaryDa => b"\x1b[>41;354;0c".to_vec(),
        Query::CursorPosition => {
            format!("\x1b[{};{}R", cursor.1 + 1, cursor.0 + 1).into_bytes()
        }
        Query::DeviceStatus => b"\x1b[0n".to_vec(),
        // Kitty keyboard protocol: supported, no flags active.
        Query::KittyKeyboard => b"\x1b[?0u".to_vec(),
        Query::XtVersion => b"\x1bP>|reel(0.1.0)\x1b\\".to_vec(),
        Query::OscColor { code, bel } => {
            // A dark theme: near-white fg, black bg/cursor accents.
            let rgb = match code {
                10 => "rgb:e6e6/e6e6/ebeb",
                11 => "rgb:0000/0000/0000",
                12 => "rgb:8a8a/b4b4/f8f8",
                _ => return None,
            };
            let term = if bel { "\x07" } else { "\x1b\\" };
            format!("\x1b]{code};{rgb}{term}").into_bytes()
        }
        Query::Other => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn responses(bytes: &[u8]) -> Vec<Vec<u8>> {
        QueryResponder::default().scan(bytes, (4, 2))
    }

    #[test]
    fn answers_the_classic_probe_set() {
        let r = responses(b"\x1b[c\x1b[>0c\x1b[6n\x1b[?u\x1b[>0q");
        assert_eq!(r.len(), 5);
        assert_eq!(r[0], b"\x1b[?62;22c");
        assert_eq!(r[2], b"\x1b[3;5R", "CPR is 1-based row;col");
        assert_eq!(r[3], b"\x1b[?0u");
    }

    #[test]
    fn osc_color_queries_mirror_their_terminator() {
        let r = responses(b"\x1b]11;?\x07\x1b]10;?\x1b\\");
        assert_eq!(r.len(), 2);
        assert!(r[0].ends_with(b"\x07"));
        assert!(r[1].ends_with(b"\x1b\\"));
        assert!(r[0].starts_with(b"\x1b]11;rgb:0000"));
    }

    #[test]
    fn split_sequences_survive_chunking() {
        let mut q = QueryResponder::default();
        assert!(q.scan(b"text \x1b[", (0, 0)).is_empty());
        let r = q.scan(b"6n more", (9, 0));
        assert_eq!(r, vec![b"\x1b[1;10R".to_vec()]);
    }

    #[test]
    fn ordinary_output_yields_nothing() {
        assert!(responses(b"\x1b[31mred\x1b[0m \x1b[2J \x1b]0;title\x07").is_empty());
    }
}
