//! End-to-end tests: the real server on 127.0.0.1:0 against in-process mocks of OpenArt and the
//! Anthropic API.

mod auth_tests;
mod claude_tests;
mod jobs_tests;
mod misc_tests;
mod mock;
mod openart_tests;
mod regression_tests;
mod upload_tests;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use reqwest::header;
use serde_json::{Value, json};

use crate::jobs::JobTiming;
use crate::server::{self, Options};
use crate::skill::Skill;
use crate::store;

pub use mock::{MockClaude, MockOpenArt};

pub const PASSWORD: &str = "test password 123";

#[derive(Default)]
pub struct Config {
    pub openart_mcp_url: Option<String>,
    pub claude_url: Option<String>,
    pub static_dir: Option<PathBuf>,
    pub skill: Option<Skill>,
    pub data_dir: Option<PathBuf>,
    pub give_up: Option<Duration>,
}

pub struct TestServer {
    pub base: String,
    pub data: PathBuf,
    client: reqwest::Client,
    cookie: Mutex<Option<String>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    owns_data: bool,
}

pub fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ps-test-{tag}-{}", store::new_id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

impl TestServer {
    pub async fn start(cfg: Config) -> TestServer {
        let owns_data = cfg.data_dir.is_none();
        let data = cfg.data_dir.unwrap_or_else(|| temp_dir("data"));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let opts = Options {
            data_dir: data.clone(),
            static_dir: cfg.static_dir.map(|d| std::fs::canonicalize(d).unwrap()),
            public_url: base.clone(),
            password: PASSWORD.into(),
            // Port 9 (discard) refuses connections: "OpenArt unreachable" unless a mock is given.
            openart_mcp_url: cfg.openart_mcp_url.unwrap_or_else(|| "http://127.0.0.1:9/mcp".into()),
            openart_issuer: None,
            anthropic_base_url: cfg.claude_url.clone().unwrap_or_else(|| "http://127.0.0.1:9".into()),
            anthropic_api_key: cfg.claude_url.map(|_| "test-key".into()),
            claude_model: "claude-opus-5-5".into(),
            claude_effort: "medium".into(),
            skill: cfg.skill.unwrap_or_else(Skill::embedded),
            trust_proxy: false,
            timing: JobTiming {
                first_poll: Duration::from_millis(10),
                min_poll: Duration::from_millis(10),
                max_poll: Duration::from_millis(50),
                give_up: cfg.give_up.unwrap_or(Duration::from_secs(60)),
            },
            allow_private_urls: true,
        };
        let a = server::build(opts).unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            server::serve(listener, a, async {
                let _ = rx.await;
            })
            .await
            .unwrap();
        });
        let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).no_proxy().build().unwrap();
        TestServer { base, data, client, cookie: Mutex::new(None), shutdown: Some(tx), owns_data }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn with_cookie(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.cookie.lock().unwrap().clone() {
            Some(c) => rb.header(header::COOKIE, format!("ps_session={c}")),
            None => rb,
        }
    }

    pub async fn get(&self, path: &str) -> reqwest::Response {
        self.with_cookie(self.client.get(self.url(path))).send().await.unwrap()
    }

    pub async fn send_json(&self, method: reqwest::Method, path: &str, body: &Value) -> reqwest::Response {
        self.with_cookie(self.client.request(method, self.url(path)))
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await
            .unwrap()
    }

    /// POST that gives up (drops the request) after `timeout`, like a closed tab or a proxy timeout.
    pub async fn post_abandoned(&self, path: &str, body: Value, timeout: Duration) -> Result<reqwest::Response, reqwest::Error> {
        self.with_cookie(self.client.post(self.url(path)))
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .timeout(timeout)
            .send()
            .await
    }

    pub fn cookie(&self) -> Option<String> {
        self.cookie.lock().unwrap().clone()
    }

    pub fn set_cookie(&self, c: Option<String>) {
        *self.cookie.lock().unwrap() = c;
    }

    pub async fn post(&self, path: &str, body: Value) -> reqwest::Response {
        self.send_json(reqwest::Method::POST, path, &body).await
    }

    pub async fn raw(&self, method: reqwest::Method, path: &str, content_type: Option<&str>, body: Vec<u8>) -> reqwest::Response {
        let mut rb = self.with_cookie(self.client.request(method, self.url(path))).body(body);
        if let Some(ct) = content_type {
            rb = rb.header(header::CONTENT_TYPE, ct);
        }
        rb.send().await.unwrap()
    }

    /// Like [`TestServer::upload`], but the server may also cut the connection (a request over the
    /// body limit is refused before it is read): `None` then.
    pub async fn try_upload(&self, file_name: &str, bytes: Vec<u8>) -> Option<reqwest::Response> {
        let (ct, body) = multipart(&[("file", Some(file_name), &bytes)]);
        self.with_cookie(self.client.post(self.url("/api/assets"))).header(header::CONTENT_TYPE, ct).body(body).send().await.ok()
    }

    pub async fn upload(&self, file_name: &str, bytes: Vec<u8>) -> reqwest::Response {
        let (ct, body) = multipart(&[("note", None, b"ignored"), ("file", Some(file_name), &bytes)]);
        self.raw(reqwest::Method::POST, "/api/assets", Some(&ct), body).await
    }

    /// Upload and return the asset JSON.
    pub async fn upload_ok(&self, bytes: Vec<u8>) -> Value {
        let r = self.upload("photo.png", bytes).await;
        assert_eq!(r.status(), 200, "{:?}", r.text().await);
        r.json().await.unwrap()
    }

    pub async fn login(&self) {
        let r = self.post("/api/login", json!({ "password": PASSWORD })).await;
        assert_eq!(r.status(), 200);
        let set = r.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap().to_string();
        let token = set.split(';').next().unwrap().strip_prefix("ps_session=").unwrap().to_string();
        *self.cookie.lock().unwrap() = Some(token);
    }

    pub fn forget_cookie(&self) {
        *self.cookie.lock().unwrap() = None;
    }

    /// Play the browser through the OpenArt OAuth flow (the mock's consent screen approves).
    pub async fn connect_openart(&self, mock: &MockOpenArt) {
        let r = self.get("/oauth/openart/start").await;
        assert_eq!(r.status(), 303);
        let location = r.headers().get(header::LOCATION).unwrap().to_str().unwrap().to_string();
        let url = reqwest::Url::parse(&location).unwrap();
        assert_eq!(url.path(), "/authorize");
        let q: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();
        mock.state.codes.lock().unwrap().insert("code-1".into(), q["code_challenge"].clone());
        let r = self.get(&format!("/oauth/openart/callback?code=code-1&state={}", q["state"])).await;
        assert_eq!(r.status(), 303);
        assert_eq!(r.headers().get(header::LOCATION).unwrap(), "/?openart=connected");
    }

    pub async fn json(&self, path: &str) -> Value {
        let r = self.get(path).await;
        assert_eq!(r.status(), 200, "GET {path}");
        r.json().await.unwrap()
    }

    /// Wait until `GET /api/jobs/:id` reports a finished job.
    pub async fn wait_job(&self, id: &str) -> Value {
        for _ in 0..500 {
            let j = self.json(&format!("/api/jobs/{id}")).await;
            if j["status"] == "completed" || j["status"] == "failed" {
                return j;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("job {id} didn't finish");
    }

    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop();
        if self.owns_data {
            let _ = std::fs::remove_dir_all(&self.data);
        }
    }
}

/// A running server connected to a mock OpenArt.
pub async fn connected() -> (TestServer, MockOpenArt) {
    let mock = MockOpenArt::start().await;
    let ts = TestServer::start(Config { openart_mcp_url: Some(mock.mcp_url()), ..Config::default() }).await;
    ts.login().await;
    ts.connect_openart(&mock).await;
    (ts, mock)
}

/// A multipart/form-data body: (content type, bytes). Fields are (name, file name, data).
pub fn multipart(fields: &[(&str, Option<&str>, &[u8])]) -> (String, Vec<u8>) {
    let boundary = format!("----ps{}", store::new_id());
    let mut body = Vec::new();
    for (name, file, data) in fields {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match file {
            Some(f) => body.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\nContent-Type: application/octet-stream\r\n\r\n")
                    .as_bytes(),
            ),
            None => body.extend_from_slice(format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes()),
        }
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

pub fn file_mode(path: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        0o600
    }
}

pub use crate::imaging::tests::{dims, png};
