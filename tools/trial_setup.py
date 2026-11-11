#!/usr/bin/env python3
"""Prepare and tear down the project-local wiring for one CBC trial case (T-15, T-17, T-20, T-24).

A case directory holds everything one run uses:

    CASE/pin/        the pinned adapters and core binary, with pin.json (commit and hashes)
    CASE/market/     a private plugin catalog holding the Claude wake mod, for a local-scope install
    CASE/core/       the core root the operator creates with `cbc-candidate init` (sockets, profile)
    CASE/seats/      one record per prepared seat: its project and the files written there
    CASE/launch-sheet-phase<N>.md   the rendered launch sheet

Every host configuration points at CASE/pin, never at a worktree, so an agent editing the adapters in
its own worktree cannot change its own transport mid-run (T-15).

This tool writes only inside the case directory, inside each seat's project, and one marked block in
that repository's shared `info/exclude`. It installs nothing, starts nothing, and never reads the
core's keys or the device profile. Steps that touch native app state (project trust, the plugin
install) are user steps on the launch sheet.
"""

import argparse
import hashlib
import json
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
import install_trial_skills as trial  # noqa: E402


ROOT = Path(__file__).resolve().parents[1]
SERVER = trial.SERVER
PLUGIN = "cbc-candidate-wake"
ADAPTERS = "crates/cbc-candidate/adapters"
TEMPLATE = ROOT / "docs/design/trial-launch-sheet-phase{phase}.md"
# The longest Unix socket path macOS can bind is 103 bytes.
SOCKET_LIMIT = 104
SOCKETS = ("connector.sock", "device.sock")
LEGACY_SKILL = re.compile(r"^(?:cbc|cbc-.+|emperor)$")
SEAT_NAME = re.compile(r"^[a-z][a-z0-9-]{0,31}$")
# The case name becomes the context `trial-<name>` and the run `trial-<name>-run1`; a run id is 1 to 64
# letters, digits, '-', '_' or '.'.
CASE_NAME = re.compile(r"^[a-z0-9][a-z0-9._-]{0,39}$")
HOSTS = ("claude", "codex")
PHASES = {1: {"peer-a": "claude", "peer-b": "codex"},
          2: {"primarch": "codex", "cm-a": "claude", "cm-b": "claude"}}
# Everything prepare writes, plus the hierarchy skill's own state directory, which it keeps out of
# commits without changing an ignore file.
EXCLUDES = ("/.mcp.json", "/.cbc-candidate-wake.json", "/.claude/settings.local.json", "/.codex/config.toml",
            "/.claude/skills/wip-*/", "/.agents/skills/wip-*/", "/.cbc/hierarchy/")
CREATED_DIRS = (".claude", ".claude/skills", ".agents", ".agents/skills", ".codex")
SKILL_HOSTS = trial.HOSTS


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def git(directory, *args, check=True):
    result = subprocess.run(["git", "-C", str(directory), *args], capture_output=True, text=True)
    if check and result.returncode:
        raise ValueError(f"git {' '.join(args)} failed in {directory}: {result.stderr.strip()}")
    return result.stdout


def market_name(case):
    return f"cbc-trial-{Path(case).name}"


def core(case):
    return Path(case) / "core"


def canonical(path):
    """The real path: the core writes canonical socket paths, and the driver compares them literally."""
    return Path(path).expanduser().resolve()


def check_case_name(case):
    if not CASE_NAME.match(Path(case).name):
        raise ValueError(f"case name must be 1 to 40 lowercase letters, digits, '.', '_' or '-', starting with a "
                         f"letter or digit, so it can form the run id: {Path(case).name!r}")


def shell_safe(*paths):
    """Refuse paths the launch sheet could not paste into a shell unquoted."""
    for path in paths:
        if shlex.quote(str(path)) != str(path):
            raise ValueError(f"path needs shell quoting; choose one without spaces or shell characters: {path}")


def tree_digests(directory, skip=()):
    return {str(path.relative_to(directory)): sha256(path)
            for path in sorted(Path(directory).rglob("*")) if path.is_file() and path.name not in skip}


# --- pin ---------------------------------------------------------------------------------------------

