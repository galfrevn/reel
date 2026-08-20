//! Encoders. The GIF path implements the size strategy from the spec:
//! change-driven frames (upstream), exact palette when the content fits in
//! 256 colors (terminal themes almost always do), delta rectangles, and a
//! quantizer fallback for gradient-heavy content.

pub mod webm;
mod yuv;

#[cfg(feature = "video")]
mod opus;
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

/// Incremental WebM encoder: push straight-RGBA frames as they render
/// (borrowed slices — no per-frame ownership churn), then `finish` with the
/// audio buffer. Peak memory stays at one frame regardless of length.
#[cfg(feature = "video")]
pub struct WebmEncoder {
    enc: vp9::Vp9Encoder,
    blocks: Vec<webm::Block>,
    i420: yuv::I420Frame,
    width: u32,
    height: u32,
    clock_s: f64,
    tick: usize,
    frame_idx: usize,
    cfr: Option<f64>,
    cq_level: u32,
    bitrate_kbps: u32,
}

#[cfg(feature = "video")]
impl WebmEncoder {
    pub fn new(width: u32, height: u32, opts: &WebmOptions) -> Result<Self, EncodeError> {
        let bitrate = opts
            .bitrate_kbps
            .unwrap_or_else(|| (width * height / 500).clamp(300, 4000));
        let enc = vp9::Vp9Encoder::new(width, height, &vp9::Vp9Config {
            cq_level: opts.cq_level,
            bitrate_kbps: bitrate,
            cpu_used: opts.cpu_used,
        })?;
        Ok(WebmEncoder {
            enc,
            blocks: Vec::new(),
            i420: yuv::I420Frame { width, height, y: Vec::new(), u: Vec::new(), v: Vec::new() },
            width,
            height,
            clock_s: 0.0,
            tick: 0,
            frame_idx: 0,
            cfr: opts.cfr_fps.map(|f| f.clamp(1, 120) as f64),
            cq_level: opts.cq_level,
            bitrate_kbps: bitrate,
        })
    }

    pub fn push(&mut self, rgba: &[u8], width: u32, height: u32, dur_s: f64) -> Result<(), EncodeError> {
        if width != self.width || height != self.height {
            return Err(EncodeError::DimensionMismatch(self.frame_idx));
        }
        let end = self.clock_s + dur_s;
        match self.cfr {
            Some(fps) => {
                // Convert only when at least one tick shows this frame.
                if (self.tick as f64) / fps < end - 1e-9 {
                    yuv::rgba_to_i420_into(self.width, self.height, rgba, &mut self.i420);
                    while (self.tick as f64) / fps < end - 1e-9 {
                        let pts = (self.tick as f64 * 1000.0 / fps).round() as i64;
                        let next = ((self.tick + 1) as f64 * 1000.0 / fps).round() as i64;
                        self.blocks
                            .extend(self.enc.encode(&self.i420, pts, (next - pts).max(1) as u64)?);
                        self.tick += 1;
                    }
                }
            }
            None => {
                let pts = (self.clock_s * 1000.0).round() as i64;
                let dur = ((end * 1000.0).round() as i64 - pts).max(1) as u64;
                yuv::rgba_to_i420_into(self.width, self.height, rgba, &mut self.i420);
                self.blocks.extend(self.enc.encode(&self.i420, pts, dur)?);
            }
        }
        self.clock_s = end;
        self.frame_idx += 1;
        Ok(())
    }

    pub fn finish(self, audio: Option<&[f32]>) -> Result<WebmReport, EncodeError> {
        self.finish_with_cues(audio, &[])
    }

