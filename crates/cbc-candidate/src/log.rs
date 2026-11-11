//! Structured logs (B2): one JSON object per line, schema v1, shared with the driver's log.
//!
//! Every line starts with `v`, `t_ms`, `level`, `component`, `run`, `instance` and `event`, then
//! the event's own fields. Only the fields in [`FIELDS`] are written: a field outside the list is
//! dropped, so a message body, a token or a path cannot reach the log by accident.
//!
//! The level is live. `normal` writes info and above, `debug` adds debug events. The core reads it
//! from the runtime settings, so a settings edit changes it without a restart.
//!
//! The file is private (0600, never through a link, in a 0700 directory), bounded in size and
//! rotated, keeping [`KEEP`] older files. A line that cannot be written is counted, and the count
//! is written as a `log_dropped` line once writing works again.
use crate::sampler::Mode;
use anyhow::{ensure, Result};
use std::{
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU8, Ordering},
        Mutex,
    },
};
use tracing::{field::Field, Event, Level, Metadata, Subscriber};
use tracing_subscriber::{
    layer::{Context, Layer},
    prelude::*,
};

pub const SCHEMA: u32 = 1;
pub const LOG_DIR: &str = "logs";
pub const MAX_BYTES: u64 = 8 * 1024 * 1024;
pub const KEEP: u32 = 3;

/// The fields a line may carry besides its header. Identifiers and outcomes only.
pub const FIELDS: &[&str] = &[
    "member",
    "chat",
    "context",
    "room",
    "batch",
    "reference",
    "delivery",
    "kind",
    "reason",
    "outcome",
    "count",
    "seq",
    "attempt",
    "targets",
    "pending",
    "session",
    "retry_ms",
];

/// The live level the core's layer reads on every event.
pub static LEVEL: AtomicU8 = AtomicU8::new(0);

fn encode(mode: Mode) -> u8 {
    match mode {
        Mode::Normal => 0,
        Mode::Debug => 1,
    }
}

/// Sets the live level. Called whenever the runtime settings are read.
pub fn set_level(mode: Mode) {
    LEVEL.store(encode(mode), Ordering::Relaxed);
}

/// A size-bounded, rotated, private log file. Writing never fails the caller: a line that
/// cannot be written returns false.
pub struct LogFile {
    path: PathBuf,
    max_bytes: u64,
    keep: u32,
    file: Option<fs::File>,
    size: u64,
}

impl LogFile {
    /// Creates `dir` (0700) if missing and refuses one that is not private. The file is opened
    /// now, so descriptor pressure later cannot cost the log; if that fails, every write retries.
    pub fn open(dir: &Path, name: &str, max_bytes: u64, keep: u32) -> Result<Self> {
        ensure!(
            keep >= 1 && max_bytes > 0,
            "a log keeps at least one older file"
        );
        match fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let meta = fs::symlink_metadata(dir)?;
        ensure!(
            meta.is_dir() && meta.mode() & 0o077 == 0,
            "the log directory must be a private directory"
        );
        let mut log = Self {
            path: dir.join(name),
            max_bytes,
            keep,
            file: None,
            size: 0,
        };
        let _ = log.reopen();
        Ok(log)
    }

    /// Appends one line, rotating first when it would cross the bound.
    pub fn append(&mut self, line: &[u8]) -> bool {
        let written = (|| -> std::io::Result<()> {
            if self.file.is_none() {
                self.reopen()?;
            }
            if self.size > 0 && self.size + line.len() as u64 > self.max_bytes {
                self.rotate()?;
            }
            let file = self.file.as_mut().expect("opened above");
            file.write_all(line)?;
            self.size += line.len() as u64;
            Ok(())
        })();
        if written.is_err() {
            // Reopen at the next line: the file may have been moved or replaced.
            self.file = None;
        }
        written.is_ok()
    }

