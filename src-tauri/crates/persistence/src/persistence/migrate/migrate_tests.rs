//! The migration rules one at a time, on small hand-built legacy projects
//! (built through the frozen 0.6.0 API, written by the 0.6.0 save, opened by
//! the frozen reader), plus every builder of the legacy crate's own tests.

use std::collections::BTreeMap;

use chrono::DateTime;
use stalagmight_legacy::models::{
    self as m, ClosureStrategy as OldStrategy, GpsCoordinates, LineData, LineId, ShotData,
    StationMerge, StationRef,
};
use stalagmight_legacy::persistence::ResolvedWarning;
use stalagmight_legacy::{
    open_bytes, write_v18, write_v18_with, LegacyExtras, LegacySurvey, OpenOptions, WriteOptions,
};

use crate::models::survey::{ClosureStrategy, Project};

use super::test_support::{check, edit_sqlite};
use super::{migrate, migrate_bytes, AdjustedCheck, Migrated};

/// The legacy crate's shared builders, written against the alias `m`
/// (`super::m` from inside, the import above).
#[path = "../../../../legacy/tests/support/builder.rs"]
mod builder;

fn shot(azimuth: f64, length: f64, depth_in: f64, depth_out: f64, uuid: &str) -> ShotData {
    ShotData {
        uuid: Some(uuid.to_string()),
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in,
        depth_out,
        ..ShotData::default()
    }
}

/// A line of `legs` (azimuth, length), depths stepping by 0.5 m.
fn line(p: &mut m::Project, name: &str, legs: &[(f64, f64)]) -> LineId {
    let n = p.lines.len();
    let shots = legs
        .iter()
        .enumerate()
        .map(|(i, &(az, len))| {
            let d = i as f64 * 0.5;
            shot(az, len, d, d + 0.5, &format!("{n:04}-{i:04}-uuid"))
        })
        .collect();
    p.add_line(LineData {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#336699".to_string(),
        direction: None,
        quality: 1.0,
        shots,
    })
    .id
}

fn at(line: LineId, index: u32) -> StationRef {
    StationRef::new(line, index)
}

fn project() -> m::Project {
    let mut p = m::Project::new("Rules".to_string(), None);
    p.created_at = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    p.modified_at = p.created_at;
    p
}

fn open(p: &m::Project, extras: &LegacyExtras) -> LegacySurvey {
    open_bytes(&write_v18(p, extras).unwrap(), &OpenOptions::default()).unwrap()
}

/// Write, open, migrate, and run the full property check.
fn migrated(p: &m::Project) -> (LegacySurvey, Migrated) {
    let legacy = open(p, &LegacyExtras::default());
    let migrated = migrate(&legacy).unwrap();
    if let Err(e) = check(&legacy, &migrated) {
        panic!("{e}\n{}", migrated.report.summary());
    }
    (legacy, migrated)
}

fn station(p: &Project, line: LineId, index: u32) -> u32 {
    p.station_at(crate::models::StationRef::new(line, index))
        .unwrap()
}

fn raw_merge(p: &mut m::Project, a: StationRef, b: StationRef) {
    let station_a = p.key_for_ref(a).unwrap();
    let station_b = p.key_for_ref(b).unwrap();
    let merged_at = p.created_at;
    p.station_merges.push(StationMerge {
        station_a,
        station_b,
        merged_at,
    });
}

#[test]
fn every_legacy_builder_migrates_with_every_station_in_place() {
    for (name, project) in builder::all() {
        let legacy = open(&project, &LegacyExtras::default());
        let migrated = migrate(&legacy).unwrap();
        if let Err(e) = check(&legacy, &migrated) {
            panic!("{name}: {e}\n{}", migrated.report.summary());
        }
    }
}

