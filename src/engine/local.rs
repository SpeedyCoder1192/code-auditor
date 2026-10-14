//! Ollama over its local HTTP API. Pulls the model on first use if it is missing.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;
use tokio::sync::OnceCell;

use super::{read_json, Engine};
use crate::config::Config;

pub struct Ollama {
    http: Client,
    url: String,
    model: String,
    auto_pull: bool,
    ready: OnceCell<()>,
}

impl Ollama {
    pub fn new(http: Client, cfg: &Config) -> Self {
        Self {
            http,
            url: cfg.local_url.trim_end_matches('/').to_string(),
            model: cfg.local_model.clone(),
            auto_pull: cfg.auto_pull,
            ready: OnceCell::new(),
        }
    }

    async fn ensure_model(&self) -> Result<()> {
        self.ready
            .get_or_try_init(|| async {
                let resp = self
                    .http
                    .get(format!("{}/api/tags", self.url))
                    .send()
                    .await
                    .with_context(|| {
                        format!(
                            "cannot reach Ollama at {} (start it with `ollama serve`, or set AUDITOR_API_KEY to use a cloud model)",
                            self.url
                        )
                    })?;
                let tags = read_json(resp).await?;

                let present = tags["models"].as_array().is_some_and(|m| {
                    m.iter()
                        .filter_map(|x| x["name"].as_str())
                        .any(|n| same_model(n, &self.model))
                });
                if present {
                    return Ok::<(), anyhow::Error>(());
                }
                if !self.auto_pull {
                    bail!("model '{}' is not installed (run `ollama pull {}`)", self.model, self.model);
                }

                // Blocks until the download finishes; can take a while on a slow link.
                let resp = self
                    .http
                    .post(format!("{}/api/pull", self.url))
                    .json(&json!({ "model": self.model, "name": self.model, "stream": false }))
                    .timeout(Duration::from_secs(3600))
                    .send()
                    .await
                    .context("model pull failed")?;
                read_json(resp).await.context("model pull failed")?;
                Ok(())
            })
            .await
            .map(|_| ())
    }
}

fn same_model(have: &str, want: &str) -> bool {
    have == want || (!want.contains(':') && have == format!("{want}:latest"))
}

#[async_trait]
impl Engine for Ollama {
    fn name(&self) -> String {
        format!("ollama:{}", self.model)
    }

    async fn complete(&self, system: &str, user: &str) -> Result<String> {
        self.ensure_model().await?;
        let body = json!({
            "model": self.model,
            "stream": false,
            "format": "json",
            "options": { "temperature": 0, "num_ctx": 8192 },
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
        });
        let resp = self
            .http
            .post(format!("{}/api/chat", self.url))
            .json(&body)
            .send()
            .await
            .context("request to Ollama failed")?;
        let v = read_json(resp).await?;
        v["message"]["content"]
            .as_str()
            .map(str::to_owned)
            .context("empty reply from Ollama")
    }
}
