//! The station labels of an image export: what a station name draws as, and
//! which station labels a point where stations are colocated (merged, or the
//! two ends of an aligned loop closure). Kept beside `image.rs`, whose Pass 4a
//! draws them.

use std::collections::HashMap;

use super::image::is_xml_illegal_char;
use super::image_sketch::SketchLine;
use crate::models::{LineId, Station};

/// The longest station name the export draws whole; longer names are cut to
/// one char less and end in an ellipsis. Matches the live map's cap.
pub(super) const STATION_NAME_LABEL_MAX_CHARS: usize = 24;

/// A station name as the export draws it: control characters and bidi
/// overrides dropped (names come from the file, so they are untrusted text),
/// trimmed, and capped at [`STATION_NAME_LABEL_MAX_CHARS`]. `None` when nothing
/// is left, so an unnamed station gets no label rather than its index.
///
/// Mirrors the live map's `formatStationNameLabel` (`labelText.ts`), so the
/// preview and the file say the same thing: it also drops the Arabic letter
/// mark (U+061C) that `sanitize_display_text` keeps and the XML-illegal
/// U+FFFE/U+FFFF (a name of only those would otherwise reserve space for an
/// empty `<text>`), and trims a byte-order mark (U+FEFF) the way JavaScript's
/// `trim` does - at both ends and again at the cut.
pub(super) fn station_name_label(name: Option<&str>) -> Option<String> {
    let blank = |c: char| c.is_whitespace() || c == '\u{feff}';
    let clean: String = crate::models::sanitize_display_text(name?, usize::MAX)
        .chars()
        .filter(|&c| c != '\u{061c}' && !is_xml_illegal_char(c))
        .collect();
    let clean = clean.trim_matches(blank);
    if clean.is_empty() {
        return None;
    }
    if clean.chars().count() <= STATION_NAME_LABEL_MAX_CHARS {
        return Some(clean.to_string());
    }
    let mut out: String = clean
        .chars()
        .take(STATION_NAME_LABEL_MAX_CHARS - 1)
        .collect();
    out.truncate(out.trim_end_matches(blank).len());
    out.push('\u{2026}');
    Some(out)
}

/// The station that labels each point, keyed by the point (`point_of(i)`, the
/// root of station `i`'s colocation group): among the stations `has_label`
/// accepts, a station that is not its line's first, then one that is not its
/// line's last, then the line's order in `lines`, then the station
/// index (docs/SPEC.md 4.2, Labels; the map's `labelMergeGroups.ts` picks the
/// same). So a branch merged onto the main line is labelled with the main
/// line's station, not the branch's `0`. A point with no accepted station has
/// no entry.
pub(super) fn point_representatives(
    lines: &[SketchLine],
    stations: &[Station],
    mut point_of: impl FnMut(usize) -> usize,
    has_label: impl Fn(usize) -> bool,
) -> HashMap<usize, usize> {
    let lines: HashMap<LineId, (usize, u32)> = lines
        .iter()
        .enumerate()
        .map(|(order, line)| {
            let last = u32::try_from(line.shots.len()).unwrap_or(u32::MAX);
            (line.id, (order, last))
        })
        .collect();
    // Lower is better.
    let rank = |i: usize| {
        let at = stations[i].station_ref;
        let (order, last) = lines
            .get(&at.line_id)
            .copied()
            .unwrap_or((usize::MAX, u32::MAX));
        (
            at.station_index == 0,
            at.station_index == last,
            order,
            at.station_index,
        )
    };
    let mut rep: HashMap<usize, usize> = HashMap::new();
    for i in (0..stations.len()).filter(|&i| has_label(i)) {
        rep.entry(point_of(i))
            .and_modify(|best| {
                if rank(i) < rank(*best) {
                    *best = i;
                }
            })
            .or_insert(i);
    }
    rep
}
