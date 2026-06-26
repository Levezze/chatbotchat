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
        // Scan remaining tokens for --as / --model flag values.
        let rest_toks: Vec<&str> = rest.split_whitespace().collect();
        let flag_after = |flag: &str| -> Option<String> {
            rest_toks
                .iter()
                .position(|t| *t == flag)
                .and_then(|i| rest_toks.get(i + 1))
                .map(|s| s.to_string())
        };
        conns.push(DeclaredConnection {
            name,
            room_id: room_id.to_string(),
            identity: flag_after("--as"),
            model: flag_after("--model").unwrap_or_else(|| "<model>".to_string()),
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

// ── Reconcile primitives (B2) ──────────────────────────────────────────────────

/// What the Stop/SessionStart reconcile must do for one declared connection,
/// given how many of its polls are currently alive and whether one was launched
/// in the just-ended turn.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ReconcileAction {
    /// Exactly one healthy poll — leave it alone (never thrash).
    Ok,
    /// More than one — the hook kills the surplus itself (identity-scoped, so
    /// safe), leaving one.  `surplus` = how many to kill.
    KillExtras { surplus: usize },
    /// Zero polls and none launched this turn — block turn-end and force the
    /// model to relaunch (only a model-launched poll can wake the session).
    BlockAndRelaunch,
}

/// The pure reconcile decision.  `n` = live polls matching this connection's
/// `{room, identity}`; `launched_this_turn` = the transcript shows a matching
/// `cbc poll` launch in the turn that just ended (race window: a brand-new poll
/// isn't process-visible yet).
pub fn reconcile_action(n: usize, launched_this_turn: bool) -> ReconcileAction {
    match n {
        1 => ReconcileAction::Ok,
        0 if launched_this_turn => ReconcileAction::Ok,
        0 => ReconcileAction::BlockAndRelaunch,
        more => ReconcileAction::KillExtras { surplus: more - 1 },
    }
}

/// The exact relaunch / declared poll command for a connection.  This is both
/// what the reconcile injects on `BlockAndRelaunch` and the canonical form a
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
    // Legacy worker: single room-id.
    if let Some(w) = parse_worker(content) {
        return vec![DeclaredConnection {
            name: w.poll_label,
            room_id: w.room_id,
            identity: None,
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
    /// How many surplus polls to kill (leaving exactly one).
    pub surplus: usize,
}

/// The reconcile plan for one Stop event: surplus kills to perform now, and the
/// relaunch commands to inject if the turn must be blocked.
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
            ReconcileAction::KillExtras { surplus } => plan.kills.push(KillOrder {
                room_id: c.room_id.clone(),
                identity: c.identity.clone(),
                surplus,
            }),
            ReconcileAction::BlockAndRelaunch => {
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
pub fn scan_declared(cbc_dir: &Path) -> Vec<DeclaredConnection> {
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
        out.extend(declared_connections(&content));
    }
    out
}

/// The Stop-hook block JSON that forces the model to relaunch dead polls.
/// `additionalContext` carries the SOLE-relaunch-authority directive plus the
/// exact commands, so the model relaunches here and ONLY here (a poll exit is
/// never an independent relaunch trigger).
fn stop_block_json(relaunch: &[String]) -> String {
    let mut ctx = String::from(
        "⚠ CBC RECONCILE — one or more declared rooms have NO live poll and none was launched \
         this turn. You are deaf on those rooms. Before yielding, relaunch each as a background \
         task (run_in_background) — this hook is the SOLE relaunch authority; do NOT relaunch \
         polls on your own from a poll-exit notification:",
    );
    for cmd in relaunch {
        ctx.push_str("\n  ");
        ctx.push_str(cmd);
    }
    let payload = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "Stop",
            "decision": "block",
            "additionalContext": ctx,
        }
    });
    payload.to_string()
}

/// Handle a `Stop` hook event: the per-turn poll reconcile (B2).
///
/// Reads the hook JSON (`cwd`, `stop_hook_active`), scans `<cwd>/.cbc` for
/// declared connections, and for each reconciles its live poll count against
/// exactly-one.  Surplus polls are killed via `kill_fn` (identity-scoped, safe).
/// If any room has zero polls and none was launched this turn — and the loop
/// guard (`stop_hook_active`) is not set — it blocks turn-end and injects the
/// relaunch commands.  `count_fn` / `launched_fn` are seams over the process
/// table and the transcript.
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

    let conns = scan_declared(&cwd.join(".cbc"));
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

    // Block only when a relaunch is owed AND the loop guard is clear.
    if !plan.relaunch.is_empty() && !stop_hook_active {
        write!(writer, "{}", stop_block_json(&plan.relaunch))?;
    }
    Ok(())
}

// ── Output ────────────────────────────────────────────────────────────────────

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
    let declared = scan_declared(&cbc_dir);
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
    fn reconcile_zero_not_launched_blocks_and_relaunches() {
        assert_eq!(
            reconcile_action(0, false),
            ReconcileAction::BlockAndRelaunch
        );
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
    fn reconcile_surplus_kills_all_but_one() {
        assert_eq!(
            reconcile_action(5, false),
            ReconcileAction::KillExtras { surplus: 4 }
        );
        assert_eq!(
            reconcile_action(2, true),
            ReconcileAction::KillExtras { surplus: 1 },
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

        let conns = scan_declared(&cbc);
        let rooms: Vec<&str> = conns.iter().map(|c| c.room_id.as_str()).collect();
        assert!(rooms.contains(&"room-a-20260625-1000"));
        assert!(rooms.contains(&"room-b-20260625-1005"));
        assert!(!rooms.contains(&"room-z"), "DONE file excluded");
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
    fn run_stop_blocks_and_injects_relaunch_for_dead_poll() {
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
        assert!(
            text.contains("\"decision\":\"block\""),
            "must block; got:\n{text}"
        );
        assert!(
            text.contains("cbc poll room-a-20260625-1000 --model opus --as api"),
            "must inject relaunch cmd; got:\n{text}"
        );
        assert!(kills.is_empty());
    }

    #[test]
    fn run_stop_loop_guard_suppresses_block_when_stop_hook_active() {
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
        assert_eq!(kills[0].surplus, 2);
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
    fn plan_stop_blocks_and_relaunches_dead_poll() {
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
                surplus: 3,
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
        assert_eq!(plan.kills[0].surplus, 1);
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
