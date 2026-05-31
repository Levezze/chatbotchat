use chatbotchat_core::event::EventKind;
use chatbotchat_core::message::{MessageType, Severity};
use chatbotchat_core::participant::Participant;
use chatbotchat_core::room::{Room, RoomConfig, RoomState};
use chatbotchat_core::storage::Storage;
use time::{Duration, OffsetDateTime};

async fn fresh_storage() -> Storage {
    Storage::connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite")
}

fn sample_room() -> Room {
    let now = OffsetDateTime::now_utc();
    Room {
        id: "smoke-test-20260528-1500".into(),
        subject: "smoke test".into(),
        started_at: now,
        last_activity_at: now,
        state: RoomState::Active,
        state_changed_at: now,
        config: RoomConfig::default(),
        prev_room_id: None,
    }
}

#[tokio::test]
async fn create_then_get_room_round_trips() {
    let storage = fresh_storage().await;
    let room = sample_room();

    storage.create_room(&room).await.expect("create_room ok");

    let fetched = storage
        .get_room(&room.id)
        .await
        .expect("get_room ok")
        .expect("room exists");

    assert_eq!(fetched.id, room.id);
    assert_eq!(fetched.subject, room.subject);
    assert_eq!(fetched.state, RoomState::Active);
    assert_eq!(fetched.config, room.config);
    assert!(fetched.prev_room_id.is_none());
}

#[tokio::test]
async fn get_room_returns_none_for_missing() {
    let storage = fresh_storage().await;
    let missing = storage
        .get_room("does-not-exist")
        .await
        .expect("get_room ok");
    assert!(missing.is_none());
}

fn sample_participant(room_id: &str, cwd: &str) -> Participant {
    let now = OffsetDateTime::now_utc();
    Participant {
        handle: format!("smoke-test-opus47-{}", &cwd.len()),
        room_id: room_id.into(),
        repo: "smoke-test".into(),
        model: "opus47".into(),
        cwd: cwd.into(),
        joined_at: now,
        last_poll_at: now,
        last_read_seq: 0,
    }
}

#[tokio::test]
async fn participant_create_get_by_tuple_and_list_round_trip() {
    let storage = fresh_storage().await;
    let room = sample_room();
    storage.create_room(&room).await.expect("create_room ok");

    let p = sample_participant(&room.id, "/work/a");
    storage.create_participant(&p).await.expect("create ok");

    // Exact tuple round-trips.
    let fetched = storage
        .get_participant_by_tuple(&room.id, &p.repo, &p.model, &p.cwd)
        .await
        .expect("get ok")
        .expect("participant exists");
    assert_eq!(fetched, p);

    // A different cwd is a different tuple — not found.
    let other = storage
        .get_participant_by_tuple(&room.id, &p.repo, &p.model, "/work/b")
        .await
        .expect("get ok");
    assert!(other.is_none());

    // The room lists exactly this one participant.
    let listed = storage.list_participants(&room.id).await.expect("list ok");
    assert_eq!(listed, vec![p]);
}

fn participant_with_handle(room_id: &str, handle: &str, cwd: &str) -> Participant {
    let now = OffsetDateTime::now_utc();
    Participant {
        handle: handle.into(),
        room_id: room_id.into(),
        repo: "smoke-test".into(),
        model: "opus47".into(),
        cwd: cwd.into(),
        joined_at: now,
        last_poll_at: now,
        last_read_seq: 0,
    }
}

