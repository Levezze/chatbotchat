//! Synthetic binding primitives. Provisioning is trusted fixture administration, not enrollment.
use crate::store::Store;
use anyhow::{anyhow, ensure, Context, Result};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqliteConnection};
use std::fmt;
use subtle::ConstantTimeEq;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    ProjectLead,
    RepoLead,
    DomainLead,
    FeatureLead,
    TaskAgent,
    Participant,
}

/// Host-adapter credential. Never serialize this into model-facing tool results or logs.
pub struct SessionBinding {
    pub(crate) context_id: String,
    pub(crate) member_id: String,
    pub(crate) device_id: String,
    pub(crate) native_session_id: String,
    pub(crate) epoch: i64,
    token: String,
}

impl fmt::Debug for SessionBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionBinding")
            .field("credential", &"[REDACTED]")
            .finish()
    }
}

impl fmt::Display for SessionBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionBinding([REDACTED])")
    }
}

impl SessionBinding {
    /// Parsing claims is not authorization. Every operation must validate the binding anew.
    pub fn from_token(
        context: &str,
        member: &str,
        device: &str,
        session: &str,
        epoch: i64,
        token: &str,
    ) -> Result<Self> {
        for id in [context, member, device, session] {
            valid_id(id)?;
        }
        ensure!(epoch > 0, "invalid binding credential");
        let secret = token
            .strip_prefix("cbcc1_")
            .context("invalid binding credential")?;
        ensure!(
            secret.len() == 64 && secret.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid binding credential"
        );
        Ok(Self {
            context_id: context.into(),
            member_id: member.into(),
            device_id: device.into(),
            native_session_id: session.into(),
            epoch,
            token: token.into(),
        })
    }

    /// Explicit secret access for a trusted host adapter, not for agent chat/model output.
    pub fn expose_token_for_adapter(&self) -> &str {
        &self.token
    }
    pub fn epoch(&self) -> i64 {
        self.epoch
    }
}

impl Role {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ProjectLead => "project_lead",
            Self::RepoLead => "repo_lead",
            Self::DomainLead => "domain_lead",
            Self::FeatureLead => "feature_lead",
            Self::TaskAgent => "task_agent",
            Self::Participant => "participant",
        }
    }
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "project_lead" => Ok(Self::ProjectLead),
            "repo_lead" => Ok(Self::RepoLead),
            "domain_lead" => Ok(Self::DomainLead),
            "feature_lead" => Ok(Self::FeatureLead),
            "task_agent" => Ok(Self::TaskAgent),
            "participant" => Ok(Self::Participant),
            _ => Err(anyhow!("invalid stored role")),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct MemberStatus {
    pub context_id: String,
    pub member_id: String,
    pub role: Role,
    pub device_id: String,
    pub native_session_id: String,
    pub epoch: i64,
}

/// Definitive credential/binding invalidation, distinct from storage or transport failure.
#[derive(Debug, Clone, Copy)]
pub struct InvalidBinding;

impl fmt::Display for InvalidBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid or superseded binding")
    }
}

impl std::error::Error for InvalidBinding {}

/// Content-free snapshot for device reconciliation. No batch is created or acknowledged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingActivity {
    pub last_receipted_sequence: i64,
    pub first_unread_sequence: Option<i64>,
    #[serde(deserialize_with = "crate::native_target::required_target")]
    pub native_target: Option<crate::native_target::NativeTarget>,
}

