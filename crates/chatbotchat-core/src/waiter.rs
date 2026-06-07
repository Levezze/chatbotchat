//! Long-poll waiting. A [`Hub`] holds one broadcast channel per room; `send`
//! notifies it, and [`wait_for_message`] parks on it until a message addressed
//! to the caller arrives or the server-side cap elapses.

use crate::message::{Message, Severity};
use crate::storage::{Storage, StorageError};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use time::OffsetDateTime;
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
    timeout: Duration,
) -> Result<WaitOutcome, StorageError> {
    // No mid-park yield condition: park purely on the message channel and the
    // cap. Callers that must react to a room-state change while parked use
    // [`wait_for_message_until`] directly.
    wait_for_message_until(storage, hub, room_id, handle, timeout, || async {
        Ok(false)
    })
    .await
}

/// Like [`wait_for_message`], but on every *contentless* wake (a hub notify that
/// delivered no claimable message) it consults `should_yield`. When that returns
/// `true`, the park ends early with [`WaitOutcome::PausedByTimeout`] so the
/// caller can re-derive its response against fresh state — e.g. a room that
/// closed, or a close that was just proposed, while this waiter was parked.
/// Without it, a notify carrying no message re-parks to the full cap and the
/// state change is reported a whole cap late.
///
/// `should_yield` is consulted *after* the message claim (so a queued message is
/// still drained first) and only while park time remains (so the zero-cap drain
/// path never pays for it).
pub async fn wait_for_message_until<F, Fut>(
    storage: &Storage,
    hub: &Hub,
    room_id: &str,
    handle: &str,
    timeout: Duration,
    should_yield: F,
) -> Result<WaitOutcome, StorageError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<bool, StorageError>>,
{
    // Refresh liveness: this poll proves the participant is alive (consumed by
    // stale-counterpart detection in a later slice).
    storage
        .touch_last_poll(handle, OffsetDateTime::now_utc())
        .await?;

    // Subscribe BEFORE the first read: a message inserted (and notified) between
    // the read and the park is still observed on `recv`, so there is no lost
    // wakeup. The ping carries no payload — the DB is the source of truth and we
    // re-query on every wake.
    let mut rx = hub.subscribe(room_id);
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        if let Some(m) = storage.claim_next_unread(room_id, handle).await? {
            return Ok(WaitOutcome::Message(m));
        }

        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(WaitOutcome::PausedByTimeout);
        }

        // Park-relevant state may have changed under us (the room closed, or a
        // close was proposed) — the notify carrying that change wakes us with no
        // claimable message. Yield so the caller re-derives, rather than
        // re-parking to the full cap and reporting the change a cap late.
        if should_yield().await? {
            return Ok(WaitOutcome::PausedByTimeout);
        }

        match tokio::time::timeout(remaining, rx.recv()).await {
            // Woken, or we missed some pings (Lagged) — either way, re-query.
            Ok(Ok(())) | Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            // Sender dropped (Hub holds it, so this is defensive): one last read.
            Ok(Err(broadcast::error::RecvError::Closed)) => {
                return Ok(match storage.claim_next_unread(room_id, handle).await? {
                    Some(m) => WaitOutcome::Message(m),
                    None => WaitOutcome::PausedByTimeout,
                });
            }
            // Cap elapsed.
            Err(_elapsed) => return Ok(WaitOutcome::PausedByTimeout),
        }
    }
}

/// Server-side polling backoff for a counterpart parked behind an active
/// `waiting_user` sentinel (slice 5b). Returns the number of seconds the wait
/// should park, given the sentinel's `severity` and how long it has been active
/// (`elapsed_secs`, from the sentinel's `created_at` to now).
///
/// Per-state time-decay: a flat base for the first five minutes, then a
/// compounding 1.5× step per full minute past the 5:00 mark, capped at 60s.
/// `n = max(0, floor((elapsed − 300) / 60))`; `retry = min(60, round(base·1.5ⁿ))`.
/// Bases: `low=10`, `med=20`, `high=45`. `high` is effectively a single step
/// (45·1.5 > 60), and the re-signal that resets the clock measures decay from
/// the latest sentinel's `created_at`.
pub fn backoff_secs(severity: Severity, elapsed_secs: i64) -> u32 {
    let base: f64 = match severity {
        Severity::Low => 10.0,
        Severity::Med => 20.0,
        Severity::High => 45.0,
    };
    // Full minutes past the 5:00 mark, floored at 0 (no decay in the first 5 min).
    let n = (elapsed_secs - 300).max(0) / 60;
    let raw = base * 1.5f64.powi(n as i32);
    // `f64::round` is half-up for positives; the float→u32 cast saturates, so a
    // huge `n` lands on `u32::MAX` and the `.min(60)` cap still holds.
    (raw.round() as u32).min(60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bases_apply_before_the_five_minute_mark() {
        // n = 0 anywhere in [0, 300): each severity sits on its base.
        assert_eq!(backoff_secs(Severity::Low, 0), 10);
        assert_eq!(backoff_secs(Severity::Med, 0), 20);
        assert_eq!(backoff_secs(Severity::High, 0), 45);
        assert_eq!(backoff_secs(Severity::Low, 299), 10);
    }

    #[test]
    fn exactly_five_minutes_is_still_base() {
        // 300s ⇒ n = floor(0/60) = 0. The decay starts strictly after 5:00.
        assert_eq!(backoff_secs(Severity::Low, 300), 10);
        assert_eq!(backoff_secs(Severity::Med, 300), 20);
        assert_eq!(backoff_secs(Severity::High, 300), 45);
        // Still n = 0 at 359s (< one full minute past the mark).
        assert_eq!(backoff_secs(Severity::Low, 359), 10);
    }

    #[test]
    fn six_minutes_is_one_decay_step() {
        // 360s ⇒ n = 1, one compounding 1.5× step.
        assert_eq!(backoff_secs(Severity::Low, 360), 15); // 10 × 1.5
        assert_eq!(backoff_secs(Severity::Med, 360), 30); // 20 × 1.5
    }

    #[test]
    fn high_decays_in_a_single_step_to_the_cap() {
        // 45 × 1.5 = 67.5 → round 68 → capped at 60.
        assert_eq!(backoff_secs(Severity::High, 360), 60);
    }

    #[test]
    fn rounds_half_up() {
        // low, n = 2 (420s): 10 × 1.5² = 22.5 → 23.
        assert_eq!(backoff_secs(Severity::Low, 420), 23);
    }

    #[test]
    fn caps_at_sixty_for_long_pauses() {
        // low keeps compounding until it crosses 60 and pins there.
        assert_eq!(backoff_secs(Severity::Low, 600), 60); // 10 × 1.5⁵ = 75.9 → 76 → 60
        assert_eq!(backoff_secs(Severity::Med, 6_000), 60);
    }

    #[test]
    fn negative_elapsed_floors_to_base() {
        // A future-dated sentinel (clock skew) must not underflow the step count.
        assert_eq!(backoff_secs(Severity::High, -100), 45);
    }
}
