mod pipeline;
mod record;
mod script;
mod suggest;
mod templates;
mod themes;
mod watch;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "reel",
    about = "Your terminal demo, edited like video.",
    version,
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Render a .reel edit file (or a raw .cast with default styling)
    Render {
        /// Path to a .reel file, or a .cast for a quick default render
        file: PathBuf,
        /// Output file; extension picks the format (.gif, .webm, .png, .txt)
        #[arg(long, short)]
        out: Option<PathBuf>,
        /// Template override (minimal, glass, classic, paper)
        #[arg(long)]
        template: Option<String>,
        /// Size budget like 800kb or 2mb; the encoder degrades to fit
        #[arg(long)]
        budget: Option<String>,
        /// Supersampling scale override (1-4)
        #[arg(long)]
        scale: Option<u32>,
        /// Canvas aspect ratio like 16:9 (grows the canvas, never crops)
        #[arg(long)]
        aspect: Option<String>,
        /// Exact canvas size like 1920x1080 (solves the font size to fit)
        #[arg(long)]
        size: Option<String>,
        /// Render silent even if the .reel configures audio (webm only)
        #[arg(long)]
        no_audio: bool,
        /// Suppress progress output
        #[arg(long, short)]
        quiet: bool,
    },
    /// Re-render on save; optionally serve a live browser preview
    Watch {
        file: PathBuf,
        #[arg(long, short)]
        out: Option<PathBuf>,
        #[arg(long)]
        template: Option<String>,
        /// Serve a live preview at http://127.0.0.1:PORT/
        #[arg(long, value_name = "PORT", num_args = 0..=1, default_missing_value = "4171")]
        serve: Option<u16>,
    },
    /// Execute a script-mode .reel (capture the program live) and render it
    Run {
        /// A .reel with script ops (run/type/key/wait_text/…), no [source]
        file: PathBuf,
        /// Only capture; skip rendering
        #[arg(long)]
        no_render: bool,
        /// Suppress progress output
        #[arg(long, short)]
        quiet: bool,
    },
    /// Record a terminal session to a .cast (+ .reelmeta input sidecar)
    Record {
        /// Where to write the recording
        #[arg(long, short, default_value = "session.cast")]
        out: PathBuf,
        /// PTY size like 120x40 (defaults to your terminal's size)
        #[arg(long, value_name = "COLSxROWS")]
        size: Option<String>,
        /// Command to record, after `--` (defaults to your shell)
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Render a single frame to PNG
    Shot {
        file: PathBuf,
        /// Output timestamp (3s, 1200ms, 1:24, end, end-2s)
        #[arg(long, default_value = "0s")]
        at: String,
        #[arg(long, short)]
        out: Option<PathBuf>,
        #[arg(long)]
        template: Option<String>,
    },
    /// Summarize a .reel file: timeline, markers, size estimate
    Inspect { file: PathBuf },
    /// Analyze a recording and draft the edit script (trims, speed ramps)
    Suggest {
        /// A .cast recording
        file: PathBuf,
        /// Write a complete .reel file instead of printing the ops
        #[arg(long, value_name = "FILE.reel")]
        write: Option<PathBuf>,
        /// Template for the written file
        #[arg(long, default_value = "glass")]
        template: String,
    },
    /// Scaffold a new .reel file
    Init {
        /// Template to reference (minimal, glass, classic, paper)
        #[arg(default_value = "glass")]
        template: String,
        #[arg(long, short, default_value = "demo.reel")]
        out: PathBuf,
    },
    /// List available templates (alias for `template list`)
    Templates,
    /// Manage templates
    Template {
        #[command(subcommand)]
        action: TemplateAction,
    },
    /// List available themes (alias for `theme list`)
    Themes,
    /// Manage themes
    Theme {
        #[command(subcommand)]
        action: ThemeAction,
    },
}

