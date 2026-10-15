//! Owner authentication: the password check, sessions (cookie `ps_session`), the login rate
//! limit, and the middleware that guards `/api/*` (session required, JSON-only writes as a CSRF
//! guard).
//!
//! Sessions are random 256-bit tokens. Only their SHA-256 is kept (in memory and in
//! `data/secrets/sessions.json`), so the file alone doesn't let anyone in. The file also holds a
//! salted fingerprint of the password: when the owner changes the password, every session ends.
//!
//! The login rate limit counts attempts per client (IPv4 address, or IPv6 /64 so rotating
//! addresses inside one allocation doesn't help) and across all clients; an attempt is counted
//! before the password is checked and forgiven when it was right.

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
/// Failed logins allowed per client per window before 429.
pub const MAX_FAILURES: usize = 5;
/// Failed logins allowed from all clients together per window.
pub const MAX_GLOBAL_FAILURES: usize = 30;
/// Iterations of the password fingerprint.
const FINGERPRINT_ROUNDS: u32 = 20_000;
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
    /// Random salt and iterated SHA-256 of salt + password: sessions belong to that password.
    #[serde(default)]
    salt: String,
    #[serde(default)]
    fingerprint: String,
}

fn fingerprint(salt: &str, password: &str) -> String {
    let mut d = store::sha256(format!("photo-studio sessions\0{salt}\0{password}").as_bytes());
    for _ in 0..FINGERPRINT_ROUNDS {
        d = store::sha256(&d);
    }
    store::hex(&d)
}

/// Failed-login bookkeeping.
#[derive(Default)]
struct Limiter {
    per_client: HashMap<IpAddr, VecDeque<u64>>,
    global: VecDeque<u64>,
}

/// A counted login attempt; [`Auth::forgive`] it when the password was right.
pub struct Attempt {
    key: IpAddr,
    at: u64,
}

/// The rate-limit key of `ip`: IPv4 as is (also when IPv4-mapped), IPv6 by its /64.
pub fn limit_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let s = v6.segments();
                IpAddr::V6(std::net::Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
            }
        },
    }
}

pub struct Auth {
    password: [u8; 32],
    sessions: Mutex<HashMap<String, SessionRec>>,
    salt: String,
    fingerprint: String,
    failures: Mutex<Limiter>,
    file: PathBuf,
    persist: tokio::sync::Mutex<()>,
    pub secure_cookie: bool,
    pub trust_proxy: bool,
}

