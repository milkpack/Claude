//! The HTTP server: the JSON API under `/api`, the OpenArt OAuth redirects, `/healthz`, and the
//! built front-end with SPA fallback, all on one port.

use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::multipart::MultipartRejection;
use axum::extract::rejection::QueryRejection;
use axum::extract::{ConnectInfo, DefaultBodyLimit, FromRequest, Multipart, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::response::{IntoResponse, Json, Redirect, Response};
use axum::routing::{get, post};
use log::{info, warn};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::assets::{AssetKind, Assets, NewAsset};
use crate::auth::{self, Auth};
use crate::claude::Claude;
use crate::error::{ApiError, ApiResult};
use crate::jobs::{self, CreateJob, JobTiming, Jobs};
use crate::oauth::{OAuth, OAuthConfig};
use crate::openart::{self, OpenArt};
use crate::skill::Skill;
use crate::workflows::{WorkflowInput, Workflows};
use crate::{director, imaging, review, static_files, store};

/// JSON request bodies.
pub const JSON_BODY_LIMIT: usize = 2 << 20;
/// Uploaded images.
pub const MAX_UPLOAD_BYTES: usize = 30 << 20;
/// The whole multipart request of an upload (file plus framing).
const UPLOAD_BODY_LIMIT: usize = MAX_UPLOAD_BYTES + (1 << 20);
/// `/api/me` waits at most this long for the OpenArt account summary.
const ACCOUNT_TIMEOUT: Duration = Duration::from_secs(5);

/// Everything the server needs.
pub struct Options {
    pub data_dir: PathBuf,
    /// Canonicalized front-end directory.
    pub static_dir: Option<PathBuf>,
    pub public_url: String,
    pub password: String,
    pub openart_mcp_url: String,
    pub openart_issuer: Option<String>,
    pub anthropic_base_url: String,
    pub anthropic_api_key: Option<String>,
    pub claude_model: String,
    pub claude_effort: String,
    pub skill: Skill,
    pub trust_proxy: bool,
    pub timing: JobTiming,
    /// Accept plain http and private addresses for upstream URLs (tests against local mocks
    /// only; always `false` in the binary).
    pub allow_private_urls: bool,
    /// Routine URL and token from the environment (settings saved in the app win).
    pub routine_env: Option<(String, String)>,
    /// How long a routine fire may take.
    pub routine_timeout: Duration,
    /// Outbound proxy for Anthropic traffic only (`PHOTO_STUDIO_ANTHROPIC_PROXY`).
    pub anthropic_proxy: Option<String>,
}

/// Shortest accepted owner password.
pub const MIN_PASSWORD_CHARS: usize = 12;

pub struct App {
    pub auth: Auth,
    pub openart: OpenArt,
    pub assets: Assets,
    pub jobs: Jobs,
    pub workflows: Workflows,
    pub claude: Option<Claude>,
    pub claude_model: String,
    pub routing: HashMap<String, Vec<String>>,
    pub director_system: String,
    pub review_system: String,
    /// For URLs that came from upstream (CDN downloads): no credentials, checked redirects.
    pub fetch: reqwest::Client,
    pub allow_private_urls: bool,
    pub static_dir: Option<PathBuf>,
    /// Set to `true` when the server stops; background pollers exit on it.
    pub stop: tokio::sync::watch::Sender<bool>,
    pub tasks: crate::tasks::Tasks,
    /// External base URL, without a trailing slash (absolute links in task prompts).
    pub public_url: String,
    /// The photo skill as one text block (appended to some task prompts).
    pub skill_text: String,
    /// Requests to the public `/t/*` routes, per client.
    pub token_limit: auth::WindowLimiter,
    /// Unknown or wrong tokens, per client.
    pub bad_token_limit: auth::WindowLimiter,
    pub routine: crate::routine::Routine,
}

/// Public `/t/*` requests allowed per client per minute.
pub const TOKEN_REQUESTS_PER_MINUTE: usize = 120;
/// Wrong tokens allowed per client per minute (then 429 for every `/t/*` request).
pub const BAD_TOKENS_PER_MINUTE: usize = 10;

/// Create the data directories and the application state.
pub fn build(opts: Options) -> Result<Arc<App>, String> {
    if opts.password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(format!("the owner password must have at least {MIN_PASSWORD_CHARS} characters"));
    }
    let secrets = opts.data_dir.join("secrets");
    store::ensure_dir(&opts.data_dir, false)?;
    store::ensure_dir(&secrets, true)?;
    // OpenArt: never through a proxy (environment proxy variables included).
    let http = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(15))
        .user_agent(concat!("photo-studio/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("HTTP client: {e}"))?;
    let fetch = crate::net::fetch_client(opts.allow_private_urls)?;
    let anthropic = Arc::new(crate::net::AnthropicHttp::new(opts.anthropic_proxy.clone()).map_err(|e| format!("PHOTO_STUDIO_ANTHROPIC_PROXY: {e}"))?);
    let public = opts.public_url.trim_end_matches('/').to_string();
    let oauth = OAuth::load(
        OAuthConfig {
            mcp_url: opts.openart_mcp_url.clone(),
            issuer: opts.openart_issuer.clone(),
            redirect_uri: format!("{public}/oauth/openart/callback"),
            allow_insecure: opts.allow_private_urls,
        },
        http.clone(),
        secrets.join("openart.json"),
    );
    let claude = opts
        .anthropic_api_key
        .clone()
        .map(|key| Claude::new(anthropic.clone(), opts.anthropic_base_url.clone(), key, opts.claude_model.clone(), opts.claude_effort.clone()));
    Ok(Arc::new(App {
        auth: Auth::load(&opts.password, secrets.join("sessions.json"), public.starts_with("https://"), opts.trust_proxy),
        openart: OpenArt::new(oauth, opts.openart_mcp_url, http.clone(), fetch.clone(), opts.allow_private_urls),
        assets: Assets::load(opts.data_dir.join("assets"))?,
        jobs: Jobs::load(opts.data_dir.join("jobs"), opts.timing)?,
        workflows: Workflows::new(opts.data_dir.join("workflows"))?,
        claude,
        claude_model: opts.claude_model,
        routing: opts.skill.routing(),
        director_system: director::system_prompt(&opts.skill),
        review_system: review::system_prompt(&opts.skill),
        fetch,
        allow_private_urls: opts.allow_private_urls,
        static_dir: opts.static_dir,
        stop: tokio::sync::watch::channel(false).0,
        tasks: crate::tasks::Tasks::load(opts.data_dir.join("tasks"))?,
        skill_text: opts.skill.prompt_text(),
        public_url: public,
        token_limit: auth::WindowLimiter::new(TOKEN_REQUESTS_PER_MINUTE, 60_000),
        bad_token_limit: auth::WindowLimiter::new(BAD_TOKENS_PER_MINUTE, 60_000),
        routine: crate::routine::Routine::load(
            secrets.join("routine.json"),
            opts.routine_env,
            opts.allow_private_urls,
            opts.routine_timeout,
            anthropic,
            opts.anthropic_base_url.clone(),
        )?,
    }))
}

pub fn router(app: Arc<App>) -> Router {
    let protected = Router::new()
        .route("/api/openart/disconnect", post(disconnect))
        .route("/oauth/openart/start", get(oauth_start))
        .route("/api/models", get(models))
        .route("/api/models/{model}/{mode}/form", get(form))
        .route("/api/cost", post(cost))
        .route("/api/assets", get(list_assets).post(upload_asset).layer(DefaultBodyLimit::max(UPLOAD_BODY_LIMIT)))
        .route("/api/assets/{id}", get(get_asset).delete(delete_asset))
        .route("/api/assets/{id}/file", get(asset_file))
        .route("/api/assets/{id}/thumb", get(asset_thumb))
        .route("/api/jobs", get(list_jobs).post(create_job))
        .route("/api/jobs/{id}", get(get_job))
        .route("/api/director", post(director_handler))
        .route("/api/review", post(review_handler))
        .route("/api/workflows", get(list_workflows).post(create_workflow))
        .route("/api/workflows/{id}", get(get_workflow).put(put_workflow).delete(delete_workflow))
        .route("/api/workflows/{id}/duplicate", post(duplicate_workflow))
        .route("/api/tasks", get(crate::handoff::list_tasks).post(crate::handoff::create_task))
        .route("/api/tasks/{id}", get(crate::handoff::get_task))
        .route("/api/tasks/{id}/result", post(crate::handoff::task_result).layer(DefaultBodyLimit::max(crate::tasks::RESULT_BODY_LIMIT)))
        .route("/api/tasks/{id}/cancel", post(crate::handoff::cancel_task))
        .route("/api/tasks/{id}/fire", post(crate::handoff::fire_task))
        .route("/api/settings/routine", get(crate::handoff::routine_status).post(crate::handoff::routine_save).delete(crate::handoff::routine_clear))
        .route("/api/settings/routine/check", get(crate::handoff::routine_check))
        .route("/api/tasks/{id}/files/{name}", get(crate::handoff::task_file))
        .route_layer(from_fn_with_state(app.clone(), auth::require_session));
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/me", get(me))
        .route("/api/login", post(login))
        // Public so a stale cookie can always be cleared; signing out everywhere needs a session.
        .route("/api/logout", post(logout))
        .route("/oauth/openart/callback", get(oauth_callback))
        // Token-authorised (never by the session cookie).
        .route(
            "/t/{token}",
            get(crate::handoff::token_page).post(crate::handoff::token_result).layer(DefaultBodyLimit::max(crate::tasks::RESULT_BODY_LIMIT)),
        )
        .route("/t/{token}/result", post(crate::handoff::token_result).layer(DefaultBodyLimit::max(crate::tasks::RESULT_BODY_LIMIT)))
        .route("/t/{token}/inputs/{asset_id}", get(crate::handoff::token_input))
        .merge(protected)
        .fallback(fallback)
        .layer(from_fn(auth::csrf_guard))
        .layer(DefaultBodyLimit::max(JSON_BODY_LIMIT))
        .with_state(app)
}

/// Resume unfinished jobs, then serve on `listener` until `shutdown` completes.
pub async fn serve(listener: TcpListener, app: Arc<App>, shutdown: impl Future<Output = ()> + Send + 'static) -> std::io::Result<()> {
    jobs::resume(&app).await;
    let result =
        axum::serve(listener, router(app.clone()).into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(shutdown).await;
    // Unfinished jobs resume from disk on the next start.
    app.stop.send_replace(true);
    result
}

/// A JSON body (at most [`JSON_BODY_LIMIT`]) with API-shaped errors.
pub struct JsonBody<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for JsonBody<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, ApiError> {
        let bytes = Bytes::from_request(req, state).await.map_err(|e| {
            if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
                ApiError::too_large(format!("request bodies are limited to {} MB", JSON_BODY_LIMIT >> 20))
            } else {
                ApiError::bad_request(e.body_text())
            }
        })?;
        let body: &[u8] = if bytes.iter().all(u8::is_ascii_whitespace) { b"{}" } else { &bytes };
        serde_json::from_slice(body).map(JsonBody).map_err(|e| ApiError::bad_request(format!("invalid request body: {e}")))
    }
}

