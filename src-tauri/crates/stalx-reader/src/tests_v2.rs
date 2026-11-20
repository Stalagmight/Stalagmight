//! The JSON (version 2) path, the format sniff, and the two formats against
//! each other. Inputs are synthetic surveys built with the station-graph
//! construction API and written by the app's own encoder.

#![expect(clippy::unwrap_used, reason = "test assertions")]

use stalagmight_models::survey::{ClosureStrategy, LineMeta, Project, StartAt, ZeroDatum};
use stalagmight_models::{GpsCoordinates, ShotData};
use stalagmight_persistence::persistence::stalx_v2::{self, Extras, Mode};
use tempfile::TempDir;

use super::tests::{after_load, invalid_kind, name_at};
use super::{
    read_bytes, read_file_with, CountLimits, FileFormat, InvalidKind, Limit, ReadError,
    ReadOptions, StationId, Survey, SUPPORTED_FORMAT_VERSION,
};

fn leg(depth: f64) -> ShotData {
    ShotData {
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        length: 10.0,
        depth_in: depth,
        depth_out: depth + 5.0,
        pitch_in: Some(30.0),
        ..Default::default()
    }
}

fn meta(name: &str) -> LineMeta {
    LineMeta {
        name: Some(name.to_string()),
        color: "#ff5733".to_string(),
        ..LineMeta::default()
    }
}

/// The ids of what [`hub_project`] builds.
struct Hub {
    main: u32,
    side: u32,
    branch: u32,
    loose: u32,
    closure: u64,
}

/// Main (4 shots) with a datum at its start; Side and Branch both start at
/// Main 2, a junction of three sections; a closure from Main 4 to Side 2;
/// comments on the junction and on Side 2; Main 4 renamed; a Loose section
/// placed on its own.
fn hub_project() -> (Project, Hub) {
    let mut p = Project::new("Hub", Some("Nowhere".to_string()));
    let main = p
        .add_line(
            meta("Main"),
            StartAt::New,
            (0..4).map(|i| leg(5.0 * f64::from(i))).collect(),
        )
        .unwrap();
    let chain = p.line_stations(main).unwrap().to_vec();
    let side = p
        .add_line(
            meta("Side"),
            StartAt::Station(chain[2]),
            vec![leg(10.0), leg(15.0)],
        )
        .unwrap();
    let branch = p
        .add_line(meta("Branch"), StartAt::Station(chain[2]), vec![leg(10.0)])
        .unwrap();
    let loose = p
        .add_line(meta("Loose"), StartAt::New, vec![leg(0.0), leg(5.0)])
        .unwrap();
    let mut datum = ZeroDatum::new(chain[0], GpsCoordinates::new(20.5, -87.4, 0.0).unwrap());
    datum.via = Some(main);
    p.push_datum(datum);
    let loose_start = p.line_stations(loose).unwrap()[0];
    p.set_placement(loose_start, 500.0, 250.0).unwrap();
    let side_end = *p.line_stations(side).unwrap().last().unwrap();
    let closure = p.push_closure(
        chain[4],
        Some(main),
        side_end,
        Some(side),
        ClosureStrategy::Batch,
    );
    p.station_mut(chain[2]).unwrap().comment = Some("hub".to_string());
    p.station_mut(side_end).unwrap().comment = Some("lead".to_string());
    p.station_mut(chain[4]).unwrap().name = "17".to_string();
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    (
        p,
        Hub {
            main,
            side,
            branch,
            loose,
            closure,
        },
    )
}

fn encoded(project: &Project) -> Vec<u8> {
    stalx_v2::encode(project, &Extras::default(), Mode::Normal).unwrap()
}

fn at(section_id: u32, index: u32) -> StationId {
    StationId { section_id, index }
}

fn with_stations() -> ReadOptions {
    ReadOptions::default().with_stations(true)
}

/// `bytes` with `from` replaced by `to` (once; it must be there).
fn edited(bytes: &[u8], from: &str, to: &str) -> Vec<u8> {
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains(from), "{from} is not in the file");
    text.replacen(from, to, 1).into_bytes()
}

