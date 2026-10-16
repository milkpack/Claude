//! HTTP handlers of handoff mode: `/api/tasks/*` (session) and the public, token-authorised
//! `/t/<token>` routes (a plain HTML page with the task, its inputs and an upload form; the input
//! files; returning a result). The token routes never look at the session cookie, and every
//! response from them is `no-store`, `no-referrer` (the token is in the URL) and `noindex`.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::rejection::QueryRejection;
use axum::extract::{ConnectInfo, FromRequest, Multipart, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Json, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{ApiError, ApiResult};
use crate::server::{App, JsonBody, check_id, image_response};
use crate::tasks::{self, Incoming, JsonResult, StoredTask, TaskStatus};

// ---- session routes ----

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksQuery {
    session_id: Option<String>,
    status: Option<String>,
}

pub async fn list_tasks(State(app): State<Arc<App>>, q: Result<Query<TasksQuery>, QueryRejection>) -> ApiResult<Json<Value>> {
    let Query(q) = q.map_err(|e| ApiError::bad_request(e.body_text()))?;
    let status = match q.status.as_deref().filter(|s| !s.is_empty()) {
        None => None,
        Some("open") => Some(TaskStatus::Open),
        Some("done") => Some(TaskStatus::Done),
        Some("cancelled") => Some(TaskStatus::Cancelled),
        Some(other) => return Err(ApiError::bad_request(format!("unknown status `{other}`"))),
    };
    let session = q.session_id.filter(|s| !s.is_empty());
    let list: Vec<Value> = app.tasks.list(session.as_deref(), status).iter().map(|t| tasks::task_json(&app, t)).collect();
    Ok(Json(json!({ "tasks": list })))
}

pub async fn create_task(State(app): State<Arc<App>>, JsonBody(body): JsonBody<tasks::CreateTask>) -> ApiResult<Json<Value>> {
    let t = tasks::create(&app, body).await?;
    Ok(Json(tasks::task_json(&app, &t)))
}

fn find(app: &App, id: &str) -> ApiResult<StoredTask> {
    check_id(id, "task")?;
    app.tasks.get(id).ok_or_else(|| ApiError::not_found("no such task"))
}

pub async fn get_task(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(tasks::task_json(&app, &find(&app, &id)?)))
}

pub async fn cancel_task(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    find(&app, &id)?;
    let t = tasks::cancel(&app, &id).await?;
    Ok(Json(tasks::task_json(&app, &t)))
}

/// Read a result body: JSON `{text?, json?}` or multipart (`text`, `json`, `files`).
async fn incoming(app: &App, t: &StoredTask, req: Request) -> ApiResult<Incoming> {
    let ct = req.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
    if ct.starts_with("multipart/form-data") {
        let mp = Multipart::from_request(req, &()).await.map_err(|e| ApiError::bad_request(e.body_text()))?;
        tasks::read_multipart(app, t, mp).await
    } else if ct.starts_with("application/json") {
        let JsonBody(body) = JsonBody::<JsonResult>::from_request(req, &()).await?;
        Ok(body.incoming())
    } else {
        Err(ApiError::unsupported_media("send multipart/form-data (text, json, files) or application/json ({text, json})"))
    }
}

pub async fn task_result(State(app): State<Arc<App>>, Path(id): Path<String>, req: Request) -> ApiResult<Json<Value>> {
    let t = find(&app, &id)?;
    tasks::check_open(&t)?;
    let inc = incoming(&app, &t, req).await?;
    let t = tasks::apply(&app, &t.id, inc).await?;
    Ok(Json(tasks::task_json(&app, &t)))
}

pub async fn task_file(State(app): State<Arc<App>>, Path((id, name)): Path<(String, String)>) -> ApiResult<Response> {
    let t = find(&app, &id)?;
    let known = t.result.as_ref().is_some_and(|r| r.files.iter().any(|f| f.name == name));
    if !known {
        return Err(ApiError::not_found("no such file"));
    }
    let path = app.tasks.attachment_path(&t.id, &name);
    let bytes = tokio::task::spawn_blocking(move || crate::store::read_capped(&path, tasks::MAX_RESULT_FILE as u64 + 1))
        .await
        .map_err(|e| ApiError::internal(format!("read task: {e}")))?
        .map_err(|_| ApiError::internal("can't read the file"))?
        .ok_or_else(|| ApiError::not_found("the file is missing"))?;
    let mut resp = Response::new(axum::body::Body::from(bytes));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(resp)
}

pub async fn fire_task(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    find(&app, &id)?;
    let t = tasks::fire(&app, &id).await?;
    Ok(Json(tasks::task_json(&app, &t)))
}

// ---- cloud run settings (the token never comes back) ----