pub fn check_id(id: &str, what: &str) -> ApiResult<()> {
    if store::valid_id(id) { Ok(()) } else { Err(ApiError::not_found(format!("no such {what}"))) }
}

// ---- session and account ----

#[derive(Deserialize)]
struct LoginBody {
    #[serde(default)]
    password: String,
}

async fn me_json(app: &App, authenticated: bool) -> Value {
    if !authenticated {
        return json!({
            "authenticated": false,
            "openart": { "connected": false, "needsReconnect": false },
            "claude": { "configured": false, "model": "" },
            "routine": { "configured": false },
        });
    }
    let (connected, needs_reconnect) = app.openart.oauth.status().await;
    let mut openart = json!({ "connected": connected, "needsReconnect": needs_reconnect });
    if connected && let Ok(Some(acc)) = tokio::time::timeout(ACCOUNT_TIMEOUT, app.openart.account()).await {
        let email = acc.get("user").and_then(|u| u.get("email")).or_else(|| acc.get("email")).and_then(Value::as_str);
        if let Some(email) = email {
            openart["email"] = json!(email);
        }
        if let Some(plan) = acc.get("plan").and_then(Value::as_str) {
            openart["plan"] = json!(plan);
        }
        if let Some(credits) = acc.get("credits").and_then(Value::as_f64) {
            openart["credits"] = json!(credits);
        }
    }
    json!({
        "authenticated": true,
        "openart": openart,
        "claude": { "configured": app.claude.is_some(), "model": app.claude_model },
        "routine": { "configured": app.routine.configured() },
    })
}

