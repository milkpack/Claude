//! Cloud runs through a Claude Code routine's API trigger (mocked).

use reqwest::Method;
use serde_json::{Value, json};

use super::mock::{MockRoutine, ROUTINE_TOKEN, fired};
use super::{Config, TestServer, file_mode, png};

async fn setup(routine: &MockRoutine) -> (TestServer, String, String) {
    let ts = TestServer::start(Config::default()).await;
    ts.login().await;
    let r = ts.post("/api/settings/routine", json!({ "url": routine.url, "token": ROUTINE_TOKEN })).await;
    assert_eq!(r.status(), 200);
    let w: Value = ts.post("/api/workflows", json!({ "name": "S", "graph": { "nodes": [], "edges": [] } })).await.json().await.unwrap();
    let asset = ts.upload_ok(png(16, 16)).await["id"].as_str().unwrap().to_string();
    (ts, w["id"].as_str().unwrap().to_string(), asset)
}

async fn auto_task(ts: &TestServer, sid: &str, kind: &str, inputs: Value) -> Value {
    let r = ts
        .post("/api/tasks", json!({ "sessionId": sid, "nodeId": "n1", "kind": kind, "instructions": "A portrait", "inputs": inputs, "auto": true }))
        .await;
    assert_eq!(r.status(), 200);
    r.json().await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_never_return_the_token() {
    let routine = MockRoutine::start().await;
    let ts = TestServer::start(Config::default()).await;
    ts.login().await;
    assert_eq!(ts.json("/api/settings/routine").await, json!({ "configured": false }));
    assert_eq!(ts.json("/api/me").await["routine"], json!({ "configured": false }));
    for bad in [
        json!({ "url": "https://evil.example/x", "token": ROUTINE_TOKEN }),
        json!({ "url": routine.url, "token": "x" }),
        json!({ "url": routine.url }),
    ] {
        assert_eq!(ts.post("/api/settings/routine", bad.clone()).await.status(), 400, "{bad}");
    }
    let r = ts.post("/api/settings/routine", json!({ "url": routine.url, "token": ROUTINE_TOKEN })).await;
    let text = r.text().await.unwrap();
    assert!(!text.contains("SECRET"), "{text}");
    let s: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(s["configured"], true);
    assert_eq!(s["source"], "settings");
    assert!(s["url"].as_str().unwrap().ends_with("/routines/trig_01A…TUVW/fire"));
    assert_eq!(ts.json("/api/me").await["routine"], json!({ "configured": true }));
    let file = ts.data.join("secrets/routine.json");
    assert_eq!(file_mode(&file), 0o600);
    assert!(std::fs::read_to_string(&file).unwrap().contains(ROUTINE_TOKEN));
    assert!(!ts.get("/api/settings/routine").await.text().await.unwrap().contains("SECRET"));
    let r = ts.send_json(Method::DELETE, "/api/settings/routine", &json!({})).await;
    assert_eq!(r.json::<Value>().await.unwrap(), json!({ "configured": false }));
    assert!(!file.exists());
    ts.forget_cookie();
    assert_eq!(ts.get("/api/settings/routine").await.status(), 401);

    // Environment fallback.
    let ts = TestServer::start(Config { routine_env: Some((routine.url.clone(), ROUTINE_TOKEN.into())), ..Config::default() }).await;
    ts.login().await;
    let s = ts.json("/api/settings/routine").await;
    assert_eq!((s["configured"].clone(), s["source"].clone()), (json!(true), json!("env")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_tasks_fire_the_routine() {
    let routine = MockRoutine::start().await;
    routine.reply(fired(7));
    let (ts, sid, asset) = setup(&routine).await;
    let t = auto_task(&ts, &sid, "director", json!([asset])).await;
    assert_eq!(t["run"]["state"], "fired", "{}", t["run"]);
    assert_eq!(t["run"]["sessionUrl"], "https://claude.ai/code/session_7");
    assert!(t["run"]["at"].as_u64().unwrap() > 0);
    assert!(!t.to_string().contains("SECRET"));
    let (headers, body) = routine.last();
    assert_eq!(headers["authorization"], format!("Bearer {ROUTINE_TOKEN}"));
    assert_eq!(headers["anthropic-version"], "2023-06-01");
    assert_eq!(headers["content-type"], "application/json");
    let text = body["text"].as_str().unwrap();
    assert!(text.starts_with("Photo Studio task — do it and return the result to the link below."));
    assert!(text.contains(t["returnUrl"].as_str().unwrap()) && text.contains("type DirectorResponse"));
    assert!(text.len() <= 64 << 10, "{} bytes", text.len());
    assert_eq!(ts.json(&format!("/api/tasks/{}", t["id"].as_str().unwrap())).await["run"]["state"], "fired");

    // Without `auto`, nothing is fired; re-firing is explicit.
    let before = routine.count();
    let r = ts.post("/api/tasks", json!({ "sessionId": sid, "nodeId": "n1", "kind": "custom", "instructions": "x" })).await;
    let plain: Value = r.json().await.unwrap();
    assert!(plain.get("run").is_none());
    assert_eq!(routine.count(), before);
    let again: Value = ts.post(&format!("/api/tasks/{}/fire", plain["id"].as_str().unwrap()), json!({})).await.json().await.unwrap();
    assert_eq!(again["run"]["state"], "fired");
    assert_eq!(routine.count(), before + 1);

    // Kinds that need a local tool aren't sent.
    let before = routine.count();
    let b = auto_task(&ts, &sid, "blender", json!([])).await;
    assert_eq!(b["run"]["state"], "failed");
    assert!(b["run"]["error"].as_str().unwrap().contains("local tool"));
    assert_eq!(routine.count(), before);
    // A cancelled task can't be fired.
    let id = plain["id"].as_str().unwrap();
    ts.post(&format!("/api/tasks/{id}/cancel"), json!({})).await;
    assert_eq!(ts.post(&format!("/api/tasks/{id}/fire"), json!({})).await.status(), 410);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_fires_are_recorded_and_never_retried() {
    let routine = MockRoutine::start().await;
    let (ts, sid, _) = setup(&routine).await;
    let cases: Vec<(super::mock::FireReply, &str)> = vec![
        (
            (400, vec![], json!({ "type": "error", "error": { "type": "invalid_request_error", "message": "routine is paused" } }), 0),
            "routine is paused",
        ),
        ((404, vec![], json!({ "type": "error", "error": { "type": "not_found_error", "message": "x" } }), 0), "doesn't exist"),
        (
            (
                429,
                vec![("retry-after".into(), "1200".into())],
                json!({ "type": "error", "error": { "type": "rate_limit_error", "message": "x" } }),
                0,
            ),
            "retry after 1200",
        ),
        ((500, vec![], json!({ "type": "error", "error": { "type": "api_error", "message": "boom" } }), 0), "may or may not have started"),
        ((503, vec![], json!({ "type": "error", "error": { "type": "overloaded_error", "message": "busy" } }), 0), "may or may not have started"),
        // Slower than the fire timeout: ambiguous.
        (fired(9), "may or may not have started"),
    ];
    for (i, (reply, expect)) in cases.into_iter().enumerate() {
        let reply = if i == 5 { (reply.0, reply.1, reply.2, 2000) } else { reply };
        routine.reply(reply);
        let before = routine.count();
        let t = auto_task(&ts, &sid, "review", json!([])).await;
        assert_eq!(t["run"]["state"], "failed", "case {i}: {}", t["run"]);
        let e = t["run"]["error"].as_str().unwrap();
        assert!(e.contains(expect), "case {i}: {e}");
        assert!(!e.contains("SECRET") && !t.to_string().contains("SECRET"));
        assert_eq!(routine.count(), before + 1, "case {i}: exactly one request, no retry");
        assert_eq!(t["status"], "open", "the task stays open for a manual run or a re-fire");
    }
    // A wrong token.
    ts.post("/api/settings/routine", json!({ "url": routine.url, "token": "sk-ant-oat01-some-other-token" })).await;
    routine.reply(fired(1));
    let t = auto_task(&ts, &sid, "custom", json!([])).await;
    assert!(t["run"]["error"].as_str().unwrap().contains("rejected the routine token"));
    // Not configured.
    ts.send_json(Method::DELETE, "/api/settings/routine", &json!({})).await;
    let before = routine.count();
    let t = auto_task(&ts, &sid, "custom", json!([])).await;
    assert!(t["run"]["error"].as_str().unwrap().contains("aren't set up"));
    assert_eq!(routine.count(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_hourly_limit() {
    let routine = MockRoutine::start().await;
    routine.reply(fired(1));
    let (ts, sid, _) = setup(&routine).await;
    let r: Value =
        ts.post("/api/tasks", json!({ "sessionId": sid, "nodeId": "n1", "kind": "custom", "instructions": "x" })).await.json().await.unwrap();
    let id = r["id"].as_str().unwrap();
    for i in 0..crate::routine::MAX_FIRES_PER_HOUR {
        let t: Value = ts.post(&format!("/api/tasks/{id}/fire"), json!({})).await.json().await.unwrap();
        assert_eq!(t["run"]["state"], "fired", "fire {i}");
    }
    let t: Value = ts.post(&format!("/api/tasks/{id}/fire"), json!({})).await.json().await.unwrap();
    assert_eq!(t["run"]["state"], "failed");
    assert!(t["run"]["error"].as_str().unwrap().contains("25 cloud runs per hour"));
    assert_eq!(routine.count(), crate::routine::MAX_FIRES_PER_HOUR, "the 26th never left the server");
}