#[test]
fn reads_every_part_of_a_json_file() {
    let (project, hub) = hub_project();

    let survey = read_bytes(&encoded(&project), &with_stations()).unwrap();

    assert_eq!(survey.format, FileFormat::Json);
    assert_eq!(survey.format_version, SUPPORTED_FORMAT_VERSION);
    assert_eq!(survey.schema_version, 0);
    assert_eq!(survey.newer_content, None);
    assert_eq!(survey.unresolved_references, 0);
    assert_eq!(survey.name, "Hub");
    assert_eq!(survey.location.as_deref(), Some("Nowhere"));
    let ids: Vec<u32> = survey.sections.iter().map(|s| s.id).collect();
    let mut sorted = vec![hub.main, hub.side, hub.branch, hub.loose];
    sorted.sort_unstable();
    assert_eq!(ids, sorted);
    let section = |id: u32| survey.sections.iter().find(|s| s.id == id).unwrap();
    let (main, side, branch) = (section(hub.main), section(hub.side), section(hub.branch));
    assert_eq!(main.shots.len(), 4);
    assert_eq!(main.shots[0].pitch_in, Some(30.0));
    assert!(main.shots.iter().all(|s| !s.uuid_derived));

    // One station at the hub: one Station ID on all three sections, the
    // positions paired from the first one.
    assert_eq!(side.station_ids[0], main.station_ids[2]);
    assert_eq!(branch.station_ids[0], main.station_ids[2]);
    let hub_at = at(hub.main, 2);
    let mut merges: Vec<(StationId, StationId)> =
        survey.station_merges.iter().map(|m| (m.a, m.b)).collect();
    merges.sort_unstable();
    assert_eq!(
        merges,
        [(hub_at, at(hub.side, 0)), (hub_at, at(hub.branch, 0))]
    );

    // Names at every position (the hub's one name on each section); the
    // hub's comment once, at its first position.
    assert_eq!(name_at(&survey, at(hub.main, 4)), "17");
    assert_eq!(name_at(&survey, at(hub.side, 0)), "2");
    assert_eq!(name_at(&survey, at(hub.branch, 0)), "2");
    assert_eq!(survey.station_names.len(), 5 + 3 + 2 + 3);
    let comments: Vec<(StationId, &str)> = survey
        .station_comments
        .iter()
        .map(|c| (c.station, c.text.as_str()))
        .collect();
    assert_eq!(comments, [(hub_at, "hub"), (at(hub.side, 2), "lead")]);

    assert_eq!(survey.zero_datums.len(), 1);
    assert_eq!(survey.zero_datums[0].station, at(hub.main, 0));
    assert_eq!(survey.frame_datum, Some(at(hub.main, 0)));
    assert_eq!(survey.loop_closures.len(), 1);
    let closure = survey.loop_closures[0];
    assert_eq!(
        (closure.id, closure.a, closure.b, closure.tie),
        (hub.closure, at(hub.main, 4), at(hub.side, 2), false)
    );

    // Positions are the station-graph solver's, slot for slot.
    let raw = project.raw_positions();
    let stations = survey.stations.unwrap();
    let mut want: Vec<(StationId, u32, [f64; 3])> = raw
        .slots(&project)
        .into_iter()
        .map(|s| (StationId::from(s.slot), s.station, [s.x, s.y, s.z]))
        .collect();
    want.sort_by_key(|w| w.0);
    let got: Vec<(StationId, u32, [f64; 3])> = stations
        .iter()
        .map(|p| (p.station, p.station_id, [p.x, p.y, p.z]))
        .collect();
    assert_eq!(got, want);
    for p in &stations {
        if p.station.section_id == hub.loose {
            assert_eq!(p.anchor, None);
            assert!(p.latitude.is_none());
        } else {
            assert_eq!(p.anchor, Some(at(hub.main, 0)), "{:?}", p.station);
            assert!(p.latitude.is_some());
        }
    }
    let loose_start = stations
        .iter()
        .find(|p| p.station == at(hub.loose, 0))
        .unwrap();
    assert_eq!((loose_start.x, loose_start.y), (500.0, 250.0));
    assert!(survey.zero_datums[0].residual_m.unwrap() < 1e-9);
}