async fn me(State(app): State<Arc<App>>, headers: HeaderMap) -> Json<Value> {
    let authenticated = app.auth.valid_headers(&headers);
    Json(me_json(&app, authenticated).await)
}

async fn login(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    JsonBody(body): JsonBody<LoginBody>,
) -> Response {
    let ip = app.auth.client_ip(peer, &headers);
    let Some(attempt) = app.auth.begin_attempt(ip) else {
        warn!("login: too many failed attempts (from {ip} or overall)");
        return ApiError::rate_limited("too many failed sign-in attempts; wait a minute").into_response();
    };
    if !app.auth.check_password(&body.password) {
        warn!("login: wrong password from {ip}");
        return ApiError::unauthenticated("wrong password").into_response();
    }
    app.auth.forgive(attempt);
    let token = app.auth.create_session().await;
    info!("login from {ip}");
    let mut resp = Json(me_json(&app, true).await).into_response();
    if let Ok(v) = HeaderValue::from_str(&app.auth.set_cookie(&token)) {
        resp.headers_mut().insert(header::SET_COOKIE, v);
    }
    resp
}

#[derive(Deserialize)]
struct LogoutQuery {
    all: Option<String>,
}

async fn logout(State(app): State<Arc<App>>, headers: HeaderMap, q: Result<Query<LogoutQuery>, QueryRejection>) -> Response {
    let all = q.ok().and_then(|Query(q)| q.all).is_some_and(|a| a == "1" || a == "true");
    if all && app.auth.valid_headers(&headers) {
        info!("signing out every session");
        app.auth.destroy_all().await;
    } else if let Some(t) = auth::session_token(&headers) {
        app.auth.destroy(&t).await;
    }
    let mut resp = Json(json!({})).into_response();
    if let Ok(v) = HeaderValue::from_str(&app.auth.clear_cookie()) {
        resp.headers_mut().insert(header::SET_COOKIE, v);
    }
    resp
}

