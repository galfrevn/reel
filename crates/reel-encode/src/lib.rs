//! Encoders. The GIF path implements the size strategy from the spec:
//! change-driven frames (upstream), exact palette when the content fits in
//! 256 colors (terminal themes almost always do), delta rectangles, and a
//! quantizer fallback for gradient-heavy content.

mod opus;
pub mod webm;
mod yuv;

#[cfg(feature = "video")]
mod vp9;

use std::borrow::Cow;
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("gif encoding failed: {0}")]
    Gif(#[from] gif::EncodingError),
    #[error("png encoding failed: {0}")]
    Png(#[from] png::EncodingError),
    #[error("no frames to encode")]
    NoFrames,
    #[error("frame {0} has mismatched dimensions")]
    DimensionMismatch(usize),
    #[error("vp9 encoding failed: {0}")]
    Vpx(String),
    #[error("opus encoding failed: {0}")]
    Opus(String),
    #[error("this build of reel has no video support (compiled without the `video` feature; libvpx is required)")]
    VideoDisabled,
}

// ---------------------------------------------------------------------------
// WebM (VP9 + Opus)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct WebmOptions {
    /// Constrained-quality level, 0 (best) to 63 (worst).
    pub cq_level: u32,
    /// Bitrate cap in kbit/s; `None` picks one from the frame area.
    pub bitrate_kbps: Option<u32>,
    /// libvpx speed dial, 0 (slowest/best) to 9.
    pub cpu_used: i32,
    /// Emit constant-frame-rate video at this fps, duplicating stills.
    /// Players judder on variable frame rate; VP9 encodes an unchanged
    /// frame in a few hundred bytes, so CFR costs almost nothing.
    pub cfr_fps: Option<u32>,
}

impl Default for WebmOptions {
    fn default() -> Self {
        WebmOptions { cq_level: 24, bitrate_kbps: None, cpu_used: 2, cfr_fps: Some(60) }
    }
}

pub struct WebmReport {
    pub bytes: Vec<u8>,
    pub frames: usize,
    pub has_audio: bool,
    pub cq_level: u32,
    pub bitrate_kbps: u32,
}

/// Encodes frames (+ optional 48kHz mono audio) to WebM: VP9 + Opus.
#[cfg(feature = "video")]
pub fn encode_webm(
    frames: &[RgbaFrame],
    audio: Option<&[f32]>,
    opts: &WebmOptions,
) -> Result<WebmReport, EncodeError> {
    let first = frames.first().ok_or(EncodeError::NoFrames)?;
    let (w, h) = (first.width, first.height);
    for (i, f) in frames.iter().enumerate() {
        if f.width != w || f.height != h {
            return Err(EncodeError::DimensionMismatch(i));
        }
    }
    let bitrate = opts
        .bitrate_kbps
        .unwrap_or_else(|| (w * h / 500).clamp(300, 4000));

    let mut enc = vp9::Vp9Encoder::new(w, h, &vp9::Vp9Config {
        cq_level: opts.cq_level,
        bitrate_kbps: bitrate,
        cpu_used: opts.cpu_used,
    })?;
    let mut blocks = Vec::with_capacity(frames.len() + 64);
    let mut clock_ms = 0f64;
    match opts.cfr_fps {
        Some(fps) => {
            // Constant frame rate: walk a fixed clock, converting to YUV
            // only when the source frame actually changes underneath it.
            let fps = fps.clamp(1, 120) as f64;
            let total_s: f64 = frames.iter().map(|f| f.duration_s).sum();
            let n = ((total_s * fps).round() as usize).max(1);
            let mut src = 0usize;
            let mut src_end = frames[0].duration_s;
            let mut img = yuv::rgba_to_i420(w, h, &frames[0].data);
            for k in 0..n {
                let t = k as f64 / fps;
                let mut changed = false;
                while t >= src_end - 1e-9 && src + 1 < frames.len() {
                    src += 1;
                    src_end += frames[src].duration_s;
                    changed = true;
                }
                if changed {
                    img = yuv::rgba_to_i420(w, h, &frames[src].data);
                }
                let pts = (k as f64 * 1000.0 / fps).round() as i64;
                let next_pts = ((k + 1) as f64 * 1000.0 / fps).round() as i64;
                blocks.extend(enc.encode(&img, pts, (next_pts - pts).max(1) as u64)?);
            }
            clock_ms = total_s * 1000.0;
        }
        None => {
            for f in frames {
                let pts = clock_ms.round() as i64;
                clock_ms += f.duration_s * 1000.0;
                let dur = (clock_ms.round() as i64 - pts).max(1) as u64;
                let img = yuv::rgba_to_i420(w, h, &f.data);
                blocks.extend(enc.encode(&img, pts, dur)?);
            }
        }
    }
    blocks.extend(enc.finish()?);
    let frame_count = blocks.len();

    let has_audio = match audio {
        Some(samples) if !samples.is_empty() => {
            blocks.extend(opus::encode_opus(samples)?);
            true
        }
        _ => false,
    };

    let video_track = webm::VideoTrack { width: w, height: h };
    let audio_track = webm::AudioTrack {
        channels: 1,
        sample_rate: opus::OPUS_SAMPLE_RATE,
        pre_skip: 0,
    };
    let bytes = webm::mux(
        &video_track,
        has_audio.then_some(&audio_track),
        blocks,
        clock_ms,
    );
    Ok(WebmReport { bytes, frames: frame_count, has_audio, cq_level: opts.cq_level, bitrate_kbps: bitrate })
}