#[test]
fn a_merge_is_one_station_named_by_the_label_representative() {
    let mut p = project();
    let main = line(&mut p, "Main", &[(90.0, 10.0), (80.0, 10.0), (70.0, 10.0)]);
    let side = line(&mut p, "Side", &[(180.0, 5.0), (190.0, 5.0)]);
    p.set_station_name(at(main, 2), "Junction").unwrap();
    p.merge_stations(at(side, 0), at(main, 2)).unwrap();
    let key = p.key_for_ref(at(main, 2)).unwrap();
    p.station_comments.insert(key, "sump".to_string());
    let key = p.key_for_ref(at(side, 0)).unwrap();
    p.station_comments.insert(key, "low air".to_string());
    let (legacy, out) = migrated(&p);

    let j = station(&out.project, main, 2);
    assert_eq!(j, station(&out.project, side, 0));
    let s = &out.project.stations[&j];
    // Main:2 is neither its line's first nor last station; Side:0 is first.
    let ids = &legacy.project.station_ids;
    let id_of = |sr| ids[&legacy.project.key_for_ref(sr).unwrap()];
    assert_eq!(s.id, id_of(at(main, 2)));
    assert_eq!(s.name, "Junction");
    assert_eq!(s.aliases.len(), 1);
    assert_eq!(s.aliases[0].id, id_of(at(side, 0)));
    assert_eq!(s.aliases[0].name.as_deref(), Some("0"));
    assert_eq!(s.comment.as_deref(), Some("sump\n\nlow air"));
    assert_eq!(out.report.merges_collapsed, 1);
    assert_eq!(out.report.aliases_created, 1);
    assert_eq!(out.report.comments_joined, 1);
}

#[test]
fn a_junction_name_taken_on_another_line_gets_a_letter() {
    let mut p = project();
    let main = line(&mut p, "Main", &[(90.0, 10.0), (80.0, 10.0), (70.0, 10.0)]);
    let side = line(&mut p, "Side", &[(180.0, 5.0), (190.0, 5.0), (200.0, 5.0)]);
    // Main:2 is named "2" and Side:2 too: once Side:0 joins Main:2, the
    // junction's name "2" would repeat on Side.
    p.merge_stations(at(side, 0), at(main, 2)).unwrap();
    let (_, out) = migrated(&p);
    let j = station(&out.project, main, 2);
    assert_eq!(out.project.stations[&j].name, "2a");
    assert_eq!(out.report.stations_renamed, 1);
}

#[test]
fn a_merge_closing_a_cycle_becomes_a_tie_at_the_front() {
    let mut p = project();
    let a = line(&mut p, "A", &[(90.0, 10.0), (0.0, 10.0), (270.0, 10.0)]);
    let b = line(&mut p, "B", &[(0.0, 10.0), (90.0, 3.0)]);
    p.create_loop_closure(at(a, 0), at(a, 3), OldStrategy::Batch)
        .unwrap();
    p.merge_stations(at(b, 0), at(a, 1)).unwrap();
    // A second merge between the same two lines: a cycle the old cascade
    // leaves torn (B's end is not where A:2 is).
    raw_merge(&mut p, at(b, 2), at(a, 2));
    let (_, out) = migrated(&p);
    assert_eq!(out.report.merges_collapsed, 1);
    assert_eq!(out.report.merges_demoted_cycle, 1);
    let first = out.project.closures[0];
    assert_eq!(first.strategy, ClosureStrategy::Tie);
    let ends = [station(&out.project, b, 2), station(&out.project, a, 2)];
    assert_eq!([first.a, first.b], ends);
    assert_eq!(first.a_via, Some(b));
    assert_eq!(first.b_via, Some(a));
    // The user's closure keeps its id, after the tie.
    assert_eq!(out.project.closures[1].strategy, ClosureStrategy::Batch);
}

#[test]
fn the_merge_the_old_cascade_left_torn_is_the_one_demoted() {
    // Three lines in a merge cycle, the merges listed C~B, B~A, C~A. The
    // 0.6.0 cascade starts at A (the lowest line) and walks A's merges: it
    // aligns B through B~A and C through C~A, and leaves C~B torn, although
    // C~B is first in the list. Taking the merges in list order would keep
    // C~B and move C by the loop's misclosure.
    let mut p = project();
    let a = line(&mut p, "A", &[(90.0, 10.0), (90.0, 10.0)]);
    let b = line(&mut p, "B", &[(0.0, 10.0)]);
    let c = line(&mut p, "C", &[(0.0, 10.0), (90.0, 7.5)]);
    raw_merge(&mut p, at(c, 2), at(b, 1));
    raw_merge(&mut p, at(b, 0), at(a, 1));
    raw_merge(&mut p, at(c, 0), at(a, 2));
    let (_, out) = migrated(&p);
    assert_eq!(out.report.merges_collapsed, 2);
    assert_eq!(out.report.merges_demoted_cycle, 1);
    let tie = out.project.closures[0];
    assert_eq!(tie.strategy, ClosureStrategy::Tie);
    assert_eq!(
        [tie.a, tie.b],
        [station(&out.project, c, 2), station(&out.project, b, 1)]
    );
    assert_eq!(station(&out.project, b, 0), station(&out.project, a, 1));
    assert_eq!(station(&out.project, c, 0), station(&out.project, a, 2));
}

