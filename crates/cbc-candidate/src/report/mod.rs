//! The trial report (C2): protocol §8, printed from a manifest and an end snapshot.
//!
//! Deterministic: the same manifest, snapshot, tally and window give the same bytes. The report
//! reads no clock. Every fact is taken as it stood at `until`, from its own timestamp, never
//! from a state column (those describe the moment the snapshot was taken). Each distribution
//! carries its sample count, and a measurement the records cannot give is printed as
//! `missing (<reason>)`, never estimated. Paths are printed as file names only: the report is
//! pasted into a public run log.
use crate::{
    evidence::{sha256_file, Evidence, Seat},
    waits::{Derivation, WaitCause},
    Sandbox,
};
use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use sqlx::Row;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    path::Path,
};

mod cost;
mod facts;
mod render;
#[cfg(test)]
mod tests;

pub use cost::Samples;
pub(crate) use facts::*;
use render::*;

/// The kinds of approval prompt the user counts during a run (protocol §6).
pub const APPROVAL_KINDS: [&str; 4] = ["cbc", "native_setup", "native_refusal", "other"];

/// The report's time window, in wall milliseconds, both ends inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub from_ms: i64,
    pub until_ms: i64,
}

impl Window {
    fn holds(&self, wall_ms: i64) -> bool {
        (self.from_ms..=self.until_ms).contains(&wall_ms)
    }
    /// A stamp counts only if it happened by `until`.
    fn seen<'a>(&self, stamp: &'a Option<Stamp>) -> Option<&'a Stamp> {
        stamp.as_ref().filter(|s| s.wall_ms <= self.until_ms)
    }
    /// Replaced by a newer notice by `until`, with no receipt by then: neither pending nor
    /// waited on.
    fn superseded(&self, delivery: &Delivery) -> bool {
        matches!(delivery.superseded, Superseded::At(at) if at <= self.until_ms)
    }
    /// Possibly replaced by `until`, at a time no record gives: neither pending nor waited on is
    /// known. Before the newer notice it was still pending.
    fn undated(&self, delivery: &Delivery) -> bool {
        matches!(delivery.superseded, Superseded::Undated { after } if after <= self.until_ms)
    }
    fn length(&self) -> i64 {
        self.until_ms - self.from_ms
    }
}

/// Count, median, p90 and maximum by nearest rank, in whole milliseconds.
pub(crate) fn distribution(values: &[i64]) -> String {
    if values.is_empty() {
        return "n=0".into();
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = |p: usize| sorted[(p * sorted.len()).div_ceil(100) - 1];
    format!(
        "n={} median={} p90={} max={}",
        sorted.len(),
        rank(50),
        rank(90),
        sorted[sorted.len() - 1]
    )
}

/// The total length of the union of half-open intervals.
pub(super) fn union_length(intervals: &[(i64, i64)]) -> i64 {
    let mut sorted = intervals.to_vec();
    sorted.sort_unstable();
    let mut total = 0;
    let mut current: Option<(i64, i64)> = None;
    for (start, end) in sorted {
        match current {
            Some((s, e)) if start <= e => current = Some((s, e.max(end))),
            Some((s, e)) => {
                total += e - s;
                current = Some((start, end));
            }
            None => current = Some((start, end)),
        }
    }
    total + current.map_or(0, |(s, e)| e - s)
}

/// A seat as (context, member). IDs may contain `/`, so a joined string could collide.
pub(crate) type SeatId = (String, String);

/// One wait, clipped to the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WaitInterval {
    pub seat: SeatId,
    /// None when the records do not say why: printed as `unknown`.
    pub cause: Option<WaitCause>,
    pub id: String,
    pub start_ms: i64,
    pub end_ms: i64,
    /// Still open at `until`.
    pub open: bool,
}

impl WaitInterval {
    fn duration(&self) -> i64 {
        self.end_ms - self.start_ms
    }
}

/// Clip `[start, end)` to the window; None when it lies outside. An end after `until`, or none,
/// leaves the wait open at `until`.
pub(super) fn clip(window: Window, start: i64, end: Option<i64>) -> Option<(i64, i64, bool)> {
    if start > window.until_ms {
        return None;
    }
    let (end, open) = match end {
        Some(end) if end <= window.until_ms => (end, false),
        _ => (window.until_ms, true),
    };
    if end < window.from_ms {
        return None;
    }
    Some((start.max(window.from_ms), end, open))
}