#[test]
fn a_json_file_reads_the_same_every_time_from_a_file_or_bytes() {
    let (project, _) = hub_project();
    let bytes = encoded(&project);
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("hub.stalx");
    std::fs::write(&path, &bytes).unwrap();

    let first = read_file_with(&path, &with_stations()).unwrap();
    let second = read_bytes(&bytes, &with_stations()).unwrap();

    assert_eq!(first, second);
    assert!(first
        .station_names
        .windows(2)
        .all(|w| w[0].station < w[1].station));
}

#[test]
fn whitespace_between_the_prefix_tokens_is_tolerated() {
    let (project, _) = hub_project();
    let bytes = edited(
        &encoded(&project),
        "{\"format\":\"stalx\",\"version\":",
        "{\n  \"format\" : \"stalx\" ,\n  \"version\" : ",
    );

    let survey = read_bytes(&bytes, &ReadOptions::default()).unwrap();

    assert_eq!(survey.format, FileFormat::Json);
    assert_eq!(survey.sections.len(), 4);
}

#[test]
fn a_newer_major_format_version_is_refused() {
    let (project, _) = hub_project();
    let bytes = edited(&encoded(&project), "\"version\":2,", "\"version\":3,");

    let err = read_bytes(&bytes, &ReadOptions::default()).unwrap_err();

    assert!(
        matches!(err, ReadError::NewerFormat { found: 3, supported } if supported == SUPPORTED_FORMAT_VERSION),
        "{err}"
    );
}

#[test]
fn malformed_truncated_and_inconsistent_json_is_corrupt() {
    let (project, _) = hub_project();
    let bytes = encoded(&project);
    let text = String::from_utf8(bytes.clone()).unwrap();
    // A closure onto a station the file does not hold.
    let closure_line = text
        .lines()
        .find(|l| l.contains("\"strategy\""))
        .unwrap()
        .to_string();
    let dangling = closure_line.replacen("\"b\":", "\"b\":999999,\"was_b\":", 1);
    for (what, input) in [
        ("truncated", bytes[..bytes.len() / 2].to_vec()),
        ("no end", edited(&bytes, ",\n\"end\":true}", "}")),
        ("a NaN", edited(&bytes, "\"quality\":1", "\"quality\":NaN")),
        (
            "a dangling closure",
            edited(&bytes, &closure_line, &dangling),
        ),
        (
            "other conventions",
            edited(&bytes, "\"length\":\"m\"", "\"length\":\"ft\""),
        ),
    ] {
        let err = read_bytes(&input, &ReadOptions::default()).unwrap_err();
        assert_eq!(
            invalid_kind(&err),
            Some(InvalidKind::Corrupt),
            "{what}: {err}"
        );
    }
}

#[test]
fn json_caps_refuse_with_the_readers_limit_names() {
    let (project, _) = hub_project();
    let bytes = encoded(&project);
    for (limits, want, max) in [
        (CountLimits::default().shots(3), Limit::Shots, 3),
        (CountLimits::default().sections(2), Limit::Sections, 2),
        (
            CountLimits::default().loop_closures(0),
            Limit::LoopClosures,
            0,
        ),
        (CountLimits::default().station_ids(4), Limit::StationIds, 4),
        (
            CountLimits::default().zero_datum_extras(0),
            Limit::ZeroDatumExtras,
            0,
        ),
        (CountLimits::default().placements(0), Limit::Placements, 0),
    ] {
        let err = read_bytes(&bytes, &ReadOptions::default().count_limits(limits)).unwrap_err();
        assert!(
            matches!(err, ReadError::TooComplex { what, count, max: m }
                if what == want && m == max && count == max + 1),
            "{want:?}: {err}"
        );
    }
    // The junction pairs are capped after the decode, like an SQLite file's
    // merges after its load.
    let err = read_bytes(
        &bytes,
        &ReadOptions::default().count_limits(CountLimits::default().station_merges(1)),
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            ReadError::TooComplex {
                what: Limit::StationMerges,
                count: 2,
                max: 1
            }
        ),
        "{err}"
    );
}

