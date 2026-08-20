//! `reel run`: script-mode capture. Executes the `.reel` file's script ops
//! against a live program in a PTY — typing with human pacing, waiting on
//! *screen state* instead of blind sleeps — records the session exactly
//! like `reel record`, then renders the same file's edit ops over it.

use anyhow::{anyhow, bail, Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use reel_cast::{CastHeader, CastWriter, InputEvent, ReelMeta};
use crate::queries::QueryResponder;
use reel_format::{ReelFile, ScriptOp, TypingCfg};
use reel_term::LiveTerm;
use std::io::Read;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Runs the script and returns the path of the cast it recorded.
pub fn capture(path: &Path, file: &ReelFile, quiet: bool) -> Result<PathBuf> {
    let script = &file.script;
    let Some(ScriptOp::Run { command }) = script.first() else {
        bail!("script mode starts with `run \"command\"`");
    };
    let (cols, rows) = (file.config.terminal.cols, file.config.terminal.rows);
    let cast_path = path.with_extension("cast");

    if !quiet {
        eprintln!(
            "reel run: {command} in a {cols}x{rows} pty → {}",
            cast_path.display()
        );
    }

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| anyhow!("openpty: {e}"))?;
    let mut cmd = shell_words(command)?;
    let program = cmd.remove(0);
    // Branded prompt from the template: only `reel run` can do this honestly,
    // because here reel is the one launching the shell.
    let prompt = prompt_injection(&file.config.template.name, &program, &cmd);
    let mut builder = CommandBuilder::new(&program);
    if let Some(p) = &prompt {
        builder.args(&p.extra_args);
    }
    builder.args(&cmd);
    builder.env("TERM", "xterm-256color");
    builder.env("COLORTERM", "truecolor");
    if let Some(p) = &prompt {
        for (k, v) in &p.env {
            builder.env(k, v);
        }
    }
    // `[env]` table: extra variables for the child (PS1, LANG, …) — set
    // after the template prompt so an explicit PS1 wins.
    if let Some(toml::Value::Table(env)) = &file.config.env {
        for (k, v) in env {
            if let toml::Value::String(v) = v {
                builder.env(k, v);
            }
        }
    }
    if let Some(dir) = path.parent() {
        if dir.as_os_str().is_empty() {
            builder.cwd(std::env::current_dir()?);
        } else {
            builder.cwd(dir.canonicalize()?);
        }
    }

    let header = CastHeader {
        version: 2,
        width: cols,
        height: rows,
        timestamp: SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs()),
        duration: None,
        title: None,
        command: Some(command.clone()),
        env: Some([("TERM".to_string(), "xterm-256color".to_string())].into()),
        extra: Default::default(),
    };
    let out = std::fs::File::create(&cast_path)
        .with_context(|| format!("creating {}", cast_path.display()))?;
    let writer = Arc::new(Mutex::new(CastWriter::new(std::io::BufWriter::new(out), &header)?));
    let live = Arc::new(Mutex::new(LiveTerm::new(cols, rows).map_err(|e| anyhow!("{e}"))?));
    let last_output = Arc::new(Mutex::new(Instant::now()));
    let done = Arc::new(AtomicBool::new(false));
    let started = Instant::now();

    let mut child = pair
        .slave
        .spawn_command(builder)
        .map_err(|e| anyhow!("spawning `{command}`: {e}"))?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().map_err(|e| anyhow!("pty reader: {e}"))?;
    let pty_writer = Arc::new(Mutex::new(
        pair.master.take_writer().map_err(|e| anyhow!("pty writer: {e}"))?,
    ));

    // Output → cast + live grid (for wait_text) + idle clock, answering
    // terminal queries (DA1, DSR, OSC color…) so headless programs don't
    // hang or bail waiting for a terminal that isn't there (SPEC §9).
    let out_thread = {
        let writer = writer.clone();
        let live = live.clone();
        let last_output = last_output.clone();
        let responder_writer = pty_writer.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            let mut pending: Vec<u8> = Vec::new();
            let mut responder = QueryResponder::default();
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let cursor = {
                    let mut lt = live.lock().unwrap();
                    lt.feed(&buf[..n]);
                    lt.cursor()
                };
                for resp in responder.scan(&buf[..n], cursor) {
                    let mut w = responder_writer.lock().unwrap();
                    let _ = w.write_all(&resp);
                    let _ = w.flush();
                }
                *last_output.lock().unwrap() = Instant::now();
                pending.extend_from_slice(&buf[..n]);
                // Same UTF-8 carry rule as `reel record`.
                let valid = match std::str::from_utf8(&pending) {
                    Ok(_) => pending.len(),
                    Err(e) if e.error_len().is_none() => e.valid_up_to(),
                    Err(_) => pending.len(),
                };
                if valid > 0 {
                    let text = String::from_utf8_lossy(&pending[..valid]).into_owned();
                    let t = started.elapsed().as_secs_f64();
                    let _ = writer.lock().unwrap().event(t, "o", &text);
                    pending.drain(..valid);
                }
            }
        })
    };

    let mut inputs: Vec<InputEvent> = Vec::new();
    // Deterministic typing jitter.
    let mut rng_state: u64 = 0x0000_da5c_1eed;
    let mut jitter = |cfg: &TypingCfg| -> Duration {
        rng_state = rng_state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let unit = ((rng_state >> 33) as f64 / (1u64 << 31) as f64) * 2.0 - 1.0;
        let ms = cfg.delay_ms as f64 * (1.0 + cfg.jitter * unit);
        Duration::from_millis(ms.max(15.0) as u64)
    };
    let send = |bytes: &[u8], inputs: &mut Vec<InputEvent>| -> Result<()> {
        let mut w = pty_writer.lock().unwrap();
        w.write_all(bytes)?;
        w.flush()?;
        inputs.push(InputEvent {
            t: started.elapsed().as_secs_f64(),
            kind: "key".into(),
            value: String::from_utf8_lossy(bytes).into_owned(),
        });
        Ok(())
    };

    let mut failure: Option<String> = None;
    'ops: for op in &script[1..] {
        match op {
            ScriptOp::Run { .. } => {
                failure = Some("`run` may only appear once, first".into());
                break 'ops;
            }
            ScriptOp::Type { text } => {
                for ch in text.chars() {
                    send(ch.to_string().as_bytes(), &mut inputs)?;
                    std::thread::sleep(jitter(&file.config.typing));
                }
            }
            ScriptOp::Key { key } => {
                match key_bytes(key) {
                    Some(bytes) => send(&bytes, &mut inputs)?,
                    None => {
                        failure = Some(format!("unknown key `{key}`"));
                        break 'ops;
                    }
                }
                std::thread::sleep(Duration::from_millis(80));
            }
            ScriptOp::Sleep { dur } => std::thread::sleep(Duration::from_secs_f64(*dur)),
            ScriptOp::WaitText { text, timeout } => {
                let deadline = Instant::now() + Duration::from_secs_f64(*timeout);
                loop {
                    if live.lock().unwrap().contains(text) {
                        break;
                    }
                    if Instant::now() > deadline {
                        failure = Some(format!(
                            "wait_text \"{text}\" timed out after {timeout}s"
                        ));
                        break 'ops;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            ScriptOp::WaitIdle { quiet: q, timeout } => {
                let deadline = Instant::now() + Duration::from_secs_f64(*timeout);
                loop {
                    if last_output.lock().unwrap().elapsed().as_secs_f64() >= *q {
                        break;
                    }
                    if Instant::now() > deadline {
                        failure = Some(format!("wait_idle {q}s timed out after {timeout}s"));
                        break 'ops;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }

    // Let trailing output land, then stop the child.
    std::thread::sleep(Duration::from_millis(400));
    done.store(true, Ordering::Relaxed);
    let _ = child.kill();
    let _ = child.wait();
    drop(pair.master);
    let _ = out_thread.join();

    let n_inputs = inputs.len();
    ReelMeta {
        version: 1,
        input_events: inputs,
        term_env: [("TERM".to_string(), "xterm-256color".to_string())].into(),
        cols,
        rows,
    }
    .save_sidecar(&cast_path)?;
    if let Ok(m) = Arc::try_unwrap(writer).map(Mutex::into_inner) {
        m.expect("writer lock").finish()?;
    }

    if let Some(why) = failure {
        bail!(
            "script failed: {why} — the partial recording is at {} for debugging",
            cast_path.display()
        );
    }
    if !quiet {
        eprintln!(
            "captured {:.1}s ({n_inputs} input events) → {}",
            started.elapsed().as_secs_f64(),
            cast_path.display()
        );
    }
    Ok(cast_path)
}

/// Named key → bytes on the wire.
fn key_bytes(key: &str) -> Option<Vec<u8>> {
    let k = key.to_ascii_lowercase();
    Some(match k.as_str() {
        "enter" | "return" => b"\r".to_vec(),
        "esc" | "escape" => b"\x1b".to_vec(),
        "tab" => b"\t".to_vec(),
        "space" => b" ".to_vec(),
        "backspace" => b"\x7f".to_vec(),
        "up" => b"\x1b[A".to_vec(),
        "down" => b"\x1b[B".to_vec(),
        "right" => b"\x1b[C".to_vec(),
        "left" => b"\x1b[D".to_vec(),
        "home" => b"\x1b[H".to_vec(),
        "end" => b"\x1b[F".to_vec(),
        "pageup" => b"\x1b[5~".to_vec(),
        "pagedown" => b"\x1b[6~".to_vec(),
        _ => {
            if let Some(c) = k.strip_prefix("ctrl+") {
                let ch = c.chars().next()?;
                if ch.is_ascii_alphabetic() {
                    return Some(vec![(ch.to_ascii_uppercase() as u8) & 0x1f]);
                }
                return None;
            }
            let mut chars = key.chars();
            let ch = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            ch.to_string().into_bytes()
        }
    })
}

/// What a template `[prompt]` turns into for the spawned command: env vars
/// carrying the prompt string, plus rc-skipping flags when the command is a
/// bare shell (a themed .zshrc would immediately repaint over our prompt).
struct PromptInjection {
    extra_args: Vec<String>,
    env: Vec<(String, String)>,
}

fn prompt_injection(template: &str, program: &str, args: &[String]) -> Option<PromptInjection> {
    let tpl = reel_render::template::lookup(template)?;
    let p = tpl.prompt?;
    let shell = Path::new(program).file_name()?.to_str()?;
    let bare = args.is_empty();
    use reel_render::template::PromptPath;

    let paint = |sym: &str, wrap: &dyn Fn(&str) -> String| match p.color {
        Some(c) => format!(
            "{}{sym}{}",
            wrap(&format!("\x1b[38;2;{};{};{}m", c.r, c.g, c.b)),
            wrap("\x1b[0m"),
        ),
        None => sym.to_string(),
    };

    let mut env = Vec::new();
    let mut extra_args = Vec::new();
    match shell {
        "zsh" => {
            let path = match p.path {
                PromptPath::None => "",
                PromptPath::Short => "%1~ ",
                PromptPath::Full => "%~ ",
            };
            let sym = paint(&p.symbol, &|esc| format!("%{{{esc}%}}"));
            env.push(("PROMPT".to_string(), format!("{sym} {path}")));
            if bare {
                // No rc files: reproducible demos, and .zshrc prompt themes
                // would clobber the injected one.
                extra_args.push("-f".to_string());
            }
        }
        "bash" | "sh" | "dash" | "ksh" => {
            let path = match p.path {
                PromptPath::None => "",
                PromptPath::Short => "\\W ",
                PromptPath::Full => "\\w ",
            };
            let sym = paint(&p.symbol, &|esc| format!("\\[{esc}\\]"));
            env.push(("PS1".to_string(), format!("{sym} {path}")));
            if shell == "bash" {
                // macOS prints a "default shell is now zsh" banner into every
                // bash demo otherwise.
                env.push(("BASH_SILENCE_DEPRECATION_WARNING".to_string(), "1".to_string()));
                if bare {
                    extra_args.push("--noprofile".to_string());
                    extra_args.push("--norc".to_string());
                }
            }
        }
        // Unknown program: export PS1 anyway — harmless for non-shells,
        // picked up by anything POSIX-ish spawned inside.
        _ => {
            let sym = paint(&p.symbol, &|esc| esc.to_string());
            env.push(("PS1".to_string(), format!("{sym} ")));
        }
    }
    Some(PromptInjection { extra_args, env })
}

/// Minimal shell-style splitting: whitespace-separated, quotes respected.
fn shell_words(s: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '\'') | (None, '"') => quote = Some(c),
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if quote.is_some() {
        bail!("unclosed quote in command");
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    if words.is_empty() {
        bail!("empty command");
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names_map_to_wire_bytes() {
        assert_eq!(key_bytes("enter").unwrap(), b"\r");
        assert_eq!(key_bytes("ctrl+c").unwrap(), vec![0x03]);
        assert_eq!(key_bytes("up").unwrap(), b"\x1b[A");
        assert_eq!(key_bytes("x").unwrap(), b"x");
        assert!(key_bytes("hyperkey").is_none());
    }

    #[test]
    fn shell_words_respects_quotes() {
        assert_eq!(
            shell_words("opencode -m 'openai/gpt x' --flag").unwrap(),
            vec!["opencode", "-m", "openai/gpt x", "--flag"]
        );
        assert!(shell_words("broken 'quote").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn captures_a_scripted_session_end_to_end() {
        let dir = std::env::temp_dir().join(format!("reel-script-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let reel_path = dir.join("demo.reel");
        let text = "---\n[terminal]\ncols = 60\nrows = 12\n---\nrun \"sh\"\nwait_text \"$\" timeout 10s\ntype \"echo done-marker\"\nenter\nwait_text \"done-marker\" timeout 10s\nkey ctrl+d\n";
        std::fs::write(&reel_path, text).unwrap();
        let file = ReelFile::parse(text).unwrap();
        let cast_path = capture(&reel_path, &file, true).unwrap();
        let cast = reel_cast::Cast::load(&cast_path).unwrap();
        assert!(cast.events.iter().any(|e| e.data.contains("done-marker")));
        let meta = reel_cast::ReelMeta::load_sidecar(&cast_path).unwrap();
        assert!(meta.input_events.len() > 10, "per-char keys recorded");
        let _ = std::fs::remove_dir_all(dir);
    }
}
