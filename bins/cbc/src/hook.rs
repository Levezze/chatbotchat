//! `cbc hook` — Claude Code hook handlers.
//!
//! Each event maps to one handler. Handlers are pure functions over
//! path-injected I/O and a kill seam so every branch is unit-tested without
//! spawning real processes or touching `~/.claude/settings.json`.
//!
//! # `cbc hook session-start`
//!
//! Reads the Claude Code `SessionStart` hook JSON from stdin. On `compact` or
//! `resume` sources it scans `<cwd>/.cbc/` for active CBC state files, kills
//! any stale poll process for each active room via `kill_fn`, and writes a
//! high-salience relaunch directive to stdout — injected as a system reminder
//! that the model sees before its first turn.
//!
//! On `startup` or `clear` sources it exits silently (fresh sessions run the
//! skill from scratch). On a missing or malformed `.cbc/` directory it exits
//! silently (no CBC rooms open — correct behavior for non-CBC sessions).

use anyhow::Context as _;
use serde_json::Value;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

// ── Parsed state ─────────────────────────────────────────────────────────────

/// Fields extracted from an active `.cbc/worker-*.md` file.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct WorkerEntry {
    /// The bare CBC room id.
    pub room_id: String,
    /// The background-poll task label (used in the emitted comment).
    pub poll_label: String,
    /// The self-declared model (e.g. `claude-sonnet-4-6`).
    pub model: String,
}

/// Fields extracted from an active `.cbc/orchestration-*.md` file.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct OrchestratorEntry {
    /// The orchestrator's own model — used for all its `cbc poll` commands.
    pub model: String,
    /// `(agent_name, room_id)` pairs extracted from the `agents:` registry.
    pub rooms: Vec<(String, String)>,
}

/// One active entry found in `.cbc/`.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum ActiveEntry {
    Worker(WorkerEntry),
    Orchestrator(OrchestratorEntry),
}

// ── Parsing helpers ───────────────────────────────────────────────────────────

/// Extract the value part of a `key: value` line.  Returns `Some(value.trim())`
/// when the line starts with `key:` (any trailing whitespace stripped).
fn kv<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.strip_prefix(key)
        .and_then(|rest| rest.strip_prefix(':'))
        .map(str::trim)
}

/// Return `true` when the file contains `status: ACTIVE`.
fn is_active(content: &str) -> bool {
    content
        .lines()
        .any(|l| kv(l.trim(), "status") == Some("ACTIVE"))
}

/// Parse a `.cbc/worker-*.md` file.  Returns `None` when not `ACTIVE` or when
/// required fields are absent.
///
/// Required: `room-id`.
/// Optional with defaults: `model` (falls back to `"<model>"` for backward
/// compat with files written before this field existed), `poll-label` (falls
/// back to empty so the emitted command is still correct).
pub fn parse_worker(content: &str) -> Option<WorkerEntry> {
    if !is_active(content) {
        return None;
    }
    let mut room_id = None;
    let mut poll_label = None;
    let mut model = None;
    for line in content.lines() {
        let t = line.trim();
        if room_id.is_none() {
            if let Some(v) = kv(t, "room-id") {
                room_id = Some(v.to_string());
            }
        }
        if poll_label.is_none() {
            if let Some(v) = kv(t, "poll-label") {
                poll_label = Some(v.to_string());
            }
        }
        if model.is_none() {
            if let Some(v) = kv(t, "model") {
                model = Some(v.to_string());
            }
        }
    }
    let room_id = room_id?;
    Some(WorkerEntry {
        room_id,
        poll_label: poll_label.unwrap_or_default(),
        model: model.unwrap_or_else(|| "<model>".to_string()),
    })
}

