//! Regression tests for the review of the first version (findings H1, H2, M1, M3, L5, L8; H3
//! and M2 are covered by unit tests in `auth.rs`, `cli.rs` and `imaging.rs`).

use std::sync::atomic::Ordering;
use std::time::Duration;

use reqwest::header;
use serde_json::{Value, json};

use super::{Config, MockOpenArt, TestServer, connected, png, temp_dir};

fn completed(mock: &MockOpenArt, history: &str, name: &str) -> Value {
    let url = mock.add_image(&format!("{name}.png"), png(64, 48));
    json!({ "status": "COMPLETED", "historyId": history, "resources": [{ "id": name, "mediaType": "image", "url": url, "status": "completed" }] })
}

/// H1: a request dropped while OpenArt is accepting the job must not lose the (paid) job.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn job_survives_a_dropped_request() {
    let (ts, mock) = connected().await;
    mock.script(vec![completed(&mock, "h1", "d1")]);
    mock.state.generate_delay_ms.store(800, Ordering::SeqCst);
    let body = json!({ "model": "nano-banana-2-1", "mode": "text2image", "params": { "prompt": "x" } });
    let r = ts.post_abandoned("/api/jobs", body, Duration::from_millis(300)).await;
    assert!(r.is_err(), "the client gave up");
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(mock.state.calls_to("openart_generate_image").len(), 1);
    let jobs = ts.json("/api/jobs").await;
    let list = jobs["jobs"].as_array().unwrap();
    assert_eq!(list.len(), 1, "the job was kept");
    let done = ts.wait_job(list[0]["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "completed", "{done}");
    assert_eq!(done["historyId"], "h1");
    assert_eq!(done["assets"][0]["openart"]["id"], "d1");
}

/// H1: a job saved but never confirmed by OpenArt (the server stopped mid-submission) is
/// reported, not left queued forever or resubmitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupted_submission_is_reported_after_restart() {
    let data = temp_dir("submit");
    std::fs::create_dir_all(data.join("jobs")).unwrap();
    let id = crate::store::new_id();
    let now = crate::store::now_ms();
    let job = json!({ "id": id, "status": "queued", "model": "m", "mode": "text2image", "params": {}, "references": [], "createdAt": now, "updatedAt": now });
    std::fs::write(data.join(format!("jobs/{id}.json")), job.to_string()).unwrap();
    let ts = TestServer::start(Config { data_dir: Some(data.clone()), ..Config::default() }).await;
    ts.login().await;
    let j = ts.wait_job(&id).await;
    assert_eq!(j["status"], "failed");
    assert!(j["error"].as_str().unwrap().contains("check the OpenArt history"), "{j}");
    drop(ts);
    let _ = std::fs::remove_dir_all(&data);
}

/// H2: a request cancelled during a token refresh must not lose the rotated refresh token.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_refresh_keeps_the_rotated_token() {
    let (ts, mock) = connected().await;
    mock.state.rotate_refresh.store(true, Ordering::SeqCst);
    mock.state.refresh_delay_ms.store(800, Ordering::SeqCst);
    mock.state.expire_access_tokens();
    let cost = json!({ "model": "nano-banana-2-1", "mode": "text2image", "params": {} });
    assert!(ts.post_abandoned("/api/cost", cost.clone(), Duration::from_millis(300)).await.is_err());
    tokio::time::sleep(Duration::from_millis(1200)).await;
    mock.state.refresh_delay_ms.store(0, Ordering::SeqCst);
    let r = ts.post("/api/cost", cost).await;
    assert_eq!(r.status(), 200, "{:?}", r.text().await);
    let me = ts.json("/api/me").await;
    assert_eq!(me["openart"]["connected"], true);
    assert_eq!(me["openart"]["needsReconnect"], false);
    let stored = std::fs::read_to_string(ts.data.join("secrets/openart.json")).unwrap();
    assert!(stored.contains("rt-1") && !stored.contains("rt-0"), "the rotated refresh token was saved");
}

