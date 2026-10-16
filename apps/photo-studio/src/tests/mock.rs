//! In-process mock servers: OpenArt (OAuth discovery, registration, tokens, revocation, the MCP
//! endpoint with JSON or SSE replies, a CDN and presigned uploads) and the Anthropic API.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::store;

/// An `application/x-www-form-urlencoded` body as a map.
fn form(body: &Bytes) -> HashMap<String, String> {
    let url = reqwest::Url::parse(&format!("http://x/?{}", String::from_utf8_lossy(body))).unwrap();
    url.query_pairs().into_owned().collect()
}

pub async fn spawn(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

pub struct OpenArtState {
    pub base: Mutex<String>,
    pub registrations: Mutex<Vec<Value>>,
    /// Authorization code → PKCE challenge (the test plays the browser and the consent screen).
    pub codes: Mutex<HashMap<String, String>>,
    pub token_forms: Mutex<Vec<HashMap<String, String>>>,
    pub valid_access: Mutex<HashSet<String>>,
    pub valid_refresh: Mutex<HashSet<String>>,
    pub counter: AtomicU64,
    pub reject_refresh: AtomicBool,
    pub revoked: Mutex<Vec<String>>,
    /// Reply to MCP requests as server-sent events instead of JSON.
    pub sse: AtomicBool,
    pub sessions: Mutex<HashSet<String>>,
    pub initializes: AtomicU64,
    pub notifications: AtomicU64,
    pub protocol_headers: Mutex<Vec<String>>,
    pub calls: Mutex<Vec<(String, Value)>>,
    /// Successive `openart_creation_get` replies (the last one repeats).
    pub status_script: Mutex<VecDeque<Value>>,
    pub creation_items: Mutex<Vec<Value>>,
    pub generate_reply: Mutex<Value>,
    pub affordable: AtomicBool,
    pub cdn: Mutex<HashMap<String, Vec<u8>>>,
    pub cdn_hits: AtomicU64,
    pub puts: Mutex<Vec<(String, Vec<u8>, String)>>,
    /// Delay before answering `openart_generate_image` (after recording the call).
    pub generate_delay_ms: AtomicU64,
    /// Rotate refresh tokens (the old one dies at once) and answer refreshes after this delay.
    pub rotate_refresh: AtomicBool,
    pub refresh_delay_ms: AtomicU64,
}

impl OpenArtState {
    pub fn calls_to(&self, tool: &str) -> Vec<Value> {
        self.calls.lock().unwrap().iter().filter(|(t, _)| t == tool).map(|(_, a)| a.clone()).collect()
    }
    /// Invalidate every access token (as if they expired server-side).
    pub fn expire_access_tokens(&self) {
        self.valid_access.lock().unwrap().clear();
    }
    pub fn base(&self) -> String {
        self.base.lock().unwrap().clone()
    }
}

pub struct MockOpenArt {
    pub base: String,
    pub state: Arc<OpenArtState>,
}

impl MockOpenArt {
    pub async fn start() -> MockOpenArt {
        let state = Arc::new(OpenArtState {
            base: Mutex::new(String::new()),
            registrations: Mutex::default(),
            codes: Mutex::default(),
            token_forms: Mutex::default(),
            valid_access: Mutex::default(),
            valid_refresh: Mutex::default(),
            counter: AtomicU64::new(0),
            reject_refresh: AtomicBool::new(false),
            revoked: Mutex::default(),
            sse: AtomicBool::new(false),
            sessions: Mutex::default(),
            initializes: AtomicU64::new(0),
            notifications: AtomicU64::new(0),
            protocol_headers: Mutex::default(),
            calls: Mutex::default(),
            status_script: Mutex::default(),
            creation_items: Mutex::default(),
            generate_reply: Mutex::new(json!({ "historyId": "h1", "status": "PENDING", "pollAfterSeconds": 0.01 })),
            affordable: AtomicBool::new(true),
            cdn: Mutex::default(),
            cdn_hits: AtomicU64::new(0),
            puts: Mutex::default(),
            generate_delay_ms: AtomicU64::new(0),
            rotate_refresh: AtomicBool::new(false),
            refresh_delay_ms: AtomicU64::new(0),
        });
        let router = Router::new()
            .route("/.well-known/oauth-protected-resource/mcp", get(prm))
            .route("/.well-known/oauth-authorization-server", get(as_meta))
            .route("/register", post(register))
            .route("/token", post(token))
            .route("/revoke", post(revoke))
            .route("/mcp", post(mcp))
            .route("/cdn/{name}", get(cdn))
            .route("/upload/{id}", put(upload))
            .with_state(state.clone());
        let base = spawn(router).await;
        *state.base.lock().unwrap() = base.clone();
        MockOpenArt { base, state }
    }

    pub fn mcp_url(&self) -> String {
        format!("{}/mcp", self.base)
    }

    pub fn add_image(&self, name: &str, bytes: Vec<u8>) -> String {
        self.state.cdn.lock().unwrap().insert(name.to_string(), bytes);
        format!("{}/cdn/{name}", self.base)
    }

    pub fn script(&self, replies: Vec<Value>) {
        *self.state.status_script.lock().unwrap() = replies.into();
    }
}

async fn prm(State(s): State<Arc<OpenArtState>>) -> Json<Value> {
    let base = s.base();
    Json(
        json!({ "resource": format!("{base}/mcp"), "authorization_servers": [base], "bearer_methods_supported": ["header"], "scopes_supported": ["full_access"] }),
    )
}

async fn as_meta(State(s): State<Arc<OpenArtState>>) -> Json<Value> {
    let base = s.base();
    Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/authorize"),
        "token_endpoint": format!("{base}/token"),
        "registration_endpoint": format!("{base}/register"),
        "revocation_endpoint": format!("{base}/revoke"),
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "response_types_supported": ["code"],
        "scopes_supported": ["full_access"],
    }))
}

