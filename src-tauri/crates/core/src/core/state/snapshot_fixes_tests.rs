//! GPS fixes in whole-project undo snapshots (#741): a snapshot keeps them out
//! of `zero_datums`, where an older build reading the persisted history would
//! take them for placements, and a restore puts them back in list order.
//! Synthetic surveys only.

use super::AppState;
use crate::core::{Command, UndoData};
use crate::models::{
    DatumRole, GpsCoordinates, LineId, Project, ShotData, StationRef, SurveyDirection,
};

fn gps(lat: f64) -> GpsCoordinates {
    GpsCoordinates::new(lat, -87.25, 0.0).unwrap()
}

fn create_line(state: &AppState, name: &str) -> LineId {
    state
        .execute(Command::CreateLine {
            name: Some(name.to_string()),
            surveyor: None,
            date: None,
            color: "#123456".to_string(),
            direction: Some(SurveyDirection::In),
            shots: (0..3)
                .map(|_| ShotData {
                    length: 10.0,
                    ..Default::default()
                })
                .collect(),
        })
        .line_id
        .unwrap()
}

fn set_datum(state: &AppState, station_ref: StationRef, lat: f64, accuracy_m: Option<f64>) {
    let r = state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: gps(lat),
        accuracy_m,
        entrance: None,
    });
    assert!(r.success, "{:?}", r.error);
}

/// A merged to B (A3 = B0), C on its own, D on its own. Datums in list order:
/// A0 places A+B, B3 is a fix, C0 places C, A2 is a fix (accuracy 4 m). The
/// fix B3 sits between two placements, so the order is observable. D carries
/// nothing: deleting it is a whole-project snapshot that changes no datum.
fn surveyed() -> (AppState, LineId) {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Fixes".to_string(),
        location: None,
    });
    let a = create_line(&state, "A");
    let b = create_line(&state, "B");
    let c = create_line(&state, "C");
    let d = create_line(&state, "D");
    let r = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 3),
        station_b: StationRef::new(b, 0),
    });
    assert!(r.success, "{:?}", r.error);
    set_datum(&state, StationRef::new(a, 0), 20.1, None);
    set_datum(&state, StationRef::new(b, 3), 20.2, Some(0.0));
    set_datum(&state, StationRef::new(c, 0), 20.3, None);
    set_datum(&state, StationRef::new(a, 2), 20.4, Some(4.0));
    (state, d)
}

/// Each datum as (station, latitude, accuracy, role), in list order.
type Summary = Vec<(StationRef, f64, Option<f64>, Option<DatumRole>)>;

fn summary(project: &Project) -> Summary {
    project
        .zero_datums
        .iter()
        .zip(project.datum_roles())
        .map(|(zd, role)| {
            (
                project.resolve_key(&zd.station_ref).unwrap(),
                zd.gps.latitude,
                zd.accuracy_m,
                role,
            )
        })
        .collect()
}

/// What an older build gets from a snapshot: a plain `Project` parse, which
/// ignores keys it does not know.
fn as_an_older_build_reads(snapshot: &str) -> Project {
    let mut project: Project = serde_json::from_str(snapshot).unwrap();
    project.rebuild_line_index();
    project.rebuild_shot_uuid_index();
    project
}

fn restored(snapshot: &str) -> Project {
    let mut slot = None;
    assert!(AppState::restore_snapshot_into(&mut slot, snapshot));
    slot.unwrap()
}

#[test]
fn a_snapshot_keeps_fixes_out_of_zero_datums_and_restores_them_in_order() {
    let (state, _) = surveyed();
    let live = state.get_project_state().unwrap();
    let expected = summary(&live);
    assert_eq!(
        expected.iter().map(|d| d.3).collect::<Vec<_>>(),
        vec![
            Some(DatumRole::Placement),
            Some(DatumRole::GpsFix),
            Some(DatumRole::Placement),
            Some(DatumRole::GpsFix),
        ],
        "premise: the fixture's roles"
    );

    let snapshot = AppState::serialize_project_snapshot(&live).unwrap();

    let older = as_an_older_build_reads(&snapshot);
    assert_eq!(
        summary(&older),
        vec![expected[0], expected[2]],
        "zero_datums holds the placements only"
    );
    assert_eq!(summary(&restored(&snapshot)), expected);
}

/// A snapshot written before the split (its fixes inside `zero_datums`, no
/// fixes key) still restores, unchanged.
#[test]
fn a_snapshot_from_before_the_split_still_restores() {
    let (state, _) = surveyed();
    let live = state.get_project_state().unwrap();
    let old_format = serde_json::to_string(&live).unwrap();
    assert!(!old_format.contains("zero_datum_fixes"), "premise");

    assert_eq!(summary(&restored(&old_format)), summary(&live));
}

