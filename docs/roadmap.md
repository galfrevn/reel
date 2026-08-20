# Roadmap

What works today is in the [README](../README.md); how to use it is in
[setup.md](setup.md). This file tracks what's *next*, grouped by theme, in
rough priority order within each group. No dates — items ship when they're
done.

## Capture

- **Script mode** — `type`, `key`, `wait_text`, `wait_idle`, `capture_live`
  for deterministic CLI demos. Waits match against the rendered grid (not the
  byte stream) so scripts don't break on slow machines. Hybrid mode (scripted
  setup, live middle) is the target: boring setup automated, the interesting
  part performed once by hand.
- **`.tape` import** — zero-switching-cost adoption for users coming from
  tape-scripted session generators: translate the styling and, once script
  mode exists, the input ops.
- **Windows validation** — ConPTY capture compiles in CI but has never been
  run by a human. Needs real testing before it's claimed as supported.

## Rendering

- **Raw-render performance** — wall time on long recordings is ~1.5× what
  the fastest existing renderers manage. Rasterization is single-threaded
  and unprofiled; frame-level parallelism is the obvious first win.
- **Exact-palette hit rate** — glyph antialiasing alone generates hundreds of
  fg→bg blend shades, so the lossless 256-color GIF path fires less often
  than designed. Fix: quantize AA ramps to a fixed number of levels per color
  pair so themed content genuinely stays under 256 colors.
- **Gradient auto-flatten for GIF** — gradient canvases (e.g. `glass`) fight
  palette efficiency; reel currently warns. It should auto-flatten to a solid
  (or a small dithered ramp) for GIF targets and say what it did.
- **Sixel / Kitty graphics protocol** — more TUIs render inline images every
  month; supporting them in the VT layer is a real differentiator.

## Output formats

- **APNG / animated WebP** — better than GIF where supported, cheap to add on
  top of the existing frame pipeline.
- **`--frames-out`** — dump raw frames so anyone who needs MP4/H.264 can pipe
  to their own ffmpeg. (Shipping an MP4 encoder is off the table: licensing.)

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
of animated previews to GitHub Pages. Next:

- **`reel template publish`** — validates the TOML, renders the preview
  locally, and scaffolds the pack repo / opens the index PR via `gh`.

Constraints kept on purpose: templates stay declarative TOML (installing a
stranger's template can't execute anything), and packs never bundle fonts
(licensing) — templates reference fonts by name with the system-chain
fallback.

## Distribution

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
