//! Candidate-only runtime boundary. No legacy database connection or network client lives here.
pub mod bindings;
pub mod connector;
pub mod enrollment;
pub mod facts;
pub mod inbox;
pub mod native_target;
pub mod owner;
pub mod requests;
pub mod settings;
pub mod store;
pub mod work;
use anyhow::{anyhow, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
};

const FILES: [&str; 14] = [
    "candidate.json",
    "candidate.sqlite3",
    "candidate.sqlite3-wal",
    "candidate.sqlite3-shm",
    "candidate.sqlite3-journal",
    "candidate.lock",
    "candidate.bootstrap.sqlite3",
    "candidate.bootstrap.sqlite3-journal",
    "candidate.bootstrap.sqlite3-wal",
    "candidate.bootstrap.sqlite3-shm",
    "runtime-settings.json",
    "runtime-settings.last-good.json",
    "runtime-settings.lock",
    "owner.key",
];
pub const APPLICATION_ID: u32 = 0x43424332;
pub const SCHEMA_VERSION: u32 = 11;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    namespace: String,
    version: u32,
    root: PathBuf,
    port: u16,
    protected_databases: Vec<PathBuf>,
}

/// Constructed only through validation, never deserialized directly from caller input.
#[derive(Debug, Serialize)]
pub struct Sandbox {
    #[serde(flatten)]
    manifest: Manifest,
}

impl Sandbox {
    pub fn port(&self) -> u16 {
        self.manifest.port
    }

    pub fn root(&self) -> &Path {
        &self.manifest.root
    }

    pub fn database_path(&self) -> PathBuf {
        self.root().join("candidate.sqlite3")
    }

    /// Library seam for synthetic policies. The executable always adds home-derived protection.
    pub fn initialize(root: &Path, port: u16, protected_dbs: &[PathBuf]) -> Result<Self> {
        clean_absolute(root)?;
        validate_port(port)?;
        let parent = root
            .parent()
            .context("root has no parent")?
            .canonicalize()?;
        let target = parent.join(root.file_name().context("root has no name")?);
        protect_location(&target, protected_dbs)?;
        // Exclusive create: an existing directory, symlink or file is never adopted.
        fs::DirBuilder::new().mode(0o700).create(&target)?;
        let manifest = Manifest {
            namespace: "cbc-candidate".into(),
            version: 1,
            root: target.canonicalize()?,
            port,
            protected_databases: protected_dbs.to_vec(),
        };
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(manifest.root.join(FILES[0]))?;
        file.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
        file.sync_all()?;
        fs::File::open(&manifest.root)?.sync_all()?;
        Self::inspect(&manifest.root, protected_dbs)
    }

    /// Read-only preflight. Must also run immediately before a later store opens SQLite.
    pub fn inspect(root: &Path, protected_dbs: &[PathBuf]) -> Result<Self> {
        clean_absolute(root)?;
        let metadata = fs::symlink_metadata(root)?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "candidate root must be a real directory"
        );
        ensure!(
            metadata.uid() == effective_uid() && metadata.mode() & 0o777 == 0o700,
            "candidate root must be owned by this user with mode 0700"
        );
        let canonical = root.canonicalize()?;
        // Check protection and ALL file aliases before reading even the manifest.
        protect_location(&canonical, protected_dbs)?;
        for name in FILES {
            let path = canonical.join(name);
            match fs::symlink_metadata(&path) {
                Ok(meta) => {
                    ensure!(
                        meta.file_type().is_file() && meta.nlink() == 1,
                        "candidate files must be regular files with a single link"
                    );
                    ensure!(
                        meta.uid() == effective_uid() && meta.mode() & 0o777 == 0o600,
                        "candidate files must be owned by this user with mode 0600"
                    );
                    for protected in protected_dbs {
                        for suffix in ["", "-wal", "-shm", "-journal"] {
                            let mut protected_file = protected.as_os_str().to_os_string();
                            protected_file.push(suffix);
                            match fs::metadata(PathBuf::from(protected_file)) {
                                Ok(other) => ensure!(
                                    (meta.dev(), meta.ino()) != (other.dev(), other.ino()),
                                    "candidate file aliases a protected file"
                                ),
                                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                                Err(error) => return Err(error.into()),
                            }
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && name != FILES[0] => {}
                Err(error) => return Err(error.into()),
            }
        }
        let mut bytes = Vec::new();
        fs::File::open(canonical.join(FILES[0]))?
            .take(4097)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 4096, "candidate manifest exceeds size bound");
        let manifest: Manifest =
            serde_json::from_slice(&bytes).context("invalid candidate manifest")?;
        ensure!(
            manifest.namespace == "cbc-candidate" && manifest.version == 1,
            "foreign candidate manifest"
        );
        ensure!(
            manifest.root == canonical,
            "candidate root does not match manifest"
        );
        validate_port(manifest.port)?;
        // Persisted protection can only add restrictions on reopen.
        protect_location(&canonical, &manifest.protected_databases)?;
        let db = canonical.join("candidate.sqlite3");
        if db.exists() {
            let mut header = [0; 72];
            fs::File::open(db)?
                .read_exact(&mut header)
                .context("invalid candidate database header")?;
            ensure!(
                &header[..16] == b"SQLite format 3\0"
                    && u32::from_be_bytes(header[68..72].try_into()?) == APPLICATION_ID,
                "foreign database; candidate will not open or migrate it"
            );
            ensure!(
                u32::from_be_bytes(header[60..64].try_into()?) <= SCHEMA_VERSION,
                "unsupported candidate database schema; refusing before SQLite open"
            );
        }
        Ok(Self { manifest })
    }
}

fn validate_port(port: u16) -> Result<()> {
    ensure!(
        port >= 1024 && port != 8484,
        "invalid or protected candidate port"
    );
    Ok(())
}

fn clean_absolute(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute()
            && !path
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir)),
        "paths must be absolute without dot components"
    );
    Ok(())
}

