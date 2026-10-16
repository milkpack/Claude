//! Cloud runs: a handoff task can be sent to a Claude Code routine through its API trigger
//! (`POST <routine url>` with the routine's bearer token), which starts a cloud session that
//! does the task and returns the result through the task's link. No API key is involved: the
//! run draws on the owner's Claude subscription.
//!
//! The URL and token live in `data/secrets/routine.json` (0600), or come from
//! `PHOTO_STUDIO_ROUTINE_URL` / `PHOTO_STUDIO_ROUTINE_TOKEN`; the token never leaves the server.
//! A fire is never retried: the endpoint has no idempotency key, so a retry after an ambiguous
//! failure could start a second session. The owner re-fires explicitly.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{ApiError, ApiResult};
use crate::net::{neterr, read_limited, snippet};
use crate::store::{self, lock, now_ms};

/// Fires allowed per hour, all tasks together (the routine itself allows 30).
pub const MAX_FIRES_PER_HOUR: usize = 25;
/// Largest `text` sent (the endpoint takes 65,536 characters; bytes are the stricter bound).
pub const MAX_FIRE_TEXT: usize = 64 << 10;
pub const FIRE_HEADER: &str = "Photo Studio task — do it and return the result to the link below.\n\n";
const SKILL_CUT_NOTE: &str = "\n\n[The photo skill was cut here to fit the run's size limit: use your installed photo-studio skill for the rest.]\n";

#[derive(Clone, Serialize, Deserialize)]
pub struct RoutineConfig {
    pub url: String,
    pub token: String,
    /// Outbound proxy for Anthropic traffic (may hold credentials: never sent back).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    #[serde(default)]
    pub saved_at: u64,
}

/// What a settings save does to the proxy.
pub enum ProxyChange {
    Keep,
    Clear,
    Set(String),
}

/// The outcome of the last fire of a task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStateKind {
    Fired,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunState {
    pub state: RunStateKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub at: u64,
}

impl RunState {
    pub fn failed(error: impl Into<String>) -> RunState {
        RunState { state: RunStateKind::Failed, session_url: None, error: Some(error.into()), at: now_ms() }
    }
}

pub struct Routine {
    file: PathBuf,
    env: Option<RoutineConfig>,
    stored: Mutex<Option<RoutineConfig>>,
    fires: Mutex<VecDeque<u64>>,
    http: std::sync::Arc<crate::net::AnthropicHttp>,
    /// Probed by [`Routine::check`] (`https://api.anthropic.com` in production).
    probe_url: String,
    allow_private: bool,
    timeout: Duration,
}

const UNREACHABLE: &str = "the server couldn't reach api.anthropic.com (blocked in some countries): set an Anthropic proxy in Settings → Cloud runs";

/// Checks a routine URL: `https://api.anthropic.com/v1/claude_code/routines/<id>/fire` (any http
/// host with `allow_private`, for tests).
pub fn check_url(url: &str, allow_private: bool) -> Result<String, String> {
    let u = reqwest::Url::parse(url.trim()).map_err(|_| "the routine URL is not a URL".to_string())?;
    let host_ok = if allow_private {
        u.scheme() == "https" || u.scheme() == "http"
    } else {
        u.scheme() == "https" && u.host_str() == Some("api.anthropic.com") && u.port().is_none()
    };
    let segs: Vec<&str> = u.path().trim_matches('/').split('/').collect();
    let path_ok = matches!(segs.as_slice(), ["v1", "claude_code", "routines", id, "fire"]
        if !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'));
    if !host_ok || !path_ok || u.query().is_some() || u.fragment().is_some() || !u.username().is_empty() {
        return Err("expected the routine's API trigger URL: https://api.anthropic.com/v1/claude_code/routines/<trig_…>/fire".into());
    }
    Ok(u.to_string())
}

fn check_token(token: &str) -> Result<String, String> {
    let t = token.trim();
    if t.len() < 16 || t.len() > 1024 || !t.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("the token doesn't look like a routine token (sk-ant-oat01-…)".into());
    }
    Ok(t.to_string())
}

