//! Non-interactive mode. Results go to stdout with no decoration; anything
//! else (errors, the summary) goes to stderr. No ads here, ever.

use std::{fmt::Write as _, path::PathBuf, sync::Arc};

use anyhow::Result;
use serde::Serialize;
use tokio::{sync::Semaphore, task::JoinSet};

use crate::{
    config::Config,
    engine::{self, Engine},
    model::{sanitize, Finding, Severity},
    scan,
};

#[derive(clap::Args, Debug)]
pub struct ScanArgs {
    /// Files or directories to scan
    #[arg(required = true)]
    pub paths: Vec<PathBuf>,

    /// Plain output even when attached to a terminal
    #[arg(long)]
    pub cli: bool,

    /// Output format. Implies --cli
    #[arg(long, value_enum)]
    pub format: Option<Format>,

    /// Exit with 1 if a finding at or above this severity exists
    #[arg(long, value_enum, default_value = "critical")]
    pub fail_on: Severity,

    /// Never exit with 1 because of findings
    #[arg(long)]
    pub no_fail: bool,

    /// Use the local model even if an API key is set
    #[arg(long)]
    pub local: bool,

    /// Print finding details in text mode
    #[arg(short, long)]
    pub verbose: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    Text,
    Json,
}

#[derive(Serialize)]
struct Record<'a> {
    file: &'a str,
    #[serde(flatten)]
    finding: &'a Finding,
}

async fn audit_file(engine: &dyn Engine, path: &std::path::Path, chunk: usize) -> Result<Vec<Finding>> {
    let text = scan::read_text(path).await?;
    engine::audit_source(engine, &text, chunk).await
}

/// Exit codes: 0 clean, 1 findings at or above the threshold, 2 error.
pub async fn run(cfg: Config, a: &ScanArgs) -> Result<i32> {
    let engine = engine::build(&cfg, a.local)?;

    let roots = a.paths.clone();
    let max = cfg.max_file_bytes;
    let files = tokio::task::spawn_blocking(move || -> Result<Vec<PathBuf>> {
        let mut all = Vec::new();
        for root in &roots {
            all.extend(scan::collect(root, max)?);
        }
        all.sort();
        all.dedup();
        Ok(all)
    })
    .await??;
    if files.is_empty() {
        eprintln!("auditor: no source files to scan");
        return Ok(0);
    }

    let sem = Arc::new(Semaphore::new(cfg.concurrency.max(1)));
    let mut set = JoinSet::new();
    for path in files {
        let engine = engine.clone();
        let sem = sem.clone();
        let chunk = cfg.chunk_lines;
        set.spawn(async move {
            let _permit = sem.acquire_owned().await;
            let res = audit_file(engine.as_ref(), &path, chunk).await;
            (path, res)
        });
    }

    let mut results = Vec::new();
    while let Some(joined) = set.join_next().await {
        results.push(joined?);
    }
    results.sort_by(|x, y| x.0.cmp(&y.0));

    let mut all: Vec<(String, Finding)> = Vec::new();
    let mut errors = 0usize;
    let mut nfiles = 0usize;
    for (path, res) in results {
        let name = path.display().to_string();
        nfiles += 1;
        match res {
            Ok(found) => all.extend(found.into_iter().map(|f| (name.clone(), f))),
            Err(e) => {
                errors += 1;
                eprintln!("auditor: {}: {}", name, sanitize(&format!("{e:#}"), false));
            }
        }
    }

    match a.format.unwrap_or(Format::Text) {
        Format::Json => {
            let recs: Vec<Record> = all.iter().map(|(f, x)| Record { file: f, finding: x }).collect();
            println!("{}", serde_json::to_string_pretty(&recs)?);
        }
        Format::Text => {
            let mut out = String::new();
            for (file, f) in &all {
                let title = sanitize(&f.title, false).replace('\n', " ");
                let _ = writeln!(out, "{}:{}: {}: {}", file, f.line, f.severity.as_str(), title);
                if a.verbose {
                    for l in sanitize(&f.detail, false).lines() {
                        let _ = writeln!(out, "    {l}");
                    }
                }
            }
            print!("{out}");
        }
    }

    eprintln!(
        "auditor: {} finding(s) in {} file(s) via {}{}",
        all.len(),
        nfiles,
        engine.name(),
        if errors > 0 { format!(", {errors} file(s) failed") } else { String::new() }
    );

    let worst = all.iter().map(|(_, f)| f.severity).max();
    let failing = !a.no_fail && worst.is_some_and(|s| s >= a.fail_on);
    Ok(if failing {
        1
    } else if errors > 0 {
        2
    } else {
        0
    })
}
