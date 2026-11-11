use super::*;

fn at(instance: &str, ms: i64) -> Stamp {
    Stamp {
        instance: instance.into(),
        mono_us: ms * 1000,
        wall_ms: ms,
    }
}

fn seat(member: &str, role: &str, host: &str) -> Seat {
    Seat {
        context: "t".into(),
        member: member.into(),
        role: role.into(),
        active: true,
        retired: false,
        epoch: 1,
        session: Some(format!("{member}-native")),
        host: Some(host.into()),
    }
}

fn delivery(member: &str, sequence: i64, committed: i64) -> Delivery {
    Delivery {
        context: "t".into(),
        member: member.into(),
        sequence,
        committed: at("a", committed),
        offered: None,
        receipted: None,
        woken: None,
        active: None,
    }
}

#[test]
fn nearest_rank_distribution() {
    assert_eq!(distribution(&[]), "n=0");
    assert_eq!(distribution(&[7]), "n=1 median=7 p90=7 max=7");
    // ranks: p50 -> ceil(5)=5th, p90 -> ceil(9)=9th
    let ten: Vec<i64> = (1..=10).rev().collect();
    assert_eq!(distribution(&ten), "n=10 median=5 p90=9 max=10");
    // p50 of 3 -> ceil(1.5)=2nd; p90 -> ceil(2.7)=3rd
    assert_eq!(distribution(&[30, 10, 20]), "n=3 median=20 p90=30 max=30");
}

#[test]
fn union_merges_overlaps_and_touching_spans() {
    assert_eq!(union_length(&[]), 0);
    assert_eq!(union_length(&[(0, 10), (5, 15), (15, 20), (30, 31)]), 21);
}

#[test]
fn clipping_keeps_only_the_window() {
    let w = Window {
        from_ms: 100,
        until_ms: 200,
    };
    assert_eq!(clip(w, 50, Some(90)), None, "ended before the window");
    assert_eq!(clip(w, 201, None), None, "started after until");
    assert_eq!(clip(w, 50, Some(150)), Some((100, 150, false)));
    assert_eq!(
        clip(w, 150, Some(250)),
        Some((150, 200, true)),
        "closes after until"
    );
    assert_eq!(clip(w, 150, None), Some((150, 200, true)));
    assert_eq!(clip(w, 200, Some(200)), Some((200, 200, false)));
}

/// State at `until` comes from timestamps: an answer after `until` leaves the request open,
/// a receipt after `until` leaves the item pending and opens no ready-idle wait.
#[test]
fn waits_take_state_at_until_from_timestamps() {
    let mut d = delivery("feature", 5, 120);
    d.offered = Some(at("a", 125));
    d.receipted = Some(at("a", 180));
    d.active = Some(at("a", 190));
    let facts = Facts {
        seats: vec![
            seat("lead", "repo_lead", "codex"),
            seat("feature", "feature_lead", "claude"),
        ],
        deliveries: vec![d],
        requests: vec![RequestFact {
            context: "t".into(),
            request: "q1".into(),
            callback: "feature".into(),
            owner_is_user: false,
            owner_member: Some("lead".into()),
            created_wall_ms: 110,
            answered_wall_ms: Some(170),
            withdrawn_wall_ms: None,
        }],
        ..Facts::default()
    };
    let early = waits(
        &facts,
        Window {
            from_ms: 100,
            until_ms: 160,
        },
    );
    assert_eq!(
        early,
        vec![
            WaitInterval {
                seat: "t/feature".into(),
                cause: Some(WaitCause::Primarch),
                id: "request q1".into(),
                start_ms: 110,
                end_ms: 160,
                open: true,
            },
            WaitInterval {
                seat: "t/feature".into(),
                cause: Some(WaitCause::Delivery),
                id: "item 5".into(),
                start_ms: 120,
                end_ms: 160,
                open: true,
            },
        ]
    );
    let late = waits(
        &facts,
        Window {
            from_ms: 100,
            until_ms: 200,
        },
    );
    let causes: Vec<_> = late
        .iter()
        .map(|w| (w.cause, w.duration(), w.open))
        .collect();
    assert_eq!(
        causes,
        vec![
            (Some(WaitCause::Primarch), 60, false),
            (Some(WaitCause::Delivery), 60, false),
            (Some(WaitCause::ReadyIdle), 10, false),
        ]
    );
}

