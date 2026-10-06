//! `vectorcraft-collab-server`: the relay VectorCraft clients co-edit through.
//!
//! A standard Yjs sync + awareness relay (like y-websocket's server) with rooms at
//! `ws(s)://HOST/collab/<room>`, documents stored as `<data dir>/<room>.ydoc`, and a small HTTP
//! API (`/api/health`, `/api/rooms`). Optionally serves the built web app (`--static DIR`).
#![forbid(unsafe_code)]

mod cli;
mod hub;
mod server;
mod static_files;
mod storage;

#[cfg(test)]
mod tests;

use std::process::ExitCode;

use log::{error, info};

use crate::cli::{Cli, CliError};

fn main() -> ExitCode {
    let cli = match Cli::parse(std::env::args().skip(1), |k| std::env::var(k).ok()) {
        Ok(c) => c,
        Err(CliError::Help) => {
            println!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Err(CliError::Version) => {
            println!("vectorcraft-collab-server {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(CliError::Bad(msg)) => {
            eprintln!("error: {msg}\n\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            error!("can't start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match rt.block_on(run(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!("{e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    std::fs::create_dir_all(&cli.data_dir).map_err(|e| format!("data directory {}: {e}", cli.data_dir.display()))?;
    let static_dir = match &cli.static_dir {
        Some(d) => {
            let real = std::fs::canonicalize(d).map_err(|e| format!("static directory {}: {e}", d.display()))?;
            if !real.is_dir() {
                return Err(format!("static directory {}: not a directory", d.display()));
            }
            Some(real)
        }
        None => None,
    };
    let addr = format!("{}:{}", cli.host, cli.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.map_err(|e| format!("can't listen on {addr}: {e}"))?;
    let local = listener.local_addr().map(|a| a.to_string()).unwrap_or(addr);
    info!("listening on {local} (ws://{local}/collab/<room>), data in {}", cli.data_dir.display());
    if let Some(d) = &static_dir {
        info!("serving the web app from {}", d.display());
    }
    let opts = server::Options { data_dir: cli.data_dir, static_dir, limits: cli.limits };
    server::serve(listener, opts, shutdown_signal()).await.map_err(|e| format!("server error: {e}"))
}

/// Ctrl-C, or SIGTERM on Unix.
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            error!("can't listen for Ctrl-C: {e}");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => {
                error!("can't listen for SIGTERM: {e}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = term => {}
    }
}
