//! Two apps co-editing through an in-memory socket and a relay that behaves like the
//! collaboration server; the dialog and commands; pointers mapped to the screen.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use serde_json::json;
use vectorcraft_doc::NodeKind;
use vectorcraft_engine::Session;
use vectorcraft_engine::collab::{Connection, Peer, SharedDoc};
use vectorcraft_geom::Point;

use super::*;
use crate::canvas::Xf;
use crate::menus;

/// One end of an in-memory socket: what the app sent, what it is to receive.
#[derive(Default)]
struct Pipe {
    url: String,
    inbox: VecDeque<Event>,
    outbox: Vec<Vec<u8>>,
    dropped: bool,
}

struct FakeSocket(Rc<RefCell<Pipe>>);

impl Socket for FakeSocket {
    fn send(&mut self, msg: Vec<u8>) {
        self.0.borrow_mut().outbox.push(msg);
    }
    fn poll(&mut self) -> Vec<Event> {
        self.0.borrow_mut().inbox.drain(..).collect()
    }
}

impl Drop for FakeSocket {
    fn drop(&mut self) {
        self.0.borrow_mut().dropped = true;
    }
}

/// The server: one replica of the room; what a client sends is applied, answered and relayed to
/// the others (as `vectorcraft-collab-server` does).
struct Relay {
    room: Connection,
    pipes: Rc<RefCell<Vec<Rc<RefCell<Pipe>>>>>,
}

impl Relay {
    fn new() -> Self {
        Self { room: Connection::new(SharedDoc::with_client_id(1_000_000), || 1u64), pipes: Rc::default() }
    }

    /// A connector for an app: each socket it opens is a pipe here, open at once.
    fn connector(&self) -> Connector {
        let pipes = self.pipes.clone();
        Box::new(move |url: &str| {
            let pipe = Rc::new(RefCell::new(Pipe { url: url.into(), inbox: VecDeque::from([Event::Opened]), ..Default::default() }));
            pipes.borrow_mut().push(pipe.clone());
            Box::new(FakeSocket(pipe)) as Box<dyn Socket>
        })
    }

    /// Run frames and deliver messages until everyone is quiet.
    fn pump(&mut self, apps: &mut [&mut VectorcraftApp], ctx: &egui::Context) {
        for _ in 0..30 {
            for app in apps.iter_mut() {
                frame(app, ctx);
            }
            let pipes: Vec<_> = self.pipes.borrow().iter().filter(|p| !p.borrow().dropped || !p.borrow().outbox.is_empty()).cloned().collect();
            let mut quiet = true;
            for (i, pipe) in pipes.iter().enumerate() {
                let out = std::mem::take(&mut pipe.borrow_mut().outbox);
                for m in out {
                    quiet = false;
                    let replies = self.room.receive(&m).unwrap();
                    pipe.borrow_mut().inbox.extend(replies.into_iter().map(Event::Message));
                    let relayed = m.first().is_some_and(|t| *t == 1) || m.get(..2).is_some_and(|h| h == [0, 2] || h == [0, 1]);
                    if relayed {
                        for (j, other) in pipes.iter().enumerate() {
                            if j != i && !other.borrow().dropped {
                                other.borrow_mut().inbox.push_back(Event::Message(m.clone()));
                            }
                        }
                    }
                }
            }
            if quiet && pipes.iter().all(|p| p.borrow().inbox.is_empty()) {
                break;
            }
        }
    }
}

fn app_with(relay: &Relay) -> VectorcraftApp {
    let mut app = VectorcraftApp::new(Session::new(), crate::Services::default());
    app.collab.connector = Some(relay.connector());
    app
}

fn paths(app: &VectorcraftApp) -> usize {
    let mut n = 0;
    if let Some(st) = app.session.active() {
        st.doc.walk(|node| {
            if matches!(node.kind, NodeKind::Path { .. }) {
                n += 1;
            }
        });
    }
    n
}

fn peers(app: &VectorcraftApp) -> Vec<Peer> {
    app.collab.session.as_ref().map(|s| s.collab.peers().into_iter().map(|(_, p)| p).collect()).unwrap_or_default()
}