#[test]
fn a_request_to_an_unclassified_owner_is_unknown_and_the_user_is_known() {
    let mut facts = Facts {
        seats: vec![seat("helper", "participant", "claude")],
        ..Facts::default()
    };
    for (id, owner) in [("a", Some("helper")), ("b", None)] {
        facts.requests.push(RequestFact {
            context: "t".into(),
            request: id.into(),
            callback: "x".into(),
            owner_is_user: owner.is_none(),
            owner_member: owner.map(str::to_owned),
            created_wall_ms: 0,
            answered_wall_ms: None,
            withdrawn_wall_ms: Some(5),
        });
    }
    let w = waits(
        &facts,
        Window {
            from_ms: 0,
            until_ms: 10,
        },
    );
    assert_eq!(w[0].cause, None);
    assert_eq!(w[1].cause, Some(WaitCause::User));
    assert!(w.iter().all(|w| w.end_ms == 5 && !w.open));
}

/// Mono time within one instance; wall time across instances, counted as an estimate.
#[test]
fn latency_prefers_monotonic_time() {
    let same = Stamp {
        instance: "a".into(),
        mono_us: 1_500,
        wall_ms: 999_999,
    };
    assert_eq!(at("a", 1).until(&same), (0, false));
    assert_eq!(at("a", 1).until(&at("b", 41)), (40, true));
}

fn header() -> Header {
    Header {
        trial: 1,
        build_commit: "abc".into(),
        manifest_name: "trial-1.json".into(),
        manifest_sha256: "m".into(),
        baseline_name: "snapshot-100-x.sqlite3".into(),
        baseline_sha256: "b".into(),
        end_name: "snapshot-200-y.sqlite3".into(),
        end_sha256: "e".into(),
    }
}

