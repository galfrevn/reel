# reel

> Your terminal demo, edited like video.

Record a terminal session once, then treat it as a timeline you can cut,
speed-ramp, zoom, caption, restyle, and score with sound — re-rendering in
milliseconds without ever re-running the underlying program.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/galfrevn/reel/main/setup.sh | bash
```

Grabs a prebuilt binary when one exists for your platform, otherwise builds
from source with cargo.

## Quick start

```sh
# Record with reel's own capture (asciinema .cast files work too)
reel record -o session.cast -- your-tui

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
file   = "demo.webm"
budget = "2mb"          # the encoder degrades predictably to fit

[audio]
keyboard = "mx-brown"   # procedural keystrokes from the recorded input
---

trim    2s..end
cut     19s..23s                  # remove the typo
speed   5x from 8s to 34s         # compress the model's thinking pause
caption "Refactor the auth module" at 4s for 2.5s
zoom    1.8x at (30,10) from 36s to 41s
sound   "success" at 41s
freeze  last 1.5s
```

## What you get

- **Timeline editing** — `trim`, `cut`, `speed`, `zoom`, `caption`,
  `highlight`, `freeze`… applied to a frozen recording, so iterating costs
  nothing (`reel watch` re-renders on save).
- **Templates that look designed** — `glass`, `minimal`, `classic`, `geist`,
  `paper`, `crt`; bring your own as TOML or install packs from GitHub.
  Themes import from base16, Alacritty, and iTerm2.
- **A community registry** — [browse every template as a live
  preview](https://galfrevn.github.io/reel/), `reel template search` finds
  looks published by others, `reel template try owner/repo/name` previews one
  against a bundled demo recording before installing anything. Publishing is
  a [PR with a TOML file](registry/README.md) — no accounts, no
  infrastructure.
- **Size budgets** — `budget = "800kb"` and the encoder walks a predictable
  degradation ladder, reporting every step.
- **Sound without audio files** — keystrokes, UI cues, and agent-thinking
  beds synthesized procedurally into WebM/Opus; `speed 5x` drops key sounds
  instead of chipmunking them.
- **Single binary** — no `ttyd`, no `ffmpeg`.

## Let your agent do it

reel ships an [agent skill](skills/reel/SKILL.md) so coding agents (Claude
Code, Cursor, etc.) can produce the demo for you — record once, then ask for
"a README GIF under 800kb with the boring part sped up":

```sh
npx skills add galfrevn/reel
```

## Documentation

| Doc | What's in it |
|---|---|
| [documentation/idea.md](documentation/idea.md) | What reel is, why it exists, and what it deliberately isn't |
| [documentation/setup.md](documentation/setup.md) | Full user guide: install, recording, the `.reel` format, templates, audio, agent skill |
| [documentation/development.md](documentation/development.md) | Building from source, workspace layout, tests, CI |
| [documentation/roadmap.md](documentation/roadmap.md) | What's next: script mode, `.tape` import, performance, formats |

## License

MIT.
