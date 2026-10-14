//! Finding and reading source files.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use walkdir::WalkDir;

const EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "jsx", "ts", "tsx", "go", "java", "kt", "c", "h", "cc", "cpp", "hpp", "cs",
    "rb", "php", "swift", "scala", "sh", "bash", "sql", "lua", "pl", "ex", "exs", "hs",
];

const SKIP_DIRS: &[&str] = &["target", "node_modules", "vendor", "dist", "build", "__pycache__"];

/// A single file path is always accepted. Directories are walked, skipping
/// hidden and build directories, files over `max_bytes`, and unknown extensions.
pub fn collect(root: &Path, max_bytes: u64) -> Result<Vec<PathBuf>> {
    let meta = std::fs::metadata(root).with_context(|| format!("cannot open {}", root.display()))?;
    if meta.is_file() {
        return Ok(vec![root.to_path_buf()]);
    }

    let walker = WalkDir::new(root).follow_links(false).into_iter().filter_entry(|e| {
        if e.depth() == 0 || !e.file_type().is_dir() {
            return true;
        }
        let name = e.file_name().to_string_lossy();
        !(name.starts_with('.') || SKIP_DIRS.contains(&&*name))
    });

    let mut out = Vec::new();
    for entry in walker.filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let ext_ok = entry
            .path()
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()));
        let size_ok = entry.metadata().map_or(true, |m| m.len() <= max_bytes);
        if ext_ok && size_ok {
            out.push(entry.into_path());
        }
    }
    out.sort();
    Ok(out)
}

pub async fn read_text(path: &Path) -> Result<String> {
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("cannot read {}", path.display()))?;
    if bytes.iter().take(8192).any(|&b| b == 0) {
        anyhow::bail!("{} looks like a binary file", path.display());
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
