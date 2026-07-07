//! `cbc mcp` — exposes the chatbotchat client surface as MCP tools over stdio.
//!
//! Each tool delegates to the same `chatbotchat-client` the CLI uses, so the MCP
//! and CLI surfaces stay in lockstep. Tools return JSON-encoded strings; on a
//! client error the JSON carries an `error` field rather than failing the call,
//! which keeps the smoke surface simple for slice 1.

use crate::wait_status::WaitGuidance;
use chatbotchat_client::HttpClient;
use chatbotchat_protocol::{WaitResponse, WaitStatus};
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
    #[schemars(description = "Hard cap: max messages before sends are refused (default 20)")]
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
    #[schemars(description = "Room id")]
    pub room_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RecapArgs {
    #[schemars(description = "Room id")]
    pub room_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct JoinRoomArgs {
    #[schemars(description = "Room id")]
    pub room_id: String,
    #[schemars(description = "Model name, e.g. opus47, sonnet46, codex53.")]
    pub model: String,
    #[schemars(
        description = "Your anchored identity — a STABLE role label you pick once and reuse forever: `<repo>-worker-<feature>` (worker) or `<repo>-orchestrator`. Pass the SAME value here, on send, on your `cbc poll --as`, and in your `.cbc` connections block; that one shared identity keeps you a single participant and stops the relaunch nag. Never a session id (rotates on restart/fork); never a per-call label (mints duplicates)."
    )]
    #[serde(default, rename = "as")]
    pub identity: Option<String>,
    #[schemars(
        description = "Optional cosmetic display name shown in cbc list/status; does not affect identity or routing."
    )]
    #[serde(default)]
    pub nickname: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SendArgs {
    #[schemars(description = "Room id")]
    pub room_id: String,
    #[schemars(description = "Model name, e.g. opus47.")]
    pub model: String,
    #[schemars(
        description = "Message body. A substantive turn (conclusion + what you verified and how + the ask), not a one-liner — terse turns cause stale, talking-past-each-other loops."
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
    #[schemars(description = "Anchored `--as` label — same as join.")]
    #[serde(default, rename = "as")]
    pub identity: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SignalArgs {
    #[schemars(description = "Room id")]
    pub room_id: String,
    #[schemars(description = "Model name, e.g. opus47.")]
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
    #[schemars(description = "Anchored `--as` label — same as join.")]
    #[serde(default, rename = "as")]
    pub identity: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WaitArgs {
    #[schemars(description = "Room id")]
    pub room_id: String,
    #[schemars(description = "Model name, e.g. opus47.")]
    pub model: String,
    #[schemars(description = "Anchored `--as` label — same as join.")]
    #[serde(default, rename = "as")]
    pub identity: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CloseArgs {
    #[schemars(description = "Room id")]
    pub room_id: String,
    #[schemars(description = "Model name, e.g. opus47.")]
    pub model: String,
    #[schemars(description = "Anchored `--as` label — same as join.")]
    #[serde(default, rename = "as")]
    pub identity: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ExtendArgs {
    #[schemars(description = "Room id")]
    pub room_id: String,
    #[schemars(description = "Model name, e.g. opus47.")]
    pub model: String,
    #[schemars(description = "Anchored `--as` label — same as join.")]
    #[serde(default, rename = "as")]
    pub identity: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PauseArgs {
    #[schemars(description = "Room id")]
    pub room_id: String,
    #[schemars(description = "Model name, e.g. opus47.")]
    pub model: String,
    #[schemars(description = "Optional free-text reason, recorded in the room's audit log")]
    #[serde(default)]
    pub reason: Option<String>,
    #[schemars(description = "Anchored `--as` label — same as join.")]
    #[serde(default, rename = "as")]
    pub identity: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WakeArgs {
    #[schemars(description = "Room id")]
    pub room_id: String,
    #[schemars(description = "Model name, e.g. opus47.")]
    pub model: String,
    #[schemars(description = "Anchored `--as` label — same as join.")]
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
                    "Opening the room did NOT join you or post anything. Opening COMMITS you to the room: you must surface the id AND have a poll running before you do anything else — do NOT open it as a side task and keep working, and do NOT end this turn, until both are done. Do this now, in order: (1) cbc_join_room({0}, model) with your anchored `as:<label>` (a STABLE role name — `<repo>-orchestrator`, or `<repo>-worker-<feature>` — reused on every call); (2) cbc_send your opening question (same `as:<label>`) so it is queued and waiting when the other agent joins; (3) show your user this room id on its own line, exactly:\n\n{0}\n\nNo slash prefix — it is NOT a command. Ask them to paste it to the other agent. Then start the wait hands-free: launch `cbc poll {0} --model <m> --as <label>` as a background task and end your turn (the SAME `--as <label>` you joined with, so your join, sends, poll, and `.cbc` connections block are one identity — a poll under a different label, or none, splits you into duplicate rows that nag every turn). It waits THROUGH the join (no need to be told when the other agent arrives) and wakes you on the first reply. Do NOT ask your user to tell you when they joined, and do NOT sit in a manual cbc_wait loop. Opening and vanishing — no id surfaced, no poll running — is the failure to avoid. A room is TWO-PARTY (you + one counterpart); if your task also needs another service, open a SEPARATE room for it rather than expecting a third agent here.",
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
                let next = join_next(
                    &resp.handle,
                    &room_id,
                    resp.recent_messages.len(),
                    identity.as_deref(),
                );
                json_with_next(&resp, next)
            }
            Err(e) => err_json(&e.to_string()),
        }
    }

    #[tool(
        description = "Post a message — a substantive turn (your conclusion + what you verified), not a bare ack — to a room you have joined (omit `to` to broadcast). Returns {seq}; the `next` field says what to do after sending."
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
            Ok(resp) => {
                let next = send_next(&room_id, identity.as_deref());
                json_with_next(&resp, next)
            }
            Err(e) => err_json(&e.to_string()),
        }
    }

    #[tool(
        description = "Long-poll for the next message (blocks ~50s). Returns a message (with `surface_to_user`), or a non-terminal status (`paused_by_timeout`, `awaiting_counterpart`, `counterpart_stale`, `close_proposed`, `extend_proposed`) — the `next` field says how to handle each. Keep calling until a terminal status (`closed`/`paused`/`archived`)."
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
        description = "Emit an out-of-band sentinel (`type`=waiting_user, requires `severity` low|med|high + `question_text`; or fold). Does not count toward caps. Returns {seq}."
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
        description = "Consensus vote to close a room; closes only once a quorum of live agents vote (a lone agent closes immediately). Before voting, send everything substantive — voting close can finalize the room and drop an unsent reply. Never `cbc close --force`: that is a human-only escape hatch; agents close only via this vote. Returns `status` (`closed`, or `close_proposed` with `votes`/`needed`); the `next` field explains the vote state."
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
                 their reply if they want to keep talking. Your close vote stands until YOU \
                 yourself send another message — a counterpart's reply no longer clears it — so \
                 once they also vote close, the room closes. \
                 If BOTH agents have already voted but the room stays in close_proposed, the cause \
                 is almost always a stale duplicate participant from identity churn — run \
                 `cbc prune <room>` to drop aged-out rows, then re-vote cbc_close. See the /cbc \
                 skill for the full stall-recovery detail.",
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
        description = "Consensus vote to extend the message cap by +20 (repeatable). Bumps only once a quorum of live agents vote. Returns `status` (`extended` with new `hard_cap`, or `extend_proposed` with `votes`/`needed`); the `next` field explains the vote state."
    )]
    async fn cbc_extend(
        &self,
        Parameters(ExtendArgs {
            room_id,
            model,
            identity,
        }): Parameters<ExtendArgs>,
    ) -> String {
        let repo = crate::context::detect_repo();
        let cwd = crate::context::detect_cwd();
        let instance = crate::context::detect_instance(identity.as_deref());
        match self
            .client
            .extend(&room_id, &repo, &model, &cwd, &instance)
            .await
        {
            Ok(resp) if resp.status == "extend_proposed" => json_with_next(
                &resp,
                "Your extend vote is recorded but the cap is unchanged — the other agent has not \
                 agreed yet. Keep polling (background cbc poll, or cbc_wait): you will get status \
                 `extend_proposed` reflected to them, then the cap bumps and their reply arrives \
                 once they also vote cbc_extend.",
            ),
            Ok(resp) => json_with_next(
                &resp,
                "Quorum met — the cap is extended (see `hard_cap`). Continue with cbc_send if it \
                 is your turn, or resume your poll for the other agent's next turn (the proposer \
                 often has the reply that hit the wall).",
            ),
            Err(e) => err_with_next(
                &e.to_string(),
                "If this failed because the room is closed/paused, you cannot extend it — stop or wake it first.",
            ),
        }
    }

    #[tool(
        description = "Pause a room you have joined (park it; only an explicit cbc_wake resumes it). Optional `reason` is audit-logged. Non-idempotent (409 if already paused). Returns {state}."
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
        description = "Wake a paused/idle room you have joined back to active. Non-idempotent (409 if already active). Returns {state}."
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
                "Room status above (state + participants). Each participant entry includes \
                 `poll_live` (true when a long-poll connection is actually parked right now); it \
                 flips false within ~10 s of the poll process dying or its connection dropping. \
                 `poll_live` is a SELF signal — read it on YOUR OWN entry. `poll_live: false` on \
                 your entry means no poll is parked right now: either your `cbc poll` is dead OR it \
                 is a healthy poll mid-backoff (a quiet poll sleeps up to ~60 s between parks). So \
                 treat it as a trigger to CONFIRM, not a verdict — check whether your `cbc poll` \
                 process is actually running, and relaunch only if it is gone. Do not judge a \
                 COUNTERPART from its `poll_live` (you can't see whether its process is alive); use \
                 `seconds_since_poll` for a counterpart instead. The legacy `seconds_since_poll` \
                 (seconds since last poll request arrived) and `stale` (only after 15 min) keep \
                 reading fresh for up to 15 min after a reaped poll — do not use them for your OWN \
                 liveness. Never subtract timestamps yourself. Then decide whether to join, send, \
                 wait, or close.",
            ),
            Err(e) => err_json(&e.to_string()),
        }
    }

    #[tool(
        description = "Re-read the WHOLE room (oldest-first markdown) WITHOUT consuming your read cursor. Your re-grounding tool: call before you summarize, decide, or reply after a /compact — never from memory. Returns {transcript_markdown}."
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
chatbotchat (CBC) is a local message bus: AI agents in different repos or sessions talk through shared rooms. A room is TWO-PARTY (you + exactly ONE counterpart) — for more counterparts, open a SEPARATE room each.

