# Primarch instructions after compaction

The installed `scripts/primarch_compaction.py` is a short-lived command, not a watcher.
A `SessionStart` hook matching `compact` reads current installed Repo Lead instructions
and returns them as model context before continuation. It also points to the shared protocol.
No network, transcript scan, model call, timer, permission decision or background process.

Enroll a Primarch once when invoking its role, including existing chats after a skill reload:

```bash
rtk proxy python3 <cbc-hierarchy-dir>/scripts/primarch_compaction.py register --host codex
```

Codex supplies its native ID in the command environment. With Claude use
`--host claude --session-id <actual-native-session-id>` from the supported session context.
Never guess an ID from a title or enroll a Chapter Master. The registry under the host's
`cbc-hierarchy/primarch-sessions/` stores only native IDs and role, not work or authority.
New/replacement sessions enroll separately; a fork does not inherit enrollment.
A project-local `wip-` trial copy refuses enrollment, because the hook reloads the globally
installed role; after compaction a trial Primarch rereads its own role and this protocol.

The one-time hook configuration is installed separately with
`tools/install_primarch_compaction.py --host <codex|claude> --backup-root <new-private-directory>`
from the CBC hierarchy source checkout with Python 3.11+ for Codex TOML validation. The hook
itself needs only Python 3.9+. The installer preserves existing hooks/settings and permissions.
Installed skill files alone do not activate a host hook. Codex requires native review/trust
of the new exact hook definition through `/hooks`; existing sessions may need a supported
configuration reload or resume before they discover it. Do not bypass trust or restart active
work automatically. Claude can also restrict hooks through managed policy.

Only an enrolled session's compact event produces context. Other sessions/events are silent.
The full role is bounded below host context-spill limits. If it grows beyond the bound, the
hook explicitly instructs a full file read instead of silently truncating rules. The skill's
current instructions are reloaded; assignments, rooms, holds and child work stay unchanged.

Sources: [Codex hooks](https://learn.chatgpt.com/docs/hooks),
[Claude Code hooks](https://code.claude.com/docs/en/hooks).
