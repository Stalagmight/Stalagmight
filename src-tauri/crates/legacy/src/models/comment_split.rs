//! Round-trippable shot/station comment encoding for Ariane `.tml` Comment fields.
//!
//! The TML format has one Comment field per shot but no per-station comment field,
//! so a Stalagmight station comment otherwise has nowhere to live in `.tml`. We
//! attach each station comment to the shot whose "to" station matches it and
//! distinguish shot vs station lines using a zero-width-space (`\u{200B}`) prefix:
//!
//! - Lines prefixed with `ZWSP` are shot comments.
//! - Lines without the prefix are station comments.
//!
//! This survives a round-trip through Ariane: the marker is invisible to humans
//! and Ariane preserves Comment text verbatim. New text Ariane users add carries
//! no marker, which routes it to the station - matching how Ariane visually
//! displays Comment text (centered on the station, not along the shot).

/// Zero-width space - the per-line marker that classifies a line as a shot
/// comment. Single Unicode codepoint (3 bytes UTF-8). Invisible in any editor.
pub const SHOT_MARKER: char = '\u{200B}';

/// Encode a shot comment and a station comment into a single TML Comment field.
///
/// Lines of `shot` are prefixed with [`SHOT_MARKER`]; lines of `station` are
/// written afterwards, unmarked. Returns `None` when both inputs are empty (so
/// callers can leave the field unset). Empty inputs are treated as None;
/// whitespace-only inputs are user content and kept.
///
/// The marker is the encoding's own character, so CONTENT cannot carry one
/// into the field: a station line that began with a marker would come back as
/// a shot line, silently changing side. TML ingestion already strips markers
/// (#483), but text typed in the UI or arriving through another importer has
/// no such guarantee - so the encoder strips them from both inputs itself.
pub fn merge(shot: Option<&str>, station: Option<&str>) -> Option<String> {
    let shot = shot.map(strip_markers).filter(|s| !s.is_empty());
    let station = station.map(strip_markers).filter(|s| !s.is_empty());
    match (shot, station) {
        (None, None) => None,
        (Some(s), None) => Some(prefix_lines(&s)),
        (None, Some(st)) => Some(st),
        (Some(s), Some(st)) => Some(format!("{}\n{}", prefix_lines(&s), st)),
    }
}

/// Decode a TML Comment field into `(shot_comment, station_comment)`.
///
/// Each line is classified by whether its first char is [`SHOT_MARKER`]. The
/// marker is stripped from shot lines; station lines are kept verbatim. Returns
/// `(None, None)` for an empty input. Either component can be `None` if no lines
/// of that class were present.
pub fn split(s: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(s) = s.and_then(non_empty) else {
        return (None, None);
    };
    let mut shot_lines: Vec<&str> = Vec::new();
    let mut station_lines: Vec<&str> = Vec::new();
    for line in s.split('\n') {
        match line.strip_prefix(SHOT_MARKER) {
            Some(rest) => shot_lines.push(rest),
            None => station_lines.push(line),
        }
    }
    (
        (!shot_lines.is_empty()).then(|| shot_lines.join("\n")),
        (!station_lines.is_empty()).then(|| station_lines.join("\n")),
    )
}

/// Remove every [`SHOT_MARKER`] from a decoded comment.
///
/// [`split`] only consumes the marker that classifies a line, because that is
/// the only position where it carries meaning. A marker anywhere else is an
/// invisible character with no semantics, and there is no way to type one - so
/// it can only have arrived from a foreign writer, or from an earlier version of
/// this importer that skipped the decode. Either way it must not reach the
/// project: it is invisible in the UI, and it survives export - `escape_csv`
/// (`export/csv.rs`) quotes delimiters and defuses formula injection but passes
/// zero-width text through untouched, and the JSON writer emits the comment
/// verbatim.
///
/// This removes the marker and nothing else. The wider hostile-character class
/// that `export/dive_profile.rs` refuses on the way out - bidi overrides, line
/// and paragraph separators, C0 controls - is NOT handled here: it arrives from
/// every importer, not just this encoding, and sanitizing it belongs at a
/// boundary all of them share.
///
/// Applied at ingestion, and by [`merge`] to its CONTENT inputs before it adds
/// the encoding's own markers - never to an already-encoded field, where
/// stripping would erase the encoding itself.
pub fn strip_markers(s: &str) -> String {
    s.replace(SHOT_MARKER, "")
}

fn non_empty(s: &str) -> Option<&str> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn prefix_lines(s: &str) -> String {
    s.split('\n')
        .map(|line| {
            let mut out = String::with_capacity(line.len() + SHOT_MARKER.len_utf8());
            out.push(SHOT_MARKER);
            out.push_str(line);
            out
        })
        .collect::<Vec<_>>()
        .join("\n")
}
