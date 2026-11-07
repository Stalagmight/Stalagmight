//! A `parse_tml` rule the Playwright IPC mock used to re-implement (#576 E3).
//!
//! The browser tier's mock derived the preview's loop closures itself
//! (`deriveTmlClosures`); it now serves a canned preview, so the one half of the
//! closure mapping no Rust test pinned is pinned here: a preview closure's
//! "from" end is the CLOSURE row's own raw position in its section
//! (`test_closure_mapping` pins the "to" end).

use super::test_helpers::{tml_shot_xml, wrap_xml};
use super::*;

#[test]
fn a_preview_closure_starts_at_the_closure_rows_raw_position() {
    let closure_row = tml_shot_xml(3, 2, "CLOSURE", "Sec B").replace(
        "<ClosureToID>-1</ClosureToID>",
        "<ClosureToID>1</ClosureToID>",
    );
    let xml = wrap_xml(
        &[
            tml_shot_xml(0, -1, "START", "Start"),
            tml_shot_xml(1, 0, "REAL", "Sec A"),
            tml_shot_xml(2, 1, "REAL", "Sec B"),
            closure_row,
        ]
        .join("\n"),
    );

    let (raw_shots, _) =
        parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));
    let (sections, closures) = build_sections(raw_shots);

    assert_eq!(closures.len(), 1, "one closure row, one closure");
    let closure = &closures[0];
    // The series join may fold both sections into one run; either way the closure
    // row must be the station the closure starts from.
    let from_section = &sections[closure.from_section_index];
    assert_eq!(
        from_section
            .shots
            .get(closure.from_station_index)
            .map(|s| s.id),
        Some(3),
        "the closure starts at the CLOSURE row itself (raw index {} of {:?})",
        closure.from_station_index,
        from_section.shots.iter().map(|s| s.id).collect::<Vec<_>>()
    );
}
