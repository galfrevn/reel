# reel — Technical & Product Specification

> Your terminal demo, edited like video.

**Status:** design spec, pre-implementation
**Target language:** Rust
**Document purpose:** hand-off document for an implementing agent. Read fully before writing code.

---

## 1. Product thesis

There is an explosion of TUI applications and terminal-based AI agents (opencode, Claude Code, lazygit, k9s, atuin, etc.). Their authors need demo recordings for READMEs, launch tweets, and docs — and most of them produce bad ones: heavyweight GIFs, unreadable fonts on mobile, 40 seconds of dead air while an LLM thinks, no visual polish.

The incumbent, **VHS** (charmbracelet), is a *session generator*: you script a session in a `.tape` file and it runs it. It is excellent for simple CLIs. It fails for the modern case because:

- It requires `ttyd` and `ffmpeg` on `PATH`. This is a recurring source of install friction and version-mismatch breakage.
- It renders by screen-capturing a headless browser, which means it cannot do pixel-level post-processing (zoom, shaders, compositing).
- Every style change re-executes the recorded command from scratch. For an AI agent demo that means paying for a new LLM call every time you want to try a different theme.
- Its script model assumes line-oriented commands. Agentic TUIs are non-deterministic, keyboard-driven, and long-running — they cannot be meaningfully scripted.

**reel is a session *editor*.** You capture a terminal session once, then treat it as a timeline you can cut, speed-ramp, zoom, annotate, restyle, and score with sound — re-rendering in milliseconds without ever re-running the underlying program.

### One-line pitch

> Record your TUI once. Edit it like a video. Ship a demo that looks designed.

### What we are explicitly NOT building

Cut these from scope and defend the boundary:

- **A TUI testing framework.** The architecture would support it. It is a different product with a different audience. Not now.
- **A general-purpose scripting language.** No custom interpreter, no loops, no conditionals. If a user needs programmability, they use the library API from a language they already know.
- **A screen recorder.** We do not capture desktop windows or arbitrary GUI apps. Terminal only.
- **A hosting/sharing service.** Files out, that's it.

---

## 2. Differentiators (ranked)

These are the reasons a user switches. Everything in the build order serves one of them.

1. **Timeline editing.** `trim`, `cut`, `speed`, `zoom`, `caption`, `freeze`. Nobody in this space does post-production. This is the product.
2. **Templates that look designed.** Not color themes — complete visual packages: font + palette + window chrome + shadow + background + motion + sound. The visual quality of the default output is the marketing.
3. **Instant iteration.** `reel watch` re-renders on file save without re-executing the recorded program, because capture and render are separate stages.
4. **File size.** Declarative `budget = "800kb"` — the encoder targets a size. README GIFs commonly ship at 5–15MB; we should be 5–20x smaller at equal or better visual quality.
5. **Single binary.** No `ttyd`, no `ffmpeg` on the user's machine. Codecs are statically linked at build time — that is our problem, not theirs.
6. **Sound.** Keyboard, UI response cues, agent-thinking beds. Nobody does this. High share-value on launch.

---

## 3. Architecture

### 3.1 Pipeline

```
                    ┌─────────── CAPTURE ───────────┐
  PTY spawn  ──▶  VT emulator  ──▶  grid snapshots  ──▶  session.cast
  (portable-pty)   (alacritty_terminal)                  (asciinema v2 + sidecar)

                    ┌─────────── EDIT ──────────────┐
  session.cast  ──▶  timeline ops  ──▶  virtual frame list + audio event list
                     (trim/cut/speed/zoom/caption/freeze)

                    ┌─────────── RENDER ────────────┐
  frame list  ──▶  rasterizer  ──▶  compositor  ──▶  encoder  ──▶  demo.gif / .webm / .mp4
                   (cosmic-text)   (chrome, bg,      (gif / vpx+opus)
                                    zoom, shader)
```

**The hard architectural rule: capture and render never touch.** The `.cast` file is the boundary. Once captured, the recorded program is never executed again. Everything downstream is a pure function of `(cast, reel file)`. This rule is what makes `watch`, cheap iteration, and deterministic output possible. Do not violate it for convenience.

