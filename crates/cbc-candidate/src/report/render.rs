//! Rendering protocol §8 from loaded facts. Pure: no clock and no I/O.
use super::*;

/// What the header prints, all from verified inputs.
pub(crate) struct Header {
    pub trial: u64,
    pub build_commit: String,
    pub manifest_name: String,
    pub manifest_sha256: String,
    pub baseline_name: String,
    pub baseline_sha256: String,
    pub end_name: String,
    pub end_sha256: String,
}

pub(super) fn missing(reason: &str) -> String {
    format!("missing ({reason})")
}

/// Renders protocol §8 in order. Pure: no I/O, no clock.
pub(crate) fn render(
    header: &Header,
    facts: &Facts,
    tally: Option<&Tally>,
    samples: &[Samples],
    window: Window,
) -> String {
    let mut out = String::new();
    let o = &mut out;
    let _ = writeln!(
        o,
        "Trial {} — build {} — result: {}",
        header.trial,
        header.build_commit,
        missing("recorded by the trial manager")
    );
    let _ = writeln!(
        o,
        "Window: {} .. {} wall ms ({} ms)",
        window.from_ms,
        window.until_ms,
        window.length()
    );
    let _ = writeln!(
        o,
        "Manifest: {}, sha256 {}",
        header.manifest_name, header.manifest_sha256
    );
    let _ = writeln!(
        o,
        "Baseline snapshot: {}, sha256 {} (verified against the manifest)",
        header.baseline_name, header.baseline_sha256
    );
    let _ = writeln!(
        o,
        "End snapshot: {}, sha256 {}",
        header.end_name, header.end_sha256
    );
    match tally {
        Some(t) => {
            let _ = writeln!(o, "Tally: {}, sha256 {}", t.name, t.sha256);
        }
        None => {
            let _ = writeln!(o, "Tally: {}", missing("no tally file given"));
        }
    }
    if samples.is_empty() {
        let _ = writeln!(o, "Samples: {}", missing("no sampler file given"));
    }
    for s in samples {
        let _ = writeln!(o, "Samples: {}, sha256 {}", s.name, s.sha256);
    }
    let _ = writeln!(
        o,
        "\nPhase 1: {}",
        missing("judged by the trial manager against protocol section 4")
    );

    render_waits(o, facts, window);
    render_approvals(o, facts, tally);
    render_delivery(o, facts, window);
    render_latency(o, facts, window);

    let _ = writeln!(o, "\nAutonomy");
    let merges = facts.merges.iter().filter(|at| window.holds(**at)).count();
    let _ = writeln!(
        o,
        "  merges recorded on work items: {merges} (from work events; an item with no report room is not seen)"
    );
    for line in [
        "merges without a Primarch yes",
        "routine permission messages",
        "go-ahead requests",
    ] {
        let _ = writeln!(
            o,
            "  {line}: {}",
            missing("needs PR observations and message classification")
        );
    }

    render_rooms_identity(o, facts, window);

    let _ = writeln!(o, "\nPrimarch");
    let _ = writeln!(
        o,
        "  interventions by kind: {}",
        missing("no intervention record")
    );
    let _ = writeln!(
        o,
        "  turns per hour: {}",
        missing("the core sees CBC items, not turns")
    );
    let hours_x1000 = window.length().max(1) * 1000 / 3_600_000;
    for seat in facts.seats.iter().filter(|s| s.role == "repo_lead") {
        let sent = facts
            .sent
            .iter()
            .filter(|i| i.context == seat.context && i.sender == seat.member)
            .filter(|i| window.holds(i.wall_ms))
            .count();
        let _ = writeln!(
            o,
            "  {}: {sent} CBC items sent in {}.{:03} h",
            seat_label(&seat.context, &seat.member),
            hours_x1000 / 1000,
            hours_x1000 % 1000
        );
    }

    cost::render_cost(o, samples, facts, window);
    for section in ["Outcome", "Findings", "Changes for the next trial"] {
        let _ = writeln!(
            o,
            "\n{section}: {}",
            missing("written by the trial manager")
        );
    }
    out
}

