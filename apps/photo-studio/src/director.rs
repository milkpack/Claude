//! The director node (`POST /api/director`): Claude turns a brief and labelled reference images
//! into a shot spec, a model plan, a prompt in the chosen model's dialect and its params,
//! following the photo skill (the system prompt). The reply is validated against
//! `DirectorResponse`; anything else is a 502.

use std::collections::HashSet;
use std::sync::Arc;

use log::warn;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::claude::Part;
use crate::error::{ApiError, ApiResult};
use crate::server::App;
use crate::skill::Skill;
use crate::store;

/// Reference images are sent at most this large (long edge, px).
pub const REF_EDGE: u32 = 1024;
const MAX_REFERENCES: usize = 16;
const MAX_BRIEF_CHARS: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Task {
    Generate,
    Edit,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefHint {
    pub asset_id: String,
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectorRequest {
    pub brief: String,
    #[serde(default)]
    pub references: Vec<RefHint>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub task: Option<Task>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotFormat {
    pub aspect_ratio: String,
    pub resolution: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy_space: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShotSpec {
    #[serde(rename = "use")]
    pub use_: String,
    pub format: ShotFormat,
    pub subject: String,
    pub setting: String,
    pub light: String,
    pub camera: String,
    pub medium: String,
    pub retouch: String,
    pub composition: String,
    pub moment: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanStep {
    pub model: String,
    pub mode: String,
    pub why: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelledRef {
    pub asset_id: String,
    pub label: String,
    pub role: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DirectorResponse {
    pub category: String,
    pub spec: ShotSpec,
    pub plan: Vec<PlanStep>,
    pub prompt: String,
    #[serde(default)]
    pub params: Map<String, Value>,
    #[serde(default)]
    pub references: Vec<LabelledRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preserve: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub questions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

pub const CONTRACT: &str = r#"# Output contract

Reply with one JSON object and nothing else: no prose before or after it, no code fences. It must
match this TypeScript type:

```ts
type DirectorResponse = {
  category: string;              // the routing category from the skill, e.g. "portrait"
  spec: {
    use: string;                 // what the image is for (campaign, e-commerce, editorial, social…)
    format: { aspectRatio: string; resolution: string; copySpace?: string };
    subject: string; setting: string; light: string; camera: string; medium: string;
    retouch: string; composition: string; moment: string;
  };
  plan: { model: string; mode: string; why: string }[];   // plan[0] is the job to run now
  prompt: string;                // the full prompt for plan[0], in that model's dialect
  params: object;                // plan[0]'s form fields except prompt and visualReferences
  references: { assetId: string; label: string; role: string }[];   // in the order they are sent
  preserve?: string[];           // edits: the "keep exactly" list
  questions?: string[];          // only when the brief is missing something decisive
  notes?: string;                // one or two lines for the owner: assumptions, cost remarks
};
```

Field rules:
- spec: short phrases in English. format.aspectRatio and format.resolution are the values you set
  in params (e.g. "3:2", "4K").
- plan: plan[0] is the recommended model and mode (use "image2image" whenever references are
  sent, "text2image" otherwise). Add more steps only for a chain (generate, then edit, then
  reframe) or an A/B alternative, and say which in `why`. Use only model ids and modes from the
  live model list when it is given, and prefer affordable ones.
- params: always the aspect-ratio field and the resolution field of that model, the image count
  for the variants you want, and `autoEnhancePrompt: false` where the model has it. Use only
  fields and enum values the skill's model reference lists for the model. The server adds
  `prompt` and `visualReferences` itself.
- references: every reference image you want sent, in the order the prompt names them, labelled
  "image1", "image2", … to match "image 1", "image 2" in the prompt. Copy each assetId exactly
  from the request. role is one of: identity, outfit, product, lighting, style, pose, background,
  edit-source. For an edit the edit source goes first. Leave out a reference you decide not to
  use, and say why in notes.
- questions: when you would ask the owner something, ask here and still fill every other field
  with your best assumption."#;

/// The director's system prompt: role, the photo skill, the output contract.
pub fn system_prompt(skill: &Skill) -> String {
    format!(
        "You are the director inside Photo Studio, a private node canvas for photographic image generation. \
The owner gives you a brief (in any language) and optional reference images; you turn them into a shot and a \
ready-to-run generation plan for OpenArt models. The photo skill below is your method: follow it.\n\n\
How the skill applies here: you don't call any tools. The server has already given you the live OpenArt model \
list (ids, modes, credits) and every reference image with its metadata. Do the skill's workflow steps 1 to 5 \
(read the brief, decide the shot, route, plan the references, write the prompt and params); the Generate node \
quotes and runs your plan, and the Review node checks the results.\n\n\
# The photo skill\n\n{}\n{CONTRACT}\n",
        skill.prompt_text()
    )
}

/// The live image models, compactly, for the request.
pub fn model_summary(raw: &Value) -> Vec<Value> {
    raw.get("models")
        .and_then(Value::as_array)
        .map(|ms| {
            ms.iter()
                .filter_map(|m| {
                    let modes: Vec<Value> = m
                        .get("modes")
                        .and_then(|x| x.get("image"))
                        .and_then(Value::as_array)?
                        .iter()
                        .filter_map(|md| {
                            Some(json!({
                                "mode": md.get("mode").and_then(Value::as_str)?,
                                "credits": md.get("totalCredits").and_then(Value::as_f64),
                                "affordable": md.get("affordable").and_then(Value::as_bool).unwrap_or(true),
                            }))
                        })
                        .collect();
                    if modes.is_empty() {
                        return None;
                    }
                    Some(json!({
                        "id": m.get("id").and_then(Value::as_str)?,
                        "name": m.get("displayName").and_then(Value::as_str),
                        "description": m.get("description").and_then(Value::as_str).unwrap_or(""),
                        "modes": modes,
                    }))
                })
                .take(200)
                .collect()
        })
        .unwrap_or_default()
}

pub fn validate(resp: &DirectorResponse, allowed: &HashSet<&str>) -> Result<(), String> {
    if resp.category.trim().is_empty() {
        return Err("category is empty".into());
    }
    if resp.prompt.trim().is_empty() {
        return Err("prompt is empty".into());
    }
    let Some(first) = resp.plan.first() else { return Err("plan is empty".into()) };
    if first.model.trim().is_empty() || first.mode.trim().is_empty() {
        return Err("plan[0] has no model or mode".into());
    }
    let mut seen = HashSet::new();
    for r in &resp.references {
        if !allowed.contains(r.asset_id.as_str()) {
            return Err(format!("reference `{}` is not one of the request's assets", r.asset_id));
        }
        if !seen.insert(r.asset_id.as_str()) {
            return Err(format!("reference `{}` is listed twice", r.asset_id));
        }
        if r.label.trim().is_empty() {
            return Err("a reference has an empty label".into());
        }
    }
    Ok(())
}

/// `POST /api/director`.
pub async fn run(app: Arc<App>, req: DirectorRequest) -> ApiResult<DirectorResponse> {
    let claude = app.claude.as_ref().ok_or_else(ApiError::claude_not_configured)?;
    let brief = req.brief.trim();
    if brief.is_empty() {
        return Err(ApiError::bad_request("brief is empty"));
    }
    if brief.chars().count() > MAX_BRIEF_CHARS {
        return Err(ApiError::bad_request(format!("brief is longer than {MAX_BRIEF_CHARS} characters")));
    }
    if req.references.len() > MAX_REFERENCES {
        return Err(ApiError::bad_request(format!("at most {MAX_REFERENCES} references")));
    }
    if let Some(m) = &req.model
        && (m.is_empty() || m.len() > 128)
    {
        return Err(ApiError::bad_request("invalid model"));
    }

    // References: metadata plus a ≤1024 px JPEG each, so Claude sees what it labels.
    let mut refs_meta = Vec::new();
    let mut images = Vec::new();
    for (i, r) in req.references.iter().enumerate() {
        let asset = store::valid_id(&r.asset_id)
            .then(|| app.assets.get(&r.asset_id))
            .flatten()
            .ok_or_else(|| ApiError::bad_request(format!("unknown reference asset `{}`", r.asset_id)))?;
        let role = r.role.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.chars().take(64).collect::<String>());
        let bytes = app.assets.read_file(&asset).await?;
        let jpeg = crate::imaging::run(move || crate::imaging::jpeg_for_claude(&bytes, REF_EDGE))
            .await
            .map_err(ApiError::internal)?
            .map_err(ApiError::bad_request)?;
        refs_meta.push(json!({
            "image": format!("Reference {}", i + 1),
            "assetId": asset.id,
            "roleHint": role,
            "kind": asset.kind,
            "width": asset.width,
            "height": asset.height,
            "createdBy": asset.model.as_ref().map(|m| json!({ "model": m, "mode": asset.mode, "prompt": asset.prompt })),
        }));
        images.push((i + 1, asset.id.clone(), role, jpeg));
    }
    let task = req.task.unwrap_or_else(|| {
        if req.references.iter().any(|r| r.role.as_deref().map(str::trim) == Some("edit-source")) { Task::Edit } else { Task::Generate }
    });

    let models = match app.openart.model_list(false).await {
        Ok(raw) => Some(model_summary(&raw)),
        Err(e) => {
            if e.code != "openart_not_connected" && e.code != "openart_reconnect" {
                warn!("director: model list unavailable: {}", e.message);
            }
            None
        }
    };

    let request = json!({ "brief": brief, "task": task, "forcedModel": req.model, "references": refs_meta });
    let mut parts = vec![Part::Text(format!(
        "## Request\n\n{}\n\n{}",
        serde_json::to_string_pretty(&request).unwrap_or_default(),
        match &req.model {
            Some(m) => format!("The owner asked for model `{m}`: use it for plan[0]."),
            None => "Route the job yourself.".to_string(),
        }
    ))];
    parts.push(Part::Text(match &models {
        Some(list) => format!(
            "## Available OpenArt image models (live; credits per job at default settings)\n\n{}",
            serde_json::to_string(list).unwrap_or_default()
        ),
        None => "## Available OpenArt image models\n\nThe live list is unavailable (OpenArt is not connected); use the skill's model reference."
            .to_string(),
    }));
    for (n, id, role, jpeg) in images {
        let hint = role.map(|r| format!(", role hint: {r}")).unwrap_or_default();
        parts.push(Part::Text(format!("Reference {n} (assetId {id}{hint}):")));
        parts.push(Part::Jpeg(jpeg));
    }
    parts.push(Part::Text("Reply with the DirectorResponse JSON object only.".into()));

    let reply = claude.json_reply("director", &app.director_system, &parts).await?;
    let resp: DirectorResponse =
        serde_json::from_value(reply).map_err(|e| ApiError::upstream(format!("Claude's director reply doesn't match DirectorResponse: {e}")))?;
    let allowed: HashSet<&str> = req.references.iter().map(|r| r.asset_id.as_str()).collect();
    validate(&resp, &allowed).map_err(|e| ApiError::upstream(format!("Claude's director reply is invalid: {e}")))?;
    Ok(resp)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn sample(asset_id: &str) -> Value {
        json!({
            "category": "portrait",
            "spec": {
                "use": "editorial", "format": {"aspectRatio": "3:2", "resolution": "4K"},
                "subject": "a woman in her thirties", "setting": "studio", "light": "soft window light",
                "camera": "85mm", "medium": "digital photograph", "retouch": "natural",
                "composition": "rule of thirds", "moment": "candid laugh"
            },
            "plan": [{"model": "nano-banana-2-1", "mode": "image2image", "why": "best for people"}],
            "prompt": "Editorial portrait of the woman in image 1…",
            "params": {"aspectRatio": "3:2", "resolution": "4K", "imageCount": 4, "autoEnhancePrompt": false},
            "references": [{"assetId": asset_id, "label": "image1", "role": "identity"}],
            "notes": "assumed natural retouch"
        })
    }

    #[test]
    fn contract_validation() {
        let allowed: HashSet<&str> = ["a1"].into_iter().collect();
        let ok: DirectorResponse = serde_json::from_value(sample("a1")).unwrap();
        assert!(validate(&ok, &allowed).is_ok());
        let back = serde_json::to_value(&ok).unwrap();
        assert_eq!(back["spec"]["use"], "editorial");
        assert!(back.get("questions").is_none());
        let unknown: DirectorResponse = serde_json::from_value(sample("zz")).unwrap();
        assert!(validate(&unknown, &allowed).is_err());
        let mut v = sample("a1");
        v["plan"] = json!([]);
        assert!(validate(&serde_json::from_value(v).unwrap(), &allowed).is_err());
        let mut v = sample("a1");
        v["params"] = json!("4K");
        assert!(serde_json::from_value::<DirectorResponse>(v).is_err());
        let mut v = sample("a1");
        v["spec"].as_object_mut().unwrap().remove("light");
        assert!(serde_json::from_value::<DirectorResponse>(v).is_err());
    }

    #[test]
    fn system_prompt_has_skill_and_contract() {
        let s = system_prompt(&Skill { source: "t".into(), files: vec![("SKILL.md".into(), "SKILL BODY".into())] });
        assert!(s.contains("SKILL BODY") && s.contains("type DirectorResponse") && s.contains("edit-source"));
    }
}
