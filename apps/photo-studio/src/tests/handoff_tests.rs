//! Handoff mode: sessions on workflows, tasks, the public token routes, results.

use reqwest::Method;
use serde_json::{Value, json};

use super::{Config, TestServer, multipart, png, temp_dir};

async fn session(ts: &TestServer, name: &str) -> String {
    let w: Value = ts.post("/api/workflows", json!({ "name": name, "graph": { "nodes": [], "edges": [] } })).await.json().await.unwrap();
    w["id"].as_str().unwrap().to_string()
}

async fn task(ts: &TestServer, sid: &str, kind: &str, inputs: Value) -> Value {
    let r = ts
        .post(
            "/api/tasks",
            json!({ "sessionId": sid, "nodeId": "node-1", "kind": kind, "instructions": "Editorial portrait, warm window light", "inputs": inputs,
                    "context": { "aspectRatio": "3:2", "resolution": "4K" } }),
        )
        .await;
    assert_eq!(r.status(), 200, "{:?}", r.text().await);
    r.json().await.unwrap()
}

/// The token part of a task's return URL.
fn token(t: &Value) -> String {
    t["returnUrl"].as_str().unwrap().rsplit('/').next().unwrap().to_string()
}

fn director_answer(asset: &str) -> String {
    format!("Here is the plan.\n\n```json\n{}\n```\n", crate::director::tests::sample(asset))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sessions_on_workflows() {
    let data = temp_dir("sessions");
    std::fs::create_dir_all(data.join("workflows")).unwrap();
    // A workflow saved before sessions existed.
    let legacy = json!({ "id": "legacy1", "name": "Old", "graph": { "nodes": [], "edges": [] }, "updatedAt": 1000 });
    std::fs::write(data.join("workflows/legacy1.json"), legacy.to_string()).unwrap();
    let ts = TestServer::start(Config { data_dir: Some(data.clone()), ..Config::default() }).await;
    ts.login().await;
    let a = session(&ts, "A").await;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let b = session(&ts, "B").await;
    let list = ts.json("/api/workflows").await;
    let ids: Vec<&str> = list["workflows"].as_array().unwrap().iter().map(|w| w["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec![b.as_str(), a.as_str(), "legacy1"], "last opened first");
    assert_eq!(
        list["workflows"][2],
        json!({ "id": "legacy1", "name": "Old", "kind": "session", "createdAt": 1000, "updatedAt": 1000, "lastOpenedAt": 1000 })
    );

    // Opening A moves it to the top.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let opened = ts.json(&format!("/api/workflows/{a}")).await;
    assert!(opened["lastOpenedAt"].as_u64().unwrap() > opened["createdAt"].as_u64().unwrap());
    assert_eq!(ts.json("/api/workflows").await["workflows"][0]["id"], a.as_str());

    // Cover: set, then clear.
    let asset = ts.upload_ok(png(30, 20)).await["id"].as_str().unwrap().to_string();
    let w: Value = ts.send_json(Method::PUT, &format!("/api/workflows/{a}"), &json!({ "cover": asset })).await.json().await.unwrap();
    assert_eq!(w["cover"], asset.as_str());
    assert_eq!(ts.json("/api/workflows").await["workflows"][0]["cover"], asset.as_str());
    let w: Value = ts.send_json(Method::PUT, &format!("/api/workflows/{a}"), &json!({ "cover": null, "name": "A2" })).await.json().await.unwrap();
    assert!(w.get("cover").is_none());
    assert_eq!(ts.send_json(Method::PUT, &format!("/api/workflows/{a}"), &json!({ "cover": 5 })).await.status(), 400);
    assert_eq!(ts.send_json(Method::PUT, &format!("/api/workflows/{a}"), &json!({ "kind": "Bad Kind" })).await.status(), 400);

    // Duplicate.
    let d: Value = ts.post(&format!("/api/workflows/{a}/duplicate"), json!({})).await.json().await.unwrap();
    assert_eq!(d["name"], "A2 copy");
    assert_ne!(d["id"], a.as_str());
    assert_eq!(d["graph"], json!({ "nodes": [], "edges": [] }));
    assert_eq!(d["kind"], "session");
    let d2: Value = ts.post(&format!("/api/workflows/{b}/duplicate"), json!({ "name": "Named" })).await.json().await.unwrap();
    assert_eq!(d2["name"], "Named");
    assert_eq!(ts.json("/api/workflows").await["workflows"][0]["id"], d2["id"]);
    assert_eq!(ts.post("/api/workflows/missing/duplicate", json!({})).await.status(), 404);
    assert_eq!(ts.json("/api/workflows").await["workflows"].as_array().unwrap().len(), 5);
    drop(ts);
    let _ = std::fs::remove_dir_all(&data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_creation_and_prompt() {
    let ts = TestServer::start(Config::default()).await;
    ts.login().await;
    let sid = session(&ts, "S").await;
    let asset = ts.upload_ok(png(64, 48)).await["id"].as_str().unwrap().to_string();
    let t = task(&ts, &sid, "director", json!([{ "assetId": asset, "role": "identity" }])).await;
    assert_eq!(t["status"], "open");
    assert_eq!(t["kind"], "director");
    assert_eq!(t["sessionId"], sid.as_str());
    assert_eq!(t["nodeId"], "node-1");
    assert_eq!(t["inputs"], json!([asset]));
    assert!(t["title"].as_str().unwrap().starts_with("Director: Editorial portrait"));
    let tok = token(&t);
    assert_eq!(tok.len(), 43, "256-bit token");
    let link = format!("{}/t/{tok}/inputs/{asset}", ts.base);
    assert_eq!(t["links"], json!([{ "assetId": asset, "url": link }]));
    assert_eq!(t["returnUrl"], format!("{}/t/{tok}", ts.base));
    let exp = t["expiresAt"].as_u64().unwrap() - t["createdAt"].as_u64().unwrap();
    assert_eq!(exp, 7 * 24 * 3600 * 1000);
    let prompt = t["prompt"].as_str().unwrap();
    for needle in [
        "Editorial portrait, warm window light",
        "\"aspectRatio\": \"3:2\"",
        "role: identity",
        &format!("assetId `{asset}`"),
        &format!("curl -fsSL -o input1.png '{link}'"),
        &format!("curl -fsS -F 'text=<answer.md' '{}/t/{tok}'", ts.base),
        "type DirectorResponse",
        "```json",
        "<skill_file path=\"references/models.md\">",
    ] {
        assert!(prompt.contains(needle), "prompt lacks {needle}");
    }
    // Other kinds: their own instructions and return lines; skill only where it helps.
    let p = task(&ts, &sid, "photoshop", json!([asset])).await;
    let pp = p["prompt"].as_str().unwrap();
    assert!(pp.contains("Keep every other pixel identical") && pp.contains("-F 'files=@edited.png'") && !pp.contains("<skill_file"));
    let r = task(&ts, &sid, "review", json!([asset])).await;
    assert!(r["prompt"].as_str().unwrap().contains("type ReviewResponse"));
    let bl = task(&ts, &sid, "blender", json!([])).await;
    assert!(bl["prompt"].as_str().unwrap().contains("blender --background") && bl["prompt"].as_str().unwrap().contains("(nothing to download)"));
    let sk = task(&ts, &sid, "sketch", json!([asset])).await;
    assert!(sk["prompt"].as_str().unwrap().contains("annotated PNG"));
    task(&ts, &sid, "custom", json!([])).await;

    // Listing and filters.
    assert_eq!(ts.json(&format!("/api/tasks?sessionId={sid}")).await["tasks"].as_array().unwrap().len(), 6);
    assert_eq!(ts.json("/api/tasks?status=done").await["tasks"], json!([]));
    assert_eq!(ts.json(&format!("/api/tasks/{}", t["id"].as_str().unwrap())).await["id"], t["id"]);
    assert_eq!(ts.get("/api/tasks?status=weird").await.status(), 400);
    assert_eq!(ts.get("/api/tasks/missing").await.status(), 404);

    // Bad requests.
    for bad in [
        json!({ "sessionId": "nope", "nodeId": "n", "kind": "director" }),
        json!({ "sessionId": sid, "nodeId": "n", "kind": "painting" }),
        json!({ "sessionId": sid, "nodeId": "", "kind": "custom" }),
        json!({ "sessionId": sid, "nodeId": "n", "kind": "custom", "inputs": ["missing"] }),
        json!({ "sessionId": sid, "nodeId": "n", "kind": "custom", "context": [1] }),
    ] {
        assert_eq!(ts.post("/api/tasks", bad.clone()).await.status(), 400, "{bad}");
    }
    // Session routes need the session.
    ts.forget_cookie();
    assert_eq!(ts.get("/api/tasks").await.status(), 401);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn token_page_inputs_and_isolation() {
    let ts = TestServer::start(Config::default()).await;
    ts.login().await;
    let sid = session(&ts, "S").await;
    let bytes = png(64, 48);
    let mine = ts.upload_ok(bytes.clone()).await["id"].as_str().unwrap().to_string();
    let other = ts.upload_ok(png(10, 10)).await["id"].as_str().unwrap().to_string();
    let t = task(&ts, &sid, "photoshop", json!([{ "assetId": mine, "role": "source" }])).await;
    let t2 = task(&ts, &sid, "custom", json!([other])).await;
    let tok = token(&t);
    ts.forget_cookie(); // the token alone authorises

    let r = ts.get(&format!("/t/{tok}")).await;
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["cache-control"], "no-store");
    assert_eq!(r.headers()["referrer-policy"], "no-referrer");
    assert!(r.headers()["content-security-policy"].to_str().unwrap().contains("default-src 'none'"));
    let html = r.text().await.unwrap();
    assert!(html.contains("<form") && html.contains(&format!("action=\"/t/{tok}/result\"")) && html.contains("multipart/form-data"));
    assert!(html.contains(&format!("/t/{tok}/inputs/{mine}?thumb=1")) && html.contains("image 1 · source"));
    assert!(html.contains("Keep every other pixel identical"), "the task text is on the page");
    assert!(!html.contains("<script"), "no JavaScript");

    let f = ts.get(&format!("/t/{tok}/inputs/{mine}")).await;
    assert_eq!(f.status(), 200);
    assert_eq!(f.headers()["content-type"], "image/png");
    assert_eq!(f.bytes().await.unwrap().to_vec(), bytes);
    let th = ts.get(&format!("/t/{tok}/inputs/{mine}?thumb=1")).await;
    assert_eq!(th.headers()["content-type"], "image/jpeg");
    // Another task's input, or any other asset, isn't reachable through this token.
    assert_eq!(ts.get(&format!("/t/{tok}/inputs/{other}")).await.status(), 404);
    assert_eq!(ts.get(&format!("/t/{}/inputs/{other}", token(&t2))).await.status(), 200);
    assert_eq!(ts.get(&format!("/t/{}/inputs/{mine}", token(&t2))).await.status(), 404);
    // The session cookie is no authority on /t/*.
    ts.login().await;
    let forged = format!("{}A", tok.get(..42).unwrap());
    assert_eq!(ts.get(&format!("/t/{forged}")).await.status(), 404);
    assert_eq!(ts.get(&format!("/t/{forged}/inputs/{mine}")).await.status(), 404);
    assert_eq!(ts.get("/t/short").await.status(), 404);
    // Too many wrong tokens: the client is shut out for a while, even with a good token.
    for i in 0..10 {
        ts.get(&format!("/t/wrong{i}")).await;
    }
    assert_eq!(ts.get(&format!("/t/{tok}")).await.status(), 429);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn results_through_the_token() {
    let ts = TestServer::start(Config::default()).await;
    ts.login().await;
    let sid = session(&ts, "S").await;
    let asset = ts.upload_ok(png(64, 48)).await["id"].as_str().unwrap().to_string();
    let t = task(&ts, &sid, "director", json!([asset])).await;
    let id = t["id"].as_str().unwrap().to_string();
    let tok = token(&t);
    let before = ts.json("/api/assets").await["assets"].as_array().unwrap().len();

    // Like `curl -F text=<answer.md -F files=@a.png -F files=@scene.blend URL`, without a cookie.
    ts.forget_cookie();
    let render = png(40, 30);
    let answer = director_answer(&asset);
    let (ct, body) =
        multipart(&[("text", None, answer.as_bytes()), ("files", Some("a.png"), &render), ("files", Some("../scene.blend"), b"BLENDER-v300")]);
    let r = ts.raw(Method::POST, &format!("/t/{tok}"), Some(&ct), body).await;
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<Value>().await.unwrap(), json!({ "ok": true, "status": "done", "images": 1, "files": 1, "json": true, "error": null }));

    ts.login().await;
    let got = ts.json(&format!("/api/tasks/{id}")).await;
    assert_eq!(got["status"], "done");
    assert_eq!(got["result"]["json"], crate::director::tests::sample(&asset));
    assert_eq!(got["result"]["text"], answer.as_str());
    assert_eq!(got["result"]["assets"][0]["taskId"], id.as_str());
    let file = &got["result"]["files"][0];
    assert!(file["name"].as_str().unwrap().ends_with("-scene.blend"));
    let dl = ts.get(file["url"].as_str().unwrap()).await;
    assert_eq!(dl.status(), 200);
    assert_eq!(dl.bytes().await.unwrap().to_vec(), b"BLENDER-v300");
    assert_eq!(ts.json("/api/assets").await["assets"].as_array().unwrap().len(), before + 1);

    // An invalid director answer: the text is kept, the JSON refused with a reason.
    let (ct, body) = multipart(&[("text", None, b"```json\n{\"category\": \"portrait\"}\n```")]);
    let r: Value = ts.raw(Method::POST, &format!("/t/{tok}/result"), Some(&ct), body).await.json().await.unwrap();
    assert_eq!(r["json"], false);
    assert!(r["error"].as_str().unwrap().contains("DirectorResponse"), "{r}");
    let got = ts.json(&format!("/api/tasks/{id}")).await;
    assert!(got["result"].get("json").is_none());
    assert!(got["result"]["text"].as_str().unwrap().contains("portrait"));
    assert_eq!(got["result"]["assets"].as_array().unwrap().len(), 1, "earlier files are kept");
    let (ct, body) = multipart(&[("text", None, b"no json here")]);
    let r: Value = ts.raw(Method::POST, &format!("/t/{tok}/result"), Some(&ct), body).await.json().await.unwrap();
    assert!(r["error"].as_str().unwrap().contains("no ```json block"));

    // A browser form post gets an HTML page back.
    let (ct, body) = multipart(&[("text", None, answer.as_bytes()), ("files", Some(""), b"")]);
    let r = ts.with_headers(Method::POST, &format!("/t/{tok}/result"), &[("content-type", &ct), ("accept", "text/html,*/*")], body).await;
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("the result was received"));

    // Review through the session route with a JSON body.
    let rv = task(&ts, &sid, "review", json!([asset])).await;
    let rid = rv["id"].as_str().unwrap();
    let text = format!("```json\n{}\n```", crate::review::tests::sample());
    let got: Value = ts.post(&format!("/api/tasks/{rid}/result"), json!({ "text": text })).await.json().await.unwrap();
    assert_eq!(got["result"]["json"], crate::review::tests::sample());
    let got: Value = ts.post(&format!("/api/tasks/{rid}/result"), json!({ "json": { "verdict": "accept" } })).await.json().await.unwrap();
    assert!(got["result"]["error"].as_str().unwrap().contains("ReviewResponse"));
    // Session route also takes multipart; anything else is refused.
    let (ct, body) = multipart(&[("text", None, b"hello")]);
    let c = ts.json(&format!("/api/tasks/{rid}")).await;
    assert_eq!(c["status"], "done");
    assert_eq!(ts.raw(Method::POST, &format!("/api/tasks/{rid}/result"), Some(&ct), body).await.status(), 200);
    assert_eq!(ts.raw(Method::POST, &format!("/api/tasks/{rid}/result"), Some("text/plain"), b"x".to_vec()).await.status(), 415);
    assert_eq!(ts.post(&format!("/api/tasks/{rid}/result"), json!({})).await.status(), 400, "nothing returned");

    // Custom tasks keep any fenced JSON as is, and plain text without an error.
    let cu = task(&ts, &sid, "custom", json!([])).await;
    let cid = cu["id"].as_str().unwrap();
    let got: Value = ts.post(&format!("/api/tasks/{cid}/result"), json!({ "text": "ok\n```json\n{\"any\": [1]}\n```" })).await.json().await.unwrap();
    assert_eq!(got["result"]["json"], json!({ "any": [1] }));
    let got: Value = ts.post(&format!("/api/tasks/{cid}/result"), json!({ "text": "just words" })).await.json().await.unwrap();
    assert!(got["result"].get("error").is_none() && got["result"].get("json").is_none());

    // Cancelled: no more results.
    let c: Value = ts.post(&format!("/api/tasks/{cid}/cancel"), json!({})).await.json().await.unwrap();
    assert_eq!(c["status"], "cancelled");
    let (ct, body) = multipart(&[("text", None, b"late")]);
    assert_eq!(ts.raw(Method::POST, &format!("/t/{}", token(&cu)), Some(&ct), body).await.status(), 410);
    assert_eq!(ts.post(&format!("/api/tasks/{cid}/result"), json!({ "text": "late" })).await.status(), 410);
    assert!(ts.get(&format!("/t/{}", token(&cu))).await.text().await.unwrap().contains("cancelled"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversize_results_are_refused() {
    let ts = TestServer::start(Config::default()).await;
    ts.login().await;
    let sid = session(&ts, "S").await;
    let t = task(&ts, &sid, "blender", json!([])).await;
    let tok = token(&t);
    let before = ts.json("/api/assets").await["assets"].as_array().unwrap().len();
    // A file over 30 MB: 413, and the image sent before it is removed again.
    let mut big = png(8, 8);
    big.resize((30 << 20) + 1024, 0);
    let small = png(8, 8);
    let (ct, body) = multipart(&[("files", Some("ok.png"), &small), ("files", Some("big.png"), &big)]);
    assert_eq!(ts.raw(Method::POST, &format!("/t/{tok}/result"), Some(&ct), body).await.status(), 413);
    // 21 files: 413.
    let parts: Vec<(&str, Option<&str>, &[u8])> = (0..21).map(|_| ("files", Some("r.png"), small.as_slice())).collect();
    let (ct, body) = multipart(&parts);
    assert_eq!(ts.raw(Method::POST, &format!("/t/{tok}/result"), Some(&ct), body).await.status(), 413);
    assert_eq!(ts.json("/api/assets").await["assets"].as_array().unwrap().len(), before, "nothing was kept");
    assert_eq!(ts.json(&format!("/api/tasks/{}", t["id"].as_str().unwrap())).await["status"], "open");
    // 20 is fine.
    let parts: Vec<(&str, Option<&str>, &[u8])> = (0..20).map(|_| ("files", Some("r.png"), small.as_slice())).collect();
    let (ct, body) = multipart(&parts);
    let r: Value = ts.raw(Method::POST, &format!("/t/{tok}/result"), Some(&ct), body).await.json().await.unwrap();
    assert_eq!(r["images"], 20);
    // Not multipart or JSON.
    assert_eq!(ts.raw(Method::POST, &format!("/t/{tok}"), Some("text/plain"), b"x".to_vec()).await.status(), 415);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tokens_expire_after_seven_days() {
    let data = temp_dir("expiry");
    let (id, tok, asset) = {
        let mut ts = TestServer::start(Config { data_dir: Some(data.clone()), ..Config::default() }).await;
        ts.login().await;
        let sid = session(&ts, "S").await;
        let asset = ts.upload_ok(png(8, 8)).await["id"].as_str().unwrap().to_string();
        let t = task(&ts, &sid, "custom", json!([asset])).await;
        assert_eq!(ts.get(&format!("/t/{}", token(&t))).await.status(), 200);
        ts.stop();
        (t["id"].as_str().unwrap().to_string(), token(&t), asset)
    };
    // Eight days later.
    let path = data.join(format!("tasks/{id}.json"));
    let mut stored: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let eight_days = 8 * 24 * 3600 * 1000;
    stored["createdAt"] = json!(stored["createdAt"].as_u64().unwrap() - eight_days);
    stored["expiresAt"] = json!(stored["expiresAt"].as_u64().unwrap() - eight_days);
    std::fs::write(&path, stored.to_string()).unwrap();
    #[cfg(unix)]
    assert_eq!(super::file_mode(&path), 0o600, "task files hold the token: owner-only");
    let ts = TestServer::start(Config { data_dir: Some(data.clone()), ..Config::default() }).await;
    assert_eq!(ts.get(&format!("/t/{tok}")).await.status(), 410);
    assert_eq!(ts.get(&format!("/t/{tok}/inputs/{asset}")).await.status(), 410);
    let (ct, body) = multipart(&[("text", None, b"late")]);
    assert_eq!(ts.raw(Method::POST, &format!("/t/{tok}/result"), Some(&ct), body).await.status(), 410);
    ts.login().await;
    assert_eq!(ts.json(&format!("/api/tasks/{id}")).await["status"], "open", "the task itself is still there");
    drop(ts);
    let _ = std::fs::remove_dir_all(&data);
}
