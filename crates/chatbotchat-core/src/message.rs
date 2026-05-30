use time::OffsetDateTime;

/// The kind of a message row. `Msg` is a conversation turn and is the only kind
/// that counts toward the caps; the rest are sentinels — out-of-band signals
/// (consulting the user, blocked on real work, folding, closing) that the
/// counterpart reads but that do not consume the cap budget. Wire/storage form
/// is the lowercase string from `docs/v1-design-locked.md` §Message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    Msg,
    WaitingUser,
    BlockerRealWork,
    Fold,
    Close,
}

impl MessageType {
    pub fn as_str(self) -> &'static str {
        match self {
            MessageType::Msg => "msg",
            MessageType::WaitingUser => "waiting_user",
            MessageType::BlockerRealWork => "blocker_real_work",
            MessageType::Fold => "fold",
            MessageType::Close => "close",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "msg" => MessageType::Msg,
            "waiting_user" => MessageType::WaitingUser,
            "blocker_real_work" => MessageType::BlockerRealWork,
            "fold" => MessageType::Fold,
            "close" => MessageType::Close,
            _ => return None,
        })
    }
}

/// Severity of a `waiting_user` sentinel — how urgently the consulting agent's
/// pause should slow the counterpart's polling. Only meaningful on a
/// `waiting_user` row; `None` everywhere else. Wire/storage form is the lowercase
/// string from `docs/v1-design-locked.md`. The backoff base table (slice 5b)
/// reads this: `low → 10s`, `med → 20s`, `high → 45s`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Low,
    Med,
    High,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Low => "low",
            Severity::Med => "med",
            Severity::High => "high",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "low" => Severity::Low,
            "med" => Severity::Med,
            "high" => Severity::High,
            _ => return None,
        })
    }
}

/// A single message posted to a room. `seq` is the monotonic primary key (SQLite
/// rowid) and doubles as the long-poll read cursor. `recipient == None` is a
/// broadcast to all participants; `Some(handle)` is targeted delivery.
/// `msg_type` distinguishes conversation turns from sentinels (see `MessageType`).
/// `from_human` is `true` when the sender folded its user's input into this turn
/// (the `--human` send) — it is the soft-cap reset boundary. `severity` and
/// `question_text` are populated only on a `waiting_user` sentinel (the question
/// the agent is asking its user); both are `None` for a plain `msg`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub seq: i64,
    pub room_id: String,
    pub sender: String,
    pub recipient: Option<String>,
    pub body: String,
    pub created_at: OffsetDateTime,
    pub msg_type: MessageType,
    pub from_human: bool,
    pub severity: Option<Severity>,
    pub question_text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `as_str` and `parse` must be exact inverses for every variant — the wire
    /// form is what lands in the DB `type` column, so a typo in either arm would
    /// silently mislabel a sentinel (or fail to read it back). Total coverage.
    #[test]
    fn message_type_str_round_trips_all_variants() {
        for t in [
            MessageType::Msg,
            MessageType::WaitingUser,
            MessageType::BlockerRealWork,
            MessageType::Fold,
            MessageType::Close,
        ] {
            assert_eq!(
                MessageType::parse(t.as_str()),
                Some(t),
                "round-trip failed for {t:?}"
            );
        }
    }

    #[test]
    fn message_type_parse_rejects_unknown() {
        assert_eq!(MessageType::parse("not-a-type"), None);
    }
}
