#![expect(clippy::unwrap_used, reason = "test assertions")]
#![expect(
    clippy::float_cmp,
    reason = "values must round-trip through SQLite exactly"
)]

use std::path::{Path, PathBuf};

use stalagmight_models::station::{GpsCoordinates, LoopClosure as ModelClosure, StationRef};
use stalagmight_models::{
    ClosureStrategy, Project, SectionData, ShotData, ShotKind as ModelShotKind, SurveyDirection,
};
use stalagmight_persistence::persistence::Database;
use tempfile::TempDir;

use super::{
    read_bytes, read_file, read_file_with, CountLimits, Direction, InvalidKind, Limit, ReadError,
    ReadOptions, ShotKind, StationId, SUPPORTED_SCHEMA_VERSION,
};

thread_local! {
    /// Runs right after the load inside every read on this thread: stands in for
    /// a slow or panicking post-load step.
    pub(super) static AFTER_LOAD: std::cell::RefCell<Box<dyn Fn()>> =
        std::cell::RefCell::new(Box::new(|| {}));
}

/// Install an [`AFTER_LOAD`] hook for the rest of this test's thread.
fn after_load(hook: impl Fn() + 'static) {
    AFTER_LOAD.with(|slot| *slot.borrow_mut() = Box::new(hook));
}

fn invalid_kind(err: &ReadError) -> Option<InvalidKind> {
    match err {
        ReadError::Invalid { kind, .. } => Some(*kind),
        _ => None,
    }
}

/// 30-degree descents (length 10, dz 5), pitch stored with `sign`.
fn descending(name: &str, sign: f64, count: usize) -> SectionData {
    SectionData {
        name: Some(name.to_string()),
        surveyor: Some("user1".to_string()),
        date: None,
        color: "#ff5733".to_string(),
        direction: Some(SurveyDirection::In),
        quality: 1.0,
        shots: (0..count)
            .map(|i| ShotData {
                azimuth_in: 90.0,
                azimuth_out: 270.0,
                length: 10.0,
                depth_in: 5.0 * i as f64,
                depth_out: 5.0 * (i + 1) as f64,
                pitch_in: Some(sign * 30.0),
                pitch_out: Some(sign * 30.0),
                ..Default::default()
            })
            .collect(),
    }
}

fn save(dir: &TempDir, project: &Project) -> PathBuf {
    let path = dir.path().join("synthetic.stalx");
    Database::open(&path)
        .unwrap()
        .save_project(project)
        .unwrap();
    path
}

fn set_user_version(path: &Path, version: i32) {
    rusqlite::Connection::open(path)
        .unwrap()
        .pragma_update(None, "user_version", version)
        .unwrap();
}

