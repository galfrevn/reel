//! `reel watch`: re-render on save, with an optional built-in live-preview
//! server.
//!
//! The whole point of the capture/render split is that this loop is fast:
//! emulation is milliseconds and the glyph cache stays warm between renders,
//! so a theme or zoom tweak lands in well under a second. A parse error
//! prints and keeps watching — never exits mid-session.

use anyhow::{anyhow, Context, Result};
use notify::{RecursiveMode, Watcher};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::pipeline;

/// Latest rendered output shared with the preview server.
#[derive(Default)]
pub struct Preview {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    /// Bumped on every successful render; SSE clients reload when it moves.
    pub generation: u64,
    pub last_error: Option<String>,
}

pub fn watch(
    path: &Path,
    out: Option<PathBuf>,
    template: Option<String>,
    serve: Option<u16>,
) -> Result<()> {
    let path = path
        .canonicalize()
        .with_context(|| format!("resolving {}", path.display()))?;
    let preview: Arc<Mutex<Preview>> = Arc::default();

    if let Some(port) = serve {
        let state = preview.clone();
        std::thread::spawn(move || serve_preview(port, state));
        eprintln!("preview: http://127.0.0.1:{port}/");
    }

    // Initial render (errors are non-fatal in watch mode).
    let mut cache = pipeline::WatchCache::default();
    let mut cast_path = render_once(&path, &out, &template, &preview, &mut cache);

    let (tx, rx) = mpsc::channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            use notify::EventKind::*;
            if matches!(ev.kind, Create(_) | Modify(_) | Remove(_)) {
                let _ = tx.send(());
            }
        }
    })?;

    // Watch the parent directories, not the files: editors replace files by
    // rename, which breaks per-file watches.
    let mut watched: Vec<PathBuf> = Vec::new();
    let ensure_watched = |watcher: &mut dyn Watcher, watched: &mut Vec<PathBuf>, p: &Path| {
        if let Some(dir) = p.parent() {
            let dir = dir.to_path_buf();
            if !watched.contains(&dir) {
                if watcher.watch(&dir, RecursiveMode::NonRecursive).is_ok() {
                    watched.push(dir);
                }
            }
        }
    };
    ensure_watched(&mut watcher, &mut watched, &path);
    if let Some(c) = &cast_path {
        ensure_watched(&mut watcher, &mut watched, c);
    }

    eprintln!("watching {} — edit and save to re-render (ctrl+c to stop)", path.display());

    // Track mtimes so unrelated files in the same directory don't retrigger.
    let mut last_render = Instant::now();
    loop {
        rx.recv().map_err(|_| anyhow!("watcher channel closed"))?;
        // Debounce: editors emit bursts of events per save.
        while rx.recv_timeout(Duration::from_millis(120)).is_ok() {}
        if relevant_changed(&path, cast_path.as_deref(), last_render) {
            let started = Instant::now();
            cast_path = render_once(&path, &out, &template, &preview, &mut cache);
            if let Some(c) = &cast_path {
                ensure_watched(&mut watcher, &mut watched, c);
            }
            last_render = started;
        }
    }
}

fn relevant_changed(reel: &Path, cast: Option<&Path>, since: Instant) -> bool {
    // Instant vs SystemTime: compare mtime against wall-clock now minus the
    // elapsed time since the last render.
    let cutoff = SystemTime::now() - since.elapsed();
    let newer = |p: &Path| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .map(|t| t >= cutoff)
            .unwrap_or(true)
    };
    newer(reel) || cast.map(newer).unwrap_or(false)
}

/// Renders once, updating the preview state; returns the cast path (for
/// watching) when the file parsed far enough to know it.
fn render_once(
    path: &Path,
    out: &Option<PathBuf>,
    template: &Option<String>,
    preview: &Arc<Mutex<Preview>>,
    cache: &mut pipeline::WatchCache,
) -> Option<PathBuf> {
    let started = Instant::now();
    match pipeline::render_for_watch(path, out.clone(), template.clone(), cache) {
        Ok(result) => {
            let mut p = preview.lock().unwrap();
            p.content_type = match result.extension.as_str() {
                "png" => "image/png",
                _ => "image/gif",
            };
            p.bytes = result.bytes;
            p.generation += 1;
            p.last_error = None;
            eprintln!(
                "rendered {} ({}) in {:.2}s",
                result.out_path.display(),
                pipeline::human_size(p.bytes.len() as u64),
                started.elapsed().as_secs_f32()
            );
            Some(result.cast_path)
        }
        Err(e) => {
            let msg = format!("{e:#}");
            eprintln!("watch: {msg} — waiting for changes");
            let mut p = preview.lock().unwrap();
            p.last_error = Some(msg);
            p.generation += 1;
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Minimal preview server: /, /out, /events (SSE). Local tool, std-only.
// ---------------------------------------------------------------------------

fn serve_preview(port: u16, state: Arc<Mutex<Preview>>) {
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("preview server failed to bind port {port}: {e}");
            return;
        }
    };
    for stream in listener.incoming().flatten() {
        let state = state.clone();
        std::thread::spawn(move || {
            let _ = handle_client(stream, state);
        });
    }
}

fn handle_client(mut stream: TcpStream, state: Arc<Mutex<Preview>>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    let path = path.split('?').next().unwrap_or("/");
    // Drain headers.
    let mut line = String::new();
    while reader.read_line(&mut line)? > 2 {
        line.clear();
    }

    match path {
        "/out" => {
            let p = state.lock().unwrap();
            write_response(&mut stream, 200, p.content_type, &p.bytes)
        }
        "/events" => {
            stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n",
            )?;
            let mut seen = state.lock().unwrap().generation;
            loop {
                std::thread::sleep(Duration::from_millis(200));
                let (generation, error) = {
                    let p = state.lock().unwrap();
                    (p.generation, p.last_error.clone())
                };
                if generation != seen {
                    seen = generation;
                    let payload = match error {
                        Some(e) => format!("event: error\ndata: {}\n\n", e.replace('\n', " ")),
                        None => "event: reload\ndata: ok\n\n".to_string(),
                    };
                    if stream.write_all(payload.as_bytes()).is_err() {
                        return Ok(()); // client gone
                    }
                } else if stream.write_all(b": keep-alive\n\n").is_err() {
                    return Ok(());
                }
            }
        }
        "/" => write_response(&mut stream, 200, "text/html; charset=utf-8", PAGE.as_bytes()),
        _ => write_response(&mut stream, 404, "text/plain", b"not found"),
    }
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = if status == 200 { "OK" } else { "Not Found" };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}

const PAGE: &str = r#"<!doctype html>
<meta charset="utf-8">
<title>reel watch</title>
<style>
  body { margin: 0; min-height: 100vh; display: grid; place-items: center;
         background: #101014; font: 13px/1.5 ui-monospace, monospace; color: #e6e6eb; }
  img { max-width: 96vw; max-height: 90vh; }
  #err { position: fixed; left: 0; right: 0; bottom: 0; padding: 10px 16px;
         background: #b3261e; color: #fff; display: none; white-space: pre-wrap; }
</style>
<img id="demo" src="/out" alt="reel output">
<div id="err"></div>
<script>
  const es = new EventSource('/events');
  const img = document.getElementById('demo');
  const err = document.getElementById('err');
  es.addEventListener('reload', () => {
    err.style.display = 'none';
    img.src = '/out?' + Date.now();
  });
  es.addEventListener('error', (e) => {
    if (e.data) { err.textContent = e.data; err.style.display = 'block'; }
  });
</script>
"#;
