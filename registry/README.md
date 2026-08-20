# reel template registry

The community template index behind `reel template search` and the
[template gallery](https://galfrevn.github.io/reel/). Deliberately just
files in a repo — no hosted infrastructure, nothing to run.

## How it works

- `index.json` lists **packs**: GitHub repos with a `templates/` directory
  of reel template TOML files. The templates themselves live in their
  authors' repos; the index only points at them.
- `reel template search [query]` fetches this index
  (override the URL with `REEL_REGISTRY_URL` for testing).
- `reel template try owner/repo/name` renders a template against the
  canonical demo cast (`crates/reel-cli/assets/demo.cast`, embedded in the
  binary) without installing it.
- `reel template add owner/repo[/name]` installs.

## Publishing a pack

The paved road — from inside your pack's git repo (any public GitHub repo):

```sh
reel template show glass > my-look.toml     # start from any builtin
reel template publish my-look.toml --tag dark --tag docs
```

`publish` validates the TOML (schema, description), renders a preview
against the canonical demo cast so you see exactly what the gallery will
show, copies the file into `templates/`, and — with `gh` installed and
authenticated — forks this repo, updates `index.json` on a branch, and opens
the PR. Without `gh` (or with `--no-pr`) it prints the exact index entry to
add manually.

The equivalent by hand:

1. Put one or more template TOML files in a `templates/` directory at the
   root of a public GitHub repo. Declare `schema = 1`.
2. Check it locally: `reel template try templates/my-look.toml`.
3. Open a PR against `index.json` adding your pack entry: repo, a one-line
   description, and one entry per template (name, description, tags).

On merge, the `gallery` workflow re-renders every template against the
canonical demo cast (`build_gallery.py`) and republishes the gallery — your
pack shows up with live previews and its install command, automatically.

Templates are declarative TOML — installing one can't execute anything.
Packs must not bundle font files (licensing); reference fonts by family
name and they resolve if the viewer has them, falling back to the system
monospace chain.
