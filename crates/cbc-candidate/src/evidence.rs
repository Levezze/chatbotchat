//! Trial evidence (C1). The running core freezes its database into a new private snapshot file
//! on the owner's request. Everything a trial records afterwards, the manifest and the report,
//! reads only such a snapshot through [`Evidence`]: no lease, no migration and no clock, so the
//! same snapshot always gives the same output and the live database is never opened twice.
use crate::{settings::SettingsSnapshot, store::Store, Sandbox, APPLICATION_ID, SCHEMA_VERSION};
use anyhow::{ensure, Context, Result};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

const SNAPSHOT_DIR: &str = "snapshots";
/// The kinds of approval prompt the user counts during a run (protocol §6).
pub const APPROVAL_KINDS: [&str; 4] = ["cbc", "native_setup", "native_refusal", "other"];

/// A frozen copy of the candidate database, written by the core that owns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub file: PathBuf,
    pub taken_wall_ms: i64,
}

/// Owner operation, run inside the core: `VACUUM INTO` a file created here and now, so a
/// snapshot can never replace an existing file, follow a link or be read by another user.
pub(crate) async fn take_snapshot(store: &Store, root: &Path) -> Result<Snapshot> {
    let dir = root.join(SNAPSHOT_DIR);
    match fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let meta = fs::symlink_metadata(&dir)?;
    ensure!(
        meta.is_dir() && meta.uid() == crate::effective_uid() && meta.mode() & 0o777 == 0o700,
        "unsafe candidate snapshot directory"
    );
    let taken_wall_ms = crate::facts::wall_ms();
    let mut random = [0_u8; 6];
    OsRng.try_fill_bytes(&mut random)?;
    let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let file = dir.join(format!("snapshot-{taken_wall_ms}-{suffix}.sqlite3"));
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&file)?;
    let path = file.to_str().context("snapshot path must be UTF-8")?;
    // SQLite writes into an existing file only while it is empty.
    if let Err(error) = sqlx::query("VACUUM INTO ?")
        .bind(path)
        .execute(&store.pool)
        .await
    {
        let _ = fs::remove_file(&file);
        return Err(error.into());
    }
    fs::File::open(&file)?.sync_all()?;
    fs::File::open(&dir)?.sync_all()?;
    Ok(Snapshot {
        file,
        taken_wall_ms,
    })
}

/// One seat as the snapshot records it. Never carries a credential digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Seat {
    pub context: String,
    pub member: String,
    pub role: String,
    pub active: bool,
    pub retired: bool,
    pub epoch: i64,
    pub session: Option<String>,
    pub host: Option<String>,
}

type SeatRow = (
    String,
    String,
    String,
    i64,
    i64,
    i64,
    Option<String>,
    Option<String>,
);

/// Values the manifest takes from outside the snapshot.
pub struct ManifestInput {
    pub trial: u32,
    pub build_commit: String,
    pub binary_sha256: String,
    pub versions: BTreeMap<String, String>,
    pub notes: BTreeMap<String, String>,
    pub settings: SettingsSnapshot,
}

/// A read-only view of one snapshot.
pub struct Evidence {
    pool: SqlitePool,
    file: PathBuf,
}

impl Evidence {
    /// Opens a snapshot written by [`take_snapshot`] under this sandbox, and nothing else.
    pub async fn open(sandbox: &Sandbox, file: &Path) -> Result<Self> {
        let sandbox = Sandbox::inspect(sandbox.root(), &sandbox.manifest.protected_databases)?;
        let dir = sandbox.root().join(SNAPSHOT_DIR);
        let name = file
            .file_name()
            .and_then(|name| name.to_str())
            .context("snapshot name must be UTF-8")?;
        ensure!(
            file.parent() == Some(dir.as_path())
                && name.starts_with("snapshot-")
                && name.ends_with(".sqlite3"),
            "evidence must be a snapshot in the candidate snapshot directory"
        );
        let meta = fs::symlink_metadata(file)?;
        ensure!(
            meta.file_type().is_file()
                && meta.nlink() == 1
                && meta.uid() == crate::effective_uid()
                && meta.mode() & 0o777 == 0o600,
            "snapshot must be a private regular file with a single link"
        );
        let options = SqliteConnectOptions::new()
            .filename(file)
            .create_if_missing(false)
            .read_only(true)
            .immutable(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let evidence = Self {
            pool,
            file: file.to_path_buf(),
        };
        if let Err(error) = evidence.check_brand().await {
            evidence.close().await;
            return Err(error);
        }
        Ok(evidence)
    }

    async fn check_brand(&self) -> Result<()> {
        let application: i64 = sqlx::query_scalar("PRAGMA application_id")
            .fetch_one(&self.pool)
            .await?;
        let version: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&self.pool)
            .await?;
        ensure!(
            application == i64::from(APPLICATION_ID),
            "snapshot is not a candidate database"
        );
        ensure!(
            version == i64::from(SCHEMA_VERSION),
            "snapshot schema {version} is not {SCHEMA_VERSION}"
        );
        Ok(())
    }

    pub async fn close(self) {
        self.pool.close().await;
    }

