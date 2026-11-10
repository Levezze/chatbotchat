//! Trusted, fresh-root fixture setup only. Not enrollment, not a model-facing tool.
use anyhow::{ensure, Context, Result};
use cbc_candidate::{bindings::Role, store::Store, Sandbox};
use serde::Deserialize;
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    context: String,
    device: String,
    room: String,
    members: Vec<Member>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    member: String,
    session: String,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let root = PathBuf::from(args.next().context("candidate root required")?);
    let spec_path = PathBuf::from(args.next().context("fixture specification required")?);
    let protected = PathBuf::from(
        args.next()
            .context("protected daily database path required")?,
    );
    ensure!(args.next().is_none(), "unexpected argument");
    let sandbox = Sandbox::inspect(&root, &[protected])?;
    ensure!(
        !sandbox.database_path().exists(),
        "fixture requires a fresh candidate database"
    );
    let mut bytes = Vec::new();
    fs::File::open(spec_path)?
        .take(65537)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 65536, "fixture specification too large");
    let spec: Spec = serde_json::from_slice(&bytes)?;
    ensure!(
        (2..=128).contains(&spec.members.len()),
        "invalid fixture member count"
    );
    let profile = sandbox.root().join("native-bindings.json");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&profile)?;
    let store = Store::open(&sandbox).await?;
    let result = async {
        store.create_context(&spec.context).await?;
        let mut bindings = Vec::new();
        let mut sessions = std::collections::HashSet::new();
        let mut members = std::collections::HashSet::new();
        for member in &spec.members {
            ensure!(sessions.insert(&member.session) && members.insert(&member.member),"duplicate fixture session or member");
            store.register_member(&spec.context,&member.member,Role::Participant).await?;
            let binding = store.bind_member(&spec.context,&member.member,&spec.device,&member.session).await?;
            bindings.push(serde_json::json!({"context":spec.context,"member":member.member,"device":spec.device,"session":member.session,"epoch":binding.epoch(),"token":binding.expose_token_for_adapter()}));
        }
        store.create_room(&spec.context,&spec.room,&spec.members.iter().map(|m|m.member.as_str()).collect::<Vec<_>>()).await?;
        file.write_all(&serde_json::to_vec(&serde_json::json!({"v":1,"core_socket":sandbox.root().join("connector.sock"),"bindings":bindings}))?)?;
        file.sync_all()?;
        fs::File::open(sandbox.root())?.sync_all()?;
        Ok::<_,anyhow::Error>(())
    }.await;
    store.close().await;
    result?;
    println!(
        "{}",
        serde_json::json!({"status":"fixture-ready","profile":profile,"room":spec.room})
    );
    Ok(())
}
