//! The Ariane TML/TMLU export: the row writer, every pass after the rows are
//! built (the GPS stamp on the STARTs, the geometry repair, the SpeleoDB
//! hardening, the orphan guard) and the document, driven through the
//! station-graph exporter (`tml_survey.rs`).
//!
//! Ported from the 0.6.0 suite that drove the export through
//! `models::project`. Each survey is built with the model's own operations
//! (`Survey`): a 0.6.0 merge is a junction here, so a line hangs off the
//! station it shares with no alias, and a row's `ID` is the Station ID of the
//! station it ends at (plan Deviations X1), so the tests read ids off the
//! survey instead of counting rows.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use tempfile::NamedTempFile;

use super::survey::{build_rows, export_tml, export_tml_for_speleodb};
use super::*;
use crate::export::survey::test_helpers::{flat as shot, meta, At, Survey};
use crate::models::survey::{ClosureStrategy, Project};
use crate::models::{
    comment_split, GpsCoordinates, LineId, Person, ShotData, ShotKind, StationId, SurveyCredit,
};

// ---- Helpers -----------------------------------------------------------

/// The azimuth declaration the export makes for `project`.
fn decision(project: &Project) -> AzimuthExport {
    AzimuthExport::decide_from(
        project
            .lines()
            .iter()
            .filter(|line| !line.shots.is_empty())
            .map(|line| project.azimuth_reference_for(line)),
    )
}

/// The rows the export writes for `project` under `profile`, and how many
/// datums ended up on no START.
fn export_rows(project: &Project, profile: TmlProfile) -> (Vec<ExportShot>, usize) {
    build_rows(project, profile, decision(project))
}

/// The user export's rows.
fn user_rows(project: &Project) -> Vec<ExportShot> {
    export_rows(project, TmlProfile::User).0
}

/// The SpeleoDB sidecar's rows.
fn sidecar_rows(project: &Project) -> Vec<ExportShot> {
    export_rows(project, TmlProfile::SpeleoDbSidecar).0
}

fn id(station: StationId) -> i64 {
    i64::from(station)
}

/// The row whose `ID` is `row_id`.
fn row(rows: &[ExportShot], row_id: i64) -> &ExportShot {
    rows.iter()
        .find(|r| r.id == row_id)
        .unwrap_or_else(|| panic!("no row {row_id}"))
}

/// `count` of [`shot`].
fn shots(count: usize, length: f64, azimuth: f64) -> Vec<ShotData> {
    (0..count).map(|_| shot(length, azimuth)).collect()
}

/// A shot that only has a length (azimuth 0, flat at depth 0).
fn tape(length: f64) -> ShotData {
    shot(length, 0.0)
}

fn gps(latitude: f64, longitude: f64) -> GpsCoordinates {
    GpsCoordinates::new(latitude, longitude, 0.0).unwrap()
}

/// The stored form of a credit, as a shot carries it.
fn stored(credit: SurveyCredit) -> Option<std::sync::Arc<SurveyCredit>> {
    credit.stored().map(Into::into)
}

/// The legacy line credit (a surveyor and a date) as every shot of the line
/// carried it after 0.6.0 folded it in.
fn line_credit(surveyor: &str, date: Option<chrono::NaiveDate>) -> ShotData {
    ShotData {
        credit: stored(SurveyCredit::from_legacy(Some(surveyor), date)),
        ..ShotData::default()
    }
}

/// "Test Cave": one line, Main (Alice, 2025-06-15, red), whose first shot
/// has a 5 degree pitch, LRUD and a comment.
fn simple_survey() -> (Survey, LineId) {
    let credit = line_credit("Alice", chrono::NaiveDate::from_ymd_opt(2025, 6, 15)).credit;
    let mut s = Survey::new("Test Cave");
    let main = s.named(
        "Main",
        "#ff0000",
        vec![
            ShotData {
                azimuth_in: 45.0,
                azimuth_out: 225.0,
                length: 10.0,
                depth_in: 0.0,
                depth_out: 3.0,
                pitch_in: Some(5.0),
                left: Some(1.0),
                right: Some(2.0),
                up: Some(3.0),
                down: Some(0.5),
                comment: Some("first shot".to_string()),
                credit: credit.clone(),
                ..ShotData::default()
            },
            ShotData {
                azimuth_in: 90.0,
                azimuth_out: 270.0,
                length: 8.0,
                depth_in: 3.0,
                depth_out: 5.0,
                credit,
                ..ShotData::default()
            },
        ],
    );
    (s, main)
}

fn simple_project() -> Project {
    simple_survey().0.build()
}

/// The station at slot `at` of `project`.
fn station(project: &Project, (line, index): At) -> StationId {
    project
        .station_at(crate::models::StationRef::new(line, index))
        .unwrap_or_else(|| panic!("no station at ({line}, {index})"))
}

/// No row but a START is parentless.
fn assert_no_orphan(rows: &[ExportShot]) {
    for r in rows {
        if r.shot_type != "START" {
            assert_ne!(r.from_id, -1, "row {} ({}) is parentless", r.id, r.line);
        }
    }
}

/// The `.tmlu` text of `project`.
fn tmlu_of(project: &Project) -> String {
    let file = NamedTempFile::with_suffix(".tmlu").unwrap();
    export_tml(project, file.path()).unwrap();
    std::fs::read_to_string(file.path()).unwrap()
}

/// The `Data.xml` of `project`'s `.tml` zip.
fn tml_data_xml(project: &Project) -> String {
    let file = NamedTempFile::with_suffix(".tml").unwrap();
    export_tml(project, file.path()).unwrap();
    let zip_file = std::fs::File::open(file.path()).unwrap();
    let mut archive = zip::ZipArchive::new(zip_file).unwrap();
    let mut data_xml = String::new();
    std::io::Read::read_to_string(&mut archive.by_name("Data.xml").unwrap(), &mut data_xml)
        .unwrap();
    data_xml
}

// ---- Inclination -------------------------------------------------------

/// The model is +down and Ariane is treated as +up, so the two must be
/// mirror images and a round-trip must land back on the original value
/// (issue #296).  Zero stays a plain `0.0` in both directions: `-0.0` would
/// survive into the XML, where it renders as a different string for a value
/// that did not change.
#[test]
fn tml_inclination_round_trips_through_the_sign_flip() {
    use crate::parsers::tml::inclination_to_pitch;

    for pitch in [30.0_f64, -30.0, 90.0, -90.0, 0.5, -0.5] {
        let inclination = pitch_to_inclination(Some(pitch));
        assert!(
            (inclination + pitch).abs() < f64::EPSILON,
            "pitch {pitch} should export as {}, got {inclination}",
            -pitch
        );
        assert!(
            (inclination_to_pitch(inclination) - pitch).abs() < f64::EPSILON,
            "round-trip of pitch {pitch} did not return it"
        );
    }

    for zero in [pitch_to_inclination(None), pitch_to_inclination(Some(0.0))] {
        assert!(
            zero == 0.0 && zero.is_sign_positive(),
            "zero pitch must export as plain 0.0, not -0.0"
        );
    }
    let back = inclination_to_pitch(0.0);
    assert!(
        back == 0.0 && back.is_sign_positive(),
        "zero inclination must import as plain 0.0, not -0.0"
    );
}

/// A reverse leg walks the shot backwards, so a descent becomes an ascent
/// and the pitch negates. `pitch_out` must not be used as the reversed
/// reading: it is a foresight in the same sense as `pitch_in`.
#[test]
fn reversed_leg_pitch_negates_the_forward_reading() {
    assert_eq!(
        reversed_pitch(Some(20.0), Some(22.0)),
        Some(-20.0),
        "a 20 degree descent reversed is a 20 degree ascent, not +22"
    );

    // Falls back to the other leg only when the forward one is absent.
    assert_eq!(reversed_pitch(None, Some(22.0)), Some(-22.0));
    assert_eq!(reversed_pitch(None, None), None);

    // A flat shot must still export as a plain 0.0, never -0.0.
    let inclination = pitch_to_inclination(reversed_pitch(Some(0.0), Some(22.0)));
    assert!(
        inclination == 0.0 && inclination.is_sign_positive(),
        "reversing a flat shot must not produce -0.0"
    );
}

/// The helper-level test above pins `pitch_to_inclination` against
/// `inclination_to_pitch`, but both could be reverted together, or one of
/// the export call sites could stop calling the helper, without it noticing.
/// This drives a real project through the file: export to `.tmlu`, parse it
/// back, and require the pitch to return unchanged.
///
/// Every `.tml` fixture in the tree carries `Inclination` 0 (Ariane does not
/// populate the field), so non-zero inclination is only reachable through a
/// file Stalagmight wrote itself - an export the user re-imports, or the
/// sidecar the SpeleoDB push renders.
#[test]
fn tml_inclination_survives_a_file_level_round_trip() {
    let project = simple_project();
    // Ariane has no way to say "no inclination recorded", so an absent pitch
    // necessarily comes back as 0.0. That is format lossiness, not a sign
    // error, so compare against the value the format can actually carry.
    let exported: Vec<f64> = project.lines()[0]
        .shots
        .iter()
        .map(|s| s.pitch_in.unwrap_or(0.0))
        .collect();
    assert!(
        exported.iter().any(|p| p.abs() > 1.0),
        "fixture has no recorded inclination, so this test would prove nothing"
    );

    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("round_trip.tmlu");
    export_tml(&project, &path).unwrap();

    let raw = std::fs::read_to_string(&path).unwrap();
    // `.tmlu` uses the short element names, so inclination is `<INC>`.
    assert!(
        raw.contains("<INC>-5"),
        "a +5 (down) pitch must leave as -5 in Ariane's +up frame: {raw}"
    );

    let parsed = crate::parsers::tml::parse_tml(&path).unwrap();
    let reimported: Vec<f64> = parsed.lines[0]
        .shots
        .iter()
        .map(|s| {
            crate::parsers::tml::normalize_tml_shot(s, s.depth_in)
                .pitch_in
                .unwrap_or(0.0)
        })
        .collect();

    assert_eq!(
        reimported, exported,
        "pitch must survive export and re-import unchanged; a flip applied \
         zero or two times here silently inverts every shot"
    );
}

// ---- Writing the file --------------------------------------------------

/// Every entry in `dir`, sorted, so a stray export temp shows up by name.
fn dir_entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn re_export_replaces_the_target_and_leaves_no_temp() {
    // #570: a re-export leaves exactly one entry - the new, parseable export -
    // and no temp. This alone cannot tell an atomic write from a direct one;
    // the symlinked-target test below is what pins both writers to the rename.
    let project = simple_project();
    for name in ["cave.tml", "cave.tmlu"] {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, b"a previous export").unwrap();

        export_tml(&project, &path).unwrap();

        assert_eq!(dir_entries(dir.path()), vec![name.to_string()]);
        let parsed = crate::parsers::tml::parse_tml(&path).unwrap();
        assert_eq!(parsed.lines.len(), 1, "{name}");
    }
}

#[cfg(unix)]
#[test]
fn export_replaces_a_symlinked_target_instead_of_writing_through_it() {
    // #570: the rename replaces a symlink at the target; a direct truncating
    // write (`std::fs::write` / `File::create`) follows it and destroys the
    // file it points at. That difference is what pins both writers to the
    // temp + rename path.
    let project = simple_project();
    for name in ["cave.tml", "cave.tmlu"] {
        let dir = tempfile::TempDir::new().unwrap();
        let victim = dir.path().join("victim.stalx");
        std::fs::write(&victim, b"not an export").unwrap();
        let path = dir.path().join(name);
        std::os::unix::fs::symlink(&victim, &path).unwrap();

        export_tml(&project, &path).unwrap();

        assert_eq!(std::fs::read(&victim).unwrap(), b"not an export", "{name}");
        let meta = std::fs::symlink_metadata(&path).unwrap();
        assert!(!meta.file_type().is_symlink(), "{name}");
    }
}

#[test]
fn test_export_emits_a_uuid_per_real_shot() {
    // The `<UUID>`/`<UU>` element is what makes a re-export of an unchanged
    // project byte-identical, which the SpeleoDB co-push render depends on
    // (#196). Synthesized shots (START here) carry none.
    let project = simple_project();
    let total_shots: usize = project.lines().iter().map(|l| l.shots.len()).sum();
    let content = tmlu_of(&project);

    // The .tmlu uses short tag names, so each real shot emits a <UU> element.
    assert_eq!(content.matches("<UU>").count(), total_shots);
    // START shot carries no UUID.
    assert!(content.contains("START"));

    // Each emitted UUID is the shot's own persistent identity, not a
    // per-export mint.
    for line in project.lines() {
        for shot in &line.shots {
            assert!(!shot.uuid.is_empty(), "every shot carries a uuid");
            assert!(
                content.contains(&format!("<UU>{}</UU>", shot.uuid)),
                "shot uuid {} missing from the export",
                shot.uuid
            );
        }
    }
}

/// A station comment rides on the one row that ends at the station, beside
/// that shot's own comment, and the comment of the station a START stands
/// on is dropped (a START keeps its hardcoded content).
#[test]
fn station_comments_fold_onto_their_shot_and_a_start_aliased_one_is_still_dropped() {
    let (mut s, main) = simple_survey();
    s.comment((main, 0), "on the START station");
    s.comment((main, 1), "at the bend");
    let project = s.build();

    let rows = user_rows(&project);

    // Station 1 is created by the line's first shot, so its comment rides
    // along there - beside that shot's own comment, split by the ZWSP marker.
    let first_real = rows.iter().find(|r| r.shot_type == "REAL").unwrap();
    let folded = first_real.comment.as_deref().unwrap_or_default();
    assert!(
        folded.contains("at the bend") && folded.contains("first shot"),
        "the station comment must be folded beside the shot comment: {folded:?}"
    );

    // Station 0 IS the START, which the fold skips, so its comment is absent
    // from the file and START keeps what it was given.
    let start = rows.iter().find(|r| r.shot_type == "START").unwrap();
    assert_eq!(start.comment.as_deref(), Some("START"));
    assert!(
        !rows.iter().any(|r| r
            .comment
            .as_deref()
            .is_some_and(|c| c.contains("on the START station"))),
        "a START-aliased station comment must not reach the file"
    );
}

