//! The outbound proxy for Anthropic traffic: used for routine fires, the Claude API and the
//! reachability check; never for OpenArt.

use serde_json::{Value, json};

use super::mock::{MockClaude, MockProxy, MockRoutine, ROUTINE_TOKEN, fired};
use super::{Config, TestServer, connected, png};

async fn auto_task(ts: &TestServer) -> Value {
    let w: Value = ts.post("/api/workflows", json!({ "name": "S", "graph": { "nodes": [], "edges": [] } })).await.json().await.unwrap();
    let r = ts.post("/api/tasks", json!({ "sessionId": w["id"], "nodeId": "n", "kind": "custom", "instructions": "x", "auto": true })).await;
    r.json().await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn routine_fires_through_the_proxy() {
    let routine = MockRoutine::start().await;
    routine.reply(fired(3));
    let proxy = MockProxy::start().await;
    let ts = TestServer::start(Config::default()).await;
    ts.login().await;
    let with_creds = proxy.url.replace("http://", "http://alice:s3cret-PW@");
    let r = ts.post("/api/settings/routine", json!({ "url": routine.url, "token": ROUTINE_TOKEN, "proxy": with_creds })).await;
    let text = r.text().await.unwrap();
    assert!(!text.contains("s3cret") && !text.contains("SECRET"), "{text}");
    let s: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(s["proxy"], proxy.url.as_str(), "masked: no credentials");
    assert_eq!(s["proxySource"], "settings");
    let t = auto_task(&ts).await;
    assert_eq!(t["run"]["state"], "fired", "{}", t["run"]);
    let seen = proxy.requests();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].0.starts_with("POST http://127.0.0.1:") && seen[0].0.contains("/v1/claude_code/routines/"), "{:?}", seen[0]);
    assert_eq!(seen[0].1.as_deref(), Some("Basic YWxpY2U6czNjcmV0LVBX"), "user:pass sent to the proxy");
    assert_eq!(routine.count(), 1);

    // Changing only the proxy keeps the token; null removes the proxy.
    let s: Value = ts.post("/api/settings/routine", json!({ "url": routine.url, "proxy": null })).await.json().await.unwrap();
    assert!(s.get("proxy").is_none() && s["configured"] == true);
    let t = auto_task(&ts).await;
    assert_eq!(t["run"]["state"], "fired");
    assert_eq!(proxy.requests().len(), 1, "direct now");
    assert_eq!(ts.post("/api/settings/routine", json!({ "url": routine.url, "proxy": "ftp://x:1" })).await.status(), 400);
    let secrets = std::fs::read_to_string(ts.data.join("secrets/routine.json")).unwrap();
    assert!(!secrets.contains("proxy"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openart_never_uses_the_proxy() {
    let proxy = MockProxy::start().await;
    let routine = MockRoutine::start().await;
    let (ts, _mock) = connected().await;
    ts.post("/api/settings/routine", json!({ "url": routine.url, "token": ROUTINE_TOKEN, "proxy": proxy.url })).await;
    assert_eq!(ts.get("/api/models").await.status(), 200);
    ts.upload_ok(png(8, 8)).await; // also pushed to OpenArt (sign, PUT, metadata)
    assert!(proxy.requests().is_empty(), "{:?}", proxy.requests());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reachability_check_and_unreachable_fires() {
    // Direct, reachable (any HTTP answer counts).
    let claude = MockClaude::start().await;
    let ts = TestServer::start(Config { claude_url: Some(claude.base.clone()), ..Config::default() }).await;
    ts.login().await;
    let c = ts.json("/api/settings/routine/check").await;
    assert_eq!(c, json!({ "reachable": true, "via": "direct", "status": 404 }));
    // Through a proxy.
    let proxy = MockProxy::start().await;
    let routine = MockRoutine::start().await;
    ts.post("/api/settings/routine", json!({ "url": routine.url, "token": ROUTINE_TOKEN, "proxy": proxy.url })).await;
    let c = ts.json("/api/settings/routine/check").await;
    assert_eq!((c["reachable"].clone(), c["via"].clone()), (json!(true), json!("proxy")));
    assert_eq!(proxy.requests().len(), 1);
    // A dead proxy.
    ts.post("/api/settings/routine", json!({ "url": routine.url, "proxy": "http://127.0.0.1:9" })).await;
    let c = ts.json("/api/settings/routine/check").await;
    assert_eq!((c["reachable"].clone(), c["via"].clone()), (json!(false), json!("proxy")));
    assert!(c["error"].as_str().unwrap().contains("check the Anthropic proxy"));
    let t = auto_task(&ts).await;
    assert!(t["run"]["error"].as_str().unwrap().contains("through the Anthropic proxy"), "{}", t["run"]);

    // Direct and unreachable: the hint says to set a proxy.
    let ts = TestServer::start(Config::default()).await;
    ts.login().await;
    let c = ts.json("/api/settings/routine/check").await;
    assert_eq!((c["reachable"].clone(), c["via"].clone()), (json!(false), json!("direct")));
    assert!(c["error"].as_str().unwrap().contains("set an Anthropic proxy"));
    ts.post("/api/settings/routine", json!({ "url": "http://127.0.0.1:9/v1/claude_code/routines/trig_1/fire", "token": ROUTINE_TOKEN })).await;
    let t = auto_task(&ts).await;
    let e = t["run"]["error"].as_str().unwrap();
    assert!(e.contains("couldn't reach api.anthropic.com (blocked in some countries): set an Anthropic proxy in Settings → Cloud runs"), "{e}");
    ts.forget_cookie();
    assert_eq!(ts.get("/api/settings/routine/check").await.status(), 401);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn env_proxy_carries_the_claude_api() {
    let proxy = MockProxy::start().await;
    let claude = MockClaude::start().await;
    let ts = TestServer::start(Config { claude_url: Some(claude.base.clone()), anthropic_proxy: Some(proxy.url.clone()), ..Config::default() }).await;
    ts.login().await;
    let s = ts.json("/api/settings/routine").await;
    assert_eq!((s["proxy"].clone(), s["proxySource"].clone()), (json!(proxy.url), json!("env")));
    let id = ts.upload_ok(png(20, 20)).await["id"].as_str().unwrap().to_string();
    claude.reply_text(&crate::director::tests::sample(&id).to_string());
    assert_eq!(ts.post("/api/director", json!({ "brief": "x", "references": [{ "assetId": id }] })).await.status(), 200);
    assert!(proxy.requests().iter().any(|(l, _)| l.contains("/v1/messages")), "{:?}", proxy.requests());
}