def check_sockets(case):
    for name in SOCKETS:
        path = core(case) / name
        if len(str(path).encode()) >= SOCKET_LIMIT:
            raise ValueError(f"socket path is {len(str(path).encode())} bytes, over the {SOCKET_LIMIT - 1}-byte "
                             f"Unix limit; choose a shorter case directory: {path}")


def stage_pin(source, binary, stage):
    ignore = shutil.ignore_patterns("__pycache__", "*.pyc", "node_modules")
    shutil.copytree(source / ADAPTERS, stage / "pin/adapters", ignore=ignore)
    shutil.copy2(binary, stage / "pin/cbc-candidate")
    (stage / "pin/cbc-candidate").chmod(0o755)
    shutil.copytree(stage / "pin/adapters/claude/mod", stage / f"market/plugins/{PLUGIN}", ignore=ignore)
    catalog = stage / "market/.claude-plugin/marketplace.json"
    catalog.parent.mkdir(parents=True)
    return catalog


def pin(source, binary, case):
    """Copy the adapters and the core binary from a clean checkout into CASE/pin. Idempotent."""
    source, binary, case = (canonical(path) for path in (source, binary, case))
    check_case_name(case)
    shell_safe(case)
    check_sockets(case)
    if git(source, "status", "--porcelain", "--", ADAPTERS):
        raise ValueError(f"uncommitted changes under {ADAPTERS}; pin a clean checkout")
    if not binary.is_file():
        raise ValueError(f"no core binary at {binary}")
    commit = git(source, "rev-parse", "HEAD").strip()
    with tempfile.TemporaryDirectory(prefix="cbc-trial-pin-") as temp:
        stage = Path(temp)
        catalog = stage_pin(source, binary, stage)
        catalog.write_text(json.dumps({"name": market_name(case), "owner": {"name": "CBC trial"},
                                       "plugins": [{"name": PLUGIN, "source": f"./plugins/{PLUGIN}"}]},
                                      indent=2) + "\n")
        record = {"v": 1, "commit": commit, "files": tree_digests(stage / "pin")}
        (stage / "pin/pin.json").write_text(json.dumps(record, indent=2) + "\n")
        if (case / "pin").exists() or (case / "market").exists():
            if (tree_digests(case / "pin") != tree_digests(stage / "pin")
                    or tree_digests(case / "market") != tree_digests(stage / "market")):
                raise ValueError(f"an existing pin in {case} differs; use a new case directory")
            return record
        case.mkdir(parents=True, exist_ok=True)
        shutil.copytree(stage / "pin", case / "pin", symlinks=True)
        shutil.copytree(stage / "market", case / "market", symlinks=True)
    return record


def verify_pin(case):
    record_path = Path(case) / "pin/pin.json"
    if not record_path.is_file():
        raise ValueError(f"no pin in {case}; run `pin` first")
    record = json.loads(record_path.read_text())
    if tree_digests(Path(case) / "pin", skip=("pin.json",)) != record["files"]:
        raise ValueError(f"the pinned files in {case} no longer match pin.json; start a new case")
    return record


# --- prepare -----------------------------------------------------------------------------------------

def legacy_skill_names(claude_skills_dir):
    directory = Path(claude_skills_dir)
    if not directory.is_dir():
        return []
    return sorted(path.name for path in directory.iterdir() if path.is_dir() and LEGACY_SKILL.match(path.name))


def claude_files(case, python, claude_skills_dir):
    socket = str(core(case) / "device.sock")
    server = {"command": python, "args": ["-B", "-I", str(Path(case) / "pin/adapters/claude/adapter.py")],
              "env": {"CBC_CANDIDATE_CONNECTOR": socket, "CBC_CANDIDATE_CHANNEL": "0"}}
    settings = {
        "enabledMcpjsonServers": [SERVER],
        # T-24: the trial server only. No shell, file or bypass allowance.
        "permissions": {"allow": [f"mcp__{SERVER}__*"], "deny": ["mcp__chatbotchat__*"]},
        "skillOverrides": {name: "off" for name in legacy_skill_names(claude_skills_dir)},
    }
    return {
        ".mcp.json": json.dumps({"mcpServers": {SERVER: server}}, indent=2) + "\n",
        ".claude/settings.local.json": json.dumps(settings, indent=2) + "\n",
        ".cbc-candidate-wake.json": json.dumps({"v": 1, "socketPath": socket}, indent=2) + "\n",
    }