/// The URL with the routine id shortened, for display.
pub fn mask_url(url: &str) -> String {
    let Some((prefix, rest)) = url.split_once("/routines/") else { return "…".into() };
    let id = rest.split('/').next().unwrap_or("");
    let shown = if id.len() > 12 { format!("{}…{}", id.get(..8).unwrap_or(""), id.get(id.len() - 4..).unwrap_or("")) } else { id.to_string() };
    format!("{prefix}/routines/{shown}/fire")
}

impl Routine {
    pub fn load(
        file: PathBuf,
        env: Option<(String, String)>,
        allow_private: bool,
        timeout: Duration,
        http: std::sync::Arc<crate::net::AnthropicHttp>,
        probe_url: String,
    ) -> Result<Routine, String> {
        let env = match env {
            Some((url, token)) => {
                let url = check_url(&url, allow_private).map_err(|e| format!("PHOTO_STUDIO_ROUTINE_URL: {e}"))?;
                let token = check_token(&token).map_err(|e| format!("PHOTO_STUDIO_ROUTINE_TOKEN: {e}"))?;
                Some(RoutineConfig { url, token, proxy: None, saved_at: 0 })
            }
            None => None,
        };
        let stored = match store::read_capped(&file, 64 << 10) {
            Ok(Some(b)) => serde_json::from_slice::<RoutineConfig>(&b).ok().filter(|c| check_url(&c.url, allow_private).is_ok()),
            Ok(None) => None,
            Err(e) => {
                warn!("{e}; the routine must be set again");
                None
            }
        };
        http.set_settings_proxy(stored.as_ref().and_then(|c| c.proxy.clone()).filter(|p| crate::net::check_proxy_url(p).is_ok()));
        Ok(Routine { file, env, stored: Mutex::new(stored), fires: Mutex::new(VecDeque::new()), http, probe_url, allow_private, timeout })
    }

    fn current(&self) -> Option<(RoutineConfig, &'static str)> {
        if let Some(c) = lock(&self.stored).clone() {
            return Some((c, "settings"));
        }
        self.env.clone().map(|c| (c, "env"))
    }

    pub fn configured(&self) -> bool {
        self.current().is_some()
    }

    /// `GET /api/settings/routine`: never the token.
    pub fn status(&self) -> Value {
        let mut v = match self.current() {
            Some((c, source)) => json!({ "configured": true, "url": mask_url(&c.url), "source": source }),
            None => json!({ "configured": false }),
        };
        if let Some((p, source)) = self.http.proxy() {
            v["proxy"] = json!(crate::net::mask_proxy(&p));
            v["proxySource"] = json!(source);
        }
        v
    }

