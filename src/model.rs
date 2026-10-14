//! Findings, patch application and text cleanup.

use std::fmt::Write as _;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    #[default]
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "critical" | "crit" | "blocker" => Self::Critical,
            "high" | "severe" | "error" => Self::High,
            "medium" | "moderate" | "warning" | "warn" => Self::Medium,
            "low" | "minor" => Self::Low,
            _ => Self::Info,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

impl<'de> Deserialize<'de> for Severity {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(v.as_str().map(Severity::parse).unwrap_or_default())
    }
}

/// Replace lines `start_line..=end_line` (1-based) with `new_text`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edit {
    #[serde(deserialize_with = "lenient_usize")]
    pub start_line: usize,
    #[serde(deserialize_with = "lenient_usize")]
    pub end_line: usize,
    pub new_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    #[serde(default, deserialize_with = "lenient_usize")]
    pub line: usize,
    #[serde(default, deserialize_with = "lenient_opt_usize")]
    pub end_line: Option<usize>,
    #[serde(default)]
    pub severity: Severity,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default, deserialize_with = "lenient_edit")]
    pub fix: Option<Edit>,
}

// Models are sloppy about types. Take what we can, zero out the rest.
fn lenient_usize<'de, D: Deserializer<'de>>(d: D) -> Result<usize, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Number(n) => n.as_u64().unwrap_or(0) as usize,
        Value::String(s) => s.trim().parse().unwrap_or(0),
        _ => 0,
    })
}

fn lenient_opt_usize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<usize>, D::Error> {
    let n = lenient_usize(d)?;
    Ok(if n == 0 { None } else { Some(n) })
}

fn lenient_edit<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Edit>, D::Error> {
    let v = Option::<Value>::deserialize(d)?;
    Ok(v.and_then(|v| serde_json::from_value(v).ok()))
}

/// Strip control characters from model output so it cannot drive the terminal.
/// `keep_tabs` is for code that ends up in a patch.
pub fn sanitize(s: &str, keep_tabs: bool) -> String {
    s.chars()
        .filter_map(|c| match c {
            '\n' => Some('\n'),
            '\t' if keep_tabs => Some('\t'),
            '\t' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect()
}

/// Apply non-overlapping edits. Edits that are out of range or overlap an
/// earlier one are skipped.
pub fn apply_edits(source: &str, edits: &[&Edit]) -> String {
    let eol = if source.contains("\r\n") { "\r\n" } else { "\n" };
    let trailing = source.ends_with('\n');
    let mut lines: Vec<String> = source.lines().map(str::to_owned).collect();

    let mut sorted: Vec<&Edit> = edits
        .iter()
        .copied()
        .filter(|e| e.start_line >= 1 && e.end_line >= e.start_line && e.end_line <= lines.len())
        .collect();
    sorted.sort_by_key(|e| e.start_line);

    let mut kept: Vec<&Edit> = Vec::new();
    let mut last_end = 0;
    for e in sorted {
        if e.start_line > last_end {
            last_end = e.end_line;
            kept.push(e);
        }
    }

    // Back to front so earlier line numbers stay valid.
    for e in kept.iter().rev() {
        let new: Vec<String> = e.new_text.lines().map(str::to_owned).collect();
        lines.splice(e.start_line - 1..e.end_line, new);
    }

    let mut out = lines.join(eol);
    if trailing && !out.is_empty() {
        out.push_str(eol);
    }
    out
}

pub fn unified_diff(path: &str, old: &str, new: &str) -> String {
    let mut out = String::new();
    let diff = similar::TextDiff::from_lines(old, new);
    let _ = write!(
        out,
        "{}",
        diff.unified_diff()
            .context_radius(2)
            .header(&format!("a/{path}"), &format!("b/{path}"))
    );
    out
}
