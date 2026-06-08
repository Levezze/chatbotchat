use anyhow::Context;
use chatbotchat_client::HttpClient;
use chatbotchat_protocol::{MessageView, RoomTranscript, WaitResponse};
use clap::{Parser, Subcommand, ValueEnum};
use std::time::Duration;

mod context;
mod install;
mod mcp;
mod settings;

/// Output format for `cbc show`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ShowFormat {
    /// Human-readable markdown transcript (default).
    Markdown,
    /// The raw transcript DTO as JSON, for scripting.
    Json,
}

/// `cbc` — the agent-facing client for chatbotchat. Talks to the local daemon
/// over HTTP. Same surface is exposed to MCP via the `mcp` subcommand.
#[derive(Debug, Parser)]
#[command(name = "cbc", version)]
struct Cli {
    /// Base URL of the chatbotchat daemon.
    #[arg(
        long,
        env = "CBC_SERVER",
        default_value = "http://127.0.0.1:8484",
        global = true
    )]
    server: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Open a new room and print its id + share line.
    Open {
        /// Subject / topic of the room.
        subject: String,
        /// Hard cap: max messages before sends are refused (default 10).
        #[arg(long)]
        hard_cap: Option<u32>,
        /// Soft cap: consecutive autonomous turns before the user is surfaced (default 4).
        #[arg(long)]
        soft_cap: Option<u32>,
    },
    /// Join a room as a participant; repo and cwd are auto-detected.
    Join {
        /// Room id to join.
        room_id: String,
        /// Self-declared model name, e.g. opus47, sonnet46, codex53.
        #[arg(long)]
        model: String,
        /// Optional identity label. Auto-derived per session when omitted; two
        /// agents in the same repo+model+dir must pass distinct values. Reuse
        /// the same label from another terminal/client/dir to resume or hand off.
        #[arg(long = "as")]
        identity: Option<String>,
        /// Optional friendly display name shown in `cbc list`/`status` (e.g.
        /// "concierge-agent"). Cosmetic only — does not affect identity. A
        /// re-join updates it.
        #[arg(long = "nick")]
        nickname: Option<String>,
    },
    /// Post a message to a room; repo and cwd are auto-detected.
    Send {
        /// Room id to post into.
        room_id: String,
        /// Self-declared model name (your identity; e.g. opus47).
        #[arg(long)]
        model: String,
        /// Message body.
        body: String,
        /// Optional recipient handle; omit to broadcast to all participants.
        #[arg(long)]
        to: Option<String>,
        /// Fold your user's input into this turn; resets the soft-cap counter.
        #[arg(long)]
        human: bool,
        /// Optional identity label (see `join --as`); pass the value you joined with.
        #[arg(long = "as")]
        identity: Option<String>,
    },
    /// Emit a sentinel (out-of-band signal) to a room; repo and cwd are auto-detected.
    Signal {
        /// Room id to signal.
        room_id: String,
        /// Self-declared model name (your identity; e.g. opus47).
        #[arg(long)]
        model: String,
        /// Signal type: waiting_user or fold.
        #[arg(long = "type")]
        signal_type: String,
        /// Severity for waiting_user: low, med, or high.
        #[arg(long)]
        severity: Option<String>,
        /// The question you are asking your user (waiting_user only).
        #[arg(long)]
        question: Option<String>,
        /// Optional identity label (see `join --as`); pass the value you joined with.
        #[arg(long = "as")]
        identity: Option<String>,
    },
    /// Long-poll for the next message addressed to you (or broadcast).
    Wait {
        /// Room id to long-poll.
        room_id: String,
        /// Self-declared model name (your identity; e.g. opus47).
        #[arg(long)]
        model: String,
        /// Optional identity label (see `join --as`); pass the value you joined with.
        #[arg(long = "as")]
        identity: Option<String>,
    },
    /// Background-friendly wait loop: long-poll in bounded chunks until a real
    /// event arrives — a message, a terminal state, or a state needing a
    /// decision — then print it and exit. Loops internally on
    /// `paused_by_timeout` (honoring any `retry_after`), so the caller is no
    /// longer the polling loop. Designed to run as a background task (e.g. via
    /// `/loop`). Pass the `--as` you joined with so the poller shares one identity
    /// with your in-session sends — the poller owns the read cursor, so do NOT
    /// also call `cbc wait` on the same identity while it runs.
    Poll {
        /// Room id to poll.
        room_id: String,
        /// Self-declared model name (your identity; e.g. opus47).
        #[arg(long)]
        model: String,
        /// Identity label (see `join --as`). REQUIRED: the poller owns the read
        /// cursor, so it must share one identity with your MCP join/send. Relying
        /// on the ambient per-process identity would split the cursor when the
        /// session id is absent or has churned — pass the value you joined with.
        #[arg(long = "as")]
        identity: String,
        /// Per-call long-poll cap (seconds): each underlying wait blocks up to
        /// this, and the loop re-waits on a timeout. Clamped to [1, 590] (a 0
        /// would make the server return instantly and the loop spin). Default 50.
        #[arg(long, default_value_t = 50)]
        poll_cap_secs: u32,
        /// Give up after this many consecutive empty polls (0 = never give up).
        /// Mainly for tests and bounded runs; background callers leave it 0.
        #[arg(long, default_value_t = 0)]
        max_polls: u32,
        /// Seconds to wait between retries after a transient `wait` error (a
        /// daemon restart / dropped long-poll). Tuning/test knob; default 2.
        #[arg(long, default_value_t = 2)]
        error_backoff_secs: u64,
        /// Seconds between re-checks while the counterpart has not joined yet
        /// (`awaiting_counterpart` returns instantly, so the loop backs off on its
        /// own cadence). Joins are human-paced; default 5.
        #[arg(long, default_value_t = 5)]
        join_backoff_secs: u64,
        /// Give up if no counterpart joins within this many seconds. Lets an
        /// initiator launch the poll right after surfacing the room id and go
        /// hands-free, while a never-pasted id still terminates. `0` means "the
        /// maximum safe window". Clamped below the server's 15-min sole-participant
        /// stale threshold: while alone the poller cannot refresh its own liveness
        /// (the server short-circuits before the liveness touch), so out-waiting
        /// that threshold would make a late-joining counterpart see it as stale.
        /// Default 300 (5 min).
        #[arg(long, default_value_t = 300)]
        max_join_wait_secs: u64,
        /// Emit the delivered event as JSON instead of human-readable text.
        #[arg(long)]
        json: bool,
    },
    /// List rooms (newest-first). Hides archived unless `--all` or `--state archived`.
    List {
        /// Include archived rooms (mutually exclusive with `--state`).
        #[arg(long, conflicts_with = "state")]
        all: bool,
        /// Filter to a single state (active, idle, paused, stale, closed, archived).
        #[arg(long)]
        state: Option<String>,
    },
    /// Show a room's full transcript (metadata, caps, participants, all messages).
    Show {
        /// Room id to show.
        room_id: String,
        /// Output format.
        #[arg(long, value_enum, default_value_t = ShowFormat::Markdown)]
        format: ShowFormat,
    },
    /// Show the status of an existing room.
    Status {
        /// Room id.
        room_id: String,
    },
    /// Explicitly close a room; repo and cwd are auto-detected.
    Close {
        /// Room id to close.
        room_id: String,
        /// Self-declared model name (your identity; e.g. opus47).
        #[arg(long)]
        model: String,
        /// Optional identity label (see `join --as`); pass the value you joined with.
        #[arg(long = "as")]
        identity: Option<String>,
        /// Force the room closed immediately, bypassing consensus. Without this,
        /// `close` is a vote: the room closes only once a quorum of live
        /// participants have voted (the counterpart sees `close_proposed` and can
        /// agree or keep talking). Use `--force` to unilaterally end a room.
        #[arg(long)]
        force: bool,
    },
    /// Pause a room; repo and cwd are auto-detected.
    Pause {
        /// Room id to pause.
        room_id: String,
        /// Self-declared model name (your identity; e.g. opus47).
        #[arg(long)]
        model: String,
        /// Optional reason, recorded in the room's audit log.
        #[arg(long)]
        reason: Option<String>,
        /// Optional identity label (see `join --as`); pass the value you joined with.
        #[arg(long = "as")]
        identity: Option<String>,
    },
    /// Wake a paused (or idle) room back to active; repo and cwd are auto-detected.
    Wake {
        /// Room id to wake.
        room_id: String,
        /// Self-declared model name (your identity; e.g. opus47).
        #[arg(long)]
        model: String,
        /// Optional identity label (see `join --as`); pass the value you joined with.
        #[arg(long = "as")]
        identity: Option<String>,
    },
    /// Run as an MCP stdio server (wired in a later cycle).
    Mcp,
    /// Install the always-on launchd agent for the daemon (macOS) and load it.
    InstallDaemon {
        /// Port the daemon should listen on (baked into the launchd agent).
        #[arg(long, default_value_t = 8484)]
        port: u16,
        /// Directory to write the agent into. Defaults to ~/Library/LaunchAgents.
        #[arg(long)]
        plist_dir: Option<std::path::PathBuf>,
    },
    /// Grant the chatbotchat MCP tools standing auto-approval in Claude Code's
    /// user settings (~/.claude/settings.json), so the bus stops stalling for
    /// per-call approval. Idempotent; backs up the file before editing.
    AllowTools,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let client = HttpClient::new(&cli.server);

    match cli.command {
        Command::Open {
            subject,
            hard_cap,
            soft_cap,
        } => {
            let resp = client
                .open_room(&subject, hard_cap, soft_cap)
                .await
                .context("opening room")?;
            println!("Room:  {}", resp.room_id);
            println!("Share: {}", resp.share_line);
            println!();
            println!("Tell the other agent: {}", resp.share_line);
        }
        Command::Join {
            room_id,
            model,
            identity,
            nickname,
        } => {
            let repo = context::detect_repo();
            let cwd = context::detect_cwd();
            let instance = context::detect_instance(identity.as_deref());
            let resp = client
                .join_room(
                    &room_id,
                    &repo,
                    &model,
                    &cwd,
                    &instance,
                    nickname.as_deref(),
                )
                .await
                .context("joining room")?;
            println!("Handle:  {}", resp.handle);
            println!("Resumed: {}", resp.resumed);
            println!("State:   {}", resp.room_state);
        }
        Command::Send {
            room_id,
            model,
            body,
            to,
            human,
            identity,
        } => {
            let repo = context::detect_repo();
            let cwd = context::detect_cwd();
            let instance = context::detect_instance(identity.as_deref());
            let resp = client
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
                .context("sending message")?;
            println!("Sent: seq {}", resp.seq);
        }
        Command::Signal {
            room_id,
            model,
            signal_type,
            severity,
            question,
            identity,
        } => {
            let repo = context::detect_repo();
            let cwd = context::detect_cwd();
            let instance = context::detect_instance(identity.as_deref());
            let resp = client
                .signal(
                    &room_id,
                    &repo,
                    &model,
                    &cwd,
                    &instance,
                    &signal_type,
                    severity.as_deref(),
                    question.as_deref(),
                )
                .await
                .context("sending signal")?;
            println!("Signal sent: seq {}", resp.seq);
        }
        Command::Wait {
            room_id,
            model,
            identity,
        } => {
            let repo = context::detect_repo();
            let cwd = context::detect_cwd();
            let instance = context::detect_instance(identity.as_deref());
            let resp = client
                .wait(&room_id, &repo, &model, &cwd, &instance, None)
                .await
                .context("waiting for message")?;
            match resp {
                WaitResponse::Message {
                    message,
                    surface_to_user,
                    retry_after,
                    room_state,
                } => {
                    print_message_human(&message, surface_to_user, room_state.as_deref());
                    print_backoff(retry_after);
                }
                WaitResponse::Timeout {
                    status,
                    retry_after,
                } => {
                    println!("{status}");
                    print_backoff(retry_after);
                }
            }
        }
        Command::Poll {
            room_id,
            model,
            identity,
            poll_cap_secs,
            max_polls,
            error_backoff_secs,
            join_backoff_secs,
            max_join_wait_secs,
            json,
        } => {
            let repo = context::detect_repo();
            let cwd = context::detect_cwd();
            let instance = context::detect_instance(Some(&identity));
            // Clamp the cap to [1, 590] (mirrors the MCP cbc_wait clamp): a 0 cap
            // makes the server return instantly with no retry_after, which would
            // otherwise spin the loop; 590 stays under the server's 600s cap.
            let poll_cap_secs = poll_cap_secs.clamp(1, 590);
            // Keep the join wait under the server's sole-participant stale window
            // (see SAFE_JOIN_WAIT_CAP_SECS); 0 means "the max safe window".
            let max_join_wait_secs = effective_max_join_wait(max_join_wait_secs);
            poll_until_event(
                &client,
                &room_id,
                &repo,
                &model,
                &cwd,
                &instance,
                poll_cap_secs,
                max_polls,
                error_backoff_secs,
                join_backoff_secs,
                max_join_wait_secs,
                json,
            )
            .await
            .context("polling room")?;
        }
        Command::List { all, state } => {
            let rooms = client
                .list_rooms(state.as_deref(), all)
                .await
                .context("listing rooms")?;
            if rooms.is_empty() {
                println!("(no rooms)");
            } else {
                for r in &rooms {
                    let plural = if r.participant_count == 1 { "" } else { "s" };
                    println!(
                        "{}  [{}]  {} participant{}  {}",
                        r.room_id, r.state, r.participant_count, plural, r.subject
                    );
                }
            }
        }
        Command::Show { room_id, format } => {
            let transcript = client
                .transcript(&room_id)
                .await
                .context("fetching transcript")?;
            match format {
                ShowFormat::Json => {
                    let json = serde_json::to_string_pretty(&transcript)
                        .context("serializing transcript")?;
                    println!("{json}");
                }
                ShowFormat::Markdown => print!("{}", render_transcript_markdown(&transcript)),
            }
        }
        Command::Status { room_id } => {
            let status = client.status(&room_id).await.context("fetching status")?;
            println!("Room:    {}", status.id);
            println!("Subject: {}", status.subject);
            println!("State:   {}", status.state);
            println!("Started: {}", status.started_at);
            println!("Active:  {}", status.last_activity_at);
            if status.participants.is_empty() {
                println!("Participants: (none)");
            } else {
                println!("Participants:");
                for p in &status.participants {
                    println!(
                        "  - {} ({} @ {})",
                        participant_label(&p.nickname, &p.handle),
                        p.model,
                        p.cwd
                    );
                }
            }
        }
        Command::Close {
            room_id,
            model,
            identity,
            force,
        } => {
            let repo = context::detect_repo();
            let cwd = context::detect_cwd();
            let instance = context::detect_instance(identity.as_deref());
            let resp = client
                .close(&room_id, &repo, &model, &cwd, &instance, force)
                .await
                .context("closing room")?;
            // `status` distinguishes a completed close from a pending proposal;
            // older servers omit it, so fall back to the bare state.
            match resp.status.as_deref() {
                Some("close_proposed") => {
                    let have = resp.votes.unwrap_or(0);
                    let need = resp.needed.unwrap_or(0);
                    println!(
                        "Close proposed ({have}/{need}) — waiting for the other agent to agree. \
                         They will see it on their next wait and can agree (cbc close) or keep \
                         talking (which cancels the proposal)."
                    );
                }
                _ => println!("State: {}", resp.state),
            }
        }
        Command::Pause {
            room_id,
            model,
            reason,
            identity,
        } => {
            let repo = context::detect_repo();
            let cwd = context::detect_cwd();
            let instance = context::detect_instance(identity.as_deref());
            let resp = client
                .pause(&room_id, &repo, &model, &cwd, &instance, reason.as_deref())
                .await
                .context("pausing room")?;
            println!("State: {}", resp.state);
        }
        Command::Wake {
            room_id,
            model,
            identity,
        } => {
            let repo = context::detect_repo();
            let cwd = context::detect_cwd();
            let instance = context::detect_instance(identity.as_deref());
            let resp = client
                .wake(&room_id, &repo, &model, &cwd, &instance)
                .await
                .context("waking room")?;
            println!("State: {}", resp.state);
        }
        Command::Mcp => {
            mcp::run(client).await.context("running MCP server")?;
        }
        Command::InstallDaemon { port, plist_dir } => {
            install::run(port, plist_dir).context("installing the launchd daemon")?;
        }
        Command::AllowTools => {
            let path = settings::settings_path()?;
            match settings::apply_allow_rule(&path) {
                Ok(outcome) => settings::print_allow_outcome(&path, &outcome),
                Err(e) => {
                    // Degrade, don't crash: a hand-maintained settings file we
                    // can't parse must not be clobbered — print the manual fix.
                    eprintln!("Could not edit {} automatically: {e:#}", path.display());
                    settings::print_manual_snippet();
                }
            }
        }
    }

    Ok(())
}

