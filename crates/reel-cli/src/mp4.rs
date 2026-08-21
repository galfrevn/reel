//! MP4 output by piping raw frames into ffmpeg.
//!
//! Unlike `.gif`/`.webm`/`.apng`, which reel encodes itself, `.mp4` leans on
//! whatever ffmpeg the user already has. H.264 is the reason: shipping an
//! encoder means shipping a patented one, and the whole point of `.mp4` is
//! reaching players that already decode H.264 anyway — so the machine that
//! wants it usually has ffmpeg too.
//!
//! Frames arrive change-driven with a duration each (same as the WebM path);
//! this writes them onto a constant-rate tick grid, because a rawvideo pipe
//! carries no timestamps. A still stretch therefore goes down the pipe once
//! per tick, and at 60fps most ticks are duplicates — so the pipe, not the
//! encoder, is what costs. Two things keep that affordable:
//!
//! - Frames are converted to I420 *here*, once per distinct frame, and the
//!   planes are re-sent for each tick that repeats them. Handing ffmpeg RGBA
//!   instead would move 2.7x the bytes and make it re-convert every tick;
//!   measured on an 11.8s 1734x1224 recording, that was 2.59s of ffmpeg
//!   against 1.21s for pre-converted I420.
//! - The conversion is `reel_encode::yuv`, the same BT.709 limited-range one
//!   VP9 uses, so `.mp4` and `.webm` come out of the same colour maths — and
//!   the output gets tagged bt709 rather than left for players to guess.

use anyhow::{anyhow, bail, Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// Where `mp4` output found its ffmpeg, for the error/report line.
pub fn locate() -> Option<PathBuf> {
    // An explicit override wins: static builds and Nix store paths are common
    // enough that "it's not on PATH" shouldn't mean "you can't render mp4".
    if let Some(p) = std::env::var_os("REEL_FFMPEG") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("ffmpeg"))
        .find(|c| c.is_file())
}

/// The message shown when there's no ffmpeg to render with. Spelled out
/// because "install ffmpeg" is a worse answer than "here's the one line".
pub fn missing_error() -> anyhow::Error {
    anyhow!(
        "mp4 output needs ffmpeg on PATH (reel encodes .gif/.webm/.apng itself, \
         but not H.264)\n  \
         macOS:  brew install ffmpeg\n  \
         Debian: apt install ffmpeg\n  \
         other:  https://ffmpeg.org/download.html\n\
         Already have one somewhere else? Point REEL_FFMPEG at it.\n\
         Or render .webm — same quality, no external tool."
    )
}

/// H.264 encoders we know how to drive, best first.
///
/// `libx264` is the quality benchmark. `h264_videotoolbox` is the common
/// fallback on macOS builds compiled without the GPL bits — hardware, fast,
/// a little larger at the same quality. `libopenh264` is last: it works, but
/// its rate control is cruder and it ignores `-crf`.
const ENCODERS: [&str; 3] = ["libx264", "h264_videotoolbox", "libopenh264"];

/// Colour tags matching `reel_encode::yuv`'s conversion, stated on both the
/// input (rawvideo has no metadata to read) and the output (so the file says
/// what it is instead of leaving players to infer it from frame size).
const BT709: [&str; 8] = [
    "-colorspace",
    "bt709",
    "-color_primaries",
    "bt709",
    "-color_trc",
    "bt709",
    "-color_range",
    "tv",
];

/// Ask ffmpeg which of `ENCODERS` it was built with.
pub fn pick_encoder(ffmpeg: &Path) -> Result<&'static str> {
    let out = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-encoders"])
        .output()
        .with_context(|| format!("running {}", ffmpeg.display()))?;
    select_encoder(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| {
        anyhow!(
            "{} has no H.264 encoder (looked for {}); install a build with libx264",
            ffmpeg.display(),
            ENCODERS.join(", ")
        )
    })
}

/// The best of [`ENCODERS`] present in an `ffmpeg -encoders` listing.
///
/// Rows read ` V....D <name>  <description>`, and the description repeats the
/// underlying library — `libx264rgb`'s says "libx264 H.264 RGB". Matching
/// anywhere on the line would therefore pick `libx264` off a build that only
/// has the RGB variant, whose output no browser will play. So: name column
/// only, which is the second whitespace-separated field.
fn select_encoder(listing: &str) -> Option<&'static str> {
    let names: Vec<&str> = listing.lines().filter_map(|l| l.split_whitespace().nth(1)).collect();
    ENCODERS.iter().copied().find(|name| names.contains(name))
}

