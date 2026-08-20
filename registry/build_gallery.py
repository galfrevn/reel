#!/usr/bin/env python3
"""Builds the template gallery: a static page where every template — builtin
and registry — is previewed against the same canonical demo cast.

Stdlib only. Run from the repo root:

    python3 registry/build_gallery.py --reel target/release/reel --out _site

The page shell lives in registry/site/ (template.html, style.css, app.js,
fonts) — this script renders previews, builds the cards, and injects them
plus a JSON blob (used by the detail view) into the template.

Registry packs that live in other repos are fetched from GitHub; a pack that
fails to fetch or render is skipped with a warning rather than sinking the
whole build.
"""

import argparse
import html
import json
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.request

try:
    import tomllib
except ModuleNotFoundError:  # Python < 3.11: previews keep the recorded prompt
    tomllib = None

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
SITE = pathlib.Path(__file__).resolve().parent / "site"
DEMO_CAST = REPO_ROOT / "crates" / "reel-cli" / "assets" / "demo.cast"
INDEX = REPO_ROOT / "registry" / "index.json"
LOGO = REPO_ROOT / "documentation" / "assets" / "logo.svg"
POSTER_AT = "9.6s"  # the diff-stat moment: prompt, colors, and output all visible


def run(reel, args):
    subprocess.run([reel, *args], check=True, capture_output=True, text=True)


def capture(reel, args):
    return subprocess.run(
        [reel, *args], check=True, capture_output=True, text=True
    ).stdout


def render_preview(reel, template_ref, slug, out, cast=DEMO_CAST):
    """Renders the demo cast with one template: a looping webm + png poster."""
    webm = out / "previews" / f"{slug}.webm"
    png = out / "posters" / f"{slug}.png"
    run(reel, ["render", str(cast), "--template", template_ref, "-o", str(webm)])
    run(reel, ["shot", str(cast), "--at", POSTER_AT, "--template", template_ref, "-o", str(png)])


# The demo cast's recorded prompt, as it appears in output events.
RECORDED_PROMPT = "\x1b[1;32m❯\x1b[0m "


def branded_demo_cast(toml_text):
    """The demo cast with the template's `[prompt]` swapped in — or None when
    the template doesn't brand the prompt. `reel run` injects prompts into the
    shell it spawns; a recorded cast keeps its own, so the gallery swaps the
    recorded one by hand to preview each template the way `reel run` shows it."""
    if tomllib is None:
        return None
    try:
        prompt = tomllib.loads(toml_text).get("prompt") or {}
    except tomllib.TOMLDecodeError:
        return None
    symbol = prompt.get("symbol")
    if not symbol:
        return None
    m = re.fullmatch(r"#([0-9a-fA-F]{6})[0-9a-fA-F]{0,2}", prompt.get("color") or "")
    if m:
        r, g, b = (int(m.group(1)[i:i + 2], 16) for i in (0, 2, 4))
        symbol = f"\x1b[38;2;{r};{g};{b}m{symbol}\x1b[0m"
    path = {"short": "reel ", "full": "~/reel "}.get(prompt.get("path"), "")
    branded = f"{symbol} {path}"
    lines = DEMO_CAST.read_text().splitlines()
    out = [lines[0]]
    for line in lines[1:]:
        if not line.strip():
            continue
        event = json.loads(line)
        if event[1] == "o":
            event[2] = event[2].replace(RECORDED_PROMPT, branded)
        out.append(json.dumps(event, ensure_ascii=False))
    return "\n".join(out) + "\n"


def cast_for(toml_text):
    """Path to the cast a template should preview with."""
    text = branded_demo_cast(toml_text)
    if text is None:
        return DEMO_CAST
    with tempfile.NamedTemporaryFile("w", suffix=".cast", delete=False) as f:
        f.write(text)
        return pathlib.Path(f.name)


def builtins(reel):
    """(name, description) pairs from `reel templates`, builtins only."""
    out = []
    for line in capture(reel, ["templates"]).splitlines():
        if not line.strip() or line.rstrip().endswith("(installed)"):
            continue
        name, _, desc = line.partition(" ")
        out.append((name.strip(), desc.strip()))
    return out


