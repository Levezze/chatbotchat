//! The Cost section (C3): CPU, memory and disk from a sampler file, per mode and process.
//!
//! Ticks are cut on the window by their wall time. CPU is the growth of each process's
//! cumulative CPU time between consecutive ticks of one mode, over the wall time those ticks
//! span; a PID whose time goes down was reused and contributes nothing for that step.
//! Memory is, per tick, the summed resident memory of a label's processes, over the ticks
//! where it had any. The components a trial should have are listed from the end snapshot's
//! seats, so one that never matched prints missing instead of passing for free.
use super::*;
use crate::sampler::{Mode, Tick, SAMPLE_VERSION};

/// A sampler file, checked line by line.
#[derive(Debug)]
pub struct Samples {
    pub name: String,
    pub sha256: String,
    ticks: Vec<Tick>,
    /// The file ended in a partial line (a sampler killed mid-write), which is ignored.
    truncated: bool,
}

impl Samples {
    /// The first and last tick times, if any.
    fn span(&self) -> Option<(i64, i64)> {
        Some((self.ticks.first()?.wall_ms, self.ticks.last()?.wall_ms))
    }

    /// Puts the files in time order and refuses two that overlap: two samplers running at
    /// once would count every process twice. A restarted sampler's files follow each other.
    pub fn in_order(mut files: Vec<Samples>) -> Result<Vec<Samples>> {
        files.sort_by_key(|f| f.span().map(|(first, _)| first));
        let timed: Vec<&Samples> = files.iter().filter(|f| f.span().is_some()).collect();
        for pair in timed.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let (Some((_, a_last)), Some((b_first, _))) = (a.span(), b.span()) else {
                continue;
            };
            ensure!(
                a_last < b_first,
                "samples {} and {} overlap in time: one sampler at a time",
                a.name,
                b.name
            );
        }
        Ok(files)
    }

    pub fn parse(name: &str, bytes: &[u8]) -> Result<Self> {
        let text = std::str::from_utf8(bytes).context("samples must be UTF-8")?;
        let mut lines: Vec<&str> = text.split('\n').collect();
        // Text ending in a newline leaves an empty last piece; anything else is a partial line.
        let truncated = !lines.pop().unwrap_or_default().is_empty();
        let mut ticks: Vec<Tick> = Vec::with_capacity(lines.len());
        for (index, line) in lines.iter().enumerate() {
            let tick: Tick = serde_json::from_str(line)
                .ok()
                .with_context(|| format!("samples line {}: not a sample tick", index + 1))?;
            ensure!(
                tick.v == SAMPLE_VERSION,
                "samples line {}: unknown version",
                index + 1
            );
            ensure!(
                ticks.last().is_none_or(|last| last.wall_ms <= tick.wall_ms),
                "samples line {}: out of order",
                index + 1
            );
            ticks.push(tick);
        }
        use sha2::Digest;
        Ok(Self {
            name: name.to_owned(),
            sha256: sha2::Sha256::digest(bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            ticks,
            truncated,
        })
    }
}

const CORE: &str = "cbc-candidate serve";
const DEVICE: &str = "adapters/device/router.py";
const CLAUDE_SHIM: &str = "adapters/claude/adapter.py";
const CODEX_SHIM: &str = "adapters/codex/adapter.py";

/// The processes a trial with these seats should have, and how many of each.
fn expected(facts: &Facts) -> BTreeMap<&'static str, usize> {
    let live = facts.seats.iter().filter(|s| s.active && !s.retired);
    let mut hosts: BTreeMap<&str, usize> = BTreeMap::new();
    for seat in live {
        *hosts.entry(seat.host.as_deref().unwrap_or("")).or_default() += 1;
    }
    let mut expected = BTreeMap::from([(CORE, 1), (DEVICE, 1)]);
    if let Some(&n) = hosts.get("claude") {
        expected.insert(CLAUDE_SHIM, n);
    }
    if let Some(&n) = hosts.get("codex") {
        expected.insert(CODEX_SHIM, n);
    }
    expected
}

#[derive(Default)]
struct Usage {
    /// Summed RSS per tick, over the ticks where the label had a process.
    rss_kb: Vec<i64>,
    max_processes: usize,
    cpu_ms: u64,
    /// Steps between consecutive ticks where one of its processes was measured. With none, its
    /// CPU is unknown, not zero.
    cpu_steps: usize,
}

#[derive(Default)]
struct ModeUsage {
    ticks: usize,
    span_ms: i64,
    labels: BTreeMap<String, Usage>,
}

