//! A small client for the Anthropic Messages API (`POST /v1/messages`), used by the director and
//! the reviewer: one user turn of text and JPEG images, a JSON object back.
//!
//! Requests use adaptive thinking with an explicit effort, a cached system prompt (the photo
//! skill), and server-side refusal fallbacks on the models that support them. Retryable failures
//! (429, 5xx, 529, network) are retried twice.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use log::{debug, warn};
use serde_json::{Value, json};

use crate::error::{ApiError, ApiResult};
use crate::net::{neterr, read_limited};

const API_VERSION: &str = "2023-06-01";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// Non-streaming requests: 16k leaves room for thinking plus a JSON reply.
pub const MAX_TOKENS: u32 = 16_000;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_RESPONSE_BYTES: usize = 8 << 20;
const RETRIES: u32 = 2;

/// One piece of the user turn.
pub enum Part {
    Text(String),
    /// A JPEG image.
    Jpeg(Vec<u8>),
}

pub struct Claude {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    pub model: String,
    effort: String,
}

/// Models that take `thinking: {type: "adaptive"}` and `output_config.effort`.
fn supports_adaptive(model: &str) -> bool {
    [
        "claude-opus-5",
        "claude-fable-5",
        "claude-mythos-5",
        "claude-sonnet-5",
        "claude-haiku-5",
        "claude-opus-4-6",
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-sonnet-4-6",
    ]
    .iter()
    .any(|p| model.starts_with(p))
}

/// Models that take `fallbacks: "default"` (server-side refusal fallback) on the Claude API.
fn supports_default_fallback(model: &str) -> bool {
    ["claude-opus-5-5", "claude-opus-5", "claude-fable-5-1", "claude-sonnet-5-5"].contains(&model)
}

impl Claude {
    pub fn new(http: reqwest::Client, base_url: String, api_key: String, model: String, effort: String) -> Claude {
        Claude { http, base_url: base_url.trim_end_matches('/').to_string(), api_key, model, effort }
    }

    /// The request body for `system` + one user turn of `parts`.
    pub fn request_body(&self, system: &str, parts: &[Part]) -> Value {
        let content: Vec<Value> = parts
            .iter()
            .map(|p| match p {
                Part::Text(t) => json!({ "type": "text", "text": t }),
                Part::Jpeg(bytes) => json!({
                    "type": "image",
                    "source": { "type": "base64", "media_type": "image/jpeg", "data": STANDARD.encode(bytes) },
                }),
            })
            .collect();
        let mut body = json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "system": [{ "type": "text", "text": system, "cache_control": { "type": "ephemeral" } }],
            "messages": [{ "role": "user", "content": content }],
        });
        if supports_adaptive(&self.model) {
            body["thinking"] = json!({ "type": "adaptive" });
            body["output_config"] = json!({ "effort": self.effort });
        }
        if supports_default_fallback(&self.model) {
            body["fallbacks"] = json!("default");
        }
        body
    }

    /// Send the request and return the JSON object Claude replied with.
    pub async fn json_reply(&self, what: &str, system: &str, parts: &[Part]) -> ApiResult<Value> {
        let body = serde_json::to_vec(&self.request_body(system, parts)).map_err(|e| ApiError::internal(e.to_string()))?;
        let text = self.send(what, body).await?;
        extract_json(&text).ok_or_else(|| {
            debug!("{what}: unparsable reply: {}", crate::net::snippet(text.as_bytes()));
            ApiError::upstream(format!("Claude's {what} reply was not a JSON object"))
        })
    }

    async fn send(&self, what: &str, body: Vec<u8>) -> ApiResult<String> {
        let url = format!("{}/v1/messages", self.base_url);
        let mut attempt = 0u32;
        loop {
            let mut req = self
                .http
                .post(&url)
                .timeout(REQUEST_TIMEOUT)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", API_VERSION)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.clone());
            if supports_default_fallback(&self.model) {
                req = req.header("anthropic-beta", FALLBACK_BETA);
            }
            let (retryable, err, wait) = match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let retry_after = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.trim().parse::<u64>().ok())
                        .map(|s| Duration::from_secs(s.min(20)));
                    let bytes = read_limited(resp, MAX_RESPONSE_BYTES).await.map_err(|e| ApiError::upstream(format!("Anthropic API: {e}")))?;
                    if status.is_success() {
                        return reply_text(what, &bytes);
                    }
                    let (kind, message) = api_error(&bytes);
                    let code = status.as_u16();
                    let retryable = code == 408 || code == 409 || code == 429 || code >= 500;
                    let msg = match code {
                        401 | 403 => format!("the Anthropic API rejected the key ({kind})"),
                        _ => format!("Anthropic API error {code} ({kind}): {message}"),
                    };
                    (retryable, msg, retry_after)
                }
                Err(e) => (true, format!("Anthropic API: {}", neterr(e)), None),
            };
            if !retryable || attempt >= RETRIES {
                warn!("{what}: {err}");
                return Err(ApiError::upstream(err));
            }
            attempt += 1;
            let wait = wait.unwrap_or(Duration::from_millis(1000 * u64::from(attempt * attempt)));
            debug!("{what}: {err}; retrying in {wait:?}");
            tokio::time::sleep(wait).await;
        }
    }
}

