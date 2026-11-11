use anyhow::{ensure, Context, Result};
use clap::{Parser, Subcommand};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
};

#[derive(Parser)]
#[command(about = "Isolated CBC candidate; never installs or contacts the daily service")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Inspect {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        protected_db: PathBuf,
    },
    Init {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        port: u16,
        #[arg(long)]
        protected_db: PathBuf,
    },
    Serve {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        protected_db: PathBuf,
        /// Also serve the dashboard on 127.0.0.1 at the candidate port.
        #[arg(long)]
        dashboard: bool,
    },
    /// Open the dashboard in the browser with a fresh single-use sign-in link. The link goes
    /// straight to the browser and is never printed.
    Dashboard {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        protected_db: PathBuf,
    },
    /// Trusted owner setup over the running candidate core; never opens another Store.
    Provision {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        protected_db: PathBuf,
        #[arg(long)]
        spec: PathBuf,
        #[arg(long)]
        profile_out: PathBuf,
    },
    /// Read effective nonsecret policy or apply a revision-checked update to the running core.
    Settings {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        protected_db: PathBuf,
        #[arg(long, requires = "expected_revision")]
        file: Option<PathBuf>,
        #[arg(long, requires = "file")]
        expected_revision: Option<String>,
    },
    /// Revoke only the expected current binding, leaving logical assignment records intact.
    Revoke {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        protected_db: PathBuf,
        #[arg(long)]
        context: String,
        #[arg(long)]
        member: String,
        #[arg(long)]
        expected_epoch: i64,
    },
    /// Retire an identity when its session ends (T-25). Its rooms close and it never returns.
    Retire {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        protected_db: PathBuf,
        #[arg(long)]
        context: String,
        #[arg(long)]
        member: String,
        #[arg(long)]
        reason: String,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| anyhow::anyhow!("HOME required for daily-resource protection"))?;
    let mut protected_dbs = vec![
        PathBuf::from(home).join(".chatbotchat/state.db"),
        cbc_candidate::account_home()?.join(".chatbotchat/state.db"),
    ];
    if let Some(path) = std::env::var_os("CBC_DB") {
        anyhow::ensure!(
            PathBuf::from(&path).is_absolute(),
            "CBC_DB must be absolute for candidate protection"
        );
        protected_dbs.push(path.into());
    }
    match Args::parse().command {
        Command::Revoke {
            root,
            protected_db,
            context,
            member,
            expected_epoch,
        } => {
            protected_dbs.push(protected_db);
            let sandbox = cbc_candidate::Sandbox::inspect(&root, &protected_dbs)?;
            protect_port(sandbox.port())?;
            ensure!(
                expected_epoch > 0,
                "expected epoch must identify a current binding"
            );
            let owner = owner_token(&sandbox)?;
            let socket = sandbox.root().join("connector.sock");
            let epoch = cbc_candidate::connector::LocalOwnerClient::new(&socket, &owner)
                .revoke_binding(&context, &member, expected_epoch)
                .await?;
            println!(
                "{}",
                serde_json::json!({"status":"binding-revoked","member":member,"epoch":epoch})
            );
            Ok(())
        }
        Command::Retire {
            root,
            protected_db,
            context,
            member,
            reason,
        } => {
            protected_dbs.push(protected_db);
            let sandbox = cbc_candidate::Sandbox::inspect(&root, &protected_dbs)?;
            protect_port(sandbox.port())?;
            let owner = owner_token(&sandbox)?;
            let socket = sandbox.root().join("connector.sock");
            cbc_candidate::connector::LocalOwnerClient::new(&socket, &owner)
                .retire_member(&context, &member, &reason)
                .await?;
            println!(
                "{}",
                serde_json::json!({"status":"retired","member":member})
            );
            Ok(())
        }
        Command::Provision {
            root,
            protected_db,
            spec,
            profile_out,
        } => {
            protected_dbs.push(protected_db);
            let sandbox = cbc_candidate::Sandbox::inspect(&root, &protected_dbs)?;
            protect_port(sandbox.port())?;
            let owner = owner_token(&sandbox)?;
            let spec_bytes = read_input(&spec, &sandbox, &protected_dbs, 64 * 1024)?;
            let spec: cbc_candidate::owner::ProvisionSpec = serde_json::from_slice(&spec_bytes)
                .context("invalid trusted provisioning specification")?;
            spec.validate()?;
            let count = spec.members.len();
            let mut output = reserve_profile(&sandbox, &profile_out)?;
            let socket = sandbox.root().join("connector.sock");
            let client = cbc_candidate::connector::LocalOwnerClient::new(&socket, &owner);
            let profile = client.provision(spec).await.context(
                "provision request failed or uncertain; inspect core/profile before retry",
            )?;
            let bytes = serde_json::to_vec(&profile)?;
            ensure!(
                bytes.len() < 128 * 1024,
                "private profile exceeds size bound; inspect provisioning before retry"
            );
            output.write_all(&bytes)?;
            output.write_all(b"\n")?;
            output.sync_all()?;
            fs::File::open(sandbox.root())?.sync_all()?;
            println!(
                "{}",
                serde_json::json!({"status":"provisioned","profile":profile_out,"members":count})
            );
            Ok(())
        }
        Command::Settings {
            root,
            protected_db,
            file,
            expected_revision,
        } => {
            protected_dbs.push(protected_db);
            let sandbox = cbc_candidate::Sandbox::inspect(&root, &protected_dbs)?;
            protect_port(sandbox.port())?;
            let owner = owner_token(&sandbox)?;
            let socket = sandbox.root().join("connector.sock");
            let client = cbc_candidate::connector::LocalOwnerClient::new(&socket, &owner);
            let snapshot = if let Some(path) = file {
                let bytes = read_input(&path, &sandbox, &protected_dbs, 64 * 1024)?;
                let settings: cbc_candidate::settings::RuntimeSettings =
                    serde_json::from_slice(&bytes).context("invalid runtime settings input")?;
                settings.validate()?;
                client
                    .update_settings(
                        expected_revision
                            .as_deref()
                            .context("expected revision required")?,
                        settings,
                    )
                    .await?
            } else {
                client.settings().await?
            };
            println!("{}", serde_json::to_string(&snapshot)?);
            Ok(())
        }
        Command::Serve {
            root,
            protected_db,
            dashboard,
        } => {
            protected_dbs.push(protected_db);
            let sandbox = cbc_candidate::Sandbox::inspect(&root, &protected_dbs)?;
            protect_port(sandbox.port())?;
            let store = cbc_candidate::store::Store::open(&sandbox).await?;
            let mut connector = cbc_candidate::connector::LocalConnector::bind(&sandbox, store)?;
            let mut address = None;
            if dashboard {
                let listener = tokio::net::TcpListener::bind(("127.0.0.1", sandbox.port())).await?;
                address = Some(format!("http://127.0.0.1:{}/", sandbox.port()));
                connector = connector.with_dashboard(listener)?;
            }
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            println!(
                "{}",
                serde_json::json!({"status":"listening","socket":connector.path(),"dashboard":address,"pid":std::process::id()})
            );
            connector
                .run(async {
                    tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
                })
                .await
        }
        Command::Dashboard { root, protected_db } => {
            protected_dbs.push(protected_db);
            let sandbox = cbc_candidate::Sandbox::inspect(&root, &protected_dbs)?;
            protect_port(sandbox.port())?;
            let owner = owner_token(&sandbox)?;
            let socket = sandbox.root().join("connector.sock");
            let code = cbc_candidate::connector::LocalOwnerClient::new(&socket, &owner)
                .dashboard_login_code()
                .await?;
            let opener = if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            };
            let opened = std::process::Command::new(opener)
                .arg(format!(
                    "http://127.0.0.1:{}/login?code={code}",
                    sandbox.port()
                ))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            // Never fall back to printing the link: whoever reads it could sign in as the user.
            ensure!(
                opened.is_ok_and(|status| status.success()),
                "could not open a browser; the sign-in link was not shown and expires in 30 seconds"
            );
            println!(
                "{}",
                serde_json::json!({"status":"dashboard-opened","dashboard":format!("http://127.0.0.1:{}/", sandbox.port())})
            );
            Ok(())
        }
        Command::Inspect { root, protected_db } => {
            protected_dbs.push(protected_db);
            let sandbox = cbc_candidate::Sandbox::inspect(&root, &protected_dbs)?;
            protect_port(sandbox.port())?;
            println!("{}", serde_json::to_string(&sandbox)?);
            Ok(())
        }
        Command::Init {
            root,
            port,
            protected_db,
        } => {
            protected_dbs.push(protected_db);
            protect_port(port)?;
            let sandbox = cbc_candidate::Sandbox::initialize(&root, port, &protected_dbs)?;
            println!("{}", serde_json::to_string(&sandbox)?);
            Ok(())
        }
    }
}

