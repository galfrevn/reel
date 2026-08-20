//! A minimal WebM (Matroska/EBML) muxer: one VP9 video track, one optional
//! Opus audio track, SimpleBlocks in timestamp-interleaved clusters.
//!
//! Hand-rolled on purpose — the subset WebM needs is small, and writing it
//! ourselves keeps the output byte-deterministic and the build free of C++.
//! Element IDs and structure follow RFC 8794 (EBML) and the WebM spec.

/// Timestamps are milliseconds throughout (TimestampScale = 1_000_000 ns).
const TIMESTAMP_SCALE: u64 = 1_000_000;
/// Start a new cluster at least this often; relative block timestamps are
/// i16 ms, so clusters must stay well under 32s.
const CLUSTER_MAX_MS: i64 = 10_000;
/// WebM's required SeekPreRoll for Opus: 80ms in nanoseconds.
const OPUS_SEEK_PREROLL_NS: u64 = 80_000_000;

// -- element ids (as written, including the class bits) ---------------------
const EBML: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const INFO: u32 = 0x1549_A966;
const TRACKS: u32 = 0x1654_AE6B;
const CLUSTER: u32 = 0x1F43_B675;

/// One encoded frame ready for muxing.
pub struct Block {
    /// Presentation time in ms.
    pub pts_ms: i64,
    /// 1-based track: 1 = video, 2 = audio, 3 = subtitles.
    pub track: u8,
    pub keyframe: bool,
    pub data: Vec<u8>,
    /// Display duration in ms; subtitle blocks need one (BlockGroup).
    pub duration_ms: Option<u64>,
}

pub struct VideoTrack {
    pub width: u32,
    pub height: u32,
}

pub struct AudioTrack {
    pub channels: u8,
    pub sample_rate: u32,
    /// Samples the decoder must drop at the start (0 for reel's encoder).
    pub pre_skip: u16,
}

/// A subtitle cue for the optional text track.
pub struct Cue {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

fn write_id(out: &mut Vec<u8>, id: u32) {
    let bytes = id.to_be_bytes();
    let skip = bytes.iter().position(|&b| b != 0).unwrap_or(3);
    out.extend_from_slice(&bytes[skip..]);
}

/// EBML variable-width size. Always ≥1 byte; picks the shortest encoding.
fn write_size(out: &mut Vec<u8>, size: u64) {
    for width in 1..=8u32 {
        // Each width reserves its top bit pattern; all-ones is "unknown size".
        let max = (1u64 << (7 * width)) - 2;
        if size <= max {
            let marker = 1u64 << (7 * width);
            let v = marker | size;
            let bytes = v.to_be_bytes();
            out.extend_from_slice(&bytes[8 - width as usize..]);
            return;
        }
    }
    unreachable!("size exceeds EBML bounds");
}

fn element(out: &mut Vec<u8>, id: u32, body: &[u8]) {
    write_id(out, id);
    write_size(out, body.len() as u64);
    out.extend_from_slice(body);
}

fn uint_body(mut v: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        bytes.insert(0, (v & 0xFF) as u8);
        v >>= 8;
        if v == 0 {
            break;
        }
    }
    bytes
}

fn el_uint(out: &mut Vec<u8>, id: u32, v: u64) {
    element(out, id, &uint_body(v));
}

fn el_float(out: &mut Vec<u8>, id: u32, v: f64) {
    element(out, id, &v.to_be_bytes());
}

fn el_string(out: &mut Vec<u8>, id: u32, s: &str) {
    element(out, id, s.as_bytes());
}

fn ebml_header() -> Vec<u8> {
    let mut body = Vec::new();
    el_uint(&mut body, 0x4286, 1); // EBMLVersion
    el_uint(&mut body, 0x42F7, 1); // EBMLReadVersion
    el_uint(&mut body, 0x42F2, 4); // EBMLMaxIDLength
    el_uint(&mut body, 0x42F3, 8); // EBMLMaxSizeLength
    el_string(&mut body, 0x4282, "webm"); // DocType
    el_uint(&mut body, 0x4287, 4); // DocTypeVersion
    el_uint(&mut body, 0x4285, 2); // DocTypeReadVersion
    let mut out = Vec::new();
    element(&mut out, EBML, &body);
    out
}

/// The 19-byte OpusHead structure WebM carries as CodecPrivate.
fn opus_head(a: &AudioTrack) -> Vec<u8> {
    let mut h = Vec::with_capacity(19);
    h.extend_from_slice(b"OpusHead");
    h.push(1); // version
    h.push(a.channels);
    h.extend_from_slice(&a.pre_skip.to_le_bytes());
    h.extend_from_slice(&a.sample_rate.to_le_bytes());
    h.extend_from_slice(&0i16.to_le_bytes()); // output gain
    h.push(0); // mapping family
    h
}