/// Two sections, one merge, one datum, one closure, one comment, two names.
fn full_project() -> Project {
    let mut project = Project::new("Synthetic".to_string(), Some("Nowhere".to_string()));
    project.add_section(descending("Main", 1.0, 4));
    let mut side = descending("Side", 1.0, 2);
    side.direction = Some(SurveyDirection::Out);
    side.shots[0].left = Some(1.5);
    side.shots[1].kind = ModelShotKind::Jump;
    side.shots[1].comment = Some("sump".to_string());
    project.add_section(side);

    let (main, side) = (project.sections[0].id, project.sections[1].id);
    project
        .merge_stations(StationRef::new(main, 2), StationRef::new(side, 0))
        .unwrap();
    project
        .set_zero_datum(
            StationRef::new(main, 0),
            GpsCoordinates::new(20.5, -87.4, 0.0).unwrap(),
        )
        .unwrap();
    let a = project.key_for_ref(StationRef::new(main, 4)).unwrap();
    let b = project.key_for_ref(StationRef::new(side, 2)).unwrap();
    project.loop_closures.push(ModelClosure {
        id: 7,
        station_a: a.clone(),
        station_b: b,
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    project
        .station_comments
        .insert(a.clone(), "lead".to_string());
    project.station_names.insert(a, "17".to_string());
    let start = project.key_for_ref(StationRef::new(main, 0)).unwrap();
    project.station_names.insert(start, "0".to_string());
    project
}

#[test]
fn reads_every_part_of_a_current_file() {
    let dir = TempDir::new().unwrap();
    let path = save(&dir, &full_project());

    let survey = read_file(&path).unwrap();

    assert_eq!(survey.schema_version, SUPPORTED_SCHEMA_VERSION);
    assert_eq!(survey.name, "Synthetic");
    assert_eq!(survey.location.as_deref(), Some("Nowhere"));
    assert_eq!(survey.sections.len(), 2);
    let (main, side) = (&survey.sections[0], &survey.sections[1]);
    assert_eq!(main.shots.len(), 4);
    assert_eq!(side.direction, Direction::Out);
    assert_eq!(main.shots[0].pitch_in, Some(30.0), "pitch is +down");
    assert_eq!(main.shots[0].azimuth_out, 270.0);
    assert_eq!(side.shots[0].left, Some(1.5));
    assert_eq!(side.shots[0].right, None, "an absent LRUD stays absent");
    assert_eq!(side.shots[1].kind, ShotKind::Jump);
    assert_eq!(side.shots[1].comment.as_deref(), Some("sump"));
    assert_eq!(main.surveyor.as_deref(), Some("user1"));
    assert_eq!(main.color, "#ff5733");
    assert_eq!(main.north, super::North::Magnetic);
    assert_eq!(main.azimuth_correction_deg, 0.0);

    let id = |section: u32, index| StationId {
        section_id: section,
        index,
    };
    assert_eq!(survey.station_merges.len(), 1);
    assert_eq!(survey.station_merges[0].a, id(main.id, 2));
    assert_eq!(survey.station_merges[0].b, id(side.id, 0));
    assert_eq!(survey.zero_datums.len(), 1);
    assert_eq!(survey.zero_datums[0].station, id(main.id, 0));
    assert_eq!(survey.zero_datums[0].latitude, 20.5);
    assert_eq!(survey.loop_closures.len(), 1);
    assert_eq!(survey.loop_closures[0].id, 7);
    assert_eq!(survey.loop_closures[0].b, id(side.id, 2));
    assert_eq!(survey.station_comments.len(), 1);
    assert_eq!(survey.station_comments[0].station, id(main.id, 4));
    assert_eq!(survey.station_comments[0].text, "lead");
    let names: Vec<_> = survey
        .station_names
        .iter()
        .map(|n| (n.station, n.name.as_str()))
        .collect();
    assert_eq!(names, [(id(main.id, 0), "0"), (id(main.id, 4), "17")]);
    assert_eq!(survey.unresolved_references, 0);
    assert!(survey.stations.is_none(), "positions are opt-in");
}

#[test]
fn the_source_file_is_never_modified() {
    let dir = TempDir::new().unwrap();
    let path = save(&dir, &full_project());
    // An old version forces the migration chain to run, on the copy only.
    set_user_version(&path, 11);
    let before = std::fs::read(&path).unwrap();

    read_file(&path).unwrap();

    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn an_old_file_is_migrated_before_it_is_read() {
    // A pre-v12 file stored descending pitch as negative; the v12 migration flips
    // it. A raw read would return -30 here.
    let dir = TempDir::new().unwrap();
    let mut project = Project::new("Legacy".to_string(), None);
    project.add_section(descending("OldConvention", -1.0, 4));
    let path = save(&dir, &project);
    set_user_version(&path, 11);

    let survey = read_file(&path).unwrap();

    assert_eq!(survey.schema_version, 11, "reports the file's own version");
    assert!(survey.sections[0]
        .shots
        .iter()
        .all(|s| s.pitch_in == Some(30.0)));
}

#[test]
fn a_newer_schema_is_refused() {
    let dir = TempDir::new().unwrap();
    let path = save(&dir, &full_project());
    set_user_version(&path, SUPPORTED_SCHEMA_VERSION + 1);

    let err = read_file(&path).unwrap_err();

    assert!(
        matches!(err, ReadError::NewerSchema { found, supported }
            if found == SUPPORTED_SCHEMA_VERSION + 1 && supported == SUPPORTED_SCHEMA_VERSION),
        "{err}"
    );
}

#[test]
fn an_oversized_input_is_refused_before_it_is_read() {
    let dir = TempDir::new().unwrap();
    let path = save(&dir, &full_project());

    let err = read_file_with(&path, &ReadOptions::default().max_file_bytes(1024)).unwrap_err();

    assert!(matches!(err, ReadError::TooLarge { max: 1024 }), "{err}");
}

#[test]
fn bytes_that_are_not_a_stalx_are_invalid() {
    // An empty file is a valid empty SQLite database, so it fails as "no project".
    for (bytes, want) in [
        (b"".as_slice(), InvalidKind::NoProjectData),
        (b"not a database".as_slice(), InvalidKind::NotSqlite),
        (&[0u8; 4096], InvalidKind::NotSqlite),
    ] {
        let err = read_bytes(bytes, &ReadOptions::default()).unwrap_err();
        assert_eq!(invalid_kind(&err), Some(want), "{err}");
    }
}

#[test]
fn a_sqlite_file_without_project_data_is_invalid() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("empty.stalx");
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE t (x INTEGER);")
        .unwrap();

    let err = read_file(&path).unwrap_err();

    assert_eq!(
        invalid_kind(&err),
        Some(InvalidKind::NoProjectData),
        "{err}"
    );
}

#[test]
fn station_positions_cover_every_station_when_requested() {
    let dir = TempDir::new().unwrap();
    let path = save(&dir, &full_project());

    let survey = read_file_with(&path, &ReadOptions::default().with_stations(true)).unwrap();

    let stations = survey.stations.unwrap();
    let main = survey.sections[0].id;
    let origin = stations
        .iter()
        .find(|s| {
            s.station
                == StationId {
                    section_id: main,
                    index: 0,
                }
        })
        .unwrap();
    let last = stations
        .iter()
        .find(|s| {
            s.station
                == StationId {
                    section_id: main,
                    index: 4,
                }
        })
        .unwrap();
    assert!(
        (last.z - origin.z - 20.0).abs() < 1e-9,
        "four 5 m descents put the end 20 m deeper"
    );
    for section in &survey.sections {
        for index in 0..=section.shots.len() as u32 {
            let want = StationId {
                section_id: section.id,
                index,
            };
            assert!(
                stations.iter().any(|s| s.station == want),
                "missing position for {want:?}"
            );
        }
    }
}

/// A file whose own schema would run SQL during the load.
fn with_schema_sql(sql: &str) -> PathBuf {
    let dir = Box::leak(Box::new(TempDir::new().unwrap()));
    let path = save(dir, &full_project());
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(sql)
        .unwrap();
    path
}

#[test]
fn views_triggers_and_virtual_tables_are_refused() {
    for sql in [
        // An endless recursive view standing in for a real table.
        "ALTER TABLE shots RENAME TO real_shots;
         CREATE VIEW shots AS WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n)
           SELECT * FROM real_shots, n;",
        "CREATE TRIGGER t AFTER UPDATE ON shots BEGIN UPDATE shots SET length = 0; END;",
        // Written as a schema row: release builds compile SQLite without RTREE.
        "PRAGMA writable_schema = ON;
         INSERT INTO sqlite_master (type, name, tbl_name, rootpage, sql)
           VALUES ('table', 'v', 'v', 0, 'CREATE VIRTUAL TABLE v USING rtree(id, a, b)');",
        // A non-canonical spelling SQLite still parses as a virtual table, which
        // a match on the stored text would miss.
        "PRAGMA writable_schema = ON;
         INSERT INTO sqlite_master (type, name, tbl_name, rootpage, sql)
           VALUES ('table', 'v2', 'v2', 0, 'CREATE/**/VIRTUAL  TABLE v2 USING rtree(id, a, b)');",
    ] {
        let err = read_file(&with_schema_sql(sql)).unwrap_err();
        assert!(
            invalid_kind(&err) == Some(InvalidKind::RefusedSchemaObject),
            "{sql}: {err}"
        );
    }
}

#[test]
fn a_newer_version_claimed_by_the_project_row_is_refused() {
    // Files from before the pragma carry their version in the project row; the
    // app falls back to it when user_version is 0, and so must the reader.
    let path = with_schema_sql(&format!(
        "PRAGMA user_version = 0; UPDATE project SET schema_version = {};",
        SUPPORTED_SCHEMA_VERSION + 1
    ));

    let err = read_file(&path).unwrap_err();

    assert!(matches!(err, ReadError::NewerSchema { .. }), "{err}");
}

#[test]
fn a_negative_schema_version_is_invalid() {
    let path = with_schema_sql("PRAGMA user_version = -1;");

    let err = read_file(&path).unwrap_err();

    assert_eq!(invalid_kind(&err), Some(InvalidKind::Corrupt), "{err}");
}

#[test]
fn read_bytes_enforces_its_own_size_cap() {
    let err = read_bytes(&[0u8; 2048], &ReadOptions::default().max_file_bytes(1024)).unwrap_err();

    assert!(matches!(err, ReadError::TooLarge { max: 1024 }), "{err}");
}

#[test]
fn an_expired_deadline_interrupts_a_long_statement() {
    // A plain filler table is allowed, but the integrity check walks it in one
    // long statement, which the load connection's deadline must interrupt.
    let path = with_schema_sql(
        "CREATE TABLE filler (x);
         WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 50000)
           INSERT INTO filler SELECT x FROM n;",
    );

    let err =
        Database::open_existing_untrusted(&path, i32::MAX, std::time::Duration::from_nanos(1))
            .map(|_| ())
            .map_err(super::invalid)
            .unwrap_err();

    assert_eq!(invalid_kind(&err), Some(InvalidKind::Deadline), "{err}");
}

