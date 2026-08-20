---
name: reel
description: Turn terminal recordings into polished, shareable demos using the reel CLI — record a session once (reel record or asciinema), then edit it like video (trim dead air, cut mistakes, speed-ramp slow parts, zoom, caption) and render a styled, size-budgeted GIF, or a WebM with procedurally synthesized sound, without ever re-running the program. Also covers reel's community registry — searching, previewing, installing, and publishing templates (terminal looks) and sound recipes. Use this skill whenever the user wants a demo of their CLI or TUI, a GIF for a README, launch tweet, or docs, wants to shorten/clean up/restyle a terminal recording, mentions asciinema or .cast files, asks to "record my terminal" or "make a demo" of a command-line tool, wants keystrokes shown on screen in a recording (screenkey-style), wants to mark/flag moments while recording to edit by name later, or wants to find, install, or share a demo template or sound — even if they never mention reel by name.
---

# Making terminal demos with reel

reel turns a terminal recording (`.cast`) into a styled, edited GIF or WebM
(VP9 + Opus audio). The core
promise: **the recorded program runs exactly once**. Every edit — trimming
dead air, speeding up slow parts, changing theme or template — is a pure
re-render of the frozen recording, so iterating takes milliseconds and costs
nothing. Never ask the user to re-record just to change styling or timing.

The user should not need to learn reel's file format or CLI. You drive the
whole workflow: inspect the recording, decide the edits, write the `.reel`
file, render, check the result, iterate. Bring the user in only for the two
things you can't do: performing the live session, and judging taste.

A second principle: **discover, don't memorize**. Templates, themes, and
sounds are open sets — users install their own and the community registry
grows daily. Ask the CLI what exists (`reel templates`, `reel themes`,
`reel template search`) instead of assuming this file's examples are the
full catalog.

## Setup

Check for the tools; install whichever is missing:

```sh
reel --version || curl -fsSL https://raw.githubusercontent.com/galfrevn/reel/main/setup.sh | bash
```

reel records, edits, and renders on its own; asciinema `.cast` files also
work as input if the user already has one.

## Workflow

### 1. Get a recording

If the user has a `.cast` file, use it. Otherwise they must record one —
this is the one step that needs their hands, since demos usually show an
interactive session. Give them the exact command and what to do in it:

```sh
reel record -o session.cast -- <the command to demo>
```

Recording in the user's own terminal inherits its size, so the demo's
geometry matches what they see daily; `--size 220x54` pins the PTY when
recording headlessly or targeting specific dimensions (wide sizes reveal
TUI sidebars). Batched echoes are repaired automatically: reel rebuilds
letter-by-letter typing from the recorded keystroke times, so typing
always renders one character per key, synced with keyboard audio.

This also writes a `session.cast.reelmeta` sidecar with timestamped
keystrokes — keep the two files together; the sidecar is what makes
keyboard audio accurate. (`asciinema rec` works too, without the sidecar.)

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

If `inspect` lists **markers**, the user dropped them on purpose while
recording — anchor your ops on them (`trim @1..@2`, `caption "…" at @done`)
instead of hunting timestamps. See "Opt-in extras" below.

