//! Keystroke display labels: turns the raw bytes of one recorded input
//! event into the chips a keystroke overlay shows (screenkey-style).
//!
//! Recorded stdin also carries terminal *responses* (DA1, DSR, OSC replies)
//! forwarded through the same channel — unknown escape sequences are dropped
//! rather than guessed at, so reports never show up as phantom keys.

/// Longest printable run shown as one chip before truncating with `…`.
const MAX_RUN: usize = 20;

/// Labels for one input event's data. A single event may produce several
/// chips (a fast batch like `"ls\r"` → `["ls", "⏎"]`) or none (a terminal
/// response).
pub fn chips(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut run = String::new();
    let mut chars = value.chars().peekable();
    let flush = |run: &mut String, out: &mut Vec<String>| {
        if run.is_empty() {
            return;
        }
        let label = if run == " " {
            "␣".to_string()
        } else if run.chars().count() > MAX_RUN {
            let mut s: String = run.chars().take(MAX_RUN).collect();
            s.push('…');
            s
        } else {
            run.clone()
        };
        out.push(label);
        run.clear();
    };
    while let Some(c) = chars.next() {
        match c {
            '\r' | '\n' => {
                flush(&mut run, &mut out);
                out.push("⏎".into());
            }
            '\t' => {
                flush(&mut run, &mut out);
                out.push("⇥".into());
            }
            '\x7f' => {
                flush(&mut run, &mut out);
                out.push("⌫".into());
            }
            '\x1b' => {
                flush(&mut run, &mut out);
                if let Some(label) = escape_label(&mut chars) {
                    out.push(label);
                }
            }
            c if (c as u32) < 0x20 => {
                flush(&mut run, &mut out);
                // ^A..^Z (0x01..0x1a); other control bytes are dropped.
                if ('\x01'..='\x1a').contains(&c) {
                    out.push(format!("^{}", (c as u8 + 0x40) as char));
                }
            }
            c => run.push(c),
        }
    }
    flush(&mut run, &mut out);
    out
}

/// Consumes one escape sequence after ESC and names it if it's a key.
/// Unknown sequences (terminal reports, OSC replies) consume their bytes and
/// return `None`.
fn escape_label(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<String> {
    match chars.peek() {
        // Bare ESC (nothing follows in this event).
        None => Some("esc".into()),
        Some('[') | Some('O') => {
            chars.next();
            // CSI/SS3: parameter bytes then one final byte in 0x40..0x7e.
            let mut body = String::new();
            for c in chars.by_ref() {
                body.push(c);
                if ('\u{40}'..='\u{7e}').contains(&c) && !matches!(c, ';' | '[') && !c.is_ascii_digit() {
                    break;
                }
            }
            let label = match body.as_str() {
                "A" => "↑",
                "B" => "↓",
                "C" => "→",
                "D" => "←",
                "H" => "Home",
                "F" => "End",
                "Z" => "⇤", // back-tab
                "3~" => "Del",
                "5~" => "PgUp",
                "6~" => "PgDn",
                "P" => "F1",
                "Q" => "F2",
                "R" => "F3",
                "S" => "F4",
                _ => return None, // reports, modifiers, OSC tails: not a key
            };
            Some(label.into())
        }
        // ESC ] (OSC reply) and friends: swallow everything, name nothing.
        Some(_) => {
            chars.next();
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_text_groups_into_one_chip() {
        assert_eq!(chips("ls -la"), vec!["ls -la"]);
        assert_eq!(chips("ls -la\r"), vec!["ls -la", "⏎"]);
    }

    #[test]
    fn special_keys_get_symbols() {
        assert_eq!(chips("\r"), vec!["⏎"]);
        assert_eq!(chips("\t"), vec!["⇥"]);
        assert_eq!(chips("\x7f"), vec!["⌫"]);
        assert_eq!(chips(" "), vec!["␣"]);
        assert_eq!(chips("\x03"), vec!["^C"]);
        assert_eq!(chips("\x1b"), vec!["esc"]);
    }

    #[test]
    fn arrows_and_navigation() {
        assert_eq!(chips("\x1b[A"), vec!["↑"]);
        assert_eq!(chips("\x1b[D"), vec!["←"]);
        assert_eq!(chips("\x1b[5~"), vec!["PgUp"]);
        assert_eq!(chips("\x1bOP"), vec!["F1"]);
    }

    #[test]
    fn terminal_reports_are_dropped() {
        assert!(chips("\x1b[?1;2c").is_empty(), "DA1 response is not a key");
        assert!(chips("\x1b[24;80R").is_empty(), "DSR response is not a key");
    }

    #[test]
    fn long_pastes_truncate() {
        let long = "x".repeat(50);
        let c = chips(&long);
        assert_eq!(c.len(), 1);
        assert!(c[0].ends_with('…') && c[0].chars().count() == MAX_RUN + 1);
    }
}
