//! End-to-end: the server on an ephemeral port, real WebSocket clients each driving a
//! `vectorcraft_collab::Connection`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use vectorcraft_collab::{Connection, PeerState, SharedDoc};
use vectorcraft_doc::appearance::Appearance;
use vectorcraft_doc::{Document, Node, NodeId};
use vectorcraft_geom::{Rect, shapes};

use crate::cli::{Cli, CliError};
use crate::hub::Limits;
use crate::server::{Options, serve};
use crate::{static_files, storage};

// ------------------------------------------------------------------ harness

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("vc-collab-server-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Server {
    addr: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<std::io::Result<()>>,
}

impl Server {
    async fn start(data: &Path) -> Server {
        Self::start_with(data, None, Limits::default()).await
    }

    async fn start_with(data: &Path, static_dir: Option<PathBuf>, limits: Limits) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = oneshot::channel::<()>();
        let opts = Options { data_dir: data.to_path_buf(), static_dir, limits };
        let task = tokio::spawn(serve(listener, opts, async move {
            let _ = rx.await;
        }));
        Server { addr, stop: Some(tx), task }
    }

    async fn stop(mut self) {
        let _ = self.stop.take().unwrap().send(());
        tokio::time::timeout(Duration::from_secs(10), self.task).await.unwrap().unwrap().unwrap();
    }

    async fn get(&self, path: &str) -> (u16, String) {
        http_get(self.addr, path).await
    }
}

async fn http_get(addr: SocketAddr, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut buf = vec![];
    s.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf).to_string();
    let status = text.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// A participant: a WebSocket, the protocol state and the document its editor shows.
struct Client {
    ws: Ws,
    conn: Connection,
    doc: Arc<Document>,
    closed: bool,
}

fn clock() -> impl Fn() -> u64 + Send + Sync + 'static {
    let start = Instant::now();
    move || start.elapsed().as_millis() as u64
}

impl Client {
    async fn connect(addr: SocketAddr, room: &str, id: u64) -> Client {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/collab/{room}")).await.unwrap();
        let shared = SharedDoc::with_client_id(id);
        let mut doc = Document::new(400.0, 300.0);
        doc.set_id_range(Some(shared.id_range()), 0);
        let mut c = Client { ws, conn: Connection::new(shared, clock()), doc: Arc::new(doc), closed: false };
        for m in c.conn.hello() {
            c.send(m).await;
        }
        c
    }

    async fn send(&mut self, m: Vec<u8>) {
        self.ws.send(WsMessage::Binary(m.into())).await.unwrap();
    }

