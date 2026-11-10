#!/usr/bin/env python3
"""Refresh compact v3 fields without resetting identity, work or real holds."""
import argparse
import json
import os
from pathlib import Path
import re
import tempfile


FIELDS = {
    "repo-lead": "unset",
    "decision-routes": "assigned Primarch or direct user; no Emperor reports", "waiting-on": "none",
    "blocked-action": "none", "wait-reason": "none", "continue-with": "assigned unblocked work",
}
ROLES = ("cbc-project-lead", "cbc-repo-lead", "cbc-domain-lead", "cbc-feature-lead", "cbc-task-agent")


def canonical_role(value):
    """Accept the project-local trial skill form (trial decision T-17); records keep the canonical role."""
    role = value.removeprefix("wip-")
    if role not in ROLES:
        raise argparse.ArgumentTypeError(f"choose from {', '.join(ROLES)} (optionally wip-prefixed)")
    return role


def refresh(directory, role):
    directory = Path(directory)
    if directory.is_symlink() or role not in ROLES:
        raise ValueError("invalid record directory or role")
    for name in ("identity.json", "state.md", "brief.md", "questions.md"):
        if (directory / name).is_symlink() or not (directory / name).is_file():
            raise ValueError("missing or symlinked member record")
    identity = json.loads((directory / "identity.json").read_text())
    if identity.get("schema") != 1 or identity.get("role") != role:
        raise ValueError("refresh cannot change the logical role")
    path = directory / "state.md"
    original = path.read_text()
    markers = list(re.finditer(r"^connections:[ \t]*$", original, re.MULTILINE))
    if len(markers) != 1:
        raise ValueError("expected exactly one connections line")
    prefix, tail = original[:markers[0].start()], original[markers[0].start():]
    for key in ("context", "member", "role"):
        matches = re.findall(rf"^{key}: (.+)$", prefix, re.MULTILINE)
        if matches != [identity.get(key)]:
            raise ValueError("state and identity disagree")
    versions = re.findall(r"^protocol-version: (.+)$", prefix, re.MULTILINE)
    if len(versions) > 1 or (versions and versions[0] not in ("1", "2", "3")):
        raise ValueError("unsupported protocol version")
    result = prefix
    if versions:
        result = re.sub(r"^protocol-version: [12]$", "protocol-version: 3", result, count=1, flags=re.MULTILINE)
    else:
        result += "protocol-version: 3\n"
    additions = [f"{key}: {value}" for key, value in FIELDS.items()
                 if not re.search(rf"^{key}:", prefix, re.MULTILINE)]
    if additions:
        result += "\n".join(additions) + "\n\n"
    result += tail
    for heading, comment in (
        ("Owned notes", "Owner notes and deadlines, separate from child facts."),
        ("Pending decisions", "ID | actual authority | blocked action/reason | since | independent work | refs"),
    ):
        if not re.search(rf"^## {heading}$", result, re.MULTILINE):
            result += f"\n## {heading}\n<!-- {comment} -->\n"
    if result != original:
        # Replace only this member's working map. Identity, brief, questions and sealed exports stay intact.
        with tempfile.NamedTemporaryFile(mode="w", dir=directory, prefix=".refresh-", delete=False) as stream:
            temporary = Path(stream.name)
            stream.write(result)
            stream.flush()
            os.fsync(stream.fileno())
        try:
            os.chmod(temporary, path.stat().st_mode & 0o777)
            os.replace(temporary, path)
        finally:
            temporary.unlink(missing_ok=True)
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", required=True, type=Path)
    parser.add_argument("--role", required=True, type=canonical_role)
    args = parser.parse_args()
    try:
        print(refresh(args.state_dir, args.role))
    except (OSError, ValueError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
