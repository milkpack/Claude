//! Generation jobs: submitted with `openart_generate_image`, then polled in the background with
//! `openart_creation_get` (honouring `pollAfterSeconds`) until they finish; the outputs are
//! downloaded into the library. Jobs live in `data/jobs/<id>.json` and unfinished ones resume
//! polling when the server starts.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::StatusCode;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::assets::{AssetKind, MAX_ASSET_BYTES, NewAsset, StoredAsset};
use crate::error::{ApiError, ApiResult};
use crate::net::{neterr, read_limited};
use crate::openart::{OpenArtRef, Output, outputs_from_list, outputs_from_status};
use crate::server::App;
use crate::store::{self, lock, now_ms};

const MAX_JOBS_LISTED: usize = 500;
const MAX_JOB_FILE: u64 = 4 << 20;
const MAX_REFERENCES: usize = 16;
const MAX_PARAMS_BYTES: usize = 256 << 10;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);
/// Pages of `openart_creation_list` searched for a job's outputs when its status has none.
const LIST_PAGES: usize = 4;

#[derive(Clone, Copy, Debug)]
pub struct JobTiming {
    /// Wait before the first status check.
    pub first_poll: Duration,
    /// `pollAfterSeconds` is clamped to this range.
    pub min_poll: Duration,
    pub max_poll: Duration,
    /// Give up on a job this long after it was created.
    pub give_up: Duration,
}