#[tokio::test]
async fn messages_seq_is_monotonic_and_filtered_by_recipient_and_cursor() {
    let storage = fresh_storage().await;
    let room = sample_room();
    storage.create_room(&room).await.expect("create_room ok");

    let alice = participant_with_handle(&room.id, "smoke-test-opus47-aaaa", "/work/a");
    let bob = participant_with_handle(&room.id, "smoke-test-opus47-bbbb", "/work/b");
    storage
        .create_participant(&alice)
        .await
        .expect("create alice");
    storage.create_participant(&bob).await.expect("create bob");

    let now = OffsetDateTime::now_utc();
    // A broadcast (recipient = None), then a message targeted to alice.
    let m1 = storage
        .create_message(&room.id, &bob.handle, None, "hello all", now)
        .await
        .expect("create m1");
    let m2 = storage
        .create_message(
            &room.id,
            &bob.handle,
            Some(&alice.handle),
            "psst alice",
            now,
        )
        .await
        .expect("create m2");

    // seq is monotonically increasing on insert.
    assert!(
        m2.seq > m1.seq,
        "seq must increase: {} !> {}",
        m2.seq,
        m1.seq
    );
    assert_eq!(m1.recipient, None);
    assert_eq!(m2.recipient, Some(alice.handle.clone()));
    assert_eq!(m1.body, "hello all");

    // From cursor 0, alice's oldest unread is the broadcast m1.
    let next = storage
        .next_unread(&room.id, &alice.handle, 0)
        .await
        .expect("next_unread ok")
        .expect("alice has an unread");
    assert_eq!(next.seq, m1.seq);

    // Claiming consumes the broadcast m1 and atomically advances alice's cursor
    // (the real wait path — no cursor mutator that bypasses the atomic claim).
    let claimed = storage
        .claim_next_unread(&room.id, &alice.handle)
        .await
        .expect("claim ok")
        .expect("alice has an unread to claim");
    assert_eq!(claimed.seq, m1.seq);

    // After consuming past m1, alice's next unread is the targeted m2.
    let next2 = storage
        .next_unread(&room.id, &alice.handle, m1.seq)
        .await
        .expect("next_unread ok")
        .expect("alice has the targeted message");
    assert_eq!(next2.seq, m2.seq);

    // The targeted-to-alice message is invisible to a different handle.
    let bob_next = storage
        .next_unread(&room.id, &bob.handle, m1.seq)
        .await
        .expect("next_unread ok");
    assert!(
        bob_next.is_none(),
        "a message addressed to alice must not surface for bob"
    );

    // The claim persisted the advanced cursor onto the participant row.
    let alice_row = storage
        .get_participant_by_tuple(&room.id, &alice.repo, &alice.model, &alice.cwd)
        .await
        .expect("get ok")
        .expect("alice exists");
    assert_eq!(alice_row.last_read_seq, m1.seq);
}

#[tokio::test]
async fn claim_next_unread_for_unknown_handle_returns_none_without_hanging() {
    let storage = fresh_storage().await;
    let room = sample_room();
    storage.create_room(&room).await.expect("create_room ok");

    // A real participant posts a broadcast (seq > 0, recipient = all).
    let p = participant_with_handle(&room.id, "smoke-test-opus47-aaaa", "/work/a");
    storage
        .create_participant(&p)
        .await
        .expect("create participant");
    let now = OffsetDateTime::now_utc();
    storage
        .create_message(&room.id, &p.handle, None, "hi all", now)
        .await
        .expect("create message");

    // Claiming for a handle that is not a participant must return None — not spin
    // forever on a CAS that can never match a non-existent row.
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        storage.claim_next_unread(&room.id, "ghost-handle-not-a-participant"),
    )
    .await;
    assert!(
        result.is_ok(),
        "claim_next_unread must not hang for an unknown handle"
    );
    assert!(
        result.unwrap().expect("claim ok").is_none(),
        "an unknown handle has nothing to claim"
    );
}

#[tokio::test]
async fn sentinel_rows_do_not_count_toward_the_cap() {
    let storage = fresh_storage().await;
    let room = sample_room();
    storage.create_room(&room).await.expect("create_room ok");
    let now = OffsetDateTime::now_utc();

    // Two real `msg` rows...
    storage
        .create_message(&room.id, "sender", None, "m0", now)
        .await
        .expect("create m0");
    storage
        .create_message(&room.id, "sender", None, "m1", now)
        .await
        .expect("create m1");

    // ...and a sentinel row interleaved. Sentinels (`type != 'msg'`) are signals,
    // not conversation turns, so they must not inflate the cap count.
    storage
        .create_message_typed(
            &room.id,
            "sender",
            None,
            "consulting my user",
            now,
            MessageType::WaitingUser,
            None,
            None,
        )
        .await
        .expect("create sentinel");

    assert_eq!(
        storage
            .count_capped_messages(&room.id)
            .await
            .expect("count ok"),
        2,
        "only `msg` rows count toward the cap; the sentinel must be excluded"
    );
}

