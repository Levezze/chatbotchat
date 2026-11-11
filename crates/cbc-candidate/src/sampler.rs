//! C3: the cost sampler. One process, run beside the core for the length of a trial, that
//! records CPU and memory for the candidate's own processes and the disk its root uses.
//!
//! It reads `ps` for the current user only and keeps a line only when its command is the
//! candidate: the `cbc-candidate` binary, or a Python adapter under `cbc-candidate/adapters/`.
//! A line that does not mention `cbc-candidate` is dropped unread. For one that does, the exact
//! argument vector is read from the kernel, because a rendered `ps` line cannot say where a path
//! with a space ends. No command line is ever stored or printed, because `ps` also lists the
//! daily CBC and other agents' sessions. A process is stored as a label,
//! its PID (needed to take CPU deltas across ticks; the report never prints it), resident
//! memory and cumulative CPU time.
use crate::Sandbox;
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

/// The sample line format version.
pub const SAMPLE_VERSION: u64 = 1;
const SAMPLE_DIR: &str = "samples";

/// The logging mode: the runtime settings' live log level. Each tick carries the level in force
/// when it was taken, so the report keeps normal and debug apart even within one file.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Normal,
    Debug,
}

impl Mode {
    pub const ALL: [Mode; 2] = [Mode::Normal, Mode::Debug];
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "normal",
            Mode::Debug => "debug",
        }
    }
}

impl std::str::FromStr for Mode {
    type Err = anyhow::Error;
    fn from_str(text: &str) -> Result<Self> {
        match text {
            "normal" => Ok(Mode::Normal),
            "debug" => Ok(Mode::Debug),
            _ => bail!("mode must be normal or debug"),
        }
    }
}

/// One candidate process at one tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Process {
    pub label: String,
    pub pid: u32,
    pub rss_kb: u64,
    /// Cumulative CPU time, user and system, since the process started.
    pub cpu_ms: u64,
}

/// Allocated bytes under the candidate root, as `du` counts them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Disk {
    /// The database file and its WAL and shared-memory files.
    pub database_bytes: u64,
    pub snapshots_bytes: u64,
    pub other_bytes: u64,
}

/// One line of a sample file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tick {
    pub v: u64,
    pub wall_ms: i64,
    pub mode: Mode,
    pub interval_ms: u64,
    pub processes: Vec<Process>,
    pub disk: Disk,
}

fn is_python(executable: &str) -> bool {
    let name = executable.rsplit('/').next().unwrap_or(executable);
    name.to_ascii_lowercase().starts_with("python")
}

/// `adapters/<dir>/<file>.py` when `path` names a script under the candidate's adapters.
fn adapter_label(path: &str) -> Option<String> {
    let (_, rest) = path.split_once("cbc-candidate/adapters/")?;
    let (dir, file) = rest.split_once('/')?;
    let plain = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    };
    let stem = file.strip_suffix(".py")?;
    (plain(dir) && plain(stem)).then(|| format!("adapters/{dir}/{file}"))
}

/// The label of a candidate process from its argument vector, or None for anything else.
///
/// Only the process that runs the code counts: the `cbc-candidate` executable itself, or a
/// Python interpreter whose script is a candidate adapter. A wrapper that merely names the
/// candidate in its arguments (a shell, `rtk proxy`, an editor) is not the candidate.
pub fn classify(argv: &[String]) -> Option<String> {
    let (executable, args) = argv.split_first()?;
    let mut args = args.iter().map(String::as_str);
    if executable.rsplit('/').next() == Some("cbc-candidate") {
        return Some(match args.next().filter(|w| !w.starts_with('-')) {
            Some(subcommand) => format!("cbc-candidate {subcommand}"),
            None => "cbc-candidate".into(),
        });
    }
    if is_python(executable) {
        let script = args.find(|w| !w.starts_with('-'))?;
        return adapter_label(script);
    }
    None
}

