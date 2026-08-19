---
name: reel-demo
description: Create polished terminal demo GIFs from asciinema recordings using the reel CLI — record once, edit the timeline (trim, cut, speed-ramp, zoom, caption), restyle with templates, and render to a size-budgeted GIF without re-running the program. Use this skill whenever the user wants a terminal demo, a README GIF, a TUI/CLI recording turned into a shareable animation, wants to trim or speed up a terminal recording, mentions asciinema/.cast files, .reel files, or asks to make a demo of their command-line tool look good — even if they never say the word "reel".
---

# Making terminal demos with reel

reel turns an asciinema `.cast` recording into a styled, edited GIF. The core
promise: **the recorded program runs exactly once**. Every edit — trimming dead
air, speeding up slow parts, changing theme or template — is a pure re-render
of the frozen recording, so iterating is free and takes milliseconds. Never ask
the user to re-record just to change styling or timing.

The user should not need to learn reel's file format or CLI. You drive the
whole workflow: inspect the recording, decide the edits, write the `.reel`
file, render, check the result, iterate.

## Prerequisites

reel is built from this repository. If `reel` is not on PATH, build and use
the debug binary:

```sh
cargo build --release
./target/release/reel --help    # or: cargo run -- <args>
```

Recording requires `asciinema` (reel's own recorder is not implemented yet):

```sh
asciinema rec session.cast -- <the command to demo>
```

## Workflow

### 1. Get a recording

If the user already has a `.cast` file, use it. Otherwise have them record one
with `asciinema rec` (you cannot drive interactive TUIs for them — recording
is the one step that may need the user's hands). A quick sanity render checks
the recording is usable:

```sh
reel render session.cast -o preview.gif
```

### 2. Understand the recording before editing

Run `reel inspect` on the cast (or on a `.reel` file referencing it) to see
duration, terminal size, and event density:

```sh
reel inspect session.cast
```

Look for the classic problems worth fixing — this is where the value is:

- **Dead air**: long idle stretches (LLM thinking, installs, waits) → `speed`
  or `cut`.
- **Slow start / trailing junk**: shell prompt setup, the final `exit` → `trim`.
- **Typos and mistakes**: → `cut` the range.
- **The money shot**: the moment worth magnifying → `zoom`, `caption`,
  `highlight`.
- **Looping**: GIFs loop; a `freeze last 1.5s` gives the eye a resting point
  before the restart.

### 3. Write the `.reel` file

A `.reel` file is TOML front-matter between `---` fences, followed by one
timeline operation per line. Minimal example:

```
---
[source]
cast = "session.cast"

[template]
name = "glass"

[output]
file   = "demo.gif"
budget = "800kb"
---

trim    2s..end
cut     19s..23s
speed   5x from 8s to 34s
caption "Refactor the auth module" at 4s for 2.5s
zoom    1.8x at (30,10) from 36s to 41s
freeze  last 1.5s
```

Timeline op timestamps use the **source clock** (the recording's own time,
before any edits are applied), so ops never have to be re-computed when you
add or reorder other ops.

For the full front-matter and operation reference (all ops, argument grammar,
time syntax, templates, themes, style overrides), read
[references/reel-file-format.md](references/reel-file-format.md).

### 4. Render and verify

```sh
reel render demo.reel
```

Verify the output before showing it to the user:

- Check the reported file size against what the demo is for (README GIFs
  should generally stay under ~1–2 MB; set `budget` and let the encoder fit it).
- Open the GIF (macOS: `open demo.gif`) or render a spot-check frame:
  `reel shot demo.reel --at 12s -o check.png` and read the PNG to confirm
  captions/zooms land where intended.
- `reel render file.reel -o out.txt` dumps the final grid as plain text —
  useful to verify content without looking at pixels.

### 5. Iterate

Re-rendering is sub-second; adjust ops freely. For a live feedback loop while
hand-tuning, `reel watch demo.reel --serve` re-renders on save and serves a
browser preview at `http://127.0.0.1:4171/` — offer this when the user wants
to fiddle with the look themselves.

## Choosing a template

`reel templates` lists them. Pick by destination, don't ask the user unless
they care:

| Template | Use when |
|---|---|
| `glass` | Default choice — gradient canvas, rounded chrome, soft shadow |
| `minimal` | High-contrast, no decoration; technical docs |
| `classic` | Bare terminal, no chrome; embedding where chrome would clash |
| `geist` | Vercel-docs-style dark look, Geist Mono |
| `paper` | Light background; daytime documentation sites |

Gradient backgrounds (like `glass`) cost GIF palette efficiency. If the size
budget is tight and quality suffers, switch to `minimal`/`classic` or set a
solid background — mention the tradeoff to the user.

## Current limits (do not promise these)

- Output formats today: `.gif`, `.png` (via `shot`), `.txt`. **No WebM/MP4 and
  no audio yet** — the parser accepts `[audio]` config and `sound`/`mute`/
  `volume` ops without failing, but nothing is rendered. Don't add them.
- No script mode (`type`, `key`, `wait_*`): reel cannot drive a program; it
  only edits existing recordings.
- `reel record` does not exist yet; use `asciinema rec`.
