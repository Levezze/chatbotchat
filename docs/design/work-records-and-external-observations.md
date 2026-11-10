# Work records and external observations

Framework requirements clarified 2026-11-09 for PRD #112 and stages #115–#118.
These capabilities follow the observable working-hierarchy gate in #113.
Updated 2026-11-10: a thin slice (registration, atomic work-item revisions,
requests with callbacks, outbox) and a dashboard skeleton move into #113 ahead
of the hierarchy trial. See [decisions T-07 to T-12](working-trial-protocol.md#2-decision-record-2026-11-10).
Stage #115 owns work records, PR observations and attention. It retains typed
deployment references and their separate ownership contract; actual provider
and environment observation integrations belong to #116 alongside their views.

## Shared coordination records

CBC is an opinionated agent-coordination framework. Project/repository/feature
views aggregate this CBC workspace's assignments, questions and evidence, not
every contributor's work or the product's authoritative specification. Link to
GitHub, repository documentation, Jira or Notion rather than replacing them.

A shared feature reference can group contributions from several repositories.
Each repository contribution retains its own finite Chapter Master and Primarch.
The repo view lists active contributions and concise current summaries; the
feature view groups owners, relevant rooms, PR links, decisions and milestones.
Agents read the same canonical service records through authorized role views as
the human GUI. Higher tiers do not gain access to private internal maps.

Chapter Masters submit small meaningful semantic changes: coarse stage, next
action, a specifically blocked operation, findings and evidence. Primarch notes,
priorities, deadlines and sequencing constraints have separate ownership. CBC
maintains timestamps, room/read state, routing and projections deterministically.
Transport activity does not reset the age of meaningful progress. Routine CI or
receipt bookkeeping does not require another model-authored summary.

A work-item update is one validated multi-field patch carrying an expected
revision and idempotency key. The service commits it as one work revision and one
outbox event; an equal or no-op patch produces no wake. Relevance and batching
apply on top. Status hints may coalesce; action and control records never do.

A coordinator-owned action survives a read receipt, an unrelated conversation
or completion of a status report. Preserve its owner, ready/request time, next
action, disposition and outcome evidence; a last-poll timestamp cannot reset its
age or resolve it. Classify actual native pending/refusal separately from agent
interpretation, product decisions, resource conflict and routine coordination.
An unattempted native action has an unknown verdict. The
[release follow-through case](../case-studies/2026-11-09-release-follow-through.md)
supplies concrete qualification scenarios without adding another approval tier.

The optional scope-supervisor [draft](../proposals/optional-scope-supervisor.md)
remains for discussion outside the execution/approval chain. It is not a
prerequisite for this store, the trial or ordinary work.

## Registration, controls and eligible merge

A Chapter Master registers its work item (assignment summary, branch/worktree,
declared surfaces and source links), receives the current constraint snapshot and
begins ordinary granted work without a startup GO or acknowledgement. A clear
snapshot means no declared hold at that revision, not proof of no conflict.
Advisory overlap metadata combines declared paths and contracts with bounded
changed-path/base/head facts collected on the execution device or by the PR
observer. The core does not clone repositories, read all code or impose blanket
file locks, and disjoint paths do not prove safety.

Primarch controls are structured records: continue, note, hold or stop, each with
scope, actor, revision, reason and release condition. An advisory note does not
become a hold. A hold affects only its operation, not independent work, and a
timeout never becomes approval. Record requested, received and applied
separately; delivery to a busy host is not immediate cancellation. The Chapter
Master checks current controls before affected effects such as a shared push or a
merge. This adds no per-tool parent permission.

A merge is eligible when repository-configured gates pass for the current head
and base, declared dependencies are met, no applicable hold exists and the work
item's HITL flag is off. With the flag on, a ready PR waits for the user's
review. The user or the Primarch may turn the flag on; only the user turns it
off; the Chapter Master cannot change it (T-07). A user instruction to promote
against the project's policy is recorded as a fact and the Primarch is notified,
not asked (T-08). Build,
test and review run outside a short conditional integration step; a moved base
revalidates only affected evidence. There is no global feature queue. Target
branches, environments and review policy are repository configuration. The
Chapter Master executes eligible merges. Semantic, cross-repo, migration and
resource exceptions within the Primarch's repository mandate go to the Primarch as targeted decisions. Questions the user owns go to the user as requests, not through the Primarch (T-10). Merge is not
deployment: an assigned promotion has a named release owner through its observed
outcome. Native permissions and grants stay separate from any role.

Controls belong to #114; work items and PR observations to #115. Until those are
implemented, Chapter Masters perform these eligibility checks themselves through
their existing tools and records; no CBC service enforces them yet.

## PR observations

Register a PR reference once, with optional agent notes. A shared authorized
GitHub integration maintains source-owned observations such as open/closed/merged,
draft state, current head revision and checks associated with that revision.
Retain provider IDs, source/evidence, observation time and unavailable/stale state.
An old successful check must not qualify a changed head. Provider observations
do not overwrite agent findings or Primarch annotations.

Use webhooks where reachable and suitable; otherwise use one bounded shared
conditional refresh for tracked references, with caching and rate-limit handling.
This is deterministic external synchronization, not an AI automation or per-agent
terminal poll. See GitHub's [pull-request API](https://docs.github.com/en/rest/pulls/pulls)
and [integration guidance](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api).

## Optional deployment observations

PR state and deployment state are separate fields/records. A merged PR does not
implicitly mean deployed, production-ready or enabled. Deployment tracking is
optional and configured per repository/environment/service. Environments are
configured names, not a hard-coded dev/staging/production enum.

A deployment observation needs an environment reference, provider/service,
release revision or artifact, observed state, observation time and source/evidence.
Verification is separate from deployment completion. Preserve multiple services
or attempts instead of collapsing them into one boolean. Distinguish absent
integration, unknown/stale data, in-progress, failure and observed success.

Delivery profiles describe the repository's branch/tag/workflow mapping and
configured observation adapter. Some projects deploy main to production; others
promote a production branch or tag. A mapping is an expectation, not runtime
evidence. Match observed releases/artifacts to work through explicit provenance,
including squash/cherry-pick differences, rather than blindly comparing a PR head.

For GCP, a configured observer may use authorized read-only provider APIs or an
existing authenticated gcloud context on the configured device. CBC does not
assume that login exists, copy credentials into chat, grant cloud permissions or
run a deployment to discover its state. Missing access remains explicit and does
not block messaging or projects without deployment integration. Provider-specific
queries and credentials require separate implementation qualification.

The same contract allows other providers and GitHub-backed deployment evidence.
Feature activation and user-visible outcomes may need their own evidence: a
release being deployed does not prove a flag is ON, an email was sent or a user
received it. The [activation case](../case-studies/2026-11-06-notification-activation-coordination.md)
records why these distinctions matter.

## Views and attention

The dashboard reflects canonical changes without asking models to rewrite boards.
Granular PR/provider observations can update the GUI without a Primarch or Emperor
wake. Role-scoped relevance determines meaningful notifications; a single core
scheduler supplies due/unchanged-responsibility attention later in #115. Questions
retain one request and its attributed review notes rather than copied hop-by-hop
versions. Requests are visible and answerable by their authorized human owner
from creation; Primarch review is not a prerequisite. Native callback expiry
is explicit, and a late dashboard answer cannot pretend to release an expired
native invocation. Both apps remain qualification targets; an unsupported lane
requires an explicit product decision, never a silent reduction to one app.
An Emperor can discuss shared project status without becoming the
mandatory path for answers or progress.

An empty inbox does not imply no responsibilities. Active managed work retains
an expected oversight checkpoint, so the hourly reminder can restore unchanged
parallel responsibilities when due. No managed work or open/due obligation means
no model wake. Core due policy is shared with the bounded #113 recovery path;
the connector does not maintain a competing business schedule.

Implementation belongs to the subsequent work-map/dashboard stages after the
working messaging trial. No dashboard, provider integration, skill or service is
implemented or activated by this requirements update.
