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

The seed exists: `reel template add owner/repo[/name]` already installs from
any GitHub repo with a `templates/` directory. The registry stays federated —
GitHub is the storage, a static site is the storefront, nothing to run. The
key trick throughout: reel renders its own previews, so every template is
shown against the same canonical demo cast — consistent, comparable, never
stale.

1. **Index repo** — `galfrevn/reel-registry` with a versioned `index.json`
   (name, author, source repo, description, tags) pointing at packs that live
   in their authors' repos. Publishing = a PR against the index (Homebrew-tap
   model). Ships with the canonical demo `.cast`: typing, ANSI color, a diff,
   tests going green.
2. **CLI: `search` + `try`** — `reel template search <query>` fetches the
   index; `reel template try owner/repo/name` downloads to a temp dir,
   renders the bundled demo cast with it, and opens the result — preview the
   look without touching the config dir.
3. **Static gallery** — a GitHub Action in the registry repo renders every
   template against the canonical cast on merge and publishes a GitHub Pages
   grid of animated previews, each with its `reel template add …` install
   line. First real consumer of the composite render Action below.
4. **CLI: `publish`** — validates the TOML, renders the preview locally, and
   scaffolds the pack repo / opens the index PR via `gh`.

Prerequisite: version the template TOML schema (`schema = 1`) before
third-party templates exist in the wild — every field added after that is a
compatibility question. Templates stay declarative TOML (no code execution),
which is what keeps installing a stranger's template trivially safe. Packs
never bundle fonts (licensing); templates reference fonts by name with the
system-chain fallback.

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
