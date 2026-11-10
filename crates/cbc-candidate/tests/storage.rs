use cbc_candidate::{store::Store, Sandbox};
use tempfile::tempdir;

#[tokio::test]
async fn a_synthetic_context_survives_closing_and_reopening_the_candidate_database() {
    let temp = tempdir().unwrap();
    let protected = vec![temp.path().join("daily/state.db")];
    let root = temp.path().join("candidate");
    let sandbox = Sandbox::initialize(&root, 8984, &protected).unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("synthetic-project").await.unwrap();
    assert!(store.context_exists("synthetic-project").await.unwrap());
    store.close().await;

    let sandbox = Sandbox::inspect(&root, &protected).unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    assert!(store.context_exists("synthetic-project").await.unwrap());
    assert!(!store.context_exists("different-project").await.unwrap());
    store.close().await;
}

#[tokio::test]
async fn refuses_a_newer_schema_without_mutating_the_database() {
    use std::io::{Seek, SeekFrom, Write};
    let temp = tempdir().unwrap();
    let protected = vec![temp.path().join("daily/state.db")];
    let sandbox = Sandbox::initialize(&temp.path().join("candidate"), 8984, &protected).unwrap();
    Store::open(&sandbox).await.unwrap().close().await;
    // Inject a future SQLite schema header at the filesystem boundary, not through store internals.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(sandbox.database_path())
        .unwrap();
    file.seek(SeekFrom::Start(60)).unwrap();
    file.write_all(&(cbc_candidate::SCHEMA_VERSION + 1).to_be_bytes())
        .unwrap();
    file.sync_all().unwrap();
    drop(file);
    let before = std::fs::read(sandbox.database_path()).unwrap();
    assert!(Store::open(&sandbox).await.is_err());
    assert_eq!(std::fs::read(sandbox.database_path()).unwrap(), before);
}

#[tokio::test]
async fn only_one_core_can_own_the_database_and_explicit_close_releases_it() {
    let temp = tempdir().unwrap();
    let protected = vec![temp.path().join("daily/state.db")];
    let sandbox = Sandbox::initialize(&temp.path().join("candidate"), 8984, &protected).unwrap();
    let first = Store::open(&sandbox).await.unwrap();
    assert!(Store::open(&sandbox).await.is_err());
    first.close().await;
    Store::open(&sandbox).await.unwrap().close().await;
}

#[tokio::test]
async fn store_rechecks_isolation_when_the_filesystem_changes_after_preflight() {
    use std::os::unix::fs::symlink;
    let temp = tempdir().unwrap();
    let daily = temp.path().join("daily");
    std::fs::create_dir(&daily).unwrap();
    let protected_db = daily.join("state.db");
    std::fs::write(&protected_db, b"daily sentinel").unwrap();
    let protected = vec![protected_db.clone()];
    let sandbox = Sandbox::initialize(&temp.path().join("candidate"), 8984, &protected).unwrap();
    symlink(&protected_db, sandbox.database_path()).unwrap();
    assert!(Store::open(&sandbox).await.is_err());
    assert_eq!(std::fs::read(protected_db).unwrap(), b"daily sentinel");
}

#[tokio::test]
async fn tolerates_an_older_header_and_reconciles_it_with_validated_migration_history() {
    use std::io::{Seek, SeekFrom, Write};
    let temp = tempdir().unwrap();
    let protected = vec![temp.path().join("daily/state.db")];
    let sandbox = Sandbox::initialize(&temp.path().join("candidate"), 8984, &protected).unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    store.create_context("keep-context").await.unwrap();
    store.close().await;
    // A checkpointed header may lag the applied migration history. The migrator still owns
    // schema compatibility; the raw header is only an early future-version rejection.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(sandbox.database_path())
        .unwrap();
    file.seek(SeekFrom::Start(60)).unwrap();
    file.write_all(&0_u32.to_be_bytes()).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let store = Store::open(&sandbox).await.unwrap();
    assert!(store.context_exists("keep-context").await.unwrap());
    store.close().await;
    let header = std::fs::read(sandbox.database_path()).unwrap();
    assert_eq!(
        u32::from_be_bytes(header[60..64].try_into().unwrap()),
        cbc_candidate::SCHEMA_VERSION
    );
}