/// #564: the shape that used to export unmarked - a shot comment with no
/// station comment anywhere. An unmarked line is indistinguishable from
/// Ariane-typed text, which the default import routes to the station.
#[test]
fn a_lone_shot_comment_exports_marked() {
    let project = simple_project();
    assert!(
        project.stations.values().all(|s| s.comment.is_none()),
        "the point of this shape is NO station comment riding along"
    );

    let rows = user_rows(&project);
    let first_real = rows.iter().find(|r| r.shot_type == "REAL").unwrap();
    assert_eq!(
        first_real.comment.as_deref(),
        Some(format!("{}first shot", comment_split::SHOT_MARKER).as_str()),
        "a lone shot comment must carry the ZWSP marker, or a default \
         re-import moves it to the to-station"
    );
    // START stays exactly as minted - unmarked, hardcoded.
    let start = rows.iter().find(|r| r.shot_type == "START").unwrap();
    assert_eq!(start.comment.as_deref(), Some("START"));
}

#[test]
fn uuid_survives_insert_between_exports() {
    // Regression for the silent-UUID-rebinding bug: identity is carried on
    // the shot's uuid, not derived from its position, so inserting a shot
    // must not re-assign any existing shot's UUID.
    let mut project = simple_project();
    let main = project.lines()[0].id;
    let dir = tempfile::TempDir::new().unwrap();
    let v1 = dir.path().join("v1.tmlu");
    let v2 = dir.path().join("v2.tmlu");

    export_tml(&project, &v1).unwrap();
    let uuids_v1: Vec<String> = project.lines()[0]
        .shots
        .iter()
        .map(|s| s.uuid.clone())
        .collect();

    // Insert a brand-new shot at the FRONT of the line. All pre-existing
    // shots' indices shift by +1.
    project
        .prepend_shot(
            main,
            ShotData {
                azimuth_in: 270.0,
                length: 1.5,
                ..Default::default()
            },
        )
        .unwrap();

    export_tml(&project, &v2).unwrap();
    let uuids_v2: Vec<String> = project.lines()[0]
        .shots
        .iter()
        .map(|s| s.uuid.clone())
        .collect();
    assert_eq!(uuids_v2.len(), uuids_v1.len() + 1);
    for (i, prev_uuid) in uuids_v1.iter().enumerate() {
        assert_eq!(
            &uuids_v2[i + 1],
            prev_uuid,
            "pre-existing shot at new index {} lost its uuid",
            i + 1
        );
    }
    assert!(!uuids_v2[0].is_empty(), "new shot must have a uuid");
    assert!(
        !uuids_v1.contains(&uuids_v2[0]),
        "new shot's uuid must not collide with any pre-existing one"
    );

    // Every pre-existing UUID is still in the emitted XML, at its new slot.
    let content = std::fs::read_to_string(&v2).unwrap();
    for uuid in &uuids_v1 {
        assert!(
            content.contains(&format!("<UU>{uuid}</UU>")),
            "uuid {uuid} lost from the export after an insert"
        );
    }
}

#[test]
fn export_tml_is_byte_stable_across_calls() {
    // #196 render-on-push: two renders of the same project must be
    // byte-identical so re-pushing unchanged content is a server no-op (304).
    let project = simple_project();
    let dir = tempfile::TempDir::new().unwrap();
    let a = dir.path().join("a.tml");
    let b = dir.path().join("b.tml");
    export_tml(&project, &a).unwrap();
    export_tml(&project, &b).unwrap();
    assert_eq!(
        std::fs::read(&a).unwrap(),
        std::fs::read(&b).unwrap(),
        "two renders of the same project must be byte-identical"
    );
}

/// The writer's fallback for a row with no stored identity (0.6.0 reached it
/// with a never-saved legacy project; every station-graph shot holds a uuid,
/// so the exporter itself never needs it): a fresh, non-empty v4 per call,
/// so an empty `<UUID>` is never written, while a stored uuid is kept.
#[test]
fn a_row_without_an_identity_gets_a_fresh_uuid_never_an_empty_one() {
    for missing in [None, Some("")] {
        let one = export_uuid(missing);
        let two = export_uuid(missing);
        assert!(!one.is_empty() && !two.is_empty());
        assert_ne!(one, two, "a mint is per call, never a shared key");
        assert!(speleodb_uuid(&one).is_some(), "the mint is a v4: {one}");
    }
    assert_eq!(export_uuid(Some("kept")), "kept");
}

#[test]
fn test_export_tmlu_creates_valid_xml() {
    let content = tmlu_of(&simple_project());
    assert!(content.contains("<?xml"));
    assert!(content.contains("<CaveFile>"));
    assert!(content.contains("Test Cave"));
    // TMLU uses short names
    assert!(content.contains("<SRVD>"));
    assert!(content.contains("<AZ>"));
    assert!(content.contains("<LG>"));
}

#[test]
fn test_export_tml_creates_zip() {
    // Verify it's a valid ZIP containing Data.xml
    let data_xml = tml_data_xml(&simple_project());
    // TML uses long names - verify ID is all-caps
    assert!(data_xml.contains("<SurveyData>"));
    assert!(data_xml.contains("<ID>"));
    assert!(data_xml.contains("<Azimut>"));
    assert!(data_xml.contains("<Length>"));
    assert!(data_xml.contains("<Locked>"));
    assert!(data_xml.contains("<Name>"));
    assert!(data_xml.contains("<Profiletype>"));
}

#[test]
fn test_export_empty_project() {
    let content = tmlu_of(&Survey::new("Empty").build());
    assert!(content.contains("<CaveFile>"));
    assert!(content.contains("<Data>"));
    // No survey data elements
    assert!(!content.contains("<SRVD>"));
}

#[test]
fn test_color_to_ariane() {
    assert_eq!(color_to_ariane("#ff0000"), "0xff0000ff");
    assert_eq!(color_to_ariane("#4ecdc4"), "0x4ecdc4ff");
    assert_eq!(color_to_ariane("invalid"), "0x4ecdc4ff");
    assert_eq!(color_to_ariane("#abc"), "0x4ecdc4ff"); // too short
}

#[test]
fn test_format_f64() {
    assert_eq!(format_f64(0.0), "0.0");
    assert_eq!(format_f64(10.0), "10.0");
    assert_eq!(format_f64(3.14159), "3.14159");
    assert_eq!(format_f64(1.5), "1.5");
    assert_eq!(format_f64(-2.0), "-2.0");
}

#[test]
fn test_reverse_pre_zero_azimuth_is_reciprocal() {
    assert!((reverse_pre_zero_azimuth(42.0) - 222.0).abs() < 1e-9);
    assert!((reverse_pre_zero_azimuth(350.0) - 170.0).abs() < 1e-9);
}

// ---- Rows, ids and parent links ----------------------------------------

#[test]
fn test_single_start_shot_at_root() {
    let rows = user_rows(&simple_project());

    // One entrance in this fixture, so one START. A survey with a line that
    // shares no station emits one per entrance - see
    // `a_detached_line_exports_as_its_own_entrance`.
    let start_count = rows.iter().filter(|r| r.shot_type == "START").count();
    assert_eq!(start_count, 1, "expected exactly 1 START shot");

    // First shot should be START with from_id = -1
    assert_eq!(rows[0].shot_type, "START");
    assert_eq!(rows[0].from_id, -1);
    assert!(rows[0].locked, "START shot should be locked");

    // Next shots should be REAL
    assert_eq!(rows[1].shot_type, "REAL");
    assert_eq!(rows[2].shot_type, "REAL");
}

/// Two lines holding the same foreign identity (a file imported twice) are a
/// state the station graph refuses (a uuid is project-unique), but an undo of
/// a Repair could restore one in 0.6.0, so the user-facing document keeps its
/// safety net: the first copy keeps the id, later copies are re-minted
/// (never dropped), and no `<UUID>` repeats.
#[test]
fn a_twice_imported_identity_exports_once_and_reminted_after() {
    const FOREIGN: &str = "beefbeef-0000-4000-8000-0000000000aa";
    let mut s = Survey::new("Twice");
    for name in ["Copy A", "Copy B"] {
        s.named(name, "#ff0000", shots(2, 10.0, 45.0));
    }
    let project = s.build();
    let mut rows = user_rows(&project);
    // Give each line's first row the same foreign identity, as the
    // duplicated import did, then run the user profile's pass again.
    for name in ["Copy A", "Copy B"] {
        let first = rows
            .iter_mut()
            .find(|r| r.line == name && r.shot_type == "REAL")
            .unwrap();
        first.uuid = Some(FOREIGN.to_string());
    }
    remint_duplicate_uuids(&mut rows);

    let uuids: Vec<&str> = rows.iter().filter_map(|r| r.uuid.as_deref()).collect();
    assert_eq!(uuids.len(), 4, "every REAL shot still emits a <UUID>");
    assert_eq!(
        uuids.iter().filter(|u| **u == FOREIGN).count(),
        1,
        "exactly one copy keeps the foreign identity"
    );
    let distinct: HashSet<&str> = uuids.iter().copied().collect();
    assert_eq!(
        distinct.len(),
        uuids.len(),
        "no <UUID> repeats in the document"
    );
    // Document order puts Copy A's shots first, so the surviving foreign id
    // is the FIRST occurrence.
    let first_foreign = rows
        .iter()
        .find(|r| r.uuid.as_deref() == Some(FOREIGN))
        .unwrap();
    assert_eq!(first_foreign.line, "Copy A");
}

/// A row's `ID` is the Station ID of the station it ends at (X1): on a
/// one-line survey those are the line's stations in order, the START's the
/// first.
#[test]
fn each_row_takes_the_station_id_it_ends_at() {
    let project = simple_project();
    let rows = user_rows(&project);
    let chain: Vec<i64> = project.lines()[0].stations().map(id).collect();
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    assert_eq!(ids, chain);
}

#[test]
fn test_from_id_chain() {
    let rows = user_rows(&simple_project());

    // START has from_id = -1
    assert_eq!(rows[0].from_id, -1);
    // First real shot chains from START
    assert_eq!(rows[1].from_id, rows[0].id);
    // Second real shot chains from first
    assert_eq!(rows[2].from_id, rows[1].id);
}

#[test]
fn test_virtual_shot_type() {
    let mut s = Survey::new("Virtual Test");
    s.named(
        "Sec",
        "#000000",
        vec![
            ShotData {
                kind: ShotKind::Jump,
                ..tape(5.0)
            },
            tape(3.0),
        ],
    );
    let rows = user_rows(&s.build());
    // rows[0] = START, rows[1] = VIRTUAL, rows[2] = REAL
    assert_eq!(rows[1].shot_type, "VIRTUAL");
    assert_eq!(rows[2].shot_type, "REAL");
}

/// A line that starts on a station of another line hangs off that station's
/// row: no START of its own.
#[test]
fn test_multi_line_no_extra_starts() {
    let mut s = Survey::new("Multi");
    let a = s.named(
        "Sec A",
        "#ff0000",
        vec![
            ShotData {
                depth_out: 5.0,
                ..tape(10.0)
            },
            ShotData {
                depth_in: 5.0,
                depth_out: 7.0,
                ..tape(8.0)
            },
        ],
    );
    let b = s.named(
        "Sec B",
        "#00ff00",
        vec![ShotData {
            depth_out: 2.0,
            ..tape(6.0)
        }],
    );
    // 0.6.0's merge of B:0 with A:1: B starts on A's station 1.
    s.merge((b, 0), (a, 1));
    let project = s.build();
    let rows = user_rows(&project);

    // Only ONE start shot should exist (for root line A)
    let start_count = rows.iter().filter(|r| r.shot_type == "START").count();
    assert_eq!(start_count, 1, "expected exactly 1 START shot");

    // Line A: START, REAL, REAL; line B: REAL - no START, linked to its parent.
    assert_eq!(rows.len(), 4);
    let types: Vec<&str> = rows.iter().map(|r| r.shot_type).collect();
    assert_eq!(types, ["START", "REAL", "REAL", "REAL"]);
    assert_eq!(rows[3].line, "Sec B");
    assert_eq!(rows[3].from_id, id(station(&project, (a, 1))));
}

/// A child line joined mid-line (0.6.0: merged at a non-zero station) is
/// walked outwards from the junction: its later shots forward, its earlier
/// shots reversed, both blocks hanging off the junction.
#[test]
fn test_child_merge_station_index_rewrites_matching_shot() {
    let mut s = Survey::new("Child Merge Index");
    let root = s.named("Root", "#ff0000", vec![tape(5.0), tape(5.0), tape(5.0)]);
    let child = s.named("Child", "#00ff00", vec![tape(2.0), tape(2.0), tape(2.0)]);
    // Child station 1 joins root station 2.
    s.merge((child, 1), (root, 2));
    let project = s.build();
    let rows = user_rows(&project);

    let junction = id(station(&project, (root, 2)));
    let at = |index| id(station(&project, (child, index)));
    let fwd_shot0 = row(&rows, at(2));
    let fwd_shot1 = row(&rows, at(3));
    let rev_shot0 = row(&rows, at(0));
    assert_eq!(
        fwd_shot0.from_id, junction,
        "forward block first shot should link to the junction (root station 2)"
    );
    assert_eq!(fwd_shot0.line, "Child");
    assert_eq!(
        fwd_shot1.from_id, fwd_shot0.id,
        "forward block second shot should chain from previous"
    );
    assert_eq!(
        rev_shot0.from_id, junction,
        "reverse block first shot should link to the junction (root station 2)"
    );
    assert_eq!(rev_shot0.line, "Child (reversed)");
}

