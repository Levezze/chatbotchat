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

        let pool = SqlitePoolOptions::new().connect_with(options).await?;
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
}

fn fmt_err(e: time::error::Format) -> StorageError {
    StorageError::Corrupt(format!("timestamp format: {e}"))
}

fn parse_ts(s: &str) -> Result<OffsetDateTime, StorageError> {
    OffsetDateTime::parse(s, &Rfc3339).map_err(|e| StorageError::Corrupt(format!("timestamp parse: {e}")))
}

fn parse_state(s: &str) -> Result<RoomState, StorageError> {
    Ok(match s {
        "active" => RoomState::Active,
        "idle" => RoomState::Idle,
        "paused" => RoomState::Paused,
        "stale" => RoomState::Stale,
        "closed" => RoomState::Closed,
        "archived" => RoomState::Archived,
        other => return Err(StorageError::Corrupt(format!("unknown room state: {other}"))),
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
