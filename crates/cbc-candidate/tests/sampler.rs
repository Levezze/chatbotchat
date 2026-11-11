//! C3: the sampler over real processes. A tick lists only candidate processes, by label, with
//! no command line or path, and the disk under the root; the file is private and new.
use cbc_candidate::{
    report::Samples,
    sampler::{sample, Mode, SampleArgs, Tick},
    store::Store,
    Sandbox,
};
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    process::{Command, Stdio},
    time::Duration,
};
use tempfile::tempdir;

struct Killed(std::process::Child);
impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn a_tick_lists_candidate_processes_by_label_and_the_disk_under_the_root() {
    let temp = tempdir().unwrap();
    let sandbox = Sandbox::initialize(
        &temp.path().join("candidate"),
        8991,
        &[temp.path().join("daily/state.db")],
    )
    .unwrap();
    let store = Store::open(&sandbox).await.unwrap();
    // A stand-in core: an executable named cbc-candidate, with a subcommand unique to this run.
    // Both stand-ins live under a directory with a space, as a checkout under "Application
    // Support" would: the rendered `ps` line cannot say where such a path ends.
    let bin = temp.path().join("with space/bin");
    fs::create_dir_all(&bin).unwrap();
    symlink("/bin/sleep", bin.join("cbc-candidate")).unwrap();
    let unique = (7000 + std::process::id() % 1000).to_string();
    let core = Killed(
        Command::new(bin.join("cbc-candidate"))
            .arg(&unique)
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    // A stand-in device router: Python running a script under cbc-candidate/adapters.
    let adapters = temp.path().join("with space/cbc-candidate/adapters/device");
    fs::create_dir_all(&adapters).unwrap();
    fs::write(adapters.join("router.py"), "import time\ntime.sleep(600)\n").unwrap();
    let router = Killed(
        Command::new("python3")
            .args(["-B", "-I"])
            .arg(adapters.join("router.py"))
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut file = None;
    let written = sample(
        &sandbox,
        SampleArgs {
            interval: Duration::from_millis(50),
            mode: Mode::Debug,
            ticks: Some(2),
        },
        |path| file = Some(path.to_path_buf()),
        std::future::pending::<()>(),
    )
    .await
    .unwrap();
    store.close().await;
    assert_eq!(written, 2);

    let path = file.unwrap();
    assert_eq!(path.parent().unwrap(), sandbox.root().join("samples"));
    let name = path.file_name().unwrap().to_str().unwrap().to_owned();
    assert!(
        name.starts_with("samples-") && name.ends_with("-debug.jsonl"),
        "{name}"
    );
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );

    let bytes = fs::read(&path).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(
        !text.contains(temp.path().to_str().unwrap()),
        "no paths: {text}"
    );
    assert!(!text.contains("python"), "no command lines: {text}");
    let ticks: Vec<Tick> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(ticks.len(), 2);
    assert!(ticks[0].wall_ms <= ticks[1].wall_ms);
    let label_of = |pid: u32| {
        ticks[0]
            .processes
            .iter()
            .find(|p| p.pid == pid)
            .map(|p| p.label.clone())
    };
    assert_eq!(
        label_of(core.0.id()),
        Some(format!("cbc-candidate {unique}"))
    );
    assert_eq!(
        label_of(router.0.id()).as_deref(),
        Some("adapters/device/router.py")
    );
    assert!(
        label_of(std::process::id()).is_none(),
        "the test process is not a candidate"
    );
    assert!(ticks
        .iter()
        .all(|t| t.mode == Mode::Debug && t.interval_ms == 50));
    assert!(ticks[0].disk.database_bytes > 0, "{:?}", ticks[0].disk);
    Samples::parse(&name, &bytes).unwrap();
}

#[test]
fn the_command_prints_its_file_and_how_many_ticks_it_wrote() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("candidate");
    Sandbox::initialize(&root, 8992, &[temp.path().join("daily/state.db")]).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_cbc-candidate"))
        .arg("sample")
        .arg("--root")
        .arg(&root)
        .arg("--protected-db")
        .arg(temp.path().join("daily/state.db"))
        .args(["--interval-s", "1", "--ticks", "1"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0]["status"], "sampling");
    let file = std::path::PathBuf::from(lines[0]["file"].as_str().unwrap());
    assert!(file
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .ends_with("-normal.jsonl"));
    assert_eq!(lines[1], serde_json::json!({"status":"stopped","ticks":1}));
    assert_eq!(fs::read_to_string(&file).unwrap().lines().count(), 1);
}
