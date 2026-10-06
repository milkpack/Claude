//! VectorCraft in the browser.
//!
//! Runs the same [`vectorcraft_ui_egui::VectorcraftApp`] as the desktop app through eframe's web
//! runner (wgpu: WebGPU where available, WebGL2 otherwise). Build with `trunk build --release`
//! from this directory; see `docs/development.md` ("Web build").
//!
//! Differences from the desktop app:
//! - no TCP control server (browsers can't listen on sockets);
//! - File → Open uses the browser file picker; bytes arrive asynchronously through
//!   `Services::inbox`;
//! - Save / Export trigger a browser download;
//! - dropped files are read asynchronously by `web::WebShell` and delivered through the inbox.
//!
//! URL query flags: `?webgl` forces the WebGL2 backend instead of WebGPU; `?room=<id>` joins that
//! collaboration room (File → Share / Collaborate…) on the page's own host
//! (`ws(s)://<host>/collab/<id>`), or on the server `&server=<ws(s) URL>` names.

#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(target_arch = "wasm32")]
fn main() {
    web::start();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("vectorcraft-web only runs in the browser: build it with `trunk build --release` in apps/vectorcraft-web");
}
