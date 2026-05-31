use serde::{Deserialize, Serialize};

/// Request body for `POST /rooms`. `hard_cap` / `soft_cap` are optional open-time
/// overrides; omitted means the server's defaults (hard 10, soft 4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenRoomRequest {
    pub subject: String,
    #[serde(default)]
    pub hard_cap: Option<u32>,
    #[serde(default)]
    pub soft_cap: Option<u32>,
}

/// Response body for `POST /rooms`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenRoomResponse {
    pub room_id: String,
    /// The line the user pastes into the other agent's session to join.
    pub share_line: String,
}

/// Request body for `POST /rooms/:id/join`. `repo` and `cwd` are self-reported
/// by the caller (auto-detected from the shell / MCP working directory); the
/// `(room_id, repo, model, cwd)` tuple keys idempotent identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinRoomRequest {
    pub repo: String,
    pub model: String,
    pub cwd: String,
}

fn default_msg_type() -> String {
    "msg".to_string()
}

/// Wire view of a single message. `from`/`to` are participant handles; `to` is
/// `None` for a broadcast to all. `seq` is the monotonic ordering key. `msg_type`
/// (wire field `type`) distinguishes a conversation turn (`msg`) from a sentinel;
/// `severity` and `question_text` are populated only on a `waiting_user` sentinel
/// (the question the other agent is asking its user) and `null` otherwise.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageView {
    pub seq: i64,
    pub from: String,
    pub to: Option<String>,
    pub body: String,
    pub created_at: String,
    #[serde(rename = "type", default = "default_msg_type")]
    pub msg_type: String,
    #[serde(default)]
    pub severity: Option<String>,
    #[serde(default)]
    pub question_text: Option<String>,
}

/// Request body for `POST /rooms/:id/messages`. Identity is the
/// `(repo, model, cwd)` tuple (resolved server-side to the sender's handle,
/// same as join); the caller must already be a participant. `to` omitted means
/// broadcast to all.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub repo: String,
    pub model: String,
    pub cwd: String,
    #[serde(default)]
    pub to: Option<String>,
    pub body: String,
    /// `true` when the sender folded its user's input into this turn (`--human`).
    /// Resets the soft-cap consecutive-autonomous-turn counter.
    #[serde(default)]
    pub from_human: bool,
}

/// Response body for `POST /rooms/:id/messages`: the assigned monotonic `seq`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageResponse {
    pub seq: i64,
}

/// Request body for `POST /rooms/:id/signals`. Identity is the `(repo, model,
/// cwd)` tuple, same as send; the caller must already be a participant. A signal
/// is an out-of-band sentinel, not a conversation turn — it does not count toward
/// the caps and is always a broadcast. `signal_type` is the wire field `type`:
/// only `waiting_user` and `fold` are accepted here (`blocker_real_work`/`close`
/// land in the lifecycle slice). `severity` (`low|med|high`) and `question_text`
/// are required for `waiting_user` and absent for `fold`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalRequest {
    pub repo: String,
    pub model: String,
    pub cwd: String,
    #[serde(rename = "type")]
    pub signal_type: String,
    #[serde(default)]
    pub severity: Option<String>,
    #[serde(default)]
    pub question_text: Option<String>,
}

/// Response body for `POST /rooms/:id/signals`: the assigned monotonic `seq`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalResponse {
    pub seq: i64,
}

/// Query parameters for `GET /rooms/:id/wait`. Same tuple identity as send.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaitRequest {
    pub repo: String,
    pub model: String,
    pub cwd: String,
}

/// Response body for `GET /rooms/:id/wait`: either the next message addressed to
/// the caller, or a timeout sentinel when the server-side long-poll cap elapsed.
/// Serializes as `{ "message": { … }, "surface_to_user": bool }` or
/// `{ "status": "paused_by_timeout" }`. `surface_to_user` is the soft-cap signal:
/// `true` when this delivery is the (soft_cap − 1)th consecutive autonomous turn,
/// telling the receiving agent to fold its user in before replying.
/// `retry_after` (slice 5b) is the server-computed polling backoff in seconds:
/// present (on either variant) only while the counterpart is parked behind an
/// active `waiting_user` sentinel, telling the receiver how long it just backed
/// off and roughly how long to stay quiet. `skip_serializing_if` keeps it *off
/// the wire* — not `null` — when no sentinel is active, which is the contract.
/// Untagged disambiguation is unaffected: `message`/`status` stay the keys that
/// pick the variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WaitResponse {
    Message {
        message: MessageView,
        #[serde(default)]
        surface_to_user: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after: Option<u32>,
    },
    Timeout {
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after: Option<u32>,
    },
}

/// Response body for `POST /rooms/:id/join`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinRoomResponse {
    pub handle: String,
    /// `true` when an existing participant matching the tuple was returned;
    /// `false` when a fresh handle was minted. Lets the caller distinguish
    /// resuming an identity from joining anew.
    pub resumed: bool,
    pub room_state: String,
    pub recent_messages: Vec<MessageView>,
}

/// Wire view of a room participant, surfaced in `RoomStatus`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParticipantView {
    pub handle: String,
    pub repo: String,
    pub model: String,
    pub cwd: String,
    pub joined_at: String,
}

/// Response body for `GET /rooms/:id`. The wire-facing view of a room's status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomStatus {
    pub id: String,
    pub subject: String,
    pub state: String,
    pub started_at: String,
    pub last_activity_at: String,
    pub participants: Vec<ParticipantView>,
}

/// Uniform error body for failed requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    pub error: String,
}