    /// Like [`finish`](Self::finish), embedding caption cues as an in-band
    /// S_TEXT/WEBVTT subtitle track (players like mpv/VLC show them).
    pub fn finish_with_cues(
        mut self,
        audio: Option<&[f32]>,
        cues: &[webm::Cue],
    ) -> Result<WebmReport, EncodeError> {
        if self.frame_idx == 0 {
            return Err(EncodeError::NoFrames);
        }
        self.blocks.extend(self.enc.finish()?);
        let frame_count = self.blocks.len();
        let has_audio = match audio {
            Some(samples) if !samples.is_empty() => {
                self.blocks.extend(opus::encode_opus(samples)?);
                true
            }
            _ => false,
        };
        let video_track = webm::VideoTrack { width: self.width, height: self.height };
        let audio_track = webm::AudioTrack {
            channels: 1,
            sample_rate: opus::OPUS_SAMPLE_RATE,
            pre_skip: 0,
        };
        let bytes = webm::mux_with_cues(
            &video_track,
            has_audio.then_some(&audio_track),
            cues,
            &mut self.blocks,
            self.clock_s * 1000.0,
        );
        Ok(WebmReport {
            bytes,
            frames: frame_count,
            has_audio,
            cq_level: self.cq_level,
            bitrate_kbps: self.bitrate_kbps,
        })
    }
}

/// Streaming WebM encode over owned frames — a thin wrapper over
/// [`WebmEncoder`].
#[cfg(feature = "video")]
pub fn encode_webm_stream<I>(
    frames: I,
    audio: Option<&[f32]>,
    opts: &WebmOptions,
) -> Result<WebmReport, EncodeError>
where
    I: IntoIterator<Item = RgbaFrame>,
{
    let mut iter = frames.into_iter();
    let first = iter.next().ok_or(EncodeError::NoFrames)?;
    let mut enc = WebmEncoder::new(first.width, first.height, opts)?;
    enc.push(&first.data, first.width, first.height, first.duration_s)?;
    for f in iter {
        enc.push(&f.data, f.width, f.height, f.duration_s)?;
    }
    enc.finish(audio)
}

/// Collected-frames convenience wrapper over the streaming encoder.
#[cfg(feature = "video")]
pub fn encode_webm(
    frames: &[RgbaFrame],
    audio: Option<&[f32]>,
    opts: &WebmOptions,
) -> Result<WebmReport, EncodeError> {
    encode_webm_stream(frames.iter().cloned(), audio, opts)
}

/// Stubs so callers get a clear runtime error instead of a compile break
/// when the `video` feature is off.
#[cfg(not(feature = "video"))]
pub fn encode_webm_stream<I>(
    _frames: I,
    _audio: Option<&[f32]>,
    _opts: &WebmOptions,
) -> Result<WebmReport, EncodeError>
where
    I: IntoIterator<Item = RgbaFrame>,
{
    Err(EncodeError::VideoDisabled)
}

#[cfg(not(feature = "video"))]
pub fn encode_webm(
    _frames: &[RgbaFrame],
    _audio: Option<&[f32]>,
    _opts: &WebmOptions,
) -> Result<WebmReport, EncodeError> {
    Err(EncodeError::VideoDisabled)
}

#[cfg(not(feature = "video"))]
pub struct WebmEncoder {}

#[cfg(not(feature = "video"))]
impl WebmEncoder {
    pub fn new(_width: u32, _height: u32, _opts: &WebmOptions) -> Result<Self, EncodeError> {
        Err(EncodeError::VideoDisabled)
    }

    pub fn push(
        &mut self,
        _rgba: &[u8],
        _width: u32,
        _height: u32,
        _dur_s: f64,
    ) -> Result<(), EncodeError> {
        Err(EncodeError::VideoDisabled)
    }

    pub fn finish(self, _audio: Option<&[f32]>) -> Result<WebmReport, EncodeError> {
        Err(EncodeError::VideoDisabled)
    }

    pub fn finish_with_cues(
        self,
        _audio: Option<&[f32]>,
        _cues: &[webm::Cue],
    ) -> Result<WebmReport, EncodeError> {
        Err(EncodeError::VideoDisabled)
    }
}

/// One output frame: straight (non-premultiplied) RGBA and a display duration.
#[derive(Clone)]
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

// ---------------------------------------------------------------------------
// GIF: streaming two-pass API (palette scan, then write) so callers never
// hold every RGBA frame in memory — at 1080p that was gigabytes.
// ---------------------------------------------------------------------------

