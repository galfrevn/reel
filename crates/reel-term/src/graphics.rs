//! Experimental in-terminal graphics: sixel and the kitty graphics
//! protocol. TUIs that preview images (yazi, ranger, chafa pipelines…)
//! finally render as images instead of blank cells.
//!
//! v0 semantics, documented and deliberate: an image anchors to the cell
//! the cursor was at when it arrived and stays until the screen clears
//! (ED 2/3, RIS, alt-screen switch) or the same anchor is redrawn.
//! Scroll-tracking is out of scope — full-screen TUIs reposition with
//! absolute cursor moves, which this handles fine.

use std::sync::Arc;

/// Cell size (px) assumed for sixel placement — recordings don't carry the
/// original terminal's pixel metrics, so image extents in *cells* use this.
pub const SIXEL_CELL: (f32, f32) = (10.0, 20.0);
/// Safety cap for decoded images.
const MAX_DIM: u32 = 2048;

/// Cap on buffered bytes while waiting for a sequence terminator. Comfortably
/// above the largest legal payload (MAX_DIM² RGBA, base64) so it only fires
/// on input that will never terminate.
const MAX_PENDING: usize = 32 << 20;

#[derive(Debug, Clone)]
pub struct PlacedImage {
    /// Anchor cell (the cursor position when the image arrived).
    pub col: u16,
    pub row: u16,
    /// Footprint in cells.
    pub cols: u16,
    pub rows: u16,
    /// Straight RGBA pixels.
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

/// Streaming extractor: splits graphics sequences out of the output stream,
/// forwarding everything else to the terminal parser via `text`.
#[derive(Default)]
pub struct GraphicsState {
    pending: Vec<u8>,
    /// Kitty chunked transmissions in flight (m=1 … m=0).
    kitty_acc: Option<(KittyMeta, Vec<u8>)>,
    pub images: Vec<PlacedImage>,
}

#[derive(Debug, Clone, Default)]
struct KittyMeta {
    action: char,
    format: u32,
    width: u32,
    height: u32,
    cols: u32,
    rows: u32,
}

pub enum Piece {
    Text(Vec<u8>),
    Image { rgba: Vec<u8>, width: u32, height: u32, cols: Option<u16>, rows: Option<u16> },
    ClearImages,
}

impl GraphicsState {
    /// Consumes a chunk of program output, yielding text pieces (to feed the
    /// emulator) interleaved with decoded images in arrival order.
    pub fn process(&mut self, bytes: &[u8]) -> Vec<Piece> {
        self.pending.extend_from_slice(bytes);
        let mut pieces = Vec::new();
        loop {
            // Find the earliest graphics introducer in the pending buffer.
            let dcs = find_sub(&self.pending, b"\x1bP");
            let apc = find_sub(&self.pending, b"\x1b_G");
            let (start, kind) = match (dcs, apc) {
                (Some(d), Some(a)) if d < a => (d, Kind::Dcs),
                (Some(_), Some(a)) => (a, Kind::Apc),
                (Some(d), None) => (d, Kind::Dcs),
                (None, Some(a)) => (a, Kind::Apc),
                (None, None) => break,
            };
            // Everything before it is plain text.
            if start > 0 {
                pieces.push(Piece::Text(self.pending[..start].to_vec()));
            }
            // Need the terminator before this sequence can be handled.
            let Some(end) = find_st(&self.pending, start) else {
                self.pending.drain(..start);
                // A sequence that never terminates (truncated recording, a
                // bare ESC P probe) must not buffer the rest of the session
                // behind it: past any plausible image payload, drop it so
                // later output flows again instead of silently freezing.
                if self.pending.len() > MAX_PENDING {
                    self.pending.clear();
                }
                self.flush_text_clears(&pieces);
                return pieces;
            };
            let seq = self.pending[start..end.0].to_vec();
            self.pending.drain(..end.1);
            match kind {
                Kind::Dcs => {
                    if let Some((rgba, w, h)) = decode_sixel_sequence(&seq) {
                        pieces.push(Piece::Image { rgba, width: w, height: h, cols: None, rows: None });
                    }
                    // Non-sixel DCS (queries etc.) vanish — alacritty would
                    // ignore them anyway.
                }
                Kind::Apc => {
                    if let Some(piece) = self.handle_kitty(&seq[3..]) {
                        pieces.push(piece);
                    }
                }
            }
        }
        if !self.pending.is_empty() {
            // No introducer at all: flush as text, keeping a possible
            // partial ESC at the very end.
            let keep = partial_intro_len(&self.pending);
            let cut = self.pending.len() - keep;
            if cut > 0 {
                pieces.push(Piece::Text(self.pending[..cut].to_vec()));
                self.pending.drain(..cut);
            }
        }
        self.flush_text_clears(&pieces);
        pieces
    }

