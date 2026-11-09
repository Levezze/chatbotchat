---
name: cbc-handoff
description: Replace a CBC lead session when explicitly requested, preserving its identity and unfinished work. Ordinary assignments and status updates need no handoff.
---

# CBC role handoff

Read `../cbc-hierarchy/SKILL.md` and the current canonical role. Transfer one logical member to a replacement native session. Keep its context, member, role, CBC label, open rooms, questions and children. Do not rebuild the whole hierarchy.
Historical domain records remain recoverable. Export actual receiving rooms, including old
routes still in use. For a skill refresh, read `../cbc-hierarchy/references/upgrade-v2.md`.
The current operational hierarchy is Primarch to Chapter Master. This utility is only for
an actual session replacement, never routine task delegation, phase changes or status reports.

With `--accept <handoff-dir>`, enter as the successor and follow the accept section, not the predecessor preparation. Without that argument, prepare from the current member's own records. This invocation does not assign you a different logical role.

This is a cooperative protocol on current CBC, not a server-enforced lease or atomic takeover. The helper writes/verifies local artifacts only. It does not stop processes, create chats, move native subagents or prove who owns a remote session.

Both sessions must use the same canonical handoff directory for release/outcome markers. Independent copies cannot arbitrate ownership. If that directory is not accessible, stop the transfer and establish an authorized shared artifact/receipt route; a local path is not a cross-device service.

## Prepare in the predecessor

1. Read your own records and catch up on every owned room. Freeze new decisions, implementation assignments and child creation. Preserve actual native session IDs, child IDs/roots, holds, pending outbound events, source links, next action and the last HANDLED message sequence per room. A consumed cursor is not proof that a message was handled.
2. Resolve native subagent children before releasing: collect their results or cancel them through supported, authorized controls. They cannot move into another session. If any remain live or unacknowledged, keep the handoff blocked and report them. Session-type children keep their existing rooms and identities.
3. Refresh your compact records, then export them with the helper:

   ```bash
   python3 <this-skill-dir>/scripts/handoff.py prepare --state-dir <own-record-dir> --from-session <actual-old-native-id> --title <current-title> --handled '{"<room-id>":123}'
   ```

   It creates a timestamped `handoffs/<UTC>/handoff.md`, snapshot files and a checksummed manifest. Treat these as immutable. Inspect the generated handoff before offering it. Subsequent changes need a fresh export, never silent edits to a sealed snapshot.
4. Create or attach one successor as an independent chat/session on the configured host, never a native subagent, only within the user's explicit replacement request and host permissions. This is a replacement, not permission for extra children or helpers. Use the same native project/root and pass `/cbc-handoff --accept <absolute-handoff-dir>`, the canonical role, logical identity and source links. The successor may read/recap but must not poll or send under the old label before release. Persist native creation intent and the returned chat ID; reconcile uncertain outcomes rather than duplicating sessions.

If native creation is unsupported, provide the handoff path and one precise attach prompt. A prepared file alone does not mean the role is superseded.

## Release before acquire

1. Establish the intended successor's actual native ID and readiness to read the handoff. Announce the pending transfer to parent/children through their existing rooms.
2. Stop and verify the termination of only your own receive tasks. Capture any delivered-but-unhandled message references. Their room transcripts remain available to the successor. Start no replacement polls.
3. Record release to exactly that successor:

   ```bash
   python3 <this-skill-dir>/scripts/handoff.py release --handoff <dir> --from-session <old-id> --to-session <new-id> --handled '{"<room-id>":130}' --outbound '[{"room":"<room-id>","seq":129,"status":"sent"}]' --evidence '<receivers stopped; native children settled>'
   ```

   Use final last-HANDLED checkpoints, not the earlier prepare values. Include late outbound references, or `[]` when none. The helper captures and hashes final state/brief/question snapshots, preserving answers and holds handled after preparation. From release onward the predecessor does not consume, send or make decisions as that member. Read-only inspection is allowed. If the successor never acquires, report `released, unowned` through the predecessor's native chat. Do not auto-reacquire or claim completion.

