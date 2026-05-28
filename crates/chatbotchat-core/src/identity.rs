//! Participant handle derivation. Pure: randomness is injected as an iterator
//! of candidate session hexes so the logic is fully deterministic under test.
//!
//! A handle is `<repo-kebab>-<model-kebab>-<sess4hex>`. Note the handle is NOT
//! round-trip parseable — `repo` may itself contain `-` — so we never split a
//! handle back into its parts; the tuple lives in dedicated columns.

use crate::ids::kebab;
use crate::participant::Participant;
use std::collections::HashSet;

/// The identity a caller presents on join. The handle is derived from this
/// tuple, and rejoining with the same tuple is idempotent.
#[derive(Debug, Clone)]
pub struct JoinIdentity {
    pub repo: String,
    pub model: String,
    pub cwd: String,
}

/// Outcome of resolving a handle for a join. `Reused` means an existing
/// participant matched the tuple (no row should be inserted); `Created` means a
/// fresh handle was minted (the caller should insert it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandleOutcome {
    Reused(String),
    Created(String),
}

/// Resolve the handle for a join.
///
/// - If an existing participant matches `(repo, model, cwd)`, reuse its handle.
/// - Otherwise mint `<repo>-<model>-<sess>` from the first candidate sess that
///   does not collide with an existing handle in the room.
pub fn derive_handle(
    id: &JoinIdentity,
    existing: &[Participant],
    sess_candidates: impl Iterator<Item = String>,
) -> HandleOutcome {
    if let Some(p) = existing
        .iter()
        .find(|p| p.repo == id.repo && p.model == id.model && p.cwd == id.cwd)
    {
        return HandleOutcome::Reused(p.handle.clone());
    }

    let prefix = format!("{}-{}", kebab(&id.repo), kebab(&id.model));
    let taken: HashSet<&str> = existing.iter().map(|p| p.handle.as_str()).collect();

    // Take the first candidate whose handle is free in this room. The caller
    // feeds an effectively-infinite RNG stream, so a free one is found almost
    // immediately; `last` keeps the function total if the (finite, test) stream
    // is exhausted — the storage-layer UNIQUE retry is the real backstop.
    let mut last = None;
    for sess in sess_candidates {
        let candidate = format!("{prefix}-{sess}");
        if !taken.contains(candidate.as_str()) {
            return HandleOutcome::Created(candidate);
        }
        last = Some(candidate);
    }
    HandleOutcome::Created(last.unwrap_or(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn ident(repo: &str, model: &str, cwd: &str) -> JoinIdentity {
        JoinIdentity {
            repo: repo.into(),
            model: model.into(),
            cwd: cwd.into(),
        }
    }

    fn participant(handle: &str, repo: &str, model: &str, cwd: &str) -> Participant {
        let now = datetime!(2026-05-28 15:00 UTC);
        Participant {
            handle: handle.into(),
            room_id: "room-20260528-1500".into(),
            repo: repo.into(),
            model: model.into(),
            cwd: cwd.into(),
            joined_at: now,
            last_poll_at: now,
        }
    }

    #[test]
    fn mints_handle_from_first_candidate_when_room_empty() {
        let id = ident("mvp-engine", "opus47", "/work/mvp");
        let outcome = derive_handle(&id, &[], ["a3f2".to_string()].into_iter());
        assert_eq!(
            outcome,
            HandleOutcome::Created("mvp-engine-opus47-a3f2".into())
        );
    }

    #[test]
    fn reuses_existing_handle_for_same_tuple() {
        let id = ident("mvp-engine", "opus47", "/work/mvp");
        let existing = [participant(
            "mvp-engine-opus47-a3f2",
            "mvp-engine",
            "opus47",
            "/work/mvp",
        )];
        // A fresh candidate is offered, but the matching tuple must win — no new
        // handle, no candidate consumed.
        let outcome = derive_handle(&id, &existing, ["zzzz".to_string()].into_iter());
        assert_eq!(
            outcome,
            HandleOutcome::Reused("mvp-engine-opus47-a3f2".into())
        );
    }

    #[test]
    fn skips_candidate_that_collides_with_an_existing_handle() {
        // A different participant (different cwd ⇒ different tuple) already holds
        // the handle the first candidate would mint. Must skip to the next.
        let id = ident("mvp-engine", "opus47", "/work/mvp");
        let existing = [participant(
            "mvp-engine-opus47-a3f2",
            "mvp-engine",
            "opus47",
            "/other/cwd",
        )];
        let outcome = derive_handle(
            &id,
            &existing,
            ["a3f2".to_string(), "b7c1".to_string()].into_iter(),
        );
        assert_eq!(
            outcome,
            HandleOutcome::Created("mvp-engine-opus47-b7c1".into())
        );
    }
}