/// A seat as printed: `context/member`, each part quoted when either contains `/`, so two
/// seats never print alike.
pub(super) fn seat_label(context: &str, member: &str) -> String {
    if context.contains('/') || member.contains('/') {
        format!("{context:?}/{member:?}")
    } else {
        format!("{context}/{member}")
    }
}

fn seat_id(context: &str, member: &str) -> SeatId {
    (context.to_owned(), member.to_owned())
}

/// A withdrawal with no recorded time (from before the time was recorded) and no answer cannot be
/// placed.
pub(super) fn untimed(request: &RequestFact) -> bool {
    // A withdrawal comes after any answer, so an answer alone already ends the wait.
    request.withdrawn && request.withdrawn_wall_ms.is_none() && request.answered_wall_ms.is_none()
}

/// Waits use wall time, the one timeline that every source shares and the window is cut on.
pub(crate) fn waits(facts: &Facts, window: Window) -> Vec<WaitInterval> {
    let roles: BTreeMap<(&str, &str), &str> = facts
        .seats
        .iter()
        .map(|s| ((s.context.as_str(), s.member.as_str()), s.role.as_str()))
        .collect();
    let mut waits = Vec::new();
    let mut push = |seat: (String, String), cause, id: String, start, end| {
        if let Some((start_ms, end_ms, open)) = clip(window, start, end) {
            waits.push(WaitInterval {
                seat,
                cause,
                id,
                start_ms,
                end_ms,
                open,
            });
        }
    };
    for request in facts.requests.iter().filter(|r| !untimed(r)) {
        // As `waits_now`: the owner's role decides, and a relayed answer ends the wait too,
        // since the asker then has an answer to act on.
        let role = request.owner_member.as_ref().and_then(|owner| {
            roles
                .get(&(request.context.as_str(), owner.as_str()))
                .copied()
        });
        let cause = match (request.owner_is_user, role) {
            (true, _) => Some(WaitCause::User),
            (false, Some("repo_lead")) => Some(WaitCause::Primarch),
            (false, Some("feature_lead")) => Some(WaitCause::ChapterMaster),
            _ => None,
        };
        let end = match (request.answered_wall_ms, request.withdrawn_wall_ms) {
            (Some(a), Some(w)) => Some(a.min(w)),
            (a, w) => a.or(w),
        };
        push(
            seat_id(&request.context, &request.callback),
            cause,
            format!("request {}", request.request),
            request.created_wall_ms,
            end,
        );
    }
    // A notice replaced at an unknown time has no known end: left out and counted.
    for delivery in facts.deliveries.iter().filter(|d| !window.undated(d)) {
        let seat = seat_id(&delivery.context, &delivery.member);
        let id = format!("item {}", delivery.sequence);
        push(
            seat.clone(),
            Some(WaitCause::Delivery),
            id.clone(),
            delivery.committed.wall_ms,
            match (&delivery.receipted, delivery.superseded) {
                (Some(receipted), _) => Some(receipted.wall_ms),
                (None, Superseded::At(at)) => Some(at),
                (None, _) => None,
            },
        );
        if let Some(receipted) = window.seen(&delivery.receipted) {
            push(
                seat,
                Some(WaitCause::ReadyIdle),
                id,
                receipted.wall_ms,
                delivery.active.as_ref().map(|s| s.wall_ms),
            );
        }
    }
    waits.sort_by(|a, b| {
        (&a.seat, a.cause, a.start_ms, &a.id).cmp(&(&b.seat, b.cause, b.start_ms, &b.id))
    });
    waits
}

pub(super) fn cause_label(cause: Option<WaitCause>) -> &'static str {
    cause.map_or("unknown", WaitCause::label)
}

/// Context, then member, then kind: nested, because a context or member ID may contain `/`.
type Counts = BTreeMap<String, BTreeMap<String, BTreeMap<String, u64>>>;

/// The user's approval-prompt tally, a file of its own so the manifest is never edited after
/// the go. Validated against the seats in the end snapshot.
pub struct Tally {
    pub name: String,
    pub sha256: String,
    counts: Counts,
}

