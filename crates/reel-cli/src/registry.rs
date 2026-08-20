//! `reel template search`: the federated template registry.
//!
//! The registry is a single `index.json` in the reel repo pointing at packs
//! that live in their authors' repos (Homebrew-tap model). No hosted
//! infrastructure: publishing a pack is a PR that adds an entry here, and
//! installing stays `reel template add owner/repo[/name]`.

use crate::net;
use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// Index schema this reel understands; entries with a newer schema are
/// skipped with a note rather than failing the whole search.
const INDEX_SCHEMA: u32 = 1;

const DEFAULT_INDEX_URL: &str =
    "https://raw.githubusercontent.com/galfrevn/reel/main/registry/index.json";

#[derive(Deserialize)]
struct Index {
    schema: u32,
    packs: Vec<Pack>,
}

#[derive(Deserialize)]
struct Pack {
    /// GitHub `owner/repo` holding a `templates/` directory.
    repo: String,
    #[serde(default)]
    description: String,
    templates: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    tags: Vec<String>,
}

fn index_url() -> String {
    std::env::var("REEL_REGISTRY_URL").unwrap_or_else(|_| DEFAULT_INDEX_URL.to_string())
}

fn fetch_index() -> Result<Index> {
    let url = index_url();
    let body = net::fetch(&url, "fetching template registry…")?;
    let index: Index = serde_json::from_str(&body)
        .with_context(|| format!("parsing registry index from {url}"))?;
    if index.schema > INDEX_SCHEMA {
        bail!(
            "registry index schema {} is newer than this reel understands \
             ({INDEX_SCHEMA}) — upgrade reel",
            index.schema
        );
    }
    Ok(index)
}

/// Prints registry templates matching `query` (name, description, or tag —
/// case-insensitive substring). No query lists everything.
pub fn search(query: Option<&str>) -> Result<()> {
    let index = fetch_index()?;
    let q = query.map(str::to_lowercase);
    let mut hits = 0usize;
    for pack in &index.packs {
        let matching: Vec<&Entry> = pack
            .templates
            .iter()
            .filter(|t| {
                let Some(q) = &q else { return true };
                t.name.to_lowercase().contains(q)
                    || t.description.to_lowercase().contains(q)
                    || t.tags.iter().any(|tag| tag.to_lowercase().contains(q))
                    || pack.repo.to_lowercase().contains(q)
            })
            .collect();
        if matching.is_empty() {
            continue;
        }
        if hits > 0 {
            println!();
        }
        println!("{} — {}", pack.repo, pack.description);
        for t in matching {
            let tags = if t.tags.is_empty() {
                String::new()
            } else {
                format!("  [{}]", t.tags.join(", "))
            };
            println!("  {:<12} {}{tags}", t.name, t.description);
            println!("               reel template add {}/{}", pack.repo, t.name);
            hits += 1;
        }
    }
    if hits == 0 {
        match query {
            Some(q) => println!("no templates matching `{q}` — try `reel template search`"),
            None => println!("the registry is empty (index: {})", index_url()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_parses_and_newer_schema_is_refused() {
        let good = r#"{"schema": 1, "packs": [{"repo": "a/b", "templates": [{"name": "x"}]}]}"#;
        let index: Index = serde_json::from_str(good).unwrap();
        assert_eq!(index.packs[0].templates[0].name, "x");

        let newer: Index =
            serde_json::from_str(r#"{"schema": 99, "packs": []}"#).unwrap();
        assert!(newer.schema > INDEX_SCHEMA);
    }
}
