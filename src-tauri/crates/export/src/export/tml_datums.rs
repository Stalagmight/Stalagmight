//! GPS on the Ariane export: every zero datum is an entrance (#741 decision 5).
//!
//! Ariane keeps GPS on START rows only, and reads EVERY START's lat/lon as a
//! hard fix, bending the loop between two of them to fit (measured 2026-11-13;
//! see docs/ariane-export.md, "Every datum is an entrance").
//!
//! - An entrance whose station holds a datum carries that datum's GPS (#378
//!   item e; the entrance split put its START on that station).
//! - An entrance with no datum of its own carries the GPS of where the raw
//!   survey puts its station, relative to the frame datum. Repeating the frame
//!   datum's coordinates instead pinned two STARTs onto one point, and Ariane
//!   collapses any loop between two STARTs that share coordinates. A START left
//!   at 0,0 would drop its component on null island.
//! - A datum anywhere else (a GPS fix part-way through the survey) gets a START
//!   of its own at its GPS, tied to its station by a zero-length CLOSURE: the
//!   shape Ariane writes for a loop that ends on an entrance.
//!
//! A child module of `tml`, so it builds `ExportShot` rows directly.

use std::collections::{HashMap, HashSet};

use crate::models::{local_to_gps, GpsCoordinates, Project, SectionId, StationRef, ZeroDatum};

use super::{speleodb_anchor, ExportShot, TmlProfile};

/// A START row: the parentless, zero-length entrance Ariane roots a tree at.
/// The GPS pass stamps its lat/lon.
pub(super) fn start_shot(
    id: i64,
    section: String,
    depth: f64,
    explorer: Option<String>,
    date: Option<String>,
    color: String,
) -> ExportShot {
    ExportShot {
        id,
        uuid: None,
        from_id: -1,
        shot_type: "START",
        section,
        azimuth: 0.0,
        length: 0.0,
        depth,
        depth_in: depth,
        depth_in_same_as_prev: false,
        inclination: 0.0,
        left: 0.0,
        right: 0.0,
        up: 0.0,
        down: 0.0,
        comment: Some("START".to_string()),
        explorer,
        date,
        color,
        latitude: 0.0,
        longitude: 0.0,
        closure_to_id: -1,
        locked: true,
    }
}

/// A CLOSURE row: Ariane nodes `from_id` and `to_id` are one physical point
/// surveyed twice, so its length IS the depth gap between the two (see the
/// closure pass in `build_export_shots_with_report`).
pub(super) fn closure_shot(
    id: i64,
    from_id: i64,
    to_id: i64,
    section: String,
    color: String,
    depth_a: f64,
    depth_b: f64,
) -> ExportShot {
    ExportShot {
        id,
        uuid: None,
        from_id,
        shot_type: "CLOSURE",
        section,
        azimuth: 0.0,
        length: (depth_b - depth_a).abs(),
        depth: depth_b,
        depth_in: depth_a,
        depth_in_same_as_prev: false,
        inclination: 0.0,
        left: 0.0,
        right: 0.0,
        up: 0.0,
        down: 0.0,
        comment: None,
        explorer: None,
        date: None,
        color,
        latitude: 0.0,
        longitude: 0.0,
        closure_to_id: to_id,
        locked: false,
    }
}

/// What the START stamp decided, and what it still owes.
pub(super) struct DatumGps {
    /// Whether the project has a frame datum at all. Without one nothing
    /// resolves, and no datum is owed a START.
    has_frame: bool,
    /// Indices in `zero_datums` whose GPS is on file.
    written: HashSet<usize>,
    /// `(node id, datum index)` for each datum whose station is an ordinary
    /// row, still owed a START of its own.
    fixes: Vec<(i64, usize)>,
}