`reel suggest session.cast --write demo.reel` drafts the edit script for you
(trims, speed ramps over dead air) — a good starting point to tune rather
than a finished edit.

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
sound   "success" at 41s
freeze  last 1.5s
```

Timestamps use the **source clock** (the recording's own time, before edits),
so ops never need recomputing when you add or reorder others.

For the complete reference — every operation, argument grammar, time syntax,
style overrides, output options — read
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

## Opt-in extras — only when the user asks

None of these belong in a demo by default. A good demo is the program on a
clean stage; these are instruments you pick up on request, not seasoning to
sprinkle. Add one only when the user asks for it (in any words), or — for
markers — when the recording shows the user already chose it.

### Markers: name moments instead of hunting timestamps

If the user wants precise control over where edits land ("I'll mark the
good parts", "how do I flag the moment it finishes?"), tell them **before
they record**: pressing `Ctrl+]` during `reel record` drops a marker at
that instant — it never reaches the program, a bell confirms it, and the
summary counts them.

Recorded markers auto-label `@1`, `@2`, … in order; `marker "name" at T`
defines named ones in the `.reel` file after the fact. Every time
expression accepts them: `trim @1..@2`, `speed 5x from @1 to @2`,
`caption "…" at @done for 2s`, `reel shot --at @done`. `reel inspect`
prints the table. An unknown `@name` fails listing what exists.

Markers already present in a cast are the one self-authorizing case: the
user pressed the key on purpose, so build your edit around them.

### Keystroke overlay: show what was typed

`keys on` (or `keys A..B` / `keys @1..@2`) overlays the recorded input as
screenkey-style chips at the bottom of the frame: typed runs group into
words (`cargo test`), special keys render as symbols (`⏎ ⇥ ⌫ ↑ ^C`), each
chip lingers ~1.2 s, and chips whose footage you `cut` disappear with it.

Reach for it only when the user asks to see the input — "show my
keystrokes", a keybindings cheat-sheet demo, TUI navigation where the
commands are the content. Never add it to an ordinary CLI demo: the typing
is already visible in the terminal.

`redact` patterns mask chip labels too, so a typed secret can't resurface
in the overlay — but the safe order is still redact first, then enable the
overlay and re-check.

## Choosing a look

Templates are an open set: six built-ins ship in the binary, users install
more locally, and the community publishes theirs to a registry. Start by
listing what's actually available:

```sh
reel templates                  # built-ins + locally installed, with descriptions
reel template search            # everything in the community registry
reel template search dark       # filter by name, description, tag, or repo
```

Pick by destination, using the printed descriptions: a decorated dark look
(gradient, chrome, shadow — the `glass` default) for READMEs and launches; a
plain or chrome-less look for docs embeds where decoration would clash; a
light look for light-mode documentation; a flashy look (scanlines, glow)
when the user wants the shareable eye-catcher. Only ask the user if they've
expressed taste.

Each `search` hit prints its exact `reel template add owner/repo/name`
install line. Before installing, preview any candidate against a bundled
demo recording — it renders a preview file and prints the path:

```sh
reel template try owner/repo/name     # also takes a local .toml or installed name
```

The gallery at <https://galfrevn.github.io/reel/> shows every registry
template as a live preview — point the user there to browse visually.

For a custom look: `reel template show glass > mine.toml`, edit, then
`reel template add mine.toml` (or pass the .toml path directly as
`--template`). `reel template add owner/repo` installs a whole pack from any
GitHub repo with a `templates/` directory.

One physics constraint transcends template choice: gradient backgrounds cost
GIF palette efficiency. If a tight size budget starts visibly degrading
quality, switch to a solid-canvas template and tell the user why.

### Publishing the user's look

When the user has a template worth sharing (or asks how to contribute):

```sh
reel template publish mine.toml --tag dark --tag docs
```

This validates the TOML, renders the same preview the gallery will show,
and — with `gh` installed and authenticated — forks the registry, updates
the index, and opens the PR automatically. `--no-pr` prints the index entry
for a manual PR instead. Publishing runs from inside the public GitHub repo
that hosts the pack (templates live under its `templates/` directory); the
registry only indexes, never hosts.

The template's `description` and the `--tag` values are what `search` and
the gallery match against — publish will refuse a template without a
description. Write it for the person searching: name the mood and the
destination ("warm light theme for daytime docs"), not the implementation.

Templates are all-in-one: if the template references a theme the user
imported locally, `publish` embeds the palette into the file as an inline
`[theme]` table automatically, so installers see exactly the author's look.

## Audio (WebM only)

Set the output to `.webm` and add an `[audio]` table to get sound — every
tone is synthesized from recipes, no audio files exist anywhere:

```
[output]
file = "demo.webm"

[audio]
keyboard = "mx-brown"    # or mx-red, mx-blue, topre, laptop, typewriter, buckling-spring, none
```

Keystroke sounds come from the recording's input events (or are inferred
from the grid), UI-response pops from the grid diff, and long idle
stretches get a low "thinking" pulse that resolves to a chime — all
automatic. `sound "name" at T` places one-shots (success, error, chime,
sparkle, droplet…); a wrong name fails with the full list of available
recipes, so guess freely. `mute A..B` and `volume 0.15 from A to B` shape
regions. Speeding a region up *drops* key sounds rather than pitch-shifting
them. GIF output ignores audio silently.

Sounds are an open set too, shared through the same registry as templates:
`reel audio list` shows built-ins plus installed, `reel audio search zap`
finds community recipes, `reel audio try <name|file>` synthesizes one to a
WAV to audition, `reel audio add owner/repo/name` installs it (usable in
`sound`, `thinking`, and `bed`). To craft one: `reel audio show chime >
mine.toml`, tweak the tone/noise layers, `try` it, and `reel audio publish
mine.toml --tag ui` opens the registry PR — same description/tag rules as
templates.

Rules of thumb: demos must read fine muted (GitHub/social autoplay silent);
audio is polish, never information. Pass `--no-audio` to A/B a silent
render.

## Themes

`reel themes` lists what's installed (built-ins plus imports). Two ways to
bring the palette the user already loves:

- `reel theme import <file>` — accepts base16 YAML, Alacritty TOML/YAML,
  and iTerm2 `.itermcolors`.
- `reel theme import --from iterm|kitty|ghostty` — reads the user's own
  terminal config directly, no file hunting; it also prints their terminal
  font as a `[style]` suggestion.

Then set `[style] theme = "<name>"` — theme layers over any template.

## Script mode (no human needed)

For demos of *non-interactive or promptable* programs, skip recording:
write a script-mode .reel (no `[source]`) with `run "cmd"`, `type`,
`enter`/`key`, `wait_text "…" timeout Ns`, `wait_idle Ns`, `sleep` — then
`reel run demo.reel` captures and renders in one step. Prefer `wait_text`
over sleeps: it reacts the moment the screen changes and never over-waits.
`[terminal]` sets the PTY size, `[env]` passes variables, `[typing]`
controls typing pace and human jitter. Edit ops (trim/speed/caption/…) in
the same file apply to the capture.

If the render warns about possible secrets on screen (emails, tokens,
ids), add `redact "pattern"` ops before sharing the output.

## Current limits (don't promise these)

- reel renders with the machine's installed fonts (`[style] font` accepts
  any family name). TUI icon glyphs need a Nerd Font installed — if icons
  render as boxes, tell the user to install one (e.g. JetBrainsMono Nerd
  Font) and re-render; no re-recording needed.
- Outputs: `.gif`, `.webm`, `.apng`, `.png` (via `shot`), `.txt`. MP4 is
  deliberately unsupported (licensing); offer WebM instead.
- Windows support is best-effort and untested.