/// CLI analog of the MCP `next` field: the re-ground discipline a waking agent
/// must follow before it replies, so it answers from the room and live facts
/// rather than stale context (the failure that motivated `cbc poll`).
const POLL_REGROUND_NEXT: &str =
    "next: re-ground before you reply — call cbc_recap to re-read the whole room, and re-verify \
     any status claims against git/gh. Do not recap from memory.";

/// Largest window a sole `cbc poll` may wait for a counterpart to join. Kept
/// below the server's `lifecycle::GHOST_AFTER` (15 min) on purpose: while alone,
/// the poll short-circuits with `awaiting_counterpart` *before* the server's
/// per-wait liveness touch, so the poller cannot refresh its own `last_poll_at`.
/// A poller that out-waited the stale threshold would be seen as `counterpart_stale`
/// by the counterpart that finally joins. The full fix (refreshing liveness while
/// alone) requires a server change and is tracked separately.
const SAFE_JOIN_WAIT_CAP_SECS: u64 = 840; // < GHOST_AFTER (900s)
                                          // Compile-time guard: the cap must stay under the server's 15-min stale window.
const _: () = assert!(SAFE_JOIN_WAIT_CAP_SECS < 900);

/// Resolve the effective join-wait bound: `0` ("max safe window") and any
/// over-large value collapse to `SAFE_JOIN_WAIT_CAP_SECS`.
fn effective_max_join_wait(secs: u64) -> u64 {
    if secs == 0 || secs > SAFE_JOIN_WAIT_CAP_SECS {
        SAFE_JOIN_WAIT_CAP_SECS
    } else {
        secs
    }
}

