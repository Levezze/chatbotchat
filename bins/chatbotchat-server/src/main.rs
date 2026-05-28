use anyhow::Context;
use chatbotchat_server::{app_state, serve};
use clap::Parser;
use std::path::PathBuf;
use tokio::net::TcpListener;

/// chatbotchat daemon — the always-on local server agents talk to.
#[derive(Debug, Parser)]
#[command(name = "chatbotchat-server", version)]
struct Args {
    /// Port to listen on (loopback only).
    #[arg(long, env = "CBC_PORT", default_value_t = 8484)]
    port: u16,

    /// Path to the SQLite database file. Defaults to ~/.chatbotchat/state.db.
    #[arg(long, env = "CBC_DB")]
    db: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();
    let db_path = resolve_db_path(args.db)?;
    let db_url = format!("sqlite://{}", db_path.display());

    let state = app_state(&db_url).await?;

    let addr = format!("127.0.0.1:{}", args.port);
    let listener = TcpListener::bind(&addr)
        .await
        .with_context(|| format!("could not bind {addr} (is another instance running? try --port)"))?;

    tracing::info!(%addr, db = %db_path.display(), "chatbotchat-server listening");
    serve(listener, state).await
}

/// Resolve the DB path, creating the parent directory if needed.
fn resolve_db_path(explicit: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    let path = match explicit {
        Some(p) => p,
        None => {
            let home = std::env::var_os("HOME")
                .context("HOME not set; pass --db explicitly")?;
            PathBuf::from(home).join(".chatbotchat").join("state.db")
        }
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating data dir {}", parent.display()))?;
    }
    Ok(path)
}