fn owner_token(sandbox: &cbc_candidate::Sandbox) -> Result<String> {
    let path = sandbox.root().join("owner.key");
    let bytes = read_regular(&path, 71, true)?;
    let text = String::from_utf8(bytes).context("invalid candidate owner key")?;
    let token = text.strip_suffix('\n').unwrap_or(&text);
    ensure!(
        token.len() == 70
            && token
                .strip_prefix("cbco1_")
                .is_some_and(|part| part.bytes().all(|b| b.is_ascii_hexdigit())),
        "invalid candidate owner key"
    );
    Ok(token.to_owned())
}

fn read_regular(path: &Path, bound: usize, private: bool) -> Result<Vec<u8>> {
    let before = fs::symlink_metadata(path)?;
    ensure!(
        before.is_file()
            && before.nlink() == 1
            && before.uid() == unsafe { libc::geteuid() }
            && before.len() <= bound as u64
            && (!private || before.mode() & 0o777 == 0o600),
        "input must be an owned bounded regular file"
    );
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let opened = file.metadata()?;
    ensure!(
        opened.is_file()
            && opened.nlink() == 1
            && opened.uid() == before.uid()
            && (opened.dev(), opened.ino()) == (before.dev(), before.ino())
            && (!private || opened.mode() & 0o777 == 0o600),
        "input changed during open"
    );
    let mut bytes = Vec::new();
    (&mut file)
        .take((bound + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= bound, "input exceeds size bound");
    Ok(bytes)
}

fn read_input(
    path: &Path,
    sandbox: &cbc_candidate::Sandbox,
    protected: &[PathBuf],
    bound: usize,
) -> Result<Vec<u8>> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    // Refuse protected directories before opening them, including aliases through a symlinked parent.
    let parent = path
        .parent()
        .context("input has no parent")?
        .canonicalize()?;
    let canonical = parent.join(path.file_name().context("input has no name")?);
    ensure!(
        canonical != sandbox.root().join("owner.key"),
        "owner key cannot be an input document"
    );
    for protected in protected {
        let mut directories = vec![protected
            .parent()
            .context("protected database has no parent")?
            .to_path_buf()];
        if let Ok(real) = protected.canonicalize() {
            directories.push(
                real.parent()
                    .context("protected database has no real parent")?
                    .to_path_buf(),
            );
        }
        for directory in directories {
            let resolved = directory.canonicalize().unwrap_or(directory);
            ensure!(
                !canonical.starts_with(resolved),
                "input overlaps a protected daily resource"
            );
        }
    }
    read_regular(&path, bound, false)
}

