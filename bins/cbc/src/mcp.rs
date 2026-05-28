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
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct StatusArgs {
    #[schemars(description = "Room id to look up")]
    pub room_id: String,
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
        Parameters(OpenRoomArgs { subject }): Parameters<OpenRoomArgs>,
    ) -> String {
        match self.client.open_room(&subject).await {
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
