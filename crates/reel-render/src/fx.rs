//! Raster post-effects — the `crt` template's scanlines, phosphor glow, and
//! vignette. Pure pixel work on the terminal image before chrome composes
//! it, so the window/canvas stays crisp while the "screen" glows.

use tiny_skia::{Pixmap, PremultipliedColorU8};

/// CRT look, all strengths 0..1 (0 = off for that component).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrtEffect {
    /// Peak darkening of the scanline troughs.
    pub scanline: f32,
    /// Bloom amount added around bright pixels.
    pub glow: f32,
    /// Corner darkening.
    pub vignette: f32,
}

pub const CRT_DEFAULT: CrtEffect = CrtEffect { scanline: 0.22, glow: 0.55, vignette: 0.28 };

/// Luma above which a pixel feeds the glow pass.
const GLOW_THRESHOLD: f32 = 110.0;
/// Scanline period in logical pixels (multiplied by the supersample scale).
const SCANLINE_PERIOD: f32 = 3.0;

pub fn apply_crt(pix: &mut Pixmap, fx: &CrtEffect, scale: f32) {
    if fx.glow > 0.0 {
        glow(pix, fx.glow, scale);
    }
    if fx.scanline > 0.0 || fx.vignette > 0.0 {
        shade(pix, fx, scale);
    }
}

/// Bloom: threshold the bright pixels, blur them (3x box ≈ gaussian), add.
fn glow(pix: &mut Pixmap, amount: f32, scale: f32) {
    let w = pix.width() as usize;
    let h = pix.height() as usize;
    let px = pix.pixels();

    let mut bright = vec![0f32; w * h * 3];
    for (i, p) in px.iter().enumerate() {
        let luma = 0.299 * p.red() as f32 + 0.587 * p.green() as f32 + 0.114 * p.blue() as f32;
        if luma > GLOW_THRESHOLD {
            let k = ((luma - GLOW_THRESHOLD) / (255.0 - GLOW_THRESHOLD)).min(1.0);
            bright[i * 3] = p.red() as f32 * k;
            bright[i * 3 + 1] = p.green() as f32 * k;
            bright[i * 3 + 2] = p.blue() as f32 * k;
        }
    }

    let radius = ((1.8 * scale).round() as usize).max(1);
    let mut tmp = vec![0f32; w * h * 3];
    for _ in 0..3 {
        box_blur_h(&bright, &mut tmp, w, h, radius);
        box_blur_v(&tmp, &mut bright, w, h, radius);
    }

    let px = pix.pixels_mut();
    for (i, p) in px.iter_mut().enumerate() {
        let add = |base: u8, g: f32| -> u8 {
            (base as f32 + g * amount).min(255.0) as u8
        };
        let (r, g, b) = (
            add(p.red(), bright[i * 3]),
            add(p.green(), bright[i * 3 + 1]),
            add(p.blue(), bright[i * 3 + 2]),
        );
        // Terminal pixels are opaque; keep alpha and re-premultiply cheaply.
        *p = PremultipliedColorU8::from_rgba(r, g, b, p.alpha()).unwrap_or(*p);
    }
}

fn box_blur_h(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize) {
    let norm = 1.0 / (2 * r + 1) as f32;
    for y in 0..h {
        for c in 0..3 {
            let row = |x: usize| src[(y * w + x) * 3 + c];
            let mut acc: f32 = (0..=r.min(w - 1)).map(row).sum::<f32>()
                + r as f32 * row(0); // clamp-extend left edge
            for x in 0..w {
                dst[(y * w + x) * 3 + c] = acc * norm;
                let out = x.saturating_sub(r);
                let inn = (x + r + 1).min(w - 1);
                acc += row(inn) - row(out);
            }
        }
    }
}

fn box_blur_v(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize) {
    let norm = 1.0 / (2 * r + 1) as f32;
    for x in 0..w {
        for c in 0..3 {
            let col = |y: usize| src[(y * w + x) * 3 + c];
            let mut acc: f32 = (0..=r.min(h - 1)).map(col).sum::<f32>()
                + r as f32 * col(0);
            for y in 0..h {
                dst[(y * w + x) * 3 + c] = acc * norm;
                let out = y.saturating_sub(r);
                let inn = (y + r + 1).min(h - 1);
                acc += col(inn) - col(out);
            }
        }
    }
}