/// The argument vector in a macOS `KERN_PROCARGS2` buffer: the argument count, the executable
/// path, NUL padding, then the arguments, each NUL-terminated. The environment that follows is
/// never read.
pub fn parse_procargs2(buffer: &[u8]) -> Option<Vec<String>> {
    let argc = usize::try_from(i32::from_ne_bytes(buffer.get(..4)?.try_into().ok()?)).ok()?;
    let rest = &buffer[4..];
    let mut start = rest.iter().position(|&b| b == 0)?;
    while rest.get(start) == Some(&0) {
        start += 1;
    }
    let mut argv = Vec::with_capacity(argc);
    let mut pieces = rest[start..].split(|&b| b == 0);
    for _ in 0..argc {
        argv.push(String::from_utf8_lossy(pieces.next()?).into_owned());
    }
    Some(argv)
}

/// The exact argument vector of a process, or None when it has gone.
#[cfg(target_os = "macos")]
fn argv_of(pid: u32) -> Option<Vec<String>> {
    let mut argmax: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: the out-pointer and its size describe `argmax`.
    let status = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&mut argmax as *mut libc::c_int).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 || argmax <= 0 {
        return None;
    }
    let mut buffer = vec![0u8; usize::try_from(argmax).ok()?];
    let mut size = buffer.len();
    let mut mib = [
        libc::CTL_KERN,
        libc::KERN_PROCARGS2,
        libc::c_int::try_from(pid).ok()?,
    ];
    // SAFETY: the out-pointer and its size describe `buffer`.
    let status = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        return None;
    }
    parse_procargs2(&buffer[..size])
}

/// The exact argument vector of a process, or None when it has gone.
#[cfg(target_os = "linux")]
fn argv_of(pid: u32) -> Option<Vec<String>> {
    let bytes = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(&bytes);
    Some(
        bytes
            .split(|&b| b == 0)
            .map(|piece| String::from_utf8_lossy(piece).into_owned())
            .collect(),
    )
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn argv_of(_pid: u32) -> Option<Vec<String>> {
    None
}

/// Cumulative CPU time as `ps -o time` prints it: `[[dd-]hh:]mm:ss[.ff]`. macOS prints
/// minutes and hundredths (`2:28.70`), procps whole seconds (`01:02:03`, `1-02:03:04`).
pub fn parse_cpu_time(text: &str) -> Option<u64> {
    let (days, rest) = match text.split_once('-') {
        Some((days, rest)) => (days.parse::<u64>().ok()?, rest),
        None => (0, text),
    };
    let parts: Vec<&str> = rest.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let (seconds, larger) = parts.split_last()?;
    let mut minutes = 0u64;
    for part in larger {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        minutes = minutes * 60 + part.parse::<u64>().ok()?;
    }
    let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let mut millis = 0u64;
    for (i, digit) in fraction.bytes().take(3).enumerate() {
        millis += u64::from(digit - b'0') * [100, 10, 1][i];
    }
    Some(days * 86_400_000 + minutes * 60_000 + whole.parse::<u64>().ok()? * 1000 + millis)
}

/// The next whitespace-separated field of `rest`, advancing past it.
fn next_field<'a>(rest: &mut &'a str) -> Option<&'a str> {
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let (value, tail) = rest.split_at(end);
    *rest = tail.trim_start();
    (!value.is_empty()).then_some(value)
}

