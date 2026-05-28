use anyhow::Context;
use chatbotchat_client::HttpClient;
use clap::{Parser, Subcommand};

mod mcp;

/// `cbc` — the agent-facing client for chatbotchat. Talks to the local daemon
/// over HTTP. Same surface is exposed to MCP via the `mcp` subcommand.
#[derive(Debug, Parser)]
#[command(name = "cbc", version)]
struct Cli {
    /// Base URL of the chatbotchat daemon.
    #[arg(long, env = "CBC_SERVER", default_value = "http://127.0.0.1:8484", global = true)]
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
        Command::Open { subject } => {
            let resp = client
                .open_room(&subject)
                .await
                .context("opening room")?;
            println!("Room:  {}", resp.room_id);
            println!("Share: {}", resp.share_line);
            println!();
            println!("Tell the other agent: {}", resp.share_line);
        }
        Command::Status { room_id } => {
            let status = client.status(&room_id).await.context("fetching status")?;
            println!("Room:    {}", status.id);
            println!("Subject: {}", status.subject);
            println!("State:   {}", status.state);
            println!("Started: {}", status.started_at);
            println!("Active:  {}", status.last_activity_at);
        }
        Command::Mcp => {
            mcp::run(client).await.context("running MCP server")?;
        }
    }

    Ok(())
}
