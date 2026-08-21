# Roadmap

What works today is in the [README](../README.md); how to use it is in
[setup.md](setup.md). This file tracks what's *next*, grouped by theme, in
rough priority order within each group. No dates — items ship when they're
done.

## Capture

- **Script mode: hybrid capture** — the scripted path shipped (`reel run`
  with `run`, `type`, `key`/`enter`, `wait_text`, `wait_idle`, `sleep`;
  waits match the rendered grid, not the byte stream, so scripts survive
  slow machines). What's missing is `capture_live`: scripted setup, then
  hand control to a human for the interesting middle. `wait_idle` also needs
  a second look — it can fail to fire on TUIs that repaint continuously.
- **`.tape` import** — zero-switching-cost adoption for users coming from
  tape-scripted session generators (VHS and friends): translate the styling
  and the input ops, which script mode now has equivalents for.
- **Windows validation** — ConPTY capture compiles in CI but has never been
  run by a human. Needs real testing before it's claimed as supported.

## Rendering

- **Raw-render performance** — largely done: rasterization now fans out
  across worker threads while the encoder consumes in order, palette passes
  use an open-addressed color map instead of SipHash, WebM holds stills at
  ~5fps instead of re-encoding every CFR tick, and the hot pixel loops work
  row-wise (4-5× wall-time on the example session for both GIF and WebM).
  Remaining: a single-pass exact-palette GIF (the two-pass rung still
  renders twice), and the CRT template's per-frame f32 blur buffers.
- **Exact-palette hit rate** — glyph antialiasing alone generates hundreds of
  fg→bg blend shades, so the lossless 256-color GIF path fires less often
  than designed. Fix: quantize AA ramps to a fixed number of levels per color
  pair so themed content genuinely stays under 256 colors.
- **Gradient auto-flatten for GIF** — gradient canvases (e.g. `glass`) fight
  palette efficiency; reel currently warns. It should auto-flatten to a solid
  (or a small dithered ramp) for GIF targets and say what it did.