async fn register(State(s): State<Arc<OpenArtState>>, Json(body): Json<Value>) -> Response {
    s.registrations.lock().unwrap().push(body);
    (StatusCode::CREATED, Json(json!({ "client_id": "client-1", "token_endpoint_auth_method": "none" }))).into_response()
}

fn issue(s: &OpenArtState, refresh: Option<String>) -> Value {
    let n = s.counter.fetch_add(1, Ordering::SeqCst);
    let access = format!("at-{n}");
    let refresh = refresh.unwrap_or_else(|| format!("rt-{n}"));
    s.valid_access.lock().unwrap().insert(access.clone());
    s.valid_refresh.lock().unwrap().insert(refresh.clone());
    json!({ "access_token": access, "token_type": "Bearer", "expires_in": 3600, "refresh_token": refresh, "scope": "full_access" })
}

async fn token(State(s): State<Arc<OpenArtState>>, body: Bytes) -> Response {
    let form = form(&body);
    s.token_forms.lock().unwrap().push(form.clone());
    let bad = |e: &str| (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response();
    if form.get("client_id").map(String::as_str) != Some("client-1") {
        return bad("invalid_client");
    }
    match form.get("grant_type").map(String::as_str) {
        Some("authorization_code") => {
            let Some(challenge) = form.get("code").and_then(|c| s.codes.lock().unwrap().remove(c)) else { return bad("invalid_grant") };
            let verifier = form.get("code_verifier").cloned().unwrap_or_default();
            if store::b64url(&store::sha256(verifier.as_bytes())) != challenge {
                return bad("invalid_grant");
            }
            Json(issue(&s, None)).into_response()
        }
        Some("refresh_token") => {
            let rt = form.get("refresh_token").cloned().unwrap_or_default();
            if s.reject_refresh.load(Ordering::SeqCst) || !s.valid_refresh.lock().unwrap().contains(&rt) {
                return bad("invalid_grant");
            }
            if s.rotate_refresh.load(Ordering::SeqCst) {
                // OAuth 2.1 public client: a new refresh token, and the old one dies at once.
                s.valid_refresh.lock().unwrap().remove(&rt);
                let v = issue(&s, None);
                let d = s.refresh_delay_ms.load(Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(d)).await;
                return Json(v).into_response();
            }
            // Rotate the access token, keep the refresh token (the response omits it).
            let mut v = issue(&s, Some(rt));
            v.as_object_mut().unwrap().remove("refresh_token");
            Json(v).into_response()
        }
        _ => bad("unsupported_grant_type"),
    }
}

async fn revoke(State(s): State<Arc<OpenArtState>>, body: Bytes) -> StatusCode {
    let form = form(&body);
    if let Some(t) = form.get("token") {
        s.revoked.lock().unwrap().push(t.clone());
        s.valid_access.lock().unwrap().remove(t);
        s.valid_refresh.lock().unwrap().remove(t);
    }
    StatusCode::OK
}

fn models() -> Value {
    json!({
        "balance": 1000,
        "models": [
            {"id": "nano-banana-2-1", "displayName": "Nano Banana 2.1", "description": "Google Nano Banana 2.1 for people",
             "modes": {"image": [
                {"mode": "text2image", "description": "Text to image", "totalCredits": 30, "affordable": true},
                {"mode": "image2image", "description": "Image to image", "totalCredits": 32, "affordable": true}]}},
            {"id": "gpt-image-2", "displayName": "GPT Image 2", "description": "OpenAI GPT Image 2",
             "modes": {"image": [{"mode": "text2image", "description": "Text to image", "totalCredits": 40, "affordable": true}]}},
            {"id": "veo3-1", "displayName": "Veo 3.1", "description": "video", "modes": {"video": [{"mode": "text2video", "totalCredits": 120, "affordable": true}]}}
        ],
        "upgradeUrl": "https://openart.ai/pricing?utm_source=mcp"
    })
}

fn tool(s: &OpenArtState, name: &str, args: &Value) -> Result<Value, String> {
    let base = s.base();
    Ok(match name {
        "openart_model_list" => models(),
        "openart_model_form_get" => json!({
            "model": args["model"], "mode": args["mode"], "media": "image",
            "jsonSchema": {"type": "object", "properties": {"prompt": {"type": "string"}, "imageCount": {"type": "integer", "minimum": 1, "maximum": 8}}, "required": ["prompt"]}
        }),
        "openart_model_cost" => json!({
            "currency": "credits", "balance": 1000,
            "items": [{"model": args["model"], "mode": args["mode"], "config": args["params"], "unitCredits": 32, "quantity": 2, "totalCredits": 64,
                       "affordable": s.affordable.load(Ordering::SeqCst), "mediaType": "image"}],
            "upgradeUrl": "https://openart.ai/pricing?utm_source=mcp"
        }),
        "openart_generate_image" => s.generate_reply.lock().unwrap().clone(),
        "openart_creation_get" => {
            let mut script = s.status_script.lock().unwrap();
            let v = if script.len() > 1 { script.pop_front() } else { script.front().cloned() };
            v.unwrap_or_else(|| json!({"status": "RUNNING", "pollAfterSeconds": 0.01}))
        }
        "openart_creation_list" => json!({ "items": s.creation_items.lock().unwrap().clone(), "hasMore": false }),
        "openart_upload_sign" => {
            json!({ "signURL": format!("{base}/upload/u1?sig=secret"), "accessURL": format!("{base}/cdn/u1.png"), "uploadId": "u1", "expiresIn": 600 })
        }
        "openart_upload_metadata_get" => json!({
            "status": "ready",
            "metadata": {"width": 64, "height": 48},
            "visualReference": {"type": "image", "id": args["uploadId"], "url": args["mediaUrl"], "label": args["label"]}
        }),
        "openart_account_get" => json!({ "user": {"uid": "user_1", "email": "owner@example.com"}, "plan": "Wonder", "credits": 1000 }),
        other => return Err(format!("unknown tool {other}")),
    })
}

/// A tools/call result: status replies look like OpenArt's (resource links + a JSON text block),
/// the others carry structuredContent.
fn tool_result(name: &str, value: Result<Value, String>) -> Value {
    match value {
        Err(e) => json!({ "isError": true, "content": [{"type": "text", "text": e}] }),
        Ok(v) if name == "openart_creation_get" || name == "openart_model_list" => json!({
            "content": [{"type": "resource_link", "uri": "https://cdn.example/x.png", "name": "x"}, {"type": "text", "text": v.to_string()}]
        }),
        Ok(v) => json!({ "content": [{"type": "text", "text": v.to_string()}], "structuredContent": v }),
    }
}

async fn mcp(State(s): State<Arc<OpenArtState>>, headers: HeaderMap, Json(msg): Json<Value>) -> Response {
    let bearer = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).unwrap_or("");
    if !s.valid_access.lock().unwrap().contains(bearer) {
        return (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, "Bearer resource_metadata=\".well-known/oauth-protected-resource/mcp\"")])
            .into_response();
    }
    let accept = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(accept.contains("application/json") && accept.contains("text/event-stream"), "Accept: {accept}");
    let method = msg["method"].as_str().unwrap_or("").to_string();
    let session = headers.get("mcp-session-id").and_then(|v| v.to_str().ok()).map(str::to_string);
    let mut extra_headers = vec![];
    let result = if method == "initialize" {
        assert_eq!(msg["params"]["protocolVersion"], crate::mcp::PROTOCOL_VERSION);
        let n = s.initializes.fetch_add(1, Ordering::SeqCst);
        let sid = format!("session-{n}");
        s.sessions.lock().unwrap().insert(sid.clone());
        extra_headers.push(sid);
        json!({ "protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "mock-openart", "version": "1"} })
    } else {
        let known = session.as_ref().is_some_and(|sid| s.sessions.lock().unwrap().contains(sid));
        if !known {
            return (StatusCode::NOT_FOUND, "session not found").into_response();
        }
        if let Some(p) = headers.get("mcp-protocol-version").and_then(|v| v.to_str().ok()) {
            s.protocol_headers.lock().unwrap().push(p.to_string());
        }
        if msg.get("id").is_none() {
            s.notifications.fetch_add(1, Ordering::SeqCst);
            return StatusCode::ACCEPTED.into_response();
        }
        assert_eq!(method, "tools/call");
        let name = msg["params"]["name"].as_str().unwrap_or("").to_string();
        let args = msg["params"]["arguments"].clone();
        s.calls.lock().unwrap().push((name.clone(), args.clone()));
        let r = tool_result(&name, tool(&s, &name, &args));
        if name == "openart_generate_image" {
            let d = s.generate_delay_ms.load(Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(d)).await;
        }
        r
    };
    let reply = json!({ "jsonrpc": "2.0", "id": msg["id"], "result": result });
    let mut resp = if s.sse.load(Ordering::SeqCst) {
        // A server notification first, then the response, split across CRLF lines.
        let body = format!(
            "event: message\ndata: {}\n\nevent: message\r\ndata: {}\r\n\r\n",
            json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"progress": 1}}),
            reply
        );
        ([(header::CONTENT_TYPE, "text/event-stream")], body).into_response()
    } else {
        Json(reply).into_response()
    };
    if let Some(sid) = extra_headers.pop() {
        resp.headers_mut().insert("mcp-session-id", sid.parse().unwrap());
    }
    resp
}

