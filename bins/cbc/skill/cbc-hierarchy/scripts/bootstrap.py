#!/usr/bin/env python3
"""Create compact, member-owned CBC hierarchy records. No server or watcher."""

import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import re
import subprocess
import sys


ROLES = ("cbc-project-lead", "cbc-repo-lead", "cbc-feature-lead")
# Project-local trial skills carry a temporary "wip-" name (trial decision T-17); records keep the canonical role.
TRIAL_PREFIX = "wip-"


def canonical_role(value):
    role = value.removeprefix(TRIAL_PREFIX)
    if role not in ROLES:
        raise argparse.ArgumentTypeError(f"choose from {', '.join(ROLES)} (optionally {TRIAL_PREFIX}-prefixed)")
    return role


def token(value):
    if not re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", value) or len(value) > 64:
        raise argparse.ArgumentTypeError("use a kebab-case ID of at most 64 characters")
    return value


def bootstrap(root, context, member, role, theme="default", domain=None):
    token(context)
    token(member)
    role = role.removeprefix(TRIAL_PREFIX)
    if role not in ROLES or theme not in ("default", "warhammer"):
        raise ValueError("unknown role or theme")
    if domain is not None:
        raise ValueError("domain coordination is retired; assign an implementation feature directly")
    base = Path(root).resolve() / ".cbc" / "hierarchy" / context
    # Do not follow a coordination directory into another member's workspace.
    for path in (base.parent.parent, base.parent, base, base / member):
        if path.is_symlink():
            raise ValueError(f"refusing symlink: {path}")
    destination = base / member
    if destination.exists():
        raise FileExistsError(f"records already exist; resume them explicitly: {destination}")
    stamp = datetime.now(timezone.utc).isoformat(timespec="seconds")
    label = f"{context}--{member}"
    powers = {
        "cbc-feature-lead": "ordinary assigned implementation, native subagents and eligible merge under repo-configured gates; existing consequential grants apply",
        "cbc-repo-lead": "repo context, constraints and exceptions; no routine merge approval; existing release grants apply",
        "cbc-project-lead": "future dashboard status and user instructions; dormant before dashboard",
    }[role]
    routes = {
        "cbc-feature-lead": "assigned Primarch report; no routine continuation approval",
        "cbc-repo-lead": "direct user and repo peers; no Emperor reports",
        "cbc-project-lead": "user instructions to Primarchs and clarification replies only; no report intake",
    }[role]
    identity = {
        "schema": 1, "context": context, "member": member,
        "role": role, "theme": theme, "label": label, "created_at": stamp,
    }
    state = f"""# Coordination state
status: SETUP
protocol-version: 3
context: {context}
member: {member}
role: {role}
theme: {theme}
identity: {label}
created-at: {stamp}
updated-at: {stamp}
host-session: unknown
session-title: unset
rename-status: not-attempted
model: unset
parent: unset
repo-lead: unset
decision-routes: {routes}
assignment: unset
source-links: unset
delegated-powers: {powers}
communication: routine coordination authorized within assignment; no per-message allowance
phase: setup
waiting-on: none
blocked-action: none
wait-reason: none
continue-with: assigned unblocked work
next-action: fill assignment, model and source links; attach assigned route if applicable
brief-revision: 0
last-sent-revision: 0
output-mode: QUIET
delivery: unverified

connections:

## Room relationships
<!-- room | kind | participants | scope/reference | close condition -->

## Active children
<!-- Chapter Master member or native helper handle | assignment/source | state@time | blocked action if any -->

## Owned notes
<!-- Owner notes and deadlines, separate from child-reported facts. -->

## Pending decisions
<!-- ID | actual authority | blocked action/reason | since | independent work | request/answer refs -->

## Pending outbound
<!-- event ID/revision | recipient | unsent or uncertain | CBC reference -->

## Working context
<!-- At most a few sentences. No product plan or transcript. -->
"""
    brief = f"""# Coordination brief
owner: {context}/{member}
role: {role}
revision: 0
updated-at: {stamp}
status: setup
readiness: unverified
blockers: none recorded
question-ids: none
artifact-links: none
next: assignment pending
"""
    questions = f"""# Questions
owner: {context}/{member}
next-question-number: 1

## Open
<!-- ID | original question/options | origin room/seq | route | state | answer ref -->

## Resolved index
<!-- ID | answer room/seq or authoritative decision link -->
"""
    base.mkdir(parents=True, exist_ok=True)
    destination.mkdir()  # Exclusive claim. Never replace another member's records.
    for name, body in (
        ("identity.json", json.dumps(identity, indent=2) + "\n"),
        ("state.md", state), ("brief.md", brief), ("questions.md", questions),
    ):
        with (destination / name).open("x", encoding="utf-8") as stream:
            stream.write(body)
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--context", required=True, type=token)
    parser.add_argument("--member", required=True, type=token)
    parser.add_argument("--role", required=True, type=canonical_role)
    parser.add_argument("--theme", default="default", choices=("default", "warhammer"))
    parser.add_argument("--domain", type=token)
    args = parser.parse_args()
    try:
        destination = bootstrap(args.root, args.context, args.member, args.role, args.theme, args.domain)
        print(destination)
        try:
            in_git = subprocess.run(
                ["git", "-C", str(args.root), "rev-parse", "--show-toplevel"],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5,
            ).returncode == 0
            ignored = not in_git or subprocess.run(
                ["git", "-C", str(args.root), "check-ignore", "-q", str(destination / "state.md")],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5,
            ).returncode == 0
            if not in_git:
                print("NOTE: project root is not a Git worktree; ignore check skipped.", file=sys.stderr)
        except (OSError, subprocess.TimeoutExpired):
            ignored = False
        if not ignored:
            print("WARNING: coordination records are not confirmed Git-ignored. "
                  "Keep .cbc/hierarchy out of commits; no ignore file was changed.",
                  file=sys.stderr)
    except (OSError, ValueError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
