---
name: reel-motion
description: Combine a reel terminal demo with motion graphics — animated titles, product shots, a scored launch film — by exporting the edited timeline as a constant-rate PNG sequence plus a frames.json manifest, then composing it in Remotion, Motion Canvas, or an NLE. Also the way to get an MP4 out of reel, which deliberately ships no H.264 encoder. Use this skill when the user wants a launch video, promo, trailer, demo reel, or "video for the landing page" that mixes terminal footage with animation; when they ask to put a terminal recording into Remotion, After Effects, Premiere, DaVinci or Final Cut; or when they want an MP4 of a terminal demo.
---

# Terminal footage in a motion-graphics pipeline

reel edits terminal recordings — it cuts, speed-ramps, zooms, annotates and
scores them. It does not animate logos, build kinetic type, or composite
product shots, and it should not try to. When a demo needs to become a
*launch film*, reel produces the footage and something else composes it.

`--frames-out` is that handoff. It writes a constant-rate PNG sequence and a
`frames.json` manifest describing the edit — every marker, caption, note,
card, zoom and speed ramp, in output seconds **and** in frame numbers.

That manifest is the reason this handoff is better than exporting a video
file. A compositor handed raw footage has to eyeball where the interesting
moments are; reel already knows, because it cut the timeline.

**Prerequisite:** the terminal demo itself. Use the `reel` skill to record
and edit it first. This skill starts from a `.reel` file that already
renders the demo you want.

## 1. Edit before you export

Do not export a raw recording and cut it in the compositor. Trimming dead
air, speed-ramping a wait, and zooming the payoff are cheap in reel
(sub-second re-renders on a frozen recording) and expensive in a timeline.
Get the demo *right* as a GIF or WebM first, then export.

Two settings matter for this destination, both in the `.reel`:

```toml
[output]
fps  = 30           # or 60 — match the composition exactly
size = "1920x1080"  # render at final size; never upscale terminal text
```

Upscaled monospace looks soft and instantly reads as a screen recording.
Rendering at the target size lets reel solve the font size to fit, so the
text is crisp at 100%.

If the terminal is going in a framed card rather than full-bleed, render at
the card's pixel size, not the canvas's.

## 2. Export the sequence

```sh
reel render demo.reel --frames-out frames/
```

Writes `frames/0000.png`, `0001.png`, … at the configured fps, plus
`frames/frames.json`. When the `.reel` configures `[audio]`, it also writes
`frames/audio.wav` — reel's procedural keystroke track, already in sync with
the frames beside it.

Add `--json` for a machine-readable summary (frame count, dimensions, paths).

**Expect it to be big.** The sequence is lossless PNG and held frames are
written out in full, so a 12-second demo at 1080p is tens of megabytes.
That is correct for an intermediate; delete `frames/` once the master is
rendered, and never commit it.

## 3. Read the manifest

```jsonc
{
  "schema": 1,
  "fps": 30,
  "frames": 345,             // total, constant-rate
  "duration_s": 11.5,
  "width": 1734, "height": 1224,
  "pattern": "%04d.png",     // printf-style, for ffmpeg and NLEs
  "audio": "audio.wav",      // null when the demo is silent
  "source": { "cast": "demo.cast", "duration_s": 11.8, "cols": 80, "rows": 24 },

  "segments":    [ /* play/still, with rate, in seconds and frames */ ],
  "speed_ramps": [ { "rate": 3.0, "frame_start": 141, "frame_end": 171 } ],
  "markers":     [ { "label": "done", "out_t_s": 8.2, "frame": 246 } ],
  "chapters":    [ /* same as markers, under the name video tools use */ ],
  "cards":       [ { "text": "1 · Install", "frame_start": 0, "frame_end": 36 } ],
  "captions":    [ { "text": "the money shot", "frame_start": 201, "frame_end": 261 } ],
  "notes":       [ /* callouts, with their anchor cell */ ],
  "highlights":  [ /* rects, in cells */ ],
  "zooms":       [ /* factor, center cell, range */ ],
  "ffmpeg":      "ffmpeg -framerate 30 -i frames/%04d.png …"
}
```

Every entry carries both `*_s` (seconds) and `frame*` fields. Use the frame
numbers — compositors count in frames, and rounding a second back into a
frame is how sync drifts.

**Markers are the sync points.** Tell the user to press `Ctrl+]` while
recording at each moment they'll want to hit — the install finishing, the
test going green — and those land in `markers` with names. Then a title
lands *on* the beat instead of near it.

## 4. Compose in Remotion

Scaffold if there's no project yet:

```sh
npx create-video@latest
npx remotion add @remotion/media   # only if you need the audio track
```

Put the export where Remotion can serve it: `public/frames/`.

Let the manifest drive the composition — duration, fps and dimensions all
come from the render, so re-editing the demo and re-exporting keeps
everything in step with no hand-editing:

```tsx
// src/Root.tsx
import { Composition, CalculateMetadataFunction, staticFile } from "remotion";
import { Promo, type Manifest } from "./Promo";

const calculateMetadata: CalculateMetadataFunction<{ manifest: Manifest }> = async () => {
  const manifest: Manifest = await fetch(staticFile("frames/frames.json")).then((r) => r.json());
  return {
    props: { manifest },
    durationInFrames: manifest.frames,
    fps: manifest.fps,
    width: manifest.width,
    height: manifest.height,
  };
};

export const RemotionRoot = () => (
  <Composition
    id="Promo"
    component={Promo}
    durationInFrames={300}
    fps={30}
    width={1920}
    height={1080}
    defaultProps={{ manifest: null as unknown as Manifest }}
    calculateMetadata={calculateMetadata}
  />
);
```

