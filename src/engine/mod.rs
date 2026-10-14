//! Backends behind one trait, the audit prompt, and reply parsing.

pub mod cloud;
pub mod local;

use std::{sync::Arc, time::Duration};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::Value;

use crate::{
    config::Config,
    model::{sanitize, Finding},
};

#[async_trait]
pub trait Engine: Send + Sync {
    fn name(&self) -> String;
    async fn complete(&self, system: &str, user: &str) -> Result<String>;
}

const SYSTEM: &str = r##"You review source code for real defects: security vulnerabilities, memory safety problems, logic errors, resource leaks. Skip style comments.

You get numbered source lines. Reply with one JSON object and nothing else:

{"findings":[{"line":<int>,"end_line":<int or null>,"severity":"critical|high|medium|low|info","title":"<short>","detail":"<why it is wrong and how it could be abused>","fix":{"start_line":<int>,"end_line":<int>,"new_text":"<replacement for those lines, no line numbers>"}}]}

Line numbers are the ones in the left column. "fix" may be null. Use "critical" only for directly exploitable problems: injection, auth bypass, remote code execution, live secrets in source. If the code is fine, reply {"findings":[]}."##;

pub fn http_client(timeout_secs: u64) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .user_agent("code-auditor")
        .build()?)
}

fn trim_url(s: &str) -> String {
    s.trim_end_matches('/').to_string()
}

/// A cloud key switches the engine to the cloud. No key means local.
pub fn build(cfg: &Config, force_local: bool) -> Result<Arc<dyn Engine>> {
    let key = cfg.api_key.clone().filter(|k| !k.is_empty());
    let cloud_provider = matches!(cfg.provider.as_str(), "openai" | "gemini");
    if cloud_provider && key.is_none() && !force_local {
        bail!("provider is '{}' but AUDITOR_API_KEY is not set", cfg.provider);
    }

    let http = http_client(cfg.timeout_secs)?;

    if force_local || cfg.provider == "local" || key.is_none() {
        let engine: Arc<dyn Engine> = match cfg.local_backend.as_str() {
            "llamacpp" | "llama.cpp" | "llama-server" => Arc::new(cloud::OpenAi {
                http,
                base: format!("{}/v1", trim_url(&cfg.local_url)),
                key: None,
                model: cfg.local_model.clone(),
                temperature: Some(0.0),
                label: "llama.cpp",
            }),
            _ => Arc::new(local::Ollama::new(http, cfg)),
        };
        return Ok(engine);
    }

    let key = key.context("missing API key")?;
    let model = cfg.cloud_model.clone();
    let gemini = cfg.provider == "gemini"
        || (cfg.provider == "auto" && model.as_deref().is_some_and(|m| m.starts_with("gemini")));

    if gemini {
        Ok(Arc::new(cloud::Gemini {
            http,
            base: trim_url(
                cfg.cloud_base_url
                    .as_deref()
                    .unwrap_or("https://generativelanguage.googleapis.com/v1beta"),
            ),
            key,
            model: model.unwrap_or_else(|| "gemini-2.5-flash".into()),
        }))
    } else {
        Ok(Arc::new(cloud::OpenAi {
            http,
            base: trim_url(cfg.cloud_base_url.as_deref().unwrap_or("https://api.openai.com/v1")),
            key: Some(key),
            model: model.unwrap_or_else(|| "gpt-4o-mini".into()),
            temperature: None,
            label: "openai",
        }))
    }
}

/// Parse an HTTP reply, keeping a short piece of the body in the error.
pub(crate) async fn read_json(resp: reqwest::Response) -> Result<Value> {
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        let snippet: String = text.chars().take(300).collect();
        bail!("HTTP {}: {}", status.as_u16(), snippet);
    }
    serde_json::from_str(&text).context("reply was not JSON")
}