#[tokio::test]
async fn msg_type_survives_the_write_read_round_trip() {
    let storage = fresh_storage().await;
    let room = sample_room();
    storage.create_room(&room).await.expect("create_room ok");
    let now = OffsetDateTime::now_utc();

    // A sentinel, then a plain msg. Reading them back must preserve each row's
    // type — proves `create_message_typed` writes and `row_to_message` reads the
    // `type` column correctly (not just that the count seam excludes sentinels).
    storage
        .create_message_typed(
            &room.id,
            "sender",
            None,
            "consulting my user",
            now,
            MessageType::WaitingUser,
            None,
            None,
        )
        .await
        .expect("create sentinel");
    storage
        .create_message(&room.id, "sender", None, "a turn", now)
        .await
        .expect("create msg");

    // recent_messages is oldest-first.
    let recent = storage
        .recent_messages(&room.id, 10)
        .await
        .expect("recent ok");
    let types: Vec<MessageType> = recent.iter().map(|m| m.msg_type).collect();
    assert_eq!(
        types,
        vec![MessageType::WaitingUser, MessageType::Msg],
        "each row's msg_type must survive the storage round-trip in order"
    );
}

#[tokio::test]
async fn sentinel_severity_and_question_survive_round_trip() {
    let storage = fresh_storage().await;
    let room = sample_room();
    storage.create_room(&room).await.expect("create_room ok");
    let now = OffsetDateTime::now_utc();

    // A `waiting_user` sentinel carries a severity and the question the agent is
    // asking its user. Both must survive the storage round-trip alongside the
    // type — the question lives in `question_text`, not `body` (body stays empty
    // for a pure sentinel).
    storage
        .create_message_typed(
            &room.id,
            "sender",
            None,
            "",
            now,
            MessageType::WaitingUser,
            Some(Severity::High),
            Some("should I merge to production?"),
        )
        .await
        .expect("create sentinel");

    let recent = storage
        .recent_messages(&room.id, 10)
        .await
        .expect("recent ok");
    let m = recent.first().expect("one row");
    assert_eq!(m.severity, Some(Severity::High), "severity must round-trip");
    assert_eq!(
        m.question_text.as_deref(),
        Some("should I merge to production?"),
        "question_text must round-trip"
    );
}

#[tokio::test]
async fn latest_message_from_other_returns_the_counterpart_active_sentinel() {
    let storage = fresh_storage().await;
    let room = sample_room();
    storage.create_room(&room).await.expect("create_room ok");
    let now = OffsetDateTime::now_utc();

    // The counterpart ("sender") paused to consult its user. From "viewer"'s
    // vantage the latest non-self row is that active sentinel — the backoff
    // driver reads its type/severity/created_at.
    storage
        .create_message_typed(
            &room.id,
            "sender",
            None,
            "",
            now,
            MessageType::WaitingUser,
            Some(Severity::High),
            Some("merge?"),
        )
        .await
        .expect("create sentinel");

    let latest = storage
        .latest_message_from_other(&room.id, "viewer")
        .await
        .expect("query ok")
        .expect("a counterpart row exists");
    assert_eq!(latest.msg_type, MessageType::WaitingUser);
    assert_eq!(latest.severity, Some(Severity::High));
    assert_eq!(latest.sender, "sender");
}