fn tracks(video: &VideoTrack, audio: Option<&AudioTrack>, subtitles: bool) -> Vec<u8> {
    let mut body = Vec::new();
    {
        let mut t = Vec::new();
        el_uint(&mut t, 0xD7, 1); // TrackNumber
        el_uint(&mut t, 0x73C5, 1); // TrackUID (deterministic on purpose)
        el_uint(&mut t, 0x83, 1); // TrackType: video
        el_uint(&mut t, 0x9C, 0); // FlagLacing
        el_string(&mut t, 0x86, "V_VP9"); // CodecID
        let mut v = Vec::new();
        el_uint(&mut v, 0xB0, video.width as u64); // PixelWidth
        el_uint(&mut v, 0xBA, video.height as u64); // PixelHeight
        element(&mut t, 0xE0, &v); // Video
        element(&mut body, 0xAE, &t); // TrackEntry
    }
    if subtitles {
        let mut t = Vec::new();
        el_uint(&mut t, 0xD7, 3);
        el_uint(&mut t, 0x73C5, 3);
        el_uint(&mut t, 0x83, 0x11); // TrackType: subtitle
        el_uint(&mut t, 0x9C, 0);
        el_string(&mut t, 0x86, "S_TEXT/WEBVTT");
        element(&mut body, 0xAE, &t);
    }
    if let Some(a) = audio {
        let mut t = Vec::new();
        el_uint(&mut t, 0xD7, 2);
        el_uint(&mut t, 0x73C5, 2);
        el_uint(&mut t, 0x83, 2); // TrackType: audio
        el_uint(&mut t, 0x9C, 0);
        el_string(&mut t, 0x86, "A_OPUS");
        let pre_skip_ns = a.pre_skip as u64 * 1_000_000_000 / a.sample_rate as u64;
        el_uint(&mut t, 0x56AA, pre_skip_ns); // CodecDelay
        el_uint(&mut t, 0x56BB, OPUS_SEEK_PREROLL_NS); // SeekPreRoll
        element(&mut t, 0x63A2, &opus_head(a)); // CodecPrivate
        let mut au = Vec::new();
        el_float(&mut au, 0xB5, a.sample_rate as f64); // SamplingFrequency
        el_uint(&mut au, 0x9F, a.channels as u64); // Channels
        element(&mut t, 0xE1, &au); // Audio
        element(&mut body, 0xAE, &t);
    }
    let mut out = Vec::new();
    element(&mut out, TRACKS, &body);
    out
}

fn simple_block(b: &Block, cluster_ts: i64) -> Vec<u8> {
    let rel = b.pts_ms - cluster_ts;
    debug_assert!((i16::MIN as i64..=i16::MAX as i64).contains(&rel));
    let mut body = Vec::with_capacity(b.data.len() + 4);
    body.push(0x80 | b.track); // track number as 1-byte vint
    body.extend_from_slice(&(rel as i16).to_be_bytes());
    body.push(if b.keyframe { 0x80 } else { 0x00 });
    body.extend_from_slice(&b.data);
    let mut out = Vec::new();
    match b.duration_ms {
        // Subtitles carry a duration: BlockGroup { Block, BlockDuration }.
        Some(dur) => {
            let mut group = Vec::new();
            // Block (0xA1) shares SimpleBlock's layout minus the flags bit.
            let mut blk = body;
            blk[3] = 0; // Block has no flags bit

            element(&mut group, 0xA1, &blk);
            el_uint(&mut group, 0x9B, dur);
            element(&mut out, 0xA0, &group);
        }
        None => element(&mut out, 0xA3, &body),
    }
    out
}

/// Muxes blocks (any order) into a complete WebM file.
pub fn mux(
    video: &VideoTrack,
    audio: Option<&AudioTrack>,
    mut blocks: Vec<Block>,
    duration_ms: f64,
) -> Vec<u8> {
    mux_with_cues(video, audio, &[], &mut blocks, duration_ms)
}

