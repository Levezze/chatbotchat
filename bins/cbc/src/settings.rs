//! `cbc allow-tools` — grant the chatbotchat MCP server standing auto-approval in
//! the host agent's settings, so the inter-agent bus stops stalling for per-call
//! approval.
//!
//! Why this is needed: Claude Code's `auto` permission mode routes any tool call
//! NOT covered by a `permissions.allow` rule to a safety classifier that inspects
//! the call and its arguments. A `cbc_send` into a room whose subject reads like
//! client work can read to that classifier as outbound external comms or an
//! escalation beyond the user's request, so the call stalls for approval — even
//! though the bus is a local loopback to the daemon. An explicit `allow` rule is
//! evaluated *first* and resolves immediately, short-circuiting the classifier.
//! See `permission-modes.md`.
//!
//! Layering mirrors `install.rs`: the merge is a pure, FS-free seam
//! ([`ensure_allow_rule`]) so every settings shape is unit-tested; the read/back
//! up/write glue ([`apply_allow_rule`]) is path-injected so it is tested against a
//! tempdir; the interactive install prompt and `~` resolution are the only
//! untested side effects.

use anyhow::Context;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The `permissions.allow` rule that grants the whole chatbotchat MCP server
/// auto-approval. The server-wide form (`mcp__<server>`) covers every `cbc_*`
/// tool, so a single rule is enough.
pub const CBC_ALLOW_RULE: &str = "mcp__chatbotchat";

/// What [`apply_allow_rule`] did, so the caller can print an honest one-liner.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// No settings file existed; one was created carrying the rule.
    Created,
    /// The file existed and the rule was appended.
    Added,
    /// The rule was already present; nothing was written.
    AlreadyPresent,
}

/// Ensure `settings["permissions"]["allow"]` is an array containing `rule`,
/// creating the `permissions` object and `allow` array if absent and leaving every
/// other key untouched. Returns `Ok(true)` if `settings` was modified, `Ok(false)`
/// if the rule was already present.
///
/// Errors rather than clobbering when `settings` is not an object, or when an
/// existing `permissions`/`allow` has the wrong JSON type — a hand-maintained
/// settings file must never be silently overwritten.
pub fn ensure_allow_rule(settings: &mut Value, rule: &str) -> anyhow::Result<bool> {
    let root = settings
        .as_object_mut()
        .context("settings root is not a JSON object; refusing to overwrite it")?;

    let permissions = root
        .entry("permissions")
        .or_insert_with(|| Value::Object(Default::default()));
    let permissions = permissions
        .as_object_mut()
        .context("`permissions` is not a JSON object; refusing to overwrite it")?;

    let allow = permissions
        .entry("allow")
        .or_insert_with(|| Value::Array(Vec::new()));
    let allow = allow
        .as_array_mut()
        .context("`permissions.allow` is not a JSON array; refusing to overwrite it")?;

    if allow.iter().any(|v| v.as_str() == Some(rule)) {
        return Ok(false);
    }
    allow.push(Value::String(rule.to_string()));
    Ok(true)
}

/// `~/.claude/settings.json` — the Claude Code *user* scope, which applies across
/// every project.
pub fn settings_path() -> anyhow::Result<PathBuf> {
    let home =
        std::env::var_os("HOME").context("HOME not set; cannot locate Claude Code settings")?;
    Ok(PathBuf::from(home).join(".claude").join("settings.json"))
}

/// Read the settings at `path` (treating a missing file as empty), merge in the
/// CBC allow rule, and — only if that changed anything — back the original up to
/// `<path>.bak` and rewrite it as 2-space-pretty JSON. Pure-merge errors and a
/// genuinely unparseable file both surface as `Err` so callers can degrade to
/// printing the manual snippet rather than crashing.
pub fn apply_allow_rule(path: &Path) -> anyhow::Result<Outcome> {
    let existed = path.exists();
    let original = if existed {
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
    } else {
        String::new()
    };

    let mut settings: Value = if original.trim().is_empty() {
        Value::Object(Default::default())
    } else {
        serde_json::from_str(&original)
            .with_context(|| format!("parsing {} as JSON", path.display()))?
    };

    let changed = ensure_allow_rule(&mut settings, CBC_ALLOW_RULE)?;
    if !changed {
        return Ok(Outcome::AlreadyPresent);
    }

    if existed {
        let backup = PathBuf::from(format!("{}.bak", path.display()));
        std::fs::write(&backup, &original)
            .with_context(|| format!("backing up to {}", backup.display()))?;
    } else if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let mut rendered = serde_json::to_string_pretty(&settings).context("serializing settings")?;
    rendered.push('\n');
    std::fs::write(path, rendered).with_context(|| format!("writing {}", path.display()))?;

    Ok(if existed {
        Outcome::Added
    } else {
        Outcome::Created
    })
}

