//! Owner-configured native placement. Audit labels are not native authentication.
use crate::{
    bindings::{valid_id, validate_on, SessionBinding},
    store::Store,
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqliteConnection};
use std::path::Path;

pub const MAX_TARGET_BYTES: usize = 16 * 1024;
pub const MAX_TARGET_PATH_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionProfile {
    Desktop,
    Cli,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum SavedSelection {
    NotExposed,
    Configured { id: String },
}

impl<'de> Deserialize<'de> for SavedSelection {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum Status {
            NotExposed,
            Configured,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            status: Status,
            // Missing is allowed, explicit null is not an identifier.
            #[serde(default, deserialize_with = "optional_label")]
            id: Option<String>,
        }
        let input = Input::deserialize(deserializer)?;
        match (input.status, input.id) {
            (Status::NotExposed, None) => Ok(Self::NotExposed),
            (Status::Configured, Some(id)) => Ok(Self::Configured { id }),
            (Status::NotExposed, Some(_)) => Err(serde::de::Error::custom(
                "not_exposed selection must not contain id",
            )),
            (Status::Configured, None) => Err(serde::de::Error::missing_field("id")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementEvidence {
    pub evidence_ref: String,
    /// Owner-observed audit IDs. Runtime saved selections above remain separate constraints.
    #[serde(
        default,
        deserialize_with = "optional_label",
        skip_serializing_if = "Option::is_none"
    )]
    pub project_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional_label",
        skip_serializing_if = "Option::is_none"
    )]
    pub environment_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional_label",
        skip_serializing_if = "Option::is_none"
    )]
    pub project_label: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional_label",
        skip_serializing_if = "Option::is_none"
    )]
    pub environment_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeTarget {
    pub v: u8,
    pub profile: ExecutionProfile,
    pub repository: String,
    #[serde(deserialize_with = "required_nullable_path")]
    pub worktree: Option<String>,
    pub cwd: String,
    pub saved_project: SavedSelection,
    pub saved_environment: SavedSelection,
    pub placement: PlacementEvidence,
}

fn required_nullable_path<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}

fn optional_label<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    String::deserialize(d).map(Some)
}

pub(crate) fn required_target<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<NativeTarget>, D::Error> {
    Option::<NativeTarget>::deserialize(d)
}

fn path(value: &str) -> Result<()> {
    ensure!(
        value.len() <= MAX_TARGET_PATH_BYTES
            && value.starts_with('/')
            && !value.chars().any(char::is_control)
            && !value.contains("//")
            && (value == "/" || !value.ends_with('/'))
            && !value.split('/').any(|part| part == "." || part == ".."),
        "native target paths must be absolute and normalized"
    );
    Ok(())
}

impl NativeTarget {
    /// Lexical only: the core may administer a different device's filesystem.
    /// Trusted adapters must compare against actual host observations at runtime.
    pub fn validate(&self) -> Result<()> {
        ensure!(self.v == 1, "unsupported native target version");
        ensure!(
            serde_json::to_vec(self)?.len() <= MAX_TARGET_BYTES,
            "native target exceeds bound"
        );
        valid_id(&self.repository)?;
        path(&self.cwd)?;
        if let Some(worktree) = &self.worktree {
            path(worktree)?;
            ensure!(
                Path::new(&self.cwd).starts_with(worktree),
                "native target cwd is outside worktree"
            );
        }
        for selector in [&self.saved_project, &self.saved_environment] {
            if let SavedSelection::Configured { id } = selector {
                valid_id(id)?;
            }
        }
        valid_id(&self.placement.evidence_ref)?;
        for label in [
            &self.placement.project_id,
            &self.placement.environment_id,
            &self.placement.project_label,
            &self.placement.environment_label,
        ]
        .into_iter()
        .flatten()
        {
            valid_id(label)?;
        }
        Ok(())
    }

    pub(crate) fn encoded(&self) -> Result<String> {
        self.validate()?;
        Ok(serde_json::to_string(self)?)
    }

    pub(crate) fn digest(&self) -> Result<Vec<u8>> {
        Ok(Sha256::digest(self.encoded()?.as_bytes()).to_vec())
    }
}

impl Store {
    /// Target for this exact current binding epoch. Legacy fixtures have no target.
    /// Old target history remains stored, but a revoked credential cannot read it.
    pub async fn native_target(&self, binding: &SessionBinding) -> Result<Option<NativeTarget>> {
        let mut tx = self.pool.begin().await?;
        validate_on(&mut tx, binding).await?;
        let target = target_on(&mut tx, binding).await?;
        tx.commit().await?;
        Ok(target)
    }
}

/// Caller must validate the binding in this same transaction before reading its target.
pub(crate) async fn target_on(
    conn: &mut SqliteConnection,
    binding: &SessionBinding,
) -> Result<Option<NativeTarget>> {
    let row = sqlx::query("SELECT target_json,target_digest,device_id,native_session_id FROM candidate_native_targets WHERE context_id=? AND member_id=? AND epoch=?")
        .bind(&binding.context_id).bind(&binding.member_id).bind(binding.epoch)
        .fetch_optional(&mut *conn).await?;
    let Some(row) = row else {
        let configured: i64 = sqlx::query_scalar("SELECT count(*) FROM candidate_enrollment_invites WHERE context_id=? AND member_id=? AND consumed_epoch=? AND (target_json IS NOT NULL OR target_digest IS NOT NULL)")
            .bind(&binding.context_id).bind(&binding.member_id).bind(binding.epoch)
            .fetch_one(&mut *conn).await?;
        ensure!(configured == 0, "configured native target is missing");
        return Ok(None);
    };
    ensure!(
        row.try_get::<String, _>("device_id")? == binding.device_id
            && row.try_get::<String, _>("native_session_id")? == binding.native_session_id,
        "stored native target binding mismatch"
    );
    let raw: String = row.try_get("target_json")?;
    ensure!(
        raw.len() <= MAX_TARGET_BYTES,
        "stored native target exceeds bound"
    );
    let target: NativeTarget = serde_json::from_str(&raw)?;
    target.validate()?;
    ensure!(
        target.digest()? == row.try_get::<Vec<u8>, _>("target_digest")?,
        "stored native target digest mismatch"
    );
    Ok(Some(target))
}