    /// Send what's pending, then handle what arrives within `wait`.
    async fn pump(&mut self, wait: Duration) {
        for m in self.conn.outgoing() {
            self.send(m).await;
        }
        loop {
            match tokio::time::timeout(wait, self.ws.next()).await {
                Ok(Some(Ok(WsMessage::Binary(b)))) => {
                    for r in self.conn.receive(&b).unwrap() {
                        self.send(r).await;
                    }
                }
                Ok(Some(Ok(WsMessage::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => {
                    self.closed = true;
                    break;
                }
                Ok(Some(Ok(_))) => {}
                Err(_) => break,
            }
        }
        for m in self.conn.outgoing() {
            self.send(m).await;
        }
        if let Some(d) = self.conn.doc.materialize(&self.doc) {
            self.doc = d;
        }
    }

    /// Edit like a command does: copy-on-write, then publish as one undo step.
    fn edit(&mut self, f: impl FnOnce(&mut Document)) {
        let mut d = (*self.doc).clone();
        if d.id_range() != Some(self.conn.doc.id_range()) {
            d.set_id_range(Some(self.conn.doc.id_range()), 0);
        }
        f(&mut d);
        self.doc = Arc::new(d);
        self.conn.doc.publish(&self.doc);
        self.conn.doc.end_step();
    }

    fn share(&mut self) {
        let d = self.doc.clone();
        self.conn.doc.publish(&d);
        self.conn.doc.end_step();
    }

    fn add_rect(&mut self, x: f64) -> NodeId {
        let mut id = NodeId(0);
        self.edit(|d| {
            let layer = d.layers[0].id;
            id = d.alloc_id();
            let node = Node::path(id, shapes::rectangle(Rect::new(x, 0.0, x + 10.0, 10.0)), Appearance::default_art());
            d.insert(Some(layer), usize::MAX, node).unwrap();
        });
        id
    }

    fn presence(&mut self, name: &str) {
        let color = self.conn.color();
        self.conn.set_presence(&PeerState { name: name.to_string(), color, ..PeerState::default() });
    }

    fn peer_names(&self) -> Vec<String> {
        self.conn.peers().into_iter().map(|(_, s)| s.name).collect()
    }
}

/// Pump everyone until `cond` holds (or fail after a while).
async fn until(clients: &mut [&mut Client], what: &str, cond: impl Fn(&[&mut Client]) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        for c in clients.iter_mut() {
            c.pump(Duration::from_millis(30)).await;
        }
        if cond(clients) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
    }
}

/// The document as data, without what differs by design between replicas (the id counter).
fn canon(d: &Document) -> Value {
    let mut v = serde_json::to_value(d).unwrap();
    v.as_object_mut().unwrap().remove("next_id");
    v
}

// ------------------------------------------------------------------ tests

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn co_editing_presence_and_persistence() {
    let dir = TempDir::new("e2e");
    let server = Server::start(&dir.0).await;

    // A opens the room and shares its document.
    let mut a = Client::connect(server.addr, "design-1", 1).await;
    let ra = a.add_rect(10.0);
    a.share();
    until(&mut [&mut a], "A synced", |c| c[0].conn.is_synced()).await;

    // B joins and gets A's document.
    let mut b = Client::connect(server.addr, "design-1", 2).await;
    until(&mut [&mut a, &mut b], "B has A's document", |c| c[1].conn.is_synced() && c[1].doc.node(ra).is_some()).await;
    assert_eq!(canon(&a.doc), canon(&b.doc));

    // B's edit reaches A.
    let rb = b.add_rect(200.0);
    until(&mut [&mut a, &mut b], "A sees B's rectangle", |c| c[0].doc.node(rb).is_some()).await;
    assert_eq!(canon(&a.doc), canon(&b.doc));
    assert_ne!(ra, rb);

    // Presence travels both ways.
    a.presence("Anna");
    b.presence("Boris");
    until(&mut [&mut a, &mut b], "presence", |c| c[0].peer_names() == ["Boris"] && c[1].peer_names() == ["Anna"]).await;

    let (status, body) = server.get("/api/health").await;
    assert_eq!(status, 200);
    let health: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(health["ok"], true);
    assert_eq!(health["rooms"], 1);
    assert_eq!(health["clients"], 2);

    // A third client sees both peers as soon as it joins (the server sends current presence).
    let mut c = Client::connect(server.addr, "design-1", 3).await;
    until(&mut [&mut a, &mut b, &mut c], "C sees A and B", |cl| {
        let mut n = cl[2].peer_names();
        n.sort();
        n == ["Anna", "Boris"]
    })
    .await;
    assert_eq!(canon(&a.doc), canon(&c.doc));
    c.ws.close(None).await.unwrap();

    // A drops its connection without saying goodbye: the server removes its presence for B.
    drop(a);
    until(&mut [&mut b], "A's presence removed", |c| c[0].peer_names().is_empty()).await;

    // Debounced save: the file appears ~1 s after the last change, while B is still connected.
    let file = storage::room_path(&dir.0, "design-1");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !file.exists() {
        assert!(Instant::now() < deadline, "room was not saved");
        b.pump(Duration::from_millis(50)).await;
    }
    let (_, body) = server.get("/api/rooms").await;
    let rooms: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(rooms[0]["id"], "design-1");
    assert_eq!(rooms[0]["online"], 1);
    assert!(rooms[0]["bytes"].as_u64().unwrap() > 0);
    assert!(rooms[0]["updatedAt"].as_u64().is_some());

    // One more edit, then stop the server: shutdown saves it.
    let rb2 = b.add_rect(300.0);
    b.pump(Duration::from_millis(100)).await;
    let expected = canon(&b.doc);
    server.stop().await;
    b.pump(Duration::from_millis(100)).await;
    assert!(b.closed, "clients are disconnected on shutdown");

    // A new server on the same data serves the stored document to a fresh client.
    let server = Server::start(&dir.0).await;
    let mut d = Client::connect(server.addr, "design-1", 4).await;
    until(&mut [&mut d], "D loads the stored document", |c| c[0].conn.is_synced() && c[0].doc.node(rb2).is_some()).await;
    assert_eq!(canon(&d.doc), expected);
    assert!(d.doc.node(ra).is_some() && d.doc.node(rb).is_some());
    drop(d);

    // The last one out unloads the room.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (_, body) = server.get("/api/health").await;
        let h: Value = serde_json::from_str(&body).unwrap();
        if h["rooms"] == 0 && h["clients"] == 0 {
            break;
        }
        assert!(Instant::now() < deadline, "room not unloaded: {h}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_messages_are_ignored() {
    let dir = TempDir::new("garbage");
    let server = Server::start(&dir.0).await;
    let mut a = Client::connect(server.addr, "room_2", 1).await;
    let mut b = Client::connect(server.addr, "room_2", 2).await;
    a.share();
    until(&mut [&mut a, &mut b], "joined", |c| c[0].conn.is_synced() && c[1].conn.is_synced()).await;

    // Garbage of every kind: unknown types, truncated sync messages, bad updates, bad presence,
    // text frames.
    let junk: Vec<Vec<u8>> = vec![
        vec![],
        vec![0],
        vec![0, 0],
        vec![0, 1, 200, 1, 2, 3],
        vec![0, 2, 5, 1, 2, 3, 4, 5],
        vec![0, 2, 255, 255, 255, 255, 15],
        vec![1, 3, 1, 1, 2, b'{'],
        vec![1, 255, 255, 255, 255, 15],
        vec![9, 9, 9],
        (0..=255u8).collect(),
        // Client ids beyond 53 bits (Yjs ids are 53-bit), in presence and in a state vector.
        [vec![1, 14, 1], vec![0x80; 8], vec![0x10, 1, 2, b'{', b'}']].concat(),
        [vec![0, 0, 11, 1], vec![0x80; 8], vec![0x10, 5]].concat(),
    ];
    for j in junk {
        a.send(j).await;
    }
    a.ws.send(WsMessage::Text("hello".into())).await.unwrap();

    // The room still works for everyone.
    let r = a.add_rect(42.0);
    until(&mut [&mut a, &mut b], "B sees A's edit after the garbage", |c| c[1].doc.node(r).is_some()).await;
    assert!(!a.closed && !b.closed);
    assert_eq!(canon(&a.doc), canon(&b.doc));
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_room_ids_and_limits_are_refused() {
    let dir = TempDir::new("limits");
    let limits = Limits { max_room_clients: 1, ..Limits::default() };
    let server = Server::start_with(&dir.0, None, limits).await;
    for bad in ["a.b", "..", "%2e%2e", "a%2Fb", &"x".repeat(65), "sp%20ace"] {
        let r = tokio_tungstenite::connect_async(format!("ws://{}/collab/{bad}", server.addr)).await;
        assert!(r.is_err(), "room {bad} accepted");
    }
    let (status, _) = server.get("/collab/").await;
    assert_eq!(status, 404);
    // A plain GET on a room isn't a WebSocket.
    let (status, _) = server.get("/collab/ok").await;
    assert!(status >= 400);

    // The room holds one client; the second one is closed right away.
    let mut a = Client::connect(server.addr, "solo", 1).await;
    until(&mut [&mut a], "A synced", |c| c[0].conn.is_synced()).await;
    let mut b = Client::connect(server.addr, "solo", 2).await;
    b.pump(Duration::from_millis(300)).await;
    assert!(b.closed);
    assert!(!b.conn.is_synced());
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_room_file_is_moved_aside() {
    let dir = TempDir::new("corrupt");
    std::fs::write(storage::room_path(&dir.0, "broken"), b"\xff\xff\xff\xff not a yjs update").unwrap();
    let server = Server::start(&dir.0).await;
    let mut a = Client::connect(server.addr, "broken", 1).await;
    until(&mut [&mut a], "A synced", |c| c[0].conn.is_synced()).await;
    let moved = std::fs::read_dir(&dir.0).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("broken.ydoc.corrupt-"));
    assert!(moved);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn static_files_and_spa_fallback() {
    let dir = TempDir::new("static");
    let web = dir.0.join("web");
    std::fs::create_dir_all(web.join("assets")).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html>app").unwrap();
    std::fs::write(web.join("assets/app.wasm"), b"\0asm").unwrap();
    std::fs::write(dir.0.join("secret.txt"), "secret").unwrap();
    let data = dir.0.join("data");
    let server = Server::start_with(&data, Some(std::fs::canonicalize(&web).unwrap()), Limits::default()).await;

    let (s, body) = server.get("/").await;
    assert_eq!((s, body.as_str()), (200, "<!doctype html>app"));
    let (s, body) = server.get("/assets/app.wasm").await;
    assert_eq!((s, body.as_str()), (200, "\0asm"));
    let (s, body) = server.get("/room/abc").await;
    assert_eq!((s, body.as_str()), (200, "<!doctype html>app"));
    let (s, _) = server.get("/missing.js").await;
    assert_eq!(s, 404);
    for evil in ["/../secret.txt", "/%2e%2e/secret.txt", "/assets/..%2f..%2fsecret.txt", "/..%5csecret.txt"] {
        let (_, body) = server.get(evil).await;
        assert!(!body.contains("secret"), "{evil} escaped the root");
    }
    let (s, _) = server.get("/api/nope").await;
    assert_eq!(s, 404);
    server.stop().await;
}

#[test]
fn room_ids() {
    assert!(storage::valid_room("abc_DEF-123"));
    assert!(storage::valid_room(&"a".repeat(64)));
    for bad in ["", "a b", "a/b", "..", ".x", "é", &"a".repeat(65)] {
        assert!(!storage::valid_room(bad), "{bad:?}");
    }
}

#[test]
fn static_paths() {
    assert_eq!(static_files::sanitize("/a/b.js"), Some(PathBuf::from("a/b.js")));
    assert_eq!(static_files::sanitize("/"), Some(PathBuf::new()));
    assert_eq!(static_files::sanitize("/a%20b"), Some(PathBuf::from("a b")));
    for bad in ["/../x", "/%2e%2e/x", "/a/%2E%2E/x", "/.env", "/a\\b", "/c:/x", "/%00", "/%zz", "/%2", "/%ff"] {
        assert_eq!(static_files::sanitize(bad), None, "{bad}");
    }
}

#[test]
fn command_line() {
    let none = |_: &str| None;
    let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let c = Cli::parse(args(&[]), none).unwrap();
    assert_eq!((c.host.as_str(), c.port, c.data_dir.clone(), c.static_dir.clone()), ("0.0.0.0", 1234, PathBuf::from("./collab-data"), None));
    let c = Cli::parse(args(&["--port", "99", "--host=127.0.0.1", "--data", "/d", "--static=/w", "--max-clients", "5"]), none).unwrap();
    assert_eq!(
        (c.host.as_str(), c.port, c.data_dir.clone(), c.static_dir.clone()),
        ("127.0.0.1", 99, PathBuf::from("/d"), Some(PathBuf::from("/w")))
    );
    assert_eq!(c.limits.max_clients, 5);
    let env = |k: &str| match k {
        "PORT" => Some("4000".to_string()),
        "HOST" => Some("::".to_string()),
        "DATA_DIR" => Some("/env".to_string()),
        _ => None,
    };
    let c = Cli::parse(args(&[]), env).unwrap();
    assert_eq!((c.host.as_str(), c.port, c.data_dir), ("::", 4000, PathBuf::from("/env")));
    assert_eq!(Cli::parse(args(&["--port", "8"]), env).unwrap().port, 8);
    assert_eq!(Cli::parse(args(&["--help"]), none), Err(CliError::Help));
    assert!(matches!(Cli::parse(args(&["--port", "x"]), none), Err(CliError::Bad(_))));
    assert!(matches!(Cli::parse(args(&["--port"]), none), Err(CliError::Bad(_))));
    assert!(matches!(Cli::parse(args(&["--max-rooms", "0"]), none), Err(CliError::Bad(_))));
    assert!(matches!(Cli::parse(args(&["--bogus"]), none), Err(CliError::Bad(_))));
}

#[test]
fn storage_round_trip() {
    let dir = TempDir::new("storage");
    assert_eq!(storage::load(&dir.0, "r").unwrap(), None);
    storage::save(&dir.0, "r", b"one").unwrap();
    storage::save(&dir.0, "r", b"two").unwrap();
    assert_eq!(storage::load(&dir.0, "r").unwrap().as_deref(), Some(&b"two"[..]));
    let listed = storage::list(&dir.0).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!((listed[0].id.as_str(), listed[0].bytes), ("r", 3));
    // No temporary files left behind.
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
}
