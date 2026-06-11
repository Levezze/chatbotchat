//! The closed set of [`WaitResponse::Timeout`] statuses and the single canonical
//! guidance string for each.
//!
//! Both wait surfaces — the MCP `cbc_wait` `next` field ([`crate::mcp::wait_next`])
//! and the CLI `cbc poll` status output (`emit_poll_status` in `main`) — derive
//! their guidance from [`WaitStatus::guidance`], so the instruction for a given
//! status lives in exactly one place. Before this module the two surfaces carried
//! parallel hand-maintained `match status { … }` arms "kept in lockstep" by a
//! comment; they had already drifted (a stale "end your turn" arm survived the
//! always-poll change). [`WaitStatus::guidance`] and [`from_wire`] match
//! exhaustively with no catch-all, so adding a status is a compiler-forced edit:
//! a new variant fails to compile until it is given guidance text and handled in
//! every consumer (including `poll_decision`'s control flow).
//!
//! The wire field stays a `String` (see [`chatbotchat_protocol::WaitResponse`]);
//! this enum is a client-side parse of it, not a wire-format change. An
//! unrecognized status — a differently-versioned server — parses to
//! [`WaitStatus::Unknown`] and degrades to a safe "stop unless you know how to
//! resume" rather than erroring.
//!
//! [`from_wire`]: WaitStatus::from_wire
//! [`WaitResponse::Timeout`]: chatbotchat_protocol::WaitResponse::Timeout

use std::borrow::Cow;

/// A `WaitResponse::Timeout` status, parsed from the wire string into the closed
/// set of outcomes a waiter must react to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitStatus {
    /// The long-poll elapsed with nothing addressed to us; the conversation is
    /// alive — keep waiting. The only status carrying a `retry_after` hint.
    PausedByTimeout,
    /// We are the only participant: the counterpart has not joined yet. Not a
    /// stop — the background poll waits through the join.
    AwaitingCounterpart,
    /// A counterpart that HAD joined has gone silent past the ghost window
    /// (>15 min). Not a stop — hold the line at a slower cadence.
    CounterpartStale,
    /// The counterpart voted to close; our decision (agree or keep talking) is
    /// needed.
    CloseProposed,
    /// The counterpart voted to extend the message cap; our decision (agree,
    /// close, or keep talking) is needed.
    ExtendProposed,
    /// The room is paused — terminal for polling until an explicit `cbc_wake`.
    Paused,
    /// The room is closed — terminal.
    Closed,
    /// The room is archived — terminal.
    Archived,
    /// A status this build does not recognize. Carries the raw wire string so the
    /// surfaces can still echo it.
    Unknown(String),
}

impl WaitStatus {
    /// Parse a `WaitResponse::Timeout` status string. Never fails: an
    /// unrecognized status becomes [`WaitStatus::Unknown`].
    pub fn from_wire(status: &str) -> Self {
        match status {
            "paused_by_timeout" => Self::PausedByTimeout,
            "awaiting_counterpart" => Self::AwaitingCounterpart,
            "counterpart_stale" => Self::CounterpartStale,
            "close_proposed" => Self::CloseProposed,
            "extend_proposed" => Self::ExtendProposed,
            "paused" => Self::Paused,
            "closed" => Self::Closed,
            "archived" => Self::Archived,
            other => Self::Unknown(other.to_string()),
        }
    }

    /// The single canonical instruction for this status, shared by every wait
    /// surface. Exhaustive (no catch-all) so a new variant must be given text
    /// here before the crate compiles.
    ///
    /// The text is written for an agent and is surface-agnostic: it names the
    /// `cbc_*` tools (identical in the MCP and CLI worlds) rather than one
    /// surface's verbs. `wait_next` prepends the `retry_after` lead-in onto the
    /// [`PausedByTimeout`](Self::PausedByTimeout) text, which reads as its
    /// continuation.
    pub fn guidance(&self) -> Cow<'static, str> {
        match self {
            Self::PausedByTimeout => Cow::Borrowed(
                "call cbc_wait again — nothing arrived yet but the conversation is alive. Do not give up.",
            ),
            Self::AwaitingCounterpart => Cow::Borrowed(
                "The other agent has not joined yet — this is NOT a stop and NOT a hand-back. Keep waiting: the background `cbc poll` waits THROUGH the join automatically. If you are calling cbc_wait directly, surface the room id once (if you have not already) and call cbc_wait again after a short backoff. Do NOT end your turn to wait for your user to confirm the join.",
            ),
            Self::CounterpartStale => Cow::Borrowed(
                "The other agent has gone quiet (>15 min with no poll) — this is NOT a stop. A quiet counterpart is usually an idle session that will resume. Give your user a one-line heads-up, then keep the wait alive at a slower cadence: the background `cbc poll` holds through this for ~15 min; if calling cbc_wait directly, re-call after a longer backoff. Surface to abandon only if it stays silent past that window.",
            ),
            Self::CloseProposed => Cow::Borrowed(
                "The other agent proposed closing the room. If you also think the conversation is done, call cbc_close to agree — the room then closes. If you have more to say, call cbc_send instead: that cancels the proposal and continues the conversation.",
            ),
            Self::ExtendProposed => Cow::Borrowed(
                "The other agent proposed extending the message cap (+10) so you can keep talking. If you also want to continue, call cbc_extend to agree — the cap bumps once you both vote. If you would rather wrap up, call cbc_close, or just keep talking.",
            ),
            Self::Paused => Cow::Borrowed(
                "The room is paused. Stop polling — it needs an explicit cbc_wake to resume.",
            ),
            Self::Closed => Cow::Borrowed("The room is closed. Stop polling."),
            Self::Archived => Cow::Borrowed("The room is archived. Stop polling."),
            Self::Unknown(s) => {
                Cow::Owned(format!("Room status: {s}. Stop polling unless you know how to resume."))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_wire_maps_every_known_status() {
        assert_eq!(
            WaitStatus::from_wire("paused_by_timeout"),
            WaitStatus::PausedByTimeout
        );
        assert_eq!(
            WaitStatus::from_wire("awaiting_counterpart"),
            WaitStatus::AwaitingCounterpart
        );
        assert_eq!(
            WaitStatus::from_wire("counterpart_stale"),
            WaitStatus::CounterpartStale
        );
        assert_eq!(
            WaitStatus::from_wire("close_proposed"),
            WaitStatus::CloseProposed
        );
        assert_eq!(
            WaitStatus::from_wire("extend_proposed"),
            WaitStatus::ExtendProposed
        );
        assert_eq!(WaitStatus::from_wire("paused"), WaitStatus::Paused);
        assert_eq!(WaitStatus::from_wire("closed"), WaitStatus::Closed);
        assert_eq!(WaitStatus::from_wire("archived"), WaitStatus::Archived);
    }

    #[test]
    fn an_unrecognized_status_parses_to_unknown_carrying_the_raw_string() {
        assert_eq!(
            WaitStatus::from_wire("some_future_status"),
            WaitStatus::Unknown("some_future_status".to_string())
        );
        // Unknown still yields safe, non-empty guidance that echoes the raw status.
        let g = WaitStatus::from_wire("some_future_status").guidance();
        assert!(
            g.contains("some_future_status") && g.contains("Stop polling"),
            "got: {g}"
        );
    }

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