/// The candidate processes in `ps -o pid=,rss=,time=,args=` output, leaving out `own_pid` (the
/// sampler). A line that does not mention the candidate is skipped before any field is read.
/// For one that does, `argv_of` gives the exact argument vector to classify, or None for a
/// process that has gone. An error names only the line number and field, never the line, so
/// no other process's arguments reach the sampler's output.
pub fn parse_ps(
    output: &str,
    own_pid: u32,
    argv_of: impl Fn(u32) -> Option<Vec<String>>,
) -> Result<Vec<Process>> {
    let mut processes = Vec::new();
    for (index, line) in output.lines().enumerate() {
        let number = index + 1;
        if !line.contains("cbc-candidate") {
            continue;
        }
        let mut rest = line.trim_start();
        let (Some(pid), Some(rss), Some(time)) = (
            next_field(&mut rest),
            next_field(&mut rest),
            next_field(&mut rest),
        ) else {
            continue;
        };
        let pid: u32 = pid
            .parse()
            .ok()
            .with_context(|| format!("ps line {number}: bad pid"))?;
        if pid == own_pid {
            continue;
        }
        let Some(label) = argv_of(pid).as_deref().and_then(classify) else {
            continue;
        };
        processes.push(Process {
            label,
            pid,
            rss_kb: rss
                .parse()
                .ok()
                .with_context(|| format!("ps line {number}: bad rss"))?,
            cpu_ms: parse_cpu_time(time).with_context(|| format!("ps line {number}: bad time"))?,
        });
    }
    processes.sort_by(|a, b| (&a.label, a.pid).cmp(&(&b.label, b.pid)));
    Ok(processes)
}

/// Runs `ps` for the user that owns the candidate root.
fn read_processes(root: &Path) -> Result<Vec<Process>> {
    let uid = fs::metadata(root)?.uid();
    let output = std::process::Command::new("ps")
        .args(["-ww", "-U", &uid.to_string(), "-o", "pid=,rss=,time=,args="])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .context("could not run ps")?;
    ensure!(output.status.success(), "ps failed");
    parse_ps(
        &String::from_utf8_lossy(&output.stdout),
        std::process::id(),
        argv_of,
    )
}

/// Allocated bytes under the candidate root, without following links. A file that vanishes
/// while the walk runs (a snapshot being replaced, the WAL checkpointed) is skipped.
pub fn disk(root: &Path, database: &Path) -> Result<Disk> {
    let database_files: Vec<PathBuf> = ["", "-wal", "-shm"]
        .iter()
        .map(|suffix| {
            let mut name = database.as_os_str().to_owned();
            name.push(suffix);
            PathBuf::from(name)
        })
        .collect();
    let snapshots = root.join("snapshots");
    let mut disk = Disk::default();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let path = entry?.path();
            let meta = match fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if meta.is_dir() {
                pending.push(path.clone());
            }
            let bytes = meta.blocks() * 512;
            if database_files.contains(&path) {
                disk.database_bytes += bytes;
            } else if path.starts_with(&snapshots) {
                disk.snapshots_bytes += bytes;
            } else {
                disk.other_bytes += bytes;
            }
        }
    }
    Ok(disk)
}

/// What the `sample` command was given.
pub struct SampleArgs {
    pub interval: Duration,
    /// Stop after this many ticks; None runs until `stop` resolves.
    pub ticks: Option<u64>,
}

/// Creates `<root>/samples/samples-<wall_ms>.jsonl` (mode 0600, never an existing file, never
/// through a link) and returns it open, with its path.
fn create_sample_file(root: &Path) -> Result<(fs::File, PathBuf)> {
    let dir = root.join(SAMPLE_DIR);
    match fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let meta = fs::symlink_metadata(&dir)?;
    ensure!(
        meta.is_dir() && meta.mode() & 0o077 == 0,
        "the samples directory must be a private directory"
    );
    let path = dir.join(format!("samples-{}.jsonl", crate::facts::wall_ms()));
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .context("sample file must not exist yet")?;
    Ok((file, path))
}

