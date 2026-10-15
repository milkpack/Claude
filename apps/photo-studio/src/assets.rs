//! The image library: `data/assets/<id>.<ext>` (full resolution), `<id>.thumb.jpg` (512 px) and
//! `<id>.json` (metadata, written last: its presence marks a complete asset). An in-memory index
//! is loaded at startup.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use log::warn;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{ApiError, ApiResult};
use crate::imaging::{self, Format};
use crate::openart::OpenArtRef;
use crate::store::{self, lock, now_ms};

/// Largest image file we keep (uploads are capped lower, at 30 MB).
pub const MAX_ASSET_BYTES: usize = 60 << 20;
const MAX_META_BYTES: u64 = 1 << 20;
const MAX_ASSETS: usize = 200_000;
pub const DEFAULT_PAGE: usize = 50;
pub const MAX_PAGE: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    Upload,
    Generation,
}

/// What is stored in `<id>.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredAsset {
    pub id: String,
    pub kind: AssetKind,
    pub ext: String,
    pub width: u32,
    pub height: u32,
    pub created_at: u64,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openart: Option<OpenArtRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
}

/// The API's `Asset`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub id: String,
    pub kind: AssetKind,
    pub file: String,
    pub thumb: String,
    pub width: u32,
    pub height: u32,
    pub created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openart: Option<OpenArtRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
}

impl StoredAsset {
    pub fn api(&self) -> Asset {
        Asset {
            id: self.id.clone(),
            kind: self.kind,
            file: format!("/api/assets/{}/file", self.id),
            thumb: format!("/api/assets/{}/thumb", self.id),
            width: self.width,
            height: self.height,
            created_at: self.created_at,
            openart: self.openart.clone(),
            prompt: self.prompt.clone(),
            model: self.model.clone(),
            mode: self.mode.clone(),
            params: self.params.clone(),
            job_id: self.job_id.clone(),
        }
    }

    pub fn format(&self) -> Format {
        Format::from_ext(&self.ext).unwrap_or(Format::Png)
    }

    fn sort_key(&self) -> (u64, &str) {
        (self.created_at, self.id.as_str())
    }
}

/// Metadata for a new asset.
#[derive(Default)]
pub struct NewAsset {
    pub openart: Option<OpenArtRef>,
    pub prompt: Option<String>,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub params: Option<Value>,
    pub job_id: Option<String>,
    pub file_name: Option<String>,
}

pub struct Assets {
    dir: PathBuf,
    index: Mutex<HashMap<String, StoredAsset>>,
}

impl Assets {
    /// Load the index from `dir` (created if missing).
    pub fn load(dir: PathBuf) -> Result<Assets, String> {
        store::ensure_dir(&dir, false)?;
        let mut index = HashMap::new();
        for id in store::json_ids(&dir, MAX_ASSETS) {
            let path = dir.join(format!("{id}.json"));
            let asset = match store::read_capped(&path, MAX_META_BYTES) {
                Ok(Some(bytes)) => match serde_json::from_slice::<StoredAsset>(&bytes) {
                    Ok(a) => a,
                    Err(e) => {
                        warn!("{}: {e}; skipped", path.display());
                        continue;
                    }
                },
                Ok(None) => continue,
                Err(e) => {
                    warn!("{e}; skipped");
                    continue;
                }
            };
            if asset.id != id || Format::from_ext(&asset.ext).is_none() || !dir.join(format!("{id}.{}", asset.ext)).is_file() {
                warn!("{}: inconsistent asset metadata; skipped", path.display());
                continue;
            }
            index.insert(id, asset);
        }
        Ok(Assets { dir, index: Mutex::new(index) })
    }

    pub fn len(&self) -> usize {
        lock(&self.index).len()
    }

    pub fn file_path(&self, a: &StoredAsset) -> PathBuf {
        self.dir.join(format!("{}.{}", a.id, a.ext))
    }