#[tokio::test]
async fn latest_message_from_other_supersedes_a_cleared_sentinel() {
    let storage = fresh_storage().await;
    let room = sample_room();
    storage.create_room(&room).await.expect("create_room ok");
    let now = OffsetDateTime::now_utc();

    // Sentinel, then the same sender resumes with a plain turn. The later `msg`
    // self-supersedes the pause: latest-of-any-type returns the `msg`, so the
    // handler's `type == WaitingUser` check sees no active sentinel. (Filtering
    // to waiting_user rows in SQL would wrongly keep backing off forever.)
    storage
        .create_message_typed(
            &room.id,
            "sender",
            None,
            "",
            now,
            MessageType::WaitingUser,
            Some(Severity::High),
            Some("merge?"),
        )
        .await
        .expect("create sentinel");
    storage
        .create_message(&room.id, "sender", None, "back, resuming", now)
        .await
        .expect("create msg");

    let latest = storage
        .latest_message_from_other(&room.id, "viewer")
        .await
        .expect("query ok")
        .expect("a counterpart row exists");
    assert_eq!(
        latest.msg_type,
        MessageType::Msg,
        "the later plain turn must supersede the sentinel"
    );
}

#[tokio::test]
async fn latest_message_from_other_excludes_the_callers_own_rows() {
    let storage = fresh_storage().await;
    let room = sample_room();
    storage.create_room(&room).await.expect("create_room ok");
    let now = OffsetDateTime::now_utc();

    // Only the viewer has spoken — there is no counterpart row, so no backoff.
    storage
        .create_message(&room.id, "viewer", None, "anyone there?", now)
        .await
        .expect("create msg");

    let latest = storage
        .latest_message_from_other(&room.id, "viewer")
        .await
        .expect("query ok");
    assert!(
        latest.is_none(),
        "the caller's own messages never count as a counterpart pause"
    );
}

#[tokio::test]
async fn create_message_capped_gate_ignores_sentinel_rows() {
    let storage = fresh_storage().await;
    let room = sample_room();
    storage.create_room(&room).await.expect("create_room ok");
    let now = OffsetDateTime::now_utc();

    // A sentinel sits in the room before any `msg` is sent. With a cap of 1, the
    // enforcement gate must still admit the first real `msg`: the sentinel does
    // not occupy cap budget. (Both seams count `type = 'msg'` only — in lockstep.)
    storage
        .create_message_typed(
            &room.id,
            "sender",
            None,
            "consulting my user",
            now,
            MessageType::WaitingUser,
            None,
            None,
        )
        .await
        .expect("create sentinel");

    const CAP: i64 = 1;
    let admitted = storage
        .create_message_capped(&room.id, "sender", None, "first msg", now, false, CAP)
        .await
        .expect("capped insert ok");
    assert!(
        admitted.is_some(),
        "a sentinel must not consume cap budget; the first msg is under cap"
    );

    // And now the cap is genuinely full for `msg` rows.
    let refused = storage
        .create_message_capped(&room.id, "sender", None, "second msg", now, false, CAP)
        .await
        .expect("capped insert ok");
    assert!(
        refused.is_none(),
        "the second msg is at the cap and must be refused"
    );
}

#[tokio::test]
async fn from_human_survives_the_write_read_round_trip() {
    let storage = fresh_storage().await;
    let room = sample_room();
    storage.create_room(&room).await.expect("create_room ok");
    let now = OffsetDateTime::now_utc();

    // A human-tagged msg (the `--human` fold) then a plain agent msg. Reading them
    // back must preserve each row's `from_human` flag — proves the column writes
    // (capped insert binds it into the atomic statement) and `row_to_message`
    // reads it back. The soft-cap reset boundary keys off this flag.
    const CAP: i64 = 10;
    storage
        .create_message_capped(&room.id, "sender", None, "human says", now, true, CAP)
        .await
        .expect("capped insert ok")
        .expect("under cap");
    storage
        .create_message_capped(&room.id, "sender", None, "agent says", now, false, CAP)
        .await
        .expect("capped insert ok")
        .expect("under cap");

    let recent = storage
        .recent_messages(&room.id, 10)
        .await
        .expect("recent ok");
    let flags: Vec<bool> = recent.iter().map(|m| m.from_human).collect();
    assert_eq!(
        flags,
        vec![true, false],
        "from_human must survive the storage round-trip in order"
    );
}

