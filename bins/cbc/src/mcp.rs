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
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WaitArgs {
    #[schemars(description = "Room id to long-poll")]
    pub room_id: String,
    #[schemars(
        description = "Self-declared model name (your identity; e.g. opus47). repo and cwd are auto-detected from the server's working directory."
    )]
    pub model: String,
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
        Parameters(JoinRoomArgs { room_id, model }): Parameters<JoinRoomArgs>,
    ) -> String {
        let repo = crate::context::detect_repo();
        let cwd = crate::context::detect_cwd();
        match self.client.join_room(&room_id, &repo, &model, &cwd).await {
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
        }): Parameters<SendArgs>,
    ) -> String {
        let repo = crate::context::detect_repo();
        let cwd = crate::context::detect_cwd();
        match self
            .client
            .send_message(&room_id, &repo, &model, &cwd, to.as_deref(), &body, human)
            .await
        {
            Ok(resp) => json_or_err(&resp),
            Err(e) => err_json(&e.to_string()),
        }
    }

    #[tool(
        description = "Long-poll for the next message addressed to you (or broadcast) in a room. Identity is (repo, model, cwd) — repo and cwd are auto-detected, you supply the model (slice-3 identity arg). Blocks up to the server cap (10 min); returns {\"status\":\"paused_by_timeout\"} on cap, otherwise {\"message\":{...},\"surface_to_user\":bool} as JSON. When surface_to_user is true the conversation has hit the soft cap — consult your user and send your next turn with human=true."
    )]
    async fn cbc_wait(
        &self,
        Parameters(WaitArgs { room_id, model }): Parameters<WaitArgs>,
    ) -> String {
        let repo = crate::context::detect_repo();
        let cwd = crate::context::detect_cwd();
        match self.client.wait(&room_id, &repo, &model, &cwd).await {
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

/// Serve the MCP tools over stdio until the client disconnects.
pub async fn run(client: HttpClient) -> anyhow::Result<()> {
    let service = CbcMcp { client }.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
