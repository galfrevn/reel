# reel

> Your terminal demo, edited like video.

Record a terminal session once, then treat it as a timeline you can cut,
speed-ramp, zoom, caption, and restyle — re-rendering in milliseconds without
ever re-running the underlying program.

**Status: early development.** The renderer and timeline editor (Phases 0–1 of
the [spec](docs/SPEC.md)) work end to end: asciinema cast in, styled GIF out.
Own capture, `reel watch`, WebM/audio, and script mode are on the roadmap.

## How it works

```
session.cast ──▶ VT emulation ──▶ grid snapshots ──▶ timeline ops ──▶ rasterize ──▶ compose ──▶ encode
               (alacritty_terminal)                (trim/cut/speed/   (swash +      (chrome,     (GIF,
                                                    zoom/caption)      glyph cache)   shadow)      PNG)
```

The hard rule: **capture and render never touch.** Once a session is recorded,
the program is never executed again. Changing the theme, font, template, zoom,
or edits is a pure re-render — no LLM re-runs, no flaky re-recordings.

## Quick start

```sh
# Record with asciinema (reel's own recorder comes later)
asciinema rec session.cast -- your-tui

# Instant render with default styling
reel render session.cast

# Or scaffold an edit file and shape the timeline
reel init glass
reel render demo.reel
```

A `.reel` file is TOML front-matter plus a newline-delimited edit script:

```
---
[source]
cast = "session.cast"

[template]
name = "glass"

[output]
file   = "demo.gif"
budget = "800kb"        # the encoder degrades predictably to fit
---

trim    2s..end
cut     19s..23s                  # remove the typo
speed   5x from 8s to 34s         # compress the model's thinking pause
caption "Refactor the auth module" at 4s for 2.5s
zoom    1.8x at (30,10) from 36s to 41s
freeze  last 1.5s
```

All timestamps refer to the **recording's own clock** (source time), so edits
stay valid as you add or remove other edits. Bare durations (`for 2.5s`,
`freeze last 1.5s`) are output time — what the viewer experiences.

## Commands

```
reel render FILE      # .reel or .cast → .gif / .png / .txt
reel shot FILE --at T # single frame PNG
reel inspect FILE     # timeline summary
reel init [template]  # scaffold a .reel file
reel templates        # list built-in templates
reel themes           # list built-in themes
```

## What's here today

- asciinema v2 cast parsing (+ `.reelmeta` sidecar format)
- Full VT emulation via `alacritty_terminal`: alt screen, wide chars,
  synchronized output (`?2026`), OSC 4 palette overrides
- Timeline ops: `trim`, `cut`, `speed`, `hold`, `freeze`, `zoom`, `pan`,
  `caption`, `highlight`, `marker`
- Templates: `minimal`, `glass`, `classic`, `paper` — window chrome, drop
  shadows, gradient canvases
- Embedded JetBrains Mono NL Nerd Font (4 variants) — TUI icon glyphs render
  correctly everywhere, byte-identical output across machines
- Change-driven GIF encoding: frames on grid change (not a clock), exact
  palette when content fits 256 colors, delta rectangles, greedy budget
  ladder with a report of what it chose
- Zoom re-rasterizes glyphs at the target size — text stays sharp

## Building

```sh
cargo build --release   # single binary at target/release/reel
cargo test --workspace
```

Fonts are embedded at build time from `assets/fonts/` (SIL OFL, license
vendored alongside).

## License

MIT. Embedded fonts are licensed under the SIL Open Font License — see
`assets/fonts/OFL.txt`.