    pub async fn seats(&self) -> Result<Vec<Seat>> {
        let rows: Vec<SeatRow> = sqlx::query_as(
            "SELECT m.context_id, m.member_id, m.role, m.active, m.retired, m.epoch,
                    m.native_session_id, t.host
             FROM candidate_members m
             LEFT JOIN candidate_native_targets t
               ON t.context_id = m.context_id AND t.member_id = m.member_id AND t.epoch = m.epoch
             ORDER BY m.context_id, m.member_id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(context, member, role, active, retired, epoch, session, host)| Seat {
                    context,
                    member,
                    role,
                    active: active == 1,
                    retired: retired == 1,
                    epoch,
                    session,
                    host,
                },
            )
            .collect())
    }

    /// The trial manifest (protocol §7) for this snapshot. Serialize it with [`canonical_json`].
    pub async fn manifest(&self, snapshot: &Snapshot, input: &ManifestInput) -> Result<Value> {
        ensure!(
            snapshot.file == self.file,
            "manifest snapshot differs from the opened evidence"
        );
        let seats = self.seats().await?;
        let mut approvals = Map::new();
        for seat in &seats {
            let tally: Map<String, Value> = APPROVAL_KINDS
                .iter()
                .map(|kind| ((*kind).to_owned(), Value::Null))
                .collect();
            approvals.insert(format!("{}/{}", seat.context, seat.member), tally.into());
        }
        Ok(json!({
            "trial": input.trial,
            "schema_version": SCHEMA_VERSION,
            "build": {"commit": input.build_commit, "binary_sha256": input.binary_sha256},
            "snapshot": {
                "file": snapshot.file,
                "taken_wall_ms": snapshot.taken_wall_ms,
                "sha256": sha256_file(&snapshot.file)?,
            },
            "versions": input.versions,
            "notes": input.notes,
            "settings": input.settings,
            "seats": seats,
            "approval_prompts": approvals,
        }))
    }
}

/// Manifest values the operator supplies on the command line.
pub struct ManifestArgs {
    pub trial: u32,
    pub build_commit: String,
    pub versions: BTreeMap<String, String>,
    pub notes: BTreeMap<String, String>,
}

/// The `manifest` command: check `out` before anything else so a bad path costs no snapshot,
/// then snapshot (the core creates the directory), create `out`, read settings and write the
/// canonical manifest. Returns the snapshot it describes.
pub async fn record_manifest(
    sandbox: &Sandbox,
    owner: &crate::connector::LocalOwnerClient<'_>,
    out: &Path,
    args: ManifestArgs,
) -> Result<Snapshot> {
    let dir = sandbox.root().join(SNAPSHOT_DIR);
    let name = out
        .file_name()
        .and_then(|name| name.to_str())
        .context("manifest name must be UTF-8")?;
    ensure!(
        out.is_absolute()
            && out.parent() == Some(dir.as_path())
            && name.ends_with(".json")
            && !name.starts_with('.'),
        "manifest must be a new .json file directly beneath the candidate snapshot directory"
    );
    ensure!(
        fs::symlink_metadata(out).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
        "manifest file must not exist yet"
    );
    let binary_sha256 = sha256_file(&std::env::current_exe()?)?;
    let snapshot = owner.snapshot().await?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(out)
        .context("manifest file must not exist yet")?;
    let written = async {
        let settings = owner.settings().await?;
        let evidence = Evidence::open(sandbox, &snapshot.file).await?;
        let manifest = evidence
            .manifest(
                &snapshot,
                &ManifestInput {
                    trial: args.trial,
                    build_commit: args.build_commit,
                    binary_sha256,
                    versions: args.versions,
                    notes: args.notes,
                    settings,
                },
            )
            .await;
        evidence.close().await;
        std::io::Write::write_all(&mut file, &canonical_json(&manifest?))?;
        file.sync_all()?;
        fs::File::open(&dir)?.sync_all()?;
        anyhow::Ok(snapshot)
    }
    .await;
    if written.is_err() {
        let _ = fs::remove_file(out);
    }
    written
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let digest = Sha256::digest(fs::read(path)?);
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

/// Pretty JSON with object keys sorted at every level and a final newline. The workspace builds
/// serde_json with `preserve_order`, so sorting must be explicit.
pub fn canonical_json(value: &Value) -> Vec<u8> {
    fn sort(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                Value::Object(
                    keys.into_iter()
                        .map(|key| (key.clone(), sort(&map[key])))
                        .collect(),
                )
            }
            Value::Array(items) => Value::Array(items.iter().map(sort).collect()),
            other => other.clone(),
        }
    }
    let mut bytes = serde_json::to_vec_pretty(&sort(value)).expect("a JSON value serializes");
    bytes.push(b'\n');
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Evidence is read-only at the SQLite layer, not by convention.
    #[tokio::test]
    async fn evidence_refuses_writes() {
        let temp = tempfile::Builder::new()
            .prefix("cbc-evidence-unit-")
            .tempdir_in("/tmp")
            .unwrap();
        let sandbox = Sandbox::initialize(
            &temp.path().join("c"),
            8986,
            &[temp.path().join("daily/state.db")],
        )
        .unwrap();
        let store = Store::open(&sandbox).await.unwrap();
        let snapshot = take_snapshot(&store, sandbox.root()).await.unwrap();
        store.close().await;
        let evidence = Evidence::open(&sandbox, &snapshot.file).await.unwrap();
        let write = sqlx::query("INSERT INTO candidate_contexts(context_id) VALUES ('x')")
            .execute(&evidence.pool)
            .await;
        assert!(write.is_err(), "a write through evidence must fail");
        evidence.close().await;
    }

    #[test]
    fn canonical_json_sorts_nested_keys() {
        let value = json!({"b": {"z": 1, "a": [{"y": 2, "x": 3}]}, "a": null});
        assert_eq!(
            String::from_utf8(canonical_json(&value)).unwrap(),
            "{\n  \"a\": null,\n  \"b\": {\n    \"a\": [\n      {\n        \"x\": 3,\n        \"y\": 2\n      }\n    ],\n    \"z\": 1\n  }\n}\n"
        );
    }
}