// ---- OpenArt connection ----

async fn oauth_start(State(app): State<Arc<App>>) -> Response {
    match app.openart.oauth.start().await {
        Ok(url) => Redirect::to(&url).into_response(),
        Err(e) => {
            warn!("OpenArt connect: {}", e.message);
            Redirect::to("/?openart=error").into_response()
        }
    }
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// The OAuth redirect target. No session is required: the browser arrives here from OpenArt (a
/// cross-site navigation, so the SameSite=Strict cookie isn't sent); the one-time `state` from an
/// authenticated `/oauth/openart/start` is what authorizes it.
async fn oauth_callback(State(app): State<Arc<App>>, q: Result<Query<CallbackQuery>, QueryRejection>) -> Response {
    let Ok(Query(q)) = q else { return Redirect::to("/?openart=error").into_response() };
    if let Some(err) = q.error {
        warn!("OpenArt connect: authorization refused ({})", crate::net::snippet(err.as_bytes()));
        return Redirect::to("/?openart=error").into_response();
    }
    let (Some(code), Some(state)) = (q.code, q.state) else { return Redirect::to("/?openart=error").into_response() };
    if code.is_empty() || code.len() > 4096 || state.is_empty() || state.len() > 256 {
        return Redirect::to("/?openart=error").into_response();
    }
    match app.openart.oauth.finish(&code, &state).await {
        Ok(()) => {
            app.openart.reset().await;
            // Jobs that stopped while OpenArt was unreachable can finish now.
            jobs::revive(&app).await;
            Redirect::to("/?openart=connected").into_response()
        }
        Err(e) => {
            warn!("OpenArt connect: {}", e.message);
            Redirect::to("/?openart=error").into_response()
        }
    }
}

async fn disconnect(State(app): State<Arc<App>>) -> ApiResult<Json<Value>> {
    app.openart.disconnect().await?;
    Ok(Json(me_json(&app, true).await))
}

// ---- models ----

#[derive(Deserialize)]
struct ModelsQuery {
    refresh: Option<String>,
}

async fn models(State(app): State<Arc<App>>, q: Result<Query<ModelsQuery>, QueryRejection>) -> ApiResult<Json<Value>> {
    let refresh = q.ok().and_then(|Query(q)| q.refresh).is_some_and(|r| r == "1" || r == "true");
    let raw = app.openart.model_list(refresh).await?;
    Ok(Json(openart::models_response(&raw, &app.routing)))
}

fn valid_model_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

async fn form(State(app): State<Arc<App>>, Path((model, mode)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    if !valid_model_name(&model) || !valid_model_name(&mode) {
        return Err(ApiError::bad_request("invalid model or mode"));
    }
    let schema = app.openart.form(&model, &mode).await?;
    Ok(Json(json!({ "schema": schema })))
}

#[derive(Deserialize)]
struct CostBody {
    model: String,
    mode: String,
    #[serde(default)]
    params: Option<Value>,
}

async fn cost(State(app): State<Arc<App>>, JsonBody(body): JsonBody<CostBody>) -> ApiResult<Json<Value>> {
    if !valid_model_name(&body.model) || !valid_model_name(&body.mode) {
        return Err(ApiError::bad_request("invalid model or mode"));
    }
    let params = body.params.unwrap_or_else(|| json!({}));
    if !params.is_object() {
        return Err(ApiError::bad_request("params must be an object"));
    }
    let q = app.openart.cost(&body.model, &body.mode, &params).await?;
    Ok(Json(json!({ "credits": q.credits, "config": q.config, "affordable": q.affordable })))
}

// ---- assets ----

fn safe_file_name(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String =
        base.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ')).take(128).collect::<String>().trim().to_string();
    (!cleaned.is_empty() && !cleaned.starts_with('.')).then_some(cleaned)
}

async fn upload_asset(State(app): State<Arc<App>>, multipart: Result<Multipart, MultipartRejection>) -> ApiResult<Json<Value>> {
    let mut mp = multipart.map_err(|e| ApiError::bad_request(e.body_text()))?;
    let too_large = || ApiError::too_large(format!("uploads are limited to {} MB", MAX_UPLOAD_BYTES >> 20));
    let mut file: Option<(Vec<u8>, Option<String>)> = None;
    loop {
        let field = mp
            .next_field()
            .await
            .map_err(|e| if e.status() == StatusCode::PAYLOAD_TOO_LARGE { too_large() } else { ApiError::bad_request(e.body_text()) })?;
        let Some(mut field) = field else { break };
        if field.name() != Some("file") {
            continue;
        }
        let name = field.file_name().and_then(safe_file_name);
        let mut bytes = Vec::new();
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|e| if e.status() == StatusCode::PAYLOAD_TOO_LARGE { too_large() } else { ApiError::bad_request(e.body_text()) })?
        {
            if bytes.len() + chunk.len() > MAX_UPLOAD_BYTES {
                // Read the rest (bounded by the request body limit) so the client gets a clean
                // 413 instead of a connection reset mid-upload.
                while let Ok(Some(_)) = field.chunk().await {}
                return Err(too_large());
            }
            bytes.extend_from_slice(&chunk);
        }
        file = Some((bytes, name));
        break;
    }
    let (bytes, name) = file.ok_or_else(|| ApiError::bad_request("no `file` field in the upload"))?;
    let format = imaging::sniff(&bytes).ok_or_else(|| ApiError::unsupported_media("only PNG, JPEG and WebP images can be uploaded"))?;
    let asset = app.assets.create(AssetKind::Upload, bytes.clone(), NewAsset { file_name: name.clone(), ..NewAsset::default() }).await?;
    // Push it to OpenArt now so it can be used as a reference. If that isn't possible (not
    // connected, a transient failure), the job that first uses it uploads it.
    let mut asset = asset;
    if app.openart.oauth.status().await.0 {
        let label = name.unwrap_or_else(|| format!("{}.{}", asset.id, format.ext()));
        match app.openart.upload(bytes, format.mime(), &label, &label).await {
            Ok(r) => asset = app.assets.set_openart(&asset.id, r).await?,
            Err(e) => warn!("asset {}: OpenArt upload failed: {}", asset.id, e.message),
        }
    }
    Ok(Json(json!(asset.api())))
}

#[derive(Deserialize)]
struct ListQuery {
    cursor: Option<String>,
    limit: Option<String>,
}

async fn list_assets(State(app): State<Arc<App>>, q: Result<Query<ListQuery>, QueryRejection>) -> ApiResult<Json<Value>> {
    let Query(q) = q.map_err(|e| ApiError::bad_request(e.body_text()))?;
    let limit = match q.limit.as_deref().filter(|l| !l.is_empty()) {
        Some(l) => l.parse::<usize>().map_err(|_| ApiError::bad_request("limit must be a number"))?,
        None => crate::assets::DEFAULT_PAGE,
    };
    let (page, next) = app.assets.list(q.cursor.as_deref(), limit)?;
    let mut out = json!({ "assets": page.iter().map(|a| a.api()).collect::<Vec<_>>() });
    if let Some(n) = next {
        out["next"] = json!(n);
    }
    Ok(Json(out))
}

fn find_asset(app: &App, id: &str) -> ApiResult<crate::assets::StoredAsset> {
    check_id(id, "asset")?;
    app.assets.get(id).ok_or_else(|| ApiError::not_found("no such asset"))
}

async fn get_asset(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(find_asset(&app, &id)?.api())))
}

