#!/usr/bin/env python3
"""Install the CBC hierarchy package into one project under temporary wip- names (trial decision T-17).

Claude resolves same-named skills enterprise, then personal, then project, and Codex does not
document a winner for `.agents/skills`. Distinct names are the only way a project copy cannot be
shadowed by the global cbc-* skills. Writes only <project>/.claude/skills and <project>/.agents/skills.
"""

import argparse
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
import install_hierarchy_skills as hierarchy  # noqa: E402


PREFIX = "wip-"
HOSTS = (".claude/skills", ".agents/skills")
NOTICE = ("Trial copy: use only the `wip-cbc-*` skills (and `wip-emperor`) in this project and ignore any "
          "legacy `cbc-*` skill with the same role, which belongs to deployed v1. Scripts accept the `wip-` "
          "role form and record the canonical `cbc-*` role.")
# The MCP server name the trial wiring registers the candidate adapter under.
SERVER = "cbc-trial"
# The canonical package is written for both transports: legacy rules for "today", candidate rules for
# "once the candidate core is adopted". In the trial the candidate core is in use, so this notice settles
# every such fork. It goes into every installed Markdown file, because references are read on demand.
TRANSPORT_NOTICE = f"""**Trial transport.** This project runs the candidate core. This notice overrides every file in
this package, references included. Your CBC tools come from the `{SERVER}` MCP server.

- Every rule that applies once the candidate core is adopted (or "in use") applies now.
- Ignore every rule about today's or legacy transport: receivers, polls, read cursors, the
  `connections:` state section, numeric caps and extensions, and closing by consensus.
- Never load the base `cbc` skill or any CBC skill without the `wip-` prefix. Never use the
  `chatbotchat` MCP server or the `cbc` command line. A `cbc_send` or `cbc_close` from any server
  other than `{SERVER}` is legacy: stop and tell the user.
- The core binds your identity. Never type a label or pass a member.
- Routes: `cbc_discover`, then `cbc_connect` for a report or peer room. Connecting is idempotent and
  returns an existing room, so there is no room ID to carry or join.
- Receiving: you are told when items are pending; then call `cbc_check`. Start no receiver, poll or
  timer.
- Sending: `cbc_send`. Closing: `cbc_close`, alone, with a one-line reason. Rooms have no caps.
- Work items: `cbc_work_register`, `cbc_work_update`, `cbc_work_get`, and `cbc_work_hitl_on` for the
  Primarch. A Chapter Master connects its report room to the Primarch before `cbc_work_register`, or
  the Primarch never sees the registration.
- Questions: `cbc_request_*`. Holds and stops have no record yet; send them as messages.
- A handoff is not part of this trial. If asked for one, tell the user."""
# Longest first, bounded by anything that is not a word character or hyphen, so `cbc-hierarchy-dir`
# placeholders stay untouched and a second pass over rewritten text changes nothing.
NAME_PATTERN = re.compile(r"(?<![\w-])(" + "|".join(
    re.escape(name) for name in sorted(hierarchy.NAMES, key=len, reverse=True)) + r")(?![\w-])")


def trial_name(name):
    return PREFIX + name


def rewrite(text):
    return NAME_PATTERN.sub(lambda match: trial_name(match.group(1)), text)


def with_notice(skill):
    if not skill.startswith("---\n"):
        raise ValueError("skill entrypoint has no frontmatter")
    end = skill.index("\n---\n", 4) + 5
    return skill[:end] + "\n" + NOTICE + "\n\n" + TRANSPORT_NOTICE + "\n" + skill[end:]


def build(stage, source=hierarchy.SOURCE):
    """Write the renamed package into stage. Scripts keep canonical role IDs as data."""
    for name in hierarchy.NAMES:
        destination = stage / trial_name(name)
        shutil.copytree(source / name, destination, ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
        for path in destination.rglob("*"):
            if path.is_file() and path.suffix in {".md", ".yaml"}:
                text = rewrite(path.read_text())
                if path == destination / "SKILL.md":
                    text = with_notice(text)
                elif path.suffix == ".md":
                    text = TRANSPORT_NOTICE + "\n\n" + text
                path.write_text(text)


def install(project, check=False):
    project = Path(project).expanduser().absolute()
    for relative in (".claude", ".agents", *HOSTS):
        if (project / relative).is_symlink():
            raise ValueError(f"refusing symlink: {project / relative}")
    with tempfile.TemporaryDirectory(prefix="cbc-trial-skills-") as stage:
        build(Path(stage))
        return hierarchy.install([project / host for host in HOSTS], source=Path(stage), check=check,
                                 names=tuple(trial_name(name) for name in hierarchy.NAMES))


def unignored(project):
    """Hosts with any trial package file git would stage in project; empty outside a git work tree."""
    project = Path(project).expanduser().absolute()
    with tempfile.TemporaryDirectory(prefix="cbc-trial-skills-") as stage:
        build(Path(stage))
        files = sorted(str(path.relative_to(stage)) for path in Path(stage).rglob("*") if path.is_file())
    probes = {host: [f"{host}/{relative}" for relative in files] for host in HOSTS}
    try:
        result = subprocess.run(["git", "-C", str(project), "check-ignore", "--no-index", "--stdin"],
                                input="\n".join(path for paths in probes.values() for path in paths) + "\n",
                                capture_output=True, text=True)
    except OSError:
        return []
    if result.returncode not in (0, 1):
        return []  # not a work tree, or git failed: nothing to warn about
    ignored = set(result.stdout.splitlines())
    # Every file must be ignored: one ignored skill or entrypoint says nothing about its siblings.
    return [host for host, paths in probes.items() if not ignored.issuperset(paths)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", required=True, type=Path, help="project or worktree root")
    parser.add_argument("--check", action="store_true", help="preflight without writes")
    args = parser.parse_args()
    try:
        changes = install(args.project, check=args.check)
    except (OSError, ValueError) as error:
        parser.exit(1, f"{error}\n")
    print(f"{'Would add' if args.check else 'Added'} {len(changes)} trial skill directories. "
          "Matching skills unchanged; differing skills refused.")
    for path in changes:
        print(path)
    exposed = unignored(args.project)
    if exposed:
        print(f"warning: {', '.join(exposed)}/{PREFIX}* not ignored by git; a routine `git add -A` would commit "
              "the trial copy. Add `/.claude/skills/wip-*/` and `/.agents/skills/wip-*/` to the file named by "
              "`git rev-parse --git-path info/exclude`.", file=sys.stderr)


if __name__ == "__main__":
    main()