/// A closure from the station a child starts on (0.6.0: a merged member)
/// ties the junction's one row.
#[test]
fn test_child_station_zero_closure_resolves_to_parent_node() {
    let mut s = Survey::new("Child Station Zero");
    let a = s.named("A", "#ff0000", vec![tape(4.0), tape(4.0)]);
    let b = s.named("B", "#00ff00", vec![tape(3.0)]);
    // B:0 joins A:1.
    s.merge((b, 0), (a, 1));
    // Closure from B:0 (the junction) to A:2.
    s.closure((b, 0), (a, 2), ClosureStrategy::Batch);
    let project = s.build();

    let rows = user_rows(&project);
    let closure = rows.iter().find(|r| r.shot_type == "CLOSURE").unwrap();
    assert_eq!(
        closure.from_id,
        id(station(&project, (a, 1))),
        "B:0 is the junction's row"
    );
    assert_eq!(closure.closure_to_id, id(station(&project, (a, 2))));
}

#[test]
fn test_closure_export_uses_ariane_name_and_depth_in() {
    let mut s = Survey::new("Closure Name");
    let main = s.named("Main", "#ff0000", vec![tape(4.0), tape(4.0)]);
    s.closure((main, 1), (main, 2), ClosureStrategy::Batch);
    let project = s.build();

    let rows = user_rows(&project);
    let closure = rows.iter().find(|r| r.shot_type == "CLOSURE").unwrap();
    assert!(
        closure.depth_in.abs() < f64::EPSILON,
        "closure DepthIn should be exported as 0.0"
    );

    let content = tmlu_of(&project);
    assert!(content.contains("<TY>CLOSURE</TY>"));
    assert!(content.contains("<DPI>0.0</DPI>"));
    assert!(content.contains("<NM>CLOSURE:CLOSURE</NM>"));
}

// ---- GPS on the STARTs -------------------------------------------------

/// A datum mid-line splits its line there: the START stands on the datum's
/// station (its id) and carries the GPS; no other row does.
#[test]
fn test_zero_datum_coordinates_export_to_start_shot() {
    let mut s = Survey::new("Datum Node");
    let main = s.named("Main", "#ff0000", vec![tape(3.0), tape(3.0), tape(3.0)]);
    s.datum((main, 2), GpsCoordinates::new(20.25, -87.5, 12.0).unwrap());
    let project = s.build();

    let rows = user_rows(&project);
    let start = rows.iter().find(|r| r.shot_type == "START").unwrap();
    assert_eq!(start.id, id(station(&project, (main, 2))));
    assert!((start.latitude - 20.25).abs() < f64::EPSILON);
    assert!((start.longitude - (-87.5)).abs() < f64::EPSILON);

    for r in rows.iter().filter(|r| r.shot_type != "START") {
        assert!(r.latitude.abs() < f64::EPSILON && r.longitude.abs() < f64::EPSILON);
    }
}

/// #431 bug pin: a datum station 35 m deep must NOT export as 35 m of
/// entrance elevation. `depth_reference` is "meters below surface"
/// (+down); `firstStartAbsoluteElevation` is "meters above sea level"
/// (+up) - two different fields with no relationship, and a project with
/// no known elevation exports the documented no-data value, 0.0.
#[test]
fn depth_reference_never_leaks_into_entrance_elevation() {
    let mut s = Survey::new("Datum Depth");
    let main = s.named("Main", "#ff0000", vec![tape(3.0)]);
    s.datum((main, 1), GpsCoordinates::new(20.25, -87.5, 35.0).unwrap());
    let project = s.build();
    assert_eq!(project.settings.surface_elevation, None);

    let content = tmlu_of(&project);
    assert!(
        content.contains("<firstStartAbsoluteElevation>0.0</firstStartAbsoluteElevation>"),
        "no known surface_elevation must export as the documented no-data value 0.0: {content}"
    );
    assert!(
        !content.contains("<firstStartAbsoluteElevation>35.0</firstStartAbsoluteElevation>"),
        "the datum's depth_reference must never leak into entrance elevation: {content}"
    );
}

/// A surface below sea level (a below-sea-level cave entrance) survives its
/// sign through export - not flipped positive.
#[test]
fn negative_surface_elevation_survives_its_sign() {
    let mut s = Survey::new("Below Sea Level");
    s.named("Main", "#ff0000", vec![tape(3.0)]);
    let mut project = s.build();
    project.settings.surface_elevation = Some(-5.0);

    let content = tmlu_of(&project);
    assert!(
        content.contains("<firstStartAbsoluteElevation>-5.0</firstStartAbsoluteElevation>"),
        "a negative surface_elevation must not be flipped positive: {content}"
    );
}

/// The `.tml` zip profile is a different code path from `.tmlu` (its own
/// element-name table, `LONG` vs `SHORT`), so the elevation element must
/// be checked there too, not just on the `.tmlu` plain-XML path above.
#[test]
fn surface_elevation_is_emitted_in_the_tml_zip_profile_too() {
    let mut s = Survey::new("Zip Elevation");
    s.named("Main", "#ff0000", vec![tape(3.0)]);
    let mut project = s.build();
    project.settings.surface_elevation = Some(42.5);

    let data_xml = tml_data_xml(&project);
    assert!(
        data_xml.contains("<firstStartAbsoluteElevation>42.5</firstStartAbsoluteElevation>"),
        "the .tml zip profile must emit the elevation same as .tmlu: {data_xml}"
    );
}

// ---- The geometry repair -----------------------------------------------

/// Every Ariane consumer derives a shot's vertical change from its PARENT's
/// `Depth`, so the whole emitted chain has to satisfy
/// `|Depth - parent.Depth| <= Length`. A file that breaks it is not merely
/// odd - SpeleoDB's `openspeleo_lib` raises on the first offender and drops
/// GeoJSON for the ENTIRE project, while the upload still returns 200, so the
/// user is never told their survey has no map.
fn assert_ariane_chain_is_representable(shots: &[ExportShot]) {
    let depth_by_id: HashMap<i64, f64> = shots.iter().map(|s| (s.id, s.depth)).collect();
    for shot in shots {
        if shot.from_id == -1 {
            continue;
        }
        let Some(&parent_depth) = depth_by_id.get(&shot.from_id) else {
            continue;
        };
        let delta = (shot.depth - parent_depth).abs();
        assert!(
            delta <= shot.length,
            "shot id={} (from {}) drops {delta} over a length of {} - impossible geometry",
            shot.id,
            shot.from_id,
            shot.length,
        );
        // The bound above is exactly what the repair enforces, so on its own it
        // certifies whatever the repair produced. Check the file's OTHER depth
        // axis too: a consumer honouring `DepthIn` (ours does) must read the
        // same shot as one walking the parent chain.
        let emitted_in = if shot.depth_in_same_as_prev {
            parent_depth
        } else {
            shot.depth_in
        };
        assert!(
            (shot.depth - emitted_in).abs() <= shot.length,
            "shot id={} drops {} from its own DepthIn over a length of {}",
            shot.id,
            (shot.depth - emitted_in).abs(),
            shot.length,
        );
        // And the change a consumer READS has to be the change the shot
        // RECORDS - same size, same direction. The two bounds above are both
        // satisfied by a shot that has been moved without its file-visible
        // origin moving with it, which is how a re-hung branch once exported a
        // measured descent as a several-metre ascent and still passed here.
        // `depth_in` is the truth throughout: the repair keeps it in step with
        // whatever it writes as the origin.
        let read_change = shot.depth - emitted_in;
        let recorded_change = shot.depth - shot.depth_in;
        assert!(
            (read_change - recorded_change).abs() < 1e-6,
            "shot id={} is read as a {read_change} m vertical change but records \
             {recorded_change} m",
            shot.id,
        );
        assert!(
            shot.depth.is_finite() && shot.length.is_finite() && shot.azimuth.is_finite(),
            "shot id={} carries a non-finite value",
            shot.id,
        );
    }
}

/// A survey with every imported line as its own piece at the origin, the way
/// the hand-run diagnostics below read a real file.
fn survey_of_imported(configs: Vec<crate::models::ImportLineConfig>) -> (Survey, Vec<LineId>) {
    let mut s = Survey::new("Roundtrip");
    let lines = configs
        .into_iter()
        .map(|line| {
            s.line(
                crate::models::survey::LineMeta {
                    name: line.name,
                    color: line.color,
                    quality: line.quality,
                    ..Default::default()
                },
                line.shots,
            )
        })
        .collect();
    (s, lines)
}

/// Round-trip a REAL `.tml` through the importer and back out, then assert the
/// emitted chain is representable. Env-gated in the same spirit as the SpeleoDB
/// live tests: it needs a file we cannot commit (someone's survey), so it skips
/// cleanly when `TML_ROUNDTRIP_IN` is unset.
///
/// ```text
/// TML_ROUNDTRIP_IN=/path/in.tml TML_ROUNDTRIP_OUT=/path/out.tml \
///   cargo test -p stalagmight-export reexport -- --ignored --nocapture
/// ```
#[test]
#[ignore = "needs a real .tml via TML_ROUNDTRIP_IN"]
#[expect(
    clippy::print_stdout,
    reason = "a hand-run diagnostic tool; the shot count and output path are its result"
)]
fn reexport_of_a_real_tml_keeps_the_chain_representable() {
    let Ok(input) = std::env::var("TML_ROUNDTRIP_IN") else {
        return;
    };
    let tml = crate::parsers::tml::parse_tml(Path::new(&input)).unwrap();
    let (lines, _closures, _warnings) =
        crate::parsers::tml::convert_tml_to_import_configs(&tml, false, true).unwrap();

    // Straight into the model rather than through the import command: the
    // shot chain is what this asserts on, and the import epilogue (placement,
    // closures, junctions) needs an `AppState` that adds nothing here.
    let (mut s, ids) = survey_of_imported(lines);

    // Optional anchor, so the re-exported file can be taken all the way to
    // GeoJSON by a consumer (which needs one) rather than stopping at the
    // by-design no-anchor skip.
    if std::env::var("TML_ROUNDTRIP_ANCHOR").is_ok() {
        if let Some(&first) = ids.first() {
            s.datum((first, 0), gps(20.25, -87.5));
        }
    }
    let project = s.build();

    let rows = user_rows(&project);
    println!("re-exported {} shots from {input}", rows.len());
    assert_ariane_chain_is_representable(&rows);

    if let Ok(out) = std::env::var("TML_ROUNDTRIP_OUT") {
        export_tml(&project, Path::new(&out)).unwrap();
        println!("wrote {out}");
    }
}

/// Companion to `reexport_of_a_real_tml_keeps_the_chain_representable`, from the
/// project side: exports a real `.stalx` (either format: a legacy file is
/// migrated in memory, as the app opens it) and checks Ariane can root every
/// shot. A parentless non-START shot plus any closure blanks Ariane's map
/// entirely. Skips when the var is unset.
///
/// ```text
/// STALX_EXPORT_IN=/path/project.stalx \
///   cargo test -p stalagmight-export export_of_a_real_stalx -- --ignored --nocapture
/// ```
#[test]
#[ignore = "needs a real .stalx via STALX_EXPORT_IN"]
#[expect(
    clippy::print_stdout,
    reason = "a hand-run diagnostic tool; the orphan list is its result"
)]
fn export_of_a_real_stalx_roots_every_shot() {
    use stalagmight_persistence::persistence::{migrate, stalx_v2};
    let Ok(input) = std::env::var("STALX_EXPORT_IN") else {
        return;
    };
    let bytes = std::fs::read(&input).unwrap();
    let project = match stalx_v2::sniff(&bytes) {
        stalx_v2::Format::V2 => {
            stalx_v2::decode(&bytes, &stalx_v2::DecodeOptions::default())
                .unwrap()
                .project
        }
        _ => migrate::migrate_bytes(&bytes).unwrap().project,
    };

    let rows = user_rows(&project);
    let orphans: Vec<&ExportShot> = rows
        .iter()
        .filter(|r| r.from_id == -1 && r.shot_type != "START")
        .collect();
    for o in &orphans {
        println!(
            "  ORPHAN id={} type={} line={:?}",
            o.id, o.shot_type, o.line
        );
    }
    println!(
        "{} shots, {} orphan(s) from {input}",
        rows.len(),
        orphans.len()
    );

    assert!(
        orphans.is_empty(),
        "{} parentless non-START shot(s) would blank Ariane's map",
        orphans.len()
    );

    if let Ok(out) = std::env::var("STALX_EXPORT_OUT") {
        export_tml(&project, Path::new(&out)).unwrap();
        println!("wrote {out}");
    }
}

#[test]
fn closure_shots_carry_the_depths_of_the_stations_they_tie() {
    // They used to be written with `depth: 0.0` regardless of where the
    // stations actually are, which reads as a connector with a full-depth
    // drop and no length.
    let mut s = Survey::new("Closure Depths");
    let main = s.named(
        "Main",
        "#ff0000",
        vec![
            ShotData {
                depth_in: 0.0,
                depth_out: 5.0,
                ..shot(10.0, 0.0)
            },
            ShotData {
                depth_in: 5.0,
                depth_out: 9.0,
                ..shot(10.0, 90.0)
            },
        ],
    );
    s.closure((main, 1), (main, 2), ClosureStrategy::Batch);
    let rows = user_rows(&s.build());
    let closure = rows.iter().find(|r| r.shot_type == "CLOSURE").unwrap();
    assert!(
        (closure.depth - 9.0).abs() < 1e-9,
        "the connector sits at the depth of the station it closes onto, got {}",
        closure.depth
    );
    assert!(
        (closure.depth_in - 5.0).abs() < 1e-9,
        "and starts at the depth of the station it leaves, got {}",
        closure.depth_in
    );
    assert!(
        closure.length >= (closure.depth - closure.depth_in).abs(),
        "its length must cover its own vertical change"
    );
    assert_ariane_chain_is_representable(&rows);
}

#[test]
fn a_junction_depth_discontinuity_is_re_hung_rather_than_exported_impossible() {
    // A branch that starts at a different depth than the station it hangs off
    // is representable in our model and is NOT representable in Ariane's
    // chain. Exported verbatim it reads as a shot with more drop than length.
    let mut s = Survey::new("Junction");
    let parent = s.named(
        "Parent",
        "#ff0000",
        vec![ShotData {
            depth_in: 0.0,
            depth_out: 17.0,
            ..shot(20.0, 0.0)
        }],
    );
    // The branch starts 12.2 m down while the station it starts on is 17 m
    // down: a 4.8 m step across a 1.07 m shot.
    s.branch(
        meta("Branch", "#00ff00"),
        (parent, 1),
        vec![ShotData {
            depth_in: 12.21,
            depth_out: 12.5,
            ..shot(1.07, 45.0)
        }],
    );
    let rows = user_rows(&s.build());
    assert_ariane_chain_is_representable(&rows);
}

