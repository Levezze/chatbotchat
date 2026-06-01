use crate::event::{Event, EventKind};
use crate::message::{Message, MessageType, Severity};
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

/// One row of the room list: the room plus its live participant count. The count
/// is computed in the list query (a correlated subquery), not by an N+1 walk.
#[derive(Debug, Clone)]
pub struct RoomSummaryRow {
    pub room: Room,
    pub participant_count: i64,
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
        let state_changed_at = room.state_changed_at.format(&Rfc3339).map_err(fmt_err)?;
        let config = serde_json::to_string(&room.config)
            .map_err(|e| StorageError::Corrupt(e.to_string()))?;

        sqlx::query(
            "INSERT INTO rooms \
             (id, subject, started_at, last_activity_at, state, state_changed_at, config, prev_room_id) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&room.id)
        .bind(&room.subject)
        .bind(started_at)
        .bind(last_activity_at)
        .bind(room.state.as_str())
        .bind(state_changed_at)
        .bind(config)
        .bind(&room.prev_room_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_room(&self, id: &str) -> Result<Option<Room>, StorageError> {
        let row = sqlx::query(
            "SELECT id, subject, started_at, last_activity_at, state, state_changed_at, config, prev_room_id \
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
            // Bare `create_message` is the autonomous-turn path; the human-fed
            // path is `create_message_capped`'s `from_human` arg. DB default is 0.
            from_human: false,
            // A plain `msg` carries no sentinel metadata.
            severity: None,
            question_text: None,
        })
    }

    /// Append a message of an arbitrary `msg_type` (uncapped). Sentinels are
    /// written through here; the bare `create_message` is the `msg`-only path.
    /// Cap enforcement lives in `create_message_capped`, so this never gates.
    /// `severity` and `question_text` carry the `waiting_user` sentinel's payload
    /// (the question the agent is asking its user); both are `None` for `fold`.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_message_typed(
        &self,
        room_id: &str,
        sender: &str,
        recipient: Option<&str>,
        body: &str,
        created_at: OffsetDateTime,
        msg_type: MessageType,
        severity: Option<Severity>,
        question_text: Option<&str>,
    ) -> Result<Message, StorageError> {
        let created = created_at.format(&Rfc3339).map_err(fmt_err)?;

        let result = sqlx::query(
            "INSERT INTO messages \
             (room_id, sender, recipient, body, created_at, type, severity, question_text) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(room_id)
        .bind(sender)
        .bind(recipient)
        .bind(body)
        .bind(created)
        .bind(msg_type.as_str())
        .bind(severity.map(Severity::as_str))
        .bind(question_text)
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
            // Sentinels are agent-originated signals; DB default is 0.
            from_human: false,
            severity,
            question_text: question_text.map(str::to_string),
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
    #[allow(clippy::too_many_arguments)]
    pub async fn create_message_capped(
        &self,
        room_id: &str,
        sender: &str,
        recipient: Option<&str>,
        body: &str,
        created_at: OffsetDateTime,
        from_human: bool,
        hard_cap: i64,
    ) -> Result<Option<Message>, StorageError> {
        let created = created_at.format(&Rfc3339).map_err(fmt_err)?;

        let result = sqlx::query(
            "INSERT INTO messages (room_id, sender, recipient, body, created_at, type, from_human) \
             SELECT ?, ?, ?, ?, ?, 'msg', ? \
             WHERE (SELECT COUNT(*) FROM messages WHERE room_id = ? AND type = 'msg') < ?",
        )
        .bind(room_id)
        .bind(sender)
        .bind(recipient)
        .bind(body)
        .bind(created)
        .bind(from_human)
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
            from_human,
            // The capped path is `msg`-only; sentinel metadata never applies.
            severity: None,
            question_text: None,
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
            "SELECT seq, room_id, sender, recipient, body, created_at, type, from_human, \
                    severity, question_text \
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

    /// The single most-recent message in `room_id` sent by anyone *other than*
    /// `handle` — cursor-independent, regardless of read state. Drives slice 5b's
    /// polling backoff: a `waiting_user` here means the counterpart is paused, and
    /// its `severity`/`created_at` feed [`crate::waiter::backoff_secs`].
    ///
    /// Returns the latest row of *any* type on purpose: a later plain `msg` from
    /// the same sender self-supersedes an earlier sentinel, so the caller clears
    /// the backoff by checking `msg_type == WaitingUser`. Filtering to
    /// `type = 'waiting_user'` in SQL would miss that clearing and back off
    /// forever. Correct for a single counterpart (v1 two-agent) only — a third
    /// party's later `msg` would mask another's active sentinel.
    pub async fn latest_message_from_other(
        &self,
        room_id: &str,
        handle: &str,
    ) -> Result<Option<Message>, StorageError> {
        let row = sqlx::query(
            "SELECT seq, room_id, sender, recipient, body, created_at, type, from_human, \
                    severity, question_text \
             FROM messages \
             WHERE room_id = ? AND sender != ? \
             ORDER BY seq DESC LIMIT 1",
        )
        .bind(room_id)
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

    /// The soft-cap counter at the delivery of message `up_to_seq`: the number of
    /// consecutive autonomous (`from_human = 0`) `msg` rows since the last human
    /// input, counting up to and including `up_to_seq`. Advisory and read-only —
    /// rows are immutable and `seq` monotonic, so the count at delivery equals the
    /// count at send (no race, no atomicity needed). The wait handler compares
    /// this to `soft_cap - 1` to decide whether to surface to the user.
    ///
    /// The reset boundary is the highest row at or before `up_to_seq` that breaks
    /// the autonomous run (seq 0 if none): either a `from_human = 1` `msg` (the
    /// `--human` fold) OR a `waiting_user` sentinel (consulting the user also
    /// pulls a human into the loop). The count itself stays `type = 'msg' AND
    /// from_human = 0` — a sentinel resets the run but is never itself counted as
    /// a turn. This asymmetry (in the boundary, out of the count) keeps the cap
    /// seams in lockstep while letting a consultation break the run.
    pub async fn consecutive_msg_count(
        &self,
        room_id: &str,
        up_to_seq: i64,
    ) -> Result<i64, StorageError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM messages \
             WHERE room_id = ? AND type = 'msg' AND from_human = 0 AND seq <= ? \
               AND seq > (SELECT COALESCE(MAX(seq), 0) FROM messages \
                          WHERE room_id = ? AND seq <= ? \
                            AND ((type = 'msg' AND from_human = 1) OR type = 'waiting_user'))",
        )
        .bind(room_id)
        .bind(up_to_seq)
        .bind(room_id)
        .bind(up_to_seq)
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
            "SELECT seq, room_id, sender, recipient, body, created_at, type, from_human, \
                    severity, question_text \
             FROM messages WHERE room_id = ? ORDER BY seq DESC LIMIT ?",
        )
        .bind(room_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        // Fetched newest-first for the LIMIT; hand back chronological.
        rows.iter().rev().map(row_to_message).collect()
    }

    /// Every message in a room, oldest-first — the full transcript backing
    /// `cbc show`. Unlike `recent_messages` (a recency window via `DESC LIMIT`),
    /// this returns the complete log ordered by `seq`.
    pub async fn all_messages(&self, room_id: &str) -> Result<Vec<Message>, StorageError> {
        let rows = sqlx::query(
            "SELECT seq, room_id, sender, recipient, body, created_at, type, from_human, \
                    severity, question_text \
             FROM messages WHERE room_id = ? ORDER BY seq",
        )
        .bind(room_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_message).collect()
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

    /// Conditionally transition a room's state. The `UPDATE` is gated on the
    /// room *currently* being in `from` (CAS-style, like `claim_next_unread`), so
    /// a concurrent explicit transition (wake/close) cannot be clobbered by a
    /// stale sweeper read: if the precondition no longer holds, zero rows change
    /// and this returns `Ok(false)` having written nothing. On success it
    /// re-anchors `state_changed_at` to `now` and appends one `events` row —
    /// `archive`-kind when transitioning into `archived` (the `on_archive` hook),
    /// `transition`-kind otherwise. `detail` is free text (e.g. a pause reason).
    pub async fn update_room_state(
        &self,
        room_id: &str,
        from: RoomState,
        to: RoomState,
        now: OffsetDateTime,
        detail: Option<&str>,
    ) -> Result<bool, StorageError> {
        let ts = now.format(&Rfc3339).map_err(fmt_err)?;

        // The CAS update and its audit row are one transaction: if the event
        // write fails, the state change rolls back, so a room never transitions
        // without its `events` row (the on_archive / audit invariant — AC #8).
        // The event insert runs on the transaction connection via
        // `insert_event_row`, NOT `self.insert_event`: the latter borrows
        // `self.pool` and would acquire a *second* connection, deadlocking the
        // single-connection in-memory pool used by tests.
        let mut tx = self.pool.begin().await?;

        let result = sqlx::query(
            "UPDATE rooms SET state = ?, state_changed_at = ? WHERE id = ? AND state = ?",
        )
        .bind(to.as_str())
        .bind(&ts)
        .bind(room_id)
        .bind(from.as_str())
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() == 0 {
            // Precondition no longer holds: nothing changed, nothing logged.
            tx.rollback().await?;
            return Ok(false);
        }

        let kind = if to == RoomState::Archived {
            EventKind::Archive
        } else {
            EventKind::Transition
        };
        insert_event_row(&mut *tx, room_id, kind, Some(from), Some(to), detail, &ts).await?;

        tx.commit().await?;
        Ok(true)
    }

    /// Append a row to the `events` log. Used by `update_room_state`; also
    /// callable directly. The log is append-only — never updated or deleted.
    pub async fn insert_event(
        &self,
        room_id: &str,
        kind: EventKind,
        from: Option<RoomState>,
        to: Option<RoomState>,
        detail: Option<&str>,
        at: OffsetDateTime,
    ) -> Result<(), StorageError> {
        let at = at.format(&Rfc3339).map_err(fmt_err)?;
        insert_event_row(&self.pool, room_id, kind, from, to, detail, &at).await
    }

    /// All `events` for a room, oldest first.
    pub async fn list_events(&self, room_id: &str) -> Result<Vec<Event>, StorageError> {
        let rows = sqlx::query(
            "SELECT id, room_id, kind, from_state, to_state, detail, at \
             FROM events WHERE room_id = ? ORDER BY id",
        )
        .bind(room_id)
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_event).collect()
    }

    /// Mark a room as active *now* by bumping `last_activity_at`. Drives the
    /// sweeper's idle/stale timing; does not touch `state`.
    pub async fn touch_last_activity(
        &self,
        room_id: &str,
        now: OffsetDateTime,
    ) -> Result<(), StorageError> {
        let now = now.format(&Rfc3339).map_err(fmt_err)?;
        sqlx::query("UPDATE rooms SET last_activity_at = ? WHERE id = ?")
            .bind(now)
            .bind(room_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Every non-terminal room, for the hourly sweep. `archived` is terminal and
    /// excluded so the sweep cost stays proportional to live rooms.
    pub async fn list_rooms_for_sweep(&self) -> Result<Vec<Room>, StorageError> {
        let rows = sqlx::query(
            "SELECT id, subject, started_at, last_activity_at, state, state_changed_at, config, prev_room_id \
             FROM rooms WHERE state != 'archived'",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_room).collect()
    }

    /// Rooms for the browse surface (`cbc list`), newest-first by activity, each
    /// with its live participant count. Filter precedence: an explicit `state`
    /// wins (`--state X`); else `all` includes every room (`--all`); else the
    /// default hides terminal `archived` rooms. The participant count is a
    /// correlated subquery so this stays one round-trip regardless of room count.
    pub async fn list_rooms(
        &self,
        state: Option<RoomState>,
        all: bool,
    ) -> Result<Vec<RoomSummaryRow>, StorageError> {
        const SELECT: &str = "SELECT r.id, r.subject, r.started_at, r.last_activity_at, r.state, \
                    r.state_changed_at, r.config, r.prev_room_id, \
                    (SELECT COUNT(*) FROM participants p WHERE p.room_id = r.id) AS participant_count \
             FROM rooms r";
        const ORDER: &str = "ORDER BY r.last_activity_at DESC";

        let rows = match (state, all) {
            (Some(s), _) => {
                sqlx::query(&format!("{SELECT} WHERE r.state = ? {ORDER}"))
                    .bind(s.as_str())
                    .fetch_all(&self.pool)
                    .await?
            }
            (None, true) => {
                sqlx::query(&format!("{SELECT} {ORDER}"))
                    .fetch_all(&self.pool)
                    .await?
            }
            (None, false) => {
                sqlx::query(&format!("{SELECT} WHERE r.state != 'archived' {ORDER}"))
                    .fetch_all(&self.pool)
                    .await?
            }
        };

        rows.iter()
            .map(|row| {
                Ok(RoomSummaryRow {
                    room: row_to_room(row)?,
                    participant_count: row.try_get("participant_count")?,
                })
            })
            .collect()
    }
}

/// Insert one `events` row using any sqlx executor — the pool (direct
/// `insert_event`) or a transaction connection (`update_room_state`, which needs
/// the row written inside its tx). `at` is pre-formatted RFC3339.
async fn insert_event_row<'e, E>(
    executor: E,
    room_id: &str,
    kind: EventKind,
    from: Option<RoomState>,
    to: Option<RoomState>,
    detail: Option<&str>,
    at: &str,
) -> Result<(), StorageError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query(
        "INSERT INTO events (room_id, kind, from_state, to_state, detail, at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(room_id)
    .bind(kind.as_str())
    .bind(from.map(|s| s.as_str()))
    .bind(to.map(|s| s.as_str()))
    .bind(detail)
    .bind(at)
    .execute(executor)
    .await?;
    Ok(())
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
    let severity = match row.try_get::<Option<String>, _>("severity")? {
        Some(s) => Some(
            Severity::parse(&s)
                .ok_or_else(|| StorageError::Corrupt(format!("unknown severity: {s}")))?,
        ),
        None => None,
    };
    Ok(Message {
        seq: row.try_get("seq")?,
        room_id: row.try_get("room_id")?,
        sender: row.try_get("sender")?,
        recipient: row.try_get("recipient")?,
        body: row.try_get("body")?,
        created_at: parse_ts(&row.try_get::<String, _>("created_at")?)?,
        msg_type,
        from_human: row.try_get("from_human")?,
        severity,
        question_text: row.try_get("question_text")?,
    })
}

fn parse_state_opt(s: Option<String>) -> Result<Option<RoomState>, StorageError> {
    match s {
        Some(s) => Ok(Some(parse_state(&s)?)),
        None => Ok(None),
    }
}

fn row_to_event(row: &sqlx::sqlite::SqliteRow) -> Result<Event, StorageError> {
    let kind_str: String = row.try_get("kind")?;
    let kind = EventKind::parse(&kind_str)
        .ok_or_else(|| StorageError::Corrupt(format!("unknown event kind: {kind_str}")))?;
    Ok(Event {
        id: row.try_get("id")?,
        room_id: row.try_get("room_id")?,
        kind,
        from_state: parse_state_opt(row.try_get("from_state")?)?,
        to_state: parse_state_opt(row.try_get("to_state")?)?,
        detail: row.try_get("detail")?,
        at: parse_ts(&row.try_get::<String, _>("at")?)?,
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
        state_changed_at: parse_ts(&row.try_get::<String, _>("state_changed_at")?)?,
        config,
        prev_room_id: row.try_get("prev_room_id")?,
    })
}

#[cfg(test)]
mod atomicity_tests {
    use super::*;
    use crate::room::{Room, RoomConfig};

    async fn active_room(storage: &Storage, id: &str) {
        let now = OffsetDateTime::now_utc();
        let room = Room {
            id: id.into(),
            subject: "atomicity".into(),
            started_at: now,
            last_activity_at: now,
            state: RoomState::Active,
            state_changed_at: now,
            config: RoomConfig::default(),
            prev_room_id: None,
        };
        storage.create_room(&room).await.expect("create room");
    }

    // A state transition and its audit-log row must be atomic. If the events
    // insert fails, the room state must NOT change — otherwise the room
    // transitions with no audit/on_archive row, violating the invariant that
    // every transition writes exactly one event (AC #8). We force the event
    // write to fail by dropping the `events` table out from under the call,
    // then assert the room state was rolled back. This is an in-module test
    // because the fault injection needs the private pool; the assertion itself
    // is pure external behavior (observable room state).
    #[tokio::test]
    async fn update_room_state_rolls_back_when_event_write_fails() {
        let storage = Storage::connect("sqlite::memory:").await.expect("connect");
        let id = "atom-1-20260531-0000";
        active_room(&storage, id).await;

        sqlx::query("DROP TABLE events")
            .execute(&storage.pool)
            .await
            .expect("drop events to induce an insert failure");

        let now = OffsetDateTime::now_utc();
        let result = storage
            .update_room_state(id, RoomState::Active, RoomState::Closed, now, None)
            .await;
        assert!(
            result.is_err(),
            "the event write failed, so the call must surface an error"
        );

        let room = storage
            .get_room(id)
            .await
            .expect("get ok")
            .expect("room exists");
        assert_eq!(
            room.state,
            RoomState::Active,
            "state must roll back when the audit row could not be written"
        );
    }
}
