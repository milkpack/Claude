//! Director and review against a mock Anthropic API: request shape (headers, model, skill in the
//! system prompt, image blocks within the size limits, the live model list), reply validation,
//! retries, and errors.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use super::{Config, MockClaude, MockOpenArt, TestServer, dims, png};

fn images(body: &Value) -> Vec<Vec<u8>> {
    body["messages"][0]["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["type"] == "image")
        .map(|b| {
            assert_eq!(b["source"]["type"], "base64");
            assert_eq!(b["source"]["media_type"], "image/jpeg");
            STANDARD.decode(b["source"]["data"].as_str().unwrap()).unwrap()
        })
        .collect()
}

fn texts(body: &Value) -> String {
    body["messages"][0]["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["type"] == "text")
        .map(|b| b["text"].as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

async fn with_claude(openart: Option<&MockOpenArt>) -> (TestServer, MockClaude) {
    let claude = MockClaude::start().await;
    let ts =
        TestServer::start(Config { claude_url: Some(claude.base.clone()), openart_mcp_url: openart.map(|m| m.mcp_url()), ..Config::default() }).await;
    ts.login().await;
    if let Some(m) = openart {
        ts.connect_openart(m).await;
    }
    (ts, claude)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn director_builds_the_request_and_validates_the_reply() {
    let openart = MockOpenArt::start().await;
    let (ts, claude) = with_claude(Some(&openart)).await;
    let me = ts.json("/api/me").await;
    assert_eq!(me["claude"], json!({ "configured": true, "model": "claude-opus-5-5" }));
    let reference = ts.upload_ok(png(2000, 1500)).await;
    let id = reference["id"].as_str().unwrap().to_string();

    let sample = crate::director::tests::sample(&id);
    claude.reply_text(&format!("```json\n{sample}\n```"));
    let r =
        ts.post("/api/director", json!({ "brief": "Портрет для журнала, тёплый свет", "references": [{ "assetId": id, "role": "identity" }] })).await;
    assert_eq!(r.status(), 200);
    let resp: Value = r.json().await.unwrap();
    assert_eq!(resp, sample);

    let (headers, body) = claude.last_request();
    assert_eq!(headers["x-api-key"], "test-key");
    assert_eq!(headers["anthropic-version"], "2023-06-01");
    assert_eq!(headers["anthropic-beta"], "server-side-fallback-2026-07-01");
    assert_eq!(body["model"], "claude-opus-5-5");
    assert_eq!(body["max_tokens"], 16000);
    assert_eq!(body["thinking"], json!({ "type": "adaptive" }));
    assert_eq!(body["output_config"], json!({ "effort": "medium" }));
    assert_eq!(body["fallbacks"], "default");
    let system = body["system"][0]["text"].as_str().unwrap();
    assert!(system.contains("type DirectorResponse"));
    assert!(system.contains("<skill_file path=\"references/models.md\">"), "the photo skill is in the system prompt");
    assert_eq!(body["system"][0]["cache_control"], json!({ "type": "ephemeral" }));
    let imgs = images(&body);
    assert_eq!(imgs.len(), 1);
    assert_eq!(dims(&imgs[0]), (1024, 768), "references are sent at ≤1024 px");
    let t = texts(&body);
    assert!(t.contains("Портрет для журнала"), "the brief is passed as written");
    assert!(t.contains(&id) && t.contains("\"roleHint\": \"identity\"") && t.contains("\"width\": 2000"));
    assert!(t.contains("\"task\": \"generate\""));
    assert!(t.contains("Available OpenArt image models (live") && t.contains("nano-banana-2-1") && t.contains("\"credits\":32.0"));
    assert!(!t.contains("veo3-1"), "video-only models are left out");

    // An edit source makes it an edit; a forced model is passed on.
    claude.reply_text(&sample.to_string());
    let r = ts
        .post(
            "/api/director",
            json!({ "brief": "remove the lamp", "model": "gpt-image-2", "references": [{ "assetId": id, "role": "edit-source" }] }),
        )
        .await;
    assert_eq!(r.status(), 200);
    let t = texts(&claude.last_request().1);
    assert!(t.contains("\"task\": \"edit\"") && t.contains("use it for plan[0]"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn director_rejects_bad_replies_and_requests() {
    let (ts, claude) = with_claude(None).await;
    let id = ts.upload_ok(png(100, 80)).await["id"].as_str().unwrap().to_string();
    let good = crate::director::tests::sample(&id);

    // Without OpenArt the director still runs, from the skill's model reference.
    claude.reply_text(&good.to_string());
    assert_eq!(ts.post("/api/director", json!({ "brief": "a portrait", "references": [{ "assetId": id }] })).await.status(), 200);
    assert!(texts(&claude.last_request().1).contains("live list is unavailable"));

    let mut missing_field = good.clone();
    missing_field.as_object_mut().unwrap().remove("prompt");
    let mut unknown_ref = good.clone();
    unknown_ref["references"][0]["assetId"] = json!("someone-else");
    for reply in ["I'd love to help! What's the use?".to_string(), "{\"category\": 3}".into(), missing_field.to_string(), unknown_ref.to_string()] {
        claude.reply_text(&reply);
        let r = ts.post("/api/director", json!({ "brief": "a portrait", "references": [{ "assetId": id }] })).await;
        assert_eq!(r.status(), 502, "{reply}");
        let e: Value = r.json().await.unwrap();
        assert_eq!(e["code"], "upstream");
        assert!(e["error"].as_str().unwrap().contains("director"), "{e}");
    }

    // A refusal and an API error are upstream errors too.
    claude.replies(vec![(200, vec![], json!({ "content": [], "stop_reason": "refusal", "stop_details": { "type": "refusal", "category": "bio" } }))]);
    let e: Value = ts.post("/api/director", json!({ "brief": "x" })).await.json().await.unwrap();
    assert!(e["error"].as_str().unwrap().contains("declined"), "{e}");
    claude.replies(vec![(400, vec![], json!({ "type": "error", "error": { "type": "invalid_request_error", "message": "bad image" } }))]);
    let r = ts.post("/api/director", json!({ "brief": "x" })).await;
    assert_eq!(r.status(), 502);
    assert!(r.json::<Value>().await.unwrap()["error"].as_str().unwrap().contains("bad image"));
    // Overloaded (529) is retried, honouring retry-after.
    let before = claude.request_count();
    claude.replies(vec![
        (529, vec![("retry-after".into(), "0".into())], json!({ "type": "error", "error": { "type": "overloaded_error", "message": "busy" } })),
        (200, vec![], super::mock::message(&good.to_string())),
    ]);
    assert_eq!(ts.post("/api/director", json!({ "brief": "x", "references": [{ "assetId": id }] })).await.status(), 200);
    assert_eq!(claude.request_count(), before + 2);

    // Bad requests never reach Claude.
    let before = claude.request_count();
    assert_eq!(ts.post("/api/director", json!({ "brief": "  " })).await.status(), 400);
    assert_eq!(ts.post("/api/director", json!({ "brief": "x", "references": [{ "assetId": "missing" }] })).await.status(), 400);
    assert_eq!(ts.post("/api/director", json!({ "brief": "x", "task": "dance" })).await.status(), 400);
    assert_eq!(claude.request_count(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_not_configured() {
    let ts = TestServer::start(Config::default()).await;
    ts.login().await;
    for (path, body) in [("/api/director", json!({ "brief": "x" })), ("/api/review", json!({ "assetId": "x" }))] {
        let r = ts.post(path, body).await;
        assert_eq!(r.status(), 503, "{path}");
        assert_eq!(r.json::<Value>().await.unwrap()["code"], "claude_not_configured");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_sends_downscaled_images() {
    let (ts, claude) = with_claude(None).await;
    let image = ts.upload_ok(png(2000, 1000)).await["id"].as_str().unwrap().to_string();
    let source = ts.upload_ok(png(1000, 1600)).await["id"].as_str().unwrap().to_string();
    let sample = crate::review::tests::sample();
    claude.reply_text(&sample.to_string());
    let r = ts
        .post(
            "/api/review",
            json!({ "assetId": image, "brief": "editorial", "prompt": "the prompt", "sourceAssetId": source, "preserve": ["face", "background"] }),
        )
        .await;
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<Value>().await.unwrap(), sample);
    let body = claude.last_request().1;
    let imgs = images(&body);
    assert_eq!(imgs.len(), 2);
    assert_eq!(dims(&imgs[0]), (1568, 784), "the image under review, ≤1568 px");
    assert_eq!(dims(&imgs[1]), (980, 1568), "the source, ≤1568 px");
    let t = texts(&body);
    assert!(t.contains("\"isEdit\": true") && t.contains("the prompt") && t.contains("background"));
    assert!(body["system"][0]["text"].as_str().unwrap().contains("type ReviewResponse"));

    // Malformed replies are 502s.
    let mut bad = sample.clone();
    bad["scores"]["realism"] = json!(9);
    for reply in [bad.to_string(), "{\"verdict\": \"accept\"}".into(), "not json".into()] {
        claude.reply_text(&reply);
        let r = ts.post("/api/review", json!({ "assetId": image })).await;
        assert_eq!(r.status(), 502, "{reply}");
        assert_eq!(r.json::<Value>().await.unwrap()["code"], "upstream");
    }
    assert_eq!(ts.post("/api/review", json!({ "assetId": "missing" })).await.status(), 404);
    assert_eq!(ts.post("/api/review", json!({ "assetId": image, "sourceAssetId": "missing" })).await.status(), 404);
}
