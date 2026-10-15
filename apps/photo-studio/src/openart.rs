//! OpenArt as a model provider: the tools we call (models, forms, prices, generation, status,
//! uploads, account) with their caches, on top of the OAuth connection and the MCP client.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use log::{debug, warn};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{ApiError, ApiResult};
use crate::mcp::Mcp;
use crate::net::{neterr, read_limited, snippet};
use crate::oauth::OAuth;
use crate::store::{lock, now_ms};

const MODELS_TTL_MS: u64 = 10 * 60_000;
const FORM_TTL_MS: u64 = 3_600_000;
const ACCOUNT_TTL_MS: u64 = 60_000;
const MAX_FORMS_CACHED: usize = 256;
const PUT_TIMEOUT: Duration = Duration::from_secs(300);

/// An OpenArt media reference: what `visualReferences` entries are made of.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenArtRef {
    pub id: String,
    pub url: String,
}

/// A price quote from `openart_model_cost`.
#[derive(Clone, Debug, PartialEq)]
pub struct Quote {
    pub credits: f64,
    pub config: Value,
    pub affordable: bool,
    pub balance: Option<f64>,
    pub upgrade_url: Option<String>,
}

pub struct OpenArt {
    pub oauth: OAuth,
    mcp: Mcp,
    /// For the presigned upload: no credentials, checked redirects.
    http: reqwest::Client,
    allow_private_urls: bool,
    models: Mutex<Option<(u64, Value)>>,
    forms: Mutex<HashMap<(String, String), (u64, Value)>>,
    account: Mutex<Option<(u64, Value)>>,
}

impl OpenArt {
    pub fn new(oauth: OAuth, mcp_url: String, http: reqwest::Client, fetch: reqwest::Client, allow_private_urls: bool) -> OpenArt {
        OpenArt {
            oauth,
            mcp: Mcp::new(mcp_url, http),
            http: fetch,
            allow_private_urls,
            models: Mutex::new(None),
            forms: Mutex::new(HashMap::new()),
            account: Mutex::new(None),
        }
    }

    pub async fn call(&self, tool: &str, args: Value) -> ApiResult<Value> {
        self.mcp.call_tool(&self.oauth, tool, args).await
    }

    /// Forget the MCP session and every cache (after connecting or disconnecting).
    pub async fn reset(&self) {
        self.mcp.reset().await;
        *lock(&self.models) = None;
        lock(&self.forms).clear();
        *lock(&self.account) = None;
    }

    pub async fn disconnect(&self) -> ApiResult<()> {
        self.oauth.disconnect().await?;
        self.reset().await;
        Ok(())
    }

    /// `openart_model_list` (cached 10 minutes).
    pub async fn model_list(&self, refresh: bool) -> ApiResult<Value> {
        if !refresh
            && let Some((at, v)) = lock(&self.models).as_ref()
            && now_ms().saturating_sub(*at) < MODELS_TTL_MS
        {
            return Ok(v.clone());
        }
        let v = self.call("openart_model_list", json!({})).await?;
        if !v.get("models").is_some_and(Value::is_array) {
            return Err(ApiError::upstream("OpenArt openart_model_list returned no models"));
        }
        *lock(&self.models) = Some((now_ms(), v.clone()));
        Ok(v)
    }

    /// The JSON Schema of `model`/`mode` (cached 1 hour).
    pub async fn form(&self, model: &str, mode: &str) -> ApiResult<Value> {
        let key = (model.to_string(), mode.to_string());
        if let Some((at, v)) = lock(&self.forms).get(&key)
            && now_ms().saturating_sub(*at) < FORM_TTL_MS
        {
            return Ok(v.clone());
        }
        let v = self.call("openart_model_form_get", json!({ "model": model, "mode": mode })).await?;
        let schema = v
            .get("jsonSchema")
            .or_else(|| v.get("schema"))
            .filter(|s| s.is_object())
            .cloned()
            .ok_or_else(|| ApiError::upstream("OpenArt openart_model_form_get returned no schema"))?;
        let mut forms = lock(&self.forms);
        if forms.len() >= MAX_FORMS_CACHED {
            forms.clear();
        }
        forms.insert(key, (now_ms(), schema.clone()));
        Ok(schema)
    }

