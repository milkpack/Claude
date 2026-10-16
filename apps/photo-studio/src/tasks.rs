//! Handoff tasks: work for Claude that the owner pastes into a Claude app (or a Claude Code
//! session does by itself), with the answer coming back through a tokenised return link.
//!
//! A task holds a prompt written from per-kind templates and the photo skill (no LLM), the
//! asset ids it refers to, and a 256-bit token. The token authorises, for 7 days and for this
//! task only: its page (`/t/<token>`), its inputs (`/t/<token>/inputs/<assetId>`) and returning a
//! result (`POST /t/<token>` or `/t/<token>/result`). Returned images become assets linked to the
//! task; other files (a .blend, a .psd) are kept as task attachments; a fenced ```json block in
//! the text is extracted and, for director and review tasks, validated against their contract.
//! Tasks live in `data/tasks/<id>.json`, attachments in `data/tasks/<id>/`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::Multipart;
use axum::http::StatusCode;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::assets::{AssetKind, NewAsset};
use crate::error::{ApiError, ApiResult};
use crate::server::App;
use crate::store::{self, lock, now_ms};

/// Links stop working this long after the task was created.
pub const TOKEN_TTL_MS: u64 = 7 * 24 * 3600 * 1000;
pub const MAX_RESULT_FILES: usize = 20;
pub const MAX_RESULT_FILE: usize = 30 << 20;
/// The whole body of a result upload.
pub const RESULT_BODY_LIMIT: usize = MAX_RESULT_FILES * MAX_RESULT_FILE + (2 << 20);
const MAX_TEXT: usize = 1 << 20;
const MAX_INPUTS: usize = 32;
const MAX_INSTRUCTIONS: usize = 50_000;
const MAX_CONTEXT_BYTES: usize = 64 << 10;
const MAX_TASKS: usize = 100_000;
const MAX_TASK_FILE: u64 = 4 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    Director,
    Review,
    Blender,
    Photoshop,
    Sketch,
    Custom,
}

impl TaskKind {
    fn label(self) -> &'static str {
        match self {
            TaskKind::Director => "Director",
            TaskKind::Review => "Review",
            TaskKind::Blender => "Blender",
            TaskKind::Photoshop => "Photoshop",
            TaskKind::Sketch => "Sketch review",
            TaskKind::Custom => "Task",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Open,
    Done,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskInput {
    pub asset_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// A returned file that isn't an image (kept next to the task).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default)]
    pub asset_ids: Vec<String>,
    #[serde(default)]
    pub files: Vec<Attachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json: Option<Value>,
    /// Why the returned JSON was not accepted (the text is kept either way).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub at: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredTask {
    pub id: String,
    pub session_id: String,
    pub node_id: String,
    pub kind: TaskKind,
    pub status: TaskStatus,
    pub title: String,
    pub prompt: String,
    pub inputs: Vec<TaskInput>,
    pub token: String,
    pub expires_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<StoredResult>,
    /// The last cloud run (routine fire) of this task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<crate::routine::RunState>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl StoredTask {
    pub fn expired(&self) -> bool {
        now_ms() >= self.expires_at
    }
}

pub struct Tasks {
    dir: PathBuf,
    map: Mutex<HashMap<String, StoredTask>>,
    /// SHA-256 (hex) of the token → task id.
    by_token: Mutex<HashMap<String, String>>,
    /// Serialises result writes.
    write: tokio::sync::Mutex<()>,
}

fn token_key(token: &str) -> String {
    store::hex(&store::sha256(token.as_bytes()))
}

impl Tasks {
    pub fn load(dir: PathBuf) -> Result<Tasks, String> {
        store::ensure_dir(&dir, true)?;
        let mut map = HashMap::new();
        let mut by_token = HashMap::new();
        for id in store::json_ids(&dir, MAX_TASKS) {
            let path = dir.join(format!("{id}.json"));
            match store::read_capped(&path, MAX_TASK_FILE) {
                Ok(Some(bytes)) => match serde_json::from_slice::<StoredTask>(&bytes) {
                    Ok(t) if t.id == id && t.token.len() == 43 => {
                        by_token.insert(token_key(&t.token), id.clone());
                        map.insert(id, t);
                    }
                    Ok(_) => warn!("{}: inconsistent task; skipped", path.display()),
                    Err(e) => warn!("{}: {e}; skipped", path.display()),
                },
                Ok(None) => {}
                Err(e) => warn!("{e}; skipped"),
            }
        }
        Ok(Tasks { dir, map: Mutex::new(map), by_token: Mutex::new(by_token), write: tokio::sync::Mutex::new(()) })
    }

    pub fn get(&self, id: &str) -> Option<StoredTask> {
        lock(&self.map).get(id).cloned()
    }

    /// The task whose token is `token` (compared in constant time), expired or not.
    pub fn by_token(&self, token: &str) -> Option<StoredTask> {
        if token.len() != 43 {
            return None;
        }
        let id = lock(&self.by_token).get(&token_key(token)).cloned()?;
        self.get(&id).filter(|t| store::ct_eq_str(&t.token, token))
    }

    /// Newest first.
    pub fn list(&self, session: Option<&str>, status: Option<TaskStatus>) -> Vec<StoredTask> {
        let mut v: Vec<StoredTask> = lock(&self.map)
            .values()
            .filter(|t| session.is_none_or(|s| t.session_id == s) && status.is_none_or(|st| t.status == st))
            .cloned()
            .collect();
        v.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| b.id.cmp(&a.id)));
        v.truncate(1000);
        v
    }

