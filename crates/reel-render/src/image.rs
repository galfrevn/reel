//! Image assets for templates: background canvases and badges. PNG and JPEG
//! only — the two formats wallpapers and logos actually come in.

use crate::theme::Rgba;
use std::path::Path;
use std::sync::Arc;
use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, PremultipliedColorU8, Transform};

/// A decoded image plus the path it came from (kept for `template show` and
/// error messages). Equality is by path: two loads of the same file are the
/// same asset as far as template round-tripping cares.
#[derive(Clone)]
pub struct LoadedImage {
    pub pix: Arc<Pixmap>,
    pub path: String,
}

impl std::fmt::Debug for LoadedImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedImage")
            .field("path", &self.path)
            .field("size", &(self.pix.width(), self.pix.height()))
            .finish()
    }
}

impl PartialEq for LoadedImage {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}

/// Loads a PNG or JPEG into a premultiplied pixmap. `path` is resolved
/// against `base` when relative (templates reference assets next to their
/// own TOML file).
pub fn load(path: &str, base: Option<&Path>) -> Result<LoadedImage, String> {
    let resolved = match base {
        Some(dir) if Path::new(path).is_relative() => dir.join(path),
        _ => Path::new(path).to_path_buf(),
    };
    let bytes = std::fs::read(&resolved)
        .map_err(|e| format!("reading image `{}`: {e}", resolved.display()))?;
    let (rgba, w, h) = decode(&bytes)
        .map_err(|e| format!("decoding image `{}`: {e}", resolved.display()))?;
    let mut pix =
        Pixmap::new(w, h).ok_or_else(|| format!("image `{path}` has a zero dimension"))?;
    for (i, px) in rgba.as_chunks::<4>().0.iter().enumerate() {
        let a = px[3] as u16;
        let pm = |v: u8| ((v as u16 * a) / 255) as u8;
        pix.pixels_mut()[i] = PremultipliedColorU8::from_rgba(pm(px[0]), pm(px[1]), pm(px[2]), px[3])
            .unwrap_or(PremultipliedColorU8::TRANSPARENT);
    }
    Ok(LoadedImage { pix: Arc::new(pix), path: path.to_string() })
}

/// Sniffs the format by magic bytes and decodes to straight RGBA.
fn decode(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return decode_png(bytes);
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        return decode_jpeg(bytes);
    }
    Err("not a PNG or JPEG".into())
}

fn decode_png(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; reader.output_buffer_size().ok_or("png too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => {
            buf.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect()
        }
        png::ColorType::Grayscale => buf.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        png::ColorType::GrayscaleAlpha => {
            buf.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()
        }
        other => return Err(format!("unsupported png color type {other:?}")),
    };
    Ok((rgba, info.width, info.height))
}

fn decode_jpeg(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let mut decoder = jpeg_decoder::Decoder::new(bytes);
    let pixels = decoder.decode().map_err(|e| e.to_string())?;
    let info = decoder.info().ok_or("jpeg carries no dimensions")?;
    let (w, h) = (info.width as u32, info.height as u32);
    let rgba = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => {
            pixels.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect()
        }
        jpeg_decoder::PixelFormat::L8 => pixels.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        other => return Err(format!("unsupported jpeg pixel format {other:?}")),
    };
    Ok((rgba, w, h))
}

/// How a background image maps onto the canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFit {
    /// Scale to fill, cropping the overflow (wallpaper semantics).
    Cover,
    /// Scale to fit entirely, letterboxing on the canvas background.
    Contain,
    /// Repeat at native size from the top-left.
    Tile,
}

impl ImageFit {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "cover" => ImageFit::Cover,
            "contain" => ImageFit::Contain,
            "tile" => ImageFit::Tile,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            ImageFit::Cover => "cover",
            ImageFit::Contain => "contain",
            ImageFit::Tile => "tile",
        }
    }
}

/// Draws `img` over the whole canvas with the given fit, then an optional
/// darkening veil (`dim` 0..1) and blur — both there so text stays readable
/// over busy wallpapers.
pub fn draw_background(canvas: &mut Pixmap, img: &Pixmap, fit: ImageFit, dim: f32, blur: f32) {
    let (cw, ch) = (canvas.width() as f32, canvas.height() as f32);
    let (iw, ih) = (img.width() as f32, img.height() as f32);
    let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() };
    match fit {
        ImageFit::Tile => {
            let mut y = 0.0;
            while y < ch {
                let mut x = 0.0;
                while x < cw {
                    canvas.draw_pixmap(
                        x as i32,
                        y as i32,
                        img.as_ref(),
                        &paint,
                        Transform::identity(),
                        None,
                    );
                    x += iw;
                }
                y += ih;
            }
        }
        ImageFit::Cover | ImageFit::Contain => {
            let scale = if fit == ImageFit::Cover {
                (cw / iw).max(ch / ih)
            } else {
                (cw / iw).min(ch / ih)
            };
            let (dx, dy) = ((cw - iw * scale) / 2.0, (ch - ih * scale) / 2.0);
            canvas.draw_pixmap(
                0,
                0,
                img.as_ref(),
                &paint,
                Transform::from_row(scale, 0.0, 0.0, scale, dx, dy),
                None,
            );
        }
    }
    if blur > 0.0 {
        blur_region(canvas, 0, 0, canvas.width(), canvas.height(), blur);
    }
    if dim > 0.0 {
        let a = (dim.clamp(0.0, 1.0) * 255.0) as u8;
        crate::raster::fill_rect(
            canvas,
            0,
            0,
            canvas.width() as i32,
            canvas.height() as i32,
            Rgba { r: 0, g: 0, b: 0, a },
        );
    }
}

