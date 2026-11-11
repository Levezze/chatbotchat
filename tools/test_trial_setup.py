"""Trial setup: pinned transport, per-seat project-local wiring, teardown and the launch sheet.

Every test runs on temporary directories and a temporary git repository. Nothing here reads or writes
the home directory, the live daemon or a real trial project.
"""

import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_hierarchy_skills import ROOT, load


setup = load("trial_setup", ROOT / "tools/trial_setup.py")
trial = load("trial_skills_for_setup", ROOT / "tools/install_trial_skills.py")
SECRET = "sentinel-secret-value-7f3a"


def git(directory, *args):
    return subprocess.run(["git", "-C", str(directory), *args], check=True, capture_output=True, text=True).stdout


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def fake_source(base):
    """A clean git checkout with the adapter layout the setup pins."""
    source = base / "source"
    adapters = source / "crates/cbc-candidate/adapters"
    for relative, text in {
        "claude/adapter.py": "# claude adapter\n",
        "codex/adapter.py": "# codex adapter\n",
        "device/router.py": "# router\n",
        "shared/tools.py": "# tools\n",
        "claude/mod/hooks/hooks.json": '{"modules": ["./register.js"]}\n',
        "claude/mod/hooks/register.js": "// wake\n",
        "claude/mod/package.json": '{"type": "module"}\n',
    }.items():
        path = adapters / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    (adapters / "shared/__pycache__").mkdir()
    (adapters / "shared/__pycache__/tools.cpython-314.pyc").write_bytes(b"cache")
    git(base, "init", "-q", str(source))
    (source / ".gitignore").write_text("__pycache__/\n")
    git(source, "add", "-A")
    git(source, "-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "fixture")
    binary = base / "cbc-candidate"
    binary.write_bytes(b"\x7fELF fake binary")
    return source, binary


def fake_project(base, name="project"):
    project = base / name
    git(base, "init", "-q", str(project))
    (project / "README.md").write_text("project\n")
    git(project, "add", "-A")
    git(project, "-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "init")
    return project


def legacy_skills(base):
    skills = base / "claude-skills"
    for name in ("cbc", "cbc-worker", "cbc-hierarchy", "emperor", "tdd"):
        (skills / name).mkdir(parents=True)
    return skills


class Case:
    """A pinned case directory, with a seat helper."""

    def __init__(self, base):
        base = Path(base).resolve()  # the setup canonicalizes paths, so compare against real paths
        self.base = base
        self.source, self.binary = fake_source(base)
        self.dir = base / "c1"
        self.skills = legacy_skills(base)
        setup.pin(self.source, self.binary, self.dir)

    def prepare(self, project, seat, host):
        return setup.prepare(self.dir, project, seat, host, python="/usr/bin/python3",
                             claude_skills_dir=self.skills)


class PinTest(unittest.TestCase):
    def test_pin_copies_adapters_and_binary_with_hashes_and_commit(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            source, binary = fake_source(base)
            record = setup.pin(source, binary, base / "c1")
            pinned = base / "c1/pin"
            self.assertEqual(record["commit"], git(source, "rev-parse", "HEAD").strip())
            self.assertEqual((pinned / "cbc-candidate/adapters/codex/adapter.py").read_text(), "# codex adapter\n")
            self.assertFalse((pinned / "cbc-candidate/adapters/shared/__pycache__").exists())
            self.assertEqual(record["files"]["bin/cbc-candidate"], digest(binary))
            self.assertEqual(record["files"]["cbc-candidate/adapters/claude/adapter.py"], digest(pinned / "cbc-candidate/adapters/claude/adapter.py"))
            self.assertEqual(json.loads((pinned / "pin.json").read_text()), record)

    def test_pin_stages_a_private_marketplace_for_the_wake_mod(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            source, binary = fake_source(base)
            setup.pin(source, binary, base / "c1")
            market = base / "c1/market"
            catalog = json.loads((market / ".claude-plugin/marketplace.json").read_text())
            self.assertEqual(catalog["name"], "cbc-trial-c1")
            self.assertEqual(catalog["plugins"], [{"name": "cbc-candidate-wake",
                                                   "source": "./plugins/cbc-candidate-wake"}])
            self.assertTrue((market / "plugins/cbc-candidate-wake/hooks/register.js").is_file())

    def test_pin_refuses_a_dirty_adapter_tree(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            source, binary = fake_source(base)
            (source / "crates/cbc-candidate/adapters/codex/adapter.py").write_text("# edited\n")
            with self.assertRaisesRegex(ValueError, "uncommitted"):
                setup.pin(source, binary, base / "c1")
            self.assertFalse((base / "c1").exists())

    def test_pin_is_idempotent_and_refuses_a_different_existing_pin(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            source, binary = fake_source(base)
            first = setup.pin(source, binary, base / "c1")
            self.assertEqual(setup.pin(source, binary, base / "c1"), first)
            binary.write_bytes(b"another build")
            with self.assertRaisesRegex(ValueError, "differs"):
                setup.pin(source, binary, base / "c1")

    def test_pin_refuses_socket_paths_a_unix_socket_cannot_bind(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            source, binary = fake_source(base)
            deep = base / ("d" * 90) / "c1"
            with self.assertRaisesRegex(ValueError, "socket path"):
                setup.pin(source, binary, deep)


class PrepareTest(unittest.TestCase):
    def test_a_claude_seat_gets_exactly_its_files_pointing_at_the_pinned_copy(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            project = fake_project(base)
            written = case.prepare(project, "cm-a", "claude")
            self.assertEqual(set(written), {".mcp.json", ".claude/settings.local.json", ".cbc-candidate-wake.json"})
            server = json.loads((project / ".mcp.json").read_text())["mcpServers"]
            self.assertEqual(set(server), {"cbc-trial"})
            self.assertEqual(server["cbc-trial"]["command"], "/usr/bin/python3")
            self.assertEqual(server["cbc-trial"]["args"],
                             ["-B", "-I", str(case.dir / "pin/cbc-candidate/adapters/claude/adapter.py")])
            self.assertEqual(server["cbc-trial"]["env"], {
                "CBC_CANDIDATE_CONNECTOR": str(case.dir / "core/device.sock"), "CBC_CANDIDATE_CHANNEL": "0"})
            wake = json.loads((project / ".cbc-candidate-wake.json").read_text())
            self.assertEqual(wake, {"v": 1, "socketPath": str(case.dir / "core/device.sock")})
            self.assertTrue((project / ".claude/skills/wip-cbc-feature-lead/SKILL.md").is_file())
            self.assertTrue((project / ".agents/skills/wip-cbc-feature-lead/SKILL.md").is_file())

    def test_claude_settings_allow_only_the_trial_server_and_hide_legacy_cbc(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            project = fake_project(base)
            case.prepare(project, "cm-a", "claude")
            settings = json.loads((project / ".claude/settings.local.json").read_text())
            self.assertEqual(settings["enabledMcpjsonServers"], ["cbc-trial"])
            self.assertEqual(settings["permissions"], {"allow": ["mcp__cbc-trial__*"],
                                                       "deny": ["mcp__chatbotchat__*"]})
            self.assertEqual(settings["skillOverrides"], {
                "cbc": "off", "cbc-hierarchy": "off", "cbc-worker": "off", "emperor": "off"})
            self.assertEqual(set(settings), {"enabledMcpjsonServers", "permissions", "skillOverrides"})

    def test_a_codex_seat_gets_its_config_and_no_claude_files(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            project = fake_project(base)
            written = case.prepare(project, "primarch", "codex")
            self.assertEqual(set(written), {".codex/config.toml"})
            config = tomllib.loads((project / ".codex/config.toml").read_text())
            self.assertEqual(config["mcp_servers"]["chatbotchat"], {"enabled": False})
            server = config["mcp_servers"]["cbc-trial"]
            self.assertEqual(server["command"], "/usr/bin/python3")
            self.assertEqual(server["args"], ["-B", "-I", str(case.dir / "pin/cbc-candidate/adapters/codex/adapter.py")])
            self.assertEqual(server["default_tools_approval_mode"], "approve")
            self.assertEqual(server["env"], {"CBC_CANDIDATE_PROFILE": str(case.dir / "core/device-profile.json"),
                                             "CBC_CANDIDATE_CONNECTOR": str(case.dir / "core/device.sock")})
            self.assertFalse((project / ".mcp.json").exists())
            self.assertTrue((project / ".agents/skills/wip-cbc-repo-lead/SKILL.md").is_file())

    def test_no_generated_path_points_into_a_worktree_or_the_source(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            claude, codex = fake_project(base, "claude-seat"), fake_project(base, "codex-seat")
            case.prepare(claude, "cm-a", "claude")
            case.prepare(codex, "primarch", "codex")
            for project, files in ((claude, (".mcp.json", ".claude/settings.local.json", ".cbc-candidate-wake.json")),
                                   (codex, (".codex/config.toml",))):
                for relative in files:
                    for found in re.findall(r"/[^\s\"']+", (project / relative).read_text()):
                        if found.startswith(str(base)):
                            self.assertTrue(found.startswith(str(case.dir)), (relative, found))

    def test_generated_files_never_carry_profile_or_key_contents(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            (case.dir / "core").mkdir()
            (case.dir / "core/owner.key").write_text(SECRET)
            (case.dir / "core/device-profile.json").write_text(json.dumps({"token": SECRET}))
            claude, codex = fake_project(base, "claude-seat"), fake_project(base, "codex-seat")
            case.prepare(claude, "cm-a", "claude")
            case.prepare(codex, "primarch", "codex")
            for path in [*claude.rglob("*"), *codex.rglob("*"), *(case.dir / "seats").rglob("*")]:
                if path.is_file() and ".git" not in path.parts:
                    self.assertNotIn(SECRET, path.read_text(errors="replace"), path)

    def test_every_written_path_is_git_ignored_by_one_block(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            project = fake_project(base)
            written = case.prepare(project, "cm-a", "claude")
            case.prepare(project, "cm-a", "claude")
            probes = [*written, ".claude/skills/wip-cbc-hierarchy/SKILL.md",
                      ".agents/skills/wip-cbc-hierarchy/SKILL.md", ".codex/config.toml",
                      ".cbc/hierarchy/context/member.md"]
            ignored = subprocess.run(["git", "-C", str(project), "check-ignore", "--no-index", *probes],
                                     capture_output=True, text=True).stdout.split()
            self.assertEqual(sorted(ignored), sorted(probes))
            self.assertEqual(git(project, "status", "--porcelain"), "")
            exclude = Path(git(project, "rev-parse", "--path-format=absolute", "--git-path", "info/exclude").strip())
            self.assertEqual(exclude.read_text().count(setup.exclude_block(case.dir).splitlines()[0]), 1)

    def test_prepare_is_idempotent(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            project = fake_project(base)
            case.prepare(project, "cm-a", "claude")
            before = {path: path.read_bytes() for path in project.rglob("*") if path.is_file() and ".git" not in path.parts}
            case.prepare(project, "cm-a", "claude")
            after = {path: path.read_bytes() for path in project.rglob("*") if path.is_file() and ".git" not in path.parts}
            self.assertEqual(before, after)

    def test_a_differing_existing_file_is_refused_and_nothing_is_written(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            project = fake_project(base)
            (project / ".claude").mkdir()
            (project / ".claude/settings.local.json").write_text('{"permissions": {"allow": ["Bash"]}}\n')
            with self.assertRaisesRegex(ValueError, "differs"):
                case.prepare(project, "cm-a", "claude")
            self.assertFalse((project / ".mcp.json").exists())
            self.assertFalse((project / ".claude/skills").exists())
            self.assertFalse((case.dir / "seats/cm-a.json").exists())

    def test_symlinked_targets_are_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            project = fake_project(base)
            (base / "elsewhere").mkdir()
            (project / ".codex").symlink_to(base / "elsewhere")
            with self.assertRaisesRegex(ValueError, "symlink"):
                case.prepare(project, "primarch", "codex")
            self.assertEqual(list((base / "elsewhere").iterdir()), [])

    def test_a_tracked_target_path_is_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            project = fake_project(base)
            (project / ".mcp.json").write_text("{}\n")
            git(project, "add", ".mcp.json")
            with self.assertRaisesRegex(ValueError, "tracked"):
                case.prepare(project, "cm-a", "claude")

    def test_legacy_cbc_state_in_the_project_is_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            project = fake_project(base)
            (project / ".cbc").mkdir()
            (project / ".cbc/worker-x.md").write_text("status: ACTIVE\n")
            with self.assertRaisesRegex(ValueError, "legacy CBC state"):
                case.prepare(project, "cm-a", "claude")

    def test_an_edited_exclude_block_is_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            project = fake_project(base)
            exclude = Path(git(project, "rev-parse", "--path-format=absolute", "--git-path", "info/exclude").strip())
            exclude.write_text(setup.exclude_block(case.dir).splitlines()[0] + "\n/.mcp.json\n")
            with self.assertRaisesRegex(ValueError, "edited"):
                case.prepare(project, "cm-a", "claude")
            self.assertFalse((project / ".mcp.json").exists())

    def test_a_tampered_pin_is_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            project = fake_project(base)
            (case.dir / "pin/cbc-candidate/adapters/codex/adapter.py").write_text("# swapped\n")
            with self.assertRaisesRegex(ValueError, "pin"):
                case.prepare(project, "primarch", "codex")

    def test_a_seat_name_cannot_be_reused_for_another_project(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            case.prepare(fake_project(base, "one"), "cm-a", "claude")
            with self.assertRaisesRegex(ValueError, "seat"):
                case.prepare(fake_project(base, "two"), "cm-a", "claude")


class TeardownTest(unittest.TestCase):
    def test_teardown_removes_only_what_prepare_wrote(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            project = fake_project(base)
            (project / ".claude").mkdir()
            (project / ".claude/notes.txt").write_text("mine\n")
            case.prepare(project, "cm-a", "claude")
            removed, kept = setup.teardown(case.dir, "cm-a")
            self.assertEqual(kept, [])
            self.assertIn(".mcp.json", removed)
            self.assertFalse((project / ".mcp.json").exists())
            self.assertFalse((project / ".cbc-candidate-wake.json").exists())
            self.assertFalse((project / ".claude/settings.local.json").exists())
            self.assertFalse((project / ".claude/skills").exists())
            self.assertFalse((project / ".agents").exists())
            self.assertEqual((project / ".claude/notes.txt").read_text(), "mine\n")
            exclude = Path(git(project, "rev-parse", "--path-format=absolute", "--git-path", "info/exclude").strip())
            self.assertNotIn("cbc-trial", exclude.read_text())
            self.assertFalse((case.dir / "seats/cm-a.json").exists())

    def test_teardown_accepts_the_keys_the_local_plugin_install_adds(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            project = fake_project(base)
            case.prepare(project, "cm-a", "claude")
            path = project / ".claude/settings.local.json"
            settings = json.loads(path.read_text())
            settings["enabledPlugins"] = {"cbc-candidate-wake@cbc-trial-c1": True}
            settings["extraKnownMarketplaces"] = {"cbc-trial-c1": {"source": {"source": "directory",
                                                                               "path": str(case.dir / "market")}}}
            path.write_text(json.dumps(settings, indent=4))
            removed, kept = setup.teardown(case.dir, "cm-a")
            self.assertIn(".claude/settings.local.json", removed)
            self.assertFalse(path.exists())

    def test_teardown_keeps_a_file_someone_changed_and_says_so(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            project = fake_project(base)
            case.prepare(project, "cm-a", "claude")
            path = project / ".claude/settings.local.json"
            settings = json.loads(path.read_text())
            settings["permissions"]["allow"].append("Bash")
            path.write_text(json.dumps(settings))
            removed, kept = setup.teardown(case.dir, "cm-a")
            self.assertEqual(kept, [".claude/settings.local.json"])
            self.assertTrue(path.exists())
            self.assertFalse((project / ".mcp.json").exists())
            self.assertTrue((case.dir / "seats/cm-a.json").exists(), "a seat with kept files stays on record")

    def test_the_shared_exclude_block_stays_until_the_last_seat_in_that_repository_leaves(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            project = fake_project(base)
            worktree = base / "linked"
            git(project, "worktree", "add", "-q", "--detach", str(worktree))
            case.prepare(project, "cm-a", "claude")
            case.prepare(worktree, "primarch", "codex")
            exclude = Path(git(project, "rev-parse", "--path-format=absolute", "--git-path", "info/exclude").strip())
            setup.teardown(case.dir, "cm-a")
            self.assertIn(setup.exclude_block(case.dir), exclude.read_text())
            setup.teardown(case.dir, "primarch")
            self.assertNotIn("cbc-trial", exclude.read_text())


class PathSafetyTest(unittest.TestCase):
    """Paths the operator will paste into a shell, and paths the core canonicalizes."""

    def test_a_case_reached_through_a_symlink_is_wired_by_its_real_path(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp).resolve()
            case = Case(base)
            alias = base / "alias"
            alias.symlink_to(case.dir)
            project = fake_project(base, "cm-a")
            setup.prepare(alias, project, "cm-a", "claude", python="/usr/bin/python3", claude_skills_dir=case.skills)
            text = (project / ".mcp.json").read_text()
            self.assertIn(str(case.dir / "core/device.sock"), text)
            self.assertNotIn(str(alias), text)
            self.assertTrue((case.dir / "seats/cm-a.json").is_file())

    def test_paths_that_need_shell_quoting_are_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp).resolve()
            (base / "a").mkdir()
            source, binary = fake_source(base / "a")
            with self.assertRaisesRegex(ValueError, "shell"):
                setup.pin(source, binary, base / "a/trial space/c1")
            self.assertFalse((base / "a/trial space").exists())
            (base / "b").mkdir()
            case = Case(base / "b")
            project = fake_project(base / "b", "cm a")
            with self.assertRaisesRegex(ValueError, "shell"):
                case.prepare(project, "cm-a", "claude")
            self.assertFalse((project / ".mcp.json").exists())

    def test_case_names_that_cannot_form_a_run_id_are_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp).resolve()
            source, binary = fake_source(base)
            for name in ("case+1", "a" * 41, "Case"):
                with self.assertRaisesRegex(ValueError, "case name"):
                    setup.pin(source, binary, base / name)
                self.assertFalse((base / name).exists())

    def test_cases_sharing_a_basename_keep_separate_exclude_blocks(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp).resolve()
            (base / "x").mkdir()
            (base / "y").mkdir()
            first, second = Case(base / "x"), Case(base / "y")
            self.assertEqual(first.dir.name, second.dir.name)
            project = fake_project(base, "main")
            linked = base / "linked"
            git(project, "worktree", "add", "-q", "-b", "linked", str(linked))
            first.prepare(project, "cm-a", "claude")
            second.prepare(linked, "cm-a", "claude")
            setup.teardown(first.dir, "cm-a")
            self.assertEqual(git(linked, "status", "--porcelain"), "")
            self.assertEqual(git(project, "status", "--porcelain"), "")

    def test_pinned_adapter_paths_keep_the_sampler_contract(self):
        # sampler::adapter_label only counts a script under `cbc-candidate/adapters/<dir>/<stem>.py`.
        contract = re.compile(r"cbc-candidate/adapters/[a-z0-9_]+/[a-z0-9_]+\.py$")
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            claude, codex = fake_project(base, "peer-a"), fake_project(base, "peer-b")
            case.prepare(claude, "peer-a", "claude")
            case.prepare(codex, "peer-b", "codex")
            scripts = [json.loads((claude / ".mcp.json").read_text())["mcpServers"]["cbc-trial"]["args"][-1],
                       tomllib.loads((codex / ".codex/config.toml").read_text())["mcp_servers"]["cbc-trial"]["args"][-1]]
            text = setup.render_sheet(case.dir, 1).read_text()
            scripts.append(re.search(r"-B -I (\S+router\.py)", text).group(1))
            for script in scripts:
                self.assertRegex(script, contract)
                self.assertTrue(Path(script).is_file(), script)
            self.assertRegex(text, r"\S+/cbc-candidate init ")

    def test_a_gitignore_negation_that_exposes_a_trial_path_is_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            project = fake_project(base)
            (project / ".gitignore").write_text("!/.cbc/hierarchy/\n")
            git(project, "add", ".gitignore")
            git(project, "-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "negate")
            with self.assertRaisesRegex(ValueError, "not ignored"):
                case.prepare(project, "cm-a", "claude")
            self.assertFalse((project / ".mcp.json").exists())
            exclude = Path(git(project, "rev-parse", "--path-format=absolute", "--git-path", "info/exclude").strip())
            self.assertNotIn("cbc-trial", exclude.read_text())

    def test_a_case_inside_a_seat_worktree_is_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp).resolve()
            case = Case(base)
            project = fake_project(base)
            inner = project / "c2"
            setup.pin(case.source, case.binary, inner)
            with self.assertRaisesRegex(ValueError, "inside"):
                setup.prepare(inner, project, "cm-a", "claude", python="/usr/bin/python3",
                              claude_skills_dir=case.skills)
            self.assertFalse((project / ".mcp.json").exists())

    def test_teardown_keeps_a_file_reached_through_a_symlinked_directory(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            project = fake_project(base)
            case.prepare(project, "cm-a", "claude")
            elsewhere = base / "elsewhere"
            (project / ".claude").rename(elsewhere)
            (project / ".claude").symlink_to(elsewhere)
            removed, kept = setup.teardown(case.dir, "cm-a")
            self.assertIn(".claude/settings.local.json", kept)
            self.assertTrue((elsewhere / "settings.local.json").is_file())

    def test_a_negation_exposing_one_installed_skill_is_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            project = fake_project(base)
            (project / ".gitignore").write_text(f"!/.agents/skills/{trial.trial_name('cbc-hierarchy')}/\n")
            git(project, "add", ".gitignore")
            git(project, "-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "negate")
            with self.assertRaisesRegex(ValueError, "not ignored"):
                case.prepare(project, "cm-a", "claude")
            self.assertFalse((project / ".agents").exists())

    def test_a_changed_marketplace_plugin_fails_the_pin_check(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            (case.dir / "market/plugins/cbc-candidate-wake/hooks/register.js").write_text("// swapped\n")
            with self.assertRaisesRegex(ValueError, "no longer match"):
                case.prepare(fake_project(base), "cm-a", "claude")

    def test_sheets_start_the_sampler_and_feed_it_to_the_report(self):
        for phase, seats in LaunchSheetTest.SEATS.items():
            with tempfile.TemporaryDirectory() as temp:
                base = Path(temp)
                case = Case(base)
                for seat, host in seats.items():
                    case.prepare(fake_project(base, seat), seat, host)
                text = setup.render_sheet(case.dir, phase).read_text()
                self.assertRegex(text, r"cbc-candidate sample --root \S+ --protected-db")
                self.assertRegex(text, r"cbc-candidate report [^\n]*--samples ")

    def test_rendered_setup_commands_run_through_python(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            for seat, host in LaunchSheetTest.SEATS[1].items():
                case.prepare(fake_project(base, seat), seat, host)
            text = setup.render_sheet(case.dir, 1).read_text()
            script = str((ROOT / "tools/trial_setup.py").resolve())
            self.assertIn(f"/usr/bin/python3 -B {script} teardown", text)
            for match in re.finditer(re.escape(script), text):
                self.assertEqual(text[match.start() - 20:match.start()], "/usr/bin/python3 -B ")

    def test_teardown_archives_hierarchy_records_before_dropping_the_ignore_rule(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            project = fake_project(base, "cm-a")
            case.prepare(project, "cm-a", "claude")
            records = project / ".cbc/hierarchy/trial-c1/cm-a"
            records.mkdir(parents=True)
            (records / "state.json").write_text("{}")
            removed, kept = setup.teardown(case.dir, "cm-a")
            self.assertEqual(kept, [])
            self.assertFalse((project / ".cbc").exists())
            archived = case.dir / "archive/cm-a/hierarchy/trial-c1/cm-a/state.json"
            self.assertEqual(archived.read_text(), "{}")
            self.assertEqual(git(project, "status", "--porcelain"), "")

    def test_teardown_keeps_the_ignore_rule_when_hierarchy_records_cannot_be_archived(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            project = fake_project(base, "cm-a")
            case.prepare(project, "cm-a", "claude")
            (project / ".cbc/hierarchy/x").mkdir(parents=True)
            (project / ".cbc/hierarchy/x/state.json").write_text("{}")
            (case.dir / "archive/cm-a/hierarchy").mkdir(parents=True)
            removed, kept = setup.teardown(case.dir, "cm-a")
            self.assertIn(".cbc/hierarchy", kept)
            self.assertEqual(git(project, "status", "--porcelain"), "")
            self.assertTrue((case.dir / "seats/cm-a.json").is_file())


class LaunchSheetTest(unittest.TestCase):
    TEMPLATES = {1: ROOT / "docs/design/trial-launch-sheet-phase1.md",
                 2: ROOT / "docs/design/trial-launch-sheet-phase2.md"}
    SEATS = {1: {"peer-a": "claude", "peer-b": "codex"},
             2: {"primarch": "codex", "cm-a": "claude", "cm-b": "claude"}}

    def test_the_committed_templates_are_placeholders_only(self):
        for template in self.TEMPLATES.values():
            text = template.read_text()
            self.assertNotIn("/Users/", text)
            self.assertIn("<CASE>", text)

    def test_each_phase_renders_both_stages_for_every_seat_with_no_placeholder_left(self):
        for phase, seats in self.SEATS.items():
            with tempfile.TemporaryDirectory() as temp:
                base = Path(temp)
                case = Case(base)
                projects = {}
                for seat, host in seats.items():
                    projects[seat] = fake_project(base, seat)
                    case.prepare(projects[seat], seat, host)
                sheet = setup.render_sheet(case.dir, phase)
                text = sheet.read_text()
                self.assertEqual(sheet.parent, case.dir)
                self.assertIsNone(re.search(r"<[A-Z][A-Z0-9_]*>", text), text)
                for seat, project in projects.items():
                    self.assertIn(str(project), text)
                    self.assertRegex(text, rf"Stage A[^\n]*{re.escape(seat)}")
                    self.assertRegex(text, rf"Stage B[^\n]*{re.escape(seat)}")
                self.assertIn("mcp__chatbotchat__", text)
                self.assertIn(str(case.dir / "pin/bin/cbc-candidate"), text)
                self.assertIn(str(case.dir / "pin/cbc-candidate/adapters/device/router.py"), text)

    def test_each_sheet_stands_alone_and_names_its_run_literally(self):
        for phase, seats in self.SEATS.items():
            with tempfile.TemporaryDirectory() as temp:
                base = Path(temp)
                case = Case(base)
                for seat, host in seats.items():
                    case.prepare(fake_project(base, seat), seat, host)
                text = setup.render_sheet(case.dir, phase).read_text()
                self.assertNotIn("$RUN", text)
                self.assertNotIn("phase 2 sheet", text)
                self.assertIn(f"--run trial-{case.dir.name}-run1", text)
                for seat in seats:
                    self.assertRegex(text, rf"retire [^\n]*--member {re.escape(seat)}\b")
                    self.assertRegex(text, rf"teardown --case [^\n]*--seat {re.escape(seat)}\b")

    def test_a_case_holding_seats_of_both_phases_is_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            for seat, host in {**self.SEATS[1], **self.SEATS[2]}.items():
                case.prepare(fake_project(base, seat), seat, host)
            for phase in self.SEATS:
                with self.assertRaisesRegex(ValueError, "one case per phase"):
                    setup.render_sheet(case.dir, phase)

    def test_rendering_refuses_a_missing_seat(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = Case(base)
            case.prepare(fake_project(base, "cm-a"), "cm-a", "claude")
            with self.assertRaisesRegex(ValueError, "primarch"):
                setup.render_sheet(case.dir, 2)


if __name__ == "__main__":
    unittest.main()
