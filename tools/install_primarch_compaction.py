#!/usr/bin/env python3
"""Add only the enrolled-Primarch compact hook, retaining native trust and settings."""

import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import sys
import tempfile
try:
    import tomllib
except ImportError:
    tomllib = None


def configuration(root, host, python=sys.executable):
    if host == "codex" and tomllib is None:
        raise ValueError("Codex hook installation requires Python 3.11+ for TOML validation")
    script = root / "skills/cbc-hierarchy/scripts/primarch_compaction.py"
    if not script.is_file() or script.is_symlink():
        raise ValueError("install the current CBC hierarchy package first")
    command = shlex.join(["rtk", "proxy", python, str(script), "run", "--host", host])
    handler = {"type": "command", "command": command, "timeout": 5}
    if host == "codex":
        handler["additionalContextLimit"] = 4000
    group = {"matcher": "^compact$", "hooks": [handler]}
    path = root / ("config.toml" if host == "codex" else "settings.json")
    if root.is_symlink() or path.is_symlink():
        raise ValueError("refusing symlinked host configuration")
    original = path.read_bytes() if path.exists() else None
    text = (original or b"").decode()
    data = tomllib.loads(text) if host == "codex" else json.loads(text or "{}")
    hooks = data.setdefault("hooks", {})
    groups = hooks.setdefault("SessionStart", [])
    if not isinstance(groups, list):
        raise ValueError("unexpected SessionStart shape")
    for existing in groups:
        if any("primarch_compaction.py" in h.get("command", "") for h in existing.get("hooks", [])):
            if existing != group:
                raise ValueError("existing CBC compact hook differs; review before updating")
            return path, original, original
    groups.append(group)
    if host == "codex":
        updated = text.rstrip() + "\n\n# CBC Primarch skill reload; native hook trust is required.\n"
        updated += '[[hooks.SessionStart]]\nmatcher = "^compact$"\n\n[[hooks.SessionStart.hooks]]\n'
        for key, value in handler.items():
            updated += f"{key} = {json.dumps(value)}\n"
        if tomllib.loads(updated) != data:
            raise ValueError("configuration merge did not preserve existing values")
    else:
        updated = json.dumps(data, indent=2, ensure_ascii=False) + "\n"
    return path, original, updated.encode()


def install(root, host, backup):
    path, original, updated = configuration(root, host)
    if original == updated:
        return False
    backup = Path(backup)
    if not backup.is_absolute() or backup.exists() or backup.is_symlink():
        raise ValueError("backup root must be new and absolute")
    if root.resolve() in backup.resolve().parents or backup.resolve() in root.resolve().parents:
        raise ValueError("backup must not overlap host configuration")
    backup.mkdir(mode=0o700, parents=True)
    if original is not None:
        shutil.copy2(path, backup / path.name)
        os.chmod(backup / path.name, 0o600)
    else:
        (backup / "previously-absent").write_text(path.name + "\n")
    # Refuse concurrent settings edits instead of replacing them with an older snapshot.
    if (path.read_bytes() if path.exists() else None) != original:
        raise ValueError("host configuration changed during install")
    root.mkdir(exist_ok=True, parents=True)
    with tempfile.NamedTemporaryFile(dir=root, delete=False) as stream:
        temporary = Path(stream.name)
        stream.write(updated)
        stream.flush()
        os.fsync(stream.fileno())
    try:
        if original is not None:
            os.chmod(temporary, path.stat().st_mode & 0o777)
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", required=True, choices=("codex", "claude"))
    parser.add_argument("--backup-root", required=True, type=Path)
    args = parser.parse_args()
    variable = "CODEX_HOME" if args.host == "codex" else "CLAUDE_CONFIG_DIR"
    root = Path(os.environ.get(variable) or Path.home() / f".{args.host}").expanduser().absolute()
    try:
        changed = install(root, args.host, args.backup_root)
    except (OSError, ValueError) as error:
        parser.exit(1, f"{error}\n")
    print("Installed compact hook." if changed else "Compact hook already matches; unchanged.")
    if args.host == "codex":
        print("Native /hooks review/trust still required. No trust or approval settings changed.")


if __name__ == "__main__":
    main()