/// Scanlines + vignette in one multiplicative pass.
fn shade(pix: &mut Pixmap, fx: &CrtEffect, scale: f32) {
    let w = pix.width() as usize;
    let h = pix.height() as usize;
    let period = (SCANLINE_PERIOD * scale).max(2.0);
    let row_factor: Vec<f32> = (0..h)
        .map(|y| {
            let phase = (y as f32 / period) * std::f32::consts::TAU;
            1.0 - fx.scanline * (0.5 + 0.5 * phase.cos())
        })
        .collect();
    // Radial falloff separates into nx² + ny² terms.
    let nx2: Vec<f32> = (0..w)
        .map(|x| {
            let nx = (x as f32 / (w - 1).max(1) as f32) * 2.0 - 1.0;
            nx * nx
        })
        .collect();
    let ny2: Vec<f32> = (0..h)
        .map(|y| {
            let ny = (y as f32 / (h - 1).max(1) as f32) * 2.0 - 1.0;
            ny * ny
        })
        .collect();

    let px = pix.pixels_mut();
    for y in 0..h {
        for x in 0..w {
            let f = row_factor[y] * (1.0 - fx.vignette * 0.5 * (nx2[x] + ny2[y])).max(0.0);
            let p = px[y * w + x];
            let m = |v: u8| (v as f32 * f) as u8;
            px[y * w + x] =
                PremultipliedColorU8::from_rgba(m(p.red()), m(p.green()), m(p.blue()), p.alpha())
                    .unwrap_or(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(w: u32, h: u32, v: u8) -> Pixmap {
        let mut p = Pixmap::new(w, h).unwrap();
        for px in p.pixels_mut() {
            *px = PremultipliedColorU8::from_rgba(v, v, v, 255).unwrap();
        }
        p
    }

    #[test]
    fn scanlines_darken_periodically() {
        let mut p = canvas(8, 32, 200);
        apply_crt(&mut p, &CrtEffect { scanline: 0.4, glow: 0.0, vignette: 0.0 }, 1.0);
        let col: Vec<u8> = (0..32).map(|y| p.pixel(4, y).unwrap().red()).collect();
        let min = *col.iter().min().unwrap();
        let max = *col.iter().max().unwrap();
        assert!(max as i32 - min as i32 > 40, "no visible scanlines: {col:?}");
    }

    #[test]
    fn glow_bleeds_light_into_dark_neighbors() {
        let mut p = canvas(31, 31, 0);
        // A bright dot in the middle.
        let m = p.width() as usize / 2;
        p.pixels_mut()[15 * 31 + m] =
            PremultipliedColorU8::from_rgba(255, 255, 255, 255).unwrap();
        apply_crt(&mut p, &CrtEffect { scanline: 0.0, glow: 1.0, vignette: 0.0 }, 2.0);
        let near = p.pixel(m as u32 + 3, 15).unwrap().red();
        assert!(near > 0, "glow did not spread");
        let far = p.pixel(0, 0).unwrap().red();
        assert_eq!(far, 0, "glow spread too far");
    }

    #[test]
    fn vignette_darkens_corners_more_than_center() {
        let mut p = canvas(64, 64, 180);
        apply_crt(&mut p, &CrtEffect { scanline: 0.0, glow: 0.0, vignette: 0.8 }, 1.0);
        let corner = p.pixel(0, 0).unwrap().red();
        let center = p.pixel(32, 32).unwrap().red();
        assert!(center as i32 - corner as i32 > 30, "corner {corner}, center {center}");
    }

    #[test]
    fn zero_effect_is_identity() {
        let mut p = canvas(16, 16, 123);
        let before: Vec<u8> = p.data().to_vec();
        apply_crt(&mut p, &CrtEffect { scanline: 0.0, glow: 0.0, vignette: 0.0 }, 1.0);
        assert_eq!(p.data(), &before[..]);
    }
}