impl Store {
    /// Trusted fixture administration. A removed member retains its epoch history.
    pub async fn remove_member(&self, context: &str, member: &str) -> Result<()> {
        valid_id(context)?;
        valid_id(member)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let changed = sqlx::query("UPDATE candidate_members SET active = 0, epoch = epoch + 1, device_id = NULL, native_session_id = NULL, credential_digest = NULL WHERE context_id = ? AND member_id = ? AND active = 1 AND epoch < 9223372036854775807 RETURNING epoch")
            .bind(context).bind(member).fetch_optional(&mut *tx).await?;
        ensure!(changed.is_some(), "member is unavailable for removal");
        sqlx::query(
            "UPDATE candidate_room_members SET active=0 WHERE context_id=? AND member_id=?",
        )
        .bind(context)
        .bind(member)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE candidate_route_grants SET active=0 WHERE context_id=? AND (first_member=? OR second_member=?)")
            .bind(context).bind(member).bind(member).execute(&mut *tx).await?;
        sqlx::query("UPDATE candidate_batches SET state='cancelled' WHERE context_id=? AND member_id=? AND state='pending'")
            .bind(context).bind(member).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Trusted fixture administration. Unbinding keeps the logical member registered.
    pub async fn unbind_member(&self, context: &str, member: &str) -> Result<()> {
        valid_id(context)?;
        valid_id(member)?;
        let changed = sqlx::query("UPDATE candidate_members SET epoch = epoch + 1, device_id = NULL, native_session_id = NULL, credential_digest = NULL WHERE context_id = ? AND member_id = ? AND active = 1 AND epoch < 9223372036854775807 RETURNING epoch")
            .bind(context).bind(member).fetch_optional(&self.pool).await?;
        ensure!(changed.is_some(), "member is unavailable for unbinding");
        Ok(())
    }
    /// Trusted owner revocation of exactly one observed binding. A delayed request cannot
    /// revoke a newer session, and retrying a successful request never advances the epoch.
    /// Memberships and unread history remain available to a later explicit enrollment.
    pub async fn revoke_binding(
        &self,
        context: &str,
        member: &str,
        expected_epoch: i64,
    ) -> Result<i64> {
        valid_id(context)?;
        valid_id(member)?;
        ensure!(
            expected_epoch > 0 && expected_epoch < i64::MAX,
            "invalid expected binding epoch"
        );
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let revoked_epoch: Option<i64> = sqlx::query_scalar(
            "UPDATE candidate_members SET epoch=epoch+1,device_id=NULL,native_session_id=NULL,credential_digest=NULL WHERE context_id=? AND member_id=? AND active=1 AND epoch=? AND epoch<9223372036854775807 AND device_id IS NOT NULL AND native_session_id IS NOT NULL AND credential_digest IS NOT NULL RETURNING epoch",
        )
        .bind(context)
        .bind(member)
        .bind(expected_epoch)
        .fetch_optional(&mut *tx)
        .await?;
        let revoked_epoch = revoked_epoch.context("binding is unavailable at expected epoch")?;
        sqlx::query("UPDATE candidate_batches SET state='cancelled' WHERE context_id=? AND member_id=? AND state='pending'")
            .bind(context).bind(member).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(revoked_epoch)
    }
    /// Trusted fixture administration only. This is not public enrollment or a role grant API.
    pub async fn register_member(&self, context: &str, member: &str, role: Role) -> Result<()> {
        valid_id(context)?;
        valid_id(member)?;
        let registered = sqlx::query("INSERT INTO candidate_members(context_id, member_id, role) VALUES (?, ?, ?) ON CONFLICT(context_id, member_id) DO UPDATE SET active = 1 WHERE candidate_members.role = excluded.role RETURNING member_id")
            .bind(context).bind(member).bind(role.as_str()).fetch_optional(&self.pool).await?;
        ensure!(
            registered.is_some(),
            "member registration conflicts with existing attributes"
        );
        Ok(())
    }
    /// Each bind rotates the credential and epoch, including retries after a lost response.
    pub async fn bind_member(
        &self,
        context: &str,
        member: &str,
        device: &str,
        session: &str,
    ) -> Result<SessionBinding> {
        for id in [context, member, device, session] {
            valid_id(id)?;
        }
        let mut random = [0_u8; 32];
        OsRng
            .try_fill_bytes(&mut random)
            .context("cannot generate binding credential")?;
        let token = format!(
            "cbcc1_{}",
            random
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let digest = Sha256::digest(token.as_bytes()).to_vec();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        // Reserve i64::MAX for final revocation so the last issued credential stays revocable.
        let row = sqlx::query("UPDATE candidate_members SET epoch = epoch + 1, device_id = ?, native_session_id = ?, credential_digest = ? WHERE context_id = ? AND member_id = ? AND active = 1 AND epoch < 9223372036854775806 RETURNING epoch")
            .bind(device).bind(session).bind(digest).bind(context).bind(member).fetch_optional(&mut *tx).await?;
        let epoch = row
            .context("member is unavailable for binding")?
            .try_get("epoch")?;
        sqlx::query("UPDATE candidate_batches SET state='superseded' WHERE context_id=? AND member_id=? AND state='pending'")
            .bind(context).bind(member).execute(&mut *tx).await?;
        tx.commit().await?;
        SessionBinding::from_token(context, member, device, session, epoch, &token)
    }
    /// Snapshot only: future send/check/patch actions must validate inside their own transaction.
    pub async fn current_member(&self, binding: &SessionBinding) -> Result<MemberStatus> {
        validate_on(&mut *self.pool.acquire().await?, binding).await
    }

    /// None means only a definitively invalid credential/session. Operational errors propagate
    /// so a device must not prune a healthy recipient merely because its core is unavailable.
    pub async fn binding_activity(
        &self,
        binding: &SessionBinding,
    ) -> Result<Option<BindingActivity>> {
        let mut tx = self.pool.begin().await?;
        if let Err(error) = validate_on(&mut tx, binding).await {
            if error.downcast_ref::<InvalidBinding>().is_some() {
                return Ok(None);
            }
            return Err(error);
        }
        let last_receipted_sequence: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence),0) FROM candidate_receipts WHERE context_id=? AND member_id=?",
        )
        .bind(&binding.context_id)
        .bind(&binding.member_id)
        .fetch_one(&mut *tx)
        .await?;
        let first_unread_sequence: Option<i64> = sqlx::query_scalar(
            "SELECT MIN(sequence) FROM candidate_unread_items WHERE context_id=? AND recipient=?",
        )
        .bind(&binding.context_id)
        .bind(&binding.member_id)
        .fetch_one(&mut *tx)
        .await?;
        let native_target = crate::native_target::target_on(&mut tx, binding).await?;
        tx.commit().await?;
        Ok(Some(BindingActivity {
            last_receipted_sequence,
            first_unread_sequence,
            native_target,
        }))
    }
}

