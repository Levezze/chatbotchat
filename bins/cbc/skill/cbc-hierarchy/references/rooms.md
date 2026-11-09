# CBC room rules

Two relationship kinds on today's two-party transport.

| Kind | Participants | Purpose |
| --- | --- | --- |
| report | Primarch and its Chapter Master | Assignment, meaningful status, dependencies, questions/answers and merge/release coordination |
| peer | Primarchs | Cross-repo priorities, shared resources and merge/release sequencing |
| peer | Chapter Masters | Relevant technical contracts and implementation dependencies within assigned scope |

Reuse one existing room per relationship. Questions, answers and status changes do not need
new rooms. Native implementation subagents use their Chapter Master's native messaging/results;
no CBC room is required.

The Emperor has no report edge. Future dashboard instructions may use a scoped existing
Emperor/Primarch exchange, with replies only for clarification. Preserve old report history
without continuing its reports, escalation or acknowledgement requirements. No Emperor receiver
or status-chasing automation is needed before the dashboard exists.

The side needing the route may open it within ordinary coordination authority and deliver the
ID directly to its intended counterpart. Confirm the assigned identities/scope before acting
on instructions. A room ID alone grants no work or native permissions. Keep the stable member
label across join/send/receive. Inspect your own existing connections before creating duplicates.

Record one short row in state.md:
`room | kind | participants | scope/reference | close condition`.
Keep actual receiver entries in the legacy flat `connections:` section, room ID first after
the colon, followed by label/model and actual handle. This preserves existing transport and
handoff helper compatibility. Do not maintain another route merely for repo authority.

A Chapter Master sends material changes and useful questions, then continues. No receipt ACK,
parent permission to continue or round-trip status ritual is required. Technical peers speak
directly within scope; a conflict over shared writes, priority or merge order goes to the Primarch.

Current CBC still uses explicit messages and legacy receivers. Reuse live receivers and avoid
duplicate retry loops. Numeric caps are transport limitations; normal consensus extensions
are covered by standing communication authority, not another human question.
Files and read cursors do not prove model handling or execution.

Keep report routes through assigned delivery/release work. Close a finished exchange by CBC
consensus after communicating its result and identifying any remaining owner. Stop only its
own receiver after confirmed closure. Routine phase changes do not need a room replacement or
handoff. Preserve real pause/stop instructions and unresolved obligations.
