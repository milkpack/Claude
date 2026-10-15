//! `photo-studio`: the server of Photo Studio, a private node canvas for photographic image
//! generation. OpenArt (over its MCP server, connected with OAuth) is the model provider; Claude
//! is the director and the reviewer, with the photo skill as its method. See `DESIGN.md`.
#![forbid(unsafe_code)]

mod assets;
mod auth;
mod claude;
mod cli;
mod director;
mod error;
mod imaging;
mod jobs;
mod mcp;
mod net;
mod oauth;
mod openart;
mod review;
mod server;
mod skill;
mod static_files;
mod store;
mod workflows;

#[cfg(test)]
mod tests;

use std::process::ExitCode;

use log::{error, info, warn};

use crate::cli::{Cli, CliError};
use crate::skill::Skill;

fn main() -> ExitCode {
    let cli = match Cli::parse(std::env::args().skip(1), |k| std::env::var(k).ok()) {
        Ok(c) => c,
        Err(CliError::Help) => {
            println!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Err(CliError::Version) => {
            println!("photo-studio {}", env!("CARGO_PKG_VERSION"));
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
    let skill = match &cli.skill_dir {
        Some(dir) => Skill::from_dir(dir)?,
        None => Skill::embedded(),
    };
    if skill.files.is_empty() {
        warn!("the photo skill is empty: the director and reviewer will work without it");
    }
    info!("photo skill: {} ({} files)", skill.source, skill.files.len());
    let static_dir = match std::fs::canonicalize(&cli.static_dir) {
        Ok(d) if d.is_dir() => Some(d),
        _ => {
            warn!("front-end directory {} not found: only the API is served", cli.static_dir.display());
            None
        }
    };
    if cli.password.chars().count() < 12 {
        warn!("PHOTO_STUDIO_PASSWORD is shorter than 12 characters");
    }
    let claude_line = match &cli.anthropic_api_key {
        Some(_) => format!("configured ({}, effort {})", cli.claude_model, cli.claude_effort),
        None => "not configured (set ANTHROPIC_API_KEY)".to_string(),
    };
    let opts = server::Options {
        data_dir: cli.data_dir.clone(),
        static_dir,
        public_url: cli.public_url.clone(),
        password: cli.password,
        openart_mcp_url: cli.openart_mcp_url,
        openart_issuer: cli.openart_issuer,
        anthropic_base_url: cli.anthropic_base_url,
        anthropic_api_key: cli.anthropic_api_key,
        claude_model: cli.claude_model,
        claude_effort: cli.claude_effort,
        skill,
        trust_proxy: cli.trust_proxy,
        timing: jobs::JobTiming::default(),
    };
    let app = server::build(opts)?;
    let addr = format!("{}:{}", cli.host, cli.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.map_err(|e| format!("can't listen on {addr}: {e}"))?;
    let local = listener.local_addr().map(|a| a.to_string()).unwrap_or(addr);
    info!("listening on {local} (public URL {}), data in {}", cli.public_url, cli.data_dir.display());
    info!("Claude: {claude_line}; {} assets, {} unfinished jobs", app.assets.len(), app.jobs.unfinished().len());
    server::serve(listener, app, shutdown_signal()).await.map_err(|e| format!("server error: {e}"))?;
    info!("stopped");
    Ok(())
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