- **Inline graphics (sixel + kitty)** — more TUIs render images every month;
  supporting them in the VT layer is a real differentiator. Sixel decoding
  works today; the kitty half is half-built and parked — see
  [Kitty graphics in recordings](#kitty-graphics-in-recordings-parked-mid-flight).

## Kitty graphics in recordings (parked mid-flight)

The goal: `reel record --graphics -- <a kitty-graphics TUI>` (terminal
browsers, image previewers) produces a video with real images in it. Most of
the plumbing is written and verified on the `feature/markers-and-key-overlay`
branch — not merged to main:

- `reel record --graphics` (opt-in) answers kitty `a=q` capability probes
  with `OK` for `t=d` and refuses `t=s`/`t=f` (shared memory and files are
  gone by render time — the same degradation kitty clients already handle
  over SSH), and answers `CSI 14t/16t/18t` claiming the renderer's 10×20 px
  cell. Verified end-to-end against a terminal browser: it probes shm →
  file → falls back to direct `f=32,o=z` and sizes frames to exactly
  1200×800 on a 120×40 grid.
- The replay decoder (`reel-term/src/graphics.rs`) handles `o=z` (zlib RGBA
  via miniz_oxide), image ids (`i=`), per-id deletes (`a=d,d=I,i=N`), and a
  `MAX_DIM` of 4096 for Retina-sized panes.

What stopped the effort, in order of importance:

1. **Image-only frame changes dedupe away.** `replay()` keeps a snapshot
   only when `content_hash()` (text cells) or `images.len()` changes
   (`reel-term/src/lib.rs`, ~line 339). A TUI that repaints full-pane frames
   under the same image id keeps both constant, so every frame after the
   first is dropped and the video freezes. Fix: fold an image generation
   counter (or a hash of image ids + rgba pointers) into the snapshot-keep
   decision.
2. **Electron apps don't paint page content headless.** Under an
   agent/SSH-style session the UI shell renders (dark toolbar and white text
   pixels show up in the decoded frames) but web contents stay black —
   re-test interactively from a real GUI terminal.
3. Untested: live recording inside an actually-kitty-capable terminal
   (double probe replies are expected and believed harmless; confirm).

Sixel capture needs no flag and already works.

## Annotation

The layer that turns a recording into an explanation shipped: `note` (a
callout anchored to a grid cell, as a card with a leader line or a bubble
with a tail), `card` (a full-frame title or outro that inserts output time),
`highlight style=spotlight|box|underline`, an opt-in `▸▸ 5×` badge over
speed ramps, and an opt-in progress bar notched at every marker. All of it
composites after rasterization, styleable from a template's `[overlay]`
table, and anchored in cells so `zoom`/`pan` carry it.

What's next in the same direction, roughly in order of value per unit of
work:

- **Scroll smoothing** — TUIs jump a line at a time; interpolating the
  scroll between snapshots is the biggest perceived-quality win left, and it
  lives entirely in the rasterizer. It adds frames, so it belongs behind a
  `[motion]` flag like the rest.
- **`zoom auto`** — frame whatever changed, reusing the changed-cell
  tracking that already feeds the typing glow. Removes the coordinate
  guesswork an agent has to do today.
- **`blur`/`pixelate` a region** — the visual half of `redact`, for what no
  pattern can match.
- **Transitions on `cut`** — a short dissolve or flash so a jump reads as
  intentional rather than as a dropped frame.
- **Social aspect presets** — `format = "vertical"`/`"square"` letterboxing
  the window with the caption above it.

## Output formats

- **APNG / animated WebP** — better than GIF where supported, cheap to add on
  top of the existing frame pipeline.
- **`--frames-out`** — shipped. `reel render X --frames-out DIR` writes a
  constant-rate PNG sequence, a `frames.json` carrying the edit (markers,
  captions, cards, zooms, speed ramps, in seconds *and* frame numbers) and a
  synced `audio.wav`. It answers MP4 without an H.264 encoder (licensing
  keeps that off the table) and is the handoff into Remotion or an NLE — see
  the `reel-motion` skill. What's left: an option to hardlink held frames
  instead of copying them, for long demos where the duplication hurts.

## Agent surface

`--json` shipped across every command (one document on stdout, `{"error"}`
on failure, warnings on stderr, nothing interactive), alongside
`documentation/llms.txt` and `reel llms`. What's next in that direction:

- **`suggest`** — deepened: on top of trims, ramps and a freeze it now reads
  the recorded keystrokes (backspace corrections, a typed `exit`), proposes
  `redact` for anything reel's secret scanner sees, marks the biggest burst
  of output after a wait as `@payoff`, zooms it when the region is compact
  enough to magnify without cropping, anchors ops on the cast's own markers,
  and picks the template and container that suit the recording. Speculative
  ops are written commented out with their reason. What's left, in order:
  propose `note`/`caption` placement (not wording — that needs the model,
  not the CLI); recognise the "install → configure → run" shape and draft
  `card` chapter breaks; learn the typo-cut seam well enough to stop hedging
  on it. Nobody without a timeline model can copy any of this.
- **`--json` for `watch`** — a line-delimited event stream (re-render done,
  size, warnings) so an agent can drive an iteration loop.

## Template registry & gallery

The registry lives in this repo (`registry/index.json` + `registry/README.md`
+ the `templates/` seed pack) — packs live in their authors' repos, the index
only points at them (Homebrew-tap model). Shipped so far: `schema = 1` template versioning, path-based `--template`,
the seed pack in `templates/`, `registry/index.json`, the canonical demo
cast (`crates/reel-cli/assets/demo.cast`, embedded), and the
`reel template search` / `try` commands. The registry stays federated —
GitHub is the storage, a static site is the storefront, nothing to run.
Every preview renders the same demo cast, so looks stay comparable. The
static gallery shipped too: `.github/workflows/gallery.yml` runs
`registry/build_gallery.py` on every registry change and publishes the grid
of animated previews to GitHub Pages. And `reel template publish` closes the
loop: validate → local preview → scaffold the pack's `templates/` dir →
update the index (in place when run from this repo, via a `gh`-driven
fork/branch/PR otherwise, with a printed manual route when `gh` is absent).
The registry feature set is complete; what remains is operational — merge to
main, enable Pages (Settings → Pages → Source: GitHub Actions), and grow the
index.

Constraints kept on purpose: templates stay declarative TOML (installing a
stranger's template can't execute anything), and packs never bundle fonts
(licensing) — templates reference fonts by name with the system-chain
fallback.

## Distribution

- **Agent-first install** — the skill now installs the binary itself from
  GitHub Releases, so `npx skills add galfrevn/reel` is the entire setup.
  Next: a `reel doctor` that reports fonts, codecs, and PATH in one place so
  the agent can diagnose a broken install instead of guessing.
- **GitHub Action** — re-introduce a composite action (`uses: galfrevn/reel`)
  that renders `.reel`/`.cast` files in CI so README demos never go stale.
  Removed from the repo until the CLI surface stabilizes; the setup script
  already makes CI installs a one-liner.
- **Package managers** — Homebrew tap and/or crates.io publish once the
  binary name situation (`reel` is a common word) is resolved.

## Non-goals

Kept here so they don't creep back in — see [idea.md](idea.md) for the
reasoning: TUI testing framework, general-purpose scripting language, desktop
screen recording, hosted sharing of user videos (the template registry is
GitHub-federated + static — no backend).