def codex_files(case, python):
    adapter = str(Path(case) / "pin/adapters/codex/adapter.py")
    lines = [
        "# Written by tools/trial_setup.py for one CBC trial seat. Its teardown removes this file.",
        f"[mcp_servers.{SERVER}]",
        f"command = {json.dumps(python)}",
        f"args = {json.dumps(['-B', '-I', adapter])}",
        '# T-24: pre-approves the trial server\'s tools only.',
        'default_tools_approval_mode = "approve"',
        "",
        f"[mcp_servers.{SERVER}.env]",
        f"CBC_CANDIDATE_PROFILE = {json.dumps(str(core(case) / 'device-profile.json'))}",
        f"CBC_CANDIDATE_CONNECTOR = {json.dumps(str(core(case) / 'device.sock'))}",
        "",
        "# Hides the legacy server in this project only.",
        "[mcp_servers.chatbotchat]",
        "enabled = false",
    ]
    return {".codex/config.toml": "\n".join(lines) + "\n"}


def exclude_block(case):
    # The digest of the canonical case path keeps two cases that share a basename apart in one repository.
    name = f"{Path(case).name} {hashlib.sha256(str(case).encode()).hexdigest()[:12]}"
    return "\n".join([f"# cbc-trial {name} begin", *EXCLUDES, f"# cbc-trial {name} end"]) + "\n"


def exclude_path(project):
    return Path(git(project, "rev-parse", "--path-format=absolute", "--git-path", "info/exclude").strip())


def refuse_symlinks(project, relatives):
    for relative in relatives:
        parts = Path(relative).parts
        for depth in range(1, len(parts) + 1):
            path = project.joinpath(*parts[:depth])
            if path.is_symlink():
                raise ValueError(f"refusing symlink: {path}")


def seat_path(case, seat):
    return Path(case) / "seats" / f"{seat}.json"


def prepare(case, project, seat, host, python, claude_skills_dir=Path.home() / ".claude/skills"):
    """Write one seat's project-local wiring. Nothing is written unless every check passes."""
    case, project = canonical(case), canonical(project)
    check_case_name(case)
    shell_safe(case, project, python)
    if not SEAT_NAME.match(seat):
        raise ValueError(f"seat names are lowercase words and hyphens: {seat!r}")
    if host not in HOSTS:
        raise ValueError(f"host must be one of {HOSTS}")
    verify_pin(case)
    check_sockets(case)
    if Path(git(project, "rev-parse", "--show-toplevel").strip()).resolve() != project.resolve():
        raise ValueError(f"{project} is not the root of a git work tree")
    record_path = seat_path(case, seat)
    if record_path.exists():
        previous = json.loads(record_path.read_text())
        if previous["project"] != str(project) or previous["host"] != host:
            raise ValueError(f"seat {seat} is already prepared for {previous['project']} ({previous['host']})")
    legacy = sorted(path.name for pattern in ("worker-*.md", "orchestration-*.md")
                    for path in (project / ".cbc").glob(pattern))
    if legacy:
        raise ValueError(f"legacy CBC state in {project / '.cbc'} ({', '.join(legacy)}); the global legacy "
                         "hooks act on it, so use a fresh worktree")

    files = claude_files(case, python, claude_skills_dir) if host == "claude" else codex_files(case, python)
    refuse_symlinks(project, [*files, *CREATED_DIRS])
    tracked = git(project, "ls-files", "--", *files).split()
    if tracked:
        raise ValueError(f"tracked by git, refusing to write: {', '.join(tracked)}")
    for relative, text in files.items():
        path = project / relative
        if path.exists() and path.read_text() != text:
            raise ValueError(f"existing file differs; nothing written: {path}")
    trial.install(project, check=True)
    exclude = exclude_path(project)
    block = exclude_block(case)
    current = exclude.read_text() if exclude.exists() else ""
    marker = block.splitlines()[0]
    if marker in current and block not in current:
        raise ValueError(f"the trial block in {exclude} was edited; restore or remove it first")

    created = [relative for relative in CREATED_DIRS if not (project / relative).exists()]
    # The ignore rule goes first, so an interrupted run never leaves a trial file visible to git.
    if block not in current:
        exclude.parent.mkdir(parents=True, exist_ok=True)
        exclude.write_text(current + ("" if current.endswith("\n") or not current else "\n") + block)
    for relative, text in files.items():
        path = project / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    trial.install(project)
    if record_path.exists():
        created = json.loads(record_path.read_text())["created_dirs"]
    record_path.parent.mkdir(parents=True, exist_ok=True)
    record_path.write_text(json.dumps({
        "v": 1, "seat": seat, "host": host, "project": str(project), "python": python,
        "exclude": str(exclude), "created_dirs": created,
        "files": {relative: {"sha256": hashlib.sha256(text.encode()).hexdigest(), "content": text}
                  for relative, text in files.items()},
    }, indent=2) + "\n")
    return sorted(files)


