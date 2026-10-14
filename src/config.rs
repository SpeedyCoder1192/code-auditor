//! Settings. Order of precedence: environment, config file, built-in defaults.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdsConfig {
    pub enabled: bool,
    /// Endpoint returning one line of text, or {"text": "..."}.
    pub url: Option<String>,
}

impl Default for AdsConfig {
    fn default() -> Self {
        Self { enabled: true, url: None }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// "auto" | "local" | "openai" | "gemini"
    pub provider: String,
    pub api_key: Option<String>,
    pub cloud_model: Option<String>,
    pub cloud_base_url: Option<String>,
    /// "ollama" | "llamacpp"
    pub local_backend: String,
    pub local_url: String,
    pub local_model: String,
    pub auto_pull: bool,
    pub chunk_lines: usize,
    pub max_file_bytes: u64,
    pub concurrency: usize,
    pub timeout_secs: u64,
    pub ads: AdsConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: "auto".into(),
            api_key: None,
            cloud_model: None,
            cloud_base_url: None,
            local_backend: "ollama".into(),
            local_url: "http://127.0.0.1:11434".into(),
            local_model: "qwen2.5-coder:7b".into(),
            auto_pull: true,
            chunk_lines: 300,
            max_file_bytes: 512 * 1024,
            concurrency: 2,
            timeout_secs: 300,
            ads: AdsConfig::default(),
        }
    }
}

fn home() -> Option<PathBuf> {
    env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")).map(PathBuf::from)
}

fn xdg(var: &str, fallback: &str) -> Option<PathBuf> {
    match env::var_os(var) {
        Some(v) if !v.is_empty() => Some(PathBuf::from(v)),
        _ => Some(home()?.join(fallback)),
    }
}

pub fn config_path() -> Option<PathBuf> {
    Some(xdg("XDG_CONFIG_HOME", ".config")?.join("code-auditor").join("config.toml"))
}

pub fn cache_dir() -> Option<PathBuf> {
    Some(xdg("XDG_CACHE_HOME", ".cache")?.join("code-auditor"))
}

pub fn load(explicit: Option<&Path>) -> Result<Config> {
    let mut cfg = match explicit {
        Some(p) => read_file(p)?,
        None => match config_path() {
            Some(p) if p.exists() => read_file(&p)?,
            _ => Config::default(),
        },
    };
    apply_env(&mut cfg);
    cfg.provider = cfg.provider.to_ascii_lowercase();
    Ok(cfg)
}

fn read_file(p: &Path) -> Result<Config> {
    let text = fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", p.display()))
}

fn var(k: &str) -> Option<String> {
    env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn apply_env(cfg: &mut Config) {
    if let Some(v) = var("AUDITOR_API_KEY") {
        cfg.api_key = Some(v);
    }
    if let Some(v) = var("AUDITOR_PROVIDER") {
        cfg.provider = v;
    }
    if let Some(v) = var("AUDITOR_MODEL") {
        cfg.cloud_model = Some(v);
    }
    if let Some(v) = var("AUDITOR_BASE_URL") {
        cfg.cloud_base_url = Some(v);
    }
    if let Some(v) = var("AUDITOR_LOCAL_URL") {
        cfg.local_url = v;
    }
    if let Some(v) = var("AUDITOR_LOCAL_MODEL") {
        cfg.local_model = v;
    }
    if let Some(v) = var("AUDITOR_AD_URL") {
        cfg.ads.url = Some(v);
    }
    if let Some(v) = var("DISABLE_ADS") {
        if matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes") {
            cfg.ads.enabled = false;
        }
    }
}
