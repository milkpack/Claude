//! The review node (`POST /api/review`): Claude scores a generated image (and, for edits, compares
//! it with the source) following the photo skill's review checklist, and proposes a fix. Images
//! are sent as JPEGs of at most 1568 px. The reply is validated against `ReviewResponse`.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::claude::Part;
use crate::error::{ApiError, ApiResult};
use crate::server::App;
use crate::skill::Skill;
use crate::store;

/// Images are sent at most this large (long edge, px).
pub const REVIEW_EDGE: u32 = 1568;
const MAX_TEXT_CHARS: usize = 20_000;
const MAX_PRESERVE: usize = 64;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewRequest {
    pub asset_id: String,
    #[serde(default)]
    pub brief: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub source_asset_id: Option<String>,
    #[serde(default)]
    pub preserve: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scores {
    #[serde(serialize_with = "ser_score")]
    pub realism: f64,
    #[serde(serialize_with = "ser_score")]
    pub skin: f64,
    #[serde(serialize_with = "ser_score")]
    pub light: f64,
    #[serde(serialize_with = "ser_score")]
    pub anatomy: f64,
    #[serde(serialize_with = "ser_score")]
    pub brief: f64,
    #[serde(default, skip_serializing_if = "Option::is_none", serialize_with = "ser_opt_score")]
    pub identity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", serialize_with = "ser_opt_score")]
    pub preserve: Option<f64>,
}

/// Whole scores go out as integers (`4`, not `4.0`).
fn ser_score<S: serde::Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e9 { s.serialize_i64(*v as i64) } else { s.serialize_f64(*v) }
}