### 3.2 Crate selection

| Concern | Crate | Notes |
|---|---|---|
| PTY spawn | `portable-pty` | Covers Unix PTY and Windows ConPTY |
| VT emulation | `alacritty_terminal` | Full state machine, grid of cells with colors/attrs. Do not write your own. |
| VT parsing (fallback) | `vte` | If `alacritty_terminal`'s API proves too coupled |
| Text shaping + layout | `cosmic-text` | Handles fallback chains, wide chars, ligatures |
| Glyph rasterization | `swash` (via cosmic-text) | COLRv1 for color emoji |
| Image buffers | `image`, `tiny-skia` | `tiny-skia` for chrome: rounded rects, shadows, gradients |
| GIF encoding | `gif` + custom quantizer | See §7.1 |
| Video | `vpx` (VP9) + `audiopus` (Opus), muxed to WebM | Statically linked |
| Audio decode | `hound` (WAV) or `symphonia` | Samples are embedded WAV |
| CLI | `clap` v4 | |
| Config parse | `toml` + custom script parser | See §5 |
| Watch | `notify` | |
| Fonts embedded | `include_bytes!` | See §6.3 |

### 3.3 Why Rust and not Zig

Rust wins on ecosystem for exactly the two hardest subsystems: the VT state machine (`alacritty_terminal` is production-proven) and text shaping (`cosmic-text`). In Zig both would be written from scratch or bound via FFI. Zig's advantages — cross-compilation, small binaries, native C ABI — do not outweigh three months of avoidable work.

### 3.4 The intermediate format

We use **asciinema v2 cast** as the base format for interoperability (users can bring existing recordings, and `asciinema rec` can be used to generate test material before our own capture layer exists). We extend it with a **sidecar file** `session.reelmeta` (JSON) carrying data the cast format has no place for:

```json
{
  "version": 1,
  "input_events": [
    { "t": 1.243, "kind": "key", "value": "a" },
    { "t": 1.301, "kind": "key", "value": "enter" }
  ],
  "term_env": { "TERM": "xterm-256color", "COLORTERM": "truecolor" },
  "cols": 90,
  "rows": 24
}
```

`input_events` is what makes accurate keystroke audio possible — we know exactly when a key was pressed rather than inferring it from output. If the sidecar is missing (plain asciinema cast), fall back to heuristic inference (§8.2).

---

## 4. Two capture modes

This is the design consequence of the core insight: **agentic TUIs cannot be scripted.**

### 4.1 `script` mode — deterministic, for CLIs and simple TUIs

The `.reel` file drives the session. Suitable for install demos, command walkthroughs, simple navigable TUIs.

Critical requirement: the script DSL must speak **keyboard and screen state**, not lines and stdout.

- `wait_text /pattern/` matches against the **rendered grid**, not the byte stream. In a TUI the stream is escape-code noise; the grid is the truth.
- `wait_idle 500ms` — "wait until the screen stops changing for N ms". This will cover ~80% of real waits. Make it the ergonomic default.
- Never encourage bare `sleep` in docs or templates. Fixed sleeps are why VHS tapes break on slow CI runners.

### 4.2 `live` mode — for agents and anything non-deterministic

```
reel record --out session.cast -- opencode
```

The user drives the real application by hand. The session is captured with real timing. The `.reel` file then references the cast and becomes a **pure edit timeline** — no `type`, no `key`, only timeline operations.

This solves: LLM cost (one execution), non-determinism (frozen artifact), and long dead-air (speed ramps).

### 4.3 Hybrid mode

Expected to be the most-used. Scripted setup, manual middle:

```
type "opencode"
enter
wait_idle

capture_live            # script pauses; user takes control; ctrl+d to resume

wait_idle
key ctrl+c
```

Boring deterministic setup (cd, clear, env exports, app launch) is identical every run. The interesting part is performed once by hand.

---

## 5. The `.reel` file format

### 5.1 Structure

TOML front-matter (declarative config) + newline-delimited script (ordered events). Rationale: events are a *sequence*; expressing sequences in pure TOML requires `[[steps]]` blocks per keystroke, which is unreadable. Front-matter is a familiar pattern (Astro, Hugo, Jekyll).