/// Pass 1: feed every frame's pixels to decide the palette.
pub struct GifPaletteBuilder {
    max_colors: u16,
    colors: HashMap<[u8; 3], u8>,
    exact: bool,
    /// RGBA samples for NeuQuant, kept bounded by doubling the stride.
    samples: Vec<u8>,
    stride: usize,
    phase: usize,
}

const MAX_QUANT_SAMPLES: usize = 200_000;

impl GifPaletteBuilder {
    pub fn new(max_colors: u16) -> Self {
        GifPaletteBuilder {
            max_colors: max_colors.clamp(2, 256),
            colors: HashMap::new(),
            exact: true,
            samples: Vec::new(),
            stride: 17, // co-prime with row lengths, avoids column bias
            phase: 0,
        }
    }

    pub fn feed(&mut self, rgba: &[u8]) {
        for px in rgba.chunks_exact(4) {
            if self.exact {
                let key = [px[0], px[1], px[2]];
                if !self.colors.contains_key(&key) {
                    if self.colors.len() >= self.max_colors as usize {
                        self.exact = false;
                    } else {
                        let next = self.colors.len() as u8;
                        self.colors.insert(key, next);
                    }
                }
            }
            // Sample regardless: exactness can fall over in a later frame.
            if self.phase == 0 {
                self.samples.extend_from_slice(&[px[0], px[1], px[2], 255]);
                if self.samples.len() / 4 > MAX_QUANT_SAMPLES {
                    // Keep every other sample and halve the intake rate.
                    let mut keep = Vec::with_capacity(self.samples.len() / 2);
                    for pair in self.samples.chunks_exact(8) {
                        keep.extend_from_slice(&pair[..4]);
                    }
                    self.samples = keep;
                    self.stride *= 2;
                }
            }
            self.phase = (self.phase + 1) % self.stride;
        }
    }

    pub fn finish(self) -> GifPalette {
        if self.exact && !self.colors.is_empty() {
            let mut rgb = vec![0u8; self.colors.len() * 3];
            for (color, idx) in &self.colors {
                rgb[*idx as usize * 3..*idx as usize * 3 + 3].copy_from_slice(color);
            }
            let n = self.colors.len() as u16;
            GifPalette { rgb, mode: PaletteMode::Exact(n), lookup: Lookup::Exact(self.colors) }
        } else {
            let nq = color_quant::NeuQuant::new(10, self.max_colors as usize, &self.samples);
            let rgb = nq.color_map_rgb();
            let n = (rgb.len() / 3) as u16;
            GifPalette {
                rgb,
                mode: PaletteMode::Quantized(n),
                // index_of is a search; terminal frames repeat a few
                // thousand distinct RGBs, so memoizing makes it ~free.
                lookup: Lookup::Quant { nq, memo: HashMap::new() },
            }
        }
    }
}

pub struct GifPalette {
    rgb: Vec<u8>,
    mode: PaletteMode,
    lookup: Lookup,
}

enum Lookup {
    Exact(HashMap<[u8; 3], u8>),
    Quant { nq: color_quant::NeuQuant, memo: HashMap<[u8; 3], u8> },
}

impl GifPalette {
    fn index(&mut self, px: &[u8]) -> u8 {
        match &mut self.lookup {
            Lookup::Exact(map) => *map.get(&[px[0], px[1], px[2]]).unwrap_or(&0),
            Lookup::Quant { nq, memo } => *memo
                .entry([px[0], px[1], px[2]])
                .or_insert_with(|| nq.index_of(&[px[0], px[1], px[2], 255]) as u8),
        }
    }
}

/// Pass 2: push frames as they render; only the previous indexed frame
/// (1 byte/px) stays in memory for the delta rectangles.
pub struct GifStream<W: std::io::Write> {
    enc: gif::Encoder<W>,
    palette: GifPalette,
    prev: Option<Vec<u8>>,
    carry: f64,
    width: u32,
    height: u32,
    frames: usize,
}