/// A v18 file written before station names has no `station_names` table; it
/// still reads, with no names.
#[test]
fn a_file_without_station_names_reads_with_none() {
    let path = with_schema_sql("DROP TABLE station_names;");

    let survey = read_file(&path).unwrap();

    assert!(survey.station_names.is_empty());
    assert_eq!(survey.station_comments.len(), 1);
    assert_eq!(survey.schema_version, SUPPORTED_SCHEMA_VERSION);
}

#[test]
fn a_dangling_station_name_is_dropped_and_counted() {
    let path =
        with_schema_sql("UPDATE station_names SET station_index = 999 WHERE station_index = 4;");

    let survey = read_file(&path).unwrap();

    assert_eq!(survey.station_names.len(), 1);
    assert_eq!(survey.unresolved_references, 1);
}

#[test]
fn a_dangling_cross_reference_is_dropped_and_counted() {
    let path = with_schema_sql("UPDATE station_comments SET station_index = 999;");

    let survey = read_file(&path).unwrap();

    assert!(survey.station_comments.is_empty());
    assert_eq!(survey.unresolved_references, 1);
}

#[test]
fn generated_columns_are_refused() {
    // 2 MiB per row from a tiny file; allowed with trusted_schema off.
    let path = with_schema_sql(
        "ALTER TABLE sections ADD COLUMN g TEXT
           GENERATED ALWAYS AS (hex(zeroblob(1048576))) VIRTUAL;",
    );

    let err = read_file(&path).unwrap_err();

    assert!(
        invalid_kind(&err) == Some(InvalidKind::RefusedSchemaObject),
        "{err}"
    );
}