# --- teardown ----------------------------------------------------------------------------------------

def without_plugin_keys(settings, case):
    """Settings minus exactly the entries the documented local-scope plugin install adds."""
    settings = json.loads(json.dumps(settings))
    for key, entry in (("enabledPlugins", f"{PLUGIN}@{market_name(case)}"),
                       ("extraKnownMarketplaces", market_name(case))):
        if isinstance(settings.get(key), dict):
            settings[key].pop(entry, None)
            if not settings[key]:
                del settings[key]
    return settings


def owned(path, expected, case):
    if path.is_symlink() or not path.is_file():
        return False
    if sha256(path) == expected["sha256"]:
        return True
    if path.name == "settings.local.json":
        try:
            return without_plugin_keys(json.loads(path.read_text()), case) == json.loads(expected["content"])
        except ValueError:
            return False
    return False


def remove_skills(project):
    try:
        missing = set(trial.install(project, check=True))
    except ValueError:
        return False
    for host in SKILL_HOSTS:
        for name in trial.hierarchy.NAMES:
            path = project / host / trial.trial_name(name)
            if str(path) not in missing and path.is_dir() and not path.is_symlink():
                shutil.rmtree(path)
    return True


def archive_hierarchy(case, seat, project, removed, kept):
    """Move the hierarchy skill's local records into the private case before their ignore rule goes."""
    hierarchy = project / ".cbc/hierarchy"
    if not hierarchy.exists() and not hierarchy.is_symlink():
        return
    target = case / "archive" / seat / "hierarchy"
    if hierarchy.is_symlink() or not hierarchy.is_dir() or target.exists() or target.is_symlink():
        kept.append(".cbc/hierarchy")
        return
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.move(str(hierarchy), str(target))
    removed.append(".cbc/hierarchy (archived)")
    if not any(hierarchy.parent.iterdir()):
        hierarchy.parent.rmdir()


def teardown(case, seat):
    """Remove what prepare wrote for one seat, if unchanged. Returns (removed, kept) project paths."""
    case = canonical(case)
    record_path = seat_path(case, seat)
    record = json.loads(record_path.read_text())
    project = Path(record["project"])
    removed, kept = [], []
    for relative, expected in record["files"].items():
        path = project / relative
        if not path.exists() and not path.is_symlink():
            continue
        if owned(path, expected, case):
            path.unlink()
            removed.append(relative)
        else:
            kept.append(relative)
    if remove_skills(project):
        removed.extend(f"{host}/wip-*" for host in SKILL_HOSTS)
    else:
        kept.extend(f"{host}/wip-*" for host in SKILL_HOSTS)
    for relative in sorted(record["created_dirs"], key=lambda item: item.count("/"), reverse=True):
        path = project / relative
        if path.is_dir() and not path.is_symlink() and not any(path.iterdir()):
            path.rmdir()
    archive_hierarchy(case, seat, project, removed, kept)
    if kept:
        return removed, kept
    record_path.unlink()
    others = [json.loads(path.read_text()) for path in (case / "seats").glob("*.json")]
    exclude = Path(record["exclude"])
    if not any(other["exclude"] == record["exclude"] for other in others) and exclude.exists():
        text = exclude.read_text()
        block = exclude_block(case)
        if block in text:
            exclude.write_text(text.replace(block, "", 1))
            removed.append("info/exclude block")
    return removed, kept


# --- launch sheet ------------------------------------------------------------------------------------