fn reserve_profile(sandbox: &cbc_candidate::Sandbox, path: &Path) -> Result<fs::File> {
    ensure!(
        path.is_absolute()
            && !path
                .components()
                .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
            && path
                .parent()
                .context("profile has no parent")?
                .canonicalize()?
                == sandbox.root(),
        "profile must be a new JSON file directly beneath candidate root"
    );
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .context("profile name must be UTF-8")?;
    ensure!(
        name.ends_with(".json")
            && !name.starts_with('.')
            && ![
                "candidate.json",
                "runtime-settings.json",
                "runtime-settings.last-good.json"
            ]
            .contains(&name),
        "profile path is reserved"
    );
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    ensure!(
        file.metadata()?.is_file() && file.metadata()?.nlink() == 1,
        "unsafe private profile file"
    );
    Ok(file)
}

fn protect_port(port: u16) -> anyhow::Result<()> {
    let mut protected = vec![8484];
    if let Ok(value) = std::env::var("CBC_PORT") {
        protected.push(
            value
                .parse::<u16>()
                .map_err(|_| anyhow::anyhow!("invalid legacy CBC_PORT"))?,
        );
    }
    if let Ok(value) = std::env::var("CBC_SERVER") {
        let endpoint =
            url::Url::parse(&value).map_err(|_| anyhow::anyhow!("invalid legacy CBC_SERVER"))?;
        anyhow::ensure!(
            matches!(endpoint.scheme(), "http" | "https") && endpoint.host_str().is_some(),
            "invalid legacy CBC_SERVER"
        );
        protected.push(
            endpoint
                .port_or_known_default()
                .ok_or_else(|| anyhow::anyhow!("legacy endpoint has no port"))?,
        );
    }
    anyhow::ensure!(
        !protected.contains(&port),
        "candidate port overlaps a protected endpoint"
    );
    Ok(())
}
