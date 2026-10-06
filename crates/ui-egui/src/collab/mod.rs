//! Real-time co-editing (File → Share / Collaborate…): the open document shared with a room on a
//! collaboration server, everyone's pointer and selection on the canvas.
//!
//! The engine does the syncing ([`vectorcraft_engine::collab::Collab`]); this module owns the
//! socket ([`transport`]) and drives both once per frame ([`frame`]): what arrived goes to the
//! engine, the engine's replies and edits go out. The commands are `collab.share` (the dialog),
//! `collab.join`, `collab.leave` and `collab.status`; the web app also joins the room named in the
//! page's `?room=` (and `&server=`).

pub mod overlay;
pub mod transport;
pub mod urls;

#[cfg(test)]
mod tests;

use serde_json::{Value, json};
use vectorcraft_engine::collab::{Collab, Connection, SharedDoc};

use crate::VectorcraftApp;
pub use transport::{Event, Socket};
pub use urls::{default_server, invite_link, normalize_server, parse_invite, socket_url, validate_room};

/// How often the canvas redraws while collaborating, so others' edits and pointers show without
/// input (a message arriving also asks for a frame).
const REPAINT_MS: u64 = 100;

/// Opens a socket to a URL (tests use an in-memory one).
pub type Connector = Box<dyn FnMut(&str) -> Box<dyn Socket>>;

/// Where the connection stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Opening the first connection.
    Connecting,
    /// Connected; the room's document is on its way.
    Syncing,
    /// Connected and editing together.
    Live,
    /// The connection dropped; retrying.
    Reconnecting,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Connecting => "Connecting…",
            Status::Syncing => "Joining…",
            Status::Live => "Live",
            Status::Reconnecting => "Reconnecting…",
        }
    }

    /// The `collab.status` id.
    pub fn id(self) -> &'static str {
        match self {
            Status::Connecting => "connecting",
            Status::Syncing => "syncing",
            Status::Live => "live",
            Status::Reconnecting => "reconnecting",
        }
    }
}

/// One document shared with one room.
pub struct CollabSession {
    pub collab: Collab,
    /// Opened on the next frame (the platform socket asks egui for repaints).
    socket: Option<Box<dyn Socket>>,
    pub status: Status,
    /// The server's base URL (`ws://host:port`).
    pub server: String,
    pub room: String,
    /// The socket's URL (`…/collab/<room>`).
    pub url: String,
    /// The last connection problem (cleared when the connection opens).
    pub error: Option<String>,
    opened: bool,
}

impl CollabSession {
    fn send_all(&mut self, msgs: Vec<Vec<u8>>) {
        if let Some(s) = &mut self.socket {
            for m in msgs {
                s.send(m);
            }
        }
    }
}

/// The app's collaboration state.
#[derive(Default)]
pub struct CollabState {
    pub session: Option<CollabSession>,
    /// Opens sockets instead of the platform's WebSocket (tests).
    pub connector: Option<Connector>,
}

/// What `collab.join` was asked for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JoinRequest {
    pub server: Option<String>,
    pub room: Option<String>,
    pub name: Option<String>,
    /// Join in a new document instead of sharing the active one.
    pub new_document: bool,
}

impl JoinRequest {
    fn from_params(p: &Value) -> Self {
        let s = |k: &str| p.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        Self { server: s("server"), room: s("room"), name: s("name"), new_document: p.get("newDocument").and_then(Value::as_bool).unwrap_or(false) }
    }
}

/// A short random room id (8 lowercase letters and digits, no look-alikes).
pub fn random_room() -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut n = transport::random_u64();
    (0..8)
        .map(|_| {
            let c = ALPHABET.get((n % ALPHABET.len() as u64) as usize).copied().unwrap_or(b'x');
            n /= ALPHABET.len() as u64;
            char::from(c)
        })
        .collect()
}

