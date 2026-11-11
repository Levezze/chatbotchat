//! C2: the trial report over real snapshots. It reads a manifest and an end snapshot and
//! prints protocol §8; the same inputs give the same bytes, and every fact is taken as it stood
//! at `until`, from its own timestamp, even when the end snapshot shows a later state.
#[path = "support/work_fixture.rs"]
mod work_fixture;
use cbc_candidate::{
    bindings::SessionBinding,
    evidence::{sha256_file, take_snapshot, Snapshot},
    report::{report, ReportArgs},
    requests::RequestOwner,
    sampler::{Disk, Mode, Process, Tick, SAMPLE_VERSION},
};
use std::time::Duration;
use work_fixture::{drain, fixture, Case};

async fn snap(case: &Case) -> Snapshot {
    // Keep each snapshot in a millisecond of its own, so window edges are unambiguous.
    tokio::time::sleep(Duration::from_millis(5)).await;
    let snapshot = take_snapshot(&case.store, case._sandbox.root())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    snapshot
}

/// The fields the report reads from a manifest, as C1 writes them.
fn manifest(baseline: &Snapshot, sha256: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "trial": 1,
        "build": {"commit": "test-build"},
        "snapshot": {
            "file": baseline.file,
            "taken_wall_ms": baseline.taken_wall_ms,
            "sha256": sha256,
        },
    }))
    .unwrap()
}

async fn run(
    case: &Case,
    manifest: &[u8],
    end: &Snapshot,
    until: Option<i64>,
    tally: Option<&[u8]>,
) -> anyhow::Result<String> {
    report(
        &case._sandbox,
        ReportArgs {
            manifest_name: "trial-1.json",
            manifest,
            end: &end.file,
            tally: tally.map(|bytes| ("tally.json", bytes)),
            samples: &[],
            from_ms: None,
            until_ms: until,
        },
    )
    .await
}

fn line<'a>(text: &'a str, prefix: &str) -> &'a str {
    text.lines()
        .find(|l| l.starts_with(prefix))
        .unwrap_or_else(|| panic!("no line starting {prefix:?} in:\n{text}"))
}

/// committed and receipted counts from the delivery line.
fn delivered(text: &str) -> (u64, u64) {
    let line = line(text, "  committed=");
    let field = |name: &str| -> u64 {
        line.split_whitespace()
            .find_map(|w| w.strip_prefix(name))
            .unwrap()
            .parse()
            .unwrap()
    };
    (field("committed="), field("receipted="))
}

#[tokio::test]
async fn the_report_reads_state_at_until_from_timestamps_not_the_end_state() {
    let case = fixture().await;
    let base = snap(&case).await;
    let manifest = manifest(&base, &sha256_file(&base.file).unwrap());

    case.store
        .send(&case.lead, &case.report_room, "k1", "start on the parser")
        .await
        .unwrap();
    drain(&case.store, &case.worker).await;
    case.store
        .record_activity(&case.worker, "work")
        .await
        .unwrap();
    let request = case
        .store
        .open_request(
            &case.worker,
            &RequestOwner::Member("lead".into()),
            None,
            "Which branch?",
            "q1",
        )
        .await
        .unwrap()
        .request;
    let mid = snap(&case).await;
    case.store
        .answer_request(&case.lead, &request, "main", "a1")
        .await
        .unwrap();
    drain(&case.store, &case.worker).await;
    drain(&case.store, &case.lead).await;
    let end = snap(&case).await;

    // At the end the request is answered and every delivery receipted.
    let text = run(&case, &manifest, &end, None, None).await.unwrap();
    let primarch = line(&text, "  project/worker — waiting on the Primarch:");
    assert!(primarch.contains("count=1 open=0"), "{primarch}");
    let (committed, receipted) = delivered(&text);
    assert!(committed >= 2, "the message and the request events: {text}");
    assert_eq!(receipted, committed);
    line(&text, "  project/worker — waiting on delivery: count=");
    line(&text, "  project/worker — ready but idle: count=");

    // The same end snapshot, cut at `mid`: the answer is in the file but after `until`, so the
    // request is still open, and the answer's event is not yet committed.
    let early = run(&case, &manifest, &end, Some(mid.taken_wall_ms), None)
        .await
        .unwrap();
    let primarch = line(&early, "  project/worker — waiting on the Primarch:");
    assert!(primarch.contains("count=1 open=1"), "{primarch}");
    assert!(delivered(&early).0 < committed);
    assert!(early.contains("(open at until)"));

    // Cut at the baseline: nothing in the trial had happened yet. The lead's receipt of the
    // room introduction came before the baseline, and it has done nothing since, so its wait
    // is carried in, clipped to the empty window and still open; the worker had not been
    // asked anything.
    let none = run(&case, &manifest, &end, Some(base.taken_wall_ms), None)
        .await
        .unwrap();
    assert_eq!(delivered(&none), (0, 0));
    assert!(!none.contains("project/worker — waiting"), "{none}");
    let carried = line(&none, "  project/lead — ready but idle: ");
    assert!(carried.contains("count=1 open=1 sum=0 ms"), "{carried}");
    line(&none, "  wake attempts: none reported in the window");
}