```
---
# TOML front matter — configuration
---

# script body — ordered operations
```

If the front-matter contains `[source] cast = "..."`, the file is in **edit mode** and the body may only contain timeline operations. Otherwise it is in **script mode** and the body may contain both input and timeline operations. Enforce this at parse time with a clear error.

### 5.2 Full front-matter reference

```toml
[source]
cast = "session.cast"          # optional; presence switches to edit mode

[output]
file      = "demo.gif"         # extension determines format
loop      = true               # gif/webm looping
budget    = "800kb"            # optional target size; encoder adapts (§7.1)
fps       = 30                 # cap; frames are emitted on change, not on clock
scale     = 2                  # supersampling factor for crisp text

[template]
name = "glass"                 # loads templates/glass.toml as the base layer

[terminal]
cols  = 90
rows  = 24
shell = "zsh"                  # script mode only
cwd   = "./demo-project"       # script mode only

[env]                          # script mode only
DEMO_MODE = "1"

[style]                        # overrides template values
theme      = "catppuccin-mocha"
font       = "Geist Mono"
font_size  = 18
line_height = 1.4
window     = "macos"           # macos | rounded | plain | none
padding    = 48

[typing]                       # script mode only
speed  = "55ms"
jitter = 0.25                  # 0.0 = robotic, 1.0 = chaotic. Default 0.25.

[audio]
enabled   = true
keyboard  = "mx-brown"
volume    = 0.35
ui_sounds = true
thinking  = "soft-pulse"
bed       = "none"
```

**Layering order** (later wins): built-in defaults → template → `[style]`/`[audio]` overrides → CLI flags.

### 5.3 Script operations — input (script mode only)

| Op | Args | Behavior |
|---|---|---|
| `run` | `"cmd"` | Spawn command directly (no shell) |
| `type` | `"text"` | Type with configured speed + jitter |
| `paste` | `"text"` | Insert instantly, no per-char timing |
| `key` | `<keyspec>...` | Send key(s). Multiple allowed: `key down down tab` |
| `enter` | — | Sugar for `key enter` |
| `mouse click` | `(col,row)` | Send SGR mouse press+release |
| `mouse scroll` | `up\|down N` | |
| `resize` | `cols rows` | Send SIGWINCH |
| `capture_live` | — | Hand control to the user until `ctrl+d` |

**Keyspec grammar:** `ctrl+c`, `alt+enter`, `shift+tab`, `up`/`down`/`left`/`right`, `f1`–`f12`, `esc`, `tab`, `backspace`, `space`, `home`, `end`, `pgup`, `pgdn`, `delete`. Repetition: `key down*5`.

### 5.4 Script operations — waiting (script mode only)

| Op | Args | Behavior |
|---|---|---|
| `wait_idle` | `[duration]` | Screen unchanged for N ms (default 300ms) |
| `wait_text` | `/regex/` or `"literal"` | Match against rendered grid text |
| `wait_gone` | `/regex/` | Match disappears from grid |
| `sleep` | `duration` | Hard wait. Discouraged; document as such. |

All waits take an optional `timeout=Ns` (default 30s) and fail loudly on expiry with the last grid state dumped to stderr for debugging.

### 5.5 Script operations — timeline (both modes)

Timeline ops in edit mode use absolute timestamps. In script mode they may also be placed inline, where they anchor to the current position.

| Op | Args | Behavior |
|---|---|---|
| `trim` | `A..B` | Keep only this range (drop everything outside) |
| `cut` | `A..B` | Remove this range, join the seam |
| `speed` | `Nx from A to B` | Time-compress a region. `N` may be fractional. |
| `hold` | `duration at T` | Insert a still pause |
| `freeze` | `last <duration>` | Hold the final frame before loop |
| `zoom` | `Nx at (col,row) [from A to B]` | Scale into a grid region with eased in/out |
| `pan` | `to (col,row) from A to B` | Move the zoom viewport |
| `caption` | `"text" at T for D [pos=bottom]` | Overlay styled text |
| `highlight` | `(col,row,w,h) at T for D` | Dim everything except a rect |
| `marker` | `"label" at T` | No-op annotation; used by `reel inspect` |