def render_sheet(case, phase, template=None):
    """Fill the committed launch-sheet template with this case's paths. Writes CASE/launch-sheet-phaseN.md."""
    case = canonical(case)
    if phase not in PHASES:
        raise ValueError(f"phase must be one of {sorted(PHASES)}")
    record = verify_pin(case)
    seats = {}
    for seat, host in PHASES[phase].items():
        path = seat_path(case, seat)
        if path.exists():
            seats[seat] = json.loads(path.read_text())
            if seats[seat]["host"] != host:
                raise ValueError(f"seat {seat} must be a {host} seat for phase {phase}")
    others = sorted(seat for other, hosts in PHASES.items() if other != phase
                    for seat in hosts if seat_path(case, seat).exists())
    if others:
        raise ValueError(f"one case per phase: this case also holds {', '.join(others)}")
    missing = sorted(set(PHASES[phase]) - set(seats))
    if missing:
        raise ValueError(f"prepare these seats first: {', '.join(missing)}")
    values = {
        "CASE": case, "CORE": core(case), "COMMIT": record["commit"], "CONTEXT": f"trial-{case.name}",
        "BINARY": case / "pin/cbc-candidate", "ROUTER": case / "pin/adapters/device/router.py",
        "MARKET": case / "market", "MARKET_NAME": market_name(case), "PLUGIN": PLUGIN,
        "PLUGIN_DIR": case / "market/plugins" / PLUGIN, "SERVER": SERVER,
        "PYTHON": next(iter(seats.values()))["python"], "SETUP": f"{next(iter(seats.values()))['python']} -B {Path(__file__).resolve()}",
    }
    for seat, seat_record in seats.items():
        values[f"{seat.upper().replace('-', '_')}_WORKTREE"] = seat_record["project"]
    shell_safe(*(value for value in values.values() if not str(value).count(" -B ")))
    text = Path(template or str(TEMPLATE).format(phase=phase)).read_text()
    for key, value in values.items():
        text = text.replace(f"<{key}>", str(value))
    unfilled = sorted(set(re.findall(r"<[A-Z][A-Z0-9_]*>", text)))
    if unfilled:
        raise ValueError(f"unfilled placeholders: {', '.join(unfilled)}")
    out = case / f"launch-sheet-phase{phase}.md"
    out.write_text(text)
    return out


# --- command line ------------------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    command = commands.add_parser("pin", help="copy the adapters and binary from a clean checkout")
    command.add_argument("--source", type=Path, default=ROOT, help="clean checkout at the pinned commit")
    command.add_argument("--binary", type=Path, required=True, help="the cbc-candidate binary built from it")
    command.add_argument("--case", type=Path, required=True)
    command = commands.add_parser("prepare", help="write one seat's project-local wiring")
    command.add_argument("--case", type=Path, required=True)
    command.add_argument("--project", type=Path, required=True, help="the seat's worktree root")
    command.add_argument("--seat", required=True, help="peer-a, peer-b, primarch, cm-a or cm-b")
    command.add_argument("--host", choices=HOSTS, required=True)
    command.add_argument("--python", default=shutil.which("python3") or sys.executable)
    command = commands.add_parser("sheet", help="render the launch sheet into the case directory")
    command.add_argument("--case", type=Path, required=True)
    command.add_argument("--phase", type=int, choices=sorted(PHASES), required=True)
    command = commands.add_parser("teardown", help="remove one seat's wiring if unchanged")
    command.add_argument("--case", type=Path, required=True)
    command.add_argument("--seat", required=True)
    args = parser.parse_args()
    try:
        if args.command == "pin":
            record = pin(args.source, args.binary, args.case)
            print(f"Pinned commit {record['commit']}, {len(record['files'])} files.")
        elif args.command == "prepare":
            written = prepare(args.case, args.project, args.seat, args.host, args.python)
            print(f"Prepared seat {args.seat}: {', '.join(written)} and the wip- skills.")
        elif args.command == "sheet":
            print(render_sheet(args.case, args.phase))
        else:
            removed, kept = teardown(args.case, args.seat)
            print(f"Removed: {', '.join(removed) or 'nothing'}")
            if kept:
                print(f"Kept, changed since prepare: {', '.join(kept)}", file=sys.stderr)
                sys.exit(2)
    except (OSError, ValueError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