    /// The exact price of one job.
    pub async fn cost(&self, model: &str, mode: &str, params: &Value) -> ApiResult<Quote> {
        let v = self.call("openart_model_cost", json!({ "model": model, "mode": mode, "params": params })).await?;
        let item = v
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|i| i.get("model").and_then(Value::as_str) == Some(model) && i.get("mode").and_then(Value::as_str) == Some(mode))
                    .or(items.first())
            })
            .ok_or_else(|| ApiError::upstream("OpenArt openart_model_cost returned no price"))?;
        let credits = item
            .get("totalCredits")
            .or_else(|| item.get("credits"))
            .and_then(Value::as_f64)
            .ok_or_else(|| ApiError::upstream("OpenArt openart_model_cost returned no totalCredits"))?;
        Ok(Quote {
            credits,
            config: item.get("config").cloned().unwrap_or_else(|| json!({})),
            affordable: item.get("affordable").and_then(Value::as_bool).unwrap_or(true),
            balance: v.get("balance").and_then(Value::as_f64),
            upgrade_url: v.get("upgradeUrl").and_then(Value::as_str).map(str::to_string),
        })
    }

    /// Submit a generation; returns the tool's result (with `historyId`).
    pub async fn generate(&self, model: &str, mode: &str, params: &Value) -> ApiResult<Value> {
        self.call("openart_generate_image", json!({ "model": model, "mode": mode, "params": params })).await
    }

    pub async fn creation_get(&self, history_id: &str) -> ApiResult<Value> {
        self.call("openart_creation_get", json!({ "historyId": history_id })).await
    }

    pub async fn creation_list(&self, cursor: Option<&str>) -> ApiResult<Value> {
        let mut args = json!({ "limit": 50, "mediaType": "image" });
        if let Some(c) = cursor {
            args["cursor"] = json!(c);
        }
        self.call("openart_creation_list", args).await
    }

    /// `openart_account_get` (cached 1 minute); `None` when not connected or on any error.
    pub async fn account(&self) -> Option<Value> {
        if let Some((at, v)) = lock(&self.account).as_ref()
            && now_ms().saturating_sub(*at) < ACCOUNT_TTL_MS
        {
            return Some(v.clone());
        }
        match self.call("openart_account_get", json!({})).await {
            Ok(v) => {
                *lock(&self.account) = Some((now_ms(), v.clone()));
                Some(v)
            }
            Err(e) => {
                debug!("OpenArt account: {e}");
                None
            }
        }
    }

    /// Upload an image so it can be used as a reference: `openart_upload_sign` → PUT →
    /// `openart_upload_metadata_get`.
    pub async fn upload(&self, bytes: Vec<u8>, content_type: &str, filename: &str, label: &str) -> ApiResult<OpenArtRef> {
        let size = bytes.len();
        let signed = self
            .call(
                "openart_upload_sign",
                json!({ "mediaType": "image", "size": size, "contentType": content_type, "filename": filename, "label": label }),
            )
            .await?;
        let pick = |keys: &[&str]| keys.iter().find_map(|k| signed.get(*k).and_then(Value::as_str)).map(str::to_string);
        let sign_url = pick(&["signURL", "signUrl", "uploadUrl", "uploadURL", "url"])
            .filter(|u| crate::net::is_http_url(u))
            .ok_or_else(|| ApiError::upstream("OpenArt openart_upload_sign returned no upload URL"))?;
        let access_url = pick(&["accessURL", "accessUrl", "mediaUrl", "publicUrl"])
            .ok_or_else(|| ApiError::upstream("OpenArt openart_upload_sign returned no access URL"))?;
        let upload_id = pick(&["uploadId", "id"]);

        crate::net::check_fetch_url(&sign_url, self.allow_private_urls)
            .await
            .map_err(|e| ApiError::upstream(format!("OpenArt's upload URL was refused: {e}")))?;
        let mut put = self.http.put(&sign_url).timeout(PUT_TIMEOUT).header(reqwest::header::CONTENT_TYPE, content_type);
        // Storage headers only (the signature may cover them); nothing that could carry credentials.
        if let Some(headers) = signed.get("headers").and_then(Value::as_object) {
            for (k, v) in headers.iter().take(32) {
                let name = k.to_ascii_lowercase();
                let allowed = ["x-amz-", "x-goog-", "x-ms-"].iter().any(|p| name.starts_with(p))
                    || (name.starts_with("content-") && name != "content-type" && name != "content-length")
                    || name == "cache-control";
                if let (true, Some(v)) = (allowed, v.as_str()) {
                    put = put.header(k.as_str(), v);
                }
            }
        }
        let resp = put.body(bytes).send().await.map_err(|e| ApiError::upstream(format!("uploading to OpenArt: {}", neterr(e))))?;
        let status = resp.status();
        if !status.is_success() {
            let body = read_limited(resp, 16 << 10).await.unwrap_or_default();
            return Err(ApiError::upstream(format!("uploading to OpenArt: HTTP {status}: {}", snippet(&body))));
        }

        let mut args = json!({ "mediaUrl": access_url, "mediaType": "image", "label": label });
        if let Some(id) = &upload_id {
            args["uploadId"] = json!(id);
        }
        // The tool waits for acceptance; a timeout or a pending status means "not yet".
        let mut last_err = None;
        for attempt in 0..3u32 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(2 * u64::from(attempt))).await;
            }
            match self.call("openart_upload_metadata_get", args.clone()).await {
                Ok(meta) => match reference_from_metadata(&meta, &access_url, upload_id.as_deref()) {
                    Ok(r) => return Ok(r),
                    Err(e) => last_err = Some(e),
                },
                Err(e) if e.code == "upstream" => {
                    warn!("OpenArt upload metadata (attempt {}): {}", attempt + 1, e.message);
                    last_err = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_err.unwrap_or_else(|| ApiError::upstream("OpenArt didn't accept the upload")))
    }
}

