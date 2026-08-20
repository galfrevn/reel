#!/usr/bin/env python3
"""Builds the template gallery: a static page where every template — builtin
and registry — is previewed against the same canonical demo cast.

Stdlib only. Run from the repo root:

    python3 registry/build_gallery.py --reel target/release/reel --out _site

Registry packs that live in other repos are fetched from GitHub; a pack that
fails to fetch or render is skipped with a warning rather than sinking the
whole build.
"""

import argparse
import html
import json
import pathlib
import subprocess
import sys
import tempfile
import urllib.request

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
DEMO_CAST = REPO_ROOT / "crates" / "reel-cli" / "assets" / "demo.cast"
INDEX = REPO_ROOT / "registry" / "index.json"
POSTER_AT = "9.6s"  # the diff-stat moment: prompt, colors, and output all visible


def run(reel, args):
    subprocess.run([reel, *args], check=True, capture_output=True, text=True)


def render_preview(reel, template_ref, slug, out):
    """Renders the demo cast with one template: a looping webm + png poster."""
    webm = out / "previews" / f"{slug}.webm"
    png = out / "posters" / f"{slug}.png"
    run(reel, ["render", str(DEMO_CAST), "--template", template_ref, "-o", str(webm)])
    run(reel, ["shot", str(DEMO_CAST), "--at", POSTER_AT, "--template", template_ref, "-o", str(png)])


def builtins(reel):
    """(name, description) pairs from `reel templates`, builtins only."""
    listing = subprocess.run(
        [reel, "templates"], check=True, capture_output=True, text=True
    ).stdout
    out = []
    for line in listing.splitlines():
        if not line.strip() or line.rstrip().endswith("(installed)"):
            continue
        name, _, desc = line.partition(" ")
        out.append((name.strip(), desc.strip()))
    return out


def fetch_template(repo, name):
    url = f"https://raw.githubusercontent.com/{repo}/HEAD/templates/{name}.toml"
    with urllib.request.urlopen(url, timeout=30) as r:
        return r.read().decode()


CARD = """\
<article class="card">
  <video autoplay loop muted playsinline poster="posters/{slug}.png"
         src="previews/{slug}.webm"></video>
  <div class="meta">
    <h3>{name}</h3>
    <p>{desc}</p>
    {tags}
    <button class="install" data-cmd="{cmd}" onclick="copy(this)"><code>{cmd}</code></button>
  </div>
</article>
"""

