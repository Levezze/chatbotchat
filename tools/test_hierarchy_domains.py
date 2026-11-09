"""Simplified topology, historical recovery and reviewed package updates."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
import sys
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_hierarchy_skills import ROOT, bootstrap, installer, load


class DomainTests(unittest.TestCase):
    def test_v2_refresh_keeps_live_work_and_legacy_identity_without_recreating_layers(self):
        refresh = load("v2_refresh", ROOT / "bins/cbc/skill/cbc-hierarchy/scripts/refresh.py")
        with tempfile.TemporaryDirectory() as temp:
            records = bootstrap.bootstrap(Path(temp), "project", "ui", "cbc-feature-lead")
            identity_path = records / "identity.json"
            identity = json.loads(identity_path.read_text())
            identity["role"] = "cbc-domain-lead"
            identity_path.write_text(json.dumps(identity))
            original_identity = identity_path.read_bytes()
            state = records / "state.md"
            old = state.read_text().replace("protocol-version: 3", "protocol-version: 2")
            old = old.replace("role: cbc-feature-lead", "role: cbc-domain-lead")
            old += "\nbranch: owned-live-branch\nchild: actual-native-handle\nhold: deploy-only Q9\n"
            state.write_text(old)
            refresh.refresh(records, "cbc-domain-lead")
            self.assertEqual(state.read_text(), old.replace("protocol-version: 2", "protocol-version: 3"))
            self.assertEqual(identity_path.read_bytes(), original_identity)
            self.assertEqual(len(list(records.parent.iterdir())), 1)

    def test_role_resolution_uses_the_held_member_and_reports_a_mismatch(self):
        titles = load("identity_titles", ROOT / "bins/cbc/skill/cbc-hierarchy/scripts/session_title.py")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            repo = bootstrap.bootstrap(root, "project", "api", "cbc-repo-lead")
            feature = bootstrap.bootstrap(root, "project", "billing", "cbc-feature-lead")
            with self.assertRaises(ValueError):
                titles.read_identity(repo / "identity.json", "project", "billing")
            with self.assertRaises(ValueError):
                titles.read_identity(feature / "identity.json", None, None)
            identity = titles.read_identity(feature / "identity.json", "project", "billing")
            resolved = titles.resolve_entry("cbc-chapter-master", identity=identity)
            self.assertEqual(resolved, {"role": "cbc-feature-lead", "theme": "default",
                                       "invoked": "cbc-feature-lead", "mismatch": False})
            self.assertFalse(titles.resolve_entry("cbc-captain", identity=identity)["mismatch"])

    def test_refresh_matches_the_connections_line_not_free_text_and_preserves_mode(self):
        refresh = load("line_refresh", ROOT / "bins/cbc/skill/cbc-hierarchy/scripts/refresh.py")
        with tempfile.TemporaryDirectory() as temp:
            records = bootstrap.bootstrap(Path(temp), "project", "api", "cbc-repo-lead")
            state = records / "state.md"
            text = state.read_text().replace("protocol-version: 3\n", "")
            text = text.replace("next-action: fill assignment, model and source links; attach assigned route if applicable",
                                "next-action: re-attach connections: listed below; fill")
            state.write_text(text)
            state.chmod(0o640)
            refresh.refresh(records, "cbc-repo-lead")
            self.assertIn("next-action: re-attach connections: listed below; fill\n", state.read_text())
            self.assertEqual(state.read_text().count("\nconnections:\n"), 1)
            self.assertEqual(state.stat().st_mode & 0o777, 0o640)
            state.write_text(text + "\nconnections:\n")
            with self.assertRaises(ValueError):
                refresh.refresh(records, "cbc-repo-lead")

    def test_domain_handoff_retains_both_report_routes_and_owned_notes(self):
        handoff = load("domain_handoff", ROOT / "bins/cbc/skill/cbc-handoff/scripts/handoff.py")
        with tempfile.TemporaryDirectory() as temp:
            records = bootstrap.bootstrap(Path(temp), "project", "ui", "cbc-feature-lead")
            # Existing v2 records remain recoverable even though fresh domains are retired.
            identity = records / "identity.json"
            legacy = json.loads(identity.read_text())
            legacy["role"] = "cbc-domain-lead"
            identity.write_text(json.dumps(legacy))
            state = records / "state.md"
            state.write_text(state.read_text().replace("role: cbc-feature-lead", "role: cbc-domain-lead"))
            state = records / "state.md"
            state.write_text(state.read_text().replace("connections:\n", "connections:\n  repo: room-r\n  feature: room-f\n") + "\nUI constraint: preserve component contract v3\n")
            exported = handoff.prepare(records, "old-domain", "client-chapter-master-ui", {"room-r": 9, "room-f": 4})
            self.assertEqual(handoff.verify(exported)["role"], "cbc-domain-lead")
            self.assertIn("component contract v3", (exported / "state.md").read_text())

    def test_refresh_adds_missing_v1_fields_without_touching_existing_waits(self):
        refresh = load("v1_refresh", ROOT / "bins/cbc/skill/cbc-hierarchy/scripts/refresh.py")
        with tempfile.TemporaryDirectory() as temp:
            records = bootstrap.bootstrap(Path(temp), "project", "api", "cbc-repo-lead")
            state = records / "state.md"
            content = state.read_text().replace("protocol-version: 3\n", "")
            for key in refresh.FIELDS:
                if key not in ("waiting-on", "wait-reason"):
                    content = "\n".join(line for line in content.split("\n") if not line.startswith(key + ":"))
            content = content.replace("waiting-on: none", "waiting-on: Q-9")
            content = content.replace("## Owned notes", "## V1 notes")
            content = content.replace("## Pending decisions", "## V1 pending")
            state.write_text(content)
            refresh.refresh(records, "cbc-repo-lead")
            self.assertIn("waiting-on: Q-9", state.read_text())
            self.assertIn("decision-routes: assigned Primarch or direct user; no Emperor reports", state.read_text())
            self.assertIn("## Owned notes", state.read_text())

    def test_install_failure_rolls_back_prior_host_swaps_and_keeps_backup(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / "source"
            for name in installer.NAMES:
                (source / name / "agents").mkdir(parents=True)
                (source / name / "SKILL.md").write_text(f"name: {name}\n")
                (source / name / "agents/openai.yaml").write_text("interface: {}\n")
            targets = [root / "claude", root / "codex"]
            installer.install(targets, source)
            previous = {name: {p: hashlib.sha256(b).hexdigest() for p, b in installer.manifest(source / name).items()} for name in installer.NAMES}
            name = installer.NAMES[0]
            old = (source / name / "SKILL.md").read_bytes()
            (source / name / "SKILL.md").write_text("updated\n")
            replace = installer.os.replace
            failed = False

            def fail_second_host_once(src, dst):
                nonlocal failed
                if Path(dst) == targets[1] / name and not failed:
                    failed = True
                    raise OSError("synthetic second-host rename failure")
                return replace(src, dst)

            with patch.object(installer.os, "replace", side_effect=fail_second_host_once):
                with self.assertRaises(OSError):
                    installer.install(targets, source, previous=previous, backup=root / "backup")
            for target in targets:
                self.assertEqual((target / name / "SKILL.md").read_bytes(), old)
            self.assertTrue((root / "backup/targets.json").exists())
            self.assertEqual(list(root.glob(".cbc-skill-stage-*")), [])

    def test_update_backup_inside_discovery_is_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / "source"
            for name in installer.NAMES:
                (source / name / "agents").mkdir(parents=True)
                (source / name / "SKILL.md").write_text(f"name: {name}\n")
                (source / name / "agents/openai.yaml").write_text("interface: {}\n")
            target = root / "skills"
            installer.install([target], source)
            previous = {name: {p: hashlib.sha256(b).hexdigest() for p, b in installer.manifest(source / name).items()} for name in installer.NAMES}
            (source / installer.NAMES[0] / "SKILL.md").write_text("updated\n")
            with self.assertRaisesRegex(ValueError, "overlaps"):
                installer.install([target], source, previous=previous, backup=target / "backup")
            self.assertFalse((target / "backup").exists())

    def test_existing_alias_invocation_preserves_canonical_identity_and_theme(self):
        titles = load("domain_titles", ROOT / "bins/cbc/skill/cbc-hierarchy/scripts/session_title.py")
        identity = {"schema": 1, "role": "cbc-feature-lead", "theme": "default"}
        self.assertEqual(titles.resolve_role("cbc-chapter-master", identity=identity, theme="warhammer"),
                         ("cbc-feature-lead", "default"))
        self.assertEqual(titles.resolve_role("cbc-chapter-master"), ("cbc-feature-lead", "warhammer"))
        self.assertEqual(titles.resolve_role("cbc-captain"), ("cbc-feature-lead", "warhammer"))

    def test_new_records_only_create_active_leads_and_reject_retired_tiers(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for role in ("cbc-domain-lead", "cbc-task-agent"):
                with self.assertRaises(ValueError):
                    bootstrap.bootstrap(root, "project", "ui", role)
            with self.assertRaises(ValueError):
                bootstrap.bootstrap(root, "project", "ui", "cbc-feature-lead", domain="ui")
            self.assertFalse((root / ".cbc").exists())
            feature = bootstrap.bootstrap(root, "project", "billing", "cbc-feature-lead")
            self.assertEqual(json.loads((feature / "identity.json").read_text())["role"], "cbc-feature-lead")

    def test_refresh_preserves_identity_and_all_operational_state_and_is_idempotent(self):
        refresh = load("refresh", ROOT / "bins/cbc/skill/cbc-hierarchy/scripts/refresh.py")
        with tempfile.TemporaryDirectory() as temp:
            records = bootstrap.bootstrap(Path(temp), "project", "api", "cbc-repo-lead")
            state = records / "state.md"
            old = state.read_text().replace("protocol-version: 3\n", "")
            old += "\n## Holds\nfeature-a: PAUSE requested Q-9\n## Handled\nroom-a: 123\n"
            state.write_text(old)
            immutable = {name: (records / name).read_bytes() for name in ("identity.json", "brief.md", "questions.md")}
            refresh.refresh(records, "cbc-repo-lead")
            current = state.read_text()
            self.assertIn("protocol-version: 3\n", current)
            self.assertIn("feature-a: PAUSE requested Q-9", current)
            self.assertIn("room-a: 123", current)
            self.assertEqual(current.replace("protocol-version: 3\n", ""), old)
            for name, data in immutable.items():
                self.assertEqual((records / name).read_bytes(), data)
            refresh.refresh(records, "cbc-repo-lead")
            self.assertEqual(state.read_text(), current)
            with self.assertRaises(ValueError):
                refresh.refresh(records, "cbc-domain-lead")
            self.assertEqual(state.read_text(), current)

    def test_reviewed_update_backs_up_known_files_and_refuses_unknown_drift_before_any_writes(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / "source"
            for name in installer.NAMES:
                (source / name / "agents").mkdir(parents=True)
                (source / name / "SKILL.md").write_text(f"name: {name}\n")
                (source / name / "agents/openai.yaml").write_text("interface: {}\n")
            targets = [root / "claude", root / "codex"]
            installer.install(targets, source)
            previous = {name: {path: hashlib.sha256(data).hexdigest() for path, data in installer.manifest(source / name).items()} for name in installer.NAMES}
            changed = installer.NAMES[0]
            old = (source / changed / "SKILL.md").read_bytes()
            (source / changed / "SKILL.md").write_text("reviewed v2\n")
            drift = targets[1] / changed / "SKILL.md"
            drift.write_text("user-owned drift\n")
            with self.assertRaises(ValueError):
                installer.install(targets, source, previous=previous, backup=root / "backup")
            self.assertEqual((targets[0] / changed / "SKILL.md").read_bytes(), old)
            self.assertFalse((root / "backup").exists())
            drift.write_bytes(old)
            installer.install(targets, source, previous=previous, backup=root / "backup", check=True)
            self.assertFalse((root / "backup").exists())
            installer.install(targets, source, previous=previous, backup=root / "backup")
            for index, target in enumerate(targets):
                self.assertEqual((target / changed / "SKILL.md").read_text(), "reviewed v2\n")
                self.assertEqual((root / "backup" / str(index) / changed / "SKILL.md").read_bytes(), old)


if __name__ == "__main__":
    unittest.main()