async fn delete_asset(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    check_id(&id, "asset")?;
    if app.assets.delete(&id).await { Ok(Json(json!({}))) } else { Err(ApiError::not_found("no such asset")) }
}

pub fn image_response(bytes: Vec<u8>, mime: &'static str) -> Response {
    let mut resp = Response::new(Body::from(bytes));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=31536000, immutable"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    resp
}

async fn asset_file(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult<Response> {
    let a = find_asset(&app, &id)?;
    let bytes = app.assets.read_file(&a).await?;
    Ok(image_response(bytes, a.format().mime()))
}

async fn asset_thumb(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult<Response> {
    let a = find_asset(&app, &id)?;
    let bytes = app.assets.read_thumb(&a.id).await?;
    Ok(image_response(bytes, "image/jpeg"))
}

// ---- jobs ----

#[derive(Deserialize)]
struct JobsQuery {
    active: Option<String>,
}

async fn list_jobs(State(app): State<Arc<App>>, q: Result<Query<JobsQuery>, QueryRejection>) -> Json<Value> {
    let active = q.ok().and_then(|Query(q)| q.active).is_some_and(|a| a == "1" || a == "true");
    let jobs: Vec<Value> = app.jobs.list(active).iter().map(|j| jobs::job_json(&app, j)).collect();
    Json(json!({ "jobs": jobs }))
}

async fn get_job(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    check_id(&id, "job")?;
    let job = app.jobs.get(&id).ok_or_else(|| ApiError::not_found("no such job"))?;
    Ok(Json(jobs::job_json(&app, &job)))
}

async fn create_job(State(app): State<Arc<App>>, JsonBody(body): JsonBody<CreateJob>) -> ApiResult<Json<Value>> {
    let job = jobs::create(app.clone(), body).await?;
    Ok(Json(jobs::job_json(&app, &job)))
}

// ---- director and review ----

async fn director_handler(State(app): State<Arc<App>>, JsonBody(body): JsonBody<director::DirectorRequest>) -> ApiResult<Json<Value>> {
    let resp = director::run(app, body).await?;
    Ok(Json(json!(resp)))
}

async fn review_handler(State(app): State<Arc<App>>, JsonBody(body): JsonBody<review::ReviewRequest>) -> ApiResult<Json<Value>> {
    let resp = review::run(app, body).await?;
    Ok(Json(json!(resp)))
}

// ---- workflows ----

async fn list_workflows(State(app): State<Arc<App>>) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "workflows": app.workflows.list().await? })))
}

