//! `cbc allow-tools` — grant the chatbotchat MCP server and CLI standing
//! auto-approval in the host agent's settings, so the inter-agent bus stops
//! stalling for per-call approval.
//!
//! Why this is needed: Claude Code's `auto` permission mode routes any tool call
//! NOT covered by a `permissions.allow` rule to a safety classifier that inspects
//! the call and its arguments. A `cbc_send` into a room whose subject reads like
//! client work can read to that classifier as outbound external comms or an
//! escalation beyond the user's request, so the call stalls for approval — even
//! though the bus is a local loopback to the daemon. An explicit `allow` rule is
//! evaluated *first* and resolves immediately, short-circuiting the classifier.
//!
//! Two rules are needed:
//! - `mcp__chatbotchat` — server-wide; covers all 11 `cbc_*` MCP tools.
//! - `Bash(cbc *)` — covers the `cbc` CLI invoked via Bash (e.g. the background
//!   `cbc poll`, `cbc status`, `cbc send`). Without this, each Bash invocation
//!   hits the classifier even though the MCP tools are already exempt.
//!
//! Layering mirrors `install.rs`: the merge is a pure, FS-free seam
//! ([`ensure_allow_rules`]) so every settings shape is unit-tested; the read/back
//! up/write glue ([`apply_allow_rule`]) is path-injected so it is tested against a
//! tempdir; the interactive install prompt and `~` resolution are the only
//! untested side effects.

use anyhow::Context;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The full set of `permissions.allow` rules written by `cbc allow-tools`.
///
/// - `mcp__chatbotchat` — server-wide rule covering every `cbc_*` MCP tool.
/// - `Bash(cbc *)` — covers the `cbc` CLI invoked via Bash (background poll,
///   status, send, etc.) so those calls also skip the auto-mode classifier.
pub const CBC_ALLOW_RULES: &[&str] = &["mcp__chatbotchat", "Bash(cbc *)"];

/// What [`apply_allow_rule`] did, so the caller can print an honest one-liner.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// No settings file existed; one was created carrying the rules.
    Created,
    /// The file existed and at least one rule was appended.
    Added,
    /// All rules were already present; nothing was written.
    AlreadyPresent,
}

/// Ensure `settings["permissions"]["allow"]` is an array containing every entry
/// in `rules`, creating the `permissions` object and `allow` array if absent and
/// leaving every other key untouched. Returns `Ok(true)` if `settings` was
/// modified (at least one rule added), `Ok(false)` if all rules were already
/// present.
///
/// Errors rather than clobbering when `settings` is not an object, or when an
/// existing `permissions`/`allow` has the wrong JSON type — a hand-maintained
/// settings file must never be silently overwritten.
pub fn ensure_allow_rules(settings: &mut Value, rules: &[&str]) -> anyhow::Result<bool> {
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

    let mut any_added = false;
    for &rule in rules {
        if !allow.iter().any(|v| v.as_str() == Some(rule)) {
            allow.push(Value::String(rule.to_string()));
            any_added = true;
        }
    }
    Ok(any_added)
}

/// `~/.claude/settings.json` — the Claude Code *user* scope, which applies across
/// every project.
pub fn settings_path() -> anyhow::Result<PathBuf> {
    let home =
        std::env::var_os("HOME").context("HOME not set; cannot locate Claude Code settings")?;
    Ok(PathBuf::from(home).join(".claude").join("settings.json"))
}

/// Read the settings at `path` (treating a missing file as empty), merge in all
/// CBC allow rules, and — only if that changed anything — back the original up to
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

    let changed = ensure_allow_rules(&mut settings, CBC_ALLOW_RULES)?;
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
                "Granted the chatbotchat MCP tools and CLI auto-approval in Claude Code settings:\n  {}",
                path.display()
            );
            if matches!(outcome, Outcome::Added) {
                println!("(backed up the previous file to {}.bak)", path.display());
            }
            println!("Restart any open Claude Code session to pick it up.");
        }
        Outcome::AlreadyPresent => {
            println!(
                "chatbotchat MCP tools and CLI are already auto-approved in {}; nothing to do.",
                path.display()
            );
        }
    }
}

