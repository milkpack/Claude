//! Owner authentication: the password check, sessions (cookie `ps_session`), the login rate
//! limit, and the middleware that guards `/api/*` (session required, JSON-only writes as a CSRF
//! guard).
//!
//! Sessions are random 256-bit tokens. Only their SHA-256 is kept (in memory and in
//! `data/secrets/sessions.json`), so the file alone doesn't let anyone in.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use log::warn;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::server::App;
use crate::store::{self, lock, now_ms};

pub const COOKIE: &str = "ps_session";
pub const SESSION_TTL_MS: u64 = 30 * 24 * 3600 * 1000;
/// Failed logins allowed per IP per window before 429.
pub const MAX_FAILURES: usize = 5;
pub const FAILURE_WINDOW_MS: u64 = 60_000;
/// Keep at most this many sessions (oldest dropped first).
const MAX_SESSIONS: usize = 256;
/// Track at most this many IPs for the rate limit.
const MAX_TRACKED_IPS: usize = 10_000;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionRec {
    created_at: u64,
    expires_at: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct SessionFile {
    /// SHA-256 (hex) of the token → session.
    #[serde(default)]
    sessions: HashMap<String, SessionRec>,
}

pub struct Auth {
    password: [u8; 32],
    sessions: Mutex<HashMap<String, SessionRec>>,
    failures: Mutex<HashMap<IpAddr, VecDeque<u64>>>,
    file: PathBuf,
    persist: tokio::sync::Mutex<()>,
    pub secure_cookie: bool,
    pub trust_proxy: bool,
}

impl Auth {
    /// Load stored sessions from `file` (expired ones dropped).
    pub fn load(password: &str, file: PathBuf, secure_cookie: bool, trust_proxy: bool) -> Auth {
        let now = now_ms();
        let sessions = match store::read_capped(&file, 1 << 20) {
            Ok(Some(bytes)) => match serde_json::from_slice::<SessionFile>(&bytes) {
                Ok(f) => f.sessions.into_iter().filter(|(k, s)| s.expires_at > now && k.len() == 64).collect(),
                Err(e) => {
                    warn!("{}: {e}; starting without sessions", file.display());
                    HashMap::new()
                }
            },
            Ok(None) => HashMap::new(),
            Err(e) => {
                warn!("{e}; starting without sessions");
                HashMap::new()
            }
        };
        Auth {
            password: store::sha256(password.as_bytes()),
            sessions: Mutex::new(sessions),
            failures: Mutex::new(HashMap::new()),
            file,
            persist: tokio::sync::Mutex::new(()),
            secure_cookie,
            trust_proxy,
        }
    }

    /// Constant-time password check (digests are compared, so the length doesn't leak either).
    pub fn check_password(&self, input: &str) -> bool {
        store::ct_eq(&self.password, &store::sha256(input.as_bytes()))
    }

    /// Whether `ip` has used up its failed attempts for the current window.
    pub fn rate_limited(&self, ip: IpAddr) -> bool {
        let now = now_ms();
        let mut f = lock(&self.failures);
        match f.get_mut(&ip) {
            Some(q) => {
                while q.front().is_some_and(|t| now.saturating_sub(*t) >= FAILURE_WINDOW_MS) {
                    q.pop_front();
                }
                q.len() >= MAX_FAILURES
            }
            None => false,
        }
    }

    pub fn record_failure(&self, ip: IpAddr) {
        let now = now_ms();
        let mut f = lock(&self.failures);
        if f.len() >= MAX_TRACKED_IPS && !f.contains_key(&ip) {
            f.retain(|_, q| q.back().is_some_and(|t| now.saturating_sub(*t) < FAILURE_WINDOW_MS));
            if f.len() >= MAX_TRACKED_IPS {
                f.clear();
            }
        }
        let q = f.entry(ip).or_default();
        q.push_back(now);
        while q.len() > MAX_FAILURES {
            q.pop_front();
        }
    }

    /// Start a session; returns the token for the cookie.
    pub async fn create_session(&self) -> String {
        let token = store::new_token();
        let now = now_ms();
        {
            let mut s = lock(&self.sessions);
            s.retain(|_, r| r.expires_at > now);
            while s.len() >= MAX_SESSIONS {
                let Some(oldest) = s.iter().min_by_key(|(_, r)| r.created_at).map(|(k, _)| k.clone()) else { break };
                s.remove(&oldest);
            }
            s.insert(token_key(&token), SessionRec { created_at: now, expires_at: now + SESSION_TTL_MS });
        }
        self.save().await;
        token
    }

    pub fn valid(&self, token: &str) -> bool {
        let now = now_ms();
        lock(&self.sessions).get(&token_key(token)).is_some_and(|r| r.expires_at > now)
    }

    pub fn valid_headers(&self, headers: &HeaderMap) -> bool {
        session_token(headers).is_some_and(|t| self.valid(&t))
    }

    pub async fn destroy(&self, token: &str) {
        let removed = lock(&self.sessions).remove(&token_key(token)).is_some();
        if removed {
            self.save().await;
        }
    }

    async fn save(&self) {
        let _guard = self.persist.lock().await;
        let file = SessionFile { sessions: lock(&self.sessions).clone() };
        match serde_json::to_vec(&file) {
            Ok(bytes) => {
                if let Err(e) = store::write_atomic_async(self.file.clone(), bytes, true).await {
                    warn!("saving sessions: {e}");
                }
            }
            Err(e) => warn!("saving sessions: {e}"),
        }
    }

    pub fn set_cookie(&self, token: &str) -> String {
        let secure = if self.secure_cookie { "; Secure" } else { "" };
        format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{secure}", SESSION_TTL_MS / 1000)
    }

    pub fn clear_cookie(&self) -> String {
        let secure = if self.secure_cookie { "; Secure" } else { "" };
        format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{secure}")
    }

    /// The client address for the rate limit.
    pub fn client_ip(&self, peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
        if self.trust_proxy {
            let forwarded = headers
                .get_all("x-forwarded-for")
                .iter()
                .filter_map(|v| v.to_str().ok())
                .flat_map(|v| v.split(','))
                .filter_map(|s| s.trim().parse::<IpAddr>().ok())
                .next_back();
            if let Some(ip) = forwarded {
                return ip;
            }
        }
        peer.ip()
    }
}

fn token_key(token: &str) -> String {
    store::hex(&store::sha256(token.as_bytes()))
}

/// The `ps_session` cookie value, if any.
pub fn session_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty() && v.len() <= 128)
}