#[tokio::test]
async fn a_withdrawal_ends_the_wait_when_it_happened_even_without_an_inbox_event() {
    let case = fixture().await;
    let base = snap(&case).await;
    let manifest = manifest(&base, &sha256_file(&base.file).unwrap());
    // Neither withdrawal emits an inbox event: the user has no room, and retirement closes
    // the owner's rooms in the same transaction.
    let to_user = case
        .store
        .open_request(&case.worker, &RequestOwner::User, None, "Ship it?", "q1")
        .await
        .unwrap()
        .request;
    case.store
        .open_request(
            &case.worker,
            &RequestOwner::Member("lead".into()),
            None,
            "Which port?",
            "q2",
        )
        .await
        .unwrap();
    let asked = snap(&case).await;
    case.store
        .withdraw_request(&case.worker, &to_user, "not needed")
        .await
        .unwrap();
    case.store
        .retire_member("project", "lead", "trial seat ended")
        .await
        .unwrap();
    let end = snap(&case).await;

    let open = |text: &str, cause: &str| -> String {
        line(text, &format!("  project/worker — {cause}: ")).to_owned()
    };
    let before = run(&case, &manifest, &end, Some(asked.taken_wall_ms), None)
        .await
        .unwrap();
    assert!(open(&before, "waiting on the user").contains("count=1 open=1"));
    assert!(open(&before, "waiting on the Primarch").contains("count=1 open=1"));
    let after = run(&case, &manifest, &end, None, None).await.unwrap();
    assert!(
        open(&after, "waiting on the user").contains("count=1 open=0"),
        "{after}"
    );
    assert!(open(&after, "waiting on the Primarch").contains("count=1 open=0"));
}

#[tokio::test]
async fn a_coalesced_room_notice_is_superseded_not_pending() {
    let case = fixture().await;
    let base = snap(&case).await;
    let manifest = manifest(&base, &sha256_file(&base.file).unwrap());
    // A notice to the lead every two messages. The second notice replaces the first while it
    // is unread, so the first is never offered or receipted: it is superseded, not pending,
    // and the lead is not still waiting on it.
    for (n, key) in ["k1", "k2", "k3", "k4"].iter().enumerate() {
        case.store
            .send_with_notice_interval(&case.lead, &case.report_room, key, &format!("m{n}"), 2)
            .await
            .unwrap();
    }
    drain(&case.store, &case.worker).await;
    drain(&case.store, &case.lead).await;
    let end = snap(&case).await;

    let text = run(&case, &manifest, &end, None, None).await.unwrap();
    let delivery = line(&text, "  committed=");
    assert!(delivery.contains(" superseded=1 "), "{text}");
    assert_eq!(
        line(&text, "  pending at until: "),
        "  pending at until: never offered=0 offered, not receipted=0"
    );
    let waiting = line(&text, "  project/lead — waiting on delivery: ");
    assert!(waiting.contains("count=2 open=0 "), "{waiting}");
}

async fn notice_every_two(case: &Case, who: &SessionBinding, key: &str) {
    case.store
        .send_with_notice_interval(who, &case.report_room, key, key, 2)
        .await
        .unwrap();
}

