//! Command-line options, with `PHOTO_STUDIO_*` environment variables as fallbacks. Secrets (the
//! owner's password, the Anthropic API key) come from the environment only, never from flags,
//! so they don't show up in process listings.

use std::path::PathBuf;

pub const USAGE: &str = "\
photo-studio: a private node canvas for photographic image generation (OpenArt + Claude)

USAGE:
    photo-studio [OPTIONS]

OPTIONS:
    --host <HOST>               Address to bind [env: PHOTO_STUDIO_HOST] [default: 127.0.0.1]
    --port <PORT>               Port to listen on [env: PHOTO_STUDIO_PORT] [default: 8787]
    --data <DIR>                Data directory [env: PHOTO_STUDIO_DATA] [default: ./photo-studio-data]
    --public-url <URL>          External base URL: OAuth redirect and Secure cookies
                                [env: PHOTO_STUDIO_PUBLIC_URL] [default: http://HOST:PORT]
    --static <DIR>              Built front-end [env: PHOTO_STUDIO_STATIC] [default: web/dist]
    --skill <DIR>               Photo skill directory (SKILL.md + references/) instead of the
                                copy compiled into the binary [env: PHOTO_STUDIO_SKILL]
    --openart-mcp-url <URL>     OpenArt MCP endpoint [env: PHOTO_STUDIO_OPENART_MCP_URL]
                                [default: https://mcp.openart.ai/mcp]
    --openart-issuer <URL>      OAuth authorization server, skipping protected-resource discovery
                                [env: PHOTO_STUDIO_OPENART_ISSUER]
    --anthropic-base-url <URL>  Anthropic API [env: ANTHROPIC_BASE_URL] [default: https://api.anthropic.com]
    --anthropic-proxy <URL>     Proxy for Anthropic traffic only (Claude API, cloud runs), for
                                hosts where api.anthropic.com is blocked: http://, https:// or
                                socks5h://[user:pass@]host:port [env: PHOTO_STUDIO_ANTHROPIC_PROXY]
    --trust-proxy               Take the client address for the login rate limit from the last
                                X-Forwarded-For entry (only behind your own reverse proxy)
                                [env: PHOTO_STUDIO_TRUST_PROXY=1]
    -h, --help                  Print this help
    -V, --version               Print the version

ENVIRONMENT:
    PHOTO_STUDIO_PASSWORD       The owner's password (required, at least 12 characters)
    ANTHROPIC_API_KEY           Enables the director and review nodes
    PHOTO_STUDIO_CLAUDE_MODEL   Claude model [default: claude-opus-5-5]
    PHOTO_STUDIO_CLAUDE_EFFORT  low | medium | high | xhigh | max [default: medium]
    PHOTO_STUDIO_ROUTINE_URL    Cloud runs: the routine's API trigger URL (with ..._TOKEN)
    PHOTO_STUDIO_ROUTINE_TOKEN  Cloud runs: the routine's bearer token
    RUST_LOG                    Log level [default: info]";

pub const DEFAULT_MCP_URL: &str = "https://mcp.openart.ai/mcp";
pub const DEFAULT_ANTHROPIC_URL: &str = "https://api.anthropic.com";
pub const DEFAULT_CLAUDE_MODEL: &str = "claude-opus-5-5";
pub const DEFAULT_CLAUDE_EFFORT: &str = "medium";

#[derive(Clone, PartialEq)]
pub struct Cli {
    pub host: String,
    pub port: u16,
    pub data_dir: PathBuf,
    pub public_url: String,
    pub static_dir: PathBuf,
    pub skill_dir: Option<PathBuf>,
    pub password: String,
    pub anthropic_api_key: Option<String>,
    pub claude_model: String,
    pub claude_effort: String,
    pub openart_mcp_url: String,
    pub openart_issuer: Option<String>,
    pub anthropic_base_url: String,
    pub trust_proxy: bool,
    /// Cloud-run routine (URL, token) from the environment.
    pub routine: Option<(String, String)>,
    /// Outbound proxy for Anthropic traffic only.
    pub anthropic_proxy: Option<String>,
}

impl std::fmt::Debug for Cli {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cli")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("data_dir", &self.data_dir)
            .field("public_url", &self.public_url)
            .field("static_dir", &self.static_dir)
            .field("skill_dir", &self.skill_dir)
            .field("password", &"<redacted>")
            .field("anthropic_api_key", &self.anthropic_api_key.as_ref().map(|_| "<redacted>"))
            .field("claude_model", &self.claude_model)
            .field("claude_effort", &self.claude_effort)
            .field("openart_mcp_url", &self.openart_mcp_url)
            .field("openart_issuer", &self.openart_issuer)
            .field("anthropic_base_url", &self.anthropic_base_url)
            .field("trust_proxy", &self.trust_proxy)
            .field("routine", &self.routine.as_ref().map(|(u, _)| crate::routine::mask_url(u)))
            .field("anthropic_proxy", &self.anthropic_proxy.as_deref().map(crate::net::mask_proxy))
            .finish()
    }
}

#[derive(Debug, PartialEq)]
pub enum CliError {
    Help,
    Version,
    Bad(String),
}

const VALUE_FLAGS: &[&str] =
    &["--host", "--port", "--data", "--public-url", "--static", "--skill", "--openart-mcp-url", "--openart-issuer", "--anthropic-base-url"];

impl Cli {
    /// Parse `args` (without the program name); `env` looks up environment variables.
    pub fn parse(args: impl IntoIterator<Item = String>, env: impl Fn(&str) -> Option<String>) -> Result<Cli, CliError> {
        let env = |k: &str| env(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let mut flags: std::collections::HashMap<&'static str, String> = Default::default();
        let mut trust_proxy = false;
        let mut it = args.into_iter();
        while let Some(arg) = it.next() {
            let (flag, inline) = match arg.split_once('=') {
                Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
                _ => (arg.clone(), None),
            };
            match flag.as_str() {
                "-h" | "--help" => return Err(CliError::Help),
                "-V" | "--version" => return Err(CliError::Version),
                "--trust-proxy" => trust_proxy = true,
                f => {
                    let Some(known) = VALUE_FLAGS.iter().find(|k| **k == f) else {
                        return Err(CliError::Bad(format!("unknown argument `{arg}` (see --help)")));
                    };
                    let value = match inline {
                        Some(v) => v,
                        None => it.next().ok_or_else(|| CliError::Bad(format!("{flag} needs a value")))?,
                    };
                    flags.insert(known, value);
                }
            }
        }
        let pick = |flag: &str, var: &str| flags.get(flag).cloned().or_else(|| env(var));

        let host = pick("--host", "PHOTO_STUDIO_HOST").unwrap_or_else(|| "127.0.0.1".into());
        let port = match pick("--port", "PHOTO_STUDIO_PORT") {
            Some(p) => p.trim().parse::<u16>().map_err(|_| CliError::Bad(format!("invalid port `{p}`")))?,
            None => 8787,
        };
        let data_dir = PathBuf::from(pick("--data", "PHOTO_STUDIO_DATA").unwrap_or_else(|| "./photo-studio-data".into()));
        let public_url = match pick("--public-url", "PHOTO_STUDIO_PUBLIC_URL") {
            Some(u) => http_url("--public-url", &u)?,
            None => {
                let h = if host == "0.0.0.0" || host == "::" || host == "[::]" { "localhost" } else { host.as_str() };
                format!("http://{h}:{port}")
            }
        };
        let static_dir = PathBuf::from(pick("--static", "PHOTO_STUDIO_STATIC").unwrap_or_else(|| "web/dist".into()));
        let skill_dir = pick("--skill", "PHOTO_STUDIO_SKILL").map(PathBuf::from);
        let openart_mcp_url =
            http_url("--openart-mcp-url", &pick("--openart-mcp-url", "PHOTO_STUDIO_OPENART_MCP_URL").unwrap_or_else(|| DEFAULT_MCP_URL.into()))?;
        let openart_issuer = pick("--openart-issuer", "PHOTO_STUDIO_OPENART_ISSUER").map(|u| http_url("--openart-issuer", &u)).transpose()?;
        let anthropic_base_url =
            http_url("--anthropic-base-url", &pick("--anthropic-base-url", "ANTHROPIC_BASE_URL").unwrap_or_else(|| DEFAULT_ANTHROPIC_URL.into()))?;
        let anthropic_proxy = pick("--anthropic-proxy", "PHOTO_STUDIO_ANTHROPIC_PROXY")
            .map(|p| crate::net::check_proxy_url(&p).map_err(|e| CliError::Bad(format!("--anthropic-proxy: {e}"))))
            .transpose()?;
        let trust_proxy = trust_proxy || env("PHOTO_STUDIO_TRUST_PROXY").is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));

        // Secrets: environment only, and never trimmed beyond surrounding whitespace.
        let password =
            env("PHOTO_STUDIO_PASSWORD").ok_or_else(|| CliError::Bad("PHOTO_STUDIO_PASSWORD is required (the owner's password)".into()))?;
        if password.chars().count() < crate::server::MIN_PASSWORD_CHARS {
            return Err(CliError::Bad(format!("PHOTO_STUDIO_PASSWORD must have at least {} characters", crate::server::MIN_PASSWORD_CHARS)));
        }
        let anthropic_api_key = env("ANTHROPIC_API_KEY");
        let routine = match (env("PHOTO_STUDIO_ROUTINE_URL"), env("PHOTO_STUDIO_ROUTINE_TOKEN")) {
            (Some(u), Some(t)) => Some((u, t)),
            (None, None) => None,
            _ => return Err(CliError::Bad("set both PHOTO_STUDIO_ROUTINE_URL and PHOTO_STUDIO_ROUTINE_TOKEN, or neither".into())),
        };
        let claude_model = env("PHOTO_STUDIO_CLAUDE_MODEL").unwrap_or_else(|| DEFAULT_CLAUDE_MODEL.into());
        let claude_effort = env("PHOTO_STUDIO_CLAUDE_EFFORT").unwrap_or_else(|| DEFAULT_CLAUDE_EFFORT.into());
        if !["low", "medium", "high", "xhigh", "max"].contains(&claude_effort.as_str()) {
            return Err(CliError::Bad(format!("PHOTO_STUDIO_CLAUDE_EFFORT: expected low, medium, high, xhigh or max, got `{claude_effort}`")));
        }
        Ok(Cli {
            host,
            port,
            data_dir,
            public_url,
            static_dir,
            skill_dir,
            password,
            anthropic_api_key,
            claude_model,
            claude_effort,
            openart_mcp_url,
            openart_issuer,
            anthropic_base_url,
            trust_proxy,
            routine,
            anthropic_proxy,
        })
    }
}

/// `value` if it is an absolute http(s) URL, without a trailing slash.
fn http_url(flag: &str, value: &str) -> Result<String, CliError> {
    let v = value.trim().trim_end_matches('/');
    match reqwest::Url::parse(v) {
        Ok(u) if (u.scheme() == "http" || u.scheme() == "https") && u.host_str().is_some() => Ok(v.to_string()),
        _ => Err(CliError::Bad(format!("{flag}: expected an http(s) URL, got `{value}`"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str], env: &[(&str, &str)]) -> Result<Cli, CliError> {
        let env: Vec<(String, String)> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Cli::parse(args.iter().map(|s| s.to_string()), |k| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()))
    }

    #[test]
    fn defaults_need_a_password() {
        assert!(matches!(parse(&[], &[]), Err(CliError::Bad(m)) if m.contains("PHOTO_STUDIO_PASSWORD")));
        let c = parse(&[], &[("PHOTO_STUDIO_PASSWORD", "a long password")]).unwrap();
        assert_eq!((c.host.as_str(), c.port), ("127.0.0.1", 8787));
        assert_eq!(c.public_url, "http://127.0.0.1:8787");
        assert_eq!(c.data_dir, PathBuf::from("./photo-studio-data"));
        assert_eq!(c.static_dir, PathBuf::from("web/dist"));
        assert_eq!(c.claude_model, "claude-opus-5-5");
        assert_eq!(c.openart_mcp_url, DEFAULT_MCP_URL);
        assert_eq!(c.anthropic_base_url, DEFAULT_ANTHROPIC_URL);
        assert!(c.anthropic_api_key.is_none() && c.openart_issuer.is_none() && !c.trust_proxy);
        assert!(!format!("{c:?}").contains("a long password"), "the password is redacted");
        assert!(matches!(parse(&[], &[("PHOTO_STUDIO_PASSWORD", "short")]), Err(CliError::Bad(m)) if m.contains("12")));
    }

    #[test]
    fn flags_override_env() {
        let env = [
            ("PHOTO_STUDIO_PASSWORD", "a long password"),
            ("PHOTO_STUDIO_PORT", "9000"),
            ("PHOTO_STUDIO_HOST", "0.0.0.0"),
            ("ANTHROPIC_BASE_URL", "http://127.0.0.1:5/"),
            ("PHOTO_STUDIO_CLAUDE_MODEL", "claude-sonnet-5-5"),
            ("PHOTO_STUDIO_TRUST_PROXY", "1"),
        ];
        let c = parse(&["--port=9100", "--public-url", "https://studio.example/", "--skill", "/s"], &env).unwrap();
        assert_eq!(c.port, 9100);
        assert_eq!(c.host, "0.0.0.0");
        assert_eq!(c.public_url, "https://studio.example");
        assert_eq!(c.skill_dir, Some(PathBuf::from("/s")));
        assert_eq!(c.anthropic_base_url, "http://127.0.0.1:5");
        assert_eq!(c.claude_model, "claude-sonnet-5-5");
        assert!(c.trust_proxy);
        let c = parse(&[], &env).unwrap();
        assert_eq!(c.public_url, "http://localhost:9000");
    }

    #[test]
    fn bad_input() {
        let env = [("PHOTO_STUDIO_PASSWORD", "a long password")];
        assert_eq!(parse(&["--help"], &[]), Err(CliError::Help));
        assert_eq!(parse(&["-V"], &[]), Err(CliError::Version));
        assert!(matches!(parse(&["--port", "x"], &env), Err(CliError::Bad(_))));
        assert!(matches!(parse(&["--port"], &env), Err(CliError::Bad(_))));
        assert!(matches!(parse(&["--nope"], &env), Err(CliError::Bad(_))));
        assert!(matches!(parse(&["--public-url", "ftp://x"], &env), Err(CliError::Bad(_))));
        assert!(matches!(parse(&[], &[("PHOTO_STUDIO_PASSWORD", "a long password"), ("PHOTO_STUDIO_CLAUDE_EFFORT", "huge")]), Err(CliError::Bad(_))));
    }
}