/// Audit a whole file, `chunk_lines` lines per request. Line numbers in the
/// prompt are absolute, so findings need no adjusting afterwards.
pub async fn audit_source(engine: &dyn Engine, source: &str, chunk_lines: usize) -> Result<Vec<Finding>> {
    let lines: Vec<&str> = source.lines().collect();
    let size = chunk_lines.max(20);
    let mut out = Vec::new();

    for (ci, chunk) in lines.chunks(size).enumerate() {
        let start = ci * size + 1;
        let end = start + chunk.len() - 1;
        let numbered = chunk
            .iter()
            .enumerate()
            .map(|(i, l)| format!("{:>5} | {}", start + i, l))
            .collect::<Vec<_>>()
            .join("\n");
        let user = format!("Source lines {start}-{end}:\n{numbered}");

        let mut parsed = None;
        for _ in 0..2 {
            let raw = engine.complete(SYSTEM, &user).await?;
            if let Some(f) = parse_findings(&raw, start, chunk) {
                parsed = Some(f);
                break;
            }
        }
        out.extend(parsed.context("model did not return valid JSON")?);
    }

    out.sort_by_key(|f| f.line);
    Ok(out)
}

fn parse_findings(raw: &str, start: usize, chunk: &[&str]) -> Option<Vec<Finding>> {
    let end = start + chunk.len() - 1;
    let a = raw.find('{')?;
    let b = raw.rfind('}')?;
    if b < a {
        return None;
    }
    let v: Value = serde_json::from_str(&raw[a..=b]).ok()?;
    let items = v.get("findings")?.as_array()?;

    let mut out = Vec::new();
    for item in items {
        let Ok(mut f) = serde_json::from_value::<Finding>(item.clone()) else { continue };
        if f.line == 0 {
            f.line = start;
        }
        if f.line < start || f.line > end {
            continue;
        }
        f.title = sanitize(&f.title, false).replace('\n', " ");
        if f.title.trim().is_empty() {
            f.title = "Untitled finding".into();
        }
        f.detail = sanitize(&f.detail, false);
        if let Some(e) = &mut f.fix {
            if e.start_line < start || e.end_line > end || e.end_line < e.start_line {
                f.fix = None;
            } else {
                let text = strip_line_numbers(&sanitize(&e.new_text, true));
                e.new_text = reindent(&text, chunk[e.start_line - start]);
            }
        }
        out.push(f);
    }
    Some(out)
}

/// Some models drop the base indentation of the lines they replace. If the
/// replacement starts flush left but the original first line is indented,
/// indent every non-empty line by that much, keeping relative indentation.
fn reindent(text: &str, original: &str) -> String {
    let indent = &original[..original.len() - original.trim_start().len()];
    let flush = text.lines().find(|l| !l.trim().is_empty()).is_some_and(|l| !l.starts_with([' ', '\t']));
    if indent.is_empty() || !flush {
        return text.to_string();
    }
    text.lines()
        .map(|l| if l.trim().is_empty() { l.to_string() } else { format!("{indent}{l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Some models copy the "  12 | " gutter into their replacement. Remove it
/// only when every non-empty line has it.
fn strip_line_numbers(s: &str) -> String {
    let gutter = |l: &str| -> Option<usize> {
        let at = l.find(" | ")?;
        let left = l[..at].trim();
        (!left.is_empty() && left.chars().all(|c| c.is_ascii_digit())).then_some(at + 3)
    };
    let non_empty: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    if non_empty.is_empty() || !non_empty.iter().all(|l| gutter(l).is_some()) {
        return s.to_string();
    }
    s.lines()
        .map(|l| match gutter(l) {
            Some(n) => l[n..].to_string(),
            None => l.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::reindent;

    #[test]
    fn reindent_restores_dropped_indent() {
        assert_eq!(reindent("a()\n\nif x:\n    b()", "    old()"), "    a()\n\n    if x:\n        b()");
    }

    #[test]
    fn reindent_leaves_indented_text_alone() {
        assert_eq!(reindent("    a()", "    old()"), "    a()");
        assert_eq!(reindent("a()", "old()"), "a()");
    }
}