/// Parse a `.cbc/orchestration-*.md` file.  Returns `None` when not `ACTIVE`
/// or when the `agents:` block is empty (nothing to relaunch).
///
/// Required: at least one entry in the `agents:` block.
/// Optional with default: `model` (falls back to `"<model>"`).
pub fn parse_orchestrator(content: &str) -> Option<OrchestratorEntry> {
    if !is_active(content) {
        return None;
    }
    let mut model = None;
    let mut in_agents = false;
    let mut rooms: Vec<(String, String)> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Top-level `model:` (not indented inside a section).
        if model.is_none() && !line.starts_with(' ') && !line.starts_with('\t') {
            if let Some(v) = kv(trimmed, "model") {
                model = Some(v.to_string());
            }
        }

        // Detect the `agents:` section header.
        if trimmed == "agents:" {
            in_agents = true;
            continue;
        }

        if in_agents {
            if trimmed.is_empty() {
                continue;
            }
            // A non-indented non-empty line ends the agents block.
            if !line.starts_with(' ') && !line.starts_with('\t') {
                in_agents = false;
                // Still process this line for kv in the outer loop — fall through.
                continue;
            }
            // Agent line: `  <name>: <room-id> ...`
            if let Some(colon) = trimmed.find(": ") {
                let name = trimmed[..colon].trim().to_string();
                let rest = trimmed[colon + 2..].trim();
                // room-id is the first whitespace-delimited token.
                if let Some(room_id) = rest.split_whitespace().next() {
                    rooms.push((name, room_id.to_string()));
                }
            }
        }
    }

    if rooms.is_empty() {
        return None;
    }
    Some(OrchestratorEntry {
        model: model.unwrap_or_else(|| "<model>".to_string()),
        rooms,
    })
}

// ── Directory scan ────────────────────────────────────────────────────────────

/// Scan `cbc_dir` for `worker-*.md` and `orchestration-*.md` files and return
/// all active entries in deterministic (sorted) order.
pub fn scan_active(cbc_dir: &Path) -> Vec<ActiveEntry> {
    let mut entries = Vec::new();
    let iter = match std::fs::read_dir(cbc_dir) {
        Ok(it) => it,
        Err(_) => return entries,
    };
    let mut paths: Vec<PathBuf> = iter
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
        .collect();
    paths.sort();
    for path in paths {
        let fname = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if fname.starts_with("worker-") {
            if let Some(w) = parse_worker(&content) {
                entries.push(ActiveEntry::Worker(w));
            }
        } else if fname.starts_with("orchestration-") {
            if let Some(o) = parse_orchestrator(&content) {
                entries.push(ActiveEntry::Orchestrator(o));
            }
        }
    }
    entries
}

// ── Output ────────────────────────────────────────────────────────────────────

/// Write a relaunch directive for `entry` to `out`.  Calls `kill_fn(room_id)`
/// for every room before writing, so stale poll processes are killed in the
/// hook's own shell before the model is told to relaunch.
pub fn emit_relaunch<W: Write>(
    entry: &ActiveEntry,
    out: &mut W,
    kill_fn: &mut dyn FnMut(&str),
) -> std::io::Result<()> {
    match entry {
        ActiveEntry::Worker(w) => {
            kill_fn(&w.room_id);
            writeln!(out)?;
            writeln!(
                out,
                "⚠ CBC POLL RESTART REQUIRED (post-compaction). \
                 Your background poll did NOT survive."
            )?;
            writeln!(
                out,
                "FIRST ACTION, before reading files or composing anything — \
                 relaunch as a background task:"
            )?;
            if w.poll_label.is_empty() {
                writeln!(out, "  cbc poll {} --model {}", w.room_id, w.model)?;
            } else {
                writeln!(
                    out,
                    "  cbc poll {} --model {}      # {}",
                    w.room_id, w.model, w.poll_label
                )?;
            }
            writeln!(
                out,
                "The room is ACTIVE; you are deaf until this poll is running."
            )?;
        }
        ActiveEntry::Orchestrator(o) => {
            for (_, room_id) in &o.rooms {
                kill_fn(room_id);
            }
            writeln!(out)?;
            writeln!(
                out,
                "⚠ CBC POLL RESTART REQUIRED (post-compaction). \
                 Your background polls did NOT survive."
            )?;
            writeln!(
                out,
                "FIRST ACTION, before reading files or composing anything — \
                 relaunch ALL room polls as background tasks:"
            )?;
            for (name, room_id) in &o.rooms {
                writeln!(
                    out,
                    "  cbc poll {} --model {}      # {} poll",
                    room_id, o.model, name
                )?;
            }
            writeln!(
                out,
                "All rooms are ACTIVE; you are deaf until all polls are running."
            )?;
        }
    }
    Ok(())
}