/// Degrade path: the file could not be edited automatically (e.g. unparseable),
/// so tell the user how to do it by hand rather than crashing.
pub fn print_manual_snippet() {
    let rules_json = CBC_ALLOW_RULES
        .iter()
        .map(|r| format!("\"{r}\""))
        .collect::<Vec<_>>()
        .join(", ");
    println!("Add this to your Claude Code settings (~/.claude/settings.json) by hand:");
    println!("  {{ \"permissions\": {{ \"allow\": [{rules_json}] }} }}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn creates_permissions_and_allow_on_empty_object() {
        let mut v = json!({});
        let changed = ensure_allow_rules(&mut v, CBC_ALLOW_RULES).unwrap();
        assert!(changed, "an empty settings object must be modified");
        let allow = v["permissions"]["allow"].as_array().unwrap();
        for rule in CBC_ALLOW_RULES {
            assert!(
                allow.iter().any(|r| r.as_str() == Some(*rule)),
                "rule {rule:?} must be present"
            );
        }
    }

    #[test]
    fn appends_to_existing_allow_preserving_prior_entries() {
        let mut v = json!({ "permissions": { "allow": ["Read", "Write"] } });
        let changed = ensure_allow_rules(&mut v, CBC_ALLOW_RULES).unwrap();
        assert!(changed);
        let allow = v["permissions"]["allow"].as_array().unwrap();
        // Prior entries survive.
        assert!(allow.iter().any(|r| r.as_str() == Some("Read")));
        assert!(allow.iter().any(|r| r.as_str() == Some("Write")));
        // New rules are appended.
        for rule in CBC_ALLOW_RULES {
            assert!(
                allow.iter().any(|r| r.as_str() == Some(*rule)),
                "rule {rule:?} must be appended without dropping existing allow entries"
            );
        }
    }

    #[test]
    fn is_idempotent_when_all_rules_already_present() {
        let mut v = json!({ "permissions": { "allow": CBC_ALLOW_RULES } });
        let changed = ensure_allow_rules(&mut v, CBC_ALLOW_RULES).unwrap();
        assert!(!changed, "re-adding existing rules must report no change");
        let allow = v["permissions"]["allow"].as_array().unwrap();
        assert_eq!(
            allow.len(),
            CBC_ALLOW_RULES.len(),
            "no duplicates after idempotent run"
        );
    }

    #[test]
    fn preserves_unrelated_top_level_and_permissions_keys() {
        let mut v = json!({
            "hooks": { "PreToolUse": [] },
            "permissions": { "defaultMode": "auto" }
        });
        ensure_allow_rules(&mut v, CBC_ALLOW_RULES).unwrap();
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
        let allow = v["permissions"]["allow"].as_array().unwrap();
        for rule in CBC_ALLOW_RULES {
            assert!(allow.iter().any(|r| r.as_str() == Some(*rule)));
        }
    }

    #[test]
    fn errors_rather_than_clobbering_a_wrong_typed_permissions() {
        let mut v = json!({ "permissions": 5 });
        assert!(
            ensure_allow_rules(&mut v, CBC_ALLOW_RULES).is_err(),
            "a non-object permissions value must error, not be overwritten"
        );
    }

    #[test]
    fn errors_rather_than_clobbering_a_wrong_typed_allow() {
        let mut v = json!({ "permissions": { "allow": "not-an-array" } });
        assert!(
            ensure_allow_rules(&mut v, CBC_ALLOW_RULES).is_err(),
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
        let allow = written["permissions"]["allow"].as_array().unwrap();
        for rule in CBC_ALLOW_RULES {
            assert!(
                allow.iter().any(|r| r.as_str() == Some(*rule)),
                "rule {rule:?} must be in the created file"
            );
        }
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
        let allow = written["permissions"]["allow"].as_array().unwrap();
        for rule in CBC_ALLOW_RULES {
            assert!(
                allow.iter().any(|r| r.as_str() == Some(*rule)),
                "rule {rule:?} must be appended"
            );
        }
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
            "a second run must detect all rules and report no change"
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

    // --- multi-rule tests (CBC_ALLOW_RULES slice) ---

    #[test]
    fn ensure_allow_rules_adds_all_rules_to_empty_settings() {
        let mut v = json!({});
        let changed = ensure_allow_rules(&mut v, CBC_ALLOW_RULES).unwrap();
        assert!(changed, "empty settings must be modified when rules are added");
        let allow = v["permissions"]["allow"].as_array().unwrap();
        for rule in CBC_ALLOW_RULES {
            assert!(
                allow.iter().any(|r| r.as_str() == Some(rule)),
                "rule {rule:?} must be present"
            );
        }
        assert_eq!(allow.len(), CBC_ALLOW_RULES.len(), "no extra entries");
    }

    #[test]
    fn ensure_allow_rules_is_idempotent_when_all_rules_already_present() {
        let mut v = json!({});
        ensure_allow_rules(&mut v, CBC_ALLOW_RULES).unwrap();
        let changed = ensure_allow_rules(&mut v, CBC_ALLOW_RULES).unwrap();
        assert!(!changed, "second run must report no change");
        let allow = v["permissions"]["allow"].as_array().unwrap();
        assert_eq!(
            allow.len(),
            CBC_ALLOW_RULES.len(),
            "no duplicates after idempotent run"
        );
    }

    #[test]
    fn ensure_allow_rules_adds_only_missing_rules_when_partially_present() {
        // Seed with just the MCP rule; the Bash rule is missing.
        let mut v = json!({ "permissions": { "allow": [CBC_ALLOW_RULES[0]] } });
        let changed = ensure_allow_rules(&mut v, CBC_ALLOW_RULES).unwrap();
        assert!(changed, "a partial set must still report a change");
        let allow = v["permissions"]["allow"].as_array().unwrap();
        assert_eq!(
            allow.len(),
            CBC_ALLOW_RULES.len(),
            "exactly the full set after partial add"
        );
        for rule in CBC_ALLOW_RULES {
            assert!(
                allow.iter().any(|r| r.as_str() == Some(rule)),
                "rule {rule:?} must be present after partial add"
            );
        }
    }

    #[test]
    fn apply_allow_rule_writes_both_rules_to_new_settings_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".claude").join("settings.json");
        let outcome = apply_allow_rule(&path).unwrap();
        assert_eq!(outcome, Outcome::Created);
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let allow = written["permissions"]["allow"].as_array().unwrap();
        assert_eq!(allow.len(), CBC_ALLOW_RULES.len());
        for rule in CBC_ALLOW_RULES {
            assert!(
                allow.iter().any(|r| r.as_str() == Some(*rule)),
                "rule {rule:?} must be in the created file"
            );
        }
    }

    #[test]
    fn apply_allow_rule_already_present_means_all_rules_present() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        apply_allow_rule(&path).unwrap(); // Created — all rules written
        let outcome = apply_allow_rule(&path).unwrap();
        assert_eq!(
            outcome,
            Outcome::AlreadyPresent,
            "all rules already present must yield AlreadyPresent"
        );
        // No .bak should have been written since no write occurred.
        let backup = PathBuf::from(format!("{}.bak", path.display()));
        assert!(
            !backup.exists(),
            "no .bak file must be created when nothing was written"
        );
    }
}
