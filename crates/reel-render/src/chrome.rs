//! Window chrome and canvas compositing: gradient/solid background, drop
//! shadow (separable box blur ×3 ≈ gaussian), rounded window, traffic lights,
//! border, and the terminal content itself.

use crate::raster::{fill_rect, premul};
use crate::template::{CanvasBg, Template, Titlebar, WindowStyle};
use crate::theme::{Rgba, Theme};
use tiny_skia::{
    FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, PixmapPaint, Point,
    Rect, SpreadMode, Transform,
};

const TITLEBAR_H: f32 = 34.0;

pub struct Layout {
    pub canvas_w: u32,
    pub canvas_h: u32,
    /// Terminal content origin on the canvas.
    pub term_x: f32,
    pub term_y: f32,
    /// Window rect on the canvas.
    pub win_x: f32,
    pub win_y: f32,
    pub win_w: f32,
    pub win_h: f32,
    pub titlebar_h: f32,
}

/// Computes the canvas layout for a terminal image of `term_w`×`term_h`
/// pixels. `s` is the supersampling scale (all template dimensions are in
/// logical px and multiplied here).
/// How the canvas should be padded out beyond the window.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CanvasFit {
    /// Minimum width/height ratio; grows (never crops) to reach it.
    pub aspect: Option<f32>,
    /// Exact canvas size; grows and centers to land on it precisely.
    pub exact: Option<(u32, u32)>,
}

pub fn layout(
    tpl: &Template,
    term_w: u32,
    term_h: u32,
    s: f32,
    fit: CanvasFit,
) -> Layout {
    let padding = tpl.padding * s;
    let inset = tpl.inset * s;
    let titlebar_h = if tpl.window != WindowStyle::None && tpl.titlebar != Titlebar::None {
        TITLEBAR_H * s
    } else {
        0.0
    };
    let (win_w, win_h) = match tpl.window {
        WindowStyle::None => (term_w as f32, term_h as f32),
        _ => (term_w as f32 + padding * 2.0, term_h as f32 + padding * 2.0 + titlebar_h),
    };
    let (win_x, win_y) = (inset, inset);
    let (term_x, term_y) = match tpl.window {
        WindowStyle::None => (win_x, win_y),
        _ => (win_x + padding, win_y + padding + titlebar_h),
    };
    let mut canvas_w = (win_w + inset * 2.0).ceil();
    let mut canvas_h = (win_h + inset * 2.0).ceil();
    let (mut dx, mut dy) = (0.0f32, 0.0f32);
    if let Some(ratio) = fit.aspect {
        // Grow (never crop) the deficient dimension and center the window.
        if canvas_w / canvas_h < ratio {
            let grown = (canvas_h * ratio).ceil();
            dx = ((grown - canvas_w) / 2.0).floor();
            canvas_w = grown;
        } else {
            let grown = (canvas_w / ratio).ceil();
            dy = ((grown - canvas_h) / 2.0).floor();
            canvas_h = grown;
        }
    }
    if let Some((ew, eh)) = fit.exact {
        // Grow (never crop) to the exact target, centered. Content larger
        // than the target keeps its computed size; the caller warns.
        if (ew as f32) > canvas_w {
            dx += ((ew as f32 - canvas_w) / 2.0).floor();
            canvas_w = ew as f32;
        }
        if (eh as f32) > canvas_h {
            dy += ((eh as f32 - canvas_h) / 2.0).floor();
            canvas_h = eh as f32;
        }
    }
    // Keep encoder-friendly even dimensions.
    let canvas_w = (canvas_w as u32).next_multiple_of(2);
    let canvas_h = (canvas_h as u32).next_multiple_of(2);
    Layout {
        canvas_w,
        canvas_h,
        term_x: term_x + dx,
        term_y: term_y + dy,
        win_x: win_x + dx,
        win_y: win_y + dy,
        win_w,
        win_h,
        titlebar_h,
    }
}