/// Report what `apply_allow_rule` did, naming the host and file explicitly (the
/// edit is Claude-Code-specific; other hosts get their own path later).
pub fn print_allow_outcome(path: &Path, outcome: &Outcome) {
    match outcome {
        Outcome::Created | Outcome::Added => {
            println!(
                "Granted the chatbotchat MCP tools auto-approval in Claude Code settings:\n  {}",
                path.display()
            );
            if matches!(outcome, Outcome::Added) {
                println!("(backed up the previous file to {}.bak)", path.display());
            }
            println!("Restart any open Claude Code session to pick it up.");
        }
        Outcome::AlreadyPresent => {
            println!(
                "chatbotchat MCP tools are already auto-approved in {}; nothing to do.",
                path.display()
            );
        }
    }
}

/// Degrade path: the file could not be edited automatically (e.g. unparseable),
/// so tell the user how to do it by hand rather than crashing.
pub fn print_manual_snippet() {
    println!("Add this to your Claude Code settings (~/.claude/settings.json) by hand:");
    println!("  {{ \"permissions\": {{ \"allow\": [\"{CBC_ALLOW_RULE}\"] }} }}");
}

// ── install-hooks (SessionStart + Stop hooks) ────────────────────────────────

/// The `cbc hook session-start` command string registered in `hooks.SessionStart`.
pub const CBC_SESSION_START_COMMAND: &str = "cbc hook session-start";

/// The `cbc hook stop` command string registered in `hooks.Stop` — the per-turn
/// poll reconcile (B2). Clap kebab-cases the `Stop` subcommand to `stop`.
pub const CBC_STOP_COMMAND: &str = "cbc hook stop";

/// Ensure `settings["hooks"][event]` contains a wrapper object whose inner `hooks`
/// array has an entry of `{"type":"command","command":<command>}`.
///
/// Idempotent: if any existing wrapper's inner `hooks` array already has an entry
/// with that `command`, returns `Ok(false)` without modifying `settings`. Errors
/// rather than clobbering when `hooks` or `hooks[event]` has the wrong JSON type.
fn ensure_command_hook(settings: &mut Value, event: &str, command: &str) -> anyhow::Result<bool> {
    use serde_json::json;

    let root = settings
        .as_object_mut()
        .context("settings root is not a JSON object; refusing to overwrite it")?;

    let hooks_val = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Default::default()));
    let hooks_map = hooks_val
        .as_object_mut()
        .context("`hooks` is not a JSON object; refusing to overwrite it")?;

    let event_val = hooks_map
        .entry(event)
        .or_insert_with(|| Value::Array(Vec::new()));
    let event_arr = event_val.as_array_mut().with_context(|| {
        format!("`hooks.{event}` is not a JSON array; refusing to overwrite it")
    })?;

    // Already present? Check every wrapper's inner hooks array.
    for wrapper in event_arr.iter() {
        if let Some(inner) = wrapper.get("hooks").and_then(|h| h.as_array()) {
            for entry in inner {
                if entry.get("command").and_then(|c| c.as_str()) == Some(command) {
                    return Ok(false);
                }
            }
        }
    }

    event_arr.push(json!({
        "hooks": [
            {
                "type": "command",
                "command": command
            }
        ]
    }));
    Ok(true)
}

/// Ensure the CBC `SessionStart` hook is registered. See [`ensure_command_hook`].
pub fn ensure_session_start_hook(settings: &mut Value) -> anyhow::Result<bool> {
    ensure_command_hook(settings, "SessionStart", CBC_SESSION_START_COMMAND)
}

/// Ensure the CBC `Stop` hook (per-turn poll reconcile) is registered.
pub fn ensure_stop_hook(settings: &mut Value) -> anyhow::Result<bool> {
    ensure_command_hook(settings, "Stop", CBC_STOP_COMMAND)
}

