//! Work items (T-07, T-09): one owner, atomic multi-field revisions, the HITL flag, and one
//! outbox event per revision, delivered through the inbox.
//!
//! A revision commits whether or not anyone can hear about it. Its events go to the owner's
//! open report rooms in the same transaction; with no such room the revision records an empty
//! delivery list rather than waiting on transport.
use crate::{
    bindings::{validate_on, Role, SessionBinding},
    facts::{self, ItemClass},
    store::Store,
};
use anyhow::{bail, ensure, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqliteConnection};

const MAX_SUMMARY_CHARS: usize = 500;
const MAX_NEXT_ACTION_CHARS: usize = 500;
const MAX_LINE_CHARS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Planning,
    Implementing,
    InReview,
    ReadyToMerge,
    Merged,
    Abandoned,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Planning => "planning",
            Stage::Implementing => "implementing",
            Stage::InReview => "in_review",
            Stage::ReadyToMerge => "ready_to_merge",
            Stage::Merged => "merged",
            Stage::Abandoned => "abandoned",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        Ok(match value {
            "planning" => Stage::Planning,
            "implementing" => Stage::Implementing,
            "in_review" => Stage::InReview,
            "ready_to_merge" => Stage::ReadyToMerge,
            "merged" => Stage::Merged,
            "abandoned" => Stage::Abandoned,
            other => bail!("unknown work stage {other}"),
        })
    }
}

/// The owner's content fields. `None` leaves a field as it is; for the clearable fields,
/// `Some(None)` clears it. HITL is not here: it has its own routes and its own authority.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkPatch {
    pub stage: Option<Stage>,
    pub summary: Option<String>,
    pub next_action: Option<String>,
    pub blocked_operation: Option<Option<String>>,
    pub branch: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkItem {
    pub item: String,
    pub owner: String,
    pub summary: String,
    pub stage: Stage,
    pub next_action: String,
    pub blocked_operation: Option<String>,
    pub branch: Option<String>,
    pub hitl: bool,
    pub revision: i64,
    pub created_wall_ms: i64,
    pub updated_wall_ms: i64,
}