impl Auth {
    /// Load stored sessions from `file` (expired ones dropped; all of them when the password
    /// changed since they were saved).
    pub fn load(password: &str, file: PathBuf, secure_cookie: bool, trust_proxy: bool) -> Auth {
        let now = now_ms();
        let stored = match store::read_capped(&file, 1 << 20) {
            Ok(Some(bytes)) => serde_json::from_slice::<SessionFile>(&bytes).unwrap_or_else(|e| {
                warn!("{}: {e}; starting without sessions", file.display());
                SessionFile::default()
            }),
            Ok(None) => SessionFile::default(),
            Err(e) => {
                warn!("{e}; starting without sessions");
                SessionFile::default()
            }
        };
        let (salt, sessions) = if !stored.salt.is_empty() && store::ct_eq_str(&stored.fingerprint, &fingerprint(&stored.salt, password)) {
            let live: HashMap<_, _> = stored.sessions.into_iter().filter(|(k, s)| s.expires_at > now && k.len() == 64).collect();
            (stored.salt, live)
        } else {
            if !stored.sessions.is_empty() {
                warn!("the password changed: every session is signed out");
            }
            (store::new_token(), HashMap::new())
        };
        let fingerprint = fingerprint(&salt, password);
        Auth {
            password: store::sha256(password.as_bytes()),
            sessions: Mutex::new(sessions),
            salt,
            fingerprint,
            failures: Mutex::new(Limiter::default()),
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

    /// Count a login attempt from `ip`, or `None` when its client (or everyone together) has
    /// used up the attempts of the current window. Check and count happen under one lock.
    pub fn begin_attempt(&self, ip: IpAddr) -> Option<Attempt> {
        let now = now_ms();
        let key = limit_key(ip);
        let mut l = lock(&self.failures);
        let expired = |q: &mut VecDeque<u64>| {
            while q.front().is_some_and(|t| now.saturating_sub(*t) >= FAILURE_WINDOW_MS) {
                q.pop_front();
            }
        };
        expired(&mut l.global);
        if l.global.len() >= MAX_GLOBAL_FAILURES {
            warn!("login: {} failed attempts in the last minute from all clients; refusing logins for now", l.global.len());
            return None;
        }
        if !l.per_client.contains_key(&key) && l.per_client.len() >= MAX_TRACKED_IPS {
            l.per_client.retain(|_, q| q.back().is_some_and(|t| now.saturating_sub(*t) < FAILURE_WINDOW_MS));
            // Still full: forget the client whose last attempt is oldest (never everyone).
            while l.per_client.len() >= MAX_TRACKED_IPS {
                let Some(oldest) = l.per_client.iter().min_by_key(|(_, q)| q.back().copied().unwrap_or(0)).map(|(k, _)| *k) else { break };
                l.per_client.remove(&oldest);
            }
        }
        let q = l.per_client.entry(key).or_default();
        expired(q);
        if q.len() >= MAX_FAILURES {
            return None;
        }
        q.push_back(now);
        l.global.push_back(now);
        Some(Attempt { key, at: now })
    }

    /// Uncount an attempt whose password was right.
    pub fn forgive(&self, a: Attempt) {
        let mut l = lock(&self.failures);
        if let Some(q) = l.per_client.get_mut(&a.key)
            && let Some(i) = q.iter().rposition(|t| *t == a.at)
        {
            q.remove(i);
        }
        if let Some(i) = l.global.iter().rposition(|t| *t == a.at) {
            l.global.remove(i);
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

    /// End every session ("sign out everywhere").
    pub async fn destroy_all(&self) {
        lock(&self.sessions).clear();
        self.save().await;
    }

    async fn save(&self) {
        let _guard = self.persist.lock().await;
        let file = SessionFile { sessions: lock(&self.sessions).clone(), salt: self.salt.clone(), fingerprint: self.fingerprint.clone() };
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

    /// The client address for the rate limit. Behind a trusted proxy, the last
    /// `X-Forwarded-For` entry (the one our proxy added); if that can't be read, the peer.
    pub fn client_ip(&self, peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
        if self.trust_proxy {
            let last = headers.get_all("x-forwarded-for").iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(',')).next_back();
            if let Some(ip) = last.and_then(parse_forwarded) {
                return ip;
            }
        }
        peer.ip()
    }
}

/// An `X-Forwarded-For` entry: `1.2.3.4`, `1.2.3.4:5678`, `2001:db8::1` or `[2001:db8::1]:5678`.
fn parse_forwarded(entry: &str) -> Option<IpAddr> {
    let e = entry.trim();
    if let Ok(ip) = e.parse::<IpAddr>() {
        return Some(ip);
    }
    if let Some(rest) = e.strip_prefix('[') {
        return rest.split(']').next()?.parse().ok();
    }
    e.parse::<SocketAddr>().ok().map(|a| a.ip())
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
            assert!(a.begin_attempt(ip).is_some());
        }
        assert!(a.begin_attempt(ip).is_none());
        let ok = a.begin_attempt("10.0.0.2".parse().unwrap()).unwrap();
        a.forgive(ok);
        for _ in 0..MAX_FAILURES {
            assert!(a.begin_attempt("10.0.0.2".parse().unwrap()).is_some(), "a forgiven attempt doesn't count");
        }
        // IPv6: one /64 is one client, however the address varies.
        for i in 0..MAX_FAILURES {
            assert!(a.begin_attempt(format!("2001:db8:1:2::{i:x}").parse().unwrap()).is_some());
        }
        assert!(a.begin_attempt("2001:db8:1:2:ffff::1".parse().unwrap()).is_none());
        assert!(a.begin_attempt("2001:db8:1:3::1".parse().unwrap()).is_some());
        assert_eq!(limit_key("::ffff:1.2.3.4".parse().unwrap()), "1.2.3.4".parse::<IpAddr>().unwrap());
        // Everyone together: refused after the global budget.
        let b = auth();
        let mut n = 0;
        for i in 0..1000u32 {
            if b.begin_attempt(IpAddr::from([10, 1, (i >> 8) as u8, i as u8])).is_some() {
                n += 1;
            }
        }
        assert_eq!(n, MAX_GLOBAL_FAILURES);
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
        let changed = Auth::load("a new password", a.file.clone(), false, false);
        assert!(!changed.valid(&t), "a password change ends every session");
        let t2 = b.create_session().await;
        b.destroy(&t).await;
        assert!(!b.valid(&t));
        assert!(b.valid(&t2));
        b.destroy_all().await;
        assert!(!b.valid(&t2));
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
        let peer = SocketAddr::from(([127, 0, 0, 1], 1));
        let xff = |v: &str| {
            let mut h = HeaderMap::new();
            h.insert("x-forwarded-for", v.parse().unwrap());
            a.client_ip(peer, &h)
        };
        assert_eq!(xff("1.2.3.4, 5.6.7.8"), "5.6.7.8".parse::<IpAddr>().unwrap());
        assert_eq!(xff("1.2.3.4, 5.6.7.8:443"), "5.6.7.8".parse::<IpAddr>().unwrap());
        assert_eq!(xff("1.2.3.4, [2001:db8::1]:443"), "2001:db8::1".parse::<IpAddr>().unwrap());
        // An unreadable last entry falls back to the peer, never to an earlier (client-chosen) one.
        assert_eq!(xff("9.9.9.9, garbage"), peer.ip());
    }
}