#[test]
fn two_apps_co_edit_through_the_commands() {
    let ctx = egui::Context::default();
    let mut relay = Relay::new();
    let mut a = app_with(&relay);
    let mut b = app_with(&relay);

    // A shares a document with a rectangle.
    a.run("file.new", json!({"width": 400, "height": 300})).unwrap();
    a.run("shape.rectangle", json!({"x": 10, "y": 10, "width": 50, "height": 20})).unwrap();
    let r = a.run("collab.join", json!({"room": "r1", "server": "localhost:1234", "name": "Ann"})).unwrap();
    assert_eq!(r["url"], "ws://localhost:1234/collab/r1");
    assert_eq!(r["invite"], "http://localhost:1234/?room=r1");
    assert_eq!(a.run("collab.status", json!({})).unwrap()["status"], "connecting");
    relay.pump(&mut [&mut a], &ctx);
    assert_eq!(relay.pipes.borrow()[0].borrow().url, "ws://localhost:1234/collab/r1");
    let st = a.run("collab.status", json!({})).unwrap();
    assert_eq!((st["status"].as_str(), st["joined"].as_bool()), (Some("live"), Some(true)), "{st}");
    // The name and server are remembered for next time.
    assert_eq!((a.ui.collab_name.as_str(), a.ui.collab_server.as_str()), ("Ann", "ws://localhost:1234"));

    // B joins by the invite link with nothing open: a new document gets the room's.
    b.run("collab.join", json!({"room": "http://localhost:1234/?room=r1", "name": "Bo"})).unwrap();
    relay.pump(&mut [&mut a, &mut b], &ctx);
    assert_eq!(paths(&b), 1);
    assert_eq!(b.collab.session.as_ref().unwrap().status, Status::Live);

    // B draws; A sees it without doing anything.
    b.run("shape.rectangle", json!({"x": 100, "y": 10, "width": 50, "height": 20})).unwrap();
    relay.pump(&mut [&mut a, &mut b], &ctx);
    assert_eq!(paths(&a), 2);

    // Presence: names, pointer (over the shared document) and selection.
    a.hover_doc = Some(Point::new(5.0, 6.0));
    a.run("select.all", json!({})).unwrap();
    relay.pump(&mut [&mut a, &mut b], &ctx);
    let seen = peers(&b);
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].name, "Ann");
    assert_eq!(seen[0].cursor, Some([5.0, 6.0]));
    assert_eq!(seen[0].selection.len(), 2);
    let st = b.run("collab.status", json!({})).unwrap();
    assert_eq!(st["peers"][0]["name"], "Ann");
    assert_eq!(peers(&a)[0].name, "Bo");

    // A's Undo goes through the room: B's rectangle stays.
    a.run("shape.rectangle", json!({"x": 200, "y": 10, "width": 50, "height": 20})).unwrap();
    relay.pump(&mut [&mut a, &mut b], &ctx);
    assert_eq!(paths(&b), 3);
    a.run("edit.undo", json!({})).unwrap();
    relay.pump(&mut [&mut a, &mut b], &ctx);
    assert_eq!((paths(&a), paths(&b)), (2, 2));

    // A leaves, saying goodbye and closing the socket; A keeps the document.
    assert!(menus::enabled(&a, "collab.leave"));
    a.run("collab.leave", json!({})).unwrap();
    assert!(!menus::enabled(&a, "collab.leave"));
    {
        let pipes = relay.pipes.borrow();
        let pa = pipes[0].borrow();
        assert!(pa.dropped);
        assert_eq!(pa.outbox.last().and_then(|m| m.first()), Some(&1), "the goodbye (an awareness message) was queued");
    }
    relay.pump(&mut [&mut a, &mut b], &ctx);
    assert_eq!(paths(&a), 2);
    assert_eq!(a.run("collab.status", json!({})).unwrap(), json!({"active": false}));
    assert!(a.run("collab.leave", json!({})).is_err());

    // Closing the shared document ends B's session.
    b.session.execute("file.close", &json!({})).unwrap();
    assert!(b.session.documents().is_empty());
    relay.pump(&mut [&mut b], &ctx);
    assert!(b.collab.session.is_none());
    assert!(b.ui.status.contains("closed"), "{}", b.ui.status);
}

#[test]
fn reconnecting_says_hello_again() {
    let ctx = egui::Context::default();
    let mut relay = Relay::new();
    let mut a = app_with(&relay);
    a.run("collab.join", json!({"room": "r2", "server": "ws://h"})).unwrap();
    relay.pump(&mut [&mut a], &ctx);
    let pipe = relay.pipes.borrow()[0].clone();
    pipe.borrow_mut().inbox.push_back(Event::Closed("network down".into()));
    frame(&mut a, &ctx);
    let cs = a.collab.session.as_ref().unwrap();
    assert_eq!((cs.status, cs.error.as_deref()), (Status::Reconnecting, Some("network down")));
    // Reopened: the hello goes out again and the session is live once more.
    pipe.borrow_mut().inbox.push_back(Event::Opened);
    frame(&mut a, &ctx);
    assert!(!pipe.borrow().outbox.is_empty(), "no hello after reconnecting");
    relay.pump(&mut [&mut a], &ctx);
    assert_eq!(a.collab.session.as_ref().unwrap().status, Status::Live);
}

