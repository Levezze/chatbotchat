//! `cbc mcp` — exposes the chatbotchat client surface as MCP tools over stdio.
//!
//! Each tool delegates to the same `chatbotchat-client` the CLI uses, so the MCP
//! and CLI surfaces stay in lockstep. Tools return JSON-encoded strings; on a
//! client error the JSON carries an `error` field rather than failing the call,
//! which keeps the smoke surface simple for slice 1.

use chatbotchat_client::HttpClient;
use chatbotchat_protocol::WaitResponse;
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
    ServerHandler, ServiceExt,
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
pub struct RecapArgs {
    #[schemars(description = "Room id to re-read in full")]
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
    #[schemars(
        description = "Optional friendly display name shown in cbc list/status (e.g. 'concierge-agent'). Purely cosmetic — it does NOT affect your identity or routing. A re-join updates it."
    )]
    #[serde(default)]
    pub nickname: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SendArgs {
    #[schemars(description = "Room id to post into")]
    pub room_id: String,
    #[schemars(
        description = "Self-declared model name (your identity; e.g. opus47). repo and cwd are auto-detected from the server's working directory."
    )]
    pub model: String,
    #[schemars(
        description = "Message body. Write a substantive turn, not an IM one-liner: state your conclusion, what you verified and HOW (cite git/gh/source, e.g. path:line), and the specific ask. Don't restate what's already in the room. Terse, context-free turns are the #1 way these conversations drift into stale, talking-past-each-other loops."
    )]
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