/// Validate inside the operation's transaction, never as a separate authorization snapshot.
pub(crate) async fn validate_on(
    conn: &mut SqliteConnection,
    binding: &SessionBinding,
) -> Result<MemberStatus> {
    let row = sqlx::query("SELECT role, epoch, device_id, native_session_id, credential_digest FROM candidate_members WHERE context_id = ? AND member_id = ? AND active = 1")
            .bind(&binding.context_id).bind(&binding.member_id).fetch_optional(conn).await?;
    let stored: Option<Vec<u8>> = row
        .as_ref()
        .map(|r| r.try_get("credential_digest"))
        .transpose()?
        .flatten();
    let supplied = Sha256::digest(binding.token.as_bytes());
    let dummy = [0_u8; 32];
    let matches = bool::from(
        stored
            .as_deref()
            .unwrap_or(&dummy)
            .ct_eq(supplied.as_slice()),
    );
    if !matches || stored.is_none() {
        return Err(InvalidBinding.into());
    }
    let row = row.ok_or(InvalidBinding)?;
    let epoch: i64 = row.try_get("epoch")?;
    let device: Option<String> = row.try_get("device_id")?;
    let session: Option<String> = row.try_get("native_session_id")?;
    if epoch != binding.epoch
        || device.as_deref() != Some(&binding.device_id)
        || session.as_deref() != Some(&binding.native_session_id)
    {
        return Err(InvalidBinding.into());
    }
    Ok(MemberStatus {
        context_id: binding.context_id.clone(),
        member_id: binding.member_id.clone(),
        role: Role::parse(row.try_get("role")?)?,
        device_id: binding.device_id.clone(),
        native_session_id: binding.native_session_id.clone(),
        epoch,
    })
}

pub(crate) fn valid_id(id: &str) -> Result<()> {
    ensure!(
        !id.trim().is_empty() && id.len() <= 128 && !id.chars().any(char::is_control),
        "invalid binding identifier"
    );
    Ok(())
}