def fetch_pack_file(repo, dirname, name):
    url = f"https://raw.githubusercontent.com/{repo}/HEAD/{dirname}/{name}.toml"
    with urllib.request.urlopen(url, timeout=30) as r:
        return r.read().decode()


PLAY_ICONS = """\
<svg class="ic-play" viewBox="0 0 16 16" width="15" height="15" fill="currentColor" aria-hidden="true"><path d="M4 2.5v11l9-5.5z"/></svg>
<svg class="ic-pause" viewBox="0 0 16 16" width="15" height="15" fill="currentColor" aria-hidden="true"><path d="M4 2.5h3v11H4zm5 0h3v13H9z"/></svg>"""

CARD = """\
<article class="card" data-kind="template" data-slug="{slug}" data-tags="{tags_attr}" data-search="{search}" style="--i:{i}">
  <button class="preview" aria-label="View {name} details">
    <video muted loop playsinline preload="none" poster="posters/{slug}.png" data-src="previews/{slug}.webm"></video>
  </button>
  <div class="meta">
    <div class="meta-row">
      <h3 class="name">{name}</h3>
      <span class="tags">{tag_pills}</span>
    </div>
    <p class="desc">{desc}</p>
    <button class="install" data-cmd="{cmd}"><code>{cmd}</code><span class="copy-ic" aria-hidden="true"></span></button>
  </div>
</article>
"""

SOUND_CARD = """\
<article class="card card-sound" data-kind="sound" data-slug="{slug}" data-tags="{tags_attr}" data-search="{search}" style="--i:{i}">
  <div class="sound-stage">
    <button class="play" data-audio="sounds/{slug}.wav" aria-label="Play {name}">
{play_icons}
    </button>
    <div class="eq" aria-hidden="true">{eq_bars}</div>
    <span class="dur"></span>
  </div>
  <div class="meta">
    <div class="meta-row">
      <h3 class="name">{name}</h3>
      <span class="tags">{tag_pills}</span>
    </div>
    <p class="desc">{desc}</p>
    <button class="install" data-cmd="{cmd}"><code>{cmd}</code><span class="copy-ic" aria-hidden="true"></span></button>
  </div>
</article>
"""

SECTION = """\
<section class="pack">
  <div class="pack-head">
    <h2>{title}</h2>
    {desc}<span class="pack-count">{count}</span>
  </div>
  <div class="grid">
{cards}
  </div>
</section>
"""


def tag_pills(tags):
    return "".join(
        f'<button class="tag" data-tag="{html.escape(t)}">{html.escape(t)}</button>'
        for t in tags
    )


def search_attr(name, desc, tags):
    return html.escape(" ".join([name, desc, *tags]).lower())


def card(kind, slug, name, desc, tags, cmd, i):
    tmpl = CARD if kind == "template" else SOUND_CARD
    return tmpl.format(
        slug=slug, name=html.escape(name), desc=html.escape(desc),
        tags_attr=html.escape(" ".join(tags)), tag_pills=tag_pills(tags),
        search=search_attr(name, desc, tags), cmd=html.escape(cmd),
        i=i % 8, play_icons=PLAY_ICONS, eq_bars="<i></i>" * 18,
    )


