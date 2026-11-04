//! Durable member-bound inbox. Provisioning methods are trusted fixture administration only.
use crate::{
    bindings::{valid_id, validate_on, SessionBinding},
    store::Store,
};
use anyhow::{ensure, Result};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection};

pub const MAX_ITEMS: usize = 32;
pub const MAX_BODY_BYTES: usize = 16 * 1024;
pub const MAX_BATCH_BYTES: usize = 48 * 1024;
pub const MAX_ESCAPED_BODY_BYTES: usize = 2 * MAX_BODY_BYTES;
/// Retained for immutable batches and valid rows created before the smaller default budget.
pub const MAX_BATCH_HARD_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub sequence: i64,
    pub room: String,
    pub sender: String,
    pub kind: String,
    pub body: String,
}

/// Content-free routing hint for a trusted device connector, never a delivery receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxChange {
    pub context: String,
    pub member: String,
    pub sequence: i64,
}

pub struct Batch {
    pub items: Vec<Item>,
    pub(crate) reference: String,
}

impl Batch {
    /// Host-adapter receipt reference, never a model-supplied tool argument.
    pub fn reference_for_adapter(&self) -> &str {
        &self.reference
    }
}

impl Store {
    /// A device feed is authorized by each subscribed member, not by one global observer.
    /// Reading the revision creates no pending batch or receipt.
    pub(crate) async fn inbox_changes(
        &self,
        bindings: &[SessionBinding],
        after: i64,
    ) -> Result<Vec<InboxChange>> {
        ensure!(
            after >= 0 && (1..=64).contains(&bindings.len()),
            "invalid subscription count"
        );
        let mut unique = std::collections::HashSet::new();
        let mut tx = self.pool.begin().await?;
        for binding in bindings {
            ensure!(
                unique.insert((&binding.context_id, &binding.member_id)),
                "duplicate subscribed member"
            );
            validate_on(&mut tx, binding).await?;
        }
        let mut changes = Vec::new();
        for binding in bindings {
            let sequence:i64 = sqlx::query_scalar("SELECT COALESCE(MAX(i.sequence),0) FROM candidate_items i JOIN candidate_room_members m ON m.context_id=i.context_id AND m.room_id=i.room_id WHERE m.context_id=? AND m.member_id=? AND m.active=1 AND i.sender<>? AND i.sequence>? AND NOT EXISTS (SELECT 1 FROM candidate_receipts r WHERE r.context_id=i.context_id AND r.member_id=m.member_id AND r.sequence=i.sequence)")
                .bind(&binding.context_id).bind(&binding.member_id).bind(&binding.member_id).bind(after).fetch_one(&mut *tx).await?;
            if sequence > after {
                changes.push(InboxChange {
                    context: binding.context_id.clone(),
                    member: binding.member_id.clone(),
                    sequence,
                });
            }
        }
        tx.commit().await?;
        Ok(changes)
    }
    pub async fn close_room(&self, binding: &SessionBinding, room: &str) -> Result<i64> {
        valid_id(room)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        validate_on(&mut tx, binding).await?;
        authorized_room(&mut tx, binding, room).await?;
        let existing: Option<i64> = sqlx::query_scalar("SELECT sequence FROM candidate_items WHERE context_id=? AND room_id=? AND kind='closed'")
            .bind(&binding.context_id).bind(room).fetch_optional(&mut *tx).await?;
        let sequence = if let Some(sequence) = existing {
            sequence
        } else {
            let sequence = sqlx::query_scalar("INSERT INTO candidate_items(context_id,room_id,sender,kind,body) VALUES (?,?,?,'closed','') RETURNING sequence")
                .bind(&binding.context_id).bind(room).bind(&binding.member_id).fetch_one(&mut *tx).await?;
            sqlx::query("UPDATE candidate_rooms SET closed=1 WHERE context_id=? AND room_id=?")
                .bind(&binding.context_id)
                .bind(room)
                .execute(&mut *tx)
                .await?;
            sequence
        };
        tx.commit().await?;
        Ok(sequence)
    }
    /// Trusted administration. Tombstones preserve history; create_room never re-grants them.
    pub async fn revoke_room_member(&self, context: &str, room: &str, member: &str) -> Result<()> {
        for id in [context, room, member] {
            valid_id(id)?;
        }
        let changed = sqlx::query("UPDATE candidate_room_members SET active=0 WHERE context_id=? AND room_id=? AND member_id=?")
            .bind(context).bind(room).bind(member).execute(&self.pool).await?.rows_affected();
        ensure!(changed == 1, "room membership unavailable");
        Ok(())
    }
    pub async fn receipt(&self, binding: &SessionBinding, reference: &str) -> Result<()> {
        ensure!(reference.len() == 64, "invalid batch reference");
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        validate_on(&mut tx, binding).await?;
        let row = sqlx::query("SELECT state FROM candidate_batches WHERE reference=? AND context_id=? AND member_id=? AND epoch=?")
            .bind(reference).bind(&binding.context_id).bind(&binding.member_id).bind(binding.epoch)
            .fetch_optional(&mut *tx).await?;
        let row = row.ok_or_else(|| anyhow::anyhow!("invalid batch reference"))?;
        let state: String = row.try_get("state")?;
        ensure!(
            state == "pending" || state == "receipted",
            "batch no longer eligible; recheck"
        );
        if !batch_authorized(&mut tx, binding, reference).await? {
            sqlx::query("UPDATE candidate_batches SET state='cancelled' WHERE reference=? AND state='pending'")
                .bind(reference).execute(&mut *tx).await?;
            tx.commit().await?;
            anyhow::bail!("batch no longer eligible; recheck");
        }
        if state == "pending" {
            sqlx::query("INSERT INTO candidate_receipts(context_id,member_id,sequence) SELECT ?,?,sequence FROM candidate_batch_items WHERE reference=? ON CONFLICT DO NOTHING")
                .bind(&binding.context_id).bind(&binding.member_id).bind(reference).execute(&mut *tx).await?;
            sqlx::query("UPDATE candidate_batches SET state='receipted' WHERE reference=?")
                .bind(reference)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    /// Trusted administration only. Repeating creation never restores revoked membership.
    pub async fn create_room(&self, context: &str, room: &str, members: &[&str]) -> Result<()> {
        valid_id(context)?;
        valid_id(room)?;
        ensure!(
            members.len() >= 2 && members.len() <= 128,
            "invalid room member count"
        );
        for member in members {
            valid_id(member)?;
        }
        let mut members = members.to_vec();
        members.sort();
        ensure!(
            members.windows(2).all(|w| w[0] != w[1]),
            "duplicate room member"
        );
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let exists: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM candidate_rooms WHERE context_id=? AND room_id=?",
        )
        .bind(context)
        .bind(room)
        .fetch_one(&mut *tx)
        .await?;
        if exists != 0 {
            let stored: Vec<String> = sqlx::query_scalar("SELECT member_id FROM candidate_room_members WHERE context_id=? AND room_id=? ORDER BY member_id")
                .bind(context).bind(room).fetch_all(&mut *tx).await?;
            ensure!(stored == members, "room members conflict");
        } else {
            for member in &members {
                let active: i64 = sqlx::query_scalar("SELECT count(*) FROM candidate_members WHERE context_id=? AND member_id=? AND active=1")
                    .bind(context).bind(member).fetch_one(&mut *tx).await?;
                ensure!(active == 1, "room member unavailable");
            }
            sqlx::query("INSERT INTO candidate_rooms(context_id,room_id) VALUES (?,?)")
                .bind(context)
                .bind(room)
                .execute(&mut *tx)
                .await?;
            for member in members {
                sqlx::query("INSERT INTO candidate_room_members(context_id,room_id,member_id) VALUES (?,?,?)")
                    .bind(context).bind(room).bind(member).execute(&mut *tx).await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn send(
        &self,
        binding: &SessionBinding,
        room: &str,
        key: &str,
        body: &str,
    ) -> Result<i64> {
        valid_id(room)?;
        valid_id(key)?;
        ensure!(
            !body.is_empty() && body.len() <= MAX_BODY_BYTES,
            "message body exceeds bounds"
        );
        ensure!(
            serde_json::to_vec(body)?.len() - 2 <= MAX_ESCAPED_BODY_BYTES,
            "message body too large once JSON-escaped"
        );
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        validate_on(&mut tx, binding).await?;
        authorized_room(&mut tx, binding, room).await?;
        if let Some(row) = sqlx::query("SELECT sequence,room_id,body FROM candidate_items WHERE context_id=? AND sender=? AND request_key=?")
            .bind(&binding.context_id).bind(&binding.member_id).bind(key).fetch_optional(&mut *tx).await? {
            ensure!(row.try_get::<String,_>("room_id")? == room && row.try_get::<String,_>("body")? == body, "idempotency key conflicts");
            return Ok(row.try_get("sequence")?);
        }
        let closed: i64 = sqlx::query_scalar(
            "SELECT closed FROM candidate_rooms WHERE context_id=? AND room_id=?",
        )
        .bind(&binding.context_id)
        .bind(room)
        .fetch_one(&mut *tx)
        .await?;
        ensure!(closed == 0, "room is closed");
        let sequence = sqlx::query("INSERT INTO candidate_items(context_id,room_id,sender,kind,body,request_key) VALUES (?,?,?,'message',?,?) RETURNING sequence")
            .bind(&binding.context_id).bind(room).bind(&binding.member_id).bind(body).bind(key)
            .fetch_one(&mut *tx).await?.try_get("sequence")?;
        tx.commit().await?;
        Ok(sequence)
    }
    pub async fn check(&self, binding: &SessionBinding, max_items: usize) -> Result<Option<Batch>> {
        ensure!((1..=MAX_ITEMS).contains(&max_items), "invalid batch count");
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        validate_on(&mut tx, binding).await?;
        sqlx::query("UPDATE candidate_batches SET state='superseded' WHERE context_id=? AND member_id=? AND epoch<>? AND state='pending'")
            .bind(&binding.context_id).bind(&binding.member_id).bind(binding.epoch).execute(&mut *tx).await?;
        let pending: Option<String> = sqlx::query_scalar("SELECT reference FROM candidate_batches WHERE context_id=? AND member_id=? AND epoch=? AND state='pending'")
            .bind(&binding.context_id).bind(&binding.member_id).bind(binding.epoch).fetch_optional(&mut *tx).await?;
        if let Some(reference) = pending {
            if batch_authorized(&mut tx, binding, &reference).await? {
                let items = batch_items(&mut tx, &reference).await?;
                tx.commit().await?;
                return Ok(Some(Batch { items, reference }));
            }
            sqlx::query("UPDATE candidate_batches SET state='cancelled' WHERE reference=?")
                .bind(reference)
                .execute(&mut *tx)
                .await?;
        }
        let mut rooms: Vec<String> = sqlx::query_scalar("SELECT room_id FROM candidate_room_members WHERE context_id=? AND member_id=? AND active=1 ORDER BY room_id")
            .bind(&binding.context_id).bind(&binding.member_id).fetch_all(&mut *tx).await?;
        let last: Option<String> = sqlx::query_scalar(
            "SELECT last_room FROM candidate_inbox_state WHERE context_id=? AND member_id=?",
        )
        .bind(&binding.context_id)
        .bind(&binding.member_id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(last) = last {
            if !rooms.is_empty() {
                let start = rooms.partition_point(|r| r <= &last) % rooms.len();
                rooms.rotate_left(start);
            }
        }
        let mut after = std::collections::HashMap::<String, i64>::new();
        let mut items = Vec::new();
        'rounds: loop {
            let before = items.len();
            for room in &rooms {
                let row = sqlx::query("SELECT i.sequence,i.room_id,i.sender,i.kind,i.body FROM candidate_items i WHERE i.context_id=? AND i.room_id=? AND i.sender<>? AND i.sequence>? AND NOT EXISTS (SELECT 1 FROM candidate_receipts r WHERE r.context_id=i.context_id AND r.member_id=? AND r.sequence=i.sequence) ORDER BY i.sequence LIMIT 1")
                    .bind(&binding.context_id).bind(room).bind(&binding.member_id).bind(*after.get(room).unwrap_or(&0)).bind(&binding.member_id).fetch_optional(&mut *tx).await?;
                if let Some(r) = row {
                    let item = Item {
                        sequence: r.try_get("sequence")?,
                        room: r.try_get("room_id")?,
                        sender: r.try_get("sender")?,
                        kind: r.try_get("kind")?,
                        body: r.try_get("body")?,
                    };
                    let sequence = item.sequence;
                    items.push(item);
                    let bytes = serde_json::to_vec(&items)?.len();
                    if bytes > MAX_BATCH_BYTES {
                        // Older stored rows can exceed today's admission rule. Deliver one intact
                        // within the historical hard bound instead of returning a false empty.
                        if items.len() == 1 {
                            ensure!(
                                bytes <= MAX_BATCH_HARD_BYTES,
                                "stored item exceeds hard delivery bound"
                            );
                            break 'rounds;
                        }
                        items.pop();
                        break 'rounds;
                    }
                    after.insert(room.clone(), sequence);
                    if items.len() == max_items {
                        break 'rounds;
                    }
                }
            }
            if items.len() == before {
                break;
            }
        }
        let batch = if items.is_empty() {
            None
        } else {
            let mut random = [0_u8; 32];
            OsRng.try_fill_bytes(&mut random)?;
            let reference = random
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            sqlx::query("INSERT INTO candidate_batches(reference,context_id,member_id,epoch,state) VALUES (?,?,?,?,'pending')")
                .bind(&reference).bind(&binding.context_id).bind(&binding.member_id).bind(binding.epoch).execute(&mut *tx).await?;
            for (ordinal, item) in items.iter().enumerate() {
                sqlx::query(
                    "INSERT INTO candidate_batch_items(reference,ordinal,sequence) VALUES (?,?,?)",
                )
                .bind(&reference)
                .bind(ordinal as i64)
                .bind(item.sequence)
                .execute(&mut *tx)
                .await?;
            }
            sqlx::query("INSERT INTO candidate_inbox_state(context_id,member_id,last_room) VALUES (?,?,?) ON CONFLICT(context_id,member_id) DO UPDATE SET last_room=excluded.last_room")
                .bind(&binding.context_id).bind(&binding.member_id).bind(&items.last().expect("nonempty batch").room).execute(&mut *tx).await?;
            Some(Batch { items, reference })
        };
        tx.commit().await?;
        Ok(batch)
    }
}

async fn batch_authorized(
    conn: &mut SqliteConnection,
    binding: &SessionBinding,
    reference: &str,
) -> Result<bool> {
    let invalid: i64 = sqlx::query_scalar("SELECT count(*) FROM candidate_batch_items b JOIN candidate_items i ON i.sequence=b.sequence LEFT JOIN candidate_room_members m ON m.context_id=i.context_id AND m.room_id=i.room_id AND m.member_id=? WHERE b.reference=? AND (i.context_id<>? OR COALESCE(m.active,0)<>1)")
        .bind(&binding.member_id).bind(reference).bind(&binding.context_id).fetch_one(conn).await?;
    Ok(invalid == 0)
}

async fn batch_items(conn: &mut SqliteConnection, reference: &str) -> Result<Vec<Item>> {
    let rows = sqlx::query("SELECT i.sequence,i.room_id,i.sender,i.kind,i.body FROM candidate_batch_items b JOIN candidate_items i ON i.sequence=b.sequence WHERE b.reference=? ORDER BY b.ordinal")
        .bind(reference).fetch_all(conn).await?;
    rows.into_iter()
        .map(|r| {
            Ok(Item {
                sequence: r.try_get("sequence")?,
                room: r.try_get("room_id")?,
                sender: r.try_get("sender")?,
                kind: r.try_get("kind")?,
                body: r.try_get("body")?,
            })
        })
        .collect()
}

async fn authorized_room(
    conn: &mut SqliteConnection,
    binding: &SessionBinding,
    room: &str,
) -> Result<()> {
    let active: i64 = sqlx::query_scalar("SELECT count(*) FROM candidate_room_members WHERE context_id=? AND room_id=? AND member_id=? AND active=1")
        .bind(&binding.context_id).bind(room).bind(&binding.member_id).fetch_one(conn).await?;
    ensure!(active == 1, "room is unavailable");
    Ok(())
}