A bare token shaped `slug-YYYYMMDD-HHMM` is a CBC room id, NOT a command — there is no `/cbc-join`; ignore any leading slash. To engage: cbc_join_room(id, model) -> cbc_recap (read the whole thread) -> cbc_send a substantive reply -> start a background poll. Never read the opener and walk away.

The loop: cbc_open_room -> cbc_join_room + cbc_send your opening question -> surface the bare room id to your user ONCE, on its own line (no slash), and start a background poll -> ping-pong -> cbc_close (a CONSENSUS vote: the room closes only once the counterpart also closes). cbc_open_room only creates the room — it does NOT join or post.

ALWAYS BE POLLING while you are in a room — pacing is tied to PRESENCE, not just sending. The moment you open/join/send you owe the room a running `cbc poll <room> --model <m>` (a background task); start it, then end your turn — it long-polls and wakes you once with the message, waits through the counterpart joining, and holds through a quiet counterpart. RELAUNCH the poll on every wake BEFORE you compose. Pass one STABLE `--as <label>` (a role name) on join, send AND poll — same everywhere, one participant; never a session id. While a poll runs do NOT also call cbc_wait. Unless your user tells you to stop, never put the ball back in their court (\"tell me when they reply\") or kill a live poll while you remain in the room.

RE-GROUND BEFORE YOU DECIDE. Your own context goes stale and gets compacted; the room does not. Before you summarize, decide, reply after a /compact, or assert that something shipped/merged/deployed, call cbc_recap and re-verify external claims against live truth (git/gh) — never from memory.

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

/// The poll-launch instruction shared by the join/send `next` guidance. When the
/// caller anchored an explicit `as:` label, hand back the EXACT background-poll
/// command carrying that same `--as <label>` — the poll, the sends, and the
/// `.cbc` connections block must all register under one identity, which is what
/// the Stop-hook reconcile (`poll_matches`) counts. A poll under a different
/// label, or none, is exactly the every-turn "relaunch" nag this fix kills.
/// With no label the caller is keyed on a volatile session id; refuse to echo it
/// (it rotates on restart/fork/clear) and steer to a stable role label instead.
/// Takes only the caller's explicit arg — never the resolved instance — so a
/// concrete session id can never leak into copy-paste guidance.
fn poll_launch_clause(explicit: Option<&str>, room: &str) -> String {
    match explicit {
        Some(label) => format!(
            "launch `cbc poll {room} --model <m> --as {label}` as a background task and STAY \
             in the room — the SAME `--as {label}` you joined with, reused on every send and \
             in your `.cbc` connections block so all of them are one identity"
        ),
        None => format!(
            "launch your background poll as `cbc poll {room} --model <m> --as <label>`, where \
             <label> is a STABLE role name you reuse everywhere — `<repo>-worker-<feature>` \
             for a worker, `<repo>-orchestrator` for an orchestrator. Pass that SAME label on \
             join, send, poll, and your `.cbc` connections block. Do NOT poll under a bare \
             session id: it rotates on restart/fork/clear and splits you into duplicate rows \
             that nag every turn"
        ),
    }
}

/// The `next` guidance for a successful `cbc_join_room`. Pure so the identity
/// anchoring (the every-turn-nag fix) is unit-tested without a live client.
fn join_next(handle: &str, room: &str, recent: usize, explicit: Option<&str>) -> String {
    format!(
        "Joined as {handle}. {recent} recent message(s) are included in this response. \
         Joining COMMITS you to the room — reading the opener is NOT the end of your turn; \
         you owe a reply AND a running poll. Do this now, in order: (1) cbc_recap to re-read \
         the whole room (do not reply from the snippet or from memory); (2) cbc_send a \
         substantive reply (or, if you joined to speak first, your opening message), passing \
         the SAME `as:` label you joined with; (3) {poll}. Do NOT read-and-walk-away: never \
         end your turn (or resume other work) without a reply sent and a background poll \
         running. Where background tasks are unavailable, fall back to a manual cbc_wait \
         loop — but do not just leave.",
        poll = poll_launch_clause(explicit, room),
    )
}

/// The `next` guidance for a successful `cbc_send`. Pure (see `join_next`).
fn send_next(room: &str, explicit: Option<&str>) -> String {
    format!(
        "Posted. Now await the reply hands-free: {poll}. Fallback: call cbc_wait. When a \
         reply arrives, RELAUNCH the poll first (before you compose), then re-ground with \
         cbc_recap before you act — do not answer from memory. Never STOP a running poll \
         while you remain in the room. The counterpart may take a while; the poll holds for \
         about an hour of silence, so honor any retry_after and never bail to your user to \
         relay.",
        poll = poll_launch_clause(explicit, room),
    )
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
            // Single source of the per-status guidance (see `crate::wait_status`);
            // this surface and the CLI `cbc poll` output share it.
            let ws = WaitStatus::from_wire(status);
            let guidance = ws.guidance();
            // Only `paused_by_timeout` carries a `retry_after` hint server-side;
            // prepend the "stay quiet" lead-in there, where the guidance text reads
            // as its continuation. Every other status passes `retry_after: None`.
            if let (WaitStatus::PausedByTimeout, Some(s)) = (&ws, retry_after) {
                format!("Stay quiet ~{s}s, then {guidance}")
            } else {
                guidance.into_owned()
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

    // --- Footprint budgets (anti-regrowth guard) ---------------------------------
    //
    // The server instructions block and the tool definitions load into EVERY
    // session's context and re-cache every turn, whether CBC is used or not — they
    // are the fixed per-session token tax. The just-in-time `next` field on each
    // result carries the operational detail, so the upfront footprint must stay a
    // concise mental model (see the note above CBC_INSTRUCTIONS). These budgets fail
    // the build if that prose regrows, the way it did before this trim.

    #[test]
    fn cbc_instructions_within_budget() {
        assert!(
            CBC_INSTRUCTIONS.len() < 2000,
            "CBC_INSTRUCTIONS is {} chars — operational detail belongs in the per-call `next` \
             field, not the server-instructions block. Keep this a concise mental model.",
            CBC_INSTRUCTIONS.len()
        );
    }

    #[test]
    fn cbc_tool_descriptions_within_budget() {
        let tools = CbcMcp::tool_router().list_all();
        assert!(
            !tools.is_empty(),
            "expected the tool router to expose tools"
        );
        for t in &tools {
            let len = t.description.as_deref().map_or(0, str::len);
            assert!(
                len <= 500,
                "tool `{}` description is {len} chars (cap 500) — move protocol/voting/status \
                 detail to the per-call `next` field; keep the description to what it does, the \
                 non-obvious params, and what it returns.",
                t.name
            );
        }
    }

    /// Budgets the *prose* the tool definitions inject — every tool description plus
    /// every param `description` in the input schemas — excluding the fixed JSON
    /// structure schemars emits (`$schema`/`title`/type wrappers), which is not prose
    /// and only grows when real params are added. This is the thing that ballooned
    /// (a 2,723-char `cbc_wait` description, a 700-char identity param) and the thing
    /// the per-call `next` field exists to keep out of the upfront footprint.
    #[test]
    fn cbc_tool_prose_within_budget() {
        let tools = CbcMcp::tool_router().list_all();
        let mut total = 0usize;
        let mut tools_with_params = 0usize;
        for t in &tools {
            total += t.description.as_deref().map_or(0, str::len);
            if let Some(props) = t.input_schema.get("properties").and_then(|v| v.as_object()) {
                tools_with_params += 1;
                for schema in props.values() {
                    total += schema
                        .get("description")
                        .and_then(|v| v.as_str())
                        .map_or(0, str::len);
                }
            }
        }
        // Guard the guard: if the schema shape ever changes so `properties` stops
        // resolving, the param-prose sum would silently collapse to 0 and this
        // budget would quietly weaken to descriptions-only. Most cbc tools take
        // params (room_id, model, …), so at least one must expose `properties`.
        assert!(
            tools_with_params > 0,
            "no tool exposed an input_schema `properties` object — the param-prose \
             budget has silently degraded; the schema shape likely changed."
        );
        assert!(
            total <= 4000,
            "tool + param description prose is {total} chars (cap 4000) — move protocol/voting/\
             status detail to the per-call `next` field; keep descriptions to what/params/returns."
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
            instructions.contains("surface the bare room id")
                && instructions.contains("background poll"),
            "instructions must teach the surface-id-and-yield path; got: {instructions}"
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
        // The 2-party constraint: a room holds exactly ONE counterpart, so a third
        // service needs its own pairwise room (there is no multi-party mode yet).
        let lower = instructions.to_lowercase();
        assert!(
            lower.contains("two-party") && lower.contains("separate room"),
            "instructions must teach the 2-party cap and the separate-room workaround; got: {instructions}"
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

    // ── identity anchoring (the every-turn-nag fix) ───────────────────────────
    //
    // The Stop-hook reconcile counts a poll only when its `--as <id>` matches the
    // identity declared in the `.cbc` connections block (`hook::poll_matches`).
    // So the join/send `next` guidance MUST hand the agent a poll command carrying
    // the SAME label it joined with — echoing `resp.handle` (a server-minted
    // `repo-model-4hex`) or telling it to "omit `--as`" is what split declared-vs-
    // polled identity and nagged every turn. These pin that contract.

    #[test]
    fn poll_launch_clause_echoes_the_anchored_label_verbatim() {
        let s = poll_launch_clause(Some("mvp-engine-orchestrator"), "room-20260625-1000");
        assert!(
            s.contains("cbc poll room-20260625-1000 --model <m> --as mvp-engine-orchestrator"),
            "an anchored label must be handed back as the exact poll command; got: {s}"
        );
    }

    #[test]
    fn poll_launch_clause_without_a_label_steers_to_a_role_label_not_a_session_id() {
        let s = poll_launch_clause(None, "room-20260625-1000");
        // Uses the literal `<label>` placeholder + the role-label conventions —
        // never a concrete value to copy. The helper is handed only the caller's
        // explicit arg, so a resolved session id cannot leak here by construction.
        assert!(s.contains("--as <label>"), "got: {s}");
        assert!(
            s.contains("<repo>-worker-<feature>") && s.contains("<repo>-orchestrator"),
            "must name the stable role-label conventions; got: {s}"
        );
        assert!(
            s.contains("rotates") || s.contains("session id"),
            "must warn against pinning a volatile session id; got: {s}"
        );
    }

    #[test]
    fn join_next_anchors_the_poll_to_the_joined_label() {
        let s = join_next(
            "mvp-engine-opus48-2c67",
            "room-20260625-1000",
            3,
            Some("mvp-engine-orchestrator"),
        );
        assert!(s.contains("Joined as mvp-engine-opus48-2c67"), "got: {s}");
        assert!(
            s.contains("--as mvp-engine-orchestrator"),
            "the join next must hand back the anchored poll command; got: {s}"
        );
        // A regression back to "omit `--as`" is exactly what recreates the nag.
        assert!(
            !s.contains("omit `--as`"),
            "join must no longer tell the agent to omit --as; got: {s}"
        );
    }

    #[test]
    fn join_next_without_a_label_emits_corrective_guidance() {
        let s = join_next("mvp-engine-opus48-2c67", "room-20260625-1000", 0, None);
        assert!(
            s.contains("--as <label>") && s.contains("<repo>-orchestrator"),
            "got: {s}"
        );
        assert!(!s.contains("omit `--as`"), "got: {s}");
    }

    #[test]
    fn send_next_anchors_the_poll_to_the_label() {
        let s = send_next("room-20260625-1000", Some("mvp-engine-orchestrator"));
        assert!(
            s.contains("cbc poll room-20260625-1000 --model <m> --as mvp-engine-orchestrator"),
            "the send next must hand back the anchored poll command; got: {s}"
        );
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

        // Keep-waiting-not-a-hand-back bucket. The guidance deliberately NO LONGER
        // tells the agent to end its turn here — that "surface and yield" model was
        // the hand-back bug; the background poll waits through the join instead.
        let alone = wait_next(&WaitResponse::Timeout {
            status: "awaiting_counterpart".into(),
            retry_after: None,
        });
        assert!(
            alone.contains("not joined")
                && alone.contains("NOT a stop")
                && alone.contains("Keep waiting"),
            "awaiting_counterpart must say keep-waiting, not hand back; got: {alone}"
        );

        // A quiet counterpart is also held, not stopped.
        let stale = wait_next(&WaitResponse::Timeout {
            status: "counterpart_stale".into(),
            retry_after: None,
        });
        assert!(
            stale.contains("NOT a stop") && stale.contains("slower cadence"),
            "counterpart_stale must say hold-the-line, not stop; got: {stale}"
        );

        // Terminal bucket.
        let closed = wait_next(&WaitResponse::Timeout {
            status: "closed".into(),
            retry_after: None,
        });
        assert!(closed.contains("Stop polling"), "got: {closed}");
    }
}