/// The deterministic, background-friendly poll loop behind `cbc poll`. It
/// long-polls in `poll_cap_secs` chunks and returns only on a meaningful event —
/// a delivered message, a terminal room state, or a state needing a decision
/// (awaiting_counterpart / close_proposed). It loops internally on
/// `paused_by_timeout`, honoring any server `retry_after`, so the agent driving
/// it sees a single wake carrying the payload rather than a stream of empty
/// polls. Transient `wait` errors (a daemon restart, a dropped long-poll) are
/// retried with a short backoff; the loop fails only after several in a row.
#[allow(clippy::too_many_arguments)]
async fn poll_until_event(
    client: &HttpClient,
    room_id: &str,
    repo: &str,
    model: &str,
    cwd: &str,
    instance: &str,
    poll_cap_secs: u32,
    max_polls: u32,
    error_backoff_secs: u64,
    join_backoff_secs: u64,
    max_join_wait_secs: u64,
    json: bool,
) -> anyhow::Result<()> {
    const MAX_CONSECUTIVE_ERRORS: u32 = 5;
    // Floor between re-waits so the loop can never busy-spin even if a wait
    // returns `paused_by_timeout` instantly with no retry_after (belt-and-
    // suspenders alongside the poll_cap_secs clamp at the call site).
    const MIN_REPOLL_SLEEP_SECS: u64 = 1;
    // Same floor for the join wait: `awaiting_counterpart` also returns instantly,
    // so a 0 backoff must not turn the pre-join wait into a tight loop.
    const MIN_JOIN_BACKOFF_SECS: u64 = 1;
    let join_backoff_secs = join_backoff_secs.max(MIN_JOIN_BACKOFF_SECS);
    let mut empty_polls: u32 = 0;
    let mut consecutive_errors: u32 = 0;
    // Accumulated seconds spent waiting for a counterpart to join, so the loop can
    // give up at `max_join_wait_secs` instead of waiting on a never-pasted id.
    let mut join_wait_secs: u64 = 0;

    loop {
        let resp = match client
            .wait(room_id, repo, model, cwd, instance, Some(poll_cap_secs))
            .await
        {
            Ok(resp) => {
                consecutive_errors = 0;
                resp
            }
            Err(e) => {
                consecutive_errors += 1;
                if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                    return Err(anyhow::anyhow!(
                        "poll: giving up after {consecutive_errors} consecutive wait errors: {e}"
                    ));
                }
                tokio::time::sleep(Duration::from_secs(error_backoff_secs)).await;
                continue;
            }
        };

        match resp {
            WaitResponse::Message {
                message,
                surface_to_user,
                retry_after,
                room_state,
            } => {
                emit_poll_message(
                    &message,
                    surface_to_user,
                    retry_after,
                    room_state.as_deref(),
                    json,
                );
                return Ok(());
            }
            // The only keep-waiting status: nothing addressed to us arrived yet.
            WaitResponse::Timeout {
                status,
                retry_after,
            } if status == "paused_by_timeout" => {
                empty_polls += 1;
                if max_polls != 0 && empty_polls >= max_polls {
                    emit_poll_giveup(empty_polls, json);
                    return Ok(());
                }
                // Honor the server's backoff hint, but never sleep less than the
                // floor — a fast/zero-cap return must not turn into a tight loop.
                let sleep_secs =
                    (retry_after.map(u64::from).unwrap_or(0)).max(MIN_REPOLL_SLEEP_SECS);
                tokio::time::sleep(Duration::from_secs(sleep_secs)).await;
            }
            // The counterpart has not joined yet. The server returns this instantly
            // (no park). Unlike `cbc wait` — which hands back to the user here — the
            // background poll waits THROUGH the join: the agent already surfaced the
            // room id before launching this, so we back off and re-check, waking the
            // agent only when the counterpart actually arrives and speaks. Bounded by
            // `max_join_wait_secs` so a never-shared id still terminates.
            WaitResponse::Timeout { status, .. } if status == "awaiting_counterpart" => {
                if max_join_wait_secs != 0 && join_wait_secs >= max_join_wait_secs {
                    emit_poll_join_giveup(join_wait_secs, json);
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_secs(join_backoff_secs)).await;
                join_wait_secs += join_backoff_secs;
            }
            // Everything else needs the agent: a terminal state, or a decision.
            WaitResponse::Timeout { status, .. } => {
                emit_poll_status(&status, json);
                return Ok(());
            }
        }
    }
}