#[test]
fn an_unknown_required_feature_is_read_and_flagged() {
    let (project, _) = hub_project();
    let bytes = edited(
        &encoded(&project),
        "\"requires\":[]",
        "\"requires\":[\"from-the-future\"]",
    );

    let survey = read_bytes(&bytes, &ReadOptions::default()).unwrap();

    let note = survey.newer_content.unwrap();
    assert!(note.contains("from-the-future"), "{note}");
    assert_eq!(survey.sections.len(), 4);
}

#[test]
#[expect(clippy::panic, reason = "the panic is the input under test")]
fn the_json_path_keeps_the_deadline_and_panic_containment() {
    let (project, _) = hub_project();
    let bytes = encoded(&project);
    after_load(|| std::thread::sleep(std::time::Duration::from_millis(1_500)));
    let err = read_bytes(
        &bytes,
        &ReadOptions::default().deadline(std::time::Duration::from_secs(1)),
    )
    .unwrap_err();
    assert_eq!(invalid_kind(&err), Some(InvalidKind::Deadline), "{err}");

    after_load(|| panic!("boom"));
    let err = read_bytes(&bytes, &ReadOptions::default()).unwrap_err();
    assert!(
        matches!(&err, ReadError::Internal(m) if m == "boom"),
        "{err}"
    );
}

#[test]
fn an_oversized_json_file_is_refused_before_it_is_read() {
    let (project, _) = hub_project();
    let bytes = encoded(&project);
    let max = bytes.len() as u64 - 1;

    let err = read_bytes(&bytes, &ReadOptions::default().max_file_bytes(max)).unwrap_err();

    assert!(
        matches!(err, ReadError::TooLarge { max: m } if m == max),
        "{err}"
    );
}

#[test]
fn bytes_that_look_like_json_but_are_no_stalx_are_not_a_stalx() {
    for bytes in [
        b"{}".as_slice(),
        b"{\"format\":\"stalx\"}".as_slice(),
        b"\xef\xbb\xbf{\"format\":\"stalx\",\"version\":2}".as_slice(),
    ] {
        let err = read_bytes(bytes, &ReadOptions::default()).unwrap_err();
        assert!(
            matches!(
                invalid_kind(&err),
                Some(InvalidKind::NotSqlite | InvalidKind::Corrupt)
            ),
            "{:?}: {err}",
            String::from_utf8_lossy(bytes)
        );
    }
}

/// The station-graph survey an SQLite file converts to, saved as JSON, reads
/// to the same survey: the reader's model does not depend on the format.
#[test]
fn an_old_file_and_its_json_conversion_read_the_same() {
    let dir = TempDir::new().unwrap();
    let path = super::tests::save(&dir, &super::tests::full_project());
    let from_sqlite = read_file_with(&path, &with_stations()).unwrap();
    let migrated = stalagmight_persistence::persistence::migrate::migrate_path(&path).unwrap();
    let json = encoded(&migrated.project);

    let from_json = read_bytes(&json, &with_stations()).unwrap();

    assert_eq!(from_sqlite.format, FileFormat::Sqlite);
    assert_eq!(from_json.format, FileFormat::Json);
    let same = |survey: &Survey| {
        let mut s = survey.clone();
        s.format = FileFormat::Json;
        s.format_version = SUPPORTED_FORMAT_VERSION;
        s.schema_version = 0;
        for section in &mut s.sections {
            for shot in &mut section.shots {
                shot.uuid_derived = false;
            }
        }
        s
    };
    assert_eq!(same(&from_sqlite), same(&from_json));
}
