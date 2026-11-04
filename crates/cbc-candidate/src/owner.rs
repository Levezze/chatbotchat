//! Private owner administration over the already-open core, never a model-facing tool.
//! A provisioning manifest is owner-trusted input after actual host corroboration. Its evidence
//! reference is an audit label, not proof supplied by an untrusted caller.
use crate::{
    bindings::{valid_id, Role},
    enrollment::{
        ConnectionKind, NativeHost, SessionEvidence, MAX_DIRECTORY_SUMMARY_BYTES,
        MAX_DIRECTORY_SUMMARY_CHARS,
    },
    store::Store,
    Sandbox,
};
use anyhow::{ensure, Context, Result};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
use subtle::ConstantTimeEq;

const MAX_SPEC_BYTES: usize = 64 * 1024;
const MAX_MEMBERS: usize = 64;
const MAX_GRANTS: usize = 256;
const MAX_PROFILE_BYTES: usize = 128 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OwnerNativeHost {
    Codex,
    Claude,
}

impl OwnerNativeHost {
    fn native(self) -> NativeHost {
        match self {
            Self::Codex => NativeHost::Codex,
            Self::Claude => NativeHost::Claude,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvisionMember {
    pub member: String,
    pub role: String,
    pub host: OwnerNativeHost,
    pub session: String,
    pub evidence_ref: String,
    pub repository: String,
    pub label: String,
    pub summary: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvisionGrant {
    pub requester: String,
    pub target: String,
    pub kind: ConnectionKind,
    pub scope_ref: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvisionSpec {
    pub context: String,
    pub device: String,
    pub members: Vec<ProvisionMember>,
    pub grants: Vec<ProvisionGrant>,
}

/// Possession of this candidate-local key authorizes trusted setup, not arbitrary host actions.
/// No Serialize/Debug implementation: never print its authentication material.
pub struct OwnerControl {
    root: PathBuf,
    key_path: PathBuf,
    digest: [u8; 32],
    provisioning: tokio::sync::Mutex<()>,
}

impl OwnerControl {
    pub fn open(sandbox: &Sandbox) -> Result<Self> {
        let root = sandbox.root().to_path_buf();
        validate_root(&root)?;
        let key_path = root.join("owner.key");
        let token = match read_key(&key_path) {
            Ok(token) => token,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                // A dangling symlink is not a missing key and must never be replaced.
                ensure!(
                    fs::symlink_metadata(&key_path)
                        .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
                    "unsafe owner key"
                );
                let mut random = [0_u8; 32];
                OsRng.try_fill_bytes(&mut random)?;
                let token = format!(
                    "cbco1_{}",
                    random
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                );
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&key_path)?;
                file.write_all(token.as_bytes())?;
                file.write_all(b"\n")?;
                file.sync_all()?;
                fs::File::open(&root)?.sync_all()?;
                token
            }
            Err(error) => return Err(error.context("candidate owner key unavailable")),
        };
        Ok(Self {
            root,
            key_path,
            digest: Sha256::digest(token.as_bytes()).into(),
            provisioning: tokio::sync::Mutex::new(()),
        })
    }

    pub fn key_path(&self) -> &Path {
        &self.key_path
    }

    pub fn authenticate(&self, token: &str) -> bool {
        // Length/format are public; secret comparisons use fixed-size digests in constant time.
        if token.len() != 70 {
            return false;
        }
        let supplied: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        bool::from(supplied.ct_eq(&self.digest))
    }

    /// Call only after authenticate succeeds on the owner request namespace. The returned
    /// profile contains credentials: write it privately, never render it as a model tool result.
    /// Fresh IDs only. A failed/uncertain setup must be inspected, not replayed to rotate actors.
    pub async fn provision(&self, store: &Store, spec: ProvisionSpec) -> Result<Value> {
        let _provisioning = self.provisioning.lock().await;
        validate_root(&self.root)?;
        spec.validate()?;
        // Profile JSON is emitted only after successful enrollment. Bound the worst case
        // first, using the longest possible epoch and exact token length, so an output-size
        // rejection cannot happen only after mutating the core. Reserve its final newline.
        let bindings: Vec<_> = spec.members.iter().map(|member| json!({
            "context":spec.context,"member":member.member,"device":spec.device,
            "session":member.session,"epoch":i64::MAX,"token":format!("cbcc1_{}", "f".repeat(64))
        })).collect();
        let targets: Vec<_> = spec
            .members
            .iter()
            .map(|member| json!({"session":member.session,"host":member.host}))
            .collect();
        let conservative = json!({"v":1,"core_socket":self.root.join("connector.sock"),"bindings":bindings,"targets":targets});
        ensure!(
            serde_json::to_vec(&conservative)?.len() < MAX_PROFILE_BYTES,
            "private provision profile exceeds bound"
        );
        let database: String =
            sqlx::query_scalar("SELECT file FROM pragma_database_list WHERE name='main'")
                .fetch_one(&store.pool)
                .await?;
        ensure!(
            Path::new(&database) == self.root.join("candidate.sqlite3"),
            "owner belongs to another candidate store"
        );
        // Finish input and existing-owner preflight before creating any state. The core owns
        // the Store lease; this method never opens another database or adopts a live member.
        for member in &spec.members {
            let exists: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM candidate_members WHERE context_id=? AND member_id=?",
            )
            .bind(&spec.context)
            .bind(&member.member)
            .fetch_one(&store.pool)
            .await?;
            ensure!(
                exists == 0,
                "provision member already exists; inspect before recovery"
            );
        }
        let result = async {
            store.create_context(&spec.context).await?;
            for member in &spec.members {
                store.register_member(&spec.context, &member.member, role(&member.role)?).await?;
                store.describe_member(&spec.context, &member.member, &member.repository, &member.label, &member.summary).await?;
            }
            for grant in &spec.grants {
                store.authorize_connection(&spec.context, &grant.requester, &grant.target, grant.kind, &grant.scope_ref).await?;
            }
            let mut bindings = Vec::new();
            let mut targets = Vec::new();
            for member in &spec.members {
                let evidence = SessionEvidence::from_trusted_adapter(member.host.native(), &spec.device, &member.session, &member.evidence_ref)?;
                let invite = store.issue_enrollment_invite(&spec.context, &member.member, &evidence, 300).await?;
                let binding = store.consume_enrollment_invite(invite.expose_token_for_adapter(), &evidence).await?;
                bindings.push(json!({"context":spec.context,"member":member.member,"device":spec.device,
                    "session":member.session,"epoch":binding.epoch(),"token":binding.expose_token_for_adapter()}));
                targets.push(json!({"session":member.session,"host":member.host}));
            }
            Ok::<_, anyhow::Error>(json!({"v":1,"core_socket":self.root.join("connector.sock"),
                "bindings":bindings,"targets":targets}))
        }.await;
        result.context("owner provisioning may be partial on failure; inspect before retry")
    }
}

impl ProvisionSpec {
    pub fn validate(&self) -> Result<()> {
        valid_id(&self.context)?;
        valid_id(&self.device)?;
        ensure!(
            (1..=MAX_MEMBERS).contains(&self.members.len()) && self.grants.len() <= MAX_GRANTS,
            "invalid provision size"
        );
        ensure!(
            serde_json::to_vec(self)?.len() <= MAX_SPEC_BYTES,
            "provision specification exceeds bound"
        );
        let mut roles = BTreeMap::new();
        let mut sessions = BTreeSet::new();
        for member in &self.members {
            for id in [
                &member.member,
                &member.session,
                &member.evidence_ref,
                &member.repository,
                &member.label,
            ] {
                valid_id(id)?;
            }
            let role = role(&member.role)?;
            ensure!(
                roles.insert(member.member.as_str(), role).is_none(),
                "duplicate provision member"
            );
            ensure!(
                sessions.insert(member.session.as_str()),
                "duplicate provision session"
            );
            ensure!(
                member.summary.len() <= MAX_DIRECTORY_SUMMARY_BYTES
                    && member.summary.chars().count() <= MAX_DIRECTORY_SUMMARY_CHARS
                    && !member
                        .summary
                        .chars()
                        .any(|c| c.is_control() && c != '\n' && c != '\t'),
                "invalid provision summary"
            );
        }
        let mut grants = BTreeSet::new();
        for grant in &self.grants {
            for id in [&grant.requester, &grant.target, &grant.scope_ref] {
                valid_id(id)?;
            }
            let from = roles
                .get(grant.requester.as_str())
                .context("grant requester is not in manifest")?;
            let to = roles
                .get(grant.target.as_str())
                .context("grant target is not in manifest")?;
            ensure!(
                grant.requester != grant.target,
                "grant needs another member"
            );
            let eligible = match grant.kind {
                ConnectionKind::Report => matches!(
                    (from, to),
                    (Role::RepoLead, Role::FeatureLead) | (Role::FeatureLead, Role::RepoLead)
                ),
                ConnectionKind::Peer => {
                    from == to && matches!(from, Role::RepoLead | Role::FeatureLead)
                }
            };
            ensure!(eligible, "ineligible provision relationship");
            let kind = match grant.kind {
                ConnectionKind::Report => "report",
                ConnectionKind::Peer => "peer",
            };
            // The underlying relation is symmetric for admission; duplicate reverse grants
            // should not hide a mistaken manifest under an otherwise successful setup.
            let mut pair = [grant.requester.as_str(), grant.target.as_str()];
            pair.sort();
            ensure!(
                grants.insert((pair[0], pair[1], kind, grant.scope_ref.as_str())),
                "duplicate provision relationship"
            );
        }
        Ok(())
    }
}

fn role(value: &str) -> Result<Role> {
    Ok(match value {
        "repo_lead" => Role::RepoLead,
        "feature_lead" => Role::FeatureLead,
        "project_lead" => Role::ProjectLead,
        "participant" => Role::Participant,
        _ => anyhow::bail!("unknown active provision role"),
    })
}

fn validate_root(root: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(root)?;
    ensure!(
        meta.is_dir()
            && !meta.file_type().is_symlink()
            && meta.uid() == crate::effective_uid()
            && meta.mode() & 0o777 == 0o700
            && root.canonicalize()? == root,
        "unsafe owner root"
    );
    Ok(())
}

fn read_key(path: &Path) -> Result<String> {
    let before = fs::symlink_metadata(path)?;
    ensure!(
        before.is_file()
            && before.nlink() == 1
            && before.uid() == crate::effective_uid()
            && before.mode() & 0o777 == 0o600
            && before.len() <= 71,
        "unsafe owner key file"
    );
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    ensure!(
        meta.is_file()
            && meta.nlink() == 1
            && meta.uid() == crate::effective_uid()
            && meta.mode() & 0o777 == 0o600
            && meta.len() <= 71,
        "unsafe owner key file"
    );
    let mut bytes = Vec::new();
    (&mut file).take(72).read_to_end(&mut bytes)?;
    let text = String::from_utf8(bytes).context("invalid owner key")?;
    let token = text.strip_suffix('\n').unwrap_or(&text);
    ensure!(
        token.len() == 70
            && token
                .strip_prefix("cbco1_")
                .is_some_and(|v| v.bytes().all(|b| b.is_ascii_hexdigit())),
        "invalid owner key"
    );
    Ok(token.to_owned())
}