/// Samples until `ticks` are written or `stop` resolves; each tick is one line, flushed, so a
/// sampler that is killed leaves at most one partial line, which the report ignores. `started`
/// is called once with the file's path before the first tick.
pub async fn sample(
    sandbox: &Sandbox,
    args: SampleArgs,
    started: impl FnOnce(&Path),
    stop: impl std::future::Future<Output = ()>,
) -> Result<u64> {
    ensure!(!args.interval.is_zero(), "interval must be positive");
    let (mut file, path) = create_sample_file(sandbox.root())?;
    // The label follows the settings file, read each tick. On an invalid file the core keeps its
    // last accepted settings, so the label follows that checkpoint as it stands at the tick; only
    // when neither file reads does it keep the last level seen.
    let mut mode = crate::settings::accepted_log_level(sandbox.root()).unwrap_or_default();
    started(&path);
    let interval_ms = u64::try_from(args.interval.as_millis())?;
    let mut clock = tokio::time::interval(args.interval);
    clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tokio::pin!(stop);
    let mut written = 0;
    while args.ticks.is_none_or(|limit| written < limit) {
        tokio::select! {
            _ = &mut stop => break,
            _ = clock.tick() => {}
        }
        let (root, database) = (sandbox.root().to_path_buf(), sandbox.database_path());
        let (processes, disk, level) = tokio::task::spawn_blocking(move || -> Result<_> {
            let level = crate::settings::observed_log_level(&root)
                .or_else(|| crate::settings::accepted_log_level(&root));
            Ok((read_processes(&root)?, disk(&root, &database)?, level))
        })
        .await??;
        mode = level.unwrap_or(mode);
        let tick = Tick {
            v: SAMPLE_VERSION,
            wall_ms: crate::facts::wall_ms(),
            mode,
            interval_ms,
            processes,
            disk,
        };
        let mut line = serde_json::to_vec(&tick)?;
        line.push(b'\n');
        file.write_all(&line)?;
        file.flush()?;
        written += 1;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_time_reads_both_ps_formats() {
        assert_eq!(parse_cpu_time("0:00.03"), Some(30));
        assert_eq!(parse_cpu_time("2:28.70"), Some(148_700));
        assert_eq!(parse_cpu_time("123:45.67"), Some(7_425_670));
        assert_eq!(parse_cpu_time("00:00:05"), Some(5_000));
        assert_eq!(parse_cpu_time("01:02:03"), Some(3_723_000));
        assert_eq!(parse_cpu_time("1-02:03:04"), Some(93_784_000));
        for bad in ["", "5", "a:00", "1:2:3:4", "1:-2", "x-01:00:00", "1:00.x"] {
            assert_eq!(parse_cpu_time(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn only_the_process_that_runs_candidate_code_is_counted() {
        let cases = [
            (
                "/repo/target/release/cbc-candidate serve --root /r --protected-db /d",
                Some("cbc-candidate serve"),
            ),
            (
                "./cbc-candidate snapshot --root /r",
                Some("cbc-candidate snapshot"),
            ),
            ("cbc-candidate --help", Some("cbc-candidate")),
            (
                "python3 -B -I crates/cbc-candidate/adapters/device/router.py --profile p",
                Some("adapters/device/router.py"),
            ),
            (
                "/Library/Frameworks/Python.framework/Versions/3.13/Resources/Python.app/\
                 Contents/MacOS/Python -I /repo/crates/cbc-candidate/adapters/claude/adapter.py",
                Some("adapters/claude/adapter.py"),
            ),
            (
                "/usr/bin/python3.12 /repo/crates/cbc-candidate/adapters/codex/adapter.py",
                Some("adapters/codex/adapter.py"),
            ),
            // A wrapper that names the candidate is not the candidate.
            (
                "rtk proxy python3 -B -I crates/cbc-candidate/adapters/device/router.py",
                None,
            ),
            ("/bin/zsh -c cbc-candidate serve", None),
            // The daily CBC and lookalikes are never counted.
            ("/usr/local/bin/cbc serve", None),
            ("/repo/target/debug/cbc-candidate-old serve", None),
            ("python3 /elsewhere/adapters/device/router.py", None),
            ("python3 crates/cbc-candidate/adapters/device/../x.py", None),
            (
                "python3 crates/cbc-candidate/adapters/device/router.sh",
                None,
            ),
            ("python3 -c print(1)", None),
            ("", None),
        ];
        for (command, label) in cases {
            assert_eq!(classify(&words(command)).as_deref(), label, "{command:?}");
        }
        // Paths with a space, as the kernel gives them: one argument each.
        let argv = |parts: &[&str]| parts.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        assert_eq!(
            classify(&argv(&["/tmp/with space/cbc-candidate", "serve"])).as_deref(),
            Some("cbc-candidate serve")
        );
        assert_eq!(
            classify(&argv(&[
                "python3",
                "-I",
                "/tmp/with space/crates/cbc-candidate/adapters/device/router.py",
            ]))
            .as_deref(),
            Some("adapters/device/router.py")
        );
        assert_eq!(classify(&[]), None);
    }

    fn words(command: &str) -> Vec<String> {
        command.split_whitespace().map(String::from).collect()
    }

    /// The argument vector a test `ps` line stands for, keyed by its PID.
    fn argv_from(output: &'static str) -> impl Fn(u32) -> Option<Vec<String>> {
        move |pid| {
            output.lines().find_map(|line| {
                let mut fields = line.split_whitespace();
                (fields.next()? == pid.to_string())
                    .then(|| fields.skip(2).map(String::from).collect())
            })
        }
    }

    #[test]
    fn procargs2_gives_the_arguments_and_never_the_environment() {
        let mut buffer = 3i32.to_ne_bytes().to_vec();
        buffer.extend(b"/tmp/with space/cbc-candidate\0\0\0\0");
        buffer.extend(b"/tmp/with space/cbc-candidate\0serve\0--root\0");
        buffer.extend(b"SECRET=value\0\0");
        assert_eq!(
            parse_procargs2(&buffer).unwrap(),
            ["/tmp/with space/cbc-candidate", "serve", "--root"]
        );
        assert_eq!(parse_procargs2(&buffer[..2]), None);
        let mut short = 5i32.to_ne_bytes().to_vec();
        short.extend(b"/x\0a\0");
        assert_eq!(parse_procargs2(&short), None);
    }

    #[test]
    fn a_gone_process_is_skipped() {
        let output = "  101  4304   2:28.70 /repo/target/release/cbc-candidate serve\n";
        assert!(parse_ps(output, 1, |_| None).unwrap().is_empty());
    }

    #[test]
    fn ps_output_keeps_candidate_lines_and_drops_the_sampler() {
        let output = "\
  101  4304   2:28.70 /repo/target/release/cbc-candidate serve --root /r\n\
  102 12384   0:00.50 /usr/local/bin/cbc serve --secret-arg\n\
  103  2048   0:01.00 python3 -I /repo/crates/cbc-candidate/adapters/codex/adapter.py\n\
  104  1024   0:00.10 /repo/target/release/cbc-candidate sample --root /r\n\
not even a ps line\n";
        let processes = parse_ps(output, 104, argv_from(output)).unwrap();
        assert_eq!(
            processes,
            vec![
                Process {
                    label: "adapters/codex/adapter.py".into(),
                    pid: 103,
                    rss_kb: 2048,
                    cpu_ms: 1000,
                },
                Process {
                    label: "cbc-candidate serve".into(),
                    pid: 101,
                    rss_kb: 4304,
                    cpu_ms: 148_700,
                },
            ]
        );
    }

    #[test]
    fn a_bad_candidate_line_is_refused_without_echoing_any_line() {
        let output = "  7 lots 0:00.10 /repo/cbc-candidate serve --token-like-value\n";
        let error = parse_ps(output, 1, argv_from(output))
            .unwrap_err()
            .to_string();
        assert_eq!(error, "ps line 1: bad rss");
        // A malformed line that is not the candidate is never read, so it cannot fail a tick.
        let other = "  x y z /usr/bin/other --secret\n";
        assert!(parse_ps(other, 1, |_| panic!("never read"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_tick_round_trips_and_unknown_fields_are_refused() {
        let tick = Tick {
            v: SAMPLE_VERSION,
            wall_ms: 5,
            mode: Mode::Debug,
            interval_ms: 10_000,
            processes: vec![],
            disk: Disk::default(),
        };
        let text = serde_json::to_string(&tick).unwrap();
        assert!(text.contains(r#""mode":"debug""#));
        assert_eq!(serde_json::from_str::<Tick>(&text).unwrap(), tick);
        let extra = text.replacen('{', r#"{"args":"x","#, 1);
        assert!(serde_json::from_str::<Tick>(&extra).is_err());
    }
}
