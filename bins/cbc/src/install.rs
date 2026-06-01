//! `cbc install-daemon` — write a hardened launchd agent for the daemon and load
//! it, so `chatbotchat-server` stays always-on across crashes and reboots.
//!
//! The testable seams are kept pure: [`server_binary_beside`] (locate the daemon
//! binary next to the running `cbc`) and [`write_plist`] (render + write the agent
//! to an injectable directory). The `launchctl` load and the printed next-steps
//! are side-effecting and verified by hand (HITL) — launchd cannot be unit-tested.

use anyhow::Context;
use std::path::{Path, PathBuf};

/// The launchd label / plist basename. `<label>.plist` is what lands in the
/// LaunchAgents directory.
const LABEL: &str = "com.chatbotchat.server";

/// Inputs for rendering and placing the launchd agent. `plist_dir` and `log_dir`
/// are injected (real install uses `~/Library/LaunchAgents` and `~/Library/Logs`)
/// so tests can point them at a tempdir.
pub struct InstallConfig {
    pub server_binary: PathBuf,
    pub port: u16,
    pub log_dir: PathBuf,
    pub plist_dir: PathBuf,
}

/// The daemon binary sitting next to the running `cbc`, if present. `cargo
/// install` and `cargo build` both place the two binaries in the same directory,
/// so this is the common case and needs no PATH lookup.
pub fn server_binary_beside(cbc_exe: &Path) -> Option<PathBuf> {
    let candidate = cbc_exe.parent()?.join("chatbotchat-server");
    candidate.is_file().then_some(candidate)
}

/// Resolve the absolute path to `chatbotchat-server` to bake into the plist:
/// prefer the sibling of the running `cbc`, then `which`, then the bare name
/// (resolved on launchd's PATH at load time as a last resort).
pub fn resolve_server_binary(cbc_exe: &Path) -> PathBuf {
    server_binary_beside(cbc_exe)
        .or_else(|| which_on_path("chatbotchat-server"))
        .unwrap_or_else(|| PathBuf::from("chatbotchat-server"))
}

fn which_on_path(name: &str) -> Option<PathBuf> {
    let out = std::process::Command::new("which")
        .arg(name)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

fn stdout_log(config: &InstallConfig) -> PathBuf {
    config.log_dir.join("chatbotchat.log")
}

fn stderr_log(config: &InstallConfig) -> PathBuf {
    config.log_dir.join("chatbotchat.err.log")
}

/// Render the launchd agent: restart-on-crash + at-boot (`KeepAlive` +
/// `RunAtLoad`), logs under `~/Library/Logs`, and an `ExitTimeOut` that gives the
/// daemon a grace window to finish before launchd escalates to SIGKILL.
fn render_plist(config: &InstallConfig) -> String {
    let binary = config.server_binary.display();
    let port = config.port;
    let out = stdout_log(config);
    let err = stderr_log(config);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>

    <key>ProgramArguments</key>
    <array>
        <string>{binary}</string>
        <string>--port</string>
        <string>{port}</string>
    </array>

    <key>RunAtLoad</key>
    <true/>

    <key>KeepAlive</key>
    <true/>

    <key>ProcessType</key>
    <string>Background</string>

    <key>ExitTimeOut</key>
    <integer>25</integer>

    <key>StandardOutPath</key>
    <string>{out}</string>

    <key>StandardErrorPath</key>
    <string>{err}</string>
</dict>
</plist>
"#,
        out = out.display(),
        err = err.display(),
    )
}

