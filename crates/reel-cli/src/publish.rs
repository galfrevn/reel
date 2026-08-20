//! `reel template publish` / `reel audio publish`: the paved road from a
//! local TOML to a registry PR. Validates, previews, scaffolds the pack's
//! directory, and — when `gh` is available — opens the index PR without
//! leaving the terminal. Every step degrades to printed instructions rather
//! than blocking on missing tooling.

use crate::registry::Kind;
use crate::{net, registry, templates};
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use reel_render::{template, theme};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Publishes a sound recipe: validate, audition, scaffold `sounds/`, PR.
pub fn publish_sound(file: &Path, tags: &[String], no_pr: bool, no_preview: bool) -> Result<()> {
    let text = std::fs::read_to_string(file)
        .with_context(|| format!("reading {}", file.display()))?;
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("sound");
    let s = reel_audio::sound_from_toml(&text, stem).map_err(|e| anyhow!("invalid sound: {e}"))?;
    if !text.lines().any(|l| l.trim_start().starts_with("schema")) {
        bail!(
            "add `schema = {}` at the top of {} — published sounds must \
             declare the schema they were written against",
            reel_audio::SOUND_SCHEMA,
            file.display()
        );
    }
    if s.description.is_empty() {
        bail!(
            "add a `description = \"…\"` to {} — it's what search results \
             and the gallery show",
            file.display()
        );
    }
    if reel_audio::recipe(&s.name).is_some() {
        bail!(
            "`{}` is a built-in sound name — built-ins always win at \
             resolution, so rename yours before publishing",
            s.name
        );
    }
    println!("✓ `{}` is a valid schema-{} sound", s.name, reel_audio::SOUND_SCHEMA);

    // Audition: hear exactly what installers will hear.
    if !no_preview {
        crate::audio::try_sound(&file.to_string_lossy(), None)?;
    }

    scaffold_and_publish(Kind::Sound, &s.name, &s.description, tags, &text, file, no_pr)
}

/// Publishes a template: validate, embed a custom theme, preview, PR.
pub fn publish(file: &Path, tags: &[String], no_pr: bool, no_preview: bool) -> Result<()> {
    // 1. Validate: publishable means schema-stamped, described, and portable.
    let text = std::fs::read_to_string(file)
        .with_context(|| format!("reading {}", file.display()))?;
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("template");
    let mut t = template::from_toml(&text, stem)
        .map_err(|e| anyhow!("invalid template: {e}"))?;
    if !text.lines().any(|l| l.trim_start().starts_with("schema")) {
        bail!(
            "add `schema = {}` at the top of {} — published templates must \
             declare the schema they were written against",
            template::SCHEMA,
            file.display()
        );
    }
    if t.description.is_empty() {
        bail!(
            "add a `description = \"…\"` to {} — it's what search results \
             and the gallery show",
            file.display()
        );
    }
    // A published template must be self-contained: a custom theme the
    // author has locally gets embedded as an inline `[theme]` palette so
    // installers see exactly the author's look.
    let mut publish_text = text.clone();
    if t.theme_colors.is_none() && theme::builtin(&t.theme).is_none() {
        match theme::lookup(&t.theme) {
            Some(colors) => {
                t.theme_colors = Some(colors);
                publish_text = template::to_toml(&t);
                println!("✓ embedded theme `{}` as an inline palette", t.theme);
            }
            None => println!(
                "warning: theme `{}` is not builtin and not installed here — \
                 installers fall back to the default theme",
                t.theme
            ),
        }
    }
    let name = t.name.clone();
    println!("✓ `{name}` is a valid schema-{} template", template::SCHEMA);

    // 2. Preview: see exactly what the gallery will show before shipping it.
    if !no_preview {
        let out = templates::render_demo_preview(&file.to_string_lossy(), &name)?;
        println!("✓ preview rendered → {}", out.display());
        templates::open_preview(&out);
    }

    scaffold_and_publish(Kind::Template, &name, &t.description, tags, &publish_text, file, no_pr)
}

