//! OpenAI-style chat completions (also used for llama.cpp's server) and Gemini.

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;

use super::{read_json, Engine};

pub struct OpenAi {
    pub http: Client,
    pub base: String,
    pub key: Option<String>,
    pub model: String,
    /// Left out when None; some hosted models reject anything but the default.
    pub temperature: Option<f32>,
    pub label: &'static str,
}

#[async_trait]
impl Engine for OpenAi {
    fn name(&self) -> String {
        format!("{}:{}", self.label, self.model)
    }

    async fn complete(&self, system: &str, user: &str) -> Result<String> {
        let mut body = json!({
            "model": self.model,
            "response_format": { "type": "json_object" },
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
        });
        if let Some(t) = self.temperature {
            body["temperature"] = json!(t);
        }

        let mut req = self.http.post(format!("{}/chat/completions", self.base)).json(&body);
        if let Some(k) = &self.key {
            req = req.bearer_auth(k);
        }
        let resp = req.send().await.context("request failed")?;
        let v = read_json(resp).await?;
        v["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_owned)
            .context("no content in reply")
    }
}

pub struct Gemini {
    pub http: Client,
    pub base: String,
    pub key: String,
    pub model: String,
}

#[async_trait]
impl Engine for Gemini {
    fn name(&self) -> String {
        format!("gemini:{}", self.model)
    }

    async fn complete(&self, system: &str, user: &str) -> Result<String> {
        let body = json!({
            "systemInstruction": { "parts": [{ "text": system }] },
            "contents": [{ "role": "user", "parts": [{ "text": user }] }],
            "generationConfig": { "temperature": 0, "responseMimeType": "application/json" },
        });
        let resp = self
            .http
            .post(format!("{}/models/{}:generateContent", self.base, self.model))
            .header("x-goog-api-key", &self.key)
            .json(&body)
            .send()
            .await
            .context("request failed")?;
        let v = read_json(resp).await?;
        v["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .map(str::to_owned)
            .context("no content in reply")
    }
}
