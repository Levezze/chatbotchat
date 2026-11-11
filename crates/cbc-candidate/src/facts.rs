//! Durable first-occurrence delivery facts and a versioned, content-free event projection (#113 PR-A).
//!
//! Facts are written inside the send, check and receipt transactions, so they are durable
//! whatever the log level and a failed receipt can never leave a false "receipted". Dispatch and
//! native wake come from the device driver's authenticated wake reports (#113 B1); the first
//! activity after a receipt is the member's first non-transport core operation. Result writing
//! has no record of its own: both adapters write the result before they receipt it.
use crate::bindings::{validate_on, SessionBinding};
use crate::store::Store;
use anyhow::{ensure, Result};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqliteConnection};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const EVENT_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Committed,
    DispatchStarted,
    NativeWakeAccepted,
    InboxOffered,
    ResultWritten,
    Receipted,
    /// The member's first non-transport CBC operation after the receipt. Not proof the item was
    /// read or used.
    PostReceiptActivity,
    ActedOn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemClass {
    /// A member's own send.
    Participant,
    /// The deterministic connection introduction.
    Introduction,
    /// A server room notice.
    Notice,
    /// The room-closed marker.
    Closure,
    /// A work-item revision delivered through the outbox (R2).
    WorkEvent,
    /// A request opened, answered or withdrawn (R3).
    RequestEvent,
}

impl ItemClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Participant => "participant",
            Self::Introduction => "introduction",
            Self::Notice => "notice",
            Self::Closure => "closure",
            Self::WorkEvent => "work_event",
            Self::RequestEvent => "request_event",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        Ok(match value {
            "participant" => Self::Participant,
            "introduction" => Self::Introduction,
            "notice" => Self::Notice,
            "closure" => Self::Closure,
            "work_event" => Self::WorkEvent,
            "request_event" => Self::RequestEvent,
            other => anyhow::bail!("unknown item class {other}"),
        })
    }
}

/// When a stage happened. `mono_us` is comparable only to stamps with the same `instance`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub instance: String,
    pub mono_us: i64,
    pub wall_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// Both stages on one core instance's monotonic clock.
    Exact,
    /// Stages on different instances; wall-clock difference, subject to clock changes.
    Estimate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interval {
    pub micros: i64,
    pub basis: Basis,
}

impl Interval {
    fn between(from: &Stamp, to: &Stamp) -> Self {
        if from.instance == to.instance {
            Self {
                micros: to.mono_us - from.mono_us,
                basis: Basis::Exact,
            }
        } else {
            Self {
                micros: (to.wall_ms - from.wall_ms).saturating_mul(1000),
                basis: Basis::Estimate,
            }
        }
    }
}

/// How the device driver tried to wake a native session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeKind {
    ClaudeChannel,
    CodexQueue,
    ClaudeMod,
}

impl WakeKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeChannel => "claude_channel",
            Self::CodexQueue => "codex_queue",
            Self::ClaudeMod => "claude_mod",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeOutcome {
    /// The host took the hint: the channel write or queue command succeeded.
    Accepted,
    Failed,
}

/// One wake attempt, reported by the device driver with the member's own binding. A hint covers
/// the member's unread items up to `through`, so one attempt can stand for many items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WakeReport {
    pub through: i64,
    pub kind: WakeKind,
    pub outcome: WakeOutcome,
    /// A short class such as `channel_closed`; required exactly when the outcome is `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    /// Sent by the recovery path rather than by a new item.
    pub recovery: bool,
    /// The driver's own wall clock when it saw the outcome. Kept beside the core's stamp; never
    /// compared to it as if the two clocks agreed.
    pub driver_wall_ms: i64,
}

