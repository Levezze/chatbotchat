# CBC native agent lifecycle and room policy

Agreed 2026-11-04. Current requirements for [PRD #112](https://github.com/Levezze/chatbotchat/issues/112).
This supersedes earlier message-budget and environment-by-folder assumptions for the redesign.
It does not change deployed CBC, installed skills, native permissions or running agents.

Clarified 2026-11-09: CBC is a coordination framework. The immediate gate is an
[observable working hierarchy trial](message-observability-and-working-pilot.md)
before dashboard/work-map implementation. Earlier native marker/timing probes
remain preliminary evidence, not acceptance of an implementing hierarchy. Later
[work and external-observation records](work-records-and-external-observations.md)
separate agent assertions, PR facts and optional per-environment deployment facts.

## Native agent creation and continuation

A Primarch can request a fresh Chapter Master, give its initial role/assignment, message an
existing child and resume it within a standing scoped grant. Chapter Masters own finite
implementation tasks, native implementation subagents and their PRs through eligible merge. Apps and supported CLI profiles
remain first-class targets; native chat creation, messaging, wake/resume, steering, history
reading and permission return are independent capabilities.

Setup records a logical launch target pointing to:
- The selected host/device and native app or CLI profile.
- Native saved project/environment identifiers where the supported interface exposes them.
- Repository/worktree and allowed execution settings or permission-profile references.

Working directory, execution environment and UI grouping are separate facts. Opening the
right folder does not prove selection of the saved Desktop environment, including its
variables, remote machine or UI grouping. An explicitly selected Desktop target cannot
silently become a headless CLI/SDK session. Do not copy environment secrets into prompts.

The shared connector resolves the target, checks the enrolled parent's standing grant,
creates the native session through a supported interface, verifies its actual placement,
registers its returned native identity and establishes its authorized CBC report connection.
Existing-session messages and continuation use that binding. A different environment is a
mismatch to resolve, not a reason to redirect the existing session silently.

Grant creation, messaging and resume operations to enrolled member IDs and configured
targets. The display name Primarch grants no authority. Configure supported native tool
permissions at setup; ordinary permitted operations need no new CBC approval question.
Agent-origin messages retain their origin and cannot manufacture human consent for a
pending native dialog. Genuine native consent/unsupported controls stay explicit exceptions.

Record requested, created, bound/ready and first-assignment delivery separately. Launch
request IDs are idempotent. Reconcile an uncertain result before retrying so recovery does
not duplicate children or compete with an already-running session controller. Queue success,
process exit and observed history are not proof of native activation or resumed work.

### Capability evidence, not completion claims

As checked on 2026-11-04:
- Codex in-app tools expose saved project selection, local/worktree targets and an initial
  prompt, or a saved cloud environment configuration. External CBC access to the correct
  native controller needs separate qualification. App-server documents thread creation and
  turn input; a separately started server is not automatically the existing Desktop host.
- Claude CLI supports an initial prompt in the chosen directory and session settings.
  The installed CLI is 2.1.312; version/help were checked without launching a session.
- Claude Desktop supports opening the current directory and transferring an inactive CLI
  session using `claude --desktop --resume ID`. That command accepts no starting prompt
  or documented saved-Desktop-environment selector. Complete zero-click creation, correct
  saved environment/UI placement and first activation remain unverified.
- Existing-chat message delivery is separate. Use supported registered-target routes and
  retain distinctions between accepted, delivered, fetched and applied. A documented inbox
  or queue route does not establish dormant wake or native permission approval.

Keep evidence per actual app/CLI/device/version. Unsupported behavior is reported accurately;
one successful CLI demonstration never qualifies a requested Desktop profile.

## Uncapped rooms and informational milestones

Ordinary authorized sends have no lifetime message-count cap, soft/hard communication
budget, mutual extension vote or human extension approval. Bounded payloads, fair retrieval
and transient backpressure remain resource protections, not conversation budgets.

Initially notify at every 100 distinct committed participant messages: 100, 200 and so on.
Cadence is validated live configuration through the same settings service, not a restart
requirement. Idempotent send retries and server notices do not increase that count.
Persist milestone delivery and deduplicate by room, milestone and logical recipient;
restart or policy changes must not replay a backlog of old notices.
Count crossing and recipient notices commit with the successful send. Agents do not maintain
counts or emit a second announcement, and notices cannot count themselves recursively.

Example notice:
> Info: 100 messages. No action required. Keep this room focused on its coordination purpose;
> use a new linked room if that purpose changes.

The notice is informational only:
- Report room: notify only the Primarch/parent owner, not the child.
- Peer room: notify both peers, rather than treating the opener as their superior.
- Scope it to those recipients even when shown in room history or projected maps.
- Coalesce delivery; require no acknowledgement, reply, extension vote or user decision.
- Create no execution hold, waiting status, escalation, approval card or automatic closure.
- Do not increment the message count with notices or trigger recursive notices.

A room tracks one coherent coordination purpose or assignment/session relationship. Primarch
sessions and their valid report/peer relationships may span days, weeks and several features.
A count threshold, elapsed time or one feature's completion never forces rotation of a broader
valid relationship. For genuinely unrelated work, choose a fresh linked room when useful and
preserve the prior history and outstanding obligations. No parent approval is needed merely
to continue using an authorized room. No immediate action is required by a milestone.

## Slice ownership and acceptance

- #113: uncapped send/delivery, target binding, live milestone policy, private recipient routing
  and restart/idempotency behavior, structured debug/delivery timing, and a real
  isolated role-based implementation trial. Prove sending through 100/200 with no
  stop or extension. Qualify timely working delivery before dashboard stages.
- #114: actual native exceptions return through supported decision mechanisms. Milestone
  notices never become decision requests. Standing lifecycle grants avoid routine prompts.
- #115: server-owned target/count metadata in maps; notices do not change semantic progress.
  Own feature grouping, typed PR/external references, the optional deployment
  source-ownership contract and independent Primarch notes.
- #116: actual environment/provider observation integrations, repo/feature views,
  correct native links and optional informational display, without an Emperor wake
  for every provider/CI event.
- #117: complete create/bind/first-assignment/message/resume demonstration in the selected
  saved project/environment, including UI grouping, standing grants and uncertain-result
  recovery. Registration returns the current constraint snapshot and the Chapter Master
  starts without a startup GO. No extra coordination tier or human room-ID courier.
- #118: same targets/grants across devices; the Pi core does not launch a local substitute.
- #119: bounded history reads navigate verified bindings without activating the model.

Disposable tests must distinguish wrong environment, unverified placement, unavailable
adapter, native consent and dormant session. Test standing-grant revocation, duplicate launch
requests, stale bindings and count-notice recipient isolation without changing daily CBC.
The 300-test candidate checkpoint and native preliminary probes in draft #120 do
not prove the newly required working implementation trial, comprehensive timing
instrumentation or environment-aware zero-click creation.

## Proposals awaiting discussion

- [Optional scope supervisor](../proposals/optional-scope-supervisor.md): draft for
  2026-11-09. An adversarial feature-scope reviewer outside the execution and
  approval chain, with a proposed dedicated skill. This is not an agreed extra
  coordination tier or a change to current slice acceptance.

Sources:
- [Codex app-server](https://learn.chatgpt.com/docs/app-server)
- [Codex projects](https://learn.chatgpt.com/docs/projects)
- [Claude CLI reference](https://code.claude.com/docs/en/cli-reference)
- [Claude Desktop environments and handoff](https://code.claude.com/docs/en/desktop)
- [Claude cross-session messaging](https://code.claude.com/docs/en/cross-session-messaging)