/// Print a delivered message for `cbc poll` (human or JSON), followed by the
/// re-ground instruction the waking agent must honor before replying.
fn emit_poll_message(
    message: &MessageView,
    surface_to_user: bool,
    retry_after: Option<u32>,
    room_state: Option<&str>,
    json: bool,
) {
    if json {
        let payload = serde_json::json!({
            "event": "message",
            "message": message,
            "surface_to_user": surface_to_user,
            "retry_after": retry_after,
            "room_state": room_state,
            "next": POLL_REGROUND_NEXT,
        });
        println!("{payload}");
        return;
    }
    print_message_human(message, surface_to_user, room_state);
    // Surface the backoff hint (set for a delivered waiting_user sentinel / busy
    // counterpart) the same way `cbc wait` does — dropping it would let a waking
    // agent re-poll immediately instead of honoring the requested interval.
    print_backoff(retry_after);
    println!("{POLL_REGROUND_NEXT}");
}

/// Print a non-message poll outcome (terminal state or a decision-needed state)
/// plus the action the agent should take, mirroring the MCP `wait_next` guidance.
fn emit_poll_status(status: &str, json: bool) {
    if json {
        let payload = serde_json::json!({
            "event": "status",
            "status": status,
            "next": poll_status_guidance(status),
        });
        println!("{payload}");
        return;
    }
    println!("{status}");
    println!("{}", poll_status_guidance(status));
}