    fn files_dir(&self, id: &str) -> PathBuf {
        self.dir.join(id)
    }

    pub fn attachment_path(&self, id: &str, name: &str) -> PathBuf {
        self.files_dir(id).join(name)
    }

    async fn save(&self, mut t: StoredTask) -> ApiResult<StoredTask> {
        t.updated_at = now_ms();
        let bytes = serde_json::to_vec_pretty(&t).map_err(|e| ApiError::internal(e.to_string()))?;
        store::write_atomic_async(self.dir.join(format!("{}.json", t.id)), bytes, true).await.map_err(|e| {
            warn!("saving task {}: {e}", t.id);
            ApiError::internal("can't save the task")
        })?;
        lock(&self.by_token).insert(token_key(&t.token), t.id.clone());
        lock(&self.map).insert(t.id.clone(), t.clone());
        Ok(t)
    }
}

// ---- API shapes ----

pub fn input_url(app: &App, t: &StoredTask, asset_id: &str) -> String {
    format!("{}/t/{}/inputs/{asset_id}", app.public_url, t.token)
}

pub fn return_url(app: &App, t: &StoredTask) -> String {
    format!("{}/t/{}", app.public_url, t.token)
}

/// The API's `Task`.
pub fn task_json(app: &App, t: &StoredTask) -> Value {
    let links: Vec<Value> = t.inputs.iter().map(|i| json!({ "assetId": i.asset_id, "url": input_url(app, t, &i.asset_id) })).collect();
    let mut v = json!({
        "id": t.id,
        "sessionId": t.session_id,
        "nodeId": t.node_id,
        "kind": t.kind,
        "status": t.status,
        "title": t.title,
        "prompt": t.prompt,
        "inputs": t.inputs.iter().map(|i| i.asset_id.clone()).collect::<Vec<_>>(),
        "roles": t.inputs.iter().map(|i| i.role.clone()).collect::<Vec<_>>(),
        "links": links,
        "returnUrl": return_url(app, t),
        "expiresAt": t.expires_at,
        "createdAt": t.created_at,
        "updatedAt": t.updated_at,
    });
    if let Some(run) = &t.run {
        v["run"] = serde_json::to_value(run).unwrap_or(Value::Null);
    }
    if let Some(r) = &t.result {
        let assets: Vec<Value> = r.asset_ids.iter().filter_map(|id| app.assets.get(id)).filter_map(|a| serde_json::to_value(a.api()).ok()).collect();
        let files: Vec<Value> =
            r.files.iter().map(|f| json!({ "name": f.name, "size": f.size, "url": format!("/api/tasks/{}/files/{}", t.id, f.name) })).collect();
        let mut res = json!({ "assets": assets, "files": files, "at": r.at });
        if let Some(text) = &r.text {
            res["text"] = json!(text);
        }
        if let Some(j) = &r.json {
            res["json"] = j.clone();
        }
        if let Some(e) = &r.error {
            res["error"] = json!(e);
        }
        v["result"] = res;
    }
    v
}

