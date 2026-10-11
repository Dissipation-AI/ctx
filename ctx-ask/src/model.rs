//! The model client: any OpenAI-compatible chat endpoint, over blocking HTTP (`ureq`).

use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub trait Model: Sync {
    /// One chat completion: system prompt, user message, reply text.
    fn chat(&self, system: &str, user: &str, deadline: Instant) -> Result<String, String>;
}

pub struct Endpoint {
    pub url: String,
    pub model: String,
    pub api_key: Option<String>,
}

impl Endpoint {
    /// Whether the endpoint is on this machine.
    pub fn is_local(&self) -> bool {
        let host = self.url.split("://").nth(1).unwrap_or(&self.url).split(['/', ':']).next().unwrap_or("");
        matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]") || host.ends_with(".local")
    }
}

impl Model for Endpoint {
    fn chat(&self, system: &str, user: &str, deadline: Instant) -> Result<String, String> {
        let left = deadline.saturating_duration_since(Instant::now());
        if left < Duration::from_millis(200) {
            return Err("out of time".into());
        }
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(left))
            .timeout_connect(Some(Duration::from_secs(5).min(left)))
            .http_status_as_error(false)
            .build()
            .into();
        let body = json!({
            "model": self.model,
            "temperature": 0,
            "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
            "response_format": {"type": "json_object"},
        })
        .to_string();
        let url = format!("{}/chat/completions", self.url.trim_end_matches('/'));
        let mut req = agent.post(&url).header("content-type", "application/json");
        if let Some(k) = &self.api_key {
            req = req.header("authorization", &format!("Bearer {k}"));
        }
        let mut resp = req.send(body.as_bytes()).map_err(|e| format!("can't reach the model at {}: {e}", self.url))?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
        if !(200..300).contains(&status) {
            // Some servers reject response_format: try once without it.
            if status == 400 && text.contains("response_format") {
                let body = json!({
                    "model": self.model,
                    "temperature": 0,
                    "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
                })
                .to_string();
                let mut req = agent.post(&url).header("content-type", "application/json");
                if let Some(k) = &self.api_key {
                    req = req.header("authorization", &format!("Bearer {k}"));
                }
                let mut resp =
                    req.send(body.as_bytes()).map_err(|e| format!("can't reach the model at {}: {e}", self.url))?;
                let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
                return content(&text);
            }
            return Err(format!(
                "the model at {} answered {status}: {}",
                self.url,
                text.chars().take(300).collect::<String>()
            ));
        }
        content(&text)
    }
}

fn content(text: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("the model's reply wasn't JSON: {e}"))?;
    v["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "the model's reply had no content".into())
}

/// Parse JSON leniently: the first `{…}` object in the text, tolerating code fences.
pub fn lenient_json(text: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str(text.trim()) {
        return Some(v);
    }
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(&text[start..=end]).ok()
}

/// Addresses from `{"relevant": [{"line": "C14L2"}]}` (or a bare list of strings).
pub fn json_addresses(text: &str, list: &str, field: &str) -> Vec<String> {
    let Some(v) = lenient_json(text) else { return Vec::new() };
    let items = v[list].as_array().cloned().unwrap_or_default();
    items
        .iter()
        .filter_map(|i| match i {
            Value::String(s) => Some(s.clone()),
            other => other[field].as_str().map(str::to_string),
        })
        .map(|s| s.trim().to_uppercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lenient() {
        assert_eq!(
            json_addresses("```json\n{\"relevant\": [{\"line\": \"c14l2\", \"why\": \"x\"}]}\n```", "relevant", "line"),
            vec!["C14L2"]
        );
        assert_eq!(json_addresses("{\"relevant\": [\"C1L1\"]}", "relevant", "line"), vec!["C1L1"]);
        assert!(json_addresses("nonsense", "relevant", "line").is_empty());
    }

    #[test]
    fn local_endpoints() {
        for (u, local) in [
            ("http://localhost:8080/v1", true),
            ("http://127.0.0.1:11434/v1", true),
            ("https://api.example.com/v1", false),
        ] {
            assert_eq!(Endpoint { url: u.into(), model: String::new(), api_key: None }.is_local(), local, "{u}");
        }
    }
}