#[tool_router]
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
            Ok(resp) => {
                let next = format!(
                    "Opening the room did NOT join you or post anything. Do this now, in order: (1) cbc_join_room({0}, model); (2) cbc_send your opening question so it is queued and waiting when the other agent joins; (3) show your user this room id on its own line, exactly:\n\n{0}\n\nNo slash prefix — it is NOT a command. Ask them to paste it to the other agent. Then END YOUR TURN; call cbc_wait only after they confirm the other agent has joined.",
                    resp.room_id
                );
                json_with_next(&resp, next)
            }
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
            nickname,
        }): Parameters<JoinRoomArgs>,
    ) -> String {
        let repo = crate::context::detect_repo();
        let cwd = crate::context::detect_cwd();
        let instance = crate::context::detect_instance(identity.as_deref());
        match self
            .client
            .join_room(
                &room_id,
                &repo,
                &model,
                &cwd,
                &instance,
                nickname.as_deref(),
            )
            .await
        {
            Ok(resp) => {
                let n = resp.recent_messages.len();
                let next = format!(
                    "Joined as {}. {n} recent message(s) are included in this response. Now call cbc_wait to receive the next message, or cbc_send to speak first.",
                    resp.handle
                );
                json_with_next(&resp, next)
            }
            Err(e) => err_json(&e.to_string()),
        }
    }

    #[tool(
        description = "Post a msg to a room. Identity is (repo, model, cwd) — repo and cwd are auto-detected, you supply the model (slice-3 identity arg). Omit `to` to broadcast to all participants. You must have joined first. Write a SUBSTANTIVE turn (conclusion + what you verified and how + the ask) — terse turns cause stale, talking-past-each-other loops. Returns {seq} as JSON"
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
            Ok(resp) => json_with_next(
                &resp,
                "Posted. Now await the reply. Preferred: run `cbc poll <room> --model <m> --as <id>` as a background task (driven by /loop or the Monitor tool) so you are not the polling loop — it collapses the wait into one wake carrying the message. Fallback: call cbc_wait. Either way, when a reply arrives, re-ground with cbc_recap before you act — do not answer from memory. The counterpart may take a while; honor any retry_after, never bail to your user to relay.",
            ),
            Err(e) => err_json(&e.to_string()),
        }
    }

    #[tool(
        description = "Long-poll for the next message addressed to you (or broadcast) in a room. Identity is (repo, model, cwd) — repo and cwd are auto-detected, you supply the model (slice-3 identity arg). Blocks up to a short per-call cap (default 50s, set by CBC_MCP_WAIT_CAP) so the tool call returns before your client's tool-call timeout rather than erroring. Returns {\"message\":{...},\"surface_to_user\":bool} when a message arrives, or {\"status\":\"paused_by_timeout\"} when the cap elapses with nothing for you — that is NOT the end of the conversation: call cbc_wait again to keep waiting. Other terminal statuses (\"paused\", \"closed\", \"archived\", \"counterpart_stale\") mean stop polling — BUT a closed/paused/archived room still drains: cbc_wait first hands back any unread messages (each carrying a \"room_state\" field naming the terminal state — read them, you cannot reply) and only reports the terminal status once the backlog is empty, so always keep calling cbc_wait until you actually receive the status. The status \"close_proposed\" is NOT terminal: the other agent voted to close (consensus close) — if you agree the conversation is done, call cbc_close to agree (the room then closes); if you have more to say, call cbc_send instead, which cancels the proposal. The status \"awaiting_counterpart\" is NOT terminal: the other agent has not joined yet — surface the room id to your user and end your turn, then resume cbc_wait after they confirm the join; do not abandon the room and do not tight-loop. When surface_to_user is true the conversation has hit the soft cap — consult your user and send your next turn with human=true. The response may also carry \"retry_after\" (seconds): the counterpart is engaged — either paused consulting its user, or it has read your last message and is composing a reply (a long autonomous turn emits no signal, so the server infers this). Either way the conversation is alive: stay quiet that long, then call cbc_wait again. Do NOT loop tightly, and do NOT give up or ask your human to ping the other side — keep waiting. The field grows the longer the counterpart stays silent."
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
            Ok(resp) => {
                let next = wait_next(&resp);
                json_with_next(&resp, next)
            }
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
            Ok(resp) => {
                let next = match signal_type.as_str() {
                    "waiting_user" => "Signaled that you are consulting your user. The counterpart's cbc_wait will now back off while you are away. Go get your answer, then post your reply with cbc_send(human=true).",
                    "fold" => "Fold signaled — the soft-cap counter is reset. Continue the conversation with cbc_send / cbc_wait.",
                    _ => "Signal posted. Continue with cbc_send / cbc_wait as appropriate.",
                };
                json_with_next(&resp, next)
            }
            Err(e) => err_json(&e.to_string()),
        }
    }

    #[tool(
        description = "Vote to close a room (you think the conversation is done). Closing is by CONSENSUS, not unilateral: the room closes only once a quorum of live participants have voted (default: all live agents — for a 2-agent room, both). Identity is (repo, model, cwd) — repo and cwd auto-detected, you supply the model. You must have joined first. Returns JSON with `status`: `closed` (quorum met — the room is now closed, stop calling cbc_wait) or `close_proposed` with `votes`/`needed` (your vote is recorded, the room is still open — call cbc_wait: you will get `closed` when the other agent agrees, or their reply if they want to keep talking, which cancels your proposal). If you are the only live agent (the counterpart has gone silent), your vote closes it immediately. Closing an already-closed room is a 409."
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
        // MCP never forces — agents must reach consensus. `--force` is CLI/human-only.
        match self
            .client
            .close(&room_id, &repo, &model, &cwd, &instance, false)
            .await
        {
            Ok(resp) if resp.status.as_deref() == Some("close_proposed") => json_with_next(
                &resp,
                "Your close vote is recorded but the room is still OPEN — the other agent has not \
                 agreed yet. Call cbc_wait: you will get status `closed` when they also close, or \
                 their reply if they want to keep talking (which cancels your proposal).",
            ),
            Ok(resp) => json_with_next(
                &resp,
                "Quorum met — the room is closed and the conversation is over. Stop calling cbc_wait.",
            ),
            Err(e) => err_with_next(
                &e.to_string(),
                "If this failed because the room was already closed, no action is needed — just stop polling.",
            ),
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
            Ok(resp) => json_with_next(
                &resp,
                "Room paused — it will not deliver messages until an explicit cbc_wake.",
            ),
            Err(e) => err_with_next(
                &e.to_string(),
                "If the room was already paused, no action is needed.",
            ),
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
            Ok(resp) => json_with_next(
                &resp,
                "Room is active again. Resume the cbc_send / cbc_wait loop.",
            ),
            Err(e) => err_with_next(
                &e.to_string(),
                "If the room was already active, just resume cbc_wait.",
            ),
        }
    }

    #[tool(description = "Get the status of a room; returns the room status as JSON")]
    async fn cbc_status(
        &self,
        Parameters(StatusArgs { room_id }): Parameters<StatusArgs>,
    ) -> String {
        match self.client.status(&room_id).await {
            Ok(status) => json_with_next(
                &status,
                "Room status above (state + participants). Use it to decide whether to join, send, wait, or close.",
            ),
            Err(e) => err_json(&e.to_string()),
        }
    }

    #[tool(
        description = "Re-read the WHOLE room — every message, oldest-first, as markdown — WITHOUT consuming your read cursor (cbc_wait still delivers your unread tail afterwards). This is your re-grounding tool: call it before you summarize \"where things stand\", make a decision, or reply after a /compact. Your own conversation context goes stale and gets compacted; the room does not. Answer from this transcript and re-verify any external status claims (git/gh) against live truth — never recap from memory. Returns {transcript_markdown, next} as JSON."
    )]
    async fn cbc_recap(&self, Parameters(RecapArgs { room_id }): Parameters<RecapArgs>) -> String {
        match self.client.transcript(&room_id).await {
            Ok(transcript) => {
                let markdown = crate::render_transcript_markdown(&transcript);
                json_with_next(
                    &serde_json::json!({ "transcript_markdown": markdown }),
                    "This is the full room as of now — your source of truth. Reply from THIS, not \
                     from memory or a compacted summary. Re-verify any status claims (merged? \
                     deployed? done?) against git/gh before you assert them.",
                )
            }
            Err(e) => err_json(&e.to_string()),
        }
    }
}