    /// Save the routine. Without `token`, the saved one is kept (to change only the proxy).
    pub async fn save(&self, url: &str, token: Option<&str>, proxy: ProxyChange) -> ApiResult<()> {
        let url = check_url(url, self.allow_private).map_err(ApiError::bad_request)?;
        let previous = lock(&self.stored).clone();
        let token = match token.map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) => check_token(t).map_err(ApiError::bad_request)?,
            None => previous.as_ref().map(|c| c.token.clone()).ok_or_else(|| ApiError::bad_request("token is required"))?,
        };
        let proxy = match proxy {
            ProxyChange::Keep => previous.and_then(|c| c.proxy),
            ProxyChange::Clear => None,
            ProxyChange::Set(p) => Some(crate::net::check_proxy_url(&p).map_err(ApiError::bad_request)?),
        };
        let cfg = RoutineConfig { url, token, proxy: proxy.clone(), saved_at: now_ms() };
        let bytes = serde_json::to_vec_pretty(&cfg).map_err(|e| ApiError::internal(e.to_string()))?;
        store::write_atomic_async(self.file.clone(), bytes, true).await.map_err(|e| {
            warn!("saving the routine: {e}");
            ApiError::internal("can't save the routine")
        })?;
        *lock(&self.stored) = Some(cfg);
        self.http.set_settings_proxy(proxy);
        info!("cloud routine set");
        Ok(())
    }

    pub async fn clear(&self) -> ApiResult<()> {
        *lock(&self.stored) = None;
        self.http.set_settings_proxy(None);
        match tokio::fs::remove_file(&self.file).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => {
                warn!("removing the routine: {e}");
                Err(ApiError::internal("can't remove the routine"))
            }
        }
    }

    /// Count a fire against the local hourly budget; `false` when it is used up.
    fn take_slot(&self) -> bool {
        let now = now_ms();
        let mut f = lock(&self.fires);
        while f.front().is_some_and(|t| now.saturating_sub(*t) >= 3_600_000) {
            f.pop_front();
        }
        if f.len() >= MAX_FIRES_PER_HOUR {
            return false;
        }
        f.push_back(now);
        true
    }

    /// Fire the routine once with `text` (never retried).
    pub async fn fire(&self, text: &str) -> RunState {
        let Some((cfg, _)) = self.current() else { return RunState::failed("cloud runs aren't set up (Settings → Cloud runs)") };
        if text.len() > MAX_FIRE_TEXT {
            return RunState::failed("the task is too long for a cloud run");
        }
        if !self.take_slot() {
            return RunState::failed(format!("Photo Studio's limit of {MAX_FIRES_PER_HOUR} cloud runs per hour is reached; try again later"));
        }
        let via_proxy = self.http.proxy().is_some();
        let client = match self.http.client() {
            Ok(c) => c,
            Err(e) => return RunState::failed(e),
        };
        let unreachable = if via_proxy {
            "the server couldn't reach api.anthropic.com through the Anthropic proxy: check it in Settings → Cloud runs"
        } else {
            UNREACHABLE
        };
        let req = client
            .post(&cfg.url)
            .timeout(self.timeout)
            .bearer_auth(&cfg.token)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "experimental-cc-routine-2026-04-01")
            .json(&json!({ "text": text }));
        let ambiguous = "the run may or may not have started: check claude.ai/code before running it again";
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) if e.is_connect() => return RunState::failed(format!("{unreachable} ({})", neterr(e))),
            Err(e) if e.is_timeout() => return RunState::failed(format!("{unreachable} (timed out; if it did get through, {ambiguous})")),
            Err(e) => return RunState::failed(format!("{} ({ambiguous})", neterr(e))),
        };
        let status = resp.status();
        let retry_after = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_string);
        let body = match read_limited(resp, 64 << 10).await {
            Ok(b) => b,
            Err(e) => return RunState::failed(format!("{e} ({ambiguous})")),
        };
        let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        if status.is_success() {
            let url = v.get("claude_code_session_url").and_then(Value::as_str).filter(|u| u.starts_with("https://")).map(str::to_string);
            info!("cloud run started");
            return RunState { state: RunStateKind::Fired, session_url: url, error: None, at: now_ms() };
        }
        let message = v.get("error").and_then(|e| e.get("message")).and_then(Value::as_str).map(|m| snippet(m.as_bytes())).unwrap_or_default();
        let detail = if message.is_empty() { String::new() } else { format!(": {message}") };
        let error = match status.as_u16() {
            401 => "Claude rejected the routine token: generate a new one and save it in Settings".to_string(),
            403 => format!("this account can't use routine triggers{detail}"),
            404 => "the routine doesn't exist: check the URL in Settings".to_string(),
            429 => format!(
                "Claude's hourly limit for routine runs is reached{}",
                retry_after.map(|s| format!("; retry after {s} s")).unwrap_or_default()
            ),
            400..=499 => format!("Claude refused the run (HTTP {status}){detail}"),
            _ => format!("Claude returned HTTP {status}{detail} ({ambiguous})"),
        };
        warn!("cloud run failed: {error}");
        RunState::failed(error)
    }
}