impl Default for JobTiming {
    fn default() -> Self {
        JobTiming {
            first_poll: Duration::from_secs(2),
            min_poll: Duration::from_secs(2),
            max_poll: Duration::from_secs(20),
            give_up: Duration::from_secs(15 * 60),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

impl JobStatus {
    pub fn finished(self) -> bool {
        matches!(self, JobStatus::Completed | JobStatus::Failed)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefInput {
    pub asset_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredJob {
    pub id: String,
    pub status: JobStatus,
    pub model: String,
    pub mode: String,
    pub params: Value,
    pub references: Vec<RefInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<f64>,
    #[serde(default)]
    pub asset_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    /// The give-up deadline counts from here (`created_at` unless the job was revived).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_from: Option<u64>,
    /// Failed by giving up on polling (so it may be revived after a reconnect).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gave_up: bool,
}

pub struct Jobs {
    dir: PathBuf,
    map: Mutex<HashMap<String, StoredJob>>,
    pollers: Mutex<HashSet<String>>,
    /// Jobs being submitted to OpenArt right now.
    submitting: Mutex<HashSet<String>>,
    pub timing: JobTiming,
}

impl Jobs {
    pub fn load(dir: PathBuf, timing: JobTiming) -> Result<Jobs, String> {
        store::ensure_dir(&dir, false)?;
        let mut map = HashMap::new();
        for id in store::json_ids(&dir, 100_000) {
            let path = dir.join(format!("{id}.json"));
            match store::read_capped(&path, MAX_JOB_FILE) {
                Ok(Some(bytes)) => match serde_json::from_slice::<StoredJob>(&bytes) {
                    Ok(j) if j.id == id => {
                        map.insert(id, j);
                    }
                    Ok(_) => warn!("{}: id mismatch; skipped", path.display()),
                    Err(e) => warn!("{}: {e}; skipped", path.display()),
                },
                Ok(None) => {}
                Err(e) => warn!("{e}; skipped"),
            }
        }
        Ok(Jobs { dir, map: Mutex::new(map), pollers: Mutex::new(HashSet::new()), submitting: Mutex::new(HashSet::new()), timing })
    }

    pub fn get(&self, id: &str) -> Option<StoredJob> {
        lock(&self.map).get(id).cloned()
    }

    /// Newest first.
    pub fn list(&self, active_only: bool) -> Vec<StoredJob> {
        let map = lock(&self.map);
        let mut jobs: Vec<StoredJob> = map.values().filter(|j| !active_only || !j.status.finished()).cloned().collect();
        jobs.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| b.id.cmp(&a.id)));
        jobs.truncate(MAX_JOBS_LISTED);
        jobs
    }

    pub fn unfinished(&self) -> Vec<String> {
        lock(&self.map).values().filter(|j| !j.status.finished()).map(|j| j.id.clone()).collect()
    }

    /// Store `job` (in memory and on disk).
    pub async fn save(&self, mut job: StoredJob) -> StoredJob {
        job.updated_at = now_ms();
        lock(&self.map).insert(job.id.clone(), job.clone());
        match serde_json::to_vec_pretty(&job) {
            Ok(bytes) => {
                if let Err(e) = store::write_atomic_async(self.dir.join(format!("{}.json", job.id)), bytes, false).await {
                    warn!("saving job {}: {e}", job.id);
                }
            }
            Err(e) => warn!("saving job {}: {e}", job.id),
        }
        job
    }
}

/// The API's `Job`: the stored job with its assets resolved.
pub fn job_json(app: &App, job: &StoredJob) -> Value {
    let mut v = serde_json::to_value(job).unwrap_or_else(|_| json!({ "id": job.id }));
    let assets: Vec<Value> = job.asset_ids.iter().filter_map(|id| app.assets.get(id)).filter_map(|a| serde_json::to_value(a.api()).ok()).collect();
    if let Some(o) = v.as_object_mut() {
        o.remove("assetIds");
        o.remove("pollFrom");
        o.remove("gaveUp");
        o.insert("assets".into(), Value::Array(assets));
    }
    v
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateJob {
    pub model: String,
    pub mode: String,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    pub references: Vec<RefInput>,
}

fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

/// Make sure `asset` is on OpenArt (uploading it now if needed) and return its reference.
pub async fn ensure_openart(app: &App, asset: &StoredAsset) -> ApiResult<OpenArtRef> {
    if let Some(r) = &asset.openart {
        return Ok(r.clone());
    }
    let bytes = app.assets.read_file(asset).await?;
    let format = asset.format();
    let name = asset.file_name.clone().unwrap_or_else(|| format!("{}.{}", asset.id, asset.ext));
    let r = app.openart.upload(bytes, format.mime(), &name, &name).await?;
    app.assets.set_openart(&asset.id, r.clone()).await?;
    Ok(r)
}

/// `POST /api/jobs`: quote, submit, and start polling.
///
/// Everything before the submission (validation, reference uploads, the quote) spends nothing,
/// so a cancelled request there is harmless. Then the job is saved, and submitted in a task of
/// its own: a request dropped mid-submission (a closed tab, a proxy timeout) can't lose a job
/// OpenArt has accepted and charged for.
pub async fn create(app: Arc<App>, req: CreateJob) -> ApiResult<StoredJob> {
    if !valid_name(&req.model) || !valid_name(&req.mode) {
        return Err(ApiError::bad_request("model and mode must be OpenArt ids"));
    }
    let mut params = req.params.unwrap_or_else(|| json!({}));
    let Some(obj) = params.as_object_mut() else { return Err(ApiError::bad_request("params must be an object")) };
    // References come from the library only, and the server writes them.
    obj.remove("visualReferences");
    if req.references.len() > MAX_REFERENCES {
        return Err(ApiError::bad_request(format!("at most {MAX_REFERENCES} references")));
    }
    let mut assets = Vec::with_capacity(req.references.len());
    for (i, r) in req.references.iter().enumerate() {
        let asset = store::valid_id(&r.asset_id)
            .then(|| app.assets.get(&r.asset_id))
            .flatten()
            .ok_or_else(|| ApiError::bad_request(format!("unknown reference asset `{}`", r.asset_id)))?;
        let label = r
            .label
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty() && l.len() <= 64)
            .map(str::to_string)
            .unwrap_or_else(|| format!("image{}", i + 1));
        assets.push((asset, label));
    }
    if serde_json::to_vec(&params).map(|b| b.len()).unwrap_or(usize::MAX) > MAX_PARAMS_BYTES {
        return Err(ApiError::too_large("params are too large"));
    }
    let mut references = Vec::with_capacity(assets.len());
    let mut visual = Vec::with_capacity(assets.len());
    for (asset, label) in assets {
        let oa = ensure_openart(&app, &asset).await?;
        visual.push(json!({ "type": "image", "id": oa.id, "url": oa.url, "label": label }));
        references.push(RefInput { asset_id: asset.id.clone(), label: Some(label) });
    }
    if !visual.is_empty()
        && let Some(obj) = params.as_object_mut()
    {
        obj.insert("visualReferences".into(), Value::Array(visual));
    }

    // Never submit what the account can't afford. A failed quote doesn't block the job (the
    // generation itself will report a real problem).
    let mut credits = None;
    match app.openart.cost(&req.model, &req.mode, &params).await {
        Ok(q) if !q.affordable => {
            let upgrade = q.upgrade_url.map(|u| format!(" Add credits: {u}")).unwrap_or_default();
            let balance = q.balance.map(|b| format!(" and the balance is {b}")).unwrap_or_default();
            return Err(ApiError::new(
                StatusCode::PAYMENT_REQUIRED,
                "bad_request",
                format!("not enough OpenArt credits: this job costs {}{balance}.{upgrade}", q.credits),
            ));
        }
        Ok(q) => credits = Some(q.credits),
        Err(e) if e.code == "openart_not_connected" || e.code == "openart_reconnect" => return Err(e),
        Err(e) => warn!("quoting {}/{}: {}", req.model, req.mode, e.message),
    }

    let now = now_ms();
    let job = StoredJob {
        id: store::new_id(),
        status: JobStatus::Queued,
        model: req.model,
        mode: req.mode,
        params,
        references,
        history_id: None,
        credits,
        asset_ids: vec![],
        error: None,
        created_at: now,
        updated_at: now,
        poll_from: None,
        gave_up: false,
    };
    lock(&app.jobs.submitting).insert(job.id.clone());
    let job = app.jobs.save(job).await;
    let id = job.id.clone();
    let task = tokio::spawn(submit(app.clone(), job));
    let result = task.await.map_err(|e| ApiError::internal(format!("submit task: {e}")));
    lock(&app.jobs.submitting).remove(&id);
    result?
}

/// Submit a saved job to OpenArt and start polling it (runs detached from the request).
async fn submit(app: Arc<App>, mut job: StoredJob) -> ApiResult<StoredJob> {
    let result = match app.openart.generate(&job.model, &job.mode, &job.params).await {
        Ok(r) => r,
        Err(e) => {
            lock(&app.jobs.submitting).remove(&job.id);
            fail(&app, job, format!("OpenArt refused the job: {}", e.message)).await;
            return Err(e);
        }
    };
    let Some(history_id) = find_str(&result, &["historyId", "history_id"]).filter(|h| !h.is_empty() && h.len() <= 256) else {
        lock(&app.jobs.submitting).remove(&job.id);
        let e = ApiError::upstream("OpenArt accepted the job but returned no historyId");
        fail(&app, job, e.message.clone()).await;
        return Err(e);
    };
    if job.credits.is_none() {
        job.credits = ["totalCredits", "credits"].iter().find_map(|k| result.get(*k).and_then(Value::as_f64));
    }
    let status = map_status(result.get("status").and_then(Value::as_str).unwrap_or("PENDING")).unwrap_or(JobStatus::Queued);
    job.status = if status.finished() { JobStatus::Queued } else { status };
    job.history_id = Some(history_id);
    let job = app.jobs.save(job).await;
    lock(&app.jobs.submitting).remove(&job.id);
    info!("job {} submitted ({} / {})", job.id, job.model, job.mode);
    spawn_poller(app, job.id.clone());
    Ok(job)
}

/// A string under one of `keys`, at the top level or one level down.
fn find_str(v: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(Value::as_str) {
            return Some(s.to_string());
        }
    }
    v.as_object()?.values().filter(|x| x.is_object()).find_map(|x| keys.iter().find_map(|k| x.get(*k).and_then(Value::as_str)).map(str::to_string))
}

/// OpenArt's status → ours; `None` for a terminal failure.
fn map_status(s: &str) -> Option<JobStatus> {
    match s.to_ascii_uppercase().as_str() {
        "PENDING" | "QUEUED" | "SUBMITTED" => Some(JobStatus::Queued),
        "COMPLETED" | "SUCCEEDED" | "SUCCESS" | "DONE" => Some(JobStatus::Completed),
        "FAILED" | "CANCELLED" | "CANCELED" | "ERROR" | "REJECTED" => None,
        _ => Some(JobStatus::Running),
    }
}

/// Start the background poller of job `id` (once).
pub fn spawn_poller(app: Arc<App>, id: String) {
    if *app.stop.borrow() || !lock(&app.jobs.pollers).insert(id.clone()) {
        return;
    }
    tokio::spawn(async move {
        poll(&app, &id).await;
        lock(&app.jobs.pollers).remove(&id);
    });
}

/// Resume polling every unfinished job (at startup, and after reconnecting OpenArt).
pub async fn resume(app: &Arc<App>) {
    let ids = app.jobs.unfinished();
    for id in ids {
        let Some(job) = app.jobs.get(&id) else { continue };
        if job.history_id.is_none() {
            if !lock(&app.jobs.submitting).contains(&id) {
                // The server stopped between saving the job and hearing back from OpenArt.
                fail(app, job, "the server stopped while submitting this job: check the OpenArt history before running it again".into()).await;
            }
            continue;
        }
        info!("resuming job {id}");
        spawn_poller(app.clone(), id);
    }
}

/// After a reconnect: jobs that gave up while OpenArt was unreachable get another 15 minutes.
pub async fn revive(app: &Arc<App>) {
    let gave_up: Vec<StoredJob> = lock(&app.jobs.map)
        .values()
        .filter(|j| j.status == JobStatus::Failed && j.gave_up && j.history_id.is_some() && j.asset_ids.is_empty())
        .cloned()
        .collect();
    for mut job in gave_up {
        info!("job {}: polling again after the reconnect", job.id);
        job.status = JobStatus::Running;
        job.error = None;
        job.gave_up = false;
        job.poll_from = Some(now_ms());
        app.jobs.save(job).await;
    }
    resume(app).await;
}

async fn fail(app: &App, mut job: StoredJob, error: String) {
    warn!("job {} failed: {error}", job.id);
    job.status = JobStatus::Failed;
    job.gave_up = false;
    job.error = Some(error);
    app.jobs.save(job).await;
}

async fn poll(app: &App, id: &str) {
    let timing = app.jobs.timing;
    let give_up_ms = u64::try_from(timing.give_up.as_millis()).unwrap_or(u64::MAX);
    let mut delay = timing.first_poll;
    let mut checked = false;
    let mut errors = 0u32;
    // The last status check failed because OpenArt isn't connected: wait for a reconnect
    // instead of counting down (the images are paid for).
    let mut parked = false;
    let mut stop = app.stop.subscribe();
    loop {
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            // Server stopping (or gone): the job resumes from disk on the next start.
            _ = stop.wait_for(|s| *s) => return,
        }
        let Some(job) = app.jobs.get(id) else { return };
        if job.status.finished() {
            return;
        }
        let Some(history_id) = job.history_id.clone() else {
            if !lock(&app.jobs.submitting).contains(id) {
                fail(app, job, "the job has no OpenArt historyId".into()).await;
                return;
            }
            delay = timing.min_poll;
            continue;
        };
        let overdue = now_ms().saturating_sub(job.poll_from.unwrap_or(job.created_at)) > give_up_ms;
        // After a restart, check at least once before giving up.
        if overdue && !parked && (checked || errors >= 3) {
            let mut job = job;
            job.status = JobStatus::Failed;
            job.error = Some(format!("gave up after {} minutes", timing.give_up.as_secs() / 60));
            job.gave_up = true;
            warn!("job {id}: gave up");
            app.jobs.save(job).await;
            return;
        }
        let status = match app.openart.creation_get(&history_id).await {
            Ok(s) => s,
            Err(e) if e.code == "openart_not_connected" || e.code == "openart_reconnect" => {
                if !parked {
                    warn!("job {id}: waiting for OpenArt to be reconnected");
                }
                parked = true;
                delay = timing.max_poll;
                continue;
            }
            Err(e) => {
                parked = false;
                errors += 1;
                warn!("job {id}: status check failed ({}): {}", errors, e.message);
                delay = timing.max_poll;
                continue;
            }
        };
        parked = false;
        checked = true;
        errors = 0;
        let raw = status.get("status").and_then(Value::as_str).unwrap_or("RUNNING");
        match map_status(raw) {
            Some(JobStatus::Completed) => {
                finish(app, job, &history_id, &status).await;
                return;
            }
            None => {
                let why = ["error", "message", "failureReason", "reason"]
                    .iter()
                    .find_map(|k| status.get(*k).and_then(Value::as_str))
                    .map(|m| crate::net::snippet(m.as_bytes()))
                    .unwrap_or_default();
                let msg = if why.is_empty() { format!("OpenArt reported {raw}") } else { format!("OpenArt reported {raw}: {why}") };
                fail(app, job, msg).await;
                return;
            }
            Some(s) => {
                if job.status != s {
                    let mut j = job;
                    j.status = s;
                    app.jobs.save(j).await;
                }
            }
        }
        delay = status
            .get("pollAfterSeconds")
            .and_then(Value::as_f64)
            .filter(|s| s.is_finite() && *s >= 0.0)
            .map(|s| Duration::from_millis((s.min(3600.0) * 1000.0) as u64))
            .unwrap_or(timing.min_poll)
            .clamp(timing.min_poll, timing.max_poll);
    }
}

/// A COMPLETED job: find its outputs, download them, and store them as assets.
async fn finish(app: &App, mut job: StoredJob, history_id: &str, status: &Value) {
    let mut outputs = outputs_from_status(status);
    if outputs.is_empty() {
        let mut cursor: Option<String> = None;
        for _ in 0..LIST_PAGES {
            let page = match app.openart.creation_list(cursor.as_deref()).await {
                Ok(p) => p,
                Err(e) => {
                    warn!("job {}: listing creations: {}", job.id, e.message);
                    break;
                }
            };
            outputs = outputs_from_list(&page, history_id);
            if !outputs.is_empty() {
                break;
            }
            cursor = page.get("nextCursor").and_then(Value::as_str).map(str::to_string);
            if cursor.is_none() || page.get("hasMore").and_then(Value::as_bool) == Some(false) {
                break;
            }
        }
    }
    if outputs.is_empty() {
        fail(app, job, "OpenArt reported the job complete but returned no images".into()).await;
        return;
    }
    let mut ids = Vec::new();
    let mut errors = Vec::new();
    for out in outputs.iter().take(32) {
        match store_output(app, &job, out).await {
            Ok(a) => ids.push(a.id),
            Err(e) => {
                warn!("job {}: output {}: {e}", job.id, out.id);
                errors.push(e);
            }
        }
    }
    let total = outputs.len().min(32);
    if ids.is_empty() {
        let first = errors.first().cloned().unwrap_or_default();
        fail(app, job, format!("couldn't download the images: {first}")).await;
        return;
    }
    job.status = JobStatus::Completed;
    job.error = (!errors.is_empty()).then(|| format!("{} of {total} images could not be downloaded", errors.len()));
    job.asset_ids = ids;
    let job = app.jobs.save(job).await;
    info!("job {} completed with {} image(s)", job.id, job.asset_ids.len());
}

async fn store_output(app: &App, job: &StoredJob, out: &Output) -> Result<StoredAsset, String> {
    if let Some(a) = app.assets.find_generated(&job.id, &out.id) {
        return Ok(a); // downloaded before a restart
    }
    let mut last = String::new();
    for attempt in 0..3u64 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(500 * attempt)).await;
        }
        match download(app, &out.url).await {
            Ok(bytes) => {
                let meta = NewAsset {
                    openart: Some(OpenArtRef { id: out.id.clone(), url: out.url.clone() }),
                    prompt: job.params.get("prompt").and_then(Value::as_str).map(str::to_string),
                    model: Some(job.model.clone()),
                    mode: Some(job.mode.clone()),
                    params: Some(job.params.clone()),
                    job_id: Some(job.id.clone()),
                    file_name: None,
                };
                return app.assets.create(AssetKind::Generation, bytes, meta).await.map_err(|e| e.message);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

async fn download(app: &App, url: &str) -> Result<Vec<u8>, String> {
    crate::net::check_fetch_url(url, app.allow_private_urls).await?;
    let resp = app.fetch.get(url).timeout(DOWNLOAD_TIMEOUT).send().await.map_err(neterr)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    read_limited(resp, MAX_ASSET_BYTES).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_and_ids() {
        assert_eq!(map_status("PENDING"), Some(JobStatus::Queued));
        assert_eq!(map_status("RUNNING"), Some(JobStatus::Running));
        assert_eq!(map_status("whatever"), Some(JobStatus::Running));
        assert_eq!(map_status("COMPLETED"), Some(JobStatus::Completed));
        assert_eq!(map_status("CANCELLED"), None);
        assert_eq!(find_str(&json!({"historyId":"h"}), &["historyId"]), Some("h".into()));
        assert_eq!(find_str(&json!({"result":{"historyId":"n"}}), &["historyId"]), Some("n".into()));
        assert_eq!(find_str(&json!({"x":1}), &["historyId"]), None);
        assert!(valid_name("nano-banana-2-1") && valid_name("text2image") && !valid_name("a b") && !valid_name(""));
    }
}
