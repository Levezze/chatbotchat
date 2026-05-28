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
    if let Some(top) = git_toplevel() {
        if let Some(name) = basename(&top) {
            return name;
        }
    }
    basename(&detect_cwd()).unwrap_or_else(|| "repo".to_string())
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
