---
name: reel-demo
description: Turn terminal recordings into polished, shareable demo GIFs using the reel CLI — record a session once with asciinema, then edit it like video (trim dead air, cut mistakes, speed-ramp slow parts, zoom, caption) and render a styled, size-budgeted GIF without ever re-running the program. Use this skill whenever the user wants a demo of their CLI or TUI, a GIF for a README, launch tweet, or docs, wants to shorten/clean up/restyle a terminal recording, mentions asciinema or .cast files, or asks to "record my terminal" or "make a demo" of a command-line tool — even if they never mention reel by name.
---

# Making terminal demos with reel

reel turns an asciinema `.cast` recording into a styled, edited GIF. The core
promise: **the recorded program runs exactly once**. Every edit — trimming
dead air, speeding up slow parts, changing theme or template — is a pure
re-render of the frozen recording, so iterating takes milliseconds and costs
nothing. Never ask the user to re-record just to change styling or timing.

The user should not need to learn reel's file format or CLI. You drive the
whole workflow: inspect the recording, decide the edits, write the `.reel`
file, render, check the result, iterate. Bring the user in only for the two
things you can't do: performing the live session, and judging taste.

## Setup

Check for the tools; install whichever is missing:

```sh
reel --version      || curl -fsSL https://raw.githubusercontent.com/galfrevn/reel/main/setup.sh | bash
asciinema --version || brew install asciinema   # or: pipx install asciinema
```

asciinema is only needed for recording (reel's own recorder isn't shipped
yet). If the user already has a `.cast` file, reel alone is enough.

## Workflow

### 1. Get a recording

If the user has a `.cast` file, use it. Otherwise they must record one —
this is the one step that needs their hands, since demos usually show an
interactive session. Give them the exact command and what to do in it:

```sh
asciinema rec session.cast -- <the command to demo>
```

Tell them: perform the demo naturally and don't worry about pauses, typos, or
pacing — all of that gets fixed in the edit. Ctrl+D or `exit` ends the
recording. A non-interactive demo (e.g. showing a build or install) you can
record yourself.

Then sanity-check the recording with a default render:

```sh
reel render session.cast -o preview.gif
```

### 2. Understand the recording before editing

```sh
reel inspect session.cast
```

shows duration, terminal size, and where the visible changes are. Look for
the classic problems — fixing these is where the value is:

- **Dead air**: long idle stretches (thinking, installs, waits) → `speed` or `cut`.
- **Slow start / trailing junk**: prompt setup at the head, the final `exit` → `trim`.
- **Typos and mistakes** → `cut` the range.
- **The money shot**: the moment worth magnifying → `zoom`, `caption`, `highlight`.
- **Looping**: GIFs loop; `freeze last 1.5s` gives the eye a resting point
  before the restart.

### 3. Write the `.reel` file

A `.reel` file is TOML front-matter between `---` fences, followed by one
timeline operation per line:

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

Timestamps use the **source clock** (the recording's own time, before edits),
so ops never need recomputing when you add or reorder others.

For the complete reference — every operation, argument grammar, time syntax,
templates, themes, style overrides — read
[references/reel-file-format.md](references/reel-file-format.md).

### 4. Render and verify

```sh
reel render demo.reel
```

Verify before showing the user:

- Check the reported size against the destination (README GIFs should stay
  under ~1–2 MB; set `budget` and the encoder fits it).
- Spot-check frames: `reel shot demo.reel --at 12s -o check.png`, then read
  the PNG to confirm captions and zooms land where intended.
- `reel render demo.reel -o out.txt` dumps the final grid as text — verifies
  content without looking at pixels.

Then send the user the GIF, with one line on what you cut/sped/zoomed and the
final size and duration.

### 5. Iterate

Re-rendering is sub-second, so treat feedback as free: adjust ops and
re-render. If the user wants to fiddle with the look themselves, offer
`reel watch demo.reel --serve` — it re-renders on save with a live browser
preview at `http://127.0.0.1:4171/`.

## Choosing a template

`reel templates` lists them. Pick by destination; only ask if the user has
expressed taste:

| Template | Use when |
|---|---|
| `glass` | Default — gradient canvas, rounded chrome, soft shadow |
| `minimal` | High contrast, no decoration; technical docs |
| `classic` | Bare terminal, no chrome; embeds where chrome would clash |
| `geist` | Pure-black Vercel-docs look, Geist Mono |
| `paper` | Light background; daytime documentation sites |

Gradient backgrounds (like `glass`) cost GIF palette efficiency. If a tight
size budget starts visibly degrading quality, switch to a solid-canvas
template (`minimal`, `classic`, `geist`) and tell the user why.

## Current limits (don't promise these)

- Outputs today: `.gif`, `.png` (via `shot`), `.txt`. **No WebM/MP4 video and
  no audio yet** — the parser tolerates `[audio]` config and `sound`/`mute`/
  `volume` ops, but nothing is rendered. Don't write them.
- reel cannot type into or drive a program (no script mode); it only edits
  existing recordings.
- `reel record` doesn't exist yet; recording goes through `asciinema rec`.
