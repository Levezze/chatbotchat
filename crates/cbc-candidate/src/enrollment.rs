//! Scoped local enrollment and connection directory.
//!
//! Provisioning/invite/grant methods are trusted owner administration, not model tools.
//! A native adapter must corroborate its host input before constructing SessionEvidence;
//! the core cannot turn a claimed session ID, token or evidence-reference string into proof.
use crate::{
    bindings::{valid_id, validate_on, Role, SessionBinding},
    native_target::NativeTarget,
    store::Store,
};
use anyhow::{ensure, Context, Result};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqliteConnection};
use std::{
    fmt,
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_DISCOVERY: usize = 64;
pub const MAX_DIRECTORY_SUMMARY_CHARS: usize = 160;
pub const MAX_DIRECTORY_SUMMARY_BYTES: usize = 512;
pub const MAX_INTRODUCTION_CHARS: usize = 600;
pub const MAX_INTRODUCTION_BYTES: usize = 2048;
pub const MAX_INVITE_TTL_SECONDS: u32 = 3600;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeHost {
    Codex,
    Claude,
}
impl NativeHost {
    fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }
}

/// Adapter-only input. Not deserializable from a model tool or an untrusted request body.
pub struct SessionEvidence {
    host: NativeHost,
    device: String,
    session: String,
    reference: String,
    target: Option<NativeTarget>,
}
impl SessionEvidence {
    /// Call only after the adapter verifies host-owned metadata/process correlation. This
    /// constructor validates bounds; it does not perform native identity verification itself.
    pub fn from_trusted_adapter(
        host: NativeHost,
        device: &str,
        session: &str,
        reference: &str,
    ) -> Result<Self> {
        for value in [device, session, reference] {
            valid_id(value)?;
        }
        Ok(Self {
            host,
            device: device.into(),
            session: session.into(),
            reference: reference.into(),
            target: None,
        })
    }

    /// Adds a configured target only after trusted native observations have been checked.
    /// This validates representation, not host placement or saved-environment availability.
    pub fn with_target(mut self, target: NativeTarget) -> Result<Self> {
        target.validate()?;
        self.target = Some(target);
        Ok(self)
    }
}