/// Stamp GPS on every START row, and work out which datums sit on ordinary
/// rows instead. Datums claim Ariane nodes in list order, so a placement datum
/// claims its node before any fix of the same component.
pub(super) fn stamp_starts(
    project: &Project,
    frame: Option<&ZeroDatum>,
    station_to_ariane: &HashMap<(SectionId, u32), i64>,
    shots: &mut [ExportShot],
    profile: TmlProfile,
) -> DatumGps {
    let mut gps = DatumGps {
        has_frame: frame.is_some(),
        written: HashSet::new(),
        fixes: Vec::new(),
    };
    let Some(frame) = frame else {
        return gps;
    };
    let start_ids: HashSet<i64> = shots
        .iter()
        .filter(|s| s.shot_type == "START")
        .map(|s| s.id)
        .collect();

    let mut on_start: HashMap<i64, usize> = HashMap::new();
    let mut claimed: HashSet<i64> = HashSet::new();
    for (index, datum) in project.zero_datums.iter().enumerate() {
        // A GPS Ariane cannot place is unwritten, and claims no node: a
        // crafted file reaches here without `GpsCoordinates::new`'s checks.
        if !is_placeable(&datum.gps) {
            continue;
        }
        let node = project
            .resolve_key(&datum.station_ref)
            .and_then(|sr| station_to_ariane.get(&(sr.section_id, sr.station_index)));
        // Unwritten when it does not resolve, its station has no node, or an
        // earlier datum holds the node: two GPS positions for one point is a
        // conflict the file cannot carry.
        let Some(&node) = node.filter(|node| claimed.insert(**node)) else {
            continue;
        };
        if start_ids.contains(&node) {
            on_start.insert(node, index);
        } else {
            gps.fixes.push((node, index));
        }
    }

    // The station each START stands on. A merged station aliases onto the
    // START's id too; the lowest key keeps the choice deterministic, and merged
    // stations share one raw position anyway.
    let mut start_station: HashMap<i64, (SectionId, u32)> = HashMap::new();
    for (&key, &id) in station_to_ariane {
        if start_ids.contains(&id) {
            let held = start_station.entry(id).or_insert(key);
            *held = (*held).min(key);
        }
    }

    let frame_index = project
        .zero_datums
        .iter()
        .position(|d| std::ptr::eq(d, frame));
    // A frame whose own GPS cannot be placed anchors nothing: its STARTs stay
    // at 0,0, as in a project with no datum, and the datums count unwritten.
    let frame_gps = Some(&frame.gps).filter(|gps| is_placeable(gps));
    // Where the raw survey puts the frame's own station. Local `(0, 0)` is
    // meant to be there, but a legacy project need not hold that, so every
    // surveyed START is measured from it rather than from the origin.
    let frame_xy = project
        .resolve_key(&frame.station_ref)
        .and_then(|sr| project.compute_station_raw_xy(sr));
    for shot in shots.iter_mut().filter(|s| s.shot_type == "START") {
        let (latitude, longitude) = if let Some(&index) = on_start.get(&shot.id) {
            gps.written.insert(index);
            let own = &project.zero_datums[index].gps;
            (own.latitude, own.longitude)
        } else if let Some(surveyed) = frame_gps.zip(frame_xy).and_then(|(origin, from)| {
            let &(section_id, station) = start_station.get(&shot.id)?;
            surveyed_gps(project, origin, from, StationRef::new(section_id, station))
        }) {
            surveyed
        } else if let Some(origin) = frame_gps {
            // Last resort, for a START whose station has no raw position: the
            // frame datum's coordinates, which are then on file through it.
            gps.written.extend(frame_index);
            (origin.latitude, origin.longitude)
        } else {
            continue;
        };
        (shot.latitude, shot.longitude) = match profile {
            TmlProfile::User => (latitude, longitude),
            TmlProfile::SpeleoDbSidecar => speleodb_anchor(latitude, longitude),
        };
    }

    // The SpeleoDB sidecar leaves the fixes out: its converter drops CLOSURE
    // rows, so a fix START would reach the map preview as a lone anchor with
    // nothing hanging off it - no gain, and a shape nobody has checked there.
    if profile == TmlProfile::SpeleoDbSidecar {
        gps.fixes.clear();
    }
    gps
}

