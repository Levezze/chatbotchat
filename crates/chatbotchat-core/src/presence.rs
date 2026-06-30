//! In-memory poll-presence registry — the *truthful* liveness signal.
//!
//! The durable `Participant.last_poll_at` timestamp is stamped at `wait`-request
//! *arrival* and is never invalidated when the long-poll connection drops, so a
//! dead client keeps reading "fresh" for up to the 15-minute ghost window. That
//! lie is what makes a worker believe its reaped `cbc poll` is still alive and
//! end the turn deaf.
//!
//! This registry tracks *currently-parked* long-poll connections instead. A
//! [`ParkGuard`] is held for the lifetime of one park and dropped on EVERY exit
//! — normal message return, timeout, AND the framework cancelling the handler
//! future when the client disconnects — so [`Presence::is_live`] reflects a real
//! connection, not a stored timestamp. Single daemon process ⇒ this in-memory
//! map is authoritative.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long after a park ends a participant still counts as live. A healthy
/// client re-polls within a few seconds (a server-cap chunk plus the client's
/// `MIN_REPOLL_SLEEP_SECS`), so this grace smooths the gap between consecutive
/// parks without masking a real death longer than this. ~90× tighter than the
/// 15-minute `GHOST_AFTER` window the timestamp path uses.
pub const DEFAULT_GRACE: Duration = Duration::from_secs(10);

#[derive(Debug)]
struct ParkState {
    /// Long-poll connections currently parked for this handle.
    active: usize,
    /// When the most recent park was released. `None` before the first release.
    last_release: Option<Instant>,
}

/// Tracks which participant handles have a live long-poll parked right now.
pub struct Presence {
    inner: Mutex<HashMap<String, ParkState>>,
    grace: Duration,
}

impl Presence {
    pub fn new() -> Self {
        Self::with_grace(DEFAULT_GRACE)
    }

    /// Construct with an explicit grace window (tests exercise the expiry
    /// boundary without sleeping for the full default).
    pub fn with_grace(grace: Duration) -> Self {
        Presence {
            inner: Mutex::new(HashMap::new()),
            grace,
        }
    }

    /// Register a parked long-poll for `handle`. The returned guard MUST be held
    /// for the duration of the park; dropping it (return, timeout, or the
    /// handler future being cancelled on disconnect) records the release.
    pub fn enter(self: &Arc<Self>, handle: &str) -> ParkGuard {
        let mut map = self.inner.lock().expect("presence mutex");
        let st = map.entry(handle.to_string()).or_insert(ParkState {
            active: 0,
            last_release: None,
        });
        st.active += 1;
        ParkGuard {
            presence: Arc::clone(self),
            handle: handle.to_string(),
        }
    }

    /// Is there a live parked poll for `handle` as of `now`? True while a park is
    /// active, or within the grace window after the most recent release. Past
    /// grace with no active park, the entry is forgotten (lazy GC) and the
    /// handle reads dead.
    pub fn is_live(&self, handle: &str, now: Instant) -> bool {
        let mut map = self.inner.lock().expect("presence mutex");
        match map.get(handle) {
            None => false,
            Some(st) if st.active > 0 => true,
            Some(st) => {
                let within_grace = st
                    .last_release
                    .is_some_and(|t| now.saturating_duration_since(t) <= self.grace);
                if !within_grace {
                    map.remove(handle);
                }
                within_grace
            }
        }
    }

    fn release(&self, handle: &str) {
        let mut map = self.inner.lock().expect("presence mutex");
        if let Some(st) = map.get_mut(handle) {
            st.active = st.active.saturating_sub(1);
            st.last_release = Some(Instant::now());
        }
    }
}

impl Default for Presence {
    fn default() -> Self {
        Self::new()
    }
}

/// Held for the lifetime of a parked long-poll; on drop it records the release
/// so liveness reflects connection teardown (including framework cancellation
/// of the handler future on client disconnect).
pub struct ParkGuard {
    presence: Arc<Presence>,
    handle: String,
}

impl Drop for ParkGuard {
    fn drop(&mut self) {
        self.presence.release(&self.handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_park_reads_live() {
        let p = Arc::new(Presence::new());
        let _g = p.enter("h1");
        assert!(p.is_live("h1", Instant::now()));
    }

    #[test]
    fn unknown_handle_reads_dead() {
        let p = Arc::new(Presence::new());
        assert!(!p.is_live("nobody", Instant::now()));
    }

    #[test]
    fn released_park_stays_live_within_grace() {
        let p = Arc::new(Presence::with_grace(Duration::from_secs(10)));
        let t0 = Instant::now();
        {
            let _g = p.enter("h1"); // released here; last_release ≈ t0
        }
        // Half a grace window after the release: a healthy re-poller's gap.
        assert!(p.is_live("h1", t0 + Duration::from_secs(5)));
    }

    #[test]
    fn released_park_dies_past_grace() {
        let p = Arc::new(Presence::with_grace(Duration::from_secs(10)));
        let t0 = Instant::now();
        {
            let _g = p.enter("h1");
        }
        // Well past grace: a reaped poll no longer reads live.
        assert!(!p.is_live("h1", t0 + Duration::from_secs(11)));
    }

    #[test]
    fn overlapping_parks_keep_live_until_last_release() {
        // Tiny grace so the dead assertion doesn't depend on the default window.
        let p = Arc::new(Presence::with_grace(Duration::from_millis(1)));
        let g1 = p.enter("h1");
        let g2 = p.enter("h1");
        drop(g1);
        // One park still active ⇒ live regardless of how far `now` is pushed.
        assert!(p.is_live("h1", Instant::now() + Duration::from_secs(60)));
        drop(g2);
        // Both released, past the 1 ms grace ⇒ dead.
        assert!(!p.is_live("h1", Instant::now() + Duration::from_secs(60)));
    }

    #[test]
    fn past_grace_entry_is_garbage_collected() {
        let p = Arc::new(Presence::with_grace(Duration::from_secs(10)));
        let t0 = Instant::now();
        {
            let _g = p.enter("h1");
        }
        assert!(!p.is_live("h1", t0 + Duration::from_secs(11)));
        assert!(
            p.inner.lock().unwrap().is_empty(),
            "a dead handle past grace should be forgotten"
        );
    }
}