/// Middleware: a valid session is required.
pub async fn require_session(State(app): State<Arc<App>>, req: Request, next: Next) -> Response {
    if app.auth.valid_headers(req.headers()) { next.run(req).await } else { ApiError::unauthenticated("sign in first").into_response() }
}

/// Middleware: state-changing requests to `/api/*` must be JSON (or multipart for uploads).
/// Browsers can't send those cross-site without a CORS preflight, which we never grant.
pub async fn csrf_guard(req: Request, next: Next) -> Response {
    let method = req.method();
    if method == Method::GET || method == Method::HEAD || method == Method::OPTIONS || !req.uri().path().starts_with("/api/") {
        return next.run(req).await;
    }
    let ct = req.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
    let essence = ct.split(';').next().unwrap_or("").trim();
    let upload = method == Method::POST && req.uri().path() == "/api/assets";
    let ok = if upload { essence == "multipart/form-data" } else { essence == "application/json" };
    if ok {
        next.run(req).await
    } else {
        let want = if upload { "multipart/form-data" } else { "application/json" };
        ApiError::unsupported_media(format!("state-changing requests must use Content-Type: {want}")).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth() -> Auth {
        let file = std::env::temp_dir().join(format!("ps-auth-{}.json", store::new_id()));
        Auth::load("correct horse", file, false, false)
    }

    #[test]
    fn password_and_rate_limit() {
        let a = auth();
        assert!(a.check_password("correct horse"));
        assert!(!a.check_password("correct hors"));
        assert!(!a.check_password(""));
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        for _ in 0..MAX_FAILURES {
            assert!(!a.rate_limited(ip));
            a.record_failure(ip);
        }
        assert!(a.rate_limited(ip));
        assert!(!a.rate_limited("10.0.0.2".parse().unwrap()));
    }

    #[tokio::test]
    async fn sessions_persist_hashed() {
        let a = auth();
        let t = a.create_session().await;
        assert!(a.valid(&t));
        assert!(!a.valid("nope"));
        let stored = std::fs::read_to_string(&a.file).unwrap();
        assert!(!stored.contains(&t), "only the hash is stored");
        let b = Auth::load("correct horse", a.file.clone(), false, false);
        assert!(b.valid(&t), "sessions survive a restart");
        b.destroy(&t).await;
        assert!(!b.valid(&t));
        let _ = std::fs::remove_file(&a.file);
    }

    #[test]
    fn cookies() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, "a=b; ps_session=tok123; c=d".parse().unwrap());
        assert_eq!(session_token(&h).as_deref(), Some("tok123"));
        let a = Auth::load("x", PathBuf::from("/nonexistent/s.json"), true, true);
        let c = a.set_cookie("tok");
        assert!(c.contains("HttpOnly") && c.contains("SameSite=Strict") && c.contains("Secure") && c.contains("Max-Age=2592000"));
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "1.2.3.4, 5.6.7.8".parse().unwrap());
        assert_eq!(a.client_ip(SocketAddr::from(([127, 0, 0, 1], 1)), &h), "5.6.7.8".parse::<IpAddr>().unwrap());
    }
}
