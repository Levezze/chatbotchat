//! Long-poll waiting. A [`Hub`] holds one broadcast channel per room; `send`
//! notifies it, and [`wait_for_message`] parks on it until a message addressed
//! to the caller arrives or the server-side cap elapses.

use crate::message::Message;
use crate::storage::{Storage, StorageError};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::broadcast;

/// Per-room wakeup registry. Each room maps to a `broadcast` sender carrying a
/// content-free ping; waiters subscribe and re-query the DB (the source of
/// truth) on each ping, so a lagged or coalesced ping is harmless.
pub struct Hub {
    channels: Mutex<HashMap<String, broadcast::Sender<()>>>,
}

impl Hub {
    pub fn new() -> Self {
        Hub {
            channels: Mutex::new(HashMap::new()),
        }
    }

    /// Subscribe to a room's wakeups, creating the channel if absent.
    pub fn subscribe(&self, room_id: &str) -> broadcast::Receiver<()> {
        let mut channels = self.channels.lock().expect("hub mutex");
        channels
            .entry(room_id.to_string())
            .or_insert_with(|| broadcast::channel(16).0)
            .subscribe()
    }

    /// Wake every current waiter on a room. A send with no receivers is fine.
    pub fn notify(&self, room_id: &str) {
        let mut channels = self.channels.lock().expect("hub mutex");
        let sender = channels
            .entry(room_id.to_string())
            .or_insert_with(|| broadcast::channel(16).0);
        let _ = sender.send(());
    }
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

/// Outcome of a long-poll.
#[derive(Debug)]
pub enum WaitOutcome {
    /// A message addressed to the caller (or broadcast) was delivered.
    Message(Message),
    /// The server-side cap elapsed before any matching message arrived.
    PausedByTimeout,
}

/// Long-poll for the next message addressed to `handle` (or broadcast) in
/// `room_id` with `seq > last_read_seq`. Returns immediately if one already
/// exists; otherwise parks on the room's wakeup channel until a matching
/// message arrives or `timeout` elapses (`PausedByTimeout`).
pub async fn wait_for_message(
    storage: &Storage,
    hub: &Hub,
    room_id: &str,
    handle: &str,
    last_read_seq: i64,
    timeout: Duration,
) -> Result<WaitOutcome, StorageError> {
    // Subscribe BEFORE the first read: a message inserted (and notified) between
    // the read and the park is still observed on `recv`, so there is no lost
    // wakeup. The ping carries no payload — the DB is the source of truth and we
    // re-query on every wake.
    let mut rx = hub.subscribe(room_id);
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        if let Some(m) = storage.next_unread(room_id, handle, last_read_seq).await? {
            storage.advance_read_cursor(handle, m.seq).await?;
            return Ok(WaitOutcome::Message(m));
        }

        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(WaitOutcome::PausedByTimeout);
        }

        match tokio::time::timeout(remaining, rx.recv()).await {
            // Woken, or we missed some pings (Lagged) — either way, re-query.
            Ok(Ok(())) | Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            // Sender dropped (Hub holds it, so this is defensive): one last read.
            Ok(Err(broadcast::error::RecvError::Closed)) => {
                return match storage.next_unread(room_id, handle, last_read_seq).await? {
                    Some(m) => {
                        storage.advance_read_cursor(handle, m.seq).await?;
                        Ok(WaitOutcome::Message(m))
                    }
                    None => Ok(WaitOutcome::PausedByTimeout),
                };
            }
            // Cap elapsed.
            Err(_elapsed) => return Ok(WaitOutcome::PausedByTimeout),
        }
    }
}