impl Routine {
    /// Can the server reach api.anthropic.com by the configured path? Any HTTP answer counts.
    pub async fn check(&self) -> Value {
        let via = if self.http.proxy().is_some() { "proxy" } else { "direct" };
        let client = match self.http.client() {
            Ok(c) => c,
            Err(e) => return json!({ "reachable": false, "via": via, "error": e }),
        };
        match client.get(&self.probe_url).timeout(Duration::from_secs(10)).send().await {
            Ok(r) => json!({ "reachable": true, "via": via, "status": r.status().as_u16() }),
            Err(e) => {
                let why = if e.is_timeout() { "timed out".to_string() } else { neterr(e) };
                let hint = if via == "proxy" {
                    "check the Anthropic proxy"
                } else {
                    "set an Anthropic proxy (api.anthropic.com is blocked in some countries)"
                };
                json!({ "reachable": false, "via": via, "error": format!("couldn't reach api.anthropic.com ({why}): {hint}") })
            }
        }
    }
}

/// The text to fire for a task prompt: the header, then the prompt. When it's too long, the
/// appended photo skill is cut first; the task and its return instructions are never cut.
pub fn fire_text(prompt: &str) -> Result<String, String> {
    let (core, skill) = match prompt.find(crate::prompts::SKILL_MARKER) {
        Some(i) => (prompt.get(..i).unwrap_or(prompt), prompt.get(i..).unwrap_or("")),
        None => (prompt, ""),
    };
    let base = FIRE_HEADER.len() + core.len();
    if base > MAX_FIRE_TEXT {
        return Err("the task is too long for a cloud run (over 64 KB without the photo skill)".into());
    }
    if base + skill.len() <= MAX_FIRE_TEXT {
        return Ok(format!("{FIRE_HEADER}{core}{skill}"));
    }
    let room = MAX_FIRE_TEXT.saturating_sub(base + SKILL_CUT_NOTE.len());
    let mut cut = room.min(skill.len());
    while cut > 0 && !skill.is_char_boundary(cut) {
        cut -= 1;
    }
    let kept = if room == 0 { "" } else { skill.get(..cut).unwrap_or("") };
    Ok(format!("{FIRE_HEADER}{core}{kept}{SKILL_CUT_NOTE}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_masking() {
        let u = "https://api.anthropic.com/v1/claude_code/routines/trig_01ABCDEFGHJKLMNOPQRSTUVW/fire";
        assert_eq!(check_url(u, false).unwrap(), u);
        for bad in [
            "http://api.anthropic.com/v1/claude_code/routines/trig_1/fire",
            "https://evil.example/v1/claude_code/routines/trig_1/fire",
            "https://api.anthropic.com/v1/claude_code/routines/trig_1",
            "https://api.anthropic.com/v1/claude_code/routines/../fire",
            "https://api.anthropic.com/v1/claude_code/routines/trig_1/fire?x=1",
            "https://api.anthropic.com:8443/v1/claude_code/routines/trig_1/fire",
            "nope",
        ] {
            assert!(check_url(bad, false).is_err(), "{bad}");
        }
        assert!(check_url("http://127.0.0.1:9/v1/claude_code/routines/trig_1/fire", true).is_ok());
        assert_eq!(mask_url(u), "https://api.anthropic.com/v1/claude_code/routines/trig_01A…TUVW/fire");
        assert!(check_token("short").is_err() && check_token("sk-ant-oat01-abcdefghijklmnop").is_ok());
    }

    #[test]
    fn fire_text_cuts_the_skill_first() {
        let core = "# Task\n\n## Return the result\n\ncurl -F 'text=<answer.md' 'https://studio/t/x'\n";
        let small = format!("{core}{}short skill", crate::prompts::SKILL_MARKER);
        assert_eq!(fire_text(&small).unwrap(), format!("{FIRE_HEADER}{small}"));
        let big = format!("{core}{}{}", crate::prompts::SKILL_MARKER, "é".repeat(50_000));
        let t = fire_text(&big).unwrap();
        assert!(t.len() <= MAX_FIRE_TEXT && t.starts_with(FIRE_HEADER) && t.contains("curl -F 'text=<answer.md' 'https://studio/t/x'"));
        assert!(t.ends_with(SKILL_CUT_NOTE) && t.contains("é"));
        assert!(fire_text(&"x".repeat(MAX_FIRE_TEXT)).is_err(), "the task itself is never cut");
    }
}