/// Print the give-up outcome when a bounded poll (`--max-polls`) exhausts its
/// budget with no message. The conversation is still alive — the caller relaunches.
fn emit_poll_giveup(polls: u32, json: bool) {
    if json {
        let payload = serde_json::json!({
            "event": "gave_up",
            "polls": polls,
            "next": "Gave up after the poll bound with no message; the conversation is still alive — relaunch cbc poll to keep waiting.",
        });
        println!("{payload}");
        return;
    }
    println!(
        "Gave up after {polls} empty poll(s) with no message. The conversation is still alive — \
         relaunch cbc poll to keep waiting."
    );
}

/// Print the give-up outcome when no counterpart joins within the join-wait bound.
/// The room id was already surfaced; the agent should re-surface it and confirm
/// the user shared it, then relaunch the poll.
fn emit_poll_join_giveup(waited_secs: u64, json: bool) {
    const GUIDANCE: &str = "No counterpart joined yet. Re-surface the room id to your user and \
         confirm they pasted it to the other agent, then relaunch cbc poll to keep waiting.";
    if json {
        let payload = serde_json::json!({
            "event": "awaiting_counterpart_gave_up",
            "waited_secs": waited_secs,
            "next": GUIDANCE,
        });
        println!("{payload}");
        return;
    }
    println!("No counterpart joined after waiting {waited_secs}s.");
    println!("{GUIDANCE}");
}

