"""Session-scoped reload and additive config behavior in disposable directories."""

import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
try:
    import tomllib
except ImportError:
    tomllib = None
import unittest


ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


SCRIPT = ROOT / "bins/cbc/skill/cbc-hierarchy/scripts/primarch_compaction.py"
hook = load("compact_hook", SCRIPT)
installer = load("compact_installer", ROOT / "tools/install_primarch_compaction.py")


class CompactionTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.base = Path(temporary.name)
        self.root = self.base / "host"
        self.root.mkdir()
        self.role = self.root / "skills/cbc-repo-lead/SKILL.md"
        self.role.parent.mkdir(parents=True)
        self.role.write_text("---\nname: cbc-repo-lead\n---\nCurrent role instructions")
        self.script = self.root / "skills/cbc-hierarchy/scripts/primarch_compaction.py"
        self.script.parent.mkdir(parents=True)
        self.script.write_text(SCRIPT.read_text())
        self.event = {"hook_event_name": "SessionStart", "source": "compact", "session_id": "native_123"}

    def test_unregistered_other_events_and_subagents_are_silent(self):
        self.assertIsNone(hook.reload_context(self.root, self.event))
        hook.register(self.root, "native_123")
        for changes in ({"source": "startup"}, {"source": "fork"}, {"source": "resume"},
                        {"session_id": "other"}, {"agent_id": "child"},
                        {"hook_event_name": "PreCompact"}):
            self.assertIsNone(hook.reload_context(self.root, self.event | changes))

    def test_each_compaction_reads_current_skill_and_changes_no_records(self):
        path = hook.register(self.root, "native_123")
        original = path.read_bytes()
        first = hook.reload_context(self.root, self.event)["hookSpecificOutput"]
        self.assertEqual(first["hookEventName"], "SessionStart")
        self.assertIn("Current role instructions", first["additionalContext"])
        self.role.write_text("Updated role instructions")
        second = hook.reload_context(self.root, self.event)["hookSpecificOutput"]
        self.assertIn("Updated role instructions", second["additionalContext"])
        self.assertNotIn("Current role instructions", second["additionalContext"])
        self.assertEqual(path.read_bytes(), original)
        self.assertEqual(hook.register(self.root, "native_123"), path)

    def test_large_or_missing_skill_requests_read_without_partial_rules(self):
        hook.register(self.root, "native_123")
        for present in (True, False):
            if present:
                self.role.write_text("x" * 20000)
            else:
                self.role.unlink()
            result = hook.reload_context(self.root, self.event)["hookSpecificOutput"]
            self.assertIn(str(self.role), result["additionalContext"])
            self.assertLess(len(result["additionalContext"]), hook.MAX_CONTEXT)
            self.assertNotIn("x" * 50, result["additionalContext"])

    def test_host_registry_isolation_and_invalid_ids(self):
        hook.register(self.root, "native_123")
        other = self.base / "other-host"
        self.assertIsNone(hook.reload_context(other, self.event))
        for session in (None, "", "../other", "a/b", "a" * 129):
            with self.assertRaises(ValueError):
                hook.register(self.root, session)

    def test_registry_symlinks_and_conflicting_binding_refused(self):
        path = hook.register(self.root, "native_123")
        path.write_text('{"role":"cbc-feature-lead"}')
        with self.assertRaises(ValueError):
            hook.register(self.root, "native_123")
        path.unlink()
        path.symlink_to(self.role)
        with self.assertRaises(ValueError):
            hook.register(self.root, "native_123")

    @unittest.skipIf(tomllib is None, "Codex hook installer needs Python 3.11+")
    def test_installer_preserves_codex_settings_and_native_trust(self):
        path = self.root / "config.toml"
        path.write_text('model = "existing"\n[hooks.state.old]\ntrusted_hash = "unchanged"\n'
                        '[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = "command"\ncommand = "existing"\n')
        original = path.read_bytes()
        backup = self.base / "backup"
        self.assertTrue(installer.install(self.root, "codex", backup))
        data = tomllib.loads(path.read_text())
        self.assertEqual(data["model"], "existing")
        self.assertEqual(data["hooks"]["state"]["old"]["trusted_hash"], "unchanged")
        self.assertEqual(data["hooks"]["Stop"][0]["hooks"][0]["command"], "existing")
        self.assertEqual(data["hooks"]["SessionStart"][0]["matcher"], "^compact$")
        self.assertEqual((backup / "config.toml").read_bytes(), original)
        saved = path.read_bytes()
        self.assertFalse(installer.install(self.root, "codex", self.base / "not-needed"))
        self.assertEqual(path.read_bytes(), saved)
        self.assertFalse((self.base / "not-needed").exists())

    def test_installer_preserves_claude_hooks_permissions_and_backs_up(self):
        path = self.root / "settings.json"
        original = {"permissions": {"defaultMode": "plan"}, "hooks": {
            "SessionStart": [{"hooks": [{"type": "command", "command": "cbc hook session-start"}]}]}}
        path.write_text(json.dumps(original))
        self.assertTrue(installer.install(self.root, "claude", self.base / "backup"))
        data = json.loads(path.read_text())
        self.assertEqual(data["permissions"], original["permissions"])
        self.assertEqual(data["hooks"]["SessionStart"][0], original["hooks"]["SessionStart"][0])
        self.assertEqual(len(data["hooks"]["SessionStart"]), 2)
        self.assertFalse(installer.install(self.root, "claude", self.base / "unused"))

    def test_changed_existing_hook_is_not_silently_replaced(self):
        path = self.root / "settings.json"
        data = {"hooks": {"SessionStart": [{"matcher": "startup", "hooks": [
            {"type": "command", "command": "python primarch_compaction.py run --host claude"}]}]}}
        path.write_text(json.dumps(data))
        original = path.read_bytes()
        with self.assertRaises(ValueError):
            installer.install(self.root, "claude", self.base / "backup")
        self.assertEqual(path.read_bytes(), original)
        self.assertFalse((self.base / "backup").exists())

    def test_cli_is_silent_for_unenrolled_and_emits_context_when_enrolled(self):
        import os
        env = dict(os.environ, CODEX_HOME=str(self.root), CODEX_THREAD_ID="native_123")
        command = [sys.executable, "-B", str(SCRIPT)]
        quiet = subprocess.run(command + ["run", "--host", "codex"], env=env,
                               input=json.dumps(self.event), text=True, capture_output=True, check=True)
        self.assertEqual(quiet.stdout, "")
        subprocess.run(command + ["register", "--host", "codex"], env=env,
                       capture_output=True, check=True)
        result = subprocess.run(command + ["run", "--host", "codex"], env=env,
                                input=json.dumps(self.event), text=True, capture_output=True, check=True)
        data = json.loads(result.stdout)
        self.assertEqual(set(data), {"hookSpecificOutput"})
        self.assertIn("Current role instructions", data["hookSpecificOutput"]["additionalContext"])


if __name__ == "__main__":
    unittest.main()
