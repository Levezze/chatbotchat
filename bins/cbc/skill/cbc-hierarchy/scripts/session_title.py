#!/usr/bin/env python3
"""Compute a CBC native-session title. Prints only; never renames a host session."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import unicodedata


ROLE_NAMES = {
    "cbc-project-lead": ("project-lead", "emperor"),
    "cbc-repo-lead": ("repo-lead", "primarch"),
    "cbc-domain-lead": ("domain-lead", "retired-domain-lead"),
    "cbc-feature-lead": ("feature-lead", "chapter-master"),
    "cbc-task-agent": ("task-agent", "techmarine"),
}


def shorten(value, limit):
    if len(value) <= limit:
        return value
    digest = hashlib.sha256(value.encode()).hexdigest()[:8]
    return value[:limit - 9].rstrip("-") + "-" + digest


def slug(value, field, limit=48):
    if not value or not isinstance(value, str):
        raise ValueError(f"{field} is required")
    folded = unicodedata.normalize("NFKD", value.casefold()).encode("ascii", "ignore").decode()
    result = re.sub(r"[^a-z0-9]+", "-", folded).strip("-")
    if not result:
        raise ValueError(f"{field} needs an ASCII name or explicit slug")
    return shorten(result, limit)


def resolve_role(role, *, identity=None, theme=None):
    if role:
        role = role.removeprefix("wip-")  # Project-local trial skill names (trial decision T-17).
    aliases = {"cbc-" + names[1]: key for key, names in ROLE_NAMES.items()}
    aliases["emperor"] = "cbc-project-lead"
    aliases["cbc-captain"] = "cbc-feature-lead"  # Compatibility, not another tier.
    themed = role in aliases
    canonical = aliases.get(role, role)
    if canonical not in ROLE_NAMES:
        raise ValueError("unknown CBC role")
    if identity is not None:
        if identity.get("schema") != 1 or identity.get("role") not in ROLE_NAMES or identity.get("theme") not in ("default", "warhammer"):
            raise ValueError("invalid recorded role identity")
        return identity["role"], identity["theme"]
    theme = theme if theme is not None else ("warhammer" if themed else "default")
    if theme not in ("default", "warhammer"):
        raise ValueError("unknown theme")
    return canonical, theme


def build_title(role, *, theme=None, project=None, repo=None, domain=None, feature=None, task=None, suffix=None):
    canonical, theme = resolve_role(role, theme=theme)
    display_role = ROLE_NAMES[canonical][theme == "warhammer"]
    scope = slug(project, "project") if canonical == "cbc-project-lead" else slug(repo, "repo")
    parts = [scope, display_role]
    if canonical == "cbc-domain-lead":
        parts.append(slug(domain, "domain"))
    if canonical in ("cbc-feature-lead", "cbc-task-agent"):
        parts.append(slug(feature, "feature"))
    if canonical == "cbc-task-agent":
        parts.append(slug(task, "task", 32))
    if suffix is not None:
        parts.append(slug(suffix, "suffix", 24))
    return shorten("-".join(parts), 200)


def read_identity(path, context, member):
    if not context or not member:
        raise ValueError("known context and member are required; never scan for an identity")
    if path.is_symlink():
        raise ValueError("refusing symlinked identity")
    identity = json.loads(path.read_text())
    if (identity.get("context"), identity.get("member"), identity.get("label")) != (context, member, f"{context}--{member}"):
        raise ValueError("identity does not match the held context/member")
    return identity


def resolve_entry(role, *, identity=None, theme=None):
    invoked, _ = resolve_role(role, theme=theme)
    recorded, effective_theme = resolve_role(role, identity=identity, theme=theme)
    return {"role": recorded, "theme": effective_theme, "invoked": invoked,
            "mismatch": invoked != recorded}


def superseded_title(title, at):
    """Use the recorded transfer time, not a new clock reading on every retry."""
    if not title or not at:
        raise ValueError("title and recorded transfer time are required")
    parsed = datetime.fromisoformat(at.replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        raise ValueError("transfer time must include its timezone")
    stamp = parsed.astimezone(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
    tail = "-superseded-" + stamp
    return shorten(slug(title, "title", 200), 200 - len(tail)) + tail


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--role")
    parser.add_argument("--identity", type=Path, help="own recorded identity, used only to resolve role/theme")
    parser.add_argument("--resolve-only", action="store_true")
    parser.add_argument("--context", help="known context when resolving recorded identity")
    parser.add_argument("--member", help="known member when resolving recorded identity")
    parser.add_argument("--theme", choices=("default", "warhammer"))
    for field in ("project", "repo", "domain", "feature", "task", "suffix"):
        parser.add_argument("--" + field)
    parser.add_argument("--supersede-title")
    parser.add_argument("--at", help="recorded ISO timestamp, including timezone")
    args = parser.parse_args()
    try:
        if args.resolve_only:
            identity = read_identity(args.identity, args.context, args.member) if args.identity else None
            print(json.dumps(resolve_entry(args.role, identity=identity, theme=args.theme), sort_keys=True))
        elif args.identity:
            raise ValueError("use --resolve-only with recorded identity; preserve the stored title")
        elif args.supersede_title:
            print(superseded_title(args.supersede_title, args.at))
        else:
            print(build_title(args.role, theme=args.theme, project=args.project, repo=args.repo, domain=args.domain,
                              feature=args.feature, task=args.task, suffix=args.suffix))
    except (ValueError, TypeError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
