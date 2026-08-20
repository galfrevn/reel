//! Redaction: mask grid cells whose row text matches a pattern, plus a
//! scanner that warns about likely secrets before they ship in a demo.

use crate::{CellAttrs, Snapshot};
use regex_lite::Regex;

/// The character redacted cells show.
const MASK: char = '•';

/// Masks every match of `re` in every snapshot, in place. Matching runs on
/// each row's text (wide chars count once; their spacer cells mask too).
pub fn apply(snapshots: &mut [Snapshot], re: &Regex) {
    for snap in snapshots.iter_mut() {
        let cols = snap.cols as usize;
        for row in 0..snap.rows as usize {
            // Row text + map from char index back to cell column.
            let mut text = String::with_capacity(cols);
            let mut col_of_char: Vec<usize> = Vec::with_capacity(cols);
            for col in 0..cols {
                let cell = snap.cells[row * cols + col];
                if cell.attrs.contains(CellAttrs::WIDE_SPACER) {
                    continue;
                }
                text.push(cell.ch);
                col_of_char.push(col);
            }
            for m in re.find_iter(&text) {
                let start_char = text[..m.start()].chars().count();
                let match_chars = text[m.start()..m.end()].chars().count();
                for k in start_char..start_char + match_chars {
                    let Some(&col) = col_of_char.get(k) else { continue };
                    let idx = row * cols + col;
                    let wide = snap.cells[idx].attrs.contains(CellAttrs::WIDE);
                    snap.cells[idx].ch = MASK;
                    snap.cells[idx].attrs -= CellAttrs::WIDE;
                    if wide && col + 1 < cols {
                        snap.cells[idx + 1].ch = MASK;
                        snap.cells[idx + 1].attrs -= CellAttrs::WIDE_SPACER;
                    }
                }
            }
        }
    }
}

/// Patterns that usually mean "you don't want this in a published demo".
const SUSPECT: &[(&str, &str)] = &[
    ("email address", r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}"),
    ("API key/token", r"\b(?:sk-|ghp_|gho_|xox[bap]-|AKIA)[A-Za-z0-9_-]{8,}"),
    ("bearer token", r"(?i)bearer\s+[A-Za-z0-9._~+/-]{16,}"),
    ("URL with a long id", r"https?://\S*/[A-Za-z0-9_-]{16,}\S*"),
    // Opaque prefixed ids (wrk_, ses_, org_, acct_…) — catches wrapped URLs
    // whose scheme landed on the previous grid row.
    ("opaque resource id", r"\b[a-z]{2,6}_[A-Za-z0-9]{16,}\b"),
];

/// Scans all snapshots for likely secrets; returns up to `cap` distinct
/// (kind, sample) findings for the caller to warn about.
pub fn scan_sensitive(snapshots: &[Snapshot], cap: usize) -> Vec<(String, String)> {
    let regs: Vec<(&str, Regex)> = SUSPECT
        .iter()
        .filter_map(|(kind, pat)| Regex::new(pat).ok().map(|r| (*kind, r)))
        .collect();
    // Rows repeat massively across snapshots; scan each distinct row once.
    let mut seen_rows = std::collections::HashSet::new();
    let mut found: Vec<(String, String)> = Vec::new();
    for snap in snapshots {
        let cols = snap.cols as usize;
        for row in 0..snap.rows as usize {
            let text: String = snap.cells[row * cols..(row + 1) * cols]
                .iter()
                .filter(|c| !c.attrs.contains(CellAttrs::WIDE_SPACER))
                .map(|c| c.ch)
                .collect();
            let text = text.trim_end();
            if text.is_empty() || !seen_rows.insert(text.to_string()) {
                continue;
            }
            for (kind, re) in &regs {
                if let Some(m) = re.find(text) {
                    let sample = m.as_str().chars().take(48).collect::<String>();
                    if !found.iter().any(|(_, s)| s == &sample) {
                        found.push((kind.to_string(), sample));
                        if found.len() >= cap {
                            return found;
                        }
                    }
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cell, Cursor, CursorShape};

    fn snap(lines: &[&str]) -> Snapshot {
        let cols = 40u16;
        let rows = lines.len() as u16;
        let mut cells = vec![Cell::default(); cols as usize * rows as usize];
        for (r, line) in lines.iter().enumerate() {
            for (c, ch) in line.chars().enumerate() {
                cells[r * cols as usize + c].ch = ch;
            }
        }
        Snapshot {
            src_time: 0.0,
            cols,
            rows,
            cells,
            cursor: Cursor { col: 0, row: 0, shape: CursorShape::Block },
            palette_overrides: vec![],
        }
    }

    #[test]
    fn masks_matches_and_leaves_the_rest() {
        let mut snaps = vec![snap(&["token: sk-abcd1234efgh5678 ok"])];
        let re = Regex::new(r"sk-[A-Za-z0-9]+").unwrap();
        apply(&mut snaps, &re);
        let text: String = (0..40).map(|c| snaps[0].cell(c, 0).ch).collect();
        assert!(text.starts_with("token: ••••••••••••••••••• ok"), "{text:?}");
    }

    #[test]
    fn scanner_flags_secrets_once() {
        let snaps = vec![
            snap(&["mail me: dev@example.com", "https://x.io/wrk_0123456789abcdef99"]),
            snap(&["mail me: dev@example.com"]), // repeated row: no dup finding
        ];
        let found = scan_sensitive(&snaps, 5);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().any(|(k, _)| k == "email address"));
    }

    #[test]
    fn clean_content_stays_silent() {
        let snaps = vec![snap(&["$ cargo test", "test result: ok"])];
        assert!(scan_sensitive(&snaps, 5).is_empty());
    }
}