/// Everything that doesn't change frame to frame: canvas bg, shadow, window
/// body, titlebar, border. Expensive (the shadow blur especially) — compute
/// once per size and reuse.
pub fn compose_base(
    tpl: &Template,
    theme: &Theme,
    term_w: u32,
    term_h: u32,
    s: f32,
    fit: CanvasFit,
) -> Pixmap {
    let l = layout(tpl, term_w, term_h, s, fit);
    let mut canvas = Pixmap::new(l.canvas_w, l.canvas_h).expect("canvas pixmap");

    draw_canvas_bg(&mut canvas, &tpl.canvas);

    let radius = tpl.corner_radius * s;
    if tpl.window != WindowStyle::None {
        if let Some(sh) = &tpl.shadow {
            draw_shadow(&mut canvas, &l, radius, sh.blur * s, sh.opacity, sh.offset_y * s);
        }
        // Window body in the theme background so padding blends with content.
        fill_rounded(&mut canvas, l.win_x, l.win_y, l.win_w, l.win_h, radius, theme.bg);
        match tpl.titlebar {
            Titlebar::TrafficLights => draw_dots(
                &mut canvas,
                &l,
                s,
                &[Rgba::rgb(0xff, 0x5f, 0x57), Rgba::rgb(0xfe, 0xbc, 0x2e), Rgba::rgb(0x28, 0xc8, 0x40)],
            ),
            Titlebar::Dots => {
                let dot = Rgba::rgb(0x44, 0x44, 0x44);
                draw_dots(&mut canvas, &l, s, &[dot, dot, dot]);
            }
            Titlebar::None => {}
        }
        if tpl.titlebar_rule && l.titlebar_h > 0.0 {
            let rule = tpl.border.unwrap_or(Rgba { r: 255, g: 255, b: 255, a: 0x14 });
            crate::raster::fill_rect(
                &mut canvas,
                l.win_x.round() as i32,
                (l.win_y + l.titlebar_h).round() as i32,
                l.win_w.round() as i32,
                (1.0 * s).round().max(1.0) as i32,
                rule,
            );
        }
        if let Some(border) = tpl.border {
            stroke_rounded(&mut canvas, l.win_x, l.win_y, l.win_w, l.win_h, radius, border, s);
        }
    }
    canvas
}

/// Composites one frame onto a clone of the cached base.
pub fn compose_over(
    base: &Pixmap,
    tpl: &Template,
    term: &Pixmap,
    s: f32,
    fit: CanvasFit,
) -> Pixmap {
    let mut canvas = Pixmap::new(1, 1).expect("pixmap");
    compose_over_into(base, tpl, term, s, fit, &mut canvas);
    canvas
}

/// Like [`compose_over`], but reuses `canvas` (copying the cached base into
/// it) instead of cloning a fresh pixmap per frame.
pub fn compose_over_into(
    base: &Pixmap,
    tpl: &Template,
    term: &Pixmap,
    s: f32,
    fit: CanvasFit,
    canvas: &mut Pixmap,
) {
    let l = layout(tpl, term.width(), term.height(), s, fit);
    if canvas.width() != base.width() || canvas.height() != base.height() {
        *canvas = Pixmap::new(base.width(), base.height()).expect("canvas pixmap");
    }
    canvas.data_mut().copy_from_slice(base.data());
    canvas.draw_pixmap(
        l.term_x.round() as i32,
        l.term_y.round() as i32,
        term.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
}

/// One-shot compose (tests, single frames).
pub fn compose(tpl: &Template, theme: &Theme, term: &Pixmap, s: f32) -> Pixmap {
    let base = compose_base(tpl, theme, term.width(), term.height(), s, CanvasFit::default());
    compose_over(&base, tpl, term, s, CanvasFit::default())
}

fn draw_canvas_bg(canvas: &mut Pixmap, bg: &CanvasBg) {
    match *bg {
        CanvasBg::Solid(c) => crate::raster::fill(canvas, c),
        CanvasBg::Linear { angle_deg, from, to } => {
            let (w, h) = (canvas.width() as f32, canvas.height() as f32);
            // CSS angle: 0deg points up, clockwise. Convert to a start→end
            // vector through the canvas center.
            let rad = angle_deg.to_radians();
            let (dx, dy) = (rad.sin(), -rad.cos());
            let half = 0.5 * (w * dx.abs() + h * dy.abs());
            let (cx, cy) = (w / 2.0, h / 2.0);
            let start = Point::from_xy(cx - dx * half, cy - dy * half);
            let end = Point::from_xy(cx + dx * half, cy + dy * half);
            let mut paint = Paint::default();
            paint.shader = LinearGradient::new(
                start,
                end,
                vec![
                    GradientStop::new(0.0, skia_color(from)),
                    GradientStop::new(1.0, skia_color(to)),
                ],
                SpreadMode::Pad,
                Transform::identity(),
            )
            .expect("gradient");
            let rect = Rect::from_xywh(0.0, 0.0, w, h).unwrap();
            canvas.fill_rect(rect, &paint, Transform::identity(), None);
        }
    }
}

fn skia_color(c: Rgba) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(c.r, c.g, c.b, c.a)
}

