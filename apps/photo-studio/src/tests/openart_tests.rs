//! The OpenArt connection (OAuth discovery, registration, PKCE, refresh, reconnect, disconnect)
//! and the MCP client (JSON and SSE replies, sessions, re-initialization), plus the model, form
//! and cost proxies.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use reqwest::header;
use serde_json::{Value, json};

use super::{Config, MockOpenArt, TestServer, connected, file_mode};
use crate::skill::Skill;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oauth_connect_refresh_reconnect_disconnect() {
    let mock = MockOpenArt::start().await;
    let ts = TestServer::start(Config { openart_mcp_url: Some(mock.mcp_url()), ..Config::default() }).await;
    ts.login().await;
    assert_eq!(ts.get("/api/models").await.json::<Value>().await.unwrap()["code"], "openart_not_connected");

    // Start: discovery, registration, then a redirect with PKCE + state + resource.
    let r = ts.get("/oauth/openart/start").await;
    assert_eq!(r.status(), 303);
    let location = r.headers().get(header::LOCATION).unwrap().to_str().unwrap().to_string();
    let url = reqwest::Url::parse(&location).unwrap();
    let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
    assert!(location.starts_with(&format!("{}/authorize?", mock.base)));
    assert_eq!(q["response_type"], "code");
    assert_eq!(q["client_id"], "client-1");
    assert_eq!(q["redirect_uri"], format!("{}/oauth/openart/callback", ts.base));
    assert_eq!(q["scope"], "full_access");
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["resource"], mock.mcp_url());
    assert_eq!(q["code_challenge"].len(), 43);
    assert!(q["state"].len() >= 32);
    {
        let regs = mock.state.registrations.lock().unwrap();
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0]["token_endpoint_auth_method"], "none");
        assert_eq!(regs[0]["redirect_uris"], json!([format!("{}/oauth/openart/callback", ts.base)]));
        assert_eq!(regs[0]["grant_types"], json!(["authorization_code", "refresh_token"]));
    }

    // A forged or replayed state is refused.
    let r = ts.get("/oauth/openart/callback?code=code-1&state=forged").await;
    assert_eq!(r.headers().get(header::LOCATION).unwrap(), "/?openart=error");
    let r = ts.get("/oauth/openart/callback?error=access_denied").await;
    assert_eq!(r.headers().get(header::LOCATION).unwrap(), "/?openart=error");

    // The callback needs no session (SameSite=Strict cookies don't come back from OpenArt).
    mock.state.codes.lock().unwrap().insert("code-1".into(), q["code_challenge"].clone());
    ts.forget_cookie();
    let r = ts.get(&format!("/oauth/openart/callback?code=code-1&state={}", q["state"])).await;
    assert_eq!(r.status(), 303);
    assert_eq!(r.headers().get(header::LOCATION).unwrap(), "/?openart=connected");
    let r = ts.get(&format!("/oauth/openart/callback?code=code-1&state={}", q["state"])).await;
    assert_eq!(r.headers().get(header::LOCATION).unwrap(), "/?openart=error", "a state works once");
    {
        let forms = mock.state.token_forms.lock().unwrap();
        let f = forms.iter().find(|f| f["grant_type"] == "authorization_code").unwrap();
        assert_eq!(f["redirect_uri"], format!("{}/oauth/openart/callback", ts.base));
        assert_eq!(f["resource"], mock.mcp_url());
        assert_eq!(f["client_id"], "client-1");
        assert_eq!(f["code_verifier"].len(), 43);
    }
    ts.login().await;

    // Secrets are stored privately and never reach the browser.
    let secrets = ts.data.join("secrets/openart.json");
    assert_eq!(file_mode(&secrets), 0o600);
    let stored = std::fs::read_to_string(&secrets).unwrap();
    assert!(stored.contains("at-0") && stored.contains("rt-0") && stored.contains("client-1"));
    let me = ts.json("/api/me").await;
    assert_eq!(
        me["openart"],
        json!({ "connected": true, "needsReconnect": false, "email": "owner@example.com", "plan": "Wonder", "credits": 1000.0 })
    );
    assert!(!me.to_string().contains("at-0"));

    // Starting again reuses the registered client.
    assert_eq!(ts.get("/oauth/openart/start").await.status(), 303);
    assert_eq!(mock.state.registrations.lock().unwrap().len(), 1);

    // 401 from MCP → one refresh → retry succeeds.
    mock.state.expire_access_tokens();
    let r = ts.get("/api/models?refresh=1").await;
    assert_eq!(r.status(), 200);
    {
        let forms = mock.state.token_forms.lock().unwrap();
        let f = forms.iter().find(|f| f["grant_type"] == "refresh_token").unwrap();
        assert_eq!(f["refresh_token"], "rt-0");
        assert_eq!(f["client_id"], "client-1");
        assert_eq!(f["resource"], mock.mcp_url());
    }
    let stored = std::fs::read_to_string(&secrets).unwrap();
    assert!(stored.contains("at-1") && stored.contains("rt-0"), "the old refresh token is kept when not rotated");

    // A refused refresh marks the connection as needing a reconnect.
    mock.state.reject_refresh.store(true, Ordering::SeqCst);
    mock.state.expire_access_tokens();
    let r = ts.get("/api/models?refresh=1").await;
    assert_eq!(r.status(), 409);
    assert_eq!(r.json::<Value>().await.unwrap()["code"], "openart_reconnect");
    let me = ts.json("/api/me").await;
    assert_eq!(me["openart"], json!({ "connected": false, "needsReconnect": true }));
    assert_eq!(ts.post("/api/cost", json!({"model": "m", "mode": "text2image"})).await.json::<Value>().await.unwrap()["code"], "openart_reconnect");

    // Reconnecting clears the flag; disconnecting revokes and forgets the tokens.
    mock.state.reject_refresh.store(false, Ordering::SeqCst);
    ts.connect_openart(&mock).await;
    assert_eq!(ts.json("/api/me").await["openart"]["connected"], true);
    let r = ts.post("/api/openart/disconnect", json!({})).await;
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<Value>().await.unwrap()["openart"], json!({ "connected": false, "needsReconnect": false }));
    assert!(!mock.state.revoked.lock().unwrap().is_empty());
    let stored = std::fs::read_to_string(&secrets).unwrap();
    assert!(!stored.contains("\"accessToken\""), "tokens are forgotten");
    assert!(stored.contains("client-1"), "the registration is kept");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_json_and_sse_replies_and_session_reinit() {
    let (ts, mock) = connected().await;
    // JSON replies; model_list comes back as resource links + a JSON text block.
    let models = ts.json("/api/models").await;
    assert_eq!(models["balance"], json!(1000.0));
    assert_eq!(models["models"][0]["id"], "nano-banana-2-1");
    assert_eq!(
        models["models"][0]["modes"]["image"][1],
        json!({"mode":"image2image","description":"Image to image","credits":32.0,"affordable":true})
    );
    assert_eq!(mock.state.initializes.load(Ordering::SeqCst), 1);
    assert_eq!(mock.state.notifications.load(Ordering::SeqCst), 1, "notifications/initialized sent");
    assert!(mock.state.protocol_headers.lock().unwrap().iter().all(|p| p == "2025-06-18"));

    // Cached: no second call.
    ts.json("/api/models").await;
    assert_eq!(mock.state.calls_to("openart_model_list").len(), 1);

    // SSE replies (with a notification before the response).
    mock.state.sse.store(true, Ordering::SeqCst);
    let models = ts.json("/api/models?refresh=1").await;
    assert_eq!(models["models"].as_array().unwrap().len(), 3);
    assert_eq!(mock.state.calls_to("openart_model_list").len(), 2);
    let form = ts.json("/api/models/nano-banana-2-1/text2image/form").await;
    assert_eq!(form["schema"]["required"], json!(["prompt"]));
    ts.json("/api/models/nano-banana-2-1/text2image/form").await;
    assert_eq!(mock.state.calls_to("openart_model_form_get").len(), 1, "forms are cached");
    let r = ts.post("/api/cost", json!({ "model": "nano-banana-2-1", "mode": "text2image", "params": {"resolution": "4K", "imageCount": 2} })).await;
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<Value>().await.unwrap(), json!({ "credits": 64.0, "config": {"resolution": "4K", "imageCount": 2}, "affordable": true }));
    assert_eq!(ts.post("/api/cost", json!({ "model": "../x", "mode": "a" })).await.status(), 400);
    assert_eq!(ts.post("/api/cost", json!({ "model": "m", "mode": "a", "params": [] })).await.status(), 400);

    // The server forgets the session → 404 → re-initialize and retry.
    mock.state.sessions.lock().unwrap().clear();
    mock.state.sse.store(false, Ordering::SeqCst);
    let models = ts.json("/api/models?refresh=1").await;
    assert_eq!(models["models"].as_array().unwrap().len(), 3);
    assert_eq!(mock.state.initializes.load(Ordering::SeqCst), 2);

    // A tool error is a 502 with OpenArt's message.
    let r = ts.get("/api/models/x/y/form").await;
    assert_eq!(r.status(), 200, "the mock answers any model");
    let err = crate::mcp::parse_tool_result("openart_model_cost", &json!({"isError": true, "content": [{"type": "text", "text": "invalid params"}]}))
        .unwrap_err();
    assert_eq!((err.status.as_u16(), err.code), (502, "upstream"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn models_carry_routing_categories() {
    let mock = MockOpenArt::start().await;
    let routing = "# Routing\n\n| category | model |\n|---|---|\n| Portrait / headshot | `nano-banana-2-1` |\n| Product | `gpt-image-2` |\n";
    let skill = Skill { source: "test".into(), files: vec![("references/routing.md".into(), routing.into())] };
    let ts = TestServer::start(Config { openart_mcp_url: Some(mock.mcp_url()), skill: Some(skill), ..Config::default() }).await;
    ts.login().await;
    ts.connect_openart(&mock).await;
    let models = ts.json("/api/models").await;
    assert_eq!(models["models"][0]["categories"], json!(["portrait", "headshot"]));
    assert_eq!(models["models"][1]["categories"], json!(["product"]));
    assert_eq!(models["models"][2]["categories"], json!([]));
}