/// Through a command: the `ProjectSnapshotPair` an undoable delete records
/// (and a save persists verbatim) holds no fix in `zero_datums`, and undo and
/// redo bring every datum back in order.
#[test]
fn a_snapshot_pair_hides_fixes_from_older_builds_and_undo_restores_them() {
    let (state, d) = surveyed();
    let expected = summary(&state.get_project_state().unwrap());

    let r = state.execute(Command::DeleteLine { line_id: d });
    assert!(r.success, "{:?}", r.error);
    let (before, after) = {
        let stack = state.undo_stack.lock().unwrap();
        let Some(UndoData::ProjectSnapshotPair { before, after }) = stack.peek_undo_data() else {
            panic!("premise: Delete Line records a snapshot pair");
        };
        (before.clone(), after.clone())
    };
    for half in [&before, &after] {
        assert_eq!(
            summary(&as_an_older_build_reads(half)),
            vec![expected[0], expected[2]]
        );
    }

    assert!(state.undo().success);
    assert_eq!(summary(&state.get_project_state().unwrap()), expected);
    assert!(state.redo().success);
    assert_eq!(summary(&state.get_project_state().unwrap()), expected);
}

/// Fixes that cannot be read refuse the restore rather than drop them.
#[test]
fn a_snapshot_whose_fixes_cannot_be_read_is_refused() {
    let (state, _) = surveyed();
    let snapshot = AppState::serialize_project_snapshot(&state.get_project_state().unwrap())
        .unwrap()
        .replace("\"zero_datum_fixes\":[", "\"zero_datum_fixes\":[7,");
    let mut slot = None;
    assert!(!AppState::restore_snapshot_into(&mut slot, &snapshot));
    assert!(slot.is_none());
}

/// The history dropdown reads a removed datum's coordinates out of the
/// snapshot's `before` half, so a removed fix is found beside `zero_datums`.
#[test]
fn the_history_preview_of_a_removed_fix_still_shows_its_coordinates() {
    let (state, _) = surveyed();
    let fix = state.get_project_state().unwrap().zero_datums[1].clone();
    let station_ref = state
        .get_project_state()
        .unwrap()
        .resolve_key(&fix.station_ref)
        .unwrap();
    let r = state.execute(Command::RemoveZeroDatum { station_ref });
    assert!(r.success, "{:?}", r.error);

    let preview = state.get_undo_stack()[0].preview_data.clone().unwrap();
    let rows: Vec<(&str, &str)> = preview
        .changes
        .iter()
        .map(|c| (c.field.as_str(), c.old_value.as_str()))
        .collect();
    assert_eq!(rows[1], ("Latitude", "20.200000"), "{rows:?}");
}

/// Whether a save of `state` reports that older builds drop its fixes.
fn save_reports_fixes(state: &AppState, dir: &tempfile::TempDir, name: &str) -> bool {
    let r = state.execute(Command::SaveProject {
        path: Some(dir.path().join(name)),
    });
    assert!(r.success, "{:?}", r.error);
    r.warnings
        .iter()
        .any(|w| w.code == super::project_io::OLDER_BUILDS_DROP_GPS_FIXES)
}

/// D1: an older build drops `zero_datum_extras` on its next save, so a save
/// of a file that holds GPS fixes, or a stored accuracy, says so. A project
/// with neither has nothing to lose and is told nothing.
#[test]
fn a_save_reports_that_older_builds_drop_fixes_and_accuracies() {
    let dir = tempfile::tempdir().unwrap();

    let (state, _) = surveyed();
    assert!(save_reports_fixes(&state, &dir, "fixes.stalx"), "fixes");

    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Placed".to_string(),
        location: None,
    });
    let a = create_line(&state, "A");
    set_datum(&state, StationRef::new(a, 0), 20.1, None);
    assert!(
        !save_reports_fixes(&state, &dir, "placed.stalx"),
        "a placement with the default accuracy"
    );
    set_datum(&state, StationRef::new(a, 0), 20.1, Some(3.0));
    assert!(
        save_reports_fixes(&state, &dir, "accuracy.stalx"),
        "a stored accuracy"
    );
}

/// A fix's Entrance answer (#741) rides in the split-out fixes, so a restore
/// brings it back with the fix; the placement's answer stays on it.
#[test]
fn a_snapshot_keeps_each_datums_entrance_answer() {
    let (state, _) = surveyed();
    let datums = summary(&state.get_project_state().unwrap());
    // The placement A0 and the fix B3, each re-set with its own GPS and accuracy.
    for (datum, entrance) in [(datums[0], Some(true)), (datums[1], Some(false))] {
        let r = state.execute(Command::SetZeroDatum {
            station_ref: datum.0,
            coordinates: gps(datum.1),
            accuracy_m: datum.2,
            entrance,
        });
        assert!(r.success, "{:?}", r.error);
    }
    let live = state.get_project_state().unwrap();
    let answers = |p: &Project| {
        p.zero_datums
            .iter()
            .map(|zd| zd.entrance)
            .collect::<Vec<_>>()
    };
    assert_eq!(answers(&live), vec![Some(true), Some(false), None, None]);

    let snapshot = AppState::serialize_project_snapshot(&live).unwrap();
    assert_eq!(answers(&restored(&snapshot)), answers(&live));
}