PAGE = """\
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>reel templates</title>
<style>
  :root {{
    --bg: #0d1017; --panel: #151a23; --line: #232a36;
    --fg: #e6e9ef; --dim: #8a93a5; --accent: #7dd3a0;
  }}
  * {{ box-sizing: border-box; margin: 0; }}
  body {{
    background: var(--bg); color: var(--fg);
    font: 16px/1.6 ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif;
    padding: 3rem 1.5rem 5rem;
  }}
  main {{ max-width: 1100px; margin: 0 auto; }}
  h1 {{ font-size: 1.7rem; letter-spacing: -0.02em; }}
  h1 a {{ color: inherit; text-decoration: none; }}
  h1 a:hover {{ color: var(--accent); }}
  .sub {{ color: var(--dim); margin: 0.4rem 0 0; max-width: 46rem; }}
  .sub a {{ color: var(--accent); }}
  h2 {{ font-size: 1.05rem; margin: 3rem 0 1rem; color: var(--dim);
       font-weight: 600; text-transform: uppercase; letter-spacing: 0.08em; }}
  h2 span {{ color: var(--fg); text-transform: none; letter-spacing: 0; }}
  .grid {{ display: grid; gap: 1.25rem;
          grid-template-columns: repeat(auto-fill, minmax(320px, 1fr)); }}
  .card {{ background: var(--panel); border: 1px solid var(--line);
          border-radius: 12px; overflow: hidden; }}
  .card video {{ display: block; width: 100%; aspect-ratio: 4 / 3;
                object-fit: cover; background: #000; }}
  .meta {{ padding: 0.9rem 1rem 1rem; }}
  .meta h3 {{ font-size: 1rem; font-family: ui-monospace, "SF Mono", Menlo, monospace; }}
  .meta p {{ color: var(--dim); font-size: 0.88rem; margin: 0.25rem 0 0.6rem; }}
  .tags {{ display: flex; gap: 0.35rem; flex-wrap: wrap; margin-bottom: 0.7rem; }}
  .tag {{ font-size: 0.72rem; color: var(--dim); border: 1px solid var(--line);
         border-radius: 99px; padding: 0.05rem 0.55rem; }}
  .install {{ display: block; width: 100%; text-align: left; cursor: pointer;
             background: var(--bg); color: var(--fg); border: 1px solid var(--line);
             border-radius: 8px; padding: 0.5rem 0.7rem; }}
  .install code {{ font: 0.8rem ui-monospace, "SF Mono", Menlo, monospace;
                  color: var(--accent); }}
  .install:hover {{ border-color: var(--accent); }}
  .install.copied code {{ color: var(--fg); }}
  footer {{ margin-top: 4rem; color: var(--dim); font-size: 0.85rem; }}
  footer a {{ color: var(--accent); }}
</style>
</head>
<body>
<main>
  <h1><a href="https://github.com/galfrevn/reel">reel</a> templates</h1>
  <p class="sub">Every preview renders the <em>same</em> demo recording, so
  what varies is only the template. Click a command to copy it; publish your
  own with <a href="https://github.com/galfrevn/reel/blob/main/registry/README.md">a
  pull request</a> — no accounts, no infrastructure.</p>
  {sections}
  <footer>Generated by <a href="https://github.com/galfrevn/reel/blob/main/registry/build_gallery.py">build_gallery.py</a>
  from <a href="https://github.com/galfrevn/reel/blob/main/registry/index.json">registry/index.json</a>.
  Previews are re-rendered on every registry change.</footer>
</main>
<script>
function copy(btn) {{
  navigator.clipboard.writeText(btn.dataset.cmd).then(() => {{
    const code = btn.querySelector('code'), prev = code.textContent;
    code.textContent = 'copied!';
    btn.classList.add('copied');
    setTimeout(() => {{ code.textContent = prev; btn.classList.remove('copied'); }}, 1200);
  }});
}}
</script>
</body>
</html>
"""


def card(slug, name, desc, tags, cmd):
    tag_html = ""
    if tags:
        chips = "".join(f'<span class="tag">{html.escape(t)}</span>' for t in tags)
        tag_html = f'<div class="tags">{chips}</div>'
    return CARD.format(
        slug=slug, name=html.escape(name), desc=html.escape(desc),
        tags=tag_html, cmd=html.escape(cmd),
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

    cards = []
    for name, desc in builtins(args.reel):
        print(f"rendering builtin {name}…", flush=True)
        render_preview(args.reel, name, f"builtin-{name}", out)
        cards.append(card(f"builtin-{name}", name, desc, [], f"reel init {name}"))
    sections.append(
        '<h2>Built into reel</h2><div class="grid">' + "".join(cards) + "</div>"
    )

    index = json.loads(INDEX.read_text())
    for pack in index["packs"]:
        repo = pack["repo"]
        cards = []
        for entry in pack["templates"]:
            name = entry["name"]
            slug = f'{repo.replace("/", "-")}-{name}'
            try:
                if repo == args.self_repo:
                    text = (REPO_ROOT / "templates" / f"{name}.toml").read_text()
                else:
                    text = fetch_template(repo, name)
                with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
                    f.write(text)
                    tmp = f.name
                print(f"rendering {repo}/{name}…", flush=True)
                render_preview(args.reel, tmp, slug, out)
            except Exception as e:  # noqa: BLE001 — a bad pack must not sink the site
                print(f"warning: skipping {repo}/{name}: {e}", file=sys.stderr)
                continue
            cards.append(
                card(slug, name, entry.get("description", ""), entry.get("tags", []),
                     f"reel template add {repo}/{name}")
            )
        if cards:
            title = f'{html.escape(repo)} <span>— {html.escape(pack.get("description", ""))}</span>'
            sections.append(f'<h2>{title}</h2><div class="grid">' + "".join(cards) + "</div>")

    (out / "index.html").write_text(PAGE.format(sections="\n".join(sections)))
    print(f"gallery → {out / 'index.html'}")


if __name__ == "__main__":
    main()
