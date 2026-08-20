# reel template registry

The community template index behind `reel template search`. Deliberately
just files in a repo — no hosted infrastructure, nothing to run.

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

1. Put one or more template TOML files in a `templates/` directory at the
   root of a public GitHub repo. Start from any builtin:
   `reel template show glass > templates/my-look.toml`. Declare `schema = 1`.
2. Check it locally: `reel template try templates/my-look.toml`.
3. Open a PR against this file adding your pack entry: repo, a one-line
   description, and one entry per template (name, description, tags).

Templates are declarative TOML — installing one can't execute anything.
Packs must not bundle font files (licensing); reference fonts by family
name and they resolve if the viewer has them, falling back to the system
monospace chain.
