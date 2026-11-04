//! Capture with the original schema-3 build before applying enrollment migration 0004.
use anyhow::{ensure, Context, Result};
use cbc_candidate::{bindings::Role, store::Store, Sandbox, SCHEMA_VERSION};
use std::{fs, path::PathBuf};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    ensure!(
        SCHEMA_VERSION == 3,
        "capture requires the original schema-3 build"
    );
    let destination = PathBuf::from(std::env::args_os().nth(1).context("destination required")?);
    ensure!(
        destination.is_absolute() && !destination.exists(),
        "destination must be new and absolute"
    );
    let temp = tempfile::tempdir()?;
    let sandbox = Sandbox::initialize(
        &temp.path().join("c"),
        8984,
        &[temp.path().join("synthetic-daily/state.db")],
    )?;
    let store = Store::open(&sandbox).await?;
    store.create_context("v3-upgrade-kept").await?;
    for (member, session) in [("one", "one-native"), ("two", "two-native")] {
        store
            .register_member("v3-upgrade-kept", member, Role::Participant)
            .await?;
        let binding = store
            .bind_member("v3-upgrade-kept", member, "fixture-device", session)
            .await?;
        ensure!(binding.epoch() == 1, "unexpected original epoch");
    }
    store
        .create_room("v3-upgrade-kept", "original-room", &["one", "two"])
        .await?;
    store.close().await;
    fs::copy(sandbox.database_path(), destination)?;
    println!("Captured synthetic schema-3 context, room and two epoch-1 bindings; no credentials exported");
    Ok(())
}