fn protect_location(target: &Path, protected_dbs: &[PathBuf]) -> Result<()> {
    ensure!(
        !protected_dbs.is_empty(),
        "protected database set cannot be empty"
    );
    for protected_db in protected_dbs {
        clean_absolute(protected_db)?;
        let mut parents = vec![resolve_existing_ancestor(
            protected_db.parent().context("database has no parent")?,
        )?];
        match fs::canonicalize(protected_db) {
            Ok(real) => parents.push(
                real.parent()
                    .context("database has no real parent")?
                    .to_path_buf(),
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        for parent in parents {
            ensure!(
                !target.starts_with(&parent) && !parent.starts_with(target),
                "candidate root overlaps a protected database directory"
            );
            // Directory identities also catch case aliases on case-insensitive filesystems.
            if let Ok(other) = fs::metadata(&parent) {
                for ancestor in target.ancestors() {
                    if let Ok(meta) = fs::metadata(ancestor) {
                        ensure!(
                            (meta.dev(), meta.ino()) != (other.dev(), other.ino()),
                            "candidate root aliases a protected directory"
                        );
                    }
                }
            }
            if let Ok(meta) = fs::metadata(target) {
                for ancestor in parent.ancestors() {
                    if let Ok(other) = fs::metadata(ancestor) {
                        ensure!(
                            (meta.dev(), meta.ino()) != (other.dev(), other.ino()),
                            "candidate root aliases an ancestor of a protected directory"
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

fn resolve_existing_ancestor(path: &Path) -> Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(path.canonicalize()?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .ok_or_else(|| anyhow!("path has no existing ancestor"))?;
            let name = path.file_name().context("path has no name")?;
            Ok(resolve_existing_ancestor(parent)?.join(name))
        }
        Err(error) => Err(error.into()),
    }
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no arguments or memory preconditions.
    unsafe { libc::geteuid() }
}

/// Resolve the account home independently of a launcher-provided HOME environment variable.
pub fn account_home() -> Result<PathBuf> {
    use std::{
        ffi::{CStr, OsString},
        mem::MaybeUninit,
        os::unix::ffi::OsStringExt,
    };
    let mut record = MaybeUninit::<libc::passwd>::uninit();
    let mut buffer = vec![0_u8; 65536];
    let mut result = std::ptr::null_mut();
    // SAFETY: record and buffer are valid output allocations; result is checked before reading.
    let status = unsafe {
        libc::getpwuid_r(
            effective_uid(),
            record.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        )
    };
    ensure!(
        status == 0 && !result.is_null(),
        "cannot resolve account home for protection"
    );
    // SAFETY: getpwuid_r returned a valid record and its strings remain in the live buffer.
    let record = unsafe { record.assume_init() };
    ensure!(!record.pw_dir.is_null(), "account has no home directory");
    // SAFETY: pw_dir is a NUL-terminated string populated by getpwuid_r.
    let bytes = unsafe { CStr::from_ptr(record.pw_dir) }.to_bytes().to_vec();
    let home = PathBuf::from(OsString::from_vec(bytes));
    clean_absolute(&home)?;
    Ok(home)
}