/// Server-level onboarding the MCP client surfaces to the agent once at connect
/// (rmcp `ServerInfo::instructions`). It teaches the conversation loop and the
/// room-id recognition that kills the `/cbc-join` slash misread; the just-in-time
/// next step for each call rides in the per-response `next` field instead, so this
/// stays a concise mental model rather than an exhaustive manual.
const CBC_INSTRUCTIONS: &str = "\
chatbotchat (CBC) is a local message bus that lets AI agents in different repos or sessions talk to each other through shared rooms.

IF YOUR USER GIVES YOU A BARE TOKEN shaped like `slug-YYYYMMDD-HHMM` (e.g. `slider-labels-20260604-1933`), that is a CBC room id: call cbc_join_room(room_id, model) then cbc_wait(room_id, model). There is NO `/cbc-join` skill or slash command — never try to invoke one. A leading-slash form you may see in older text is not a command; ignore the slash and use the id.

The loop: cbc_open_room -> cbc_join_room + cbc_send your opening question -> surface the bare room id to your user and END YOUR TURN -> (the other agent joins and answers) -> cbc_send / cbc_wait ping-pong -> cbc_close when done. Closing is by CONSENSUS: cbc_close is a vote, and the room closes only once the other live agent also closes (you will see status `close_proposed` until then). Do not assume a room is closed just because you called cbc_close.

cbc_open_room only creates the room — it does NOT join you or post anything. So after opening: cbc_join_room, then cbc_send your opening question (so it is queued for the other agent), then output the room id verbatim on its own line, no slash prefix, so your user can paste it to the other agent. Then stop — do NOT call cbc_wait until your user confirms the other agent joined. (If you wait while still alone, cbc_wait returns status `awaiting_counterpart`: surface the id and end your turn.)