/// M1 (sign out everywhere) and L8 (a stale cookie can always be cleared).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logout_everywhere_and_stale_cookies() {
    let ts = TestServer::start(Config::default()).await;
    ts.login().await;
    let first = ts.cookie();
    ts.login().await;
    assert_eq!(ts.get("/api/assets").await.status(), 200);
    let r = ts.post("/api/logout?all=1", json!({})).await;
    assert_eq!(r.status(), 200);
    assert_eq!(ts.get("/api/assets").await.status(), 401);
    ts.set_cookie(first);
    assert_eq!(ts.get("/api/assets").await.status(), 401, "the other session ended too");
    // A stale or unknown cookie still gets cleared.
    ts.set_cookie(Some("not-a-session".into()));
    let r = ts.post("/api/logout", json!({})).await;
    assert_eq!(r.status(), 200);
    assert!(r.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap().contains("Max-Age=0"));
    // `all` without a session signs nobody else out.
    ts.login().await;
    let live = ts.cookie();
    ts.set_cookie(Some("not-a-session".into()));
    assert_eq!(ts.post("/api/logout?all=1", json!({})).await.status(), 200);
    ts.set_cookie(live);
    assert_eq!(ts.get("/api/assets").await.status(), 200);
}

/// M3: while OpenArt needs a reconnect, running jobs wait instead of giving up; after the
/// reconnect they finish. Jobs that had already given up are revived by the reconnect.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn jobs_wait_for_a_reconnect() {
    let mock = MockOpenArt::start().await;
    let ts =
        TestServer::start(Config { openart_mcp_url: Some(mock.mcp_url()), give_up: Some(Duration::from_millis(300)), ..Config::default() }).await;
    ts.login().await;
    ts.connect_openart(&mock).await;
    mock.script(vec![json!({ "status": "RUNNING", "historyId": "h1", "pollAfterSeconds": 0.01 })]);
    let body = json!({ "model": "nano-banana-2-1", "mode": "text2image", "params": { "prompt": "x" } });
    let job: Value = ts.post("/api/jobs", body.clone()).await.json().await.unwrap();
    let id = job["id"].as_str().unwrap().to_string();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The connection breaks for longer than the give-up time.
    mock.state.reject_refresh.store(true, Ordering::SeqCst);
    mock.state.expire_access_tokens();
    tokio::time::sleep(Duration::from_millis(900)).await;
    let j = ts.json(&format!("/api/jobs/{id}")).await;
    assert!(j["status"] == "queued" || j["status"] == "running", "parked, not failed: {j}");
    assert_eq!(ts.json("/api/me").await["openart"]["needsReconnect"], true);

    // Reconnect: the job finishes.
    mock.state.reject_refresh.store(false, Ordering::SeqCst);
    mock.script(vec![completed(&mock, "h1", "w1")]);
    ts.connect_openart(&mock).await;
    let done = ts.wait_job(&id).await;
    assert_eq!(done["status"], "completed", "{done}");

    // A job that gave up (OpenArt kept saying RUNNING) is revived by the next reconnect.
    mock.script(vec![json!({ "status": "RUNNING", "historyId": "h1", "pollAfterSeconds": 0.01 })]);
    let job: Value = ts.post("/api/jobs", body).await.json().await.unwrap();
    let id = job["id"].as_str().unwrap().to_string();
    let failed = ts.wait_job(&id).await;
    assert_eq!(failed["status"], "failed");
    assert!(failed["error"].as_str().unwrap().contains("gave up"));
    assert!(failed.get("gaveUp").is_none() && failed.get("pollFrom").is_none(), "internal fields stay internal");
    mock.script(vec![completed(&mock, "h1", "w2")]);
    ts.connect_openart(&mock).await;
    let done = ts.wait_job(&id).await;
    assert_eq!(done["status"], "completed", "{done}");
    assert_eq!(done["assets"][0]["openart"]["id"], "w2");
}

/// L5: client-supplied `visualReferences` never reach OpenArt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_visual_references_are_dropped() {
    let (ts, mock) = connected().await;
    let body = json!({ "model": "nano-banana-2-1", "mode": "text2image",
        "params": { "prompt": "x", "visualReferences": [{ "type": "image", "id": "evil", "url": "http://169.254.169.254/", "label": "image1" }] } });
    assert_eq!(ts.post("/api/jobs", body).await.status(), 200);
    let generate = mock.state.calls_to("openart_generate_image");
    assert!(generate[0]["params"].get("visualReferences").is_none(), "{}", generate[0]);
}
