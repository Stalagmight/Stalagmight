//! GPS fixes beside placement datums through edits that rebuild much of the
//! survey (#741): undo and redo, and a save and reopen, bring every datum
//! back in list order with its role, accuracy and Entrance answer, and the
//! history preview of a removed fix shows its coordinates. Synthetic surveys
//! only.
//!
//! The SQLite build pinned these on its whole-project undo snapshots, which
//! kept the fixes out of `zero_datums` so an older build reading the persisted
//! history would not take them for placements. The changeset engine records
//! the datum list itself and an older build cannot read a v2 file at all, so
//! the snapshot-format cases retire with it:
//! `a_snapshot_from_before_the_split_still_restores` (no snapshot format),
//! the older-build half of
//! `a_compact_delete_entry_is_unreadable_to_older_builds_and_undo_restores_the_fixes`
//! (its undo half is kept below), `a_snapshot_whose_fixes_cannot_be_read_is_refused`
//! (a damaged history entry is refused by the entry decoder, pinned in
//! `changeset/entry_tests.rs`), and
//! `a_save_reports_that_older_builds_drop_fixes_and_accuracies` (the
//! `OLDER_BUILDS_DROP_GPS_FIXES` save warning: an older build refuses a v2
//! file whole, so there is nothing it could drop).

use super::AppState;
use crate::core::Command;
use crate::models::survey::Project;
use crate::models::{DatumRole, GpsCoordinates, LineId, ShotData, StationRef, SurveyDirection};

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
            cave_section_id: None,
        })
        .line_id
        .unwrap()
}

fn set_datum(
    state: &AppState,
    station_ref: StationRef,
    lat: f64,
    accuracy_m: Option<f64>,
    entrance: Option<bool>,
) {
    let r = state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: gps(lat),
        accuracy_m,
        entrance,
    });
    assert!(r.success, "{:?}", r.error);
}

/// A joined to B (A3 = B0, a junction), C on its own, D on its own. Datums
/// in list order: A0 places A+B, B3 is a fix, C0 places C, A2 is a fix
/// (accuracy 4 m). The fix B3 sits between two placements, so the order is
/// observable. D carries nothing.
fn surveyed() -> (AppState, LineId, LineId) {
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
    set_datum(&state, StationRef::new(a, 0), 20.1, None, None);
    set_datum(&state, StationRef::new(b, 3), 20.2, Some(0.0), None);
    set_datum(&state, StationRef::new(c, 0), 20.3, None, None);
    set_datum(&state, StationRef::new(a, 2), 20.4, Some(4.0), None);
    (state, c, d)
}

/// Each datum as (slot, latitude bits, accuracy, role, entrance), in list
/// order.
type Summary = Vec<(
    Option<StationRef>,
    u64,
    Option<f64>,
    Option<DatumRole>,
    Option<bool>,
)>;

fn summary(project: &Project) -> Summary {
    project
        .datums
        .iter()
        .zip(project.datum_roles())
        .map(|(d, role)| {
            (
                project.datum_slot(d),
                d.gps.latitude.to_bits(),
                d.accuracy_m,
                role,
                d.entrance,
            )
        })
        .collect()
}

fn live(state: &AppState) -> Summary {
    summary(&state.get_project_state().unwrap())
}

#[test]
fn undo_and_redo_of_a_line_delete_bring_every_datum_back_in_order() {
    let (state, c, d) = surveyed();
    let expected = live(&state);
    assert_eq!(
        expected.iter().map(|d| d.3).collect::<Vec<_>>(),
        vec![
            Some(DatumRole::Placement),
            Some(DatumRole::GpsFix),
            Some(DatumRole::Placement),
            Some(DatumRole::GpsFix),
        ],
        "premise: the roles"
    );

    // A delete that changes no datum, and one that removes a placement.
    for line in [d, c] {
        let r = state.execute(Command::DeleteLine { line_id: line });
        assert!(r.success, "{:?}", r.error);
        assert!(state.undo().success);
        assert_eq!(live(&state), expected, "undo of deleting line {line}");
        assert!(state.redo().success);
        assert!(state.undo().success);
        assert_eq!(live(&state), expected, "undo after redo of line {line}");
    }
}

#[test]
fn the_history_preview_of_a_removed_fix_still_shows_its_coordinates() {
    let (state, _, _) = surveyed();
    let project = state.get_project_state().unwrap();
    let fix = project.datum_slot(&project.datums[1]).unwrap();
    let r = state.execute(Command::RemoveZeroDatum { station_ref: fix });
    assert!(r.success, "{:?}", r.error);

    let preview = state.get_undo_stack()[0].preview_data.clone().unwrap();
    let rows: Vec<(&str, &str)> = preview
        .changes
        .iter()
        .map(|c| (c.field.as_str(), c.old_value.as_str()))
        .collect();
    assert_eq!(rows[1], ("Latitude", "20.200000"), "{rows:?}");
}

/// A fix's Entrance answer rides with the fix, the placement's stays on it,
/// through undo and redo and through a save and reopen.
#[test]
fn each_datums_entrance_answer_survives_undo_and_a_reopen() {
    let _config = crate::test_helpers::isolated_config();
    let (state, _, d) = surveyed();
    let datums = live(&state);
    // The placement A0 and the fix B3, each re-set with its own GPS and
    // accuracy and an answer.
    let lats = [20.1, 20.2];
    for ((datum, entrance), lat) in [(&datums[0], Some(true)), (&datums[1], Some(false))]
        .into_iter()
        .zip(lats)
    {
        set_datum(&state, datum.0.unwrap(), lat, datum.2, entrance);
    }
    let answers = |s: &Summary| s.iter().map(|d| d.4).collect::<Vec<_>>();
    let expected = live(&state);
    assert_eq!(
        answers(&expected),
        vec![Some(true), Some(false), None, None]
    );

    let r = state.execute(Command::DeleteLine { line_id: d });
    assert!(r.success, "{:?}", r.error);
    assert!(state.undo().success);
    assert_eq!(live(&state), expected);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixes.stalx");
    let saved = state.execute(Command::SaveProject {
        path: Some(path.clone()),
    });
    assert!(saved.success, "{:?}", saved.error);
    let reopened = AppState::new().unwrap();
    assert!(reopened.execute(Command::OpenProject { path }).success);
    assert_eq!(live(&reopened), expected);
    // And the persisted history still undoes to the same datums.
    assert!(reopened.redo().success);
    assert!(reopened.undo().success);
    assert_eq!(live(&reopened), expected);
}