/// Render the agent and write it to `<plist_dir>/com.chatbotchat.server.plist`,
/// creating the plist and log directories if needed. Returns the written path.
pub fn write_plist(config: &InstallConfig) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(&config.plist_dir)
        .with_context(|| format!("creating {}", config.plist_dir.display()))?;
    std::fs::create_dir_all(&config.log_dir)
        .with_context(|| format!("creating {}", config.log_dir.display()))?;
    let path = config.plist_dir.join(format!("{LABEL}.plist"));
    std::fs::write(&path, render_plist(config))
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Best-effort (re)load of the agent: unload any prior copy first so re-running
/// `install-daemon` refreshes cleanly, then load with `-w` (enable at boot).
fn load_launchagent(plist_path: &Path) -> anyhow::Result<()> {
    let _ = std::process::Command::new("launchctl")
        .arg("unload")
        .arg(plist_path)
        .status();
    let status = std::process::Command::new("launchctl")
        .arg("load")
        .arg("-w")
        .arg(plist_path)
        .status()
        .context("running launchctl load")?;
    anyhow::ensure!(
        status.success(),
        "launchctl load failed for {}",
        plist_path.display()
    );
    Ok(())
}

fn print_next_steps(config: &InstallConfig, plist_path: &Path) {
    let port = config.port;
    println!("Installed launchd agent: {}", plist_path.display());
    println!("Daemon: {} --port {port}", config.server_binary.display());
    println!("Logs:   {}", stdout_log(config).display());
    println!("        {}", stderr_log(config).display());
    println!();
    println!("Register the MCP tools globally for Claude Code (one time, all sessions):");
    println!(
        "  claude mcp add --scope user chatbotchat -e CBC_SERVER=http://127.0.0.1:{port} -- cbc mcp"
    );
    println!();
    println!("Verify:  launchctl list | grep {LABEL}");
    println!("Stop:    launchctl unload {}", plist_path.display());
}

/// `cbc install-daemon` entry point: resolve the daemon binary, write the agent,
/// load it via launchd, and print the MCP-registration one-liner + log paths.
/// `plist_dir_override` lets advanced users (and the docs) target a non-default
/// LaunchAgents directory.
pub fn run(port: u16, plist_dir_override: Option<PathBuf>) -> anyhow::Result<()> {
    let cbc_exe = std::env::current_exe().context("locating the running cbc binary")?;
    let server_binary = resolve_server_binary(&cbc_exe);
    let home = std::env::var_os("HOME").context("HOME not set; cannot place the launchd agent")?;
    let home = PathBuf::from(home);
    let config = InstallConfig {
        server_binary,
        port,
        log_dir: home.join("Library").join("Logs"),
        plist_dir: plist_dir_override.unwrap_or_else(|| home.join("Library").join("LaunchAgents")),
    };
    let plist_path = write_plist(&config)?;
    load_launchagent(&plist_path)?;
    print_next_steps(&config, &plist_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_server_binary_beside_cbc() {
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join("cbc");
        std::fs::write(&cbc, b"x").unwrap();
        let server = dir.path().join("chatbotchat-server");
        std::fs::write(&server, b"x").unwrap();
        assert_eq!(server_binary_beside(&cbc), Some(server));
    }

    #[test]
    fn no_sibling_server_binary_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join("cbc");
        std::fs::write(&cbc, b"x").unwrap();
        assert_eq!(server_binary_beside(&cbc), None);
    }

    #[test]
    fn write_plist_lands_in_target_dir_with_binary_and_port() {
        let dir = tempfile::tempdir().unwrap();
        let plist_dir = dir.path().join("LaunchAgents"); // not pre-created
        let config = InstallConfig {
            server_binary: PathBuf::from("/opt/bin/chatbotchat-server"),
            port: 8485,
            log_dir: dir.path().join("Logs"),
            plist_dir: plist_dir.clone(),
        };

        let written = write_plist(&config).unwrap();

        assert_eq!(written, plist_dir.join("com.chatbotchat.server.plist"));
        let contents = std::fs::read_to_string(&written).unwrap();
        assert!(
            contents.contains("/opt/bin/chatbotchat-server"),
            "plist should bake in the resolved daemon path:\n{contents}"
        );
        assert!(
            contents.contains("8485"),
            "plist should carry the chosen --port:\n{contents}"
        );
        assert!(
            contents.contains("com.chatbotchat.server"),
            "plist should carry the launchd Label:\n{contents}"
        );
        // The log directory is created as a side effect.
        assert!(dir.path().join("Logs").is_dir());
    }
}