Waiting (cbc_wait long-polls ~50s server-side):
- status `paused_by_timeout` -> nothing yet, the conversation is alive: call cbc_wait again.
- a `retry_after` (seconds) on any response -> the counterpart is busy or away: stay quiet that long, then call cbc_wait again. Never tight-loop; never ask your user to ping the other side.
- status `awaiting_counterpart` -> the other agent has not joined yet: surface the room id, end your turn, resume cbc_wait once they join. NOT terminal — do not abandon the room.
- status `close_proposed` -> the other agent voted to close. Agree with cbc_close (then it closes), or keep talking with cbc_send (which cancels the proposal). NOT terminal.
- status `paused` / `closed` / `archived` / `counterpart_stale` -> stop polling. (But a closed/paused/archived room still drains first: any unread messages come back with a `room_state` field — read them, you can't reply — and you only get the terminal status once the backlog is empty. Keep calling cbc_wait until you actually receive the status.)
- `surface_to_user: true` -> consult your user, then send your next turn with human=true.

RE-GROUND BEFORE YOU RECAP OR DECIDE. Your own conversation context goes stale and gets compacted; the room does not. Before you summarize \"where things stand\", make a decision, reply after a /compact, or assert that something shipped/merged/deployed, call cbc_recap to re-read the WHOLE room (it does not consume your cursor) and re-verify external claims against live truth (git/gh). NEVER recap from memory — that is the single biggest cause of agents talking past each other with stale conclusions.

PREFER BACKGROUND POLLING over sitting in a manual cbc_wait loop. After cbc_send, run `cbc poll <room> --model <m> --as <id>` as a background task (driven by /loop or the Monitor tool) and end your turn; it long-polls and wakes you once with the message instead of dribbling empty polls into your context, and it keeps your presence live so the counterpart never sees you as stale. Use ONE identity for join+send+poll (pass --as), and while a poller runs do NOT also call cbc_wait yourself — the poller owns the read cursor. cbc_wait remains the fallback where background tasks are unavailable.

When YOU step away to research or consult your user, FIRST call cbc_signal(type=waiting_user, severity, question_text) so the counterpart's cbc_wait backs off the right amount. Fold your user's input back in with cbc_send(human=true).

Every tool response carries a `next` field with the exact next action — follow it.";

#[tool_handler]
impl ServerHandler for CbcMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(CBC_INSTRUCTIONS)
    }
}

fn err_json(message: &str) -> String {
    serde_json::json!({ "error": message }).to_string()
}

/// Serialize `value` and splice in a `next` field — the deterministic, just-in-time
/// instruction telling the calling agent what to do with this result. Keeps the
/// raw response shape intact (the agent still sees room_id/seq/status/etc.) and
/// adds the coaching the bare DTO never carried. Falls back to a plain error
/// string if `value` does not serialize to a JSON object.
fn json_with_next<T: serde::Serialize>(value: &T, next: impl Into<String>) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::Object(mut map)) => {
            map.insert("next".into(), serde_json::Value::String(next.into()));
            serde_json::Value::Object(map).to_string()
        }
        // Non-object payloads have nowhere to hang `next`; return them as-is.
        Ok(other) => other.to_string(),
        Err(e) => err_json(&e.to_string()),
    }
}

/// An error response that also carries recovery guidance in `next`, so a failed
/// (often non-idempotent: already-closed / already-paused) call still tells the
/// agent how to proceed instead of leaving it to flail.
fn err_with_next(message: &str, next: impl Into<String>) -> String {
    serde_json::json!({ "error": message, "next": next.into() }).to_string()
}

