//! Opus packetization via `rusty-opus` — a pure-Rust libopus port, so audio
//! encoding needs no C toolchain anywhere.

use crate::webm::Block;
use crate::EncodeError;
use rusty_opus::{Application, OpusEncoder};

pub const OPUS_SAMPLE_RATE: u32 = 48_000;
/// 20ms frames — Opus's sweet spot and WebM's conventional packet size.
const FRAME_SAMPLES: usize = 960;
const BITRATE_BPS: i32 = 64_000;

/// Encodes 48kHz mono f32 samples into 20ms Opus blocks on track 2.
pub fn encode_opus(samples: &[f32]) -> Result<Vec<Block>, EncodeError> {
    let mut enc = OpusEncoder::new(OPUS_SAMPLE_RATE as i32, 1, Application::Audio)
        .map_err(|e| EncodeError::Opus(e.to_string()))?;
    enc.bitrate_bps = BITRATE_BPS;

    let mut blocks = Vec::with_capacity(samples.len() / FRAME_SAMPLES + 1);
    let mut out = vec![0u8; 4000];
    for (i, chunk) in samples.chunks(FRAME_SAMPLES).enumerate() {
        let mut frame;
        let input = if chunk.len() == FRAME_SAMPLES {
            chunk
        } else {
            frame = chunk.to_vec();
            frame.resize(FRAME_SAMPLES, 0.0);
            &frame
        };
        let n = enc
            .encode(input, FRAME_SAMPLES, &mut out)
            .map_err(|e| EncodeError::Opus(e.to_string()))?;
        blocks.push(Block {
            pts_ms: (i * FRAME_SAMPLES) as i64 * 1000 / OPUS_SAMPLE_RATE as i64,
            track: 2,
            keyframe: true, // every Opus packet is independently decodable
            data: out[..n].to_vec(),
            duration_ms: None,
        });
    }
    Ok(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets_land_on_the_20ms_grid() {
        let samples = vec![0.1f32; FRAME_SAMPLES * 3 + 100];
        let blocks = encode_opus(&samples).unwrap();
        assert_eq!(blocks.len(), 4, "partial tail becomes a padded packet");
        let pts: Vec<i64> = blocks.iter().map(|b| b.pts_ms).collect();
        assert_eq!(pts, vec![0, 20, 40, 60]);
        assert!(blocks.iter().all(|b| b.track == 2 && !b.data.is_empty()));
    }

    #[test]
    fn silence_still_produces_packets() {
        let blocks = encode_opus(&vec![0f32; FRAME_SAMPLES * 2]).unwrap();
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn encoding_is_deterministic() {
        let samples: Vec<f32> = (0..FRAME_SAMPLES * 5)
            .map(|i| (i as f32 * 0.05).sin() * 0.3)
            .collect();
        let a = encode_opus(&samples).unwrap();
        let b = encode_opus(&samples).unwrap();
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.data, y.data);
        }
    }
}