**Time syntax:** `3s`, `1200ms`, `1:24` (mm:ss), `end`, `end-2s`.

### 5.6 Script operations — audio

| Op | Args | Behavior |
|---|---|---|
| `sound` | `"name" at T` | Fire a one-shot sample |
| `mute` | `A..B` | Silence a region |
| `volume` | `N from A to B` | Scale a region's mix level |

### 5.7 Example — edit mode (agent demo)

```
---
[source]
cast = "opencode-session.cast"

[template]
name = "glass"

[output]
file   = "demo.webm"
budget = "2mb"

[audio]
keyboard = "mx-brown"
thinking = "soft-pulse"
---

trim    2s..end
cut     19s..23s                    # remove the typo
speed   5x from 8s to 34s           # LLM thinking → compressed
volume  0.15 from 8s to 34s
caption "Refactor the auth module" at 4s for 2.5s
zoom    1.8x at (30,10) from 36s to 41s
sound   "success" at 41s
freeze  last 1.5s
```

### 5.8 Example — script mode (install demo)

```
---
[template]
name = "minimal"

[terminal]
cols = 80
rows = 20

[output]
file   = "install.gif"
loop   = true
budget = "500kb"
---

type "npm i -g mytui"
enter
wait_text /added \d+ packages/
sleep 400ms

type "mytui"
enter
wait_idle 500ms

key down*3
key enter
wait_text "Ready"

freeze last 1s
```

---

## 6. Templates

### 6.1 Concept

A template is **the complete visual and sonic package**, not a color scheme. This is the "batteries included" surface that solves the blank-page problem: a user should get a good-looking demo without making a single aesthetic decision.

### 6.2 Template file format

```toml
# templates/glass.toml
name        = "glass"
description = "Soft gradient, rounded chrome, generous air"

[font]
family = "Geist Mono"
size   = 17
weight = 500
line_height = 1.45

[colors]
palette = "tinted:tokyo-night"    # or inline 16+ color definitions
bg      = "#0d0d0f"
fg      = "#e8e8ed"
cursor  = "#89b4fa"

[chrome]
window        = "rounded"          # rounded | macos | plain | none
titlebar      = "traffic-lights"   # traffic-lights | title | none
title         = ""
corner_radius = 14
padding       = 48
shadow        = { blur = 60, opacity = 0.45, y = 20, color = "#000000" }
border        = { width = 1, color = "#ffffff12" }

[canvas]
background = "linear-gradient(135deg, #1a1a2e, #16213e)"
inset      = 32

[motion]
cursor_blink = "smooth"            # smooth | hard | none
type_easing  = "human"
zoom_easing  = "ease-in-out-cubic"

[audio]
keyboard  = "mx-brown"
volume    = 0.35
ui_sounds = true
```

### 6.3 Ship-with set

Use neutral names. Do not name templates after other companies' brands — it creates trademark exposure and looks derivative.

| Template | Character |
|---|---|
| `minimal` | High contrast, square corners, no gradient, no chrome noise |
| `glass` | Gradient canvas, rounded chrome, soft shadow, generous padding |
| `classic` | Bare terminal, no chrome — for purists and docs embeds |
| `paper` | Light background, for daytime documentation |
| `crt` | Scanlines, bloom, chromatic aberration, slight barrel distortion |

**`crt` is strategically important.** It is only possible because we rasterize frames ourselves and can run a fragment shader over the composited buffer before encoding. VHS structurally cannot do this. It is also the single most shareable artifact for launch — an effect people screenshot and post. Implement it as a post-composite GPU shader (`wgpu`) with a CPU fallback.

### 6.4 Community templates

```
reel template list
reel template add gh:user/template-name
reel template show glass
```

Templates are data files, not code. Fetch from GitHub, cache in `~/.config/reel/templates/`. This lets the catalog grow without maintainer effort — a deliberate distribution mechanism.

### 6.5 Color themes

Do **not** invent a theme format. Import existing ones:

