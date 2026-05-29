use crate::message::{Message, MessageType};
use crate::participant::Participant;
use crate::room::{Room, RoomConfig, RoomState};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("corrupt row: {0}")]
    Corrupt(String),
}

impl StorageError {
    /// True when the underlying failure is a UNIQUE/primary-key violation —
    /// e.g. inserting a room whose id already exists. Callers use this to
    /// disambiguate and retry rather than surfacing a 500.
    pub fn is_unique_violation(&self) -> bool {
        matches!(self, StorageError::Sqlx(sqlx::Error::Database(e)) if e.is_unique_violation())
    }
}

/// All database access goes through `Storage`. Callers never see SQL.
#[derive(Clone)]
pub struct Storage {
    pool: SqlitePool,
}

impl Storage {
    /// Connect to a SQLite database at `url`, creating the file if needed,
    /// enabling WAL, and applying migrations. Use `sqlite::memory:` for tests.
    pub async fn connect(url: &str) -> Result<Self, StorageError> {
        let options = SqliteConnectOptions::from_str(url)?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);

        // A bare `:memory:` database is private to a single connection — each
        // pooled connection would otherwise be a *separate* empty database, so
        // migrations and writes on one would be invisible to reads on another.
        // Pin the pool to one connection in that case so in-memory use (tests)
        // behaves like a single coherent database. File-backed databases keep
        // the default pool and benefit from WAL concurrency.
        let mut pool_options = SqlitePoolOptions::new();
        if is_memory_url(url) {
            pool_options = pool_options.max_connections(1);
        }

