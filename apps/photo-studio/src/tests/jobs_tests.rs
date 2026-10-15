//! Generation jobs: submit → PENDING → RUNNING → COMPLETED → outputs downloaded and thumbnailed;
//! failures; unaffordable jobs; resuming after a restart.

use std::sync::atomic::Ordering;

use serde_json::{Value, json};

use super::{Config, MockOpenArt, TestServer, connected, dims, png, temp_dir};

fn completed(mock: &MockOpenArt, history: &str, names: &[&str]) -> Value {
    let resources: Vec<Value> = names
        .iter()
        .map(|n| {
            let bytes = png(800, 600);
            let url = mock.add_image(&format!("{n}.png"), bytes);
            json!({ "id": n, "mediaType": "image", "url": url, "status": "completed", "aspectRatio": "4:3",
                    "thumbnailUrl": format!("{}/cdn/{n}.webp", mock.base), "metadata": {"width": 800, "height": 600, "format": "png"} })
        })
        .collect();
    json!({ "status": "COMPLETED", "historyId": history, "capabilityId": "nano-banana-2-1:image2image", "resources": resources })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn job_lifecycle_with_reference() {
    let (ts, mock) = connected().await;
    let reference = ts.upload_ok(png(64, 48)).await;
    assert_eq!(reference["openart"]["id"], "u1");

    mock.script(vec![
        json!({ "status": "PENDING", "historyId": "h1", "pollAfterSeconds": 0.01 }),
        json!({ "status": "RUNNING", "historyId": "h1", "pollAfterSeconds": 0.01 }),
        completed(&mock, "h1", &["r1", "r2"]),
    ]);
    let r = ts
        .post(
            "/api/jobs",
            json!({
                "model": "nano-banana-2-1", "mode": "image2image",
                "params": { "prompt": "the woman in image 1 on a rooftop", "imageCount": 2, "visualReferences": "client value is replaced" },
                "references": [{ "assetId": reference["id"] }]
            }),
        )
        .await;
    assert_eq!(r.status(), 200);
    let job: Value = r.json().await.unwrap();
    assert_eq!(job["historyId"], "h1");
    assert_eq!(job["credits"], json!(64.0));
    assert!(job["status"] == "queued" || job["status"] == "running");
    assert_eq!(job["references"], json!([{ "assetId": reference["id"], "label": "image1" }]));
    assert_eq!(job["assets"], json!([]));

    // The server set visualReferences from the asset's OpenArt reference, quoted first, then submitted.
    let generate = mock.state.calls_to("openart_generate_image");
    assert_eq!(generate.len(), 1);
    assert_eq!(generate[0]["model"], "nano-banana-2-1");
    assert_eq!(
        generate[0]["params"]["visualReferences"],
        json!([{ "type": "image", "id": "u1", "url": format!("{}/cdn/u1.png", mock.base), "label": "image1" }])
    );
    assert_eq!(mock.state.calls_to("openart_model_cost").len(), 1);

    let active = ts.json("/api/jobs?active=1").await;
    assert!(active["jobs"].as_array().unwrap().len() <= 1);

    let done = ts.wait_job(job["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "completed", "{done}");
    assert!(done.get("error").is_none(), "{done}");
    let assets = done["assets"].as_array().unwrap();
    assert_eq!(assets.len(), 2);
    assert!(mock.state.calls_to("openart_creation_get").len() >= 3);
    for (a, name) in assets.iter().zip(["r1", "r2"]) {
        assert_eq!(a["kind"], "generation");
        assert_eq!(a["openart"]["id"], name);
        assert_eq!(a["jobId"], job["id"]);
        assert_eq!(a["model"], "nano-banana-2-1");
        assert_eq!(a["mode"], "image2image");
        assert_eq!(a["prompt"], "the woman in image 1 on a rooftop");
        assert_eq!((a["width"].as_u64(), a["height"].as_u64()), (Some(800), Some(600)));
        let id = a["id"].as_str().unwrap();
        assert_eq!(a["file"], format!("/api/assets/{id}/file"));
        let file = ts.get(&format!("/api/assets/{id}/file")).await;
        assert_eq!(file.headers()["content-type"], "image/png");
        assert!(file.headers()["cache-control"].to_str().unwrap().contains("immutable"));
        assert_eq!(file.bytes().await.unwrap().to_vec(), mock.state.cdn.lock().unwrap()[&format!("{name}.png")]);
        let thumb = ts.get(&format!("/api/assets/{id}/thumb")).await;
        assert_eq!(thumb.headers()["content-type"], "image/jpeg");
        assert_eq!(dims(&thumb.bytes().await.unwrap()), (512, 384));
    }
    assert_eq!(ts.json("/api/jobs?active=1").await["jobs"], json!([]));
    assert_eq!(ts.json("/api/jobs").await["jobs"].as_array().unwrap().len(), 1);
    let library = ts.json("/api/assets").await;
    assert_eq!(library["assets"].as_array().unwrap().len(), 3);
    assert!(library.get("next").is_none());
    let page = ts.json("/api/assets?limit=2").await;
    assert_eq!(page["assets"].as_array().unwrap().len(), 2);
    let next = page["next"].as_str().unwrap();
    assert_eq!(ts.json(&format!("/api/assets?limit=2&cursor={next}")).await["assets"].as_array().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failures_unaffordable_and_validation() {
    let (ts, mock) = connected().await;
    mock.script(vec![json!({ "status": "FAILED", "historyId": "h1", "error": "content policy" })]);
    let job: Value =
        ts.post("/api/jobs", json!({ "model": "nano-banana-2-1", "mode": "text2image", "params": { "prompt": "x" } })).await.json().await.unwrap();
    let done = ts.wait_job(job["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "failed");
    assert!(done["error"].as_str().unwrap().contains("content policy"), "{done}");

    // Completed without images anywhere.
    mock.script(vec![json!({ "status": "COMPLETED", "historyId": "h1" })]);
    let job: Value =
        ts.post("/api/jobs", json!({ "model": "nano-banana-2-1", "mode": "text2image", "params": { "prompt": "x" } })).await.json().await.unwrap();
    let done = ts.wait_job(job["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "failed");
    assert!(!mock.state.calls_to("openart_creation_list").is_empty(), "falls back to the creation list");

    // Outputs found through the creation list.
    let url = mock.add_image("l1.png", png(40, 30));
    mock.state.creation_items.lock().unwrap().push(json!({ "id": "l1", "historyId": "h1", "mediaType": "image", "status": "completed", "url": url }));
    let job: Value =
        ts.post("/api/jobs", json!({ "model": "nano-banana-2-1", "mode": "text2image", "params": { "prompt": "x" } })).await.json().await.unwrap();
    let done = ts.wait_job(job["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "completed", "{done}");
    assert_eq!(done["assets"][0]["openart"]["id"], "l1");

    // Unaffordable: refused before submitting.
    mock.state.affordable.store(false, Ordering::SeqCst);
    let before = mock.state.calls_to("openart_generate_image").len();
    let r = ts.post("/api/jobs", json!({ "model": "nano-banana-2-1", "mode": "text2image", "params": { "prompt": "x" } })).await;
    assert_eq!(r.status(), 402);
    assert!(r.json::<Value>().await.unwrap()["error"].as_str().unwrap().contains("https://openart.ai/pricing"));
    assert_eq!(mock.state.calls_to("openart_generate_image").len(), before);
    mock.state.affordable.store(true, Ordering::SeqCst);

    for bad in [
        json!({ "model": "", "mode": "text2image" }),
        json!({ "model": "a b", "mode": "text2image" }),
        json!({ "model": "m", "mode": "text2image", "params": [1] }),
        json!({ "model": "m", "mode": "text2image", "references": [{ "assetId": "nope" }] }),
        json!({ "mode": "text2image" }),
    ] {
        let r = ts.post("/api/jobs", bad.clone()).await;
        assert_eq!(r.status(), 400, "{bad}");
    }
    assert_eq!(ts.get("/api/jobs/unknown").await.status(), 404);
    assert_eq!(ts.get("/api/jobs/..%2F..").await.status(), 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn not_connected_jobs_are_refused() {
    let ts = TestServer::start(Config::default()).await;
    ts.login().await;
    let r = ts.post("/api/jobs", json!({ "model": "nano-banana-2-1", "mode": "text2image", "params": { "prompt": "x" } })).await;
    assert_eq!(r.status(), 409);
    assert_eq!(r.json::<Value>().await.unwrap()["code"], "openart_not_connected");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unfinished_jobs_resume_after_restart() {
    let data = temp_dir("restart");
    let mock = MockOpenArt::start().await;
    {
        let mut ts = TestServer::start(Config { openart_mcp_url: Some(mock.mcp_url()), data_dir: Some(data.clone()), ..Config::default() }).await;
        ts.login().await;
        ts.connect_openart(&mock).await;
        // Keep it running while the server stops.
        mock.script(vec![json!({ "status": "RUNNING", "historyId": "h1", "pollAfterSeconds": 0.01 })]);
        let job: Value = ts
            .post("/api/jobs", json!({ "model": "nano-banana-2-1", "mode": "text2image", "params": { "prompt": "x" } }))
            .await
            .json()
            .await
            .unwrap();
        assert_eq!(job["historyId"], "h1");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        ts.stop();
    }
    // A job file left by a previous run (e.g. after a crash).
    let id = crate::store::new_id();
    let now = crate::store::now_ms();
    let stale = json!({ "id": id, "status": "running", "model": "nano-banana-2-1", "mode": "text2image", "params": {"prompt": "y"},
                        "references": [], "historyId": "h9", "assetIds": [], "createdAt": now, "updatedAt": now });
    std::fs::write(data.join(format!("jobs/{id}.json")), stale.to_string()).unwrap();
    std::fs::write(data.join("jobs/garbage.json"), "{not json").unwrap();

    mock.script(vec![completed(&mock, "h9", &["z1"])]);
    let ts = TestServer::start(Config { openart_mcp_url: Some(mock.mcp_url()), data_dir: Some(data.clone()), ..Config::default() }).await;
    ts.login().await;
    let done = ts.wait_job(&id).await;
    assert_eq!(done["status"], "completed", "{done}");
    assert_eq!(done["assets"][0]["openart"]["id"], "z1");
    // The job of the first run resumed too (the mock now answers COMPLETED for any history).
    let jobs = ts.json("/api/jobs").await;
    assert_eq!(jobs["jobs"].as_array().unwrap().len(), 2);
    for j in jobs["jobs"].as_array().unwrap() {
        let j = ts.wait_job(j["id"].as_str().unwrap()).await;
        assert_eq!(j["status"], "completed");
    }
    drop(ts);
    let _ = std::fs::remove_dir_all(&data);
}
