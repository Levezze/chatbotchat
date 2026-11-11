//! Candidate-owned runtime policy. No agent authority or native permission is configured here.
use crate::Sandbox;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const MAX_BYTES: usize = 64 * 1024;
const MAX_OVERRIDES: usize = 256;
const SETTINGS: &str = "runtime-settings.json";
const LAST_GOOD: &str = "runtime-settings.last-good.json";
const ROLES: &[&str] = &[
    "project_lead",
    "repo_lead",
    "feature_lead",
    "participant",
    "domain_lead",
    "task_agent",
];
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchPolicy {
    pub retry_initial_ms: u64,
    pub retry_max_ms: u64,
    /// Participant-message milestones only; this never limits conversation length.
    /// Rooms resolve the report owner's policy, or defaults/context only for symmetric peers.
    #[serde(default = "default_room_notice_every_messages")]
    pub room_notice_every_messages: u64,
    /// How long an unread item waits, with no wake attempt in between, before the driver
    /// re-hints the member (#113 B1).
    #[serde(default = "default_recovery_after_ms")]
    pub recovery_after_ms: u64,
    /// Recovery hints per oldest unread item; 0 turns recovery off.
    #[serde(default = "default_recovery_attempts")]
    pub recovery_attempts: u32,
}

fn default_room_notice_every_messages() -> u64 {
    100
}

fn default_recovery_after_ms() -> u64 {
    600_000
}

fn default_recovery_attempts() -> u32 {
    3
}