/// The shared back half of publish: scaffold the pack directory, update the
/// index (in place, by hand, or via a PR through `gh`).
fn scaffold_and_publish(
    kind: Kind,
    name: &str,
    description: &str,
    tags: &[String],
    publish_text: &str,
    src: &Path,
    no_pr: bool,
) -> Result<()> {
    // The file must live at <dir>/<name>.toml of a GitHub repo for
    // `add`/`try` raw URLs to resolve.
    let root = git_root().ok_or_else(|| {
        anyhow!(
            "publish runs inside the git repo that will host the pack — \
             create one, add a remote on GitHub, and re-run from there"
        )
    })?;
    let dest = root.join(kind.dir()).join(format!("{name}.toml"));
    let canonical_src = src.canonicalize()?;
    if canonical_src != dest || std::fs::read_to_string(&dest).ok().as_deref() != Some(publish_text)
    {
        std::fs::create_dir_all(dest.parent().expect("pack dir has a parent"))?;
        std::fs::write(&dest, publish_text)?;
        println!("✓ scaffolded {}", rel(&dest, &root).display());
    }

    let pack_repo = origin_github_repo(&root).ok_or_else(|| {
        anyhow!(
            "no GitHub `origin` remote found in {} — push the pack repo to \
             GitHub first so raw {} URLs resolve",
            root.display(),
            kind.noun()
        )
    })?;
    println!("✓ pack repo: {pack_repo}");
    if !clean_in_git(&root, &dest) {
        println!(
            "! {} isn't committed & pushed yet — do that before the PR \
             merges, or installs will 404",
            rel(&dest, &root).display()
        );
    }

    let entry = serde_json::json!({
        "name": name,
        "description": description,
        "tags": tags,
    });

    // Publishing from the registry repo itself: the index is right here.
    if pack_repo == registry::REGISTRY_REPO {
        let index_path = root.join(registry::INDEX_PATH);
        let index = std::fs::read_to_string(&index_path)
            .with_context(|| format!("reading {}", index_path.display()))?;
        let pack_desc = format!("Pack from {pack_repo}");
        std::fs::write(
            &index_path,
            registry::upsert_entry(&index, kind, &pack_repo, &pack_desc, entry)?,
        )?;
        println!(
            "✓ updated {} in place — commit and push, the gallery re-renders \
             on merge",
            registry::INDEX_PATH
        );
        return Ok(());
    }

    if no_pr || !gh_ready() {
        if !no_pr {
            println!("`gh` not found or not authenticated — manual route:");
        }
        println!(
            "add this entry to your pack's `{}` array in {} of {} (a PR \
             through the GitHub UI works fine):\n{}",
            kind.dir(),
            registry::INDEX_PATH,
            registry::REGISTRY_REPO,
            serde_json::to_string_pretty(&entry)?
        );
        return Ok(());
    }

    open_registry_pr(kind, &pack_repo, name, entry)
}

/// Drives `gh` through fork → sync → branch → index edit → PR. Each step is
/// one API call; any failure surfaces with enough context to finish by hand.
fn open_registry_pr(
    kind: Kind,
    pack_repo: &str,
    name: &str,
    entry: serde_json::Value,
) -> Result<()> {
    let login = gh(&["api", "user", "--jq", ".login"], "checking GitHub login…")?;
    let login = login.trim();
    let (registry_owner, registry_name) =
        registry::REGISTRY_REPO.split_once('/').expect("owner/repo constant");
    let default_branch = gh(
        &["api", &format!("repos/{}", registry::REGISTRY_REPO), "--jq", ".default_branch"],
        "reading registry default branch…",
    )?;
    let default_branch = default_branch.trim();

    // The repo the branch lands in: the registry itself for its owner,
    // otherwise the user's fork (created/synced on the fly).
    let branch = format!("reel-{}-{name}", kind.noun());
    let (work_repo, pr_head) = if login == registry_owner {
        (registry::REGISTRY_REPO.to_string(), branch.clone())
    } else {
        gh(
            &["repo", "fork", registry::REGISTRY_REPO, "--clone=false"],
            "ensuring a fork exists…",
        )?;
        let fork = format!("{login}/{registry_name}");
        gh(&["repo", "sync", &fork], "syncing the fork with upstream…")
            .with_context(|| format!("syncing {fork} — sync it manually and re-run"))?;
        (fork, format!("{login}:{branch}"))
    };

    let base_sha = gh(
        &["api", &format!("repos/{work_repo}/git/ref/heads/{default_branch}"), "--jq", ".object.sha"],
        "reading base commit…",
    )?;
    let base_sha = base_sha.trim();
    // Create the branch, or force-move it if a previous publish left one.
    let created = gh(
        &[
            "api", "-X", "POST", &format!("repos/{work_repo}/git/refs"),
            "-f", &format!("ref=refs/heads/{branch}"), "-f", &format!("sha={base_sha}"),
        ],
        "creating branch…",
    );
    if created.is_err() {
        gh(
            &[
                "api", "-X", "PATCH", &format!("repos/{work_repo}/git/refs/heads/{branch}"),
                "-f", &format!("sha={base_sha}"), "-F", "force=true",
            ],
            "reusing existing branch…",
        )?;
    }
    println!("✓ branch {work_repo}@{branch}");

    let index_json = gh(
        &["api", &format!("repos/{work_repo}/contents/{}?ref={branch}", registry::INDEX_PATH)],
        "fetching the registry index…",
    )?;
    let file: serde_json::Value = serde_json::from_str(&index_json)?;
    let b64 = file["content"].as_str().unwrap_or_default().replace(['\n', ' '], "");
    let blob_sha = file["sha"].as_str().unwrap_or_default().to_string();
    let current = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(b64)?)?;

    let pack_desc = format!("Pack from {pack_repo}");
    let updated = registry::upsert_entry(&current, kind, pack_repo, &pack_desc, entry)?;
    let updated_b64 = base64::engine::general_purpose::STANDARD.encode(updated);
    gh(
        &[
            "api", "-X", "PUT", &format!("repos/{work_repo}/contents/{}", registry::INDEX_PATH),
            "-f", &format!("message=registry: add {} {pack_repo}/{name}", kind.noun()),
            "-f", &format!("content={updated_b64}"),
            "-f", &format!("branch={branch}"),
            "-f", &format!("sha={blob_sha}"),
        ],
        "updating the index…",
    )?;
    println!("✓ {} updated on {branch}", registry::INDEX_PATH);

    let title = format!("registry: add {} {pack_repo}/{name}", kind.noun());
    let try_hint = match kind {
        Kind::Template => format!("Try it before merging: `reel template try {pack_repo}/{name}`"),
        Kind::Sound => format!(
            "Hear it before merging: `reel audio add {pack_repo}/{name} && reel audio try {name}`"
        ),
    };
    let body = format!(
        "Adds {} `{name}` from `{pack_repo}` to the registry.\n\n\
         {try_hint}\n\n\
         Opened by `reel {} publish`.",
        kind.noun(),
        match kind {
            Kind::Template => "template",
            Kind::Sound => "audio",
        },
    );
    let pr = gh(
        &[
            "api", "-X", "POST", &format!("repos/{}/pulls", registry::REGISTRY_REPO),
            "-f", &format!("title={title}"), "-f", &format!("head={pr_head}"),
            "-f", &format!("base={default_branch}"), "-f", &format!("body={body}"),
            "--jq", ".html_url",
        ],
        "opening the pull request…",
    );
    match pr {
        Ok(url) => println!("✓ PR opened: {}", url.trim()),
        // Most likely a PR for this head is already open — find and report it.
        Err(e) => {
            let existing = gh(
                &[
                    "api",
                    &format!("repos/{}/pulls?head={}", registry::REGISTRY_REPO, pr_head),
                    "--jq", ".[0].html_url",
                ],
                "checking for an existing PR…",
            ).unwrap_or_default();
            let existing = existing.trim().to_string();
            if existing.is_empty() || existing == "null" {
                return Err(e.context("opening the PR"));
            }
            println!("✓ PR already open (branch updated): {existing}");
        }
    }
    Ok(())
}