// ---- creating ----

#[derive(Deserialize)]
#[serde(untagged)]
pub enum InputSpec {
    Id(String),
    Full {
        #[serde(rename = "assetId")]
        asset_id: String,
        #[serde(default)]
        role: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTask {
    pub session_id: String,
    pub node_id: String,
    pub kind: TaskKind,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub inputs: Vec<InputSpec>,
    #[serde(default)]
    pub context: Option<Value>,
    /// Send the task to the cloud routine right away (when one is set up).
    #[serde(default)]
    pub auto: bool,
}

fn clean_line(s: &str, max: usize) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).take(max).collect::<String>().trim().to_string()
}

/// `POST /api/tasks`.
pub async fn create(app: &Arc<App>, req: CreateTask) -> ApiResult<StoredTask> {
    if !store::valid_id(&req.session_id) || !app.workflows.exists(&req.session_id) {
        return Err(ApiError::bad_request("unknown session"));
    }
    let node_id = clean_line(&req.node_id, 128);
    if node_id.is_empty() {
        return Err(ApiError::bad_request("nodeId is required"));
    }
    let instructions = req.instructions.unwrap_or_default();
    if instructions.chars().count() > MAX_INSTRUCTIONS {
        return Err(ApiError::bad_request(format!("instructions are longer than {MAX_INSTRUCTIONS} characters")));
    }
    let context = match req.context {
        None | Some(Value::Null) => None,
        Some(c @ Value::Object(_)) => {
            if serde_json::to_vec(&c).map(|b| b.len()).unwrap_or(usize::MAX) > MAX_CONTEXT_BYTES {
                return Err(ApiError::too_large("context is too large"));
            }
            Some(c)
        }
        Some(_) => return Err(ApiError::bad_request("context must be an object")),
    };
    if req.inputs.len() > MAX_INPUTS {
        return Err(ApiError::bad_request(format!("at most {MAX_INPUTS} inputs")));
    }
    let mut inputs = Vec::new();
    let mut seen = HashSet::new();
    for spec in req.inputs {
        let (asset_id, role) = match spec {
            InputSpec::Id(id) => (id, None),
            InputSpec::Full { asset_id, role } => (asset_id, role),
        };
        if !store::valid_id(&asset_id) || app.assets.get(&asset_id).is_none() {
            return Err(ApiError::bad_request(format!("unknown input asset `{asset_id}`")));
        }
        if !seen.insert(asset_id.clone()) {
            continue;
        }
        let role = role.map(|r| clean_line(&r, 64)).filter(|r| !r.is_empty());
        inputs.push(TaskInput { asset_id, role });
    }
    let title = req.title.map(|t| clean_line(&t, 200)).filter(|t| !t.is_empty()).unwrap_or_else(|| {
        let first = instructions.lines().find(|l| !l.trim().is_empty()).map(|l| clean_line(l, 80)).unwrap_or_default();
        if first.is_empty() { req.kind.label().to_string() } else { format!("{}: {first}", req.kind.label()) }
    });
    let now = now_ms();
    let mut task = StoredTask {
        id: store::new_id(),
        session_id: req.session_id,
        node_id,
        kind: req.kind,
        status: TaskStatus::Open,
        title,
        prompt: String::new(),
        inputs,
        token: store::new_token(),
        expires_at: now.saturating_add(TOKEN_TTL_MS),
        result: None,
        run: None,
        created_at: now,
        updated_at: now,
    };
    let models = if task.kind == TaskKind::Director {
        match tokio::time::timeout(Duration::from_secs(5), app.openart.model_list(false)).await {
            Ok(Ok(raw)) => Some(crate::director::model_summary(&raw)),
            _ => None,
        }
    } else {
        None
    };
    task.prompt = crate::prompts::build(app, &task, &instructions, context.as_ref(), models.as_deref());
    let task = app.tasks.save(task).await?;
    info!("task {} created ({:?})", task.id, task.kind);
    if req.auto {
        return fire(app, &task.id).await;
    }
    Ok(task)
}