fn ser_opt_score<S: serde::Serializer>(v: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(x) => ser_score(x, s),
        None => s.serialize_none(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Minor,
    Major,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Defect {
    pub area: String,
    pub problem: String,
    pub severity: Severity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Accept,
    Retry,
    Rethink,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fix {
    pub prompt: String,
    pub why: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReviewResponse {
    pub scores: Scores,
    #[serde(default)]
    pub defects: Vec<Defect>,
    pub verdict: Verdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
}

pub const CONTRACT: &str = r#"# Output contract

Reply with one JSON object and nothing else: no prose before or after it, no code fences. It must
match this TypeScript type:

```ts
type ReviewResponse = {
  scores: {                      // 1 (unusable) to 5 (indistinguishable from a professional photograph / fully on brief)
    realism: number; skin: number; light: number; anatomy: number; brief: number;
    identity?: number;           // only when a person's identity had to be kept (source or identity reference)
    preserve?: number;           // only for edits: how well the "keep exactly" list was kept
  };
  defects: { area: string; problem: string; severity: "minor" | "major" }[];
  verdict: "accept" | "retry" | "rethink";
  fix?: { prompt: string; why: string };
};
```

Field rules:
- scores are whole numbers from 1 to 5. Judge the image for its use, at the size it will be seen.
- defects: concrete and located ("left hand, ring finger", "sign text, second line"); "major" when
  it ruins the image for its use. An empty list when there is nothing to fix.
- verdict: "accept" when it can be delivered as is; "retry" when the approach is right and a
  targeted edit or a corrected prompt fixes it; "rethink" when the model or the approach is wrong.
- fix: required for "retry" and "rethink": either a corrected full prompt or a targeted edit
  instruction (change only … / keep exactly …), with why in one line."#;

/// The reviewer's system prompt: role, the photo skill, the output contract.
pub fn system_prompt(skill: &Skill) -> String {
    format!(
        "You are the reviewer inside Photo Studio, a private node canvas for photographic image generation. \
You check a generated photograph against the owner's brief and the prompt that produced it, and, for edits, \
against the source image, following the photo skill below (its review reference is your checklist). You see \
downscaled copies (at most 1568 px); judge fine detail (skin texture, hands, small text) with that in mind. \
You don't call any tools: score, list the defects, give a verdict and a fix.\n\n\
# The photo skill\n\n{}\n{CONTRACT}\n",
        skill.prompt_text()
    )
}

fn valid_score(s: f64) -> bool {
    s.is_finite() && (1.0..=5.0).contains(&s)
}

pub fn validate(r: &ReviewResponse) -> Result<(), String> {
    let s = &r.scores;
    for (name, v) in [("realism", s.realism), ("skin", s.skin), ("light", s.light), ("anatomy", s.anatomy), ("brief", s.brief)] {
        if !valid_score(v) {
            return Err(format!("scores.{name} is {v}, not 1–5"));
        }
    }
    for (name, v) in [("identity", s.identity), ("preserve", s.preserve)] {
        if let Some(v) = v
            && !valid_score(v)
        {
            return Err(format!("scores.{name} is {v}, not 1–5"));
        }
    }
    if r.verdict != Verdict::Accept && r.fix.as_ref().is_none_or(|f| f.prompt.trim().is_empty()) {
        return Err("a retry or rethink verdict needs a fix".into());
    }
    Ok(())
}

fn bounded(name: &str, s: &Option<String>) -> ApiResult<Option<String>> {
    match s.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(t) if t.chars().count() > MAX_TEXT_CHARS => Err(ApiError::bad_request(format!("{name} is longer than {MAX_TEXT_CHARS} characters"))),
        other => Ok(other.map(str::to_string)),
    }
}

/// A stored asset as a ≤1568 px JPEG.
async fn review_jpeg(app: &App, id: &str, what: &str) -> ApiResult<(crate::assets::StoredAsset, Vec<u8>)> {
    let asset = store::valid_id(id).then(|| app.assets.get(id)).flatten().ok_or_else(|| ApiError::not_found(format!("no such {what} asset")))?;
    let bytes = app.assets.read_file(&asset).await?;
    let jpeg = crate::imaging::run(move || crate::imaging::jpeg_for_claude(&bytes, REVIEW_EDGE))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::bad_request)?;
    Ok((asset, jpeg))
}

/// `POST /api/review`.
pub async fn run(app: Arc<App>, req: ReviewRequest) -> ApiResult<ReviewResponse> {
    let claude = app.claude.as_ref().ok_or_else(ApiError::claude_not_configured)?;
    let brief = bounded("brief", &req.brief)?;
    let preserve: Vec<String> = req.preserve.unwrap_or_default().into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if preserve.len() > MAX_PRESERVE || preserve.iter().any(|p| p.chars().count() > 1000) {
        return Err(ApiError::bad_request("preserve list is too long"));
    }
    let (asset, image) = review_jpeg(&app, &req.asset_id, "image").await?;
    let prompt = bounded("prompt", &req.prompt)?.or_else(|| asset.prompt.clone());
    let source = match req.source_asset_id.as_deref().filter(|s| !s.is_empty()) {
        Some(id) => Some(review_jpeg(&app, id, "source").await?),
        None => None,
    };

    let context = json!({
        "brief": brief,
        "prompt": prompt,
        "preserve": preserve,
        "isEdit": source.is_some(),
        "image": { "width": asset.width, "height": asset.height, "model": asset.model, "mode": asset.mode, "params": asset.params },
    });
    let mut parts = vec![Part::Text(format!("## Job\n\n{}", serde_json::to_string_pretty(&context).unwrap_or_default()))];
    parts.push(Part::Text("Image under review:".into()));
    parts.push(Part::Jpeg(image));
    if let Some((_, src)) = source {
        parts.push(Part::Text("Source image (before the edit):".into()));
        parts.push(Part::Jpeg(src));
    }
    parts.push(Part::Text("Reply with the ReviewResponse JSON object only.".into()));

    let reply = claude.json_reply("review", &app.review_system, &parts).await?;
    let resp: ReviewResponse =
        serde_json::from_value(reply).map_err(|e| ApiError::upstream(format!("Claude's review reply doesn't match ReviewResponse: {e}")))?;
    validate(&resp).map_err(|e| ApiError::upstream(format!("Claude's review reply is invalid: {e}")))?;
    Ok(resp)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use serde_json::Value;

    pub fn sample() -> Value {
        json!({
            "scores": {"realism": 4, "skin": 3, "light": 5, "anatomy": 4, "brief": 5},
            "defects": [{"area": "left hand", "problem": "six fingers", "severity": "major"}],
            "verdict": "retry",
            "fix": {"prompt": "Change only the left hand: five fingers. Keep exactly everything else.", "why": "anatomy"}
        })
    }

    #[test]
    fn contract_validation() {
        let ok: ReviewResponse = serde_json::from_value(sample()).unwrap();
        assert!(validate(&ok).is_ok());
        let mut v = sample();
        v["scores"]["skin"] = json!(7);
        assert!(validate(&serde_json::from_value(v).unwrap()).is_err());
        let mut v = sample();
        v["verdict"] = json!("maybe");
        assert!(serde_json::from_value::<ReviewResponse>(v).is_err());
        let mut v = sample();
        v["fix"] = Value::Null;
        assert!(validate(&serde_json::from_value(v).unwrap()).is_err(), "retry needs a fix");
        let mut v = sample();
        v["defects"][0]["severity"] = json!("catastrophic");
        assert!(serde_json::from_value::<ReviewResponse>(v).is_err());
        let back = serde_json::to_value(&ok).unwrap();
        assert!(back["scores"].get("identity").is_none());
    }

    #[test]
    fn system_prompt_has_contract() {
        let s = system_prompt(&Skill::embedded());
        assert!(s.contains("type ReviewResponse") && s.contains("models.md"));
    }
}