impl Tally {
    pub fn parse(name: &str, bytes: &[u8], seats: &[Seat]) -> Result<Self> {
        let known: BTreeSet<(&str, &str)> = seats
            .iter()
            .map(|s| (s.context.as_str(), s.member.as_str()))
            .collect();
        let counts: Counts = serde_json::from_slice(bytes)
            .context("tally must map context to member to kind to count")?;
        for (context, members) in &counts {
            for (member, kinds) in members {
                ensure!(
                    known.contains(&(context.as_str(), member.as_str())),
                    "tally names an unknown seat: {context} {member}"
                );
                for kind in kinds.keys() {
                    ensure!(
                        APPROVAL_KINDS.contains(&kind.as_str()),
                        "tally names an unknown kind: {kind}"
                    );
                }
            }
        }
        use sha2::Digest;
        let digest = sha2::Sha256::digest(bytes);
        Ok(Self {
            name: name.to_owned(),
            sha256: digest.iter().map(|b| format!("{b:02x}")).collect(),
            counts,
        })
    }
}

/// The `taken_wall_ms` a snapshot's own name carries.
fn snapshot_time(file: &Path) -> Result<i64> {
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .context("snapshot name must be UTF-8")?;
    let rest = name
        .strip_prefix("snapshot-")
        .context("not a snapshot name")?;
    let (ms, _) = rest.split_once('-').context("not a snapshot name")?;
    Ok(ms.parse()?)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// What the `report` command was given.
pub struct ReportArgs<'a> {
    pub manifest_name: &'a str,
    pub manifest: &'a [u8],
    pub end: &'a Path,
    pub tally: Option<(&'a str, &'a [u8])>,
    /// The sampler's JSONL file (C3), for the Cost section.
    pub samples: &'a [(&'a str, &'a [u8])],
    pub from_ms: Option<i64>,
    pub until_ms: Option<i64>,
}

/// The `report` command. Verifies the baseline snapshot against the manifest, fixes the window
/// (defaults: the baseline and end snapshot times; `until` may not pass the end snapshot, since
/// nothing later is in it), and renders.
pub async fn report(sandbox: &Sandbox, args: ReportArgs<'_>) -> Result<String> {
    let manifest: Value = serde_json::from_slice(args.manifest).context("invalid manifest")?;
    let text = |pointer: &str| -> Result<String> {
        Ok(manifest
            .pointer(pointer)
            .and_then(Value::as_str)
            .with_context(|| format!("manifest lacks {pointer}"))?
            .to_owned())
    };
    let number = |pointer: &str| -> Result<i64> {
        manifest
            .pointer(pointer)
            .and_then(Value::as_i64)
            .with_context(|| format!("manifest lacks {pointer}"))
    };
    let baseline = std::path::PathBuf::from(text("/snapshot/file")?);
    // Only a candidate snapshot is hashed, never whatever path a manifest names.
    Evidence::open(sandbox, &baseline)
        .await
        .context("the manifest's snapshot is not a candidate snapshot")?
        .close()
        .await;
    let baseline_sha256 = sha256_file(&baseline)?;
    if baseline_sha256 != text("/snapshot/sha256")? {
        bail!("baseline snapshot does not match the manifest");
    }
    let baseline_ms = number("/snapshot/taken_wall_ms")?;
    let end_ms = snapshot_time(args.end)?;
    ensure!(end_ms >= baseline_ms, "end snapshot predates the baseline");
    let window = Window {
        from_ms: args.from_ms.unwrap_or(baseline_ms),
        until_ms: args.until_ms.unwrap_or(end_ms),
    };
    ensure!(
        window.until_ms <= end_ms,
        "until is after the end snapshot, which cannot show anything later"
    );
    ensure!(window.from_ms <= window.until_ms, "from is after until");

    let evidence = Evidence::open(sandbox, args.end).await?;
    let facts = Facts::load(&evidence).await;
    evidence.close().await;
    let facts = facts?;
    let tally = args
        .tally
        .map(|(name, bytes)| Tally::parse(name, bytes, &facts.seats))
        .transpose()?;
    let samples = Samples::in_order(
        args.samples
            .iter()
            .map(|(name, bytes)| Samples::parse(name, bytes))
            .collect::<Result<_>>()?,
    )?;
    use sha2::Digest;
    let header = Header {
        trial: u64::try_from(number("/trial")?)?,
        build_commit: text("/build/commit")?,
        manifest_name: args.manifest_name.to_owned(),
        manifest_sha256: sha2::Sha256::digest(args.manifest)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        baseline_name: file_name(&baseline),
        baseline_sha256,
        end_name: file_name(args.end),
        end_sha256: sha256_file(args.end)?,
    };
    Ok(render(&header, &facts, tally.as_ref(), &samples, window))
}
