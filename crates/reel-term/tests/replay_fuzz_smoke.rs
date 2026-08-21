//! Deterministic adversarial coverage for `replay()`: seeded pseudo-random
//! byte soup, escape fragments, and truncated sequences must never panic,
//! hang, or produce snapshots with mismatched dimensions. Not a substitute
//! for real fuzzing, but it runs on every CI pass.

use reel_cast::Cast;

/// xorshift64* — deterministic across platforms, no dependencies.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    fn byte(&mut self) -> u8 {
        (self.next() >> 56) as u8
    }
}

fn cast_with_output(chunks: &[String]) -> Cast {
    let mut text = String::from("{\"version\": 2, \"width\": 40, \"height\": 12}\n");
    for (i, chunk) in chunks.iter().enumerate() {
        let event = serde_json::to_string(&(0.1 * (i + 1) as f64, "o", chunk)).unwrap();
        text.push_str(&event);
        text.push('\n');
    }
    Cast::parse(&text).expect("generated cast is structurally valid")
}

fn assert_uniform(snaps: &[reel_term::Snapshot]) {
    for s in snaps {
        assert_eq!((s.cols, s.rows), (40, 12), "snapshot dims must stay uniform");
        assert_eq!(s.cells.len(), 40 * 12);
    }
}

#[test]
fn random_byte_soup_never_panics() {
    let mut rng = Rng(0xDEADBEEF);
    for round in 0..20 {
        let chunks: Vec<String> = (0..8)
            .map(|_| {
                let bytes: Vec<u8> = (0..200).map(|_| rng.byte()).collect();
                String::from_utf8_lossy(&bytes).into_owned()
            })
            .collect();
        let snaps = reel_term::replay(&cast_with_output(&chunks))
            .unwrap_or_else(|e| panic!("round {round}: {e}"));
        assert_uniform(&snaps);
    }
}

#[test]
fn escape_fragments_and_truncations_never_panic() {
    // The nasty corners by construction: unterminated DCS/APC/OSC/CSI,
    // split multi-byte UTF-8, sixel fragments, kitty fragments, huge
    // parameters, and a mid-sequence end of recording.
    let cases = [
        "\u{1b}P",
        "\u{1b}Pq#1;2;100;0;0~~~",
        "\u{1b}_Ga=T,f=32,s=4,v=4;QUJD",
        "\u{1b}]11;rgb:11/22",
        "\u{1b}[999999999999;999999999999H",
        "\u{1b}[38;2;1;2",
        "text \u{1b}",
        "\u{1b}[?2026h locked away",
        "wide 世界 split \u{fffd}\u{fffd}",
        "\u{1b}[4:3m\u{1b}[58:2::255:0:0m squiggle",
    ];
    for case in cases {
        let snaps = reel_term::replay(&cast_with_output(&[case.to_string()]))
            .unwrap_or_else(|e| panic!("case {case:?}: {e}"));
        assert_uniform(&snaps);
    }
}

#[test]
fn structured_garbage_with_real_escapes_never_panics() {
    let mut rng = Rng(0xC0FFEE);
    let intros = ["\u{1b}[", "\u{1b}]", "\u{1b}P", "\u{1b}_G", "\u{1b}"];
    for round in 0..20 {
        let chunks: Vec<String> = (0..6)
            .map(|_| {
                let mut s = String::new();
                for _ in 0..30 {
                    s.push_str(intros[(rng.next() % intros.len() as u64) as usize]);
                    for _ in 0..(rng.next() % 12) {
                        s.push((0x20 + (rng.byte() % 0x5f)) as char);
                    }
                }
                s
            })
            .collect();
        let snaps = reel_term::replay(&cast_with_output(&chunks))
            .unwrap_or_else(|e| panic!("round {round}: {e}"));
        assert_uniform(&snaps);
    }
}