impl<W: std::io::Write> GifStream<W> {
    pub fn new(
        out: W,
        width: u32,
        height: u32,
        palette: GifPalette,
        looping: bool,
    ) -> Result<Self, EncodeError> {
        let mut enc = gif::Encoder::new(out, width as u16, height as u16, &palette.rgb)?;
        enc.set_repeat(if looping { gif::Repeat::Infinite } else { gif::Repeat::Finite(0) })?;
        Ok(GifStream { enc, palette, prev: None, carry: 0.0, width, height, frames: 0 })
    }

    pub fn push(&mut self, rgba: &[u8], dur_s: f64) -> Result<(), EncodeError> {
        // GIF delays are centiseconds; carry the rounding error so long
        // videos don't drift.
        let exact_cs = dur_s * 100.0 + self.carry;
        let cs = exact_cs.round().max(2.0); // <2cs renders erratically in browsers
        self.carry = exact_cs - cs;
        let delay = cs as u16;

        let idx: Vec<u8> = rgba.chunks_exact(4).map(|px| self.palette.index(px)).collect();
        let (w, h) = (self.width as usize, self.height as usize);
        let mut frame = match &self.prev {
            None => gif::Frame {
                width: w as u16,
                height: h as u16,
                buffer: Cow::Borrowed(idx.as_slice()),
                ..Default::default()
            },
            Some(prev) => match diff_rect(prev, &idx, w, h) {
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
                        let row = y * w;
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
            },
        };
        frame.delay = delay;
        frame.dispose = gif::DisposalMethod::Keep;
        self.enc.write_frame(&frame)?;
        self.prev = Some(idx);
        self.frames += 1;
        Ok(())
    }

    /// Finalizes the stream; returns (frame count, palette mode).
    pub fn finish(self) -> Result<(usize, PaletteMode), EncodeError> {
        let mode = self.palette.mode.clone();
        Ok((self.frames, mode)) // encoder writes the trailer on drop
    }
}

/// Collected-frames convenience wrapper over the streaming API.
pub fn encode_gif(frames: &[RgbaFrame], opts: &GifOptions) -> Result<GifReport, EncodeError> {
    let first = frames.first().ok_or(EncodeError::NoFrames)?;
    let (w, h) = (first.width, first.height);
    for (i, f) in frames.iter().enumerate() {
        if f.width != w || f.height != h {
            return Err(EncodeError::DimensionMismatch(i));
        }
    }
    let mut builder = GifPaletteBuilder::new(opts.max_colors);
    for f in frames {
        builder.feed(&f.data);
    }
    let mut bytes = Vec::new();
    let mut stream = GifStream::new(&mut bytes, w, h, builder.finish(), opts.looping)?;
    for f in frames {
        stream.push(&f.data, f.duration_s)?;
    }
    let (n, mode) = stream.finish()?;
    Ok(GifReport { bytes, palette: mode, frames: n })
}


/// Bounding box (x0, y0, x1, y1) of differing pixels, or None if identical.
fn diff_rect(a: &[u8], b: &[u8], w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
    diff_rect_bpp(a, b, w, h, 1)
}

/// Same, for `bpp`-byte pixels (4 for RGBA).
fn diff_rect_bpp(
    a: &[u8],
    b: &[u8],
    w: usize,
    h: usize,
    bpp: usize,
) -> Option<(usize, usize, usize, usize)> {
    let mut x0 = w;
    let mut y0 = h;
    let mut x1 = 0usize;
    let mut y1 = 0usize;
    for y in 0..h {
        let row = y * w * bpp;
        let ra = &a[row..row + w * bpp];
        let rb = &b[row..row + w * bpp];
        if ra == rb {
            continue;
        }
        let first = ra.iter().zip(rb).position(|(p, q)| p != q).unwrap() / bpp;
        let last = (ra.len() - 1 - ra.iter().zip(rb).rev().position(|(p, q)| p != q).unwrap()) / bpp;
        x0 = x0.min(first);
        x1 = x1.max(last);
        y0 = y0.min(y);
        y1 = y1.max(y);
    }
    (y1 >= y0 && x1 >= x0).then_some((x0, y0, x1, y1))
}