pub fn mux_with_cues(
    video: &VideoTrack,
    audio: Option<&AudioTrack>,
    cues: &[Cue],
    blocks: &mut Vec<Block>,
    duration_ms: f64,
) -> Vec<u8> {
    for c in cues {
        blocks.push(Block {
            pts_ms: c.start_ms,
            track: 3,
            keyframe: true,
            data: c.text.clone().into_bytes(),
            duration_ms: Some((c.end_ms - c.start_ms).max(1) as u64),
        });
    }
    // Stable interleave by pts; video before audio at equal timestamps so a
    // seek lands on the frame first.
    blocks.sort_by(|a, b| a.pts_ms.cmp(&b.pts_ms).then(a.track.cmp(&b.track)));
    let blocks: &[Block] = blocks;

    let mut segment = Vec::new();
    {
        let mut info = Vec::new();
        el_uint(&mut info, 0x002A_D7B1, TIMESTAMP_SCALE); // TimestampScale
        el_float(&mut info, 0x4489, duration_ms); // Duration
        el_string(&mut info, 0x4D80, "reel"); // MuxingApp
        el_string(&mut info, 0x5741, "reel"); // WritingApp
        element(&mut segment, INFO, &info);
    }
    segment.extend_from_slice(&tracks(video, audio, !cues.is_empty()));

    let mut i = 0usize;
    while i < blocks.len() {
        let cluster_ts = blocks[i].pts_ms;
        let mut body = Vec::new();
        el_uint(&mut body, 0xE7, cluster_ts.max(0) as u64); // Timestamp
        while i < blocks.len() {
            let b = &blocks[i];
            let over_span = b.pts_ms - cluster_ts >= CLUSTER_MAX_MS;
            // Video keyframes open a fresh cluster so players can seek.
            let key_boundary = b.track == 1 && b.keyframe && b.pts_ms != cluster_ts;
            if over_span || key_boundary {
                break;
            }
            body.extend_from_slice(&simple_block(b, cluster_ts));
            i += 1;
        }
        element(&mut segment, CLUSTER, &body);
    }

    let mut out = ebml_header();
    element(&mut out, SEGMENT, &segment);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vid() -> VideoTrack {
        VideoTrack { width: 320, height: 180 }
    }

    fn aud() -> AudioTrack {
        AudioTrack { channels: 1, sample_rate: 48_000, pre_skip: 0 }
    }

    fn block(pts_ms: i64, track: u8, keyframe: bool) -> Block {
        Block { pts_ms, track, keyframe, data: vec![0xAB; 8], duration_ms: None }
    }

    #[test]
    fn vint_size_encoding_matches_spec() {
        let mut out = Vec::new();
        write_size(&mut out, 0);
        assert_eq!(out, [0x80]);
        out.clear();
        write_size(&mut out, 126);
        assert_eq!(out, [0xFE]);
        out.clear();
        write_size(&mut out, 127); // needs 2 bytes: 0x7F collides with reserved
        assert_eq!(out, [0x40, 0x7F]);
        out.clear();
        write_size(&mut out, 500);
        assert_eq!(out, [0x41, 0xF4]);
    }

    #[test]
    fn header_declares_webm_doctype() {
        let bytes = mux(&vid(), None, vec![block(0, 1, true)], 1000.0);
        assert_eq!(&bytes[..4], &[0x1A, 0x45, 0xDF, 0xA3]);
        let hay = &bytes[..64];
        assert!(hay.windows(4).any(|w| w == b"webm"), "doctype missing");
    }

    #[test]
    fn tracks_carry_codecs_and_opus_head() {
        let bytes = mux(&vid(), Some(&aud()), vec![block(0, 1, true)], 1000.0);
        assert!(bytes.windows(5).any(|w| w == b"V_VP9"));
        assert!(bytes.windows(6).any(|w| w == b"A_OPUS"));
        assert!(bytes.windows(8).any(|w| w == b"OpusHead"));
    }

    #[test]
    fn keyframes_split_clusters() {
        let blocks = vec![
            block(0, 1, true),
            block(33, 1, false),
            block(66, 1, true), // new cluster
            block(99, 1, false),
        ];
        let bytes = mux(&vid(), None, blocks, 132.0);
        let clusters = bytes
            .windows(4)
            .filter(|w| *w == [0x1F, 0x43, 0xB6, 0x75])
            .count();
        assert_eq!(clusters, 2);
    }

    #[test]
    fn long_runs_split_clusters_within_i16_range() {
        // 40s of delta frames: must split even without keyframes.
        let blocks: Vec<Block> = (0..40).map(|s| block(s * 1000, 1, s == 0)).collect();
        let bytes = mux(&vid(), None, blocks, 40_000.0);
        let clusters = bytes
            .windows(4)
            .filter(|w| *w == [0x1F, 0x43, 0xB6, 0x75])
            .count();
        assert!(clusters >= 4, "expected cluster splits, got {clusters}");
    }

    #[test]
    fn interleave_sorts_audio_between_frames() {
        let blocks = vec![block(40, 2, true), block(0, 1, true), block(20, 2, true)];
        let bytes = mux(&vid(), Some(&aud()), blocks, 60.0);
        // The first SimpleBlock after the cluster timestamp is the video one
        // (track 1): find first 0xA3 element and check its track byte.
        let pos = bytes
            .windows(2)
            .position(|w| w[0] == 0xA3 && w[1] == 0x8C) // id + size 12
            .expect("simple block");
        assert_eq!(bytes[pos + 2], 0x81, "video track first");
    }
}
