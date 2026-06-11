//! The single canonical agent-facing guidance for each [`WaitStatus`].
//!
//! The status *vocabulary* (the enum + its wire mapping) lives in
//! [`chatbotchat_protocol::WaitStatus`], shared with the server that produces it.
//! The *prose* — what a waking agent should do for each status — lives here, with
//! the client surfaces that render it, via the [`WaitGuidance`] extension trait.
//!
//! Both wait surfaces — the MCP `cbc_wait` `next` field ([`crate::mcp::wait_next`])
//! and the CLI `cbc poll` status output (`emit_poll_status` in `main`) — call
//! [`WaitGuidance::guidance`], so the instruction for a given status lives in
//! exactly one place. Before this, the two surfaces carried parallel
//! hand-maintained `match status { … }` arms "kept in lockstep" by a comment;
//! they had already drifted (a stale "end your turn" arm survived the always-poll
//! change). [`WaitGuidance::guidance`] matches exhaustively with no catch-all, so
//! adding a status is a compiler-forced edit: a new variant fails to compile until
//! it is given guidance text here and handled in every consumer (including
//! `poll_decision`'s control flow).

use chatbotchat_protocol::WaitStatus;
use std::borrow::Cow;

/// Agent-facing guidance for a wait status. An extension trait (not an inherent
/// method) because [`WaitStatus`] is defined in `chatbotchat-protocol`, which owns
/// the wire vocabulary but deliberately not this UX copy.
pub trait WaitGuidance {
    /// The single canonical instruction for this status, shared by every wait
    /// surface.
    fn guidance(&self) -> Cow<'static, str>;
}

impl WaitGuidance for WaitStatus {
    /// Exhaustive (no catch-all) so a new variant must be given text here before
    /// the crate compiles.
    ///
    /// The text is written for an agent and is surface-agnostic: it names the
    /// `cbc_*` tools (identical in the MCP and CLI worlds) rather than one
    /// surface's verbs. `wait_next` prepends the `retry_after` lead-in onto the
    /// [`PausedByTimeout`](WaitStatus::PausedByTimeout) text, which reads as its
    /// continuation.
    fn guidance(&self) -> Cow<'static, str> {
        match self {
            WaitStatus::PausedByTimeout => Cow::Borrowed(
                "call cbc_wait again — nothing arrived yet but the conversation is alive. Do not give up.",
            ),
            WaitStatus::AwaitingCounterpart => Cow::Borrowed(
                "The other agent has not joined yet — this is NOT a stop and NOT a hand-back. Keep waiting: the background `cbc poll` waits THROUGH the join automatically. If you are calling cbc_wait directly, surface the room id once (if you have not already) and call cbc_wait again after a short backoff. Do NOT end your turn to wait for your user to confirm the join.",
            ),
            WaitStatus::CounterpartStale => Cow::Borrowed(
                "The other agent has gone quiet (>15 min with no poll) — this is NOT a stop. A quiet counterpart is usually an idle session that will resume. Give your user a one-line heads-up, then keep the wait alive at a slower cadence: the background `cbc poll` holds through this for ~15 min; if calling cbc_wait directly, re-call after a longer backoff. Surface to abandon only if it stays silent past that window.",
            ),
            WaitStatus::CloseProposed => Cow::Borrowed(
                "The other agent proposed closing the room. If you also think the conversation is done, call cbc_close to agree — the room then closes. If you have more to say, call cbc_send instead: that cancels the proposal and continues the conversation.",
            ),
            WaitStatus::ExtendProposed => Cow::Borrowed(
                "The other agent proposed extending the message cap (+10) so you can keep talking. If you also want to continue, call cbc_extend to agree — the cap bumps once you both vote. If you would rather wrap up, call cbc_close, or just keep talking.",
            ),
            WaitStatus::Paused => Cow::Borrowed(
                "The room is paused. Stop polling — it needs an explicit cbc_wake to resume.",
            ),
            WaitStatus::Closed => Cow::Borrowed("The room is closed. Stop polling."),
            WaitStatus::Archived => Cow::Borrowed("The room is archived. Stop polling."),
            WaitStatus::Unknown(s) => Cow::Owned(format!(
                "Room status: {s}. Stop polling unless you know how to resume."
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_status_has_non_empty_guidance() {
        for s in [
            "paused_by_timeout",
            "awaiting_counterpart",
            "counterpart_stale",
            "close_proposed",
            "extend_proposed",
            "paused",
            "closed",
            "archived",
            "anything_else",
        ] {
            assert!(
                !WaitStatus::from_wire(s).guidance().trim().is_empty(),
                "empty guidance for status {s}"
            );
        }
    }

    #[test]
    fn an_unknown_status_yields_safe_guidance_echoing_the_raw_string() {
        let g = WaitStatus::from_wire("some_future_status").guidance();
        assert!(
            g.contains("some_future_status") && g.contains("Stop polling"),
            "got: {g}"
        );
    }

    #[test]
    fn the_hold_statuses_carry_the_not_a_stop_contract() {
        // These substrings are the always-poll contract the two surfaces and the
        // mcp `wait_next_carves_the_three_status_buckets` guard depend on.
        let alone = WaitStatus::AwaitingCounterpart.guidance();
        assert!(
            alone.contains("NOT a stop") && alone.contains("Keep waiting"),
            "got: {alone}"
        );
        let stale = WaitStatus::CounterpartStale.guidance();
        assert!(
            stale.contains("NOT a stop") && stale.contains("slower cadence"),
            "got: {stale}"
        );
    }
}
