//! Caller-environment detection shared by the CLI and MCP surfaces. The
//! `(repo, cwd)` half of a participant's identity tuple is auto-detected here so
//! the human only ever supplies `--model`.

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
}
