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
                } => {
                    println!("From: {}", message.from);
                    println!("To:   {}", message.to.as_deref().unwrap_or("all"));
                    println!("Body: {}", message.body);
                    if surface_to_user {
                        println!();
                        println!(
                            "[soft cap] Consecutive autonomous turns hit the soft cap. \
                             Consult your user and send the next turn with --human."
                        );
                    }
                }
                WaitResponse::Timeout { status } => {
                    println!("{status}");
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
        Command::Mcp => {
            mcp::run(client).await.context("running MCP server")?;
        }
    }

    Ok(())
}