#[test]
fn a_re_hung_branch_keeps_its_measured_descent() {
    // The repair may move a branch's absolute depth - Ariane's chain cannot
    // express the junction it was measured from - but it must never change how
    // far or which WAY the shot goes. Asserting only the parent-relative bound
    // let an earlier version export this 0.29 m descent as a 1.07 m ascent.
    let start = geometry_shot(1, -1, 17.0, 17.0, 0.0, false);
    // The branch's first shot: flagged same-as-previous even though it
    // starts 4.79 m off the parent, which is the whole problem.
    let mut branch = geometry_shot(2, 1, 12.21, 12.5, 1.07, true);
    branch.azimuth = 45.0;
    let mut shots = vec![start, branch];

    enforce_ariane_geometry_invariants(&mut shots, TmlProfile::User);

    let measured_descent = 12.5 - 12.21;
    let exported_descent = shots[1].depth - shots[0].depth;
    assert!(
        (exported_descent - measured_descent).abs() < 1e-6,
        "the shot descends {measured_descent} m; exported as {exported_descent} m"
    );
    assert_ariane_chain_is_representable(&shots);
}

/// A shot carrying only the fields the geometry pass reads, so a chain of
/// them stays legible. Everything else is filler.
fn geometry_shot(
    id: i64,
    from_id: i64,
    depth_in: f64,
    depth: f64,
    length: f64,
    depth_in_same_as_prev: bool,
) -> ExportShot {
    ExportShot {
        uuid: None,
        id,
        from_id,
        shot_type: if from_id == -1 { "START" } else { "REAL" },
        line: "S".to_string(),
        azimuth: 0.0,
        length,
        depth,
        depth_in,
        depth_in_same_as_prev,
        inclination: 0.0,
        left: 0.0,
        right: 0.0,
        up: 0.0,
        down: 0.0,
        comment: None,
        explorer: None,
        date: None,
        color: "0xff0000ff".to_string(),
        latitude: 0.0,
        longitude: 0.0,
        closure_to_id: -1,
        locked: false,
    }
}

#[test]
fn a_re_hang_moves_the_rest_of_the_branch_with_it() {
    // Only the branch's FIRST shot breaks the bound, so a repair that stopped
    // there looked complete. The shot after it is contiguous in the model and
    // therefore writes the DepthIn sentinel - "I start where my parent ended" -
    // which now resolves to the moved depth. Left where the model put it, its
    // measured 0.5 m descent was read as a 4.29 m ascent: right magnitude for
    // nothing, wrong direction, and still inside its 5 m tape so every bound
    // held.
    let mut shots = vec![
        geometry_shot(1, -1, 17.0, 17.0, 0.0, false),
        geometry_shot(2, 1, 12.21, 12.5, 1.07, true),
        geometry_shot(3, 2, 12.5, 13.0, 5.0, true),
    ];

    enforce_ariane_geometry_invariants(&mut shots, TmlProfile::User);

    assert!(
        (shots[1].depth - 17.29).abs() < 1e-9,
        "the re-hung shot keeps its 0.29 m descent from the parent, got {}",
        shots[1].depth
    );
    let second = shots[2].depth - shots[1].depth;
    assert!(
        (second - 0.5).abs() < 1e-9,
        "and the shot below it still descends 0.5 m, got {second}"
    );
    assert_ariane_chain_is_representable(&shots);
}

#[test]
fn a_branch_that_writes_the_sentinel_is_hung_off_its_parent_even_within_the_bound() {
    // The same defect without any re-hang upstream. A branch's first shot is
    // always flagged same-as-previous (it is contiguous with its own line's
    // station), so it writes the sentinel and a consumer reads its origin as
    // the parent's depth - even though it was measured from a junction 4.79 m
    // away. With a long enough tape the bound holds and nothing looks wrong,
    // while the file states a 4.5 m ascent for a surveyed 0.29 m descent.
    let mut shots = vec![
        geometry_shot(1, -1, 17.0, 17.0, 0.0, false),
        geometry_shot(2, 1, 12.21, 12.5, 10.0, true),
    ];

    enforce_ariane_geometry_invariants(&mut shots, TmlProfile::User);

    let exported = shots[1].depth - shots[0].depth;
    assert!(
        (exported - 0.29).abs() < 1e-9,
        "the branch descends 0.29 m from its parent; exported as {exported} m"
    );
    assert_ariane_chain_is_representable(&shots);
}

#[test]
fn a_shot_exactly_on_the_bound_is_left_alone_and_one_past_it_is_clamped() {
    // `|Depth - parent.Depth| == Length` is a legal, perfectly vertical shot,
    // not a violation - moving it would invent a horizontal component. One
    // ten-thousandth over is broken data whichever way it is read, so the
    // repair clamps it back onto the tape rather than exporting an impossible
    // shot.
    let mut shots = vec![
        geometry_shot(1, -1, 0.0, 10.0, 0.0, false),
        geometry_shot(2, 1, 10.0, 13.0, 3.0, true),
        geometry_shot(3, 2, 13.0, 16.0001, 3.0, true),
    ];

    enforce_ariane_geometry_invariants(&mut shots, TmlProfile::User);

    assert!(
        (shots[1].depth - 13.0).abs() < 1e-9,
        "the exact-bound shot is untouched, got {}",
        shots[1].depth
    );
    assert!(
        (shots[2].depth - 16.0).abs() < 1e-9,
        "the over-bound shot is clamped to its own length, got {}",
        shots[2].depth
    );
    assert_ariane_chain_is_representable(&shots);
}

#[test]
fn a_from_id_cycle_keeps_its_measured_lengths() {
    // A cycle is unreachable from the root, so the walk never repairs it - and
    // the as-written sweep must not "fix" it either: the gap to its parent is
    // the full unrepaired one, and writing that into Length would swap a
    // measured tape reading for a fabricated one. A consumer reaches shots the
    // same way we do, from the anchor down, so it drops these before it ever
    // looks at their geometry.
    let mut shots = vec![
        geometry_shot(1, -1, 0.0, 0.0, 0.0, false),
        geometry_shot(2, 3, 0.0, 40.0, 1.0, false),
        geometry_shot(3, 2, 40.0, 0.0, 1.0, false),
    ];

    enforce_ariane_geometry_invariants(&mut shots, TmlProfile::User);

    assert!(
        (shots[1].length - 1.0).abs() < f64::EPSILON
            && (shots[2].length - 1.0).abs() < f64::EPSILON,
        "the cycle's tape readings survive, got {} and {}",
        shots[1].length,
        shots[2].length
    );
}

#[test]
fn a_depth_in_that_would_be_written_as_the_sentinel_is_nudged_off_it() {
    // The guard has to judge the value AS WRITTEN. `format_f64` rounds to six
    // decimals, so a station a hair under a metre above the datum lands in the
    // file as "-1.0" and comes back through our own parser as "starts where
    // the previous shot ended" - relocating it to wherever that was.
    for depth_in in [-1.0, -0.999_999_6, -1.000_000_4] {
        let shot = geometry_shot(2, 1, depth_in, depth_in, 1.0, false);
        let written = format_f64(emitted_depth_in(&shot));
        assert_ne!(
            written, "-1.0",
            "depth_in {depth_in} must not be written as the sentinel"
        );
        assert!(
            (emitted_depth_in(&shot) - depth_in).abs() < 1e-5,
            "and the nudge stays inside a rounding error of the truth, got {written}"
        );
    }
}

#[test]
fn non_finite_geometry_never_reaches_the_file() {
    // NaN fails every comparison, including the one a consumer uses to decide
    // whether a shot is possible, so it aborts the same conversion.
    let mut broken = geometry_shot(2, 1, -1.0, f64::NAN, f64::INFINITY, true);
    broken.azimuth = f64::NAN;
    broken.inclination = f64::NEG_INFINITY;
    let mut shots = vec![geometry_shot(1, -1, 0.0, 0.0, 0.0, false), broken];

    enforce_ariane_geometry_invariants(&mut shots, TmlProfile::User);

    assert_ariane_chain_is_representable(&shots);
}

// ---- Entrances, splits and the walk ------------------------------------

#[test]
fn test_nonzero_root_datum_splits_line_into_two_branches() {
    let mut s = Survey::new("Split Root");
    let main = s.named(
        "Main",
        "#ff0000",
        vec![
            ShotData {
                azimuth_in: 10.0,
                azimuth_out: 210.0,
                length: 1.0,
                depth_in: 0.0,
                depth_out: 1.0,
                ..Default::default()
            },
            ShotData {
                azimuth_in: 20.0,
                azimuth_out: 15.0,
                length: 1.0,
                depth_in: 1.0,
                depth_out: 2.0,
                ..Default::default()
            },
            ShotData {
                azimuth_in: 30.0,
                azimuth_out: 230.0,
                length: 1.0,
                depth_in: 2.0,
                depth_out: 3.0,
                ..Default::default()
            },
        ],
    );
    s.datum((main, 2), gps(20.0, -87.0));
    let rows = user_rows(&s.build());
    let start = rows.iter().find(|r| r.shot_type == "START").unwrap();

    // Split produces two first shots from START: forward segment and reversed prefix.
    let from_start: Vec<&ExportShot> = rows
        .iter()
        .filter(|r| r.shot_type != "START" && r.from_id == start.id)
        .collect();
    assert_eq!(from_start.len(), 2);
    assert!(
        from_start
            .iter()
            .any(|r| (r.azimuth - 30.0).abs() < f64::EPSILON),
        "forward branch should keep original azimuth at datum"
    );
    assert!(
        from_start
            .iter()
            .any(|r| (r.azimuth - 200.0).abs() < f64::EPSILON),
        "reverse branch should invert forward azimuth of prefix shot"
    );
}

#[test]
fn test_parent_links_reoriented_when_datum_forces_root() {
    let mut s = Survey::new("Datum Root Orientation");
    let a = s.named("A", "#ff0000", vec![tape(2.0), tape(2.0)]);
    let b = s.named("B", "#00ff00", vec![tape(2.0), tape(2.0), tape(2.0)]);
    // A:1 and B:2 are one station; the datum on it is set through B.
    s.merge((a, 1), (b, 2));
    s.datum((b, 2), gps(20.0, -87.0));
    let rows = user_rows(&s.build());
    let start = rows.iter().find(|r| r.shot_type == "START").unwrap();
    assert_eq!(start.line, "B");

    // Line A should now link into the forced-root line B.
    assert!(
        rows.iter().any(|r| r.line == "A" && r.from_id == start.id),
        "expected a line A shot to link to B START after rooting at datum"
    );
}

#[test]
fn test_round_trip_tmlu() {
    let project = simple_project();
    let file = NamedTempFile::with_suffix(".tmlu").unwrap();
    export_tml(&project, file.path()).unwrap();

    // Re-import
    let parsed = crate::parsers::tml::parse_tml(file.path()).unwrap();

    assert_eq!(parsed.cave_name, "Test Cave");
    assert_eq!(parsed.unit, crate::parsers::tml::TmlUnit::Metric);
    assert!(parsed.use_magnetic_azimuth);

    // Should have one line with 2 shots
    assert_eq!(
        parsed.lines.len(),
        1,
        "expected 1 line, got {}",
        parsed.lines.len()
    );
    let sec = &parsed.lines[0];
    assert_eq!(
        sec.shots.len(),
        2,
        "expected 2 shots, got {}",
        sec.shots.len()
    );

    // Verify shot data round-trips
    let s0 = &sec.shots[0];
    assert!((s0.azimuth - 45.0).abs() < 0.01);
    assert!((s0.length - 10.0).abs() < 0.01);

    let s1 = &sec.shots[1];
    assert!((s1.azimuth - 90.0).abs() < 0.01);
    assert!((s1.length - 8.0).abs() < 0.01);
}

/// Export `project`, parse it back and return every re-imported shot's
/// credit, in file order.
fn reimported_credits(project: &Project) -> Vec<SurveyCredit> {
    let file = NamedTempFile::with_suffix(".tmlu").unwrap();
    export_tml(project, file.path()).unwrap();
    let parsed = crate::parsers::tml::parse_tml(file.path()).unwrap();
    let (configs, _, _) =
        crate::parsers::tml::convert_tml_to_import_configs(&parsed, false, false).unwrap();
    configs
        .iter()
        .flat_map(|c| c.shots.iter().map(|s| s.credit().clone()))
        .collect()
}

/// A one-line survey ("Main") whose shots carry `credits`, one each.
fn credited_project(name: &str, credits: &[SurveyCredit]) -> Project {
    let mut s = Survey::new(name);
    s.line(
        crate::models::survey::LineMeta {
            name: Some("Main".to_string()),
            ..Default::default()
        },
        credits
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let i = i as f64;
                ShotData {
                    depth_in: i,
                    depth_out: i + 1.0,
                    credit: stored(c.clone()),
                    ..shot(5.0, 10.0 * i)
                }
            })
            .collect(),
    );
    s.build()
}