/// Kinds a cloud session can do by itself (the others need Blender or Photoshop on a computer).
pub fn cloud_kind(kind: TaskKind) -> bool {
    matches!(kind, TaskKind::Director | TaskKind::Review | TaskKind::Sketch | TaskKind::Custom)
}

/// Send task `id` to the cloud routine once and record the outcome on the task. The fire and
/// the save run in their own task, so a dropped request can't lose the record of a started run.
pub async fn fire(app: &Arc<App>, id: &str) -> ApiResult<StoredTask> {
    let t = app.tasks.get(id).ok_or_else(|| ApiError::not_found("no such task"))?;
    check_open(&t)?;
    if t.expired() {
        return Err(ApiError::new(StatusCode::GONE, "bad_request", "this task's links have expired: create a new task"));
    }
    let run = if !cloud_kind(t.kind) {
        Some(crate::routine::RunState::failed("cloud runs do director, review, sketch and custom tasks; this one needs a local tool"))
    } else if !app.routine.configured() {
        Some(crate::routine::RunState::failed("cloud runs aren't set up (Settings → Cloud runs)"))
    } else {
        match crate::routine::fire_text(&t.prompt) {
            Ok(_) => None,
            Err(e) => Some(crate::routine::RunState::failed(e)),
        }
    };
    let app = app.clone();
    let id = t.id.clone();
    let handle = tokio::spawn(async move {
        let run = match run {
            Some(r) => r,
            None => {
                let text = crate::routine::fire_text(&t.prompt).unwrap_or_default();
                app.routine.fire(&text).await
            }
        };
        let _g = app.tasks.write.lock().await;
        let mut current = app.tasks.get(&id).ok_or_else(|| ApiError::not_found("no such task"))?;
        current.run = Some(run);
        app.tasks.save(current).await
    });
    handle.await.map_err(|e| ApiError::internal(format!("fire task: {e}")))?
}

pub async fn cancel(app: &App, id: &str) -> ApiResult<StoredTask> {
    let _g = app.tasks.write.lock().await;
    let mut t = app.tasks.get(id).ok_or_else(|| ApiError::not_found("no such task"))?;
    t.status = TaskStatus::Cancelled;
    app.tasks.save(t).await
}

// ---- results ----

/// A result being received: text, JSON, and the files already stored.
#[derive(Default)]
pub struct Incoming {
    pub text: Option<String>,
    /// An explicit `json` field (raw text, parsed later).
    pub json: Option<String>,
    pub asset_ids: Vec<String>,
    pub files: Vec<Attachment>,
}

/// Whether a result may be returned to `t` now.
pub fn check_open(t: &StoredTask) -> ApiResult<()> {
    if t.status == TaskStatus::Cancelled {
        return Err(ApiError::new(StatusCode::GONE, "bad_request", "this task was cancelled"));
    }
    Ok(())
}

fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')).take(100).collect();
    let cleaned = cleaned.trim_start_matches('.').to_string();
    if cleaned.is_empty() { "file".into() } else { cleaned }
}

/// Read a multipart result (`text`, `json`, and `files` / `files[]` / `file` parts). Images
/// become assets as they arrive; other files are kept as attachments. On error, whatever was
/// stored is removed again.
pub async fn read_multipart(app: &App, t: &StoredTask, mut mp: Multipart) -> ApiResult<Incoming> {
    let mut inc = Incoming::default();
    let result = read_parts(app, t, &mut mp, &mut inc).await;
    if let Err(e) = result {
        for id in &inc.asset_ids {
            app.assets.delete(id).await;
        }
        for f in &inc.files {
            // Best effort: an orphaned attachment is harmless and the request failed anyway.
            let _ = tokio::fs::remove_file(app.tasks.attachment_path(&t.id, &f.name)).await;
        }
        return Err(e);
    }
    Ok(inc)
}

fn too_large() -> ApiError {
    ApiError::too_large(format!("files are limited to {} MB", MAX_RESULT_FILE >> 20))
}