/// The action a waking agent should take for each non-message poll outcome,
/// kept in lockstep with the MCP `wait_next` guidance so both surfaces agree.
fn poll_status_guidance(status: &str) -> &'static str {
    match status {
        // Retained only for parity with the MCP `wait_next` mapping. The poll loop
        // intercepts `awaiting_counterpart` (waits through it, then `emit_poll_join_giveup`)
        // before this is ever reached, so this arm is unreachable via `cbc poll`.
        "awaiting_counterpart" => {
            "The other agent has not joined yet. Surface the room id to your user and end your \
             turn; resume polling once they join. Do not abandon the room."
        }
        "close_proposed" => {
            "The other agent proposed closing the room. If you agree the conversation is done, \
             call cbc_close to agree; if you have more to say, cbc_send (which cancels the \
             proposal)."
        }
        "counterpart_stale" => {
            "The other agent has gone silent (>15 min with no poll). Stop polling and tell your \
             user the counterpart is unresponsive."
        }
        "paused" => "The room is paused. Stop polling — it needs an explicit cbc_wake to resume.",
        "closed" | "archived" => "The room is over. Stop polling.",
        _ => "Stop polling unless you know how to resume.",
    }
}

/// Render a delivered message the way `cbc wait` / `cbc poll` surface it: a
/// terminal-room note when drained from a non-active room, the from/to header,
/// the body (or a sentinel's type + the question it is asking its user), and the
/// soft-cap consult prompt when the cap is reached.
fn print_message_human(message: &MessageView, surface_to_user: bool, room_state: Option<&str>) {
    if let Some(rs) = room_state {
        println!(
            "[room {rs}] delivered from a {rs} room — read it; you cannot just reply. Keep \
             waiting/polling to drain any remaining backlog until you get status {rs}."
        );
    }
    println!("From: {}", message.from);
    println!("To:   {}", message.to.as_deref().unwrap_or("all"));
    // A sentinel (type != "msg") carries no body; surface its type and the
    // question the other agent is asking its user instead.
    if message.msg_type != "msg" {
        match &message.severity {
            Some(sev) => println!("Signal: {} ({sev})", message.msg_type),
            None => println!("Signal: {}", message.msg_type),
        }
        if let Some(q) = &message.question_text {
            println!("Asking its user: {q}");
        }
    } else {
        println!("Body: {}", message.body);
    }
    if surface_to_user {
        println!();
        println!(
            "[soft cap] Consecutive autonomous turns hit the soft cap. Consult your user and \
             send the next turn with --human."
        );
    }
}