    fn reopen(&mut self) -> std::io::Result<()> {
        let file = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&self.path)?;
        let meta = file.metadata()?;
        if !meta.is_file() || meta.mode() & 0o077 != 0 {
            return Err(std::io::Error::other("the log file must be a private file"));
        }
        self.size = meta.len();
        self.file = Some(file);
        Ok(())
    }

    fn rotated(&self, n: u32) -> PathBuf {
        let mut name = self.path.clone().into_os_string();
        name.push(format!(".{n}"));
        PathBuf::from(name)
    }

    /// `name.(keep-1)` → `name.keep`, …, `name` → `name.1`; the oldest falls off.
    fn rotate(&mut self) -> std::io::Result<()> {
        self.file = None;
        for n in (1..self.keep).rev() {
            match fs::rename(self.rotated(n), self.rotated(n + 1)) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
        fs::rename(&self.path, self.rotated(1))?;
        self.reopen()
    }
}

/// The JSON layer: formats each enabled event as a v1 line into a [`LogFile`].
pub struct JsonLayer {
    component: &'static str,
    run: Option<String>,
    instance: String,
    level: &'static AtomicU8,
    state: Mutex<State>,
}

struct State {
    file: LogFile,
    dropped: u64,
}

impl JsonLayer {
    pub fn new(
        component: &'static str,
        run: Option<String>,
        level: &'static AtomicU8,
        file: LogFile,
    ) -> Self {
        let instance: String = (0..8)
            .map(|_| format!("{:02x}", rand::random::<u8>()))
            .collect();
        Self {
            component,
            run,
            instance,
            level,
            state: Mutex::new(State { file, dropped: 0 }),
        }
    }

    fn line(
        &self,
        level: &Level,
        event: &str,
        fields: serde_json::Map<String, serde_json::Value>,
    ) -> Vec<u8> {
        let mut line = serde_json::json!({
            "v": SCHEMA,
            "t_ms": crate::facts::wall_ms(),
            "level": level.as_str().to_ascii_lowercase(),
            "component": self.component,
            "run": self.run,
            "instance": self.instance,
            "event": event,
        });
        line.as_object_mut().expect("an object").extend(fields);
        let mut bytes = serde_json::to_vec(&line).expect("a JSON value serializes");
        bytes.push(b'\n');
        bytes
    }
}

/// The level a mode lets through: info and above, or debug and above. Trace never.
fn admits(mode: u8, level: &Level) -> bool {
    let most = if mode == encode(Mode::Debug) {
        Level::DEBUG
    } else {
        Level::INFO
    };
    *level <= most
}

#[derive(Default)]
struct Fields {
    event: Option<String>,
    fields: serde_json::Map<String, serde_json::Value>,
}

impl Fields {
    fn keep(&mut self, field: &Field, value: serde_json::Value) {
        if field.name() == "event" {
            if let serde_json::Value::String(name) = value {
                self.event = Some(name);
            }
        } else if FIELDS.contains(&field.name()) {
            self.fields.insert(field.name().to_owned(), value);
        }
    }
}

impl tracing::field::Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.keep(field, value.into());
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.keep(field, value.into());
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.keep(field, value.into());
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.keep(field, value.into());
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // The free-text `message` is never an allowlisted name, so it is dropped here.
        self.keep(field, format!("{value:?}").into());
    }
}

impl<S: Subscriber> Layer<S> for JsonLayer {
    fn register_callsite(&self, _: &'static Metadata<'static>) -> tracing::subscriber::Interest {
        // The level is live, so no callsite may be cached as always on or always off.
        tracing::subscriber::Interest::sometimes()
    }

    fn enabled(&self, metadata: &Metadata<'_>, _ctx: Context<'_, S>) -> bool {
        admits(self.level.load(Ordering::Relaxed), metadata.level())
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let name = fields.event.as_deref().unwrap_or("unnamed");
        let line = self.line(event.metadata().level(), name, fields.fields);
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.dropped > 0 {
            let mut count = serde_json::Map::new();
            count.insert("count".into(), state.dropped.into());
            let report = self.line(&Level::WARN, "log_dropped", count);
            if !state.file.append(&report) {
                state.dropped += 1;
                return;
            }
            state.dropped = 0;
        }
        if !state.file.append(&line) {
            state.dropped += 1;
        }
    }
}

