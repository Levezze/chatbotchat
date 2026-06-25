# Terminal Titles — CBC Agent Self-Titling

CBC workers and orchestrators stamp their terminal tab with their agent name so the user can
identify tabs at a glance — without manual renaming that resets on every Cursor reload.

**Agent names follow the repo-first, role-in-name scheme:**
- Orchestrator: `<repo>-orchestrator` — e.g. `engine-orchestrator`
- Worker: `<repo>-worker-<feature>` — e.g. `engine-worker-recompute`, `api-worker-fix-contract`

---

## How it works

**The core mechanism is a tty-keyed name-file** (not a direct `printf` to stdout). Claude Code
agents have no controlling tty — `tty` returns "not a tty" from the Bash tool, and `$TTY` is
unset. A direct `printf '\e]0;NAME\a'` from the tool's stdout never reaches the terminal. But
the parent interactive shell's controlling terminal device _is_ reachable:

```bash
ps -o tty= -p $PPID   # → e.g. ttys066
```

That device name matches the interactive shell's `${TTY##*/}`. So the agent writes a file
keyed by that device, and the shell's `precmd` hook reads it every prompt and applies the OSC
title escape:

```bash
# Agent side (worker / orchestrator — done once on session start and again on resume):
mkdir -p /tmp/cbc-termtitle
t=$(basename "$(ps -o tty= -p $PPID | tr -d ' ')")
[ -n "$t" ] && [ "$t" != "??" ] && printf '%s' "<agent-name>" > "/tmp/cbc-termtitle/$t"
```

```zsh
# Shell side (precmd hook — applied every prompt):
_cbc_termtitle() {
  local f="/tmp/cbc-termtitle/${TTY##*/}"
  [[ -r "$f" ]] && print -Pn "\e]0;$(<"$f")\a"
}
autoload -Uz add-zsh-hook && add-zsh-hook precmd _cbc_termtitle
```

When no name-file exists for the current tty, the hook is a no-op — the default title stands.

---

## Setup by environment

### Terminal emulator

#### Cursor / VS Code
OSC title sequences are NOT shown in tab titles by default — the tab uses the `${process}`
template (shows "zsh" or the running command name).

**Required setting** (User Settings JSON):

```json
"terminal.integrated.tabs.title": "${sequence}"
```

This tells Cursor to use the title set via OSC escape (`\e]0;NAME\a`) for the tab label.
When no `${sequence}` is set (no name-file for that tty), the tab shows nothing or falls back
to an empty string. To fall back to the process name instead:

```json
"terminal.integrated.tabs.title": "${sequence}${separator}${process}"
```

#### iTerm2 / macOS Terminal.app / Alacritty / Ghostty
OSC title sequences are honored natively — no emulator setting required. Install only the
shell hook (see below).

#### Other emulators
Most modern terminal emulators support OSC 0/2 title sequences. If the tab doesn't change,
check the emulator's "Allow title changes" or "Terminal title" setting.

---

### Shell hook

The hook must be installed **after** any framework (oh-my-zsh, p10k, starship) that also sets
the terminal title — otherwise the framework's own `precmd` overwrites the CBC name on every
prompt.

#### zsh (oh-my-zsh + powerlevel10k)

Append at the **very end** of `~/.zshrc`, after the `source $ZSH/oh-my-zsh.sh` and
`source ~/.p10k.zsh` lines:

```zsh
# CBC terminal self-titling — must be last in .zshrc to win the precmd ordering race.
_cbc_termtitle() {
  local f="/tmp/cbc-termtitle/${TTY##*/}"
  [[ -r "$f" ]] && print -Pn "\e]0;$(<"$f")\a"
}
autoload -Uz add-zsh-hook && add-zsh-hook precmd _cbc_termtitle
```

#### zsh (plain / no framework)

Same hook; position doesn't matter as much without a competing framework.

#### bash

```bash
# In ~/.bashrc, after any PS1/prompt configuration:
_cbc_termtitle() {
  local f="/tmp/cbc-termtitle/$(basename $(tty 2>/dev/null || ps -o tty= -p $$ | tr -d ' '))"
  [[ -r "$f" ]] && printf '\e]0;%s\a' "$(<"$f")"
}
PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND; }_cbc_termtitle"
```

#### fish

```fish
# In ~/.config/fish/functions/fish_title.fish:
function fish_title
  set f /tmp/cbc-termtitle/(basename (tty 2>/dev/null; or echo "??"))
  if test -r $f
    cat $f
  else
    echo $argv[1]  # default: current command
  end
end
```

---

## Re-stamping on resume

When a Cursor window reloads or a terminal tab is re-opened, the tty device may change — the
old name-file under `/tmp/cbc-termtitle/<old-tty>` no longer matches the new tty. The hook
reads no file for the new device and the default title shows.

**Fix:** the CBC skills re-run the name-file write in their `Resuming?` section — so the next
time an agent acts in the terminal after a reload, it re-stamps the correct tty automatically.
No manual intervention needed.

---

## Troubleshooting

**Tab shows "zsh" or the command name (not the agent name)**
- Cursor/VS Code: check `terminal.integrated.tabs.title`. If it says `${process}` or is missing
  the `${sequence}` variable, the OSC title is ignored. Set it to `"${sequence}"`.

**Tab is blank when no agent is running**
- Expected with `"${sequence}"` alone — no file → no sequence → blank. Use
  `"${sequence}${separator}${process}"` to fall back to the process name.

**Name appears briefly then reverts to user@host:cwd**
- The precmd hook isn't registered after the framework's own title hook. Move the hook to the
  very end of `~/.zshrc` (after `source $ZSH/oh-my-zsh.sh` and `source ~/.p10k.zsh`).

**Tab never changes at all**
- Emulator isn't honoring OSC 0/2 sequences, or the tool's stdout is fully captured before
  reaching the terminal. The name-file mechanism avoids this entirely — the agent writes a file
  and the shell (your interactive session) applies the title. If even the shell's precmd write
  doesn't work, check `print -Pn "\e]0;test\a"` directly in your shell; if that doesn't change
  the tab, the emulator needs the title-change setting (see emulator section above).

**`ps -o tty= -p $PPID` returns `??`**
- The agent's parent shell has no controlling tty (can happen in deeply nested process trees).
  Walk up the ppid chain to find the first non-`??` tty:
  ```bash
  t="??"; pid=$PPID
  while [ "$t" = "??" ] && [ "$pid" -gt 1 ]; do
    t=$(basename "$(ps -o tty= -p "$pid" | tr -d ' ')")
    pid=$(ps -o ppid= -p "$pid" | tr -d ' ')
  done
  ```
