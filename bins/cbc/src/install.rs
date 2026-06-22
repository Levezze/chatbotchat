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
/// prefer the sibling of the running `cbc`, then `which`. Errors rather than
/// falling back to a bare name — launchd execs with a restricted PATH that does
/// not include `~/.cargo/bin`, so a bare name would produce an agent that loads
/// but never starts (a silent failure).
pub fn resolve_server_binary(cbc_exe: &Path) -> anyhow::Result<PathBuf> {
    choose_server_binary(
        server_binary_beside(cbc_exe),
        which_on_path("chatbotchat-server"),
    )
}

/// Pick the daemon path from the sibling and PATH candidates, or error. Pure, so
/// the precedence and the no-binary-found failure are tested without a filesystem.
fn choose_server_binary(
    beside: Option<PathBuf>,
    on_path: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    beside.or(on_path).context(
        "could not locate the chatbotchat-server binary next to cbc or on PATH; \
         install it (cargo install --path bins/chatbotchat-server) or put it on PATH first",
    )
}

/// Escape the three characters that break XML text content. macOS install paths
/// can legitimately contain `&`/`<`/`>`, and an unescaped one would make the
/// generated plist unparseable — which `launchctl load` reports only at exec
/// time, not as a load error.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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
    let binary = xml_escape(&config.server_binary.display().to_string());
    let port = config.port;
    let out = xml_escape(&stdout_log(config).display().to_string());
    let err = xml_escape(&stderr_log(config).display().to_string());
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
"#
    )
}

/// Render a `newsyslog(8)` rule set that bounds the daemon's log growth: rotate
/// each log at ~5 MB, keep 5 archives. No pidfile/signal column — the daemon
/// holds its log open and does not reopen on signal, so a rotated file keeps
/// receiving writes until the daemon's next (launchd-managed) restart; archives
/// still age out by count, so disk stays bounded. No compression, to avoid
/// compressing a file the daemon is still writing.
fn render_newsyslog_conf(config: &InstallConfig) -> String {
    let out = stdout_log(config).display().to_string();
    let err = stderr_log(config).display().to_string();
    format!(
        "# chatbotchat log rotation for newsyslog(8).\n\
         # Install: sudo cp this file to /etc/newsyslog.d/chatbotchat.conf\n\
         # Fields: logfilename mode count size(KB) when\n\
         {out}\t644\t5\t5120\t*\n\
         {err}\t644\t5\t5120\t*\n"
    )
}

/// Write the newsyslog rule set to a user-writable staging file under the log
/// dir (the real `/etc/newsyslog.d/` needs root, so `install-daemon` stages it
/// and prints the `sudo cp`). Returns the staging path.
fn write_newsyslog_conf(config: &InstallConfig) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(&config.log_dir)
        .with_context(|| format!("creating {}", config.log_dir.display()))?;
    let path = config.log_dir.join("chatbotchat.newsyslog.conf");
    std::fs::write(&path, render_newsyslog_conf(config))
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
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
    // Silenced: on a fresh install there is nothing to unload, and launchctl
    // prints "Unload failed: 5: Input/output error" to stderr. We already ignore
    // the status, so swallow the output too rather than alarming the user.
    let _ = std::process::Command::new("launchctl")
        .arg("unload")
        .arg(plist_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
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
    // `launchctl load` can exit 0 even when the job never registers (e.g. an
    // unparseable plist), so probe explicitly: a registered label makes `list`
    // exit 0. This turns a silent non-start into a loud error.
    let listed = std::process::Command::new("launchctl")
        .arg("list")
        .arg(LABEL)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("running launchctl list")?;
    anyhow::ensure!(
        listed.success(),
        "{LABEL} did not register after load — the daemon may have failed to start; \
         check ~/Library/Logs/chatbotchat.err.log"
    );
    Ok(())
}

