//! Rooms and the Yjs sync protocol, as the standard y-websocket server does it.
//!
//! Each room holds one `yrs::Doc` and its `Awareness`. A client's SyncStep1 is answered with
//! SyncStep2; its SyncStep2/Update is applied and what actually changed is relayed to the other
//! clients; awareness updates are applied and relayed, and when a client goes, the presence
//! entries it introduced are removed for everyone. Messages to a client go through a bounded queue:
//! a client that can't keep up is disconnected (it reconnects and resyncs) instead of growing the
//! server's memory or holding up the room.

use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use log::{debug, error, info, warn};
use tokio::sync::{mpsc, watch};
use yrs::sync::awareness::{Awareness, AwarenessUpdate};
use yrs::sync::{Message, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{ClientID, Doc, ReadTxn, StateVector, Transact, Update};

use crate::storage;

/// Messages queued for one client before it counts as too slow and is disconnected.
pub const PEER_QUEUE: usize = 1024;
/// Save this long after the last change…
const SAVE_DEBOUNCE: Duration = Duration::from_secs(1);
/// …but at least this often while changes keep coming.
const SAVE_MAX_DELAY: Duration = Duration::from_secs(10);
/// Retry a failed save after this long.
const SAVE_RETRY: Duration = Duration::from_secs(5);
/// Presence entries one awareness message may carry (a client announces itself, so 1 is normal).
const MAX_AWARENESS_ENTRIES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    /// Largest WebSocket message (bytes).
    pub max_message: usize,
    /// Clients connected at once, all rooms together.
    pub max_clients: usize,
    /// Rooms open at once.
    pub max_rooms: usize,
    /// Clients in one room.
    pub max_room_clients: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_message: 64 << 20, max_clients: 2000, max_rooms: 1000, max_room_clients: 200 }
    }
}

#[derive(Debug, PartialEq)]
pub enum JoinError {
    ShuttingDown,
    TooManyRooms,
    RoomFull,
    TooManyClients,
    Storage(String),
}

impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JoinError::ShuttingDown => f.write_str("server is shutting down"),
            JoinError::TooManyRooms => f.write_str("too many open rooms"),
            JoinError::RoomFull => f.write_str("room is full"),
            JoinError::TooManyClients => f.write_str("too many clients"),
            JoinError::Storage(e) => write!(f, "storage error: {e}"),
        }
    }
}

/// Milliseconds since the Unix epoch (the awareness clock).
pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)).unwrap_or(0)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while holding the lock (caught by `catch_unwind` below) must not take the room down.
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// All rooms of the server.
pub struct Hub {
    rooms: tokio::sync::Mutex<HashMap<String, Arc<Room>>>,
    pub data_dir: PathBuf,
    pub limits: Limits,
    clients: AtomicUsize,
    next_conn: AtomicU64,
    shutdown: watch::Sender<bool>,
}

impl Hub {
    pub fn new(data_dir: PathBuf, limits: Limits) -> Arc<Hub> {
        let (shutdown, _) = watch::channel(false);
        Arc::new(Hub {
            rooms: tokio::sync::Mutex::new(HashMap::new()),
            data_dir,
            limits,
            clients: AtomicUsize::new(0),
            next_conn: AtomicU64::new(1),
            shutdown,
        })
    }

    pub fn clients(&self) -> usize {
        self.clients.load(Ordering::Relaxed)
    }

    pub async fn room_count(&self) -> usize {
        self.rooms.lock().await.len()
    }

    /// Clients per open room.
    pub async fn online(&self) -> HashMap<String, usize> {
        let rooms: Vec<Arc<Room>> = self.rooms.lock().await.values().cloned().collect();
        rooms.iter().map(|r| (r.name.clone(), r.peer_count())).collect()
    }

