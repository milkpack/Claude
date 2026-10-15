//! Outbound HTTP helpers: size-capped body reads and error text without URLs (presigned URLs
//! carry signatures, so they never reach logs or error messages).

use serde_json::Value;

/// A reqwest error without its URL.
pub fn neterr(e: reqwest::Error) -> String {
    let e = e.without_url();
    if e.is_timeout() { format!("timed out ({e})") } else { e.to_string() }
}

/// Read the whole body, failing if it is larger than `max` bytes.
pub async fn read_limited(mut resp: reqwest::Response, max: usize) -> Result<Vec<u8>, String> {
    if let Some(len) = resp.content_length()
        && len > max as u64
    {
        return Err(format!("response of {len} bytes is over the {max}-byte limit"));
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(neterr)? {
        if buf.len() + chunk.len() > max {
            return Err(format!("response is over the {max}-byte limit"));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// GET `url` and parse a JSON body of at most `max` bytes. Non-2xx is an error.
pub async fn get_json(http: &reqwest::Client, url: &str, max: usize) -> Result<Value, String> {
    let resp = http.get(url).header(reqwest::header::ACCEPT, "application/json").send().await.map_err(neterr)?;
    let status = resp.status();
    let body = read_limited(resp, max).await?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {}", snippet(&body)));
    }
    serde_json::from_slice(&body).map_err(|e| format!("invalid JSON: {e}"))
}

/// The start of a response body for an error message: one line, at most 300 characters.
pub fn snippet(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let mut s: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).take(300).collect();
    if text.chars().count() > 300 {
        s.push('…');
    }
    s.trim().to_string()
}

/// `true` if `url` is an absolute http(s) URL.
pub fn is_http_url(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| (u.scheme() == "https" || u.scheme() == "http") && u.host_str().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets() {
        assert_eq!(snippet(b"  bad\nthing  "), "bad thing");
        assert_eq!(snippet("x".repeat(400).as_bytes()).chars().count(), 301);
        assert!(is_http_url("https://cdn.openart.ai/a.png"));
        assert!(!is_http_url("file:///etc/passwd"));
        assert!(!is_http_url("nope"));
    }
}