fn print_next_steps(config: &InstallConfig, plist_path: &Path, newsyslog_conf: &Path) {
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
    println!("Auto-approve the bus so cbc_send doesn't stall for per-call approval:");
    println!("  cbc allow-tools");
    println!();
    println!("Enable log rotation (needs sudo; one time):");
    println!(
        "  sudo cp {} /etc/newsyslog.d/chatbotchat.conf",
        newsyslog_conf.display()
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
    let server_binary = resolve_server_binary(&cbc_exe)?;
    let home = std::env::var_os("HOME").context("HOME not set; cannot place the launchd agent")?;
    let home = PathBuf::from(home);
    let config = InstallConfig {
        server_binary,
        port,
        log_dir: home.join("Library").join("Logs"),
        plist_dir: plist_dir_override.unwrap_or_else(|| home.join("Library").join("LaunchAgents")),
    };
    let plist_path = write_plist(&config)?;
    let newsyslog_conf = write_newsyslog_conf(&config)?;
    load_launchagent(&plist_path)?;
    print_next_steps(&config, &plist_path, &newsyslog_conf);
    maybe_prompt_allow_tools();
    install_bundled_skill();
    Ok(())
}

/// Install the bundled CBC Claude Code skills as part of setup. Unlike
/// `allow-tools` (a standing security approval that earns its interactive prompt),
/// a skill file is benign guidance text, so this runs automatically and without a
/// prompt. It is idempotent and symlink-safe: a devkit-managed symlink is left in
/// place (reported, not clobbered). Never aborts the install — a skills dir we
/// cannot write to degrades to a printed hint.
fn install_bundled_skill() {
    println!();
    match crate::skill::skills_dir()
        .and_then(|dir| crate::skill::install_all(&dir, false).map(|outcomes| (dir, outcomes)))
    {
        Ok((dir, outcomes)) => {
            for (name, outcome) in &outcomes {
                crate::skill::print_outcome(&dir, name, outcome);
            }
        }
        Err(e) => {
            eprintln!("Could not install the cbc skills automatically: {e:#}");
            eprintln!("Run `cbc install-skill` yourself once it's resolved.");
        }
    }
}

/// Offer to write the auto-approve rule during an interactive install. Defaults
/// to **No** — it grants the bus standing approval, which should be a deliberate
/// keystroke. Skipped entirely when stdin is not a TTY (piped / CI installs),
/// where the printed `cbc allow-tools` step is the opt-in instead. Never aborts
/// the install: a settings file we can't edit degrades to the manual snippet.
fn maybe_prompt_allow_tools() {
    use std::io::{IsTerminal, Write};

    if !std::io::stdin().is_terminal() {
        return;
    }
    print!("\nAuto-approve the chatbotchat MCP tools now (edits ~/.claude/settings.json)? [y/N] ");
    let _ = std::io::stdout().flush();

    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return;
    }
    if !matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        println!("Skipped. Run `cbc allow-tools` anytime to do this.");
        return;
    }

    match crate::settings::settings_path()
        .and_then(|p| crate::settings::apply_allow_rule(&p).map(|outcome| (p, outcome)))
    {
        Ok((path, outcome)) => crate::settings::print_allow_outcome(&path, &outcome),
        Err(e) => {
            eprintln!("Could not edit settings automatically: {e:#}");
            crate::settings::print_manual_snippet();
        }
    }
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

    #[test]
    fn render_plist_escapes_xml_special_chars_in_paths() {
        let dir = tempfile::tempdir().unwrap();
        let config = InstallConfig {
            server_binary: PathBuf::from("/opt/A & B/<svc>/chatbotchat-server"),
            port: 8484,
            log_dir: dir.path().join("Logs"),
            plist_dir: dir.path().join("LA"),
        };

        let written = write_plist(&config).unwrap();
        let contents = std::fs::read_to_string(&written).unwrap();

        assert!(
            contents.contains("/opt/A &amp; B/&lt;svc&gt;/chatbotchat-server"),
            "binary path must be XML-escaped:\n{contents}"
        );
        assert!(
            !contents.contains("A & B"),
            "a raw, unescaped ampersand must not reach the plist:\n{contents}"
        );
    }

    #[test]
    fn choose_server_binary_prefers_sibling_then_path_then_errors() {
        let sibling = PathBuf::from("/x/chatbotchat-server");
        let on_path = PathBuf::from("/usr/local/bin/chatbotchat-server");

        assert_eq!(
            choose_server_binary(Some(sibling.clone()), Some(on_path.clone())).unwrap(),
            sibling
        );
        assert_eq!(
            choose_server_binary(None, Some(on_path.clone())).unwrap(),
            on_path
        );
        assert!(
            choose_server_binary(None, None).is_err(),
            "with no binary found anywhere, install must error rather than bake an unrunnable path"
        );
    }

    #[test]
    fn newsyslog_conf_names_both_log_paths_with_a_rotation_rule() {
        let dir = tempfile::tempdir().unwrap();
        let config = InstallConfig {
            server_binary: PathBuf::from("/opt/bin/chatbotchat-server"),
            port: 8484,
            log_dir: dir.path().join("Logs"),
            plist_dir: dir.path().join("LA"),
        };

        let path = write_newsyslog_conf(&config).unwrap();
        let conf = std::fs::read_to_string(&path).unwrap();

        let out = dir.path().join("Logs").join("chatbotchat.log");
        let err = dir.path().join("Logs").join("chatbotchat.err.log");
        assert!(
            conf.contains(&out.display().to_string()),
            "conf must name the stdout log:\n{conf}"
        );
        assert!(
            conf.contains(&err.display().to_string()),
            "conf must name the stderr log:\n{conf}"
        );
        // A newsyslog rule is `logfile mode count size when`; assert the mode/size
        // columns ride alongside the log path on a real rule line.
        assert!(
            conf.lines()
                .any(|l| l.contains("chatbotchat.log") && l.contains("644") && l.contains("5120")),
            "conf must carry a rotation rule (mode + size) for the log:\n{conf}"
        );
    }
}
