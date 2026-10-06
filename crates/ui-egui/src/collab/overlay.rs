//! What collaborating shows: the others' pointers and selections on the canvas, and the status
//! strip (connection state and who is here) in the status bar.

use egui::{Color32, Pos2, Shape, Stroke, vec2};
use vectorcraft_doc::NodeId;
use vectorcraft_engine::collab::Peer;

use super::Status;
use crate::VectorcraftApp;
use crate::canvas::Xf;
use crate::theme::Tokens;

/// A participant's colour.
pub fn peer_color(rgb: [u8; 3]) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

/// The pointer arrow's outline, tip at the origin (screen points): two triangles (it has a notch).
const ARROW: [[f32; 2]; 4] = [[0.0, 0.0], [0.0, 15.0], [4.2, 11.4], [10.6, 10.8]];

/// The others' pointers (where they are over the shared document) and selections, drawn over the
/// art of the shared document.
pub fn paint_peers(app: &VectorcraftApp, painter: &egui::Painter, xf: &Xf) {
    let Some(cs) = &app.collab.session else { return };
    let Some(st) = app.session.active().filter(|d| d.uid == cs.collab.doc_uid) else { return };
    let peers = cs.collab.peers();
    for (_, p) in &peers {
        let color = peer_color(p.color);
        for id in &p.selection {
            if let Some(b) = st.doc.node(NodeId(*id)).and_then(|n| n.geometric_bounds()) {
                painter.add(Shape::closed_line(xf.quad(b), Stroke::new(1.5, color)));
            }
        }
    }
    if app.ui.collab_hide_cursors {
        return;
    }
    let clip = painter.clip_rect();
    for (_, p) in &peers {
        if let Some(at) = cursor_screen(p, xf).filter(|at| clip.expand(4.0).contains(*at)) {
            paint_cursor(painter, at, peer_color(p.color), &p.name);
        }
    }
}

/// Where a participant's pointer is on screen (none while it is off their canvas).
pub fn cursor_screen(p: &Peer, xf: &Xf) -> Option<Pos2> {
    let [x, y] = p.cursor?;
    if !(x.is_finite() && y.is_finite()) {
        return None;
    }
    let at = xf.to_screen(vectorcraft_geom::Point::new(x, y));
    (at.x.is_finite() && at.y.is_finite()).then_some(at)
}

/// A pointer arrow in `color` with the name in a tag beside it.
fn paint_cursor(painter: &egui::Painter, at: Pos2, color: Color32, name: &str) {
    let pts: Vec<Pos2> = ARROW.iter().map(|[x, y]| at + vec2(*x, *y)).collect();
    let outline = Stroke::new(1.0, Color32::WHITE);
    if let [a, b, c, d] = pts.as_slice() {
        painter.add(Shape::convex_polygon(vec![*a, *b, *c], color, Stroke::NONE));
        painter.add(Shape::convex_polygon(vec![*a, *c, *d], color, Stroke::NONE));
    }
    painter.add(Shape::closed_line(pts, outline));
    let name: String = name.chars().take(32).collect();
    if name.is_empty() {
        return;
    }
    let galley = painter.layout_no_wrap(name, egui::FontId::proportional(11.0), Color32::WHITE);
    let tag = egui::Rect::from_min_size(at + vec2(10.0, 16.0), galley.size() + vec2(10.0, 4.0));
    painter.rect_filled(tag, 3.0, color);
    painter.galley(tag.min + vec2(5.0, 2.0), galley, Color32::WHITE);
}

/// The status bar's collaboration strip: the connection's state and a dot per participant (us
/// first), at the right end. Laid out right to left, so it adds its parts in reverse. Clicking it opens the dialog.
pub fn status_strip(app: &mut VectorcraftApp, ui: &mut egui::Ui) {
    let Some(cs) = &app.collab.session else { return };
    let t = Tokens::get(ui.ctx());
    let mut people = vec![(format!("{} (you)", cs.collab.name), peer_color(cs.collab.color()))];
    people.extend(cs.collab.peers().into_iter().map(|(_, p)| (p.name, peer_color(p.color))));
    let (state, tip) = (cs.status, cs.error.clone());
    let room = cs.room.clone();
    let dot_color = match state {
        Status::Live => t.accent,
        Status::Connecting | Status::Syncing => t.warning,
        Status::Reconnecting => t.error,
    };
    const SHOWN: usize = 5;
    let mut clicked = false;
    if people.len() > SHOWN {
        ui.label(egui::RichText::new(format!("+{}", people.len() - SHOWN)).size(11.0).color(t.text_dim));
    }
    for (name, color) in people.iter().take(SHOWN).rev() {
        clicked |= dot(ui, *color, 5.0).on_hover_text(name).clicked();
    }
    ui.add_space(4.0);
    let mut hover = format!("Room {room} · {} here", people.len());
    if let Some(e) = &tip {
        hover.push_str(&format!("\n{e}"));
    }
    let label = egui::Label::new(egui::RichText::new(state.label()).size(11.0).color(t.text)).sense(egui::Sense::click());
    clicked |= ui.add(label).on_hover_text(&hover).clicked();
    clicked |= dot(ui, dot_color, 3.5).on_hover_text(&hover).clicked();
    ui.separator();
    if clicked {
        let _ = app.run("collab.share", serde_json::json!({}));
    }
}

/// A filled dot of radius `r` in `color`.
pub(crate) fn dot(ui: &mut egui::Ui, color: Color32, r: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(2.0 * r + 2.0, 2.0 * r + 2.0), egui::Sense::click());
    ui.painter().circle_filled(rect.center(), r, color);
    resp
}