pub(super) fn render_waits(o: &mut String, facts: &Facts, window: Window) {
    let waits = waits(facts, window);
    let _ = writeln!(o, "\nWaits (wall time, clipped to the window)");
    let mut groups: BTreeMap<(&SeatId, Option<WaitCause>), Vec<&WaitInterval>> = BTreeMap::new();
    for wait in &waits {
        groups
            .entry((&wait.seat, wait.cause))
            .or_default()
            .push(wait);
    }
    if groups.is_empty() {
        let _ = writeln!(o, "  no waits in the window");
    }
    for (((context, member), cause), group) in &groups {
        let durations: Vec<i64> = group.iter().map(|w| w.duration()).collect();
        let spans: Vec<(i64, i64)> = group.iter().map(|w| (w.start_ms, w.end_ms)).collect();
        let open = group.iter().filter(|w| w.open).count();
        let share = union_length(&spans) * 1000 / window.length().max(1);
        let _ = writeln!(
            o,
            "  {} — {}: count={} open={open} sum={} ms {} share={}.{}%",
            seat_label(context, member),
            cause_label(*cause),
            group.len(),
            durations.iter().sum::<i64>(),
            distribution(&durations),
            share / 10,
            share % 10
        );
    }
    let undated = facts
        .requests
        .iter()
        .filter(|r| untimed(r) && r.created_wall_ms <= window.until_ms)
        .count();
    if undated > 0 {
        let _ = writeln!(
            o,
            "  requests withdrawn with no recorded time: {undated} (left out)"
        );
    }
    for cause in WaitCause::ALL {
        match cause.derivation() {
            Derivation::Known => {}
            Derivation::Partial { missing: what } => {
                let _ = writeln!(o, "  {}: partial — {what}", cause.label());
            }
            Derivation::Unknown { needs } => {
                let _ = writeln!(o, "  {}: {}", cause.label(), missing(needs));
            }
        }
    }
    let mut longest: Vec<&WaitInterval> = waits.iter().collect();
    longest.sort_by(|a, b| {
        b.duration()
            .cmp(&a.duration())
            .then_with(|| (&a.seat, &a.id, a.cause).cmp(&(&b.seat, &b.id, b.cause)))
    });
    let _ = writeln!(o, "  Ten longest:");
    for wait in longest.iter().take(10) {
        let _ = writeln!(
            o,
            "    {} ms — {} — {} — {} — +{} ms .. +{} ms{}",
            wait.duration(),
            seat_label(&wait.seat.0, &wait.seat.1),
            cause_label(wait.cause),
            wait.id,
            wait.start_ms - window.from_ms,
            wait.end_ms - window.from_ms,
            if wait.open { " (open at until)" } else { "" }
        );
    }
}

pub(super) fn render_approvals(o: &mut String, facts: &Facts, tally: Option<&Tally>) {
    let _ = writeln!(o, "\nApproval prompts (per session × kind)");
    for seat in &facts.seats {
        let key = seat_label(&seat.context, &seat.member);
        let kinds = tally
            .and_then(|t| t.counts.get(&seat.context))
            .and_then(|members| members.get(&seat.member));
        let cells: Vec<String> = APPROVAL_KINDS
            .iter()
            .map(|kind| match kinds.and_then(|k| k.get(*kind)) {
                Some(count) => format!("{kind}={count}"),
                None => format!("{kind}={}", missing("not in the tally")),
            })
            .collect();
        let _ = writeln!(o, "  {key}: {}", cells.join(" "));
    }
}

pub(super) fn render_delivery(o: &mut String, facts: &Facts, window: Window) {
    let committed: Vec<&Delivery> = facts
        .deliveries
        .iter()
        .filter(|d| window.holds(d.committed.wall_ms))
        .collect();
    let receipted = committed
        .iter()
        .filter(|d| window.seen(&d.receipted).is_some())
        .count();
    let superseded = committed.iter().filter(|d| window.superseded(d)).count();
    let undated = committed.iter().filter(|d| window.undated(d)).count();
    // Pending only: a receipt with no offer stamp (a fact older than offers) is receipted.
    let never_offered = committed
        .iter()
        .filter(|d| window.seen(&d.receipted).is_none() && !window.superseded(d))
        .filter(|d| !window.undated(d))
        .filter(|d| window.seen(&d.offered).is_none())
        .count();
    let offered_unreceipted = committed.len() - receipted - superseded - undated - never_offered;
    let _ = writeln!(
        o,
        "\nDelivery (expected recipients of items committed in the window)"
    );
    let _ = writeln!(
        o,
        "  committed={} receipted={receipted} superseded={superseded} failed={}",
        committed.len(),
        missing("no delivery-failure record")
    );
    let _ = writeln!(
        o,
        "  pending at until: never offered={never_offered} offered, not receipted={offered_unreceipted}"
    );
    if undated > 0 {
        let _ = writeln!(
            o,
            "  superseded at an unknown time: {undated} (offered, then dropped unreceipted; left out of pending and the waits)"
        );
    }
    let mut wakes: BTreeMap<&str, (usize, BTreeMap<&str, usize>)> = BTreeMap::new();
    for wake in facts
        .wakes
        .iter()
        .filter(|w| window.holds(w.reported_wall_ms))
    {
        let entry = wakes.entry(&wake.kind).or_default();
        match &wake.failure {
            None => entry.0 += 1,
            Some(class) => *entry.1.entry(class).or_default() += 1,
        }
    }
    if wakes.is_empty() {
        let _ = writeln!(o, "  wake attempts: none reported in the window");
    }
    for (kind, (accepted, failures)) in &wakes {
        let failed: Vec<String> = failures.iter().map(|(c, n)| format!("{c}={n}")).collect();
        let _ = writeln!(
            o,
            "  wake attempts {kind}: accepted={accepted} failed=[{}]",
            failed.join(" ")
        );
    }
}

