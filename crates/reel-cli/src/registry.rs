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

/// The GitHub repo holding `registry/index.json` — where `publish` sends PRs.
pub const REGISTRY_REPO: &str = "galfrevn/reel";
pub const INDEX_PATH: &str = "registry/index.json";

const DEFAULT_INDEX_URL: &str =
    "https://raw.githubusercontent.com/galfrevn/reel/main/registry/index.json";

#[derive(Deserialize)]
struct Index {
    schema: u32,
    packs: Vec<Pack>,
}

/// What a pack can carry. Each kind maps to one directory in the pack repo
/// and one array in its index entry; publish/search/add are parametrized on
/// this so templates and sounds share one pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Template,
    Sound,
}

impl Kind {
    /// Directory in the pack repo and array key in the index.
    pub fn dir(self) -> &'static str {
        match self {
            Kind::Template => "templates",
            Kind::Sound => "sounds",
        }
    }
    /// The CLI command that installs one of these.
    pub fn add_cmd(self) -> &'static str {
        match self {
            Kind::Template => "reel template add",
            Kind::Sound => "reel audio add",
        }
    }
    pub fn noun(self) -> &'static str {
        match self {
            Kind::Template => "template",
            Kind::Sound => "sound",
        }
    }
}

#[derive(Deserialize)]
struct Pack {
    /// GitHub `owner/repo` holding a `templates/` (and/or `sounds/`) directory.
    repo: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    templates: Vec<Entry>,
    #[serde(default)]
    sounds: Vec<Entry>,
}

impl Pack {
    fn entries(&self, kind: Kind) -> &[Entry] {
        match kind {
            Kind::Template => &self.templates,
            Kind::Sound => &self.sounds,
        }
    }
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

/// Prints registry entries of one kind matching `query` (name, description,
/// or tag — case-insensitive substring). No query lists everything.
pub fn search(kind: Kind, query: Option<&str>) -> Result<()> {
    let index = fetch_index()?;
    let q = query.map(str::to_lowercase);
    let mut hits = 0usize;
    for pack in &index.packs {
        let matching: Vec<&Entry> = pack
            .entries(kind)
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
            println!("               {} {}/{}", kind.add_cmd(), pack.repo, t.name);
            hits += 1;
        }
    }
    if hits == 0 {
        match query {
            Some(q) => println!(
                "no {}s matching `{q}` — try `reel {} search`",
                kind.noun(),
                kind.noun().replace("sound", "audio"),
            ),
            None => println!("the registry has no {}s yet (index: {})", kind.noun(), index_url()),
        }
    }
    Ok(())
}

/// Inserts (or replaces) one entry in a raw index document. The pack is
/// matched by repo, created if absent; an entry with the same name is
/// replaced so re-publishing updates in place. Returns the new document
/// pretty-printed, ready to commit.
pub fn upsert_entry(
    index_text: &str,
    kind: Kind,
    repo: &str,
    pack_description: &str,
    entry: serde_json::Value,
) -> Result<String> {
    let mut doc: serde_json::Value =
        serde_json::from_str(index_text).context("parsing registry index")?;
    let packs = doc
        .get_mut("packs")
        .and_then(|p| p.as_array_mut())
        .ok_or_else(|| anyhow::anyhow!("index has no `packs` array"))?;

    let pack = match packs.iter_mut().find(|p| p["repo"] == repo) {
        Some(p) => p,
        None => {
            packs.push(serde_json::json!({
                "repo": repo,
                "description": pack_description,
            }));
            packs.last_mut().expect("just pushed")
        }
    };
    if pack.get(kind.dir()).is_none() {
        pack[kind.dir()] = serde_json::json!([]);
    }
    let entries = pack
        .get_mut(kind.dir())
        .and_then(|t| t.as_array_mut())
        .ok_or_else(|| anyhow::anyhow!("pack `{repo}`: `{}` is not an array", kind.dir()))?;
    match entries.iter_mut().find(|t| t["name"] == entry["name"]) {
        Some(existing) => *existing = entry,
        None => entries.push(entry),
    }
    Ok(format!("{}\n", serde_json::to_string_pretty(&doc)?))
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

    #[test]
    fn upsert_creates_pack_appends_and_replaces() {
        let base = r#"{"schema": 1, "packs": []}"#;
        let entry = serde_json::json!({"name": "neon", "description": "v1", "tags": ["dark"]});
        let one = upsert_entry(base, Kind::Template, "me/looks", "my pack", entry).unwrap();
        assert!(one.contains("me/looks") && one.contains("v1"));

        // Same name in the same pack replaces; a second name appends.
        let entry2 = serde_json::json!({"name": "neon", "description": "v2", "tags": []});
        let two = upsert_entry(&one, Kind::Template, "me/looks", "my pack", entry2).unwrap();
        assert!(two.contains("v2") && !two.contains("v1"));
        let entry3 = serde_json::json!({"name": "other", "description": "", "tags": []});
        let three = upsert_entry(&two, Kind::Template, "me/looks", "my pack", entry3).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&three).unwrap();
        assert_eq!(doc["packs"][0]["templates"].as_array().unwrap().len(), 2);
        assert_eq!(doc["packs"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn upsert_sounds_lands_in_its_own_array_of_the_same_pack() {
        let base = r#"{"schema": 1, "packs": [{"repo": "me/looks", "templates": [{"name": "neon"}]}]}"#;
        let entry = serde_json::json!({"name": "laser", "description": "pew", "tags": []});
        let out = upsert_entry(base, Kind::Sound, "me/looks", "my pack", entry).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["packs"].as_array().unwrap().len(), 1, "same pack reused");
        assert_eq!(doc["packs"][0]["templates"].as_array().unwrap().len(), 1);
        assert_eq!(doc["packs"][0]["sounds"][0]["name"], "laser");
    }
}