fn usage(ticks: &[&Tick]) -> BTreeMap<Mode, ModeUsage> {
    let mut modes: BTreeMap<Mode, ModeUsage> = BTreeMap::new();
    let mut previous: Option<&Tick> = None;
    for tick in ticks {
        let mode = modes.entry(tick.mode).or_default();
        mode.ticks += 1;
        let mut per_label: BTreeMap<&str, (i64, usize)> = BTreeMap::new();
        for process in &tick.processes {
            let entry = per_label.entry(&process.label).or_default();
            entry.0 += i64::try_from(process.rss_kb).unwrap_or(i64::MAX);
            entry.1 += 1;
        }
        for (label, (rss, count)) in per_label {
            let usage = mode.labels.entry(label.to_owned()).or_default();
            usage.rss_kb.push(rss);
            usage.max_processes = usage.max_processes.max(count);
        }
        // CPU only between consecutive ticks of one mode: a mode switch starts a new run.
        if let Some(before) = previous.filter(|p| p.mode == tick.mode) {
            mode.span_ms += tick.wall_ms - before.wall_ms;
            let earlier: BTreeMap<u32, &crate::sampler::Process> =
                before.processes.iter().map(|p| (p.pid, p)).collect();
            for process in &tick.processes {
                let Some(old) = earlier.get(&process.pid) else {
                    continue;
                };
                if old.label == process.label && process.cpu_ms >= old.cpu_ms {
                    let usage = mode.labels.entry(process.label.clone()).or_default();
                    usage.cpu_ms += process.cpu_ms - old.cpu_ms;
                    usage.cpu_steps += 1;
                }
            }
        }
        previous = Some(tick);
    }
    modes
}

fn per_mille(part: u64, whole: i64) -> String {
    let value = i64::try_from(part).unwrap_or(i64::MAX).saturating_mul(1000) / whole.max(1);
    format!("{}.{}%", value / 10, value % 10)
}