#[test]
fn same_line_self_and_repeated_merges() {
    let mut p = project();
    let a = line(&mut p, "A", &[(90.0, 10.0), (0.0, 10.0), (270.0, 10.0), (180.0, 4.0)]);
    let b = line(&mut p, "B", &[(45.0, 3.0)]);
    p.merge_stations(at(b, 0), at(a, 2)).unwrap();
    raw_merge(&mut p, at(a, 0), at(a, 3));
    raw_merge(&mut p, at(a, 1), at(a, 1));
    raw_merge(&mut p, at(b, 0), at(a, 2));
    let (_, out) = migrated(&p);
    let r = &out.report;
    assert_eq!(r.merges_collapsed, 1);
    assert_eq!(r.merges_demoted_same_line, 1);
    assert_eq!(r.merges_dropped_self, 1);
    assert_eq!(r.merges_dropped_redundant, 1);
    // The same-line tie keeps the two stations' depths averaged (Z checked
    // to 1e-9 m by `check`).
    let tie = out.project.closures[0];
    assert_eq!(tie.strategy, ClosureStrategy::Tie);
    assert_eq!(tie.a_via, tie.b_via);
}

#[test]
fn an_aligned_non_move_closure_becomes_a_tie_and_a_snap_stays_a_snap() {
    let mut p = project();
    let a = line(&mut p, "A", &[(90.0, 10.0), (0.0, 10.0), (270.0, 10.0)]);
    let c = p.create_loop_closure(at(a, 0), at(a, 3), OldStrategy::Batch).unwrap();
    // A destructive adjust stamped it aligned; 0.6.0 trusts the stamp.
    if let Some(stored) = p.loop_closures.iter_mut().find(|x| x.id == c.id) {
        stored.is_aligned = true;
    }
    // A resurvey of A's first two legs, snapped onto A:2.
    let bypass = line(&mut p, "Bypass", &[(90.0, 10.0), (0.0, 10.0)]);
    p.merge_stations(at(bypass, 0), at(a, 0)).unwrap();
    p.create_loop_closure(at(bypass, 2), at(a, 2), OldStrategy::Move)
        .unwrap();
    let (legacy, out) = migrated(&p);
    assert_eq!(out.report.aligned_closures_to_tie, 1);
    let strategies: BTreeMap<u64, ClosureStrategy> = out
        .project
        .closures
        .iter()
        .map(|x| (x.id, x.strategy))
        .collect();
    assert_eq!(strategies[&c.id], ClosureStrategy::Tie);
    let snap = legacy
        .project
        .loop_closures
        .iter()
        .find(|x| x.strategy == OldStrategy::Move)
        .unwrap();
    assert!(snap.is_aligned);
    assert_eq!(strategies[&snap.id], ClosureStrategy::Move);
    let positions = out.project.raw_positions();
    let migrated_snap = out.project.closures.iter().find(|x| x.id == snap.id).unwrap();
    assert!(out.project.closure_is_aligned(&positions, migrated_snap));
}

#[test]
fn datum_pieces_hold_their_legacy_positions_off_gps_and_off_origin() {
    let mut p = project();
    let a = line(&mut p, "A", &[(90.0, 10.0), (0.0, 10.0)]);
    let b = line(&mut p, "B", &[(180.0, 10.0)]);
    let free = line(&mut p, "Free", &[(30.0, 5.0)]);
    p.set_zero_datum(at(a, 0), GpsCoordinates::new(20.5, -87.3, 0.0).unwrap())
        .unwrap();
    p.set_zero_datum(at(b, 1), GpsCoordinates::new(20.501, -87.299, 0.0).unwrap())
        .unwrap();
    // The frame piece off the origin and B's piece off its GPS: an older
    // build could leave both, and the 0.6.0 load keeps them where they are.
    p.adjust_rigid_group_offsets(a, 12.5, -3.25);
    p.adjust_rigid_group_offsets(b, -40.0, 7.5);
    p.adjust_rigid_group_offsets(free, 100.0, 50.0);
    let (legacy, out) = migrated(&p);
    assert_eq!(out.report.corrections_stored, 2);
    let frame = out.project.datums[0].correction.unwrap();
    let held = legacy.raw_positions[&stalagmight_legacy::Slot {
        line_id: a,
        station_index: 0,
    }];
    assert!((frame[0] - held[0]).abs() < 1e-12 && (frame[1] - held[1]).abs() < 1e-12);
    assert!(out.project.datums[1].correction.is_some());
    assert_eq!(out.project.placements.len(), 1);
    assert_eq!(out.report.placements_default_gauge, 1);
    let placement = out.project.placements[0];
    assert_eq!(placement.station, station(&out.project, free, 0));
    assert_eq!((placement.x, placement.y), (100.0, 50.0));
}

