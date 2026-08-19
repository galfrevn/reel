---
name: reel-dev
description: Architecture rules, build order, and testing conventions for developing the reel codebase itself (the Rust workspace in this repository). Use this skill whenever implementing features, fixing bugs, adding crates, or planning work in reel — especially anything touching capture, rendering, encoding, the .reel format, audio, or the remaining roadmap phases (audio/WebM, own capture, script mode). Consult it before writing code, not after.
---

# Developing reel

reel is a Rust workspace that turns terminal recordings into styled, edited
GIFs. `docs/SPEC.md` is the authoritative design document — read the relevant
section before implementing anything in its area. This skill is the map: what
exists, what's next, the rules that must not be broken, and how to verify work.

## The one hard rule

**Capture and render never touch.** The `.cast` file is the boundary. Once a
session is recorded, the program is never executed again; everything
downstream is a pure function of `(cast, reel file)`. This rule is what makes
`reel watch`, cheap iteration, and deterministic output possible. If a design
shortcut requires violating it, the design is wrong — stop and rethink.

Two corollaries:

- Timeline op timestamps are **source time** (the recording's clock), resolved
  to output time in `reel-timeline`. New ops follow the same convention.
- Output must be deterministic: same cast + same `.reel` → byte-identical
  frames. Don't introduce wall-clock time, randomness, or system-font
  dependence into the render path.

## Workspace map

| Crate | Owns |
|---|---|
| `reel-cast` | asciinema v2 cast parsing |
| `reel-term` | VT emulation (`alacritty_terminal`) → grid snapshots |
| `reel-format` | `.reel` parsing: TOML front-matter + script ops, line-numbered errors |
| `reel-timeline` | Resolving edit ops (trim/cut/speed/hold/freeze) into a frame plan; visual op scheduling |
| `reel-render` | Rasterization (cosmic-text/swash), templates, themes, chrome, zoom compositing |
| `reel-encode` | GIF (frame dedup, exact palette, delta rects, budget search), PNG, txt |
| `reel-cli` | clap CLI: `render`, `watch`, `shot`, `inspect`, `init`, `templates`, `themes` |

## What is done vs. remaining

**Done (Phases 0–1):** cast → grid → timeline editing (trim, cut, speed, hold,
freeze, zoom, pan, caption, highlight, marker) → templates (minimal, glass,
classic, geist, paper) → GIF/PNG/txt with size budget → `watch --serve`.

**Remaining, in spec build order (SPEC.md §11):**

1. **Phase 1.5 — Audio + WebM** (~1.5 wk): VP9/Opus/WebM statically linked;
   audio as a timestamped *event list* mixed after timeline resolution (never
   a pre-rendered waveform — speed-ramping must drop events, not pitch-shift);
   keyboard sample profiles; auto UI cues from grid diffs; thinking beds.
   `reel-format` already parses `[audio]` and `sound`/`mute`/`volume` — wire
   them up, don't redesign them.
2. **Phase 2 — Own capture** (~3 wk): `reel record` via `portable-pty`;
   answering terminal queries (DA1/DA2/DSR/OSC 10/11/kitty — SPEC §9, the
   hard week); Nerd Font embedding with PUA fallback; `.reelmeta` sidecar
   with `input_events` for keystroke audio.
3. **Phase 3 — Breadth**: script mode (`type`, `key`, `wait_idle`,
   `wait_text`, `capture_live`); VHS `.tape` import; theme importers (base16,
   iTerm2, Alacritty); community template registry; Windows/ConPTY; GitHub
   Action.

Also unimplemented from the spec: `crt` template (needs the shader path),
APNG/WebP outputs, `reel template add`/`show`, `reel theme add`.

**Scope discipline (SPEC §1):** no TUI testing framework, no scripting
language, no screen recording, no hosting service. Push back on features that
drift there.

## How to verify work

```sh
cargo test                 # unit tests live in each crate's src (52 today); add yours alongside
cargo run -- render examples/demo.reel        # end-to-end smoke test
cargo run -- render examples/demo.cast -o /tmp/raw.gif   # bare-cast path
cargo run -- inspect examples/demo.reel
```

- `examples/demo.cast` is the checked-in fixture; `examples/make_fixture.py`
  regenerates it if the format needs richer material.
- For render changes, don't eyeball GIFs only: render to `.txt` for grid
  content, and `reel shot --at Ts` + reading the PNG for pixel checks.
- Parser changes: every error must carry the script line number and say what
  was expected — see existing `FormatError` messages for tone.
- Encoder changes: re-run the honest benchmark against `agg` and update
  `docs/COMPARISON.md`; size/quality claims in the README must stay true.
- Emulation work (Phase 2): validate against the fixed list — `opencode`,
  `lazygit`, `k9s`, `btop`, `yazi`, `helix` (SPEC §9.3).

## Conventions

- Follow the installed Rust skills (`rust-best-practices`, `rust-patterns`,
  `rust-testing`) for idiom, ownership, and test style.
- Errors: `thiserror` enums in library crates, `anyhow` with context only in
  `reel-cli`. Fail loudly with actionable messages (see the `wait_*` timeout
  rule in SPEC §5.4 for the spirit).
- Dependencies are chosen in SPEC §3.2 — don't substitute crates without a
  reason worth writing down.
- New user-facing behavior updates `README.md`; design changes update
  `docs/SPEC.md` in the same PR.