#[test]
fn a_pending_kind_classification_is_completed_like_the_app_does() {
    // Without the loop closure, the Side section's Jump reaches real survey at one
    // point only, which the classifier calls a Reference - but only when an old
    // file's migration left the one-shot classification pending.
    let path = with_schema_sql(
        "DELETE FROM loop_closures;
         INSERT INTO project_settings (key, value) VALUES ('pending_kind_classification', '1');",
    );

    let survey = read_file(&path).unwrap();

    assert_eq!(survey.sections[1].shots[1].kind, ShotKind::Reference);
}

#[test]
fn drifted_section_offsets_are_re_derived_before_positions() {
    let dir = TempDir::new().unwrap();
    let path = save(&dir, &full_project());
    let survey = read_file(&path).unwrap();
    let (main, side) = (survey.sections[0].id, survey.sections[1].id);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE section_offsets SET offset_x = offset_x + 100 WHERE section_id = ?1",
            [side],
        )
        .unwrap();

    let survey = read_file_with(&path, &ReadOptions::default().with_stations(true)).unwrap();

    let at = |section_id, index| {
        survey
            .stations
            .as_ref()
            .unwrap()
            .iter()
            .find(|s| s.station == StationId { section_id, index })
            .copied()
            .unwrap()
    };
    let (a, b) = (at(main, 2), at(side, 0));
    assert!(
        (a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6,
        "merged stations coincide: {a:?} vs {b:?}"
    );
}

#[test]
fn legacy_closures_get_ids_like_the_app_assigns() {
    let path = with_schema_sql("UPDATE loop_closures SET closure_id = 0;");

    let survey = read_file(&path).unwrap();

    assert_eq!(survey.loop_closures.len(), 1);
    assert_ne!(survey.loop_closures[0].id, 0);
}

#[test]
fn an_oversized_file_is_refused_without_buffering_it() {
    // `read_file_with` streams into the copy and stops one byte past the cap.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("big.stalx");
    std::fs::write(&path, vec![0u8; 4096]).unwrap();

    let err = read_file_with(&path, &ReadOptions::default().max_file_bytes(1024)).unwrap_err();

    assert!(matches!(err, ReadError::TooLarge { max: 1024 }), "{err}");
}

#[test]
fn positions_report_their_datum_anchor_and_the_frame_datum() {
    // Main carries the datum at its start; Side is merged into Main, so it shares
    // the anchor; Loose is connected to nothing, so it floats.
    let mut project = full_project();
    project.add_section(descending("Loose", 1.0, 2));
    let dir = TempDir::new().unwrap();
    let path = save(&dir, &project);

    let survey = read_file_with(&path, &ReadOptions::default().with_stations(true)).unwrap();

    let main = survey.sections[0].id;
    let datum = StationId {
        section_id: main,
        index: 0,
    };
    assert_eq!(survey.frame_datum, Some(datum));
    let loose = survey.sections[2].id;
    for p in survey.stations.unwrap() {
        if p.station.section_id == loose {
            assert_eq!(p.anchor, None, "{:?} floats", p.station);
        } else {
            assert_eq!(
                p.anchor,
                Some(datum),
                "{:?} is tied to the datum",
                p.station
            );
        }
    }
}