    pub fn shutdown_signal(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    pub fn is_shutting_down(&self) -> bool {
        *self.shutdown.borrow()
    }

    /// Ask every connection (and the saver) to finish.
    pub fn begin_shutdown(&self) {
        self.shutdown.send_replace(true);
    }

    /// Add a client to `room` (opening it from disk if needed). Its messages arrive on `tx`,
    /// starting with the server's SyncStep1 and the current presence.
    pub async fn join(&self, room: &str, tx: mpsc::Sender<Bytes>) -> Result<(Arc<Room>, u64), JoinError> {
        if self.is_shutting_down() {
            return Err(JoinError::ShuttingDown);
        }
        let mut rooms = self.rooms.lock().await;
        let r = match rooms.get(room) {
            Some(r) => r.clone(),
            None => {
                if rooms.len() >= self.limits.max_rooms {
                    return Err(JoinError::TooManyRooms);
                }
                let r = Arc::new(self.open(room).await?);
                rooms.insert(room.to_string(), r.clone());
                r
            }
        };
        if r.peer_count() >= self.limits.max_room_clients {
            return Err(JoinError::RoomFull);
        }
        // Reserve a client slot (the check and the increment are one step).
        let max = self.limits.max_clients;
        if self.clients.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < max).then_some(n + 1)).is_err() {
            return Err(JoinError::TooManyClients);
        }
        let conn = self.next_conn.fetch_add(1, Ordering::Relaxed);
        r.add_peer(conn, tx);
        Ok((r, conn))
    }

    /// The client `conn` left `room` (or was dropped). The last one out saves and unloads the room.
    pub async fn leave(&self, room: &Arc<Room>, conn: u64) {
        room.remove_peer(conn);
        self.clients.fetch_sub(1, Ordering::AcqRel);
        if room.peer_count() == 0 {
            self.unload_if_idle(room).await;
        }
    }

    async fn open(&self, name: &str) -> Result<Room, JoinError> {
        let dir = self.data_dir.clone();
        let n = name.to_string();
        let loaded = tokio::task::spawn_blocking(move || storage::load(&dir, &n)).await.map_err(|e| JoinError::Storage(e.to_string()))?;
        let bytes = loaded.map_err(|e| {
            error!("room {name}: can't read stored document: {e}");
            JoinError::Storage(e)
        })?;
        let room = Room::new(name);
        if let Some(bytes) = bytes {
            match room.load(&bytes) {
                Ok(()) => info!("room {name}: opened ({} bytes from disk)", bytes.len()),
                Err(e) => {
                    // Keep the bad file for inspection; start empty rather than refuse the room forever.
                    let dir = self.data_dir.clone();
                    let n = name.to_string();
                    let moved =
                        tokio::task::spawn_blocking(move || storage::quarantine(&dir, &n)).await.map_err(|e| JoinError::Storage(e.to_string()))?;
                    match moved {
                        Ok(to) => error!("room {name}: stored document is corrupt ({e}); moved it to {} and started empty", to.display()),
                        Err(e2) => {
                            error!("room {name}: stored document is corrupt ({e}) and couldn't be moved aside ({e2})");
                            return Err(JoinError::Storage(e2));
                        }
                    }
                }
            }
        } else {
            info!("room {name}: created");
        }
        Ok(room)
    }

    async fn unload_if_idle(&self, room: &Arc<Room>) {
        // Holding the room map while saving: a client joining the same room meanwhile waits and
        // then loads what was just written.
        let mut rooms = self.rooms.lock().await;
        if room.peer_count() > 0 || !rooms.get(&room.name).is_some_and(|r| Arc::ptr_eq(r, room)) {
            return;
        }
        match self.save(room).await {
            Ok(()) => {
                rooms.remove(&room.name);
                info!("room {}: closed (no clients)", room.name);
            }
            Err(e) => error!("room {}: not unloaded, saving failed: {e}", room.name),
        }
    }

    /// Save `room` if it changed since its last save.
    pub async fn save(&self, room: &Arc<Room>) -> Result<(), String> {
        let _order = room.save_lock.lock().await;
        let Some(bytes) = room.take_snapshot() else { return Ok(()) };
        let dir = self.data_dir.clone();
        let name = room.name.clone();
        let len = bytes.len();
        let result = tokio::task::spawn_blocking(move || storage::save(&dir, &name, &bytes)).await.map_err(|e| e.to_string()).and_then(|r| r);
        match &result {
            Ok(()) => debug!("room {}: saved {len} bytes", room.name),
            Err(e) => {
                error!("room {}: save failed: {e}", room.name);
                room.save_failed();
            }
        }
        result
    }

    /// Save every open room that has unsaved changes.
    pub async fn save_all(&self) {
        let rooms: Vec<Arc<Room>> = self.rooms.lock().await.values().cloned().collect();
        for r in rooms {
            let _ = self.save(&r).await; // logged by `save`
        }
    }

    /// Save rooms ~1 s after their last change (and unload idle ones) until shutdown.
    pub async fn run_saver(self: Arc<Self>) {
        let mut stop = self.shutdown_signal();
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                _ = stopped(&mut stop) => return,
            }
            let now = Instant::now();
            let rooms: Vec<Arc<Room>> = self.rooms.lock().await.values().cloned().collect();
            for r in rooms {
                if r.save_due(now) {
                    let _ = self.save(&r).await; // logged by `save`
                }
                // A room left open without clients (its unloading save failed earlier, now done).
                if r.peer_count() == 0 && !r.is_dirty() {
                    self.unload_if_idle(&r).await;
                }
            }
        }
    }
}