- **tinted-theming / base16** (YAML) — hundreds available, referenced as `tinted:name`
- **iTerm2** `.itermcolors` (plist XML)
- **Alacritty** TOML
- **Windows Terminal** JSON fragments

A one-afternoon parser gets "400+ themes included" in the README on day one.

### 6.6 Fonts

**Embed 4 fonts in the binary** so output is byte-identical across machines and CI:

- JetBrains Mono NL (Nerd Font patched variant)
- Geist Mono
- IBM Plex Mono
- Fira Code

All are OFL/SIL licensed — redistribution is fine. Verify and vendor the license files.

**Nerd Font support is not optional.** Nearly every modern TUI renders icon glyphs from the Private Use Area. If reel renders tofu boxes where the user sees icons, the output is unusable and the project is dead on arrival for its core audience. At minimum one patched font must be embedded and used as the fallback for PUA codepoints.

Additional fonts resolve from the system, then download on demand to `~/.cache/reel/fonts/`.

---

## 7. Rendering & encoding

### 7.1 GIF

GIF remains mandatory — it is the format of GitHub READMEs.

**Size strategy** (this is a headline feature, treat it as such):

1. **Emit frames on grid change, not on a clock.** Terminal output is sparse. A 40-second session may have 200 meaningful frames, not 1200.
2. **Exact palette when possible.** Terminal themes typically use under 256 distinct colors. When the frame set fits in the palette, skip quantization entirely — output is lossless and small. Only fall back to NeuQuant/median-cut when gradients or images push past 256.
3. **Delta rectangles.** Emit only the changed bounding box per frame with GIF's local image descriptor.
4. **Budget loop.** When `budget` is set, binary-search across (fps cap, palette size, dithering on/off, scale) until the output fits. Report the final settings to the user so it isn't a black box.

Gradient canvas backgrounds fight palette efficiency. Mitigation: for GIF output, quantize the gradient to a small dithered ramp, or auto-flatten to a solid color and warn.

### 7.2 Video (WebM)

Required for the agent-demo case: a 40-second GIF is 15–30MB and looks bad. It is also the prerequisite for audio.

**VP9 + Opus in WebM**, statically linked at build time. The user still downloads one binary — the "no dependencies" promise holds on their side, which is the side that matters. Binary grows from ~8MB to ~20MB; nobody cares.

MP4/H.264 is deferred: licensing complexity for marginal gain. Provide `reel render --frames-out ./frames/` so anyone who needs MP4 can pipe to their own ffmpeg.

### 7.3 Other outputs

- **APNG / animated WebP** — better than GIF where supported, cheap to add
- **PNG** — `reel shot demo.reel --at 12s`
- **SVG animation** — crisp, tiny, selectable text. Caveat: GitHub sanitizes SVG and animation via `<img>` is inconsistent — verify before promising it for READMEs.
- **`.txt`** — plain grid dump, useful for debugging

### 7.4 Zoom

Zoom targets **grid coordinates**, not pixels, so it survives font-size and template changes. Render the region at native resolution (re-rasterize glyphs at the zoomed size rather than upscaling pixels) so text stays sharp. This is a quality difference users will notice immediately versus a naive image scale.

---

## 8. Audio

### 8.1 Core design rule

**Audio is an event list with timestamps, mixed after the timeline is resolved — never a pre-rendered waveform.**

Reason: `speed 5x from 8s to 34s` applied to a waveform produces chipmunk artifacts and turns keystroke sound into white noise. With an event list, time-compressing a region simply **drops events** while preserving pitch. Keys still sound like keys, there are just fewer of them.

Pipeline: resolve timeline → produce final event list with adjusted timestamps → mix into an f32 buffer → encode Opus → mux.

Mixing is buffer addition. Do not pull in an audio engine.

### 8.2 Layers

**1. Keyboard.** The detail that separates credible from toy:

- 8–12 samples per profile, round-robin with no immediate repeat
- Random pitch ±3%, gain ±15%
- Distinct samples for `space`, `enter`, `backspace` — these are the most noticeable
- Profiles: `mx-brown`, `mx-blue`, `topre`, `laptop`, `typewriter`, `none`