/// A notice offered, then re-offered in a new batch after a rebind, is still held when a newer
/// notice arrives: it is pending, not superseded. Once a further rebind drops that batch unreceipted
/// it is replaced, at a time no record gives.
#[tokio::test]
async fn a_notice_held_in_a_batch_across_a_handoff_stays_pending() {
    let case = fixture().await;
    let base = snap(&case).await;
    let manifest = manifest(&base, &sha256_file(&base.file).unwrap());
    let send = |who, key| notice_every_two(&case, who, key);
    send(&case.lead, "k1").await;
    send(&case.lead, "k2").await;
    assert!(case.store.check(&case.lead, 32).await.unwrap().is_some());
    let lead = case
        .store
        .bind_member("project", "lead", "device", "lead-session-2")
        .await
        .unwrap();
    assert!(
        case.store.check(&lead, 32).await.unwrap().is_some(),
        "re-offered"
    );
    let offered = snap(&case).await;
    send(&lead, "k3").await;
    send(&lead, "k4").await;
    drain(&case.store, &case.worker).await;
    let held = snap(&case).await;

    let text = run(&case, &manifest, &held, None, None).await.unwrap();
    assert!(
        line(&text, "  committed=").contains(" superseded=0 "),
        "{text}"
    );
    assert_eq!(
        line(&text, "  pending at until: "),
        "  pending at until: never offered=1 offered, not receipted=1"
    );
    assert!(line(&text, "  project/lead — waiting on delivery: ").contains(" open=2 "));

    case.store
        .bind_member("project", "lead", "device", "lead-session-3")
        .await
        .unwrap();
    let dropped = snap(&case).await;
    let text = run(&case, &manifest, &dropped, None, None).await.unwrap();
    assert_eq!(
        line(&text, "  pending at until: "),
        "  pending at until: never offered=1 offered, not receipted=0"
    );
    line(&text, "  superseded at an unknown time: 1 ");
    assert!(line(&text, "  project/lead — waiting on delivery: ").contains(" open=1 "));

    // Cut before the newer notice, the dropped notice was still pending, beside the worker's two
    // unread messages: the later snapshot must say what the earlier one said.
    for end in [&held, &dropped] {
        let text = run(&case, &manifest, end, Some(offered.taken_wall_ms), None)
            .await
            .unwrap();
        assert_eq!(
            line(&text, "  pending at until: "),
            "  pending at until: never offered=2 offered, not receipted=1",
            "{text}"
        );
        assert!(!text.contains("unknown time"), "{text}");
    }
}

#[tokio::test]
async fn the_report_is_byte_identical_names_files_only_and_prints_the_tally() {
    let case = fixture().await;
    let base = snap(&case).await;
    let manifest = manifest(&base, &sha256_file(&base.file).unwrap());
    case.store
        .send(&case.lead, &case.report_room, "k1", "hello")
        .await
        .unwrap();
    let end = snap(&case).await;
    let tally = br#"{"project":{"worker":{"cbc":2,"other":0}}}"#;

    let first = run(&case, &manifest, &end, None, Some(tally))
        .await
        .unwrap();
    let second = run(&case, &manifest, &end, None, Some(tally))
        .await
        .unwrap();
    assert_eq!(first, second, "same inputs, same bytes");

    let root = case._sandbox.root().to_str().unwrap();
    assert!(!first.contains(root), "no absolute path in a public report");
    let base_name = base.file.file_name().unwrap().to_str().unwrap();
    assert!(line(&first, "Baseline snapshot: ").contains(base_name));
    assert!(line(&first, "Trial 1 — build test-build").contains("result: missing"));
    assert_eq!(
        line(&first, "  project/worker: "),
        "  project/worker: cbc=2 native_setup=missing (not in the tally) \
         native_refusal=missing (not in the tally) other=0"
    );
    assert!(line(&first, "  project/lead: ").contains("cbc=missing (not in the tally)"));
    // One unreceipted message, offered or not depending on whether the worker checked.
    assert_eq!(delivered(&first), (1, 0));
}

