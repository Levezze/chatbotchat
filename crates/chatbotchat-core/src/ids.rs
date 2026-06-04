use time::OffsetDateTime;

/// Derive a room id of the form `<subject-kebab>-<YYYYMMDD-HHMM>`.
///
/// Pure: the timestamp is passed in so callers control it and tests stay
/// deterministic. The slug is lowercased, non-alphanumeric runs collapse to a
/// single `-`, and leading/trailing dashes are trimmed. An empty slug falls
/// back to `room`.
pub fn room_id(subject: &str, now: OffsetDateTime) -> String {
    let slug = kebab(subject);
    let slug = if slug.is_empty() { "room" } else { &slug };
    let stamp = format!(
        "{:04}{:02}{:02}-{:02}{:02}",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
    );
    format!("{slug}-{stamp}")
}

/// The line a user pastes into another agent's session to join a room.
///
/// Deliberately slash-free: a leading `/` makes agents misread it as a slash
/// command / skill (there is no `cbc-join` skill — that misread was the original
/// failure). It leads with the bare room id so a human can copy just the id, and
/// the receiving agent recognizes the `slug-YYYYMMDD-HHMM` shape and calls
/// `cbc_join_room` (see the MCP server instructions).
pub fn share_line(room_id: &str) -> String {
    format!("Join CBC room {room_id}")
}

/// Lowercase, collapse non-alphanumeric runs to a single `-`, trim dashes.
/// Shared by room-id derivation and participant-handle derivation.
pub(crate) fn kebab(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_dash = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn builds_kebab_plus_timestamp() {
        let now = datetime!(2026-05-28 14:23:09 UTC);
        assert_eq!(room_id("Slider Labels", now), "slider-labels-20260528-1423");
    }

    #[test]
    fn collapses_punctuation_and_trims() {
        let now = datetime!(2026-01-02 03:04:00 UTC);
        assert_eq!(
            room_id("  Per-position slider labels?! ", now),
            "per-position-slider-labels-20260102-0304"
        );
    }

    #[test]
    fn empty_subject_falls_back() {
        let now = datetime!(2026-01-02 03:04:00 UTC);
        assert_eq!(room_id("!!!", now), "room-20260102-0304");
    }

    #[test]
    fn share_line_is_slash_free_and_carries_the_room_id() {
        let line = share_line("abc-20260102-0304");
        assert!(
            line.contains("abc-20260102-0304"),
            "share line must carry the bare room id so the user can paste it; got: {line}"
        );
        assert!(
            !line.starts_with('/'),
            "share line must not start with a slash — agents misread it as a slash command; got: {line}"
        );
        assert!(
            !line.contains("/cbc-join"),
            "the /cbc-join slash trap must be gone (no such skill exists); got: {line}"
        );
    }
}