#[tokio::test]
async fn consecutive_msg_count_climbs_across_non_human_msgs() {
    let storage = fresh_storage().await;
    let room = sample_room();
    storage.create_room(&room).await.expect("create_room ok");
    let now = OffsetDateTime::now_utc();
    const CAP: i64 = 10;

    // Three autonomous (non-human) turns in a row. At delivery of each seq, the
    // soft counter — consecutive `msg` rows since the last human input — is that
    // turn's position in the run. This is what the wait response compares to
    // `soft_cap - 1` to decide whether to surface the conversation to the user.
    let mut seqs = Vec::new();
    for i in 0..3 {
        let m = storage
            .create_message_capped(&room.id, "a", None, &format!("m{i}"), now, false, CAP)
            .await
            .expect("capped insert ok")
            .expect("under cap");
        seqs.push(m.seq);
    }

    for (i, seq) in seqs.iter().enumerate() {
        assert_eq!(
            storage
                .consecutive_msg_count(&room.id, *seq)
                .await
                .expect("count ok"),
            (i + 1) as i64,
            "the run length at the {}th msg must be {}",
            i + 1,
            i + 1
        );
    }
}

#[tokio::test]
async fn consecutive_msg_count_resets_at_a_human_msg() {
    let storage = fresh_storage().await;
    let room = sample_room();
    storage.create_room(&room).await.expect("create_room ok");
    let now = OffsetDateTime::now_utc();
    const CAP: i64 = 10;

    // Two autonomous turns, then a `--human` fold, then another autonomous turn.
    storage
        .create_message_capped(&room.id, "a", None, "m1", now, false, CAP)
        .await
        .expect("ok")
        .expect("under cap");
    let m2 = storage
        .create_message_capped(&room.id, "a", None, "m2", now, false, CAP)
        .await
        .expect("ok")
        .expect("under cap");
    let human = storage
        .create_message_capped(&room.id, "a", None, "user says", now, true, CAP)
        .await
        .expect("ok")
        .expect("under cap");
    let m4 = storage
        .create_message_capped(&room.id, "a", None, "m4", now, false, CAP)
        .await
        .expect("ok")
        .expect("under cap");

    // Before the human fold the run is 2.
    assert_eq!(
        storage
            .consecutive_msg_count(&room.id, m2.seq)
            .await
            .expect("count ok"),
        2
    );
    // The human row IS the reset boundary — excluded from its own run, so the
    // count at its delivery is 0 (no autonomous turns after the reset yet).
    assert_eq!(
        storage
            .consecutive_msg_count(&room.id, human.seq)
            .await
            .expect("count ok"),
        0,
        "a from_human msg resets the run to 0"
    );
    // The next autonomous turn restarts the run at 1, not 3.
    assert_eq!(
        storage
            .consecutive_msg_count(&room.id, m4.seq)
            .await
            .expect("count ok"),
        1,
        "the run restarts after the human fold"
    );
}

#[tokio::test]
async fn waiting_user_sentinel_resets_the_consecutive_msg_run() {
    let storage = fresh_storage().await;
    let room = sample_room();
    storage.create_room(&room).await.expect("create_room ok");
    let now = OffsetDateTime::now_utc();
    const CAP: i64 = 10;

    // An autonomous turn, then a `waiting_user` sentinel, then another turn.
    // Consulting the user pulls a human into the loop, so it BREAKS the
    // consecutive-autonomous run exactly like a `--human` fold does: the run at
    // m2 is 1 (just m2), not 2. m1 sits before the reset boundary.
    //
    // (Contract change in slice 5a: pre-activation, the sentinel was transparent
    // to the run and this scenario counted 2. Activating the documented
    // extension point in `consecutive_msg_count` makes `waiting_user` a reset
    // boundary — see `crates/chatbotchat-core/src/storage.rs`.)
    storage
        .create_message_capped(&room.id, "a", None, "m1", now, false, CAP)
        .await
        .expect("ok")
        .expect("under cap");
    storage
        .create_message_typed(
            &room.id,
            "a",
            None,
            "",
            now,
            MessageType::WaitingUser,
            Some(Severity::High),
            Some("should I merge?"),
        )
        .await
        .expect("create sentinel");
    let m2 = storage
        .create_message_capped(&room.id, "a", None, "m2", now, false, CAP)
        .await
        .expect("ok")
        .expect("under cap");

    assert_eq!(
        storage
            .consecutive_msg_count(&room.id, m2.seq)
            .await
            .expect("count ok"),
        1,
        "a waiting_user sentinel resets the autonomous-turn run"
    );
}