pub async fn routine_status(State(app): State<Arc<App>>) -> Json<Value> {
    Json(app.routine.status())
}

#[derive(Deserialize)]
pub struct RoutineBody {
    url: String,
    token: String,
}

pub async fn routine_save(State(app): State<Arc<App>>, JsonBody(body): JsonBody<RoutineBody>) -> ApiResult<Json<Value>> {
    app.routine.save(&body.url, &body.token).await?;
    Ok(Json(app.routine.status()))
}

pub async fn routine_clear(State(app): State<Arc<App>>) -> ApiResult<Json<Value>> {
    app.routine.clear().await?;
    Ok(Json(app.routine.status()))
}

// ---- token routes ----

fn harden(mut resp: Response, html: bool) -> Response {
    let h = resp.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert("x-robots-tag", HeaderValue::from_static("noindex, nofollow"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    if html {
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
            ),
        );
    }
    resp
}

fn wants_html(headers: &HeaderMap) -> bool {
    headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()).is_some_and(|a| a.contains("text/html"))
}

fn token_error(e: ApiError, html: bool) -> Response {
    if html {
        let body = page("Photo Studio", &format!("<p class=\"error\">{}</p>", esc(&e.message)));
        harden((e.status, Html(body)).into_response(), true)
    } else {
        harden(e.into_response(), false)
    }
}

/// The task of `token` for a request from `peer`, after the rate limits.
fn authorize(app: &App, peer: SocketAddr, headers: &HeaderMap, token: &str) -> ApiResult<StoredTask> {
    let ip = app.auth.client_ip(peer, headers);
    if !app.bad_token_limit.allowed(ip) || !app.token_limit.hit(ip) {
        return Err(ApiError::rate_limited("too many requests; wait a minute"));
    }
    let Some(t) = app.tasks.by_token(token) else {
        app.bad_token_limit.hit(ip);
        return Err(ApiError::not_found("this link is not valid"));
    };
    if t.expired() {
        return Err(ApiError::new(StatusCode::GONE, "not_found", "this link has expired"));
    }
    Ok(t)
}

pub async fn token_page(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Response {
    match authorize(&app, peer, &headers, &token) {
        Ok(t) => harden(Html(task_page(&t, None)).into_response(), true),
        Err(e) => token_error(e, true),
    }
}

#[derive(Deserialize)]
pub struct InputQuery {
    thumb: Option<String>,
}

pub async fn token_input(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path((token, asset_id)): Path<(String, String)>,
    q: Result<Query<InputQuery>, QueryRejection>,
) -> Response {
    let run = async {
        let t = authorize(&app, peer, &headers, &token)?;
        // Only this task's own inputs are reachable through its token.
        if !t.inputs.iter().any(|i| i.asset_id == asset_id) {
            return Err(ApiError::not_found("no such input"));
        }
        let a = app.assets.get(&asset_id).ok_or_else(|| ApiError::not_found("the input was deleted"))?;
        let thumb = q.ok().and_then(|Query(q)| q.thumb).is_some_and(|v| v == "1");
        let resp = if thumb {
            image_response(app.assets.read_thumb(&a.id).await?, "image/jpeg")
        } else {
            image_response(app.assets.read_file(&a).await?, a.format().mime())
        };
        Ok(resp)
    };
    match run.await {
        Ok(resp) => {
            let mut r = harden(resp, false);
            r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
            r
        }
        Err(e) => token_error(e, false),
    }
}

pub async fn token_result(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(token): Path<String>,
    req: Request,
) -> Response {
    let headers = req.headers().clone();
    let html = wants_html(&headers);
    let run = async {
        let t = authorize(&app, peer, &headers, &token)?;
        tasks::check_open(&t)?;
        let inc = incoming(&app, &t, req).await?;
        tasks::apply(&app, &t.id, inc).await
    };
    match run.await {
        Ok(t) if html => harden(Html(task_page(&t, Some("Thank you: the result was received."))).into_response(), true),
        Ok(t) => {
            let r = t.result.as_ref();
            let body = json!({
                "ok": true,
                "status": t.status,
                "images": r.map(|r| r.asset_ids.len()).unwrap_or(0),
                "files": r.map(|r| r.files.len()).unwrap_or(0),
                "json": r.is_some_and(|r| r.json.is_some()),
                "error": r.and_then(|r| r.error.clone()),
            });
            harden(Json(body).into_response(), false)
        }
        Err(e) => token_error(e, html),
    }
}

// ---- the page ----

/// HTML-escape `s`.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

const STYLE: &str = "\
:root{color-scheme:light dark;--bg:#f2f2f2;--fg:#222;--muted:#666;--card:#fff;--line:#d4d4d4;--accent:#444}\
@media (prefers-color-scheme:dark){:root{--bg:#1b1b1b;--fg:#e6e6e6;--muted:#9a9a9a;--card:#262626;--line:#3a3a3a;--accent:#cfcfcf}}\
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 system-ui,-apple-system,Segoe UI,sans-serif}\
main{max-width:860px;margin:0 auto;padding:24px 16px 48px}h1{font-size:22px;margin:0 0 4px}h2{font-size:16px;margin:24px 0 8px}\
.muted{color:var(--muted);font-size:13px}.card{background:var(--card);border:1px solid var(--line);border-radius:8px;padding:16px;margin:12px 0}\
pre{white-space:pre-wrap;word-break:break-word;font:13px/1.45 ui-monospace,SFMono-Regular,Menlo,monospace;margin:0;max-height:480px;overflow:auto}\
.inputs{display:grid;grid-template-columns:repeat(auto-fill,minmax(150px,1fr));gap:12px}.inputs a{color:var(--fg);text-decoration:none;font-size:13px}\
.inputs img{width:100%;height:120px;object-fit:cover;border-radius:6px;border:1px solid var(--line);background:var(--bg);display:block;margin-bottom:4px}\
textarea{width:100%;min-height:160px;font:13px ui-monospace,Menlo,monospace;padding:8px;border:1px solid var(--line);border-radius:6px;background:var(--bg);color:var(--fg)}\
button{margin-top:12px;padding:8px 18px;border:0;border-radius:6px;background:var(--accent);color:var(--bg);font-weight:600;cursor:pointer}\
.notice{border-left:4px solid var(--accent);padding:8px 12px;background:var(--card)}.error{color:#b33;font-weight:600}";

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<meta name=\"robots\" content=\"noindex\"><title>{}</title><style>{STYLE}</style></head><body><main>{body}</main></body></html>",
        esc(title)
    )
}

