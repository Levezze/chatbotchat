//! `cbc mcp` — exposes the chatbotchat client surface as MCP tools over stdio.
//!
//! Each tool delegates to the same `chatbotchat-client` the CLI uses, so the MCP
//! and CLI surfaces stay in lockstep. Tools return JSON-encoded strings; on a
//! client error the JSON carries an `error` field rather than failing the call,
//! which keeps the smoke surface simple for slice 1.

use chatbotchat_client::HttpClient;
use rmcp::{
    handler::server::wrapper::Parameters, schemars, tool, tool_router, transport::stdio, ServiceExt,
};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct OpenRoomArgs {
    #[schemars(description = "Subject / topic of the room")]
    pub subject: String,
    #[schemars(description = "Hard cap: max messages before sends are refused (default 10)")]
    #[serde(default)]
    pub hard_cap: Option<u32>,
    #[schemars(
        description = "Soft cap: consecutive autonomous turns before the user is surfaced (default 4)"
    )]
    #[serde(default)]
    pub soft_cap: Option<u32>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct StatusArgs {
    #[schemars(description = "Room id to look up")]
    pub room_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct JoinRoomArgs {
    #[schemars(description = "Room id to join")]
    pub room_id: String,
    #[schemars(description = "Self-declared model name, e.g. opus47, sonnet46, codex53")]
    pub model: String,
    #[schemars(
        description = "Optional identity label. Auto-derived per session when omitted; two agents in the same repo+model+dir MUST pass distinct values to be seen as separate participants. Reuse the same label from another terminal/client/dir to resume or hand off this identity."
    )]
    #[serde(default, rename = "as")]
    pub identity: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SendArgs {
    #[schemars(description = "Room id to post into")]
    pub room_id: String,
    #[schemars(
        description = "Self-declared model name (your identity; e.g. opus47). repo and cwd are auto-detected from the server's working directory."
    )]
    pub model: String,
    #[schemars(description = "Message body")]
    pub body: String,
    #[schemars(description = "Optional recipient handle; omit to broadcast to all participants")]
    #[serde(default)]
    pub to: Option<String>,
    #[schemars(
        description = "Set true when folding your user's input into this turn; resets the soft-cap counter"
    )]
    #[serde(default)]
    pub human: bool,
    #[schemars(
        description = "Optional identity label (see cbc_join_room). Pass the same value you joined with."
    )]
    #[serde(default, rename = "as")]
    pub identity: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SignalArgs {
    #[schemars(description = "Room id to signal")]
    pub room_id: String,
    #[schemars(
        description = "Self-declared model name (your identity; e.g. opus47). repo and cwd are auto-detected from the server's working directory."
    )]
    pub model: String,
    #[serde(rename = "type")]
    #[schemars(description = "Signal type: waiting_user or fold")]
    pub signal_type: String,
    #[schemars(
        description = "Severity for waiting_user: low, med, or high (required for waiting_user)"
    )]
    #[serde(default)]
    pub severity: Option<String>,
    #[schemars(
        description = "The question you are asking your user (required for waiting_user, omit for fold)"
    )]
    #[serde(default)]
    pub question_text: Option<String>,
    #[schemars(
        description = "Optional identity label (see cbc_join_room). Pass the same value you joined with."
    )]
    #[serde(default, rename = "as")]
    pub identity: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WaitArgs {
    #[schemars(description = "Room id to long-poll")]
    pub room_id: String,
    #[schemars(
        description = "Self-declared model name (your identity; e.g. opus47). repo and cwd are auto-detected from the server's working directory."
    )]
    pub model: String,
    #[schemars(
        description = "Optional identity label (see cbc_join_room). Pass the same value you joined with."
    )]
    #[serde(default, rename = "as")]
    pub identity: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CloseArgs {
    #[schemars(description = "Room id to close")]
    pub room_id: String,
    #[schemars(
        description = "Self-declared model name (your identity; e.g. opus47). repo and cwd are auto-detected from the server's working directory."
    )]
    pub model: String,
    #[schemars(
        description = "Optional identity label (see cbc_join_room). Pass the same value you joined with."
    )]
    #[serde(default, rename = "as")]
    pub identity: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PauseArgs {
    #[schemars(description = "Room id to pause")]
    pub room_id: String,
    #[schemars(
        description = "Self-declared model name (your identity; e.g. opus47). repo and cwd are auto-detected from the server's working directory."
    )]
    pub model: String,
    #[schemars(description = "Optional free-text reason, recorded in the room's audit log")]
    #[serde(default)]
    pub reason: Option<String>,
    #[schemars(
        description = "Optional identity label (see cbc_join_room). Pass the same value you joined with."
    )]
    #[serde(default, rename = "as")]
    pub identity: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WakeArgs {
    #[schemars(description = "Room id to wake back to active")]
    pub room_id: String,
    #[schemars(
        description = "Self-declared model name (your identity; e.g. opus47). repo and cwd are auto-detected from the server's working directory."
    )]
    pub model: String,
    #[schemars(
        description = "Optional identity label (see cbc_join_room). Pass the same value you joined with."
    )]
    #[serde(default, rename = "as")]
    pub identity: Option<String>,
}