#[tokio::test]
async fn waiting_user_sentinel_is_not_itself_counted_as_a_turn() {
    let storage = fresh_storage().await;
    let room = sample_room();
    storage.create_room(&room).await.expect("create_room ok");
    let now = OffsetDateTime::now_utc();
    const CAP: i64 = 10;

    // A sentinel followed by two autonomous turns. The sentinel resets the run,
    // and the two `msg` rows after it count — but the sentinel row itself is
    // never tallied as a turn (the count seam stays `type='msg'`). Run at m2 = 2.
    storage
        .create_message_typed(
            &room.id,
            "a",
            None,
            "",
            now,
            MessageType::WaitingUser,
            Some(Severity::Low),
            Some("hold on"),
        )
        .await
        .expect("create sentinel");
    storage
        .create_message_capped(&room.id, "a", None, "m1", now, false, CAP)
        .await
        .expect("ok")
        .expect("under cap");
    let m2 = storage
        .create_message_capped(&room.id, "a", None, "m2", now, false, CAP)
        .await
        .expect("ok")
        .expect("under cap");

    assert_eq!(
        storage
            .consecutive_msg_count(&room.id, m2.seq)
            .await
            .expect("count ok"),
        2,
        "the sentinel resets but is not itself counted; only the two msgs after it"
    );
}

#[tokio::test]
async fn create_message_capped_enforces_the_cap_atomically_and_honors_the_configured_value() {
    let storage = fresh_storage().await;
    let room = sample_room();
    storage.create_room(&room).await.expect("create_room ok");
    let now = OffsetDateTime::now_utc();

    // The enforcement reads an arbitrary cap value (not a hard-coded 10): a
    // cap of 2 admits exactly two messages.
    const CAP: i64 = 2;
    for i in 0..CAP {
        let inserted = storage
            .create_message_capped(&room.id, "sender", None, &format!("m{i}"), now, false, CAP)
            .await
            .expect("capped insert ok");
        assert!(
            inserted.is_some(),
            "send {i} is under the cap and must be inserted"
        );
    }

    // The cap+1th is refused atomically (the count test + insert are one SQL
    // statement, so there is no read-then-write window) and nothing is written.
    let refused = storage
        .create_message_capped(&room.id, "sender", None, "over", now, false, CAP)
        .await
        .expect("capped insert ok");
    assert!(refused.is_none(), "a send at the cap must be refused");
    assert_eq!(
        storage
            .count_capped_messages(&room.id)
            .await
            .expect("count ok"),
        CAP,
        "the refused message must not be persisted"
    );
}

// --- lifecycle storage seams (slice 6a) ---

fn room_with_id(id: &str) -> Room {
    let now = OffsetDateTime::now_utc();
    Room {
        id: id.into(),
        subject: "lifecycle".into(),
        started_at: now,
        last_activity_at: now,
        state: RoomState::Active,
        state_changed_at: now,
        config: RoomConfig::default(),
        prev_room_id: None,
    }
}

#[tokio::test]
async fn update_room_state_changes_state_and_records_a_transition_event() {
    let storage = fresh_storage().await;
    let room = room_with_id("life-1-20260530-0000");
    storage.create_room(&room).await.expect("create_room ok");

    let now = OffsetDateTime::now_utc();
    let changed = storage
        .update_room_state(&room.id, RoomState::Active, RoomState::Closed, now, None)
        .await
        .expect("update ok");
    assert!(changed, "precondition matched, so the write applies");

    let fetched = storage
        .get_room(&room.id)
        .await
        .expect("get ok")
        .expect("exists");
    assert_eq!(fetched.state, RoomState::Closed);
    assert_eq!(
        fetched.state_changed_at.unix_timestamp(),
        now.unix_timestamp(),
        "state_changed_at re-anchored on transition"
    );

    let events = storage.list_events(&room.id).await.expect("events ok");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, EventKind::Transition);
    assert_eq!(events[0].from_state, Some(RoomState::Active));
    assert_eq!(events[0].to_state, Some(RoomState::Closed));
}

