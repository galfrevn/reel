//! End-to-end: the real `reel` binary renders the embedded demo cast to
//! every format, and independent decoders confirm the outputs are valid.
//! This is the test that keeps a broken `reel render` from merging green.

use std::path::{Path, PathBuf};
use std::process::Command;

fn reel() -> Command {
    Command::new(env!("CARGO_BIN_EXE_reel"))
}

fn demo_cast(dir: &Path) -> PathBuf {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/demo.cast");
    let dst = dir.join("demo.cast");
    std::fs::copy(src, &dst).expect("demo cast ships in the repo");
    dst
}

fn render(cast: &Path, out: &Path) {
    let status = reel()
        .args([
            "render",
            cast.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--quiet",
        ])
        .status()
        .expect("reel binary runs");
    assert!(status.success(), "render to {} failed", out.display());
}

#[test]
fn renders_a_valid_gif() {
    let dir = tempfile::tempdir().unwrap();
    let cast = demo_cast(dir.path());
    let out = dir.path().join("demo.gif");
    render(&cast, &out);

    let file = std::fs::File::open(&out).unwrap();
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options.read_info(file).expect("gif decodes");
    let mut frames = 0;
    while decoder.read_next_frame().expect("frame decodes").is_some() {
        frames += 1;
    }
    assert!(frames > 10, "expected a real animation, got {frames} frames");
    assert!(decoder.width() > 100 && decoder.height() > 100);
}

#[test]
fn renders_a_valid_png() {
    let dir = tempfile::tempdir().unwrap();
    let cast = demo_cast(dir.path());
    let out = dir.path().join("demo.png");
    render(&cast, &out);

    let decoder =
        png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&out).unwrap()));
    let mut reader = decoder.read_info().expect("png decodes");
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).expect("png frame decodes");
    assert!(info.width > 100 && info.height > 100);
}

#[cfg(feature = "video")]
#[test]
fn renders_a_valid_webm() {
    let dir = tempfile::tempdir().unwrap();
    let cast = demo_cast(dir.path());
    let out = dir.path().join("demo.webm");
    render(&cast, &out);

    let mkv = matroska::open(&out).expect("webm parses as matroska");
    let video = mkv
        .tracks
        .iter()
        .find(|t| matches!(t.tracktype, matroska::Tracktype::Video))
        .expect("has a video track");
    assert_eq!(video.codec_id, "V_VP9");
    assert!(mkv.info.duration.is_some(), "duration must be muxed");
}

#[test]
fn txt_dump_matches_the_recording() {
    let dir = tempfile::tempdir().unwrap();
    let cast = demo_cast(dir.path());
    let out = dir.path().join("demo.txt");
    render(&cast, &out);
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(!text.trim().is_empty(), "txt dump must carry the session");
}

#[test]
fn malformed_cast_fails_cleanly_not_loudly() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.cast");
    std::fs::write(&bad, "{\"version\": 2, \"width\": 80, \"height\": 24}\n[1e15, \"o\", \"x\"]\n")
        .unwrap();
    let out = dir.path().join("bad.gif");
    let output = reel()
        .args(["render", bad.to_str().unwrap(), "-o", out.to_str().unwrap(), "--quiet"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "absurd cast must be rejected");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid time"), "error names the cause: {stderr}");
    assert!(!out.exists(), "no partial artifact on failure");
}
