//! File → Share / Collaborate…: share the active document with a room on a collaboration server,
//! or join someone else's room, and see who is here.
//!
//! Fields: `server`, `room` (an id, or a pasted invite link), `name` (shown to the others) and
//! `__generated` (the room id the dialog made up: keeping it shares the active document, another
//! room opens in a new document). Join runs `collab.join`; while collaborating the dialog shows
//! the invite link, who is here and Stop Collaborating instead.

use serde_json::{Value, json};

use super::{DialogSpec, form};
use crate::collab::{self, Status, overlay};
use crate::state::Dialog;
use crate::theme::Tokens;
use crate::{VectorcraftApp, widgets};

/// The dialog kind of Share / Collaborate.
pub const KIND: &str = "collab";

pub(super) const SPEC: DialogSpec = DialogSpec {
    heading: |_| "Share / Collaborate".into(),
    body,
    confirm,
    ok: Some("Join"),
    ok_label: Some(|app| if app.collab.session.is_some() { "Done" } else { "Join" }),
    min_width: 420.0,
    max_width: Some(520.0),
    ..DialogSpec::FORM
};

/// Open the dialog: the remembered server and name, a new room id.
pub fn open(app: &mut VectorcraftApp) {
    let room = app.collab.session.as_ref().map_or_else(collab::random_room, |s| s.room.clone());
    let fields = json!({
        "server": collab::preferred_server(app),
        "room": room,
        "name": collab::default_name(app),
        "__generated": room,
    });
    app.ui.dialog = Some(Dialog::new(KIND, fields));
}

/// Whether the dialog shares the active document (its own room id, a document open) or opens the
/// room in a new document.
fn shares_active(app: &VectorcraftApp, d: &Dialog) -> bool {
    app.session.active().is_some() && d.str("room").trim() == d.str("__generated")
}

/// The invite link for what the fields say (none while they don't make an address).
fn invite(d: &Dialog) -> Option<String> {
    let (server, room) = match collab::parse_invite(&d.str("room")) {
        Some((s, r)) => (s.map_or_else(|| collab::normalize_server(&d.str("server")), Ok).ok()?, r),
        None => return None,
    };
    Some(collab::invite_link(&server, &room, collab::urls::page_url().as_deref()))
}

/// The invite link with a Copy button.
fn invite_row(ui: &mut egui::Ui, link: Option<String>) {
    let t = Tokens::get(ui.ctx());
    form::caption(ui, "Invite link (anyone with it can edit):");
    ui.horizontal(|ui| {
        let shown = link.clone().unwrap_or_else(|| "—".into());
        let w = (ui.available_width() - 90.0).max(120.0);
        ui.add_sized([w, 20.0], egui::Label::new(egui::RichText::new(shown).monospace().size(11.0).color(t.text)).truncate());
        if let Some(l) = link
            && widgets::secondary_button(ui, "Copy").clicked()
        {
            ui.ctx().copy_text(l);
        }
    });
}

fn body(app: &mut VectorcraftApp, ui: &mut egui::Ui, d: &mut Dialog) -> bool {
    let t = Tokens::get(ui.ctx());
    if let Some(cs) = &app.collab.session {
        let (status, error, room, server) = (cs.status, cs.error.clone(), cs.room.clone(), cs.server.clone());
        let mut people = vec![(format!("{} (you)", cs.collab.name), cs.collab.color())];
        people.extend(cs.collab.peers().into_iter().map(|(_, p)| (p.name, p.color)));
        let link = collab::invite_link(&server, &room, collab::urls::page_url().as_deref());
        ui.horizontal(|ui| {
            let c = if status == Status::Live { t.accent } else { t.warning };
            overlay::dot(ui, c, 4.0);
            ui.label(egui::RichText::new(format!("{} — room {room} on {server}", status.label())).color(t.text));
        });
        if let Some(e) = error.filter(|_| status != Status::Live) {
            ui.label(egui::RichText::new(e).size(11.0).color(t.error));
        }
        ui.add_space(8.0);
        invite_row(ui, Some(link));
        ui.add_space(8.0);
        form::caption(ui, &format!("Here now ({}):", people.len()));
        for (name, color) in people {
            ui.horizontal(|ui| {
                overlay::dot(ui, overlay::peer_color(color), 5.0);
                ui.label(egui::RichText::new(name).color(t.text));
            });
        }
        ui.add_space(10.0);
        if widgets::secondary_button(ui, "Stop Collaborating").clicked() {
            let _ = app.run("collab.leave", json!({}));
            return true;
        }
        return false;
    }
    egui::Grid::new("collab-fields").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
        for (key, label) in [("server", "Server:"), ("room", "Room:"), ("name", "Your name:")] {
            ui.label(egui::RichText::new(label).color(t.text_dim));
            form::text(ui, d, key, 260.0);
            ui.end_row();
        }
    });
    ui.add_space(6.0);
    let what = if shares_active(app, d) {
        let title = app.session.active().map(|s| s.title()).unwrap_or_default();
        format!("Shares “{title}” in a new room: send the invite link to the others.")
    } else {
        "Opens the room in a new document. Paste an invite link into Room to join it.".into()
    };
    ui.label(egui::RichText::new(what).size(11.5).color(t.text_dim));
    ui.add_space(8.0);
    invite_row(ui, invite(d));
    false
}

fn confirm(app: &mut VectorcraftApp, d: &Dialog) -> Result<Value, String> {
    if app.collab.session.is_some() {
        app.ui.dialog = None;
        return Ok(Value::Null);
    }
    let params = json!({
        "server": d.str("server"),
        "room": d.str("room"),
        "name": d.str("name"),
        "newDocument": !shares_active(app, d),
    });
    // A bad address keeps the dialog open to fix it.
    let r = app.run("collab.join", params)?;
    app.ui.dialog = None;
    Ok(r)
}
