//! The OpenArt connection: OAuth 2.1 as a public client (PKCE S256, `state`, dynamic client
//! registration), discovered from the MCP server's protected-resource metadata (RFC 9728) and the
//! authorization server's metadata (RFC 8414).
//!
//! The registration and the tokens live in `data/secrets/openart.json` (0600) and never reach the
//! browser or the logs. Access tokens refresh automatically; a refresh the server rejects marks
//! the connection as needing a reconnect.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{ApiError, ApiResult};
use crate::net::{self, neterr, read_limited, snippet};
use crate::store::{self, lock, now_ms};

pub const SCOPE: &str = "full_access";
/// Refresh this long before the access token expires.
const REFRESH_MARGIN_MS: u64 = 60_000;
/// An authorization started with `/oauth/openart/start` must finish within this time.
const PENDING_TTL_MS: u64 = 10 * 60_000;
const MAX_PENDING: usize = 32;
/// Re-discover the endpoints after this long.
const DISCOVERY_TTL_MS: u64 = 3_600_000;
const MAX_META_BYTES: usize = 256 << 10;
const OAUTH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub struct OAuthConfig {
    /// The MCP endpoint (also the OAuth `resource` unless the metadata names another).
    pub mcp_url: String,
    /// Authorization server to use without protected-resource discovery.
    pub issuer: Option<String>,
    /// `<public url>/oauth/openart/callback`.
    pub redirect_uri: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Discovery {
    pub issuer: String,
    pub resource: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub revocation_endpoint: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientReg {
    client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_secret: Option<String>,
    redirect_uri: String,
    issuer: String,
    registered_at: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tokens {
    access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    /// Milliseconds since the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    /// The client the tokens were issued to (refreshes must use it).
    client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_secret: Option<String>,
    obtained_at: u64,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Secrets {
    #[serde(default)]
    client: Option<ClientReg>,
    #[serde(default)]
    tokens: Option<Tokens>,
    #[serde(default)]
    needs_reconnect: bool,
}

struct Pending {
    verifier: String,
    created_at: u64,
    discovery: Discovery,
    client_id: String,
    client_secret: Option<String>,
}

pub struct OAuth {
    cfg: OAuthConfig,
    http: reqwest::Client,
    file: PathBuf,
    secrets: tokio::sync::Mutex<Secrets>,
    discovery: tokio::sync::Mutex<Option<(u64, Discovery)>>,
    pending: Mutex<HashMap<String, Pending>>,
}

/// How a token request failed.
enum TokenError {
    /// The server answered and refused (invalid_grant, revoked client…): reconnect needed.
    Rejected(String),
    /// Network trouble or a server error: try again later.
    Transient(String),
}

impl OAuth {
    pub fn load(cfg: OAuthConfig, http: reqwest::Client, file: PathBuf) -> OAuth {
        let secrets = match store::read_capped(&file, 1 << 20) {
            Ok(Some(bytes)) => serde_json::from_slice::<Secrets>(&bytes).unwrap_or_else(|e| {
                warn!("{}: {e}; OpenArt must be connected again", file.display());
                Secrets::default()
            }),
            Ok(None) => Secrets::default(),
            Err(e) => {
                warn!("{e}; OpenArt must be connected again");
                Secrets::default()
            }
        };
        OAuth {
            cfg,
            http,
            file,
            secrets: tokio::sync::Mutex::new(secrets),
            discovery: tokio::sync::Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// `(connected, needs_reconnect)`.
    pub async fn status(&self) -> (bool, bool) {
        let s = self.secrets.lock().await;
        (s.tokens.is_some() && !s.needs_reconnect, s.needs_reconnect)
    }

    async fn save(&self, s: &Secrets) -> ApiResult<()> {
        let bytes = serde_json::to_vec_pretty(s).map_err(|e| ApiError::internal(format!("saving the OpenArt connection: {e}")))?;
        store::write_atomic_async(self.file.clone(), bytes, true).await.map_err(|e| {
            warn!("saving the OpenArt connection: {e}");
            ApiError::internal("can't save the OpenArt connection")
        })
    }

    /// The OAuth endpoints (cached for an hour).
    pub async fn discover(&self) -> ApiResult<Discovery> {
        let mut cache = self.discovery.lock().await;
        if let Some((at, d)) = cache.as_ref()
            && now_ms().saturating_sub(*at) < DISCOVERY_TTL_MS
        {
            return Ok(d.clone());
        }
        let d = self.discover_uncached().await.map_err(|e| ApiError::upstream(format!("OpenArt OAuth discovery failed: {e}")))?;
        *cache = Some((now_ms(), d.clone()));
        Ok(d)
    }

    async fn discover_uncached(&self) -> Result<Discovery, String> {
        let mcp = reqwest::Url::parse(&self.cfg.mcp_url).map_err(|e| format!("MCP URL: {e}"))?;
        let (issuer, resource) = match &self.cfg.issuer {
            Some(i) => (i.clone(), self.cfg.mcp_url.clone()),
            None => {
                let origin = mcp.origin().ascii_serialization();
                let path = mcp.path().trim_matches('/');
                let mut candidates = vec![];
                if !path.is_empty() {
                    candidates.push(format!("{origin}/.well-known/oauth-protected-resource/{path}"));
                }
                candidates.push(format!("{origin}/.well-known/oauth-protected-resource"));
                let mut found = None;
                let mut last_err = String::new();
                for url in candidates {
                    match net::get_json(&self.http, &url, MAX_META_BYTES).await {
                        Ok(v) => {
                            found = Some(v);
                            break;
                        }
                        Err(e) => last_err = format!("{url}: {e}"),
                    }
                }
                let prm = found.ok_or(last_err)?;
                let issuer = prm
                    .get("authorization_servers")
                    .and_then(Value::as_array)
                    .and_then(|a| a.iter().find_map(Value::as_str))
                    .ok_or("the protected-resource metadata names no authorization server")?
                    .trim_end_matches('/')
                    .to_string();
                let resource = prm.get("resource").and_then(Value::as_str).unwrap_or(&self.cfg.mcp_url).to_string();
                (issuer, resource)
            }
        };
        if !net::is_http_url(&issuer) {
            return Err(format!("authorization server `{issuer}` is not an http(s) URL"));
        }
        let iu = reqwest::Url::parse(&issuer).map_err(|e| format!("issuer: {e}"))?;
        let origin = iu.origin().ascii_serialization();
        let ipath = iu.path().trim_matches('/');
        let mut candidates = vec![];
        if ipath.is_empty() {
            candidates.push(format!("{origin}/.well-known/oauth-authorization-server"));
        } else {
            candidates.push(format!("{origin}/.well-known/oauth-authorization-server/{ipath}"));
        }
        candidates.push(format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/')));
        let mut meta = None;
        let mut last_err = String::new();
        for url in candidates {
            match net::get_json(&self.http, &url, MAX_META_BYTES).await {
                Ok(v) => {
                    meta = Some(v);
                    break;
                }
                Err(e) => last_err = format!("{url}: {e}"),
            }
        }
        let meta = meta.ok_or(last_err)?;
        let s = |k: &str| meta.get(k).and_then(Value::as_str).map(str::to_string).filter(|u| net::is_http_url(u));
        if let Some(i) = meta.get("issuer").and_then(Value::as_str)
            && i.trim_end_matches('/') != issuer.trim_end_matches('/')
        {
            return Err(format!("metadata issuer `{i}` doesn't match `{issuer}`"));
        }
        if let Some(methods) = meta.get("code_challenge_methods_supported").and_then(Value::as_array)
            && !methods.iter().any(|m| m.as_str() == Some("S256"))
        {
            return Err("the authorization server doesn't support PKCE S256".into());
        }
        Ok(Discovery {
            issuer,
            resource,
            authorization_endpoint: s("authorization_endpoint").ok_or("no authorization_endpoint")?,
            token_endpoint: s("token_endpoint").ok_or("no token_endpoint")?,
            registration_endpoint: s("registration_endpoint"),
            revocation_endpoint: s("revocation_endpoint"),
        })
    }

    /// The registered client for `d`, registering one (RFC 7591) when needed.
    async fn ensure_client(&self, d: &Discovery) -> ApiResult<(String, Option<String>)> {
        let mut s = self.secrets.lock().await;
        if let Some(c) = &s.client
            && c.redirect_uri == self.cfg.redirect_uri
            && c.issuer == d.issuer
        {
            return Ok((c.client_id.clone(), c.client_secret.clone()));
        }
        let endpoint = d.registration_endpoint.as_deref().ok_or_else(|| ApiError::upstream("OpenArt doesn't offer dynamic client registration"))?;
        let body = json!({
            "client_name": "Photo Studio",
            "redirect_uris": [self.cfg.redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
            "scope": SCOPE,
        });
        let resp = self
            .http
            .post(endpoint)
            .timeout(OAUTH_TIMEOUT)
            .json(&body)
            .send()
            .await
            .map_err(|e| ApiError::upstream(format!("OpenArt client registration failed: {}", neterr(e))))?;
        let status = resp.status();
        let bytes = read_limited(resp, MAX_META_BYTES).await.map_err(|e| ApiError::upstream(format!("OpenArt client registration: {e}")))?;
        if !status.is_success() {
            return Err(ApiError::upstream(format!("OpenArt client registration failed: HTTP {status}: {}", snippet(&bytes))));
        }
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| ApiError::upstream(format!("OpenArt client registration: {e}")))?;
        let client_id = v
            .get("client_id")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty() && c.len() <= 512)
            .ok_or_else(|| ApiError::upstream("OpenArt client registration returned no client_id"))?
            .to_string();
        let client_secret = v.get("client_secret").and_then(Value::as_str).filter(|c| !c.is_empty()).map(str::to_string);
        s.client = Some(ClientReg {
            client_id: client_id.clone(),
            client_secret: client_secret.clone(),
            redirect_uri: self.cfg.redirect_uri.clone(),
            issuer: d.issuer.clone(),
            registered_at: now_ms(),
        });
        self.save(&s).await?;
        info!("registered an OAuth client with {}", d.issuer);
        Ok((client_id, client_secret))
    }

    /// Begin an authorization: returns the URL to send the browser to.
    pub async fn start(&self) -> ApiResult<String> {
        let d = self.discover().await?;
        let (client_id, client_secret) = self.ensure_client(&d).await?;
        let verifier = store::new_token();
        let challenge = store::b64url(&store::sha256(verifier.as_bytes()));
        let state = store::new_token();
        let mut url = reqwest::Url::parse(&d.authorization_endpoint).map_err(|e| ApiError::upstream(format!("authorization endpoint: {e}")))?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &client_id)
            .append_pair("redirect_uri", &self.cfg.redirect_uri)
            .append_pair("scope", SCOPE)
            .append_pair("state", &state)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("resource", &d.resource);
        let now = now_ms();
        let mut p = lock(&self.pending);
        p.retain(|_, v| now.saturating_sub(v.created_at) < PENDING_TTL_MS);
        while p.len() >= MAX_PENDING {
            let Some(oldest) = p.iter().min_by_key(|(_, v)| v.created_at).map(|(k, _)| k.clone()) else { break };
            p.remove(&oldest);
        }
        p.insert(state, Pending { verifier, created_at: now, discovery: d, client_id, client_secret });
        Ok(url.into())
    }

    /// Finish an authorization from the callback's `code` and `state`.
    pub async fn finish(&self, code: &str, state: &str) -> ApiResult<()> {
        let pending = lock(&self.pending).remove(state);
        let p = pending
            .filter(|p| now_ms().saturating_sub(p.created_at) < PENDING_TTL_MS)
            .ok_or_else(|| ApiError::bad_request("unknown or expired authorization; start again"))?;
        let mut form = vec![
            ("grant_type", "authorization_code".to_string()),
            ("code", code.to_string()),
            ("redirect_uri", self.cfg.redirect_uri.clone()),
            ("client_id", p.client_id.clone()),
            ("code_verifier", p.verifier.clone()),
            ("resource", p.discovery.resource.clone()),
        ];
        if let Some(secret) = &p.client_secret {
            form.push(("client_secret", secret.clone()));
        }
        let tokens = match self.token_request(&p.discovery.token_endpoint, &form, None).await {
            Ok(t) => t,
            Err(TokenError::Rejected(m) | TokenError::Transient(m)) => return Err(ApiError::upstream(format!("OpenArt token exchange failed: {m}"))),
        };
        let mut s = self.secrets.lock().await;
        s.tokens = Some(Tokens { client_id: p.client_id, client_secret: p.client_secret, ..tokens });
        s.needs_reconnect = false;
        self.save(&s).await?;
        info!("OpenArt connected");
        Ok(())
    }

    /// POST a token request; `old_refresh` is kept when the server doesn't rotate it.
    async fn token_request(&self, endpoint: &str, form: &[(&str, String)], old_refresh: Option<&str>) -> Result<Tokens, TokenError> {
        let resp = self
            .http
            .post(endpoint)
            .timeout(OAUTH_TIMEOUT)
            .header(reqwest::header::ACCEPT, "application/json")
            .form(form)
            .send()
            .await
            .map_err(|e| TokenError::Transient(neterr(e)))?;
        let status = resp.status();
        let bytes = read_limited(resp, MAX_META_BYTES).await.map_err(TokenError::Transient)?;
        if !status.is_success() {
            // Report the OAuth error code only (descriptions may echo request data).
            let code = serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_else(|| format!("HTTP {status}"));
            return Err(if status.is_client_error() { TokenError::Rejected(code) } else { TokenError::Transient(code) });
        }
        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            #[serde(default)]
            refresh_token: Option<String>,
            #[serde(default)]
            expires_in: Option<f64>,
            #[serde(default)]
            scope: Option<String>,
        }
        let t: TokenResponse = serde_json::from_slice(&bytes).map_err(|e| TokenError::Transient(format!("invalid token response: {e}")))?;
        if t.access_token.is_empty() || t.access_token.len() > 16 << 10 {
            return Err(TokenError::Transient("invalid access token".into()));
        }
        let now = now_ms();
        let expires_at =
            t.expires_in.filter(|s| s.is_finite() && *s >= 0.0).map(|s| now.saturating_add((s.min(10.0 * 365.0 * 86400.0) * 1000.0) as u64));
        Ok(Tokens {
            access_token: t.access_token,
            refresh_token: t.refresh_token.filter(|r| !r.is_empty()).or_else(|| old_refresh.map(str::to_string)),
            expires_at,
            scope: t.scope,
            client_id: String::new(),
            client_secret: None,
            obtained_at: now,
        })
    }

    /// A usable access token, refreshing it when it is about to expire.
    pub async fn access_token(&self) -> ApiResult<String> {
        let mut s = self.secrets.lock().await;
        if s.needs_reconnect {
            return Err(ApiError::openart_reconnect());
        }
        let Some(t) = &s.tokens else { return Err(ApiError::openart_not_connected()) };
        if t.expires_at.is_some_and(|e| now_ms().saturating_add(REFRESH_MARGIN_MS) >= e) {
            return self.refresh_locked(&mut s).await;
        }
        Ok(t.access_token.clone())
    }

    /// After a 401 with `stale`: refresh, unless another request already did.
    pub async fn refresh_after_401(&self, stale: &str) -> ApiResult<String> {
        let mut s = self.secrets.lock().await;
        if s.needs_reconnect {
            return Err(ApiError::openart_reconnect());
        }
        match &s.tokens {
            None => Err(ApiError::openart_not_connected()),
            Some(t) if t.access_token != stale => Ok(t.access_token.clone()),
            Some(_) => self.refresh_locked(&mut s).await,
        }
    }

    async fn refresh_locked(&self, s: &mut Secrets) -> ApiResult<String> {
        let Some(t) = s.tokens.clone() else { return Err(ApiError::openart_not_connected()) };
        let Some(refresh) = t.refresh_token.clone() else {
            s.needs_reconnect = true;
            self.save(s).await?;
            return Err(ApiError::openart_reconnect());
        };
        let d = self.discover().await?;
        let mut form = vec![
            ("grant_type", "refresh_token".to_string()),
            ("refresh_token", refresh.clone()),
            ("client_id", t.client_id.clone()),
            ("resource", d.resource.clone()),
        ];
        if let Some(secret) = &t.client_secret {
            form.push(("client_secret", secret.clone()));
        }
        match self.token_request(&d.token_endpoint, &form, Some(&refresh)).await {
            Ok(new) => {
                let access = new.access_token.clone();
                s.tokens = Some(Tokens { client_id: t.client_id, client_secret: t.client_secret, ..new });
                s.needs_reconnect = false;
                self.save(s).await?;
                info!("OpenArt access token refreshed");
                Ok(access)
            }
            Err(TokenError::Rejected(code)) => {
                warn!("OpenArt refused the token refresh ({code}): reconnect needed");
                s.needs_reconnect = true;
                self.save(s).await?;
                Err(ApiError::openart_reconnect())
            }
            Err(TokenError::Transient(m)) => Err(ApiError::upstream(format!("OpenArt token refresh failed: {m}"))),
        }
    }

    /// Mark the connection as needing a reconnect (the server keeps rejecting our tokens).
    pub async fn mark_reconnect(&self) {
        let mut s = self.secrets.lock().await;
        if s.tokens.is_some() && !s.needs_reconnect {
            s.needs_reconnect = true;
            if let Err(e) = self.save(&s).await {
                warn!("{e}");
            }
        }
    }

    /// Forget the tokens (revoking them first, best effort). The client registration is kept.
    pub async fn disconnect(&self) -> ApiResult<()> {
        let mut s = self.secrets.lock().await;
        let Some(t) = s.tokens.take() else {
            s.needs_reconnect = false;
            return self.save(&s).await;
        };
        s.needs_reconnect = false;
        self.save(&s).await?;
        drop(s);
        if let Ok(d) = self.discover().await
            && let Some(endpoint) = d.revocation_endpoint
        {
            let mut tokens = vec![(t.access_token.clone(), "access_token")];
            if let Some(r) = &t.refresh_token {
                tokens.push((r.clone(), "refresh_token"));
            }
            for (token, hint) in tokens {
                let form = [("token", token), ("token_type_hint", hint.to_string()), ("client_id", t.client_id.clone())];
                // Best effort: the tokens are already forgotten here either way.
                if let Err(e) = self.http.post(&endpoint).timeout(Duration::from_secs(10)).form(&form).send().await {
                    warn!("revoking the OpenArt {hint}: {}", neterr(e));
                }
            }
        }
        info!("OpenArt disconnected");
        Ok(())
    }
}
