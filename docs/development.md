# Developing reel

How to build, test, and navigate the codebase. For what reel *is*, read
[idea.md](idea.md) first — especially the one hard rule (capture and render
never touch); it explains most of the architecture.

## Prerequisites

- **Rust** (stable) — the workspace uses edition 2021.
- **libvpx** — linked at build time for WebM/VP9 output:
  `brew install libvpx` (macOS) or `apt install libvpx-dev` (Debian/Ubuntu).
  If pkg-config can't find it, point `PKG_CONFIG_PATH` at its
  `lib/pkgconfig` directory.

No libvpx handy? Build everything except `.webm` output in pure Rust —
audio synthesis included:

```sh
cargo build --no-default-features -p reel-cli
```

## Build and test

```sh
cargo build --release      # single binary at target/release/reel
cargo test --workspace
```

Rendering uses the fonts installed on the machine — no fonts in the repo or
binary. Install a Nerd Font if you're testing TUI content with icon glyphs;
without one, expect tofu boxes (that's correct behavior, not a bug).

## Workspace layout

The pipeline is one crate per stage, in data-flow order:

| Crate | Stage |
|---|---|
| `reel-cast` | asciinema v2 cast + `.reelmeta` sidecar parsing |
| `reel-term` | VT emulation (`alacritty_terminal`) → grid snapshots; typing repair |
| `reel-timeline` | timeline ops (`trim`/`cut`/`speed`/`zoom`/…) → virtual frame list |
| `reel-format` | `.reel` file parsing: TOML front-matter + script body |
| `reel-render` | rasterization (swash + glyph cache), chrome, effects, compositing |
| `reel-encode` | GIF (change-driven, exact palette, delta rects) and WebM (VP9 + Opus, in-house muxer); size-budget ladder |
| `reel-audio` | event-list audio model, procedural synthesis recipes, mixer |
| `reel-cli` | `clap` CLI tying it together: record, render, watch, shot, inspect, templates, themes |

The dependency direction follows that order; upstream crates never know
about downstream ones. In particular, nothing in capture (`reel-cast`,
`record` in the CLI) may depend on rendering, and vice versa.

## Demo assets

The GIFs embedded in the README live in `assets/demos/` and are committed
(they're the exception to the `*.gif` ignore rule in `.gitignore`). They were
rendered with reel itself; if a change affects visual output, re-render and
recommit them so the README shows current behavior.

## CI and releases

- `.github/workflows/ci.yml` — build + test on Linux, macOS, and Windows
  (Windows compiles but is untested at runtime; don't treat a green build as
  platform support).
- `.github/workflows/release.yml` — tagged releases publish prebuilt
  binaries with libvpx linked statically. `setup.sh` at the repo root is the
  user-facing installer that fetches them.