async fn cdn(State(s): State<Arc<OpenArtState>>, Path(name): Path<String>) -> Response {
    s.cdn_hits.fetch_add(1, Ordering::SeqCst);
    match s.cdn.lock().unwrap().get(&name) {
        Some(b) => ([(header::CONTENT_TYPE, "image/png")], b.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn upload(State(s): State<Arc<OpenArtState>>, Path(id): Path<String>, headers: HeaderMap, body: Bytes) -> StatusCode {
    let ct = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    assert!(headers.get(header::AUTHORIZATION).is_none(), "the presigned PUT must not carry the OpenArt token");
    s.cdn.lock().unwrap().insert(format!("{id}.png"), body.to_vec());
    s.puts.lock().unwrap().push((id, body.to_vec(), ct));
    StatusCode::OK
}

// ---- Anthropic ----

/// A scripted reply: status, extra headers, JSON body.
pub type Reply = (u16, Vec<(String, String)>, Value);

pub struct ClaudeState {
    /// (status, extra headers, body) replies in order; the last one repeats.
    pub replies: Mutex<VecDeque<Reply>>,
    pub requests: Mutex<Vec<(HashMap<String, String>, Value)>>,
}

pub struct MockClaude {
    pub base: String,
    pub state: Arc<ClaudeState>,
}

/// A Messages API response whose text is `text`.
pub fn message(text: &str) -> Value {
    json!({
        "id": "msg_test", "type": "message", "role": "assistant", "model": "claude-opus-5-5",
        "content": [{"type": "thinking", "thinking": "", "signature": "sig"}, {"type": "text", "text": text}],
        "stop_reason": "end_turn", "stop_details": null,
        "usage": {"input_tokens": 10, "output_tokens": 10}
    })
}

impl MockClaude {
    pub async fn start() -> MockClaude {
        let state = Arc::new(ClaudeState { replies: Mutex::default(), requests: Mutex::default() });
        let router = Router::new().route("/v1/messages", post(messages)).with_state(state.clone());
        MockClaude { base: spawn(router).await, state }
    }

    pub fn reply_text(&self, text: &str) {
        *self.state.replies.lock().unwrap() = VecDeque::from([(200, vec![], message(text))]);
    }

    pub fn replies(&self, replies: Vec<Reply>) {
        *self.state.replies.lock().unwrap() = replies.into();
    }

    pub fn last_request(&self) -> (HashMap<String, String>, Value) {
        self.state.requests.lock().unwrap().last().cloned().unwrap()
    }

    pub fn request_count(&self) -> usize {
        self.state.requests.lock().unwrap().len()
    }
}

async fn messages(State(s): State<Arc<ClaudeState>>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let h: HashMap<String, String> = headers.iter().map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string())).collect();
    s.requests.lock().unwrap().push((h, body));
    let (status, extra, reply) = {
        let mut r = s.replies.lock().unwrap();
        if r.len() > 1 { r.pop_front().unwrap() } else { r.front().cloned().unwrap_or((200, vec![], message("{}"))) }
    };
    let mut resp = (StatusCode::from_u16(status).unwrap(), Json(reply)).into_response();
    for (k, v) in extra {
        resp.headers_mut().insert(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
    }
    resp
}

// ---- Claude Code routine API trigger ----

/// A scripted fire reply: status, extra headers, body, delay before answering (ms).
pub type FireReply = (u16, Vec<(String, String)>, Value, u64);

pub struct RoutineState {
    pub replies: Mutex<VecDeque<FireReply>>,
    pub requests: Mutex<Vec<(HashMap<String, String>, Value)>>,
}

pub struct MockRoutine {
    pub url: String,
    pub state: Arc<RoutineState>,
}

pub const ROUTINE_TOKEN: &str = "sk-ant-oat01-test-token-SECRET-1234567890";

impl MockRoutine {
    pub async fn start() -> MockRoutine {
        let state = Arc::new(RoutineState { replies: Mutex::default(), requests: Mutex::default() });
        let router = Router::new().route("/v1/claude_code/routines/{id}/fire", post(fire)).with_state(state.clone());
        let base = spawn(router).await;
        MockRoutine { url: format!("{base}/v1/claude_code/routines/trig_01ABCDEFGHJKLMNOPQRSTUVW/fire"), state }
    }

    pub fn reply(&self, r: FireReply) {
        *self.state.replies.lock().unwrap() = VecDeque::from([r]);
    }

    pub fn count(&self) -> usize {
        self.state.requests.lock().unwrap().len()
    }

    pub fn last(&self) -> (HashMap<String, String>, Value) {
        self.state.requests.lock().unwrap().last().cloned().unwrap()
    }
}

pub fn fired(n: u32) -> FireReply {
    (
        200,
        vec![],
        json!({ "type": "routine_fire", "claude_code_session_id": format!("session_{n}"), "claude_code_session_url": format!("https://claude.ai/code/session_{n}") }),
        0,
    )
}

async fn fire(State(s): State<Arc<RoutineState>>, headers: HeaderMap, body: Bytes) -> Response {
    let h: HashMap<String, String> = headers.iter().map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string())).collect();
    let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    s.requests.lock().unwrap().push((h.clone(), v));
    if h.get("authorization").map(String::as_str) != Some(&format!("Bearer {ROUTINE_TOKEN}")) {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "type": "error", "error": { "type": "authentication_error", "message": "bad token" } })))
            .into_response();
    }
    let (status, extra, reply, delay) = {
        let r = s.replies.lock().unwrap();
        r.front().cloned().unwrap_or_else(|| fired(1))
    };
    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    let mut resp = (StatusCode::from_u16(status).unwrap(), Json(reply)).into_response();
    for (k, v) in extra {
        resp.headers_mut().insert(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
    }
    resp
}