impl Default for DispatchPolicy {
    fn default() -> Self {
        Self {
            retry_initial_ms: 1000,
            retry_max_ms: 30_000,
            room_notice_every_messages: default_room_notice_every_messages(),
            recovery_after_ms: default_recovery_after_ms(),
            recovery_attempts: default_recovery_attempts(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_initial_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_max_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room_notice_every_messages: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_attempts: Option<u32>,
}

impl PolicyPatch {
    fn apply(&self, policy: &mut DispatchPolicy) {
        if let Some(value) = self.retry_initial_ms {
            policy.retry_initial_ms = value;
        }
        if let Some(value) = self.retry_max_ms {
            policy.retry_max_ms = value;
        }
        if let Some(value) = self.room_notice_every_messages {
            policy.room_notice_every_messages = value;
        }
        if let Some(value) = self.recovery_after_ms {
            policy.recovery_after_ms = value;
        }
        if let Some(value) = self.recovery_attempts {
            policy.recovery_attempts = value;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSettings {
    pub schema: u32,
    pub defaults: DispatchPolicy,
    #[serde(default)]
    pub contexts: BTreeMap<String, PolicyPatch>,
    #[serde(default)]
    pub roles: BTreeMap<String, PolicyPatch>,
    /// Member IDs are scoped by context, never a global member-name lookup.
    #[serde(default)]
    pub members: BTreeMap<String, BTreeMap<String, PolicyPatch>>,
    /// The live log level of the core and the driver (B2). Left out when normal, so a file
    /// written before B2 keeps its revision.
    #[serde(default, skip_serializing_if = "is_normal")]
    pub log_level: crate::sampler::Mode,
}

fn is_normal(mode: &crate::sampler::Mode) -> bool {
    *mode == crate::sampler::Mode::Normal
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            schema: 1,
            defaults: DispatchPolicy::default(),
            contexts: BTreeMap::new(),
            roles: BTreeMap::new(),
            members: BTreeMap::new(),
            log_level: crate::sampler::Mode::Normal,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveSettings {
    #[serde(flatten)]
    pub policy: DispatchPolicy,
    pub sources: Vec<String>,
}

impl RuntimeSettings {
    /// Fixed precedence: defaults, context, role, then context-scoped member.
    pub fn effective(&self, context: &str, role: &str, member: &str) -> EffectiveSettings {
        let mut policy = self.defaults.clone();
        let mut sources = vec!["defaults".into()];
        for (patch, source) in [
            (self.contexts.get(context), "context"),
            (self.roles.get(role), "role"),
            (
                self.members
                    .get(context)
                    .and_then(|entries| entries.get(member)),
                "member",
            ),
        ] {
            if let Some(patch) = patch {
                patch.apply(&mut policy);
                if *patch != PolicyPatch::default() {
                    sources.push(source.into());
                }
            }
        }
        EffectiveSettings { policy, sources }
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema == 1, "unsupported runtime settings schema");
        validate_policy(&self.defaults)?;
        let count = self.contexts.len()
            + self.roles.len()
            + self.members.values().map(BTreeMap::len).sum::<usize>();
        ensure!(count <= MAX_OVERRIDES, "too many runtime policy overrides");
        let mut contexts: BTreeSet<&str> = self.contexts.keys().map(String::as_str).collect();
        contexts.extend(self.members.keys().map(String::as_str));
        contexts.insert(""); // Also validate roles outside explicitly configured contexts.
        for context in self.contexts.keys().chain(self.members.keys()) {
            valid_key(context)?;
        }
        for role in self.roles.keys() {
            ensure!(
                ROLES.contains(&role.as_str()),
                "unknown runtime policy role"
            );
        }
        for members in self.members.values() {
            for member in members.keys() {
                valid_key(member)?;
            }
        }
        // Validate composed values, including patches individually hidden by a member override.
        for context in contexts {
            let members = self.members.get(context);
            for role in ROLES {
                validate_policy(&self.effective(context, role, "").policy)?;
                if let Some(members) = members {
                    for member in members.keys() {
                        validate_policy(&self.effective(context, role, member).policy)?;
                    }
                }
            }
            let mut context_only = self.defaults.clone();
            if let Some(patch) = self.contexts.get(context) {
                patch.apply(&mut context_only);
            }
            validate_policy(&context_only)?;
        }
        ensure!(
            serde_json::to_vec_pretty(self)?.len() < MAX_BYTES,
            "runtime settings exceed size bound"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsSnapshot {
    pub revision: String,
    pub source: PathBuf,
    pub recovered_last_good: bool,
    pub last_error: Option<String>,
    pub settings: RuntimeSettings,
}

impl SettingsSnapshot {
    pub fn effective(&self, context: &str, role: &str, member: &str) -> EffectiveSettings {
        self.settings.effective(context, role, member)
    }
}

/// One instance belongs to the core; call refresh at settings/use boundaries, not on an idle poll.
/// The parent serializes access. Managed updates compare disk's current accepted revision first.
pub struct RuntimeSettingsService {
    root: PathBuf,
    snapshot: SettingsSnapshot,
}

impl RuntimeSettingsService {
    pub fn open(sandbox: &Sandbox) -> Result<Self> {
        let root = sandbox.root().to_path_buf();
        validate_root(&root)?;
        let _lock = settings_lock(&root)?;
        let primary = root.join(SETTINGS);
        let checkpoint = root.join(LAST_GOOD);
        let (settings, recovered, error) = match read_settings(&primary) {
            Ok(settings) => (settings, false, None),
            Err(error) => {
                if let Ok(settings) = read_settings(&checkpoint) {
                    (settings, true, Some("primary runtime settings unavailable or invalid; using last accepted policy".into()))
                } else if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
                    && missing_path(&primary)?
                    && missing_path(&checkpoint)?
                {
                    let settings = RuntimeSettings::default();
                    publish(&root, SETTINGS, &settings, true)?;
                    (settings, false, None)
                } else {
                    return Err(
                        error.context("no validated runtime settings or last-good checkpoint")
                    );
                }
            }
        };
        if !recovered {
            publish(&root, LAST_GOOD, &settings, false)?;
        }
        Ok(Self {
            snapshot: make_snapshot(&root, settings, recovered, error)?,
            root,
        })
    }

    pub fn snapshot(&self) -> SettingsSnapshot {
        self.snapshot.clone()
    }

    /// Invalid edits retain the accepted policy and report a bounded diagnostic; never default-reset.
    pub fn refresh(&mut self) -> SettingsSnapshot {
        match settings_lock(&self.root) {
            Ok(_lock) => self.refresh_unlocked(),
            Err(_) => {
                self.snapshot.last_error =
                    Some("runtime settings unavailable; retaining last accepted policy".into());
                self.snapshot()
            }
        }
    }

    fn refresh_unlocked(&mut self) -> SettingsSnapshot {
        let update = (|| -> Result<()> {
            validate_root(&self.root)?;
            let settings = read_settings(&self.root.join(SETTINGS))?;
            let revision = revision(&settings)?;
            if revision != self.snapshot.revision {
                publish(&self.root, LAST_GOOD, &settings, false)?;
                self.snapshot = make_snapshot(&self.root, settings, false, None)?;
            } else {
                self.snapshot.last_error = None;
                self.snapshot.recovered_last_good = false;
            }
            Ok(())
        })();
        if update.is_err() {
            self.snapshot.last_error =
                Some("runtime settings rejected; retaining last accepted policy".into());
        }
        self.snapshot()
    }

    /// Trusted local management API, not exposed to ordinary members or native model arguments.
    /// A malformed current external file must be repaired explicitly before management CAS.
    pub fn update(
        &mut self,
        expected_revision: &str,
        settings: RuntimeSettings,
    ) -> Result<SettingsSnapshot> {
        settings.validate()?;
        let _lock = settings_lock(&self.root)?;
        self.refresh_unlocked();
        ensure!(
            self.snapshot.last_error.is_none(),
            "repair rejected external settings before update"
        );
        ensure!(
            self.snapshot.revision == expected_revision,
            "runtime settings revision conflict"
        );
        publish(&self.root, SETTINGS, &settings, false)?;
        // Accepted primary is durable first. If checkpoint publication fails, report failure;
        // a later refresh/reopen can recover it without inventing successful propagation.
        publish(&self.root, LAST_GOOD, &settings, false)?;
        self.snapshot = make_snapshot(&self.root, settings, false, None)?;
        Ok(self.snapshot())
    }
}

fn make_snapshot(
    root: &Path,
    settings: RuntimeSettings,
    recovered: bool,
    error: Option<String>,
) -> Result<SettingsSnapshot> {
    // Every accepted snapshot passes here, so the live log level follows the settings file.
    crate::log::set_level(settings.log_level);
    Ok(SettingsSnapshot {
        revision: revision(&settings)?,
        source: root.join(SETTINGS),
        recovered_last_good: recovered,
        last_error: error,
        settings,
    })
}

fn revision(settings: &RuntimeSettings) -> Result<String> {
    let digest = Sha256::digest(serde_json::to_vec(settings)?);
    Ok(format!("cfg1_{digest:x}"))
}

fn valid_key(key: &str) -> Result<()> {
    ensure!(
        !key.trim().is_empty() && key.len() <= 128 && !key.chars().any(char::is_control),
        "invalid runtime policy scope"
    );
    Ok(())
}

fn validate_policy(policy: &DispatchPolicy) -> Result<()> {
    ensure!(
        (25..=300_000).contains(&policy.retry_initial_ms)
            && (25..=300_000).contains(&policy.retry_max_ms)
            && policy.retry_initial_ms <= policy.retry_max_ms,
        "invalid runtime retry policy"
    );
    ensure!(
        (1..=1_000_000).contains(&policy.room_notice_every_messages),
        "invalid room notice cadence"
    );
    ensure!(
        (1000..=86_400_000).contains(&policy.recovery_after_ms) && policy.recovery_attempts <= 20,
        "invalid recovery policy"
    );
    Ok(())
}

fn validate_root(root: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(root)?;
    ensure!(
        meta.is_dir()
            && !meta.file_type().is_symlink()
            && meta.uid() == crate::effective_uid()
            && meta.mode() & 0o777 == 0o700
            && root.canonicalize()? == root,
        "unsafe runtime settings root"
    );
    Ok(())
}

fn validate_file(meta: &fs::Metadata) -> Result<()> {
    ensure!(
        meta.is_file()
            && meta.nlink() == 1
            && meta.uid() == crate::effective_uid()
            && meta.mode() & 0o777 == 0o600
            && meta.len() <= MAX_BYTES as u64,
        "unsafe runtime settings file"
    );
    Ok(())
}

fn missing_path(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error.into()),
    }
}

fn settings_lock(root: &Path) -> Result<fs::File> {
    use std::os::fd::AsRawFd;
    validate_root(root)?;
    let path = root.join("runtime-settings.lock");
    match fs::symlink_metadata(&path) {
        Ok(meta) => validate_file(&meta)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    validate_file(&file.metadata()?)?;
    // All management API writers cooperate on this lock, including separate CLI processes.
    // Manual file editors are expected to publish complete files atomically.
    ensure!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "runtime settings busy or unavailable"
    );
    Ok(file)
}

/// The log level in the settings file as it stands, for an observer such as the sampler: no lock,
/// no writes. None when the file is missing or invalid; the core then keeps its last accepted
/// settings, which `accepted_log_level` reads.
pub fn observed_log_level(root: &Path) -> Option<crate::sampler::Mode> {
    read_settings(&root.join(SETTINGS))
        .ok()
        .map(|s| s.log_level)
}

/// The level in the last accepted settings (the last-good checkpoint), as it stands.
pub fn accepted_log_level(root: &Path) -> Option<crate::sampler::Mode> {
    read_settings(&root.join(LAST_GOOD))
        .ok()
        .map(|s| s.log_level)
}

fn read_settings(path: &Path) -> Result<RuntimeSettings> {
    validate_file(&fs::symlink_metadata(path)?)?;
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    validate_file(&file.metadata()?)?;
    let mut bytes = Vec::new();
    (&mut file)
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_BYTES,
        "runtime settings exceed size bound"
    );
    let settings: RuntimeSettings =
        serde_json::from_slice(&bytes).context("invalid runtime settings JSON/schema")?;
    settings.validate()?;
    Ok(settings)
}

fn publish(root: &Path, name: &str, settings: &RuntimeSettings, create_only: bool) -> Result<()> {
    validate_root(root)?;
    let target = root.join(name);
    match fs::symlink_metadata(&target) {
        Ok(meta) => {
            ensure!(!create_only, "runtime settings already exist");
            validate_file(&meta)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let bytes = serde_json::to_vec_pretty(settings)?;
    ensure!(
        bytes.len() < MAX_BYTES,
        "runtime settings exceed size bound"
    );
    let temporary = root.join(format!(
        ".runtime-settings-{}-{}.tmp",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut created = false;
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        created = true;
        file.write_all(&bytes)?;
        file.sync_all()?;
        if create_only {
            // Atomic no-clobber publication; remove the temporary link before any later validation.
            fs::hard_link(&temporary, &target)?;
            fs::remove_file(&temporary)?;
        } else {
            fs::rename(&temporary, &target)?;
        }
        fs::File::open(root)?.sync_all()?;
        Ok(())
    })();
    if created {
        let _ = fs::remove_file(&temporary);
    }
    result
}