#[test]
fn join_rejects_bad_addresses() {
    let mut app = VectorcraftApp::new(Session::new(), crate::Services::default());
    assert!(app.run("collab.join", json!({"room": "a b"})).is_err());
    assert!(app.run("collab.join", json!({"room": "ok", "server": "ftp://x"})).is_err());
    assert!(app.collab.session.is_none());
    // No room: a random one, on the remembered (here: default) server.
    let r = app.run("collab.join", json!({})).unwrap();
    let room = r["room"].as_str().unwrap();
    assert_eq!(room.len(), 8);
    assert_eq!(r["server"], urls::NATIVE_DEFAULT_SERVER);
    assert_ne!(random_room(), random_room());
}

#[test]
fn page_query_joins_in_a_new_document() {
    let mut app = VectorcraftApp::new(Session::new(), crate::Services::default());
    app.run("file.new", json!({})).unwrap();
    assert!(join_from_query(&mut app, "?webgl").is_none());
    join_from_query(&mut app, "?room=team&server=wss%3A%2F%2Fcollab.example.org").unwrap().unwrap();
    let cs = app.collab.session.as_ref().unwrap();
    assert_eq!(cs.url, "wss://collab.example.org/collab/team");
    // The document that was open stays as it was; the room has its own.
    assert_eq!(app.session.documents().len(), 2);
}

#[test]
fn share_dialog_joins_and_shows_the_room() {
    let ctx = egui::Context::default();
    let mut relay = Relay::new();
    let mut app = app_with(&relay);
    app.run("file.new", json!({})).unwrap();
    assert!(crate::dialogs::DialogKind::of(crate::dialogs::collab::KIND).is_some());
    app.run("collab.share", json!({})).unwrap();
    let d = app.ui.dialog.clone().unwrap();
    assert_eq!(d.kind, "collab");
    assert_eq!(d.str("room"), d.str("__generated"));
    assert_eq!(d.str("server"), urls::NATIVE_DEFAULT_SERVER);
    // The dialog draws (both states) without trouble.
    crate::theme::install_fonts(&ctx);
    let draw = |app: &mut VectorcraftApp| {
        for _ in 0..2 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(app, ui.ctx()));
            out.textures_delta.clear();
        }
    };
    draw(&mut app);
    // Join shares the open document (its own room id was kept).
    crate::dialogs::confirm(&mut app).unwrap();
    assert!(app.ui.dialog.is_none());
    assert_eq!(app.session.documents().len(), 1);
    relay.pump(&mut [&mut app], &ctx);
    app.run("collab.share", json!({})).unwrap();
    draw(&mut app);
    // While collaborating, OK just closes.
    crate::dialogs::confirm(&mut app).unwrap();
    assert!(app.ui.dialog.is_none() && app.collab.session.is_some());
}

#[test]
fn every_collab_command_is_registered_and_in_the_file_menu() {
    for id in ["collab.share", "collab.join", "collab.leave", "collab.status"] {
        assert!(menus::UI_COMMANDS.iter().any(|c| c.0 == id), "{id}");
    }
    let tree = menus::menu_tree();
    let (_, file) = tree.iter().find(|(t, _)| *t == "File").unwrap();
    let ids: Vec<&str> = file.iter().filter_map(|i| if let menus::Item::Cmd(_, id, _) = i { Some(*id) } else { None }).collect();
    assert!(ids.contains(&"collab.share") && ids.contains(&"collab.leave"));
}

#[test]
fn peer_pointers_map_to_the_screen() {
    let rect = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(800.0, 600.0));
    let xf = Xf { rect, zoom: 2.0, center: Point::new(200.0, 150.0), rot: 0.0 };
    let peer = |cursor| Peer { name: "P".into(), color: [1, 2, 3], cursor, ..Default::default() };
    // The view's centre is the canvas's; 10 pt away is 20 px at 200 %.
    let at = overlay::cursor_screen(&peer(Some([210.0, 150.0])), &xf).unwrap();
    assert!((at.x - 520.0).abs() < 1e-3 && (at.y - 350.0).abs() < 1e-3, "{at:?}");
    assert!(overlay::cursor_screen(&peer(None), &xf).is_none());
    assert!(overlay::cursor_screen(&peer(Some([f64::NAN, 0.0])), &xf).is_none());
    // A rotated view: screen → document → screen comes back.
    let xf = Xf { rot: 0.7, ..xf };
    let p = Point::new(37.5, -12.0);
    let at = overlay::cursor_screen(&peer(Some([p.x, p.y])), &xf).unwrap();
    let back = xf.to_doc(at);
    assert!((back.x - p.x).abs() < 1e-3 && (back.y - p.y).abs() < 1e-3, "{back:?}");
}