#[test]
fn hand_moves_stay_on_their_own_line_even_when_merged_members_differ() {
    let mut p = project();
    let a = line(&mut p, "A", &[(90.0, 10.0), (90.0, 10.0)]);
    let b = line(&mut p, "B", &[(0.0, 5.0)]);
    for (sr, offset) in [(at(a, 1), (1.0, 0.0, 0.5)), (at(b, 0), (0.0, 2.0, -1.0))] {
        let key = p.key_for_ref(sr).unwrap();
        p.station_offsets.insert(key, offset);
    }
    p.merge_stations(at(b, 0), at(a, 1)).unwrap();
    let (_, out) = migrated(&p);
    let j = station(&out.project, a, 1);
    assert_eq!(out.project.line(a).unwrap().hand(j), [1.0, 0.0, 0.5]);
    assert_eq!(out.project.line(b).unwrap().hand(j), [0.0, 2.0, -1.0]);
    assert_eq!(out.report.hand_moves_carried, 2);
}

#[test]
fn an_orphan_closure_promoted_by_the_load_is_one_station() {
    let mut p = project();
    let a = line(&mut p, "A", &[(90.0, 10.0)]);
    let b = line(&mut p, "B", &[(0.0, 10.0)]);
    p.adjust_rigid_group_offsets(b, 3.0, 4.0);
    let (ka, kb) = (p.key_for_ref(at(a, 1)).unwrap(), p.key_for_ref(at(b, 0)).unwrap());
    p.loop_closures.push(m::LoopClosure {
        id: 0,
        station_a: ka,
        station_b: kb,
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: OldStrategy::Batch,
        is_aligned: false,
    });
    let (_, out) = migrated(&p);
    assert_eq!(out.report.legacy_merges_created, 1);
    assert_eq!(station(&out.project, a, 1), station(&out.project, b, 0));
    assert!(out.project.closures.is_empty());
}