#[tokio::test]
async fn update_room_state_is_conditional_on_the_from_state() {
    let storage = fresh_storage().await;
    let room = room_with_id("life-2-20260530-0000"); // starts Active
    storage.create_room(&room).await.expect("create_room ok");

    let now = OffsetDateTime::now_utc();
    // Precondition `Idle` does not match the actual `Active` state.
    let changed = storage
        .update_room_state(&room.id, RoomState::Idle, RoomState::Closed, now, None)
        .await
        .expect("update ok");
    assert!(!changed, "stale precondition must not clobber the row");

    let fetched = storage
        .get_room(&room.id)
        .await
        .expect("get ok")
        .expect("exists");
    assert_eq!(fetched.state, RoomState::Active, "state unchanged");
    assert!(
        storage
            .list_events(&room.id)
            .await
            .expect("events ok")
            .is_empty(),
        "no event written when the write did not apply"
    );
}

#[tokio::test]
async fn transition_into_archived_emits_an_archive_kind_event() {
    let storage = fresh_storage().await;
    let room = room_with_id("life-3-20260530-0000");
    storage.create_room(&room).await.expect("create_room ok");

    let now = OffsetDateTime::now_utc();
    storage
        .update_room_state(&room.id, RoomState::Active, RoomState::Closed, now, None)
        .await
        .expect("close ok");
    storage
        .update_room_state(&room.id, RoomState::Closed, RoomState::Archived, now, None)
        .await
        .expect("archive ok");

    let events = storage.list_events(&room.id).await.expect("events ok");
    assert_eq!(events.len(), 2, "one row per transition");
    // The first is the close; the second (archive) carries the hook kind.
    assert_eq!(events[0].kind, EventKind::Transition);
    assert_eq!(events[1].kind, EventKind::Archive);
    assert_eq!(events[1].to_state, Some(RoomState::Archived));
}

#[tokio::test]
async fn touch_last_activity_updates_the_column() {
    let storage = fresh_storage().await;
    let room = room_with_id("life-4-20260530-0000");
    storage.create_room(&room).await.expect("create_room ok");

    let later = OffsetDateTime::now_utc() + Duration::hours(3);
    storage
        .touch_last_activity(&room.id, later)
        .await
        .expect("touch ok");

    let fetched = storage
        .get_room(&room.id)
        .await
        .expect("get ok")
        .expect("exists");
    assert_eq!(
        fetched.last_activity_at.unix_timestamp(),
        later.unix_timestamp()
    );
}

#[tokio::test]
async fn list_rooms_for_sweep_excludes_archived() {
    let storage = fresh_storage().await;
    let live = room_with_id("sweep-live-20260530-0000");
    let dead = room_with_id("sweep-dead-20260530-0000");
    storage.create_room(&live).await.expect("create live");
    storage.create_room(&dead).await.expect("create dead");

    let now = OffsetDateTime::now_utc();
    storage
        .update_room_state(&dead.id, RoomState::Active, RoomState::Archived, now, None)
        .await
        .expect("archive dead");

    let rooms = storage.list_rooms_for_sweep().await.expect("sweep list ok");
    let ids: Vec<&str> = rooms.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&live.id.as_str()), "live room is swept");
    assert!(
        !ids.contains(&dead.id.as_str()),
        "archived room is excluded from the sweep"
    );
}

#[tokio::test]
async fn update_room_state_persists_the_detail_field() {
    let storage = fresh_storage().await;
    let room = room_with_id("life-5-20260530-0000");
    storage.create_room(&room).await.expect("create_room ok");

    let now = OffsetDateTime::now_utc();
    storage
        .update_room_state(
            &room.id,
            RoomState::Active,
            RoomState::Paused,
            now,
            Some("went to do real work"),
        )
        .await
        .expect("pause ok");

    let events = storage.list_events(&room.id).await.expect("events ok");
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].detail.as_deref(),
        Some("went to do real work"),
        "the pause reason must survive the events round-trip"
    );
}
