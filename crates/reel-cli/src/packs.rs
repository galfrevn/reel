//! Fetching pack files from GitHub. A pack is any public repo with a
//! `templates/` (or `sounds/`) directory of TOML files; these helpers are
//! the raw-URL plumbing `template add` and `audio add` share.

use crate::net;
use anyhow::{anyhow, bail, Context, Result};

/// All `<dir>/*.toml` files of `owner/repo`, as (stem, text) pairs.
pub fn fetch_all(owner: &str, repo: &str, dir: &str) -> Result<Vec<(String, String)>> {
    let listing = net::fetch(
        &format!("https://api.github.com/repos/{owner}/{repo}/contents/{dir}"),
        &format!("listing {dir} in {owner}/{repo}…"),
    )
    .with_context(|| format!("listing {dir} in {owner}/{repo}"))?;
    let entries: Vec<serde_json::Value> =
        serde_json::from_str(&listing).context("parsing GitHub response")?;
    let mut files = Vec::new();
    for e in &entries {
        let name = e["name"].as_str().unwrap_or_default();
        let Some(stem) = name.strip_suffix(".toml") else { continue };
        let url = e["download_url"]
            .as_str()
            .ok_or_else(|| anyhow!("no download_url for {name}"))?;
        let text = net::fetch(url, &format!("downloading {name}…"))
            .with_context(|| format!("downloading {name}"))?;
        files.push((stem.to_string(), text));
    }
    if files.is_empty() {
        bail!("{owner}/{repo} has no {dir}/*.toml files");
    }
    Ok(files)
}

/// One `<dir>/<name>.toml` of `owner/repo`.
pub fn fetch_one(owner: &str, repo: &str, dir: &str, name: &str) -> Result<String> {
    let url =
        format!("https://raw.githubusercontent.com/{owner}/{repo}/HEAD/{dir}/{name}.toml");
    net::fetch(&url, &format!("downloading {name} from {owner}/{repo}…"))
        .with_context(|| format!("downloading {dir}/{name}.toml from {owner}/{repo}"))
}

/// Splits an `owner/repo[/name]` source string.
pub fn parse_source(source: &str) -> Option<(&str, &str, Option<&str>)> {
    let parts: Vec<&str> = source.split('/').filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        [owner, repo] => Some((owner, repo, None)),
        [owner, repo, name] => Some((owner, repo, Some(name))),
        _ => None,
    }
}