#[derive(Clone)]
pub struct CbcMcp {
    client: HttpClient,
}

#[tool_router(server_handler)]
impl CbcMcp {
    #[tool(description = "Open a new chatbotchat room; returns {room_id, share_line} as JSON")]
    async fn cbc_open_room(
        &self,
        Parameters(OpenRoomArgs {
            subject,
            hard_cap,
            soft_cap,
        }): Parameters<OpenRoomArgs>,
    ) -> String {
        match self.client.open_room(&subject, hard_cap, soft_cap).await {
            Ok(resp) => json_or_err(&resp),
            Err(e) => err_json(&e.to_string()),
        }
    }

    #[tool(
        description = "Join a room as a participant; repo and cwd are auto-detected from the server's working directory. Returns {handle, resumed, room_state, recent_messages} as JSON"
    )]
    async fn cbc_join_room(
        &self,
        Parameters(JoinRoomArgs {
            room_id,
            model,
            identity,
        }): Parameters<JoinRoomArgs>,
    ) -> String {
        let repo = crate::context::detect_repo();
        let cwd = crate::context::detect_cwd();
        let instance = crate::context::detect_instance(identity.as_deref());
        match self
            .client
            .join_room(&room_id, &repo, &model, &cwd, &instance)
            .await
        {
            Ok(resp) => json_or_err(&resp),
            Err(e) => err_json(&e.to_string()),
        }
    }

    #[tool(
        description = "Post a msg to a room. Identity is (repo, model, cwd) — repo and cwd are auto-detected, you supply the model (slice-3 identity arg). Omit `to` to broadcast to all participants. You must have joined first. Returns {seq} as JSON"
    )]
    async fn cbc_send(
        &self,
        Parameters(SendArgs {
            room_id,
            model,
            body,
            to,
            human,
            identity,
        }): Parameters<SendArgs>,
    ) -> String {
        let repo = crate::context::detect_repo();
        let cwd = crate::context::detect_cwd();
        let instance = crate::context::detect_instance(identity.as_deref());
        match self
            .client
            .send_message(
                &room_id,
                &repo,
                &model,
                &cwd,
                &instance,
                to.as_deref(),
                &body,
                human,
            )
            .await
        {
            Ok(resp) => json_or_err(&resp),
            Err(e) => err_json(&e.to_string()),
        }
    }

    #[tool(
        description = "Long-poll for the next message addressed to you (or broadcast) in a room. Identity is (repo, model, cwd) — repo and cwd are auto-detected, you supply the model (slice-3 identity arg). Blocks up to a short per-call cap (default 50s, set by CBC_MCP_WAIT_CAP) so the tool call returns before your client's tool-call timeout rather than erroring. Returns {\"message\":{...},\"surface_to_user\":bool} when a message arrives, or {\"status\":\"paused_by_timeout\"} when the cap elapses with nothing for you — that is NOT the end of the conversation: call cbc_wait again to keep waiting. Other terminal statuses (\"paused\", \"closed\", \"archived\", \"counterpart_stale\") mean stop polling. When surface_to_user is true the conversation has hit the soft cap — consult your user and send your next turn with human=true. The response may also carry \"retry_after\" (seconds): the counterpart is engaged — either paused consulting its user, or it has read your last message and is composing a reply (a long autonomous turn emits no signal, so the server infers this). Either way the conversation is alive: stay quiet that long, then call cbc_wait again. Do NOT loop tightly, and do NOT give up or ask your human to ping the other side — keep waiting. The field grows the longer the counterpart stays silent."
    )]
    async fn cbc_wait(
        &self,
        Parameters(WaitArgs {
            room_id,
            model,
            identity,
        }): Parameters<WaitArgs>,
    ) -> String {
        let repo = crate::context::detect_repo();
        let cwd = crate::context::detect_cwd();
        let instance = crate::context::detect_instance(identity.as_deref());
        match self
            .client
            .wait(
                &room_id,
                &repo,
                &model,
                &cwd,
                &instance,
                Some(mcp_wait_cap_secs()),
            )
            .await
        {
            Ok(resp) => json_or_err(&resp),
            Err(e) => err_json(&e.to_string()),
        }
    }

    #[tool(
        description = "Emit a sentinel (out-of-band signal) to a room. `type` is waiting_user (you are consulting your user) or fold. waiting_user requires `severity` (low|med|high) and `question_text` (the question you are asking your user); fold takes neither. Identity is (repo, model, cwd) — repo and cwd auto-detected, you supply the model. You must have joined first. Sentinels do not count toward the caps. Returns {seq} as JSON"
    )]
    async fn cbc_signal(
        &self,
        Parameters(SignalArgs {
            room_id,
            model,
            signal_type,
            severity,
            question_text,
            identity,
        }): Parameters<SignalArgs>,
    ) -> String {
        let repo = crate::context::detect_repo();
        let cwd = crate::context::detect_cwd();
        let instance = crate::context::detect_instance(identity.as_deref());
        match self
            .client
            .signal(
                &room_id,
                &repo,
                &model,
                &cwd,
                &instance,
                &signal_type,
                severity.as_deref(),
                question_text.as_deref(),
            )
            .await
        {
            Ok(resp) => json_or_err(&resp),
            Err(e) => err_json(&e.to_string()),
        }
    }

    #[tool(
        description = "Explicitly close a room (the conversation is done). Identity is (repo, model, cwd) — repo and cwd auto-detected, you supply the model. You must have joined first. Non-idempotent: closing an already-closed room is a 409. Returns {state} as JSON"
    )]
    async fn cbc_close(
        &self,
        Parameters(CloseArgs {
            room_id,
            model,
            identity,
        }): Parameters<CloseArgs>,
    ) -> String {
        let repo = crate::context::detect_repo();
        let cwd = crate::context::detect_cwd();
        let instance = crate::context::detect_instance(identity.as_deref());
        match self
            .client
            .close(&room_id, &repo, &model, &cwd, &instance)
            .await
        {
            Ok(resp) => json_or_err(&resp),
            Err(e) => err_json(&e.to_string()),
        }
    }

    #[tool(
        description = "Pause a room (park it; only an explicit wake resumes it). Optional `reason` is recorded in the audit log. Identity is (repo, model, cwd) — repo and cwd auto-detected, you supply the model. You must have joined first. Non-idempotent: pausing an already-paused room is a 409. Returns {state} as JSON"
    )]
    async fn cbc_pause(
        &self,
        Parameters(PauseArgs {
            room_id,
            model,
            reason,
            identity,
        }): Parameters<PauseArgs>,
    ) -> String {
        let repo = crate::context::detect_repo();
        let cwd = crate::context::detect_cwd();
        let instance = crate::context::detect_instance(identity.as_deref());
        match self
            .client
            .pause(&room_id, &repo, &model, &cwd, &instance, reason.as_deref())
            .await
        {
            Ok(resp) => json_or_err(&resp),
            Err(e) => err_json(&e.to_string()),
        }
    }

    #[tool(
        description = "Wake a paused (or idle) room back to active. Identity is (repo, model, cwd) — repo and cwd auto-detected, you supply the model. You must have joined first. Non-idempotent: waking an already-active room is a 409. Returns {state} as JSON"
    )]
    async fn cbc_wake(
        &self,
        Parameters(WakeArgs {
            room_id,
            model,
            identity,
        }): Parameters<WakeArgs>,
    ) -> String {
        let repo = crate::context::detect_repo();
        let cwd = crate::context::detect_cwd();
        let instance = crate::context::detect_instance(identity.as_deref());
        match self
            .client
            .wake(&room_id, &repo, &model, &cwd, &instance)
            .await
        {
            Ok(resp) => json_or_err(&resp),
            Err(e) => err_json(&e.to_string()),
        }
    }

    #[tool(description = "Get the status of a room; returns the room status as JSON")]
    async fn cbc_status(
        &self,
        Parameters(StatusArgs { room_id }): Parameters<StatusArgs>,
    ) -> String {
        match self.client.status(&room_id).await {
            Ok(status) => json_or_err(&status),
            Err(e) => err_json(&e.to_string()),
        }
    }
}

