#!/usr/bin/env python3
"""Install or recoverably update only the reviewed CBC hierarchy package."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import tempfile


ALIASES = {
    "cbc-emperor": "cbc-project-lead",
    "cbc-primarch": "cbc-repo-lead",
    "cbc-chapter-master": "cbc-feature-lead",
    "cbc-captain": "cbc-feature-lead",
    "cbc-techmarine": "cbc-task-agent",
    "emperor": "cbc-project-lead",
}
NAMES = (
    "cbc-hierarchy", "cbc-project-lead", "cbc-repo-lead",
    "cbc-domain-lead", "cbc-feature-lead", "cbc-task-agent", *ALIASES, "cbc-handoff",
)
SOURCE = Path(__file__).resolve().parents[1] / "bins" / "cbc" / "skill"


def manifest(directory):
    if directory.is_symlink():
        raise ValueError(f"refusing symlink: {directory}")
    result = {}
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"refusing symlink: {path}")
        if "__pycache__" in path.parts or path.suffix == ".pyc":
            continue
        if path.is_file():
            result[str(path.relative_to(directory))] = path.read_bytes()
    return result


def install(targets, source=SOURCE, check=False, previous=None, backup=None):
    """Update only exact reviewed baseline directories; preflight all hosts before writes."""
    expected = {}
    for name in NAMES:
        files = manifest(source / name)
        if "SKILL.md" not in files or "agents/openai.yaml" not in files:
            raise ValueError(f"incomplete skill: {source / name}")
        expected[name] = files
    plan, updates = [], []
    targets = list(dict.fromkeys(Path(target).expanduser().absolute() for target in targets))
    for target in targets:
        target = Path(target).expanduser().absolute()
        if target.is_symlink() or (target.exists() and not target.is_dir()):
            raise ValueError(f"target must be a real skills directory: {target}")
        for name in NAMES:
            destination = target / name
            if destination.is_symlink():
                raise ValueError(f"refusing symlink: {destination}")
            if destination.exists():
                current = manifest(destination) if destination.is_dir() else None
                if current != expected[name]:
                    hashes = {path: hashlib.sha256(data).hexdigest() for path, data in (current or {}).items()}
                    if current is None or not previous or hashes != previous.get(name):
                        raise ValueError(f"existing skill differs; nothing overwritten: {destination}")
                    if not set(current) <= set(expected[name]):
                        raise ValueError("update would remove files; explicit migration required")
                    updates.append((target, name))
            elif (target, name) not in plan:
                plan.append((target, name))
    if updates:
        if backup is None:
            raise ValueError("a new absolute backup root is required for updates")
        backup = Path(backup).expanduser()
        if not backup.is_absolute() or backup.exists() or backup.is_symlink():
            raise ValueError("backup root must be new and absolute")
        resolved = backup.resolve()
        for protected in [source.resolve(), *(target.resolve() for target in targets)]:
            if resolved == protected or resolved in protected.parents or protected in resolved.parents:
                raise ValueError("backup root overlaps source or an installed skills directory")
    if not check:
        if updates:
            backup.mkdir(parents=True, mode=0o700)
            for target, name in updates:
                saved = backup / str(targets.index(target)) / name
                saved.parent.mkdir(exist_ok=True)
                shutil.copytree(target / name, saved,
                                ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
            (backup / "targets.json").write_text(json.dumps([str(target) for target in targets], indent=2) + "\n")
        # Stage outside discovery on each target's filesystem. Ordinary failures roll back
        # prior directory swaps; this is not crash-atomic across hosts.
        stages, applied, retain_stages = {}, [], False
        try:
            for target, name in [*updates, *plan]:
                target.parent.mkdir(parents=True, exist_ok=True)
                if target not in stages:
                    stages[target] = Path(tempfile.mkdtemp(prefix=".cbc-skill-stage-", dir=target.parent))
                shutil.copytree(source / name, stages[target] / name,
                                ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
            for target, name in [*updates, *plan]:
                target.mkdir(parents=True, exist_ok=True)
                destination = target / name
                previous_path = stages[target] / (name + ".previous") if destination.exists() else None
                if previous_path:
                    os.replace(destination, previous_path)
                applied.append((destination, previous_path))
                os.replace(stages[target] / name, destination)
        except OSError as error:
            rollback_errors = []
            for destination, previous_path in reversed(applied):
                try:
                    if destination.exists():
                        shutil.rmtree(destination)
                    if previous_path:
                        os.replace(previous_path, destination)
                except OSError as rollback_error:
                    rollback_errors.append(str(rollback_error))
            if rollback_errors:
                retain_stages = True
                raise OSError(f"update failed and rollback is incomplete; retain backups and stages {list(stages.values())}: {rollback_errors}") from error
            raise
        finally:
            if not retain_stages:
                for stage in stages.values():
                    shutil.rmtree(stage)
    return [str(target / name) for target, name in [*updates, *plan]]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", action="append", required=True, type=Path,
                        help="skills directory; repeat for Claude and Codex")
    parser.add_argument("--check", action="store_true", help="preflight without writes")
    parser.add_argument("--update-v1", action="store_true", help="allow the bundled exact v1 baseline")
    parser.add_argument("--previous-manifest", type=Path, help="reviewed exact hashes of installed package before this update")
    parser.add_argument("--backup-root", type=Path, help="new recovery directory required for updates")
    args = parser.parse_args()
    try:
        if args.update_v1 and args.previous_manifest:
            raise ValueError("choose one reviewed baseline")
        baseline = args.previous_manifest or (Path(__file__).parent / "hierarchy-v1-sha256.json" if args.update_v1 else None)
        previous = json.loads(baseline.read_text()) if baseline else None
        changes = install(args.target, check=args.check, previous=previous, backup=args.backup_root)
    except (OSError, ValueError) as error:
        parser.exit(1, f"{error}\n")
    print(f"{'Would add/update' if args.check and previous else 'Would add' if args.check else 'Added/updated'} {len(changes)} skill directories. "
          "Matching skills unchanged; differing unknown skills refused.")
    for path in changes:
        print(path)


if __name__ == "__main__":
    main()
