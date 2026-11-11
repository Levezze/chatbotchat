"""Project-local WIP trial skill install (decision T-17). No home or daemon writes."""

from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_hierarchy_skills import ROOT, installer, load


trial = load("trial", ROOT / "tools/install_trial_skills.py")
PRESERVED = "cbc-preserved"
ADAPTERS = ROOT / "crates/cbc-candidate/adapters"
record_tools = load("trial_record_tools", ADAPTERS / "shared/tools.py")

# Legacy-transport instructions in the canonical package (phrase found there, phrase the trial notice
# must use to override it). The package names no legacy tool; its legacy behaviour is prose.
LEGACY_TRANSPORT = {
    "base skill": (r"base CBC skill", "base `cbc` skill"),
    "legacy transport": (r"today's transport|legacy (?:transport|receivers?)", "today's or legacy transport"),
    "receivers": (r"\breceivers?\b", "receivers"),
    "polls": (r"\bpoll", "polls"),
    "consensus": (r"\bconsensus\b", "consensus"),
    "caps": (r"numeric caps", "numeric caps"),
    "connections section": (r"connections:", "`connections:`"),
    "read cursors": (r"read cursor", "read cursors"),
    "adoption": (r"candidate core is (?:adopted|in use)", "candidate core is adopted"),
}


def legacy_categories(text):
    return {name for name, (pattern, _) in LEGACY_TRANSPORT.items() if re.search(pattern, text, re.I)}


def candidate_tool_names():
    """Tools both host adapters list: the base four plus the shared record tools."""
    listed = [set(re.findall(r'"name": "(cbc_\w+)"', (ADAPTERS / host / "adapter.py").read_text()))
              for host in ("claude", "codex")]
    return (listed[0] & listed[1]) | set(record_tools.NAMES)


class TrialSkillTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cbc-trial-skills-test-")
        self.addCleanup(self.temp.cleanup)
        self.project = Path(self.temp.name) / "worktree"
        self.project.mkdir()

    def targets(self):
        return [self.project / ".claude/skills", self.project / ".agents/skills"]

    def test_installs_prefixed_package_into_both_project_hosts_only(self):
        trial.install(self.project)
        for target in self.targets():
            names = sorted(path.name for path in target.iterdir())
            self.assertEqual(names, sorted(trial.trial_name(name) for name in installer.NAMES))
            for name in installer.NAMES:
                skill = (target / trial.trial_name(name) / "SKILL.md").read_text()
                self.assertRegex(skill, rf"(?m)^name: {re.escape(trial.trial_name(name))}$")
        self.assertEqual(sorted(path.name for path in self.project.iterdir()), [".agents", ".claude"])

    def test_every_name_gets_a_unique_wip_prefix(self):
        self.assertEqual(trial.trial_name("cbc-repo-lead"), "wip-cbc-repo-lead")
        self.assertEqual(trial.trial_name("emperor"), "wip-emperor")
        names = [trial.trial_name(name) for name in installer.NAMES]
        self.assertEqual(len(set(names)), len(names))

    def test_each_entrypoint_tells_the_agent_to_ignore_legacy_skills(self):
        trial.install(self.project)
        for target in self.targets():
            for name in installer.NAMES:
                body = (target / trial.trial_name(name) / "SKILL.md").read_text()
                frontmatter_end = body.index("\n---\n", 4) + 5
                first_line = body[frontmatter_end:].lstrip("\n").splitlines()[0]
                self.assertIn("legacy", first_line)
                self.assertIn("wip-cbc-", first_line)

    def test_no_installed_instruction_routes_to_a_legacy_skill_name(self):
        trial.install(self.project)
        pattern = re.compile(r"(?<![\w-])(" + "|".join(
            re.escape(name) for name in sorted(installer.NAMES, key=len, reverse=True)) + r")(?![\w-])")
        for path in self.project.rglob("*"):
            if path.is_file() and path.suffix in {".md", ".yaml"}:
                with self.subTest(path=path.relative_to(self.project)):
                    self.assertEqual(pattern.findall(path.read_text()), [])

    def test_every_file_with_legacy_transport_rules_carries_the_transport_notice(self):
        # Agents read references on demand, so a notice only in SKILL.md would not reach rooms.md.
        trial.install(self.project)
        flagged = []
        for path in self.project.rglob("*.md"):
            text = path.read_text()
            if legacy_categories(text.replace(trial.TRANSPORT_NOTICE, "")):
                flagged.append(path.relative_to(self.project).as_posix())
                with self.subTest(path=flagged[-1]):
                    self.assertIn(trial.TRANSPORT_NOTICE, text)
        self.assertIn(".claude/skills/wip-cbc-hierarchy/references/rooms.md", flagged)
        for target in self.targets():
            for name in installer.NAMES:
                self.assertIn(trial.TRANSPORT_NOTICE, (target / trial.trial_name(name) / "SKILL.md").read_text())

    def test_the_transport_notice_overrides_each_legacy_rule_the_package_contains(self):
        found = set()
        for name in installer.NAMES:
            for path in (installer.SOURCE / name).rglob("*.md"):
                found |= legacy_categories(path.read_text())
        self.assertGreaterEqual(found, {"base skill", "legacy transport", "receivers", "consensus", "adoption"})
        for category in sorted(found):
            with self.subTest(category=category):
                self.assertIn(LEGACY_TRANSPORT[category][1], trial.TRANSPORT_NOTICE)
        for forbidden in ("`chatbotchat`", "`cbc` command line"):
            self.assertIn(forbidden, trial.TRANSPORT_NOTICE)
        self.assertIn(f"`{trial.SERVER}`", trial.TRANSPORT_NOTICE)

    def test_the_transport_notice_names_only_candidate_tools(self):
        # cbc_send and cbc_close exist on the legacy server too, so a wrong name would reach v1.
        candidate = candidate_tool_names()
        self.assertGreaterEqual(candidate, {"cbc_check", "cbc_send", "cbc_discover", "cbc_connect", "cbc_close"})
        named = re.findall(r"`(cbc_[a-z_]+\*?)`", trial.TRANSPORT_NOTICE)
        for tool in named:
            with self.subTest(tool=tool):
                if tool.endswith("*"):
                    self.assertTrue(any(name.startswith(tool[:-1]) for name in candidate))
                else:
                    self.assertIn(tool, candidate)
        for needed in ("cbc_check", "cbc_connect", "cbc_close", "cbc_work_register"):
            self.assertIn(needed, named)

    def test_relative_references_resolve_inside_the_trial_package(self):
        trial.install(self.project)
        checked = 0
        for target in self.targets():
            for document in target.rglob("*.md"):
                for link in re.findall(r"((?:\.\./)+[\w./-]+\.md)", document.read_text()):
                    checked += 1
                    with self.subTest(document=document.relative_to(target), link=link):
                        resolved = (document.parent / link).resolve()
                        self.assertTrue(resolved.is_file())
                        self.assertIn(target.resolve(), resolved.parents)
        self.assertGreater(checked, 20)

    def test_installed_scripts_accept_prefixed_roles_and_record_canonical_ones(self):
        trial.install(self.project)
        scripts = self.targets()[1] / trial.trial_name("cbc-hierarchy") / "scripts"
        workspace = Path(self.temp.name) / "records"
        result = subprocess.run(
            [sys.executable, "-B", "-I", str(scripts / "bootstrap.py"), "--root", str(workspace),
             "--context", "trial-one", "--member", "primarch", "--role", "wip-cbc-repo-lead"],
            capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        record = (workspace / ".cbc/hierarchy/trial-one/primarch/state.md").read_text()
        self.assertRegex(record, r"(?m)^role: cbc-repo-lead$")
        refreshed = subprocess.run(
            [sys.executable, "-B", "-I", str(scripts / "refresh.py"), "--state-dir",
             str(workspace / ".cbc/hierarchy/trial-one/primarch"), "--role", "wip-cbc-repo-lead"],
            capture_output=True, text=True)
        self.assertEqual(refreshed.returncode, 0, refreshed.stderr)
        titles = load("trial_titles", scripts / "session_title.py")
        self.assertEqual(titles.resolve_role("wip-cbc-primarch"), ("cbc-repo-lead", "warhammer"))
        self.assertEqual(titles.resolve_role("wip-cbc-feature-lead"), ("cbc-feature-lead", "default"))

    def test_trial_copy_refuses_global_compaction_enrollment(self):
        # The hook reloads the globally installed role, so a trial Primarch must not enroll (T-17).
        trial.install(self.project)
        home = Path(self.temp.name) / "codex-home"
        home.mkdir()
        script = self.targets()[1] / trial.trial_name("cbc-hierarchy") / "scripts/primarch_compaction.py"
        result = subprocess.run(
            [sys.executable, "-B", "-I", str(script), "register", "--host", "codex", "--session-id", "trial-1"],
            capture_output=True, text=True, env={"CODEX_HOME": str(home), "PATH": "/usr/bin:/bin"})
        self.assertEqual(result.returncode, 1)
        self.assertIn("wip-cbc-repo-lead", result.stderr)
        self.assertEqual(list(home.iterdir()), [])

    def test_trial_copy_names_an_existing_global_enrollment(self):
        # Refusing new enrollment cannot undo an old one; the session must be replaced (Codex review, PR #124).
        trial.install(self.project)
        home = Path(self.temp.name) / "codex-home"
        registry = home / "cbc-hierarchy/primarch-sessions/trial-1.json"
        registry.parent.mkdir(parents=True)
        registry.write_text('{"schema": 1, "role": "cbc-repo-lead", "session_id": "trial-1"}\n')
        script = self.targets()[1] / trial.trial_name("cbc-hierarchy") / "scripts/primarch_compaction.py"
        result = subprocess.run(
            [sys.executable, "-B", "-I", str(script), "register", "--host", "codex", "--session-id", "trial-1"],
            capture_output=True, text=True, env={"CODEX_HOME": str(home), "PATH": "/usr/bin:/bin"})
        self.assertEqual(result.returncode, 1)
        self.assertIn("already enrolled", result.stderr)
        self.assertIn("new session", result.stderr)
        self.assertTrue(registry.is_file())

    def test_cli_warns_when_trial_skills_are_not_git_ignored(self):
        # A routine `git add -A` in a Chapter Master worktree would otherwise commit the trial copy.
        subprocess.run(["git", "init", "-q", str(self.project)], check=True)
        command = [sys.executable, "-B", str(ROOT / "tools/install_trial_skills.py"), "--project", str(self.project)]
        warned = subprocess.run(command + ["--check"], capture_output=True, text=True)
        self.assertEqual(warned.returncode, 0, warned.stderr)
        self.assertIn("not ignored", warned.stderr)
        self.assertIn("info/exclude", warned.stderr)
        exclude = Path(subprocess.check_output(
            ["git", "-C", str(self.project), "rev-parse", "--git-path", "info/exclude"], text=True).strip())
        exclude = exclude if exclude.is_absolute() else self.project / exclude
        exclude.parent.mkdir(parents=True, exist_ok=True)
        # Ignoring one skill (or only entrypoints) still leaves the rest of the package stageable.
        exclude.write_text("/.claude/skills/wip-cbc-hierarchy/\n/.agents/skills/wip-cbc-hierarchy/\n")
        partial = subprocess.run(command + ["--check"], capture_output=True, text=True)
        self.assertIn("not ignored", partial.stderr)
        exclude.write_text("/.claude/skills/wip-*/\n/.agents/skills/wip-*/\n")
        quiet = subprocess.run(command + ["--check"], capture_output=True, text=True)
        self.assertEqual(quiet.returncode, 0, quiet.stderr)
        self.assertNotIn("not ignored", quiet.stderr)

    def test_rerun_is_idempotent_and_other_project_skills_survive(self):
        for target in self.targets():
            (target / PRESERVED).mkdir(parents=True)
            (target / PRESERVED / "SKILL.md").write_text("project skill")
        first = trial.install(self.project)
        self.assertTrue(first)
        self.assertEqual(trial.install(self.project), [])
        for target in self.targets():
            self.assertEqual((target / PRESERVED / "SKILL.md").read_text(), "project skill")

    def test_differing_existing_trial_skill_is_refused_without_writes(self):
        claude = self.targets()[0] / trial.trial_name("cbc-hierarchy")
        claude.mkdir(parents=True)
        (claude / "SKILL.md").write_text("hand edited")
        with self.assertRaisesRegex(ValueError, "differs"):
            trial.install(self.project)
        self.assertEqual((claude / "SKILL.md").read_text(), "hand edited")
        self.assertFalse(self.targets()[1].exists())

    def test_symlinked_skills_directory_is_refused(self):
        outside = Path(self.temp.name) / "outside"
        outside.mkdir()
        (self.project / ".claude").mkdir()
        (self.project / ".claude/skills").symlink_to(outside)
        with self.assertRaisesRegex(ValueError, "symlink"):
            trial.install(self.project)
        self.assertEqual(list(outside.iterdir()), [])

    def test_symlinked_host_directory_is_refused(self):
        outside = Path(self.temp.name) / "outside"
        outside.mkdir()
        (self.project / ".agents").symlink_to(outside)
        with self.assertRaisesRegex(ValueError, "symlink"):
            trial.install(self.project)
        self.assertEqual(list(outside.iterdir()), [])
        self.assertFalse((self.project / ".claude").exists())

    def test_cli_check_is_read_only(self):
        result = subprocess.run(
            [sys.executable, "-B", str(ROOT / "tools/install_trial_skills.py"),
             "--project", str(self.project), "--check"], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"Would add {2 * len(installer.NAMES)}", result.stdout)
        self.assertEqual(list(self.project.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