/// One debug line per answered core request: its reply type and identifiers. Only called when
/// debug is on; the reply's items and bodies are never read, and its reference only as its batch tag.
pub fn reply(context: &str, member: &str, reply: &serde_json::Value) {
    let field = |name: &str| reply.get(name);
    tracing::debug!(
        event = "request",
        context,
        member,
        kind = field("type").and_then(|v| v.as_str()),
        seq = field("sequence").and_then(|v| v.as_i64()),
        reference = field("reference")
            .and_then(|v| v.as_str())
            .map(crate::facts::batch_tag),
        count = field("items")
            .and_then(|v| v.as_array())
            .map(|a| a.len() as u64),
        outcome = field("covered").map(|v| v.to_string()),
    );
}

/// Installs the core's logger for `serve`: `<root>/logs/core.jsonl`, at the settings' level.
pub fn init(root: &Path, run: Option<String>) -> Result<()> {
    let file = LogFile::open(&root.join(LOG_DIR), "core.jsonl", MAX_BYTES, KEEP)?;
    let layer = JsonLayer::new("core", run, &LEVEL, file);
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(layer))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn lines(path: &Path) -> Vec<serde_json::Value> {
        fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn level() -> &'static AtomicU8 {
        Box::leak(Box::new(AtomicU8::new(0)))
    }

    fn with_layer(dir: &Path, level: &'static AtomicU8, max: u64, f: impl FnOnce()) {
        let file = LogFile::open(dir, "core.jsonl", max, 2).unwrap();
        let layer = JsonLayer::new("core", Some("run-7".into()), level, file);
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), f);
    }

    #[test]
    fn a_line_carries_the_v1_header_and_only_allowlisted_fields() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("logs");
        with_layer(&dir, level(), MAX_BYTES, || {
            tracing::info!(
                event = "delivered",
                member = "lead",
                seq = 4_u64,
                body = "hello there",
                token = "cbct_secret"
            );
            tracing::info!("free text without an event");
        });
        let got = lines(&dir.join("core.jsonl"));
        assert_eq!(got.len(), 2, "{got:?}");
        let line = &got[0];
        assert_eq!(line["v"], 1);
        assert!(line["t_ms"].as_u64().unwrap() > 0);
        assert_eq!(line["level"], "info");
        assert_eq!(line["component"], "core");
        assert_eq!(line["run"], "run-7");
        assert_eq!(line["instance"].as_str().unwrap().len(), 16);
        assert_eq!(line["event"], "delivered");
        assert_eq!(line["member"], "lead");
        assert_eq!(line["seq"], 4);
        let text = fs::read_to_string(dir.join("core.jsonl")).unwrap();
        assert!(
            !text.contains("hello there") && !text.contains("cbct_secret"),
            "{text}"
        );
        assert!(line.get("body").is_none() && line.get("token").is_none());
        // An event without a name is kept, under a fixed name, with no free text.
        assert_eq!(got[1]["event"], "unnamed");
        assert!(!text.contains("free text"), "{text}");
    }

    #[test]
    fn debug_events_appear_only_while_the_level_is_debug() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("logs");
        let live = level();
        with_layer(&dir, live, MAX_BYTES, || {
            tracing::debug!(event = "routed", seq = 1_u64);
            live.store(encode(Mode::Debug), Ordering::Relaxed);
            tracing::debug!(event = "routed", seq = 2_u64);
            tracing::trace!(event = "never", seq = 3_u64);
            live.store(encode(Mode::Normal), Ordering::Relaxed);
            tracing::debug!(event = "routed", seq = 4_u64);
            tracing::warn!(event = "kept", seq = 5_u64);
        });
        let seqs: Vec<u64> = lines(&dir.join("core.jsonl"))
            .iter()
            .map(|l| l["seq"].as_u64().unwrap())
            .collect();
        assert_eq!(seqs, [2, 5]);
    }

    #[test]
    fn the_file_rotates_at_its_bound_and_keeps_two_older_files() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("logs");
        with_layer(&dir, level(), 600, || {
            for seq in 0..40_u64 {
                tracing::info!(event = "delivered", seq);
            }
        });
        let mut names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["core.jsonl", "core.jsonl.1", "core.jsonl.2"]);
        for name in &names {
            let len = fs::metadata(dir.join(name)).unwrap().len();
            assert!(len > 0 && len <= 600, "{name} is {len} bytes");
        }
        // The newest line is in the live file, and the lines run on across the rotation.
        let live = lines(&dir.join("core.jsonl"));
        let older = lines(&dir.join("core.jsonl.1"));
        assert_eq!(live.last().unwrap()["seq"], 39);
        assert_eq!(
            older.last().unwrap()["seq"].as_u64().unwrap() + 1,
            live[0]["seq"].as_u64().unwrap()
        );
    }

    #[test]
    fn a_line_that_cannot_be_written_is_counted_and_reported_on_the_next_good_line() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("logs");
        let path = dir.join("core.jsonl");
        let elsewhere = temp.path().join("elsewhere");
        fs::write(&elsewhere, b"").unwrap();
        // A link at the log's path is refused, never followed.
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
        let file = LogFile::open(&dir, "core.jsonl", MAX_BYTES, 2).unwrap();
        let layer = JsonLayer::new("core", None, level(), file);
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
            tracing::info!(event = "lost", seq = 1_u64);
            tracing::info!(event = "lost", seq = 2_u64);
            fs::remove_file(&path).unwrap();
            tracing::info!(event = "kept", seq = 3_u64);
        });
        assert_eq!(fs::read(&elsewhere).unwrap(), b"");
        let got = lines(&path);
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0]["event"], "log_dropped");
        assert_eq!(got[0]["level"], "warn");
        assert_eq!(got[0]["count"], 2);
        assert_eq!(got[0]["run"], serde_json::Value::Null);
        assert_eq!(got[1]["event"], "kept");
    }

    #[test]
    fn a_reference_is_logged_as_the_facts_batch_tag_the_driver_computes_too() {
        // adapters/shared/test_log.py pins the same value.
        assert_eq!(crate::facts::batch_tag("ref-secret-1"), "6292735d22f1737a");
    }

    #[test]
    fn a_core_reply_is_logged_by_its_identifiers_and_never_its_items() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("logs");
        let live = level();
        live.store(encode(Mode::Debug), Ordering::Relaxed);
        with_layer(&dir, live, MAX_BYTES, || {
            let batch = serde_json::json!({"type": "batch", "reference": "r1",
                "items": [{"room": "x", "body": "private words"}, {"body": "more"}]});
            reply("api", "lead", &batch);
            reply(
                "api",
                "lead",
                &serde_json::json!({"type": "sent", "sequence": 9}),
            );
            reply(
                "api",
                "lead",
                &serde_json::json!({"type": "woken", "covered": true}),
            );
        });
        let got = lines(&dir.join("core.jsonl"));
        let text = fs::read_to_string(dir.join("core.jsonl")).unwrap();
        assert!(!text.contains("private words"), "{text}");
        assert_eq!(got.len(), 3);
        assert!(got
            .iter()
            .all(|l| l["level"] == "debug" && l["event"] == "request"));
        assert!(got
            .iter()
            .all(|l| l["context"] == "api" && l["member"] == "lead"));
        assert_eq!(got[0]["kind"], "batch");
        assert_eq!(got[0]["reference"], crate::facts::batch_tag("r1"));
        assert!(
            !text.contains(r#""r1""#),
            "a receipt reference is a capability: {text}"
        );
        assert_eq!(got[0]["count"], 2);
        assert_eq!(got[1]["kind"], "sent");
        assert_eq!(got[1]["seq"], 9);
        assert_eq!(got[2]["outcome"], "true");
    }

    #[test]
    fn the_log_is_private_and_a_shared_directory_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("logs");
        with_layer(&dir, level(), MAX_BYTES, || {
            tracing::info!(event = "hello");
        });
        let mode = |p: &Path| fs::metadata(p).unwrap().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("core.jsonl")), 0o600);
        let shared = temp.path().join("shared");
        fs::create_dir(&shared).unwrap();
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(LogFile::open(&shared, "core.jsonl", MAX_BYTES, 2).is_err());
    }
}
