//! The Entrance answer of each zero datum in `zero_datum_extras` (#741):
//! it round-trips, it alone is enough to write rows, and a value no build
//! writes reads as the default rule. Synthetic surveys only.

use super::station_names_tests::dump;
use super::*;
use crate::models::{ShotData, StationRef};

/// One section of four shots; datums on A0 (placement) and A2 and A4 (fixes),
/// with the given entrance answers.
fn project_with(answers: [Option<bool>; 3]) -> Project {
    let mut project = Project::new("Entrances".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: (0..4)
                .map(|_| ShotData {
                    length: 10.0,
                    azimuth_in: 0.0,
                    azimuth_out: 0.0,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    project.zero_datums = [0, 2, 4]
        .into_iter()
        .zip(answers)
        .map(|(index, entrance)| ZeroDatum {
            station_ref: StationRef::new(a, index).into(),
            gps: GpsCoordinates::new(20.0 + f64::from(index) * 1e-4, -87.5, 0.0).unwrap(),
            accuracy_m: None,
            entrance,
        })
        .collect();
    project
}

fn answers(project: &Project) -> Vec<Option<bool>> {
    project.zero_datums.iter().map(|zd| zd.entrance).collect()
}

fn round_trip(project: &Project) -> (Database, Project) {
    let mut db = Database::in_memory().unwrap();
    db.save_project(project).unwrap();
    let loaded = db.load_project().unwrap();
    (db, loaded)
}

#[test]
fn entrance_answers_round_trip_on_placements_and_fixes() {
    let project = project_with([Some(false), Some(true), None]);
    let (_db, loaded) = round_trip(&project);
    assert_eq!(answers(&loaded), vec![Some(false), Some(true), None]);
    let resolved: Vec<bool> = loaded
        .zero_datums_wire()
        .iter()
        .map(|w| w.is_entrance)
        .collect();
    assert_eq!(resolved, vec![true, true, true]);
}

#[test]
fn an_entrance_answer_alone_writes_the_rows() {
    // No accuracy is stored and the only fix carries no answer: the placement
    // datum's answer is the only thing worth a row.
    let mut project = project_with([Some(false), None, None]);
    project.zero_datums.truncate(1);
    let (db, loaded) = round_trip(&project);
    assert_eq!(dump(&db, "zero_datum_extras", &[]).len(), 1);
    assert_eq!(answers(&loaded), vec![Some(false)]);
}

#[test]
fn an_entrance_value_no_build_writes_reads_as_the_default() {
    let project = project_with([None, Some(false), Some(true)]);
    let mut db = Database::in_memory().unwrap();
    db.save_project(&project).unwrap();
    db.conn
        .execute(
            "UPDATE zero_datum_extras SET entrance = 7 WHERE position = 1",
            [],
        )
        .unwrap();
    let loaded = db.load_project().unwrap();
    assert_eq!(answers(&loaded), vec![None, None, Some(true)]);
}

#[test]
fn a_table_without_the_entrance_column_reads_every_row_as_unanswered() {
    // A dev-build table written before `entrance` existed: the fixes and
    // accuracies still load, and no datum has an answer.
    let mut project = project_with([Some(false), Some(true), None]);
    project.zero_datums[1].accuracy_m = Some(3.0);
    let mut db = Database::in_memory().unwrap();
    db.save_project(&project).unwrap();
    db.conn
        .execute_batch("ALTER TABLE zero_datum_extras DROP COLUMN entrance;")
        .unwrap();
    let loaded = db.load_project().unwrap();
    assert_eq!(answers(&loaded), vec![None, None, None]);
    let accuracies: Vec<Option<f64>> = loaded.zero_datums.iter().map(|zd| zd.accuracy_m).collect();
    assert_eq!(accuracies, vec![None, Some(3.0), None]);
}