#[test]
fn every_list_is_sorted_and_reads_are_deterministic() {
    // Store merges and closures in the reverse of their sorted order, so only the
    // reader's own sort can put them right.
    let mut project = full_project();
    project.add_section(descending("Third", 1.0, 3));
    let (main, third) = (project.sections[0].id, project.sections[2].id);
    project
        .merge_stations(StationRef::new(main, 1), StationRef::new(third, 0))
        .unwrap();
    let late = project.key_for_ref(StationRef::new(third, 3)).unwrap();
    let early = project.key_for_ref(StationRef::new(main, 3)).unwrap();
    let mut closure = project.loop_closures[0].clone();
    closure.id = 3;
    closure.station_a = early;
    closure.station_b = late;
    project.loop_closures.insert(0, closure);
    project.loop_closures[1].id = 9;
    project.loop_closures.swap(0, 1);
    let dir = TempDir::new().unwrap();
    let path = save(&dir, &project);
    let options = ReadOptions::default().with_stations(true);

    let first = read_file_with(&path, &options).unwrap();
    let second = read_file_with(&path, &options).unwrap();

    assert_eq!(first, second);
    assert!(first.sections.windows(2).all(|w| w[0].id < w[1].id));
    assert!(first
        .station_merges
        .windows(2)
        .all(|w| (w[0].a, w[0].b) <= (w[1].a, w[1].b)));
    assert!(first
        .zero_datums
        .windows(2)
        .all(|w| w[0].station <= w[1].station));
    assert!(first.loop_closures.windows(2).all(|w| w[0].id <= w[1].id));
    assert!(first
        .stations
        .unwrap()
        .windows(2)
        .all(|w| w[0].station < w[1].station));
}

#[test]
fn count_limits_refuse_a_file_before_the_post_load_steps() {
    let dir = TempDir::new().unwrap();
    let path = save(&dir, &full_project());
    let options = ReadOptions::default().count_limits(CountLimits::default().loop_closures(0));

    let err = read_file_with(&path, &options).unwrap_err();

    assert!(
        matches!(
            err,
            ReadError::TooComplex {
                what: Limit::LoopClosures,
                count: 1,
                max: 0
            }
        ),
        "{err}"
    );
}

/// A hostile upload can fill `station_names` with rows that name no station;
/// they are counted before the load builds them, like every other capped table.
#[test]
fn station_names_are_counted_against_their_own_cap() {
    let dir = TempDir::new().unwrap();
    let path = save(&dir, &full_project());
    let options = ReadOptions::default().count_limits(CountLimits::default().station_names(1));

    let err = read_file_with(&path, &options).unwrap_err();

    assert!(
        matches!(
            err,
            ReadError::TooComplex {
                what: Limit::StationNames,
                count: 2,
                max: 1
            }
        ),
        "{err}"
    );
    assert!(read_file_with(
        &path,
        &ReadOptions::default().count_limits(CountLimits::default().station_names(2))
    )
    .is_ok());
}

#[test]
fn the_frame_datum_anchors_its_component_even_when_it_is_not_the_lowest() {
    // The app refuses a second datum in one component, but a crafted or older
    // file can carry one. Put a datum on Side (merged into Main) FIRST, so it is
    // the frame datum although Main's datum has the lower station id.
    let mut project = full_project();
    let side = project.sections[1].id;
    let key = project.key_for_ref(StationRef::new(side, 1)).unwrap();
    let mut extra = project.zero_datums[0].clone();
    extra.station_ref = key;
    project.zero_datums.insert(0, extra);
    let dir = TempDir::new().unwrap();
    let path = save(&dir, &project);

    let survey = read_file_with(&path, &ReadOptions::default().with_stations(true)).unwrap();

    let frame = StationId {
        section_id: side,
        index: 1,
    };
    assert_eq!(survey.frame_datum, Some(frame));
    assert!(survey
        .stations
        .unwrap()
        .iter()
        .all(|p| p.anchor == Some(frame)));
}

#[test]
#[expect(clippy::panic, reason = "the panic is the input under test")]
fn a_panic_during_the_read_becomes_an_internal_error() {
    let dir = TempDir::new().unwrap();
    let path = save(&dir, &full_project());
    after_load(|| panic!("boom"));

    let from_file = read_file(&path).unwrap_err();
    let from_bytes =
        read_bytes(&std::fs::read(&path).unwrap(), &ReadOptions::default()).unwrap_err();

    for err in [from_file, from_bytes] {
        assert!(
            matches!(&err, ReadError::Internal(m) if m == "boom"),
            "{err}"
        );
    }
}

#[test]
fn the_private_copy_goes_in_the_given_temp_dir_and_is_removed() {
    let dir = TempDir::new().unwrap();
    let path = save(&dir, &full_project());
    let scratch = TempDir::new().unwrap();

    read_file_with(&path, &ReadOptions::default().temp_dir(scratch.path())).unwrap();

    assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 0);
}

#[test]
fn the_correction_source_tells_a_recorded_zero_from_nothing_recorded() {
    use stalagmight_models::{AzimuthReference, CorrectionSource as Source};

    let dir = TempDir::new().unwrap();
    let survey = read_file(&save(&dir, &full_project())).unwrap();
    assert_eq!(
        survey.sections[0].azimuth_correction_source,
        super::CorrectionSource::NotRecorded
    );

    let mut project = full_project();
    project.default_azimuth_reference = Some(AzimuthReference::magnetic(1.5, Source::Manual));
    let dir = TempDir::new().unwrap();
    let survey = read_file(&save(&dir, &project)).unwrap();
    assert_eq!(
        survey.sections[0].azimuth_correction_source,
        super::CorrectionSource::Manual
    );
    assert_eq!(survey.sections[0].azimuth_correction_deg, 1.5);
}