/// Read the settings at `path` (treating a missing file as empty), merge in BOTH
/// CBC hook entries (`SessionStart` + `Stop`), and — only if that changed anything
/// — back the original up to `<path>.bak` and rewrite it as 2-space-pretty JSON.
/// Installing both here means an existing SessionStart-only install gains the Stop
/// reconcile on the next `install-hooks` run.
pub fn apply_hook_rule(path: &Path) -> anyhow::Result<Outcome> {
    let existed = path.exists();
    let original = if existed {
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
    } else {
        String::new()
    };

    let mut settings: Value = if original.trim().is_empty() {
        Value::Object(Default::default())
    } else {
        serde_json::from_str(&original)
            .with_context(|| format!("parsing {} as JSON", path.display()))?
    };

    // Apply both as separate statements (not short-circuited) so a SessionStart-only
    // install still gets Stop added; `changed` is true if either was written.
    let changed_session_start = ensure_session_start_hook(&mut settings)?;
    let changed_stop = ensure_stop_hook(&mut settings)?;
    let changed = changed_session_start || changed_stop;
    if !changed {
        return Ok(Outcome::AlreadyPresent);
    }

    if existed {
        let backup = PathBuf::from(format!("{}.bak", path.display()));
        std::fs::write(&backup, &original)
            .with_context(|| format!("backing up to {}", backup.display()))?;
    } else if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let mut rendered = serde_json::to_string_pretty(&settings).context("serializing settings")?;
    rendered.push('\n');
    std::fs::write(path, rendered).with_context(|| format!("writing {}", path.display()))?;

    Ok(if existed {
        Outcome::Added
    } else {
        Outcome::Created
    })
}

/// Report what `apply_hook_rule` did.
pub fn print_hook_outcome(path: &Path, outcome: &Outcome) {
    match outcome {
        Outcome::Created | Outcome::Added => {
            println!(
                "Registered the CBC SessionStart + Stop hooks in Claude Code settings:\n  {}",
                path.display()
            );
            if matches!(outcome, Outcome::Added) {
                println!("(backed up the previous file to {}.bak)", path.display());
            }
            println!(
                "Restart any open Claude Code session to pick them up.\n\
                 SessionStart relaunches your CBC polls on compact/resume; Stop reconciles \
                 them to exactly one per declared room at every turn-end."
            );
        }
        Outcome::AlreadyPresent => {
            println!(
                "CBC SessionStart + Stop hooks already registered in {}; nothing to do.",
                path.display()
            );
        }
    }
}

