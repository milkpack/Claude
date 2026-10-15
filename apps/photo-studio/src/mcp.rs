//! A minimal MCP client over Streamable HTTP: `initialize` → `notifications/initialized` →
//! `tools/call`. Responses may be plain JSON or a server-sent event stream; the session id
//! (`Mcp-Session-Id`) is kept and the session re-initialized when the server forgets it; a 401
//! refreshes the access token once and retries.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use log::{debug, info};
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap};
use serde_json::{Value, json};

use crate::error::{ApiError, ApiResult};
use crate::net::{neterr, read_limited, snippet};
use crate::oauth::OAuth;

/// The protocol version we ask for; the server may answer with another one it supports.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// Largest JSON-RPC response (JSON body or the whole event stream).
const MAX_RESPONSE_BYTES: usize = 16 << 20;
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Debug, PartialEq)]
struct Session {
    id: Option<String>,
    protocol: String,
}

#[derive(Debug, PartialEq)]
pub enum McpError {
    /// 401: the access token was refused.
    Unauthorized,
    /// The server no longer knows our session.
    SessionGone,
    /// Anything else, with a message for the owner.
    Failed(String),
}

impl From<McpError> for ApiError {
    fn from(e: McpError) -> ApiError {
        match e {
            McpError::Unauthorized => ApiError::openart_reconnect(),
            McpError::SessionGone => ApiError::upstream("OpenArt MCP session lost"),
            McpError::Failed(m) => ApiError::upstream(m),
        }
    }
}

pub struct Mcp {
    url: String,
    http: reqwest::Client,
    session: tokio::sync::Mutex<Option<Session>>,
    next_id: AtomicU64,
}

impl Mcp {
    pub fn new(url: String, http: reqwest::Client) -> Mcp {
        Mcp { url, http, session: tokio::sync::Mutex::new(None), next_id: AtomicU64::new(1) }
    }

    /// Drop the session (after a reconnect or a disconnect).
    pub async fn reset(&self) {
        *self.session.lock().await = None;
    }

