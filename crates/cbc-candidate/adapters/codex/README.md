# Candidate Codex adapter

`adapter.py` exposes bound candidate MCP tools. It selects identity from host-supplied
`_meta.threadId`, reads the private profile on every call, and writes an exact-batch receipt
only after the tool result is written. Tokens and receipt references stay inside the adapter.

For provisioning, runtime cwd checks, supported target fields and v1 compatibility, read
[the native target contract](../../NATIVE_TARGETS.md). Codex currently requires an unknown
execution profile because its MCP metadata does not distinguish Desktop from CLI.

`queue_delivery.py` sends content-free hints through the explicit installed Codex binary.
It is part of [one shared device driver](../device/README.md), not a per-room polling process.
Queue acceptance does not prove fetch, comprehension, human approval or completion.

Use [TESTING.md](../../TESTING.md) for isolated project setup and
[EVIDENCE.md](../../EVIDENCE.md) for actual native results. `binding_probe.py` and its tests
remain disposable identity probes, not enrollment authority.