def section(title, desc, cards):
    desc_html = f'<span class="pack-desc">{html.escape(desc)}</span>' if desc else ""
    return SECTION.format(
        title=html.escape(title), desc=desc_html, count=len(cards),
        cards="\n".join(cards),
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--reel", required=True, help="path to the reel binary")
    ap.add_argument("--out", required=True, help="output directory for the site")
    ap.add_argument(
        "--self-repo", default="galfrevn/reel",
        help="pack repo whose templates load from the local templates/ dir",
    )
    args = ap.parse_args()

    out = pathlib.Path(args.out)
    (out / "previews").mkdir(parents=True, exist_ok=True)
    (out / "posters").mkdir(parents=True, exist_ok=True)

    sections = []
    items = []  # feeds the detail view: name, tags, install command, TOML source

    cards = []
    for name, desc in builtins(args.reel):
        print(f"rendering builtin {name}…", flush=True)
        slug = f"builtin-{name}"
        cmd = f"reel init {name}"
        toml = capture(args.reel, ["template", "show", name])
        render_preview(args.reel, name, slug, out, cast=cast_for(toml))
        items.append({"slug": slug, "kind": "template", "name": name,
                      "desc": desc, "tags": [], "cmd": cmd, "toml": toml})
        cards.append(card("template", slug, name, desc, [], cmd, len(cards)))
    sections.append(section("Built into reel", "", cards))

    def pack_file(repo, dirname, name):
        if repo == args.self_repo:
            return (REPO_ROOT / dirname / f"{name}.toml").read_text()
        return fetch_pack_file(repo, dirname, name)

    index = json.loads(INDEX.read_text())
    for pack in index["packs"]:
        repo = pack["repo"]
        cards = []
        for entry in pack.get("templates", []):
            name = entry["name"]
            slug = f'{repo.replace("/", "-")}-{name}'
            try:
                text = pack_file(repo, "templates", name)
                with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
                    f.write(text)
                    tmp = f.name
                print(f"rendering {repo}/{name}…", flush=True)
                render_preview(args.reel, tmp, slug, out, cast=cast_for(text))
            except Exception as e:  # noqa: BLE001 — a bad pack must not sink the site
                print(f"warning: skipping {repo}/{name}: {e}", file=sys.stderr)
                continue
            desc, tags = entry.get("description", ""), entry.get("tags", [])
            cmd = f"reel template add {repo}/{name}"
            items.append({"slug": slug, "kind": "template", "name": name,
                          "desc": desc, "tags": tags, "cmd": cmd, "toml": text})
            cards.append(card("template", slug, name, desc, tags, cmd, len(cards)))
        for entry in pack.get("sounds", []):
            name = entry["name"]
            slug = f'{repo.replace("/", "-")}-{name}'
            try:
                text = pack_file(repo, "sounds", name)
                with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
                    f.write(text)
                    tmp = f.name
                (out / "sounds").mkdir(exist_ok=True)
                print(f"synthesizing {repo}/{name}…", flush=True)
                run(args.reel, ["audio", "try", tmp, "--out", str(out / "sounds" / f"{slug}.wav")])
            except Exception as e:  # noqa: BLE001
                print(f"warning: skipping sound {repo}/{name}: {e}", file=sys.stderr)
                continue
            desc, tags = entry.get("description", ""), entry.get("tags", [])
            cmd = f"reel audio add {repo}/{name}"
            items.append({"slug": slug, "kind": "sound", "name": name,
                          "desc": desc, "tags": tags, "cmd": cmd, "toml": text})
            cards.append(card("sound", slug, name, desc, tags, cmd, len(cards)))
        if cards:
            sections.append(section(repo, pack.get("description", ""), cards))

    n_templates = sum(1 for i in items if i["kind"] == "template")
    n_sounds = sum(1 for i in items if i["kind"] == "sound")

    page = (SITE / "template.html").read_text()
    page = page.replace("{{T_COUNT}}", str(n_templates))
    page = page.replace("{{S_COUNT}}", str(n_sounds))
    page = page.replace("{{SECTIONS}}", "\n".join(sections))
    # "</" would terminate the inline <script> block early if left unescaped.
    page = page.replace("{{DATA}}", json.dumps(items).replace("</", "<\\/"))
    (out / "index.html").write_text(page)

    for asset in ("style.css", "app.js"):
        shutil.copy(SITE / asset, out / asset)
    shutil.copytree(SITE / "fonts", out / "fonts", dirs_exist_ok=True)
    shutil.copy(LOGO, out / "logo.svg")

    print(f"gallery → {out / 'index.html'}")


if __name__ == "__main__":
    main()
