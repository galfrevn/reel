//! `reel record`: own capture over a PTY (Phase 2).
//!
//! Interactive passthrough model: reel sits between your real terminal and
//! the child like asciinema does — bytes from the child go to your screen
//! *and* the cast; your keystrokes go to the child *and* the `.reelmeta`
//! sidecar (as timestamped input events, which is what makes accurate
//! keystroke audio possible later). Terminal queries (DA1, DSR, OSC 10/11…)
//! are answered by the real terminal you're sitting at, because responses it
//! writes to stdin are forwarded to the child like any other input.

use anyhow::{anyhow, Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use reel_cast::{CastHeader, CastWriter, InputEvent, ReelMeta};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Restores cooked mode even on panic or error paths.
struct RawGuard;

impl RawGuard {
    fn new() -> Result<Self> {
        crossterm::terminal::enable_raw_mode().context("enabling raw mode")?;
        Ok(RawGuard)
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Buffers a trailing incomplete UTF-8 sequence between reads so multi-byte
/// glyphs split across chunk boundaries never turn into replacement chars.
#[derive(Default)]
struct Utf8Carry {
    pending: Vec<u8>,
}

impl Utf8Carry {
    fn push(&mut self, chunk: &[u8]) -> String {
        self.pending.extend_from_slice(chunk);
        match std::str::from_utf8(&self.pending) {
            Ok(s) => {
                let s = s.to_string();
                self.pending.clear();
                s
            }
            Err(e) => {
                let valid = e.valid_up_to();
                // An error mid-buffer is real garbage; only hold back a
                // short (< 4 byte) clean tail that could be a split glyph.
                if e.error_len().is_none() && self.pending.len() - valid < 4 {
                    let s = String::from_utf8_lossy(&self.pending[..valid]).into_owned();
                    self.pending.drain(..valid);
                    s
                } else {
                    let s = String::from_utf8_lossy(&self.pending).into_owned();
                    self.pending.clear();
                    s
                }
            }
        }
    }
}

pub fn record(out: &Path, size: Option<String>, command: Vec<String>) -> Result<()> {
    if out.exists() {
        return Err(anyhow!(
            "{} already exists — pick another --out or remove it first",
            out.display()
        ));
    }
    let (cols, rows) = match &size {
        Some(s) => parse_size(s)
            .ok_or_else(|| anyhow!("invalid --size `{s}` (expected COLSxROWS, e.g. 120x40)"))?,
        None => crossterm::terminal::size().unwrap_or((80, 24)),
    };

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| anyhow!("openpty: {e}"))?;

    let mut cmd = if command.is_empty() {
        CommandBuilder::new(default_shell())
    } else {
        let mut c = CommandBuilder::new(&command[0]);
        c.args(&command[1..]);
        c
    };
    // The feature set reel's own renderer emulates (spec §9.1).
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }

    let header = CastHeader {
        version: 2,
        width: cols,
        height: rows,
        timestamp: SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs()),
        duration: None,
        title: None,
        command: (!command.is_empty()).then(|| command.join(" ")),
        env: Some(
            [
                ("TERM".to_string(), "xterm-256color".to_string()),
                ("SHELL".to_string(), std::env::var("SHELL").unwrap_or_default()),
            ]
            .into(),
        ),
        extra: Default::default(),
    };
    let file = std::fs::File::create(out)
        .with_context(|| format!("creating {}", out.display()))?;
    let writer = Arc::new(Mutex::new(CastWriter::new(
        std::io::BufWriter::new(file),
        &header,
    )?));
    let inputs: Arc<Mutex<Vec<InputEvent>>> = Arc::default();
    let started = Instant::now();
    let done = Arc::new(AtomicBool::new(false));

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| anyhow!("spawning command: {e}"))?;
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| anyhow!("pty reader: {e}"))?;
    let mut pty_writer = pair.master.take_writer().map_err(|e| anyhow!("pty writer: {e}"))?;

    eprintln!(
        "reel record: capturing to {} — exit the program (or shell) to stop; \
         Ctrl+] drops a marker",
        out.display()
    );
    // Raw mode needs a real terminal; without one (CI, scripts) we still
    // capture child output fine — there's just no keyboard to pass through.
    let _raw = if crossterm::tty::IsTty::is_tty(&std::io::stdin()) {
        Some(RawGuard::new()?)
    } else {
        None
    };

    // Child output → real terminal + cast.
    let out_thread = {
        let writer = writer.clone();
        std::thread::spawn(move || {
            let mut stdout = std::io::stdout();
            let mut carry = Utf8Carry::default();
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let _ = stdout.write_all(&buf[..n]);
                let _ = stdout.flush();
                let text = carry.push(&buf[..n]);
                if !text.is_empty() {
                    let t = started.elapsed().as_secs_f64();
                    let _ = writer.lock().unwrap().event(t, "o", &text);
                }
            }
        })
    };

    // Real keyboard → child + sidecar. Detached: a blocking stdin read has
    // no portable cancel, and the process exits right after the child does.
    // Ctrl+] never reaches the child: it becomes a cast marker instead.
    let markers = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let inputs = inputs.clone();
        let done = done.clone();
        let writer = writer.clone();
        let markers = markers.clone();
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin();
            let mut carry = Utf8Carry::default();
            let mut buf = [0u8; 1024];
            let mut clean = Vec::with_capacity(1024);
            while let Ok(n) = stdin.read(&mut buf) {
                if n == 0 || done.load(Ordering::Relaxed) {
                    break;
                }
                let dropped = strip_marker_bytes(&buf[..n], &mut clean);
                for _ in 0..dropped {
                    let t = started.elapsed().as_secs_f64();
                    let _ = writer.lock().unwrap().event(t, "m", "");
                    markers.fetch_add(1, Ordering::Relaxed);
                    // Audible ack; nothing visual, so the child's screen
                    // stays untouched.
                    let mut stdout = std::io::stdout();
                    let _ = stdout.write_all(b"\x07");
                    let _ = stdout.flush();
                }
                if clean.is_empty() {
                    continue;
                }
                if pty_writer.write_all(&clean).is_err() {
                    break;
                }
                let _ = pty_writer.flush();
                let value = carry.push(&clean);
                if !value.is_empty() {
                    inputs.lock().unwrap().push(InputEvent {
                        t: started.elapsed().as_secs_f64(),
                        kind: "key".into(),
                        value,
                    });
                }
            }
        });
    }

    // Window resizes → PTY + cast "r" events (polled: portable and simple).
    // With an explicit --size the PTY is pinned: tracking the real terminal
    // would undo exactly what the flag asked for.
    if size.is_none() {
        let writer = writer.clone();
        let done = done.clone();
        let master = pair.master;
        std::thread::spawn(move || {
            let mut last = (cols, rows);
            while !done.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(250));
                if let Ok(size) = crossterm::terminal::size() {
                    if size != last {
                        last = size;
                        let _ = master.resize(PtySize {
                            rows: size.1,
                            cols: size.0,
                            pixel_width: 0,
                            pixel_height: 0,
                        });
                        let t = started.elapsed().as_secs_f64();
                        let _ = writer
                            .lock()
                            .unwrap()
                            .event(t, "r", &format!("{}x{}", size.0, size.1));
                    }
                }
            }
        });
    }

    let status = child.wait().map_err(|e| anyhow!("waiting for child: {e}"))?;
    done.store(true, Ordering::Relaxed);
    let _ = out_thread.join();
    drop(_raw);

    let input_events = std::mem::take(&mut *inputs.lock().unwrap());
    let n_inputs = input_events.len();
    let meta = ReelMeta {
        version: 1,
        input_events,
        term_env: [
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("COLORTERM".to_string(), "truecolor".to_string()),
        ]
        .into(),
        cols,
        rows,
    };
    meta.save_sidecar(out)?;

    // The writer Arc is shared with the output thread (already joined) and
    // the resize thread (detached but only touches it while !done).
    if let Ok(m) = Arc::try_unwrap(writer).map(Mutex::into_inner) {
        m.expect("writer lock").finish()?;
    }

    let secs = started.elapsed().as_secs_f64();
    let n_markers = markers.load(Ordering::Relaxed);
    let marker_note = match n_markers {
        0 => String::new(),
        1 => ", 1 marker (@1)".to_string(),
        n => format!(", {n} markers (@1..@{n})"),
    };
    eprintln!(
        "\nrecorded {secs:.1}s → {} (+.reelmeta, {n_inputs} input events{marker_note}){}",
        out.display(),
        if status.success() { String::new() } else { format!(" — child exited with {status:?}") }
    );
    eprintln!("render it: reel render {}", out.display());
    Ok(())
}