#[tokio::test]
async fn the_report_refuses_inputs_it_cannot_vouch_for() {
    let case = fixture().await;
    let base = snap(&case).await;
    let good = manifest(&base, &sha256_file(&base.file).unwrap());
    let end = snap(&case).await;

    // A baseline that is not what the manifest hashed.
    let wrong = manifest(&base, &"0".repeat(64));
    let refused = run(&case, &wrong, &end, None, None).await.unwrap_err();
    assert!(refused.to_string().contains("does not match"), "{refused}");

    // A window past the end snapshot, which cannot show anything later.
    let refused = run(&case, &good, &end, Some(end.taken_wall_ms + 1), None)
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("after the end snapshot"),
        "{refused}"
    );

    // An end snapshot older than the baseline.
    let later = manifest(&end, &sha256_file(&end.file).unwrap());
    let refused = run(&case, &later, &base, None, None).await.unwrap_err();
    assert!(refused.to_string().contains("predates"), "{refused}");

    // A manifest naming a file that is not a candidate snapshot.
    let mut outside = base.clone();
    outside.file = case._sandbox.database_path();
    let refused = run(
        &case,
        &manifest(&outside, &sha256_file(&outside.file).unwrap()),
        &end,
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(
        refused.to_string().contains("not a candidate snapshot"),
        "{refused}"
    );

    // A tally that names a seat the snapshot does not have.
    let refused = run(
        &case,
        &good,
        &end,
        None,
        Some(br#"{"project":{"ghost":{"cbc":1}}}"#),
    )
    .await
    .unwrap_err();
    assert!(refused.to_string().contains("unknown seat"), "{refused}");
}

#[tokio::test]
async fn the_command_takes_bare_file_names_from_inside_the_snapshot_directory() {
    let case = fixture().await;
    let base = snap(&case).await;
    let end = snap(&case).await;
    let dir = base.file.parent().unwrap().to_path_buf();
    std::fs::write(
        dir.join("trial-1.json"),
        manifest(&base, &sha256_file(&base.file).unwrap()),
    )
    .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cbc-candidate"))
        .current_dir(&dir)
        .arg("report")
        .arg("--root")
        .arg(case._sandbox.root())
        .arg("--protected-db")
        .arg(case._temp.path().join("daily/state.db"))
        .args(["--manifest", "trial-1.json", "--snapshot"])
        .arg(end.file.file_name().unwrap())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with("Trial 1 — build test-build"), "{text}");
    assert!(line(&text, "End snapshot: ").contains(end.file.file_name().unwrap().to_str().unwrap()));
}