    fn flush_text_clears(&mut self, _pieces: &[Piece]) {}

    fn handle_kitty(&mut self, payload: &[u8]) -> Option<Piece> {
        let semi = payload.iter().position(|&b| b == b';').unwrap_or(payload.len());
        let controls = std::str::from_utf8(&payload[..semi]).ok()?;
        let data = payload.get(semi + 1..).unwrap_or(&[]);
        let mut meta = KittyMeta { action: 't', format: 32, ..Default::default() };
        let mut more = 0u32;
        let mut quiet_delete = false;
        for kv in controls.split(',') {
            let Some((k, v)) = kv.split_once('=') else { continue };
            match k {
                "a" => {
                    meta.action = v.chars().next().unwrap_or('t');
                    if meta.action == 'd' {
                        quiet_delete = true;
                    }
                }
                "f" => meta.format = v.parse().unwrap_or(32),
                "s" => meta.width = v.parse().unwrap_or(0),
                "v" => meta.height = v.parse().unwrap_or(0),
                "c" => meta.cols = v.parse().unwrap_or(0),
                "r" => meta.rows = v.parse().unwrap_or(0),
                "m" => more = v.parse().unwrap_or(0),
                _ => {}
            }
        }
        if quiet_delete {
            self.kitty_acc = None;
            return Some(Piece::ClearImages);
        }
        // Accumulate chunked payloads.
        let (meta, b64) = match self.kitty_acc.take() {
            Some((m, mut acc)) => {
                acc.extend_from_slice(data);
                (m, acc)
            }
            None => (meta, data.to_vec()),
        };
        if more == 1 {
            self.kitty_acc = Some((meta, b64));
            return None;
        }
        if !matches!(meta.action, 't' | 'T') {
            return None;
        }
        let raw = base64_decode(&b64)?;
        let (rgba, w, h) = match meta.format {
            100 => decode_png(&raw)?,
            32 => (raw, meta.width, meta.height),
            24 => {
                let mut rgba = Vec::with_capacity(raw.len() / 3 * 4);
                for px in raw.as_chunks::<3>().0 {
                    rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
                (rgba, meta.width, meta.height)
            }
            _ => return None,
        };
        if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM || rgba.len() < (w * h * 4) as usize {
            return None;
        }
        Some(Piece::Image {
            rgba,
            width: w,
            height: h,
            cols: (meta.cols > 0).then_some(meta.cols as u16),
            rows: (meta.rows > 0).then_some(meta.rows as u16),
        })
    }
}

enum Kind {
    Dcs,
    Apc,
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Finds the string terminator; returns (payload_end, consumed_end).
fn find_st(buf: &[u8], from: usize) -> Option<(usize, usize)> {
    for i in from..buf.len() {
        if buf[i] == 0x9c {
            return Some((i, i + 1));
        }
        if buf[i] == 0x1b && buf.get(i + 1) == Some(&b'\\') && i > from {
            return Some((i, i + 2));
        }
    }
    None
}

/// Bytes at the tail that could be the start of a graphics introducer.
fn partial_intro_len(buf: &[u8]) -> usize {
    for keep in (1..=2.min(buf.len())).rev() {
        let tail = &buf[buf.len() - keep..];
        if b"\x1bP".starts_with(tail) || b"\x1b_G".starts_with(tail) {
            return keep;
        }
    }
    0
}

// ---------------------------------------------------------------------------
// Sixel
// ---------------------------------------------------------------------------

/// Decodes a full DCS sixel sequence (starting at ESC P) to RGBA.
fn decode_sixel_sequence(seq: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    // ESC P <params> q <data>
    let q = seq.iter().position(|&b| b == b'q')?;
    let data = &seq[q + 1..];

    // VT340's default registers, so undefined colors aren't all black.
    let mut palette: Vec<[u8; 3]> = vec![
        [0, 0, 0], [51, 51, 204], [204, 36, 36], [51, 204, 51],
        [204, 51, 204], [51, 204, 204], [204, 204, 51], [135, 135, 135],
        [66, 66, 66], [84, 84, 153], [153, 66, 66], [84, 153, 84],
        [153, 84, 153], [84, 153, 153], [153, 153, 84], [204, 204, 204],
    ];
    palette.resize(256, [0, 0, 0]);

    let mut grid: Vec<Vec<u8>> = Vec::new(); // color index per pixel, 255 = transparent marker? use Option via u16
    let mut width = 0usize;
    let (mut x, mut y) = (0usize, 0usize);
    let mut color = 0usize;
    let mut repeat: Option<usize> = None;
    let mut i = 0usize;

    let set = |grid: &mut Vec<Vec<u8>>, x: usize, y: usize, c: usize, width: &mut usize| {
        if x >= MAX_DIM as usize || y >= MAX_DIM as usize {
            return;
        }
        while grid.len() <= y {
            grid.push(Vec::new());
        }
        let row = &mut grid[y];
        if row.len() <= x {
            row.resize(x + 1, 255);
        }
        row[x] = c as u8;
        *width = (*width).max(x + 1);
    };

    while i < data.len() {
        let b = data[i];
        match b {
            b'"' => {
                // Raster attributes: skip params.
                i += 1;
                while i < data.len() && (data[i].is_ascii_digit() || data[i] == b';') {
                    i += 1;
                }
                continue;
            }
            b'#' => {
                i += 1;
                let mut params: Vec<u32> = Vec::new();
                let mut cur: Option<u32> = None;
                while i < data.len() && (data[i].is_ascii_digit() || data[i] == b';') {
                    if data[i] == b';' {
                        params.push(cur.take().unwrap_or(0));
                    } else {
                        cur = Some(cur.unwrap_or(0) * 10 + (data[i] - b'0') as u32);
                    }
                    i += 1;
                }
                if let Some(c) = cur {
                    params.push(c);
                }
                match params.as_slice() {
                    [reg] => color = *reg as usize % 256,
                    [reg, 2, r, g, b] => {
                        color = *reg as usize % 256;
                        palette[color] = [
                            (*r * 255 / 100).min(255) as u8,
                            (*g * 255 / 100).min(255) as u8,
                            (*b * 255 / 100).min(255) as u8,
                        ];
                    }
                    [reg, 1, h, l, s] => {
                        color = *reg as usize % 256;
                        palette[color] = hls_to_rgb(*h as f32, *l as f32 / 100.0, *s as f32 / 100.0);
                    }
                    _ => {}
                }
                continue;
            }
            b'!' => {
                i += 1;
                let mut n = 0usize;
                while i < data.len() && data[i].is_ascii_digit() {
                    n = n * 10 + (data[i] - b'0') as usize;
                    i += 1;
                }
                repeat = Some(n.max(1).min(MAX_DIM as usize));
                continue;
            }
            b'$' => {
                x = 0;
            }
            b'-' => {
                x = 0;
                y += 6;
            }
            0x3f..=0x7e => {
                let bits = b - 0x3f;
                let n = repeat.take().unwrap_or(1);
                for _ in 0..n {
                    for dy in 0..6 {
                        if bits & (1 << dy) != 0 {
                            set(&mut grid, x, y + dy, color, &mut width);
                        }
                    }
                    x += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }

    let height = grid.len();
    if width == 0 || height == 0 {
        return None;
    }
    let mut rgba = vec![0u8; width * height * 4];
    for (yy, row) in grid.iter().enumerate() {
        for (xx, &c) in row.iter().enumerate() {
            if c == 255 {
                continue; // transparent
            }
            let [r, g, b] = palette[c as usize];
            let o = (yy * width + xx) * 4;
            rgba[o..o + 4].copy_from_slice(&[r, g, b, 255]);
        }
    }
    Some((rgba, width as u32, height as u32))
}

fn hls_to_rgb(h: f32, l: f32, s: f32) -> [u8; 3] {
    // Sixel hue 0 = blue; rotate to the usual wheel.
    let h = (h + 240.0) % 360.0 / 360.0;
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round().clamp(0.0, 255.0) as u8
    };
    [f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0)]
}

// ---------------------------------------------------------------------------
// Small self-contained decoders (no new dependencies)
// ---------------------------------------------------------------------------

fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    };
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for &c in input {
        if c == b'=' || c == b'\n' || c == b'\r' {
            continue;
        }
        acc = (acc << 6) | val(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

fn decode_png(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity(buf.len() / 3 * 4);
            for px in buf.as_chunks::<3>().0 {
                out.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            out
        }
        png::ColorType::Grayscale => {
            let mut out = Vec::with_capacity(buf.len() * 4);
            for &v in &buf {
                out.extend_from_slice(&[v, v, v, 255]);
            }
            out
        }
        _ => return None,
    };
    Some((rgba, w, h))
}

/// Does this text chunk clear the screen (and therefore any images)?
pub fn clears_screen(text: &[u8]) -> bool {
    [
        b"\x1b[2J".as_slice(),
        b"\x1b[3J".as_slice(),
        b"\x1bc".as_slice(),
        b"\x1b[?1049h".as_slice(),
        b"\x1b[?1049l".as_slice(),
        b"\x1b[?47h".as_slice(),
        b"\x1b[?47l".as_slice(),
    ]
    .iter()
    .any(|seq| find_sub(text, seq).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4px-wide, 6px-tall, two colors: left half red, right half blue.
    fn tiny_sixel() -> Vec<u8> {
        let mut s = Vec::new();
        s.extend_from_slice(b"\x1bPq");
        s.extend_from_slice(b"#1;2;100;0;0"); // register 1 = red
        s.extend_from_slice(b"#2;2;0;0;100"); // register 2 = blue
        s.extend_from_slice(b"#1~~#2~~"); // ~ = all 6 bits set
        s.extend_from_slice(b"\x1b\\");
        s
    }

    #[test]
    fn sixel_decodes_colors_and_dimensions() {
        let seq = tiny_sixel();
        let (rgba, w, h) = decode_sixel_sequence(&seq[..seq.len() - 2]).unwrap();
        assert_eq!((w, h), (4, 6));
        assert_eq!(&rgba[0..4], &[255, 0, 0, 255]); // left: red
        let right = 2 * 4; // row 0, col 2
        assert_eq!(&rgba[right..right + 4], &[0, 0, 255, 255]); // right: blue
    }

    #[test]
    fn sixel_repeat_and_newline() {
        let mut s = Vec::new();
        s.extend_from_slice(b"\x1bPq#1;2;0;100;0!8~-!8~");
        let (rgba, w, h) = decode_sixel_sequence(&s).unwrap();
        assert_eq!((w, h), (8, 12));
        assert_eq!(&rgba[..4], &[0, 255, 0, 255]);
        let second_band = ((6 * w) * 4) as usize;
        assert_eq!(&rgba[second_band..second_band + 4], &[0, 255, 0, 255]);
    }

    #[test]
    fn unterminated_sequence_does_not_swallow_the_session_forever() {
        let mut gs = GraphicsState::default();
        // A bare DCS introducer with no terminator, then a session's worth
        // of output. Once past MAX_PENDING the buffer must reset so later
        // text reaches the emulator again.
        gs.process(b"\x1bPq#1;2;100;0;0");
        let chunk = vec![b'x'; 1 << 20];
        for _ in 0..(MAX_PENDING >> 20) + 1 {
            gs.process(&chunk);
        }
        assert!(gs.pending.len() <= MAX_PENDING, "pending must be bounded");
        let pieces = gs.process(b"hello after the bad sequence");
        assert!(
            pieces.iter().any(|p| matches!(p, Piece::Text(t) if !t.is_empty())),
            "text must flow again after the cap fires"
        );
    }

    #[test]
    fn extractor_splits_text_and_images_across_chunks() {
        let mut gs = GraphicsState::default();
        let seq = tiny_sixel();
        let mut stream = b"before ".to_vec();
        stream.extend_from_slice(&seq);
        stream.extend_from_slice(b" after");
        // Feed in awkward 5-byte chunks.
        let mut texts = Vec::new();
        let mut images = 0;
        for chunk in stream.chunks(5) {
            for piece in gs.process(chunk) {
                match piece {
                    Piece::Text(t) => texts.extend_from_slice(&t),
                    Piece::Image { width, height, .. } => {
                        images += 1;
                        assert_eq!((width, height), (4, 6));
                    }
                    Piece::ClearImages => {}
                }
            }
        }
        assert_eq!(images, 1);
        assert_eq!(String::from_utf8_lossy(&texts), "before  after");
    }

    #[test]
    fn kitty_png_transmit_places_an_image() {
        // Encode a 3x2 PNG in-process, then wrap it in the kitty APC.
        let mut png_bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png_bytes, 3, 2);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[10u8; 24]).unwrap();
        }
        let b64 = base64_encode_for_test(&png_bytes);
        let mut apc = Vec::new();
        apc.extend_from_slice(b"\x1b_Ga=T,f=100,c=2,r=1;");
        apc.extend_from_slice(b64.as_bytes());
        apc.extend_from_slice(b"\x1b\\");

        let mut gs = GraphicsState::default();
        let pieces = gs.process(&apc);
        let found = pieces.iter().any(|p| matches!(
            p,
            Piece::Image { width: 3, height: 2, cols: Some(2), rows: Some(1), .. }
        ));
        assert!(found);
    }

    #[test]
    fn clear_sequences_detected() {
        assert!(clears_screen(b"junk\x1b[2Jmore"));
        assert!(clears_screen(b"\x1b[?1049h"));
        assert!(!clears_screen(b"plain text \x1b[31m"));
    }

    fn base64_encode_for_test(data: &[u8]) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            out.push(T[(n >> 18) as usize & 63] as char);
            out.push(T[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
            out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
        }
        out
    }
}
