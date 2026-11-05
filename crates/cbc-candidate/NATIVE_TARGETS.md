# Native target contract

Use this contract when provisioning a candidate session or diagnosing a target mismatch.
It records where an enrolled session is allowed to execute. Native chat creation and saved
Desktop environment selection remain separate lifecycle work in #117.

## Provision and persist

Every new owner manifest member requires `target`, and its `repository` must equal the
member's repository. A target has this exact shape:

```json
{
  "v": 1,
  "profile": "unknown",
  "repository": "cbc-test",
  "worktree": null,
  "cwd": "/ACTUAL/CANONICAL/PROJECT",
  "saved_project": {"status": "not_exposed"},
  "saved_environment": {"status": "not_exposed"},
  "placement": {"evidence_ref": "ACTUAL_PLACEMENT_OBSERVATION"}
}
```

Capture the actual canonical cwd once before enrollment. Expected paths remain literal;
adapters canonicalize runtime observations, not the saved expectation. Paths must be absolute
and normalized. An optional non-null worktree must contain cwd. The target is bounded to
16 KiB, paths to 4096 bytes, and identifiers to 128 bytes.

Owner provisioning emits a private profile with `v:2`; each `targets` entry contains
`session`, `host` and the complete `target`. The core fixes that target to the invitation and
resulting binding epoch. Claiming the invitation with a changed target fails. Target history
survives restart and is not reassigned to later epochs. `binding_status.native_target` exposes
the current epoch's authority to trusted adapters. A local profile edit cannot replace it.

Profile v1 remains compatibility input for old unqualified bindings only. Its absent target
must match an explicit core `native_target:null`. A missing core field is an error, and a
qualified v2 binding cannot be downgraded by substituting a v1 profile.

## Current runtime capabilities

| Adapter | Runtime qualification |
| --- | --- |
| Codex | Host-supplied `_meta.threadId` plus the shim's actual canonical cwd. Desktop versus CLI is not exposed, so owner provisioning requires `profile:"unknown"`. |
| Claude MCP | Kernel peer PID/UID, native parent record/start time, actual host and shim cwd, and literal native entrypoint. `claude-desktop` qualifies Desktop; `cli` qualifies CLI. |
| Claude mod HTTP | Actual native caller PID/UID/start, host record, cwd and entrypoint, rechecked around core I/O. No caller-supplied identity or token. |

The current adapters cannot observe saved project/environment IDs reliably. Owner
provisioning therefore requires both selectors to be `not_exposed`. Supplying a configured
selector fails before provisioning; models cannot supply the missing observation. Supported
host tools can still provide UI placement evidence for an audit. Keep those IDs or labels in
the private evidence record; optional `placement.project_label` and `environment_label` are
bounded audit labels, not runtime identity or permission grants.

Before core operations or receipts, adapters require both runtime qualification and agreement
with the epoch-bound core target. The device driver's SIGHUP refresh rejects a profile whose
target differs from core authority, preserving its working registry. These checks use native
OS APIs in-process, not a subprocess per call. Queue acceptance, a matching display name,
working directory alone, or an evidence label does not prove saved UI placement.

For the actual native probes and remaining limitations, read [EVIDENCE.md](EVIDENCE.md).
For an isolated run, follow [TESTING.md](TESTING.md).