pub(super) fn render_latency(o: &mut String, facts: &Facts, window: Window) {
    let mut profiles: BTreeMap<String, Vec<&Delivery>> = BTreeMap::new();
    for delivery in facts
        .deliveries
        .iter()
        .filter(|d| window.holds(d.committed.wall_ms))
    {
        // The binding in force at commit: the latest one dated at or before it.
        let binding = facts
            .bindings
            .iter()
            .filter(|b| b.context == delivery.context && b.member == delivery.member)
            .filter(|b| b.from_wall_ms <= delivery.committed.wall_ms)
            .max_by_key(|b| (b.from_wall_ms, b.epoch));
        let profile = match binding.and_then(|b| b.host.as_deref().map(|h| (h, b))) {
            Some((host, b)) => format!("{host}/{}", b.profile.as_deref().unwrap_or("unknown")),
            None => "unknown".to_owned(),
        };
        profiles.entry(profile).or_default().push(delivery);
    }
    let _ = writeln!(
        o,
        "\nWake latency (monotonic within a core instance; across instances wall time, counted as estimates)"
    );
    if profiles.is_empty() {
        let _ = writeln!(o, "  no items committed in the window");
    }
    for (host, deliveries) in &profiles {
        for (label, pick) in [
            (
                "commit to wake accepted",
                (|d: &Delivery| d.woken.clone()) as fn(&Delivery) -> Option<Stamp>,
            ),
            ("commit to first activity", |d: &Delivery| d.active.clone()),
        ] {
            let mut values = Vec::new();
            let mut estimates = 0;
            let mut absent = 0;
            for delivery in deliveries {
                match window.seen(&pick(delivery)) {
                    Some(stamp) => {
                        let (ms, estimate) = delivery.committed.until(stamp);
                        values.push(ms);
                        estimates += usize::from(estimate);
                    }
                    // A replaced notice is never woken for: its successor carries the news.
                    None if window.superseded(delivery) || window.undated(delivery) => {}
                    None => absent += 1,
                }
            }
            let _ = writeln!(
                o,
                "  {host} {label}: {} missing={absent} estimates={estimates}",
                distribution(&values)
            );
        }
    }
}

pub(super) fn render_rooms_identity(o: &mut String, facts: &Facts, window: Window) {
    let _ = writeln!(o, "\nRooms and identity");
    let mut chains: BTreeMap<(&str, &str, &str), Vec<&str>> = BTreeMap::new();
    for connection in facts.connections.iter().filter(|c| {
        c.supersedes.is_none() && c.introduced_wall_ms.is_none_or(|t| t <= window.until_ms)
    }) {
        let (a, b) = if connection.first <= connection.second {
            (&connection.first, &connection.second)
        } else {
            (&connection.second, &connection.first)
        };
        chains
            .entry((&connection.context, a, b))
            .or_default()
            .push(&connection.room);
    }
    let duplicates: Vec<String> = chains
        .iter()
        .filter(|(_, rooms)| rooms.len() > 1)
        .map(|((c, a, b), rooms)| format!("{c}/{a}+{b}: {} rooms", rooms.len()))
        .collect();
    // Every chain head beyond a pair's first is one duplicate room.
    let extra: usize = chains.values().map(|rooms| rooms.len() - 1).sum();
    let _ = writeln!(
        o,
        "  duplicate peer rooms: {extra}{}",
        if duplicates.is_empty() {
            String::new()
        } else {
            format!(" [{}]", duplicates.join("; "))
        }
    );
    let changes: Vec<String> = facts
        .identity
        .iter()
        .filter(|e| e.kind == "rebound" && window.holds(e.at_wall_ms))
        .map(|e| {
            format!(
                "{} epoch {} at +{} ms",
                seat_label(&e.context, &e.member),
                e.epoch,
                e.at_wall_ms - window.from_ms
            )
        })
        .collect();
    let _ = writeln!(
        o,
        "  identity changes (rebinds): {}{}",
        changes.len(),
        if changes.is_empty() {
            String::new()
        } else {
            format!(" [{}]", changes.join("; "))
        }
    );
    for kind in ["enrolled", "retired"] {
        let n = facts
            .identity
            .iter()
            .filter(|e| e.kind == kind && window.holds(e.at_wall_ms))
            .count();
        let _ = writeln!(o, "  {kind} in the window: {n}");
    }
    let _ = writeln!(
        o,
        "  legacy skills loaded: {}",
        missing("the core does not see skills")
    );
}
