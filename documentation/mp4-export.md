# MP4 export

reel encodes `.gif`, `.webm`, `.apng` and `.png` itself. `.mp4` is the one
format it doesn't: it shells out to ffmpeg.

## Why ffmpeg and not our own encoder

Every other format reel writes is either patent-free or already a dependency.
H.264 is neither. Shipping an encoder means either bundling OpenH264's C++
(BSD-2 source, but Cisco's patent umbrella only covers *their* binaries) or
building one, and the ladder of alternatives all dead-end:

| Option | Why not |
|---|---|
| `openh264` crate | Fast (measured 1300 fps at 720p on terminal content, ~690 KB of binary), but bundles a C++ encoder and the patent question with it. |
| `x264` | GPL. reel is MIT. |
| `rusty_h264` | Pure Rust and seriously built, but two months old, and measured 25× slower and ~2× larger than openh264 on the same frames. |
| `less-avc` | Pure Rust, but lossless all-intra — output is near-raw. |
| `rav1e` → AV1 in MP4 | Royalty-free, but that's the compatibility story `.webm` already tells. |

Meanwhile the *point* of `.mp4` is reaching players that already decode
H.264 — so the machines that want it are exactly the ones likely to have
ffmpeg. Leaning on it costs nothing at build time, keeps reel's binary and
licensing where they are, and gets libx264's rate control for free.

The tradeoff is honest and stated in the error: no ffmpeg, no `.mp4`.

## What it does

`crates/reel-cli/src/mp4.rs` spawns ffmpeg and pipes frames into its stdin.
Frames arrive change-driven with a duration each, the same as the WebM path;
a rawvideo pipe carries no timestamps, so each frame is held for every output
tick its display window covers. A still stretch therefore goes down the pipe
once per tick, and at 60fps most ticks are duplicates — so the pipe, not the
encoder, is what costs. See [Performance](#performance) for what that means
and what's done about it.

- **Encoder**, best available of `libx264` → `h264_videotoolbox` →
  `libopenh264`, chosen by asking `ffmpeg -encoders`.
- **I420 on the pipe, not RGBA.** The conversion is `reel_encode::yuv` — the
  same BT.709 limited-range one VP9 uses — done once per *distinct* frame,
  with the finished planes re-sent for each tick that repeats them. So
  `.mp4` and `.webm` come out of identical colour maths, and the stream is
  tagged `bt709` rather than left for players to infer from frame size.
- **Quality** is constant-quality wherever the encoder has it: `-crf` for
  libx264, `-q:v` for VideoToolbox, and a bits-per-pixel bitrate for
  libopenh264, which has neither. Terminal frames are mostly flat, so a
  bitrate that suits one recording badly overshoots the next.
- **`-pix_fmt yuv420p -profile:v high`**, and the level left to the encoder.
  Pinning a level breaks large canvases: reel routinely renders past 1080p,
  and VideoToolbox refuses to open (`-12902`) where libx264 only warns.
- **Odd canvases** are padded, not scaled, so glyphs stay pixel-exact.
- **`-movflags +faststart`** puts `moov` ahead of `mdat`, so the file plays
  before it finishes downloading.
- **Audio** is piped in as bare `f32le` and encoded to AAC — Opus in MP4
  doesn't play in Safari or QuickTime, which is the audience that makes MP4
  worth having in the first place.
- **Captions** become an in-band `mov_text` track when `subtitles = true`,
  alongside the `.vtt` sidecar every format writes.

The budget ladder works the same as WebM's, walking CRF instead of CQ. The
two scales don't line up numerically, so the rungs are placed by eye:

```
as configured → crf 28 → scale 1 → crf 33, fps 20 → crf 38, fps 15
```

## Using it

```sh
reel render demo.cast -o demo.mp4
reel render demo.reel -o demo.mp4 --budget 500kb
```

If ffmpeg lives somewhere off `PATH` — a static build, a Nix store path —
point `REEL_FFMPEG` at it.

## Measured

`demo.cast`, 11.8s, 1734×1224, on an M-series Mac. Sizes at 60fps:

| | Size | Notes |
|---|---|---|
| `.webm` (VP9, built in) | 187 KB | |
| `.mp4` (libx264, crf 23) | 215 KB | 139 kb/s |
| `.mp4` + AAC + captions | 278 KB | three streams |
| `.mp4` (h264_videotoolbox) | 1.98 MB | hardware encoders are poor at screen content |

Colour survives the trip: frame 0 of the `.mp4` scores **45.56 dB** PSNR
against the PNG reel renders directly, and the `.webm` scores 45.58 dB —
the same, because it's the same conversion. The ~45 dB is 4:2:0 chroma
subsampling on high-contrast text, not the pipe.

## Performance

Wall clock, best of three, against the `.webm` path on the same cast:

| fps | `.mp4` | `.webm` | frames reaching the encoder |
|---|---|---|---|
| 15 | **0.57s** | 0.93s | 178 vs 82 |
| 30 | **0.97s** | 0.96s | 355 vs 84 |
| 60 | **1.41s** | 0.98s | 709 vs 85 |

WebM's time is flat across frame rates because it re-holds stills at ~5fps
and hands VP9 only ~85 frames whatever the grid. MP4 can't do that — a CFR
rawvideo pipe has nowhere to put a duration — so its cost tracks the tick
count, and 60fps is where the two diverge.

What made that affordable was moving the pixel-format conversion out of
ffmpeg. Measured on 709 frames of 1734×1224 with the encode held constant:

| | ffmpeg time | bytes piped |
|---|---|---|
| RGBA in, ffmpeg converts every tick | 2.59s | 5740 MB |
| I420 in, converted once per distinct frame | **1.21s** | 2152 MB |

2.7× fewer bytes and 53 conversions instead of 709. End to end that took
60fps renders from 2.21s to 1.41s and 15fps renders from 1.21s to 0.57s.
The pad filter, incidentally, costs nothing measurable (2.54s vs 2.59s).

Of the remaining 1.21s of ffmpeg at 60fps, ~0.76s is the pipe itself and
~0.45s is x264 — so the path is now pipe-bound, and reel's own rendering
overlaps it almost entirely.

## Known rough edges

- **VideoToolbox output is large.** It's tuned for camera video, not text.
  It's a fallback for ffmpeg builds without libx264; if you have libx264,
  you'll never see it.
- **No `-q:v` on older VideoToolbox.** Intel Macs and pre-7.x ffmpeg may
  reject it. ffmpeg's error is surfaced verbatim, which is the fix (install
  a build with libx264).
- **Frame duplication is inherent to the CFR pipe.** I420 cut the cost by
  more than half, but a 60fps 4K render still pushes real volume. Escaping
  it entirely means giving ffmpeg per-frame timestamps, which rawvideo has
  no room for — a concat demuxer (one temp file per frame) or writing
  fragmented MP4 ourselves would, at a cost that isn't worth paying yet.