/// Stub so callers get a clear runtime error instead of a compile break when
/// the `video` feature is off.
#[cfg(not(feature = "video"))]
pub fn encode_webm(
    _frames: &[RgbaFrame],
    _audio: Option<&[f32]>,
    _opts: &WebmOptions,
) -> Result<WebmReport, EncodeError> {
    Err(EncodeError::VideoDisabled)
}

/// One output frame: straight (non-premultiplied) RGBA and a display duration.
pub struct RgbaFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub duration_s: f64,
}

#[derive(Debug, Clone)]
pub struct GifOptions {
    pub looping: bool,
    /// Maximum palette size (2-256). The exact-palette path uses however many
    /// colors the frames actually contain, up to this.
    pub max_colors: u16,
}

impl Default for GifOptions {
    fn default() -> Self {
        GifOptions { looping: true, max_colors: 256 }
    }
}

/// How the encoder resolved colors — reported to the user so the budget loop
/// isn't a black box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteMode {
    /// Content had ≤ max_colors distinct colors: lossless.
    Exact(u16),
    /// NeuQuant quantization to this many colors.
    Quantized(u16),
}

pub struct GifReport {
    pub bytes: Vec<u8>,
    pub palette: PaletteMode,
    pub frames: usize,
}

pub fn encode_gif(frames: &[RgbaFrame], opts: &GifOptions) -> Result<GifReport, EncodeError> {
    let first = frames.first().ok_or(EncodeError::NoFrames)?;
    let (w, h) = (first.width, first.height);
    for (i, f) in frames.iter().enumerate() {
        if f.width != w || f.height != h {
            return Err(EncodeError::DimensionMismatch(i));
        }
    }
    let max_colors = opts.max_colors.clamp(2, 256);

    // --- Palette: exact if possible, else NeuQuant ------------------------
    let mut color_set: HashMap<[u8; 3], u8> = HashMap::new();
    let mut exact = true;
    'scan: for f in frames {
        for px in f.data.chunks_exact(4) {
            let key = [px[0], px[1], px[2]];
            let next = color_set.len();
            if !color_set.contains_key(&key) {
                if next >= max_colors as usize {
                    exact = false;
                    break 'scan;
                }
                color_set.insert(key, next as u8);
            }
        }
    }

    let (palette_rgb, mut index_fn, mode): (Vec<u8>, Box<dyn FnMut(&[u8]) -> u8>, PaletteMode) = if exact
    {
        let mut palette = vec![0u8; color_set.len() * 3];
        for (rgb, idx) in &color_set {
            let i = *idx as usize * 3;
            palette[i..i + 3].copy_from_slice(rgb);
        }
        let set = color_set.clone();
        let n = color_set.len() as u16;
        (
            palette,
            Box::new(move |px: &[u8]| *set.get(&[px[0], px[1], px[2]]).unwrap_or(&0)),
            PaletteMode::Exact(n),
        )
    } else {
        // Sample pixels for speed: NeuQuant is O(samples).
        let mut samples = Vec::with_capacity(1 << 20);
        let total: usize = frames.iter().map(|f| f.data.len() / 4).sum();
        let stride = (total / 200_000).max(1);
        let mut k = 0usize;
        for f in frames {
            for px in f.data.chunks_exact(4) {
                if k % stride == 0 {
                    samples.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
                k += 1;
            }
        }
        let nq = color_quant::NeuQuant::new(10, max_colors as usize, &samples);
        let map = nq.color_map_rgb();
        let n = (map.len() / 3) as u16;
        // index_of is a search; terminal frames repeat a few thousand
        // distinct RGBs, so memoizing makes indexing effectively free.
        let mut memo: HashMap<[u8; 3], u8> = HashMap::new();
        (
            map,
            Box::new(move |px: &[u8]| {
                *memo
                    .entry([px[0], px[1], px[2]])
                    .or_insert_with(|| nq.index_of(&[px[0], px[1], px[2], 255]) as u8)
            }),
            PaletteMode::Quantized(n),
        )
    };

    // --- Index all frames --------------------------------------------------
    let indexed: Vec<Vec<u8>> = frames
        .iter()
        .map(|f| f.data.chunks_exact(4).map(&mut index_fn).collect())
        .collect();

    // --- Write, with delta rectangles --------------------------------------
    let mut bytes = Vec::new();
    {
        let mut enc = gif::Encoder::new(&mut bytes, w as u16, h as u16, &palette_rgb)?;
        enc.set_repeat(if opts.looping { gif::Repeat::Infinite } else { gif::Repeat::Finite(0) })?;

        // GIF delays are centiseconds; carry the rounding error so long
        // videos don't drift.
        let mut carry = 0.0f64;
        let mut delay_cs = |dur_s: f64| -> u16 {
            let exact_cs = dur_s * 100.0 + carry;
            let cs = exact_cs.round().max(2.0); // <2cs renders erratically in browsers
            carry = exact_cs - cs;
            cs as u16
        };

        for (i, idx) in indexed.iter().enumerate() {
            let delay = delay_cs(frames[i].duration_s);
            let mut frame = if i == 0 {
                gif::Frame {
                    width: w as u16,
                    height: h as u16,
                    buffer: Cow::Borrowed(idx.as_slice()),
                    ..Default::default()
                }
            } else {
                match diff_rect(&indexed[i - 1], idx, w as usize, h as usize) {
                    // Nothing changed pixel-wise (can happen after palette
                    // quantization); emit a 1x1 keep-alive to carry the delay.
                    None => gif::Frame {
                        width: 1,
                        height: 1,
                        buffer: Cow::Borrowed(&idx[..1]),
                        ..Default::default()
                    },
                    Some((x0, y0, x1, y1)) => {
                        let rw = x1 - x0 + 1;
                        let rh = y1 - y0 + 1;
                        let mut buf = Vec::with_capacity(rw * rh);
                        for y in y0..=y1 {
                            let row = y * w as usize;
                            buf.extend_from_slice(&idx[row + x0..row + x1 + 1]);
                        }
                        gif::Frame {
                            left: x0 as u16,
                            top: y0 as u16,
                            width: rw as u16,
                            height: rh as u16,
                            buffer: Cow::Owned(buf),
                            ..Default::default()
                        }
                    }
                }
            };
            frame.delay = delay;
            frame.dispose = gif::DisposalMethod::Keep;
            enc.write_frame(&frame)?;
        }
    }

    Ok(GifReport { bytes, palette: mode, frames: frames.len() })
}

