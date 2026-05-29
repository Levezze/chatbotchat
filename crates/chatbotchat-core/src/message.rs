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

/// A single message posted to a room. `seq` is the monotonic primary key (SQLite
/// rowid) and doubles as the long-poll read cursor. `recipient == None` is a
/// broadcast to all participants; `Some(handle)` is targeted delivery.
/// `msg_type` distinguishes conversation turns from sentinels (see `MessageType`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub seq: i64,
    pub room_id: String,
    pub sender: String,
    pub recipient: Option<String>,
    pub body: String,
    pub created_at: OffsetDateTime,
    pub msg_type: MessageType,
}