/// Per-row survey credit (#329): every combination of roles leaves as
/// Ariane's `Explorer` field (always the element form, so a row of
/// explorers alone is not read back as surveyors) and comes back to the
/// same people on the same shot, and a second export of what came back is
/// the same again. Two one-word names in a role stay two people: a TML
/// role is a name list, with no `Last, First` rule. (A `,` inside ONE name
/// cannot round-trip: Ariane separates people with commas.)
#[test]
fn survey_credit_round_trips_per_row_and_is_stable() {
    let p = |n: &str| Person::named(n).unwrap();
    let day = |d: u32| chrono::NaiveDate::from_ymd_opt(2024, 3, d);
    let credits = vec![
        SurveyCredit {
            surveyors: vec![p("S One"), p("S Two")],
            explorers: vec![p("E One")],
            date: day(1),
        },
        SurveyCredit {
            surveyors: vec![p("Only Surveyor")],
            explorers: Vec::new(),
            date: day(2),
        },
        SurveyCredit {
            surveyors: Vec::new(),
            explorers: vec![p("Only Explorer"), p("Second")],
            date: day(3),
        },
        SurveyCredit {
            surveyors: Vec::new(),
            explorers: Vec::new(),
            date: day(4),
        },
        SurveyCredit::default(),
        SurveyCredit {
            surveyors: vec![p("Ann"), p("Bo")],
            explorers: vec![p("Cy"), p("Di")],
            date: day(5),
        },
        SurveyCredit {
            surveyors: vec![p("Solo")],
            explorers: Vec::new(),
            date: day(6),
        },
    ];

    let once = reimported_credits(&credited_project("Credit Cave", &credits));
    // `<Date>` is not optional to every consumer, so an undated row leaves
    // with the stand-in date and comes back with it - the one value that
    // changes, and only on the first trip.
    let mut expected = credits;
    expected[4].date = chrono::NaiveDate::from_ymd_opt(2000, 1, 1);
    assert_eq!(
        once, expected,
        "every row comes back with its own people and date"
    );
    let again = credited_project("Credit Cave", &once);
    assert_eq!(
        reimported_credits(&again),
        once,
        "and a re-export of that is stable"
    );
}

/// Names that look like the field's own syntax keep their person and
/// their role through an export and re-import: an explorer `"A / B"` stays
/// one explorer (the plain form would read it as explorer then surveyor),
/// and a name holding `</Explorer><Surveyor>` cannot move anyone between
/// roles.
#[test]
fn names_that_look_like_the_field_syntax_keep_their_role() {
    let reimport = |credit: SurveyCredit| {
        reimported_credits(&credited_project("Syntax", &[credit]))[0].clone()
    };
    let person = |n: &str| Person::named(n).unwrap();
    let day = chrono::NaiveDate::from_ymd_opt(2024, 1, 2);

    let back = reimport(SurveyCredit {
        explorers: vec![person("A / B")],
        surveyors: Vec::new(),
        date: day,
    });
    assert_eq!(back.explorers, vec![person("A / B")]);
    assert!(back.surveyors.is_empty());

    let back = reimport(SurveyCredit {
        explorers: vec![person("x</Explorer><Surveyor>y")],
        surveyors: vec![person("Real <Surveyor>")],
        date: day,
    });
    assert_eq!(back.explorers, vec![person("x/ExplorerSurveyory")]);
    assert_eq!(back.surveyors, vec![person("Real Surveyor")]);

    let back = reimport(SurveyCredit {
        explorers: vec![person("x</Explorer><Surveyor>y")],
        surveyors: Vec::new(),
        date: day,
    });
    assert_eq!(back.explorers, vec![person("x/ExplorerSurveyory")]);
    assert!(back.surveyors.is_empty());
}

#[test]
fn test_round_trip_tml_zip() {
    let project = simple_project();
    let file = NamedTempFile::with_suffix(".tml").unwrap();
    export_tml(&project, file.path()).unwrap();

    // Re-import
    let parsed = crate::parsers::tml::parse_tml(file.path()).unwrap();

    assert_eq!(parsed.cave_name, "Test Cave");
    assert_eq!(parsed.lines.len(), 1);
    assert_eq!(parsed.lines[0].shots.len(), 2);
}

#[test]
fn test_unicode_line_name() {
    let mut s = Survey::new("Grotte de l'Ours");
    s.named(
        "Galería Principal",
        "#ff0000",
        vec![ShotData {
            length: 5.0,
            ..line_credit("José García", None)
        }],
    );
    let content = tmlu_of(&s.build());
    // quick_xml escapes apostrophes to &apos;
    assert!(content.contains("Grotte de l") && content.contains("Ours"));
    assert!(content.contains("Galería Principal"));
    assert!(content.contains("José García"));
}

#[test]
fn test_depth_in_sentinel() {
    // When depth_in equals the previous depth_out, export should use -1.0
    let mut s = Survey::new("Depth");
    s.named(
        "Sec",
        "#ff0000",
        vec![
            ShotData {
                depth_in: 0.0,
                depth_out: 5.0,
                ..tape(10.0)
            },
            ShotData {
                depth_in: 5.0, // equals previous depth_out -> should be -1.0
                depth_out: 8.0,
                ..tape(7.0)
            },
            ShotData {
                depth_in: 6.0, // differs from previous depth_out (8.0) -> 6.0
                depth_out: 10.0,
                ..tape(5.0)
            },
        ],
    );
    let rows = user_rows(&s.build());
    // rows[0] = START, rows[1..3] = REAL
    // `depth_in` always carries the TRUE start depth and the sentinel rides
    // in its own flag, so assert on what actually reaches the file.
    assert!(
        (emitted_depth_in(&rows[1]) - (-1.0)).abs() < 1e-9,
        "first shot writes the sentinel (same as start)"
    );
    assert!(
        (emitted_depth_in(&rows[2]) - (-1.0)).abs() < 1e-9,
        "second shot writes the sentinel (same as prev depth_out)"
    );
    assert!(
        (emitted_depth_in(&rows[3]) - 6.0).abs() < 1e-9,
        "third shot writes 6.0 (differs from prev depth_out)"
    );
    assert!(
        (rows[2].depth_in - 5.0).abs() < 1e-9,
        "and the true start depth is kept alongside it, so a repair can read \
         the measured vertical change back"
    );
}

#[test]
fn test_id_element_is_all_caps() {
    let data_xml = tml_data_xml(&simple_project());
    // Must be <ID>, never <Id>
    assert!(data_xml.contains("<ID>"), "ID element must be all-caps");
    assert!(!data_xml.contains("<Id>"), "must not use <Id> casing");
}

#[test]
fn test_required_fields_present() {
    let content = tmlu_of(&simple_project());
    // Every shot must have Locked, Name, and Profiletype (short names: LCK, NM, PT)
    assert!(content.contains("<LCK>"));
    assert!(content.contains("<NM>"));
    assert!(content.contains("<PT>VERTICAL</PT>"));
}

/// A child whose parent station is ANOTHER line's junction must still be
/// rooted, whatever order the lines happen to be stored in.
///
/// 0.6.0 resolved blocks in storage order, which left a child stored earlier
/// than the line it hangs off pointing at nothing, shipped with
/// `from_id = -1`: Ariane reads that as a second entrance, abandons the loop
/// solve, and draws an empty map for any survey that also has closures (a
/// real 538-shot export, at a station shared by three lines). The station
/// graph walk visits lines through their junctions, so the order cannot
/// matter; this pins it.
#[test]
fn a_child_of_a_merge_station_is_rooted_regardless_of_line_order() {
    let mut s = Survey::new("Out Of Order");
    // Deliberately added BEFORE the line it hangs off: Root, Branch, Middle.
    let ids: Vec<LineId> = ["Root", "Branch", "Middle"]
        .iter()
        .map(|name| s.named(name, "#ff0000", shots(2, 5.0, 90.0)))
        .collect();
    let (root, branch, middle) = (ids[0], ids[1], ids[2]);
    // Middle hangs off Root, and Branch starts on Middle's station 0 - the
    // same station, so three lines meet there.
    s.merge((middle, 0), (root, 1));
    s.merge((branch, 0), (middle, 0));
    let rows = user_rows(&s.build());

    let orphans: Vec<&ExportShot> = rows
        .iter()
        .filter(|r| r.from_id == -1 && r.shot_type != "START")
        .collect();
    assert!(
        orphans.is_empty(),
        "parentless non-START shot(s) {:?} - Ariane would draw an empty map",
        orphans
            .iter()
            .map(|r| (r.id, r.shot_type, r.line.as_str()))
            .collect::<Vec<_>>()
    );

    // And the guard agrees, so a regression cannot slip out as a written file.
    assert!(ensure_rooted_chain(&rows, TmlProfile::User).is_ok());
}

/// Two galleries that share no station are an ordinary project, not an
/// error. Each is an entrance and gets its own START, which is how Ariane
/// stores them (its own files ship two). Before this, the second gallery's
/// first shot was left parentless - harmless until the survey gained a
/// closure, at which point Ariane drew nothing at all.
#[test]
fn a_detached_line_exports_as_its_own_entrance() {
    let mut s = Survey::new("Detached");
    let a = s.named("Gallery A", "#ff0000", vec![shot(5.0, 90.0)]);
    s.named("Gallery B", "#ff0000", vec![shot(5.0, 90.0)]);
    s.datum((a, 0), gps(20.25, -87.5));
    let rows = user_rows(&s.build());

    let starts: Vec<&ExportShot> = rows.iter().filter(|r| r.shot_type == "START").collect();
    assert_eq!(starts.len(), 2, "each detached gallery is its own entrance");

    // Every entrance is anchored. Leaving the second at 0,0 puts that
    // gallery on null island, ~9000 km from the rest of the survey. Both
    // galleries start on local (0, 0), so the datum-less one's surveyed
    // position is the datum's GPS, to within the `local_to_gps` round trip.
    for start in &starts {
        assert!(
            (start.latitude - 20.25).abs() < 1e-9 && (start.longitude - -87.5).abs() < 1e-9,
            "entrance in line {:?} was left unanchored at {},{}",
            start.line,
            start.latitude,
            start.longitude
        );
    }
    assert_no_orphan(&rows);
    assert!(ensure_rooted_chain(&rows, TmlProfile::User).is_ok());
}

/// A gallery of joined lines that touches nothing else is ONE entrance, not
/// one per line (0.6.0 once rooted every line of a second component as its
/// own entrance, stacked on one anchor, with its junctions dropped).
#[test]
fn a_disconnected_gallery_is_rooted_once_not_per_line() {
    let mut s = Survey::new("Two Galleries");
    // Root component: A. Second component: B -> C -> D, chained to each other only.
    let ids: Vec<LineId> = ["A", "B", "C", "D"]
        .iter()
        .map(|name| s.named(name, "#ff0000", shots(2, 5.0, 90.0)))
        .collect();
    s.merge((ids[2], 0), (ids[1], 1));
    s.merge((ids[3], 0), (ids[2], 1));
    let rows = user_rows(&s.build());
    let starts: Vec<&ExportShot> = rows.iter().filter(|r| r.shot_type == "START").collect();

    assert_eq!(
        starts.len(),
        2,
        "one entrance per component, got {:?}",
        starts.iter().map(|r| r.line.as_str()).collect::<Vec<_>>()
    );
    assert!(
        ensure_rooted_chain(&rows, TmlProfile::User).is_ok(),
        "the second component must be rooted, not orphaned"
    );
}

/// The guard is the backstop for a chain the builder should never produce. It must
/// fire on the orphan+closure COMBINATION only - an orphan alone still draws, so
/// refusing it would cost a user an export for nothing - and it must stay
/// Ariane-only: the SpeleoDB sidecar pads a lone START with a parentless VIRTUAL
/// shot on purpose.
#[test]
fn the_guard_rejects_an_orphan_only_when_a_closure_is_present() {
    let mut s = Survey::new("Guard");
    s.named("Only", "#ff0000", shots(2, 5.0, 90.0));
    let mut rows = user_rows(&s.build());
    assert!(ensure_rooted_chain(&rows, TmlProfile::User).is_ok());

    // Force the exact shape the fix prevents.
    let last = rows.last_mut().unwrap();
    last.from_id = -1;
    assert_ne!(last.shot_type, "START");

    // An orphan ALONE is survivable, and such files exported and drew correctly
    // before. Refusing here would turn a drawable file into no file at all.
    assert!(
        ensure_rooted_chain(&rows, TmlProfile::User).is_ok(),
        "an orphan with no closure must still export - Ariane draws it"
    );

    // Turn a DIFFERENT shot into a closure - the same chain is now the blank-map
    // combination, and only the pairing changed.
    let orphan_idx = rows.len() - 1;
    rows[orphan_idx - 1].shot_type = "CLOSURE";
    assert_eq!(
        rows[orphan_idx].from_id, -1,
        "the orphan is still the orphan"
    );

    assert!(
        ensure_rooted_chain(&rows, TmlProfile::User).is_err(),
        "an orphan alongside a closure must not reach a written Ariane file"
    );
    assert!(
        ensure_rooted_chain(&rows, TmlProfile::SpeleoDbSidecar).is_ok(),
        "the sidecar's deliberate VIRTUAL pad must still be allowed"
    );
}

/// A child joined at its station 2 (0.6.0: merged there) is split there:
/// its later shots forward off the junction, its earlier shots reversed off
/// it, each block chaining its own rows.
#[test]
fn test_child_nonzero_merge_splits_line() {
    let mut s = Survey::new("Split Child");
    let root = s.named("Root", "#ff0000", shots(2, 5.0, 90.0));
    let branch = s.named(
        "Branch",
        "#00ff00",
        vec![
            shot(3.0, 10.0),
            shot(4.0, 20.0),
            shot(5.0, 30.0),
            shot(6.0, 40.0),
        ],
    );
    s.merge((branch, 2), (root, 1));
    let project = s.build();
    let rows = user_rows(&project);
    let junction = id(station(&project, (root, 1)));
    let at = |index| id(station(&project, (branch, index)));

    // Forward block (shots 2, 3) chains from the junction.
    let fwd0 = row(&rows, at(3));
    let fwd1 = row(&rows, at(4));
    assert_eq!(fwd0.from_id, junction, "forward first shot links to parent");
    assert_eq!(fwd0.line, "Branch");
    assert_eq!(
        fwd1.from_id, fwd0.id,
        "forward second shot chains from previous"
    );

    // Reverse block (shots 1, 0 reversed) chains from the junction.
    let rev0 = row(&rows, at(1));
    let rev1 = row(&rows, at(0));
    assert_eq!(rev0.from_id, junction, "reverse first shot links to parent");
    assert_eq!(rev0.line, "Branch (reversed)");
    assert_eq!(
        rev1.from_id, rev0.id,
        "reverse second shot chains from previous"
    );

    // Reverse shots have reciprocal azimuths: shot 1 (20) -> 200, shot 0 (10) -> 190.
    assert!(
        (rev0.azimuth - 200.0).abs() < 1e-9,
        "first reversed shot should have reciprocal azimuth of shot 1"
    );
    assert!(
        (rev1.azimuth - 190.0).abs() < 1e-9,
        "second reversed shot should have reciprocal azimuth of shot 0"
    );

    assert_no_orphan(&rows);
}

