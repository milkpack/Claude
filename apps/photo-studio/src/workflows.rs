//! Saved canvases: `data/workflows/<id>.json`.

use std::path::PathBuf;

use log::warn;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{ApiError, ApiResult};
use crate::store::{self, now_ms};

const MAX_FILE: u64 = 4 << 20;
const MAX_WORKFLOWS: usize = 10_000;
const MAX_NAME: usize = 200;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    pub id: String,
    pub name: String,
    pub graph: Value,
    pub updated_at: u64,
}

#[derive(Deserialize)]
pub struct WorkflowInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub graph: Option<Value>,
}

pub struct Workflows {
    dir: PathBuf,
    lock: tokio::sync::Mutex<()>,
}

fn check_name(name: &str) -> ApiResult<String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > MAX_NAME {
        return Err(ApiError::bad_request(format!("name must be 1–{MAX_NAME} characters")));
    }
    Ok(n.to_string())
}

fn check_graph(graph: &Value) -> ApiResult<()> {
    let ok = graph.get("nodes").is_some_and(Value::is_array) && graph.get("edges").is_some_and(Value::is_array);
    if ok { Ok(()) } else { Err(ApiError::bad_request("graph must be an object with nodes and edges arrays")) }
}

impl Workflows {
    pub fn new(dir: PathBuf) -> Result<Workflows, String> {
        store::ensure_dir(&dir, false)?;
        Ok(Workflows { dir, lock: tokio::sync::Mutex::new(()) })
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
            Some(b) => serde_json::from_slice(&b).map(Some).map_err(|e| {
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
        })
    }

    pub async fn get(&self, id: &str) -> ApiResult<Workflow> {
        self.read(id).await?.ok_or_else(|| ApiError::not_found("no such workflow"))
    }

    pub async fn create(&self, input: WorkflowInput) -> ApiResult<Workflow> {
        let name = check_name(input.name.as_deref().unwrap_or(""))?;
        let graph = input.graph.ok_or_else(|| ApiError::bad_request("graph is required"))?;
        check_graph(&graph)?;
        let w = Workflow { id: store::new_id(), name, graph, updated_at: now_ms() };
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
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ApiError::not_found("no such workflow")),
            Err(e) => {
                warn!("deleting workflow {id}: {e}");
                Err(ApiError::internal("can't delete the workflow"))
            }
        }
    }

    /// `{id, name, updatedAt}` of every workflow, most recently updated first.
    pub async fn list(&self) -> ApiResult<Vec<Value>> {
        let dir = self.dir.clone();
        let list = tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            for id in store::json_ids(&dir, MAX_WORKFLOWS) {
                let path = dir.join(format!("{id}.json"));
                let Ok(Some(bytes)) = store::read_capped(&path, MAX_FILE) else { continue };
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Summary {
                    id: String,
                    name: String,
                    updated_at: u64,
                }
                if let Ok(s) = serde_json::from_slice::<Summary>(&bytes)
                    && s.id == id
                {
                    out.push(s);
                }
            }
            out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| a.id.cmp(&b.id)));
            out.into_iter().map(|s| json!({ "id": s.id, "name": s.name, "updatedAt": s.updated_at })).collect::<Vec<_>>()
        })
        .await
        .map_err(|e| ApiError::internal(format!("list task: {e}")))?;
        Ok(list)
    }
}