#[test]
fn impossible_datum_coordinates_are_invalid_values() {
    for sql in [
        "UPDATE zero_datums SET latitude = 120.0;",
        "UPDATE zero_datums SET longitude = -181.0;",
        "UPDATE zero_datums SET latitude = 9e999;",
    ] {
        let err = read_file(&with_schema_sql(sql)).unwrap_err();
        assert_eq!(
            invalid_kind(&err),
            Some(InvalidKind::InvalidValue),
            "{sql}: {err}"
        );
    }
}

#[test]
fn missing_and_repeated_uuids_are_derived_the_same_way_every_read() {
    // Main's first shot loses its UUID, and its third repeats its second, which
    // makes the third (the later of the two) the repeat.
    let path = with_schema_sql(
        "UPDATE shots SET uuid = NULL
           WHERE rowid = (SELECT rowid FROM shots ORDER BY section_id, sequence_order LIMIT 1);
         UPDATE shots SET uuid = (SELECT uuid FROM shots ORDER BY section_id, sequence_order
                                  LIMIT 1 OFFSET 1)
           WHERE rowid = (SELECT rowid FROM shots ORDER BY section_id, sequence_order
                          LIMIT 1 OFFSET 2);",
    );

    let first = read_file(&path).unwrap();
    let second = read_file(&path).unwrap();

    assert_eq!(first, second, "derived ids must not change between reads");
    let shots = &first.sections[0].shots;
    let flags: Vec<bool> = shots.iter().map(|s| s.uuid_derived).collect();
    assert_eq!(flags, [true, false, true, false]);
    let unique: std::collections::HashSet<&str> = shots.iter().map(|s| s.uuid.as_str()).collect();
    assert_eq!(unique.len(), shots.len());
}