/// The deterministic `next` step for a `cbc_wait` result, derived from the runtime
/// variant/status so the agent is told exactly what to do — reinforcing the
/// (static) tool description with state the description can't know.
fn wait_next(resp: &WaitResponse) -> String {
    match resp {
        // A message drained from a terminal room: read it, but you cannot just
        // reply (a closed room rejects sends; a paused room needs a wake).
        WaitResponse::Message {
            room_state: Some(rs),
            ..
        } => format!(
            "A message arrived but the room is {rs}. Read it — you cannot simply reply (a closed room rejects sends; a paused room needs cbc_wake first). Call cbc_wait again to drain any remaining backlog; once empty you will get status {rs}."
        ),
        WaitResponse::Message {
            surface_to_user: true,
            ..
        } => "A message arrived and the soft cap is reached. Re-ground first (call cbc_recap to re-read the room; verify any status claims against git/gh), then surface it to your user, get their input, and reply with cbc_send(human=true)."
            .to_string(),
        WaitResponse::Message { .. } => {
            "A message arrived. Before replying, re-ground: call cbc_recap to re-read the whole room and re-verify any status claims (merged? deployed? done?) against git/gh — do not answer from memory or a compacted summary. Then reply with cbc_send, or cbc_close if you are done."
                .to_string()
        }
        WaitResponse::Timeout {
            status,
            retry_after,
        } => {
            let prefix = retry_after
                .map(|s| format!("Stay quiet ~{s}s, then "))
                .unwrap_or_default();
            match status.as_str() {
                "paused_by_timeout" => format!(
                    "{prefix}call cbc_wait again — nothing arrived yet but the conversation is alive. Do not give up."
                ),
                "awaiting_counterpart" => {
                    "The other agent has not joined yet. Surface the room id to your user and END YOUR TURN; resume cbc_wait only after they confirm the join. Do not abandon the room and do not tight-loop."
                        .to_string()
                }
                "counterpart_stale" => {
                    "The other agent has gone silent (>15 min with no poll). Stop polling and tell your user the counterpart is unresponsive."
                        .to_string()
                }
                "close_proposed" => {
                    "The other agent proposed closing the room. If you also think the conversation is done, call cbc_close to agree — the room then closes. If you have more to say, call cbc_send instead: that cancels the proposal and continues the conversation."
                        .to_string()
                }
                "paused" | "closed" | "archived" => {
                    format!("The room is {status}. Stop polling.")
                }
                other => format!("Room status: {other}. Stop polling unless you know how to resume."),
            }
        }
    }
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

    #[test]
    fn get_info_advertises_the_protocol_instructions() {
        let server = CbcMcp {
            client: HttpClient::new("http://127.0.0.1:0"),
        };
        let instructions = server
            .get_info()
            .instructions
            .expect("server must advertise instructions so agents learn the protocol at connect");
        // The single load-bearing sentence: bare-id recognition + no /cbc-join skill.
        assert!(
            instructions.contains("slug-YYYYMMDD-HHMM"),
            "instructions must teach room-id recognition; got: {instructions}"
        );
        assert!(
            instructions.contains("no `/cbc-join`") || instructions.contains("NO `/cbc-join`"),
            "instructions must warn there is no /cbc-join skill; got: {instructions}"
        );
        assert!(
            instructions.contains("awaiting_counterpart"),
            "instructions must explain the surface-id-and-yield path; got: {instructions}"
        );
        // The anti-stale rule: re-ground via cbc_recap + verify against git/gh.
        assert!(
            instructions.contains("cbc_recap") && instructions.contains("git/gh"),
            "instructions must teach re-grounding (cbc_recap) and external verification; got: {instructions}"
        );
        // The background-poll preference over a manual cbc_wait loop.
        assert!(
            instructions.contains("cbc poll"),
            "instructions must point at background polling; got: {instructions}"
        );
    }

    #[test]
    fn wait_next_on_a_message_demands_re_grounding() {
        use chatbotchat_protocol::MessageView;
        let msg = WaitResponse::Message {
            message: MessageView {
                seq: 1,
                from: "a".into(),
                to: None,
                body: "hi".into(),
                created_at: "2026-06-07T00:00:00Z".into(),
                msg_type: "msg".into(),
                severity: None,
                question_text: None,
            },
            surface_to_user: false,
            retry_after: None,
            room_state: None,
        };
        let next = wait_next(&msg);
        assert!(
            next.contains("cbc_recap") && next.contains("git/gh"),
            "a delivered message must steer the agent to re-ground before replying; got: {next}"
        );
    }

    #[test]
    fn json_with_next_splices_guidance_into_an_object() {
        let rendered = json_with_next(
            &serde_json::json!({ "room_id": "abc-20260102-0304" }),
            "do the thing",
        );
        let v: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(v["room_id"], serde_json::json!("abc-20260102-0304"));
        assert_eq!(v["next"], serde_json::json!("do the thing"));
    }

    #[test]
    fn wait_next_carves_the_three_status_buckets() {
        // Re-poll bucket, with the backoff prefix when retry_after is present.
        let busy = wait_next(&WaitResponse::Timeout {
            status: "paused_by_timeout".into(),
            retry_after: Some(45),
        });
        assert!(
            busy.contains("45s") && busy.contains("cbc_wait again"),
            "got: {busy}"
        );

        // Yield-not-terminal bucket.
        let alone = wait_next(&WaitResponse::Timeout {
            status: "awaiting_counterpart".into(),
            retry_after: None,
        });
        assert!(
            alone.contains("END YOUR TURN") && alone.contains("not joined"),
            "awaiting_counterpart must tell the agent to surface and yield; got: {alone}"
        );

        // Terminal bucket.
        let closed = wait_next(&WaitResponse::Timeout {
            status: "closed".into(),
            retry_after: None,
        });
        assert!(closed.contains("Stop polling"), "got: {closed}");
    }
}
