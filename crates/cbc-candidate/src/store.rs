//! Trusted synthetic-fixture API only. Not exposed as an agent or HTTP administration route.
use crate::Sandbox;
use anyhow::{ensure, Context, Result};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    SqlitePool,
};
use std::{
    fs,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::Path,
    time::Duration,
};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[must_use = "call close().await before deliberately transferring candidate database ownership"]
pub struct Store {
    pub(crate) pool: SqlitePool,
    pub(crate) clock: crate::facts::CoreClock,
    lease: Option<OwnerLease>,
}

struct OwnerLease {
    file: Option<fs::File>,
    may_release: bool,
}

impl Drop for OwnerLease {
    fn drop(&mut self) {
        if !self.may_release {
            // Shutdown failed or was cancelled: fail closed until process exit.
            if let Some(file) = self.file.take() {
                std::mem::forget(file);
            }
        }
    }
}

impl Store {
    pub async fn open(sandbox: &Sandbox) -> Result<Self> {
        // No caller-supplied SQLite URL or database path. Revalidate immediately before access.
        let sandbox = Sandbox::inspect(sandbox.root(), &sandbox.manifest.protected_databases)?;
        let lease = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(sandbox.root().join("candidate.lock"))?;
        let meta = lease.metadata()?;
        ensure!(
            meta.is_file()
                && meta.nlink() == 1
                && meta.uid() == crate::effective_uid()
                && meta.mode() & 0o777 == 0o600,
            "unsafe candidate lock"
        );
        // SAFETY: the descriptor belongs to lease and remains open for the entire store lifetime.
        ensure!(
            unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
            "candidate database already has an owner"
        );
        let db = sandbox.database_path();
        if !db.exists() {
            // Publish only a fully branded/migrated database. An interrupted bootstrap remains
            // isolated and fails closed on the next attempt instead of adopting an unknown file.
            let staging = sandbox.root().join("candidate.bootstrap.sqlite3");
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&staging)
                .context("interrupted candidate bootstrap; use a fresh candidate root")?;
            let staging_pool = connect(&staging, SqliteJournalMode::Delete).await?;
            let migrated = apply_migrations(&staging_pool).await;
            staging_pool.close().await;
            migrated?;
            ensure!(!db.exists(), "candidate database appeared during bootstrap");
            // All cooperative owners hold the same OS lease. Hostile same-user path replacement
            // is outside this private-directory boundary, as documented in the brief.
            fs::rename(&staging, &db)?;
            fs::File::open(sandbox.root())?.sync_all()?;
        }
        Sandbox::inspect(sandbox.root(), &sandbox.manifest.protected_databases)?;
        let pool = connect(&db, SqliteJournalMode::Wal).await?;
        if let Err(error) = apply_migrations(&pool).await {
            pool.close().await;
            return Err(error);
        }
        let clock = match start_clock(&pool).await {
            Ok(clock) => clock,
            Err(error) => {
                pool.close().await;
                return Err(error);
            }
        };
        Ok(Self {
            pool,
            clock,
            lease: Some(OwnerLease {
                file: Some(lease),
                may_release: false,
            }),
        })
    }

    pub async fn create_context(&self, id: &str) -> Result<()> {
        ensure!(
            !id.is_empty() && id.len() <= 128,
            "invalid context identifier"
        );
        sqlx::query("INSERT INTO candidate_contexts(context_id) VALUES (?) ON CONFLICT DO NOTHING")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn context_exists(&self, id: &str) -> Result<bool> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM candidate_contexts WHERE context_id = ?",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?
            > 0)
    }

    /// Context IDs after `after` ("" for the first page), in ID order, at most `limit`.
    pub async fn contexts(&self, after: &str, limit: u32) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT context_id FROM candidate_contexts WHERE context_id > ? ORDER BY context_id LIMIT ?",
        )
        .bind(after)
        .bind(i64::from(limit.min(100)))
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn close(mut self) {
        self.pool.close().await;
        if let Some(lease) = &mut self.lease {
            lease.may_release = true;
        }
        self.lease.take();
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        if let Some(mut lease) = self.lease.take() {
            let pool = self.pool.clone();
            // Pool drop only signals SQLite's worker. Keep ownership until close has awaited
            // every connection. Explicit close avoids this short-lived cleanup thread entirely.
            // A dedicated runtime also works when the caller's runtime is already shutting down.
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                if let Ok(runtime) = runtime {
                    runtime.block_on(pool.close());
                    lease.may_release = true;
                    drop(lease);
                }
            });
        }
    }
}

async fn apply_migrations(pool: &SqlitePool) -> Result<()> {
    // The SQLx history/checksums are canonical. Keep ignore_missing disabled so a lagging
    // SQLite file header cannot hide unknown migrations in a newer WAL/database.
    MIGRATOR.run(pool).await?;
    let applied: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success = 1",
    )
    .fetch_one(pool)
    .await?;
    ensure!(
        applied == i64::from(crate::SCHEMA_VERSION),
        "candidate schema declaration disagrees with migration history"
    );
    let advertised: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(pool)
        .await?;
    ensure!(
        advertised <= i64::from(crate::SCHEMA_VERSION),
        "candidate database is newer than this build"
    );
    if advertised != applied {
        sqlx::query(&format!("PRAGMA user_version = {applied}"))
            .execute(pool)
            .await?;
    }
    Ok(())
}

async fn start_clock(pool: &SqlitePool) -> Result<crate::facts::CoreClock> {
    let mut conn = pool.acquire().await?;
    crate::facts::CoreClock::start(&mut conn).await
}

async fn connect(path: &Path, journal: SqliteJournalMode) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(2))
        .journal_mode(journal);
    Ok(SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn implicit_drop_holds_the_owner_lease_until_connections_finish_closing() {
        let temp = tempfile::tempdir().unwrap();
        let sandbox = Sandbox::initialize(
            &temp.path().join("candidate"),
            8984,
            &[temp.path().join("daily/state.db")],
        )
        .unwrap();
        let store = Store::open(&sandbox).await.unwrap();
        let connection = store.pool.acquire().await.unwrap();
        drop(store);
        assert!(
            Store::open(&sandbox).await.is_err(),
            "the old SQLite connection still owns the database"
        );
        drop(connection);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(next) = Store::open(&sandbox).await {
                    next.close().await;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
}
