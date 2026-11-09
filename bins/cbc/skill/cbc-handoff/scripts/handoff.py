#!/usr/bin/env python3
"""Export and verify cooperative CBC handoffs. No host, network or process control."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path


SNAPSHOTS = ("identity.json", "state.md", "brief.md", "questions.md")
LEADS = ("cbc-project-lead", "cbc-repo-lead", "cbc-domain-lead", "cbc-feature-lead")


def now():
    return datetime.now(timezone.utc).isoformat(timespec="microseconds").replace("+00:00", "Z")


def exclusive_json(path, value):
    with path.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2, sort_keys=True)
        stream.write("\n")


def require_id(value):
    if not value or value in ("unknown", "unset") or "\n" in value:
        raise ValueError("an actual native session ID is required")


def read_safe(path):
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"missing or symlinked handoff file: {path}")
    return path.read_bytes()


def declared_rooms(content):
    rooms, inside = set(), False
    for line in content.splitlines():
        if line.strip() == "connections:":
            inside = True
        elif inside and line.strip():
            if not line.startswith((" ", "\t")):
                break
            if ":" not in line:
                raise ValueError("malformed connections block")
            tokens = line.split(":", 1)[1].split()
            if not tokens:
                raise ValueError("empty room in connections block")
            rooms.add(tokens[0])
    return rooms


def validate_handled(handled, rooms, minimum=None):
    if not isinstance(handled, dict) or set(handled) != set(rooms):
        raise ValueError("handled sequences must cover exactly the declared rooms")
    if any(type(seq) is not int or seq < 0 for seq in handled.values()):
        raise ValueError("handled sequences must be nonnegative integers")
    if minimum and any(handled[room] < minimum[room] for room in minimum):
        raise ValueError("release checkpoints cannot move backwards")


def prepare(state_dir, predecessor, title, handled):
    require_id(predecessor)
    if not title or not isinstance(handled, dict):
        raise ValueError("current title and handled-sequence object are required")
    directory = Path(state_dir)
    if directory.is_symlink():
        raise ValueError("state directory must not be a symlink")
    files = {name: read_safe(directory / name) for name in SNAPSHOTS}
    identity = json.loads(files["identity.json"])
    if identity.get("role") not in LEADS:
        raise ValueError("only hierarchy lead roles can use this handoff")
    for key in ("context", "member", "label"):
        if not identity.get(key):
            raise ValueError(f"missing identity field: {key}")
    validate_handled(handled, declared_rooms(files["state.md"].decode("utf-8")))
    created = now()
    stamp = datetime.fromisoformat(created.replace("Z", "+00:00")).strftime("%Y%m%dT%H%M%S%fZ")
    base = directory / "handoffs"
    if base.is_symlink():
        raise ValueError("handoffs directory must not be a symlink")
    base.mkdir(exist_ok=True)
    destination = base / stamp
    destination.mkdir()
    checkpoints = "\n".join(f"- `{room}`: last handled `{seq}`" for room, seq in sorted(handled.items()))
    document = f"""# CBC role handoff

Prepared at: {created}
Logical member: `{identity['context']}/{identity['member']}`
Canonical role: `{identity['role']}`
CBC label: `{identity['label']}`
Predecessor session: `{predecessor}`
Predecessor title: `{title}`

## Current responsibility and next action

Read [working state](state.md). It owns the assignment, source links, children, native handles,
holds and pending outbound events. Verify external facts before continuing.

## Outward brief

Read [the current brief](brief.md), preserving its revision and reporting obligations.

## Questions and decisions

Read [the question index](questions.md). Keep original IDs, answer routes and source references.

## Room checkpoints

{checkpoints or 'No receiving rooms declared.'}

These are PREPARE checkpoints. After release, use released.json and the final release snapshots
instead. Process messages after those final checkpoints even if an old poll consumed them.

## Transfer conditions