The terminal itself is one `<Img>` per frame. Use Remotion's `<Img>`, never
a bare `<img>` — it guarantees the image is decoded before the frame is
captured, which is the difference between a clean render and random blank
frames:

```tsx
// src/Promo.tsx
import { AbsoluteFill, Img, Sequence, staticFile, useCurrentFrame, useVideoConfig } from "remotion";
import { Audio } from "@remotion/media";

export type Manifest = {
  fps: number; frames: number; width: number; height: number; audio: string | null;
  markers: { label: string; frame: number }[];
  speed_ramps: { rate: number; frame_start: number; frame_end: number }[];
};

const pad = (n: number) => String(n).padStart(4, "0");

const Terminal: React.FC<{ total: number; offset?: number }> = ({ total, offset = 0 }) => {
  const frame = useCurrentFrame();
  const i = Math.min(Math.max(frame + offset, 0), total - 1);
  return <Img src={staticFile(`frames/${pad(i)}.png`)} />;
};

export const Promo: React.FC<{ manifest: Manifest }> = ({ manifest }) => {
  const { fps } = useVideoConfig();
  const done = manifest.markers.find((m) => m.label === "done");

  return (
    <AbsoluteFill style={{ backgroundColor: "#0b0b0d" }}>
      <Terminal total={manifest.frames} />

      {manifest.audio && <Audio src={staticFile(`frames/${manifest.audio}`)} />}

      {/* A title that lands on the beat the user marked while recording. */}
      {done && (
        <Sequence from={done.frame} durationInFrames={2 * fps} premountFor={fps}>
          <Headline>Green in 4 seconds</Headline>
        </Sequence>
      )}
    </AbsoluteFill>
  );
};
```

Render the master:

```sh
npx remotion render Promo out/promo.mp4 --crf 15
```

Other things the manifest makes easy, in rough order of usefulness:

- **Scene boundaries from `cards`** — a demo already sectioned with
  `card "1 · Install"` gives you its own chapter list; use `<Series>` with
  each card's frame span as a scene.
- **Reacting to `speed_ramps`** — a music cue or a `▸▸` flourish over the
  compressed stretch reads as intentional rather than as a glitch.
- **Suppressing reel's own overlays** — if you'd rather animate the titles
  in React, drop `caption`/`note`/`card` from the `.reel` and re-export.
  The manifest still lists them, so you keep the timings and lose the
  burned-in pixels. This is usually the right call for a polished film.

## Just want an MP4?

reel ships no H.264 encoder (licensing), so this is the path:

```sh
reel render demo.reel --frames-out frames/
# the exact line, with the right pixel format and even dimensions, is
# printed after the export and stored in frames.json under "ffmpeg":
ffmpeg -framerate 30 -i frames/%04d.png -c:v libx264 -crf 18 \
  -pix_fmt yuv420p -vf "scale=trunc(iw/2)*2:trunc(ih/2)*2" out.mp4
```

`-pix_fmt yuv420p` is what makes it play in browsers and Quicktime; the
`scale` filter exists because H.264 rejects odd dimensions. When the demo
has audio, add `-i frames/audio.wav -c:a aac -shortest`.

Don't reach for this when a GIF or WebM would do — `reel render` produces
those directly, with a size budget.

## Other compositors

- **Motion Canvas** — import the sequence as an image sequence; read
  `frames.json` in the project to drive `waitUntil` beats from markers.
- **Premiere / Final Cut / DaVinci** — File → Import, pick `0000.png`, tick
  "image sequence", set the frame rate to the manifest's `fps`. Import
  `audio.wav` as a separate track; it starts at 0 and needs no offset.
- **After Effects** — same import, then use the marker frames to place layer
  markers on the comp.

## Design notes

These are the ones that actually decide whether it looks professional:

- **Never scale the terminal above 100%.** Re-export at the right size
  instead. Soft text is the single biggest tell.
- **One idea per scene.** Cut when the visual arrives, not after it settles.
- **Match the background to the template's canvas color** so the terminal
  card sits on the page instead of floating over an unrelated gradient.
- **Let the terminal breathe.** If the demo is the point, give it whole
  seconds without a title on top of it.
- **Length:** social cut under 30s; a feature tour ~60–90s.
- **Audio:** reel's keystroke track is a texture, not a soundtrack. Duck it
  under music rather than dropping it — typing you can hear is a lot of the
  reason terminal footage feels alive.

## Gotchas

- **fps must match.** If the `.reel` says 30 and the composition says 60,
  every frame plays twice. Read `manifest.fps` rather than hardcoding.
- **Re-exporting overwrites, it doesn't clean.** A shorter re-edit leaves
  the old tail behind — `rm -rf frames/` before re-exporting, or the
  sequence ends with stale frames.
- **Don't hand-edit files in `frames/`.** Held frames are written as
  independent copies; editing one and not its duplicates produces a
  one-frame flicker that is miserable to track down. Fix it in the `.reel`
  and re-export.
- **`--frames-out` replaces the video encode.** It does not also write the
  `.gif`/`.webm` named in `[output]`; run `reel render` again without the
  flag if you want both.
- **Frame numbers assume the manifest's own fps.** If you retime in the
  compositor, the marker frames no longer point where you think.