    /// Call `tool` with `args` and return its parsed result.
    pub async fn call_tool(&self, oauth: &OAuth, tool: &str, args: Value) -> ApiResult<Value> {
        let mut token = oauth.access_token().await?;
        let mut refreshed = false;
        let mut reinitialized = false;
        loop {
            match self.try_call(&token, tool, &args).await {
                Ok(result) => return parse_tool_result(tool, &result),
                Err(McpError::Unauthorized) if !refreshed => {
                    refreshed = true;
                    debug!("OpenArt MCP: 401 on {tool}, refreshing the token");
                    self.reset().await;
                    token = oauth.refresh_after_401(&token).await?;
                }
                Err(McpError::Unauthorized) => {
                    oauth.mark_reconnect().await;
                    return Err(ApiError::openart_reconnect());
                }
                Err(McpError::SessionGone) if !reinitialized => {
                    reinitialized = true;
                    info!("OpenArt MCP session expired; initializing a new one");
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    async fn try_call(&self, token: &str, tool: &str, args: &Value) -> Result<Value, McpError> {
        let session = self.ensure_session(token).await?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": tool, "arguments": args } });
        match self.post(token, Some(&session), &body, Some(id)).await {
            Ok((Some(msg), _)) => rpc_result(msg),
            Ok((None, _)) => Err(McpError::Failed(format!("OpenArt MCP sent no response to {tool}"))),
            Err(McpError::SessionGone) => {
                let mut g = self.session.lock().await;
                if g.as_ref() == Some(&session) {
                    *g = None;
                }
                Err(McpError::SessionGone)
            }
            Err(e) => Err(e),
        }
    }

    async fn ensure_session(&self, token: &str) -> Result<Session, McpError> {
        let mut g = self.session.lock().await;
        if let Some(s) = g.as_ref() {
            return Ok(s.clone());
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let init = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "photo-studio", "version": env!("CARGO_PKG_VERSION") },
            },
        });
        let (msg, headers) = self.post(token, None, &init, Some(id)).await?;
        let result = rpc_result(msg.ok_or_else(|| McpError::Failed("OpenArt MCP sent no initialize response".into()))?)?;
        let protocol = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty() && v.len() <= 32 && v.bytes().all(|b| b.is_ascii_graphic()))
            .unwrap_or(PROTOCOL_VERSION)
            .to_string();
        let session_id = headers
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.is_empty() && v.len() <= 1024 && v.bytes().all(|b| b.is_ascii_graphic()))
            .map(str::to_string);
        let session = Session { id: session_id, protocol };
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        self.post(token, Some(&session), &note, None).await?;
        debug!("OpenArt MCP session initialized (protocol {})", session.protocol);
        *g = Some(session.clone());
        Ok(session)
    }

    /// POST one JSON-RPC message. With `expect` (a request id), returns the matching response.
    async fn post(&self, token: &str, session: Option<&Session>, body: &Value, expect: Option<u64>) -> Result<(Option<Value>, HeaderMap), McpError> {
        let mut req = self
            .http
            .post(&self.url)
            .timeout(CALL_TIMEOUT)
            .bearer_auth(token)
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec(body).map_err(|e| McpError::Failed(e.to_string()))?);
        let has_session = session.is_some_and(|s| s.id.is_some());
        if let Some(s) = session {
            if let Some(id) = &s.id {
                req = req.header("mcp-session-id", id);
            }
            req = req.header("mcp-protocol-version", &s.protocol);
        }
        let resp = req.send().await.map_err(|e| McpError::Failed(format!("OpenArt MCP: {}", neterr(e))))?;
        let status = resp.status();
        let headers = resp.headers().clone();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(McpError::Unauthorized);
        }
        if has_session && status == reqwest::StatusCode::NOT_FOUND {
            return Err(McpError::SessionGone);
        }
        if !status.is_success() {
            let text = read_limited(resp, 64 << 10).await.unwrap_or_default();
            if has_session && status == reqwest::StatusCode::BAD_REQUEST && String::from_utf8_lossy(&text).to_lowercase().contains("session") {
                return Err(McpError::SessionGone);
            }
            return Err(McpError::Failed(format!("OpenArt MCP returned HTTP {status}: {}", snippet(&text))));
        }
        let Some(want) = expect else { return Ok((None, headers)) };
        if status == reqwest::StatusCode::ACCEPTED {
            return Ok((None, headers));
        }
        let ct = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
        if ct.starts_with("text/event-stream") {
            let msg = read_sse(resp, want).await?;
            return Ok((Some(msg), headers));
        }
        let bytes = read_limited(resp, MAX_RESPONSE_BYTES).await.map_err(|e| McpError::Failed(format!("OpenArt MCP: {e}")))?;
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| McpError::Failed(format!("OpenArt MCP sent invalid JSON: {e}")))?;
        let msg = match v {
            Value::Array(items) => items.into_iter().find(|m| is_response_to(m, want)),
            m if is_response_to(&m, want) => Some(m),
            _ => None,
        };
        msg.map(|m| (Some(m), headers)).ok_or_else(|| McpError::Failed("OpenArt MCP sent no matching response".into()))
    }
}

fn is_response_to(msg: &Value, id: u64) -> bool {
    let id_matches = match msg.get("id") {
        Some(Value::Number(n)) => n.as_u64() == Some(id),
        Some(Value::String(s)) => s.parse::<u64>().ok() == Some(id),
        _ => false,
    };
    id_matches && (msg.get("result").is_some() || msg.get("error").is_some())
}

fn rpc_result(msg: Value) -> Result<Value, McpError> {
    if let Some(err) = msg.get("error") {
        let m = err.get("message").and_then(Value::as_str).unwrap_or("unknown error");
        return Err(McpError::Failed(format!("OpenArt MCP error: {}", snippet(m.as_bytes()))));
    }
    match msg {
        Value::Object(mut o) => o.remove("result").ok_or_else(|| McpError::Failed("OpenArt MCP response without a result".into())),
        _ => Err(McpError::Failed("OpenArt MCP response is not an object".into())),
    }
}

