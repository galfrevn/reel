//! Machine-readable output.
//!
//! `--json` is a global flag, so it works on every subcommand. The contract
//! an agent can rely on:
//!
//! * exactly **one** JSON document on stdout, and nothing else;
//! * `{"error": "…"}` (plus exit 1) when the command fails;
//! * progress, warnings and notes move to stderr — stdout stays parseable
//!   even when a render is chatty;
//! * nothing ever opens a viewer, a browser, or waits on stdin.
//!
//! Human output is unchanged when the flag is absent.

use std::sync::atomic::{AtomicBool, Ordering};

static JSON: AtomicBool = AtomicBool::new(false);

pub fn set(on: bool) {
    JSON.store(on, Ordering::Relaxed);
}

/// Whether `--json` is in effect. Commands consult this to swap their
/// `println!` summary for a document, and to skip anything interactive.
pub fn on() -> bool {
    JSON.load(Ordering::Relaxed)
}

/// Prints the one document. Called at most once per run, on success.
pub fn emit(value: serde_json::Value) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

/// The failure document. `{:#}` flattens anyhow's context chain into the
/// single sentence a caller can show or log.
pub fn fail(err: &anyhow::Error) {
    let doc = serde_json::json!({ "error": format!("{err:#}") });
    match serde_json::to_string_pretty(&doc) {
        Ok(s) => println!("{s}"),
        // Serializing a string can't realistically fail; if it somehow does,
        // still leave valid JSON behind rather than nothing.
        Err(_) => println!("{{\"error\": \"unprintable error\"}}"),
    }
}