This snapshot is immutable. Verify manifest hashes. The predecessor must settle native
subagents and release its receivers before the named successor acquires them. Only a matching
acceptance receipt completes transfer. Rooms stay open; logical identity does not change.
No server-enforced lock, process control or native rename is provided by this file.
"""
    files["handoff.md"] = document.encode()
    for name, content in files.items():
        with (destination / name).open("xb") as stream:
            stream.write(content)
    manifest = {
        "schema": 1, "protocol": "cooperative-release-before-acquire",
        "prepared_at": created, "context": identity["context"], "member": identity["member"],
        "role": identity["role"], "label": identity["label"],
        "predecessor_session": predecessor, "predecessor_title": title,
        "last_handled": handled,
        "sha256": {name: hashlib.sha256(content).hexdigest() for name, content in files.items()},
    }
    exclusive_json(destination / "manifest.json", manifest)
    return destination


def verify(directory):
    directory = Path(directory)
    if directory.is_symlink():
        raise ValueError("handoff directory must not be a symlink")
    manifest = json.loads(read_safe(directory / "manifest.json"))
    if manifest.get("schema") != 1 or manifest.get("protocol") != "cooperative-release-before-acquire":
        raise ValueError("unsupported handoff manifest")
    if set(manifest.get("sha256", {})) != {*SNAPSHOTS, "handoff.md"}:
        raise ValueError("invalid snapshot file set")
    for name, digest in manifest["sha256"].items():
        if hashlib.sha256(read_safe(directory / name)).hexdigest() != digest:
            raise ValueError(f"snapshot changed: {name}")
    state = "prepared"
    release_path = directory / "released.json"
    if release_path.exists() or release_path.is_symlink():
        marker = json.loads(read_safe(release_path))
        if marker.get("from_session") != manifest["predecessor_session"]:
            raise ValueError("release has wrong predecessor")
        validate_handled(marker.get("last_handled"), manifest["last_handled"], manifest["last_handled"])
        expected = {"release-state.md", "release-brief.md", "release-questions.md"}
        if set(marker.get("sha256", {})) != expected:
            raise ValueError("invalid final release snapshot set")
        for name, digest in marker["sha256"].items():
            if hashlib.sha256(read_safe(directory / name)).hexdigest() != digest:
                raise ValueError(f"release snapshot changed: {name}")
        state = "released"
    outcome_path = directory / "outcome.json"
    if outcome_path.exists() or outcome_path.is_symlink():
        outcome = json.loads(read_safe(outcome_path))
        if outcome.get("status") not in ("accepted", "revoked"):
            raise ValueError("invalid transfer outcome")
        if outcome["status"] == "accepted":
            if state != "released" or outcome.get("session") != marker.get("to_session"):
                raise ValueError("accepted outcome has wrong successor or no release")
        elif outcome.get("session") != manifest["predecessor_session"]:
            raise ValueError("revocation has wrong predecessor")
        state = "accepted-awaiting-activation" if outcome["status"] == "accepted" else "revoked"
    activation_path = directory / "activated.json"
    if activation_path.exists() or activation_path.is_symlink():
        activation = json.loads(read_safe(activation_path))
        if state != "accepted-awaiting-activation" or activation.get("session") != outcome["session"]:
            raise ValueError("activation has no matching accepted owner")
        state = "activated"
    superseded_path = directory / "superseded.json"
    if superseded_path.exists() or superseded_path.is_symlink():
        superseded = json.loads(read_safe(superseded_path))
        if state != "activated" or superseded.get("session") != manifest["predecessor_session"]:
            raise ValueError("superseded marker has no activated successor")
        state = "superseded"
    manifest["transfer_state"] = state
    return manifest


def release(directory, predecessor, successor, evidence, handled, outbound=None, state_dir=None):
    require_id(predecessor)
    require_id(successor)
    manifest = verify(directory)
    if manifest["transfer_state"] != "prepared":
        raise ValueError("handoff already released or resolved")
    if predecessor != manifest["predecessor_session"] or predecessor == successor:
        raise ValueError("release must come from the predecessor to a distinct successor")
    if not evidence.strip():
        raise ValueError("verified receiver/native-child evidence is required")
    validate_handled(handled, manifest["last_handled"], manifest["last_handled"])
    outbound = [] if outbound is None else outbound
    if not isinstance(outbound, list) or any(
        not isinstance(item, dict) or item.get("room") not in handled
        or type(item.get("seq")) is not int or item["seq"] < 0
        or item.get("status") not in ("sent", "uncertain") for item in outbound
    ):
        raise ValueError("late outbound references need room, seq and sent/uncertain status")
    directory = Path(directory)
    current = Path(state_dir) if state_dir else directory.parent.parent
    if read_safe(current / "identity.json") != read_safe(directory / "identity.json"):
        raise ValueError("final state belongs to a different logical identity")
    final = {"release-" + name: read_safe(current / name)
             for name in ("state.md", "brief.md", "questions.md")}
    validate_handled(handled, declared_rooms(final["release-state.md"].decode()), manifest["last_handled"])
    for name, content in final.items():
        with (directory / name).open("xb") as stream:
            stream.write(content)
    marker = {"released_at": now(), "from_session": predecessor,
              "to_session": successor, "evidence": evidence, "last_handled": handled,
              "late_outbound": outbound,
              "sha256": {name: hashlib.sha256(content).hexdigest() for name, content in final.items()}}
    exclusive_json(directory / "released.json", marker)
    return marker


def accept(directory, session, evidence):
    require_id(session)
    manifest = verify(directory)
    marker = json.loads(read_safe(Path(directory) / "released.json"))
    if marker.get("from_session") != manifest["predecessor_session"] or marker.get("to_session") != session:
        raise ValueError("release names a different predecessor or successor")
    if not evidence.strip():
        raise ValueError("catch-up and receiver verification evidence is required")
    receipt = {"status": "accepted", "at": now(), "predecessor_session": marker["from_session"],
               "session": session, "evidence": evidence}
    exclusive_json(Path(directory) / "outcome.json", receipt)
    return receipt


def revoke(directory, session, evidence):
    require_id(session)
    manifest = verify(directory)
    if session != manifest["predecessor_session"] or not evidence.strip():
        raise ValueError("revocation requires predecessor ID and explicit human-authority reference")
    outcome = {"status": "revoked", "session": session, "at": now(), "evidence": evidence}
    exclusive_json(Path(directory) / "outcome.json", outcome)
    return outcome


def activate(directory, session, evidence):
    require_id(session)
    verify(directory)
    outcome = json.loads(read_safe(Path(directory) / "outcome.json"))
    if outcome.get("status") != "accepted" or outcome.get("session") != session or not evidence.strip():
        raise ValueError("activation requires accepted successor and verified receiving evidence")
    receipt = {"session": session, "at": now(), "evidence": evidence}
    exclusive_json(Path(directory) / "activated.json", receipt)
    return receipt


def supersede(directory, session):
    require_id(session)
    manifest = verify(directory)
    if manifest["transfer_state"] != "activated" or manifest["predecessor_session"] != session:
        raise ValueError("only predecessor can supersede after successor activation")
    outcome = json.loads(read_safe(Path(directory) / "outcome.json"))
    marker = {"session": session, "successor_session": outcome["session"], "superseded_at": outcome["at"]}
    exclusive_json(Path(directory) / "superseded.json", marker)
    return marker


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    prep = sub.add_parser("prepare")
    prep.add_argument("--state-dir", required=True, type=Path)
    prep.add_argument("--from-session", required=True)
    prep.add_argument("--title", required=True)
    prep.add_argument("--handled", required=True, type=json.loads)
    for action in ("verify", "release", "accept", "revoke", "activate", "supersede"):
        command = sub.add_parser(action)
        command.add_argument("--handoff", required=True, type=Path)
        if action == "release":
            command.add_argument("--from-session", required=True)
            command.add_argument("--to-session", required=True)
            command.add_argument("--handled", required=True, type=json.loads)
            command.add_argument("--outbound", default=[], type=json.loads)
            command.add_argument("--state-dir", type=Path)
        elif action != "verify":
            command.add_argument("--session", required=True)
        if action not in ("verify", "supersede"):
            command.add_argument("--evidence", required=True)
    args = parser.parse_args()
    try:
        if args.action == "prepare":
            print(prepare(args.state_dir, args.from_session, args.title, args.handled))
        elif args.action == "verify":
            print(json.dumps(verify(args.handoff), sort_keys=True))
        elif args.action == "release":
            print(json.dumps(release(args.handoff, args.from_session, args.to_session, args.evidence,
                                     args.handled, args.outbound, args.state_dir)))
        elif args.action == "accept":
            print(json.dumps(accept(args.handoff, args.session, args.evidence)))
        elif args.action == "revoke":
            print(json.dumps(revoke(args.handoff, args.session, args.evidence)))
        elif args.action == "activate":
            print(json.dumps(activate(args.handoff, args.session, args.evidence)))
        else:
            print(json.dumps(supersede(args.handoff, args.session)))
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