#[test]
fn canvas_draws_the_others() {
    let ctx = egui::Context::default();
    let mut relay = Relay::new();
    let mut a = app_with(&relay);
    let mut b = app_with(&relay);
    a.run("file.new", json!({})).unwrap();
    a.run("shape.rectangle", json!({"x": 10, "y": 10, "width": 50, "height": 20})).unwrap();
    a.run("collab.join", json!({"room": "r3", "server": "ws://h"})).unwrap();
    relay.pump(&mut [&mut a], &ctx);
    b.run("collab.join", json!({"room": "r3", "server": "ws://h", "name": "Bo"})).unwrap();
    relay.pump(&mut [&mut a, &mut b], &ctx);
    b.hover_doc = Some(Point::new(20.0, 15.0));
    b.run("select.all", json!({})).unwrap();
    relay.pump(&mut [&mut a, &mut b], &ctx);
    // A full frame with the canvas, the status strip and B's pointer and selection.
    let input = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))), ..Default::default() };
    let mut texts = vec![];
    for _ in 0..3 {
        let mut out = ctx.run_ui(input(), |ui| {
            a.logic(ui.ctx());
            a.ui(ui);
        });
        out.textures_delta.clear();
        texts = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>();
    }
    assert!(!a.ui.status.contains("Internal error"), "{}", a.ui.status);
    assert_eq!(peers(&a)[0].cursor, Some([20.0, 15.0]));
    // B's name tag is on the canvas, the status strip says the session is live.
    assert!(texts.iter().any(|t| t == "Bo"), "{texts:?}");
    assert!(texts.iter().any(|t| t == "Live"), "{texts:?}");
}

/// Two apps through a real `vectorcraft-collab-server` and the platform socket. Runs when
/// `VECTORCRAFT_COLLAB_SERVER` names one (e.g. `ws://127.0.0.1:1234`), else passes at once.
#[test]
fn live_server_round_trip() {
    let Ok(server) = std::env::var("VECTORCRAFT_COLLAB_SERVER") else { return };
    let ctx = egui::Context::default();
    let room = format!("uitest-{}", random_room());
    let mut a = VectorcraftApp::new(Session::new(), crate::Services::default());
    let mut b = VectorcraftApp::new(Session::new(), crate::Services::default());
    a.run("file.new", json!({})).unwrap();
    a.run("shape.rectangle", json!({"x": 10, "y": 10, "width": 50, "height": 20})).unwrap();
    a.run("collab.join", json!({"room": room, "server": server, "name": "Ann"})).unwrap();
    let until = |apps: &mut [&mut VectorcraftApp], done: &dyn Fn(&[&mut VectorcraftApp]) -> bool| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while std::time::Instant::now() < deadline {
            for app in apps.iter_mut() {
                frame(app, &ctx);
            }
            if done(apps) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    };
    assert!(until(&mut [&mut a], &|x| x[0].collab.session.as_ref().is_some_and(|s| s.status == Status::Live)), "A never went live");
    b.run("collab.join", json!({"room": room, "server": server, "name": "Bo"})).unwrap();
    assert!(until(&mut [&mut a, &mut b], &|x| paths(x[1]) == 1 && peers(x[0]).len() == 1), "B never got A's document");
    b.run("shape.rectangle", json!({"x": 100, "y": 10, "width": 50, "height": 20})).unwrap();
    assert!(until(&mut [&mut a, &mut b], &|x| paths(x[0]) == 2), "A never got B's rectangle");
    a.hover_doc = Some(Point::new(7.0, 8.0));
    assert!(until(&mut [&mut a, &mut b], &|x| peers(x[1]).first().is_some_and(|p| p.cursor == Some([7.0, 8.0]))), "B never saw A's pointer");
    a.run("collab.leave", json!({})).unwrap();
    // The server drops A's presence when its socket closes.
    assert!(until(&mut [&mut b], &|x| peers(x[0]).is_empty()), "B still sees A");
}