/// The whole template, byte for byte, from hand-built facts.
#[test]
fn the_report_is_the_section_8_template_in_order() {
    let mut first = delivery("feature", 5, 120);
    first.offered = Some(at("a", 125));
    first.receipted = Some(at("a", 130));
    first.woken = Some(at("a", 128));
    first.active = Some(at("b", 150));
    let mut second = delivery("lead", 6, 140);
    second.offered = Some(at("a", 141));
    let third = delivery("lead", 7, 150);
    let facts = Facts {
        seats: vec![
            seat("feature", "feature_lead", "claude"),
            seat("lead", "repo_lead", "codex"),
        ],
        deliveries: vec![first, second, third],
        wakes: vec![
            WakeFact {
                kind: "claude_channel".into(),
                failure: None,
                reported_wall_ms: 128,
            },
            WakeFact {
                kind: "codex_queue".into(),
                failure: Some("timeout".into()),
                reported_wall_ms: 142,
            },
            WakeFact {
                kind: "codex_queue".into(),
                failure: None,
                reported_wall_ms: 999,
            },
        ],
        identity: vec![IdentityFact {
            context: "t".into(),
            member: "feature".into(),
            kind: "rebound".into(),
            epoch: 2,
            at_wall_ms: 160,
        }],
        connections: vec![
            ConnectionFact {
                context: "t".into(),
                first: "feature".into(),
                second: "lead".into(),
                room: "r1".into(),
                supersedes: None,
                introduced_wall_ms: Some(101),
            },
            ConnectionFact {
                context: "t".into(),
                first: "lead".into(),
                second: "feature".into(),
                room: "r2".into(),
                supersedes: None,
                introduced_wall_ms: Some(102),
            },
            ConnectionFact {
                context: "t".into(),
                first: "feature".into(),
                second: "lead".into(),
                room: "r3".into(),
                supersedes: Some("r1".into()),
                introduced_wall_ms: Some(103),
            },
        ],
        sent: vec![SentFact {
            context: "t".into(),
            sender: "lead".into(),
            wall_ms: 150,
        }],
        merges: vec![170, 250],
        ..Facts::default()
    };
    let tally = Tally::parse(
        "tally.json",
        br#"{"t":{"feature":{"cbc":0,"native_setup":2}}}"#,
        &facts.seats,
    )
    .unwrap();
    let window = Window {
        from_ms: 100,
        until_ms: 200,
    };
    let text = render(&header(), &facts, Some(&tally), window);
    let expected = format!(
        "Trial 1 — build abc — result: missing (recorded by the trial manager)
Window: 100 .. 200 wall ms (100 ms)
Manifest: trial-1.json, sha256 m
Baseline snapshot: snapshot-100-x.sqlite3, sha256 b (verified against the manifest)
End snapshot: snapshot-200-y.sqlite3, sha256 e
Tally: tally.json, sha256 {tally_sha}

Phase 1: missing (judged by the trial manager against protocol section 4)

Waits (wall time, clipped to the window)
  t/feature — waiting on delivery: count=1 open=0 total=10 ms n=1 median=10 p90=10 max=10 share=10.0%
  t/feature — ready but idle: count=1 open=0 total=20 ms n=1 median=20 p90=20 max=20 share=20.0%
  t/lead — waiting on delivery: count=2 open=2 total=110 ms n=2 median=50 p90=60 max=60 share=60.0%
  waiting on human review, by policy: partial — a PR observation saying the PR is ready (the HITL flag exists)
  waiting on the Primarch: partial — holds (#114); open requests to the Primarch are derived
  waiting on a resource: missing (holds on named resources (#114))
  waiting on CI: missing (PR observations with check state on the current head)
  ready but idle: partial — holds (#114): receipt to first activity is derived, not whether anything else blocked the member
  Ten longest:
    60 ms — t/lead — waiting on delivery — item 6 — +40 ms .. +100 ms (open at until)
    50 ms — t/lead — waiting on delivery — item 7 — +50 ms .. +100 ms (open at until)
    20 ms — t/feature — ready but idle — item 5 — +30 ms .. +50 ms
    10 ms — t/feature — waiting on delivery — item 5 — +20 ms .. +30 ms

Approval prompts (per session × kind)
  t/feature: cbc=0 native_setup=2 native_refusal=missing (not in the tally) other=missing (not in the tally)
  t/lead: cbc=missing (not in the tally) native_setup=missing (not in the tally) native_refusal=missing (not in the tally) other=missing (not in the tally)

Delivery (expected recipients of items committed in the window)
  committed=3 receipted=1 failed=missing (no delivery-failure record)
  pending at until: never offered=1 offered, not receipted=1
  wake attempts claude_channel: accepted=1 failed=[]
  wake attempts codex_queue: accepted=0 failed=[timeout=1]

Wake latency (monotonic within a core instance; across instances wall time, counted as estimates)
  claude commit to wake accepted: n=1 median=8 p90=8 max=8 missing=0 estimates=0
  claude commit to first activity: n=1 median=30 p90=30 max=30 missing=0 estimates=1
  codex commit to wake accepted: n=0 missing=2 estimates=0
  codex commit to first activity: n=0 missing=2 estimates=0

Autonomy
  merges recorded on work items: 1 (from work events; an item with no report room is not seen)
  merges without a Primarch yes: missing (needs PR observations and message classification)
  routine permission messages: missing (needs PR observations and message classification)
  go-ahead requests: missing (needs PR observations and message classification)

Rooms and identity
  duplicate peer rooms: 1 [t/feature+lead: 2 rooms]
  identity changes (rebinds): 1 [t/feature epoch 2 at +60 ms]
  enrolled in the window: 0
  retired in the window: 0
  legacy skills loaded: missing (the core does not see skills)

Primarch
  interventions by kind: missing (no intervention record)
  turns per hour: missing (the core sees CBC items, not turns)
  t/lead: 1 CBC items sent in 0.000 h

Cost: missing (no sampler file; the sampler lands in C3)

Outcome: missing (written by the trial manager)

Findings: missing (written by the trial manager)

Changes for the next trial: missing (written by the trial manager)
",
        tally_sha = tally.sha256
    );
    assert_eq!(text, expected);
    assert_eq!(render(&header(), &facts, Some(&tally), window), text);
}

#[test]
fn only_a_stage_change_to_merged_is_a_merge() {
    let event = |changed: &str, stage: &str| {
        format!(
            r#"{{"item":"w","revision":4,"actor":"m","changed":{changed},"stage":"{stage}","hitl":false}}"#
        )
    };
    assert_eq!(merge_revision(&event(r#"["stage"]"#, "merged")), Some(4));
    assert_eq!(merge_revision(&event(r#"["next_action"]"#, "merged")), None);
    assert_eq!(merge_revision(&event(r#"["stage"]"#, "in_review")), None);
    assert_eq!(merge_revision("not json"), None);
}

#[test]
fn a_tally_must_name_known_seats_and_kinds() {
    let seats = [seat("lead", "repo_lead", "codex")];
    assert!(Tally::parse("x", br#"{"t":{"ghost":{"cbc":1}}}"#, &seats).is_err());
    assert!(Tally::parse("x", br#"{"t":{"lead":{"clicks":1}}}"#, &seats).is_err());
    assert!(Tally::parse("x", br#"{"t":{"lead":{"cbc":-1}}}"#, &seats).is_err());
    assert!(Tally::parse("x", br#"{"t/lead":{"cbc":3}}"#, &seats).is_err());
    assert!(Tally::parse("x", br#"{"t":{"lead":{"cbc":3}}}"#, &seats).is_ok());
}