/// The name shown to the others when none was given: the remembered one, else the account's.
pub fn default_name(app: &VectorcraftApp) -> String {
    let saved = app.ui.collab_name.trim();
    if !saved.is_empty() {
        return saved.to_string();
    }
    if let Some(n) = storage::recall("name").filter(|n| !n.trim().is_empty()) {
        return n;
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(n) = ["USER", "USERNAME"].iter().find_map(|k| std::env::var(k).ok()).filter(|n| !n.trim().is_empty()) {
        return n;
    }
    format!("Guest {}", transport::random_u64() % 900 + 100)
}

/// The server to offer: the remembered one, else the default for this build.
pub fn preferred_server(app: &VectorcraftApp) -> String {
    let saved = app.ui.collab_server.trim();
    if !saved.is_empty() {
        return saved.to_string();
    }
    storage::recall("server").filter(|s| !s.trim().is_empty()).unwrap_or_else(|| default_server(urls::page_url().as_deref()))
}

/// The shared document's replica: a random client id (the browser's randomness on the web).
fn shared_doc() -> SharedDoc {
    #[cfg(target_arch = "wasm32")]
    {
        SharedDoc::with_client_id(transport::random_u64())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        SharedDoc::new()
    }
}

/// `collab.join`: share the active document with a room (a new document when asked, or when none
/// is open). The socket opens on the next frame.
pub fn join(app: &mut VectorcraftApp, req: JoinRequest) -> Result<Value, String> {
    let mut server = req.server.clone();
    let mut room = req.room.clone();
    // An invite link (or a socket URL) pasted as the room names the server too.
    if let Some((s, r)) = room.as_deref().and_then(parse_invite) {
        server = s.or(server);
        room = Some(r);
    }
    let server = normalize_server(&server.unwrap_or_else(|| preferred_server(app)))?;
    let room = match room {
        Some(r) => validate_room(&r)?,
        None => random_room(),
    };
    let name: String = req.name.clone().unwrap_or_else(|| default_name(app)).trim().chars().take(40).collect();
    if app.collab.session.is_some() {
        leave(app);
    }
    if req.new_document || app.session.active().is_none() {
        app.run("file.new", json!({}))?;
    }
    let uid = app.session.active().map(|d| d.uid).ok_or("no document open")?;
    app.ui.collab_name = name.clone();
    app.ui.collab_server = server.clone();
    storage::remember("name", &name);
    storage::remember("server", &server);
    let url = socket_url(&server, &room);
    let collab = Collab::new(Connection::new(shared_doc(), transport::clock_ms), uid, name);
    app.collab.session = Some(CollabSession {
        collab,
        socket: None,
        status: Status::Connecting,
        server: server.clone(),
        room: room.clone(),
        url: url.clone(),
        error: None,
        opened: false,
    });
    app.status(format!("Joining room {room}…"));
    let invite = invite_link(&server, &room, urls::page_url().as_deref());
    Ok(json!({"room": room, "server": server, "url": url, "invite": invite}))
}

/// `collab.leave`: stop sharing; the document stays open with its own Undo history.
pub fn leave(app: &mut VectorcraftApp) -> bool {
    let Some(mut cs) = app.collab.session.take() else { return false };
    if let Some(bye) = cs.collab.leave(&mut app.session) {
        cs.send_all(vec![bye]);
    }
    // Dropping the socket sends what is queued and closes it.
    drop(cs);
    app.status("Stopped collaborating");
    true
}

/// Join the room a page URL's query names (`?room=…&server=…`): the web app's invite links.
pub fn join_from_query(app: &mut VectorcraftApp, query: &str) -> Option<Result<Value, String>> {
    let (server, room) = urls::query_invite(query)?;
    let new_document = app.session.active().is_some();
    Some(join(app, JoinRequest { server, room: Some(room), name: None, new_document }))
}

/// `collab.status`.
pub fn status(app: &VectorcraftApp) -> Value {
    let Some(cs) = &app.collab.session else { return json!({"active": false}) };
    let peers: Vec<Value> = cs
        .collab
        .peers()
        .into_iter()
        .map(|(id, p)| json!({"id": id, "name": p.name, "color": p.color, "cursor": p.cursor, "selection": p.selection, "tool": p.tool}))
        .collect();
    json!({
        "active": true,
        "status": cs.status.id(),
        "joined": cs.collab.is_joined(),
        "room": cs.room,
        "server": cs.server,
        "url": cs.url,
        "invite": invite_link(&cs.server, &cs.room, urls::page_url().as_deref()),
        "error": cs.error,
        "document": cs.collab.doc_uid,
        "you": {"name": cs.collab.name, "color": cs.collab.color()},
        "peers": peers,
    })
}

/// Once per frame: deliver what arrived, sync the document, send what the engine has to say.
pub fn frame(app: &mut VectorcraftApp, ctx: &egui::Context) {
    let VectorcraftApp { collab: state, session, hover_doc, ui: prefs, .. } = app;
    let Some(cs) = state.session.as_mut() else { return };
    if cs.socket.is_none() {
        cs.socket = Some(match state.connector.as_mut() {
            Some(open) => open(&cs.url),
            None => transport::connect(&cs.url, Some(ctx.clone())),
        });
    }
    let events = cs.socket.as_mut().map(|s| s.poll()).unwrap_or_default();
    for e in events {
        match e {
            Event::Opened => {
                cs.opened = true;
                cs.error = None;
                cs.status = Status::Syncing;
                let hello = cs.collab.on_connect();
                cs.send_all(hello);
            }
            Event::Message(m) => match cs.collab.receive(&m) {
                Ok(replies) => cs.send_all(replies),
                // A message we can't read is skipped; the next sync repairs what it carried.
                Err(e) => {
                    log::warn!("collaboration: {e}");
                    cs.error = Some(e);
                }
            },
            Event::Closed(why) => {
                cs.status = if cs.opened { Status::Reconnecting } else { Status::Connecting };
                cs.error = Some(why);
            }
            Event::Error(e) => cs.error = Some(e),
        }
    }
    // Our pointer is shared only over the shared document, and not at all while kept private.
    let pointer = (*hover_doc).filter(|_| !prefs.collab_private_cursor && session.active().is_some_and(|d| d.uid == cs.collab.doc_uid));
    let (report, out) = cs.collab.tick(session, pointer);
    cs.send_all(out);
    if cs.status == Status::Syncing && cs.collab.is_joined() && cs.collab.conn.is_synced() {
        cs.status = Status::Live;
    }
    if report.changed {
        ctx.request_repaint();
    }
    if report.closed {
        // The goodbye went out with the tick: drop the socket.
        state.session = None;
        app.status("Stopped collaborating: the shared document was closed");
        return;
    }
    app.sync_views();
    ctx.request_repaint_after(std::time::Duration::from_millis(REPAINT_MS));
}

/// Run a `collab.*` UI command (None: not one).
pub fn run_command(app: &mut VectorcraftApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        "collab.share" => {
            crate::dialogs::collab::open(app);
            Ok(Value::Null)
        }
        "collab.join" => join(app, JoinRequest::from_params(p)),
        "collab.leave" => {
            if leave(app) {
                Ok(Value::Null)
            } else {
                Err("not collaborating".into())
            }
        }
        "collab.status" => Ok(status(app)),
        "collab.showCursors" => {
            let show = p.get("show").and_then(Value::as_bool).unwrap_or(app.ui.collab_hide_cursors);
            app.ui.collab_hide_cursors = !show;
            Ok(json!({ "show": show }))
        }
        "collab.shareCursor" => {
            let share = p.get("share").and_then(Value::as_bool).unwrap_or(app.ui.collab_private_cursor);
            app.ui.collab_private_cursor = !share;
            Ok(json!({ "share": share }))
        }
        _ => return None,
    })
}

/// Remembers the name and server in the browser's storage on the web (the desktop app keeps them
/// with its UI preferences).
mod storage {
    #[cfg(target_arch = "wasm32")]
    fn store() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok()?
    }

    pub fn recall(key: &str) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            store()?.get_item(&format!("vectorcraft.collab.{key}")).ok()?
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = key;
            None
        }
    }

    pub fn remember(key: &str, value: &str) {
        #[cfg(target_arch = "wasm32")]
        if let Some(s) = store() {
            // Storage turned off or full: the value is just not remembered.
            let _ = s.set_item(&format!("vectorcraft.collab.{key}"), value);
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (key, value);
    }
}
