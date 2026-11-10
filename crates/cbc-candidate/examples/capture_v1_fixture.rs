//! One-time capture tool for the original synthetic v1 build, not a daemon or installer.
use anyhow::{ensure, Context, Result};
use cbc_candidate::{store::Store, Sandbox, SCHEMA_VERSION};
use std::{fs, path::PathBuf};

#[tokio::main]
async fn main() -> Result<()> {
    ensure!(
        SCHEMA_VERSION == 1,
        "v1 fixture must be captured with the original v1 build"
    );
    let destination = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .context("explicit fixture destination required")?,
    );
    ensure!(
        destination.is_absolute() && !destination.exists(),
        "fixture destination must be new and absolute"
    );
    fs::create_dir_all(destination.parent().context("fixture has no parent")?)?;
    let temp = tempfile::tempdir()?;
    let protected = vec![temp.path().join("synthetic-daily/state.db")];
    let sandbox = Sandbox::initialize(&temp.path().join("candidate"), 8984, &protected)?;
    let store = Store::open(&sandbox).await?;
    store.create_context("v1-upgrade-kept").await?;
    store.close().await;
    fs::copy(sandbox.database_path(), &destination)?;
    println!("captured synthetic v1 fixture: {}", destination.display());
    Ok(())
}
