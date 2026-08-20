//! Tiny HTTP helper shared by the registry and template installer, plus the
//! spinner every network wait hides behind.

use anyhow::{anyhow, Result};
use indicatif::{ProgressBar, ProgressStyle};
use std::io::Read;
use std::time::Duration;

/// A steady-tick spinner for anything that leaves the machine (or renders
/// long enough to feel like it). Callers finish it with `finish_and_clear`
/// or let `fetch` do so.
pub fn spinner(msg: impl Into<String>) -> ProgressBar {
    let pb = ProgressBar::new_spinner().with_message(msg.into());
    pb.set_style(
        ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .expect("static template")
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "✓"]),
    );
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

/// GET a URL (4 MiB cap) behind a spinner labelled `what`.
pub fn fetch(url: &str, what: &str) -> Result<String> {
    let pb = spinner(what.to_string());
    let result = fetch_quiet(url);
    pb.finish_and_clear();
    result
}

pub fn fetch_quiet(url: &str) -> Result<String> {
    let mut response = ureq::get(url)
        .header("User-Agent", "reel-cli")
        .call()
        .map_err(|e| anyhow!("GET {url}: {e}"))?;
    let mut body = String::new();
    response
        .body_mut()
        .as_reader()
        .take(4 << 20)
        .read_to_string(&mut body)?;
    Ok(body)
}
