# The idea behind reel

> Record your TUI once. Edit it like a video. Ship a demo that looks designed.

## The problem

TUI apps and terminal-based AI agents (Claude Code, opencode, lazygit, k9s…)
all need demo recordings for READMEs, launch posts, and docs — and most of
them ship bad ones: 10MB GIFs, unreadable fonts on mobile, 40 seconds of dead
air while an LLM thinks, zero visual polish.

The incumbent, [VHS](https://github.com/charmbracelet/vhs), is a *session
generator*: you script a session in a `.tape` file and it executes it. That
works for simple, deterministic CLIs and breaks down for the modern case:

- Agentic TUIs are non-deterministic, keyboard-driven, and long-running —
  they can't be meaningfully scripted.
- Every style change re-executes the session from scratch. For an AI agent
  demo that means paying for new LLM calls to try a different theme.
- It renders by screen-capturing a headless browser, so it can't do
  pixel-level post-production (zoom, shaders, compositing).
- It needs `ttyd` and `ffmpeg` on `PATH` — recurring install friction.

## The thesis

**reel is a session *editor*, not a session generator.** You capture a
terminal session once, then treat it as a timeline you can trim, cut,
speed-ramp, zoom, caption, restyle, and score with sound — re-rendering in
milliseconds without ever re-running the underlying program.

The one hard architectural rule that makes this work: **capture and render
never touch.** The recording (`.cast` + `.reelmeta` sidecar) is the boundary.
Once a session is captured, the program is never executed again; everything
downstream is a pure function of `(recording, edit file)`. That rule is what
makes `reel watch` instant, iteration free, and output deterministic.

```
session.cast ──▶ VT emulation ──▶ grid snapshots ──▶ timeline ops ──▶ rasterize ──▶ compose ──▶ encode
  (+ .reelmeta)  (alacritty_terminal)              (trim/cut/speed/   (swash +      (chrome,     (GIF, WebM,
                                                    zoom/caption/…)    glyph cache)   fx, shadow)   PNG)
                                                        │
                                                        └─▶ audio events ──▶ synthesize ──▶ mix ──▶ Opus
```

## Why users switch (ranked)

1. **Timeline editing.** `trim`, `cut`, `speed`, `zoom`, `caption`, `freeze`.
   Nobody else in this space does post-production. This is the product.
2. **Templates that look designed.** Complete visual packages — font,
   palette, window chrome, shadow, background, effects — not just color
   themes. The default output quality is the marketing.
3. **Instant iteration.** `reel watch` re-renders on save without
   re-executing anything, because capture and render are separate stages.
4. **File size.** Declarative `budget = "800kb"` — the encoder walks a
   predictable degradation ladder and reports every step. README GIFs
   commonly ship at 5–15MB; edited reel output is a fraction of that.
5. **Single binary.** No `ttyd`, no `ffmpeg`. Codecs are linked at build
   time — our problem, not the user's.
6. **Sound.** Keyboard, UI-response cues, agent-thinking beds — synthesized
   procedurally from recipes, no audio files anywhere. Nobody does this.

## Honest positioning

On raw, unedited playback reel is at **parity** with
[agg](https://github.com/asciinema/agg) (asciinema's GIF renderer) — both
emit frames on change and write delta rectangles, so there is no
order-of-magnitude encoding win to claim, and we don't claim one.

The value is upstream of the encoder: *editing* shrinks output more than any
codec tuning can. Cutting dead air and speed-ramping idle stretches removes
frames entirely — no amount of encoding cleverness competes with not emitting
4/5 of the frames. agg has no timeline ops; that's the gap.

## What reel is explicitly NOT

Scope creep kills projects like this. The boundary is defended:

- **Not a TUI testing framework.** The architecture could support it; it's a
  different product for a different audience.
- **Not a general-purpose scripting language.** No loops, no conditionals,
  no interpreter.
- **Not a screen recorder.** Terminal only — no desktop windows, no GUI apps.
- **Not a hosting/sharing service.** Files out, that's it.