/// A child joined at its last station is walked back entirely, reversed.
#[test]
fn test_child_end_merge_reverses_line() {
    let mut s = Survey::new("End Merge");
    let root = s.named("Root", "#ff0000", shots(2, 5.0, 90.0));
    let tail = s.named(
        "Tail",
        "#00ff00",
        vec![shot(3.0, 10.0), shot(4.0, 20.0), shot(5.0, 30.0)],
    );
    s.merge((tail, 3), (root, 2));
    let project = s.build();
    let rows = user_rows(&project);
    let junction = id(station(&project, (root, 2)));
    let at = |index| id(station(&project, (tail, index)));

    // Shots 2, 1, 0 reversed, chained off the junction.
    let rev0 = row(&rows, at(2));
    let rev1 = row(&rows, at(1));
    let rev2 = row(&rows, at(0));
    assert_eq!(
        rev0.from_id, junction,
        "first reversed shot links to parent"
    );
    assert_eq!(rev0.line, "Tail (reversed)");
    assert_eq!(rev1.from_id, rev0.id, "second reversed shot chains");
    assert_eq!(rev2.from_id, rev1.id, "third reversed shot chains");

    assert!(
        (rev0.azimuth - 210.0).abs() < 1e-9,
        "shot 2 (az=30) reversed -> 210"
    );
    assert!(
        (rev1.azimuth - 200.0).abs() < 1e-9,
        "shot 1 (az=20) reversed -> 200"
    );
    assert!(
        (rev2.azimuth - 190.0).abs() < 1e-9,
        "shot 0 (az=10) reversed -> 190"
    );

    assert_no_orphan(&rows);
}

/// A line hanging off the end of a split child's forward block hangs off
/// the row of that station.
#[test]
fn test_downstream_of_split_child_resolves() {
    let mut s = Survey::new("Downstream");
    let a = s.named("A", "#ff0000", vec![tape(5.0), tape(5.0)]);
    let b = s.named("B", "#00ff00", vec![tape(3.0), tape(4.0), tape(5.0)]);
    let c = s.named("C", "#0000ff", vec![tape(2.0)]);
    // B:1 joins A:1 (a split child); C starts on B:3.
    s.merge((b, 1), (a, 1));
    s.merge((c, 0), (b, 3));
    let project = s.build();
    let rows = user_rows(&project);

    let c_shot = row(&rows, id(station(&project, (c, 1))));
    assert_eq!(
        c_shot.from_id,
        id(station(&project, (b, 3))),
        "C's first shot should chain from B's forward block station 3"
    );
    assert_eq!(c_shot.line, "C");

    assert_no_orphan(&rows);
}

// ---- SpeleoDB sidecar profile ------------------------------------------

/// The check SpeleoDB's converter runs before it will draw anything: it
/// parses `Depth` and `Length` back out of the XML and rejects the WHOLE
/// survey when a shot's drop from its parent's depth exceeds its tape. There
/// is no epsilon, so the six-decimal write is part of the check - which is
/// why this measures the formatted values rather than the f64s we hold.
fn assert_written_chain_satisfies_speleodb(shots: &[ExportShot]) {
    let depth_by_id: HashMap<i64, f64> =
        shots.iter().map(|s| (s.id, as_written(s.depth))).collect();
    for shot in shots {
        let Some(&parent_depth) = depth_by_id.get(&shot.from_id) else {
            continue;
        };
        let delta = (as_written(shot.depth) - parent_depth).abs();
        let length = as_written(shot.length);
        assert!(
            delta <= length,
            "shot {} writes Depth {} under a parent at {} with Length {}: the server \
             reads a {delta} m drop over a {length} m tape and fails the file",
            shot.id,
            format_f64(shot.depth),
            format_f64(parent_depth),
            format_f64(shot.length),
        );
    }
}

/// A survey whose only line, Main, descends 0 -> 4.1 -> 5.4 m, with a
/// closure tying the two stations that end at those depths. The closure's
/// length is their difference by construction, so it sits exactly on the
/// bound in f64 and crosses it once written: 5.4 - 4.1 is
/// 1.3000000000000007, and the tape that covers it goes into the file as
/// "1.3".
fn closure_on_the_bound_survey() -> (Survey, LineId) {
    let mut s = Survey::new("On The Bound");
    let main = s.named(
        "Main",
        "#ff0000",
        vec![
            ShotData {
                depth_in: 0.0,
                depth_out: 4.1,
                ..tape(5.0)
            },
            ShotData {
                depth_in: 4.1,
                depth_out: 5.4,
                ..tape(5.0)
            },
        ],
    );
    s.closure((main, 1), (main, 2), ClosureStrategy::Batch);
    (s, main)
}

fn closure_on_the_bound_project() -> Project {
    closure_on_the_bound_survey().0.build()
}

#[test]
fn a_tape_that_would_write_short_is_rounded_up_for_the_sidecar() {
    // The production failure. A 4.1 m -> 5.4 m drop is 1.3000000000000007 m
    // in f64, and the tape that exactly covers it writes as "1.3". The
    // server re-parses both, computes the same 1.3000000000000007, and
    // fails the whole file with "Shot is shorter than the vertical
    // variation" - so repairing with the raw delta, which is what the user
    // profile does and must keep doing, changes nothing.
    let on_the_bound = 5.4_f64 - 4.1;
    let chain = || {
        vec![
            geometry_shot(1, -1, 4.1, 4.1, 0.0, false),
            geometry_shot(2, 1, 4.1, 5.4, on_the_bound, false),
        ]
    };

    let mut user = chain();
    enforce_ariane_geometry_invariants(&mut user, TmlProfile::User);
    assert_eq!(
        format_f64(user[1].length),
        "1.3",
        "the user export stays byte-for-byte what it always was"
    );

    let mut sidecar = chain();
    enforce_ariane_geometry_invariants(&mut sidecar, TmlProfile::SpeleoDbSidecar);
    assert_eq!(
        format_f64(sidecar[1].length),
        "1.300001",
        "the sidecar tape has to still cover the drop after being written"
    );
    assert_written_chain_satisfies_speleodb(&sidecar);
}

#[test]
fn the_repaired_tape_never_writes_short() {
    // The ceil is on the f64 delta while the file carries six decimals, and
    // `{:.6}` rounds half to even where `round()` rounds half away from
    // zero. Whatever the pair does at a tie, the written tape must still
    // cover the written drop.
    for (parent, depth) in [
        (4.1_f64, 5.4_f64),
        (0.1, 0.3),
        (12.345_678, 19.87),
        (-3.3, 2.2),
        (0.0, 0.000_001),
        (99.999_999_5, 100.0),
    ] {
        let delta = (as_written(depth) - as_written(parent)).abs();
        let fixed = length_covering_written_delta(delta);
        assert!(
            as_written(fixed) >= delta,
            "a {delta} m drop repaired to {fixed} writes as {} and no longer covers it",
            format_f64(fixed)
        );
        // TIGHTNESS, which is the half the covering check above cannot see. That
        // one restates the postcondition the repair's own verify loop enforces, so
        // it holds for any implementation the loop can walk up to - including ones
        // that overshoot. This is the bound that says the repair absorbs the
        // rounding and does not invent survey data: a tape may grow into the last
        // written decimal and no further.
        //
        // Loop exhaustion needs no separate assertion - it returns a value that
        // still writes short, which is exactly what the covering check catches.
        let one_ulp = 1.0 / ARIANE_WRITTEN_DECIMALS;
        assert!(
            fixed - delta < 2.0 * one_ulp,
            "repairing a {delta} m drop to {fixed} lengthens the tape by more than \
             the rounding it exists to absorb"
        );
    }
}

#[test]
fn a_closure_sitting_on_the_bound_still_reads_back_for_the_sidecar() {
    // A closure's length IS the vertical part of its own error, so every one
    // of them sits exactly on the bound and only the rounding can push it
    // over. This is the whole-project path to the same defect as the unit
    // test above.
    let project = closure_on_the_bound_project();

    let user = user_rows(&project);
    let user_closure = user.iter().find(|r| r.shot_type == "CLOSURE").unwrap();
    assert_eq!(
        format_f64(user_closure.length),
        "1.3",
        "the user export is unchanged"
    );

    let sidecar = sidecar_rows(&project);
    let sidecar_closure = sidecar.iter().find(|r| r.shot_type == "CLOSURE").unwrap();
    assert_eq!(format_f64(sidecar_closure.length), "1.300001");
    assert_written_chain_satisfies_speleodb(&sidecar);
}

#[test]
fn each_entrance_carries_its_own_datum() {
    // #378 item (e): two detached galleries with distinct GPS anchors used to
    // both get the first datum's coordinates - plausibly wrong, so likelier
    // shipped unnoticed than the old 0,0 null-island form.
    let mut s = Survey::new("Two Anchors");
    let north = s.named("North", "#ff0000", vec![shot(5.0, 90.0)]);
    let south = s.named("South", "#ff0000", vec![shot(5.0, 90.0)]);
    s.datum((north, 0), gps(20.25, -87.5));
    s.datum((south, 0), gps(20.31, -87.42));
    let project = s.build();

    let rows = user_rows(&project);
    let start_of = |name: &str| {
        rows.iter()
            .find(|r| r.shot_type == "START" && r.line == name)
            .unwrap()
    };
    let north = start_of("North");
    assert!(
        (north.latitude - 20.25).abs() < f64::EPSILON
            && (north.longitude - -87.5).abs() < f64::EPSILON
    );
    let south = start_of("South");
    assert!(
        (south.latitude - 20.31).abs() < f64::EPSILON
            && (south.longitude - -87.42).abs() < f64::EPSILON,
        "the second entrance must carry its own datum, got {},{}",
        south.latitude,
        south.longitude
    );

    // Same resolution, sidecar profile: applied per entrance BEFORE the
    // speleodb_anchor transform, so both entrances stay distinct there too.
    let sidecar = sidecar_rows(&project);
    let starts: Vec<&ExportShot> = sidecar.iter().filter(|r| r.shot_type == "START").collect();
    assert_eq!(starts.len(), 2);
    assert!(
        (starts[0].latitude - starts[1].latitude).abs() > 1e-6
            || (starts[0].longitude - starts[1].longitude).abs() > 1e-6,
        "sidecar entrances collapsed onto one anchor"
    );
}

#[test]
fn a_detached_entrance_with_a_mid_line_datum_splits_there() {
    // #378 item (e), the geometric half: GPS lands only on a START, so an
    // entrance whose datum is mid-line must split at the datum station -
    // stamping that datum's GPS onto a station-0 START would displace the
    // whole component by the station-to-datum vector. The root does this;
    // detached entrances share the path.
    let mut s = Survey::new("Mid Datum");
    let main = s.named("Main", "#ff0000", vec![shot(5.0, 90.0)]);
    let gallery = s.named("Gallery", "#00ff00", shots(2, 5.0, 90.0));
    s.datum((main, 0), gps(20.25, -87.5));
    // The detached gallery's datum sits on its MIDDLE station (index 1).
    s.datum((gallery, 1), gps(20.31, -87.42));
    let rows = user_rows(&s.build());

    let gallery_start = rows
        .iter()
        .find(|r| r.shot_type == "START" && r.line == "Gallery")
        .unwrap();
    assert!(
        (gallery_start.latitude - 20.31).abs() < f64::EPSILON
            && (gallery_start.longitude - -87.42).abs() < f64::EPSILON,
        "the gallery START carries the gallery's own datum"
    );
    // The split exists: a reversed pre-datum leg means the line name shows
    // up in both its forms, and every shot still chains (no orphan roots).
    assert!(
        rows.iter().any(|r| r.line == "Gallery (pre-zero-datum)"),
        "mid-line datum must split the entrance so the START sits on it"
    );
    assert_no_orphan(&rows);
    assert!(ensure_rooted_chain(&rows, TmlProfile::User).is_ok());
}

#[test]
fn a_disconnected_component_roots_at_its_datum_line() {
    // #378 review follow-up: a detached gallery of three joined lines with
    // the datum on the MIDDLE one must make that line the component's
    // entrance - rooting at the first line in project order would anchor the
    // whole gallery at the frame datum's coordinates while its own datum went
    // unwritten.
    let mut s = Survey::new("Datum Root");
    let ids: Vec<LineId> = ["Main", "Gallery A", "Gallery B", "Gallery C"]
        .iter()
        .map(|name| s.named(name, "#ff0000", vec![shot(5.0, 90.0)]))
        .collect();
    // Chain the galleries (A-B-C) to each other, none to Main.
    s.merge((ids[2], 0), (ids[1], 1));
    s.merge((ids[3], 0), (ids[2], 1));
    s.datum((ids[0], 0), gps(20.25, -87.5));
    // The gallery's own anchor sits on its MIDDLE line's first station.
    s.datum((ids[2], 0), gps(20.31, -87.42));
    let rows = user_rows(&s.build());

    let starts: Vec<&ExportShot> = rows.iter().filter(|r| r.shot_type == "START").collect();
    assert_eq!(starts.len(), 2, "one entrance per component");
    let gallery_start = starts.iter().find(|r| r.line == "Gallery B");
    assert!(
        gallery_start.is_some(),
        "the datum-carrying line must be the component's entrance; STARTs: {:?}",
        starts.iter().map(|r| r.line.as_str()).collect::<Vec<_>>()
    );
    let gallery_start = gallery_start.unwrap();
    assert!(
        (gallery_start.latitude - 20.31).abs() < f64::EPSILON
            && (gallery_start.longitude - -87.42).abs() < f64::EPSILON,
        "the component's START carries its own datum, got {},{}",
        gallery_start.latitude,
        gallery_start.longitude
    );
    assert_no_orphan(&rows);
    assert!(ensure_rooted_chain(&rows, TmlProfile::User).is_ok());
}