async fn create_workflow(State(app): State<Arc<App>>, JsonBody(body): JsonBody<WorkflowInput>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(app.workflows.create(body).await?)))
}

async fn get_workflow(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    check_id(&id, "workflow")?;
    Ok(Json(json!(app.workflows.open(&id).await?)))
}

#[derive(Deserialize)]
struct DuplicateBody {
    #[serde(default)]
    name: Option<String>,
}

async fn duplicate_workflow(State(app): State<Arc<App>>, Path(id): Path<String>, JsonBody(body): JsonBody<DuplicateBody>) -> ApiResult<Json<Value>> {
    check_id(&id, "workflow")?;
    Ok(Json(json!(app.workflows.duplicate(&id, body.name).await?)))
}

async fn put_workflow(State(app): State<Arc<App>>, Path(id): Path<String>, JsonBody(body): JsonBody<WorkflowInput>) -> ApiResult<Json<Value>> {
    check_id(&id, "workflow")?;
    Ok(Json(json!(app.workflows.update(&id, body).await?)))
}

async fn delete_workflow(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    app.workflows.delete(&id).await?;
    Ok(Json(json!({})))
}

// ---- everything else ----

async fn fallback(State(app): State<Arc<App>>, method: Method, uri: Uri) -> Response {
    let path = uri.path();
    if path.starts_with("/api/") || path == "/api" || path.starts_with("/oauth/") {
        return ApiError::not_found("no such endpoint").into_response();
    }
    match &app.static_dir {
        Some(root) => static_files::serve(root, &method, path).await,
        None => (StatusCode::NOT_FOUND, "not found (no front-end is configured)").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names() {
        assert_eq!(safe_file_name("C:\\Users\\me\\photo 1.png").as_deref(), Some("photo 1.png"));
        assert_eq!(safe_file_name("../../etc/passwd").as_deref(), Some("passwd"));
        assert_eq!(safe_file_name(".hidden"), None);
        assert_eq!(safe_file_name("ünï.jpg").as_deref(), Some("n.jpg"));
        assert!(valid_model_name("nano-banana-2-1") && !valid_model_name("../x"));
    }
}
