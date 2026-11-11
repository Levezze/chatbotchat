//! Facts loaded from a snapshot, each with the timestamps that place it in time.
use super::*;

/// One reading of the core clock. Monotonic offsets compare only within one core instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stamp {
    pub instance: String,
    pub mono_us: i64,
    pub wall_ms: i64,
}

impl Stamp {
    /// Milliseconds from `self` to `later`, and whether it is a cross-instance estimate.
    pub(super) fn until(&self, later: &Stamp) -> (i64, bool) {
        if self.instance == later.instance {
            ((later.mono_us - self.mono_us).div_euclid(1000), false)
        } else {
            (later.wall_ms - self.wall_ms, true)
        }
    }
}

/// One expected recipient of one committed item.
#[derive(Debug, Clone)]
pub(crate) struct Delivery {
    pub context: String,
    pub member: String,
    pub sequence: i64,
    pub committed: Stamp,
    pub offered: Option<Stamp>,
    pub receipted: Option<Stamp>,
    pub woken: Option<Stamp>,
    pub active: Option<Stamp>,
    /// When an unreceipted room notice stopped being deliverable, if it did by the end snapshot:
    /// a newer notice for the same room replaced it. See `superseded_at`.
    pub superseded_wall_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub(crate) struct RequestFact {
    pub context: String,
    pub request: String,
    pub callback: String,
    pub owner_is_user: bool,
    pub owner_member: Option<String>,
    pub created_wall_ms: i64,
    pub answered_wall_ms: Option<i64>,
    /// Withdrawn by the end snapshot. With no time it cannot be placed: left out and counted.
    pub withdrawn: bool,
    pub withdrawn_wall_ms: Option<i64>,
}

/// One native binding of a seat, in force from its enrollment or rebind. A delivery belongs to
/// the binding in force when its item was committed, not to the binding at the end snapshot.
#[derive(Debug, Clone)]
pub(crate) struct BindingFact {
    pub context: String,
    pub member: String,
    pub epoch: i64,
    /// From the epoch's native target, else the invite it was enrolled with.
    pub host: Option<String>,
    /// None for an epoch with no native target.
    pub profile: Option<String>,
    /// `i64::MIN` when no identity event dates it: in force from the start.
    pub from_wall_ms: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct WakeFact {
    pub kind: String,
    pub failure: Option<String>,
    pub reported_wall_ms: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct IdentityFact {
    pub context: String,
    pub member: String,
    pub kind: String,
    pub epoch: i64,
    pub at_wall_ms: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct ConnectionFact {
    pub context: String,
    pub first: String,
    pub second: String,
    pub room: String,
    pub supersedes: Option<String>,
    /// None for a room introduced before delivery facts existed.
    pub introduced_wall_ms: Option<i64>,
}

/// When a room notice was replaced, given the first newer notice for the same room that reached
/// the same member. Unread notices coalesce, so a replaced notice is never offered or receipted.
/// One already offered stays in its batch until receipt, or until a rebind or re-enrolment of the
/// member drops the batch. A receipt by then means it was delivered, not replaced.
pub(super) fn superseded_at(
    delivery: &Delivery,
    newer_notice_wall_ms: Option<i64>,
    identity: &[IdentityFact],
) -> Option<i64> {
    let newer = newer_notice_wall_ms?;
    let at = match &delivery.offered {
        Some(offered) if offered.wall_ms <= newer => identity
            .iter()
            .filter(|e| e.context == delivery.context && e.member == delivery.member)
            .filter(|e| matches!(e.kind.as_str(), "enrolled" | "rebound"))
            .filter(|e| e.at_wall_ms >= offered.wall_ms)
            .map(|e| e.at_wall_ms.max(newer))
            .min()?,
        _ => newer,
    };
    match &delivery.receipted {
        Some(receipted) if receipted.wall_ms <= at => None,
        _ => Some(at),
    }
}

/// The revision a work event records moving its item to `merged`, if it does.
pub(super) fn merge_revision(body: &str) -> Option<i64> {
    let event: Value = serde_json::from_str(body).ok()?;
    let moved = event["changed"]
        .as_array()?
        .iter()
        .any(|field| field == "stage");
    (moved && event["stage"] == "merged").then(|| event["revision"].as_i64())?
}

#[derive(Debug, Clone)]
pub(crate) struct SentFact {
    pub context: String,
    pub sender: String,
    pub wall_ms: i64,
}

/// Everything the report reads from the end snapshot, in a total order.
#[derive(Debug, Clone, Default)]
pub(crate) struct Facts {
    pub seats: Vec<Seat>,
    pub bindings: Vec<BindingFact>,
    pub deliveries: Vec<Delivery>,
    pub requests: Vec<RequestFact>,
    pub wakes: Vec<WakeFact>,
    pub identity: Vec<IdentityFact>,
    pub connections: Vec<ConnectionFact>,
    pub sent: Vec<SentFact>,
    /// When each work item revision that moved its item to `merged` was first committed.
    pub merges: Vec<i64>,
}

pub(super) fn stamp(row: &sqlx::sqlite::SqliteRow, prefix: &str) -> Result<Option<Stamp>> {
    let instance: Option<String> = row.try_get(format!("{prefix}_instance").as_str())?;
    let mono: Option<i64> = row.try_get(format!("{prefix}_mono_us").as_str())?;
    let wall: Option<i64> = row.try_get(format!("{prefix}_wall_ms").as_str())?;
    Ok(match (instance, mono, wall) {
        (Some(instance), Some(mono_us), Some(wall_ms)) => Some(Stamp {
            instance,
            mono_us,
            wall_ms,
        }),
        _ => None,
    })
}

impl Facts {
    pub(crate) async fn load(evidence: &Evidence) -> Result<Self> {
        let pool = evidence.pool();
        let mut facts = Self {
            seats: evidence.seats().await?,
            ..Self::default()
        };
        // Resolved once identity events are loaded: a rebind can end an offered notice's batch.
        let mut newer_notices = Vec::new();
        for row in sqlx::query(
            "SELECT f.context_id, r.member_id, r.sequence,
                    f.committed_instance, f.committed_mono_us, f.committed_wall_ms,
                    r.offered_instance, r.offered_mono_us, r.offered_wall_ms,
                    r.receipted_instance, r.receipted_mono_us, r.receipted_wall_ms,
                    r.woken_instance, r.woken_mono_us, r.woken_wall_ms,
                    r.active_instance, r.active_mono_us, r.active_wall_ms,
                    (SELECT min(nf.committed_wall_ms)
                       FROM candidate_room_notices n
                       JOIN candidate_room_notices newer
                         ON newer.context_id = n.context_id AND newer.room_id = n.room_id
                        AND newer.message_count > n.message_count
                       JOIN candidate_notice_recipients nr
                         ON nr.sequence = newer.sequence AND nr.member_id = r.member_id
                       JOIN candidate_item_facts nf ON nf.sequence = newer.sequence
                      WHERE n.sequence = r.sequence) AS newer_notice_wall_ms
             FROM candidate_recipient_facts r
             JOIN candidate_item_facts f ON f.sequence = r.sequence
             WHERE r.expected = 1
             ORDER BY r.sequence, r.member_id",
        )
        .fetch_all(pool)
        .await?
        {
            facts.deliveries.push(Delivery {
                context: row.try_get("context_id")?,
                member: row.try_get("member_id")?,
                sequence: row.try_get("sequence")?,
                committed: stamp(&row, "committed")?.context("item fact without commit")?,
                offered: stamp(&row, "offered")?,
                receipted: stamp(&row, "receipted")?,
                woken: stamp(&row, "woken")?,
                active: stamp(&row, "active")?,
                superseded_wall_ms: None,
            });
            newer_notices.push(row.try_get::<Option<i64>, _>("newer_notice_wall_ms")?);
        }
        for row in sqlx::query(
            "SELECT q.context_id, q.request_id, q.callback_member, q.owner_kind, q.owner_member,
                    q.created_wall_ms, q.state = 'withdrawn' AS withdrawn, q.withdrawn_wall_ms,
                    (SELECT min(a.at_wall_ms) FROM candidate_request_answers a
                      WHERE a.context_id = q.context_id AND a.request_id = q.request_id)
                      AS answered_wall_ms
             FROM candidate_requests q
             ORDER BY q.context_id, q.request_id",
        )
        .fetch_all(pool)
        .await?
        {
            facts.requests.push(RequestFact {
                context: row.try_get("context_id")?,
                request: row.try_get("request_id")?,
                callback: row.try_get("callback_member")?,
                owner_is_user: row.try_get::<String, _>("owner_kind")? == "user",
                owner_member: row.try_get("owner_member")?,
                created_wall_ms: row.try_get("created_wall_ms")?,
                answered_wall_ms: row.try_get("answered_wall_ms")?,
                withdrawn: row.try_get("withdrawn")?,
                withdrawn_wall_ms: row.try_get("withdrawn_wall_ms")?,
            });
        }
        for row in sqlx::query(
            // Every enrolled or rebound epoch, plus target epochs no identity event dates. A
            // targetless epoch still ends the one before it: its host comes from the invite.
            "WITH epochs AS (
                 SELECT context_id, member_id, epoch, min(at_wall_ms) AS from_wall_ms
                 FROM candidate_identity_events WHERE kind IN ('enrolled','rebound')
                 GROUP BY context_id, member_id, epoch
                 UNION ALL
                 SELECT t.context_id, t.member_id, t.epoch, NULL
                 FROM candidate_native_targets t
                 WHERE NOT EXISTS (
                     SELECT 1 FROM candidate_identity_events e
                     WHERE e.context_id = t.context_id AND e.member_id = t.member_id
                       AND e.epoch = t.epoch AND e.kind IN ('enrolled','rebound'))
             )
             SELECT p.context_id, p.member_id, p.epoch, p.from_wall_ms,
                    coalesce(t.host,
                      (SELECT min(i.host) FROM candidate_enrollment_invites i
                        WHERE i.context_id = p.context_id AND i.member_id = p.member_id
                          AND i.consumed_epoch = p.epoch)) AS host,
                    json_extract(t.target_json, '$.profile') AS profile
             FROM epochs p
             LEFT JOIN candidate_native_targets t
               ON t.context_id = p.context_id AND t.member_id = p.member_id AND t.epoch = p.epoch
             ORDER BY p.context_id, p.member_id, p.epoch",
        )
        .fetch_all(pool)
        .await?
        {
            facts.bindings.push(BindingFact {
                context: row.try_get("context_id")?,
                member: row.try_get("member_id")?,
                epoch: row.try_get("epoch")?,
                host: row.try_get("host")?,
                profile: row.try_get("profile")?,
                from_wall_ms: row
                    .try_get::<Option<i64>, _>("from_wall_ms")?
                    .unwrap_or(i64::MIN),
            });
        }
        for row in sqlx::query(
            "SELECT kind, failure, reported_wall_ms FROM candidate_wake_attempts
             ORDER BY attempt_id",
        )
        .fetch_all(pool)
        .await?
        {
            facts.wakes.push(WakeFact {
                kind: row.try_get("kind")?,
                failure: row.try_get("failure")?,
                reported_wall_ms: row.try_get("reported_wall_ms")?,
            });
        }
        for row in sqlx::query(
            "SELECT context_id, member_id, kind, epoch, at_wall_ms
             FROM candidate_identity_events ORDER BY event_id",
        )
        .fetch_all(pool)
        .await?
        {
            facts.identity.push(IdentityFact {
                context: row.try_get("context_id")?,
                member: row.try_get("member_id")?,
                kind: row.try_get("kind")?,
                epoch: row.try_get("epoch")?,
                at_wall_ms: row.try_get("at_wall_ms")?,
            });
        }
        for row in sqlx::query(
            "SELECT c.context_id, c.first_member, c.second_member, c.room_id, c.supersedes_room,
                    f.committed_wall_ms
             FROM candidate_connections c
             LEFT JOIN candidate_item_facts f ON f.sequence = c.introduction_sequence
             ORDER BY c.context_id, c.room_id",
        )
        .fetch_all(pool)
        .await?
        {
            facts.connections.push(ConnectionFact {
                context: row.try_get("context_id")?,
                first: row.try_get("first_member")?,
                second: row.try_get("second_member")?,
                room: row.try_get("room_id")?,
                supersedes: row.try_get("supersedes_room")?,
                introduced_wall_ms: row.try_get("committed_wall_ms")?,
            });
        }
        for row in sqlx::query(
            "SELECT i.context_id, i.sender, f.committed_wall_ms
             FROM candidate_items i JOIN candidate_item_facts f ON f.sequence = i.sequence
             WHERE f.item_class = 'participant'
             ORDER BY i.sequence",
        )
        .fetch_all(pool)
        .await?
        {
            facts.sent.push(SentFact {
                context: row.try_get("context_id")?,
                sender: row.try_get("sender")?,
                wall_ms: row.try_get("committed_wall_ms")?,
            });
        }
        for (delivery, newer) in facts.deliveries.iter_mut().zip(newer_notices) {
            delivery.superseded_wall_ms = superseded_at(delivery, newer, &facts.identity);
        }
        // One revision emits an event per report room; the first commit stands for it.
        let mut merges: BTreeMap<(String, String, i64), i64> = BTreeMap::new();
        for row in sqlx::query(
            "SELECT e.context_id, e.item_id, i.body, f.committed_wall_ms
             FROM candidate_work_events e
             JOIN candidate_items i ON i.sequence = e.sequence
             JOIN candidate_item_facts f ON f.sequence = e.sequence
             ORDER BY e.sequence",
        )
        .fetch_all(pool)
        .await?
        {
            let body: String = row.try_get("body")?;
            if let Some(revision) = merge_revision(&body) {
                let wall_ms: i64 = row.try_get("committed_wall_ms")?;
                merges
                    .entry((
                        row.try_get("context_id")?,
                        row.try_get("item_id")?,
                        revision,
                    ))
                    .and_modify(|at| *at = (*at).min(wall_ms))
                    .or_insert(wall_ms);
            }
        }
        facts.merges = merges.into_values().collect();
        Ok(facts)
    }
}