/// Box-blurs (×3 ≈ gaussian) an axis-aligned region of the canvas in place.
/// Shared by image backgrounds and the window's backdrop blur.
pub fn blur_region(pix: &mut Pixmap, x: u32, y: u32, w: u32, h: u32, blur: f32) {
    let (pw, ph) = (pix.width(), pix.height());
    let x1 = (x + w).min(pw);
    let y1 = (y + h).min(ph);
    if x >= x1 || y >= y1 {
        return;
    }
    let (rw, rh) = ((x1 - x) as usize, (y1 - y) as usize);
    let px = pix.pixels_mut();
    // Work in straight RGB floats; the region is background, alpha stays.
    let mut buf = vec![0f32; rw * rh * 3];
    for ry in 0..rh {
        for rx in 0..rw {
            let p = px[(y as usize + ry) * pw as usize + x as usize + rx];
            let i = (ry * rw + rx) * 3;
            buf[i] = p.red() as f32;
            buf[i + 1] = p.green() as f32;
            buf[i + 2] = p.blue() as f32;
        }
    }
    let radius = ((blur / 2.0).max(1.0)) as usize;
    let mut tmp = vec![0f32; buf.len()];
    for _ in 0..3 {
        crate::fx::box_blur_h(&buf, &mut tmp, rw, rh, radius);
        crate::fx::box_blur_v(&tmp, &mut buf, rw, rh, radius);
    }
    for ry in 0..rh {
        for rx in 0..rw {
            let i = (ry * rw + rx) * 3;
            let p = &mut px[(y as usize + ry) * pw as usize + x as usize + rx];
            let a = p.alpha();
            let clamp = |v: f32| (v.min(a as f32)) as u8;
            *p = PremultipliedColorU8::from_rgba(
                clamp(buf[i]),
                clamp(buf[i + 1]),
                clamp(buf[i + 2]),
                a,
            )
            .unwrap_or(*p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_png() -> Vec<u8> {
        // 2x1 opaque red/blue.
        let mut bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut bytes, 2, 1);
            enc.set_color(png::ColorType::Rgba);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
        }
        bytes
    }

    #[test]
    fn png_roundtrips_through_load() {
        let dir = std::env::temp_dir().join("reel-image-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dot.png");
        std::fs::write(&path, tiny_png()).unwrap();
        let img = load(path.to_str().unwrap(), None).unwrap();
        assert_eq!((img.pix.width(), img.pix.height()), (2, 1));
        assert_eq!(img.pix.pixel(0, 0).unwrap().red(), 255);
        assert_eq!(img.pix.pixel(1, 0).unwrap().blue(), 255);
    }

    #[test]
    fn relative_paths_resolve_against_base() {
        let dir = std::env::temp_dir().join("reel-image-base-test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bg.png"), tiny_png()).unwrap();
        assert!(load("bg.png", Some(&dir)).is_ok());
        assert!(load("bg.png", None).is_err() || std::path::Path::new("bg.png").exists());
    }

    #[test]
    fn cover_fills_the_canvas() {
        let dir = std::env::temp_dir().join("reel-image-cover-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dot.png");
        std::fs::write(&path, tiny_png()).unwrap();
        let img = load(path.to_str().unwrap(), None).unwrap();
        let mut canvas = Pixmap::new(8, 8).unwrap();
        draw_background(&mut canvas, &img.pix, ImageFit::Cover, 0.0, 0.0);
        assert!(canvas.pixels().iter().all(|p| p.alpha() == 255));
    }

    #[test]
    fn dim_darkens() {
        let dir = std::env::temp_dir().join("reel-image-dim-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dot.png");
        std::fs::write(&path, tiny_png()).unwrap();
        let img = load(path.to_str().unwrap(), None).unwrap();
        let mut a = Pixmap::new(4, 4).unwrap();
        let mut b = Pixmap::new(4, 4).unwrap();
        draw_background(&mut a, &img.pix, ImageFit::Cover, 0.0, 0.0);
        draw_background(&mut b, &img.pix, ImageFit::Cover, 0.5, 0.0);
        assert!(b.pixel(0, 0).unwrap().red() < a.pixel(0, 0).unwrap().red());
    }
}