Event source: `input_events` from the sidecar. When absent (imported plain cast), infer from output: printable characters appearing one at a time in sequence at the cursor position are typing; large block updates are not.

**2. UI response cues.** Auto-derived from the grid diff we already compute: screen was static, then a significant region changed → place a subtle pop. Requires zero user configuration. This is "batteries included" applied to sound.

**3. Agent thinking bed.** A low, quiet loop during long idle periods, resolving to a chime when output resumes. This is exactly the region being speed-ramped, and it gives narrative shape to what is otherwise dead air. Highest-value layer for the primary use case.

**4. Ambient bed.** Optional soft pad. Default off.

### 8.3 Constraint to respect

**Twitter, LinkedIn, and GitHub autoplay muted.** The demo must be fully comprehensible in silence. Audio is a polish layer, never an information layer — never encode meaning in a sound that isn't also visible.

### 8.4 Samples & licensing

Samples ship inside the binary. Use **CC0 only**, or record them in-house.

**Recommendation: record them in-house.** An afternoon with a real keyboard and a decent mic gives completely clean rights and a launch story ("every sample recorded on real hardware") that generates its own attention.

Store as 48kHz mono WAV, embedded via `include_bytes!`. Expect ~2–4MB total.

---

## 9. Terminal emulation: the hard part

**Budget one full week for this alone.** It is the difference between "works with `echo`" and "works with opencode", and it is where the project fails if underestimated.

### 9.1 Queries that must be answered

Real TUIs interrogate the terminal on startup. If we don't respond, the app **hangs waiting** or silently degrades to an ugly fallback mode:

| Query | Response |
|---|---|
| DA1 `ESC [ c` | Advertise a credible xterm-256color feature set |
| DA2 `ESC [ > c` | Secondary device attributes |
| DSR cursor `ESC [ 6 n` | Current cursor position |
| OSC 10/11 | Foreground/background color — answer from the active theme |
| OSC 4 | Palette color query |
| Kitty keyboard protocol `ESC [ ? u` | Report support level (declining is fine, but respond) |
| XTVERSION `ESC [ > 0 q` | Identify as reel |

Set `TERM=xterm-256color` and `COLORTERM=truecolor` in the child environment.

### 9.2 Other emulation requirements

- **Alternate screen buffer** (`?1049h/l`) — every full-screen TUI uses it
- **Mouse reporting** modes `?1000`, `?1002`, `?1003`, `?1006` (SGR)
- **Bracketed paste** `?2004`
- **Synchronized output** `?2026` — increasingly used; respect it to avoid capturing torn frames
- **Wide characters** — CJK and many emoji occupy two cells; `unicode-width` plus correct grid handling
- **Sixel / Kitty graphics protocol** — deferred, but a real differentiator later since more TUIs display images

### 9.3 Recommended validation target

Test against a fixed list from day one: `opencode`, `lazygit`, `k9s`, `btop`, `yazi`, `helix`. If those six render correctly, coverage is broadly adequate.

---

## 10. CLI surface

```
reel init [template]              # scaffold a .reel file
reel record --out FILE -- CMD     # live capture
reel run FILE                     # capture (script mode) + render
reel render FILE                  # render only (edit mode)
reel watch FILE                   # live-reload preview
reel shot FILE --at 12s           # single frame PNG
reel inspect FILE                 # timeline summary, markers, size estimate
reel template list|add|show
reel theme list|add
```

Global flags: `--template`, `--out`, `--budget`, `--scale`, `--no-audio`, `--quiet`.

### `reel watch` — the feature that sells the tool

File watcher on the `.reel`, re-render on save, display in a native window (`winit` + `softbuffer`, or `wgpu` if the shader path is already there). Because the cast is cached, changing theme, font, padding, template, or zoom is a sub-second re-render with no re-execution.

This turns producing a demo from a chore into something a user fiddles with until it's right. It is the single largest experiential gap versus VHS, where every color tweak re-runs the whole tape.

---

## 11. Build order

Do not build the PTY layer first. Validate the thesis with the cheapest possible artifact.

### Phase 0 — Renderer only (~2 weeks) — ✅ shipped

Input: an existing `.cast` (generate with `asciinema rec -- opencode`; no capture code needed yet). Output: a GIF.

