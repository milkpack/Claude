//! Outbound HTTP helpers: size-capped body reads and error text without URLs (presigned URLs
//! carry signatures, so they never reach logs or error messages).

use std::net::IpAddr;
use std::time::Duration;

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

/// Whether `ip` is a public unicast address: not loopback, private, link-local, shared (CGNAT),
/// unique-local, multicast, documentation, benchmarking or reserved.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                || a == 0
                || (a == 100 && (64..128).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 198 && (b == 18 || b == 19))
                || a >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let s0 = v6.segments().first().copied().unwrap_or(0);
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s0 & 0xfe00) == 0xfc00
                || (s0 & 0xffc0) == 0xfe80
                || (s0 == 0x2001 && v6.segments().get(1) == Some(&0x0db8))
                || v6.segments().iter().take(6).all(|x| *x == 0))
        }
    }
}

/// Check a URL that came from upstream (CDN downloads, presigned uploads) before fetching it:
/// https only, and its host must resolve to public addresses only. `allow_private` (tests
/// against local mocks) also accepts plain http and private addresses.
pub async fn check_fetch_url(url: &str, allow_private: bool) -> Result<(), String> {
    let u = reqwest::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;
    let scheme_ok = u.scheme() == "https" || (allow_private && u.scheme() == "http");
    if !scheme_ok {
        return Err("only https URLs are fetched".into());
    }
    if allow_private {
        return Ok(());
    }
    let host = u.host_str().ok_or("URL without a host")?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse::<IpAddr>() {
        return if is_public_ip(ip) { Ok(()) } else { Err("the URL points to a private address".into()) };
    }
    let port = u.port_or_known_default().unwrap_or(443);
    let addrs: Vec<IpAddr> = tokio::time::timeout(Duration::from_secs(10), tokio::net::lookup_host((host, port)))
        .await
        .map_err(|_| "DNS lookup timed out".to_string())?
        .map_err(|e| format!("DNS lookup failed: {e}"))?
        .map(|a| a.ip())
        .collect();
    if addrs.is_empty() || !addrs.iter().all(|ip| is_public_ip(*ip)) {
        return Err("the URL's host resolves to a private address".into());
    }
    Ok(())
}

/// A client for URLs that came from upstream: no credentials, at most 3 redirects, each one
/// re-checked (https, no private IP literals, no `localhost`).
pub fn fetch_client(allow_private: bool) -> Result<reqwest::Client, String> {
    let policy = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= 3 {
            return attempt.error("too many redirects");
        }
        let u = attempt.url();
        let host = u.host_str().unwrap_or("").trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
        let private_literal = host.parse::<IpAddr>().is_ok_and(|ip| !is_public_ip(ip));
        if allow_private || (u.scheme() == "https" && !private_literal && host != "localhost" && !host.ends_with(".localhost")) {
            attempt.follow()
        } else {
            attempt.stop()
        }
    });
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .redirect(policy)
        .user_agent(concat!("photo-studio/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("HTTP client: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn upstream_urls() {
        for bad in [
            "http://cdn.example.com/a.png",
            "https://127.0.0.1/a.png",
            "https://169.254.169.254/latest/meta-data",
            "https://10.1.2.3/x",
            "https://100.64.0.1/x",
            "https://[::1]/x",
            "https://[fd00::1]/x",
            "https://[::ffff:192.168.0.1]/x",
            "https://localhost/x",
            "file:///etc/passwd",
        ] {
            assert!(check_fetch_url(bad, false).await.is_err(), "{bad}");
        }
        assert!(check_fetch_url("https://8.8.8.8/x", false).await.is_ok());
        assert!(check_fetch_url("https://[2606:4700::1111]/x", false).await.is_ok());
        assert!(check_fetch_url("http://127.0.0.1:9/x", true).await.is_ok());
        assert!(check_fetch_url("ftp://127.0.0.1/x", true).await.is_err());
    }

    #[test]
    fn snippets() {
        assert_eq!(snippet(b"  bad\nthing  "), "bad thing");
        assert_eq!(snippet("x".repeat(400).as_bytes()).chars().count(), 301);
        assert!(is_http_url("https://cdn.openart.ai/a.png"));
        assert!(!is_http_url("file:///etc/passwd"));
        assert!(!is_http_url("nope"));
    }
}
