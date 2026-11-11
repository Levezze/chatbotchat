//! Durable first-occurrence delivery facts and a versioned, content-free event projection (#113 PR-A).
//!
//! Facts are written inside the send, check and receipt transactions, so they are durable
//! whatever the log level and a failed receipt can never leave a false "receipted". Only the
//! stages the core observes itself have a write path here; dispatch, native wake and result
//! stages are declared for the schema and are written by the authenticated connector route.
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
    fn as_str(self) -> &'static str {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecipientTimeline {
    pub member: String,
    /// In the commit-time delivery snapshot.
    pub expected: bool,
    pub offer_count: i64,
    pub offered: Option<Stamp>,
    pub receipted: Option<Stamp>,
    /// None when either end is unknown.
    pub to_offer: Option<Interval>,
    pub to_receipt: Option<Interval>,
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

impl Store {
    /// This open's core instance ID.
    pub fn instance_id(&self) -> &str {
        &self.clock.instance
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
                offered,
                receipted,
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
