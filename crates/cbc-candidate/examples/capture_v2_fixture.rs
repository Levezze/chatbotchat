//! One-time capture with the actual v2 implementation before migration 0003.
use anyhow::{ensure, Context, Result};
use cbc_candidate::{bindings::Role, store::Store, Sandbox, SCHEMA_VERSION};
use std::{fs, path::PathBuf};

#[tokio::main]
async fn main() -> Result<()> {
    ensure!(
        SCHEMA_VERSION == 2,
        "capture requires the original v2 build"
    );
    let destination = PathBuf::from(std::env::args_os().nth(1).context("destination required")?);
    ensure!(
        destination.is_absolute() && !destination.exists(),
        "destination must be new and absolute"
    );
    let temp = tempfile::tempdir()?;
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        8984,
        &[temp.path().join("synthetic-daily/state.db")],
    )?;
    let store = Store::open(&sandbox).await?;
    store.create_context("v2-upgrade-kept").await?;
    store
        .register_member("v2-upgrade-kept", "lead", Role::RepoLead)
        .await?;
    let binding = store
        .bind_member(
            "v2-upgrade-kept",
            "lead",
            "fixture-device",
            "fixture-session",
        )
        .await?;
    ensure!(binding.epoch() == 1, "unexpected fixture epoch");
    drop(binding);
    store.close().await;
    fs::copy(sandbox.database_path(), destination)?;
    println!("captured synthetic v2 database with member epoch 1; no credential exported");
    Ok(())
}