fn mp_err(e: axum::extract::multipart::MultipartError) -> ApiError {
    if e.status() == StatusCode::PAYLOAD_TOO_LARGE { ApiError::too_large("the upload is too large") } else { ApiError::bad_request(e.body_text()) }
}

async fn read_field(field: &mut axum::extract::multipart::Field<'_>, max: usize) -> ApiResult<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = field.chunk().await.map_err(mp_err)? {
        if bytes.len() + chunk.len() > max {
            // Read the rest (bounded by the body limit) so the client gets a clean 413.
            while let Ok(Some(_)) = field.chunk().await {}
            return Err(if max == MAX_TEXT { ApiError::too_large("text is limited to 1 MB") } else { too_large() });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn read_parts(app: &App, t: &StoredTask, mp: &mut Multipart, inc: &mut Incoming) -> ApiResult<()> {
    let mut count = 0usize;
    while let Some(mut field) = mp.next_field().await.map_err(mp_err)? {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "text" | "json" => {
                let bytes = read_field(&mut field, MAX_TEXT).await?;
                let text = String::from_utf8(bytes).map_err(|_| ApiError::bad_request(format!("{name} must be UTF-8 text")))?;
                if name == "text" {
                    inc.text = Some(text);
                } else {
                    inc.json = Some(text);
                }
            }
            "files" | "files[]" | "file" => {
                let file_name = field.file_name().map(safe_name).unwrap_or_else(|| "file".into());
                let bytes = read_field(&mut field, MAX_RESULT_FILE).await?;
                if bytes.is_empty() {
                    continue; // an empty file input in a browser form
                }
                count += 1;
                if count > MAX_RESULT_FILES {
                    return Err(ApiError::too_large(format!("at most {MAX_RESULT_FILES} files")));
                }
                if crate::imaging::sniff(&bytes).is_some() {
                    let meta = NewAsset { task_id: Some(t.id.clone()), file_name: Some(file_name), ..NewAsset::default() };
                    let a = app.assets.create(AssetKind::Upload, bytes, meta).await?;
                    inc.asset_ids.push(a.id);
                } else {
                    let stored = format!("{}-{file_name}", store::new_id().get(..8).unwrap_or("x"));
                    let dir = app.tasks.files_dir(&t.id);
                    store::ensure_dir(&dir, true).map_err(|e| {
                        warn!("{e}");
                        ApiError::internal("can't store the file")
                    })?;
                    let size = bytes.len() as u64;
                    store::write_atomic_async(dir.join(&stored), bytes, true).await.map_err(|e| {
                        warn!("{e}");
                        ApiError::internal("can't store the file")
                    })?;
                    inc.files.push(Attachment { name: stored, size });
                }
            }
            _ => {} // unknown parts are skipped
        }
    }
    Ok(())
}

/// The first fenced ```json block of `text`.
pub fn fenced_json(text: &str) -> Option<&str> {
    let mut rest = text;
    while let Some(start) = rest.find("```") {
        let after = rest.get(start + 3..)?;
        let (tag, body) = after.split_once('\n')?;
        let end = body.find("```")?;
        if tag.trim().eq_ignore_ascii_case("json") {
            return body.get(..end);
        }
        rest = body.get(end + 3..)?;
    }
    None
}

/// The JSON of a result and why it was refused, if it was.
fn result_json(t: &StoredTask, text: Option<&str>, explicit: Option<&str>) -> (Option<Value>, Option<String>) {
    let strict = matches!(t.kind, TaskKind::Director | TaskKind::Review);
    let parsed: Option<Result<Value, String>> = match explicit {
        Some(j) => Some(serde_json::from_str::<Value>(j).map_err(|e| format!("the json field is not valid JSON: {e}"))),
        None => text.and_then(|text| match fenced_json(text) {
            Some(block) => Some(serde_json::from_str::<Value>(block.trim()).map_err(|e| format!("the ```json block is not valid JSON: {e}"))),
            None if strict => crate::claude::extract_json(text).map(Ok),
            None => None,
        }),
    };
    let value = match parsed {
        None if strict => return (None, Some("no ```json block found in the answer".into())),
        None => return (None, None),
        Some(Err(e)) => return (None, Some(e)),
        Some(Ok(v)) => v,
    };
    match t.kind {
        TaskKind::Director => {
            let allowed: HashSet<&str> = t.inputs.iter().map(|i| i.asset_id.as_str()).collect();
            match serde_json::from_value::<crate::director::DirectorResponse>(value) {
                Ok(r) => match crate::director::validate(&r, &allowed) {
                    Ok(()) => (serde_json::to_value(&r).ok(), None),
                    Err(e) => (None, Some(format!("the JSON is not a valid DirectorResponse: {e}"))),
                },
                Err(e) => (None, Some(format!("the JSON doesn't match DirectorResponse: {e}"))),
            }
        }
        TaskKind::Review => match serde_json::from_value::<crate::review::ReviewResponse>(value) {
            Ok(r) => match crate::review::validate(&r) {
                Ok(()) => (serde_json::to_value(&r).ok(), None),
                Err(e) => (None, Some(format!("the JSON is not a valid ReviewResponse: {e}"))),
            },
            Err(e) => (None, Some(format!("the JSON doesn't match ReviewResponse: {e}"))),
        },
        _ => (Some(value), None),
    }
}

/// Record a returned result: the text and JSON replace earlier ones, files add up.
pub async fn apply(app: &App, id: &str, inc: Incoming) -> ApiResult<StoredTask> {
    if inc.text.as_deref().is_none_or(|t| t.trim().is_empty()) && inc.json.is_none() && inc.asset_ids.is_empty() && inc.files.is_empty() {
        return Err(ApiError::bad_request("nothing returned: send text, json or files"));
    }
    if inc.text.as_ref().is_some_and(|t| t.len() > MAX_TEXT) || inc.json.as_ref().is_some_and(|t| t.len() > MAX_TEXT) {
        return Err(ApiError::too_large("text is limited to 1 MB"));
    }
    let _g = app.tasks.write.lock().await;
    let mut t = app.tasks.get(id).ok_or_else(|| ApiError::not_found("no such task"))?;
    check_open(&t)?;
    let prev = t.result.take();
    let mut asset_ids = prev.as_ref().map(|r| r.asset_ids.clone()).unwrap_or_default();
    asset_ids.extend(inc.asset_ids);
    let mut files = prev.as_ref().map(|r| r.files.clone()).unwrap_or_default();
    files.extend(inc.files);
    let text = inc.text.filter(|s| !s.trim().is_empty());
    let (json, error) = if text.is_some() || inc.json.is_some() {
        result_json(&t, text.as_deref(), inc.json.as_deref())
    } else {
        (prev.as_ref().and_then(|r| r.json.clone()), prev.as_ref().and_then(|r| r.error.clone()))
    };
    let text = text.or_else(|| prev.and_then(|r| r.text));
    if let Some(e) = &error {
        warn!("task {id}: {e}");
    }
    t.result = Some(StoredResult { text, asset_ids, files, json, error, at: now_ms() });
    t.status = TaskStatus::Done;
    let t = app.tasks.save(t).await?;
    info!("task {id}: result received");
    Ok(t)
}

/// A JSON result body: `{text?, json?}` (json may be any value or a string).
#[derive(Deserialize)]
pub struct JsonResult {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub json: Option<Value>,
}

impl JsonResult {
    pub fn incoming(self) -> Incoming {
        let json = self.json.filter(|j| !j.is_null()).map(|j| match j {
            Value::String(s) => s,
            other => other.to_string(),
        });
        Incoming { text: self.text, json, ..Incoming::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fenced_blocks() {
        assert_eq!(fenced_json("x\n```json\n{\"a\":1}\n```\ny"), Some("{\"a\":1}\n"));
        assert_eq!(fenced_json("```sh\ncurl\n```\n```JSON\n[1]\n```"), Some("[1]\n"));
        assert_eq!(fenced_json("```\n{}\n```"), None);
        assert_eq!(fenced_json("```json\nunterminated"), None);
        assert_eq!(fenced_json("plain"), None);
    }

    #[test]
    fn names() {
        assert_eq!(safe_name("../../scene.blend"), "scene.blend");
        assert_eq!(safe_name(".hidden"), "hidden");
        assert_eq!(safe_name("ü"), "file");
    }
}
