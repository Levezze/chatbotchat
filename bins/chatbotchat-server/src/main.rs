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
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();
    let db_path = resolve_db_path(args.db)?;
    let db_url = format!("sqlite://{}", db_path.display());

    let state = app_state(&db_url).await?;

    let addr = format!("127.0.0.1:{}", args.port);
    let listener = match TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(err) => {
            let pid = conflicting_pid(args.port);
            return Err(anyhow::Error::new(err).context(port_conflict_message(args.port, pid)));
        }
    };

    tracing::info!(%addr, db = %db_path.display(), "chatbotchat-server listening");
    serve(listener, state).await
}

/// Build the user-facing error shown when the daemon cannot bind its port —
/// almost always because another instance already holds it. Names the port,
/// the conflicting PID when we could find it, and the `--port` escape hatch.
fn port_conflict_message(port: u16, pid: Option<u32>) -> String {
    let pid_note = match pid {
        Some(pid) => format!(" (PID {pid})"),
        None => String::new(),
    };
    format!(
        "could not bind 127.0.0.1:{port} — another instance may already be \
         running{pid_note}. Use --port <N> to run on a different port (e.g. --port 8485)."
    )
}

/// Best-effort lookup of the PID holding `port` on loopback, via `lsof`. Returns
/// `None` whenever `lsof` is missing, fails, or names no listener — the error
/// message degrades to omitting the PID rather than failing.
fn conflicting_pid(port: u16) -> Option<u32> {
    let output = std::process::Command::new("lsof")
        .args(["-ti", &format!("tcp:{port}")])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
}

/// Resolve the DB path, creating the parent directory if needed.
fn resolve_db_path(explicit: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    let path = match explicit {
        Some(p) => p,
        None => {
            let home = std::env::var_os("HOME").context("HOME not set; pass --db explicitly")?;
            PathBuf::from(home).join(".chatbotchat").join("state.db")
        }
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating data dir {}", parent.display()))?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_conflict_message_names_port_and_flag() {
        let msg = port_conflict_message(8484, None);
        assert!(msg.contains("8484"), "should name the port: {msg}");
        assert!(msg.contains("--port"), "should hint at --port: {msg}");
        assert!(
            msg.to_lowercase().contains("another instance"),
            "should mention another instance: {msg}"
        );
    }

    #[test]
    fn port_conflict_message_includes_pid_when_known() {
        let msg = port_conflict_message(8484, Some(4242));
        assert!(
            msg.contains("4242"),
            "should name the conflicting PID: {msg}"
        );
        assert!(
            msg.to_lowercase().contains("pid"),
            "should label the PID: {msg}"
        );
    }
}
