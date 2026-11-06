//! The Ariane importer's reading bound (#576 FZ-PARSE-5-D1).
//!
//! A length, depth, start depth or passage dimension beyond the limit no real
//! survey reaches is read as missing, exactly as a non-finite one already is
//! (`parse_f64`): 0.0 for a length, depth or LRUD (0.0 is "not measured" for
//! an Ariane LRUD), the "same as previous" marker for a `DepthIn`, and the
//! no-data 0.0 for the entrance elevation. Done on the raw shots, before the
//! sections are built, so a branch's start depth never inherits the value.
//! Each one is noted with the shot ID Ariane shows.

use super::{schema, RawShot, TmlUnit, FEET_TO_METERS};
use crate::parsers::reading_bounds::{beyond_limit, note, BoundNotes};

/// Bound every reading of `shots` and the entrance elevation, both still in
/// the file's `unit`; the notes, or `None` for a clean file.
pub(super) fn bound_readings(
    shots: &mut [RawShot],
    entrance_elevation: &mut f64,
    unit: TmlUnit,
) -> Option<Vec<String>> {
    let to_metres = if unit == TmlUnit::Imperial {
        FEET_TO_METERS
    } else {
        1.0
    };
    let mut notes = BoundNotes::default();
    let mut bound = |at: &dyn Fn() -> String, what: &str, value: &mut f64, missing: f64| {
        let metres = *value * to_metres;
        if beyond_limit(metres) {
            notes.push(note(&at(), what, metres, "read as missing"));
            *value = missing;
        }
    };
    bound(
        &|| "The file".to_string(),
        "entrance elevation",
        entrance_elevation,
        0.0,
    );
    for shot in shots {
        let id = shot.id;
        let at = move || format!("Shot {id}");
        bound(&at, "length", &mut shot.length, 0.0);
        bound(&at, "depth", &mut shot.depth, 0.0);
        if !schema::is_depth_in_sentinel(shot.depth_in) {
            let same_as_previous = schema::DEPTH_IN_SAME_AS_PREVIOUS;
            bound(&at, "start depth", &mut shot.depth_in, same_as_previous);
        }
        bound(&at, "left", &mut shot.left, 0.0);
        bound(&at, "right", &mut shot.right, 0.0);
        bound(&at, "up", &mut shot.up, 0.0);
        bound(&at, "down", &mut shot.down, 0.0);
    }
    let mut out = Vec::new();
    notes.finish_into(&mut out);
    (!out.is_empty()).then_some(out)
}