- Cast parser → `alacritty_terminal` → grid snapshots
- Rasterizer with one embedded font
- Two templates: `minimal` and `glass` (chrome, shadow, gradient)
- GIF encoder with frame dedup and exact palette

**Validation gate.** Publish a side-by-side comparison: same cast rendered by `agg` versus reel, with file sizes. If this generates no reaction, the thesis is wrong and two weeks were spent, not three months.

### Phase 1 — Timeline (~2 weeks) — ✅ shipped

- Full `.reel` parser (front-matter + script)
- `trim`, `cut`, `speed`, `hold`, `freeze`, `caption`, `zoom`, `pan`, `highlight`
- `reel watch` with live preview

This is the actual product. Everything before it was infrastructure.

### Phase 1.5 — Audio (~1.5 weeks) — ✅ shipped (procedural synthesis instead of samples: recipes ported from cuelume, no audio files at all)

- WebM/VP9/Opus encoder path
- Event-list audio model, mixer, keyboard profiles
- Auto UI cues from grid diff, thinking beds

Audio comes after the timeline because it needs the timeline to synchronize against.

### Phase 2 — Own capture (~3 weeks) — ✅ shipped (interactive passthrough: the real terminal answers queries; §9's query responder becomes a script-mode concern)

- `portable-pty` spawn, `reel record`
- Terminal query responses (§9) — the week-long slog
- Nerd Font embedding and PUA fallback
- Sidecar `input_events` for accurate keystroke audio

### Phase 3 — Breadth — partially shipped

Shipped: theme importers, community templates (client-side via GitHub, no
hosted registry), the `crt` template, the GitHub Action, Windows builds in
CI (untested). Remaining:

- Script mode (`type`, `key`, `wait_*`, `capture_live`)
- VHS `.tape` import for zero-switching-cost adoption
- Theme importers (base16, iTerm2, Alacritty)
- Community template registry
- Windows/ConPTY
- GitHub Action for regenerating README demos in CI

---

## 12. Risks

| Risk | Severity | Mitigation |
|---|---|---|
| Terminal emulation underestimated; opencode hangs or renders wrong | **Critical** | Dedicated week, six-app validation list, do it in Phase 2 with a hard gate |
| Nerd Font glyphs render as tofu | **Critical** | Embed a patched font; test against icon-heavy TUIs |
| Scope creep (testing, scripting language, MCP, 300 themes) | **High** | This conversation already generated all four. The project dies from accumulation, not competition. Defend §1's exclusion list. |
| Charm ships timeline editing | Medium | Their headless-browser architecture blocks pixel-level work. Move fast on `crt`/shaders where the gap is structural. |
| Gradient backgrounds break GIF palette efficiency | Medium | Auto-flatten with a warning for GIF targets |
| Color emoji rendering (COLRv1/CBDT) | Medium | `swash` handles most; accept degradation on exotic cases |
| Name collision on crates.io / npm / GitHub | Low | **Verify `reel` availability before publishing.** Common word — likely contested. Fallbacks: `reelcli`, `getreel`, or scope the npm package. |

---

## 13. Positioning & launch

**Do not lead with "written in Rust, no dependencies."** Nobody switches tools for that. It is a footnote, not a pitch.

Lead with the output. The demo GIF *is* the pitch:

> **reel** — your terminal demo, edited like video.

Launch assets, in priority order:

1. A README whose hero demo is generated by reel itself, with the `.reel` source shown beside it
2. A side-by-side size/quality comparison against the incumbents
3. A `crt`-template demo — the shareable one
4. An agent-demo before/after: 40 seconds of raw opencode versus 12 seconds edited
5. A GitHub Action so demos regenerate on release (this is a large part of how VHS spread)

---

## 14. Immediate next action

Before writing any code:

```
asciinema rec demo.cast -- opencode
```

Record a real session of the TUI most worth showing. Watch it back and write down: where does time drag, what needs magnification, what looks wrong, where would sound help.

That list is the Phase 1 specification, written by the actual problem rather than by speculation — and it will also confirm whether the pain is as large as this document assumes.
