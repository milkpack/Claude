//! Saved canvases, which are also the owner's sessions (projects): `data/workflows/<id>.json`.
//! A session is a workflow with `kind: "session"` (the default), `createdAt`, `lastOpenedAt` and
//! an optional `cover` asset; the list is ordered by `lastOpenedAt`.

use std::collections::HashMap;
use std::path::PathBuf;

use log::warn;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{ApiError, ApiResult};
use crate::store::{self, now_ms};

const MAX_FILE: u64 = 4 << 20;
const MAX_WORKFLOWS: usize = 10_000;
const MAX_NAME: usize = 200;

fn session_kind() -> String {
    "session".into()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    pub id: String,
    pub name: String,
    #[serde(default = "session_kind")]
    pub kind: String,
    pub graph: Value,
    #[serde(default)]
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub last_opened_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
}

impl Workflow {
    /// Fill the session fields of workflows saved before they existed.
    fn normalized(mut self) -> Workflow {
        if self.created_at == 0 {
            self.created_at = self.updated_at;
        }
        if self.last_opened_at == 0 {
            self.last_opened_at = self.updated_at;
        }
        self
    }

    fn summary(&self) -> Summary {
        Summary {
            name: self.name.clone(),
            kind: self.kind.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            last_opened_at: self.last_opened_at,
            cover: self.cover.clone(),
        }
    }
}

#[derive(Clone)]
struct Summary {
    name: String,
    kind: String,
    created_at: u64,
    updated_at: u64,
    last_opened_at: u64,
    cover: Option<String>,
}

#[derive(Deserialize)]
pub struct WorkflowInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub graph: Option<Value>,
    #[serde(default)]
    pub kind: Option<String>,
    /// An asset id; `null` removes the cover; absent keeps it.
    #[serde(default, deserialize_with = "present")]
    pub cover: Option<Value>,
}

/// `Some` for any value that is present, `null` included (so `null` can clear a field).
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

pub struct Workflows {
    dir: PathBuf,
    lock: tokio::sync::Mutex<()>,
    /// id → summary, so listing doesn't read every graph.
    index: std::sync::Mutex<HashMap<String, Summary>>,
}

fn check_name(name: &str) -> ApiResult<String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > MAX_NAME {
        return Err(ApiError::bad_request(format!("name must be 1–{MAX_NAME} characters")));
    }
    Ok(n.to_string())
}

fn check_kind(kind: &str) -> ApiResult<String> {
    let ok = !kind.is_empty() && kind.len() <= 32 && kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'-');
    if ok { Ok(kind.to_string()) } else { Err(ApiError::bad_request("kind must be 1–32 of a-z and -")) }
}

/// `Some(new cover)` when the input changes it.
fn check_cover(cover: &Option<Value>) -> ApiResult<Option<Option<String>>> {
    match cover {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(id)) if store::valid_id(id) => Ok(Some(Some(id.clone()))),
        Some(_) => Err(ApiError::bad_request("cover must be an asset id or null")),
    }
}

fn check_graph(graph: &Value) -> ApiResult<()> {
    let ok = graph.get("nodes").is_some_and(Value::is_array) && graph.get("edges").is_some_and(Value::is_array);
    if ok { Ok(()) } else { Err(ApiError::bad_request("graph must be an object with nodes and edges arrays")) }
}

