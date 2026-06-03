//! Caller-environment detection shared by the CLI and MCP surfaces. `repo`/`cwd`
//! are auto-detected here (so the human only supplies `--model`), and the
//! `instance` identity key is resolved from the explicit `--as` label, the
//! environment, or a per-process floor.

use std::path::Path;
use std::process::Command;

/// Absolute current working directory as a string, or `"."` if it can't be read.
pub fn detect_cwd() -> String {
    std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".to_string())
}

/// Repository name: basename of `git rev-parse --show-toplevel`, falling back to
/// the cwd basename when not inside a git work tree (per design § Identity).
pub fn detect_repo() -> String {
    repo_from(git_toplevel(), &detect_cwd())
}

/// Pure resolution of the repo name from a (possibly absent) git toplevel and
/// the cwd. Split out from `detect_repo` so the fallback chain — git basename →
/// cwd basename → literal `"repo"` — is unit-testable without a real git tree.
fn repo_from(git_toplevel: Option<String>, cwd: &str) -> String {
    git_toplevel
        .as_deref()
        .and_then(basename)
        .or_else(|| basename(cwd))
        .unwrap_or_else(|| "repo".to_string())
}

fn git_toplevel() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

fn basename(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
}

/// The identity key for this caller. Two agents sharing `(repo, model, cwd)` are
/// told apart by this value, so it must be distinct per live agent and stable
/// across that agent's calls. Resolution (first non-empty wins):
///
/// 1. an explicit `--as` / `as:` label — also the way to *resume or hand off* an
///    identity: reuse the same label from another terminal/client/dir;
/// 2. `CBC_INSTANCE` — whole-process override (tests, power users);
/// 3. `CLAUDE_CODE_SESSION_ID` — best-effort: stable per Claude Code session
///    (and inherited by the long-lived `cbc mcp` child), survives resume within
///    Claude Code;
/// 4. a per-process floor (the PID) — guarantees a non-empty, distinct-per-live-
///    process value when nothing else is set. Least preferred: a new process
///    (a restart, or each one-shot CLI invocation outside a session) gets a new
///    value, so deliberate continuity should use the `--as` label.
pub fn detect_instance(explicit: Option<&str>) -> String {
    let cbc = std::env::var("CBC_INSTANCE").ok();
    let session = std::env::var("CLAUDE_CODE_SESSION_ID").ok();
    resolve_instance(
        explicit,
        cbc.as_deref(),
        session.as_deref(),
        &process_floor(),
    )
}

/// Pure resolution of the instance key from its candidate sources, split out so
/// the precedence and never-empty guarantee are unit-testable without touching
/// the environment. `floor` must itself be non-empty.
fn resolve_instance(
    explicit: Option<&str>,
    cbc_instance: Option<&str>,
    session_id: Option<&str>,
    floor: &str,
) -> String {
    [explicit, cbc_instance, session_id]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or(floor)
        .to_string()
}

/// Per-process identity floor: the OS process id. Constant for a process's life
/// (so every call from one long-lived `cbc mcp` server agrees) and distinct
/// among concurrently-live processes (so two separate sessions never collide).
fn process_floor() -> String {
    format!("pid-{}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_from_uses_git_toplevel_basename_when_present() {
        let repo = repo_from(
            Some("/Users/me/code/mvp-engine".to_string()),
            "/tmp/elsewhere",
        );
        assert_eq!(repo, "mvp-engine");
    }

    #[test]
    fn repo_from_falls_back_to_cwd_basename_outside_git() {
        let repo = repo_from(None, "/Users/me/work/api-server");
        assert_eq!(repo, "api-server");
    }

    #[test]
    fn repo_from_falls_back_to_literal_repo_at_filesystem_root() {
        // Neither a git toplevel nor a cwd with a basename (root has none).
        let repo = repo_from(None, "/");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn basename_trims_trailing_slash_and_rejects_empty() {
        assert_eq!(basename("/a/b/c/"), Some("c".to_string()));
        assert_eq!(basename("/"), None);
        assert_eq!(basename(""), None);
    }

    #[test]
    fn resolve_instance_prefers_explicit_label_over_everything() {
        let got = resolve_instance(
            Some("concierge"),
            Some("cbc-env"),
            Some("session-id"),
            "pid-1",
        );
        assert_eq!(got, "concierge");
    }

    #[test]
    fn resolve_instance_precedence_falls_through_cbc_then_session_then_floor() {
        assert_eq!(
            resolve_instance(None, Some("cbc-env"), Some("session-id"), "pid-1"),
            "cbc-env"
        );
        assert_eq!(
            resolve_instance(None, None, Some("session-id"), "pid-1"),
            "session-id"
        );
        assert_eq!(resolve_instance(None, None, None, "pid-1"), "pid-1");
    }

    #[test]
    fn resolve_instance_skips_blank_candidates_and_never_returns_empty() {
        // An explicit empty/whitespace label must not shadow the real sources,
        // and the floor guarantees a non-empty result.
        assert_eq!(
            resolve_instance(Some("   "), Some(""), Some("session-id"), "pid-9"),
            "session-id"
        );
        assert_eq!(
            resolve_instance(Some(""), Some(""), Some(""), "pid-9"),
            "pid-9"
        );
        assert!(!resolve_instance(None, None, None, "pid-9").is_empty());
    }

    #[test]
    fn process_floor_is_non_empty() {
        assert!(!process_floor().is_empty());
    }
}