#[test]
fn shot_uuids_and_ignores_follow_every_legacy_rule() {
    let mut p = project();
    let a = line(&mut p, "A", &[(90.0, 10.0), (0.0, 10.0)]);
    let b = line(&mut p, "B", &[(0.0, 5.0), (10.0, 6.0)]);
    let long = "x".repeat(200);
    p.lines[1].shots[1].uuid = Some(long.clone());
    let shot = |p: &m::Project, line: usize, i: usize| p.lines[line].shots[i].clone();
    let ignore = |s: &m::Shot, line: LineId, code: &str, uuid: Option<String>| ResolvedWarning {
        line_id: line,
        shot_id: s.id,
        warning_code: code.to_string(),
        resolved_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        resolved_comment: None,
        shot_uuid: uuid,
    };
    let (a0, b0, b1) = (shot(&p, 0, 0), shot(&p, 1, 0), shot(&p, 1, 1));
    let extras = LegacyExtras {
        resolved_warnings: vec![
            ignore(&a0, a, "SHORT_SHOT", a0.uuid.clone()),
            // Stored under line B but naming A's shot (a split or combine
            // moved it): it lands on A's shot, which the first row already
            // ignores, so it is dropped as a repeat.
            ignore(&b0, b, "SHORT_SHOT", a0.uuid.clone()),
            // A pre-v8 row on a shot whose uuid the file lacks (pre-v6): the
            // load cannot back-fill it, so it is matched by session shot id.
            ignore(&b0, b, "LONG_SHOT", None),
            // Its shot's uuid is too long for the new model: re-keyed.
            ignore(&b1, b, "SHORT_SHOT", Some(long.clone())),
            // A shot that does not exist: dropped.
            ignore(&b1, b, "LONG_SHOT", Some("gone".to_string())),
        ],
        display_settings: BTreeMap::from([("k".to_string(), "v".to_string())]),
        ..LegacyExtras::default()
    };
    let b0_uuid = b0.uuid.clone().unwrap();
    let bytes = edit_sqlite(&write_v18(&p, &extras).unwrap(), |conn| {
        conn.execute("UPDATE shots SET uuid = NULL WHERE uuid = ?1", [&b0_uuid])
            .unwrap();
    });
    let legacy = open_bytes(&bytes, &OpenOptions::default()).unwrap();
    let out = migrate(&legacy).unwrap();
    check(&legacy, &out).unwrap();
    let r = &out.report;
    assert_eq!(r.shot_uuids_minted, 1);
    assert_eq!(r.legacy_derived_shot_uuids, 1);
    let minted = out.project.lines()[1].shots[1].uuid.clone();
    assert_ne!(minted, long);
    assert!(minted.len() <= 128);
    assert_eq!((r.ignores_kept, r.ignores_rekeyed, r.ignores_dropped), (1, 2, 2));
    let keys: Vec<(&str, &str)> = out
        .extras
        .ignored_warnings
        .iter()
        .map(|w| (w.shot_uuid.as_str(), w.code.as_str()))
        .collect();
    let b0_uuid = out.project.lines()[1].shots[0].uuid.as_str();
    assert_eq!(
        keys,
        vec![
            (out.project.lines()[0].shots[0].uuid.as_str(), "SHORT_SHOT"),
            (b0_uuid, "LONG_SHOT"),
            (minted.as_str(), "SHORT_SHOT"),
        ]
    );
    assert_eq!(out.extras.display_settings, extras.display_settings);
}

#[test]
fn history_is_dropped_and_said_so() {
    let mut p = project();
    line(&mut p, "A", &[(90.0, 10.0)]);
    let options = WriteOptions {
        undo_rows: vec![("{}".to_string(), "{}".to_string(), None)],
        ..WriteOptions::default()
    };
    let bytes = write_v18_with(&p, &LegacyExtras::default(), &options).unwrap();
    let out = migrate_bytes(&bytes).unwrap();
    assert!(out.had_history);
    assert_eq!(out.report.history_rows_dropped, 1);
    assert_eq!(out.report.verification.adjusted, AdjustedCheck::AwaitingSolver);
}

#[test]
fn a_non_sqlite_input_is_refused_with_a_printable_class() {
    let error = migrate_bytes(b"{\"format\":\"stalx\",\"version\":2}").unwrap_err();
    assert_eq!(error.class(), "legacy-not-sqlite");
}

#[test]
fn the_speleodb_link_and_validation_state_are_carried() {
    let mut p = project();
    line(&mut p, "A", &[(90.0, 10.0)]);
    let extras = LegacyExtras {
        speleo_link: Some(m::SpeleoLink {
            endpoint: "https://example.invalid".to_string(),
            project_id: "00000000-0000-4000-8000-000000000002".to_string(),
            project_name: "Synthetic".to_string(),
            base_commit_sha: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
            has_unpushed_edits: true,
            last_edit_at: Some("2026-01-02T03:04:05Z".to_string()),
            last_verify_at: None,
            undo_mode: Some(m::UndoMode::Disabled),
        }),
        speleo_undo_chain: Some("89abcdef0123456789abcdef0123456789abcdef".to_string()),
        speleo_undo_watermark: Some("2026-01-01T00:00:00Z".to_string()),
        ..LegacyExtras::default()
    };
    let legacy = open(&p, &extras);
    let out = migrate(&legacy).unwrap();
    let link = out.extras.speleo_link.as_ref().unwrap();
    assert_eq!(link.project_id, "00000000-0000-4000-8000-000000000002");
    assert_eq!(link.undo_mode, Some(crate::models::UndoMode::Disabled));
    assert!(link.has_unpushed_edits);
    assert_eq!(out.extras.speleo_undo_chain, extras.speleo_undo_chain);
    assert_eq!(out.extras.speleo_undo_watermark, extras.speleo_undo_watermark);
    // The 0.6.0 save always writes the thresholds it holds.
    assert!(out.extras.validation_stored);
}
