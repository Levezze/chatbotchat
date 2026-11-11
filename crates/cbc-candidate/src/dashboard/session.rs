//! Dashboard sign-in (T-12). A login code and a browser session live only in this process's
//! memory, and only as SHA-256 digests: nothing here is written to SQLite, a file or a log, so a
//! restart signs every browser out.
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// A code is for the browser the user's own command opens, so it only has to last that long.
pub const CODE_TTL: Duration = Duration::from_secs(30);
/// Proposed default, pending the user's review: one working day.
pub const SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);
const MAX_CODES: usize = 8;
const MAX_SESSIONS: usize = 16;

type Digests = HashMap<[u8; 32], Instant>;

pub struct Sessions {
    inner: Mutex<(Digests, Digests)>,
    code_ttl: Duration,
    session_ttl: Duration,
}

impl Sessions {
    pub fn new() -> Arc<Self> {
        Self::with_ttls(CODE_TTL, SESSION_TTL)
    }

    pub fn with_ttls(code_ttl: Duration, session_ttl: Duration) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new((HashMap::new(), HashMap::new())),
            code_ttl,
            session_ttl,
        })
    }

    /// A single-use 128-bit code, as 32 hex characters.
    pub fn mint_code(&self) -> String {
        let code = secret::<16>();
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        insert(&mut inner.0, digest(&code), self.code_ttl, MAX_CODES);
        code
    }

    /// Spend a code for a 256-bit session token. A code works once, and only before it expires.
    pub fn redeem(&self, code: &str) -> Option<String> {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let deadline = inner.0.remove(&digest(code))?;
        if Instant::now() >= deadline {
            return None;
        }
        let session = secret::<32>();
        insert(
            &mut inner.1,
            digest(&session),
            self.session_ttl,
            MAX_SESSIONS,
        );
        Some(session)
    }

    pub fn valid(&self, session: &str) -> bool {
        let inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner
            .1
            .get(&digest(session))
            .is_some_and(|deadline| Instant::now() < *deadline)
    }
}

/// Drop what has expired; when still full, the entry nearest expiry makes room.
fn insert(map: &mut Digests, key: [u8; 32], ttl: Duration, cap: usize) {
    let now = Instant::now();
    map.retain(|_, deadline| now < *deadline);
    if map.len() >= cap {
        if let Some(oldest) = map.iter().min_by_key(|(_, d)| **d).map(|(k, _)| *k) {
            map.remove(&oldest);
        }
    }
    map.insert(key, now + ttl);
}

/// A fresh key for one rendered answer form, so a resubmit of that form is a retry.
pub fn form_key() -> String {
    secret::<16>()
}

fn secret<const N: usize>() -> String {
    let mut bytes = [0_u8; N];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_expired_code_or_session_is_refused() {
        let sessions = Sessions::with_ttls(Duration::from_millis(20), Duration::from_millis(20));
        let late = sessions.mint_code();
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(sessions.redeem(&late), None, "an expired code");
        let session = sessions.redeem(&sessions.mint_code()).unwrap();
        assert!(sessions.valid(&session));
        std::thread::sleep(Duration::from_millis(30));
        assert!(!sessions.valid(&session), "an expired session");
    }

    #[test]
    fn a_code_is_spent_once_and_is_not_a_session() {
        let sessions = Sessions::new();
        let code = sessions.mint_code();
        assert!(!sessions.valid(&code));
        let session = sessions.redeem(&code).unwrap();
        assert_eq!(sessions.redeem(&code), None);
        assert!(sessions.valid(&session));
        assert!(!sessions.valid(""));
    }

    #[test]
    fn outstanding_codes_and_sessions_are_bounded() {
        let sessions = Sessions::new();
        let first = sessions.mint_code();
        for _ in 0..MAX_CODES {
            sessions.mint_code();
        }
        assert_eq!(sessions.redeem(&first), None, "the oldest code made room");
        let first = sessions.redeem(&sessions.mint_code()).unwrap();
        for _ in 0..MAX_SESSIONS {
            sessions.redeem(&sessions.mint_code()).unwrap();
        }
        assert!(!sessions.valid(&first), "the oldest session made room");
        let inner = sessions.inner.lock().unwrap();
        assert!(inner.0.len() <= MAX_CODES && inner.1.len() <= MAX_SESSIONS);
    }
}