#[test]
fn the_rooted_chain_error_caps_its_line_list() {
    // #378 item (g): line names come from imported files, so the refusal
    // message must not scale with their count or length, and must not carry
    // control characters a crafted name could forge log lines with.
    let long = "L".repeat(500);
    let names: Vec<&str> = vec!["A", "B", "C", "D", "E", "F", "G", &long];
    let listed = list_capped(&names);
    assert!(
        listed.ends_with("and 3 more"),
        "8 lines must summarise the tail: {listed}"
    );
    assert!(
        listed.len() < 120,
        "capped list stays bounded, got {} chars",
        listed.len()
    );

    let shown = list_capped(&[&long]);
    assert!(
        shown.len() <= LISTED_LINE_NAME_MAX + "...".len(),
        "a single long name is truncated: {} chars",
        shown.len()
    );
    assert!(shown.ends_with("..."));

    // Multibyte: the cap is in CHARS, so the byte budget is chars x 4 at
    // worst - bounded either way, and no char-boundary panic.
    let wide = "é".repeat(500);
    let shown = list_capped(&[&wide]);
    assert_eq!(shown.chars().count(), LISTED_LINE_NAME_MAX + "...".len());
    assert!(shown.len() <= LISTED_LINE_NAME_MAX * 4 + "...".len());

    // Control characters are stripped, not merely truncated around: the
    // newline and the ESC go; the (printable) SGR remnant stays inert.
    assert_eq!(
        list_capped(&["A\nERROR forged line\x1b[31m"]),
        "AERROR forged line[31m"
    );

    // The common case stays verbatim - no cap artifacts in a normal message.
    assert_eq!(list_capped(&["Main", "Side"]), "Main, Side");

    // And the cap is actually wired into the refusal the user sees: an
    // orphan REAL alongside a CLOSURE takes the bail path, whose message
    // must carry the summarised list, not every name.
    let mut shots: Vec<ExportShot> = Vec::new();
    shots.push(geometry_shot(0, -1, 0.0, 0.0, 0.0, false));
    for i in 0..8_i64 {
        let mut orphan = geometry_shot(100 + i, -1, 0.0, 0.0, 5.0, false);
        orphan.shot_type = "REAL";
        orphan.line = format!("Orphan {i}");
        shots.push(orphan);
    }
    let mut closure = geometry_shot(200, 0, 0.0, 0.0, 0.0, false);
    closure.shot_type = "CLOSURE";
    shots.push(closure);
    let refusal = ensure_rooted_chain(&shots, TmlProfile::User);
    assert!(refusal.is_err(), "orphans + closure must refuse the export");
    let error = refusal.unwrap_err().to_string();
    assert!(
        error.contains("and 3 more"),
        "the refusal must use the capped list: {error}"
    );
    assert!(
        !error.contains("Orphan 7"),
        "the tail must be summarised: {error}"
    );
}

#[test]
fn the_sidecar_accepts_multiple_starts() {
    // #378 item (c): entrances are not profile-gated, so a project with a
    // detached line ships a multi-START sidecar to SpeleoDB. The server's
    // converter supports that, PROVIDED each START is a usable anchor:
    // `is_geolocation_known()` there reads a coordinate within ~1e-6 of zero
    // as unknown, and a component with no anchor is dropped from the GeoJSON
    // wholesale.
    let mut s = Survey::new("Two Entrances");
    let main = s.named(
        "Main",
        "#ff0000",
        vec![ShotData {
            depth_out: 2.0,
            ..shot(5.0, 90.0)
        }],
    );
    s.named(
        "Detached",
        "#ff0000",
        vec![ShotData {
            depth_out: 2.0,
            ..shot(5.0, 90.0)
        }],
    );
    s.datum((main, 0), gps(20.25, -87.5));

    let sidecar = sidecar_rows(&s.build());
    let starts: Vec<&ExportShot> = sidecar.iter().filter(|r| r.shot_type == "START").collect();
    assert_eq!(starts.len(), 2, "the sidecar keeps one START per entrance");
    for start in &starts {
        assert!(
            start.latitude.abs() > 1e-6 && start.longitude.abs() > 1e-6,
            "START in {:?} is at {},{} - below openspeleo_lib's anchor \
             threshold, so its whole component would be dropped as orphans",
            start.line,
            start.latitude,
            start.longitude,
        );
    }
    assert_no_orphan(&sidecar);
    assert_written_chain_satisfies_speleodb(&sidecar);
}

#[test]
fn the_sidecar_never_emits_exactly_one_survey_data() {
    // A lone `<SurveyData>` parses as a dict in SpeleoDB's XML decoder, so
    // iterating it yields the field NAMES and the conversion dies with an
    // AttributeError. Zero elements it handles cleanly (EmptySurveyError),
    // and two or more is the normal path - so the fix is to never emit one.
    let mut s = Survey::new("Lone Start");
    s.named("Empty", "#ff0000", Vec::new());
    let project = s.build();

    let user = user_rows(&project);
    assert_eq!(user.len(), 1, "a shotless line exports its START alone");

    let sidecar = sidecar_rows(&project);
    assert_eq!(sidecar.len(), 2, "the sidecar pads the lone START");
    let pad = &sidecar[1];
    assert_eq!(pad.shot_type, "VIRTUAL", "VIRTUAL is dropped from the map");
    assert_eq!(pad.from_id, -1, "a root orphan the converter ignores");
    assert!(pad.uuid.is_none());
    assert!(pad.length.abs() < f64::EPSILON);
    assert!(
        (pad.depth - sidecar[0].depth).abs() < f64::EPSILON,
        "the pad sits on the START, so it cannot displace anything"
    );
    assert_ne!(pad.id, sidecar[0].id, "a duplicate ID fails the whole file");

    let file = NamedTempFile::with_suffix(".tmlu").unwrap();
    export_tml_for_speleodb(&project, file.path()).unwrap();
    let content = std::fs::read_to_string(file.path()).unwrap();
    assert_eq!(content.matches("<SRVD>").count(), 2);

    // Nothing else is padded: no line means no shots at all, which is
    // safe, and a project already emitting two or more is left alone.
    assert!(sidecar_rows(&Survey::new("Nothing").build()).is_empty());
    let simple = simple_project();
    assert_eq!(sidecar_rows(&simple).len(), user_rows(&simple).len());
}

#[test]
fn the_sidecar_emits_a_date_the_server_can_parse_and_model() {
    // A missing, empty or non-ISO `<Date>` fails validation for the ENTIRE
    // file, and a year outside IGRF's [1900, 2035] silently drops the
    // magnetic declination to 0.0, rotating the map.
    assert_eq!(speleodb_date(None), DEFAULT_EXPORT_DATE);
    assert_eq!(speleodb_date(Some("")), DEFAULT_EXPORT_DATE);
    assert_eq!(speleodb_date(Some("   ")), DEFAULT_EXPORT_DATE);
    assert_eq!(speleodb_date(Some("15/06/2025")), DEFAULT_EXPORT_DATE);
    assert_eq!(speleodb_date(Some("2025-06-15")), "2025-06-15");
    assert_eq!(speleodb_date(Some("1850-06-01")), "1900-01-01");
    assert_eq!(speleodb_date(Some("2999-01-01")), "2035-12-31");

    // Every emitted shot carries one, including the closure and the START,
    // which have no line date of their own.
    for shot in &sidecar_rows(&closure_on_the_bound_project()) {
        let date = shot.date.as_deref().unwrap_or_default();
        assert!(
            chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok(),
            "shot {} carries {date:?}, which the server cannot parse",
            shot.id
        );
    }
}

#[test]
fn the_sidecar_emits_only_uuids_the_server_will_accept() {
    // `<UUID>` is typed UUID4 there, so a v1 or a non-UUID fails the whole
    // file; a repeat aliases two shots onto one GeoJSON feature. The model
    // keeps shot UUIDs unique, but the gate does not rely on it.
    let v4 = "3f1e2d4c-5b6a-4789-8abc-def012345678";
    let other_v4 = "11111111-2222-4333-9444-555555555555";
    let v1 = "6f9619ff-8b86-1d01-b42d-00cf4fc964ff";
    let wrong_variant = "3f1e2d4c-5b6a-4789-cabc-def012345678";

    assert!(speleodb_uuid(v4).is_some());
    assert!(speleodb_uuid(v1).is_none(), "not a v4");
    assert!(speleodb_uuid(wrong_variant).is_none(), "not RFC 4122");
    assert!(speleodb_uuid("not-a-uuid").is_none());
    assert!(speleodb_uuid("").is_none());
    assert!(
        speleodb_uuid("00000000-0000-0000-0000-000000000000").is_none(),
        "the nil UUID is version 0"
    );

    let mut shots = vec![
        geometry_shot(1, -1, 0.0, 0.0, 0.0, false),
        geometry_shot(2, 1, 0.0, 1.0, 5.0, false),
        geometry_shot(3, 2, 1.0, 2.0, 5.0, false),
        geometry_shot(4, 3, 2.0, 3.0, 5.0, false),
        geometry_shot(5, 4, 3.0, 4.0, 5.0, false),
    ];
    shots[1].uuid = Some(v4.to_string());
    shots[2].uuid = Some(v4.to_uppercase());
    shots[3].uuid = Some(v1.to_string());
    shots[4].uuid = Some(other_v4.to_string());

    harden_for_speleodb(&mut shots);

    assert_eq!(shots[1].uuid.as_deref(), Some(v4), "a valid v4 is kept");
    assert!(
        shots[2].uuid.is_none(),
        "the same UUID in another case is still the same feature id"
    );
    assert!(shots[3].uuid.is_none(), "a v1 fails the whole file");
    assert_eq!(shots[4].uuid.as_deref(), Some(other_v4));
}

/// The gate above decides WHETHER to emit; this is about WHAT gets emitted.
///
/// `Uuid::parse_str` is permissive - `{braced}`, `urn:uuid:` and hyphenless all
/// parse, and all of them are genuine v4s that sail through `speleodb_uuid`. Wire
/// the accepted value straight through and the file carries a spelling the
/// server's `UUID4` field will not match, failing the whole document on a value
/// the check had just approved. That is the same class of silent, whole-file
/// rejection the rest of this hardening exists to remove, so the canonical
/// `8-4-4-4-12` form is what goes in.
#[test]
fn a_uuid_the_gate_accepts_is_written_in_the_form_the_server_expects() {
    let canonical = "3f1e2d4c-5b6a-4789-8abc-def012345678";
    for spelling in [
        format!("{{{canonical}}}"),
        format!("urn:uuid:{canonical}"),
        canonical.replace('-', ""),
        canonical.to_uppercase(),
    ] {
        assert!(
            speleodb_uuid(&spelling).is_some(),
            "{spelling} is a real v4 and the gate accepts it"
        );

        let mut shots = vec![
            geometry_shot(1, -1, 0.0, 0.0, 0.0, false),
            geometry_shot(2, 1, 0.0, 1.0, 5.0, false),
        ];
        shots[1].uuid = Some(spelling.clone());

        harden_for_speleodb(&mut shots);

        assert_eq!(
            shots[1].uuid.as_deref(),
            Some(canonical),
            "{spelling} must be written as the canonical hyphenated form"
        );
    }
}

#[test]
fn an_anchor_on_the_equator_is_nudged_off_zero_for_the_sidecar() {
    // SpeleoDB anchors the map only when BOTH components exceed 1e-7, so a
    // survey on the equator or the prime meridian silently gets no map at
    // all. The nudge is a micro-degree, roughly 11 cm: a tenth of that is
    // below `format_f64`'s six decimals and would go into the file as "0.0".
    let (lat, lon) = speleodb_anchor(0.0, -87.5);
    assert_eq!(format_f64(lat), "0.000001");
    assert_eq!(format_f64(lon), "-87.5", "a real component is left alone");

    let (lat, lon) = speleodb_anchor(-0.0, 0.0);
    assert_eq!(format_f64(lat), "0.000001", "negative zero is still zero");
    assert_eq!(format_f64(lon), "0.000001");

    // Below the six decimals the sign is all that survives, and it is kept.
    assert_eq!(format_f64(speleodb_anchor(-1e-9, 1.0).0), "-0.000001");

    // Out of range parses fine there and explodes inside the projection.
    let (lat, lon) = speleodb_anchor(95.0, 200.0);
    assert_eq!(format_f64(lat), "90.0");
    assert_eq!(format_f64(lon), "180.0");

    // No datum means no anchor, in both profiles: inventing one would put
    // the survey somewhere it has never been.
    let no_datum = sidecar_rows(&simple_project());
    let start = no_datum.iter().find(|r| r.shot_type == "START").unwrap();
    assert!(start.latitude.abs() < f64::EPSILON);
    assert!(start.longitude.abs() < f64::EPSILON);
}

