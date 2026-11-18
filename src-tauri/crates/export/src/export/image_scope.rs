//! What an image export draws while Sections are hidden on the map (#327):
//! the hidden Sections' lines (`ExportImageOptions::hidden_line_ids`) are left
//! out of the picture, but distances are still measured along the whole
//! survey. Kept beside `image.rs`, which only threads the two through.

use std::collections::{HashMap, HashSet};

use crate::models::{LineId, Project, Station, StationRef};

/// The whole survey, project and every station, when the export draws less
/// of it (`None` = what is drawn is the whole survey).
pub(super) type Whole<'a> = Option<(&'a Project, &'a [Station])>;

/// `project` and `stations` without the lines `hidden` names, or `None` when
/// it names none the project holds (absent, empty, or stale ids).
///
/// The copy drops the lines and nothing else: every renderer pass walks
/// `project.lines` and the stations, and a merge, closure, datum or comment
/// whose station is on a dropped line no longer resolves, so it goes with its
/// line.
pub(super) fn without_hidden_lines(
    project: &Project,
    hidden: Option<&[LineId]>,
    stations: &[Station],
) -> Option<(Project, Vec<Station>)> {
    let hidden: HashSet<LineId> = hidden
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|id| project.get_line(*id).is_some())
        .collect();
    if hidden.is_empty() {
        return None;
    }
    let mut drawn = project.clone();
    drawn.lines.retain(|line| !hidden.contains(&line.id));
    drawn.rebuild_line_index();
    let shown = stations
        .iter()
        .filter(|s| !hidden.contains(&s.station_ref.line_id))
        .cloned()
        .collect();
    Some((drawn, shown))
}

/// The along-cave penetration distance of each of `stations` (drawn), measured
/// along `whole` when the export draws less than the survey: hiding a Section
/// does not make the cave beyond it any closer.
pub(super) fn penetration_distances(
    project: &Project,
    whole: Whole<'_>,
    stations: &[Station],
    include_reference: bool,
) -> Vec<f64> {
    let Some((whole, all)) = whole else {
        return project.compute_penetration_distances_with(stations, include_reference);
    };
    let by_ref: HashMap<StationRef, f64> = all
        .iter()
        .map(|s| s.station_ref)
        .zip(whole.compute_penetration_distances_with(all, include_reference))
        .collect();
    stations
        .iter()
        .map(|s| by_ref.get(&s.station_ref).copied().unwrap_or(f64::INFINITY))
        .collect()
}

/// Whether there is a zero datum to measure from: one drawn, or (a datum on a
/// hidden line still measures the drawn ones) one anywhere in `whole`.
pub(super) fn has_datum(drawn_datums: bool, whole: Whole<'_>) -> bool {
    drawn_datums || whole.is_some_and(|(_, all)| all.iter().any(|s| s.is_zero_datum))
}

/// The distance ramp's field, by station, and its farthest reachable
/// distance, measured along `whole` when the export draws less than the
/// survey.
///
/// The map colors the distance ramp from MEASURED distances (it draws
/// adjusted geometry but deliberately computes the distance field from
/// measured coords - see `AppState::distance_field`). Match that: distances
/// key by station_ref, so this stays consistent with the adjusted geometry
/// we draw. (It also skips the adjusted solve the map was designed to avoid.)
pub(super) fn distance_field(
    project: &Project,
    whole: Whole<'_>,
    include_reference: bool,
) -> (HashMap<StationRef, Option<f64>>, f64) {
    let measured = whole.map_or(project, |(whole, _)| whole);
    let field = measured.distance_field_from_with(
        &measured.compute_station_coordinates(),
        None,
        include_reference,
    );
    let by_ref = field
        .station_refs
        .iter()
        .copied()
        .zip(field.distances.iter().copied())
        .collect();
    (by_ref, field.max_reachable)
}
