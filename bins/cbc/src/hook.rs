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
    /// This session's `--as <identity>`, recovered from the `poll-label` line
    /// when it carries the full `cbc poll … --as <id>` command.  `None` for a
    /// bare label with no flags.  Lets a legacy worker file (no `connections:`
    /// block) still relaunch identity-scoped instead of stripping its `--as`.
    pub identity: Option<String>,
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

/// A worker's attachment mode, declared in its instructions file as
/// `mode: worker | direct`.
///
/// `Worker` = attached to an orchestrator: the orchestrator is the default
/// counterpart for everything that would otherwise go to the user (questions,
/// plans, merge approval).  `Direct` = standalone: no orchestrator, the user is
/// the authority.  Absent ⇒ `Direct` (today's standalone behavior).
///
/// Parsed here as part of the B1 two-file model; the consumer is the B6
/// `PreToolUse` deny of `AskUserQuestion` for `mode: worker`, which lands in a
/// later increment — hence `allow(dead_code)` until then.
#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq, Clone, Copy, Default)]
pub enum WorkerMode {
    /// Attached to an orchestrator.
    Worker,
    /// Standalone — pairs with the user or another worker directly.
    #[default]
    Direct,
}

/// One declared poll connection: exactly the args a `cbc poll` invocation needs.
///
/// Parsed from a `connections:` block line of the form
/// `  <name>: <room-id> --as <identity> --model <model>`.  The line literally
/// carries the poll args so the reconcile can both *match* a running poll and
/// *relaunch* a dead one from the same source of truth.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct DeclaredConnection {
    /// Human label for the counterpart (the peer/worker name).
    pub name: String,
    /// The bare CBC room id.
    pub room_id: String,
    /// This session's `--as <identity>` for the poll, when declared.  `None`
    /// for back-compat with pre-identity files (reconcile then falls back to a
    /// room-only match, accepting the friendly-fire risk the skill migration
    /// removes).
    pub identity: Option<String>,
    /// The model the poll runs as.  Falls back to `"<model>"` when absent.
    pub model: String,
}

/// Parse a `connections:` block into declared poll connections.
///
/// The block is a `connections:` header followed by indented `  name: room-id
/// [--as identity] [--model model]` lines.  A non-indented non-empty line ends
/// the block.  Returns an empty vec when no block or no entries are present.
pub fn parse_connections(content: &str) -> Vec<DeclaredConnection> {
    let mut conns = Vec::new();
    let mut in_block = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "connections:" {
            in_block = true;
            continue;
        }
        if !in_block {
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        // A non-indented non-empty line ends the block.
        if !line.starts_with(' ') && !line.starts_with('\t') {
            in_block = false;
            continue;
        }
        // `<name>: <room-id> [--as <identity>] [--model <model>]`
        let Some(colon) = trimmed.find(':') else {
            continue;
        };
        let name = trimmed[..colon].trim().to_string();
        let rest = trimmed[colon + 1..].trim();
        let mut toks = rest.split_whitespace();
        let Some(room_id) = toks.next() else {
            continue;
        };
        // Scan remaining tokens for --as / --model flag values (shared helper).
        conns.push(DeclaredConnection {
            name,
            room_id: room_id.to_string(),
            identity: flag_value(rest, "--as"),
            model: flag_value(rest, "--model").unwrap_or_else(|| "<model>".to_string()),
        });
    }
    conns
}

/// Parse the worker `mode:` field.  Absent or unrecognized ⇒ `Direct`.
///
/// B1 file-model parser; consumed by the B6 `PreToolUse` hook in a later
/// increment, so `allow(dead_code)` holds the API until that wiring lands.
#[allow(dead_code)]
pub fn parse_worker_mode(content: &str) -> WorkerMode {
    for line in content.lines() {
        if let Some(v) = kv(line.trim(), "mode") {
            return match v {
                "worker" => WorkerMode::Worker,
                _ => WorkerMode::Direct,
            };
        }
    }
    WorkerMode::Direct
}

// ── Parsing helpers ───────────────────────────────────────────────────────────

/// Extract the value part of a `key: value` line.  Returns `Some(value.trim())`
/// when the line starts with `key:` (any trailing whitespace stripped).
fn kv<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.strip_prefix(key)
        .and_then(|rest| rest.strip_prefix(':'))
        .map(str::trim)
}

