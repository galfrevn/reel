# The `.reel` file format — full reference

A `.reel` file is TOML front-matter between `---` fences, followed by a
newline-delimited script of timeline operations. Blank lines and `#` comments
are allowed in the script body.

```
---
# TOML front-matter (configuration)
---

# script body (one operation per line)
```

## Front-matter

All sections are optional except that edit mode requires `[source]`.

```toml
[source]
cast = "session.cast"    # path to the asciinema v2 recording, relative to the .reel file

[output]
file   = "demo.gif"      # extension picks the format: .gif or .txt (.png only via `reel shot`)
loop   = true            # GIF looping (default true)
budget = "800kb"         # optional size target, e.g. "500kb", "2mb"; encoder degrades settings to fit
fps    = 30              # frame-rate cap; frames are emitted on grid change, not on a clock
scale  = 2               # supersampling factor for crisp text (1-4)

[template]
name = "glass"           # minimal | glass | classic | geist | paper | crt | any installed template

[style]                  # each key overrides the template's value
theme       = "tokyo-night"   # reel-dark | catppuccin-mocha | tokyo-night | geist-dark | paper-light | phosphor | any imported theme
font        = "Berkeley Mono"     # any installed font family; reel warns and falls back if missing
font_size   = 18
line_height = 1.4
window      = "macos"    # macos | rounded | plain | none
padding     = 48

[output]
aspect = "16:9"          # optional: grow (never crop) the canvas to a ratio; window stays centered

[audio]                  # rendered into .webm output only; ignored for .gif
enabled   = true         # optional; defaults to on when any audio key or sound op is present
keyboard  = "mx-brown"   # mx-brown | mx-blue | topre | laptop | typewriter | none
volume    = 0.35         # master level 0..1
ui_sounds = true         # auto pops when the screen responds after idle
thinking  = "soft-pulse" # idle-gap bed recipe, or "none"
bed       = "none"       # ambient loop recipe, default off
```

Layering order (later wins): built-in defaults → template → `[style]`
overrides → CLI flags (`--template`, `--budget`, `--scale`, `-o`).

`[terminal]`, `[env]`, and `[typing]` are accepted by the parser for forward
compatibility with script mode but do nothing today.

## Time syntax

Anywhere a time or duration appears: `3s`, `1200ms`, `1:24` (mm:ss), `end`,
`end-2s`. Ranges are written `A..B`, e.g. `2s..end`, `19s..23s`.

**All timestamps refer to the source clock** — the recording's own timeline
before any edits. A `caption ... at 40s` stays attached to the same moment of
the recording even if you later `cut 10s..20s`. Durations (`for 2.5s`,
`freeze last 1.5s`, `hold 2s`) are output time.

## Timeline operations

### Editing (change what plays)

| Op | Grammar | Effect |
|---|---|---|
| `trim` | `trim A..B` | Keep only this range, drop everything outside |
| `cut` | `cut A..B` | Remove the range, join the seam |
| `speed` | `speed Nx from A to B` | Time-compress (or expand, N<1) a region; N may be fractional, e.g. `2.5x` |
| `hold` | `hold DUR at T` | Insert a still pause at T |
| `freeze` | `freeze last DUR` | Hold the final frame before the loop restarts |

Constraints enforced at parse/resolve time: `speed` regions must not overlap
each other; `trim` must lie inside the recording. Errors name the line number.

### Visual overlays (change how it looks)

| Op | Grammar | Effect |
|---|---|---|
| `zoom` | `zoom Nx at (col,row) [from A to B]` | Ease into a grid region; without a range it applies to the whole demo |
| `pan` | `pan to (col,row) from A to B` | Move the zoom viewport while zoomed |
| `caption` | `caption "text" at T for DUR [pos=bottom\|top\|center]` | Styled text overlay (default `pos=bottom`) |
| `highlight` | `highlight (col,row,w,h) at T for DUR` | Dim everything except the rect |
| `marker` | `marker "label" at T` | No-op annotation, shown by `reel inspect` |

Zoom coordinates are **grid cells** (column, row), not pixels — they survive
font-size and template changes. Text is re-rasterized at the zoomed size, so
it stays sharp.

### Audio ops (heard in .webm output; silently ignored in .gif)

- `sound "name" at T` — place a one-shot. Names: chime, sparkle, droplet,
  bloom, whisper, tick, press, release, toggle, success, error, page,
  loading, ready, pulse, scan, arrival, soft-pulse.
- `mute A..B` — drop every generated sound anchored in the source range.
- `volume LEVEL from A to B` — scale generated sounds in the range (e.g.
  `volume 0.15 from 8s to 34s` under a sped-up thinking pause).

Keyboard/UI/thinking layers are automatic from the `[audio]` table; ops are
for moments you choose. Anchors are source time like everything else.

## Worked example

Recording: 45 s agent session. 0–2 s prompt setup, 8–34 s the agent thinks
(dead air), a typo at 19–23 s, the result appears around 36 s, ends at 43 s.

```
---
[source]
cast = "agent-session.cast"

[template]
name = "glass"

[output]
file   = "demo.gif"
budget = "1mb"
---

trim    2s..end                      # drop the prompt setup
cut     19s..23s                     # remove the typo
speed   6x from 8s to 34s            # compress the thinking
caption "Refactor the auth module" at 4s for 2.5s
zoom    1.8x at (30,10) from 36s to 41s
freeze  last 1.5s
```

Result: ~12 s output from a 45 s recording, one caption for context, a zoom on
the payoff, and a resting frame before the loop.

## CLI quick reference

```sh
reel render FILE [-o OUT] [--template T] [--budget 800kb] [--scale N] [-q]
reel watch  FILE [--serve [PORT]]      # re-render on save; browser preview at 127.0.0.1:4171
reel shot   FILE --at 12s [-o out.png] # single frame PNG
reel inspect FILE                      # duration, ops summary, markers, size estimate
reel init [TEMPLATE] [-o demo.reel]    # scaffold a .reel file
reel templates                         # list built-in templates
reel themes                            # list built-in themes
```

`reel render session.cast` (a bare cast, no `.reel`) renders with default
styling — good for a first preview.