## Accept in the successor

1. Verify snapshot hashes and read the manifest, all three snapshots and handoff. Identify the exact role/context/member and your intended native session ID. Reconcile external facts needed next against their actual sources.
2. Read rooms and PLAN catch-up only. Before ownership is acquired, make no sends, decisions, acknowledgements or child instructions. Read-only recap can happen before release.
3. Wait for a valid `released.json` naming your actual session. Re-run helper `verify` to check final snapshot hashes and transfer state. Read the release snapshots, its final checkpoints and late outbound references. A missing marker, wrong recipient, revoked outcome or ambiguous status blocks acquisition.
4. BEFORE starting any receiver or acting, claim the one exclusive outcome:

   ```bash
   python3 <this-skill-dir>/scripts/handoff.py accept --handoff <dir> --session <actual-new-id> --evidence '<release, final snapshots, checkpoints and intended native ownership verified>'
   ```

   `outcome.json` arbitrates accept versus revoke with an exclusive file creation. If it already exists, inspect it. Continue only if it is an accepted outcome naming YOU; otherwise stand down. This is filesystem coordination in that one directory, not global server fencing.
5. After winning the claim, start one receiver per room under the SAME label. Process all relevant messages after the RELEASE checkpoints, even if the old poll consumed them. Reconcile STOP/PAUSE, questions and uncertain sends. Preserve the final maps, sources and revisions; change only physical ownership. Never bootstrap a second logical member.
6. After verifying receivers and catch-up, acknowledge activation:

   ```bash
   python3 <this-skill-dir>/scripts/handoff.py activate --handoff <dir> --session <new-id> --evidence '<catch-up complete; actual receivers verified>'
   ```

   Notify existing parent/child/peer rooms once with the transfer/checkpoint reference. Accepted ownership and verified activation are different facts; failure after acceptance remains visible and does not let the predecessor silently resume.

## Supersede the predecessor

The predecessor waits for accepted ownership AND matching `activated.json`. Then run `handoff.py supersede --handoff <dir> --session <old-id>`. It verifies the activated successor and exclusively writes `superseded.json`, without overwriting the successor's live map. Compute the old-chat title using the accepted outcome's fixed `at` timestamp:

```bash
python3 <hierarchy-skill-dir>/scripts/session_title.py --supersede-title <old-title> --at <accepted-at>
```

The title is `<old-title>-superseded-YYYYMMDDTHHMMSSffffffZ`. Rename only the predecessor's own native session through the supported mechanism. A declined rename remains a recorded cosmetic failure; do not undo an accepted transfer. The successor keeps the normal active title.

## Cancel an unaccepted transfer

Only on an explicit user direction, the predecessor may run `handoff.py revoke --handoff <dir> --session <old-id> --evidence <actual-user-authority-reference>`. Revoke competes for the SAME exclusive outcome file as accept. Resume only after verifying that the revoked outcome won; an already accepted outcome forbids predecessor reacquisition. The helper does not validate human authority for you. Keep markers rather than deleting/resetting them. A failed or abandoned accepted owner needs separate explicit recovery, not reuse of an old release.

No room closes, forced closes, worktree deletion, automatic archiving or global permission changes are part of this workflow. Quiet rules still apply. Preserve the handoff and receipts until recovery no longer depends on their worktree.

## Uncooperative predecessor

If the predecessor crashed or cannot release, normal `/cbc-handoff` cannot prove a safe takeover. Do not infer ownership from a stale timestamp or `poll_live: false` alone. Surface an explicit recovery request to the user/parent, establish that old receivers and native children are stopped or contained, and record any weaker recovery guarantee. This skill does not silently perform orphan takeover. On any later resume, a predecessor must check transfer markers before restarting work or receives.