    pub fn thumb_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.thumb.jpg"))
    }

    fn meta_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    pub fn get(&self, id: &str) -> Option<StoredAsset> {
        lock(&self.index).get(id).cloned()
    }

    /// The generated asset of job `job_id` made from OpenArt item `openart_id`, if already stored.
    pub fn find_generated(&self, job_id: &str, openart_id: &str) -> Option<StoredAsset> {
        lock(&self.index).values().find(|a| a.job_id.as_deref() == Some(job_id) && a.openart.as_ref().is_some_and(|o| o.id == openart_id)).cloned()
    }

    /// Store `bytes` (sniffed, decoded and thumbnailed on the blocking pool).
    pub async fn create(&self, kind: AssetKind, bytes: Vec<u8>, meta: NewAsset) -> ApiResult<StoredAsset> {
        if bytes.len() > MAX_ASSET_BYTES {
            return Err(ApiError::too_large(format!("images are limited to {} MB", MAX_ASSET_BYTES >> 20)));
        }
        let (bytes, processed) = imaging::run(move || {
            let p = imaging::process(&bytes);
            (bytes, p)
        })
        .await
        .map_err(ApiError::internal)?;
        let processed = processed.map_err(ApiError::bad_request)?;
        let asset = StoredAsset {
            id: store::new_id(),
            kind,
            ext: processed.format.ext().to_string(),
            width: processed.width,
            height: processed.height,
            created_at: now_ms(),
            bytes: bytes.len() as u64,
            openart: meta.openart,
            prompt: meta.prompt,
            model: meta.model,
            mode: meta.mode,
            params: meta.params,
            job_id: meta.job_id,
            file_name: meta.file_name,
        };
        let json = serde_json::to_vec_pretty(&asset).map_err(|e| ApiError::internal(e.to_string()))?;
        let fail = |e: String| {
            warn!("storing asset {}: {e}", asset.id);
            ApiError::internal("can't store the image")
        };
        store::write_atomic_async(self.file_path(&asset), bytes, false).await.map_err(fail)?;
        store::write_atomic_async(self.thumb_path(&asset.id), processed.thumb, false).await.map_err(fail)?;
        store::write_atomic_async(self.meta_path(&asset.id), json, false).await.map_err(fail)?;
        lock(&self.index).insert(asset.id.clone(), asset.clone());
        Ok(asset)
    }

    /// Record the OpenArt reference of an asset.
    pub async fn set_openart(&self, id: &str, r: OpenArtRef) -> ApiResult<StoredAsset> {
        let updated = {
            let mut index = lock(&self.index);
            let a = index.get_mut(id).ok_or_else(|| ApiError::not_found("no such asset"))?;
            a.openart = Some(r);
            a.clone()
        };
        let json = serde_json::to_vec_pretty(&updated).map_err(|e| ApiError::internal(e.to_string()))?;
        store::write_atomic_async(self.meta_path(id), json, false).await.map_err(|e| {
            warn!("updating asset {id}: {e}");
            ApiError::internal("can't update the asset")
        })?;
        Ok(updated)
    }

    /// Newest first, `limit` at a time; `cursor` is the `next` of the previous page.
    pub fn list(&self, cursor: Option<&str>, limit: usize) -> ApiResult<(Vec<StoredAsset>, Option<String>)> {
        let after = match cursor.filter(|c| !c.is_empty()) {
            Some(c) => {
                let (t, id) = c.split_once('-').ok_or_else(|| ApiError::bad_request("invalid cursor"))?;
                let t = t.parse::<u64>().map_err(|_| ApiError::bad_request("invalid cursor"))?;
                if !store::valid_id(id) {
                    return Err(ApiError::bad_request("invalid cursor"));
                }
                Some((t, id.to_string()))
            }
            None => None,
        };
        let limit = limit.clamp(1, MAX_PAGE);
        let index = lock(&self.index);
        let mut all: Vec<&StoredAsset> = index
            .values()
            .filter(|a| match &after {
                Some((t, id)) => a.sort_key() < (*t, id.as_str()),
                None => true,
            })
            .collect();
        all.sort_by(|a, b| b.sort_key().cmp(&a.sort_key()));
        let more = all.len() > limit;
        let page: Vec<StoredAsset> = all.into_iter().take(limit).cloned().collect();
        let next = if more { page.last().map(|a| format!("{}-{}", a.created_at, a.id)) } else { None };
        Ok((page, next))
    }

    /// Remove an asset and its files. `false` if it didn't exist.
    pub async fn delete(&self, id: &str) -> bool {
        let Some(a) = lock(&self.index).remove(id) else { return false };
        // Metadata first: a crash afterwards leaves stray files, never a broken asset.
        for path in [self.meta_path(id), self.file_path(&a), self.thumb_path(id)] {
            if let Err(e) = tokio::fs::remove_file(&path).await
                && e.kind() != std::io::ErrorKind::NotFound
            {
                warn!("{}: {e}", path.display());
            }
        }
        true
    }

    /// The full-resolution bytes of `a`.
    pub async fn read_file(&self, a: &StoredAsset) -> ApiResult<Vec<u8>> {
        read_bounded(self.file_path(a)).await
    }

    pub async fn read_thumb(&self, id: &str) -> ApiResult<Vec<u8>> {
        read_bounded(self.thumb_path(id)).await
    }
}

async fn read_bounded(path: PathBuf) -> ApiResult<Vec<u8>> {
    tokio::task::spawn_blocking(move || store::read_capped(&path, MAX_ASSET_BYTES as u64 + 1))
        .await
        .map_err(|e| ApiError::internal(format!("read task: {e}")))?
        .map_err(|e| {
            warn!("{e}");
            ApiError::internal("can't read the image")
        })?
        .ok_or_else(|| ApiError::not_found("the image file is missing"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imaging::tests::png;

    #[tokio::test]
    async fn create_list_delete_reload() {
        let dir = std::env::temp_dir().join(format!("ps-assets-{}", store::new_id()));
        let assets = Assets::load(dir.clone()).unwrap();
        let mut ids = vec![];
        for i in 0..5u32 {
            let a = assets.create(AssetKind::Upload, png(10 + i, 10), NewAsset::default()).await.unwrap();
            ids.push(a.id);
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        assert!(assets.create(AssetKind::Upload, b"nope".to_vec(), NewAsset::default()).await.is_err());
        let (p1, next) = assets.list(None, 2).unwrap();
        assert_eq!(p1.iter().map(|a| a.id.clone()).collect::<Vec<_>>(), vec![ids[4].clone(), ids[3].clone()]);
        let (p2, next2) = assets.list(next.as_deref(), 2).unwrap();
        assert_eq!(p2.iter().map(|a| a.id.clone()).collect::<Vec<_>>(), vec![ids[2].clone(), ids[1].clone()]);
        let (p3, next3) = assets.list(next2.as_deref(), 2).unwrap();
        assert_eq!(p3.len(), 1);
        assert!(next3.is_none());
        assert!(assets.list(Some("garbage"), 2).is_err());

        assert!(assets.delete(&ids[0]).await);
        assert!(!assets.delete(&ids[0]).await);
        let r = OpenArtRef { id: "o".into(), url: "https://x".into() };
        assets.set_openart(&ids[1], r.clone()).await.unwrap();
        let reloaded = Assets::load(dir.clone()).unwrap();
        assert_eq!(reloaded.len(), 4);
        assert_eq!(reloaded.get(&ids[1]).unwrap().openart, Some(r));
        assert_eq!(reloaded.get(&ids[4]).unwrap().api().file, format!("/api/assets/{}/file", ids[4]));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