/// Return the whitespace-delimited token that follows `flag` in `source`, if
/// any.  A trailing `#` comment is ignored: scanning stops at the first token
/// that begins with `#`, so an `--as`/`--model` mentioned only in prose (e.g.
/// `repo-worker  # remember --as <id>`) is NOT mistaken for a real flag — that
/// would fabricate a wrong identity and recreate the 400.  Among the real
/// (pre-comment) tokens the FIRST occurrence wins.  Shared by `parse_connections`
/// (the `connections:` block) and `parse_worker` (the legacy `poll-label` command
/// line) so both extract flags identically.  A value that is itself another
/// `--flag` (a malformed line with a missing argument, e.g. `--as --model x`)
/// is rejected — returning it would fabricate a garbage handle that
/// `poll_matches` can never match, recreating the friendly-fire this guards.
fn flag_value(source: &str, flag: &str) -> Option<String> {
    let toks: Vec<&str> = source
        .split_whitespace()
        .take_while(|t| !t.starts_with('#'))
        .collect();
    toks.iter()
        .position(|t| *t == flag)
        .and_then(|i| toks.get(i + 1))
        .filter(|v| !v.starts_with("--"))
        // Real-world `poll-label` values are often descriptive prose, not the
        // canonical command line — e.g. `(bg task X, --as <id>) -- relaunched…`.
        // With no space before the closing paren, whitespace-splitting leaves
        // it stuck to the value. Trim trailing characters that can't appear in
        // an identity so punctuation from the surrounding prose never leaks
        // into the recovered value (a corrupted identity means the hook's kill
        // never matches the real poll and the relaunch mints yet another one).
        .map(|s| s.trim_end_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_'))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Return `true` when the file contains `status: ACTIVE`.
fn is_active(content: &str) -> bool {
    content
        .lines()
        .any(|l| kv(l.trim(), "status") == Some("ACTIVE"))
}

/// The `session-id:` stamp identifying the Claude Code session that owns a
/// state file (the skills write `$CLAUDE_CODE_SESSION_ID` on creation and
/// re-stamp on every resume, because the id rotates there).  `None` for a
/// pre-stamp legacy file.
///
/// First occurrence wins — a file-format invariant: the templates put the stamp
/// in the Status header (before any log/prose), so the header stamp always
/// beats a stale line lingering later in the file.
fn owner_session(content: &str) -> Option<String> {
    content
        .lines()
        .find_map(|l| kv(l.trim(), "session-id"))
        .filter(|v| !v.is_empty())
        .map(str::to_string)
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
    let poll_label = poll_label.unwrap_or_default();
    // Recover poll flags the worker already wrote into its `poll-label` command
    // line.  Pre-`connections:`-block files (the live legacy shape) keep their
    // `--as <id>` only here; honoring it lets the reconcile relaunch
    // identity-scoped instead of stripping the `--as` and 400-ing.
    let identity = flag_value(&poll_label, "--as");
    // `model:` field is canonical; fall back to a `--model` in the poll-label,
    // then the placeholder.
    let model = model
        .or_else(|| flag_value(&poll_label, "--model"))
        .unwrap_or_else(|| "<model>".to_string());
    Some(WorkerEntry {
        room_id,
        identity,
        model,
        poll_label,
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

// ── Reconcile primitives (B2) ──────────────────────────────────────────────────

/// What the Stop/SessionStart reconcile must do for one declared connection,
/// given how many of its polls are currently alive and whether one was launched
/// in the just-ended turn.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ReconcileAction {
    /// Exactly one healthy poll — leave it alone (never thrash).
    Ok,
    /// More than one — the hook kills the surplus itself (identity-scoped, so
    /// safe), leaving one. The kill re-derives the exact pids to reap from the
    /// live process table at kill time (see `kill_surplus_polls`), so no count
    /// is carried here — a planned count would only go stale before the kill.
    KillExtras,
    /// Zero polls and none launched this turn — surface a NON-blocking advisory
    /// to relaunch. A held room should ALWAYS have one standing poll running, so
    /// zero means it genuinely died (a timer beat is a backstop that re-arms it,
    /// never a substitute). Deliberately does NOT block turn-end: a forced block
    /// was the engine of the overnight relaunch-thrash loop (block → relaunch →
    /// SIGTERM → block …). The hook is a BACKUP hint that catches a dead standing
    /// poll, not the relaunch authority.
    AdviseRelaunch,
}

/// The pure reconcile decision.  `n` = live polls matching this connection's
/// `{room, identity}`; `launched_this_turn` = the transcript shows a matching
/// `cbc poll` launch in the turn that just ended (race window: a brand-new poll
/// isn't process-visible yet).
pub fn reconcile_action(n: usize, launched_this_turn: bool) -> ReconcileAction {
    match n {
        1 => ReconcileAction::Ok,
        0 if launched_this_turn => ReconcileAction::Ok,
        0 => ReconcileAction::AdviseRelaunch,
        _ => ReconcileAction::KillExtras,
    }
}

/// The exact relaunch / declared poll command for a connection.  This is both
/// what the reconcile surfaces on `AdviseRelaunch` and the canonical form a
/// running poll is matched against — one source of truth.
pub fn poll_command(room_id: &str, model: &str, identity: Option<&str>) -> String {
    match identity {
        Some(id) => format!("cbc poll {room_id} --model {model} --as {id}"),
        None => format!("cbc poll {room_id} --model {model}"),
    }
}

/// The unified declared-connection list for a state file, bridging the new
/// `connections:` block and the legacy single-`room-id:` (worker) / `agents:`
/// (orchestrator) shapes.  Precedence: a `connections:` block wins; otherwise
/// fall back to the legacy shape (identity `None`, model from the legacy
/// fields).  This is the single source of truth the reconcile polls against.
pub fn declared_connections(content: &str) -> Vec<DeclaredConnection> {
    // New shape wins.
    let from_block = parse_connections(content);
    if !from_block.is_empty() {
        return from_block;
    }
    // Legacy orchestrator: agents: block.
    if let Some(o) = parse_orchestrator(content) {
        return o
            .rooms
            .into_iter()
            .map(|(name, room_id)| DeclaredConnection {
                name,
                room_id,
                identity: None,
                model: o.model.clone(),
            })
            .collect();
    }
    // Legacy worker: single room-id.  Recover the `--as` identity from the
    // poll-label when present (the file's own command line), so a worker file
    // written before the `connections:` block still reconciles identity-scoped.
    if let Some(w) = parse_worker(content) {
        return vec![DeclaredConnection {
            name: w.poll_label,
            room_id: w.room_id,
            identity: w.identity,
            model: w.model,
        }];
    }
    Vec::new()
}

/// Decide whether a process command line is a live `cbc poll` for `room`
/// (optionally scoped to `identity`).  Encodes the two B0.5 traps:
///
/// * The `/bin/zsh -c "…cbc poll…"` background-task *wrapper* must NOT match —
///   only the real `cbc` child counts, else every poll is double-counted.  We
///   require `argv[0]`'s basename to be exactly `cbc` and `argv[1]` to be
///   `poll`; a shell wrapper has `argv[0] = zsh`, so it is excluded.
/// * When `identity` is `Some`, the line MUST carry the matching `--as <id>`,
///   so one session's reconcile never counts or kills another session's poll of
///   the same shared room.
pub fn poll_matches(cmdline: &str, room: &str, identity: Option<&str>) -> bool {
    let toks: Vec<&str> = cmdline.split_whitespace().collect();
    // argv[0] basename == "cbc", argv[1] == "poll", argv[2] == room.
    let argv0_is_cbc = toks
        .first()
        .map(|a| a.rsplit('/').next().unwrap_or(a) == "cbc")
        .unwrap_or(false);
    if !argv0_is_cbc {
        return false;
    }
    if toks.get(1) != Some(&"poll") {
        return false;
    }
    if toks.get(2) != Some(&room) {
        return false;
    }
    match identity {
        None => true,
        Some(id) => toks
            .iter()
            .position(|t| *t == "--as")
            .and_then(|i| toks.get(i + 1))
            .map(|v| *v == id)
            .unwrap_or(false),
    }
}

/// One identity-scoped kill the reconcile performs itself (surplus polls).
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct KillOrder {
    pub room_id: String,
    pub identity: Option<String>,
}

/// The reconcile plan for one Stop event: surplus kills to perform now, and the
/// relaunch commands to surface as a non-blocking advisory.
#[derive(Debug, PartialEq, Eq, Clone, Default)]
pub struct StopPlan {
    pub kills: Vec<KillOrder>,
    pub relaunch: Vec<String>,
}

/// Pure reconcile planner over the declared connections.  `count` returns the
/// number of live identity-scoped polls for `{room, identity}`; `launched`
/// returns whether a matching poll was launched in the just-ended turn.
pub fn plan_stop(
    conns: &[DeclaredConnection],
    mut count: impl FnMut(&str, Option<&str>) -> usize,
    mut launched: impl FnMut(&str, Option<&str>) -> bool,
) -> StopPlan {
    let mut plan = StopPlan::default();
    for c in conns {
        let id = c.identity.as_deref();
        let n = count(&c.room_id, id);
        match reconcile_action(n, launched(&c.room_id, id)) {
            ReconcileAction::Ok => {}
            ReconcileAction::KillExtras => plan.kills.push(KillOrder {
                room_id: c.room_id.clone(),
                identity: c.identity.clone(),
            }),
            ReconcileAction::AdviseRelaunch => {
                plan.relaunch.push(poll_command(&c.room_id, &c.model, id))
            }
        }
    }
    plan
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

/// Scan `cbc_dir` for active state files and return every declared connection
/// across all of them, in deterministic order.  Any active `.md` whose content
/// yields connections (new `connections:` block or legacy `room-id`/`agents:`)
/// contributes; non-active or non-CBC files contribute nothing.
///
/// SESSION SCOPING: co-located agents (an orchestrator and its workers, whose
/// shell cwd snaps back to the repo root) all write to ONE shared `.cbc/`, so an
/// unscoped scan made every session's Stop nag about every OTHER session's rooms
/// — every turn, under the other session's `--as` identity (the cross-session
/// nightmare of 2026-07-08). With `session_id` = `Some(sid)` (the Stop payload's
/// id), only files whose `session-id:` stamp equals `sid` contribute; files
/// stamped by another session AND unstamped legacy files are skipped — ownership
/// that can't be proven must never nag (fails CLOSED: the worst case is a lost
/// backup hint for the file's real owner until it re-stamps on its next resume,
/// never a cross-nag). With `None` (payload without a session id: non-Claude-Code
/// harness, or the SessionStart relaunch path where the rotating id is stale by
/// construction — see `run_session_start`), all active files contribute, exactly
/// the pre-scoping behavior.
pub fn scan_declared(cbc_dir: &Path, session_id: Option<&str>) -> Vec<DeclaredConnection> {
    let mut out = Vec::new();
    let iter = match std::fs::read_dir(cbc_dir) {
        Ok(it) => it,
        Err(_) => return out,
    };
    let mut paths: Vec<PathBuf> = iter
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
        .collect();
    paths.sort();
    for path in paths {
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if !is_active(&content) {
            continue;
        }
        if let Some(sid) = session_id {
            if owner_session(&content).as_deref() != Some(sid) {
                continue;
            }
        }
        out.extend(declared_connections(&content));
    }
    out
}

/// The Stop-hook advisory JSON — a NON-blocking nudge to relaunch absent polls.
///
/// Deliberately NOT a top-level `{"decision":"block"}`: blocking turn-end was the
/// engine of the overnight relaunch-thrash loop (block → forced relaunch →
/// harness SIGTERM in seconds → next Stop sees zero → block again, all night). So
/// the relaunch hint rides `hookSpecificOutput.additionalContext` (the
/// non-blocking channel): surfaced as context where the harness supports it,
/// harmlessly inert where it does not, and never forcing a turn either way.
/// Liveness is owned by the always-running standing poll (one per held room); a
/// timer beat is only a backstop that re-arms a dead one, never a substitute. This
/// hook is a BACKUP hint that catches a standing poll that actually died. See
/// code.claude.com/docs/en/hooks.md.
fn stop_advise_json(relaunch: &[String]) -> String {
    let mut ctx = String::from(
        "CBC liveness (advisory) — one or more declared rooms have NO live poll right now and \
         none was launched this turn. A held room must ALWAYS have one standing `cbc poll` \
         running; if you still hold these rooms, relaunch each as a background task now \
         (run_in_background). A timer beat is only a backstop that re-arms a dead standing poll, \
         never a substitute for it. This is a hint, not a block:",
    );
    for cmd in relaunch {
        ctx.push_str("\n  ");
        ctx.push_str(cmd);
    }
    let payload = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "Stop",
            "additionalContext": ctx,
        }
    });
    payload.to_string()
}

/// Handle a `Stop` hook event: the per-turn poll reconcile (B2).
///
/// Reads the hook JSON (`cwd`, `stop_hook_active`, `session_id`), scans
/// `<cwd>/.cbc` for declared connections — scoped to the files stamped with the
/// payload's `session_id` (see [`scan_declared`]) — and for each reconciles its
/// live poll count against exactly-one.  Surplus polls are killed via `kill_fn` (identity-scoped, safe).
/// If any room has zero polls and none was launched this turn — and the loop
/// guard (`stop_hook_active`) is not set — it surfaces a NON-blocking advisory
/// (never blocks turn-end) listing the relaunch commands.  `count_fn` /
/// `launched_fn` are seams over the process table and the transcript.
pub fn run_stop<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    count_fn: &mut dyn FnMut(&str, Option<&str>) -> usize,
    launched_fn: &mut dyn FnMut(&str, Option<&str>) -> bool,
    kill_fn: &mut dyn FnMut(&KillOrder),
) -> anyhow::Result<()> {
    let mut raw = String::new();
    reader
        .read_to_string(&mut raw)
        .context("reading Stop hook JSON from stdin")?;
    if raw.trim().is_empty() {
        return Ok(());
    }
    let json: Value = serde_json::from_str(&raw).unwrap_or(Value::Object(Default::default()));
    let stop_hook_active = json
        .get("stop_hook_active")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let cwd: PathBuf = json
        .get("cwd")
        .and_then(|c| c.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    // Empty ⇒ treated as missing: degrade to the documented include-all
    // back-compat, never an accidental skip-all that would silently disable the
    // reconcile for the session's own rooms.
    let session_id = json
        .get("session_id")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty());

    let conns = scan_declared(&cwd.join(".cbc"), session_id);
    if conns.is_empty() {
        return Ok(());
    }

    let plan = plan_stop(
        &conns,
        |room, id| count_fn(room, id),
        |room, id| launched_fn(room, id),
    );

    // Kill surplus polls ourselves — identity-scoped, so always safe.
    for order in &plan.kills {
        kill_fn(order);
    }

    // Surface a NON-blocking relaunch advisory when one is owed AND the loop
    // guard is clear. Never blocks turn-end (see `stop_advise_json`): a forced
    // block was the overnight relaunch-thrash engine, and the standing poll — not
    // a timer beat, and not this hook — owns liveness (the beat is only a backstop).
    if !plan.relaunch.is_empty() && !stop_hook_active {
        write!(writer, "{}", stop_advise_json(&plan.relaunch))?;
    }
    Ok(())
}

// ── Output ────────────────────────────────────────────────────────────────────

/// Inoculation appended to every post-compaction relaunch directive: a
/// pre-compaction poll can read alive (process up, `poll_live: true`) while
/// orphaned from THIS session and unable to deliver, so no liveness check may
/// gate the relaunch.  Written into both `emit_relaunch` arms via one `const`
/// so the two copies cannot silently drift (both inoculation tests assert this
/// exact text).
const POLL_LIVE_INOCULATION: &str =
    "Do NOT check poll_live or ps to decide whether to skip this — a \
     pre-compaction poll can read alive (process running, poll_live: true) \
     while orphaned from THIS session and unable to reach it. Relaunch \
     regardless of what any liveness check says.";

/// Write a relaunch directive for `entry` to `out`.  For every room it resolves
/// the session's `--as <identity>` via `id_of` (from the declared-connections
/// block), calls `kill_fn(room, identity)` to reap the stale poll *identity-scoped*
/// (so a peer's poll of a shared room is never touched), and emits the relaunch
/// command carrying that same `--as` — so the command the model relaunches matches
/// what the Stop reconcile counts.  `id_of` returns `None` for legacy files with no
/// identity, in which case the command is the room-only form (byte-identical to the
/// pre-identity output) and the kill falls back to room-wide (documented residual
/// for un-migrated files).
pub fn emit_relaunch<W: Write>(
    entry: &ActiveEntry,
    out: &mut W,
    id_of: &dyn Fn(&str) -> Option<String>,
    kill_fn: &mut dyn FnMut(&str, Option<&str>),
) -> std::io::Result<()> {
    match entry {
        ActiveEntry::Worker(w) => {
            let id = id_of(&w.room_id);
            kill_fn(&w.room_id, id.as_deref());
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
            let cmd = poll_command(&w.room_id, &w.model, id.as_deref());
            if w.poll_label.is_empty() {
                writeln!(out, "  {cmd}")?;
            } else {
                writeln!(out, "  {cmd}      # {}", w.poll_label)?;
            }
            writeln!(
                out,
                "The room is ACTIVE; you are deaf until this poll is running."
            )?;
            writeln!(out, "{POLL_LIVE_INOCULATION}")?;
        }
        ActiveEntry::Orchestrator(o) => {
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
                let id = id_of(room_id);
                kill_fn(room_id, id.as_deref());
                let cmd = poll_command(room_id, &o.model, id.as_deref());
                writeln!(out, "  {cmd}      # {name} poll")?;
            }
            writeln!(
                out,
                "All rooms are ACTIVE; you are deaf until all polls are running."
            )?;
            writeln!(out, "{POLL_LIVE_INOCULATION}")?;
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
    kill_fn: &mut dyn FnMut(&str, Option<&str>),
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

    // Resolve each room's `--as <identity>` from the declared-connections block so
    // the kill and the relaunch command are identity-scoped (B3 friendly-fire fix).
    // Legacy files contribute no identity ⇒ `None` ⇒ room-wide kill + room-only
    // relaunch command (the documented un-migrated residual).
    //
    // Session UNSCOPED (`None`) on purpose, unlike the Stop reconcile: the session
    // id rotates exactly at this resume/compact boundary, so the session's own file
    // still carries its PRE-resume stamp here — an owner match would skip it and
    // reintroduce post-compaction deafness (#103). The relaunch directive exists to
    // prevent deafness; it must never be gated on a key that is stale by
    // construction at the moment it runs.
    let declared = scan_declared(&cbc_dir, None);
    let id_of = |room: &str| -> Option<String> {
        declared
            .iter()
            .find(|c| c.room_id == room)
            .and_then(|c| c.identity.clone())
    };

    for entry in &entries {
        emit_relaunch(entry, writer, &id_of, kill_fn)?;
    }

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── reconcile_action ──────────────────────────────────────────────────────

    #[test]
    fn reconcile_one_poll_is_ok() {
        assert_eq!(reconcile_action(1, false), ReconcileAction::Ok);
        assert_eq!(reconcile_action(1, true), ReconcileAction::Ok);
    }

    #[test]
    fn reconcile_zero_not_launched_advises_relaunch() {
        assert_eq!(reconcile_action(0, false), ReconcileAction::AdviseRelaunch);
    }

    #[test]
    fn reconcile_zero_launched_this_turn_is_ok_race_window() {
        assert_eq!(
            reconcile_action(0, true),
            ReconcileAction::Ok,
            "a poll launched this turn isn't process-visible yet — must not block"
        );
    }

    #[test]
    fn reconcile_more_than_one_is_kill_extras() {
        assert_eq!(reconcile_action(5, false), ReconcileAction::KillExtras);
        assert_eq!(
            reconcile_action(2, true),
            ReconcileAction::KillExtras,
            "launched-this-turn is irrelevant when polls already exist"
        );
    }

    // ── poll_command ──────────────────────────────────────────────────────────

    #[test]
    fn poll_command_includes_identity_when_present() {
        assert_eq!(
            poll_command("room-20260625-1000", "claude-sonnet-4-6", Some("api-cbc")),
            "cbc poll room-20260625-1000 --model claude-sonnet-4-6 --as api-cbc"
        );
    }

    #[test]
    fn poll_command_omits_identity_when_absent() {
        assert_eq!(
            poll_command("room-20260625-1000", "sonnet", None),
            "cbc poll room-20260625-1000 --model sonnet"
        );
    }

    // ── poll_matches (B0.5 traps) ─────────────────────────────────────────────

    #[test]
    fn poll_matches_real_child_process() {
        let line = "cbc poll report-orch-20260624-0502 --model claude-opus-4-8 --as api-cbc";
        assert!(poll_matches(
            line,
            "report-orch-20260624-0502",
            Some("api-cbc")
        ));
    }

    #[test]
    fn poll_matches_accepts_absolute_argv0_path() {
        let line = "/Users/me/.cargo/bin/cbc poll room-20260625-1000 --model sonnet --as me";
        assert!(poll_matches(line, "room-20260625-1000", Some("me")));
    }

    #[test]
    fn poll_matches_rejects_zsh_wrapper_to_avoid_double_count() {
        // The run_in_background wrapper — must NOT match, else every poll counts twice.
        let wrapper = "/bin/zsh -c cbc poll room-20260625-1000 --model sonnet --as me";
        assert!(
            !poll_matches(wrapper, "room-20260625-1000", Some("me")),
            "the /bin/zsh -c wrapper must be excluded (B0.5 trap a)"
        );
    }

    #[test]
    fn poll_matches_rejects_foreign_identity() {
        let line = "cbc poll shared-room-20260625-1000 --model sonnet --as OTHER-session";
        assert!(
            !poll_matches(line, "shared-room-20260625-1000", Some("my-session")),
            "a different --as must not match (B0.5 trap b: friendly-fire)"
        );
    }

    #[test]
    fn poll_matches_rejects_different_room() {
        let line = "cbc poll room-A-20260625-1000 --model sonnet --as me";
        assert!(!poll_matches(line, "room-B-20260625-1000", Some("me")));
    }

    #[test]
    fn poll_matches_identity_none_ignores_as_flag() {
        let line = "cbc poll room-20260625-1000 --model sonnet --as whoever";
        assert!(
            poll_matches(line, "room-20260625-1000", None),
            "identity None ⇒ room-only match (back-compat)"
        );
    }

    // ── scan_declared + run_stop ──────────────────────────────────────────────

    #[test]
    fn scan_declared_collects_across_active_files_only() {
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join(".cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(
            cbc.join("api-instructions-20260625.md"),
            "status: ACTIVE\nconnections:\n  orch: room-a-20260625-1000 --as api --model opus\n",
        )
        .unwrap();
        std::fs::write(
            cbc.join("worker-legacy-20260625.md"),
            "status: ACTIVE\nroom-id: room-b-20260625-1005\nmodel: sonnet\n",
        )
        .unwrap();
        std::fs::write(
            cbc.join("old-done-20260620.md"),
            "status: DONE\nroom-id: room-z\n",
        )
        .unwrap();

        let conns = scan_declared(&cbc, None);
        let rooms: Vec<&str> = conns.iter().map(|c| c.room_id.as_str()).collect();
        assert!(rooms.contains(&"room-a-20260625-1000"));
        assert!(rooms.contains(&"room-b-20260625-1005"));
        assert!(!rooms.contains(&"room-z"), "DONE file excluded");
    }

    #[test]
    fn scan_declared_scopes_to_owning_session() {
        // THE cross-session guard: co-located sessions share one repo-root
        // `.cbc/`; a session's reconcile must see ONLY the files stamped with
        // its own session id — never a co-located session's rooms.
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join(".cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(
            cbc.join("worker-mine-20260708.md"),
            "status: ACTIVE\nsession-id: sess-aaa\nconnections:\n  orch: room-mine-20260708-1000 --as me --model opus\n",
        )
        .unwrap();
        std::fs::write(
            cbc.join("worker-theirs-20260708.md"),
            "status: ACTIVE\nsession-id: sess-bbb\nconnections:\n  orch: room-theirs-20260708-1005 --as them --model opus\n",
        )
        .unwrap();

        let conns = scan_declared(&cbc, Some("sess-aaa"));
        let rooms: Vec<&str> = conns.iter().map(|c| c.room_id.as_str()).collect();
        assert_eq!(
            rooms,
            vec!["room-mine-20260708-1000"],
            "a session-scoped scan must return only the caller's own rooms"
        );
    }

    #[test]
    fn scan_declared_skips_unstamped_when_session_known() {
        // An unstamped (legacy) file has unprovable ownership. When the caller
        // knows its session id, skipping is the only choice that can never
        // cross-nag — the file's owner re-stamps on its next resume.
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join(".cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(
            cbc.join("worker-legacy-20260707.md"),
            "status: ACTIVE\nconnections:\n  orch: room-legacy-20260707-1828 --as fresh --model opus\n",
        )
        .unwrap();

        let conns = scan_declared(&cbc, Some("sess-aaa"));
        assert!(
            conns.is_empty(),
            "unstamped files must be skipped when the session is known; got {conns:?}"
        );
    }

    #[test]
    fn scan_declared_includes_all_when_session_unknown() {
        // No session id in the payload (non-Claude-Code harness, older hook
        // wiring): scoping is impossible — keep today's include-all behavior.
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join(".cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(
            cbc.join("worker-stamped-20260708.md"),
            "status: ACTIVE\nsession-id: sess-bbb\nconnections:\n  orch: room-stamped-20260708-1000 --as them --model opus\n",
        )
        .unwrap();
        std::fs::write(
            cbc.join("worker-legacy-20260707.md"),
            "status: ACTIVE\nconnections:\n  orch: room-legacy-20260707-1828 --as fresh --model opus\n",
        )
        .unwrap();

        let conns = scan_declared(&cbc, None);
        let rooms: Vec<&str> = conns.iter().map(|c| c.room_id.as_str()).collect();
        assert!(rooms.contains(&"room-stamped-20260708-1000"));
        assert!(rooms.contains(&"room-legacy-20260707-1828"));
    }

    fn stop_input(cwd: &Path, stop_hook_active: bool) -> String {
        serde_json::json!({
            "hook_event_name": "Stop",
            "stop_hook_active": stop_hook_active,
            "cwd": cwd.to_str().unwrap(),
        })
        .to_string()
    }

    #[test]
    fn run_stop_advises_relaunch_for_dead_poll_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join(".cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(
            cbc.join("api-instructions-20260625.md"),
            "status: ACTIVE\nconnections:\n  orch: room-a-20260625-1000 --as api --model opus\n",
        )
        .unwrap();

        let input = stop_input(dir.path(), false);
        let mut out = Vec::new();
        let mut kills: Vec<KillOrder> = Vec::new();
        run_stop(
            &mut input.as_bytes(),
            &mut out,
            &mut |_, _| 0,     // no live poll
            &mut |_, _| false, // not launched this turn
            &mut |o| kills.push(o.clone()),
        )
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&text).expect("Stop advisory output must be valid JSON");
        // KEY REGRESSION GUARD: the Stop hook must NEVER emit a top-level
        // `decision: block` — blocking turn-end was the engine of the overnight
        // relaunch-thrash loop and is incompatible with the timer-driven model
        // (bounded beat polls exit by design → every Stop would see zero).
        assert!(
            v.get("decision").is_none(),
            "Stop must not block turn-end; got a `decision` field:\n{text}"
        );
        // The relaunch command rides the NON-blocking additionalContext channel.
        let ctx = v
            .get("hookSpecificOutput")
            .and_then(|h| h.get("additionalContext"))
            .and_then(|c| c.as_str())
            .unwrap_or("");
        assert!(
            ctx.contains("cbc poll room-a-20260625-1000 --model opus --as api"),
            "the relaunch command must ride the non-blocking additionalContext; got:\n{text}"
        );
        assert!(kills.is_empty());
    }

    #[test]
    fn run_stop_legacy_worker_advisory_carries_recovered_as() {
        // Stop-path twin of run_compact_legacy_worker_recovers_identity_…: the
        // ACTUAL bug shape — a LEGACY worker file (no `connections:` block) whose
        // poll-label holds the full `cbc poll … --as <id>` command.  With no live
        // poll, run_stop surfaces a NON-blocking advisory whose relaunch command
        // must carry the recovered `--as` (else a relaunch would 400 identity-less).
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join(".cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(
            cbc.join("worker-legacy-fullcmd-20260626.md"),
            "status: ACTIVE\nroom-id: cbc-report-seeds-20260626-0027\npoll-label: cbc poll cbc-report-seeds-20260626-0027 --as mvp-api-9df0 --model claude-sonnet-4-6\nmodel: claude-sonnet-4-6\n",
        )
        .unwrap();

        let input = stop_input(dir.path(), false);
        let mut out = Vec::new();
        let mut kills: Vec<KillOrder> = Vec::new();
        run_stop(
            &mut input.as_bytes(),
            &mut out,
            &mut |_, _| 0,     // no live poll
            &mut |_, _| false, // not launched this turn
            &mut |o| kills.push(o.clone()),
        )
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&text).expect("Stop advisory output must be valid JSON");
        assert!(
            v.get("decision").is_none(),
            "the Stop hook must not block turn-end; got:\n{text}"
        );
        let ctx = v
            .get("hookSpecificOutput")
            .and_then(|h| h.get("additionalContext"))
            .and_then(|c| c.as_str())
            .unwrap_or("");
        assert!(
            ctx.contains(
                "cbc poll cbc-report-seeds-20260626-0027 --model claude-sonnet-4-6 --as mvp-api-9df0"
            ),
            "the advisory relaunch command must carry the `--as` recovered from the legacy \
             poll-label (else a relaunch 400s); got:\n{ctx}"
        );
    }

    #[test]
    fn run_stop_does_not_nag_about_another_sessions_room() {
        // THE cross-session nightmare (live transcript, 2026-07-08): a co-located
        // session's ACTIVE file made every OTHER session's Stop advise relaunching
        // that room — every turn, under the other session's identity. The Stop
        // payload carries session_id; only files stamped with it may reconcile.
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join(".cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        // My file — my standing poll is healthy.
        std::fs::write(
            cbc.join("worker-vet-exam-20260708.md"),
            "status: ACTIVE\nsession-id: sess-mine\nconnections:\n  orch: room-mine-20260708-1756 --as engine-worker-vet-exam --model opus\n",
        )
        .unwrap();
        // A co-located session's file — MY session runs no poll for it (count 0).
        std::fs::write(
            cbc.join("worker-fresh-20260707.md"),
            "status: ACTIVE\nsession-id: sess-other\nconnections:\n  orch: room-other-20260707-1828 --as engine-worker-fresh --model opus\n",
        )
        .unwrap();

        let input = serde_json::json!({
            "hook_event_name": "Stop",
            "stop_hook_active": false,
            "cwd": dir.path().to_str().unwrap(),
            "session_id": "sess-mine",
        })
        .to_string();
        let mut out = Vec::new();
        run_stop(
            &mut input.as_bytes(),
            &mut out,
            &mut |room, _| usize::from(room == "room-mine-20260708-1756"),
            &mut |_, _| false,
            &mut |o| panic!("no kill expected, got {o:?}"),
        )
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(
            !text.contains("room-other-20260707-1828"),
            "Stop must NEVER advise about another session's room; got:\n{text}"
        );
        assert!(
            text.is_empty(),
            "own poll healthy + foreign file not mine ⇒ no advisory owed at all; got:\n{text}"
        );
    }

    #[test]
    fn run_stop_scoped_positive_own_dead_poll_still_advises() {
        // The other half of the scoping guarantee: filtering must never eat the
        // LEGITIMATE advisory for the session's own dead poll. Without this, an
        // over-eager filter (or a payload-key mismatch) that skips everything
        // would still pass the cross-session negative test — silence satisfies
        // both. Payload session_id matches the stamp, poll count is 0 ⇒ the
        // advisory must fire, and for exactly the own room.
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join(".cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(
            cbc.join("worker-mine-20260708.md"),
            "status: ACTIVE\nsession-id: sess-mine\nconnections:\n  orch: room-mine-20260708-1756 --as engine-worker-mine --model opus\n",
        )
        .unwrap();
        std::fs::write(
            cbc.join("worker-other-20260707.md"),
            "status: ACTIVE\nsession-id: sess-other\nconnections:\n  orch: room-other-20260707-1828 --as engine-worker-fresh --model opus\n",
        )
        .unwrap();

        let input = serde_json::json!({
            "hook_event_name": "Stop",
            "stop_hook_active": false,
            "cwd": dir.path().to_str().unwrap(),
            "session_id": "sess-mine",
        })
        .to_string();
        let mut out = Vec::new();
        run_stop(
            &mut input.as_bytes(),
            &mut out,
            &mut |_, _| 0,     // ALL polls dead — including my own
            &mut |_, _| false, // none launched this turn
            &mut |o| panic!("no kill expected, got {o:?}"),
        )
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains(
                "cbc poll room-mine-20260708-1756 --model opus --as engine-worker-mine"
            ),
            "scoping must not eat the advisory for the session's OWN dead poll; got:\n{text}"
        );
        assert!(
            !text.contains("room-other-20260707-1828"),
            "the foreign room must still be excluded; got:\n{text}"
        );
    }

    #[test]
    fn run_stop_empty_session_id_falls_back_to_include_all() {
        // A degenerate payload (`"session_id": ""`) must behave like a MISSING
        // session id — the documented include-all back-compat — not silently
        // skip every file (which would disable the reconcile for the session's
        // own rooms with no signal anywhere).
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join(".cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(
            cbc.join("worker-mine-20260708.md"),
            "status: ACTIVE\nsession-id: sess-mine\nconnections:\n  orch: room-mine-20260708-1756 --as engine-worker-mine --model opus\n",
        )
        .unwrap();

        let input = serde_json::json!({
            "hook_event_name": "Stop",
            "stop_hook_active": false,
            "cwd": dir.path().to_str().unwrap(),
            "session_id": "",
        })
        .to_string();
        let mut out = Vec::new();
        run_stop(
            &mut input.as_bytes(),
            &mut out,
            &mut |_, _| 0,
            &mut |_, _| false,
            &mut |o| panic!("no kill expected, got {o:?}"),
        )
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("room-mine-20260708-1756"),
            "empty session_id must degrade to include-all, not skip-all; got:\n{text}"
        );
    }

    #[test]
    fn stop_advisory_mandates_a_standing_poll_not_a_heartbeat() {
        // Defect #3: the advisory must frame a STANDING poll as the always-on
        // default — not tell the agent a timer heartbeat owns liveness, which
        // was the wording that authorized going deaf between beats.
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join(".cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(
            cbc.join("api-instructions-20260625.md"),
            "status: ACTIVE\nconnections:\n  orch: room-a-20260625-1000 --as api --model opus\n",
        )
        .unwrap();
        let input = stop_input(dir.path(), false);
        let mut out = Vec::new();
        run_stop(
            &mut input.as_bytes(),
            &mut out,
            &mut |_, _| 0,
            &mut |_, _| false,
            &mut |_| {},
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let ctx = v
            .get("hookSpecificOutput")
            .and_then(|h| h.get("additionalContext"))
            .and_then(|c| c.as_str())
            .unwrap_or("");
        assert!(
            ctx.contains("standing"),
            "advisory must mandate a standing poll; got:\n{ctx}"
        );
        assert!(
            !ctx.contains("owns liveness"),
            "advisory must not say a heartbeat owns liveness; got:\n{ctx}"
        );
    }

    #[test]
    fn run_stop_loop_guard_suppresses_advisory_when_stop_hook_active() {
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join(".cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(
            cbc.join("api-instructions-20260625.md"),
            "status: ACTIVE\nconnections:\n  orch: room-a-20260625-1000 --as api --model opus\n",
        )
        .unwrap();

        let input = stop_input(dir.path(), true); // loop guard engaged
        let mut out = Vec::new();
        let mut kills: Vec<KillOrder> = Vec::new();
        run_stop(
            &mut input.as_bytes(),
            &mut out,
            &mut |_, _| 0,
            &mut |_, _| false,
            &mut |o| kills.push(o.clone()),
        )
        .unwrap();

        assert!(
            out.is_empty(),
            "stop_hook_active must suppress re-block; got:\n{}",
            String::from_utf8_lossy(&out)
        );
    }

    #[test]
    fn run_stop_kills_surplus_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join(".cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(
            cbc.join("api-instructions-20260625.md"),
            "status: ACTIVE\nconnections:\n  orch: room-a-20260625-1000 --as api --model opus\n",
        )
        .unwrap();

        let input = stop_input(dir.path(), false);
        let mut out = Vec::new();
        let mut kills: Vec<KillOrder> = Vec::new();
        run_stop(
            &mut input.as_bytes(),
            &mut out,
            &mut |_, _| 3, // surplus
            &mut |_, _| false,
            &mut |o| kills.push(o.clone()),
        )
        .unwrap();

        assert!(out.is_empty(), "surplus-only must not block");
        assert_eq!(kills.len(), 1);
        assert_eq!(kills[0].room_id, "room-a-20260625-1000");
        assert_eq!(kills[0].identity.as_deref(), Some("api"));
    }

    #[test]
    fn run_stop_no_cbc_dir_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let input = stop_input(dir.path(), false);
        let mut out = Vec::new();
        let mut kills: Vec<KillOrder> = Vec::new();
        run_stop(
            &mut input.as_bytes(),
            &mut out,
            &mut |_, _| 0,
            &mut |_, _| false,
            &mut |o| kills.push(o.clone()),
        )
        .unwrap();
        assert!(out.is_empty());
        assert!(kills.is_empty());
    }

    // ── plan_stop ─────────────────────────────────────────────────────────────

    fn conn(name: &str, room: &str, id: Option<&str>, model: &str) -> DeclaredConnection {
        DeclaredConnection {
            name: name.to_string(),
            room_id: room.to_string(),
            identity: id.map(str::to_string),
            model: model.to_string(),
        }
    }

    #[test]
    fn plan_stop_advises_relaunch_for_dead_poll() {
        let conns = vec![conn("orch", "room-20260625-1000", Some("me"), "sonnet")];
        let plan = plan_stop(&conns, |_, _| 0, |_, _| false);
        assert!(plan.kills.is_empty());
        assert_eq!(
            plan.relaunch,
            vec!["cbc poll room-20260625-1000 --model sonnet --as me"]
        );
    }

    #[test]
    fn plan_stop_healthy_poll_no_action() {
        let conns = vec![conn("orch", "room-20260625-1000", Some("me"), "sonnet")];
        let plan = plan_stop(&conns, |_, _| 1, |_, _| false);
        assert_eq!(plan, StopPlan::default());
    }

    #[test]
    fn plan_stop_kills_surplus() {
        let conns = vec![conn("orch", "room-20260625-1000", Some("me"), "sonnet")];
        let plan = plan_stop(&conns, |_, _| 4, |_, _| false);
        assert!(plan.relaunch.is_empty());
        assert_eq!(
            plan.kills,
            vec![KillOrder {
                room_id: "room-20260625-1000".to_string(),
                identity: Some("me".to_string()),
            }]
        );
    }

    #[test]
    fn plan_stop_race_window_no_block() {
        let conns = vec![conn("orch", "room-20260625-1000", Some("me"), "sonnet")];
        let plan = plan_stop(&conns, |_, _| 0, |_, _| true);
        assert_eq!(plan, StopPlan::default(), "launched-this-turn ⇒ no block");
    }

    #[test]
    fn plan_stop_mixed_connections() {
        let conns = vec![
            conn("a", "room-a-20260625-1000", Some("me"), "sonnet"), // dead → relaunch
            conn("b", "room-b-20260625-1000", Some("me"), "opus"),   // healthy → ok
            conn("c", "room-c-20260625-1000", Some("me"), "sonnet"), // surplus → kill
        ];
        let plan = plan_stop(
            &conns,
            |room, _| match room {
                "room-a-20260625-1000" => 0,
                "room-b-20260625-1000" => 1,
                "room-c-20260625-1000" => 2,
                _ => 0,
            },
            |_, _| false,
        );
        assert_eq!(plan.relaunch.len(), 1);
        assert!(plan.relaunch[0].contains("room-a-20260625-1000"));
        assert_eq!(plan.kills.len(), 1);
        assert_eq!(plan.kills[0].room_id, "room-c-20260625-1000");
        assert_eq!(plan.kills[0].identity.as_deref(), Some("me"));
    }

    // ── declared_connections (unified) ────────────────────────────────────────

    #[test]
    fn declared_connections_prefers_connections_block() {
        let content = "\
status: ACTIVE
room-id: legacy-room-should-be-ignored
connections:
  orch: new-room-20260625-1200 --as me --model sonnet
";
        let c = declared_connections(content);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].room_id, "new-room-20260625-1200");
        assert_eq!(c[0].identity.as_deref(), Some("me"));
    }

    #[test]
    fn declared_connections_falls_back_to_worker_room_id() {
        let content = "\
status: ACTIVE
room-id: legacy-worker-room-20260625-1000
model: claude-sonnet-4-6
";
        let c = declared_connections(content);
        assert_eq!(c.len(), 1, "legacy worker ⇒ one connection");
        assert_eq!(c[0].room_id, "legacy-worker-room-20260625-1000");
        assert_eq!(c[0].identity, None, "legacy ⇒ no identity");
        assert_eq!(c[0].model, "claude-sonnet-4-6");
    }

    #[test]
    fn declared_connections_falls_back_to_orchestrator_agents() {
        let content = "\
status: ACTIVE
model: claude-opus-4-8

agents:
  repo-worker-feat: room-a-20260625-1100 (handle abc) — feat
  repo-worker-fix: room-b-20260625-1105 (handle def) — fix
";
        let c = declared_connections(content);
        assert_eq!(c.len(), 2, "legacy orchestrator ⇒ one per agent");
        assert_eq!(c[0].room_id, "room-a-20260625-1100");
        assert_eq!(c[0].model, "claude-opus-4-8", "uses orchestrator model");
        assert_eq!(c[0].identity, None);
        assert_eq!(c[1].room_id, "room-b-20260625-1105");
    }

    // ── parse_connections ─────────────────────────────────────────────────────

    #[test]
    fn parse_connections_extracts_room_identity_and_model() {
        let content = "\
status: ACTIVE

connections:
  engine-orch: report-engine-orch-20260624-0502 --as api-cbc --model claude-opus-4-8
  pdf-worker:  pdf-extractor-20260624-0600 --as api-cbc --model claude-sonnet-4-6
";
        let conns = parse_connections(content);
        assert_eq!(conns.len(), 2, "should parse both connection lines");
        assert_eq!(conns[0].name, "engine-orch");
        assert_eq!(conns[0].room_id, "report-engine-orch-20260624-0502");
        assert_eq!(conns[0].identity.as_deref(), Some("api-cbc"));
        assert_eq!(conns[0].model, "claude-opus-4-8");
        assert_eq!(conns[1].room_id, "pdf-extractor-20260624-0600");
        assert_eq!(conns[1].model, "claude-sonnet-4-6");
    }

    #[test]
    fn parse_connections_identity_none_and_model_default_when_absent() {
        let content = "\
connections:
  bare: some-room-20260625-1000
";
        let conns = parse_connections(content);
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0].room_id, "some-room-20260625-1000");
        assert_eq!(conns[0].identity, None, "absent --as ⇒ None");
        assert_eq!(conns[0].model, "<model>", "absent --model ⇒ fallback");
    }

    #[test]
    fn parse_connections_ignores_lines_outside_block() {
        let content = "\
status: ACTIVE
room-id: not-a-connection
connections:
  real: real-room-20260625-1200 --as me --model sonnet

## Some other section
  fake: should-not-parse-20260625-1300 --as no --model no
";
        let conns = parse_connections(content);
        assert_eq!(conns.len(), 1, "only the indented line under connections:");
        assert_eq!(conns[0].room_id, "real-room-20260625-1200");
    }

    #[test]
    fn parse_connections_empty_when_no_block() {
        assert!(parse_connections("status: ACTIVE\nno block here\n").is_empty());
    }

    // ── declared_connections (the fallback chain B2 reconciles against) ─────────

    /// Pins the worker SKILL.md template: a `mode: worker` file with the documented
    /// `connections:` block must resolve to one identity-scoped connection.  If the
    /// skill's block format drifts from the parser, this fails.
    #[test]
    fn declared_connections_worker_template_is_identity_scoped() {
        let content = "\
## Status
status: ACTIVE
mode: worker
room-id: report-engine-recompute-20260626-1430
model: claude-sonnet-4-6

connections:
  orchestrator: report-engine-recompute-20260626-1430 --as engine-worker-recompute --model claude-sonnet-4-6
";
        let conns = declared_connections(content);
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0].room_id, "report-engine-recompute-20260626-1430");
        assert_eq!(
            conns[0].identity.as_deref(),
            Some("engine-worker-recompute"),
            "the --as identity must reach the reconcile so it is session-scoped"
        );
        assert_eq!(conns[0].model, "claude-sonnet-4-6");
    }

    // ── parse_worker_mode ─────────────────────────────────────────────────────

    #[test]
    fn parse_worker_mode_reads_worker() {
        assert_eq!(
            parse_worker_mode("status: ACTIVE\nmode: worker\n"),
            WorkerMode::Worker
        );
    }

    #[test]
    fn parse_worker_mode_reads_direct() {
        assert_eq!(parse_worker_mode("mode: direct\n"), WorkerMode::Direct);
    }

    #[test]
    fn parse_worker_mode_defaults_to_direct_when_absent() {
        assert_eq!(
            parse_worker_mode("status: ACTIVE\nroom-id: x\n"),
            WorkerMode::Direct,
            "absent mode ⇒ Direct (standalone)"
        );
    }

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
        assert_eq!(w.identity, None, "no poll-label ⇒ no identity");
    }

    #[test]
    fn parse_worker_recovers_identity_from_full_command_poll_label() {
        // The live legacy shape: the worker stuffed the whole `cbc poll …`
        // command (with `--as`) into poll-label, plus a trailing `#` comment
        // that ALSO mentions `--as` in prose.  The first `--as` token wins.
        let content = "\
## Status
status: ACTIVE
room-id: cbc-report-seeds-20260626-0027
poll-label: cbc poll cbc-report-seeds-20260626-0027 --as mvp-api-claude-sonnet-4-6-9df0 --model claude-sonnet-4-6  # MUST include --as to match join identity
model: claude-sonnet-4-6
";
        let w = parse_worker(content).expect("should parse active worker");
        assert_eq!(w.room_id, "cbc-report-seeds-20260626-0027");
        assert_eq!(
            w.identity.as_deref(),
            Some("mvp-api-claude-sonnet-4-6-9df0"),
            "the --as handle already in poll-label must be recovered, not discarded"
        );
        assert_eq!(w.model, "claude-sonnet-4-6");
    }

    #[test]
    fn parse_worker_identity_none_for_bare_label() {
        let content = "\
## Status
status: ACTIVE
room-id: feat-room-20260625-1200
poll-label: repo-worker-feat
model: claude-sonnet-4-6
";
        let w = parse_worker(content).expect("should parse");
        assert_eq!(
            w.identity, None,
            "a bare label (no --as flag) must yield identity None — no false match"
        );
    }

    #[test]
    fn parse_worker_ignores_as_and_model_mentioned_only_in_a_comment() {
        // Footgun: a BARE label whose ONLY `--as`/`--model` mention lives inside a
        // trailing `#` comment (prose, not a real flag).  Scanning the comment
        // would fabricate identity="ghost"/model="claude-x" — WORSE than None,
        // because a wrong --as makes poll_matches miss the healthy poll and the
        // reconcile relaunches `--as ghost` → the exact 400 this PR kills.  The
        // comment must be stripped before flag extraction.
        let content = "\
## Status
status: ACTIVE
room-id: feat-room-20260625-1400
poll-label: repo-worker-feat  # remember to pass --as ghost --model claude-x or it 400s
model: claude-sonnet-4-6
";
        let w = parse_worker(content).expect("should parse");
        assert_eq!(
            w.identity, None,
            "an --as mentioned only in a # comment must NOT be recovered as a real flag"
        );
        assert_eq!(
            w.model, "claude-sonnet-4-6",
            "model must come from the real `model:` field, not the comment's --model"
        );
    }

    #[test]
    fn parse_worker_recovers_model_from_poll_label_when_field_absent() {
        // No dedicated `model:` line — the `--model` inside poll-label is the
        // only source.  (Identity comes from the same line.)
        let content = "\
## Status
status: ACTIVE
room-id: feat-room-20260625-1300
poll-label: cbc poll feat-room-20260625-1300 --as me-9df0 --model claude-opus-4-8
";
        let w = parse_worker(content).expect("should parse");
        assert_eq!(w.identity.as_deref(), Some("me-9df0"));
        assert_eq!(
            w.model, "claude-opus-4-8",
            "absent model: field ⇒ fall back to the poll-label --model"
        );
    }

    #[test]
    fn flag_value_rejects_a_following_flag_as_the_value() {
        // Malformed line: `--as` immediately followed by another flag (a missing
        // `<id>`).  Without a guard, identity becomes `"--model"` — a garbage
        // handle that `poll_matches` can never match, so the reconcile both
        // relaunches a stacked poll AND reaps the healthy one: the exact
        // friendly-fire this PR exists to kill.  The value after a flag must be
        // a real token, never another `--flag`.  The helper now backs BOTH
        // parse paths, so the guard hardens `parse_connections` too.
        assert_eq!(
            flag_value("cbc poll room --as --model claude-x", "--as"),
            None,
            "a flag whose value is itself another --flag must yield None, not the next flag"
        );
        // Sanity: a real value immediately before a `#` comment still resolves.
        assert_eq!(
            flag_value("room --as good-9df0 # note", "--as"),
            Some("good-9df0".to_string())
        );
    }

    /// The end-to-end self-heal: a legacy worker file (no `connections:` block)
    /// whose poll-label carries the full command must yield a declared
    /// connection WITH identity, so the Stop reconcile's relaunch emits `--as`
    /// (no 400) and the kill is identity-scoped (no 143 on the healthy poll).
    #[test]
    fn declared_connections_legacy_worker_recovers_identity_from_poll_label() {
        let content = "\
## Status
status: ACTIVE
room-id: cbc-report-seeds-20260626-0027
poll-label: cbc poll cbc-report-seeds-20260626-0027 --as mvp-api-9df0 --model claude-sonnet-4-6
model: claude-sonnet-4-6
";
        let conns = declared_connections(content);
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0].room_id, "cbc-report-seeds-20260626-0027");
        assert_eq!(
            conns[0].identity.as_deref(),
            Some("mvp-api-9df0"),
            "legacy worker must self-heal: identity reaches the reconcile"
        );
        // And the relaunch command the reconcile would emit carries --as.
        assert_eq!(
            poll_command(
                &conns[0].room_id,
                &conns[0].model,
                conns[0].identity.as_deref()
            ),
            "cbc poll cbc-report-seeds-20260626-0027 --model claude-sonnet-4-6 --as mvp-api-9df0"
        );
    }

    #[test]
    fn declared_connections_strips_trailing_punctuation_from_real_world_poll_label() {
        // A real production state file's `poll-label` is not the canonical
        // `cbc poll <room> --as <id> --model <m>` line the skill originally
        // taught — agents write a descriptive comment instead:
        // `cbc poll (bg task <taskid>, --as <id>) -- <prose>`. With no space
        // before the closing paren, whitespace-splitting yields `<id>)` as one
        // token. Without stripping, the recovered identity is corrupted, so the
        // hook's own kill/relaunch never matches the real running poll's
        // `--as <id>` (no paren) — a second, independent churn source from the
        // one already fixed for random-hex minting.
        let content = "\
## Status
status: ACTIVE
room-id: cbc-report-mvp-engine-chemistry-panel-casing-orchestrator-20260629-1742
poll-label: cbc poll (bg task bsjmeuspl, --as mvp-engine-opus48-9ed8) -- relaunched again post-compaction
model: opus48
";
        let conns = declared_connections(content);
        assert_eq!(conns.len(), 1);
        assert_eq!(
            conns[0].identity.as_deref(),
            Some("mvp-engine-opus48-9ed8"),
            "trailing punctuation from the parenthetical label must not leak into the identity"
        );
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
        assert_eq!(
            o.rooms[0],
            (
                "repo-worker-feat".to_string(),
                "feat-room-20260625-1100".to_string()
            )
        );
        assert_eq!(
            o.rooms[1],
            (
                "repo-worker-fix".to_string(),
                "fix-room-20260625-1105".to_string()
            )
        );
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
            identity: None,
        });
        let mut out = Vec::new();
        let mut killed = Vec::new();
        emit_relaunch(&entry, &mut out, &|_| None, &mut |id, _identity| {
            killed.push(id.to_string())
        })
        .unwrap();

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
                (
                    "repo-worker-feat".to_string(),
                    "room-a-20260625-1100".to_string(),
                ),
                (
                    "repo-worker-fix".to_string(),
                    "room-b-20260625-1105".to_string(),
                ),
            ],
        });
        let mut out = Vec::new();
        let mut killed = Vec::new();
        emit_relaunch(&entry, &mut out, &|_| None, &mut |id, _identity| {
            killed.push(id.to_string())
        })
        .unwrap();

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

    #[test]
    fn emit_relaunch_worker_inoculates_against_trusting_poll_live() {
        // A live incident: an agent read `poll_live: true` on its own pre-
        // compaction poll (still parked, orphaned from the new session) and
        // concluded "no relaunch needed" — overriding this very directive. The
        // directive text must pre-empt that override explicitly, not just say
        // "relaunch."
        let entry = ActiveEntry::Worker(WorkerEntry {
            room_id: "feat-room-20260625-1000".to_string(),
            poll_label: "repo-worker-feat".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            identity: None,
        });
        let mut out = Vec::new();
        let mut killed = Vec::new();
        emit_relaunch(&entry, &mut out, &|_| None, &mut |id, _identity| {
            killed.push(id.to_string())
        })
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("Do NOT check poll_live or ps to decide whether to skip this"),
            "directive must inoculate against trusting a self-liveness check; got:\n{text}"
        );
    }

    #[test]
    fn emit_relaunch_orchestrator_inoculates_against_trusting_poll_live() {
        let entry = ActiveEntry::Orchestrator(OrchestratorEntry {
            model: "claude-opus-4-8".to_string(),
            rooms: vec![(
                "repo-worker-feat".to_string(),
                "room-a-20260625-1100".to_string(),
            )],
        });
        let mut out = Vec::new();
        let mut killed = Vec::new();
        emit_relaunch(&entry, &mut out, &|_| None, &mut |id, _identity| {
            killed.push(id.to_string())
        })
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("Do NOT check poll_live or ps to decide whether to skip this"),
            "directive must inoculate against trusting a self-liveness check; got:\n{text}"
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
        run_session_start(&mut input.as_bytes(), &mut out, &mut |id, _identity| {
            killed.push(id.to_string())
        })
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
        run_session_start(&mut input.as_bytes(), &mut out, &mut |id, _identity| {
            killed.push(id.to_string())
        })
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("cbc poll feat-room-20260625-1000 --model claude-sonnet-4-6"),
            "compact output must contain poll command; got:\n{text}"
        );
        assert_eq!(killed, vec!["feat-room-20260625-1000"]);
    }

    #[test]
    fn run_compact_legacy_worker_recovers_identity_for_relaunch_and_kill() {
        // The compaction half of the bug: a legacy worker file (no `connections:`
        // block) whose poll-label carries the full `cbc poll … --as <id>` command
        // must relaunch identity-scoped on SessionStart.  This path runs through
        // emit_relaunch -> id_of -> scan_declared -> declared_connections, distinct
        // from the Stop path, so it needs its own guard.  Two assertions: the
        // emitted command carries the recovered `--as` (else the poll 400s), and
        // the stale-poll kill is identity-scoped (else it reaps another session's
        // poll on the shared report room -> the 143 friendly-fire).
        let dir = tempfile::tempdir().unwrap();
        let cbc = dir.path().join(".cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(
            cbc.join("worker-mvp-seeds-20260626.md"),
            "status: ACTIVE\nroom-id: cbc-report-seeds-20260626-0027\npoll-label: cbc poll cbc-report-seeds-20260626-0027 --as mvp-api-9df0 --model claude-sonnet-4-6\nmodel: claude-sonnet-4-6\n",
        )
        .unwrap();

        let input = serde_json::json!({
            "source": "compact",
            "cwd": dir.path().to_str().unwrap()
        })
        .to_string();

        let mut out = Vec::new();
        // Capture BOTH args — existing tests discard the identity, which is exactly
        // the bit that proves the kill is identity-scoped.
        let mut killed: Vec<(String, Option<String>)> = Vec::new();
        run_session_start(&mut input.as_bytes(), &mut out, &mut |id, identity| {
            killed.push((id.to_string(), identity.map(str::to_string)))
        })
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains(
                "cbc poll cbc-report-seeds-20260626-0027 --model claude-sonnet-4-6 --as mvp-api-9df0"
            ),
            "compact relaunch must carry the recovered --as (else the poll 400s); got:\n{text}"
        );
        assert_eq!(
            killed,
            vec![(
                "cbc-report-seeds-20260626-0027".to_string(),
                Some("mvp-api-9df0".to_string())
            )],
            "the stale-poll kill must be identity-scoped — Some(handle), not None — \
             so it cannot reap another session's poll on the shared report room"
        );
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
        run_session_start(&mut input.as_bytes(), &mut out, &mut |id, _identity| {
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
        run_session_start(&mut input.as_bytes(), &mut out, &mut |id, _identity| {
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
        run_session_start(&mut input.as_bytes(), &mut out, &mut |id, _identity| {
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
        run_session_start(&mut input.as_bytes(), &mut out, &mut |id, _identity| {
            killed.push(id.to_string())
        })
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("room-a-20260625-1000"),
            "must emit first room"
        );
        assert!(
            text.contains("room-b-20260625-1005"),
            "must emit second room"
        );
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
        run_session_start(&mut input.as_bytes(), &mut out, &mut |id, _identity| {
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
        run_session_start(&mut "".as_bytes(), &mut out, &mut |id, _identity| {
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
            &mut |id, _identity| killed.push(id.to_string()),
        )
        .unwrap();
        assert!(out.is_empty(), "malformed JSON must be a silent no-op");
        assert!(killed.is_empty());
    }
}
