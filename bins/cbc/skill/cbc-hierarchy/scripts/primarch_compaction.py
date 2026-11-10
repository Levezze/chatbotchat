#!/usr/bin/env python3
"""Reload current Primarch instructions on compact, only for enrolled native sessions."""

import argparse
import json
import os
from pathlib import Path
import re
import sys
import tempfile


MAX_INPUT = 131072
MAX_CONTEXT = 9500  # Below Claude's 10,000-character context spill threshold.


def host_root(host):
    variable = "CODEX_HOME" if host == "codex" else "CLAUDE_CONFIG_DIR"
    return Path(os.environ.get(variable) or Path.home() / f".{host}").expanduser().absolute()


def session_token(value):
    if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,127}", value):
        raise ValueError("a real native session ID is required")
    return value


def registry_path(root, session):
    path = root / "cbc-hierarchy" / "primarch-sessions" / (session_token(session) + ".json")
    for item in (root, path.parent.parent, path.parent, path):
        if item.is_symlink():
            raise ValueError("refusing a symlinked session registry")
    return path


def register(root, session):
    path = registry_path(root, session)
    record = {"schema": 1, "role": "cbc-repo-lead", "session_id": session}
    if path.exists():
        if json.loads(path.read_text()) != record:
            raise ValueError("existing session registration differs")
        return path
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with tempfile.NamedTemporaryFile(mode="w", dir=path.parent, delete=False) as stream:
        temporary = Path(stream.name)
        json.dump(record, stream)
        stream.write("\n")
    try:
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)
    return path


def reload_context(root, event):
    if not isinstance(event, dict) or event.get("hook_event_name") != "SessionStart":
        return None
    if event.get("source") != "compact" or event.get("agent_id"):
        return None
    session = event.get("session_id")
    try:
        path = registry_path(root, session)
    except ValueError:
        return None
    if not path.exists():
        return None
    expected = {"schema": 1, "role": "cbc-repo-lead", "session_id": session}
    if json.loads(path.read_text()) != expected:
        raise ValueError("session registration does not match")
    role = root / "skills" / "cbc-repo-lead" / "SKILL.md"
    shared = root / "skills" / "cbc-hierarchy" / "SKILL.md"
    prefix = ("CBC Primarch instructions reloaded after compaction from the installed skill. "
              "Apply them now and read the shared protocol at " + str(shared) + ". "
              "Resume the existing board and active assignments without an acknowledgement, "
              "handoff or pause of children.\n\n")
    try:
        with role.open(encoding="utf-8") as stream:
            text = stream.read(MAX_CONTEXT + 1)
    except OSError:
        text = ""
    if text.startswith("---\n"):
        text = text.split("\n---\n", 1)[-1]
    context = prefix + text
    if not text or len(context) > MAX_CONTEXT:
        # Never silently truncate a rule. Give an explicit file read when the skill grows.
        context = prefix + "Read the full current role skill before continuing: " + str(role)
    return {"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": context}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("register", "run"))
    parser.add_argument("--host", required=True, choices=("codex", "claude"))
    parser.add_argument("--session-id", help="native ID; defaults to Codex's native environment")
    args = parser.parse_args()
    root = host_root(args.host)
    try:
        if args.action == "register":
            if Path(__file__).resolve().parents[1].name.startswith("wip-"):
                # The hook reloads the globally installed role, never this project copy (trial decision T-17).
                raise ValueError("trial copy: do not enroll the compaction hook; it would reload the global "
                                 "role. After compaction, reread wip-cbc-repo-lead and wip-cbc-hierarchy yourself.")
            session = args.session_id
            if not session and args.host == "codex":
                session = os.environ.get("CODEX_THREAD_ID") or os.environ.get("CODEX_SESSION_ID")
            print(register(root, session_token(session)))
        else:
            raw = sys.stdin.read(MAX_INPUT + 1)
            if len(raw) > MAX_INPUT:
                raise ValueError("hook input exceeds bound")
            result = reload_context(root, json.loads(raw))
            if result is not None:
                print(json.dumps(result))
    except (OSError, ValueError) as error:
        if args.action == "register":
            parser.exit(1, f"{error}\n")
        # Advisory only: never manufacture a permission decision or stop useful work.
        print(json.dumps({"systemMessage": "CBC Primarch reload hook failed: " + str(error)}))


if __name__ == "__main__":
    main()
