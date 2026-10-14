//! Entry point. Picks CLI or TUI depending on whether we are attached to a terminal.

mod ads;
mod cli;
mod config;
mod engine;
mod model;
mod scan;
mod tui;

use std::{
    io::{self, IsTerminal},
    path::PathBuf,
};

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};

#[derive(Parser)]
#[command(name = "auditor", version, about = "Audit source code with a local or cloud model")]
struct Args {
    #[command(subcommand)]
    cmd: Option<Cmd>,

    /// Path to a config file (default: ~/.config/code-auditor/config.toml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scan a file or directory
    Scan(cli::ScanArgs),
}

#[tokio::main]
async fn main() {
    let code = match run().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("auditor: {e:#}");
            2
        }
    };
    std::process::exit(code);
}

async fn run() -> Result<i32> {
    let args = Args::parse();
    let cfg = config::load(args.config.as_deref())?;

    // The TUI needs a real terminal on both ends.
    let interactive = io::stdout().is_terminal() && io::stdin().is_terminal();

    match args.cmd {
        Some(Cmd::Scan(a)) => {
            // The TUI shows one tree; several paths (e.g. from a git hook) go to the CLI.
            if interactive && !a.cli && a.format.is_none() && a.paths.len() == 1 {
                tui::run(cfg, a.paths[0].clone(), a.local).await
            } else {
                cli::run(cfg, &a).await
            }
        }
        None if interactive => tui::run(cfg, PathBuf::from("."), false).await,
        None => {
            Args::command().print_help()?;
            Ok(2)
        }
    }
}