#[derive(Subcommand)]
enum TemplateAction {
    /// List built-in and installed templates
    List,
    /// Print a template as TOML (a starting point for your own)
    Show { name: String },
    /// Install templates from a .toml file or a GitHub repo (owner/repo[/name])
    Add { source: String },
}

#[derive(Subcommand)]
enum ThemeAction {
    /// List built-in and imported themes
    List,
    /// Import a theme file (base16 .yaml, alacritty .toml/.yml, iTerm2 .itermcolors)
    Import {
        /// Theme file to import; omit when using --from
        file: Option<PathBuf>,
        /// Import straight from an installed terminal: iterm, kitty, ghostty
        #[arg(long, value_name = "TERMINAL", conflicts_with = "file")]
        from: Option<String>,
        /// Name to install it under (defaults to the scheme/file name)
        #[arg(long)]
        name: Option<String>,
    },
}

fn list_themes() {
    for name in reel_render::theme::theme_names() {
        println!("{name}");
    }
    for name in reel_render::theme::user_theme_names() {
        println!("{name} (imported)");
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Render { file, out, template, budget, scale, aspect, size, no_audio, quiet } => {
            pipeline::render(&file, out, template, budget, scale, aspect, size, no_audio, quiet)
        }
        Command::Watch { file, out, template, serve } => watch::watch(&file, out, template, serve),
        Command::Run { file, no_render, quiet } => {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;
            let parsed = reel_format::ReelFile::parse(&text)?;
            if parsed.config.source.is_some() {
                bail!(
                    "{} is an edit-mode file ([source].cast) — use `reel render`",
                    file.display()
                );
            }
            let cast = script::capture(&file, &parsed, quiet)?;
            if no_render {
                println!("{}", cast.display());
                return Ok(());
            }
            pipeline::render_with_source(&file, &cast, quiet)
        }
        Command::Record { out, size, command } => record::record(&out, size, command),
        Command::Shot { file, at, out, template } => pipeline::shot(&file, &at, out, template),
        Command::Inspect { file } => pipeline::inspect(&file),
        Command::Suggest { file, write, template } => {
            suggest::suggest(&file, write.as_deref(), &template)
        }
        Command::Init { template, out } => init(&template, &out),
        Command::Templates | Command::Template { action: TemplateAction::List } => {
            templates::list();
            Ok(())
        }
        Command::Template { action: TemplateAction::Show { name } } => templates::show(&name),
        Command::Template { action: TemplateAction::Add { source } } => templates::add(&source),
        Command::Themes | Command::Theme { action: ThemeAction::List } => {
            list_themes();
            Ok(())
        }
        Command::Theme { action: ThemeAction::Import { file, from, name } } => match (file, from) {
            (Some(f), None) => themes::import(&f, name),
            (None, Some(t)) => themes::import_from_terminal(&t, name),
            _ => bail!("pass a theme file or --from iterm|kitty|ghostty"),
        },
    }
}

fn init(template: &str, out: &PathBuf) -> Result<()> {
    if reel_render::template::lookup(template).is_none() {
        bail!(
            "unknown template `{template}` (available: {})",
            reel_render::template::template_names().join(", ")
        );
    }
    if out.exists() {
        bail!("{} already exists", out.display());
    }
    let content = format!(
        r#"---
[source]
cast = "session.cast"          # record with: reel record -- your-tui

[template]
name = "{template}"

[output]
file = "demo.gif"
# budget = "800kb"             # let the encoder target a size
---

# Timeline ops use the recording's own clock (source time).
# trim    2s..end
# cut     19s..23s
# speed   5x from 8s to 34s
# caption "What's happening here" at 4s for 2.5s
# zoom    1.8x at (30,10) from 36s to 41s
# freeze  last 1.5s
"#
    );
    std::fs::write(out, content).with_context(|| format!("writing {}", out.display()))?;
    println!("wrote {}", out.display());
    println!("next: record a cast, then `reel render {}`", out.display());
    Ok(())
}
