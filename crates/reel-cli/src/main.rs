mod pipeline;
mod record;
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
    /// Record a terminal session to a .cast (+ .reelmeta input sidecar)
    Record {
        /// Where to write the recording
        #[arg(long, short, default_value = "session.cast")]
        out: PathBuf,
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
    /// Scaffold a new .reel file
    Init {
        /// Template to reference (minimal, glass, classic, paper)
        #[arg(default_value = "glass")]
        template: String,
        #[arg(long, short, default_value = "demo.reel")]
        out: PathBuf,
    },
    /// List built-in templates
    Templates,
    /// List available themes (alias for `theme list`)
    Themes,
    /// Manage themes
    Theme {
        #[command(subcommand)]
        action: ThemeAction,
    },
}

#[derive(Subcommand)]
enum ThemeAction {
    /// List built-in and imported themes
    List,
    /// Import a theme file (base16 .yaml, alacritty .toml/.yml, iTerm2 .itermcolors)
    Import {
        file: PathBuf,
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
        Command::Render { file, out, template, budget, scale, no_audio, quiet } => {
            pipeline::render(&file, out, template, budget, scale, no_audio, quiet)
        }
        Command::Watch { file, out, template, serve } => watch::watch(&file, out, template, serve),
        Command::Record { out, command } => record::record(&out, command),
        Command::Shot { file, at, out, template } => pipeline::shot(&file, &at, out, template),
        Command::Inspect { file } => pipeline::inspect(&file),
        Command::Init { template, out } => init(&template, &out),
        Command::Templates => {
            for name in reel_render::template::template_names() {
                let t = reel_render::template::builtin(name).unwrap();
                println!("{name:<10} {}", t.description);
            }
            Ok(())
        }
        Command::Themes | Command::Theme { action: ThemeAction::List } => {
            list_themes();
            Ok(())
        }
        Command::Theme { action: ThemeAction::Import { file, name } } => {
            themes::import(&file, name)
        }
    }
}

fn init(template: &str, out: &PathBuf) -> Result<()> {
    if reel_render::template::builtin(template).is_none() {
        bail!(
            "unknown template `{template}` (built-ins: {})",
            reel_render::template::template_names().join(", ")
        );
    }
    if out.exists() {
        bail!("{} already exists", out.display());
    }
    let content = format!(
        r#"---
[source]
cast = "session.cast"          # record with: asciinema rec session.cast -- your-tui

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
