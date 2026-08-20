# reel registry

The community index behind `reel template search`, `reel audio search`, and
the [gallery](https://galfrevn.github.io/reel/). Deliberately just files in
a repo — no hosted infrastructure, nothing to run.

## How it works

- `index.json` lists **packs**: GitHub repos with a `templates/` and/or
  `sounds/` directory of reel TOML files. The files themselves live in
  their authors' repos; the index only points at them.
- `reel template search [query]` / `reel audio search [query]` fetch this
  index (override the URL with `REEL_REGISTRY_URL` for testing).
- `reel template try owner/repo/name` renders a template against the
  canonical demo cast (`crates/reel-cli/assets/demo.cast`, embedded in the
  binary) without installing it. `reel audio try` auditions a sound the
  same way.
- `reel template add owner/repo[/name]` / `reel audio add owner/repo[/name]`
  install.

Templates are **all-in-one**: colors travel inside the template. A template
can embed its palette as an inline `[theme]` table, and `publish` does this
automatically when your template references a theme you imported locally —
installers see exactly your look with no separate theme install.

## Publishing

The paved road — from inside your pack's git repo (any public GitHub repo):

```sh
reel template show glass > my-look.toml     # start from any builtin
reel template publish my-look.toml --tag dark --tag docs

reel audio show chime > my-sound.toml       # sounds work the same way
reel audio publish my-sound.toml --tag chime --tag success
```

`publish` validates the TOML (schema, description), previews it — a render
against the canonical demo cast for templates, a synthesized WAV audition
for sounds — copies the file into `templates/` (or `sounds/`), and — with
`gh` installed and authenticated — forks this repo, updates `index.json` on
a branch, and opens the PR. Without `gh` (or with `--no-pr`) it prints the
exact index entry to add manually.

The equivalent by hand:

1. Put one or more TOML files in a `templates/` (or `sounds/`) directory at
   the root of a public GitHub repo. Declare `schema = 1`.
2. Check it locally: `reel template try templates/my-look.toml`, or
   `reel audio try sounds/my-sound.toml`.
3. Open a PR against `index.json` adding your pack entry: repo, a one-line
   description, and one entry per file (name, description, tags) in the
   pack's `templates` or `sounds` array.

On merge, the `gallery` workflow re-renders every template against the
canonical demo cast and synthesizes every sound (`build_gallery.py`), then
republishes the gallery — your pack shows up with live previews and its
install command, automatically.

Templates and sounds are declarative TOML — installing one can't execute
anything. Sound recipes are pure synthesis parameters (no audio files), and
their values are range-checked on install. Packs must not bundle font files
(licensing); reference fonts by family name and they resolve if the viewer
has them, falling back to the system monospace chain.
