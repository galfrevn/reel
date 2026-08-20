# Using reel

How to install reel, produce your first demo, and — if you'd rather not learn
any of this — let a coding agent do the whole thing for you.

## Install

Using the [agent skill](../skills/reel/SKILL.md)? Skip this section — the
agent installs reel for you the first time it needs it. To install it
yourself:

```sh
curl -fsSL https://raw.githubusercontent.com/galfrevn/reel/main/setup.sh | bash
```

The script grabs a prebuilt binary when one exists for your platform and
falls back to building from source with cargo. To build manually instead,
see [development.md](development.md).

**Fonts.** reel renders with the fonts installed on your machine — nothing
ships in the binary. For TUI demos, install any
[Nerd Font](https://www.nerdfonts.com) build so icon glyphs render; reel
prefers one automatically when present. Fonts dropped in
`~/.config/reel/fonts/` work without a system-wide install.

## Your first demo

**1. Record once.** reel captures over a PTY while you drive the app
normally; Ctrl+D or `exit` ends the recording:

```sh
reel record -o session.cast -- your-tui
```

Don't worry about pauses, typos, or pacing — all of that is fixed in the
edit. Recording writes a `session.cast.reelmeta` sidecar with timestamped
keystrokes (keep the two files together — it's what makes keyboard audio
accurate). Existing asciinema `.cast` files work too. `--size 120x40` pins
the terminal dimensions when you need specific geometry.

**2. Render instantly**, or scaffold an edit file:

```sh
reel render session.cast        # default styling, straight to GIF
reel init glass                 # scaffold demo.reel with the glass template
reel render demo.reel
```

**3. Iterate live.** `reel watch demo.reel --serve` re-renders on every save
with a browser preview at `http://127.0.0.1:4171/`. Because the recording is
frozen, changing theme, template, zoom, or edits is a sub-second re-render —
the program never runs again.

## The `.reel` file

TOML front-matter between `---` fences, then one timeline operation per line:

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

Timestamps use the **recording's own clock** (source time), so edits stay
valid as you add or remove other edits. Bare durations (`for 2.5s`,
`freeze last 1.5s`) are output time — what the viewer experiences.

While recording, `Ctrl+]` drops a **marker** at that instant; edits can
then reference moments by name instead of hunting for timestamps
(`trim @1..@2`, `caption "done" at @2 for 2s` — `reel inspect` lists them).
`marker "name" at T` defines one after the fact. And `keys on` (or
`keys A..B`) overlays the recorded keystrokes as screenkey-style chips —
typed text groups into words, special keys get symbols (`⏎` `^C` `↑`).

Available operations: `trim`, `cut`, `speed`, `hold`, `freeze`, `zoom`,
`pan`, `caption`, `highlight`, `marker`, `keys`, `redact`, `sound`, `mute`,
`volume`. The
complete grammar — every argument, time syntax, style overrides — lives in
the [skill reference](../skills/reel/references/reel-file-format.md).

## Command reference

```
reel record -o FILE -- CMD     # capture over a PTY (--size 120x40; Ctrl+] drops a marker)
reel render FILE               # .reel or .cast → .gif / .webm / .png / .txt
reel watch FILE                # re-render on save; --serve for browser preview
reel shot FILE --at T          # single frame PNG
reel inspect FILE              # timeline summary: duration, size, where changes happen
reel init [template]           # scaffold a .reel file
reel template list|show|add|search|try|publish   # looks, incl. the community registry
reel theme list|import         # themes, incl. base16 / Alacritty / iTerm2 import
reel audio list|show|try|add|search|publish      # sound recipes, same registry
```

## Templates and themes

Pick a template by destination:

| Template | Use when |
|---|---|
| `glass` | Default — gradient canvas, rounded chrome, soft shadow |
| `minimal` | High contrast, no decoration; technical docs |
| `classic` | Bare terminal, no chrome; embeds where chrome would clash |
| `geist` | Pure-black docs look, Geist Mono |
| `paper` | Light background; daytime documentation sites |
| `crt` | Phosphor glow, scanlines, vignette; the eye-catching share |

Customize, discover, or bring your own:

```sh
reel template show glass > mine.toml    # start from a built-in
reel template try mine.toml             # preview it on the bundled demo cast
reel template add mine.toml             # register your edit
reel template search neon               # find community templates
reel template try owner/repo/name       # preview one without installing
reel template add owner/repo            # install a pack from GitHub
reel template publish mine.toml --tag dark  # validate → preview → registry PR
reel theme import my-colors.itermcolors # base16 YAML, Alacritty, iTerm2
```

`--template` also takes a `.toml` path directly anywhere a name works, and
publishing your own pack is a PR — see [registry/README.md](../registry/README.md).

Templates are all-in-one: a template can embed its full color palette as an
inline `[theme]` table instead of referencing a theme by name, and `publish`
embeds your imported theme automatically — whoever installs the template
sees exactly your look, no separate theme install.

Gradient backgrounds (like `glass`) cost GIF palette efficiency — under a
tight `budget`, a solid-canvas template (`minimal`, `classic`, `geist`)
degrades more gracefully.

## Audio (WebM output)

Render to `.webm` and add an `[audio]` table. Every sound is synthesized
from recipes — no audio files exist anywhere, output is byte-identical
everywhere:

- **Keyboard** — per-keystroke press/release from the recorded input, with
  profiles: `mx-brown`, `mx-red`, `mx-blue`, `topre`, `laptop`,
  `typewriter`, `buckling-spring`, `none`.
- **UI cues** — the grid diff knows when the screen answers; a subtle pop,
  zero configuration.
- **Thinking bed** — long idle stretches get a low pulse resolving to a
  chime when output resumes.
- Audio is an event list mixed *after* the timeline resolves: `speed 5x`
  drops keystrokes instead of pitch-shifting them, `cut` deletes their
  sounds, `mute`/`volume` shape regions.

Sounds are shareable like templates. A recipe is a small TOML file (tone
and noise layers with envelopes), and the same registry commands apply:

```sh
reel audio list                        # built-ins + installed
reel audio show chime > mine.toml      # start from a built-in
reel audio try mine.toml               # synthesize to a WAV and listen
reel audio add mine.toml               # install; `sound "mine" at 3s` now works
reel audio search zap                  # find community sounds
reel audio add owner/repo/name         # install one from a pack
reel audio publish mine.toml --tag ui  # validate → audition → registry PR
```

One rule: demos must work muted (GitHub and social feeds autoplay silent).
Audio is polish, never information.

## Let your agent do it

reel ships an [agent skill](../skills/reel/SKILL.md) so coding agents
(Claude Code, Cursor, etc.) can produce the demo for you. Install it from
[skills.sh](https://skills.sh):

```sh
npx skills add galfrevn/reel
```

Then ask your agent for the outcome, not the steps — *"record a demo of my
CLI and make me a README GIF under 800kb with the boring install part sped
up"*. The agent handles recording setup, inspection, the `.reel` edit,
rendering, and size verification; you only perform the live session and
judge the result.

## Current limits

- Outputs are `.gif`, `.webm`, `.png`, `.txt`. MP4 is deliberately
  unsupported (licensing) — use WebM.
- reel's main mode edits recordings that already exist. It can also drive a
  program itself — script mode (`reel run`) types, waits on screen text, and
  renders in one step — but that path suits deterministic CLIs, not
  interactive TUIs. Hybrid (scripted setup, live middle) is
  [on the roadmap](roadmap.md).
- Windows builds compile in CI but are untested.
