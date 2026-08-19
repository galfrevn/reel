# reel vs agg — honest numbers

The spec's Phase 0 validation gate (§11): render the same cast with
[`agg`](https://github.com/asciinema/agg) (asciinema's GIF generator) and
reel, side by side, with file sizes. Measured 2026-08-19 on the same machine,
`agg 1.9.0` vs reel at commit `ee56f2a`, both release builds.

## Raw, unedited renders (apples to apples)

Bare-terminal settings for both: reel used `--template classic --scale 1`
(no chrome, no supersampling), agg used its defaults.

**Synthetic 10s fixture** (`examples/demo.cast`, 64×16, 64 events):

| | size | wall time |
|---|---|---|
| agg | 14.0 KB | 0.59 s |
| reel | 17.9 KB | 0.50 s |

**Real 279s recording** ([asciinema 590145](https://asciinema.org/a/590145),
130×30 zsh + editor session, 2.2 MB cast):

| | size | wall time | canvas |
|---|---|---|---|
| agg | 2.20 MB | 29.5 s | 1272×694 |
| reel | 2.43 MB | 43.7 s | 1248×630 |

**Conclusion: on raw playback, reel is at parity — slightly larger and
slower.** Both tools already emit frames on change and both write delta
rectangles, so there is no order-of-magnitude gap to claim here, and we won't
claim one. The ~10% size difference is palette/antialiasing detail; the speed
difference is untuned rasterization (agg has years of optimization).

## Where reel actually wins

Raw playback is agg's *only* mode. It is reel's *worst case* — the product is
the editor:

1. **Editing shrinks output more than any encoder can.** A README demo of the
   590145 session would `trim`/`cut`/`speed`-ramp the idle stretches: cutting
   output duration cuts frames, and no amount of encoding cleverness competes
   with not emitting 4/5 of the frames at all. agg has no timeline ops.
2. **Declarative size budgets.** `budget = "1mb"` walks a predictable
   degradation ladder (fps → scale → palette) and reports what it chose. With
   agg, hitting a target size is manual re-runs.
3. **Styling without re-recording.** Templates (chrome, shadows, gradients,
   zoom, captions) and instant re-render via `reel watch`. agg restyles only
   via theme/font flags and always re-encodes from scratch.
4. **Lossless-exact palettes when content fits.** Terminal-theme content
   under 256 distinct colors encodes with zero quantization loss.

## The budget ladder on the same recording

`--budget 1mb` on the 590145 cast walks the degradation ladder and reports
every step instead of silently failing:

```
budget: 2.63MB at fps 30 → 20 exceeds 1.00MB, degrading (scale 1 → 1)…
budget: 2.57MB at fps → 15 exceeds 1.00MB, degrading (palette → 128)…
budget: 2.40MB at palette → 128 exceeds 1.00MB, degrading (fps → 10, palette → 64)…
reel_real_budget.gif: 2.30MB — 863 frames, quantized to 64 colors, fps cap 10
warning: could not reach budget 1.00MB even at lowest quality
```

The honest takeaway cuts both ways: a 279-second raw session simply doesn't
fit in 1MB — encoder knobs move it ~13%, not 60%. Hitting real budgets on
long sessions requires *editing* (cut/speed the dead air), which is the
product's whole thesis.

## Known gaps to close (tracked honestly)

- Raw-render wall time: ~1.5× agg on long recordings. Rasterization is
  single-threaded and unprofiled; frame-level parallelism is the obvious win.
- Raw-render size: ~10% over agg at comparable settings; worth a look at
  per-frame palette locality before calling it done.
- Gradient-canvas templates (e.g. `glass`) push GIF output past the exact
  256-color palette and into quantization; reel currently warns rather than
  auto-flattening (the spec's §7.1 mitigation).
- The exact-palette path fires less often than the spec assumed: glyph
  antialiasing alone generates hundreds of fg→bg blend shades at scale 2.
  Real fix: quantize AA ramps to a fixed number of levels per color pair so
  themed content genuinely stays under 256 colors.
