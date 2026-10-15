//! Uploads: stored, thumbnailed, pushed to OpenArt (upload_sign → PUT → upload_metadata_get);
//! rejected when not an image or too large; kept locally when OpenArt isn't connected.

use reqwest::Method;
use serde_json::{Value, json};

use super::{Config, TestServer, connected, dims, multipart, png};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_is_pushed_to_openart() {
    let (ts, mock) = connected().await;
    let bytes = png(640, 480);
    let r = ts.upload("../../evil name.png", bytes.clone()).await;
    assert_eq!(r.status(), 200);
    let asset: Value = r.json().await.unwrap();
    let id = asset["id"].as_str().unwrap();
    assert_eq!(asset["kind"], "upload");
    assert_eq!((asset["width"].as_u64(), asset["height"].as_u64()), (Some(640), Some(480)));
    assert_eq!(asset["openart"], json!({ "id": "u1", "url": format!("{}/cdn/u1.png", mock.base) }));
    assert_eq!(asset["file"], format!("/api/assets/{id}/file"));
    assert_eq!(asset["thumb"], format!("/api/assets/{id}/thumb"));

    let sign = mock.state.calls_to("openart_upload_sign");
    assert_eq!(sign.len(), 1);
    assert_eq!(sign[0]["mediaType"], "image");
    assert_eq!(sign[0]["contentType"], "image/png");
    assert_eq!(sign[0]["size"], json!(bytes.len()));
    assert_eq!(sign[0]["filename"], "evil name.png");
    {
        let puts = mock.state.puts.lock().unwrap();
        assert_eq!(puts.len(), 1);
        assert_eq!(puts[0].1, bytes);
        assert_eq!(puts[0].2, "image/png");
    }
    let meta = mock.state.calls_to("openart_upload_metadata_get");
    assert_eq!(meta[0]["mediaUrl"], format!("{}/cdn/u1.png", mock.base));
    assert_eq!(meta[0]["uploadId"], "u1");

    assert_eq!(ts.json(&format!("/api/assets/{id}")).await, asset);
    let file = ts.get(&format!("/api/assets/{id}/file")).await;
    assert_eq!(file.headers()["x-content-type-options"], "nosniff");
    assert_eq!(file.bytes().await.unwrap().to_vec(), bytes);
    assert_eq!(dims(&ts.get(&format!("/api/assets/{id}/thumb")).await.bytes().await.unwrap()), (512, 384));
    assert!(ts.data.join(format!("assets/{id}.png")).is_file());
    assert!(ts.data.join(format!("assets/{id}.thumb.jpg")).is_file());
    assert!(ts.data.join(format!("assets/{id}.json")).is_file());

    let r = ts.send_json(Method::DELETE, &format!("/api/assets/{id}"), &json!({})).await;
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<Value>().await.unwrap(), json!({}));
    assert_eq!(ts.get(&format!("/api/assets/{id}")).await.status(), 404);
    assert_eq!(ts.get(&format!("/api/assets/{id}/file")).await.status(), 404);
    assert!(!ts.data.join(format!("assets/{id}.png")).exists());
    assert_eq!(ts.send_json(Method::DELETE, &format!("/api/assets/{id}"), &json!({})).await.status(), 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_uploads_and_offline_uploads() {
    let ts = TestServer::start(Config::default()).await;
    ts.login().await;
    // Not an image.
    let r = ts.upload("notes.txt", b"hello".to_vec()).await;
    assert_eq!(r.status(), 415);
    // A PNG header with garbage after it.
    let mut broken = png(32, 32);
    broken.truncate(60);
    assert_eq!(ts.upload("broken.png", broken).await.status(), 400);
    // No file field.
    let (ct, body) = multipart(&[("other", None, b"x")]);
    assert_eq!(ts.raw(Method::POST, "/api/assets", Some(&ct), body).await.status(), 400);
    // Over 30 MB, within the request limit: a clean 413.
    let mut big = png(8, 8);
    big.resize((30 << 20) + (64 << 10), 0);
    assert_eq!(ts.upload("big.png", big).await.status(), 413);
    // Over the request limit: 413, or the connection is cut before the body is read.
    let mut huge = png(8, 8);
    huge.resize(40 << 20, 0);
    if let Some(r) = ts.try_upload("huge.png", huge).await {
        assert_eq!(r.status(), 413);
    }
    assert!(ts.json("/api/assets").await["assets"].as_array().unwrap().is_empty(), "nothing was stored");
    // Not connected to OpenArt: stored locally, pushed when a job first uses it.
    let asset = ts.upload_ok(png(20, 10)).await;
    assert!(asset.get("openart").is_none());
    assert_eq!(ts.json("/api/assets").await["assets"].as_array().unwrap().len(), 1);
    assert_eq!(ts.get("/api/assets?limit=abc").await.status(), 400);
    assert_eq!(ts.get("/api/assets?cursor=zzz").await.status(), 400);
}
