//! Server wiring shared between `main.rs` and integration tests.
//!
//! Keeping `app_state` and `serve` here (rather than inlined in `main`) lets the
//! daemon tests drive the exact same router + serve path the binary uses, over a
//! real TCP listener, without shelling out to the compiled binary.

use anyhow::Context;
use chatbotchat_core::http::{router, AppState};
use chatbotchat_core::storage::Storage;
use tokio::net::TcpListener;

/// Build application state by connecting to the database at `db_url`
/// (e.g. `sqlite:///Users/me/.chatbotchat/state.db`). Migrations run on connect.
pub async fn app_state(db_url: &str) -> anyhow::Result<AppState> {
    let storage = Storage::connect(db_url)
        .await
        .with_context(|| format!("connecting to database at {db_url}"))?;
    Ok(AppState::new(storage))
}

/// Serve the router on an already-bound listener until shutdown.
pub async fn serve(listener: TcpListener, state: AppState) -> anyhow::Result<()> {
    axum::serve(listener, router(state))
        .await
        .context("axum serve")?;
    Ok(())
}