/// Render a room transcript as human-readable markdown for `cbc show`. Sentinel
/// messages (type != `msg`) are rendered like the `wait` handler surfaces them:
/// the signal type, its severity, and the question the other agent is asking its
/// user — a `msg` shows its body instead.
/// A participant's display label: its nickname with the handle in brackets when
/// a nickname is set (so identity stays visible), else just the handle.
fn participant_label(nickname: &Option<String>, handle: &str) -> String {
    match nickname {
        Some(n) => format!("{n} [{handle}]"),
        None => handle.to_string(),
    }
}

/// `pub(crate)` so the MCP `cbc_recap` tool (in `mcp.rs`) can reuse the exact
/// same rendering `cbc show` uses — one transcript renderer, two surfaces.
pub(crate) fn render_transcript_markdown(t: &RoomTranscript) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", t.subject));
    out.push_str(&format!("- Room: {}\n", t.id));
    out.push_str(&format!("- State: {}\n", t.state));
    out.push_str(&format!("- Started: {}\n", t.started_at));
    out.push_str(&format!("- Last activity: {}\n", t.last_activity_at));
    out.push_str(&format!(
        "- Caps: hard {}/{}, soft {}/{}\n",
        t.hard_cap_count, t.hard_cap, t.soft_cap_consecutive, t.soft_cap
    ));

    out.push_str("\n## Participants\n");
    if t.participants.is_empty() {
        out.push_str("- (none)\n");
    } else {
        for p in &t.participants {
            out.push_str(&format!(
                "- {} ({} @ {})\n",
                participant_label(&p.nickname, &p.handle),
                p.model,
                p.cwd
            ));
        }
    }

    out.push_str("\n## Messages\n");
    if t.messages.is_empty() {
        out.push_str("- (none)\n");
    } else {
        for m in &t.messages {
            out.push_str(&render_message_markdown(m));
        }
    }
    out
}