/// Quality knob, per encoder — each one spells constant quality differently,
/// and a bitrate target is a poor substitute for it here. Terminal frames
/// are mostly flat, so a rate that suits one recording wildly overshoots the
/// next; constant quality lets the file be as small as the content allows.
fn quality_args(encoder: &str, crf: u32, width: u32, height: u32, fps: u32) -> Vec<String> {
    match encoder {
        "libx264" => vec!["-crf".into(), crf.to_string()],
        // VideoToolbox takes 1-100, higher is better — roughly the inverse of
        // CRF over the range the ladder walks (23 → 63, 38 → 39).
        "h264_videotoolbox" => {
            let q = (100.0 - crf as f64 * 1.6).clamp(10.0, 90.0);
            vec!["-q:v".into(), format!("{}", q.round() as u32)]
        }
        // libopenh264 really has no constant-quality mode, so this one gets a
        // bitrate: ~0.015 bits per pixel at CRF 23, halving every +6 rungs.
        _ => {
            let pixels_per_s = width as f64 * height as f64 * fps.max(1) as f64;
            let kbps = (pixels_per_s * 0.015 / 1000.0 * 2f64.powf((23.0 - crf as f64) / 6.0))
                .clamp(150.0, 8000.0);
            vec!["-b:v".into(), format!("{}k", kbps.round() as u32)]
        }
    }
}

/// Borrowed throughout: the budget ladder builds one of these per rung, and
/// the audio buffer has no business being copied five times.
pub struct Options<'a> {
    /// x264 CRF, or the anchor for the bitrate fallback. Lower is better.
    pub crf: u32,
    /// Output frame rate; frames are held across ticks to reach it.
    pub fps: u32,
    /// Mono f32 samples at [`reel_audio::SAMPLE_RATE`].
    pub audio: Option<&'a [f32]>,
    /// WebVTT source for an in-band `mov_text` subtitle track.
    pub vtt: Option<&'a str>,
}

/// A running ffmpeg, fed RGBA frames on stdin.
pub struct Encoder {
    child: Child,
    stderr: std::thread::JoinHandle<String>,
    /// Temp inputs (raw audio, subtitles) to clean up on finish.
    temps: Vec<PathBuf>,
    encoder: &'static str,
    quality_label: String,
    /// Scratch for the RGBA -> I420 conversion, reused every frame.
    i420: reel_encode::yuv::I420Frame,
    /// The three planes laid out contiguously, so a tick is one `write_all`
    /// rather than three — and a repeated tick is a straight re-send with no
    /// conversion behind it.
    planes: Vec<u8>,
    width: u32,
    height: u32,
    fps: f64,
    clock_s: f64,
    tick: usize,
    frames_written: usize,
}

impl Encoder {
    pub fn start(
        ffmpeg: &Path,
        encoder: &'static str,
        width: u32,
        height: u32,
        out_path: &Path,
        opts: &Options<'_>,
    ) -> Result<Self> {
        let mut temps = Vec::new();
        let mut cmd = Command::new(ffmpeg);
        cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);

        // Input 0: the frame pipe, already I420 so ffmpeg has no conversion
        // to do. The tags have to be stated: rawvideo carries no metadata,
        // and an untagged stream leaves players guessing at the matrix.
        cmd.args(["-f", "rawvideo", "-pix_fmt", "yuv420p"]);
        cmd.args(["-s", &format!("{width}x{height}")]);
        cmd.args(["-r", &opts.fps.to_string()]);
        cmd.args(BT709);
        cmd.args(["-i", "pipe:0"]);

