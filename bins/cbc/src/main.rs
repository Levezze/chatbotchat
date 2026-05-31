use anyhow::Context;
use chatbotchat_client::HttpClient;
use chatbotchat_protocol::WaitResponse;
use clap::{Parser, Subcommand};

mod context;
mod mcp;

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
    },
    /// Long-poll for the next message addressed to you (or broadcast).
    Wait {
        /// Room id to long-poll.
        room_id: String,
        /// Self-declared model name (your identity; e.g. opus47).
        #[arg(long)]
        model: String,
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
    },
    /// Wake a paused (or idle) room back to active; repo and cwd are auto-detected.
    Wake {
        /// Room id to wake.
        room_id: String,
        /// Self-declared model name (your identity; e.g. opus47).
        #[arg(long)]
        model: String,
    },
    /// Run as an MCP stdio server (wired in a later cycle).
    Mcp,
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
        Command::Join { room_id, model } => {
            let repo = context::detect_repo();
            let cwd = context::detect_cwd();
            let resp = client
                .join_room(&room_id, &repo, &model, &cwd)
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
        } => {
            let repo = context::detect_repo();
            let cwd = context::detect_cwd();
            let resp = client
                .send_message(&room_id, &repo, &model, &cwd, to.as_deref(), &body, human)
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
        } => {
            let repo = context::detect_repo();
            let cwd = context::detect_cwd();
            let resp = client
                .signal(
                    &room_id,
                    &repo,
                    &model,
                    &cwd,
                    &signal_type,
                    severity.as_deref(),
                    question.as_deref(),
                )
                .await
                .context("sending signal")?;
            println!("Signal sent: seq {}", resp.seq);
        }
        Command::Wait { room_id, model } => {
            let repo = context::detect_repo();
            let cwd = context::detect_cwd();
            let resp = client
                .wait(&room_id, &repo, &model, &cwd)
                .await
                .context("waiting for message")?;
            match resp {
                WaitResponse::Message {
                    message,
                    surface_to_user,
                    retry_after,
                } => {
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
                    println!("  - {} ({} @ {})", p.handle, p.model, p.cwd);
                }
            }
        }
        Command::Close { room_id, model } => {
            let repo = context::detect_repo();
            let cwd = context::detect_cwd();
            let resp = client
                .close(&room_id, &repo, &model, &cwd)
                .await
                .context("closing room")?;
            println!("State: {}", resp.state);
        }
        Command::Pause {
            room_id,
            model,
            reason,
        } => {
            let repo = context::detect_repo();
            let cwd = context::detect_cwd();
            let resp = client
                .pause(&room_id, &repo, &model, &cwd, reason.as_deref())
                .await
                .context("pausing room")?;
            println!("State: {}", resp.state);
        }
        Command::Wake { room_id, model } => {
            let repo = context::detect_repo();
            let cwd = context::detect_cwd();
            let resp = client
                .wake(&room_id, &repo, &model, &cwd)
                .await
                .context("waking room")?;
            println!("State: {}", resp.state);
        }
        Command::Mcp => {
            mcp::run(client).await.context("running MCP server")?;
        }
    }

    Ok(())
}

/// Surface the slice 5b polling backoff hint, when present. The server already
/// held this poll for `secs` because the counterpart is parked behind a
/// `waiting_user` sentinel; tell the agent so it stays quiet rather than
/// hammering the room while the other side consults its user.
fn print_backoff(retry_after: Option<u32>) {
    if let Some(secs) = retry_after {
        println!("[backoff] Counterpart is paused; server held this poll ~{secs}s.");
    }
}
