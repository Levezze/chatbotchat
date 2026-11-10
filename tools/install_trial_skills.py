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
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
import install_hierarchy_skills as hierarchy  # noqa: E402


PREFIX = "wip-"
HOSTS = (".claude/skills", ".agents/skills")
NOTICE = ("Trial copy: use only the `wip-cbc-*` skills (and `wip-emperor`) in this project and ignore any "
          "legacy `cbc-*` skill with the same role, which belongs to deployed v1. Scripts accept the `wip-` "
          "role form and record the canonical `cbc-*` role.")
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
    return skill[:end] + "\n" + NOTICE + "\n" + skill[end:]


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


if __name__ == "__main__":
    main()