impl ErrorEnvelope {
    pub fn new(message: impl Into<String>) -> Self {
        ErrorEnvelope {
            error: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A `waiting_user` sentinel view serializes the type under the wire key
    /// `type` and carries its severity + question, and survives a full
    /// serialize → deserialize round-trip unchanged.
    #[test]
    fn message_view_sentinel_round_trips() {
        let view = MessageView {
            seq: 7,
            from: "repo-a-opus47-0496".into(),
            to: None,
            body: String::new(),
            created_at: "2026-05-30T01:00:00Z".into(),
            msg_type: "waiting_user".into(),
            severity: Some("high".into()),
            question_text: Some("should I merge?".into()),
        };

        let value = serde_json::to_value(&view).expect("serialize");
        assert_eq!(value["type"], json!("waiting_user"), "wire key is `type`");
        assert_eq!(value["severity"], json!("high"));
        assert_eq!(value["question_text"], json!("should I merge?"));
        assert!(
            value.get("msg_type").is_none(),
            "the Rust field name must not leak onto the wire"
        );

        let back: MessageView = serde_json::from_value(value).expect("deserialize");
        assert_eq!(back.msg_type, "waiting_user");
        assert_eq!(back.severity.as_deref(), Some("high"));
        assert_eq!(back.question_text.as_deref(), Some("should I merge?"));
    }

    /// A plain `msg` view emits `type: "msg"` with null sentinel fields, and a
    /// legacy payload that omits the three new fields entirely deserializes to a
    /// `msg` with no severity/question (forward-compat defaults).
    #[test]
    fn message_view_plain_msg_and_legacy_payload_default_to_msg() {
        let view = MessageView {
            seq: 1,
            from: "repo-a-opus47-0496".into(),
            to: Some("repo-b-sonnet46-1234".into()),
            body: "hello".into(),
            created_at: "2026-05-30T01:00:00Z".into(),
            msg_type: "msg".into(),
            severity: None,
            question_text: None,
        };
        let value = serde_json::to_value(&view).expect("serialize");
        assert_eq!(value["type"], json!("msg"));
        assert_eq!(value["severity"], json!(null));
        assert_eq!(value["question_text"], json!(null));

        // Legacy shape (pre-5a): no type/severity/question_text keys.
        let legacy = json!({
            "seq": 2,
            "from": "repo-a-opus47-0496",
            "to": null,
            "body": "old client message",
            "created_at": "2026-05-30T01:00:00Z"
        });
        let parsed: MessageView = serde_json::from_value(legacy).expect("deserialize legacy");
        assert_eq!(parsed.msg_type, "msg", "missing type defaults to msg");
        assert_eq!(parsed.severity, None);
        assert_eq!(parsed.question_text, None);
    }

    fn sample_view() -> MessageView {
        MessageView {
            seq: 3,
            from: "repo-a-opus47-0496".into(),
            to: None,
            body: String::new(),
            created_at: "2026-05-30T01:00:00Z".into(),
            msg_type: "waiting_user".into(),
            severity: Some("high".into()),
            question_text: Some("merge?".into()),
        }
    }

    /// `retry_after` rides on the `Message` variant when a sentinel is active,
    /// and is *absent from the wire* (not `null`) when there is none — the
    /// "omitted" contract needs `skip_serializing_if`, which a round-trip alone
    /// would not catch. Untagged disambiguation still keys off `message`.
    #[test]
    fn wait_response_message_carries_optional_retry_after() {
        let with = WaitResponse::Message {
            message: sample_view(),
            surface_to_user: false,
            retry_after: Some(45),
        };
        let value = serde_json::to_value(&with).expect("serialize");
        assert_eq!(value["retry_after"], json!(45));
        let back: WaitResponse = serde_json::from_value(value).expect("deserialize");
        assert!(matches!(
            back,
            WaitResponse::Message {
                retry_after: Some(45),
                ..
            }
        ));

        let without = WaitResponse::Message {
            message: sample_view(),
            surface_to_user: false,
            retry_after: None,
        };
        let text = serde_json::to_string(&without).expect("serialize");
        assert!(
            !text.contains("retry_after"),
            "no sentinel ⇒ retry_after omitted, got: {text}"
        );
        let back: WaitResponse = serde_json::from_str(&text).expect("deserialize");
        assert!(matches!(
            back,
            WaitResponse::Message {
                retry_after: None,
                ..
            }
        ));
    }

    /// `retry_after` also rides on the `Timeout` variant (a parked wait that
    /// elapsed while a sentinel was active still hands back the backoff hint),
    /// and is likewise omitted when absent. Untagged disambiguation keys off
    /// `status`.
    #[test]
    fn wait_response_timeout_carries_optional_retry_after() {
        let with = WaitResponse::Timeout {
            status: "paused_by_timeout".into(),
            retry_after: Some(60),
        };
        let value = serde_json::to_value(&with).expect("serialize");
        assert_eq!(value["retry_after"], json!(60));
        let back: WaitResponse = serde_json::from_value(value).expect("deserialize");
        assert!(matches!(
            back,
            WaitResponse::Timeout {
                retry_after: Some(60),
                ..
            }
        ));

        let without = WaitResponse::Timeout {
            status: "paused_by_timeout".into(),
            retry_after: None,
        };
        let text = serde_json::to_string(&without).expect("serialize");
        assert!(
            !text.contains("retry_after"),
            "no sentinel ⇒ retry_after omitted, got: {text}"
        );
        let back: WaitResponse = serde_json::from_str(&text).expect("deserialize");
        assert!(matches!(
            back,
            WaitResponse::Timeout {
                retry_after: None,
                ..
            }
        ));
    }
}