/// The Cost section over the sampler files, which `Samples::in_order` has put in time order.
pub(super) fn render_cost(o: &mut String, samples: &[Samples], facts: &Facts, window: Window) {
    if samples.is_empty() {
        let _ = writeln!(o, "\nCost: {}", missing("no sampler file given"));
        return;
    }
    let ticks: Vec<&Tick> = samples
        .iter()
        .flat_map(|s| &s.ticks)
        .filter(|t| window.holds(t.wall_ms))
        .collect();
    let _ = writeln!(o, "\nCost (sampler ticks in the window)");
    for file in samples.iter().filter(|s| s.truncated) {
        let _ = writeln!(o, "  {} ends in a partial line, ignored", file.name);
    }
    let modes = usage(&ticks);
    let expected = expected(facts);
    for mode in Mode::ALL {
        let name = mode.label();
        let Some(usage) = modes.get(&mode) else {
            let reason = match mode {
                Mode::Normal => "no normal-mode ticks in the window",
                Mode::Debug => "no debug-mode ticks in the window; debug logging lands in B2",
            };
            let _ = writeln!(o, "  {name}: {}", missing(reason));
            continue;
        };
        let interval = ticks
            .iter()
            .find(|t| t.mode == mode)
            .map_or(0, |t| t.interval_ms);
        let _ = writeln!(
            o,
            "  {name}: ticks={} every {interval} ms over {} ms",
            usage.ticks, usage.span_ms
        );
        let labels: BTreeSet<&str> = expected
            .keys()
            .copied()
            .chain(usage.labels.keys().map(String::as_str))
            .collect();
        for label in labels {
            let expect = expected
                .get(label)
                .map(|n| format!(" expected={n}"))
                .unwrap_or_default();
            let Some(found) = usage.labels.get(label) else {
                let _ = writeln!(
                    o,
                    "  {name} — {label}: {}",
                    missing(&format!("no process matched;{expect}"))
                );
                continue;
            };
            let cpu = if found.cpu_steps == 0 {
                missing("never seen in two consecutive ticks")
            } else if usage.span_ms > 0 {
                format!(
                    "{} of one core ({} ms CPU)",
                    per_mille(found.cpu_ms, usage.span_ms),
                    found.cpu_ms
                )
            } else {
                missing("needs two ticks")
            };
            let _ = writeln!(
                o,
                "  {name} — {label}: processes max={}{expect} rss_kb {} cpu={cpu}",
                found.max_processes,
                distribution(&found.rss_kb),
            );
        }
        let _ = writeln!(
            o,
            "  {name} — Claude mod: {}",
            missing("runs inside the host app")
        );
    }
    match (ticks.first(), ticks.last()) {
        (Some(first), Some(last)) => {
            let max = |pick: fn(&crate::sampler::Disk) -> u64| {
                ticks.iter().map(|t| pick(&t.disk)).max().unwrap_or(0)
            };
            let _ = writeln!(
                o,
                "  disk bytes: database start={} end={} max={}; snapshots start={} end={} max={}; \
                 other start={} end={} max={}",
                first.disk.database_bytes,
                last.disk.database_bytes,
                max(|d| d.database_bytes),
                first.disk.snapshots_bytes,
                last.disk.snapshots_bytes,
                max(|d| d.snapshots_bytes),
                first.disk.other_bytes,
                last.disk.other_bytes,
                max(|d| d.other_bytes),
            );
        }
        _ => {
            let _ = writeln!(o, "  disk: {}", missing("no ticks in the window"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sampler::{Disk, Process};

    fn process(label: &str, pid: u32, rss_kb: u64, cpu_ms: u64) -> Process {
        Process {
            label: label.into(),
            pid,
            rss_kb,
            cpu_ms,
        }
    }

    fn tick(wall_ms: i64, mode: Mode, processes: Vec<Process>, database: u64) -> Tick {
        Tick {
            v: SAMPLE_VERSION,
            wall_ms,
            mode,
            interval_ms: 1000,
            processes,
            disk: Disk {
                database_bytes: database,
                snapshots_bytes: 0,
                other_bytes: 8,
            },
        }
    }

    fn file(ticks: &[Tick]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for tick in ticks {
            bytes.extend(serde_json::to_vec(tick).unwrap());
            bytes.push(b'\n');
        }
        bytes
    }

    fn seat(member: &str, host: &str) -> crate::evidence::Seat {
        crate::evidence::Seat {
            context: "t".into(),
            member: member.into(),
            role: "feature_lead".into(),
            active: true,
            retired: false,
            epoch: 1,
            session: Some(member.into()),
            host: Some(host.into()),
        }
    }

    #[test]
    fn a_partial_last_line_is_ignored_and_any_other_bad_line_refused() {
        let good = file(&[tick(1, Mode::Normal, vec![], 0)]);
        let mut partial = good.clone();
        partial.extend(br#"{"v":1,"wall"#);
        let parsed = Samples::parse("s.jsonl", &partial).unwrap();
        assert_eq!(parsed.ticks.len(), 1);
        assert!(parsed.truncated);
        assert!(!Samples::parse("s.jsonl", &good).unwrap().truncated);

        let mut bad = br#"{"secret":"value"}"#.to_vec();
        bad.push(b'\n');
        bad.extend(&good);
        let error = Samples::parse("s.jsonl", &bad).unwrap_err().to_string();
        assert_eq!(error, "samples line 1: not a sample tick");

        let backwards = file(&[
            tick(5, Mode::Normal, vec![], 0),
            tick(4, Mode::Normal, vec![], 0),
        ]);
        let error = Samples::parse("s.jsonl", &backwards)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "samples line 2: out of order");
    }

    #[test]
    fn cpu_is_growth_between_ticks_of_one_mode_and_a_reused_pid_adds_nothing() {
        let core = |pid, cpu| process(CORE, pid, 100, cpu);
        let samples = file(&[
            tick(0, Mode::Normal, vec![core(1, 1000)], 10),
            tick(1000, Mode::Normal, vec![core(1, 1100)], 30),
            // PID reused by a new process with less CPU: no step for it.
            tick(2000, Mode::Normal, vec![core(1, 20)], 20),
            tick(3000, Mode::Normal, vec![core(1, 70)], 20),
            // A debug run: its first tick does not take a step from the normal run.
            tick(4000, Mode::Debug, vec![core(1, 5000)], 20),
            tick(5000, Mode::Debug, vec![core(1, 5200)], 20),
            // Outside the window.
            tick(9000, Mode::Debug, vec![core(1, 9000)], 99),
        ]);
        let samples = Samples::parse("s.jsonl", &samples).unwrap();
        let facts = Facts {
            seats: vec![seat("feature", "claude"), seat("lead", "codex")],
            ..Facts::default()
        };
        let window = Window {
            from_ms: 0,
            until_ms: 5000,
        };
        let mut text = String::new();
        render_cost(&mut text, std::slice::from_ref(&samples), &facts, window);
        let expected = "
Cost (sampler ticks in the window)
  normal: ticks=4 every 1000 ms over 3000 ms
  normal — adapters/claude/adapter.py: missing (no process matched; expected=1)
  normal — adapters/codex/adapter.py: missing (no process matched; expected=1)
  normal — adapters/device/router.py: missing (no process matched; expected=1)
  normal — cbc-candidate serve: processes max=1 expected=1 rss_kb n=4 median=100 p90=100 max=100 cpu=5.0% of one core (150 ms CPU)
  normal — Claude mod: missing (runs inside the host app)
  debug: ticks=2 every 1000 ms over 1000 ms
  debug — adapters/claude/adapter.py: missing (no process matched; expected=1)
  debug — adapters/codex/adapter.py: missing (no process matched; expected=1)
  debug — adapters/device/router.py: missing (no process matched; expected=1)
  debug — cbc-candidate serve: processes max=1 expected=1 rss_kb n=2 median=100 p90=100 max=100 cpu=20.0% of one core (200 ms CPU)
  debug — Claude mod: missing (runs inside the host app)
  disk bytes: database start=10 end=20 max=30; snapshots start=0 end=0 max=0; other start=8 end=8 max=8
";
        assert_eq!(text, expected);
    }

    #[test]
    fn shims_are_summed_per_tick_and_unexpected_labels_still_listed() {
        let shim = |pid, rss| process(CLAUDE_SHIM, pid, rss, 0);
        let samples = file(&[
            tick(0, Mode::Normal, vec![shim(1, 10), shim(2, 30)], 0),
            tick(
                1000,
                Mode::Normal,
                vec![shim(1, 10), process("cbc-candidate snapshot", 9, 5, 0)],
                0,
            ),
        ]);
        let samples = Samples::parse("s.jsonl", &samples).unwrap();
        let facts = Facts {
            seats: vec![seat("a", "claude"), seat("b", "claude")],
            ..Facts::default()
        };
        let mut text = String::new();
        render_cost(
            &mut text,
            std::slice::from_ref(&samples),
            &facts,
            Window {
                from_ms: 0,
                until_ms: 1000,
            },
        );
        assert!(text.contains(
            "  normal — adapters/claude/adapter.py: processes max=2 expected=2 rss_kb n=2 median=10 p90=40 max=40"
        ), "{text}");
        assert!(text.contains("  normal — cbc-candidate snapshot: processes max=1 rss_kb n=1"));
        assert!(!text.contains("codex"));
        assert!(text.contains(
            "  debug: missing (no debug-mode ticks in the window; debug logging lands in B2)"
        ));
    }

    #[test]
    fn cpu_is_missing_for_a_label_never_seen_in_two_consecutive_ticks() {
        let samples = file(&[
            tick(0, Mode::Normal, vec![process(CORE, 1, 100, 0)], 0),
            tick(
                1000,
                Mode::Normal,
                vec![
                    process(CORE, 1, 100, 10),
                    process("cbc-candidate snapshot", 9, 5, 400),
                ],
                0,
            ),
            tick(2000, Mode::Normal, vec![process(CORE, 1, 100, 10)], 0),
        ]);
        let samples = Samples::parse("s.jsonl", &samples).unwrap();
        let mut text = String::new();
        render_cost(
            &mut text,
            std::slice::from_ref(&samples),
            &Facts::default(),
            Window {
                from_ms: 0,
                until_ms: 2000,
            },
        );
        // Seen in one tick only: its CPU was never measured, which is not idle.
        assert!(
            text.contains(
                "  normal — cbc-candidate snapshot: processes max=1 rss_kb n=1 median=5 p90=5 \
                 max=5 cpu=missing (never seen in two consecutive ticks)\n"
            ),
            "{text}"
        );
        // Measured and idle across a step is a real zero.
        assert!(
            text.contains("  normal — cbc-candidate serve: processes max=1 expected=1 rss_kb n=3 median=100 p90=100 max=100 cpu=0.5% of one core (10 ms CPU)\n"),
            "{text}"
        );
    }

    #[test]
    fn no_sampler_file_and_no_ticks_print_missing() {
        let mut text = String::new();
        let window = Window {
            from_ms: 0,
            until_ms: 10,
        };
        render_cost(&mut text, &[], &Facts::default(), window);
        assert_eq!(text, "\nCost: missing (no sampler file given)\n");

        let late = Samples::parse("s.jsonl", &file(&[tick(50, Mode::Normal, vec![], 0)])).unwrap();
        let mut text = String::new();
        render_cost(
            &mut text,
            std::slice::from_ref(&late),
            &Facts::default(),
            window,
        );
        assert!(text.contains("  normal: missing (no normal-mode ticks in the window)"));
        assert!(text.contains("  disk: missing (no ticks in the window)"));
    }
}
