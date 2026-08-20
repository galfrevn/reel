//! End-to-end WebM validation: encode real frames + audio, then read the
//! container back with an independent Matroska parser.

#![cfg(feature = "video")]

use reel_encode::{encode_webm, RgbaFrame, WebmOptions};

fn gradient_frame(w: u32, h: u32, phase: u8, duration_s: f64) -> RgbaFrame {
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            data.extend_from_slice(&[
                (x as u8).wrapping_add(phase),
                (y as u8).wrapping_mul(2),
                phase,
                255,
            ]);
        }
    }
    RgbaFrame { width: w, height: h, data, duration_s }
}

fn beep(seconds: f64) -> Vec<f32> {
    let n = (seconds * 48_000.0) as usize;
    (0..n)
        .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.2)
        .collect()
}

#[test]
fn webm_with_audio_parses_and_reports_both_tracks() {
    let frames: Vec<RgbaFrame> = (0..12)
        .map(|i| gradient_frame(160, 90, i * 20, 0.25))
        .collect();
    let audio = beep(3.0);
    let report = encode_webm(&frames, Some(&audio), &WebmOptions::default()).unwrap();
    assert!(report.has_audio);
    assert_eq!(report.frames, 12);

    let dir = std::env::temp_dir().join("reel-webm-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("roundtrip.webm");
    std::fs::write(&path, &report.bytes).unwrap();

    let mkv = matroska::open(&path).expect("independent parser accepts the file");
    let duration = mkv.info.duration.expect("duration present");
    assert!((duration.as_secs_f64() - 3.0).abs() < 0.1, "duration {duration:?}");
    assert_eq!(mkv.tracks.len(), 2);
    assert_eq!(mkv.tracks[0].codec_id, "V_VP9");
    assert_eq!(mkv.tracks[1].codec_id, "A_OPUS");
    match &mkv.tracks[0].settings {
        matroska::Settings::Video(v) => {
            assert_eq!((v.pixel_width, v.pixel_height), (160, 90));
        }
        other => panic!("track 0 not video: {other:?}"),
    }
    match &mkv.tracks[1].settings {
        matroska::Settings::Audio(a) => {
            assert_eq!(a.channels, 1);
            assert!((a.sample_rate - 48_000.0).abs() < 1e-6);
        }
        other => panic!("track 1 not audio: {other:?}"),
    }
}

#[test]
fn webm_without_audio_has_one_track() {
    let frames = vec![gradient_frame(64, 64, 0, 0.5), gradient_frame(64, 64, 90, 0.5)];
    let report = encode_webm(&frames, None, &WebmOptions::default()).unwrap();
    assert!(!report.has_audio);

    let dir = std::env::temp_dir().join("reel-webm-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("video-only.webm");
    std::fs::write(&path, &report.bytes).unwrap();
    let mkv = matroska::open(&path).unwrap();
    assert_eq!(mkv.tracks.len(), 1);
}

#[test]
fn output_is_deterministic() {
    let frames = vec![gradient_frame(80, 60, 10, 0.2), gradient_frame(80, 60, 60, 0.2)];
    let audio = beep(0.4);
    let a = encode_webm(&frames, Some(&audio), &WebmOptions::default()).unwrap();
    let b = encode_webm(&frames, Some(&audio), &WebmOptions::default()).unwrap();
    assert_eq!(a.bytes, b.bytes);
}

#[test]
fn higher_cq_level_shrinks_output() {
    let frames: Vec<RgbaFrame> = (0..10)
        .map(|i| gradient_frame(160, 120, i * 25, 0.1))
        .collect();
    let good = encode_webm(&frames, None, &WebmOptions { cq_level: 10, ..Default::default() }).unwrap();
    let rough = encode_webm(&frames, None, &WebmOptions { cq_level: 55, ..Default::default() }).unwrap();
    assert!(
        rough.bytes.len() < good.bytes.len(),
        "cq 55 ({}) should be smaller than cq 10 ({})",
        rough.bytes.len(),
        good.bytes.len()
    );
}
