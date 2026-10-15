//! Owner auth: public routes, login, the rate limit, sessions, CSRF guard, body limits.

use reqwest::Method;
use reqwest::header;
use serde_json::{Value, json};

use super::{Config, PASSWORD, TestServer, file_mode};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_routes_and_session_required() {
    let ts = TestServer::start(Config::default()).await;
    let r = ts.get("/healthz").await;
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "ok");

    let me: Value = ts.json("/api/me").await;
    assert_eq!(me["authenticated"], false);
    assert_eq!(me["openart"]["connected"], false);

    for path in ["/api/assets", "/api/jobs", "/api/models", "/api/workflows", "/api/assets/abc/file", "/oauth/openart/start"] {
        let r = ts.get(path).await;
        assert_eq!(r.status(), 401, "{path}");
        let body: Value = r.json().await.unwrap();
        assert_eq!(body["code"], "unauthenticated", "{path}");
        assert!(body["error"].is_string());
    }
    let r = ts.post("/api/director", json!({ "brief": "x" })).await;
    assert_eq!(r.status(), 401);
    // Unknown API paths are JSON 404s.
    let r = ts.get("/api/nope").await;
    assert_eq!(r.status(), 404);
    assert_eq!(r.json::<Value>().await.unwrap()["code"], "not_found");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_logout_and_persisted_sessions() {
    let ts = TestServer::start(Config::default()).await;
    let r = ts.post("/api/login", json!({ "password": PASSWORD })).await;
    assert_eq!(r.status(), 200);
    let cookie = r.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap().to_string();
    assert!(cookie.starts_with("ps_session="), "{cookie}");
    for attr in ["HttpOnly", "SameSite=Strict", "Path=/", "Max-Age=2592000"] {
        assert!(cookie.contains(attr), "{cookie} lacks {attr}");
    }
    assert!(!cookie.contains("Secure"), "plain http public URL: no Secure flag");
    let me: Value = r.json().await.unwrap();
    assert_eq!(me["authenticated"], true);
    assert_eq!(me["claude"], json!({ "configured": false, "model": "claude-opus-5-5" }));

    ts.login().await;
    assert_eq!(ts.json("/api/me").await["authenticated"], true);
    assert_eq!(ts.get("/api/assets").await.status(), 200);
    let sessions = ts.data.join("secrets/sessions.json");
    assert_eq!(file_mode(&sessions), 0o600);
    assert_eq!(file_mode(&ts.data.join("secrets")), 0o700);

    let r = ts.post("/api/logout", json!({})).await;
    assert_eq!(r.status(), 200);
    assert!(r.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap().contains("Max-Age=0"));
    assert_eq!(ts.get("/api/assets").await.status(), 401, "the session is gone");
    assert_eq!(ts.json("/api/me").await["authenticated"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_rate_limit() {
    let ts = TestServer::start(Config::default()).await;
    for i in 0..5 {
        let r = ts.post("/api/login", json!({ "password": format!("wrong {i}") })).await;
        assert_eq!(r.status(), 401);
        assert_eq!(r.json::<Value>().await.unwrap()["code"], "unauthenticated");
    }
    let r = ts.post("/api/login", json!({ "password": "wrong again" })).await;
    assert_eq!(r.status(), 429);
    assert_eq!(r.json::<Value>().await.unwrap()["code"], "rate_limited");
    // Even the right password waits out the window.
    let r = ts.post("/api/login", json!({ "password": PASSWORD })).await;
    assert_eq!(r.status(), 429);
    // A missing password is just wrong, not a crash.
    let ts2 = TestServer::start(Config::default()).await;
    assert_eq!(ts2.post("/api/login", json!({})).await.status(), 401);
    assert_eq!(ts2.raw(Method::POST, "/api/login", Some("application/json"), b"{not json".to_vec()).await.status(), 400);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn csrf_guard_and_body_limits() {
    let ts = TestServer::start(Config::default()).await;
    // Login itself must be JSON (no cross-site form logins).
    let r = ts.raw(Method::POST, "/api/login", Some("application/x-www-form-urlencoded"), format!("password={PASSWORD}").into_bytes()).await;
    assert_eq!(r.status(), 415);
    ts.login().await;
    let graph = json!({ "name": "w", "graph": { "nodes": [], "edges": [] } }).to_string().into_bytes();
    for ct in [Some("text/plain"), Some("application/x-www-form-urlencoded"), Some("multipart/form-data; boundary=x"), None] {
        let r = ts.raw(Method::POST, "/api/workflows", ct, graph.clone()).await;
        assert_eq!(r.status(), 415, "{ct:?}");
        assert_eq!(r.json::<Value>().await.unwrap()["code"], "bad_request");
    }
    assert_eq!(ts.raw(Method::DELETE, "/api/workflows/abc", None, vec![]).await.status(), 415);
    assert_eq!(ts.raw(Method::POST, "/api/logout", Some("text/plain"), vec![]).await.status(), 415);
    // Uploads must be multipart.
    assert_eq!(ts.raw(Method::POST, "/api/assets", Some("application/json"), b"{}".to_vec()).await.status(), 415);
    // JSON with a charset is fine.
    let r = ts.raw(Method::POST, "/api/workflows", Some("application/json; charset=utf-8"), graph).await;
    assert_eq!(r.status(), 200);
    // Bodies over 2 MB are refused.
    let big = json!({ "name": "big", "graph": { "nodes": ["x".repeat(3 << 20)], "edges": [] } });
    let r = ts.post("/api/workflows", big).await;
    assert_eq!(r.status(), 413);
}
