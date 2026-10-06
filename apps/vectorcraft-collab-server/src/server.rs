//! HTTP + WebSocket front end: `/collab/<room>` (WebSocket), `/api/health`, `/api/rooms`, and
//! optionally a static web app.

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code};
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{Method, StatusCode, Uri};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use log::{debug, info, warn};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use crate::hub::{Hub, Limits, PEER_QUEUE, now_ms, stopped};
use crate::{static_files, storage};

/// Ping clients this often…
const PING_EVERY: Duration = Duration::from_secs(30);
/// …and drop those silent for this long (dead network, sleeping laptop).
const IDLE_TIMEOUT_MS: u64 = 75_000;
/// After a shutdown signal, wait this long for connections to close before the final save.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub struct Options {
    pub data_dir: PathBuf,
    /// Canonicalized web app directory.
    pub static_dir: Option<PathBuf>,
    pub limits: Limits,
}

#[derive(Clone)]
struct AppState {
    hub: Arc<Hub>,
    static_dir: Option<Arc<PathBuf>>,
}

/// Serve on `listener` until `shutdown` completes, then close every connection and save every room.
pub async fn serve(listener: TcpListener, opts: Options, shutdown: impl Future<Output = ()> + Send + 'static) -> std::io::Result<()> {
    std::fs::create_dir_all(&opts.data_dir)?;
    let hub = Hub::new(opts.data_dir.clone(), opts.limits);
    let state = AppState { hub: hub.clone(), static_dir: opts.static_dir.map(Arc::new) };
    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/rooms", get(rooms))
        .route("/collab/{room}", get(collab))
        .fallback(fallback)
        .with_state(state);

    let saver = tokio::spawn(hub.clone().run_saver());
    let h = hub.clone();
    let result = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async move {
            shutdown.await;
            info!("shutting down: closing connections and saving rooms");
            h.begin_shutdown();
        })
        .await;
    // `serve` can also return on an error; make sure everyone stops either way.
    hub.begin_shutdown();
    let deadline = tokio::time::Instant::now() + DRAIN_TIMEOUT;
    while hub.clients() > 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    if let Err(e) = saver.await {
        warn!("saver task: {e}");
    }
    hub.save_all().await;
    info!("stopped");
    result
}

async fn health(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({ "ok": true, "rooms": s.hub.room_count().await, "clients": s.hub.clients() }))
}

async fn rooms(State(s): State<AppState>) -> Response {
    let dir = s.hub.data_dir.clone();
    let stored = match tokio::task::spawn_blocking(move || storage::list(&dir)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            warn!("listing rooms: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "can't list rooms" }))).into_response();
        }
        Err(e) => {
            warn!("listing rooms: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "can't list rooms" }))).into_response();
        }
    };
    let mut online = s.hub.online().await;
    let mut list: Vec<serde_json::Value> = stored
        .into_iter()
        .map(|r| {
            let n = online.remove(&r.id).unwrap_or(0);
            json!({ "id": r.id, "bytes": r.bytes, "updatedAt": r.updated_at, "online": n })
        })
        .collect();
    // Rooms open but not saved yet.
    let mut fresh: Vec<_> = online.into_iter().collect();
    fresh.sort();
    list.extend(fresh.into_iter().map(|(id, n)| json!({ "id": id, "bytes": 0, "updatedAt": null, "online": n })));
    Json(serde_json::Value::Array(list)).into_response()
}

async fn collab(
    State(s): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(room): Path<String>,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    if !storage::valid_room(&room) {
        return (StatusCode::BAD_REQUEST, "invalid room id (use 1-64 of A-Z a-z 0-9 _ -)").into_response();
    }
    let ws = match ws {
        Ok(ws) => ws,
        Err(e) => return e.into_response(),
    };
    if s.hub.is_shutting_down() {
        return (StatusCode::SERVICE_UNAVAILABLE, "shutting down").into_response();
    }
    if s.hub.clients() >= s.hub.limits.max_clients {
        return (StatusCode::SERVICE_UNAVAILABLE, "too many clients").into_response();
    }
    let max = s.hub.limits.max_message;
    let hub = s.hub.clone();
    ws.max_message_size(max)
        .max_frame_size(max)
        .on_failed_upgrade(move |e| debug!("{addr}: WebSocket upgrade failed: {e}"))
        .on_upgrade(move |socket| session(hub, room, socket, addr))
}

async fn fallback(State(s): State<AppState>, method: Method, uri: Uri) -> Response {
    let path = uri.path();
    match &s.static_dir {
        Some(root) if !path.starts_with("/api/") && !path.starts_with("/collab/") => static_files::serve(root, &method, path).await,
        _ => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// One client connection, from join to leave.
async fn session(hub: Arc<Hub>, room_name: String, socket: WebSocket, addr: SocketAddr) {
    let (tx, mut rx) = mpsc::channel::<Bytes>(PEER_QUEUE);
    let (mut sink, mut stream) = socket.split();
    let (room, conn) = match hub.join(&room_name, tx).await {
        Ok(j) => j,
        Err(e) => {
            warn!("room {room_name}: refused {addr}: {e}");
            let frame = CloseFrame { code: close_code::AGAIN, reason: e.to_string().into() };
            let _ = sink.send(Message::Close(Some(frame))).await; // the client may already be gone
            return;
        }
    };
    info!("room {room_name}: client {conn} joined from {addr} ({} here, {} total)", room.peer_count(), hub.clients());

    let last_seen = AtomicU64::new(now_ms());
    let mut stop = hub.shutdown_signal();
    let writer = async {
        let mut ping = tokio::time::interval(PING_EVERY);
        ping.tick().await; // the first tick is immediate
        loop {
            tokio::select! {
                m = rx.recv() => match m {
                    Some(bytes) => {
                        if sink.send(Message::Binary(bytes)).await.is_err() {
                            break;
                        }
                    }
                    None => {
                        // The room dropped us: we couldn't keep up.
                        let frame = CloseFrame { code: close_code::AGAIN, reason: "too slow".into() };
                        let _ = sink.send(Message::Close(Some(frame))).await; // best effort
                        break;
                    }
                },
                _ = ping.tick() => {
                    if now_ms().saturating_sub(last_seen.load(Ordering::Relaxed)) > IDLE_TIMEOUT_MS {
                        info!("room {room_name}: client {conn} timed out");
                        break;
                    }
                    if sink.send(Message::Ping(Bytes::new())).await.is_err() {
                        break;
                    }
                }
                _ = stopped(&mut stop) => {
                    let frame = CloseFrame { code: close_code::AWAY, reason: "server shutting down".into() };
                    let _ = sink.send(Message::Close(Some(frame))).await; // best effort
                    break;
                }
            }
        }
    };
    let reader = async {
        while let Some(m) = stream.next().await {
            last_seen.store(now_ms(), Ordering::Relaxed);
            match m {
                Ok(Message::Binary(b)) => room.receive(conn, &b),
                Ok(Message::Text(_)) => debug!("room {room_name}: client {conn}: ignoring text message"),
                Ok(Message::Close(_)) => break,
                Ok(Message::Ping(_) | Message::Pong(_)) => {}
                Err(e) => {
                    debug!("room {room_name}: client {conn}: {e}");
                    break;
                }
            }
        }
    };
    tokio::select! {
        _ = writer => {}
        _ = reader => {}
    }
    hub.leave(&room, conn).await;
    info!("room {room_name}: client {conn} left ({} total)", hub.clients());
}
