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
file   = "demo.gif"      # extension picks the format: .gif, .webm, .apng, .txt (.png via `reel shot`)
loop   = true            # GIF looping (default true)
budget = "800kb"         # optional size target, e.g. "500kb", "2mb"; encoder degrades settings to fit
fps    = 30              # frame-rate cap; frames are emitted on grid change, not on a clock
scale  = 2               # supersampling factor for crisp text (1-4)

[template]
name = "glass"           # any name from `reel templates` (built-in or installed), or a .toml path

[style]                  # each key overrides the template's value
theme       = "tokyo-night"   # any name from `reel themes` (built-in or imported)
                              # (template .toml files may instead embed a full
                              # palette as an inline [theme] table — that's how
                              # published templates stay self-contained)
font        = "Berkeley Mono"     # any installed font family; reel warns and falls back if missing
font_size   = 18
line_height = 1.4
window      = "macos"    # macos | rounded | plain | none
padding     = 48
cursor_blink = false     # override the template's cursor blink

[output]
aspect = "16:9"          # optional: grow (never crop) the canvas to a ratio; window stays centered
size = "1920x1080"       # optional: exact canvas pixels; reel solves the font size to fit
subtitles = true         # optional: captions also become a .vtt sidecar + WebM text track

[terminal]               # script mode only: PTY geometry for `reel run`
cols = 200
rows = 50

[env]                    # script mode only: extra variables for the child
DEMO_MODE = "1"

[typing]                 # script mode only: how `type` paces keystrokes
delay_ms = 70            # mean delay between keys (default 70)
jitter   = 0.35          # human variance around the mean, 0..1 (default 0.35)

[audio]                  # rendered into .webm output only; ignored for .gif
enabled   = true         # optional; defaults to on when any audio key or sound op is present
keyboard  = "mx-brown"   # mx-brown | mx-blue | topre | laptop | typewriter | none
volume    = 0.35         # master level 0..1
ui_sounds = true         # auto pops when the screen responds after idle
thinking  = "soft-pulse" # idle-gap bed recipe, or "none"
bed       = "none"       # ambient loop recipe, default off
```

Layering order (later wins): built-in defaults → template → `[style]`
overrides → CLI flags (`--template`, `--budget`, `--scale`, `--aspect`,
`--size`, `--no-audio`, `-o`).

## Template TOML (schema 2)

A template file is the complete visual identity. Every field is optional
(unset fields inherit `minimal`'s neutral defaults; decorations are opt-in),
and `reel template show <name>` prints any template in this format:

```toml
schema      = 2
name        = "frost"
description = "Frosted glass over a wallpaper"
theme       = "tokyo-night"   # name, or an inline [theme] table (fg/bg/cursor/ansi)
font        = "Geist Mono"
font_size   = 17.0
line_height = 1.45
window      = "macos"          # macos | rounded | plain | none
titlebar    = "traffic-lights" # none | traffic-lights | dots
title       = "~/app — zsh"    # text centered in the titlebar
corner_radius = 14.0
padding     = 28.0
inset       = 48.0
border      = "#ffffff12"
window_opacity = 0.85          # < 1 = glassmorphism
window_blur    = 14.0          # backdrop blur behind the window (px)

[canvas]                       # exactly one of solid | gradient | image
grain = 0.05                   # film grain 0..1, composes with any kind
[canvas.gradient]
kind  = "linear"               # linear | radial
angle = 135.0
from  = "#1a1a2e"              # two-stop shorthand…
to    = "#16213e"
# stops = [["#1a1a2e", 0.0], ["#302b63", 0.6], ["#16213e", 1.0]]  # …or multi-stop
# [canvas.image]               # wallpaper instead (PNG/JPEG, relative to this TOML)
# path = "bg.jpg"
# fit  = "cover"               # cover | contain | tile
# dim  = 0.35                  # darken for text contrast
# blur = 8.0

[shadow]
blur = 42.0
opacity = 0.45
offset_y = 14.0

[crt]                          # scanline/phosphor post-fx
scanline = 0.22
glow     = 0.55
vignette = 0.28

[cursor]
style = "beam"                 # block | beam | underline (forces the shape)
color = "#ff9e64"

[badge]                        # corner watermark: text and/or image = "logo.png"
text     = "made with reel"
position = "bottom-right"      # top-left | top-right | bottom-left | bottom-right
opacity  = 0.55

[prompt]                       # injected by `reel run` into the shell it spawns
symbol = "▲"
color  = "#ffffff"
path   = "short"               # none | short | full

