//! Serving a built web app (`--static DIR`): files by path, `index.html` for app routes.
//!
//! Paths are percent-decoded and taken apart segment by segment: `..`, hidden (`.`-prefixed)
//! segments, backslashes, drive letters and NULs are refused, and the resolved file must still be
//! inside the (canonicalized) root, so neither `../` tricks nor symlinks reach outside it.

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};

/// Don't serve files larger than this (a web bundle is far smaller).
const MAX_STATIC_BYTES: u64 = 256 << 20;

/// `uri_path` (still percent-encoded) as a relative path, `None` when it's not acceptable.
pub fn sanitize(uri_path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(uri_path)?;
    let mut out = PathBuf::new();
    for seg in decoded.split('/') {
        if seg.is_empty() {
            continue;
        }
        if seg.starts_with('.') || seg.contains(['\\', '\0', ':']) {
            return None;
        }
        out.push(seg);
    }
    Some(out)
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hex = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn content_type(path: &Path) -> &'static str {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "wasm" => "application/wasm",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "txt" => "text/plain; charset=utf-8",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "webmanifest" => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

/// Serve `uri_path` from `root` (already canonicalized).
pub async fn serve(root: &Path, method: &Method, uri_path: &str) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return (StatusCode::METHOD_NOT_ALLOWED, "method not allowed").into_response();
    }
    let Some(rel) = sanitize(uri_path) else { return not_found() };
    let candidate = root.join(&rel);
    let file = match find(root, &candidate).await {
        Some(f) => f,
        // An app route (no file extension): let the single-page app handle it.
        None if rel.extension().is_none() => match find(root, &root.join("index.html")).await {
            Some(f) => f,
            None => return not_found(),
        },
        None => return not_found(),
    };
    let body = match tokio::fs::read(&file).await {
        Ok(b) => b,
        Err(_) => return not_found(),
    };
    let mut resp = if method == Method::HEAD { Response::new(Body::empty()) } else { Response::new(Body::from(body)) };
    let headers = resp.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type(&file)));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    let cache = if file.file_name().is_some_and(|n| n == "index.html") { "no-cache" } else { "public, max-age=300" };
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    resp
}

/// `path` (or `path/index.html` for a directory) if it is a regular file inside `root`.
async fn find(root: &Path, path: &Path) -> Option<PathBuf> {
    let mut real = tokio::fs::canonicalize(path).await.ok()?;
    if !real.starts_with(root) {
        return None;
    }
    let mut meta = tokio::fs::metadata(&real).await.ok()?;
    if meta.is_dir() {
        real = tokio::fs::canonicalize(real.join("index.html")).await.ok()?;
        if !real.starts_with(root) {
            return None;
        }
        meta = tokio::fs::metadata(&real).await.ok()?;
    }
    (meta.is_file() && meta.len() <= MAX_STATIC_BYTES).then_some(real)
}