/// `(error.type, error.message)` of an API error body.
fn api_error(bytes: &[u8]) -> (String, String) {
    let v: Value = serde_json::from_slice(bytes).unwrap_or(Value::Null);
    let e = v.get("error");
    let kind = e.and_then(|e| e.get("type")).and_then(Value::as_str).unwrap_or("error").to_string();
    let message = e.and_then(|e| e.get("message")).and_then(Value::as_str).map(|m| crate::net::snippet(m.as_bytes())).unwrap_or_default();
    (kind, message)
}

/// The text of a successful Messages response, after checking why it stopped.
fn reply_text(what: &str, bytes: &[u8]) -> ApiResult<String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|e| ApiError::upstream(format!("Anthropic API sent invalid JSON: {e}")))?;
    match v.get("stop_reason").and_then(Value::as_str) {
        Some("refusal") => {
            let category = v.get("stop_details").and_then(|d| d.get("category")).and_then(Value::as_str).unwrap_or("unspecified");
            return Err(ApiError::upstream(format!("Claude declined the {what} request (category: {category})")));
        }
        Some("max_tokens") => return Err(ApiError::upstream(format!("Claude's {what} reply was cut off (max_tokens)"))),
        _ => {}
    }
    let text: String = v
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    if text.trim().is_empty() {
        return Err(ApiError::upstream(format!("Claude's {what} reply had no text")));
    }
    Ok(text)
}

/// The JSON object in `text`: the whole text, a fenced block, or the outermost `{…}`.
pub fn extract_json(text: &str) -> Option<Value> {
    let t = text.trim();
    let parse = |s: &str| serde_json::from_str::<Value>(s.trim()).ok().filter(Value::is_object);
    if let Some(v) = parse(t) {
        return Some(v);
    }
    if let Some(start) = t.find("```") {
        let after = t.get(start + 3..)?;
        let after = after.strip_prefix("json").unwrap_or(after);
        if let Some(end) = after.find("```")
            && let Some(v) = after.get(..end).and_then(parse)
        {
            return Some(v);
        }
    }
    let (start, end) = (t.find('{')?, t.rfind('}')?);
    if end <= start {
        return None;
    }
    t.get(start..=end).and_then(parse)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_extraction() {
        assert_eq!(extract_json(r#"{"a":1}"#), Some(json!({"a":1})));
        assert_eq!(extract_json("```json\n{\"a\":2}\n```"), Some(json!({"a":2})));
        assert_eq!(extract_json("Here you go: {\"a\":3} hope it helps"), Some(json!({"a":3})));
        assert_eq!(extract_json("no json here"), None);
        assert_eq!(extract_json("[1,2]"), None);
        assert_eq!(extract_json("} {"), None);
    }

    #[test]
    fn request_shape() {
        let c = Claude::new(reqwest::Client::new(), "http://x/".into(), "k".into(), "claude-opus-5-5".into(), "medium".into());
        let b = c.request_body("sys", &[Part::Text("hi".into()), Part::Jpeg(vec![1, 2, 3])]);
        assert_eq!(b["model"], "claude-opus-5-5");
        assert_eq!(b["thinking"], json!({"type":"adaptive"}));
        assert_eq!(b["output_config"], json!({"effort":"medium"}));
        assert_eq!(b["fallbacks"], "default");
        assert_eq!(b["system"][0]["cache_control"], json!({"type":"ephemeral"}));
        assert_eq!(b["messages"][0]["content"][1]["source"]["media_type"], "image/jpeg");
        assert_eq!(b["messages"][0]["content"][1]["source"]["data"], "AQID");
        let old = Claude::new(reqwest::Client::new(), "http://x".into(), "k".into(), "claude-haiku-4-5".into(), "medium".into());
        let b = old.request_body("s", &[]);
        assert!(b.get("thinking").is_none() && b.get("fallbacks").is_none());
    }

    #[test]
    fn stop_reasons() {
        let ok = json!({"content":[{"type":"thinking","thinking":""},{"type":"text","text":"{}"}],"stop_reason":"end_turn"});
        assert_eq!(reply_text("x", ok.to_string().as_bytes()).unwrap(), "{}");
        let refusal = json!({"content":[],"stop_reason":"refusal","stop_details":{"type":"refusal","category":"cyber"}});
        assert!(reply_text("x", refusal.to_string().as_bytes()).unwrap_err().message.contains("cyber"));
        let cut = json!({"content":[{"type":"text","text":"{"}],"stop_reason":"max_tokens"});
        assert_eq!(reply_text("x", cut.to_string().as_bytes()).unwrap_err().code, "upstream");
    }
}
