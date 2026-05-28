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
pub fn share_line(room_id: &str) -> String {
    format!("/cbc-join {room_id}")
}

fn kebab(input: &str) -> String {
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
    fn share_line_format() {
        assert_eq!(
            share_line("abc-20260102-0304"),
            "/cbc-join abc-20260102-0304"
        );
    }
}