/// Review of #719, item 2: a control character typed or imported into a
/// name never reaches the exported document (one makes it invalid XML):
/// the shot stores an imported credit cleaned, so the export carries no C0
/// byte but tab, newline and carriage return, and reads back.
#[test]
fn a_control_character_in_a_name_never_reaches_the_export() {
    let raw = |name: &str| Person {
        name: name.to_string(),
        email: None,
    };
    let mut s = Survey::new("Controls");
    s.line(
        crate::models::survey::LineMeta {
            name: Some("Main".to_string()),
            ..Default::default()
        },
        vec![ShotData {
            credit: Some(
                SurveyCredit {
                    surveyors: vec![raw("Bad\u{1}Name")],
                    explorers: vec![raw("Bell\u{7}\u{1b}Ringer")],
                    date: None,
                }
                .into(),
            ),
            ..tape(5.0)
        }],
    );
    let file = NamedTempFile::with_suffix(".tmlu").unwrap();
    export_tml(&s.build(), file.path()).unwrap();
    let bytes = std::fs::read(file.path()).unwrap();
    assert!(
        !bytes
            .iter()
            .any(|&b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r')),
        "a control byte reached the export"
    );
    let parsed = crate::parsers::tml::parse_tml(file.path()).unwrap();
    let (configs, _, _) =
        crate::parsers::tml::convert_tml_to_import_configs(&parsed, false, false).unwrap();
    let credit = configs[0].shots[0].credit();
    assert_eq!(credit.surveyors[0].name, "BadName");
    assert_eq!(credit.explorers[0].name, "BellRinger");
}

#[test]
fn the_sidecar_strips_the_line_identity_separator() {
    // SpeleoDB groups shots into lines by hashing
    // name/description/date/explorers/surveyors joined with U+220E, so the
    // character inside one of those fields re-cuts the survey.
    let mut project = simple_project();
    let main = project.lines()[0].id;
    let line = project.line_mut(main).unwrap();
    line.name = Some("Main\u{220e}Line".to_string());
    for shot in &mut line.shots {
        shot.credit = stored(SurveyCredit {
            explorers: vec![Person::named("Alice\u{220e}Bob").unwrap()],
            ..Default::default()
        });
    }

    let user = user_rows(&project);
    assert_eq!(user[0].line, "Main\u{220e}Line");
    assert_eq!(
        user[0].explorer.as_deref(),
        Some("<Explorer>Alice\u{220e}Bob</Explorer><Surveyor></Surveyor>")
    );

    let sidecar = sidecar_rows(&project);
    assert_eq!(sidecar[0].line, "Main Line");
    assert_eq!(
        sidecar[0].explorer.as_deref(),
        Some("<Explorer>Alice Bob</Explorer><Surveyor></Surveyor>")
    );
}

#[test]
fn the_sidecar_normalizes_azimuth_for_every_shot_type() {
    // The server normalises REAL shots and passes every other type through
    // raw, so a VIRTUAL leg at -10 degrees walks off in a direction nothing
    // was surveyed in.
    let mut project = simple_project();
    let main = project.lines()[0].id;
    let line = project.line_mut(main).unwrap();
    line.shots[0].azimuth_in = -10.0;
    line.shots[0].kind = ShotKind::Jump;
    line.shots[1].azimuth_in = 400.0;

    let user = user_rows(&project);
    assert!((user[1].azimuth + 10.0).abs() < f64::EPSILON);
    assert!((user[2].azimuth - 400.0).abs() < f64::EPSILON);

    let sidecar = sidecar_rows(&project);
    assert_eq!(sidecar[1].shot_type, "VIRTUAL");
    assert!((sidecar[1].azimuth - 350.0).abs() < f64::EPSILON);
    assert!((sidecar[2].azimuth - 40.0).abs() < f64::EPSILON);
}

#[test]
fn exported_ariane_ids_are_unique_across_the_whole_document() {
    // A duplicate `<ID>` anywhere - across lines, split blocks, closures,
    // the synthesized START - fails the whole file with a DuplicateValueError.
    // Station rows take their Station IDs, which are unique by construction,
    // and CLOSURE rows and fix STARTs are numbered above every station; this
    // pins it on the topologies that mint the most of them (a datum split, a
    // mid-line junction, a closure).
    let mut s = Survey::new("Ids");
    let root = s.named(
        "Root",
        "#ff0000",
        (0..4)
            .map(|i| ShotData {
                depth_in: f64::from(i),
                depth_out: f64::from(i) + 1.0,
                ..tape(5.0)
            })
            .collect(),
    );
    let child = s.named("Child", "#00ff00", vec![tape(2.0), tape(2.0), tape(2.0)]);
    s.merge((child, 1), (root, 2));
    s.closure((root, 1), (child, 3), ClosureStrategy::Batch);
    // A datum away from station 0 splits the root into two blocks.
    s.datum((root, 2), gps(20.25, -87.5));
    let project = s.build();

    for profile in [TmlProfile::User, TmlProfile::SpeleoDbSidecar] {
        let (rows, _) = export_rows(&project, profile);
        assert!(rows.len() > 8, "the topology should mint plenty of ids");
        let unique: HashSet<i64> = rows.iter().map(|r| r.id).collect();
        assert_eq!(
            unique.len(),
            rows.len(),
            "{profile:?} emitted a duplicate ID"
        );
    }
}

#[test]
fn the_user_export_keeps_every_value_the_sidecar_repairs() {
    // The sidecar's concessions are lies about the survey, small enough for
    // a map preview and not for the file a user asked for. One project that
    // trips four of them at once, exported both ways.
    let (mut s, main) = closure_on_the_bound_survey();
    s.datum((main, 0), gps(20.25, 0.0));
    let mut project = s.build();
    let line = project.line_mut(main).unwrap();
    line.name = Some("Main\u{220e}Line".to_string());
    line.shots[0].uuid = "not-a-uuid".to_string();

    let dir = tempfile::TempDir::new().unwrap();
    let user_path = dir.path().join("user.tmlu");
    let sidecar_path = dir.path().join("sidecar.tmlu");
    export_tml(&project, &user_path).unwrap();
    export_tml_for_speleodb(&project, &sidecar_path).unwrap();
    let user = std::fs::read_to_string(&user_path).unwrap();
    let sidecar = std::fs::read_to_string(&sidecar_path).unwrap();

    assert!(user.contains("<UU>not-a-uuid</UU>"), "verbatim, as always");
    assert!(user.contains("Main\u{220e}Line"));
    assert!(user.contains("<LGT>0.0</LGT>"));
    assert!(
        !user.contains("<LG>1.300001</LG>"),
        "the closure tape is untouched in the user export"
    );

    assert!(!sidecar.contains("not-a-uuid"));
    assert!(!sidecar.contains('\u{220e}'));
    assert!(sidecar.contains("<LGT>0.000001</LGT>"));
    assert!(sidecar.contains("<LG>1.300001</LG>"));

    // And the sidecar is still deterministic, which the push relies on to
    // stay a 304 no-op when nothing changed.
    let again = dir.path().join("again.tmlu");
    export_tml_for_speleodb(&project, &again).unwrap();
    assert_eq!(sidecar, std::fs::read_to_string(&again).unwrap());
}

// ---- Reversed legs keep their shot's identity (export-roundtrip-fuzz X6) ----

/// A line split by a zero datum: the shots before the datum are re-emitted
/// as the reversed `(pre-zero-datum)` leg.
fn project_split_by_a_zero_datum() -> Project {
    let mut s = Survey::new("Split");
    let main = s.named(
        "Main",
        "#ff0000",
        (0..4)
            .map(|i| shot(3.0 + f64::from(i), 10.0 * f64::from(i)))
            .collect(),
    );
    s.datum((main, 2), GpsCoordinates::new(20.25, -87.5, 12.0).unwrap());
    s.build()
}

/// A child joined MID-line: a forward block plus a reversed block.
fn project_with_a_mid_line_merge() -> Project {
    let mut s = Survey::new("Mid Merge");
    let root = s.named("Root", "#ff0000", shots(2, 5.0, 90.0));
    let branch = s.named(
        "Branch",
        "#00ff00",
        (0..4)
            .map(|i| shot(3.0 + f64::from(i), 20.0 * f64::from(i)))
            .collect(),
    );
    // Child station 2 is in the MIDDLE of its four shots, so the exporter
    // emits shots 2..4 forward and shots 0..2 reversed.
    s.merge((branch, 2), (root, 2));
    s.build()
}

/// A child joined at its LAST station: the whole line is reversed and there
/// is no forward block at all.
fn project_with_an_end_merge() -> Project {
    let mut s = Survey::new("End Merge");
    let root = s.named("Root", "#ff0000", shots(2, 5.0, 90.0));
    let tail = s.named(
        "Tail",
        "#00ff00",
        (0..3)
            .map(|i| shot(3.0 + f64::from(i), 10.0 * f64::from(i)))
            .collect(),
    );
    s.merge((tail, 3), (root, 2));
    s.build()
}

fn project_uuids(project: &Project) -> HashSet<String> {
    project
        .lines()
        .iter()
        .flat_map(|l| l.shots.iter().map(|s| s.uuid.clone()))
        .collect()
}

/// How many PROJECT shots the export re-emitted into a reversed block,
/// counted by the line label only the reversal writes and the shot types
/// only a real project shot gets. Deliberately blind to the UUID, so it
/// stays a statement about the SHAPE of the export and cannot pass or fail
/// for the reason the tests below are about.
fn reversed_leg_shot_count(rows: &[ExportShot]) -> usize {
    rows.iter()
        .filter(|r| {
            (r.line.ends_with("(pre-zero-datum)") || r.line.ends_with("(reversed)"))
                && (r.shot_type == "REAL" || r.shot_type == "VIRTUAL")
        })
        .count()
}

/// The three shapes that make the exporter emit a reversed leg, each with
/// the number of project shots that leg has to carry.
///
/// The count is asserted HERE, once, for every consumer below, because
/// these tests are worth nothing the moment a fixture stops reaching the
/// reversal at all: a project whose shots are all emitted forward satisfies
/// every assertion trivially. WHICH shots get reversed is a function of the
/// export root, and the root is chosen from the frame datum and the station
/// graph - both of which have moved before and can move again without
/// touching a line of this module.
fn reversal_shapes() -> Vec<(&'static str, Project)> {
    let shapes = vec![
        // 4 shots split at station 2: shots 0 and 1 come back reversed.
        ("zero-datum split", project_split_by_a_zero_datum(), 2),
        // The child's 4 shots join at its station 2: shots 0 and 1 reversed.
        ("mid-line merge", project_with_a_mid_line_merge(), 2),
        // The child joins at its LAST station: all 3 shots reversed, and
        // there is no forward block at all.
        ("end merge", project_with_an_end_merge(), 3),
    ];
    shapes
        .into_iter()
        .map(|(label, project, reversed)| {
            assert_eq!(
                reversed_leg_shot_count(&user_rows(&project)),
                reversed,
                "{label}: fixture rot - the export no longer re-emits these shots \
                 as a reversed leg, so every assertion about one is vacuous"
            );
            (label, project)
        })
        .collect()
}

/// Every REAL project shot reaches the file with its own UUID, whichever
/// block it lands in.
///
/// The reversed legs were once emitted with `uuid: None`, so the reader had
/// nothing to preserve and minted a fresh one - 78 of a real 532-shot
/// survey's shots were re-identified by a round trip through a file this app
/// had just written. Found by `export-roundtrip-fuzz`'s X6.
#[test]
fn every_real_shot_reaches_the_export_with_its_own_uuid() {
    for (label, project) in reversal_shapes() {
        let expected = project_uuids(&project);
        assert!(!expected.is_empty(), "{label}: fixture has no uuids");

        let rows = user_rows(&project);
        let emitted: HashSet<String> = rows.iter().filter_map(|r| r.uuid.clone()).collect();
        assert_eq!(emitted, expected, "{label}: emitted uuids != project uuids");

        // The synthesized rows - the ones that stand for no project shot -
        // still carry none.
        for r in &rows {
            if r.shot_type == "START" || r.shot_type == "CLOSURE" {
                assert!(
                    r.uuid.is_none(),
                    "{label}: a synthesized {} row carries a uuid",
                    r.shot_type
                );
            }
        }
    }
}

/// A reversed leg is its shot's ONLY appearance, so nothing is re-minted.
///
/// If a shot were ever emitted twice, `remint_duplicate_uuids` would read a
/// forward/reverse pair as a document duplicate and re-mint one of them -
/// silently re-identifying exactly the shots the X6 fix was made for.
#[test]
fn a_reversed_leg_is_a_shots_only_appearance_so_nothing_is_reminted() {
    for (label, project) in reversal_shapes() {
        let rows = user_rows(&project);
        let mut seen: HashMap<String, usize> = HashMap::new();
        for r in &rows {
            if let Some(uuid) = &r.uuid {
                *seen.entry(uuid.clone()).or_default() += 1;
            }
        }
        let repeats: Vec<&String> = seen
            .iter()
            .filter(|(_, count)| **count > 1)
            .map(|(uuid, _)| uuid)
            .collect();
        assert!(
            repeats.is_empty(),
            "{label}: {} uuid(s) appear more than once in the document: {repeats:?}",
            repeats.len()
        );
        // The rows have already been through `remint_duplicate_uuids` for the
        // User profile, so this equality is what proves it did not fire: a
        // re-mint would put a uuid in the document that the project does not
        // have.
        let emitted: HashSet<String> = rows.iter().filter_map(|r| r.uuid.clone()).collect();
        assert_eq!(
            emitted,
            project_uuids(&project),
            "{label}: a uuid was re-minted"
        );
    }
}

/// The identity survives the whole trip: write the file, read it back with
/// the app's own parser, and the reversed leg's shots still answer to the
/// uuids they left with.
#[test]
fn a_reversed_legs_uuid_survives_an_export_and_reparse() {
    for (label, project) in reversal_shapes() {
        let expected = project_uuids(&project);

        let file = NamedTempFile::with_suffix(".tmlu").unwrap();
        export_tml(&project, file.path()).unwrap();
        let parsed = crate::parsers::tml::parse_tml(file.path()).unwrap();

        let read_back: HashSet<String> = parsed
            .lines
            .iter()
            .flat_map(|l| l.shots.iter().filter_map(|sh| sh.uuid.clone()))
            .collect();
        assert_eq!(
            read_back, expected,
            "{label}: a shot's identity did not survive export + parse_tml"
        );
    }
}

/// The SpeleoDB sidecar is unaffected: it still renders, and still renders
/// the same bytes twice, which is what keeps a re-push a 304 no-op.
#[test]
fn reversed_leg_uuids_leave_the_speleodb_sidecar_deterministic() {
    for (label, project) in reversal_shapes() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("a.tmlu");
        let second = dir.path().join("b.tmlu");
        export_tml_for_speleodb(&project, &first).unwrap();
        export_tml_for_speleodb(&project, &second).unwrap();
        assert_eq!(
            std::fs::read(&first).unwrap(),
            std::fs::read(&second).unwrap(),
            "{label}: the sidecar render is no longer deterministic"
        );
    }
}
