//! Collaboration addresses: the server's base URL (`ws://host:port`), the socket's
//! (`…/collab/<room>`), invite links (`https://app.example/?room=<id>[&server=…]`) and reading
//! them back.

/// The desktop app's default server (the collaboration server's default port, on this machine).
pub const NATIVE_DEFAULT_SERVER: &str = "ws://localhost:1234";

/// The longest room id.
const ROOM_MAX: usize = 64;

/// The page the web app runs in (`https://host/path`, without query); none on desktop.
pub fn page_url() -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        let loc = web_sys::window()?.location();
        Some(format!("{}{}", loc.origin().ok()?, loc.pathname().ok()?))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

/// The default server: the page's own host on the web (`wss` for an `https` page), else
/// [`NATIVE_DEFAULT_SERVER`].
pub fn default_server(page: Option<&str>) -> String {
    page.and_then(|p| {
        let (scheme, rest) = p.split_once("://")?;
        let host = rest.split(['/', '?', '#']).next().filter(|h| !h.is_empty())?;
        let ws = if scheme.eq_ignore_ascii_case("https") { "wss" } else { "ws" };
        Some(format!("{ws}://{host}"))
    })
    .unwrap_or_else(|| NATIVE_DEFAULT_SERVER.to_string())
}

/// A server address as typed → its base URL: `ws://`/`wss://` (http(s) are mapped to them, no
/// scheme means `ws://`), without a trailing `/` or `/collab`.
pub fn normalize_server(s: &str) -> Result<String, String> {
    let s = s.trim();
    let (scheme, rest) = match s.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        None => ("ws".to_string(), s),
    };
    let scheme = match scheme.as_str() {
        "ws" | "http" => "ws",
        "wss" | "https" => "wss",
        _ => return Err(format!("the server address must start with ws:// or wss:// (got {s})")),
    };
    let mut rest = rest.split(['?', '#']).next().unwrap_or_default().trim_end_matches('/');
    if let Some(r) = rest.strip_suffix("/collab") {
        rest = r.trim_end_matches('/');
    }
    let host = rest.split('/').next().unwrap_or_default();
    if host.is_empty() || host.chars().any(|c| c.is_whitespace() || c == '@') {
        return Err(format!("no server host in {s}"));
    }
    Ok(format!("{scheme}://{rest}"))
}

/// A room id as typed → the id (1–64 ASCII letters, digits, `-` and `_`, as the server takes).
pub fn validate_room(s: &str) -> Result<String, String> {
    let r = s.trim();
    if r.is_empty() {
        return Err("the room needs a name".into());
    }
    if r.chars().count() > ROOM_MAX {
        return Err(format!("a room name has at most {ROOM_MAX} characters"));
    }
    if let Some(c) = r.chars().find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))) {
        return Err(format!("a room name can't contain {c:?} (letters, digits, - and _ only)"));
    }
    Ok(r.to_string())
}

/// The socket's URL for a room on a server (a base URL from [`normalize_server`]).
pub fn socket_url(server: &str, room: &str) -> String {
    format!("{}/collab/{room}", server.trim_end_matches('/'))
}

/// The link that opens the web app in the room: the web app's own page, else the page the server
/// serves (`http(s)://` of its host). `&server=` is added only when it isn't the page's default.
pub fn invite_link(server: &str, room: &str, page: Option<&str>) -> String {
    let page = page.map(str::to_string).unwrap_or_else(|| {
        let (scheme, rest) = server.split_once("://").unwrap_or(("ws", server));
        let http = if scheme == "wss" { "https" } else { "http" };
        format!("{http}://{}/", rest.split('/').next().unwrap_or_default())
    });
    let mut link = format!("{page}?room={}", encode(room));
    if default_server(Some(&page)) != server {
        link.push_str("&server=");
        link.push_str(&encode(server));
    }
    link
}

/// Read what the user pasted as a room: an invite link (`…?room=id[&server=…]`), a socket URL
/// (`ws://host/collab/id`) or a bare room id → (the server it names, the room).
pub fn parse_invite(s: &str) -> Option<(Option<String>, String)> {
    let s = s.trim();
    let Some((scheme, rest)) = s.split_once("://") else {
        return validate_room(s).ok().map(|r| (None, r));
    };
    let (before_query, query) = rest.split_once('?').unwrap_or((rest, ""));
    let query = query.split('#').next().unwrap_or_default();
    let host = before_query.split('/').next().filter(|h| !h.is_empty())?;
    let own_server = normalize_server(&format!("{scheme}://{host}")).ok();
    if let Some((server, room)) = query_invite(query) {
        return Some((server.or(own_server), room));
    }
    let (base, room) = before_query.split_once("/collab/")?;
    let room = validate_room(room.trim_end_matches('/')).ok()?;
    Some((normalize_server(&format!("{scheme}://{base}")).ok(), room))
}