#[tokio::test]
async fn the_cost_section_reads_the_sampler_file_cut_on_the_window() {
    let case = fixture().await;
    let base = snap(&case).await;
    let manifest = manifest(&base, &sha256_file(&base.file).unwrap());
    let end = snap(&case).await;
    let tick = |wall_ms: i64, cpu_ms: u64| Tick {
        v: SAMPLE_VERSION,
        wall_ms,
        mode: Mode::Normal,
        interval_ms: 10_000,
        processes: vec![Process {
            label: "cbc-candidate serve".into(),
            pid: 41,
            rss_kb: 2048,
            cpu_ms,
        }],
        disk: Disk {
            database_bytes: 4096,
            snapshots_bytes: 0,
            other_bytes: 512,
        },
    };
    // One tick before the window, two inside it, one after the end snapshot.
    let mut samples = Vec::new();
    for t in [
        tick(base.taken_wall_ms - 1, 0),
        tick(base.taken_wall_ms, 100),
        tick(end.taken_wall_ms, 150),
        tick(end.taken_wall_ms + 1, 900),
    ] {
        samples.extend(serde_json::to_vec(&t).unwrap());
        samples.push(b'\n');
    }
    let text = report(
        &case._sandbox,
        ReportArgs {
            manifest_name: "trial-1.json",
            manifest: &manifest,
            end: &end.file,
            tally: None,
            samples: &[("samples-1-normal.jsonl", &samples)],
            from_ms: None,
            until_ms: None,
        },
    )
    .await
    .unwrap();
    assert!(line(&text, "Samples: ").starts_with("Samples: samples-1-normal.jsonl, sha256 "));
    line(&text, "Cost (sampler ticks in the window)");
    let span = end.taken_wall_ms - base.taken_wall_ms;
    assert!(
        line(&text, "  normal: ").contains(&format!("ticks=2 every 10000 ms over {span} ms")),
        "{text}"
    );
    let core = line(&text, "  normal — cbc-candidate serve: ");
    assert!(core.contains("processes max=1 expected=1"), "{core}");
    assert!(
        core.contains("(50 ms CPU)"),
        "only growth inside the window: {core}"
    );
    line(&text, "  debug: missing (no debug-mode ticks in the window");
    assert!(
        line(&text, "  normal — disk bytes: ").contains("database start=4096 end=4096 max=4096")
    );

    // A file that is not sampler output is refused, naming the line, not echoing it.
    let refused = report(
        &case._sandbox,
        ReportArgs {
            manifest_name: "trial-1.json",
            manifest: &manifest,
            end: &end.file,
            tally: None,
            samples: &[("bad.jsonl", &b"secret command line\n"[..])],
            from_ms: None,
            until_ms: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(refused.to_string(), "samples line 1: not a sample tick");
}

#[tokio::test]
async fn normal_and_debug_runs_are_read_from_their_own_files_into_one_cost_section() {
    let case = fixture().await;
    let base = snap(&case).await;
    let manifest = manifest(&base, &sha256_file(&base.file).unwrap());
    let end = snap(&case).await;
    let (from, until) = (base.taken_wall_ms, end.taken_wall_ms);
    let file = |mode: Mode, walls: [i64; 2]| {
        let mut bytes = Vec::new();
        for (i, wall_ms) in walls.into_iter().enumerate() {
            let tick = Tick {
                v: SAMPLE_VERSION,
                wall_ms,
                mode,
                interval_ms: 1,
                processes: vec![Process {
                    label: "cbc-candidate serve".into(),
                    pid: 41,
                    rss_kb: 2048,
                    cpu_ms: 10 * i as u64,
                }],
                disk: Disk::default(),
            };
            bytes.extend(serde_json::to_vec(&tick).unwrap());
            bytes.push(b'\n');
        }
        bytes
    };
    let split = from + (until - from) / 2;
    let normal = file(Mode::Normal, [from, split]);
    let debug = file(Mode::Debug, [split + 1, until]);
    let run = |samples: Vec<(&'static str, Vec<u8>)>| {
        let (manifest, end) = (&manifest, &end.file);
        let sandbox = &case._sandbox;
        async move {
            let samples: Vec<(&str, &[u8])> =
                samples.iter().map(|(n, b)| (*n, b.as_slice())).collect();
            report(
                sandbox,
                ReportArgs {
                    manifest_name: "trial-1.json",
                    manifest,
                    end,
                    tally: None,
                    samples: &samples,
                    from_ms: None,
                    until_ms: None,
                },
            )
            .await
        }
    };
    // Given in either order, both files are named and both modes measured.
    let text = run(vec![
        ("debug.jsonl", debug.clone()),
        ("normal.jsonl", normal.clone()),
    ])
    .await
    .unwrap();
    let names: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("Samples: "))
        .collect();
    assert_eq!(names.len(), 2, "{text}");
    assert!(
        names[0].starts_with("Samples: normal.jsonl, sha256 "),
        "{text}"
    );
    assert!(
        names[1].starts_with("Samples: debug.jsonl, sha256 "),
        "{text}"
    );
    assert!(line(&text, "  normal: ").contains("ticks=2"), "{text}");
    assert!(line(&text, "  debug: ").contains("ticks=2"), "{text}");
    assert!(
        line(&text, "  debug — cbc-candidate serve: ").contains("(10 ms CPU)"),
        "{text}"
    );

    // Two samplers running at once would count every process twice: refused.
    let overlapping = file(Mode::Debug, [from, until]);
    let error = run(vec![("normal.jsonl", normal), ("debug.jsonl", overlapping)])
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "samples normal.jsonl and debug.jsonl overlap in time: one sampler at a time"
    );
}