/// A failure class or an operation name: 1 to 64 of `[a-z0-9_]`.
fn valid_label(label: &str) -> bool {
    (1..=64).contains(&label.len())
        && label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecipientTimeline {
    pub member: String,
    /// In the commit-time delivery snapshot.
    pub expected: bool,
    pub offer_count: i64,
    pub offered: Option<Stamp>,
    pub receipted: Option<Stamp>,
    /// The first wake attempt that covered this item, whatever its outcome.
    pub dispatched: Option<Stamp>,
    /// The first accepted wake that covered this item.
    pub woken: Option<Stamp>,
    /// The first non-transport operation after the receipt, and which one.
    pub active: Option<Stamp>,
    pub active_operation: Option<String>,
    /// None when either end is unknown. Each is measured from the commit.
    pub to_offer: Option<Interval>,
    pub to_receipt: Option<Interval>,
    /// Includes the report's own hop over the local socket.
    pub to_wake: Option<Interval>,
    pub to_activity: Option<Interval>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemTimeline {
    pub sequence: i64,
    pub room: String,
    /// None for an item committed before delivery facts existed.
    pub class: Option<ItemClass>,
    pub committed: Option<Stamp>,
    pub recipients: Vec<RecipientTimeline>,
}

/// Versioned, content-free event. Carries no body and no batch reference; `batch_tag` is a
/// one-way derivative of the reference, usable only for correlation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryEvent {
    pub schema: u32,
    pub stage: Stage,
    pub context: String,
    pub room: String,
    pub sequence: i64,
    pub member: Option<String>,
    pub class: Option<ItemClass>,
    pub at: Stamp,
    pub batch_tag: Option<String>,
}

/// One per Store::open: a random instance ID and a monotonic origin.
pub(crate) struct CoreClock {
    instance: String,
    origin: Instant,
}

impl CoreClock {
    pub(crate) async fn start(conn: &mut SqliteConnection) -> Result<Self> {
        let mut random = [0_u8; 16];
        OsRng.try_fill_bytes(&mut random)?;
        let clock = Self {
            instance: hex(&random),
            origin: Instant::now(),
        };
        sqlx::query(
            "INSERT INTO candidate_core_instances(instance_id,started_wall_ms) VALUES (?,?)",
        )
        .bind(&clock.instance)
        .bind(wall_ms())
        .execute(conn)
        .await?;
        Ok(clock)
    }

    fn now(&self) -> (i64, i64) {
        let mono = i64::try_from(self.origin.elapsed().as_micros()).unwrap_or(i64::MAX);
        (mono, wall_ms())
    }
}

pub(crate) fn wall_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// One-way correlation tag. Never reversible to the receipt capability.
pub(crate) fn batch_tag(reference: &str) -> String {
    let digest = Sha256::digest(format!("cbc-batch-tag-v1:{reference}").as_bytes());
    hex(&digest[..8])
}

/// Record a committed item and its commit-time recipient snapshot. Call after any notice
/// recipients are written, inside the committing transaction. First occurrence wins.
pub(crate) async fn record_commit(
    conn: &mut SqliteConnection,
    clock: &CoreClock,
    sequence: i64,
    class: ItemClass,
) -> Result<()> {
    let (mono, wall) = clock.now();
    let inserted = sqlx::query("INSERT INTO candidate_item_facts(sequence,context_id,room_id,item_class,committed_instance,committed_mono_us,committed_wall_ms) SELECT sequence,context_id,room_id,?,?,?,? FROM candidate_items WHERE sequence=? ON CONFLICT DO NOTHING")
        .bind(class.as_str()).bind(&clock.instance).bind(mono).bind(wall).bind(sequence)
        .execute(&mut *conn).await?.rows_affected();
    if inserted == 1 {
        sqlx::query("INSERT INTO candidate_recipient_facts(sequence,member_id,expected) SELECT DISTINCT sequence,recipient,1 FROM candidate_delivery_items WHERE sequence=? ON CONFLICT DO NOTHING")
            .bind(sequence).execute(&mut *conn).await?;
    }
    Ok(())
}

/// Record an offer (first or repeated) of every item in a batch to its member.
pub(crate) async fn record_offer(
    conn: &mut SqliteConnection,
    clock: &CoreClock,
    member: &str,
    reference: &str,
    sequences: &[i64],
) -> Result<()> {
    let (mono, wall) = clock.now();
    let tag = batch_tag(reference);
    for sequence in sequences {
        sqlx::query("INSERT INTO candidate_recipient_facts(sequence,member_id,expected,offer_count,first_batch_tag,offered_instance,offered_mono_us,offered_wall_ms) VALUES (?,?,0,1,?,?,?,?) ON CONFLICT(sequence,member_id) DO UPDATE SET offer_count=offer_count+1, first_batch_tag=COALESCE(first_batch_tag,excluded.first_batch_tag), offered_instance=COALESCE(offered_instance,excluded.offered_instance), offered_mono_us=COALESCE(offered_mono_us,excluded.offered_mono_us), offered_wall_ms=COALESCE(offered_wall_ms,excluded.offered_wall_ms)")
            .bind(sequence).bind(member).bind(&tag).bind(&clock.instance).bind(mono).bind(wall)
            .execute(&mut *conn).await?;
    }
    Ok(())
}

/// Record the first receipt of a batch's items. Only called once the receipt is authorized.
/// A batch offered before these facts existed has no rows yet, so the receipt creates one
/// (expected=0, offer unknown) rather than being lost.
pub(crate) async fn record_receipt(
    conn: &mut SqliteConnection,
    clock: &CoreClock,
    member: &str,
    reference: &str,
) -> Result<()> {
    let (mono, wall) = clock.now();
    sqlx::query("INSERT INTO candidate_recipient_facts(sequence,member_id,expected,receipted_instance,receipted_mono_us,receipted_wall_ms) SELECT sequence,?,0,?,?,? FROM candidate_batch_items WHERE reference=? ON CONFLICT(sequence,member_id) DO UPDATE SET receipted_instance=COALESCE(receipted_instance,excluded.receipted_instance), receipted_mono_us=COALESCE(receipted_mono_us,excluded.receipted_mono_us), receipted_wall_ms=COALESCE(receipted_wall_ms,excluded.receipted_wall_ms)")
        .bind(member).bind(&clock.instance).bind(mono).bind(wall).bind(reference)
        .execute(conn).await?;
    Ok(())
}

fn stamp(row: &sqlx::sqlite::SqliteRow, prefix: &str) -> Result<Option<Stamp>> {
    let instance: Option<String> = row.try_get(format!("{prefix}_instance").as_str())?;
    Ok(match instance {
        None => None,
        Some(instance) => Some(Stamp {
            instance,
            mono_us: row.try_get(format!("{prefix}_mono_us").as_str())?,
            wall_ms: row.try_get(format!("{prefix}_wall_ms").as_str())?,
        }),
    })
}

/// A context's outbox events below a sequence, newest first. It must read through
/// `candidate_outbox_facts`: without that index a quiet context scans the whole facts history on
/// the store's one connection.
const OUTBOX_PAGE: &str = "SELECT sequence FROM candidate_item_facts WHERE context_id=? AND item_class IN ('work_event','request_event') AND sequence<? ORDER BY sequence DESC LIMIT ?";

impl Store {
    /// This open's core instance ID.
    pub fn instance_id(&self) -> &str {
        &self.clock.instance
    }

    /// Record one wake attempt reported with the member's own binding, and mark the first
    /// dispatch (and, if accepted, the first wake) of each of its unread items up to `through`.
    /// Returns how many unread items the attempt covered, already marked or not.
    pub async fn record_wake(&self, binding: &SessionBinding, report: &WakeReport) -> Result<u64> {
        ensure!(report.through > 0, "invalid wake: through must be positive");
        match (report.outcome, report.failure.as_deref()) {
            (WakeOutcome::Accepted, None) => {}
            (WakeOutcome::Failed, Some(failure)) if valid_label(failure) => {}
            (WakeOutcome::Failed, Some(_)) => {
                anyhow::bail!("invalid wake: failure must be 1 to 64 of [a-z0-9_]")
            }
            _ => anyhow::bail!("invalid wake: a failure class goes with a failed outcome only"),
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        validate_on(&mut tx, binding).await?;
        let (mono, wall) = self.clock.now();
        sqlx::query("INSERT INTO candidate_wake_attempts(context_id,member_id,through_sequence,kind,outcome,failure,recovery,reported_instance,reported_mono_us,reported_wall_ms,driver_wall_ms) VALUES (?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&binding.context_id).bind(&binding.member_id).bind(report.through)
            .bind(report.kind.as_str())
            .bind(match report.outcome { WakeOutcome::Accepted => "accepted", WakeOutcome::Failed => "failed" })
            .bind(report.failure.as_deref()).bind(report.recovery)
            .bind(&self.clock.instance).bind(mono).bind(wall).bind(report.driver_wall_ms)
            .execute(&mut *tx).await?;
        let woken = report.outcome == WakeOutcome::Accepted;
        let covered = sqlx::query("INSERT INTO candidate_recipient_facts(sequence,member_id,expected,dispatched_instance,dispatched_mono_us,dispatched_wall_ms,woken_instance,woken_mono_us,woken_wall_ms) SELECT DISTINCT sequence,?,0,?,?,?,?,?,? FROM candidate_unread_items WHERE context_id=? AND recipient=? AND sequence<=? ON CONFLICT(sequence,member_id) DO UPDATE SET dispatched_instance=COALESCE(dispatched_instance,excluded.dispatched_instance), dispatched_mono_us=COALESCE(dispatched_mono_us,excluded.dispatched_mono_us), dispatched_wall_ms=COALESCE(dispatched_wall_ms,excluded.dispatched_wall_ms), woken_instance=COALESCE(woken_instance,excluded.woken_instance), woken_mono_us=COALESCE(woken_mono_us,excluded.woken_mono_us), woken_wall_ms=COALESCE(woken_wall_ms,excluded.woken_wall_ms)")
            .bind(&binding.member_id).bind(&self.clock.instance).bind(mono).bind(wall)
            .bind(woken.then_some(self.clock.instance.as_str())).bind(woken.then_some(mono)).bind(woken.then_some(wall))
            .bind(&binding.context_id).bind(&binding.member_id).bind(report.through)
            .execute(&mut *tx).await?.rows_affected();
        tx.commit().await?;
        Ok(covered)
    }

    /// Mark each of the member's receipted items that has no activity yet with this operation.
    /// Called by the connector before a non-transport operation runs, so an attempt counts.
    pub async fn record_activity(&self, binding: &SessionBinding, operation: &str) -> Result<()> {
        ensure!(valid_label(operation), "invalid operation name");
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        validate_on(&mut tx, binding).await?;
        let (mono, wall) = self.clock.now();
        sqlx::query("UPDATE candidate_recipient_facts SET active_instance=?, active_mono_us=?, active_wall_ms=?, active_operation=? WHERE member_id=? AND receipted_instance IS NOT NULL AND active_instance IS NULL AND EXISTS (SELECT 1 FROM candidate_items i WHERE i.sequence=candidate_recipient_facts.sequence AND i.context_id=?)")
            .bind(&self.clock.instance).bind(mono).bind(wall).bind(operation)
            .bind(&binding.member_id).bind(&binding.context_id)
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Whether the driver should re-hint the member: an unread item has waited longer than
    /// `interval_ms` since its commit, no wake attempt came within `interval_ms`, and fewer than
    /// `attempts` recovery attempts have covered the oldest unread item. Zero attempts is off.
    /// Read on the core's own wall clock.
    pub async fn recovery_due(
        &self,
        binding: &SessionBinding,
        interval_ms: i64,
        attempts: u32,
    ) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        validate_on(&mut tx, binding).await?;
        if attempts == 0 {
            return Ok(false);
        }
        let Some(oldest): Option<i64> = sqlx::query_scalar(
            "SELECT MIN(sequence) FROM candidate_unread_items WHERE context_id=? AND recipient=?",
        )
        .bind(&binding.context_id)
        .bind(&binding.member_id)
        .fetch_one(&mut *tx)
        .await?
        else {
            return Ok(false);
        };
        let now = wall_ms();
        // An item committed before the facts existed has no commit time: treat it as old.
        let committed: Option<i64> = sqlx::query_scalar(
            "SELECT committed_wall_ms FROM candidate_item_facts WHERE sequence=?",
        )
        .bind(oldest)
        .fetch_optional(&mut *tx)
        .await?;
        if now - committed.unwrap_or(0) < interval_ms {
            return Ok(false);
        }
        let row = sqlx::query("SELECT MAX(reported_wall_ms) AS last, COALESCE(SUM(recovery=1 AND through_sequence>=?),0) AS recoveries FROM candidate_wake_attempts WHERE context_id=? AND member_id=?")
            .bind(oldest).bind(&binding.context_id).bind(&binding.member_id)
            .fetch_one(&mut *tx).await?;
        let last: Option<i64> = row.try_get("last")?;
        let recoveries: i64 = row.try_get("recoveries")?;
        tx.commit().await?;
        if last.is_some_and(|last| now - last < interval_ms) {
            return Ok(false);
        }
        Ok(recoveries < i64::from(attempts))
    }

    /// Trusted administration (owner and dashboard). None only when the item does not exist.
    pub async fn delivery_timeline(
        &self,
        context: &str,
        sequence: i64,
    ) -> Result<Option<ItemTimeline>> {
        Ok(self
            .timeline_with_tags(context, sequence)
            .await?
            .map(|(t, _)| t))
    }

    /// Trusted administration (the dashboard). The outbox's events, work and request events,
    /// with sequence below `before`, newest first, at most `limit` (and never more than 100).
    pub async fn outbox_timelines(
        &self,
        context: &str,
        before: Option<i64>,
        limit: u32,
    ) -> Result<Vec<ItemTimeline>> {
        crate::bindings::valid_id(context)?;
        let sequences: Vec<i64> = sqlx::query_scalar(OUTBOX_PAGE)
            .bind(context)
            .bind(before.unwrap_or(i64::MAX))
            .bind(i64::from(limit.min(100)))
            .fetch_all(&self.pool)
            .await?;
        let mut timelines = Vec::with_capacity(sequences.len());
        for sequence in sequences {
            timelines.extend(self.delivery_timeline(context, sequence).await?);
        }
        Ok(timelines)
    }

    async fn timeline_with_tags(
        &self,
        context: &str,
        sequence: i64,
    ) -> Result<Option<(ItemTimeline, Vec<Option<String>>)>> {
        crate::bindings::valid_id(context)?;
        ensure!(sequence > 0, "invalid sequence");
        let mut conn = self.pool.acquire().await?;
        let Some(item) = sqlx::query("SELECT i.room_id, f.item_class, f.committed_instance, f.committed_mono_us, f.committed_wall_ms FROM candidate_items i LEFT JOIN candidate_item_facts f ON f.sequence=i.sequence WHERE i.sequence=? AND i.context_id=?")
            .bind(sequence).bind(context).fetch_optional(&mut *conn).await? else {
            return Ok(None);
        };
        let class: Option<String> = item.try_get("item_class")?;
        let committed = stamp(&item, "committed")?;
        let rows = sqlx::query(
            "SELECT * FROM candidate_recipient_facts WHERE sequence=? ORDER BY member_id",
        )
        .bind(sequence)
        .fetch_all(&mut *conn)
        .await?;
        let mut recipients = Vec::new();
        let mut tags = Vec::new();
        for row in rows {
            let offered = stamp(&row, "offered")?;
            let receipted = stamp(&row, "receipted")?;
            let dispatched = stamp(&row, "dispatched")?;
            let woken = stamp(&row, "woken")?;
            let active = stamp(&row, "active")?;
            let span = |end: &Option<Stamp>| match (&committed, end) {
                (Some(from), Some(to)) => Some(Interval::between(from, to)),
                _ => None,
            };
            recipients.push(RecipientTimeline {
                member: row.try_get("member_id")?,
                expected: row.try_get::<i64, _>("expected")? == 1,
                offer_count: row.try_get("offer_count")?,
                to_offer: span(&offered),
                to_receipt: span(&receipted),
                to_wake: span(&woken),
                to_activity: span(&active),
                offered,
                receipted,
                dispatched,
                woken,
                active,
                active_operation: row.try_get("active_operation")?,
            });
            tags.push(row.try_get("first_batch_tag")?);
        }
        Ok(Some((
            ItemTimeline {
                sequence,
                room: item.try_get("room_id")?,
                class: class.as_deref().map(ItemClass::parse).transpose()?,
                committed,
                recipients,
            },
            tags,
        )))
    }

    /// Trusted administration. Projects the durable facts into versioned events, oldest stage first.
    pub async fn delivery_events(
        &self,
        context: &str,
        sequence: i64,
    ) -> Result<Vec<DeliveryEvent>> {
        let Some((timeline, tags)) = self.timeline_with_tags(context, sequence).await? else {
            return Ok(Vec::new());
        };
        let event = |stage, member: Option<&str>, at: &Stamp, tag: Option<&String>| DeliveryEvent {
            schema: EVENT_SCHEMA,
            stage,
            context: context.to_owned(),
            room: timeline.room.clone(),
            sequence,
            member: member.map(str::to_owned),
            class: timeline.class,
            at: at.clone(),
            batch_tag: tag.cloned(),
        };
        let mut events = Vec::new();
        if let Some(at) = &timeline.committed {
            events.push(event(Stage::Committed, None, at, None));
        }
        for (recipient, tag) in timeline.recipients.iter().zip(&tags) {
            if let Some(at) = &recipient.offered {
                events.push(event(
                    Stage::InboxOffered,
                    Some(&recipient.member),
                    at,
                    tag.as_ref(),
                ));
            }
            if let Some(at) = &recipient.receipted {
                events.push(event(Stage::Receipted, Some(&recipient.member), at, None));
            }
            for (stage, at) in [
                (Stage::DispatchStarted, &recipient.dispatched),
                (Stage::NativeWakeAccepted, &recipient.woken),
                (Stage::PostReceiptActivity, &recipient.active),
            ] {
                if let Some(at) = at {
                    events.push(event(stage, Some(&recipient.member), at, None));
                }
            }
        }
        // Oldest first. Instances are ordered by when they opened (one owner at a time), and
        // stamps within an instance by its monotonic clock, so a wall-clock step cannot reorder.
        let mut conn = self.pool.acquire().await?;
        let mut rank = std::collections::HashMap::new();
        for event in &events {
            if !rank.contains_key(&event.at.instance) {
                let order: i64 = sqlx::query_scalar(
                    "SELECT rowid FROM candidate_core_instances WHERE instance_id=?",
                )
                .bind(&event.at.instance)
                .fetch_one(&mut *conn)
                .await?;
                rank.insert(event.at.instance.clone(), order);
            }
        }
        events.sort_by_key(|e| (rank[&e.at.instance], e.at.mono_us));
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_outbox_page_reads_through_its_index() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::store::MIGRATOR.run(&pool).await.unwrap();
        let plan: Vec<String> = sqlx::query(&format!("EXPLAIN QUERY PLAN {OUTBOX_PAGE}"))
            .bind("c")
            .bind(i64::MAX)
            .bind(21)
            .fetch_all(&pool)
            .await
            .unwrap()
            .iter()
            .map(|row| row.get::<String, _>("detail"))
            .collect();
        assert!(
            plan.iter()
                .any(|d| d.contains("INDEX candidate_outbox_facts")),
            "{plan:?}"
        );
        assert!(!plan.iter().any(|d| d.contains("TEMP B-TREE")), "{plan:?}");
    }

    #[test]
    fn batch_tag_is_short_stable_and_not_the_reference() {
        let reference = "a".repeat(64);
        let tag = batch_tag(&reference);
        assert_eq!(tag.len(), 16);
        assert_eq!(tag, batch_tag(&reference));
        assert!(!reference.contains(&tag));
    }

    #[test]
    fn cross_instance_intervals_are_estimates() {
        let a = Stamp {
            instance: "a".into(),
            mono_us: 10,
            wall_ms: 1_000,
        };
        let b = Stamp {
            instance: "b".into(),
            mono_us: 5,
            wall_ms: 1_250,
        };
        assert_eq!(
            Interval::between(&a, &b),
            Interval {
                micros: 250_000,
                basis: Basis::Estimate
            }
        );
        let c = Stamp {
            instance: "a".into(),
            mono_us: 40,
            wall_ms: 0,
        };
        assert_eq!(
            Interval::between(&a, &c),
            Interval {
                micros: 30,
                basis: Basis::Exact
            }
        );
    }
}