/// Ctrl+] (0x1D), intercepted from the recording keyboard as "drop a marker".
const MARKER_BYTE: u8 = 0x1d;

/// Copies `buf` into `clean` minus any marker bytes; returns how many were
/// stripped.
fn strip_marker_bytes(buf: &[u8], clean: &mut Vec<u8>) -> usize {
    clean.clear();
    let mut dropped = 0;
    for &b in buf {
        if b == MARKER_BYTE {
            dropped += 1;
        } else {
            clean.push(b);
        }
    }
    dropped
}

/// "120x40" → (120, 40).
fn parse_size(s: &str) -> Option<(u16, u16)> {
    let (c, r) = s.trim().split_once(['x', 'X'])?;
    let cols: u16 = c.trim().parse().ok()?;
    let rows: u16 = r.trim().parse().ok()?;
    (cols >= 20 && rows >= 5 && cols <= 500 && rows <= 200).then_some((cols, rows))
}

fn default_shell() -> String {
    #[cfg(windows)]
    {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_parses_and_bounds() {
        assert_eq!(parse_size("120x40"), Some((120, 40)));
        assert_eq!(parse_size(" 88X24 "), Some((88, 24)));
        assert_eq!(parse_size("0x40"), None);
        assert_eq!(parse_size("banana"), None);
    }

    #[test]
    fn utf8_carry_reassembles_split_glyphs() {
        let mut c = Utf8Carry::default();
        let bytes = "héllo → 世界".as_bytes();
        let (a, b) = bytes.split_at(7); // splits inside a multi-byte char
        let mut out = c.push(a);
        out.push_str(&c.push(b));
        assert_eq!(out, "héllo → 世界");
    }

    #[test]
    fn marker_bytes_strip_and_count() {
        let mut clean = Vec::new();
        assert_eq!(strip_marker_bytes(b"hello", &mut clean), 0);
        assert_eq!(clean, b"hello");
        assert_eq!(strip_marker_bytes(b"ab\x1dcd\x1d", &mut clean), 2);
        assert_eq!(clean, b"abcd");
        assert_eq!(strip_marker_bytes(b"\x1d", &mut clean), 1);
        assert!(clean.is_empty());
    }

    #[test]
    fn utf8_carry_flushes_real_garbage() {
        let mut c = Utf8Carry::default();
        let out = c.push(&[0x68, 0xFF, 0xFE, 0x69]);
        assert!(out.contains('h') && out.contains('i'));
        assert!(out.contains('\u{FFFD}'));
        assert!(c.pending.is_empty());
    }
}