/// Bounding box (x0, y0, x1, y1) of differing pixels, or None if identical.
fn diff_rect(a: &[u8], b: &[u8], w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
    let mut x0 = w;
    let mut y0 = h;
    let mut x1 = 0usize;
    let mut y1 = 0usize;
    for y in 0..h {
        let row = y * w;
        let ra = &a[row..row + w];
        let rb = &b[row..row + w];
        if ra == rb {
            continue;
        }
        let first = ra.iter().zip(rb).position(|(p, q)| p != q).unwrap();
        let last = w - 1 - ra.iter().zip(rb).rev().position(|(p, q)| p != q).unwrap();
        x0 = x0.min(first);
        x1 = x1.max(last);
        y0 = y0.min(y);
        y1 = y1.max(y);
    }
    (y1 >= y0 && x1 >= x0).then_some((x0, y0, x1, y1))
}

/// Straight-RGBA → PNG.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, EncodeError> {
    let mut bytes = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut bytes, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header()?;
        writer.write_image_data(rgba)?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 3], dur: f64) -> RgbaFrame {
        let mut data = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            data.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
        RgbaFrame { width: w, height: h, data, duration_s: dur }
    }

    #[test]
    fn exact_palette_for_flat_content() {
        let frames = vec![solid(20, 10, [0, 0, 0], 0.5), solid(20, 10, [255, 0, 0], 0.5)];
        let rep = encode_gif(&frames, &GifOptions::default()).unwrap();
        assert_eq!(rep.palette, PaletteMode::Exact(2));
        assert!(rep.bytes.starts_with(b"GIF89a"));
    }

    #[test]
    fn diff_rect_finds_tight_bbox() {
        let w = 20;
        let a = vec![0u8; w * 10];
        let mut b = a.clone();
        b[3 * w + 5] = 1;
        b[6 * w + 12] = 1;
        assert_eq!(diff_rect(&a, &b, w, 10), Some((5, 3, 12, 6)));
        assert_eq!(diff_rect(&a, &a, w, 10), None);
    }

    #[test]
    fn delta_rects_keep_animated_gifs_small() {
        // 200x100, 10 frames, only a 6x4 box changes → the whole animation
        // should stay near first-frame size, not 10x it.
        let mut frames = vec![];
        for i in 0..10u8 {
            let mut f = solid(200, 100, [10, 10, 10], 0.1);
            for y in 40..44usize {
                for x in 60..66usize {
                    let p = (y * 200 + x) * 4;
                    f.data[p] = 200 + i;
                }
            }
            frames.push(f);
        }
        let rep = encode_gif(&frames, &GifOptions::default()).unwrap();
        assert!(rep.bytes.len() < 1500, "expected tiny delta gif, got {}", rep.bytes.len());
    }

    #[test]
    fn gradient_falls_back_to_quantizer() {
        let mut f = solid(64, 64, [0, 0, 0], 0.5);
        for (i, px) in f.data.chunks_exact_mut(4).enumerate() {
            px[0] = (i % 256) as u8;
            px[1] = (i / 64 % 256) as u8;
            px[2] = (i / 3 % 256) as u8;
        }
        let rep = encode_gif(&[f], &GifOptions { looping: true, max_colors: 128 }).unwrap();
        assert!(matches!(rep.palette, PaletteMode::Quantized(n) if n <= 128));
    }

    #[test]
    fn png_roundtrips_header() {
        let bytes = encode_png(4, 4, &[128u8; 64]).unwrap();
        assert_eq!(&bytes[1..4], b"PNG");
    }

    #[test]
    fn identical_frames_emit_keepalive() {
        let frames = vec![solid(10, 10, [5, 5, 5], 0.5), solid(10, 10, [5, 5, 5], 1.0)];
        let rep = encode_gif(&frames, &GifOptions::default()).unwrap();
        assert_eq!(rep.frames, 2);
    }
}
