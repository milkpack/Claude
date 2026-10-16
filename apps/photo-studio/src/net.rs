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
        .no_proxy()
        .connect_timeout(Duration::from_secs(15))
        .redirect(policy)
        .user_agent(concat!("photo-studio/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("HTTP client: {e}"))
}

/// Check an outbound proxy URL: `http://`, `https://`, `socks5://` or `socks5h://`, a host, an
/// optional port and `user:pass`, nothing else.
pub fn check_proxy_url(url: &str) -> Result<String, String> {
    let u = reqwest::Url::parse(url.trim()).map_err(|_| "the proxy is not a URL (e.g. socks5h://user:pass@host:1080)".to_string())?;
    let scheme_ok = matches!(u.scheme(), "http" | "https" | "socks5" | "socks5h");
    if !scheme_ok || u.host_str().is_none_or(str::is_empty) || !matches!(u.path(), "" | "/") || u.query().is_some() || u.fragment().is_some() {
        return Err("the proxy must be http://, https://, socks5:// or socks5h:// with a host (and optional user:pass and port)".into());
    }
    Ok(u.to_string().trim_end_matches('/').to_string())
}

/// A proxy URL without its credentials, for display: `socks5h://host:1080`.
pub fn mask_proxy(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(u) => {
            let port = u.port().map(|p| format!(":{p}")).unwrap_or_default();
            format!("{}://{}{port}", u.scheme(), u.host_str().unwrap_or(""))
        }
        Err(_) => "(invalid proxy)".into(),
    }
}

/// HTTP for Anthropic (the Claude API and routine fires): the only traffic that may go through a
/// proxy, because api.anthropic.com is unreachable from some countries. The proxy saved in the
/// cloud-run settings wins over `PHOTO_STUDIO_ANTHROPIC_PROXY`. Without one, no proxy is used
/// (environment proxy variables are ignored).
pub struct AnthropicHttp {
    env_proxy: Option<String>,
    settings_proxy: std::sync::Mutex<Option<String>>,
    cache: std::sync::Mutex<Option<(Option<String>, reqwest::Client)>>,
}

impl AnthropicHttp {
    pub fn new(env_proxy: Option<String>) -> Result<AnthropicHttp, String> {
        let env_proxy = env_proxy.map(|p| check_proxy_url(&p)).transpose()?;
        Ok(AnthropicHttp { env_proxy, settings_proxy: std::sync::Mutex::new(None), cache: std::sync::Mutex::new(None) })
    }

    pub fn set_settings_proxy(&self, proxy: Option<String>) {
        *crate::store::lock(&self.settings_proxy) = proxy;
    }

    /// The proxy in use and where it comes from ("settings" or "env").
    pub fn proxy(&self) -> Option<(String, &'static str)> {
        if let Some(p) = crate::store::lock(&self.settings_proxy).clone() {
            return Some((p, "settings"));
        }
        self.env_proxy.clone().map(|p| (p, "env"))
    }

    /// The client for the current proxy (rebuilt when it changes).
    pub fn client(&self) -> Result<reqwest::Client, String> {
        let proxy = self.proxy().map(|(p, _)| p);
        let mut cache = crate::store::lock(&self.cache);
        if let Some((p, c)) = cache.as_ref()
            && *p == proxy
        {
            return Ok(c.clone());
        }
        let mut b = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .user_agent(concat!("photo-studio/", env!("CARGO_PKG_VERSION")));
        if let Some(p) = &proxy {
            b = b.proxy(reqwest::Proxy::all(p).map_err(|_| format!("invalid Anthropic proxy {}", mask_proxy(p)))?);
        }
        let c = b.build().map_err(|e| format!("HTTP client: {e}"))?;
        *cache = Some((proxy, c.clone()));
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxies() {
        assert_eq!(check_proxy_url("socks5h://u:p@proxy.example:1080").unwrap(), "socks5h://u:p@proxy.example:1080");
        assert_eq!(check_proxy_url("http://10.0.0.1:3128/").unwrap(), "http://10.0.0.1:3128");
        for bad in ["ftp://x:1", "socks5h://", "http://x/path", "nope", "http://x?q=1"] {
            assert!(check_proxy_url(bad).is_err(), "{bad}");
        }
        assert_eq!(mask_proxy("socks5h://user:secret@proxy.example:1080"), "socks5h://proxy.example:1080");
        let a = AnthropicHttp::new(Some("http://env.example:1".into())).unwrap();
        assert_eq!(a.proxy(), Some(("http://env.example:1".to_string(), "env")));
        a.set_settings_proxy(Some("socks5h://s.example:2".into()));
        assert_eq!(a.proxy().unwrap().1, "settings");
        assert!(a.client().is_ok());
        assert!(AnthropicHttp::new(Some("bad".into())).is_err());
    }

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