fn json_or_err<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|e| err_json(&e.to_string()))
}

fn err_json(message: &str) -> String {
    serde_json::json!({ "error": message }).to_string()
}

/// Default per-call cap (seconds) for the MCP `cbc_wait` long-poll. Short enough
/// to return before a typical MCP client's tool-call timeout so the call never
/// errors out; the agent simply re-polls. The server's own cap (10 min) still
/// applies to the CLI, which omits the per-call cap.
const DEFAULT_MCP_WAIT_CAP_SECS: u32 = 50;

/// Resolve the MCP wait cap from the raw `CBC_MCP_WAIT_CAP` value. Falls back to
/// the default when unset or unparseable, and clamps to `[1, 590]` so it stays a
/// positive value under the server's 600s cap.
fn parse_mcp_wait_cap(raw: Option<&str>) -> u32 {
    raw.and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(DEFAULT_MCP_WAIT_CAP_SECS)
        .clamp(1, 590)
}

/// The per-call cap the MCP `cbc_wait` tool passes to the server, from
/// `CBC_MCP_WAIT_CAP` (seconds) or the default.
fn mcp_wait_cap_secs() -> u32 {
    parse_mcp_wait_cap(std::env::var("CBC_MCP_WAIT_CAP").ok().as_deref())
}

/// Serve the MCP tools over stdio until the client disconnects.
pub async fn run(client: HttpClient) -> anyhow::Result<()> {
    let service = CbcMcp { client }.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_wait_cap_defaults_when_unset_or_garbage() {
        assert_eq!(parse_mcp_wait_cap(None), DEFAULT_MCP_WAIT_CAP_SECS);
        assert_eq!(
            parse_mcp_wait_cap(Some("nonsense")),
            DEFAULT_MCP_WAIT_CAP_SECS
        );
    }

    #[test]
    fn mcp_wait_cap_honors_a_valid_override() {
        assert_eq!(parse_mcp_wait_cap(Some("45")), 45);
        assert_eq!(parse_mcp_wait_cap(Some("  120 ")), 120);
    }

    #[test]
    fn mcp_wait_cap_clamps_under_the_server_cap() {
        assert_eq!(parse_mcp_wait_cap(Some("0")), 1);
        assert_eq!(parse_mcp_wait_cap(Some("100000")), 590);
    }
}