// ---------------------------------------------------------------------------
// APNG: animated PNG, streamed frame by frame with delta rectangles.
// Lossless truecolor — the right choice where GIF's 256 colors pinch
// (gradients, glow) and WebM isn't embeddable.
// ---------------------------------------------------------------------------

pub struct ApngStream<W: std::io::Write> {
    writer: png::Writer<W>,
    prev: Option<Vec<u8>>,
    width: u32,
    height: u32,
    frames: usize,
    expected: u32,
}

impl<W: std::io::Write> ApngStream<W> {
    /// `frames` must be the exact number of frames that will be pushed.
    pub fn new(
        out: W,
        width: u32,
        height: u32,
        frames: u32,
        looping: bool,
    ) -> Result<Self, EncodeError> {
        let mut enc = png::Encoder::new(out, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_animated(frames, if looping { 0 } else { 1 })?;
        let writer = enc.write_header()?;
        Ok(ApngStream { writer, prev: None, width, height, frames: 0, expected: frames })
    }

    pub fn push(&mut self, rgba: &[u8], dur_s: f64) -> Result<(), EncodeError> {
        let ms = (dur_s * 1000.0).round().clamp(1.0, 65_535.0) as u16;
        self.writer.set_frame_delay(ms, 1000)?;
        self.writer.set_dispose_op(png::DisposeOp::None)?;
        self.writer.set_blend_op(png::BlendOp::Source)?;
        let (w, h) = (self.width as usize, self.height as usize);
        match &self.prev {
            None => {
                // First frame doubles as the PNG's default image; the fcTL
                // is implicit and must stay full-size.
                self.writer.write_image_data(rgba)?;
            }
            Some(prev) => {
                let rect = diff_rect_bpp(prev, rgba, w, h, 4)
                    // Identical frame: repaint one pixel to carry the delay.
                    .unwrap_or((0, 0, 0, 0));
                let (x0, y0, x1, y1) = rect;
                let (rw, rh) = (x1 - x0 + 1, y1 - y0 + 1);
                let mut buf = Vec::with_capacity(rw * rh * 4);
                for y in y0..=y1 {
                    let row = (y * w + x0) * 4;
                    buf.extend_from_slice(&rgba[row..row + rw * 4]);
                }
                // Order matters: reset clears the previous frame's rect so
                // the bounds checks see a clean slate.
                self.writer.reset_frame_position()?;
                self.writer.set_frame_dimension(rw as u32, rh as u32)?;
                self.writer.set_frame_position(x0 as u32, y0 as u32)?;
                self.writer.write_image_data(&buf)?;
            }
        }
        self.prev = Some(rgba.to_vec());
        self.frames += 1;
        Ok(())
    }

    pub fn finish(self) -> Result<usize, EncodeError> {
        debug_assert_eq!(self.frames as u32, self.expected, "frame count promise broken");
        self.writer.finish()?;
        Ok(self.frames)
    }
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
    fn apng_streams_delta_frames() {
        let mut bytes = Vec::new();
        {
            let mut s = ApngStream::new(&mut bytes, 20, 10, 3, true).unwrap();
            let f0 = solid(20, 10, [0, 0, 0], 0.5);
            let mut f1 = f0.data.clone();
            f1[(3 * 20 + 5) * 4] = 200; // one pixel changes
            s.push(&f0.data, 0.5).unwrap();
            s.push(&f1, 0.5).unwrap();
            s.push(&f1, 0.5).unwrap(); // identical: keep-alive
            assert_eq!(s.finish().unwrap(), 3);
        }
        assert_eq!(&bytes[1..4], b"PNG");
        // acTL chunk marks it animated.
        assert!(bytes.windows(4).any(|w| w == b"acTL"));
        // Delta frame stays tiny: whole file well under full 3-frame size.
        assert!(bytes.len() < 1200, "apng too big: {}", bytes.len());
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
