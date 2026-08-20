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
- **VHS `.tape` import** — zero-switching-cost adoption for existing VHS
  users: translate the styling and, once script mode exists, the input ops.
- **Windows validation** — ConPTY capture compiles in CI but has never been
  run by a human. Needs real testing before it's claimed as supported.

## Rendering

- **Raw-render performance** — wall time is ~1.5× agg on long recordings.
  Rasterization is single-threaded and unprofiled; frame-level parallelism is
  the obvious first win.
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
screen recording, hosted sharing service.
