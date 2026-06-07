use anyhow::Context;
use chatbotchat_client::HttpClient;
use chatbotchat_protocol::{MessageView, RoomTranscript, WaitResponse};
use clap::{Parser, Subcommand, ValueEnum};

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
                    if let Some(rs) = &room_state {
                        println!(
                            "[room {rs}] delivered from a {rs} room — read it; you cannot just \
                             reply. Keep calling wait to drain any backlog until you get status \
                             {rs}."
                        );
                    }
                    println!("From: {}", message.from);
                    println!("To:   {}", message.to.as_deref().unwrap_or("all"));
                    // A sentinel (type != "msg") carries no body; surface its type
                    // and the question the other agent is asking its user instead.
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
                            "[soft cap] Consecutive autonomous turns hit the soft cap. \
                             Consult your user and send the next turn with --human."
                        );
                    }
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

fn render_transcript_markdown(t: &RoomTranscript) -> String {
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