/// Degrade path for `install-hooks`.
pub fn print_manual_hook_snippet() {
    println!("Add this to your Claude Code settings (~/.claude/settings.json) by hand:");
    println!(
        r#"  {{"hooks": {{"SessionStart": [{{"hooks": [{{"type":"command","command":"{CBC_SESSION_START_COMMAND}"}}]}}], "Stop": [{{"hooks": [{{"type":"command","command":"{CBC_STOP_COMMAND}"}}]}}]}}}}"#
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn creates_permissions_and_allow_on_empty_object() {
        let mut v = json!({});
        let changed = ensure_allow_rule(&mut v, CBC_ALLOW_RULE).unwrap();
        assert!(changed, "an empty settings object must be modified");
        assert_eq!(v["permissions"]["allow"][0], json!(CBC_ALLOW_RULE));
    }

    #[test]
    fn appends_to_existing_allow_preserving_prior_entries() {
        let mut v = json!({ "permissions": { "allow": ["Read", "Write"] } });
        let changed = ensure_allow_rule(&mut v, CBC_ALLOW_RULE).unwrap();
        assert!(changed);
        assert_eq!(
            v["permissions"]["allow"],
            json!(["Read", "Write", CBC_ALLOW_RULE]),
            "the rule must be appended without dropping existing allow entries"
        );
    }

    #[test]
    fn is_idempotent_when_rule_already_present() {
        let mut v = json!({ "permissions": { "allow": [CBC_ALLOW_RULE] } });
        let changed = ensure_allow_rule(&mut v, CBC_ALLOW_RULE).unwrap();
        assert!(!changed, "re-adding an existing rule must report no change");
        assert_eq!(
            v["permissions"]["allow"],
            json!([CBC_ALLOW_RULE]),
            "an idempotent run must not duplicate the rule"
        );
    }

    #[test]
    fn preserves_unrelated_top_level_and_permissions_keys() {
        let mut v = json!({
            "hooks": { "PreToolUse": [] },
            "permissions": { "defaultMode": "auto" }
        });
        ensure_allow_rule(&mut v, CBC_ALLOW_RULE).unwrap();
        assert_eq!(
            v["hooks"]["PreToolUse"],
            json!([]),
            "unrelated keys survive"
        );
        assert_eq!(
            v["permissions"]["defaultMode"],
            json!("auto"),
            "sibling permission keys survive"
        );
        assert_eq!(v["permissions"]["allow"][0], json!(CBC_ALLOW_RULE));
    }

    #[test]
    fn errors_rather_than_clobbering_a_wrong_typed_permissions() {
        let mut v = json!({ "permissions": 5 });
        assert!(
            ensure_allow_rule(&mut v, CBC_ALLOW_RULE).is_err(),
            "a non-object permissions value must error, not be overwritten"
        );
    }

    #[test]
    fn errors_rather_than_clobbering_a_wrong_typed_allow() {
        let mut v = json!({ "permissions": { "allow": "not-an-array" } });
        assert!(
            ensure_allow_rule(&mut v, CBC_ALLOW_RULE).is_err(),
            "a non-array allow value must error, not be overwritten"
        );
    }

    #[test]
    fn apply_creates_settings_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".claude").join("settings.json");
        let outcome = apply_allow_rule(&path).unwrap();
        assert_eq!(outcome, Outcome::Created);
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["permissions"]["allow"][0], json!(CBC_ALLOW_RULE));
    }

    #[test]
    fn apply_appends_and_backs_up_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            "{\n  \"permissions\": {\n    \"defaultMode\": \"auto\"\n  }\n}\n",
        )
        .unwrap();

        let outcome = apply_allow_rule(&path).unwrap();
        assert_eq!(outcome, Outcome::Added);

        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["permissions"]["allow"][0], json!(CBC_ALLOW_RULE));
        assert_eq!(
            written["permissions"]["defaultMode"],
            json!("auto"),
            "the existing mode must be preserved through the rewrite"
        );

        let backup = PathBuf::from(format!("{}.bak", path.display()));
        assert!(
            backup.is_file(),
            "the original must be backed up before rewrite"
        );
        assert!(
            std::fs::read_to_string(&backup)
                .unwrap()
                .contains("defaultMode"),
            "the backup must hold the pre-edit contents"
        );
    }

    #[test]
    fn apply_is_idempotent_and_skips_write_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        apply_allow_rule(&path).unwrap(); // Created
        let outcome = apply_allow_rule(&path).unwrap();
        assert_eq!(
            outcome,
            Outcome::AlreadyPresent,
            "a second run must detect the rule and report no change"
        );
    }

    #[test]
    fn apply_errors_on_unparseable_settings_rather_than_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{ this is not valid json, // comment\n").unwrap();
        assert!(
            apply_allow_rule(&path).is_err(),
            "a corrupt settings file must surface an error so the caller can degrade, \
             never be silently overwritten"
        );
    }

    // ── ensure_session_start_hook ─────────────────────────────────────────────

    #[test]
    fn hook_adds_entry_to_empty_settings() {
        let mut v = json!({});
        let changed = ensure_session_start_hook(&mut v).unwrap();
        assert!(changed, "empty settings must be modified");
        let arr = v["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(
            arr[0]["hooks"][0]["command"],
            json!(CBC_SESSION_START_COMMAND)
        );
        assert_eq!(arr[0]["hooks"][0]["type"], json!("command"));
    }

    #[test]
    fn hook_is_idempotent_when_already_present() {
        let mut v = json!({
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            { "type": "command", "command": CBC_SESSION_START_COMMAND }
                        ]
                    }
                ]
            }
        });
        let changed = ensure_session_start_hook(&mut v).unwrap();
        assert!(!changed, "re-adding must report no change");
        let arr = v["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "must not duplicate the entry");
    }

    #[test]
    fn hook_appends_without_removing_existing_session_start_entries() {
        let mut v = json!({
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            { "type": "command", "command": "node caveman-activate.js" }
                        ]
                    }
                ]
            }
        });
        ensure_session_start_hook(&mut v).unwrap();
        let arr = v["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(arr.len(), 2, "must append, not replace");
        // caveman entry still present
        assert_eq!(
            arr[0]["hooks"][0]["command"],
            json!("node caveman-activate.js")
        );
        // cbc entry appended
        assert_eq!(
            arr[1]["hooks"][0]["command"],
            json!(CBC_SESSION_START_COMMAND)
        );
    }

    #[test]
    fn hook_preserves_permissions_allow_and_other_keys() {
        let mut v = json!({
            "permissions": { "allow": ["mcp__chatbotchat"] },
            "hooks": { "PreToolUse": [] }
        });
        ensure_session_start_hook(&mut v).unwrap();
        assert_eq!(
            v["permissions"]["allow"][0],
            json!("mcp__chatbotchat"),
            "allow rule must survive"
        );
        assert_eq!(
            v["hooks"]["PreToolUse"],
            json!([]),
            "other hooks must survive"
        );
        // cbc hook added
        assert_eq!(
            v["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            json!(CBC_SESSION_START_COMMAND)
        );
    }

    #[test]
    fn hook_errors_on_wrong_typed_hooks_object() {
        let mut v = json!({ "hooks": 5 });
        assert!(
            ensure_session_start_hook(&mut v).is_err(),
            "non-object hooks must error"
        );
    }

    #[test]
    fn hook_errors_on_wrong_typed_session_start_array() {
        let mut v = json!({ "hooks": { "SessionStart": "not-an-array" } });
        assert!(
            ensure_session_start_hook(&mut v).is_err(),
            "non-array SessionStart must error"
        );
    }

    // ── apply_hook_rule ───────────────────────────────────────────────────────

    #[test]
    fn apply_hook_creates_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".claude").join("settings.json");
        let outcome = apply_hook_rule(&path).unwrap();
        assert_eq!(outcome, Outcome::Created);
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            written["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            json!(CBC_SESSION_START_COMMAND)
        );
    }

    #[test]
    fn apply_hook_appends_and_backs_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            "{\n  \"permissions\": { \"allow\": [\"mcp__chatbotchat\"] }\n}\n",
        )
        .unwrap();

        let outcome = apply_hook_rule(&path).unwrap();
        assert_eq!(outcome, Outcome::Added);

        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // hook was added
        assert_eq!(
            written["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            json!(CBC_SESSION_START_COMMAND)
        );
        // allow rule preserved
        assert_eq!(
            written["permissions"]["allow"][0],
            json!("mcp__chatbotchat")
        );

        let backup = PathBuf::from(format!("{}.bak", path.display()));
        assert!(backup.is_file(), "backup must exist");
    }

    #[test]
    fn apply_hook_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        apply_hook_rule(&path).unwrap(); // Created
        let outcome = apply_hook_rule(&path).unwrap();
        assert_eq!(outcome, Outcome::AlreadyPresent);
    }

    // ── ensure_stop_hook ──────────────────────────────────────────────────────

    #[test]
    fn stop_hook_adds_entry_to_empty_settings() {
        let mut v = json!({});
        let changed = ensure_stop_hook(&mut v).unwrap();
        assert!(changed, "empty settings must be modified");
        assert_eq!(
            v["hooks"]["Stop"][0]["hooks"][0]["command"],
            json!(CBC_STOP_COMMAND)
        );
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["type"], json!("command"));
    }

    #[test]
    fn stop_hook_is_idempotent_when_already_present() {
        let mut v = json!({
            "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": CBC_STOP_COMMAND }] }] }
        });
        let changed = ensure_stop_hook(&mut v).unwrap();
        assert!(!changed, "re-adding must report no change");
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn stop_hook_coexists_with_session_start_and_preserves_it() {
        let mut v = json!({});
        ensure_session_start_hook(&mut v).unwrap();
        ensure_stop_hook(&mut v).unwrap();
        assert_eq!(
            v["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            json!(CBC_SESSION_START_COMMAND),
            "SessionStart hook survives adding Stop"
        );
        assert_eq!(
            v["hooks"]["Stop"][0]["hooks"][0]["command"],
            json!(CBC_STOP_COMMAND)
        );
    }

    #[test]
    fn stop_hook_errors_on_wrong_typed_stop_array() {
        let mut v = json!({ "hooks": { "Stop": "not-an-array" } });
        assert!(
            ensure_stop_hook(&mut v).is_err(),
            "non-array Stop must error, not be overwritten"
        );
    }

    #[test]
    fn apply_hook_installs_both_session_start_and_stop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".claude").join("settings.json");
        apply_hook_rule(&path).unwrap();
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            written["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            json!(CBC_SESSION_START_COMMAND)
        );
        assert_eq!(
            written["hooks"]["Stop"][0]["hooks"][0]["command"],
            json!(CBC_STOP_COMMAND)
        );
    }

    #[test]
    fn apply_hook_upgrades_a_session_start_only_install_by_adding_stop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        // Pre-existing install with ONLY the SessionStart hook (the old shape).
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&json!({
                "hooks": {
                    "SessionStart": [
                        { "hooks": [{ "type": "command", "command": CBC_SESSION_START_COMMAND }] }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let outcome = apply_hook_rule(&path).unwrap();
        assert_eq!(
            outcome,
            Outcome::Added,
            "adding Stop to a SessionStart-only install is a change"
        );
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            written["hooks"]["Stop"][0]["hooks"][0]["command"],
            json!(CBC_STOP_COMMAND),
            "the Stop hook must be added"
        );
        assert_eq!(
            written["hooks"]["SessionStart"].as_array().unwrap().len(),
            1,
            "the existing SessionStart hook must not be duplicated"
        );
    }
}
