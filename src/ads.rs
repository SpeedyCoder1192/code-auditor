//! One line of sponsor text for the TUI footer.
//!
//! Plain GET, no identifiers, no cookies. The result is cached on disk for
//! 30 minutes so startup never waits on the network. Disabled by DISABLE_ADS=1
//! or `ads.enabled = false`. The CLI never calls this module.

use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::config::{self, Config};

const TTL_SECS: u64 = 30 * 60;
const MAX_CHARS: usize = 100;
const MAX_BODY: u64 = 16 * 1024;

#[derive(Serialize, Deserialize)]
struct Cached {
    fetched_at: u64,
    text: String,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn cache_file() -> Option<PathBuf> {
    Some(config::cache_dir()?.join("ad.json"))
}

async fn read_cache() -> Option<Cached> {
    let raw = tokio::fs::read(cache_file()?).await.ok()?;
    serde_json::from_slice(&raw).ok()
}

async fn write_cache(text: &str) {
    let Some(path) = cache_file() else { return };
    if let Some(dir) = path.parent() {
        let _ = tokio::fs::create_dir_all(dir).await;
    }
    let c = Cached { fetched_at: now(), text: text.to_string() };
    if let Ok(bytes) = serde_json::to_vec(&c) {
        let _ = tokio::fs::write(path, bytes).await;
    }
}

/// First non-empty line, control characters removed, length capped.
fn clean(s: &str) -> Option<String> {
    let line = s.lines().map(str::trim).find(|l| !l.is_empty())?;
    let t: String = line.chars().filter(|c| !c.is_control()).take(MAX_CHARS).collect();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

fn url_ok(u: &str) -> bool {
    u.starts_with("https://") || u.starts_with("http://127.0.0.1") || u.starts_with("http://localhost")
}

async fn fetch(url: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .user_agent("code-auditor")
        .redirect(reqwest::redirect::Policy::limited(2))
        .build()
        .ok()?;
    let resp = client.get(url).send().await.ok()?.error_for_status().ok()?;
    if resp.content_length().is_some_and(|n| n > MAX_BODY) {
        return None;
    }
    let body = resp.text().await.ok()?;
    let text = match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(v) => v.get("text")?.as_str()?.to_string(),
        Err(_) => body,
    };
    clean(&text)
}

/// Fresh cache wins. Otherwise fetch; if that fails, fall back to a stale copy.
pub async fn get(cfg: &Config) -> Option<String> {
    if !cfg.ads.enabled {
        return None;
    }
    let cached = read_cache().await;
    if let Some(c) = &cached {
        if now().saturating_sub(c.fetched_at) < TTL_SECS {
            return Some(c.text.clone());
        }
    }
    if let Some(url) = cfg.ads.url.as_deref().filter(|u| url_ok(u)) {
        if let Some(t) = fetch(url).await {
            write_cache(&t).await;
            return Some(t);
        }
    }
    cached.map(|c| c.text)
}
