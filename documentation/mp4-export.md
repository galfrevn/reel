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

`crates/reel-cli/src/mp4.rs` spawns ffmpeg and pipes RGBA frames into its
stdin. Frames arrive change-driven with a duration each, the same as the WebM
path; a rawvideo pipe carries no timestamps, so each frame is held for every
output tick its display window covers.

- **Encoder**, best available of `libx264` → `h264_videotoolbox` →
  `libopenh264`, chosen by asking `ffmpeg -encoders`.
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

`demo.cast`, 11.8s, 1734×1224 at 60fps, on an M-series Mac:

| | Size | Notes |
|---|---|---|
| `.webm` (VP9, built in) | 187 KB | |
| `.mp4` (libx264, crf 23) | 215 KB | 139 kb/s |
| `.mp4` + AAC + captions | 276 KB | three streams |
| `.mp4` (h264_videotoolbox) | 1.97 MB | hardware encoders are poor at screen content |

Rendering to `.mp4` took 3.3s against 2.2s for `.webm` — the gap is the
duplicated frames going through the pipe, since ffmpeg can't be told "hold
this one for 200ms" over rawvideo.

## Known rough edges

- **VideoToolbox output is large.** It's tuned for camera video, not text.
  It's a fallback for ffmpeg builds without libx264; if you have libx264,
  you'll never see it.
- **No `-q:v` on older VideoToolbox.** Intel Macs and pre-7.x ffmpeg may
  reject it. ffmpeg's error is surfaced verbatim, which is the fix (install
  a build with libx264).
- **Frame duplication.** A long 4K render pushes a lot of bytes through the
  pipe. Nothing has hit this yet, but a fragmented-MP4 or concat-demuxer
  path would avoid it if it ever matters.