/// One-use secret, handed to the trusted adapter through a private channel.
pub struct EnrollmentInvite {
    token: String,
    pub expires_at: i64,
}
impl EnrollmentInvite {
    pub fn expose_token_for_adapter(&self) -> &str {
        &self.token
    }
}
impl fmt::Debug for EnrollmentInvite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EnrollmentInvite([REDACTED])")
    }
}
impl fmt::Display for EnrollmentInvite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EnrollmentInvite([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionKind {
    Report,
    Peer,
}
impl ConnectionKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Report => "report",
            Self::Peer => "peer",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryEntry {
    pub member: String,
    pub role: String,
    pub repository: String,
    pub label: String,
    pub summary: String,
    pub connection_kind: ConnectionKind,
    pub scope_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    pub room: String,
    pub kind: ConnectionKind,
    pub scope_ref: String,
    pub introduction_sequence: i64,
    pub reused: bool,
    pub closed: bool,
    pub generation: i64,
    pub supersedes: Option<String>,
    pub closure: Option<RoomClosure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomClosure {
    pub by: String,
    pub reason: String,
}

fn now() -> Result<i64> {
    Ok(i64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    )?)
}
fn secret(prefix: &str) -> Result<String> {
    let mut bytes = [0_u8; 32];
    OsRng.try_fill_bytes(&mut bytes)?;
    Ok(format!(
        "{prefix}{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ))
}
fn introduction(value: &str) -> Result<()> {
    ensure!(
        value.len() <= MAX_INTRODUCTION_BYTES && value.chars().count() <= MAX_INTRODUCTION_CHARS,
        "introduction exceeds bounds"
    );
    ensure!(
        !value
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t'),
        "invalid introduction"
    );
    Ok(())
}
fn pair<'a>(
    requester: &'a str,
    target: &'a str,
    kind: ConnectionKind,
    requester_role: Role,
) -> (&'a str, &'a str) {
    if (kind == ConnectionKind::Peer && requester > target)
        || (kind == ConnectionKind::Report && requester_role == Role::RepoLead)
    {
        (target, requester)
    } else {
        (requester, target)
    }
}
fn eligible(kind: ConnectionKind, requester: Role, target: Role) -> bool {
    match kind {
        ConnectionKind::Report => matches!(
            (requester, target),
            (Role::FeatureLead, Role::RepoLead) | (Role::RepoLead, Role::FeatureLead)
        ),
        ConnectionKind::Peer => {
            requester == target && matches!(requester, Role::RepoLead | Role::FeatureLead)
        }
    }
}

impl Store {
    /// Trusted owner setup. Register the fixed role separately; never creates a caller-chosen
    /// member on first contact. Directory metadata is outward data, not an execution prompt.
    pub async fn describe_member(
        &self,
        context: &str,
        member: &str,
        repository: &str,
        label: &str,
        summary: &str,
    ) -> Result<()> {
        for value in [context, member, repository, label] {
            valid_id(value)?;
        }
        introduction(summary)?;
        ensure!(
            summary.len() <= MAX_DIRECTORY_SUMMARY_BYTES
                && summary.chars().count() <= MAX_DIRECTORY_SUMMARY_CHARS,
            "directory summary exceeds bounds"
        );
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let active:i64=sqlx::query_scalar("SELECT count(*) FROM candidate_members WHERE context_id=? AND member_id=? AND active=1")
            .bind(context).bind(member).fetch_one(&mut *tx).await?;
        ensure!(active == 1, "directory member unavailable");
        sqlx::query("INSERT INTO candidate_directory(context_id,member_id,repository,label,summary) VALUES (?,?,?,?,?) ON CONFLICT(context_id,member_id) DO UPDATE SET repository=excluded.repository,label=excluded.label,summary=excluded.summary")
            .bind(context).bind(member).bind(repository).bind(label).bind(summary).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Trusted owner setup after host corroboration. An invite is fixed to the current epoch
    /// and specific native evidence; issuance alone never alters the current session binding.
    pub async fn issue_enrollment_invite(
        &self,
        context: &str,
        member: &str,
        evidence: &SessionEvidence,
        ttl_seconds: u32,
    ) -> Result<EnrollmentInvite> {
        for value in [context, member] {
            valid_id(value)?;
        }
        ensure!(
            (1..=MAX_INVITE_TTL_SECONDS).contains(&ttl_seconds),
            "invalid enrollment lifetime"
        );
        let token = secret("cbce1_")?;
        let expires_at = now()?
            .checked_add(i64::from(ttl_seconds))
            .context("invalid enrollment lifetime")?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let epoch: Option<i64> = sqlx::query_scalar(
            "SELECT epoch FROM candidate_members WHERE context_id=? AND member_id=? AND active=1",
        )
        .bind(context)
        .bind(member)
        .fetch_optional(&mut *tx)
        .await?;
        let epoch = epoch.context("enrollment member unavailable")?;
        ensure!(epoch < i64::MAX - 1, "member is unavailable for binding");
        let target_json = evidence
            .target
            .as_ref()
            .map(NativeTarget::encoded)
            .transpose()?;
        let target_digest = evidence
            .target
            .as_ref()
            .map(NativeTarget::digest)
            .transpose()?;
        sqlx::query("INSERT INTO candidate_enrollment_invites(token_digest,context_id,member_id,expected_epoch,host,device_id,native_session_id,evidence_ref,expires_at,target_json,target_digest) VALUES (?,?,?,?,?,?,?,?,?,?,?)")
            .bind(Sha256::digest(token.as_bytes()).to_vec()).bind(context).bind(member).bind(epoch)
            .bind(evidence.host.as_str()).bind(&evidence.device).bind(&evidence.session).bind(&evidence.reference).bind(expires_at)
            .bind(target_json).bind(target_digest)
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(EnrollmentInvite { token, expires_at })
    }

    /// Adapter-only admission. Corroborate evidence before calling; an invitation token alone
    /// does not establish native identity. A lost successful response requires explicit owner
    /// recovery: repeating this one-use claim never rotates away the issued binding.
    pub async fn consume_enrollment_invite(
        &self,
        token: &str,
        evidence: &SessionEvidence,
    ) -> Result<SessionBinding> {
        let part = token
            .strip_prefix("cbce1_")
            .context("invalid enrollment invitation")?;
        ensure!(
            part.len() == 64 && part.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid enrollment invitation"
        );
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row=sqlx::query("SELECT context_id,member_id,expected_epoch,host,device_id,native_session_id,evidence_ref,expires_at,consumed_epoch,target_json,target_digest FROM candidate_enrollment_invites WHERE token_digest=?")
            .bind(Sha256::digest(token.as_bytes()).to_vec()).fetch_optional(&mut *tx).await?.context("invalid enrollment invitation")?;
        ensure!(
            row.try_get::<Option<i64>, _>("consumed_epoch")?.is_none(),
            "enrollment invitation already consumed"
        );
        ensure!(
            row.try_get::<i64, _>("expires_at")? > now()?,
            "enrollment invitation expired"
        );
        ensure!(
            row.try_get::<String, _>("host")? == evidence.host.as_str()
                && row.try_get::<String, _>("device_id")? == evidence.device
                && row.try_get::<String, _>("native_session_id")? == evidence.session
                && row.try_get::<String, _>("evidence_ref")? == evidence.reference,
            "enrollment evidence mismatch"
        );
        let target_json = evidence
            .target
            .as_ref()
            .map(NativeTarget::encoded)
            .transpose()?;
        let target_digest = evidence
            .target
            .as_ref()
            .map(NativeTarget::digest)
            .transpose()?;
        ensure!(
            row.try_get::<Option<String>, _>("target_json")? == target_json
                && row.try_get::<Option<Vec<u8>>, _>("target_digest")? == target_digest,
            "enrollment target mismatch"
        );
        let context: String = row.try_get("context_id")?;
        let member: String = row.try_get("member_id")?;
        let expected: i64 = row.try_get("expected_epoch")?;
        let binding_token = secret("cbcc1_")?;
        let epoch:Option<i64>=sqlx::query_scalar("UPDATE candidate_members SET epoch=epoch+1,device_id=?,native_session_id=?,credential_digest=? WHERE context_id=? AND member_id=? AND active=1 AND epoch=? AND epoch<9223372036854775806 RETURNING epoch")
            .bind(&evidence.device).bind(&evidence.session).bind(Sha256::digest(binding_token.as_bytes()).to_vec())
            .bind(&context).bind(&member).bind(expected).fetch_optional(&mut *tx).await?;
        let epoch = epoch.context("enrollment was superseded or revoked")?;
        // T-25: the first enrollment binds the identity; every later one is a handoff of the
        // same identity to a new native session.
        let earlier: i64 = sqlx::query_scalar("SELECT count(*) FROM candidate_identity_events WHERE context_id=? AND member_id=? AND kind IN ('enrolled','rebound')")
            .bind(&context).bind(&member).fetch_one(&mut *tx).await?;
        sqlx::query("INSERT INTO candidate_identity_events(context_id,member_id,kind,epoch,reason,at_wall_ms) VALUES (?,?,?,?,NULL,?)")
            .bind(&context).bind(&member).bind(if earlier == 0 { "enrolled" } else { "rebound" })
            .bind(epoch).bind(crate::facts::wall_ms()).execute(&mut *tx).await?;
        if let (Some(target_json), Some(target_digest)) = (target_json, target_digest) {
            sqlx::query("INSERT INTO candidate_native_targets(context_id,member_id,epoch,host,device_id,native_session_id,target_json,target_digest) VALUES (?,?,?,?,?,?,?,?)")
                .bind(&context).bind(&member).bind(epoch).bind(evidence.host.as_str())
                .bind(&evidence.device).bind(&evidence.session).bind(target_json).bind(target_digest)
                .execute(&mut *tx).await?;
        }
        sqlx::query("UPDATE candidate_enrollment_invites SET consumed_epoch=? WHERE token_digest=? AND consumed_epoch IS NULL")
            .bind(epoch).bind(Sha256::digest(token.as_bytes()).to_vec()).execute(&mut *tx).await?;
        sqlx::query("UPDATE candidate_batches SET state='superseded' WHERE context_id=? AND member_id=? AND state='pending'")
            .bind(&context).bind(&member).execute(&mut *tx).await?;
        tx.commit().await?;
        SessionBinding::from_token(
            &context,
            &member,
            &evidence.device,
            &evidence.session,
            epoch,
            &binding_token,
        )
    }

    /// Trusted assignment policy, not a per-room approval. Report relationships retain the
    /// child/parent ordering; either endpoint can establish the granted route. Peer grants are
    /// symmetric. Future connects and discovery require the grant to remain active.
    pub async fn authorize_connection(
        &self,
        context: &str,
        requester: &str,
        target: &str,
        kind: ConnectionKind,
        scope_ref: &str,
    ) -> Result<()> {
        for value in [context, requester, target, scope_ref] {
            valid_id(value)?;
        }
        ensure!(requester != target, "connection requires another member");
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let from = active_role(&mut tx, context, requester).await?;
        let to = active_role(&mut tx, context, target).await?;
        ensure!(
            eligible(kind, from, to),
            "roles are not eligible for this connection"
        );
        let (first, second) = pair(requester, target, kind, from);
        sqlx::query("INSERT INTO candidate_route_grants(context_id,first_member,second_member,kind,scope_ref) VALUES (?,?,?,?,?) ON CONFLICT(context_id,first_member,second_member,kind,scope_ref) DO UPDATE SET active=1")
            .bind(context).bind(first).bind(second).bind(kind.as_str()).bind(scope_ref).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Trusted revocation also revokes the relationship's actual room memberships. Regranting
    /// discovery policy alone cannot silently restore a revoked room or its history access.
    pub async fn revoke_connection(
        &self,
        context: &str,
        requester: &str,
        target: &str,
        kind: ConnectionKind,
        scope_ref: &str,
    ) -> Result<()> {
        for value in [context, requester, target, scope_ref] {
            valid_id(value)?;
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let role: Option<String> = sqlx::query_scalar(
            "SELECT role FROM candidate_members WHERE context_id=? AND member_id=?",
        )
        .bind(context)
        .bind(requester)
        .fetch_optional(&mut *tx)
        .await?;
        let (first, second) = pair(
            requester,
            target,
            kind,
            Role::parse(&role.context("connection member unavailable")?)?,
        );
        let changed=sqlx::query("UPDATE candidate_route_grants SET active=0 WHERE context_id=? AND first_member=? AND second_member=? AND kind=? AND scope_ref=? AND active=1")
            .bind(context).bind(first).bind(second).bind(kind.as_str()).bind(scope_ref).execute(&mut *tx).await?.rows_affected();
        ensure!(changed == 1, "connection grant unavailable");
        sqlx::query("UPDATE candidate_room_members SET active=0 WHERE context_id=? AND room_id IN (SELECT room_id FROM candidate_connections WHERE context_id=? AND first_member=? AND second_member=? AND kind=? AND scope_ref=?)")
            .bind(context).bind(context).bind(first).bind(second).bind(kind.as_str()).bind(scope_ref).execute(&mut *tx).await?;
        // A revoked room stops being the pair's active room, so it never blocks a later
        // authorized open (T-26). That open gets a replacement, never the revoked room.
        sqlx::query("UPDATE candidate_connections SET active=0 WHERE context_id=? AND first_member=? AND second_member=? AND kind=? AND scope_ref=?")
            .bind(context).bind(first).bind(second).bind(kind.as_str()).bind(scope_ref).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Only already-authorized counterparts' outward metadata is returned. No native-session,
    /// device, credential, internal map or unrelated-directory rows are exposed.
    pub async fn discover_connections(
        &self,
        binding: &SessionBinding,
        limit: usize,
    ) -> Result<Vec<DirectoryEntry>> {
        ensure!(
            (1..=MAX_DISCOVERY).contains(&limit),
            "invalid discovery limit"
        );
        let mut tx = self.pool.begin().await?;
        validate_on(&mut tx, binding).await?;
        let rows=sqlx::query("SELECT d.member_id,m.role,d.repository,d.label,d.summary,g.kind,g.scope_ref FROM candidate_route_grants g JOIN candidate_directory d ON d.context_id=g.context_id AND d.member_id=CASE WHEN g.first_member=? THEN g.second_member ELSE g.first_member END JOIN candidate_members m ON m.context_id=d.context_id AND m.member_id=d.member_id WHERE g.context_id=? AND g.active=1 AND m.active=1 AND (g.first_member=? OR g.second_member=?) ORDER BY d.member_id,g.kind,g.scope_ref LIMIT ?")
            .bind(&binding.member_id).bind(&binding.context_id).bind(&binding.member_id).bind(&binding.member_id).bind(limit as i64).fetch_all(&mut *tx).await?;
        let mut result = Vec::new();
        for row in rows {
            result.push(DirectoryEntry {
                member: row.try_get("member_id")?,
                role: row.try_get("role")?,
                repository: row.try_get("repository")?,
                label: row.try_get("label")?,
                summary: row.try_get("summary")?,
                connection_kind: if row.try_get::<String, _>("kind")? == "report" {
                    ConnectionKind::Report
                } else {
                    ConnectionKind::Peer
                },
                scope_ref: row.try_get("scope_ref")?,
            });
        }
        tx.commit().await?;
        Ok(result)
    }

    /// Connect with a target obtained from authorized discovery. Room selection, membership and
    /// one introduction are transactional; neither actor needs to copy a room ID to the other.
    pub async fn connect_members(
        &self,
        binding: &SessionBinding,
        target: &str,
        kind: ConnectionKind,
        scope_ref: &str,
        request_key: &str,
        explanation: &str,
    ) -> Result<Connection> {
        for value in [target, scope_ref, request_key] {
            valid_id(value)?;
        }
        introduction(explanation)?;
        ensure!(
            target != binding.member_id,
            "connection requires another member"
        );
        ensure!(
            kind != ConnectionKind::Report || explanation.is_empty(),
            "report introductions are deterministic"
        );
        let fingerprint =
            Sha256::digest(serde_json::to_vec(&(target, kind, scope_ref, explanation))?).to_vec();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current = validate_on(&mut tx, binding).await?;
        ensure!(
            eligible(
                kind,
                current.role,
                active_role(&mut tx, &binding.context_id, target).await?
            ),
            "roles are not eligible for this connection"
        );
        let (first, second) = pair(&binding.member_id, target, kind, current.role);
        let allowed:i64=sqlx::query_scalar("SELECT count(*) FROM candidate_route_grants WHERE context_id=? AND first_member=? AND second_member=? AND kind=? AND scope_ref=? AND active=1")
            .bind(&binding.context_id).bind(first).bind(second).bind(kind.as_str()).bind(scope_ref).fetch_one(&mut *tx).await?;
        ensure!(allowed == 1, "connection is not authorized");
        if let Some(row)=sqlx::query("SELECT fingerprint,room_id FROM candidate_connection_requests WHERE context_id=? AND member_id=? AND request_key=?")
            .bind(&binding.context_id).bind(&binding.member_id).bind(request_key).fetch_optional(&mut *tx).await? {
            ensure!(row.try_get::<Vec<u8>,_>("fingerprint")?==fingerprint,"connection request key conflicts");
            let room:String=row.try_get("room_id")?;
            let result=connection_row(&mut tx,&binding.context_id,&room,true).await?;
            ensure!(room_authorized(&mut tx,&binding.context_id,&room,&binding.member_id).await?,"connection room unavailable");
            tx.commit().await?; return Ok(result);
        }
        // T-26: one active room per unordered pair, whatever the kind or scope. A second open
        // joins it; after a close the next open creates one replacement that supersedes it.
        let existing:Option<String>=sqlx::query_scalar("SELECT room_id FROM candidate_connections WHERE context_id=? AND min(first_member,second_member)=min(?,?) AND max(first_member,second_member)=max(?,?) AND active=1")
            .bind(&binding.context_id).bind(first).bind(second).bind(first).bind(second).fetch_optional(&mut *tx).await?;
        let reused = existing.is_some();
        let room = if let Some(room) = existing {
            // A revoked membership is never restored by another open, under any scope.
            ensure!(
                room_authorized(&mut tx, &binding.context_id, &room, &binding.member_id).await?
                    && room_authorized(&mut tx, &binding.context_id, &room, target).await?,
                "connection room unavailable"
            );
            room
        } else {
            let latest = sqlx::query("SELECT room_id,generation FROM candidate_connections WHERE context_id=? AND min(first_member,second_member)=min(?,?) AND max(first_member,second_member)=max(?,?) ORDER BY held_slot DESC,generation DESC,introduction_sequence DESC LIMIT 1")
                .bind(&binding.context_id).bind(first).bind(second).bind(first).bind(second).fetch_optional(&mut *tx).await?;
            let (generation, supersedes) = match latest {
                Some(row) => (
                    row.try_get::<i64, _>("generation")? + 1,
                    Some(row.try_get::<String, _>("room_id")?),
                ),
                None => (1, None),
            };
            // Generation 1 keeps the pre-T-26 room ID, so existing rooms keep their names.
            let hash = if generation == 1 {
                Sha256::digest(serde_json::to_vec(&(
                    &binding.context_id,
                    first,
                    second,
                    kind,
                    scope_ref,
                ))?)
            } else {
                Sha256::digest(serde_json::to_vec(&(
                    &binding.context_id,
                    first,
                    second,
                    kind,
                    scope_ref,
                    generation,
                ))?)
            };
            let room = format!(
                "cbc-{}-{}",
                kind.as_str(),
                hash[..16]
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
            sqlx::query("INSERT INTO candidate_rooms(context_id,room_id) VALUES (?,?)")
                .bind(&binding.context_id)
                .bind(&room)
                .execute(&mut *tx)
                .await?;
            for member in [first, second] {
                sqlx::query("INSERT INTO candidate_room_members(context_id,room_id,member_id) VALUES (?,?,?)").bind(&binding.context_id).bind(&room).bind(member).execute(&mut *tx).await?;
            }
            // Structured introduction is bounded source data. It is never an executable prompt.
            let body = serde_json::to_string(
                &serde_json::json!({"event":"connection_opened","kind":kind,"scope_ref":scope_ref,"members":[first,second],"explanation":explanation}),
            )?;
            let sequence:i64=sqlx::query_scalar("INSERT INTO candidate_items(context_id,room_id,sender,kind,body) VALUES (?,?,?,'message',?) RETURNING sequence")
                .bind(&binding.context_id).bind(&room).bind(&binding.member_id).bind(body).fetch_one(&mut *tx).await?;
            crate::facts::record_commit(
                &mut tx,
                &self.clock,
                sequence,
                crate::facts::ItemClass::Introduction,
            )
            .await?;
            sqlx::query("INSERT INTO candidate_connections(context_id,first_member,second_member,kind,scope_ref,room_id,introduction_sequence,generation,supersedes_room,active,held_slot) VALUES (?,?,?,?,?,?,?,?,?,1,1)")
                .bind(&binding.context_id).bind(first).bind(second).bind(kind.as_str()).bind(scope_ref).bind(&room).bind(sequence)
                .bind(generation).bind(supersedes).execute(&mut *tx).await?;
            room
        };
        sqlx::query("INSERT INTO candidate_connection_requests(context_id,member_id,request_key,fingerprint,room_id) VALUES (?,?,?,?,?)")
            .bind(&binding.context_id).bind(&binding.member_id).bind(request_key).bind(fingerprint).bind(&room).execute(&mut *tx).await?;
        let result = connection_row(&mut tx, &binding.context_id, &room, reused).await?;
        tx.commit().await?;
        Ok(result)
    }
}

async fn active_role(conn: &mut SqliteConnection, context: &str, member: &str) -> Result<Role> {
    let role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM candidate_members WHERE context_id=? AND member_id=? AND active=1",
    )
    .bind(context)
    .bind(member)
    .fetch_optional(conn)
    .await?;
    Role::parse(&role.context("connection member unavailable")?)
}
async fn room_authorized(
    conn: &mut SqliteConnection,
    context: &str,
    room: &str,
    member: &str,
) -> Result<bool> {
    Ok(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM candidate_room_members WHERE context_id=? AND room_id=? AND member_id=? AND active=1").bind(context).bind(room).bind(member).fetch_one(conn).await?==1)
}
/// The stored room, as created: a reused room reports the kind and scope it was opened for.
async fn connection_row(
    conn: &mut SqliteConnection,
    context: &str,
    room: &str,
    reused: bool,
) -> Result<Connection> {
    let row=sqlx::query("SELECT c.kind,c.scope_ref,c.introduction_sequence,c.generation,c.supersedes_room,c.closed_by,c.close_reason,r.closed FROM candidate_connections c JOIN candidate_rooms r ON r.context_id=c.context_id AND r.room_id=c.room_id WHERE c.context_id=? AND c.room_id=?")
        .bind(context).bind(room).fetch_one(conn).await?;
    let kind = match row.try_get::<String, _>("kind")?.as_str() {
        "report" => ConnectionKind::Report,
        "peer" => ConnectionKind::Peer,
        other => anyhow::bail!("unknown connection kind {other}"),
    };
    let closure = match (
        row.try_get::<Option<String>, _>("closed_by")?,
        row.try_get::<Option<String>, _>("close_reason")?,
    ) {
        (Some(by), Some(reason)) => Some(RoomClosure { by, reason }),
        _ => None,
    };
    Ok(Connection {
        room: room.into(),
        kind,
        scope_ref: row.try_get("scope_ref")?,
        introduction_sequence: row.try_get("introduction_sequence")?,
        reused,
        closed: row.try_get::<i64, _>("closed")? != 0,
        generation: row.try_get("generation")?,
        supersedes: row.try_get("supersedes_room")?,
        closure,
    })
}