/// The `{id, url}` of an accepted upload from `openart_upload_metadata_get`'s result.
fn reference_from_metadata(meta: &Value, access_url: &str, upload_id: Option<&str>) -> ApiResult<OpenArtRef> {
    let status = meta.get("status").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
    if status == "rejected" || status == "failed" || status == "error" {
        let why = meta.get("reason").or_else(|| meta.get("error")).and_then(Value::as_str).unwrap_or("rejected");
        return Err(ApiError::bad_request(format!("OpenArt rejected the upload: {}", snippet(why.as_bytes()))));
    }
    if status == "pending" || status == "processing" {
        return Err(ApiError::upstream("OpenArt is still processing the upload"));
    }
    let vr = meta.get("visualReference").unwrap_or(meta);
    let id = vr.get("id").and_then(Value::as_str).or(upload_id).filter(|s| !s.is_empty());
    let url = vr.get("url").and_then(Value::as_str).unwrap_or(access_url);
    match id {
        Some(id) => Ok(OpenArtRef { id: id.to_string(), url: url.to_string() }),
        None => Err(ApiError::upstream("OpenArt upload metadata has no id")),
    }
}

/// `GET /api/models` from `openart_model_list`: our `Model` shape plus the skill's categories.
pub fn models_response(raw: &Value, routing: &HashMap<String, Vec<String>>) -> Value {
    let models: Vec<Value> = raw
        .get("models")
        .and_then(Value::as_array)
        .map(|ms| {
            ms.iter()
                .take(1000)
                .filter_map(|m| {
                    let id = m.get("id").and_then(Value::as_str)?;
                    let mut modes = serde_json::Map::new();
                    for media in ["image", "video"] {
                        let Some(list) = m.get("modes").and_then(|x| x.get(media)).and_then(Value::as_array) else { continue };
                        let converted: Vec<Value> = list
                            .iter()
                            .filter_map(|md| {
                                Some(json!({
                                    "mode": md.get("mode").and_then(Value::as_str)?,
                                    "description": md.get("description").and_then(Value::as_str).unwrap_or(""),
                                    "credits": md.get("totalCredits").or_else(|| md.get("credits")).and_then(Value::as_f64).unwrap_or(0.0),
                                    "affordable": md.get("affordable").and_then(Value::as_bool).unwrap_or(true),
                                }))
                            })
                            .collect();
                        modes.insert(media.to_string(), Value::Array(converted));
                    }
                    Some(json!({
                        "id": id,
                        "name": m.get("displayName").or_else(|| m.get("name")).and_then(Value::as_str).unwrap_or(id),
                        "description": m.get("description").and_then(Value::as_str).unwrap_or(""),
                        "modes": modes,
                        "categories": routing.get(id).cloned().unwrap_or_default(),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    json!({ "balance": raw.get("balance").and_then(Value::as_f64).unwrap_or(0.0), "models": models })
}

/// One output image of a finished generation.
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub id: String,
    pub url: String,
}

/// The images of a COMPLETED `openart_creation_get` payload (`resources`, or similar lists).
pub fn outputs_from_status(status: &Value) -> Vec<Output> {
    for key in ["resources", "outputs", "images", "items", "results"] {
        if let Some(list) = status.get(key).and_then(Value::as_array) {
            let out: Vec<Output> = list.iter().filter_map(output_from_item).collect();
            if !out.is_empty() {
                return out;
            }
        }
    }
    vec![]
}

/// The images of `history_id` among `openart_creation_list` items.
pub fn outputs_from_list(list: &Value, history_id: &str) -> Vec<Output> {
    list.get("items")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter(|i| i.get("historyId").and_then(Value::as_str) == Some(history_id)).filter_map(output_from_item).collect())
        .unwrap_or_default()
}

fn output_from_item(item: &Value) -> Option<Output> {
    let media = item.get("mediaType").and_then(Value::as_str).unwrap_or("image");
    let status = item.get("status").and_then(Value::as_str).unwrap_or("completed").to_ascii_lowercase();
    if media != "image" || !(status == "completed" || status == "success" || status == "succeeded") {
        return None;
    }
    let url = item.get("url").or_else(|| item.get("imageUrl")).and_then(Value::as_str).filter(|u| crate::net::is_http_url(u))?;
    let id = item.get("id").and_then(Value::as_str).filter(|s| !s.is_empty() && s.len() <= 256)?;
    Some(Output { id: id.to_string(), url: url.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_conversion() {
        let raw = json!({"balance": 88804, "models": [
            {"id":"nano-banana-2-1","displayName":"Nano Banana 2.1","description":"d","modes":{"image":[
                {"mode":"text2image","description":"t","totalCredits":30,"affordable":true},
                {"mode":"image2image","description":"i","totalCredits":32,"affordable":false}]}},
            {"id":"veo3-1","displayName":"Veo","description":"v","modes":{"video":[{"mode":"text2video","totalCredits":120}]}},
            {"displayName":"no id"}
        ], "upgradeUrl": "u"});
        let routing = HashMap::from([("nano-banana-2-1".to_string(), vec!["portrait".to_string()])]);
        let r = models_response(&raw, &routing);
        assert_eq!(r["balance"], json!(88804.0));
        let ms = r["models"].as_array().unwrap();
        assert_eq!(ms.len(), 2);
        assert_eq!(ms[0]["name"], "Nano Banana 2.1");
        assert_eq!(ms[0]["modes"]["image"][1], json!({"mode":"image2image","description":"i","credits":32.0,"affordable":false}));
        assert_eq!(ms[0]["categories"], json!(["portrait"]));
        assert_eq!(ms[1]["modes"]["video"][0]["credits"], json!(120.0));
        assert_eq!(ms[1]["categories"], json!([]));
    }

    #[test]
    fn outputs() {
        // Shape observed from openart_creation_get on 2026-10-15.
        let status = json!({"status":"COMPLETED","historyId":"h","resources":[
            {"id":"a","mediaType":"image","url":"https://cdn.openart.ai/a.png","status":"completed","metadata":{"width":2528,"height":1696}},
            {"id":"b","mediaType":"video","url":"https://cdn.openart.ai/b.mp4","status":"completed"},
            {"id":"c","mediaType":"image","url":"file:///etc/passwd","status":"completed"}]});
        assert_eq!(outputs_from_status(&status), vec![Output { id: "a".into(), url: "https://cdn.openart.ai/a.png".into() }]);
        let list = json!({"items":[{"id":"x","historyId":"h","mediaType":"image","status":"completed","url":"https://cdn.openart.ai/x.png"},
                                   {"id":"y","historyId":"other","mediaType":"image","status":"completed","url":"https://cdn.openart.ai/y.png"}]});
        assert_eq!(outputs_from_list(&list, "h").len(), 1);
        assert!(outputs_from_status(&json!({"status":"COMPLETED"})).is_empty());
    }

    #[test]
    fn upload_metadata() {
        let m = json!({"visualReference":{"type":"image","id":"u1","url":"https://cdn/u1.png","label":"x"}});
        assert_eq!(reference_from_metadata(&m, "https://a", None).unwrap(), OpenArtRef { id: "u1".into(), url: "https://cdn/u1.png".into() });
        assert_eq!(reference_from_metadata(&json!({}), "https://a", Some("up")).unwrap(), OpenArtRef { id: "up".into(), url: "https://a".into() });
        assert_eq!(reference_from_metadata(&json!({"status":"rejected"}), "https://a", Some("up")).unwrap_err().code, "bad_request");
    }
}