fn render_message_markdown(m: &MessageView) -> String {
    let to = m.to.as_deref().unwrap_or("all");
    if m.msg_type != "msg" {
        let sev = match &m.severity {
            Some(s) => format!(" ({s})"),
            None => String::new(),
        };
        let question = match &m.question_text {
            Some(q) => format!(" — asks its user: {q}"),
            None => String::new(),
        };
        format!(
            "- [{}] **{}** → {to} _signal: {}{sev}_{question}\n",
            m.seq, m.from, m.msg_type
        )
    } else {
        format!("- [{}] **{}** → {to}: {}\n", m.seq, m.from, m.body)
    }
}

/// Surface the polling backoff hint, when present. The counterpart is engaged —
/// either paused behind a `waiting_user` sentinel (consulting its user) or
/// composing a reply it has already read (server-inferred from the read cursor) —
/// so tell the agent to space out its re-polls rather than hammering the room.
fn print_backoff(retry_after: Option<u32>) {
    if let Some(secs) = retry_after {
        println!(
            "[backoff] Counterpart is engaged (paused or composing); wait ~{secs}s before re-polling."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_wait_is_clamped_below_the_stale_threshold() {
        // 0 ("max safe window") and any over-large value collapse to the cap, so a
        // sole poller can never out-wait the server's 15-min stale threshold.
        assert_eq!(effective_max_join_wait(0), SAFE_JOIN_WAIT_CAP_SECS);
        assert_eq!(effective_max_join_wait(100_000), SAFE_JOIN_WAIT_CAP_SECS);
        // Reasonable explicit values pass through unchanged.
        assert_eq!(effective_max_join_wait(300), 300);
        assert_eq!(
            effective_max_join_wait(SAFE_JOIN_WAIT_CAP_SECS),
            SAFE_JOIN_WAIT_CAP_SECS
        );
    }
}
