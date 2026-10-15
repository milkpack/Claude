//! Workflows CRUD and the static front-end.

use reqwest::Method;
use serde_json::{Value, json};

use super::{Config, TestServer, temp_dir};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflows_crud() {
    let ts = TestServer::start(Config::default()).await;
    ts.login().await;
    assert_eq!(ts.json("/api/workflows").await, json!({ "workflows": [] }));
    let graph =
        json!({ "nodes": [{ "id": "n1", "type": "brief", "data": { "text": "hi" } }], "edges": [], "viewport": { "x": 0, "y": 0, "zoom": 1 } });
    let w: Value = ts.post("/api/workflows", json!({ "name": " Editorial ", "graph": graph })).await.json().await.unwrap();
    let id = w["id"].as_str().unwrap().to_string();
    assert_eq!(w["name"], "Editorial");
    assert_eq!(w["graph"], graph);
    assert!(w["updatedAt"].as_u64().unwrap() > 0);
    let w2: Value = ts.post("/api/workflows", json!({ "name": "Second", "graph": { "nodes": [], "edges": [] } })).await.json().await.unwrap();

    assert_eq!(ts.json(&format!("/api/workflows/{id}")).await, w);
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let r = ts.send_json(Method::PUT, &format!("/api/workflows/{id}"), &json!({ "name": "Renamed" })).await;
    assert_eq!(r.status(), 200);
    let updated: Value = r.json().await.unwrap();
    assert_eq!(updated["name"], "Renamed");
    assert_eq!(updated["graph"], graph, "the graph is kept when not sent");
    let list = ts.json("/api/workflows").await;
    assert_eq!(list["workflows"][0], json!({ "id": id, "name": "Renamed", "updatedAt": updated["updatedAt"] }));
    assert_eq!(list["workflows"][1]["id"], w2["id"]);

    for bad in
        [json!({ "name": "", "graph": { "nodes": [], "edges": [] } }), json!({ "name": "x" }), json!({ "name": "x", "graph": { "nodes": {} } })]
    {
        assert_eq!(ts.post("/api/workflows", bad.clone()).await.status(), 400, "{bad}");
    }
    assert_eq!(ts.send_json(Method::PUT, &format!("/api/workflows/{id}"), &json!({ "graph": [] })).await.status(), 400);
    assert_eq!(ts.send_json(Method::DELETE, &format!("/api/workflows/{id}"), &json!({})).await.status(), 200);
    assert_eq!(ts.get(&format!("/api/workflows/{id}")).await.status(), 404);
    assert_eq!(ts.send_json(Method::DELETE, &format!("/api/workflows/{id}"), &json!({})).await.status(), 404);
    assert_eq!(ts.send_json(Method::PUT, "/api/workflows/missing", &json!({ "name": "x" })).await.status(), 404);
    assert_eq!(ts.json("/api/workflows").await["workflows"].as_array().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn static_front_end_with_spa_fallback() {
    let dir = temp_dir("web");
    std::fs::write(dir.join("index.html"), "<!doctype html><title>Photo Studio</title>").unwrap();
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("assets/app-1234.js"), "console.log(1)").unwrap();
    std::fs::write(dir.join(".secret"), "no").unwrap();
    let ts = TestServer::start(Config { static_dir: Some(dir.clone()), ..Config::default() }).await;

    for path in ["/", "/canvas/abc", "/settings"] {
        let r = ts.get(path).await;
        assert_eq!(r.status(), 200, "{path}");
        assert_eq!(r.headers()["cache-control"], "no-cache");
        assert!(r.text().await.unwrap().contains("Photo Studio"));
    }
    let r = ts.get("/assets/app-1234.js").await;
    assert_eq!(r.status(), 200);
    assert!(r.headers()["content-type"].to_str().unwrap().starts_with("text/javascript"));
    assert!(r.headers()["cache-control"].to_str().unwrap().contains("immutable"));
    assert_eq!(ts.get("/missing.png").await.status(), 404);
    assert_eq!(ts.get("/.secret").await.status(), 404);
    assert_eq!(ts.get("/assets/missing.js").await.status(), 404);
    let r = ts.get("/api/unknown").await;
    assert_eq!(r.status(), 404);
    assert_eq!(r.json::<Value>().await.unwrap()["code"], "not_found");
    assert_eq!(ts.get("/healthz").await.text().await.unwrap(), "ok");
    drop(ts);
    let _ = std::fs::remove_dir_all(&dir);
}