/// The room (and server) a page URL's query names: `?room=…&server=…`.
pub fn query_invite(query: &str) -> Option<(Option<String>, String)> {
    let query = query.trim_start_matches('?');
    let get = |key: &str| query.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| decode(v)));
    let room = validate_room(&get("room")?).ok()?;
    let server = get("server").and_then(|s| normalize_server(&s).ok());
    Some((server, room))
}

/// Percent-encode everything but the unreserved characters.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Undo [`encode`] (and `+` for a space); a malformed escape stays as it is.
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b, hex) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_server_follows_the_page() {
        assert_eq!(default_server(Some("https://draw.example.com/app/")), "wss://draw.example.com");
        assert_eq!(default_server(Some("http://localhost:8080/index.html")), "ws://localhost:8080");
        assert_eq!(default_server(None), NATIVE_DEFAULT_SERVER);
        assert_eq!(default_server(Some("nonsense")), NATIVE_DEFAULT_SERVER);
    }

    #[test]
    fn servers_normalize() {
        assert_eq!(normalize_server("localhost:1234").unwrap(), "ws://localhost:1234");
        assert_eq!(normalize_server(" wss://x.org/collab/ ").unwrap(), "wss://x.org");
        assert_eq!(normalize_server("https://x.org/base/").unwrap(), "wss://x.org/base");
        assert_eq!(normalize_server("http://10.0.0.2:1234").unwrap(), "ws://10.0.0.2:1234");
        assert!(normalize_server("ftp://x.org").is_err());
        assert!(normalize_server("ws://").is_err());
        assert!(normalize_server("").is_err());
    }

    #[test]
    fn rooms_validate() {
        assert_eq!(validate_room(" team-1_b ").unwrap(), "team-1_b");
        assert!(validate_room("a.b").is_err());
        assert!(validate_room("").is_err());
        assert!(validate_room("a/b").is_err());
        assert!(validate_room("a b").is_err());
        assert!(validate_room(&"x".repeat(65)).is_err());
        assert_eq!(socket_url("ws://h:1234", "r1"), "ws://h:1234/collab/r1");
    }

    #[test]
    fn invite_links_round_trip() {
        // Web: the page's own server is implied.
        let link = invite_link("wss://draw.example.com", "abc123", Some("https://draw.example.com/"));
        assert_eq!(link, "https://draw.example.com/?room=abc123");
        assert_eq!(parse_invite(&link), Some((Some("wss://draw.example.com".into()), "abc123".into())));
        // Another server is spelled out.
        let link = invite_link("ws://10.0.0.2:1234", "r", Some("https://draw.example.com/app/"));
        assert_eq!(link, "https://draw.example.com/app/?room=r&server=ws%3A%2F%2F10.0.0.2%3A1234");
        assert_eq!(parse_invite(&link), Some((Some("ws://10.0.0.2:1234".into()), "r".into())));
        // Desktop: the server's own page.
        let link = invite_link("ws://localhost:1234", "r", None);
        assert_eq!(link, "http://localhost:1234/?room=r");
        assert_eq!(parse_invite(&link), Some((Some("ws://localhost:1234".into()), "r".into())));
        // A socket URL and a bare id.
        assert_eq!(parse_invite("ws://h:1234/collab/xyz"), Some((Some("ws://h:1234".into()), "xyz".into())));
        assert_eq!(parse_invite("xyz"), Some((None, "xyz".into())));
        assert_eq!(parse_invite("not a room"), None);
        assert_eq!(parse_invite("https://h/?other=1"), None);
    }

    #[test]
    fn page_queries() {
        assert_eq!(query_invite("?room=abc"), Some((None, "abc".into())));
        assert_eq!(query_invite("?webgl&room=abc&server=wss%3A%2F%2Fx.org"), Some((Some("wss://x.org".into()), "abc".into())));
        assert_eq!(query_invite("?webgl"), None);
        assert_eq!(query_invite("?room=bad%20id"), None);
        assert_eq!(decode("%zz%41+"), "%zzA ");
        assert_eq!(decode("%E2%9C%93"), "✓");
    }
}
