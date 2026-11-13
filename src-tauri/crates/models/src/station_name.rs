//! Station names: the short free-text identifier a station may carry beside its
//! positional `S:I` id - the station number an mNemo dump gave it (a DMP import
//! records it), or one typed in the Data workspace. Stored in
//! [`crate::Project::station_names`].

/// Longest station name an edit stores, in chars. Longer input is cut on a char
/// boundary, the same rule a credited person's name follows
/// ([`crate::MAX_PERSON_TEXT_CHARS`]): a name is a label, and an unbounded one
/// would ride into every save, undo entry, map label and export.
pub const MAX_STATION_NAME_CHARS: usize = 256;

/// Whether `c` can never stay in a stored station name: a control character, one
/// of the two noncharacters XML 1.0 forbids (a name reaches the Ariane export),
/// or an invisible bidi control that could reorder or disguise the positional id
/// displayed beside the name.
///
/// The same set `isUnsafeForDisplay` (src/utils/formatters.ts) strips when a name
/// is shown, so what an edit stores is what the UI displays.
pub fn is_unsafe_in_station_name(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{fffe}'
                | '\u{ffff}'
                | '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
}

/// A station name as an edit stores it, or `None` when nothing is left (an empty
/// or whitespace-only name removes the station's name).
///
/// Unsafe characters ([`is_unsafe_in_station_name`]) are dropped, except that a
/// whitespace control (a tab or newline from a pasted cell) becomes a space, as
/// in [`crate::clean_text`]. Then surrounding whitespace is trimmed and the
/// result is clamped to [`MAX_STATION_NAME_CHARS`].
///
/// Idempotent: a name it returns comes back unchanged, so redoing an edit lands
/// on exactly what the edit stored.
pub fn sanitize_station_name(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter_map(|c| match c {
            c if !is_unsafe_in_station_name(c) => Some(c),
            c if c.is_whitespace() => Some(' '),
            _ => None,
        })
        .collect();
    let trimmed = cleaned.trim();
    let clamped = match trimmed.char_indices().nth(MAX_STATION_NAME_CHARS) {
        Some((cut, _)) => trimmed[..cut].trim_end(),
        None => trimmed,
    };
    (!clamped.is_empty()).then(|| clamped.to_string())
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn keeps_an_ordinary_name_and_trims_it() {
        assert_eq!(sanitize_station_name("17").as_deref(), Some("17"));
        assert_eq!(
            sanitize_station_name("  A12 east \t").as_deref(),
            Some("A12 east")
        );
        assert_eq!(sanitize_station_name("נקודה 3").as_deref(), Some("נקודה 3"));
    }

    #[test]
    fn empty_and_whitespace_only_mean_remove() {
        assert_eq!(sanitize_station_name(""), None);
        assert_eq!(sanitize_station_name("   \t\n "), None);
        // Nothing but invisible controls is empty too.
        assert_eq!(sanitize_station_name("\u{202e}\u{200f}\u{0007}"), None);
    }

    #[test]
    fn strips_controls_and_bidi_overrides() {
        assert_eq!(
            sanitize_station_name("A\u{202e}B\u{2066}C\u{0000}D\u{fffe}").as_deref(),
            Some("ABCD")
        );
        // A newline inside a pasted name becomes a space instead of gluing words.
        assert_eq!(
            sanitize_station_name("Sump\nentrance").as_deref(),
            Some("Sump entrance")
        );
    }

    #[test]
    fn clamps_to_the_char_limit_on_a_char_boundary() {
        let long = "é".repeat(MAX_STATION_NAME_CHARS + 50);
        let name = sanitize_station_name(&long).unwrap();
        assert_eq!(name.chars().count(), MAX_STATION_NAME_CHARS);
        // A cut that lands after a space does not leave trailing whitespace.
        let spaced = format!("{} tail", "x".repeat(MAX_STATION_NAME_CHARS - 1));
        let name = sanitize_station_name(&spaced).unwrap();
        assert_eq!(name, "x".repeat(MAX_STATION_NAME_CHARS - 1));
    }

    #[test]
    fn is_idempotent() {
        let long = "z".repeat(400);
        for raw in ["  a\tb  ", "A\u{202e}B", long.as_str(), "17", " x \n y "] {
            let once = sanitize_station_name(raw).unwrap();
            assert_eq!(sanitize_station_name(&once).as_deref(), Some(once.as_str()));
        }
    }
}