fn rounded_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> tiny_skia::Path {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let mut pb = PathBuilder::new();
    if r <= 0.5 {
        pb.push_rect(Rect::from_xywh(x, y, w, h).unwrap());
        return pb.finish().unwrap();
    }
    // Cubic approximation of quarter circles.
    let k = 0.5523 * r;
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish().unwrap()
}

fn fill_rounded(canvas: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, c: Rgba) {
    let path = rounded_path(x, y, w, h, r);
    let mut paint = Paint::default();
    paint.set_color(skia_color(c));
    paint.anti_alias = true;
    canvas.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
}

fn stroke_rounded(canvas: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, c: Rgba, s: f32) {
    let path = rounded_path(x, y, w, h, r);
    let mut paint = Paint::default();
    paint.set_color(skia_color(c));
    paint.anti_alias = true;
    let stroke = tiny_skia::Stroke { width: 1.0 * s, ..Default::default() };
    canvas.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

fn draw_dots(canvas: &mut Pixmap, l: &Layout, s: f32, colors: &[Rgba; 3]) {
    let r = 6.0 * s;
    let cy = l.win_y + l.titlebar_h / 2.0;
    for (i, c) in colors.iter().enumerate() {
        let cx = l.win_x + 18.0 * s + i as f32 * 20.0 * s;
        let path = PathBuilder::from_circle(cx, cy, r).unwrap();
        let mut paint = Paint::default();
        paint.set_color(skia_color(*c));
        paint.anti_alias = true;
        canvas.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

/// Drop shadow: render the window's rounded-rect silhouette into an alpha
/// mask, blur it (3-pass box blur ≈ gaussian), tint, and composite offset.
fn draw_shadow(canvas: &mut Pixmap, l: &Layout, radius: f32, blur: f32, opacity: f32, dy: f32) {
    if blur <= 0.0 || opacity <= 0.0 {
        return;
    }
    let (w, h) = (canvas.width() as usize, canvas.height() as usize);
    let mut mask = vec![0f32; w * h];
    // Silhouette (no AA needed pre-blur): a rect check with rounded corners.
    let (rx0, ry0) = (l.win_x, l.win_y + dy);
    let (rx1, ry1) = (l.win_x + l.win_w, l.win_y + l.win_h + dy);
    let r = radius.max(0.0);
    for y in 0..h {
        let fy = y as f32 + 0.5;
        if fy < ry0 || fy > ry1 {
            continue;
        }
        for x in 0..w {
            let fx = x as f32 + 0.5;
            if fx < rx0 || fx > rx1 {
                continue;
            }
            let inside = if r > 0.0 {
                let cx = fx.clamp(rx0 + r, rx1 - r);
                let cy = fy.clamp(ry0 + r, ry1 - r);
                (fx - cx).hypot(fy - cy) <= r
            } else {
                true
            };
            if inside {
                mask[y * w + x] = 1.0;
            }
        }
    }

    // Box blur ×3. Radius chosen so the triple pass approximates a gaussian
    // with sigma ≈ blur/2.
    let box_r = ((blur / 2.0) * 0.8).max(1.0) as usize;
    box_blur(&mut mask, w, h, box_r);
    box_blur(&mut mask, w, h, box_r);
    box_blur(&mut mask, w, h, box_r);

    let data = canvas.pixels_mut();
    for (i, &m) in mask.iter().enumerate() {
        if m <= 0.001 {
            continue;
        }
        let a = (m * opacity * 255.0).min(255.0) as u8;
        if a == 0 {
            continue;
        }
        let shadow = premul(Rgba { r: 0, g: 0, b: 0, a });
        let dst = data[i];
        let ia = 255 - a as u32;
        data[i] = tiny_skia::PremultipliedColorU8::from_rgba(
            (shadow.red() as u32 + dst.red() as u32 * ia / 255) as u8,
            (shadow.green() as u32 + dst.green() as u32 * ia / 255) as u8,
            (shadow.blue() as u32 + dst.blue() as u32 * ia / 255) as u8,
            (a as u32 + dst.alpha() as u32 * ia / 255).min(255) as u8,
        )
        .unwrap();
    }
}

/// Separable box blur, horizontal then vertical, in place.
fn box_blur(buf: &mut [f32], w: usize, h: usize, r: usize) {
    if r == 0 {
        return;
    }
    let norm = 1.0 / (2 * r + 1) as f32;
    let mut tmp = vec![0f32; buf.len()];
    // Horizontal.
    for y in 0..h {
        let row = y * w;
        let mut acc = 0.0;
        for x in 0..(r.min(w)) {
            acc += buf[row + x];
        }
        for x in 0..w {
            if x + r < w {
                acc += buf[row + x + r];
            }
            if x > r {
                acc -= buf[row + x - r - 1];
            }
            tmp[row + x] = acc * norm;
        }
    }
    // Vertical.
    for x in 0..w {
        let mut acc = 0.0;
        for y in 0..(r.min(h)) {
            acc += tmp[y * w + x];
        }
        for y in 0..h {
            if y + r < h {
                acc += tmp[(y + r) * w + x];
            }
            if y > r {
                acc -= tmp[(y - r - 1) * w + x];
            }
            buf[y * w + x] = acc * norm;
        }
    }
}

/// Dims everything outside `rect` (px coords on the given pixmap).
pub fn dim_except(pix: &mut Pixmap, rect: (i32, i32, i32, i32), strength: f32) {
    let (rx, ry, rw, rh) = rect;
    let a = (strength.clamp(0.0, 1.0) * 255.0) as u8;
    let (w, h) = (pix.width() as i32, pix.height() as i32);
    // Four bands around the hole.
    fill_rect(pix, 0, 0, w, ry, Rgba { r: 0, g: 0, b: 0, a });
    fill_rect(pix, 0, ry + rh, w, h - ry - rh, Rgba { r: 0, g: 0, b: 0, a });
    fill_rect(pix, 0, ry, rx, rh, Rgba { r: 0, g: 0, b: 0, a });
    fill_rect(pix, rx + rw, ry, w - rx - rw, rh, Rgba { r: 0, g: 0, b: 0, a });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::builtin;

    #[test]
    fn aspect_grows_and_centers_wide() {
        let tpl = builtin("glass").unwrap();
        let base = layout(&tpl, 800, 500, 1.0, CanvasFit::default());
        let wide = layout(&tpl, 800, 500, 1.0, CanvasFit { aspect: Some(16.0 / 9.0), exact: None });
        let ratio = wide.canvas_w as f32 / wide.canvas_h as f32;
        assert!((ratio - 16.0 / 9.0).abs() < 0.01, "ratio {ratio}");
        assert!(wide.canvas_w >= base.canvas_w && wide.canvas_h >= base.canvas_h, "never crops");
        // The window is centered in the grown dimension.
        let left = wide.win_x;
        let right = wide.canvas_w as f32 - (wide.win_x + wide.win_w);
        assert!((left - right).abs() <= 2.0, "left {left} right {right}");
    }

    #[test]
    fn aspect_grows_height_for_tall_targets() {
        let tpl = builtin("minimal").unwrap();
        let l = layout(&tpl, 1200, 300, 1.0, CanvasFit { aspect: Some(1.0), exact: None });
        assert!((l.canvas_w as f32 / l.canvas_h as f32 - 1.0).abs() < 0.01);
        let top = l.win_y;
        let bottom = l.canvas_h as f32 - (l.win_y + l.win_h);
        assert!((top - bottom).abs() <= 2.0);
    }

    #[test]
    fn exact_size_pads_and_centers() {
        let tpl = builtin("classic").unwrap();
        let l = layout(&tpl, 800, 500, 1.0, CanvasFit { aspect: None, exact: Some((1920, 1080)) });
        assert_eq!((l.canvas_w, l.canvas_h), (1920, 1080));
        let left = l.win_x;
        let right = l.canvas_w as f32 - (l.win_x + l.win_w);
        assert!((left - right).abs() <= 2.0);
        // Content bigger than the target never gets cropped.
        let big = layout(&tpl, 3000, 500, 1.0, CanvasFit { aspect: None, exact: Some((1920, 1080)) });
        assert!(big.canvas_w >= 3000);
    }

    #[test]
    fn canvas_dimensions_stay_even() {
        let tpl = builtin("classic").unwrap();
        for (w, h) in [(101, 55), (333, 217)] {
            let l = layout(&tpl, w, h, 1.0, CanvasFit { aspect: Some(16.0 / 9.0), exact: None });
            assert_eq!(l.canvas_w % 2, 0);
            assert_eq!(l.canvas_h % 2, 0);
        }
    }
}