/// Runs `gh` behind a spinner, returning stdout. Errors carry stderr.
fn gh(args: &[&str], what: &str) -> Result<String> {
    let pb = net::spinner(what.to_string());
    let out = Command::new("gh").args(args).output();
    pb.finish_and_clear();
    let out = out.context("running gh — is it installed?")?;
    if !out.status.success() {
        bail!(
            "gh {} failed: {}",
            args.first().copied().unwrap_or(""),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn gh_ready() -> bool {
    Command::new("gh")
        .args(["auth", "status"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn git_root() -> Option<PathBuf> {
    let out = Command::new("git").args(["rev-parse", "--show-toplevel"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

/// `owner/repo` from the origin remote, if it points at GitHub. Handles
/// https, ssh, and git@ forms.
fn origin_github_repo(root: &Path) -> Option<String> {
    let out = Command::new("git")
        .args(["-C", &root.to_string_lossy(), "remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    parse_github_repo(&url)
}

fn parse_github_repo(url: &str) -> Option<String> {
    let rest = url
        .split_once("github.com")
        .map(|(_, r)| r.trim_start_matches([':', '/']))?;
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let mut parts = rest.split('/');
    let owner = parts.next().filter(|s| !s.is_empty())?;
    let repo = parts.next().filter(|s| !s.is_empty())?;
    Some(format!("{owner}/{repo}"))
}

fn clean_in_git(root: &Path, path: &Path) -> bool {
    Command::new("git")
        .args(["-C", &root.to_string_lossy(), "status", "--porcelain", &path.to_string_lossy()])
        .output()
        .map(|o| o.status.success() && o.stdout.is_empty())
        .unwrap_or(false)
}

fn rel<'a>(path: &'a Path, root: &Path) -> &'a Path {
    path.strip_prefix(root).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_urls_parse_in_all_remote_forms() {
        for url in [
            "https://github.com/me/looks.git",
            "https://github.com/me/looks",
            "git@github.com:me/looks.git",
            "ssh://git@github.com/me/looks.git",
        ] {
            assert_eq!(parse_github_repo(url).as_deref(), Some("me/looks"), "{url}");
        }
        assert_eq!(parse_github_repo("https://gitlab.com/me/looks"), None);
    }
}
