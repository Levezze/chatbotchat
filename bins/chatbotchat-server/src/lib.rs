//! Server wiring shared between `main.rs` and integration tests.
//!
//! Keeping `app_state` and `serve` here (rather than inlined in `main`) lets the
//! daemon tests drive the exact same router + serve path the binary uses, over a
//! real TCP listener, without shelling out to the compiled binary.

use anyhow::Context;
use chatbotchat_core::http::{router, AppState};
use chatbotchat_core::storage::Storage;
use chatbotchat_core::sweeper::run_sweeper;
use tokio::net::TcpListener;

/// Build application state by connecting to the database at `db_url`
/// (e.g. `sqlite:///Users/me/.chatbotchat/state.db`). Migrations run on connect.
pub async fn app_state(db_url: &str) -> anyhow::Result<AppState> {
    let storage = Storage::connect(db_url)
        .await
        .with_context(|| format!("connecting to database at {db_url}"))?;
    Ok(AppState::new(storage))
}

/// Serve the router on an already-bound listener until the process is
/// terminated. Graceful shutdown (draining in-flight connections on SIGTERM)
/// is not yet wired — deferred to the daemon-lifecycle hardening in slice #10.
pub async fn serve(listener: TcpListener, state: AppState) -> anyhow::Result<()> {
    // Background sweeper: hourly time-based room transitions (idle/stale/archive).
    // Detached for the process lifetime — no graceful join yet, same as the
    // deferred slice-#10 shutdown work. Its first tick is one hour out, so a
    // freshly-started daemon does not sweep at boot.
    tokio::spawn(run_sweeper(state.storage()));

    axum::serve(listener, router(state))
        .await
        .context("axum serve")?;
    Ok(())
}