/// One shared document and who is connected to it.
pub struct Room {
    pub name: String,
    state: Mutex<RoomState>,
    save_lock: tokio::sync::Mutex<()>,
}

struct RoomState {
    awareness: Awareness,
    /// What the last transaction changed (filled by the document's update observer).
    changes: Arc<Mutex<Vec<Vec<u8>>>>,
    peers: HashMap<u64, Peer>,
    dirty: bool,
    first_change: Instant,
    last_change: Instant,
    retry_at: Option<Instant>,
    /// Peers whose queue overflowed, to disconnect once the current message is handled.
    kicked: Vec<u64>,
}

struct Peer {
    tx: mpsc::Sender<Bytes>,
    /// Presence entries this connection introduced (removed when it goes).
    presence: HashSet<ClientID>,
}

impl Room {
    fn new(name: &str) -> Room {
        let doc = Doc::new();
        let changes: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
        let sink = changes.clone();
        // A fresh document has no transaction open, so this can't fail; if it ever did, updates
        // would still be applied and saved, only not relayed — log it loudly.
        if let Err(e) = doc.observe_update_v1("relay", move |_txn, e| lock(&sink).push(e.update.clone())) {
            error!("room {name}: can't observe document updates: {e}");
        }
        let awareness = Awareness::with_clock(doc, now_ms);
        let now = Instant::now();
        Room {
            name: name.to_string(),
            state: Mutex::new(RoomState {
                awareness,
                changes,
                peers: HashMap::new(),
                dirty: false,
                first_change: now,
                last_change: now,
                retry_at: None,
                kicked: vec![],
            }),
            save_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Seed the room with a stored update (not marked as a change).
    fn load(&self, bytes: &[u8]) -> Result<(), String> {
        let mut st = lock(&self.state);
        let r = guarded(|| {
            let update = Update::decode_v1(bytes).map_err(|e| e.to_string())?;
            let mut txn = st.awareness.doc().transact_mut();
            txn.apply_update(update).map_err(|e| e.to_string())
        });
        lock(&st.changes).clear();
        st.dirty = false;
        r
    }

    pub fn peer_count(&self) -> usize {
        lock(&self.state).peers.len()
    }

    fn add_peer(&self, conn: u64, tx: mpsc::Sender<Bytes>) {
        let mut st = lock(&self.state);
        let sv = st.awareness.doc().transact().state_vector();
        let mut hello = vec![Message::Sync(SyncMessage::SyncStep1(sv)).encode_v1()];
        match st.awareness.update() {
            Ok(u) if !u.clients.is_empty() => hello.push(Message::Awareness(u).encode_v1()),
            Ok(_) => {}
            Err(e) => warn!("room {}: can't encode presence: {e}", self.name),
        }
        st.peers.insert(conn, Peer { tx, presence: HashSet::new() });
        for m in hello {
            st.send(conn, Bytes::from(m));
        }
        st.drop_kicked(&self.name);
    }

    fn remove_peer(&self, conn: u64) {
        let mut st = lock(&self.state);
        st.remove_peer(conn);
        st.drop_kicked(&self.name);
    }

    /// Handle one binary message from client `conn`. Malformed messages are logged and ignored.
    pub fn receive(&self, conn: u64, data: &[u8]) {
        let msg = match guarded(|| Message::decode_v1(data).map_err(|e| e.to_string())) {
            Ok(m) => m,
            Err(e) => {
                warn!("room {}: client {conn}: ignoring malformed message ({} bytes): {e}", self.name, data.len());
                return;
            }
        };
        let mut st = lock(&self.state);
        if !st.peers.contains_key(&conn) {
            return; // disconnected (too slow) while this message was in flight
        }
        match msg {
            Message::Sync(SyncMessage::SyncStep1(sv)) => {
                let diff = guarded(|| Ok(st.awareness.doc().transact().encode_diff_v1(&sv)));
                match diff {
                    Ok(d) => st.send(conn, Bytes::from(Message::Sync(SyncMessage::SyncStep2(d)).encode_v1())),
                    Err(e) => warn!("room {}: client {conn}: can't answer SyncStep1: {e}", self.name),
                }
            }
            Message::Sync(SyncMessage::SyncStep2(u) | SyncMessage::Update(u)) => st.apply_update(&self.name, conn, &u),
            Message::Awareness(update) => st.apply_awareness(&self.name, conn, update),
            Message::AwarenessQuery => match st.awareness.update() {
                Ok(u) => st.send(conn, Bytes::from(Message::Awareness(u).encode_v1())),
                Err(e) => warn!("room {}: can't encode presence: {e}", self.name),
            },
            Message::Auth(_) | Message::Custom(..) => debug!("room {}: client {conn}: ignoring auth/custom message", self.name),
        }
        st.drop_kicked(&self.name);
    }

    /// The whole document as one v1 update if it changed since the last snapshot.
    fn take_snapshot(&self) -> Option<Vec<u8>> {
        let mut st = lock(&self.state);
        if !st.dirty {
            return None;
        }
        let bytes = guarded(|| Ok(st.awareness.doc().transact().encode_state_as_update_v1(&StateVector::default())));
        match bytes {
            Ok(b) => {
                st.dirty = false;
                st.retry_at = None;
                Some(b)
            }
            Err(e) => {
                error!("room {}: can't encode the document: {e}", self.name);
                None
            }
        }
    }

    fn save_failed(&self) {
        let mut st = lock(&self.state);
        let now = Instant::now();
        if !st.dirty {
            st.dirty = true;
            st.first_change = now;
            st.last_change = now;
        }
        st.retry_at = Some(now + SAVE_RETRY);
    }

    fn is_dirty(&self) -> bool {
        lock(&self.state).dirty
    }

    fn save_due(&self, now: Instant) -> bool {
        let st = lock(&self.state);
        st.dirty
            && st.retry_at.is_none_or(|t| now >= t)
            && (now.duration_since(st.last_change) >= SAVE_DEBOUNCE || now.duration_since(st.first_change) >= SAVE_MAX_DELAY)
    }
}

impl RoomState {
    fn send(&mut self, conn: u64, msg: Bytes) {
        if let Some(p) = self.peers.get(&conn)
            && p.tx.try_send(msg).is_err()
        {
            self.kicked.push(conn);
        }
    }

    fn broadcast(&mut self, except: u64, msg: &Bytes) {
        for (&id, p) in &self.peers {
            if id != except && p.tx.try_send(msg.clone()).is_err() {
                self.kicked.push(id);
            }
        }
    }

    fn apply_update(&mut self, room: &str, conn: u64, update: &[u8]) {
        let result = guarded(|| {
            let u = Update::decode_v1(update).map_err(|e| e.to_string())?;
            let mut txn = self.awareness.doc().transact_mut();
            txn.apply_update(u).map_err(|e| e.to_string())
        });
        if let Err(e) = result {
            warn!("room {room}: client {conn}: bad document update ({} bytes): {e}", update.len());
        }
        // Relay what was integrated (even from an update that failed half-way).
        let changes = std::mem::take(&mut *lock(&self.changes));
        if changes.is_empty() {
            return;
        }
        let now = Instant::now();
        if !self.dirty {
            self.dirty = true;
            self.first_change = now;
        }
        self.last_change = now;
        for c in changes {
            let msg = Bytes::from(Message::Sync(SyncMessage::Update(c)).encode_v1());
            self.broadcast(conn, &msg);
        }
    }

    fn apply_awareness(&mut self, room: &str, conn: u64, update: AwarenessUpdate) {
        if update.clients.len() > MAX_AWARENESS_ENTRIES {
            warn!("room {room}: client {conn}: ignoring presence update with {} entries", update.clients.len());
            return;
        }
        let summary = match guarded(|| self.awareness.apply_update_summary(update).map_err(|e| e.to_string())) {
            Ok(Some(s)) => s,
            Ok(None) => return,
            Err(e) => {
                warn!("room {room}: client {conn}: bad presence update: {e}");
                return;
            }
        };
        if let Some(p) = self.peers.get_mut(&conn) {
            p.presence.extend(summary.added.iter().chain(&summary.updated).copied());
            for id in &summary.removed {
                p.presence.remove(id);
            }
        }
        match self.awareness.update_with_clients(summary.all_changes()) {
            Ok(u) => {
                let msg = Bytes::from(Message::Awareness(u).encode_v1());
                self.broadcast(conn, &msg);
            }
            Err(e) => warn!("room {room}: can't encode presence: {e}"),
        }
    }

    /// Forget `conn` and tell the others its presence is gone.
    fn remove_peer(&mut self, conn: u64) {
        let Some(peer) = self.peers.remove(&conn) else { return };
        let ids: Vec<ClientID> = peer.presence.into_iter().collect();
        if ids.is_empty() {
            return;
        }
        for id in &ids {
            self.awareness.remove_state(*id);
        }
        match self.awareness.update_with_clients(ids) {
            Ok(u) => {
                let msg = Bytes::from(Message::Awareness(u).encode_v1());
                self.broadcast(conn, &msg);
            }
            Err(e) => warn!("can't encode presence removal: {e}"),
        }
    }

    /// Disconnect clients whose queue overflowed (dropping the sender ends their session).
    fn drop_kicked(&mut self, room: &str) {
        while let Some(id) = self.kicked.pop() {
            if self.peers.contains_key(&id) {
                warn!("room {room}: client {id} can't keep up; disconnecting it");
                self.remove_peer(id);
            }
        }
    }
}

/// Resolves once shutdown has begun.
pub async fn stopped(rx: &mut watch::Receiver<bool>) {
    // Err: the hub is gone, which also means stop.
    let _ = rx.wait_for(|s| *s).await;
}

/// Run decoding/applying of untrusted data, turning a panic inside `yrs` into an error so one bad
/// message can't take down the room or the server.
fn guarded<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(p) => {
            let what = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
            Err(format!("internal error in the CRDT library: {what}"))
        }
    }
}