// ── Top-level handler ─────────────────────────────────────────────────────────

/// Handle a `SessionStart` hook event.
///
/// Reads the hook JSON from `reader`.  On `compact` or `resume` sources it
/// scans `<cwd>/.cbc/` for active state files, calls `kill_fn` for each room,
/// and writes relaunch directives to `writer`.  Silently exits on any other
/// source or when no active CBC state is found.
///
/// The `cwd` path is read from the hook JSON's `"cwd"` field when present;
/// otherwise the process's working directory is used as a fallback.
pub fn run_session_start<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    kill_fn: &mut dyn FnMut(&str),
) -> anyhow::Result<()> {
    let mut raw = String::new();
    reader
        .read_to_string(&mut raw)
        .context("reading SessionStart hook JSON from stdin")?;

    if raw.trim().is_empty() {
        return Ok(());
    }

    let json: Value = serde_json::from_str(&raw).unwrap_or(Value::Object(Default::default()));

    let source = json
        .get("source")
        .and_then(|s| s.as_str())
        .unwrap_or("startup");

    if source != "compact" && source != "resume" {
        return Ok(());
    }

    let cwd: PathBuf = json
        .get("cwd")
        .and_then(|c| c.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let cbc_dir = cwd.join(".cbc");
    let entries = scan_active(&cbc_dir);

    for entry in &entries {
        emit_relaunch(entry, writer, kill_fn)?;
    }

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_worker ──────────────────────────────────────────────────────────

    #[test]
    fn parse_worker_returns_entry_for_active_file_with_all_fields() {
        let content = "\
## Worker charter — read me first, every session
(charter)

## Status
status: ACTIVE
next-action: keep implementing
phase: implementing
last-synced-to-orchestrator: implementing
task: add the thing
branch: feat/the-thing
worktree: /home/me/worktrees/the-thing
room-id: repo-worker-feat-20260625-1200
poll-label: repo-worker-feat
model: claude-sonnet-4-6
state-file-path: /home/me/worktrees/the-thing/.cbc/worker-repo-feat-20260625.md
";
        let w = parse_worker(content).expect("should parse active worker");
        assert_eq!(w.room_id, "repo-worker-feat-20260625-1200");
        assert_eq!(w.poll_label, "repo-worker-feat");
        assert_eq!(w.model, "claude-sonnet-4-6");
    }

    #[test]
    fn parse_worker_returns_none_for_done_status() {
        let content = "\
## Status
status: DONE
room-id: some-room
model: sonnet
";
        assert!(
            parse_worker(content).is_none(),
            "DONE status must not produce an entry"
        );
    }

    #[test]
    fn parse_worker_returns_none_when_no_room_id() {
        let content = "\
## Status
status: ACTIVE
model: sonnet
";
        assert!(
            parse_worker(content).is_none(),
            "missing room-id must produce None"
        );
    }

    #[test]
    fn parse_worker_falls_back_gracefully_without_model_or_label() {
        let content = "\
## Status
status: ACTIVE
room-id: some-room-20260625-0900
";
        let w = parse_worker(content).expect("should parse with defaults");
        assert_eq!(w.room_id, "some-room-20260625-0900");
        assert_eq!(w.poll_label, "");
        assert_eq!(w.model, "<model>");
    }

    // ── parse_orchestrator ────────────────────────────────────────────────────

    #[test]
    fn parse_orchestrator_returns_entry_for_active_file() {
        let content = "\
## Orchestrator charter

status: ACTIVE
next-action: recap all rooms
branch: main
worktree: /home/me/projects/repo
model: claude-opus-4-8

agents:
  repo-worker-feat: feat-room-20260625-1100 (handle abc123) — feat work — ✓
  repo-worker-fix:  fix-room-20260625-1105 (handle def456) — fix work — quiet
";
        let o = parse_orchestrator(content).expect("should parse active orchestrator");
        assert_eq!(o.model, "claude-opus-4-8");
        assert_eq!(o.rooms.len(), 2);
        assert_eq!(o.rooms[0], ("repo-worker-feat".to_string(), "feat-room-20260625-1100".to_string()));
        assert_eq!(o.rooms[1], ("repo-worker-fix".to_string(), "fix-room-20260625-1105".to_string()));
    }

    #[test]
    fn parse_orchestrator_returns_none_for_done() {
        let content = "\
status: DONE
model: opus

agents:
  repo-worker-feat: some-room-123 (handle abc) — done
";
        assert!(parse_orchestrator(content).is_none());
    }

    #[test]
    fn parse_orchestrator_returns_none_when_agents_empty() {
        let content = "\
status: ACTIVE
model: opus

agents:
";
        assert!(
            parse_orchestrator(content).is_none(),
            "empty agents block must return None"
        );
    }

    #[test]
    fn parse_orchestrator_falls_back_model_without_model_field() {
        let content = "\
status: ACTIVE

agents:
  repo-worker-feat: some-room-20260625-1200 (handle abc) — feat
";
        let o = parse_orchestrator(content).expect("should parse");
        assert_eq!(o.model, "<model>");
    }

    // ── scan_active ───────────────────────────────────────────────────────────

    #[test]
    fn scan_active_returns_empty_for_missing_dir() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no-such-dir").join(".cbc");
        assert!(scan_active(&missing).is_empty());
    }

    #[test]
    fn scan_active_finds_worker_and_ignores_done() {
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join(".cbc");
        std::fs::create_dir_all(&cbc).unwrap();

        std::fs::write(
            cbc.join("worker-repo-feat-20260625.md"),
            "status: ACTIVE\nroom-id: feat-room-20260625-1000\nmodel: sonnet\n",
        )
        .unwrap();
        std::fs::write(
            cbc.join("worker-repo-done-20260620.md"),
            "status: DONE\nroom-id: old-room\n",
        )
        .unwrap();

        let entries = scan_active(&cbc);
        assert_eq!(entries.len(), 1);
        match &entries[0] {
            ActiveEntry::Worker(w) => assert_eq!(w.room_id, "feat-room-20260625-1000"),
            _ => panic!("expected Worker entry"),
        }
    }

    #[test]
    fn scan_active_finds_orchestration_file() {
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join(".cbc");
        std::fs::create_dir_all(&cbc).unwrap();

        std::fs::write(
            cbc.join("orchestration-repo-20260625.md"),
            "status: ACTIVE\nmodel: opus\n\nagents:\n  repo-worker-feat: room-abc-20260625-1000 (handle xyz) — feat\n",
        )
        .unwrap();

        let entries = scan_active(&cbc);
        assert_eq!(entries.len(), 1);
        match &entries[0] {
            ActiveEntry::Orchestrator(o) => {
                assert_eq!(o.rooms.len(), 1);
                assert_eq!(o.rooms[0].1, "room-abc-20260625-1000");
            }
            _ => panic!("expected Orchestrator entry"),
        }
    }

    // ── emit_relaunch ─────────────────────────────────────────────────────────

    #[test]
    fn emit_relaunch_worker_contains_poll_command_and_calls_kill() {
        let entry = ActiveEntry::Worker(WorkerEntry {
            room_id: "feat-room-20260625-1000".to_string(),
            poll_label: "repo-worker-feat".to_string(),
            model: "claude-sonnet-4-6".to_string(),
        });
        let mut out = Vec::new();
        let mut killed = Vec::new();
        emit_relaunch(&entry, &mut out, &mut |id| killed.push(id.to_string())).unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("cbc poll feat-room-20260625-1000 --model claude-sonnet-4-6"),
            "output must contain the exact poll command; got:\n{text}"
        );
        assert!(
            text.contains("repo-worker-feat"),
            "output must contain the poll label; got:\n{text}"
        );
        assert_eq!(
            killed,
            vec!["feat-room-20260625-1000"],
            "kill_fn must be called with the room id"
        );
    }

    #[test]
    fn emit_relaunch_orchestrator_lists_all_rooms_and_kills_each() {
        let entry = ActiveEntry::Orchestrator(OrchestratorEntry {
            model: "claude-opus-4-8".to_string(),
            rooms: vec![
                ("repo-worker-feat".to_string(), "room-a-20260625-1100".to_string()),
                ("repo-worker-fix".to_string(), "room-b-20260625-1105".to_string()),
            ],
        });
        let mut out = Vec::new();
        let mut killed = Vec::new();
        emit_relaunch(&entry, &mut out, &mut |id| killed.push(id.to_string())).unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("cbc poll room-a-20260625-1100 --model claude-opus-4-8"));
        assert!(text.contains("cbc poll room-b-20260625-1105 --model claude-opus-4-8"));
        assert!(text.contains("repo-worker-feat"));
        assert!(text.contains("repo-worker-fix"));
        assert_eq!(
            killed,
            vec!["room-a-20260625-1100", "room-b-20260625-1105"],
            "kill_fn must be called once per room"
        );
    }

    // ── run_session_start ─────────────────────────────────────────────────────

    #[test]
    fn run_startup_source_emits_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join(".cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(
            cbc.join("worker-repo-feat-20260625.md"),
            "status: ACTIVE\nroom-id: some-room\nmodel: sonnet\n",
        )
        .unwrap();

        let input = serde_json::json!({
            "hook_event_name": "SessionStart",
            "source": "startup",
            "cwd": dir.path().to_str().unwrap()
        })
        .to_string();

        let mut out = Vec::new();
        let mut killed = Vec::new();
        run_session_start(
            &mut input.as_bytes(),
            &mut out,
            &mut |id| killed.push(id.to_string()),
        )
        .unwrap();

        assert!(out.is_empty(), "startup source must produce no output");
        assert!(killed.is_empty(), "startup source must call no kill");
    }

    #[test]
    fn run_compact_source_emits_relaunch_for_active_worker() {
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join(".cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(
            cbc.join("worker-repo-feat-20260625.md"),
            "status: ACTIVE\nroom-id: feat-room-20260625-1000\npoll-label: repo-worker-feat\nmodel: claude-sonnet-4-6\n",
        )
        .unwrap();

        let input = serde_json::json!({
            "hook_event_name": "SessionStart",
            "source": "compact",
            "cwd": dir.path().to_str().unwrap()
        })
        .to_string();

        let mut out = Vec::new();
        let mut killed = Vec::new();
        run_session_start(
            &mut input.as_bytes(),
            &mut out,
            &mut |id| killed.push(id.to_string()),
        )
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("cbc poll feat-room-20260625-1000 --model claude-sonnet-4-6"),
            "compact output must contain poll command; got:\n{text}"
        );
        assert_eq!(killed, vec!["feat-room-20260625-1000"]);
    }

    #[test]
    fn run_resume_source_also_emits_relaunch() {
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join(".cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(
            cbc.join("worker-repo-feat-20260625.md"),
            "status: ACTIVE\nroom-id: resume-room-20260625-1000\nmodel: sonnet\n",
        )
        .unwrap();

        let input = serde_json::json!({
            "source": "resume",
            "cwd": dir.path().to_str().unwrap()
        })
        .to_string();

        let mut out = Vec::new();
        let mut killed = Vec::new();
        run_session_start(&mut input.as_bytes(), &mut out, &mut |id| {
            killed.push(id.to_string())
        })
        .unwrap();

        assert!(!out.is_empty(), "resume source must emit output");
        assert_eq!(killed, vec!["resume-room-20260625-1000"]);
    }

    #[test]
    fn run_done_worker_emits_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join(".cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(
            cbc.join("worker-repo-feat-20260625.md"),
            "status: DONE\nroom-id: feat-room\nmodel: sonnet\n",
        )
        .unwrap();

        let input = serde_json::json!({
            "source": "compact",
            "cwd": dir.path().to_str().unwrap()
        })
        .to_string();

        let mut out = Vec::new();
        let mut killed: Vec<String> = Vec::new();
        run_session_start(&mut input.as_bytes(), &mut out, &mut |id| {
            killed.push(id.to_string())
        })
        .unwrap();

        assert!(
            out.is_empty(),
            "DONE worker must produce no output; got:\n{}",
            String::from_utf8_lossy(&out)
        );
        assert!(killed.is_empty());
    }

    #[test]
    fn run_no_cbc_dir_emits_nothing() {
        let dir = tempfile::tempdir().unwrap();
        // No .cbc/ created.
        let input = serde_json::json!({
            "source": "compact",
            "cwd": dir.path().to_str().unwrap()
        })
        .to_string();

        let mut out = Vec::new();
        let mut killed: Vec<String> = Vec::new();
        run_session_start(&mut input.as_bytes(), &mut out, &mut |id| {
            killed.push(id.to_string())
        })
        .unwrap();

        assert!(out.is_empty(), "no .cbc dir must emit nothing");
        assert!(killed.is_empty());
    }

    #[test]
    fn run_compact_with_two_active_workers_emits_both() {
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join(".cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(
            cbc.join("worker-repo-feat-20260625.md"),
            "status: ACTIVE\nroom-id: room-a-20260625-1000\nmodel: sonnet\n",
        )
        .unwrap();
        std::fs::write(
            cbc.join("worker-repo-fix-20260625.md"),
            "status: ACTIVE\nroom-id: room-b-20260625-1005\nmodel: sonnet\n",
        )
        .unwrap();

        let input = serde_json::json!({
            "source": "compact",
            "cwd": dir.path().to_str().unwrap()
        })
        .to_string();

        let mut out = Vec::new();
        let mut killed: Vec<String> = Vec::new();
        run_session_start(&mut input.as_bytes(), &mut out, &mut |id| {
            killed.push(id.to_string())
        })
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("room-a-20260625-1000"), "must emit first room");
        assert!(text.contains("room-b-20260625-1005"), "must emit second room");
        assert_eq!(killed.len(), 2);
    }

    #[test]
    fn run_compact_with_orchestration_file_emits_all_agent_rooms() {
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join(".cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(
            cbc.join("orchestration-repo-20260625.md"),
            "status: ACTIVE\nmodel: claude-opus-4-8\n\nagents:\n  repo-worker-feat: room-a-20260625-1100 (handle abc) — feat\n  repo-worker-fix: room-b-20260625-1105 (handle def) — fix\n",
        )
        .unwrap();

        let input = serde_json::json!({
            "source": "compact",
            "cwd": dir.path().to_str().unwrap()
        })
        .to_string();

        let mut out = Vec::new();
        let mut killed: Vec<String> = Vec::new();
        run_session_start(&mut input.as_bytes(), &mut out, &mut |id| {
            killed.push(id.to_string())
        })
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("cbc poll room-a-20260625-1100 --model claude-opus-4-8"));
        assert!(text.contains("cbc poll room-b-20260625-1105 --model claude-opus-4-8"));
        assert_eq!(killed.len(), 2);
    }

    #[test]
    fn run_empty_stdin_is_no_op() {
        let mut out = Vec::new();
        let mut killed: Vec<String> = Vec::new();
        run_session_start(&mut "".as_bytes(), &mut out, &mut |id| {
            killed.push(id.to_string())
        })
        .unwrap();
        assert!(out.is_empty());
        assert!(killed.is_empty());
    }

    #[test]
    fn run_malformed_json_is_no_op() {
        let mut out = Vec::new();
        let mut killed: Vec<String> = Vec::new();
        run_session_start(
            &mut "{ not valid json".as_bytes(),
            &mut out,
            &mut |id| killed.push(id.to_string()),
        )
        .unwrap();
        assert!(out.is_empty(), "malformed JSON must be a silent no-op");
        assert!(killed.is_empty());
    }
}