        // Input 1: audio, as bare f32le — no WAV header needed.
        if let Some(samples) = opts.audio {
            let path = temp_path(out_path, "audio.f32");
            let mut bytes = Vec::with_capacity(samples.len() * 4);
            for s in samples {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            std::fs::write(&path, &bytes)
                .with_context(|| format!("writing {}", path.display()))?;
            temps.push(path.clone());
            cmd.args(["-f", "f32le"]);
            cmd.args(["-ar", &reel_audio::SAMPLE_RATE.to_string()]);
            cmd.args(["-ac", "1"]);
            cmd.arg("-i").arg(&path);
        }

        // Input 2: captions.
        if let Some(vtt) = opts.vtt {
            let path = temp_path(out_path, "subs.vtt");
            std::fs::write(&path, vtt).with_context(|| format!("writing {}", path.display()))?;
            temps.push(path.clone());
            cmd.arg("-i").arg(&path);
        }

        cmd.args(["-c:v", encoder]);
        if encoder == "libx264" {
            // `medium` is x264's default and the right trade here: terminal
            // frames are mostly skip blocks, so the slower presets buy little.
            cmd.args(["-preset", "medium"]);
        }
        let quality = quality_args(encoder, opts.crf, width, height, opts.fps);
        // "-crf 23" -> "crf 23"; whatever knob was used is what gets reported.
        let quality_label = quality.join(" ").trim_start_matches('-').to_string();
        cmd.args(&quality);
        // yuv420p + High is the combination every player takes, and the one
        // Safari and QuickTime insist on. The level is deliberately left to
        // the encoder: reel canvases routinely run past 1080p, and pinning a
        // level that the frame size doesn't fit makes VideoToolbox refuse to
        // open at all (-12902) where libx264 would only warn.
        cmd.args(["-pix_fmt", "yuv420p", "-profile:v", "high"]);
        cmd.args(BT709);
        // yuv420p needs even dimensions; pad rather than scale so glyphs stay
        // pixel-exact. The pad colour is the canvas edge, not black.
        cmd.args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2"]);
        // moov ahead of mdat: the file plays before it finishes downloading,
        // which is the whole point on a README or a timeline.
        cmd.args(["-movflags", "+faststart"]);
        if opts.audio.is_some() {
            cmd.args(["-c:a", "aac", "-b:a", "96k"]);
        }
        if opts.vtt.is_some() {
            cmd.args(["-c:s", "mov_text"]);
        }
        cmd.arg(out_path);

        cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawning {}", ffmpeg.display()))?;

        // Drain stderr on a thread: ffmpeg blocks once the pipe buffer fills,
        // and a blocked ffmpeg means a deadlocked write on our side.
        let mut err = child.stderr.take().expect("stderr piped");
        let stderr = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = std::io::Read::read_to_string(&mut err, &mut s);
            s
        });

        Ok(Encoder {
            child,
            stderr,
            temps,
            encoder,
            quality_label,
            i420: reel_encode::yuv::I420Frame {
                width,
                height,
                y: Vec::new(),
                u: Vec::new(),
                v: Vec::new(),
            },
            planes: Vec::new(),
            width,
            height,
            fps: opts.fps.max(1) as f64,
            clock_s: 0.0,
            tick: 0,
            frames_written: 0,
        })
    }

    /// Hold `rgba` for every output tick its display window covers.
    ///
    /// The conversion happens once here even when the frame covers thirty
    /// ticks; only the finished planes are re-sent.
    pub fn push(&mut self, rgba: &[u8], width: u32, height: u32, dur_s: f64) -> Result<()> {
        if width != self.width || height != self.height {
            bail!("frame {} changed size mid-render", self.frames_written);
        }
        let end = self.clock_s + dur_s;
        // How many ticks this frame's display window covers; zero means a
        // frame shorter than one tick, which costs us nothing to skip.
        let ticks = {
            let mut n = 0;
            while ((self.tick + n) as f64) / self.fps < end - 1e-9 {
                n += 1;
            }
            n
        };
        if ticks > 0 {
            reel_encode::yuv::rgba_to_i420_into(self.width, self.height, rgba, &mut self.i420);
            self.planes.clear();
            self.planes.extend_from_slice(&self.i420.y);
            self.planes.extend_from_slice(&self.i420.u);
            self.planes.extend_from_slice(&self.i420.v);

            let stdin = self.child.stdin.as_mut().expect("stdin piped");
            for _ in 0..ticks {
                if let Err(e) = stdin.write_all(&self.planes) {
                    // A broken pipe means ffmpeg already exited; its stderr
                    // says why, and that's a far better error than "broken
                    // pipe".
                    if e.kind() == std::io::ErrorKind::BrokenPipe {
                        return Err(self.fail("ffmpeg exited early"));
                    }
                    return Err(e).context("writing frames to ffmpeg");
                }
            }
            self.tick += ticks;
            self.frames_written += ticks;
        }
        self.clock_s = end;
        Ok(())
    }

    /// Close the pipe, wait for the mux, and report what came out.
    pub fn finish(mut self) -> Result<Report> {
        if self.frames_written == 0 {
            let _ = self.child.kill();
            self.cleanup();
            bail!("no frames to encode");
        }
        drop(self.child.stdin.take());
        let status = self.child.wait().context("waiting for ffmpeg")?;
        let stderr = self.join_stderr();
        self.cleanup();
        if !status.success() {
            bail!("ffmpeg failed ({status}){}", indent_stderr(&stderr));
        }
        Ok(Report {
            frames: self.frames_written,
            encoder: self.encoder,
            quality: std::mem::take(&mut self.quality_label),
        })
    }

    /// Kill ffmpeg and fold whatever it said into the error.
    fn fail(&mut self, what: &str) -> anyhow::Error {
        let _ = self.child.wait();
        let stderr = std::mem::replace(&mut self.stderr, std::thread::spawn(String::new))
            .join()
            .unwrap_or_default();
        anyhow!("{what}{}", indent_stderr(&stderr))
    }

    fn join_stderr(&mut self) -> String {
        std::mem::replace(&mut self.stderr, std::thread::spawn(String::new))
            .join()
            .unwrap_or_default()
    }

    fn cleanup(&mut self) {
        for p in self.temps.drain(..) {
            let _ = std::fs::remove_file(p);
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // An error path that unwinds past `finish` must not leave an ffmpeg
        // running on a pipe nobody will close, nor temp files behind.
        if self.child.try_wait().map(|s| s.is_none()).unwrap_or(false) {
            drop(self.child.stdin.take());
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        self.cleanup();
    }
}

pub struct Report {
    pub frames: usize,
    pub encoder: &'static str,
    /// The quality knob actually used, e.g. `crf 23` or `q:v 63`.
    pub quality: String,
}

fn indent_stderr(stderr: &str) -> String {
    let body = stderr.trim();
    if body.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n");
    for line in body.lines().take(12) {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Temp files live beside the output, not in /tmp: same filesystem, so the
/// staging file can be renamed into place rather than copied, a sandboxed or
/// read-only /tmp can't break a render, and a crash leaves the debris
/// somewhere the user will actually notice.
pub fn temp_path(out_path: &Path, suffix: &str) -> PathBuf {
    let dir = out_path.parent().unwrap_or(Path::new("."));
    let stem = out_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    dir.join(format!(".{stem}.reel-{}-{suffix}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real `ffmpeg -encoders` run.
    const LISTING: &str = "\
 V....D libx264              libx264 H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10 (codec h264)
 V....D libx264rgb           libx264 H.264 RGB (codec h264)
 V....D h264_videotoolbox    VideoToolbox H.264 Encoder (codec h264)
 V....D hevc_videotoolbox    VideoToolbox H.265 Encoder (codec hevc)
";

    #[test]
    fn best_encoder_wins() {
        assert_eq!(select_encoder(LISTING), Some("libx264"));
    }

    #[test]
    fn falls_through_to_videotoolbox() {
        let without_x264: String =
            LISTING.lines().filter(|l| !l.contains("libx264")).collect::<Vec<_>>().join("\n");
        assert_eq!(select_encoder(&without_x264), Some("h264_videotoolbox"));
    }

    #[test]
    fn no_h264_encoder_is_none() {
        assert_eq!(select_encoder(" V....D libwebp  libwebp\n"), None);
    }

    /// `libx264rgb` must not be mistaken for `libx264`: it encodes RGB, which
    /// no browser will play, and it would silently win the search.
    #[test]
    fn rgb_variant_is_not_a_match() {
        assert_eq!(select_encoder(" V....D libx264rgb  libx264 H.264 RGB\n"), None);
    }

    #[test]
    fn quality_knob_matches_the_encoder() {
        assert_eq!(quality_args("libx264", 23, 1280, 720, 60), vec!["-crf", "23"]);
        // VideoToolbox counts the other way: higher is better.
        assert_eq!(quality_args("h264_videotoolbox", 23, 1280, 720, 60), vec!["-q:v", "63"]);
        assert_eq!(quality_args("h264_videotoolbox", 38, 1280, 720, 60), vec!["-q:v", "39"]);
        // libopenh264 has no constant quality, so it gets a bitrate.
        let openh264 = quality_args("libopenh264", 23, 1280, 720, 60);
        assert_eq!(openh264[0], "-b:v");
        assert!(openh264[1].ends_with('k'), "{openh264:?}");
    }

    /// Every ladder rung must ask for less than the one above it.
    #[test]
    fn the_ladder_only_descends() {
        for enc in ["h264_videotoolbox", "libopenh264"] {
            let rung = |crf| {
                quality_args(enc, crf, 1280, 720, 60)[1]
                    .trim_end_matches('k')
                    .parse::<u32>()
                    .unwrap()
            };
            for pair in [23u32, 28, 33, 38].windows(2) {
                assert!(rung(pair[1]) < rung(pair[0]), "{enc}: {} !< {}", pair[1], pair[0]);
            }
        }
    }
}