        let pool = pool_options.connect_with(options).await?;
        let storage = Storage { pool };
        storage.migrate().await?;
        Ok(storage)
    }

    async fn migrate(&self) -> Result<(), StorageError> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }

    pub async fn create_room(&self, room: &Room) -> Result<(), StorageError> {
        let started_at = room.started_at.format(&Rfc3339).map_err(fmt_err)?;
        let last_activity_at = room.last_activity_at.format(&Rfc3339).map_err(fmt_err)?;
        let config = serde_json::to_string(&room.config)
            .map_err(|e| StorageError::Corrupt(e.to_string()))?;

        sqlx::query(
            "INSERT INTO rooms \
             (id, subject, started_at, last_activity_at, state, config, prev_room_id) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&room.id)
        .bind(&room.subject)
        .bind(started_at)
        .bind(last_activity_at)
        .bind(room.state.as_str())
        .bind(config)
        .bind(&room.prev_room_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_room(&self, id: &str) -> Result<Option<Room>, StorageError> {
        let row = sqlx::query(
            "SELECT id, subject, started_at, last_activity_at, state, config, prev_room_id \
             FROM rooms WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            None => Ok(None),
            Some(row) => Ok(Some(row_to_room(&row)?)),
        }
    }

    pub async fn create_participant(&self, p: &Participant) -> Result<(), StorageError> {
        let joined_at = p.joined_at.format(&Rfc3339).map_err(fmt_err)?;
        let last_poll_at = p.last_poll_at.format(&Rfc3339).map_err(fmt_err)?;

        sqlx::query(
            "INSERT INTO participants \
             (handle, room_id, repo, model, cwd, joined_at, last_poll_at, last_read_seq) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&p.handle)
        .bind(&p.room_id)
        .bind(&p.repo)
        .bind(&p.model)
        .bind(&p.cwd)
        .bind(joined_at)
        .bind(last_poll_at)
        .bind(p.last_read_seq)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Append a `msg` to a room. Returns the persisted message with its assigned
    /// monotonic `seq`.
    pub async fn create_message(
        &self,
        room_id: &str,
        sender: &str,
        recipient: Option<&str>,
        body: &str,
        created_at: OffsetDateTime,
    ) -> Result<Message, StorageError> {
        let created = created_at.format(&Rfc3339).map_err(fmt_err)?;

        let result = sqlx::query(
            "INSERT INTO messages (room_id, sender, recipient, body, created_at, type) \
             VALUES (?, ?, ?, ?, ?, 'msg')",
        )
        .bind(room_id)
        .bind(sender)
        .bind(recipient)
        .bind(body)
        .bind(created)
        .execute(&self.pool)
        .await?;

        Ok(Message {
            seq: result.last_insert_rowid(),
            room_id: room_id.to_string(),
            sender: sender.to_string(),
            recipient: recipient.map(str::to_string),
            body: body.to_string(),
            created_at,
            msg_type: MessageType::Msg,
        })
    }

    /// Append a message of an arbitrary `msg_type` (uncapped). Sentinels are
    /// written through here; the bare `create_message` is the `msg`-only path.
    /// Cap enforcement lives in `create_message_capped`, so this never gates.
    pub async fn create_message_typed(
        &self,
        room_id: &str,
        sender: &str,
        recipient: Option<&str>,
        body: &str,
        created_at: OffsetDateTime,
        msg_type: MessageType,
    ) -> Result<Message, StorageError> {
        let created = created_at.format(&Rfc3339).map_err(fmt_err)?;

        let result = sqlx::query(
            "INSERT INTO messages (room_id, sender, recipient, body, created_at, type) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(room_id)
        .bind(sender)
        .bind(recipient)
        .bind(body)
        .bind(created)
        .bind(msg_type.as_str())
        .execute(&self.pool)
        .await?;

        Ok(Message {
            seq: result.last_insert_rowid(),
            room_id: room_id.to_string(),
            sender: sender.to_string(),
            recipient: recipient.map(str::to_string),
            body: body.to_string(),
            created_at,
            msg_type,
        })
    }

    /// Append a `msg` only if the room is below `hard_cap` cap-counting messages,
    /// returning `None` (nothing written) when the cap is already reached. The
    /// count and the insert are a single SQL statement (`INSERT … SELECT … WHERE
    /// (SELECT COUNT(*) …) < ?`), so the gate is atomic: concurrent senders
    /// cannot both observe `count < cap` and both insert past it. This is the
    /// enforcement path; the bare `create_message` stays for uncapped inserts.
    ///
    /// The `COUNT(*)` here and `count_capped_messages` are the two cap-count
    /// seams: both count `type = 'msg'` only, so sentinels never consume cap
    /// budget. They must move in lockstep, or the hard cap and the soft counter
    /// disagree about what counts.
    pub async fn create_message_capped(
        &self,
        room_id: &str,
        sender: &str,
        recipient: Option<&str>,
        body: &str,
        created_at: OffsetDateTime,
        hard_cap: i64,
    ) -> Result<Option<Message>, StorageError> {
        let created = created_at.format(&Rfc3339).map_err(fmt_err)?;

        let result = sqlx::query(
            "INSERT INTO messages (room_id, sender, recipient, body, created_at, type) \
             SELECT ?, ?, ?, ?, ?, 'msg' \
             WHERE (SELECT COUNT(*) FROM messages WHERE room_id = ? AND type = 'msg') < ?",
        )
        .bind(room_id)
        .bind(sender)
        .bind(recipient)
        .bind(body)
        .bind(created)
        .bind(room_id)
        .bind(hard_cap)
        .execute(&self.pool)
        .await?;

        // Zero rows means the `WHERE` filtered the insert out — the room was at
        // the cap. `last_insert_rowid()` would be stale here, so guard first.
        if result.rows_affected() == 0 {
            return Ok(None);
        }

        Ok(Some(Message {
            seq: result.last_insert_rowid(),
            room_id: room_id.to_string(),
            sender: sender.to_string(),
            recipient: recipient.map(str::to_string),
            body: body.to_string(),
            created_at,
            msg_type: MessageType::Msg,
        }))
    }

    /// The oldest message in `room_id` with `seq > after_seq` that is addressed
    /// to `handle` or broadcast to all (`recipient IS NULL`), excluding the
    /// caller's own messages (`wait` is an inbox, not a log). `None` when the
    /// caller is caught up.
    pub async fn next_unread(
        &self,
        room_id: &str,
        handle: &str,
        after_seq: i64,
    ) -> Result<Option<Message>, StorageError> {
        let row = sqlx::query(
            "SELECT seq, room_id, sender, recipient, body, created_at, type \
             FROM messages \
             WHERE room_id = ? AND seq > ? AND sender != ? \
               AND (recipient IS NULL OR recipient = ?) \
             ORDER BY seq LIMIT 1",
        )
        .bind(room_id)
        .bind(after_seq)
        .bind(handle)
        .bind(handle)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            None => Ok(None),
            Some(row) => Ok(Some(row_to_message(&row)?)),
        }
    }

    /// The highest message `seq` in a room, or 0 if it has none. Used to seed a
    /// new participant's read cursor at join, so `wait` only delivers messages
    /// that arrive *after* they joined — the pre-join backlog is the log view
    /// (`recent_messages`), not unread inbox traffic.
    pub async fn current_seq(&self, room_id: &str) -> Result<i64, StorageError> {
        let seq: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(seq), 0) FROM messages WHERE room_id = ?")
                .bind(room_id)
                .fetch_one(&self.pool)
                .await?;
        Ok(seq)
    }

    /// The number of cap-counting messages in a room. Read-only view of the same
    /// count `create_message_capped` enforces against (used by tests and, later,
    /// the room status/summary cap counters). Only `type = 'msg'` rows count —
    /// sentinels are signals, not conversation turns. This filter and the
    /// `COUNT(*)` subquery in `create_message_capped` are the two cap-count seams
    /// and must stay in lockstep, or the hard cap and the soft counter disagree.
    pub async fn count_capped_messages(&self, room_id: &str) -> Result<i64, StorageError> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE room_id = ? AND type = 'msg'")
                .bind(room_id)
                .fetch_one(&self.pool)
                .await?;
        Ok(count)
    }

    /// The most recent `limit` messages in a room, returned oldest-first. This
    /// is the log view (every sender), surfaced to a joining participant.
    pub async fn recent_messages(
        &self,
        room_id: &str,
        limit: i64,
    ) -> Result<Vec<Message>, StorageError> {
        let rows = sqlx::query(
            "SELECT seq, room_id, sender, recipient, body, created_at, type \
             FROM messages WHERE room_id = ? ORDER BY seq DESC LIMIT ?",
        )
        .bind(room_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        // Fetched newest-first for the LIMIT; hand back chronological.
        rows.iter().rev().map(row_to_message).collect()
    }

    /// Refresh a participant's `last_poll_at` (liveness). Called on every `wait`;
    /// consumed by stale-counterpart detection in a later slice.
    pub async fn touch_last_poll(
        &self,
        handle: &str,
        now: OffsetDateTime,
    ) -> Result<(), StorageError> {
        let ts = now.format(&Rfc3339).map_err(fmt_err)?;
        sqlx::query("UPDATE participants SET last_poll_at = ? WHERE handle = ?")
            .bind(ts)
            .bind(handle)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Atomically claim the next message addressed to `handle` (or broadcast)
    /// that it has not yet read, advancing its cursor in the same step. Returns
    /// `None` when the caller is caught up. Safe under concurrent `wait` calls
    /// for the same handle: the cursor advance is a compare-and-swap, so a
    /// message is delivered to at most one claimant.
    pub async fn claim_next_unread(
        &self,
        room_id: &str,
        handle: &str,
    ) -> Result<Option<Message>, StorageError> {
        loop {
            // An absent participant has nothing to claim. Without this guard the
            // CAS below could never match a row, spinning the loop forever.
            let Some(cursor) = self.read_cursor(handle).await? else {
                return Ok(None);
            };
            let Some(m) = self.next_unread(room_id, handle, cursor).await? else {
                return Ok(None);
            };

            // Compare-and-swap the cursor from the value we read to this seq. If a
            // concurrent claim moved it first, we affect 0 rows and retry from the
            // new cursor — so the message is delivered to at most one claimant.
            let claimed = sqlx::query(
                "UPDATE participants SET last_read_seq = ? WHERE handle = ? AND last_read_seq = ?",
            )
            .bind(m.seq)
            .bind(handle)
            .bind(cursor)
            .execute(&self.pool)
            .await?
            .rows_affected()
                == 1;

            if claimed {
                return Ok(Some(m));
            }
        }
    }

    /// A participant's current long-poll read cursor, or `None` if the handle is
    /// not a participant of any room (distinguishing "absent" from "cursor 0").
    pub async fn read_cursor(&self, handle: &str) -> Result<Option<i64>, StorageError> {
        let cursor: Option<i64> =
            sqlx::query_scalar("SELECT last_read_seq FROM participants WHERE handle = ?")
                .bind(handle)
                .fetch_optional(&self.pool)
                .await?;
        Ok(cursor)
    }

    pub async fn get_participant_by_tuple(
        &self,
        room_id: &str,
        repo: &str,
        model: &str,
        cwd: &str,
    ) -> Result<Option<Participant>, StorageError> {
        let row = sqlx::query(
            "SELECT handle, room_id, repo, model, cwd, joined_at, last_poll_at, last_read_seq \
             FROM participants \
             WHERE room_id = ? AND repo = ? AND model = ? AND cwd = ?",
        )
        .bind(room_id)
        .bind(repo)
        .bind(model)
        .bind(cwd)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            None => Ok(None),
            Some(row) => Ok(Some(row_to_participant(&row)?)),
        }
    }

    pub async fn list_participants(&self, room_id: &str) -> Result<Vec<Participant>, StorageError> {
        let rows = sqlx::query(
            "SELECT handle, room_id, repo, model, cwd, joined_at, last_poll_at, last_read_seq \
             FROM participants WHERE room_id = ? ORDER BY joined_at",
        )
        .bind(room_id)
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_participant).collect()
    }
}

/// True for SQLite URLs that map to a private in-memory database.
fn is_memory_url(url: &str) -> bool {
    url.contains(":memory:") || url.contains("mode=memory")
}

fn fmt_err(e: time::error::Format) -> StorageError {
    StorageError::Corrupt(format!("timestamp format: {e}"))
}

fn parse_ts(s: &str) -> Result<OffsetDateTime, StorageError> {
    OffsetDateTime::parse(s, &Rfc3339)
        .map_err(|e| StorageError::Corrupt(format!("timestamp parse: {e}")))
}

fn parse_state(s: &str) -> Result<RoomState, StorageError> {
    Ok(match s {
        "active" => RoomState::Active,
        "idle" => RoomState::Idle,
        "paused" => RoomState::Paused,
        "stale" => RoomState::Stale,
        "closed" => RoomState::Closed,
        "archived" => RoomState::Archived,
        other => {
            return Err(StorageError::Corrupt(format!(
                "unknown room state: {other}"
            )))
        }
    })
}

fn row_to_participant(row: &sqlx::sqlite::SqliteRow) -> Result<Participant, StorageError> {
    Ok(Participant {
        handle: row.try_get("handle")?,
        room_id: row.try_get("room_id")?,
        repo: row.try_get("repo")?,
        model: row.try_get("model")?,
        cwd: row.try_get("cwd")?,
        joined_at: parse_ts(&row.try_get::<String, _>("joined_at")?)?,
        last_poll_at: parse_ts(&row.try_get::<String, _>("last_poll_at")?)?,
        last_read_seq: row.try_get("last_read_seq")?,
    })
}

fn row_to_message(row: &sqlx::sqlite::SqliteRow) -> Result<Message, StorageError> {
    let type_str: String = row.try_get("type")?;
    let msg_type = MessageType::parse(&type_str)
        .ok_or_else(|| StorageError::Corrupt(format!("unknown message type: {type_str}")))?;
    Ok(Message {
        seq: row.try_get("seq")?,
        room_id: row.try_get("room_id")?,
        sender: row.try_get("sender")?,
        recipient: row.try_get("recipient")?,
        body: row.try_get("body")?,
        created_at: parse_ts(&row.try_get::<String, _>("created_at")?)?,
        msg_type,
    })
}

fn row_to_room(row: &sqlx::sqlite::SqliteRow) -> Result<Room, StorageError> {
    let config_str: String = row.try_get("config")?;
    let config: RoomConfig = serde_json::from_str(&config_str)
        .map_err(|e| StorageError::Corrupt(format!("config json: {e}")))?;

    Ok(Room {
        id: row.try_get("id")?,
        subject: row.try_get("subject")?,
        started_at: parse_ts(&row.try_get::<String, _>("started_at")?)?,
        last_activity_at: parse_ts(&row.try_get::<String, _>("last_activity_at")?)?,
        state: parse_state(&row.try_get::<String, _>("state")?)?,
        config,
        prev_room_id: row.try_get("prev_room_id")?,
    })
}