fn task_page(t: &StoredTask, notice: Option<&str>) -> String {
    let token = esc(&t.token);
    let mut b = String::new();
    if let Some(n) = notice {
        b.push_str(&format!("<p class=\"notice\">{}</p>", esc(n)));
    }
    let status = match t.status {
        TaskStatus::Open => "open",
        TaskStatus::Done => "answered",
        TaskStatus::Cancelled => "cancelled",
    };
    b.push_str(&format!(
        "<h1>{}</h1><p class=\"muted\">Photo Studio task · {status} · link valid until {}</p>",
        esc(&t.title),
        esc(&crate::prompts::utc(t.expires_at))
    ));
    if !t.inputs.is_empty() {
        b.push_str("<h2>Inputs</h2><div class=\"inputs\">");
        for (i, inp) in t.inputs.iter().enumerate() {
            let id = esc(&inp.asset_id);
            let role = inp.role.as_deref().map(esc).unwrap_or_else(|| "reference".into());
            b.push_str(&format!(
                "<a href=\"/t/{token}/inputs/{id}\" download=\"input{n}\"><img src=\"/t/{token}/inputs/{id}?thumb=1\" alt=\"input {n}\">image {n} · {role}</a>",
                n = i + 1
            ));
        }
        b.push_str("</div>");
    }
    b.push_str(&format!("<h2>Task</h2><div class=\"card\"><pre>{}</pre></div>", esc(&t.prompt)));
    if let Some(r) = &t.result {
        let mut summary = format!("{} image(s), {} other file(s)", r.asset_ids.len(), r.files.len());
        if r.json.is_some() {
            summary.push_str(", JSON accepted");
        }
        b.push_str(&format!("<h2>Answer received</h2><p class=\"muted\">{} · {}</p>", esc(&summary), esc(&crate::prompts::utc(r.at))));
        if let Some(e) = &r.error {
            b.push_str(&format!("<p class=\"error\">{}</p>", esc(e)));
        }
    }
    if t.status == TaskStatus::Cancelled {
        b.push_str("<p class=\"error\">This task was cancelled; results are no longer accepted.</p>");
    } else {
        b.push_str(&format!(
            "<h2>Return the result</h2><form class=\"card\" method=\"post\" action=\"/t/{token}/result\" enctype=\"multipart/form-data\">\
<label for=\"text\">Answer (text or a fenced <code>```json</code> block)</label><textarea id=\"text\" name=\"text\"></textarea>\
<p><label for=\"files\">Files (images, or a .blend / .psd if asked; up to {} files of {} MB)</label><br>\
<input id=\"files\" type=\"file\" name=\"files\" multiple></p><button type=\"submit\">Send</button></form>",
            tasks::MAX_RESULT_FILES,
            tasks::MAX_RESULT_FILE >> 20
        ));
    }
    page(&t.title, &b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping() {
        assert_eq!(esc("<a href=\"x\">'&'</a>"), "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;");
    }
}