/// Where the RAW survey puts `station`, as GPS: its raw offset from `frame_xy`
/// (the frame datum station's raw position) placed against `origin` (that
/// datum's GPS). Ariane runs its own loop solve, so the measurements are what
/// it should start from.
fn surveyed_gps(
    project: &Project,
    origin: &GpsCoordinates,
    (frame_x, frame_y): (f64, f64),
    station: StationRef,
) -> Option<(f64, f64)> {
    let (x, y) = project.compute_station_raw_xy(station)?;
    local_to_gps(x - frame_x, y - frame_y, origin)
        .filter(|&(latitude, longitude)| in_range(latitude, longitude))
}

/// Whether Ariane can place `gps`: finite, and on the globe. A loaded project
/// can carry anything here, since persistence builds the struct directly.
fn is_placeable(gps: &GpsCoordinates) -> bool {
    in_range(gps.latitude, gps.longitude)
}

fn in_range(latitude: f64, longitude: f64) -> bool {
    // A range check is false for NaN and for either infinity.
    (-90.0..=90.0).contains(&latitude) && (-180.0..=180.0).contains(&longitude)
}

impl DatumGps {
    /// Append each fix's START (at its GPS) and the zero-length CLOSURE tying
    /// it to its station, minting ids from `next_id`. Returns how many datums
    /// ended up on no START (the `gps-anchors-not-written` note).
    ///
    /// Call AFTER the geometry repair, so the START sits at the depth its
    /// station is written at: the repair can move a branch onto its parent's
    /// depth, and a START left at the pre-repair depth would put a vertical
    /// misclosure into a tie that measures none. Neither row needs the repair:
    /// a zero-length tie between two equal depths satisfies every bound.
    ///
    /// A file that already holds an orphan root gets no fix rows at all: Ariane
    /// draws an empty map for an orphan alongside ANY CLOSURE row (#379), and
    /// nothing shows a CLOSURE onto a START is exempt. Leaving the fixes out (and
    /// counting them unwritten) keeps a file that drew before drawing, where
    /// writing them would turn it blank or have `ensure_rooted_chain` refuse it
    /// over closures the user never made.
    pub(super) fn append_fix_starts(
        mut self,
        project: &Project,
        shots: &mut Vec<ExportShot>,
        mut next_id: i64,
    ) -> usize {
        if !self.fixes.is_empty() && shots.iter().any(super::is_orphan_root) {
            tracing::warn!(
                fixes = self.fixes.len(),
                "Ariane export: GPS fixes left out, because this survey has a shot with no \
                 parent station and a fix's CLOSURE tie would blank Ariane's map"
            );
            self.fixes.clear();
        }
        // Built once: the rows appended below are never looked up, so the
        // indices stay valid while `shots` grows.
        let index_by_id: HashMap<i64, usize> =
            shots.iter().enumerate().map(|(i, s)| (s.id, i)).collect();
        for (node, index) in std::mem::take(&mut self.fixes) {
            let gps = &project.zero_datums[index].gps;
            let Some(row) = index_by_id.get(&node).map(|&i| &shots[i]) else {
                continue;
            };
            let mut start = start_shot(
                next_id,
                row.section.clone(),
                row.depth,
                row.explorer.clone(),
                row.date.clone(),
                row.color.clone(),
            );
            (start.latitude, start.longitude) = (gps.latitude, gps.longitude);
            let tie = closure_shot(
                next_id + 1,
                node,
                next_id,
                row.section.clone(),
                row.color.clone(),
                row.depth,
                row.depth,
            );
            next_id += 2;
            shots.extend([start, tie]);
            self.written.insert(index);
        }

        if !self.has_frame {
            return 0;
        }
        let unwritten = project.zero_datums.len() - self.written.len();
        if unwritten > 0 {
            tracing::warn!(
                datum_count = project.zero_datums.len(),
                unwritten,
                "Ariane stores GPS on START rows only; {unwritten} GPS anchor(s) were not \
                 written: a datum that no longer resolves, whose station has no row in this \
                 file, whose GPS is not a point on the globe, or that shares its station with an earlier \
                 datum (datums part-way through the survey are also left out of the \
                 SpeleoDB sidecar, and of a file with a shot that has no parent station)."
            );
        }
        unwritten
    }
}
