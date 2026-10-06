//! Command-line options (`--port`, `--host`, `--data`, `--static`, limits), with environment
//! variables as fallbacks (`PORT`, `HOST`, `DATA_DIR`, `STATIC_DIR`).

use std::path::PathBuf;

use crate::hub::Limits;

pub const USAGE: &str = "\
vectorcraft-collab-server: real-time co-editing relay for VectorCraft (Yjs sync + awareness)

USAGE:
    vectorcraft-collab-server [OPTIONS]

OPTIONS:
    --port <PORT>          Port to listen on [env: PORT] [default: 1234]
    --host <HOST>          Address to bind [env: HOST] [default: 0.0.0.0]
    --data <DIR>           Where rooms are stored as <room>.ydoc [env: DATA_DIR] [default: ./collab-data]
    --static <DIR>         Also serve a built web app from DIR (SPA fallback to index.html) [env: STATIC_DIR]
    --max-clients <N>      Connected clients, all rooms together [default: 2000]
    --max-rooms <N>        Rooms open at once [default: 1000]
    --max-room-clients <N> Clients in one room [default: 200]
    -h, --help             Print this help
    -V, --version          Print the version

Clients connect to ws://HOST:PORT/collab/<room> (room: A-Z a-z 0-9 _ -, at most 64 characters).
HTTP: GET /api/health, GET /api/rooms. Log level: RUST_LOG (default info).";

#[derive(Clone, Debug, PartialEq)]
pub struct Cli {
    pub host: String,
    pub port: u16,
    pub data_dir: PathBuf,
    pub static_dir: Option<PathBuf>,
    pub limits: Limits,
}

#[derive(Debug, PartialEq)]
pub enum CliError {
    Help,
    Version,
    Bad(String),
}

impl Cli {
    /// Parse `args` (without the program name); `env` looks up fallback environment variables.
    pub fn parse(args: impl IntoIterator<Item = String>, env: impl Fn(&str) -> Option<String>) -> Result<Cli, CliError> {
        let mut host = None;
        let mut port = None;
        let mut data = None;
        let mut stat = None;
        let mut limits = Limits::default();
        let mut it = args.into_iter();
        while let Some(arg) = it.next() {
            let (flag, inline) = match arg.split_once('=') {
                Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
                _ => (arg.clone(), None),
            };
            match flag.as_str() {
                "-h" | "--help" => return Err(CliError::Help),
                "-V" | "--version" => return Err(CliError::Version),
                "--port" | "--host" | "--data" | "--static" | "--max-clients" | "--max-rooms" | "--max-room-clients" => {
                    let value = match inline {
                        Some(v) => v,
                        None => it.next().ok_or_else(|| CliError::Bad(format!("{flag} needs a value")))?,
                    };
                    match flag.as_str() {
                        "--port" => port = Some(parse_port(&value)?),
                        "--host" => host = Some(value),
                        "--data" => data = Some(PathBuf::from(value)),
                        "--static" => stat = Some(PathBuf::from(value)),
                        "--max-clients" => limits.max_clients = parse_count(&flag, &value)?,
                        "--max-rooms" => limits.max_rooms = parse_count(&flag, &value)?,
                        _ => limits.max_room_clients = parse_count(&flag, &value)?,
                    }
                }
                _ => return Err(CliError::Bad(format!("unknown argument `{arg}` (see --help)"))),
            }
        }
        let port = match port {
            Some(p) => p,
            None => match env("PORT").filter(|s| !s.trim().is_empty()) {
                Some(p) => parse_port(&p)?,
                None => 1234,
            },
        };
        let host = host.or_else(|| env("HOST").filter(|s| !s.trim().is_empty())).unwrap_or_else(|| "0.0.0.0".to_string());
        let data_dir =
            data.or_else(|| env("DATA_DIR").filter(|s| !s.is_empty()).map(PathBuf::from)).unwrap_or_else(|| PathBuf::from("./collab-data"));
        let static_dir = stat.or_else(|| env("STATIC_DIR").filter(|s| !s.is_empty()).map(PathBuf::from));
        Ok(Cli { host, port, data_dir, static_dir, limits })
    }
}

fn parse_port(s: &str) -> Result<u16, CliError> {
    s.trim().parse::<u16>().map_err(|_| CliError::Bad(format!("invalid port `{s}`")))
}

fn parse_count(flag: &str, s: &str) -> Result<usize, CliError> {
    match s.trim().parse::<usize>() {
        Ok(n) if n > 0 => Ok(n),
        _ => Err(CliError::Bad(format!("{flag}: expected a positive number, got `{s}`"))),
    }
}