impl Workflows {
    pub fn new(dir: PathBuf) -> Result<Workflows, String> {
        store::ensure_dir(&dir, false)?;
        let mut index = HashMap::new();
        for id in store::json_ids(&dir, MAX_WORKFLOWS) {
            let Ok(Some(bytes)) = store::read_capped(&dir.join(format!("{id}.json")), MAX_FILE) else { continue };
            match serde_json::from_slice::<Workflow>(&bytes) {
                Ok(w) if w.id == id => {
                    index.insert(id, w.normalized().summary());
                }
                _ => warn!("workflow {id}: unreadable; skipped"),
            }
        }
        Ok(Workflows { dir, lock: tokio::sync::Mutex::new(()), index: std::sync::Mutex::new(index) })
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    async fn read(&self, id: &str) -> ApiResult<Option<Workflow>> {
        if !store::valid_id(id) {
            return Ok(None);
        }
        let path = self.path(id);
        let bytes = tokio::task::spawn_blocking(move || store::read_capped(&path, MAX_FILE))
            .await
            .map_err(|e| ApiError::internal(format!("read task: {e}")))?
            .map_err(|e| {
                warn!("{e}");
                ApiError::internal("can't read the workflow")
            })?;
        match bytes {
            Some(b) => serde_json::from_slice::<Workflow>(&b).map(|w| Some(w.normalized())).map_err(|e| {
                warn!("workflow {id}: {e}");
                ApiError::internal("the stored workflow is corrupt")
            }),
            None => Ok(None),
        }
    }

    async fn write(&self, w: &Workflow) -> ApiResult<()> {
        let bytes = serde_json::to_vec(w).map_err(|e| ApiError::internal(e.to_string()))?;
        store::write_atomic_async(self.path(&w.id), bytes, false).await.map_err(|e| {
            warn!("{e}");
            ApiError::internal("can't save the workflow")
        })?;
        store::lock(&self.index).insert(w.id.clone(), w.summary());
        Ok(())
    }

    pub async fn get(&self, id: &str) -> ApiResult<Workflow> {
        self.read(id).await?.ok_or_else(|| ApiError::not_found("no such workflow"))
    }

    pub fn exists(&self, id: &str) -> bool {
        store::lock(&self.index).contains_key(id)
    }

    /// Open a session: `GET` records `lastOpenedAt`.
    pub async fn open(&self, id: &str) -> ApiResult<Workflow> {
        let _g = self.lock.lock().await;
        let mut w = self.get(id).await?;
        w.last_opened_at = now_ms().max(w.last_opened_at);
        self.write(&w).await?;
        Ok(w)
    }

    pub async fn create(&self, input: WorkflowInput) -> ApiResult<Workflow> {
        let name = check_name(input.name.as_deref().unwrap_or(""))?;
        let graph = input.graph.ok_or_else(|| ApiError::bad_request("graph is required"))?;
        check_graph(&graph)?;
        let kind = match &input.kind {
            Some(k) => check_kind(k)?,
            None => session_kind(),
        };
        let cover = check_cover(&input.cover)?.flatten();
        let now = now_ms();
        let w = Workflow { id: store::new_id(), name, kind, graph, created_at: now, updated_at: now, last_opened_at: now, cover };
        self.write(&w).await?;
        Ok(w)
    }

    /// A copy of workflow `id` (graph, kind, cover) under a new id; `name` defaults to "<name> copy".
    pub async fn duplicate(&self, id: &str, name: Option<String>) -> ApiResult<Workflow> {
        let src = self.get(id).await?;
        let name = match name {
            Some(n) => check_name(&n)?,
            None => check_name(&format!("{} copy", src.name.chars().take(MAX_NAME - 5).collect::<String>()))?,
        };
        let now = now_ms();
        let w = Workflow { id: store::new_id(), name, created_at: now, updated_at: now, last_opened_at: now, ..src };
        self.write(&w).await?;
        Ok(w)
    }

    pub async fn update(&self, id: &str, input: WorkflowInput) -> ApiResult<Workflow> {
        let _g = self.lock.lock().await;
        let mut w = self.get(id).await?;
        if let Some(n) = input.name {
            w.name = check_name(&n)?;
        }
        if let Some(g) = input.graph {
            check_graph(&g)?;
            w.graph = g;
        }
        if let Some(k) = &input.kind {
            w.kind = check_kind(k)?;
        }
        if let Some(c) = check_cover(&input.cover)? {
            w.cover = c;
        }
        w.updated_at = now_ms();
        self.write(&w).await?;
        Ok(w)
    }

    pub async fn delete(&self, id: &str) -> ApiResult<()> {
        if !store::valid_id(id) {
            return Err(ApiError::not_found("no such workflow"));
        }
        let _g = self.lock.lock().await;
        match tokio::fs::remove_file(self.path(id)).await {
            Ok(()) => {
                store::lock(&self.index).remove(id);
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ApiError::not_found("no such workflow")),
            Err(e) => {
                warn!("deleting workflow {id}: {e}");
                Err(ApiError::internal("can't delete the workflow"))
            }
        }
    }

    /// The summary of every workflow, most recently opened first.
    pub async fn list(&self) -> ApiResult<Vec<Value>> {
        let mut list: Vec<(String, Summary)> = store::lock(&self.index).iter().map(|(id, s)| (id.clone(), s.clone())).collect();
        list.sort_by(|a, b| {
            b.1.last_opened_at.cmp(&a.1.last_opened_at).then_with(|| b.1.updated_at.cmp(&a.1.updated_at)).then_with(|| a.0.cmp(&b.0))
        });
        Ok(list
            .into_iter()
            .map(|(id, s)| {
                let mut v = json!({ "id": id, "name": s.name, "kind": s.kind, "createdAt": s.created_at, "updatedAt": s.updated_at, "lastOpenedAt": s.last_opened_at });
                if let Some(c) = s.cover {
                    v["cover"] = json!(c);
                }
                v
            })
            .collect())
    }
}
