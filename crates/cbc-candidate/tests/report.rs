//! C2: the trial report over real snapshots. It reads a manifest and an end snapshot and
//! prints protocol §8; the same inputs give the same bytes, and every fact is taken as it stood
//! at `until`, from its own timestamp, even when the end snapshot shows a later state.
#[path = "support/work_fixture.rs"]
mod work_fixture;
use cbc_candidate::{
    evidence::{sha256_file, take_snapshot, Snapshot},
    report::{report, ReportArgs},
    requests::RequestOwner,
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
