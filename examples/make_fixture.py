#!/usr/bin/env python3
"""Generates examples/demo.cast — a synthetic but realistic TUI session used
to exercise the renderer: typed prompt, spinner, colors, Nerd Font icons,
box drawing, and a long "thinking" pause worth speed-ramping."""

import json

events = []
t = 0.0


def emit(dt, data):
    global t
    t = round(t + dt, 4)
    events.append([t, "o", data])


E = ""
emit(0.1, f"{E}[2J{E}[H")

# Typed prompt.
emit(0.4, f"{E}[1;35m❯{E}[0m ")
for ch in "reel render demo.reel":
    emit(0.055, ch)
emit(0.35, "\r\n")

# Spinner ("thinking") — braille frames, then cleared.
frames = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"
emit(0.2, f"{E}[?25l")
for i in range(28):
    ch = frames[i % len(frames)]
    emit(0.16, f"\r{E}[36m{ch}{E}[0m {E}[2mrendering frames…{E}[0m")
emit(0.2, f"\r{E}[2K{E}[?25h")

# Results with icons and colors.
emit(0.25, f"{E}[32m✓{E}[0m parsed {E}[1mdemo.reel{E}[0m (edit mode)\r\n")
emit(0.30, f"{E}[32m✓{E}[0m  main • replayed 214 events → 63 snapshots\r\n")
emit(0.30, f"{E}[32m✓{E}[0m  rasterized 78 frames {E}[2m(glyph cache: 97% hits){E}[0m\r\n")
emit(0.35, "\r\n")

# Summary box.
content = "  demo.gif  •  381KB  •  12.4s  •  30fps  "
box_w = len(content)
emit(0.2, f"{E}[38;5;213m╭" + "─" * box_w + "╮\r\n")
emit(0.1, "│" + f"{E}[0m  {E}[1mdemo.gif{E}[0m  •  381KB  •  12.4s  •  30fps  {E}[38;5;213m│\r\n")
emit(0.1, "╰" + "─" * box_w + f"╯{E}[0m\r\n")
emit(0.3, "\r\n")
emit(0.2, f"{E}[2mDone in {E}[0m{E}[1;32m1.2s{E}[0m {E}[33m⚡{E}[0m\r\n")
emit(1.2, f"{E}[1;35m❯{E}[0m ")

header = {"version": 2, "width": 64, "height": 16, "title": "reel fixture"}
with open("examples/demo.cast", "w") as f:
    f.write(json.dumps(header) + "\n")
    for ev in events:
        f.write(json.dumps(ev) + "\n")
print(f"wrote examples/demo.cast: {len(events)} events, {t:.2f}s")