/// Who committed a revision: a bound member, or the user through the owner route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Actor {
    Member(String),
    Owner,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkRevision {
    pub revision: i64,
    pub actor: Actor,
    pub changed: Vec<String>,
    pub at_wall_ms: i64,
    /// Rooms that received this revision's event. Empty means nobody was told.
    pub delivered_to: Vec<String>,
    pub event_sequences: Vec<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkOutcome {
    Applied { revision: i64 },
    NoOp { revision: i64 },
}

impl Store {
    /// A feature lead registers the item it owns. Revision 1 records `["registered"]`.
    pub async fn register_work_item(
        &self,
        binding: &SessionBinding,
        item: &str,
        summary: &str,
        key: &str,
    ) -> Result<WorkItem> {
        valid_item(item)?;
        valid_key(key)?;
        let summary = valid_summary(summary)?;
        let fingerprint =
            fingerprint(&serde_json::json!({"op": "register", "item": item, "summary": summary}));
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let status = validate_on(&mut tx, binding).await?;
        ensure!(
            status.role == Role::FeatureLead,
            "not authorized: only a feature lead registers a work item"
        );
        let actor = Actor::Member(binding.member_id.clone());
        if replayed(&mut tx, &binding.context_id, &actor, key, &fingerprint)
            .await?
            .is_some()
        {
            let current = load_item(&mut tx, &binding.context_id, item)
                .await?
                .ok_or_else(|| anyhow::anyhow!("unknown work item"))?;
            tx.commit().await?;
            return Ok(current);
        }
        ensure!(
            load_item(&mut tx, &binding.context_id, item)
                .await?
                .is_none(),
            "work item already registered"
        );
        let now = facts::wall_ms();
        let created = WorkItem {
            item: item.into(),
            owner: binding.member_id.clone(),
            summary,
            stage: Stage::Planning,
            next_action: String::new(),
            blocked_operation: None,
            branch: None,
            hitl: false,
            revision: 1,
            created_wall_ms: now,
            updated_wall_ms: now,
        };
        sqlx::query("INSERT INTO candidate_work_items(context_id,item_id,owner_member,summary,stage,next_action,blocked_operation,branch,hitl,revision,created_wall_ms,updated_wall_ms) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&binding.context_id).bind(&created.item).bind(&created.owner).bind(&created.summary)
            .bind(created.stage.as_str()).bind(&created.next_action).bind(&created.blocked_operation)
            .bind(&created.branch).bind(i64::from(created.hitl)).bind(created.revision)
            .bind(now).bind(now).execute(&mut *tx).await?;
        self.commit_revision(
            &mut tx,
            &binding.context_id,
            &created,
            &actor,
            key,
            &fingerprint,
            &["registered".to_string()],
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(created)
    }

    /// The owner patches any of its content fields in one revision. A patch that changes
    /// nothing records nothing and wakes nobody.
    pub async fn update_work_item(
        &self,
        binding: &SessionBinding,
        item: &str,
        expected_revision: i64,
        key: &str,
        patch: &WorkPatch,
    ) -> Result<WorkOutcome> {
        valid_item(item)?;
        valid_key(key)?;
        let patch = normalize(patch)?;
        let fingerprint = fingerprint(&serde_json::json!({
            "op": "update",
            "item": item,
            "expected_revision": expected_revision,
            "stage": patch.stage.map(Stage::as_str),
            "summary": patch.summary,
            "next_action": patch.next_action,
            "blocked_operation": clearable(&patch.blocked_operation),
            "branch": clearable(&patch.branch),
        }));
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        validate_on(&mut tx, binding).await?;
        let current = load_item(&mut tx, &binding.context_id, item)
            .await?
            .ok_or_else(|| anyhow::anyhow!("unknown work item"))?;
        ensure!(
            current.owner == binding.member_id,
            "not authorized: only the owner patches a work item"
        );
        let actor = Actor::Member(binding.member_id.clone());
        if let Some(revision) =
            replayed(&mut tx, &binding.context_id, &actor, key, &fingerprint).await?
        {
            tx.commit().await?;
            return Ok(WorkOutcome::Applied { revision });
        }
        ensure!(
            current.revision == expected_revision,
            "revision conflict: expected {expected_revision}, current {}",
            current.revision
        );
        let mut next = current.clone();
        let mut changed = Vec::new();
        if let Some(stage) = patch.stage {
            if stage != next.stage {
                next.stage = stage;
                changed.push("stage");
            }
        }
        if let Some(summary) = patch.summary {
            if summary != next.summary {
                next.summary = summary;
                changed.push("summary");
            }
        }
        if let Some(action) = patch.next_action {
            if action != next.next_action {
                next.next_action = action;
                changed.push("next_action");
            }
        }
        if let Some(blocked) = patch.blocked_operation {
            if blocked != next.blocked_operation {
                next.blocked_operation = blocked;
                changed.push("blocked_operation");
            }
        }
        if let Some(branch) = patch.branch {
            if branch != next.branch {
                next.branch = branch;
                changed.push("branch");
            }
        }
        if changed.is_empty() {
            tx.commit().await?;
            return Ok(WorkOutcome::NoOp {
                revision: current.revision,
            });
        }
        changed.sort_unstable();
        let changed: Vec<String> = changed.into_iter().map(String::from).collect();
        let revision = self
            .advance(
                &mut tx,
                &binding.context_id,
                &current,
                next,
                &actor,
                key,
                &fingerprint,
                &changed,
            )
            .await?;
        tx.commit().await?;
        Ok(WorkOutcome::Applied { revision })
    }

    /// T-07 from a member: a repo lead may turn HITL on; nobody but the user turns it off.
    pub async fn set_work_hitl(
        &self,
        binding: &SessionBinding,
        item: &str,
        on: bool,
        key: &str,
    ) -> Result<WorkOutcome> {
        valid_item(item)?;
        valid_key(key)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let status = validate_on(&mut tx, binding).await?;
        ensure!(
            status.role == Role::RepoLead,
            "not authorized: only a repo lead or the user sets HITL"
        );
        ensure!(on, "only the user turns HITL off");
        let actor = Actor::Member(binding.member_id.clone());
        let outcome = self
            .set_hitl(&mut tx, &binding.context_id, item, on, &actor, key)
            .await?;
        tx.commit().await?;
        Ok(outcome)
    }

    /// T-07 from the user, through the trusted owner route: HITL on or off.
    pub async fn owner_set_work_hitl(
        &self,
        context: &str,
        item: &str,
        on: bool,
        key: &str,
    ) -> Result<WorkOutcome> {
        valid_item(item)?;
        valid_key(key)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let outcome = self
            .set_hitl(&mut tx, context, item, on, &Actor::Owner, key)
            .await?;
        tx.commit().await?;
        Ok(outcome)
    }

    pub async fn work_item(&self, context: &str, item: &str) -> Result<Option<WorkItem>> {
        let mut conn = self.pool.acquire().await?;
        load_item(&mut conn, context, item).await
    }

    pub async fn work_revisions(&self, context: &str, item: &str) -> Result<Vec<WorkRevision>> {
        let mut conn = self.pool.acquire().await?;
        let rows = sqlx::query("SELECT revision,actor_kind,actor_member,changed,at_wall_ms FROM candidate_work_revisions WHERE context_id=? AND item_id=? ORDER BY revision")
            .bind(context).bind(item).fetch_all(&mut *conn).await?;
        let mut revisions = Vec::with_capacity(rows.len());
        for row in rows {
            let revision: i64 = row.try_get("revision")?;
            let actor = match row.try_get::<String, _>("actor_kind")?.as_str() {
                "owner" => Actor::Owner,
                _ => Actor::Member(row.try_get("actor_member")?),
            };
            let events = sqlx::query("SELECT e.sequence,i.room_id FROM candidate_work_events e JOIN candidate_items i ON i.sequence=e.sequence WHERE e.context_id=? AND e.item_id=? AND e.revision=? ORDER BY e.sequence")
                .bind(context).bind(item).bind(revision).fetch_all(&mut *conn).await?;
            let mut delivered_to = Vec::with_capacity(events.len());
            let mut event_sequences = Vec::with_capacity(events.len());
            for event in events {
                event_sequences.push(event.try_get("sequence")?);
                delivered_to.push(event.try_get("room_id")?);
            }
            revisions.push(WorkRevision {
                revision,
                actor,
                changed: serde_json::from_str(&row.try_get::<String, _>("changed")?)?,
                at_wall_ms: row.try_get("at_wall_ms")?,
                delivered_to,
                event_sequences,
            });
        }
        Ok(revisions)
    }

    async fn set_hitl(
        &self,
        conn: &mut SqliteConnection,
        context: &str,
        item: &str,
        on: bool,
        actor: &Actor,
        key: &str,
    ) -> Result<WorkOutcome> {
        let fingerprint = fingerprint(&serde_json::json!({"op": "hitl", "item": item, "on": on}));
        let current = load_item(conn, context, item)
            .await?
            .ok_or_else(|| anyhow::anyhow!("unknown work item"))?;
        if let Some(revision) = replayed(conn, context, actor, key, &fingerprint).await? {
            return Ok(WorkOutcome::Applied { revision });
        }
        if current.hitl == on {
            return Ok(WorkOutcome::NoOp {
                revision: current.revision,
            });
        }
        let mut next = current.clone();
        next.hitl = on;
        let revision = self
            .advance(
                conn,
                context,
                &current,
                next,
                actor,
                key,
                &fingerprint,
                &["hitl".to_string()],
            )
            .await?;
        Ok(WorkOutcome::Applied { revision })
    }

    /// Write the next state of an item as one revision, guarded on the revision it was read at.
    #[allow(clippy::too_many_arguments)]
    async fn advance(
        &self,
        conn: &mut SqliteConnection,
        context: &str,
        current: &WorkItem,
        mut next: WorkItem,
        actor: &Actor,
        key: &str,
        fingerprint: &[u8; 32],
        changed: &[String],
    ) -> Result<i64> {
        let now = facts::wall_ms();
        next.revision = current.revision + 1;
        next.updated_wall_ms = now;
        let updated = sqlx::query("UPDATE candidate_work_items SET summary=?,stage=?,next_action=?,blocked_operation=?,branch=?,hitl=?,revision=?,updated_wall_ms=? WHERE context_id=? AND item_id=? AND revision=?")
            .bind(&next.summary).bind(next.stage.as_str()).bind(&next.next_action)
            .bind(&next.blocked_operation).bind(&next.branch).bind(i64::from(next.hitl))
            .bind(next.revision).bind(now).bind(context).bind(&next.item).bind(current.revision)
            .execute(&mut *conn).await?.rows_affected();
        ensure!(updated == 1, "revision conflict");
        self.commit_revision(conn, context, &next, actor, key, fingerprint, changed, now)
            .await?;
        Ok(next.revision)
    }

    /// Record the revision and, in the same transaction, one tagged inbox item per open report
    /// room of the owner. The acting member is never woken by its own change.
    #[allow(clippy::too_many_arguments)]
    async fn commit_revision(
        &self,
        conn: &mut SqliteConnection,
        context: &str,
        item: &WorkItem,
        actor: &Actor,
        key: &str,
        fingerprint: &[u8; 32],
        changed: &[String],
        at: i64,
    ) -> Result<()> {
        let (kind, member) = actor_columns(actor);
        sqlx::query("INSERT INTO candidate_work_revisions(context_id,item_id,revision,actor_kind,actor_member,request_key,fingerprint,changed,at_wall_ms) VALUES (?,?,?,?,?,?,?,?,?)")
            .bind(context).bind(&item.item).bind(item.revision).bind(kind).bind(member)
            .bind(key).bind(&fingerprint[..]).bind(serde_json::to_string(changed)?).bind(at)
            .execute(&mut *conn).await?;
        let body = serde_json::json!({
            "item": item.item,
            "revision": item.revision,
            "actor": member.unwrap_or("user"),
            "changed": changed,
            "stage": item.stage,
            "hitl": item.hitl,
        })
        .to_string();
        let rooms: Vec<String> = sqlx::query_scalar("SELECT c.room_id FROM candidate_connections c JOIN candidate_rooms r ON r.context_id=c.context_id AND r.room_id=c.room_id AND r.closed=0 WHERE c.context_id=? AND c.kind='report' AND c.active=1 AND (c.first_member=? OR c.second_member=?) ORDER BY c.room_id")
            .bind(context).bind(&item.owner).bind(&item.owner).fetch_all(&mut *conn).await?;
        // The stored sender is a real member for the item's foreign keys; readers see 'cbc'.
        let sender = member.unwrap_or(&item.owner);
        for room in rooms {
            let recipients: Vec<String> = sqlx::query_scalar("SELECT r.member_id FROM candidate_room_members r JOIN candidate_members m ON m.context_id=r.context_id AND m.member_id=r.member_id AND m.active=1 WHERE r.context_id=? AND r.room_id=? AND r.active=1 ORDER BY r.member_id")
                .bind(context).bind(&room).fetch_all(&mut *conn).await?;
            let recipients: Vec<String> = recipients
                .into_iter()
                .filter(|recipient| Some(recipient.as_str()) != member)
                .collect();
            if recipients.is_empty() {
                continue;
            }
            let sequence: i64 = sqlx::query_scalar("INSERT INTO candidate_items(context_id,room_id,sender,kind,body) VALUES (?,?,?,'message',?) RETURNING sequence")
                .bind(context).bind(&room).bind(sender).bind(&body)
                .fetch_one(&mut *conn).await?;
            sqlx::query("INSERT INTO candidate_work_events(sequence,context_id,item_id,revision) VALUES (?,?,?,?)")
                .bind(sequence).bind(context).bind(&item.item).bind(item.revision)
                .execute(&mut *conn).await?;
            for recipient in recipients {
                sqlx::query(
                    "INSERT INTO candidate_work_event_recipients(sequence,member_id) VALUES (?,?)",
                )
                .bind(sequence)
                .bind(recipient)
                .execute(&mut *conn)
                .await?;
            }
            // After the recipients exist, so the snapshot is the event's own audience.
            facts::record_commit(conn, &self.clock, sequence, ItemClass::WorkEvent).await?;
        }
        Ok(())
    }
}

fn actor_columns(actor: &Actor) -> (&'static str, Option<&str>) {
    match actor {
        Actor::Member(member) => ("member", Some(member.as_str())),
        Actor::Owner => ("owner", None),
    }
}

/// The revision a key already committed, if the same request is being retried. The same key
/// with a different request is refused rather than silently ignored.
async fn replayed(
    conn: &mut SqliteConnection,
    context: &str,
    actor: &Actor,
    key: &str,
    fingerprint: &[u8; 32],
) -> Result<Option<i64>> {
    let (kind, member) = actor_columns(actor);
    let row = sqlx::query("SELECT revision,fingerprint FROM candidate_work_revisions WHERE context_id=? AND actor_kind=? AND coalesce(actor_member,'')=? AND request_key=?")
        .bind(context).bind(kind).bind(member.unwrap_or("")).bind(key)
        .fetch_optional(conn).await?;
    let Some(row) = row else { return Ok(None) };
    ensure!(
        row.try_get::<Vec<u8>, _>("fingerprint")? == fingerprint[..],
        "idempotency key conflicts with an earlier request"
    );
    Ok(Some(row.try_get("revision")?))
}

async fn load_item(
    conn: &mut SqliteConnection,
    context: &str,
    item: &str,
) -> Result<Option<WorkItem>> {
    let row = sqlx::query("SELECT owner_member,summary,stage,next_action,blocked_operation,branch,hitl,revision,created_wall_ms,updated_wall_ms FROM candidate_work_items WHERE context_id=? AND item_id=?")
        .bind(context).bind(item).fetch_optional(conn).await?;
    row.map(|row| {
        Ok(WorkItem {
            item: item.into(),
            owner: row.try_get("owner_member")?,
            summary: row.try_get("summary")?,
            stage: Stage::parse(row.try_get("stage")?)?,
            next_action: row.try_get("next_action")?,
            blocked_operation: row.try_get("blocked_operation")?,
            branch: row.try_get("branch")?,
            hitl: row.try_get::<i64, _>("hitl")? != 0,
            revision: row.try_get("revision")?,
            created_wall_ms: row.try_get("created_wall_ms")?,
            updated_wall_ms: row.try_get("updated_wall_ms")?,
        })
    })
    .transpose()
}

/// A stable digest of a request, so a retry still matches after a toolchain or library
/// upgrade. Keys keep the fixed order of the literals above (serde_json preserve_order).
fn fingerprint(request: &serde_json::Value) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"cbc-work-request-v1\0");
    hasher.update(request.to_string().as_bytes());
    hasher.finalize().into()
}

/// Leave, clear and set are three different requests.
fn clearable(value: &Option<Option<String>>) -> serde_json::Value {
    match value {
        None => serde_json::Value::Null,
        Some(None) => serde_json::json!({"clear": true}),
        Some(Some(text)) => serde_json::json!({"set": text}),
    }
}

fn valid_item(item: &str) -> Result<()> {
    ensure!(
        !item.trim().is_empty() && item.len() <= 128 && !item.chars().any(char::is_control),
        "invalid work item id"
    );
    Ok(())
}

fn valid_key(key: &str) -> Result<()> {
    ensure!(
        !key.trim().is_empty() && key.len() <= 128 && !key.chars().any(char::is_control),
        "invalid request key"
    );
    Ok(())
}

fn valid_summary(summary: &str) -> Result<String> {
    let summary = summary.trim();
    ensure!(!summary.is_empty(), "invalid summary: empty");
    ensure!(
        summary.chars().count() <= MAX_SUMMARY_CHARS,
        "invalid summary: longer than {MAX_SUMMARY_CHARS} characters"
    );
    ensure!(
        !summary.chars().any(|c| c.is_control() && c != '\n'),
        "invalid summary: control characters"
    );
    Ok(summary.into())
}

fn valid_line(field: &str, value: &str) -> Result<String> {
    ensure!(!value.trim().is_empty(), "invalid {field}: empty");
    ensure!(
        value.chars().count() <= MAX_LINE_CHARS,
        "invalid {field}: longer than {MAX_LINE_CHARS} characters"
    );
    ensure!(
        !value.chars().any(char::is_control),
        "invalid {field}: must be one line"
    );
    Ok(value.into())
}

/// Validate every field before any database work, and trim the summary as registration does,
/// so a retried patch fingerprints identically.
fn normalize(patch: &WorkPatch) -> Result<WorkPatch> {
    ensure!(
        patch.stage.is_some()
            || patch.summary.is_some()
            || patch.next_action.is_some()
            || patch.blocked_operation.is_some()
            || patch.branch.is_some(),
        "empty patch"
    );
    let next_action = match &patch.next_action {
        Some(action) => {
            ensure!(
                action.chars().count() <= MAX_NEXT_ACTION_CHARS,
                "invalid next_action: longer than {MAX_NEXT_ACTION_CHARS} characters"
            );
            ensure!(
                !action.chars().any(|c| c.is_control() && c != '\n'),
                "invalid next_action: control characters"
            );
            Some(action.clone())
        }
        None => None,
    };
    let line = |field: &str, value: &Option<Option<String>>| -> Result<Option<Option<String>>> {
        Ok(match value {
            Some(Some(text)) => Some(Some(valid_line(field, text)?)),
            Some(None) => Some(None),
            None => None,
        })
    };
    Ok(WorkPatch {
        stage: patch.stage,
        summary: patch.summary.as_deref().map(valid_summary).transpose()?,
        next_action,
        blocked_operation: line("blocked_operation", &patch.blocked_operation)?,
        branch: line("branch", &patch.branch)?,
    })
}