/// Read a server-sent event stream until the response to request `want` arrives.
async fn read_sse(mut resp: reqwest::Response, want: u64) -> Result<Value, McpError> {
    let mut buf: Vec<u8> = Vec::new();
    let mut total = 0usize;
    loop {
        let chunk = resp.chunk().await.map_err(|e| McpError::Failed(format!("OpenArt MCP stream: {}", neterr(e))))?;
        let Some(chunk) = chunk else { break };
        total += chunk.len();
        if total > MAX_RESPONSE_BYTES {
            return Err(McpError::Failed("OpenArt MCP stream is too large".into()));
        }
        buf.extend_from_slice(&chunk);
        while let Some(event) = take_event(&mut buf) {
            if let Some(msg) = event_message(&event)
                && is_response_to(&msg, want)
            {
                return Ok(msg);
            }
        }
    }
    // The last event may end without a blank line.
    if let Some(msg) = event_message(&buf)
        && is_response_to(&msg, want)
    {
        return Ok(msg);
    }
    Err(McpError::Failed("OpenArt MCP stream ended without a response".into()))
}

/// Remove and return the first complete event (terminated by a blank line) from `buf`.
fn take_event(buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    let find = |pat: &[u8]| buf.windows(pat.len()).position(|w| w == pat).map(|p| (p, pat.len()));
    let end = match (find(b"\n\n"), find(b"\r\n\r\n")) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    };
    let (pos, len) = end?;
    let event: Vec<u8> = buf.drain(..pos + len).collect();
    Some(event)
}

/// The JSON message carried by one event's `data:` lines.
fn event_message(event: &[u8]) -> Option<Value> {
    let text = String::from_utf8_lossy(event);
    let data: Vec<&str> = text.lines().filter_map(|l| l.strip_prefix("data:")).map(|d| d.strip_prefix(' ').unwrap_or(d)).collect();
    if data.is_empty() {
        return None;
    }
    serde_json::from_str(&data.join("\n")).ok()
}

/// A tool result as JSON: `structuredContent`, else the first text block that parses as JSON.
pub fn parse_tool_result(tool: &str, result: &Value) -> ApiResult<Value> {
    let texts: Vec<&str> = result
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("text")).filter_map(|b| b.get("text")?.as_str()).collect()
        })
        .unwrap_or_default();
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        let msg = texts.join(" ");
        let msg = if msg.trim().is_empty() { "unknown error".to_string() } else { snippet(msg.as_bytes()) };
        return Err(ApiError::upstream(format!("OpenArt {tool}: {msg}")));
    }
    if let Some(sc) = result.get("structuredContent").filter(|v| v.is_object() || v.is_array()) {
        return Ok(sc.clone());
    }
    for t in texts {
        if let Ok(v) = serde_json::from_str::<Value>(t)
            && (v.is_object() || v.is_array())
        {
            return Ok(v);
        }
    }
    Err(ApiError::upstream(format!("OpenArt {tool} returned an unexpected result")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_events() {
        let mut buf =
            b"event: message\ndata: {\"a\":1}\n\nid: 2\r\ndata: {\"jsonrpc\":\"2.0\",\r\ndata: \"id\":7,\"result\":{}}\r\n\r\npartial".to_vec();
        let e1 = take_event(&mut buf).unwrap();
        assert_eq!(event_message(&e1), Some(json!({"a":1})));
        let e2 = take_event(&mut buf).unwrap();
        let m = event_message(&e2).unwrap();
        assert!(is_response_to(&m, 7));
        assert!(!is_response_to(&m, 8));
        assert_eq!(take_event(&mut buf), None);
        assert_eq!(buf, b"partial");
        assert_eq!(event_message(b": comment only\n\n"), None);
    }

    #[test]
    fn tool_results() {
        let structured = json!({"content":[{"type":"text","text":"ignored"}],"structuredContent":{"x":1}});
        assert_eq!(parse_tool_result("t", &structured).unwrap(), json!({"x":1}));
        let text =
            json!({"content":[{"type":"resource_link","uri":"https://x"},{"type":"text","text":"not json"},{"type":"text","text":"{\"y\":2}"}]});
        assert_eq!(parse_tool_result("t", &text).unwrap(), json!({"y":2}));
        let err = json!({"isError":true,"content":[{"type":"text","text":"bad params"}]});
        let e = parse_tool_result("t", &err).unwrap_err();
        assert_eq!(e.code, "upstream");
        assert!(e.message.contains("bad params"));
        assert!(parse_tool_result("t", &json!({"content":[]})).is_err());
        assert!(rpc_result(json!({"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"nope"}})).is_err());
        assert_eq!(rpc_result(json!({"jsonrpc":"2.0","id":1,"result":{"ok":true}})).unwrap(), json!({"ok":true}));
    }
}