#[test]
fn an_id_two_sections_hold_is_derived_for_the_later_one_the_way_the_app_does() {
    const IDS: [&str; 2] = [
        "d1d1d1d1-0000-4000-8000-000000000001",
        "d1d1d1d1-0000-4000-8000-000000000002",
    ];
    let copy = || SectionData {
        shots: IDS
            .iter()
            .map(|id| ShotData {
                uuid: Some((*id).to_string()),
                azimuth_in: 90.0,
                azimuth_out: 90.0,
                length: 5.0,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let mut project = Project::new("Copies".to_string(), None);
    project.add_section(copy());
    project.add_section(copy());
    let dir = TempDir::new().unwrap();
    let path = save(&dir, &project);

    let first = read_file(&path).unwrap();
    let second = read_file(&path).unwrap();
    assert_eq!(first, second, "derived ids must not change between reads");

    let ids = |i: usize| -> Vec<(String, bool)> {
        first.sections[i]
            .shots
            .iter()
            .map(|s| (s.uuid.clone(), s.uuid_derived))
            .collect()
    };
    let kept: Vec<(String, bool)> = IDS.iter().map(|id| ((*id).to_string(), false)).collect();
    assert_eq!(ids(0), kept, "the lower section keeps the file's ids");
    let later = first.sections[1].id;
    let renamed: Vec<(String, bool)> = IDS
        .iter()
        .map(|id| (stalagmight_models::reminted_shot_uuid(later, id), true))
        .collect();
    assert_eq!(
        ids(1),
        renamed,
        "the later one gets the app's derived ids, flagged"
    );
}

#[test]
fn anchored_positions_carry_lat_lon_and_datums_carry_their_residual() {
    let mut project = full_project();
    // Fore- and back-sight both 90: Main really runs due east.
    for shot in &mut project.sections[0].shots {
        shot.azimuth_out = 90.0;
    }
    project.add_section(descending("Loose", 1.0, 2));
    let dir = TempDir::new().unwrap();
    let path = save(&dir, &project);

    let survey = read_file_with(&path, &ReadOptions::default().with_stations(true)).unwrap();

    let stations = survey.stations.unwrap();
    let datum = survey.zero_datums[0];
    let at_datum = stations
        .iter()
        .find(|p| p.station == datum.station)
        .unwrap();
    assert!((at_datum.latitude.unwrap() - datum.latitude).abs() < 1e-9);
    assert!((at_datum.longitude.unwrap() - datum.longitude).abs() < 1e-9);
    // Longitude moves the same way as the local east coordinate does.
    let end = stations
        .iter()
        .find(|p| p.station.section_id == datum.station.section_id && p.station.index == 4)
        .unwrap();
    assert!(
        (end.x - at_datum.x).abs() > 1.0,
        "the fixture moves east-west"
    );
    assert_eq!(
        (end.longitude.unwrap() - datum.longitude).signum(),
        (end.x - at_datum.x).signum()
    );
    let loose = survey.sections[2].id;
    assert!(stations
        .iter()
        .filter(|p| p.station.section_id == loose)
        .all(|p| p.latitude.is_none() && p.longitude.is_none()));
    assert!(
        datum.residual_m.unwrap() < 1e-6,
        "the frame datum is its own origin"
    );
}

#[test]
fn a_datum_the_stored_offsets_no_longer_honour_has_a_large_residual() {
    // A second datum whose GPS disagrees with where the survey puts its station.
    let mut project = full_project();
    let side = project.sections[1].id;
    let mut extra = project.zero_datums[0].clone();
    extra.station_ref = project.key_for_ref(StationRef::new(side, 2)).unwrap();
    extra.gps = GpsCoordinates::new(20.6, -87.4, 0.0).unwrap();
    project.zero_datums.push(extra);
    let dir = TempDir::new().unwrap();
    let path = save(&dir, &project);

    let survey = read_file_with(&path, &ReadOptions::default().with_stations(true)).unwrap();

    let far = survey
        .zero_datums
        .iter()
        .find(|d| d.station.section_id == side)
        .unwrap();
    assert!(far.residual_m.unwrap() > 10_000.0, "{:?}", far.residual_m);
}

#[test]
fn the_deadline_covers_the_whole_read_not_only_long_statements() {
    // No statement here is long enough for SQLite to interrupt; a slow step
    // after the load uses up the deadline, and the read-wide clock stops it.
    let dir = TempDir::new().unwrap();
    let path = save(&dir, &full_project());
    after_load(|| std::thread::sleep(std::time::Duration::from_millis(1_500)));

    let err = read_file_with(
        &path,
        &ReadOptions::default().deadline(std::time::Duration::from_secs(1)),
    )
    .unwrap_err();

    assert_eq!(invalid_kind(&err), Some(InvalidKind::Deadline), "{err}");
}

#[test]
fn row_counts_are_refused_before_the_load() {
    let dir = TempDir::new().unwrap();
    let path = save(&dir, &full_project());
    after_load(|| unreachable!("the load ran"));
    let options = ReadOptions::default().count_limits(CountLimits::default().shots(1));

    let err = read_file_with(&path, &options).unwrap_err();

    assert!(
        matches!(
            err,
            ReadError::TooComplex {
                what: Limit::Shots,
                max: 1,
                ..
            }
        ),
        "{err}"
    );
}

#[test]
fn schema_expressions_a_migration_would_evaluate_are_refused() {
    for sql in [
        "CREATE INDEX e ON shots (length(hex(zeroblob(3000000))) + length);",
        "CREATE INDEX p ON shots (length) WHERE length > 0;",
        "CREATE TABLE d (x TEXT DEFAULT (hex(zeroblob(1000))));",
        "CREATE TABLE c (x CHECK (length(hex(zeroblob(4000000))) > 0));",
        "CREATE TABLE c (x, CHECK(x = 1) /* CHECK (id = 1) */);",
        "CREATE TABLE c (x CHECK/**/(x > 0));",
    ] {
        let err = read_file(&with_schema_sql(sql)).unwrap_err();
        assert_eq!(
            invalid_kind(&err),
            Some(InvalidKind::RefusedSchemaObject),
            "{sql}: {err}"
        );
    }
}

#[test]
fn the_apps_own_check_and_literal_defaults_are_allowed() {
    // The app's schema has `CHECK (id = 1)` singletons; plain literal defaults and
    // identifiers containing "check" are not expressions.
    let path = with_schema_sql(
        "CREATE TABLE extra (id INTEGER PRIMARY KEY CHECK ( id=1 ), checked_at TEXT DEFAULT 'x',
                             n REAL DEFAULT -1.5, t TEXT DEFAULT CURRENT_TIMESTAMP);",
    );

    read_file(&path).unwrap();
}

#[test]
fn a_derived_uuid_steps_past_one_the_file_already_holds() {
    // The namespace is public: give Main's first shot the id the reader would
    // derive for its (UUID-less) second shot.
    let path = with_schema_sql("SELECT 1;");
    let conn = rusqlite::Connection::open(&path).unwrap();
    let section: u32 = conn
        .query_row(
            "SELECT section_id FROM shots ORDER BY section_id, sequence_order LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let squat = uuid::Uuid::new_v5(
        &super::DERIVED_UUID_NAMESPACE,
        format!("{section}/1/").as_bytes(),
    )
    .to_string();
    conn.execute_batch(&format!(
        "UPDATE shots SET uuid = '{squat}'
           WHERE rowid = (SELECT rowid FROM shots ORDER BY section_id, sequence_order LIMIT 1);
         UPDATE shots SET uuid = NULL
           WHERE rowid = (SELECT rowid FROM shots ORDER BY section_id, sequence_order
                          LIMIT 1 OFFSET 1);"
    ))
    .unwrap();
    drop(conn);

    let first = read_file(&path).unwrap();
    let second = read_file(&path).unwrap();

    assert_eq!(first, second, "derived ids must not change between reads");
    let shots = &first.sections[0].shots;
    assert_eq!(shots[0].uuid, squat);
    assert!(shots[1].uuid_derived && shots[1].uuid != squat);
}

#[test]
fn station_positions_are_bit_exact_with_the_apps_own_open() {
    // A merge group, a datum, a loop closure and a loose section, read once by
    // the reader and once the way the app opens a file; every position must be
    // the same f64, not merely close.
    let mut project = full_project();
    let template = project.station_merges.iter().next().cloned().unwrap();
    let hub = project
        .key_for_ref(StationRef::new(project.sections[0].id, 1))
        .unwrap();
    for i in 0..50 {
        let id = project.add_section(descending(&format!("S{i}"), 1.0, 2)).id;
        let start = project.key_for_ref(StationRef::new(id, 0)).unwrap();
        project
            .station_merges
            .push(stalagmight_models::station::StationMerge {
                station_a: hub.clone(),
                station_b: start,
                merged_at: template.merged_at,
            });
    }
    project.add_section(descending("Loose", 1.0, 3));
    let dir = TempDir::new().unwrap();
    let path = save(&dir, &project);
    let app_copy = dir.path().join("app.stalx");
    std::fs::copy(&path, &app_copy).unwrap();

    let survey = read_file_with(&path, &ReadOptions::default().with_stations(true)).unwrap();
    let mut app = Database::open_existing(&app_copy)
        .unwrap()
        .load_project()
        .unwrap();
    app.assign_closure_ids();
    app.redrive_offsets_after_load();
    let mut expected: Vec<(StationId, [u64; 3])> = app
        .compute_station_coordinates()
        .into_iter()
        .map(|s| {
            let c = s.coordinates;
            (
                StationId::from(s.station_ref),
                [c.x.to_bits(), c.y.to_bits(), c.z.to_bits()],
            )
        })
        .collect();
    expected.sort_by_key(|(id, _)| *id);
    let actual: Vec<(StationId, [u64; 3])> = survey
        .stations
        .unwrap()
        .iter()
        .map(|p| (p.station, [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]))
        .collect();

    assert_eq!(actual.len(), expected.len());
    assert_eq!(actual, expected);
}

#[test]
fn objects_that_shadow_sqlite_pragmas_are_refused() {
    // A file's own `pragma_table_list` would be read in place of SQLite's and
    // could report the view below as an ordinary table.
    for sql in [
        "CREATE TABLE pragma_table_list (schema, name, type, ncol, wr, strict);
         CREATE VIEW sneaky AS SELECT 1;",
        "CREATE VIEW PRAGMA_TABLE_XINFO AS SELECT 0 AS hidden;",
        "CREATE TABLE pragma_index_list (seq, name, \"unique\", origin, partial);",
    ] {
        let err = read_file(&with_schema_sql(sql)).unwrap_err();
        assert_eq!(
            invalid_kind(&err),
            Some(InvalidKind::RefusedSchemaObject),
            "{sql}: {err}"
        );
    }
}

#[test]
fn a_column_named_like_virtual_is_not_a_virtual_table() {
    // Pre-v4 shots tables had an `is_virtual` column; a substring match on
    // "virtual" would refuse every such file.
    let path = with_schema_sql("ALTER TABLE shots ADD COLUMN is_virtual INTEGER;");

    read_file(&path).unwrap();
}

#[test]
fn schema_rows_retyped_in_another_case_are_refused() {
    // SQLite loads `type` case-insensitively; an exact-match check would let a
    // 'View', a 'TABLE'-typed virtual table, or a 'Table'-typed table whose
    // generated column every `type = 'table'` filter then misses, straight through.
    for sql in [
        "CREATE VIEW v AS SELECT 42;
         PRAGMA writable_schema = ON;
         UPDATE sqlite_master SET type = 'View' WHERE name = 'v';",
        "PRAGMA writable_schema = ON;
         INSERT INTO sqlite_master (type, name, tbl_name, rootpage, sql)
           VALUES ('TABLE', 'vt', 'vt', 0, 'CREATE VIRTUAL TABLE vt USING fts5(x)');",
        "CREATE TABLE g (a, b AS (zeroblob(1000)));
         PRAGMA writable_schema = ON;
         UPDATE sqlite_master SET type = 'Table' WHERE name = 'g';",
        "CREATE TABLE PRAGMA_INDEX_LIST (seq);",
    ] {
        let err = read_file(&with_schema_sql(sql)).unwrap_err();
        assert_eq!(
            invalid_kind(&err),
            Some(InvalidKind::RefusedSchemaObject),
            "{sql}: {err}"
        );
    }
}