[motion]                       # off by default (they add frames)
cursor_slide = true            # cursor slides between cells, Neovide-style
slide_ms     = 90.0
typing_glow  = 0.5             # freshly typed cells glow and decay, 0..1
```

`[prompt]` only affects `reel run` — reel launches that shell itself, so
bare `bash`/`zsh` start without rc files and the branded prompt survives.
`reel record` keeps your real prompt; recordings are never rewritten.
Image canvases and badge logos work locally (`template add` copies the
files next to the installed TOML) but are not publishable to the registry,
which stores single TOML files.

## Time syntax

Anywhere a time or duration appears: `3s`, `1200ms`, `1:24` (mm:ss), `end`,
`end-2s`, `@marker`. Ranges are written `A..B`, e.g. `2s..end`, `19s..23s`,
`@1..@done`.

`@name` references a **marker**: either dropped live during `reel record` by
pressing `Ctrl+]` (auto-labeled `@1`, `@2`, … in order), or defined in the
file with `marker "name" at T`. `reel inspect` lists every marker with its
time. An unknown `@name` fails with the list of known markers.

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
| `marker` | `marker "label" at T` | Name a moment; reference it as `@label` in any time |
| `keys` | `keys on` or `keys A..B` | Show recorded keystrokes as chips (screenkey-style) |

`keys` reads the `.reelmeta` sidecar (or the cast's "i" events): typed runs
group into one chip (`cargo test`), special keys get symbols (`⏎` `⇥` `⌫`
`↑` `^C` …), and each chip lingers ~1.2 s. Chips whose footage is cut or
trimmed away disappear with it. Terminal query responses are filtered out.

Zoom coordinates are **grid cells** (column, row), not pixels — they survive
font-size and template changes. Text is re-rasterized at the zoomed size, so
it stays sharp.

### Redaction

`redact "REGEX"` masks every match across every frame (all formats,
including .txt dumps). Renders warn about emails/tokens/opaque ids they
spot; add redact ops until the warnings stop.

### Script ops (files without [source]; run with `reel run`)

`run "cmd"` (first), `type "text"`, `key enter|esc|tab|up|ctrl+c|…`,
`enter`, `sleep 2s`, `wait_text "needle" [timeout 30s]`,
`wait_idle 2s [timeout 60s]`. Timeout defaults: 30s for `wait_text`, 60s
for `wait_idle`; on expiry the run fails loudly with the last grid state.
Edit ops in the same file apply to the capture afterward.
`reel run FILE --no-render` captures only and prints the cast path.

### Audio ops (heard in .webm output; silently ignored in .gif)

- `sound "name" at T` — place a one-shot (success, error, chime, sparkle,
  droplet, tick, …). An unknown name fails with the complete list of
  available recipes — built-ins plus anything installed with
  `reel audio add` — so there is no need to memorize it.
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
reel record  [-o session.cast] [--size 220x54] -- CMD   # live capture + .reelmeta sidecar
                                       # Ctrl+] while recording drops a marker (@1, @2, …)
reel render  FILE [-o OUT] [--template T] [--budget 800kb] [--scale N]
             [--aspect 16:9] [--size 1920x1080] [--no-audio] [-q]
reel run     FILE [--no-render] [-q]   # script mode: capture + render in one step
reel watch   FILE [--serve [PORT]]     # re-render on save; browser preview at 127.0.0.1:4171
reel shot    FILE --at 12s [-o out.png]  # single frame PNG (--at @marker works too)
reel inspect FILE                      # duration, ops summary, markers, size estimate
reel suggest CAST [--write demo.reel]  # draft the edit script from a recording
reel init [TEMPLATE] [-o demo.reel]    # scaffold a .reel file

reel templates                         # list built-in + installed templates
reel template search [QUERY]           # search the community registry
reel template try SOURCE               # preview (name, .toml, or owner/repo/name) without installing
reel template add SOURCE               # install a .toml or a GitHub pack (owner/repo[/name])
reel template show NAME                # print a template's TOML (starting point for custom looks)
reel template publish FILE [--tag T]…  # validate + preview + open the registry PR (via gh)

reel themes                            # list built-in + imported themes
reel theme import FILE [--name N]      # base16 YAML, Alacritty, iTerm2 .itermcolors
reel theme import --from iterm|kitty|ghostty   # straight from the user's terminal config

reel audio list                        # built-in + installed sounds (alias: reel sounds)
reel audio show NAME                   # print a recipe's TOML (starting point for custom sounds)
reel audio try SOURCE [-o out.wav]     # synthesize a name or .toml to a WAV and audition it
reel audio add SOURCE                  # install a .toml or a GitHub pack (owner/repo[/name])
reel audio search [QUERY]              # search the community sound registry
reel audio publish FILE [--tag T]…     # validate + audition + open the registry PR (via gh)
```

`reel render session.cast` (a bare cast, no `.reel`) renders with default
styling — good for a first preview. The community template gallery lives at
<https://galfrevn.github.io/reel/>.
