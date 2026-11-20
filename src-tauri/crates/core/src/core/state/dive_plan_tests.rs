//! Dive plans on the station graph (plan 10.3): the `.plnx` CRUD and its ownership
//! check, pins across every survey edit that renumbers, moves or retires a station,
//! 0.6.0's anchor forms (pre-#24 keys, pair anchors read with the record parity a
//! migrated project carries), the upgrade of a migrated project's `.plnx`, the
//! segment reconciliation, and the IPC anchor pair.
//!
//! Every survey here is synthetic: free lines whose shots turn as they go, so a pin
//! landing one station off shows in its position.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests build from known-valid literals; a failed unwrap should fail the test"
)]

use chrono::{TimeZone, Utc};
use tempfile::TempDir;

use super::{
    classify_ownership, inherited_segment, merge_segments_for_surviving_waypoints, probe_ownership,
    to_anchor, AppState, Ownership, PersistedDivePlanSnapshot, PersistedMarkedPoint, PlanAnchor,
    PlnxContext, Resolver, StoredAnchor, MAX_MARKED_LABEL_CHARS, MAX_MARKED_POINTS,
    MAX_PLAN_ANCHOR_BATCH, MAX_SEGMENTS, MAX_WAYPOINTS,
};
use crate::config::GasConfig;
use crate::core::Command;
use crate::models::survey::{
    changeset, LineMeta, Project, RangeDeleteMode, ShotPlace, SplitOptions, StartAt,
};
use crate::models::{
    CombineLinesOptions, CombinePick, CylRole, Cylinder, DivePlanSnapshot, Diver, GasReachField,
    LineEnd, LineId, MarkedPoint, PlanSegment, ShotData, ShotId, ShotKind, SplitAt, StationAnchor,
    StationId, StationKey, StationRef, TankMaterial,
};
use crate::persistence::migrate::LegacyParity;
use crate::persistence::plnx::PlnxStore;

// ----- fixtures -------------------------------------------------------------

/// A shot heading `azimuth`, `length` long.
fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..ShotData::default()
    }
}

/// `n` ten-metre shots that turn as they go.
fn turning(n: u32) -> Vec<ShotData> {
    (0..n)
        .map(|i| shot(10.0 + 37.0 * f64::from(i), 10.0))
        .collect()
}

/// Add a free line (its own piece, its start placed at the origin).
fn free_line(p: &mut Project, shots: Vec<ShotData>) -> LineId {
    let line = p
        .add_line(LineMeta::default(), StartAt::New, shots)
        .unwrap();
    let start = p.line(line).unwrap().start;
    p.set_placement(start, 0.0, 0.0).unwrap();
    line
}

/// A one-line project with `n` shots (stations `0..=n`).
fn one_line(n: u32) -> (Project, LineId) {
    let mut p = Project::new("Test", None);
    let line = free_line(&mut p, turning(n));
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    (p, line)
}

fn slot(line: LineId, index: u32) -> StationRef {
    StationRef::new(line, index)
}

fn shot_id(p: &Project, line: LineId, index: usize) -> ShotId {
    p.line(line).unwrap().shots[index].id
}

fn shot_uuid(p: &Project, line: LineId, index: usize) -> String {
    p.line(line).unwrap().shots[index].uuid.clone()
}

fn station_at(p: &Project, at: StationRef) -> StationId {
    p.station_at(at).unwrap()
}

/// The anchor this build writes for a live slot.
fn anchor(p: &Project, at: StationRef) -> StoredAnchor {
    to_anchor(p, Some(at)).expect("a live slot anchors")
}

fn pair(a: &StoredAnchor) -> &PlanAnchor {
    match a {
        StoredAnchor::Pair(p) => Some(p),
        StoredAnchor::Legacy(_) => None,
    }
    .expect("a pair anchor")
}

/// `a` as 0.6.0 wrote it: the same neighbours, no station id.
fn without_station(a: &StoredAnchor) -> StoredAnchor {
    StoredAnchor::Pair(PlanAnchor {
        station: None,
        ..pair(a).clone()
    })
}

/// A pre-#24 key.
fn key(line: LineId, anchor: StationAnchor) -> StoredAnchor {
    StoredAnchor::Legacy(StationKey::new(line, anchor))
}

/// Place `a` on `p` outside a migrated session.
fn place(p: &Project, a: &StoredAnchor) -> Option<StationRef> {
    Resolver::new(p, None).place(a)
}

/// The raw (measured) horizontal position of a slot.
fn xy(p: &Project, at: StationRef) -> [f64; 2] {
    let [x, y, _] = p.raw_positions().slot(p, at).expect("a live slot");
    [x, y]
}

fn near(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-6
}

/// Every slot of `line` with its anchor and its position.
fn pins_of(p: &Project, line: LineId) -> Vec<(StoredAnchor, [f64; 2])> {
    let n = u32::try_from(p.line(line).unwrap().shots.len()).unwrap();
    (0..=n)
        .map(|i| (anchor(p, slot(line, i)), xy(p, slot(line, i))))
        .collect()
}

/// Every pin places on `p`, on the position it was dropped on.
fn assert_pins_land(p: &Project, pins: &[(StoredAnchor, [f64; 2])], what: &str) {
    for (i, (pin, at)) in pins.iter().enumerate() {
        let placed = place(p, pin);
        assert!(placed.is_some(), "{what}: pin {i} is stale");
        let placed = placed.unwrap();
        assert!(
            near(xy(p, placed), *at),
            "{what}: pin {i} moved to {placed:?}"
        );
    }
}

/// How many of `pins` place on `p`; each that does is on its position (never on a
/// neighbouring station).
fn count_landing(p: &Project, pins: &[(StoredAnchor, [f64; 2])], what: &str) -> usize {
    let mut landed = 0;
    for (i, (pin, at)) in pins.iter().enumerate() {
        let Some(placed) = place(p, pin) else {
            continue;
        };
        assert!(
            near(xy(p, placed), *at),
            "{what}: pin {i} moved to {placed:?}"
        );
        landed += 1;
    }
    landed
}

/// An `AppState` holding `project` (a new, unsaved one).
fn state_with(project: Project) -> AppState {
    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);
    state
}

/// [`state_with`], with the project's `.stalx` path in `dir` (nothing is written there; the
/// `.plnx` sits beside it).
fn saved_state(dir: &TempDir, mut project: Project) -> AppState {
    project.file_path = Some(dir.path().join("cave.stalx"));
    state_with(project)
}

fn project_of(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

fn default_diver() -> Diver {
    Diver {
        rmv_l_per_min: 20.0,
        label: None,
    }
}

fn empty_snapshot() -> DivePlanSnapshot {
    serde_json::from_str("{}").unwrap()
}

/// A snapshot carrying every kind of route point, for the #298 persistence tests.
fn snapshot_with_route_points(line: LineId) -> DivePlanSnapshot {
    DivePlanSnapshot {
        origin: Some(slot(line, 0)),
        endpoint: Some(slot(line, 3)),
        waypoints: vec![slot(line, 2)],
        diver: default_diver(),
        cylinders: Vec::new(),
        gas_config: GasConfig::default(),
        stage_drop_overrides: Vec::new(),
        marked_points: vec![MarkedPoint {
            station: slot(line, 2),
            label: Some("photo".to_string()),
        }],
        segments: Vec::new(),
    }
}

fn persisted_with_waypoints(waypoints: Vec<StoredAnchor>) -> PersistedDivePlanSnapshot {
    PersistedDivePlanSnapshot {
        origin: None,
        endpoint: None,
        waypoints,
        diver: default_diver(),
        cylinders: Vec::new(),
        gas_config: GasConfig::default(),
        stage_drop_overrides: Vec::new(),
        marked_points: Vec::new(),
        segments: Vec::new(),
    }
}

fn split_options() -> SplitOptions {
    SplitOptions {
        name_a: "A".to_string(),
        name_b: "B".to_string(),
        color_a: "#111111".to_string(),
        color_b: "#222222".to_string(),
    }
}

fn combine_options() -> CombineLinesOptions {
    CombineLinesOptions {
        name: "K".to_string(),
        color: "#123456".to_string(),
        swap_lr: false,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Kept,
        join: None,
    }
}

// ----- ownership and the CRUD guards -----------------------------------------

fn ctx(path: std::path::PathBuf, created_at: &str) -> PlnxContext {
    PlnxContext {
        path,
        project_created_at: created_at.to_string(),
        project_name: "P".to_string(),
    }
}

#[test]
fn classify_ownership_reads_fresh_owned_foreign() {
    let unused = || std::path::PathBuf::from("unused.plnx");
    assert!(classify_ownership(None, &ctx(unused(), "A")) == Ownership::Fresh);
    assert!(classify_ownership(Some("A".to_string()), &ctx(unused(), "A")) == Ownership::Owned);
    assert!(classify_ownership(Some("A".to_string()), &ctx(unused(), "B")) == Ownership::Foreign);
}

#[test]
fn probe_ownership_matches_classify_ownership_against_a_real_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("owned.plnx");

    // No file yet => Fresh.
    assert!(probe_ownership(&ctx(path.clone(), "A")).unwrap() == Ownership::Fresh);

    // Stamp it as "A"'s file (via the normal open+ensure_identity path).
    {
        let store = PlnxStore::open(&path).unwrap();
        store.ensure_identity("A", "P").unwrap();
    }
    assert!(probe_ownership(&ctx(path.clone(), "A")).unwrap() == Ownership::Owned);
    assert!(probe_ownership(&ctx(path, "B")).unwrap() == Ownership::Foreign);
}

/// Regression guard for the Copilot #256 finding: every command called `PlnxStore::open()`
/// (which unconditionally runs `init_schema` + `set_user_version`) BEFORE checking
/// ownership, so a `Foreign` `.plnx` got its schema/pragma silently touched before being
/// rejected. The file's bytes must be bit-for-bit unchanged after every command rejects it.
#[test]
fn foreign_plnx_is_never_opened_by_the_dive_plan_commands() {
    let dir = TempDir::new().unwrap();
    let plnx_path = dir.path().join("cave.plnx");

    // A .plnx belonging to a DIFFERENT project ("Owner"), with a plan in it.
    {
        let store = PlnxStore::open(&plnx_path).unwrap();
        store
            .ensure_identity("2020-01-01T00:00:00Z", "Owner")
            .unwrap();
        store
            .insert("Owner's plan", "2020-01-01T00:00:00Z", 1, "{}")
            .unwrap();
    }
    let bytes_before = std::fs::read(&plnx_path).unwrap();

    // `Project::new` stamps a fresh created_at (now), which differs from the stamp above.
    let (project, _line) = one_line(3);
    let state = saved_state(&dir, project);

    assert!(
        state.list_dive_plans().unwrap().is_empty(),
        "a Foreign file's plans must never be listed"
    );
    let load_err = state.load_dive_plan(1).unwrap_err();
    assert!(load_err.contains("different project"), "got: {load_err}");
    let save_err = state
        .save_dive_plan(None, "New".to_string(), empty_snapshot())
        .unwrap_err();
    assert!(save_err.contains("different project"), "got: {save_err}");
    let delete_err = state.delete_dive_plan(1).unwrap_err();
    assert!(
        delete_err.contains("different project"),
        "got: {delete_err}"
    );
    // Nor does the upgrade, even on a migrated session.
    state.with_file(|f| f.legacy_parity = Some(LegacyParity::default()));
    assert_eq!(state.upgrade_dive_plan_anchors().unwrap(), 0);

    assert_eq!(
        bytes_before,
        std::fs::read(&plnx_path).unwrap(),
        "a Foreign .plnx must not be mutated by ANY dive-plan command"
    );
}

/// #275 B3: a deferred new project has a RESERVED file_path that was never written. Its
/// first implicit save may auto-rename the .stalx on collision, which would strand any
/// .plnx created beside the reserved name. So while the initial save is deferred, every
/// dive-plan command must refuse (same error as the no-path case) and write nothing.
#[test]
fn deferred_initial_save_blocks_dive_plan_persistence() {
    let dir = TempDir::new().unwrap();
    let plnx_path = dir.path().join("cave.plnx");
    let (project, _line) = one_line(3);
    let state = saved_state(&dir, project);
    state.set_initial_save_deferred_for_tests(true);

    let save_err = state
        .save_dive_plan(None, "New".to_string(), empty_snapshot())
        .unwrap_err();
    assert!(save_err.contains("Save the project"), "got: {save_err}");
    assert!(
        !plnx_path.exists(),
        "no .plnx may be written beside a reserved-but-unwritten .stalx"
    );
    assert!(state.list_dive_plans().unwrap().is_empty());
    let load_err = state.load_dive_plan(1).unwrap_err();
    assert!(load_err.contains("Save the project"), "got: {load_err}");
    let delete_err = state.delete_dive_plan(1).unwrap_err();
    assert!(delete_err.contains("Save the project"), "got: {delete_err}");

    state.set_initial_save_deferred_for_tests(false);
    let meta = state
        .save_dive_plan(None, "New".to_string(), empty_snapshot())
        .unwrap();
    assert_eq!(meta.name, "New");
    assert!(plnx_path.exists(), "post-save the sidecar lands normally");
}

/// The ownership stamp is `created_at.to_rfc3339()` compared as a string, so the instant
/// must survive the `.stalx` v2 round trip to the nanosecond: a project saved, closed and
/// reopened still owns its plans.
#[test]
fn a_reopened_project_still_owns_its_plans_to_the_nanosecond() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("nano.stalx");
    let created = Utc.timestamp_opt(1_700_000_000, 123_456_789).unwrap();
    let mut project = Project::new_at("Nano", None, created);
    let line = free_line(&mut project, turning(3));
    let state = state_with(project);
    let saved = state.execute(Command::SaveProject {
        path: Some(path.clone()),
    });
    assert!(saved.success, "{:?}", saved.error);
    let meta = state
        .save_dive_plan(None, "Plan".to_string(), snapshot_with_route_points(line))
        .unwrap();

    let reopened = AppState::new().unwrap();
    let r = reopened.execute(Command::OpenProject { path });
    assert!(r.success, "{:?}", r.error);
    assert_eq!(project_of(&reopened).created_at, created);
    let listed = reopened.list_dive_plans().unwrap();
    assert_eq!(listed.len(), 1, "the reopened project must own its .plnx");
    let plan = reopened.load_dive_plan(meta.id).unwrap();
    assert_eq!(plan.stale_ref_count, 0);
    assert_eq!(plan.snapshot.origin, Some(slot(line, 0)));
    assert_eq!(plan.snapshot.endpoint, Some(slot(line, 3)));
}

// ----- the anchor itself ----------------------------------------------------

#[test]
fn an_anchor_records_its_station_and_the_shots_it_sits_between() {
    let (project, line) = one_line(3);
    let mid = anchor(&project, slot(line, 2));
    let mid = pair(&mid);
    assert_eq!(
        mid.a.as_deref(),
        Some(shot_uuid(&project, line, 1).as_str())
    );
    assert_eq!(
        mid.b.as_deref(),
        Some(shot_uuid(&project, line, 2).as_str())
    );
    assert_eq!(mid.station, Some(station_at(&project, slot(line, 2))));
    assert!(mid.terminal_shot_length_bits.is_none());
    // The ends have one neighbour, one free side and the neighbour's length.
    let start = anchor(&project, slot(line, 0));
    let start = pair(&start);
    assert!(start.a.is_none() && start.b.is_some());
    assert!(start.terminal_shot_length_bits.is_some());
    let end = anchor(&project, slot(line, 3));
    let end = pair(&end);
    assert!(end.a.is_some() && end.b.is_none());
    // A station-graph project keeps no record parity.
    assert!(!end.parity && end.parity_b.is_none());
    // A ref past the line's end, or into a missing line, cannot be anchored.
    assert!(to_anchor(&project, Some(slot(line, 4))).is_none());
    assert!(to_anchor(&project, Some(slot(line + 99, 1))).is_none());
    assert!(to_anchor(&project, None).is_none());
}

#[test]
fn round_trip_is_identity_when_the_survey_is_unchanged() {
    let (project, line) = one_line(3);
    for i in 0..=3 {
        assert_eq!(
            place(&project, &anchor(&project, slot(line, i))),
            Some(slot(line, i))
        );
        assert_eq!(
            place(&project, &without_station(&anchor(&project, slot(line, i)))),
            Some(slot(line, i)),
            "the 0.6.0 form reads the same on an unedited survey"
        );
    }
}

/// A `.plnx` written before #24 stores `{section_id, anchor}` and must keep working; a
/// pair-form row must NOT be mistaken for one (its fields all default, so the untagged
/// order decides), and the station id must survive serde.
#[test]
fn a_pair_anchor_round_trips_through_json_and_a_legacy_key_stays_legacy() {
    let (project, line) = one_line(3);
    for i in 0..=3 {
        let a = anchor(&project, slot(line, i));
        let back: StoredAnchor = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(back, a);
        assert!(matches!(back, StoredAnchor::Pair(_)));
        assert!(pair(&back).station.is_some());
    }
    // A 0.6.0 pair writes no `station` key at all, so a 0.6.0 row writes back unchanged.
    let old = without_station(&anchor(&project, slot(line, 1)));
    assert!(!serde_json::to_string(&old).unwrap().contains("station"));
    let legacy: StoredAnchor =
        crate::models::disk_names::from_disk_str(r#"{"section_id": 7, "anchor": "SectionStart"}"#)
            .unwrap();
    assert!(matches!(legacy, StoredAnchor::Legacy(_)));
}

/// A plan snapshot in the sidecar's pre-rename vocabulary, holding all three anchor
/// forms, reads and writes back to the same JSON (C3 left the `.plnx` half of the
/// pre-rename disk test here).
#[test]
fn a_pre_rename_plan_blob_reads_and_writes_back_unchanged() {
    let raw = serde_json::json!({
        "origin": { "section_id": 3, "anchor": "SectionStart" },
        "endpoint": { "section_id": 3, "anchor": { "AfterShot": "u-2" } },
        "waypoints": [
            { "section_id": 3, "a": "u-0", "b": "u-1", "after_a": true, "parity": true,
              "parity_b": false, "terminal_shot_length_bits": null }
        ],
        "diver": { "rmv_l_per_min": 18.0, "label": "lead" },
        "cylinders": [],
        "gas_config": serde_json::to_value(GasConfig::default()).unwrap(),
        "stage_drop_overrides": [null],
        "marked_points": [
            { "station": { "section_id": 3, "a": "u-1", "b": null, "after_a": true,
                           "parity": false, "terminal_shot_length_bits": 4_621_819_117_588_971_520_u64 },
              "label": "photo" }
        ],
        "segments": [{ "speed_m_per_min": 10.0, "on_scooter": true }, { "speed_m_per_min": null, "on_scooter": false }]
    });
    let plan: PersistedDivePlanSnapshot =
        crate::models::disk_names::from_disk_str(&raw.to_string()).unwrap();
    assert!(matches!(plan.origin, Some(StoredAnchor::Legacy(_))));
    assert!(matches!(plan.waypoints[0], StoredAnchor::Pair(_)));
    let written = crate::models::disk_names::to_disk_string(&plan).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&written).unwrap(),
        raw
    );
}

// ----- pins across survey edits ---------------------------------------------

/// THE point of the pair form (#24): a reverse renumbers every station in the line, and
/// every stored reference must come back naming the SAME physical station.
#[test]
fn every_anchor_survives_a_reverse() {
    let (mut project, line) = one_line(3);
    let anchors: Vec<_> = (0..=3).map(|i| anchor(&project, slot(line, i))).collect();
    project.reverse_line(line, false).unwrap();
    for (i, a) in (0u32..).zip(&anchors) {
        assert_eq!(
            place(&project, a),
            Some(slot(line, 3 - i)),
            "anchor for station {i} must name the same physical station after the reverse"
        );
    }
}

/// A terminal anchor has one neighbour, so the side is what places it; across a reverse
/// IN BOTH DIRECTIONS its station id is what keeps it right.
#[test]
fn terminal_anchors_survive_a_reverse_both_ways() {
    let (mut project, line) = one_line(3);
    let start = anchor(&project, slot(line, 0));
    let end = anchor(&project, slot(line, 3));

    project.reverse_line(line, false).unwrap();
    assert_eq!(
        place(&project, &start),
        Some(slot(line, 3)),
        "the entrance is now the far end"
    );
    assert_eq!(place(&project, &end), Some(slot(line, 0)));

    project.reverse_line(line, false).unwrap();
    assert_eq!(place(&project, &start), Some(slot(line, 0)));
    assert_eq!(place(&project, &end), Some(slot(line, 3)));
}

/// A single-shot line's two ends touch the same one shot; the station ids tell them apart
/// across a reverse.
#[test]
fn a_single_shot_lines_ends_survive_a_reverse() {
    let (mut project, line) = one_line(1);
    let start = anchor(&project, slot(line, 0));
    let end = anchor(&project, slot(line, 1));
    assert_eq!(place(&project, &start), Some(slot(line, 0)));
    assert_eq!(place(&project, &end), Some(slot(line, 1)));
    project.reverse_line(line, false).unwrap();
    assert_eq!(place(&project, &start), Some(slot(line, 1)));
    assert_eq!(place(&project, &end), Some(slot(line, 0)));
}

/// What the station id buys over 0.6.0's form on a station-graph project: with no record
/// parity to read, a one-neighbour pin in 0.6.0's form reads its side literally, which a
/// reverse makes wrong.
#[test]
fn without_its_station_id_a_terminal_pin_reads_its_side_literally() {
    let (mut project, line) = one_line(3);
    let start = without_station(&anchor(&project, slot(line, 0)));
    let mid = without_station(&anchor(&project, slot(line, 2)));
    project.reverse_line(line, false).unwrap();
    assert_eq!(
        place(&project, &start),
        Some(slot(line, 2)),
        "the saved side of the shot, read against its direction now"
    );
    assert_eq!(
        place(&project, &mid),
        Some(slot(line, 1)),
        "a pin with both neighbours needs no parity"
    );
}

/// Extending a passage is routine, and must not disturb a saved route.
#[test]
fn anchors_survive_a_shot_appended_to_the_end() {
    let (mut project, line) = one_line(3);
    let end = anchor(&project, slot(line, 3));
    let mid = anchor(&project, slot(line, 2));
    project.append_shot(line, shot(90.0, 4.0)).unwrap();
    assert_eq!(place(&project, &end), Some(slot(line, 3)));
    assert_eq!(place(&project, &mid), Some(slot(line, 2)));
    assert_eq!(place(&project, &without_station(&end)), Some(slot(line, 3)));
}

/// Prepending pushes every later station up by one, and the anchored station goes with it.
#[test]
fn a_terminal_anchor_survives_a_prepend() {
    let (mut project, line) = one_line(2);
    let start = anchor(&project, slot(line, 0));
    project.prepend_shot(line, shot(200.0, 4.0)).unwrap();
    assert_eq!(
        place(&project, &start),
        Some(slot(line, 1)),
        "the old entrance is now station 1; a new one sits in front of it"
    );
    // 0.6.0's form too: the length guard does not fire on an extension.
    assert_eq!(
        place(&project, &without_station(&start)),
        Some(slot(line, 1))
    );
}

/// An insert before a shot mid-line pushes the stations after it down the numbering.
#[test]
fn an_anchored_pin_follows_its_station_across_an_insert() {
    let (mut project, line) = one_line(3);
    let pinned = slot(line, 2);
    let a = anchor(&project, pinned);
    let before = shot_id(&project, line, 1);
    project
        .create_shot(line, shot(300.0, 3.0), ShotPlace::Before(before))
        .unwrap();
    assert_eq!(place(&project, &a), Some(slot(line, 3)));
    assert_eq!(place(&project, &without_station(&a)), Some(slot(line, 3)));
}

/// Deleting the head and then appending puts the anchored shot back at position 0 with a
/// follower - the SAME observable state a reverse produces, but needing the opposite
/// answer.
#[test]
fn a_terminal_anchor_survives_delete_then_append() {
    let (mut project, line) = one_line(2);
    let end = anchor(&project, slot(line, 2));
    let head = shot_id(&project, line, 0);
    project.delete_shot(line, head).unwrap();
    project.append_shot(line, shot(90.0, 4.0)).unwrap();
    assert_eq!(place(&project, &end), Some(slot(line, 1)));
    assert_eq!(
        place(&project, &without_station(&end)),
        Some(slot(line, 1)),
        "no reverse happened, so the saved side is read literally"
    );
}

/// Insert Station splits a shot, so an interior anchor's two neighbours stop being
/// adjacent; it must still name the same station.
#[test]
fn an_interior_anchor_survives_a_split_between_its_neighbours() {
    let (mut project, line) = one_line(3);
    let mid = anchor(&project, slot(line, 2));
    let split = shot_id(&project, line, 1);
    project
        .split_shot_at_distance(line, split, 0.5, None)
        .unwrap();
    assert_eq!(project.line(line).unwrap().shots.len(), 4);
    assert_eq!(place(&project, &mid), Some(slot(line, 3)));
    assert_eq!(place(&project, &without_station(&mid)), Some(slot(line, 3)));
}

/// Splitting the shot a terminal anchor names: 0.6.0's form cannot name the station any
/// more (the old uuid stays on the far half) and must FAIL SAFE, reporting stale rather
/// than guessing the split point. The station-graph anchor names the station itself, which
/// is still there, so it places exactly.
#[test]
fn splitting_the_anchored_end_shot_keeps_the_station_or_reports_stale() {
    let (mut project, line) = one_line(2);
    let start = anchor(&project, slot(line, 0));
    let first = shot_id(&project, line, 0);
    project
        .split_shot_at_distance(line, first, 0.5, None)
        .unwrap();
    assert_eq!(place(&project, &start), Some(slot(line, 0)));
    let mut stale = 0;
    assert_eq!(
        Resolver::new(&project, None).place_counting(&without_station(&start), &mut stale),
        None,
        "0.6.0's form cannot name the station after the split, so it must not guess"
    );
    assert_eq!(stale, 1, "and the caller is told, so the UI can warn");
}

/// A shot-less line is one station and no shots - what a GPX waypoint imports as.
#[test]
fn a_shot_less_line_anchors_its_only_station() {
    let mut project = Project::new("Test", None);
    let line = free_line(&mut project, Vec::new());
    let a = anchor(&project, slot(line, 0));
    assert!(pair(&a).a.is_none() && pair(&a).b.is_none());
    assert_eq!(place(&project, &a), Some(slot(line, 0)));
    assert_eq!(place(&project, &without_station(&a)), Some(slot(line, 0)));
}

#[test]
fn a_plan_on_a_shot_less_line_round_trips() {
    let mut project = Project::new("Test", None);
    let line = free_line(&mut project, Vec::new());
    let state = state_with(project);
    let only = slot(line, 0);
    let snapshot = DivePlanSnapshot {
        origin: Some(only),
        endpoint: Some(only),
        waypoints: vec![only],
        diver: default_diver(),
        cylinders: Vec::new(),
        gas_config: GasConfig::default(),
        stage_drop_overrides: Vec::new(),
        marked_points: vec![MarkedPoint {
            station: only,
            label: Some("wp".to_string()),
        }],
        segments: Vec::new(),
    };
    let persisted = state
        .to_persisted(&snapshot)
        .expect("a way point on a shot-less line must not be refused");
    let (back, _anchors, stale) = state.resolve_persisted(persisted).unwrap();
    assert_eq!(stale, 0);
    assert_eq!(back.origin, Some(only));
    assert_eq!(back.endpoint, Some(only));
    assert_eq!(back.waypoints, vec![only]);
    assert_eq!(back.marked_points[0].station, only);
}

/// THE headline regression (#2): a saved ref follows its station across a survey edit,
/// not silently rebinding to whatever is now at the old index.
#[test]
fn a_pin_tracks_its_station_across_an_upstream_shot_deletion() {
    let (mut project, line) = one_line(3);
    let pinned = anchor(&project, slot(line, 2));
    let first = shot_id(&project, line, 0);
    project.delete_shot(line, first).unwrap();
    assert_eq!(place(&project, &pinned), Some(slot(line, 1)));
    assert_eq!(
        place(&project, &without_station(&pinned)),
        Some(slot(line, 1))
    );
}

/// When the pinned station itself is deleted (the shot into it went, and the line now
/// bypasses it), the neighbours place the pin as 0.6.0 did: it survives losing either one,
/// landing on the station the surviving shot now starts at.
#[test]
fn a_pin_whose_station_is_deleted_falls_back_to_its_neighbours() {
    let (mut project, line) = one_line(3);
    let pinned = anchor(&project, slot(line, 2));
    let into = shot_id(&project, line, 1);
    project.delete_shot(line, into).unwrap();
    assert!(!project
        .stations
        .contains_key(&pair(&pinned).station.unwrap()));
    assert_eq!(place(&project, &pinned), Some(slot(line, 1)));
}

/// Losing BOTH neighbours (and the station) leaves nothing to name it by: `None`.
#[test]
fn a_pin_whose_both_neighbouring_shots_are_deleted_no_longer_places() {
    let (mut project, line) = one_line(3);
    let state = state_with(project.clone());
    let pinned = state.plan_anchors_for_refs(vec![slot(line, 2)]).unwrap()[0]
        .clone()
        .expect("station 2 anchors");
    let after = shot_id(&project, line, 2);
    let into = shot_id(&project, line, 1);
    project.delete_shot(line, after).unwrap();
    project.delete_shot(line, into).unwrap();
    let state = state_with(project);
    assert_eq!(
        state.plan_refs_for_anchors(vec![pinned]).unwrap(),
        vec![None]
    );
}

#[test]
fn a_dangling_anchor_after_a_line_delete_is_nulled_and_counted() {
    let (mut project, line) = one_line(3);
    let pinned = anchor(&project, slot(line, 2));
    let other = free_line(&mut project, turning(1));
    project.delete_line(line).unwrap();
    assert!(project.line(other).is_some());
    let mut stale = 0;
    let r = Resolver::new(&project, None);
    assert_eq!(r.place_counting(&pinned, &mut stale), None);
    assert_eq!(stale, 1);
    assert_eq!(r.place_optional(None, &mut stale), None);
    assert_eq!(stale, 1, "an absent reference is not a stale one");
}

/// Split Line Here moves the shots past the cut into a new line, so an anchor recorded
/// against the old line id follows its shots there. The split station is a junction of
/// both halves now, and is shown on the first half (the anchor's line).
#[test]
fn anchors_past_a_line_split_resolve_in_the_second_half() {
    let (mut project, line) = one_line(5);
    let anchors: Vec<_> = (0..=5).map(|i| anchor(&project, slot(line, i))).collect();
    let second = project
        .split_line(
            line,
            SplitAt::Station { station_index: 2 },
            &split_options(),
        )
        .unwrap()
        .value;
    for (i, a) in (0u32..).zip(&anchors) {
        let expected = if i <= 2 {
            slot(line, i)
        } else {
            slot(second, i - 2)
        };
        assert_eq!(place(&project, a), Some(expected), "anchor for station {i}");
        assert_eq!(
            place(&project, &without_station(a)),
            Some(expected),
            "0.6.0 form, station {i}"
        );
    }
}

/// The frontend anchors its positional refs before an edit and places them again after it.
/// Across a Split Line Here, its undo and its redo (the changeset steps the core runs),
/// every station of the cut line lands on the same physical station each time.
#[test]
fn anchors_survive_a_split_its_undo_and_its_redo() {
    for reversed in [false, true] {
        let (mut project, line) = one_line(5);
        if reversed {
            project.reverse_line(line, false).unwrap();
        }
        let pins = pins_of(&project, line);
        let before = project.clone();
        project
            .split_line(
                line,
                SplitAt::Station { station_index: 2 },
                &split_options(),
            )
            .unwrap();
        let changes = changeset::capture(&before, &project);
        assert_pins_land(&project, &pins, &format!("split, reversed {reversed}"));
        changeset::undo(&mut project, &changes).unwrap();
        assert_pins_land(&project, &pins, &format!("undo, reversed {reversed}"));
        changeset::redo(&mut project, &changes).unwrap();
        assert_pins_land(&project, &pins, &format!("redo, reversed {reversed}"));
    }
}

/// A 5-shot line, reversed when asked, with every station's pin and position.
fn delete_split_fixture(reversed: bool) -> (Project, LineId, Vec<(StoredAnchor, [f64; 2])>) {
    let mut project = Project::new("Pins", None);
    let line = free_line(
        &mut project,
        vec![
            shot(90.0, 10.0),
            shot(80.0, 5.0),
            shot(0.0, 4.0),
            shot(300.0, 6.0),
            shot(270.0, 3.0),
        ],
    );
    if reversed {
        project.reverse_line(line, false).unwrap();
    }
    let pins = pins_of(&project, line);
    (project, line, pins)
}

/// Delete-and-split builds the second half fresh; every pin lands on its station or is
/// stale (its station went with the deleted shots), never on a neighbouring one.
#[test]
fn pins_survive_a_delete_and_split_of_a_reversed_line() {
    for reversed in [false, true] {
        let (mut project, line, pins) = delete_split_fixture(reversed);
        let doomed = shot_id(&project, line, 2);
        project
            .delete_shot_range(
                line,
                doomed,
                doomed,
                &RangeDeleteMode::Split {
                    name_a: "A".to_string(),
                    name_b: "B".to_string(),
                },
            )
            .unwrap();
        let what = format!("delete-split reversed {reversed}");
        // Every station survives a single-shot delete-and-split.
        assert_eq!(count_landing(&project, &pins, &what), 6, "{what}");
    }
}

#[test]
fn pins_survive_a_range_delete_and_split_of_a_reversed_line() {
    for reversed in [false, true] {
        let (mut project, line, pins) = delete_split_fixture(reversed);
        let first = shot_id(&project, line, 1);
        let last = shot_id(&project, line, 2);
        project
            .delete_shot_range(
                line,
                first,
                last,
                &RangeDeleteMode::Split {
                    name_a: "A".to_string(),
                    name_b: "B".to_string(),
                },
            )
            .unwrap();
        let what = format!("range delete-split reversed {reversed}");
        // Station 2, between the two deleted shots, goes with them.
        assert_eq!(count_landing(&project, &pins, &what), 5, "{what}");
    }
}

// ----- merges, junctions, combine -------------------------------------------

/// A merge retires one of the two stations into the other (an alias): a pin on the
/// retired one follows it to the survivor, also where only its id can place it.
#[test]
fn a_pin_on_a_merged_away_station_follows_the_survivor() {
    let mut project = Project::new("Merge", None);
    let main = free_line(&mut project, turning(3));
    let side = free_line(&mut project, vec![shot(200.0, 4.0), shot(240.0, 5.0)]);
    let side_start = station_at(&project, slot(side, 0));
    let main_end = station_at(&project, slot(main, 3));
    let pinned = anchor(&project, slot(side, 0));
    let merged = project.merge_stations(side_start, main_end).unwrap().value;
    assert_eq!(
        merged.retired, side_start,
        "the side piece moved onto the main line"
    );
    assert!(!project.stations.contains_key(&side_start));
    assert_eq!(place(&project, &pinned), Some(slot(side, 0)));

    // Reverse the side line: the pin has one neighbour, so only the id (through the
    // alias) knows which end of it the pin is.
    project.reverse_line(side, false).unwrap();
    assert_eq!(place(&project, &pinned), Some(slot(side, 2)));
    assert_eq!(station_at(&project, slot(side, 2)), main_end);
    assert_ne!(
        place(&project, &without_station(&pinned)),
        Some(slot(side, 2))
    );
}

/// A main line with a branch starting at its station 2 (one junction station).
fn junction_fixture() -> (Project, LineId, LineId, StationId) {
    let mut project = Project::new("Junction", None);
    let main = free_line(&mut project, turning(4));
    let junction = station_at(&project, slot(main, 2));
    let branch = project
        .add_line(
            LineMeta::default(),
            StartAt::Station(junction),
            vec![shot(160.0, 6.0), shot(120.0, 5.0)],
        )
        .unwrap();
    assert!(project.validate().is_empty(), "{:?}", project.validate());
    (project, main, branch, junction)
}

/// Detaching a line from a junction gives that line's slot a new, co-located station. The
/// pin's shots are on the detached line and no longer touch the junction, so they place it
/// on the detached line's new station, as 0.6.0 did.
#[test]
fn a_pin_on_a_junction_stays_on_its_line_when_the_line_is_detached() {
    let (mut project, _main, branch, junction) = junction_fixture();
    let pinned = anchor(&project, slot(branch, 0));
    let at = xy(&project, slot(branch, 0));
    project.detach_line(junction, branch).unwrap();
    assert_ne!(station_at(&project, slot(branch, 0)), junction);
    let placed = place(&project, &pinned).unwrap();
    assert_eq!(placed, slot(branch, 0));
    assert!(near(xy(&project, placed), at));
}

/// Deleting the shot into a junction takes the line off the junction (the other lines keep
/// it). The pin's surviving shot no longer touches the junction, so it places by its
/// neighbours, on the station that shot starts at now, as 0.6.0 did.
#[test]
fn a_pin_on_a_junction_follows_its_line_when_the_line_leaves_it() {
    let (mut project, main, _branch, junction) = junction_fixture();
    let pinned = anchor(&project, slot(main, 2));
    let into = shot_id(&project, main, 1);
    project.delete_shot(main, into).unwrap();
    assert!(project.stations.contains_key(&junction));
    assert!(project
        .line_stations(main)
        .unwrap()
        .iter()
        .all(|s| *s != junction));
    assert_eq!(place(&project, &pinned), Some(slot(main, 1)));
}

/// A Station ID is unique in one project's lineage only: a copy edited elsewhere mints the
/// same next id for another station. An id the anchor's own shots do not touch or hold is
/// not trusted: the pin places by its neighbours, and an anchor whose shots this project
/// never had does not place at all.
#[test]
fn a_station_id_its_shots_do_not_vouch_for_is_not_trusted() {
    let (mut project, main, branch, _junction) = junction_fixture();
    let elsewhere = station_at(&project, slot(branch, 2));
    let forged = |a: &StoredAnchor| {
        StoredAnchor::Pair(PlanAnchor {
            station: Some(elsewhere),
            ..pair(a).clone()
        })
    };
    let end = forged(&anchor(&project, slot(main, 4)));
    let mid = forged(&anchor(&project, slot(main, 1)));
    assert_eq!(place(&project, &end), Some(slot(main, 4)));
    assert_eq!(place(&project, &mid), Some(slot(main, 1)));

    // An anchor made on another project, whose line and station ids exist here too.
    let (other, other_line) = one_line(4);
    let foreign = anchor(&other, slot(other_line, 2));
    assert!(project.line(pair(&foreign).line_id).is_some());
    assert!(project
        .stations
        .contains_key(&pair(&foreign).station.unwrap()));
    assert_eq!(place(&project, &foreign), None);

    // A shot-less line's pin is vouched for by its recorded line.
    let lone = free_line(&mut project, Vec::new());
    let lone_pin = anchor(&project, slot(lone, 0));
    assert_eq!(place(&project, &lone_pin), Some(slot(lone, 0)));
    assert_eq!(place(&project, &forged(&lone_pin)), Some(slot(lone, 0)));
}

/// Two lines K (2 shots) and O (3 shots) joined so that `kept_end` of K meets `other_end`
/// of O, each reversed first when asked.
fn combine_fixture(
    kept_end: LineEnd,
    other_end: LineEnd,
    reverse_k: bool,
    reverse_o: bool,
) -> (Project, LineId, LineId) {
    let mut project = Project::new("Pins", None);
    let k = free_line(&mut project, vec![shot(90.0, 10.0), shot(80.0, 5.0)]);
    let o = free_line(
        &mut project,
        vec![shot(0.0, 4.0), shot(300.0, 6.0), shot(270.0, 3.0)],
    );
    if reverse_k {
        project.reverse_line(k, false).unwrap();
    }
    if reverse_o {
        project.reverse_line(o, false).unwrap();
    }
    let end = |p: &Project, line, end| match end {
        LineEnd::Start => p.line(line).unwrap().start,
        LineEnd::End => p.line(line).unwrap().stations().last().unwrap(),
    };
    let (join_k, join_o) = (end(&project, k, kept_end), end(&project, o, other_end));
    project.merge_stations(join_o, join_k).unwrap();
    (project, k, o)
}

/// [`combine_fixture`] with a third line B (two shots) starting at the join: a junction,
/// where B stays attached to the joined station.
fn combine_fixture_with_branch(
    kept_end: LineEnd,
    other_end: LineEnd,
    reverse_k: bool,
    reverse_o: bool,
) -> (Project, LineId, LineId, LineId) {
    let (mut project, k, o) = combine_fixture(kept_end, other_end, reverse_k, reverse_o);
    let join = match kept_end {
        LineEnd::Start => project.line(k).unwrap().start,
        LineEnd::End => project.line(k).unwrap().stations().last().unwrap(),
    };
    let b = project
        .add_line(
            LineMeta::default(),
            StartAt::Station(join),
            vec![shot(200.0, 3.0), shot(160.0, 3.0)],
        )
        .unwrap();
    (project, k, o, b)
}

/// Every pairing and every combination of the two lines' orientations.
fn every_combine_shape() -> Vec<(LineEnd, LineEnd, bool, bool)> {
    let mut out = Vec::new();
    for kept_end in [LineEnd::Start, LineEnd::End] {
        for other_end in [LineEnd::Start, LineEnd::End] {
            for (reverse_k, reverse_o) in
                [(false, false), (true, false), (false, true), (true, true)]
            {
                out.push((kept_end, other_end, reverse_k, reverse_o));
            }
        }
    }
    out
}

/// Combine Lines moves the other line's shots into the kept one, reversing them when the
/// two meet end to end or start to start. Every pin, on either line and at every station
/// including the two line ends, lands on the same physical station, for every pairing and
/// orientation; and pins captured AFTER the combine land again after its undo and redo.
#[test]
fn pins_on_both_lines_survive_a_combine_its_undo_and_its_redo() {
    for (kept_end, other_end, reverse_k, reverse_o) in every_combine_shape() {
        let what = format!("{kept_end:?}-{other_end:?} reversed {reverse_k}/{reverse_o}");
        let (mut project, k, o) = combine_fixture(kept_end, other_end, reverse_k, reverse_o);
        let mut pins = pins_of(&project, k);
        pins.extend(pins_of(&project, o));
        let before = project.clone();

        project.combine_lines(k, o, &combine_options()).unwrap();
        assert!(project.line(o).is_none());
        assert_pins_land(&project, &pins, &format!("combine: {what}"));

        let after = pins_of(&project, k);
        assert_eq!(after.len(), 6);
        let changes = changeset::capture(&before, &project);
        changeset::undo(&mut project, &changes).unwrap();
        assert_pins_land(&project, &after, &format!("undo: {what}"));
        assert_pins_land(&project, &pins, &format!("undo, old pins: {what}"));
        changeset::redo(&mut project, &changes).unwrap();
        assert_pins_land(&project, &after, &format!("redo: {what}"));
    }
}

/// The same at a junction: a third line B at the join stays attached as a branch, and
/// every pin on K, O and B lands on its station across the combine, and pins captured
/// after it across the undo and the redo.
#[test]
fn pins_survive_a_combine_at_a_junction_and_its_undo() {
    for (kept_end, other_end, reverse_k, reverse_o) in every_combine_shape() {
        let what = format!("{kept_end:?}-{other_end:?} reversed {reverse_k}/{reverse_o}");
        let (mut project, k, o, b) =
            combine_fixture_with_branch(kept_end, other_end, reverse_k, reverse_o);
        let before = project.clone();
        let mut pins = pins_of(&project, k);
        pins.extend(pins_of(&project, o));
        pins.extend(pins_of(&project, b));

        project.combine_lines(k, o, &combine_options()).unwrap();
        assert_pins_land(&project, &pins, &format!("combine: {what}"));

        let mut after = pins_of(&project, k);
        after.extend(pins_of(&project, b));
        let changes = changeset::capture(&before, &project);
        changeset::undo(&mut project, &changes).unwrap();
        assert_pins_land(&project, &after, &format!("undo: {what}"));
        changeset::redo(&mut project, &changes).unwrap();
        assert_pins_land(&project, &after, &format!("redo: {what}"));
    }
}

// ----- 0.6.0's pre-#24 keys ---------------------------------------------------

/// `LineStart(line)` is the line's start and `AfterShot(uuid)` that shot's `to`, which is
/// what 0.6.0 resolved them to positionally (station 0, the station after the shot) after
/// every edit, a reverse included: these keys never were reverse-safe.
#[test]
fn a_legacy_key_resolves_to_the_station_0_6_0_resolved_it_to() {
    let (mut project, line) = one_line(3);
    let start = key(line, StationAnchor::LineStart);
    let after_1 = key(line, StationAnchor::AfterShot(shot_uuid(&project, line, 1)));
    let index_3 = key(line, StationAnchor::Index(3));
    assert_eq!(place(&project, &start), Some(slot(line, 0)));
    assert_eq!(place(&project, &after_1), Some(slot(line, 2)));
    assert_eq!(place(&project, &index_3), Some(slot(line, 3)));
    assert_eq!(place(&project, &key(line, StationAnchor::Index(4))), None);

    // An insert before the shot: the station after it moves down the numbering with it.
    let before = shot_id(&project, line, 0);
    project
        .create_shot(line, shot(300.0, 3.0), ShotPlace::Before(before))
        .unwrap();
    assert_eq!(place(&project, &after_1), Some(slot(line, 3)));
    // A prepend: the line's start is the new station.
    project.prepend_shot(line, shot(250.0, 2.0)).unwrap();
    assert_eq!(place(&project, &start), Some(slot(line, 0)));
    assert_eq!(place(&project, &after_1), Some(slot(line, 4)));
    let before_reverse = project.clone();
    // A reverse: still the station after the shot (shot 1 is now index 1 of five), in
    // the other direction: the old station 1, not the old station 2.
    project.reverse_line(line, false).unwrap();
    assert_eq!(place(&project, &after_1), Some(slot(line, 2)));
    assert_eq!(
        project.line(line).unwrap().shots[1].uuid,
        shot_uuid(&before_reverse, line, 3)
    );
    assert_eq!(
        station_at(&project, slot(line, 0)),
        project.line(line).unwrap().start
    );
    assert_eq!(place(&project, &start), Some(slot(line, 0)));
}

/// Shot uuids are project-unique, so an `AfterShot` key finds its shot in whichever line
/// holds it now (0.6.0 looked in the key's line only, and lost it).
#[test]
fn a_legacy_after_shot_key_follows_its_shot_across_a_split_and_a_combine() {
    let (mut project, line) = one_line(5);
    let after_3 = key(line, StationAnchor::AfterShot(shot_uuid(&project, line, 3)));
    let at = xy(&project, slot(line, 4));
    let second = project
        .split_line(
            line,
            SplitAt::Station { station_index: 2 },
            &split_options(),
        )
        .unwrap()
        .value;
    let placed = place(&project, &after_3).unwrap();
    assert_eq!(placed, slot(second, 2));
    assert!(near(xy(&project, placed), at));

    let (mut project, k, o) = combine_fixture(LineEnd::End, LineEnd::Start, false, false);
    let after_o0 = key(o, StationAnchor::AfterShot(shot_uuid(&project, o, 0)));
    let at_o = xy(&project, slot(o, 1));
    assert_eq!(place(&project, &after_o0), Some(slot(o, 1)));
    project.combine_lines(k, o, &combine_options()).unwrap();
    let placed = place(&project, &after_o0).unwrap();
    assert_eq!(placed, slot(k, 3));
    assert!(near(xy(&project, placed), at_o));
}

/// A `.plnx` written before #24 stores `{section_id, anchor}` and must keep working: the
/// whole plan, on upgrade.
#[test]
fn a_legacy_station_key_blob_still_resolves() {
    let (project, line) = one_line(3);
    let uuid = shot_uuid(&project, line, 1);
    // Exactly the shape the old writer produced, in the sidecar's on-disk vocabulary.
    let json = serde_json::json!({
        "origin": { "section_id": line, "anchor": "SectionStart" },
        "endpoint": { "section_id": line, "anchor": { "AfterShot": uuid } },
        "waypoints": [],
        "diver": { "rmv_l_per_min": 20.0, "label": null },
        "cylinders": [],
        "gas_config": serde_json::to_value(GasConfig::default()).unwrap(),
        "stage_drop_overrides": [],
        "marked_points": []
    })
    .to_string();
    let persisted: PersistedDivePlanSnapshot =
        crate::models::disk_names::from_disk_str(&json).unwrap();
    assert!(
        matches!(persisted.origin, Some(StoredAnchor::Legacy(_))),
        "a legacy row must be recognised as legacy, not silently read as an empty pair"
    );
    let state = state_with(project);
    let (snapshot, anchors, stale) = state.resolve_persisted(persisted).unwrap();
    assert_eq!(stale, 0, "an upgrade must not strand anyone's route");
    assert_eq!(snapshot.origin, Some(slot(line, 0)));
    assert_eq!(snapshot.endpoint, Some(slot(line, 2)));
    // The loaded plan hands out station-graph anchors for the same stations.
    let origin = anchors.origin.unwrap();
    assert!(pair(&origin).station.is_some());
    assert_eq!(
        state.plan_refs_for_anchors(vec![origin]).unwrap(),
        vec![Some(slot(line, 0))]
    );
}

#[test]
fn a_pre_298_plnx_blob_still_loads() {
    // The claim "no schema bump needed" rests on the ON-DISK struct's serde defaults.
    let json = serde_json::json!({
        "origin": null,
        "endpoint": null,
        "diver": { "rmv_l_per_min": 20.0, "label": null },
        "cylinders": [],
        "gas_config": serde_json::to_value(GasConfig::default()).unwrap(),
        "stage_drop_overrides": []
    })
    .to_string();
    let persisted: PersistedDivePlanSnapshot = serde_json::from_str(&json).unwrap();
    assert!(persisted.waypoints.is_empty());
    assert!(persisted.marked_points.is_empty());
    assert!(persisted.segments.is_empty());
}

// ----- the persisted snapshot -----------------------------------------------

#[test]
fn waypoints_and_marked_points_track_their_station_across_a_shot_deletion() {
    let (mut project, line) = one_line(3);
    let state = state_with(project.clone());
    let persisted = state
        .to_persisted(&snapshot_with_route_points(line))
        .unwrap();

    // Delete shot 0: its start (the plan's origin) goes, and the station that was at
    // index 2 is now at index 1.
    let first = shot_id(&project, line, 0);
    project.delete_shot(line, first).unwrap();
    state.set_project_for_tests(project);
    let (snapshot, _anchors, stale) = state.resolve_persisted(persisted).unwrap();

    assert_eq!(stale, 1, "the origin's station no longer exists");
    assert_eq!(snapshot.origin, None);
    assert_eq!(snapshot.endpoint, Some(slot(line, 2)));
    assert_eq!(snapshot.waypoints, vec![slot(line, 1)]);
    assert_eq!(snapshot.marked_points.len(), 1);
    assert_eq!(snapshot.marked_points[0].station, slot(line, 1));
    assert_eq!(snapshot.marked_points[0].label.as_deref(), Some("photo"));
}

#[test]
fn a_reverse_leaves_a_saved_plan_pointing_at_the_same_stations() {
    let (mut project, line) = one_line(3);
    let state = state_with(project.clone());
    let persisted = state
        .to_persisted(&snapshot_with_route_points(line))
        .unwrap();
    project.reverse_line(line, false).unwrap();
    state.set_project_for_tests(project);

    let (snapshot, _anchors, stale) = state.resolve_persisted(persisted).unwrap();
    assert_eq!(stale, 0);
    // Saved with origin 0, endpoint 3, way point 2 and a marked point at 2: those
    // physical stations are now numbered 3, 0, 1 and 1.
    assert_eq!(snapshot.origin, Some(slot(line, 3)));
    assert_eq!(snapshot.endpoint, Some(slot(line, 0)));
    assert_eq!(snapshot.waypoints, vec![slot(line, 1)]);
    assert_eq!(snapshot.marked_points[0].station, slot(line, 1));
}

/// End to end through the real `.plnx` and the command pipeline: the reverse and its undo.
#[test]
fn a_saved_plan_follows_its_stations_through_a_reverse_and_its_undo() {
    let dir = TempDir::new().unwrap();
    let (project, line) = one_line(3);
    let state = saved_state(&dir, project);
    let saved = state
        .save_dive_plan(None, "Plan".to_string(), snapshot_with_route_points(line))
        .unwrap();

    let result = state.execute(Command::ReverseLine {
        line_id: line,
        swap_left_right: false,
    });
    assert!(result.success, "{:?}", result.error);

    let plan = state.load_dive_plan(saved.id).unwrap();
    assert_eq!(plan.stale_ref_count, 0);
    assert_eq!(plan.snapshot.origin, Some(slot(line, 3)));
    assert_eq!(plan.snapshot.endpoint, Some(slot(line, 0)));
    assert_eq!(plan.snapshot.waypoints, vec![slot(line, 1)]);
    assert_eq!(plan.snapshot.marked_points[0].station, slot(line, 1));
    // The plan still names the same physical stations, so this is not a user edit and
    // must not surface as one in the plan list.
    assert_eq!(plan.meta.updated_at, saved.updated_at);

    let undone = state.undo();
    assert!(undone.success, "{:?}", undone.error);
    let plan = state.load_dive_plan(saved.id).unwrap();
    assert_eq!(plan.stale_ref_count, 0);
    assert_eq!(plan.snapshot.origin, Some(slot(line, 0)));
    assert_eq!(plan.snapshot.endpoint, Some(slot(line, 3)));
    assert_eq!(plan.snapshot.waypoints, vec![slot(line, 2)]);
}

/// The live pins through the IPC pair around a real reverse, its undo and its redo.
#[test]
fn the_plan_stores_anchors_survive_a_reverse_and_its_undo_and_redo() {
    let (project, line) = one_line(3);
    let state = state_with(project);
    let refs: Vec<_> = (0..=3).map(|i| slot(line, i)).collect();
    let anchors: Vec<_> = state
        .plan_anchors_for_refs(refs.clone())
        .unwrap()
        .into_iter()
        .map(Option::unwrap)
        .collect();
    let mirrored: Vec<_> = (0..=3).rev().map(|i| Some(slot(line, i))).collect();
    let original: Vec<_> = refs.into_iter().map(Some).collect();

    let r = state.execute(Command::ReverseLine {
        line_id: line,
        swap_left_right: false,
    });
    assert!(r.success, "{:?}", r.error);
    assert_eq!(
        state.plan_refs_for_anchors(anchors.clone()).unwrap(),
        mirrored
    );
    assert!(state.undo().success);
    assert_eq!(
        state.plan_refs_for_anchors(anchors.clone()).unwrap(),
        original
    );
    assert!(state.redo().success);
    assert_eq!(state.plan_refs_for_anchors(anchors).unwrap(), mirrored);
}

/// A batch that reverses ONE line twice leaves the numbering as it found it, and the saved
/// plan with it; `[A, B, A]` moves only B's stations (C3 left the `.plnx` halves here).
#[test]
fn batched_reverses_leave_a_saved_plan_on_its_stations() {
    let dir = TempDir::new().unwrap();
    let mut project = Project::new("Batch", None);
    let first = free_line(&mut project, turning(3));
    let second = free_line(&mut project, turning(2));
    let state = saved_state(&dir, project);
    let reverse_of = |line_id| Command::ReverseLine {
        line_id,
        swap_left_right: false,
    };

    let mut snapshot = empty_snapshot();
    snapshot.origin = Some(slot(first, 0));
    snapshot.endpoint = Some(slot(first, 3));
    snapshot.waypoints = vec![slot(first, 2)];
    snapshot.marked_points = vec![MarkedPoint {
        station: slot(first, 1),
        label: Some("photo".to_string()),
    }];
    let twice = state
        .save_dive_plan(None, "Twice".to_string(), snapshot)
        .unwrap();
    let r = state.execute(Command::Batch {
        commands: vec![reverse_of(first), reverse_of(first)],
        label: None,
    });
    assert!(r.success, "{:?}", r.error);
    let plan = state.load_dive_plan(twice.id).unwrap();
    assert_eq!(plan.stale_ref_count, 0);
    assert_eq!(plan.snapshot.origin, Some(slot(first, 0)));
    assert_eq!(plan.snapshot.endpoint, Some(slot(first, 3)));
    assert_eq!(plan.snapshot.waypoints, vec![slot(first, 2)]);
    assert_eq!(plan.snapshot.marked_points[0].station, slot(first, 1));

    let mut snapshot = empty_snapshot();
    snapshot.origin = Some(slot(first, 0));
    snapshot.endpoint = Some(slot(second, 0));
    snapshot.waypoints = vec![slot(first, 2)];
    let aba = state
        .save_dive_plan(None, "ABA".to_string(), snapshot)
        .unwrap();
    let r = state.execute(Command::Batch {
        commands: vec![reverse_of(first), reverse_of(second), reverse_of(first)],
        label: None,
    });
    assert!(r.success, "{:?}", r.error);
    let plan = state.load_dive_plan(aba.id).unwrap();
    assert_eq!(plan.stale_ref_count, 0);
    assert_eq!(
        plan.snapshot.origin,
        Some(slot(first, 0)),
        "reversed twice: unmoved"
    );
    assert_eq!(plan.snapshot.waypoints, vec![slot(first, 2)]);
    assert_eq!(
        plan.snapshot.endpoint,
        Some(slot(second, 2)),
        "the once-reversed line's station 0 is now its station 2"
    );
}

#[test]
fn dangling_waypoints_and_marked_points_are_dropped_and_counted() {
    // Unlike the single origin/endpoint slots there is nothing to null out, so a dangling
    // entry leaves the list entirely - the engine treats an unresolvable way point as a hard
    // stall - and still counts as stale so the UI can warn.
    let (mut project, line) = one_line(3);
    let state = state_with(project.clone());
    let persisted = state
        .to_persisted(&snapshot_with_route_points(line))
        .unwrap();
    free_line(&mut project, turning(1));
    project.delete_line(line).unwrap();
    state.set_project_for_tests(project);
    let (snapshot, anchors, stale) = state.resolve_persisted(persisted).unwrap();
    assert!(snapshot.waypoints.is_empty());
    assert!(snapshot.marked_points.is_empty());
    assert!(anchors.waypoints.is_empty() && anchors.marked_points.is_empty());
    // origin + endpoint + way point + marked point all dangled.
    assert_eq!(stale, 4);
}

#[test]
fn save_refuses_a_route_point_that_no_longer_anchors_rather_than_dropping_it() {
    // Silently dropping was data loss the user could not see: the frontend rebaselines its
    // dirty check on the snapshot it SENT. Refusing keeps file and panel in agreement.
    let dir = TempDir::new().unwrap();
    let (project, line) = one_line(3);
    let state = saved_state(&dir, project);

    let mut snapshot = snapshot_with_route_points(line);
    snapshot.waypoints = vec![slot(line + 99, 1)];
    let err = state
        .save_dive_plan(None, "Plan".to_string(), snapshot)
        .unwrap_err();
    assert!(err.contains("way point"), "got: {err}");

    let mut snapshot = snapshot_with_route_points(line);
    snapshot.waypoints = Vec::new();
    snapshot.marked_points = vec![MarkedPoint {
        station: slot(line, 9),
        label: None,
    }];
    let err = state
        .save_dive_plan(None, "Plan".to_string(), snapshot)
        .unwrap_err();
    assert!(err.contains("marked point"), "got: {err}");

    assert!(state
        .save_dive_plan(None, "Plan".to_string(), snapshot_with_route_points(line))
        .is_ok());
}

#[test]
fn the_save_path_caps_the_route_lists() {
    let dir = TempDir::new().unwrap();
    let (project, line) = one_line(3);
    let state = saved_state(&dir, project);
    let mut snapshot = snapshot_with_route_points(line);
    snapshot.waypoints = vec![slot(line, 1); MAX_WAYPOINTS + 1];
    let err = state
        .save_dive_plan(None, "Plan".to_string(), snapshot)
        .unwrap_err();
    assert!(err.contains("too many way points"), "got: {err}");
    let mut snapshot = snapshot_with_route_points(line);
    snapshot.marked_points = vec![
        MarkedPoint {
            station: slot(line, 1),
            label: None,
        };
        MAX_MARKED_POINTS + 1
    ];
    let err = state
        .save_dive_plan(None, "Plan".to_string(), snapshot)
        .unwrap_err();
    assert!(err.contains("too many marked points"), "got: {err}");
}

#[test]
fn over_long_lists_and_labels_are_capped_on_the_way_in() {
    // The caps exist for a hand-edited `.plnx`, which by definition never crosses the save
    // path - so the LOAD path has to enforce them too.
    let (project, line) = one_line(3);
    let pinned = anchor(&project, slot(line, 2));
    let state = state_with(project);
    let mut persisted = persisted_with_waypoints(vec![pinned.clone(); MAX_WAYPOINTS + 25]);
    persisted.marked_points = vec![
        PersistedMarkedPoint {
            station: pinned.clone(),
            label: Some("x".repeat(MAX_MARKED_LABEL_CHARS + 500)),
        };
        MAX_MARKED_POINTS + 25
    ];
    persisted.stage_drop_overrides = vec![Some(pinned); MAX_WAYPOINTS + 3];
    let (snapshot, anchors, _stale) = state.resolve_persisted(persisted).unwrap();
    assert_eq!(snapshot.waypoints.len(), MAX_WAYPOINTS);
    assert_eq!(anchors.waypoints.len(), MAX_WAYPOINTS);
    assert_eq!(snapshot.marked_points.len(), MAX_MARKED_POINTS);
    assert_eq!(snapshot.stage_drop_overrides.len(), MAX_WAYPOINTS);
    assert_eq!(
        snapshot.marked_points[0]
            .label
            .as_ref()
            .unwrap()
            .chars()
            .count(),
        MAX_MARKED_LABEL_CHARS
    );
}

#[test]
fn over_long_marked_point_label_is_rejected() {
    let dir = TempDir::new().unwrap();
    let (project, line) = one_line(3);
    let state = saved_state(&dir, project);
    let mut snapshot = snapshot_with_route_points(line);
    snapshot.marked_points[0].label = Some("x".repeat(MAX_MARKED_LABEL_CHARS + 1));
    let err = state
        .save_dive_plan(None, "Plan".to_string(), snapshot.clone())
        .unwrap_err();
    assert!(err.contains("label is too long"), "got: {err}");
    // Exactly at the cap is fine - the boundary is inclusive.
    snapshot.marked_points[0].label = Some("x".repeat(MAX_MARKED_LABEL_CHARS));
    assert!(state
        .save_dive_plan(None, "Plan".to_string(), snapshot)
        .is_ok());
}

#[test]
fn a_plan_name_is_required_and_bounded() {
    let dir = TempDir::new().unwrap();
    let (project, line) = one_line(3);
    let state = saved_state(&dir, project);
    let err = state
        .save_dive_plan(None, "  ".to_string(), snapshot_with_route_points(line))
        .unwrap_err();
    assert!(err.contains("Give the dive plan a name"), "got: {err}");
    let err = state
        .save_dive_plan(None, "n".repeat(201), snapshot_with_route_points(line))
        .unwrap_err();
    assert!(err.contains("too long"), "got: {err}");
    let saved = state
        .save_dive_plan(None, " Plan ".to_string(), snapshot_with_route_points(line))
        .unwrap();
    assert_eq!(saved.name, "Plan");
    let renamed = state
        .save_dive_plan(
            Some(saved.id),
            "Again".to_string(),
            snapshot_with_route_points(line),
        )
        .unwrap();
    assert_eq!(renamed.id, saved.id);
    assert_eq!(state.list_dive_plans().unwrap().len(), 1);
    state.delete_dive_plan(saved.id).unwrap();
    assert!(state.list_dive_plans().unwrap().is_empty());
    let err = state.delete_dive_plan(saved.id).unwrap_err();
    assert!(err.contains("no longer exists"), "got: {err}");
}

// ----- segments (#317) ------------------------------------------------------

#[test]
fn segments_round_trip_when_the_survey_is_unchanged() {
    let (project, line) = one_line(3);
    let state = state_with(project);
    let mut snapshot = snapshot_with_route_points(line);
    snapshot.segments = vec![
        PlanSegment {
            speed_m_per_min: Some(12.0),
            on_scooter: true,
        },
        PlanSegment {
            speed_m_per_min: Some(20.0),
            on_scooter: false,
        },
    ];
    let persisted = state.to_persisted(&snapshot).unwrap();
    let (resolved, _anchors, stale) = state.resolve_persisted(persisted).unwrap();
    assert_eq!(stale, 0);
    assert_eq!(resolved.segments, snapshot.segments);
}

/// Delete shots 2 and 3 of `line` (station 3 and both its neighbours go; stations 1 and 5
/// keep theirs).
fn strand_station_3(project: &mut Project, line: LineId) {
    let s2 = shot_id(project, line, 2);
    let s3 = shot_id(project, line, 3);
    project.delete_shot(line, s2).unwrap();
    project.delete_shot(line, s3).unwrap();
}

#[test]
fn dropping_a_waypoint_merges_its_two_adjacent_segments_to_the_slower_speed_and_ors_the_flag() {
    // THE #317 headline case. Dropping way point `j` collapses the spans on either side of
    // it into one, the direction that can only cost MORE: the slower speed, scootered if
    // EITHER half was. Only min(speed) + (a || b) lands on {8.0, true}.
    let (mut project, line) = one_line(6);
    let state = state_with(project.clone());
    let snapshot = DivePlanSnapshot {
        origin: Some(slot(line, 0)),
        endpoint: Some(slot(line, 6)),
        waypoints: vec![slot(line, 1), slot(line, 3), slot(line, 5)],
        diver: default_diver(),
        cylinders: Vec::new(),
        gas_config: GasConfig::default(),
        stage_drop_overrides: Vec::new(),
        marked_points: Vec::new(),
        segments: vec![
            PlanSegment {
                speed_m_per_min: Some(20.0),
                on_scooter: true,
            },
            PlanSegment {
                speed_m_per_min: Some(8.0),
                on_scooter: false,
            },
            PlanSegment {
                speed_m_per_min: Some(15.0),
                on_scooter: true,
            },
            PlanSegment {
                speed_m_per_min: Some(30.0),
                on_scooter: true,
            },
        ],
    };
    let persisted = state.to_persisted(&snapshot).unwrap();
    strand_station_3(&mut project, line);
    state.set_project_for_tests(project);
    let (resolved, _anchors, stale) = state.resolve_persisted(persisted).unwrap();

    assert_eq!(stale, 1, "only way point 1's anchor should dangle");
    assert_eq!(resolved.waypoints.len(), 2);
    assert_eq!(
        resolved.segments,
        vec![
            PlanSegment {
                speed_m_per_min: Some(20.0),
                on_scooter: true,
            },
            PlanSegment {
                speed_m_per_min: Some(8.0),
                on_scooter: true,
            },
            PlanSegment {
                speed_m_per_min: Some(30.0),
                on_scooter: true,
            },
        ]
    );
}

/// The flag on its own, with the speeds held equal: ANDing it was the reach-EXTENDING
/// direction (range is a cap).
#[test]
fn a_merged_span_is_scootered_when_either_half_was() {
    let merged = merge_segments_for_surviving_waypoints(
        vec![
            PlanSegment {
                speed_m_per_min: Some(55.0),
                on_scooter: true,
            },
            PlanSegment {
                speed_m_per_min: Some(55.0),
                on_scooter: false,
            },
        ],
        &[false],
        10.0,
    );
    assert_eq!(
        merged,
        vec![PlanSegment {
            speed_m_per_min: Some(55.0),
            on_scooter: true,
        }]
    );
}

/// The merge and the engine must agree on which stored speeds are REAL: a denormal from a
/// hand-edited `.plnx` must not win the "slowest span" prize.
#[test]
fn a_speed_the_engine_will_not_honour_never_wins_the_merge() {
    let merged = merge_segments_for_surviving_waypoints(
        vec![
            PlanSegment {
                speed_m_per_min: Some(1e-320),
                on_scooter: false,
            },
            PlanSegment {
                speed_m_per_min: Some(3.0),
                on_scooter: false,
            },
        ],
        &[false],
        10.0,
    );
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].speed_m_per_min, Some(3.0));
}

#[test]
fn an_over_long_segment_list_is_folded_into_the_closing_span_not_truncated() {
    let dir = TempDir::new().unwrap();
    let (project, line) = one_line(3);
    let state = saved_state(&dir, project);
    let mut long_list = vec![inherited_segment(), inherited_segment()];
    long_list.extend(std::iter::repeat_n(
        PlanSegment {
            speed_m_per_min: Some(5.0),
            on_scooter: false,
        },
        MAX_SEGMENTS + 24,
    ));
    long_list.push(PlanSegment {
        speed_m_per_min: Some(3.0),
        on_scooter: true,
    });
    let folded = PlanSegment {
        speed_m_per_min: Some(3.0),
        on_scooter: true,
    };

    // Save-side: one way point, so two spans.
    let mut snapshot = snapshot_with_route_points(line);
    snapshot.segments = long_list.clone();
    let persisted = state.to_persisted(&snapshot).unwrap();
    assert_eq!(persisted.segments.len(), 2);
    assert_eq!(persisted.segments[1], folded);

    // Load-side, which is where a doctored file actually enters.
    let mut doctored = persisted;
    doctored.segments = long_list;
    let (resolved, _anchors, _stale) = state.resolve_persisted(doctored).unwrap();
    assert_eq!(resolved.waypoints.len(), 1);
    assert_eq!(resolved.segments.len(), 2);
    assert_eq!(resolved.segments[1], folded);
}

#[test]
fn a_plan_saved_before_segments_existed_loads_one_segment_per_span() {
    let (project, line) = one_line(6);
    let a = |i| anchor(&project, slot(line, i));
    let mut persisted = persisted_with_waypoints(vec![a(1), a(3), a(5)]);
    persisted.origin = Some(a(0));
    persisted.endpoint = Some(a(6));
    let state = state_with(project);
    let (resolved, _anchors, stale) = state.resolve_persisted(persisted).unwrap();
    assert_eq!(stale, 0);
    assert_eq!(resolved.waypoints.len(), 3);
    assert_eq!(resolved.segments.len(), 4);
    assert!(resolved
        .segments
        .iter()
        .all(|s| s.speed_m_per_min.is_none() && !s.on_scooter));
}

#[test]
fn a_short_segment_list_is_padded_rather_than_left_misaligned() {
    let (project, line) = one_line(6);
    let a = |i| anchor(&project, slot(line, i));
    let mut persisted = persisted_with_waypoints(vec![a(1), a(3), a(5)]);
    persisted.segments = vec![PlanSegment {
        speed_m_per_min: Some(42.0),
        on_scooter: true,
    }];
    let state = state_with(project);
    let (resolved, _anchors, _stale) = state.resolve_persisted(persisted).unwrap();
    assert_eq!(resolved.segments.len(), 4);
    assert_eq!(resolved.segments[0].speed_m_per_min, Some(42.0));
    assert!(resolved.segments[0].on_scooter);
    assert!(resolved.segments[3].speed_m_per_min.is_none());
}

// ----- the gas engine behind a loaded plan ----------------------------------

/// A loaded plan is "set the gas inputs and recompute": its refs go straight into
/// `AppState::gas_reach_field` (the survey's gas engine, `compute_gas_reach_field`'s door),
/// which honours the plan's `include_reference_legs` switch: a station reached only across a
/// Reference tie is out of reach unless the plan opts in (#27).
#[test]
fn a_loaded_plan_computes_its_reach_field_and_honours_the_reference_leg_switch() {
    let dir = TempDir::new().unwrap();
    let mut project = Project::new("Gas", None);
    let mut shots = turning(3);
    shots[1].kind = ShotKind::Reference;
    let line = free_line(&mut project, shots);
    let state = saved_state(&dir, project);
    let mut snapshot = empty_snapshot();
    snapshot.origin = Some(slot(line, 0));
    snapshot.diver = Diver::new(20.0, None).unwrap();
    snapshot.cylinders = vec![Cylinder::new(
        1000.0,
        300.0,
        300.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap()];
    let saved = state
        .save_dive_plan(None, "Gas".to_string(), snapshot)
        .unwrap();
    let plan = state.load_dive_plan(saved.id).unwrap();
    let s = &plan.snapshot;
    let field = |config: &GasConfig| {
        state
            .gas_reach_field(
                s.origin.as_ref(),
                s.endpoint.as_ref(),
                &s.waypoints,
                &s.segments,
                &s.diver,
                &s.cylinders,
                config,
                &s.stage_drop_overrides,
            )
            .expect("a project is open")
    };
    let reachable = |f: &GasReachField, i: u32| {
        f.station_refs
            .iter()
            .position(|r| *r == slot(line, i))
            .map(|k| f.reachable[k])
    };
    assert!(!s.gas_config.include_reference_legs);
    let excluded = field(&s.gas_config);
    assert_eq!(reachable(&excluded, 1), Some(true));
    assert_eq!(
        reachable(&excluded, 2),
        Some(false),
        "beyond a Reference tie"
    );
    let included = field(&GasConfig {
        include_reference_legs: true,
        ..s.gas_config.clone()
    });
    assert_eq!(reachable(&included, 2), Some(true));
    assert_eq!(reachable(&included, 3), Some(true));
}

// ----- the IPC pair (#310) --------------------------------------------------

#[test]
fn plan_anchor_round_trip_is_identity_on_an_untouched_project() {
    let (project, line) = one_line(3);
    let state = state_with(project);
    let refs: Vec<_> = (0..=3).map(|i| slot(line, i)).collect();
    let anchors = state.plan_anchors_for_refs(refs.clone()).unwrap();
    assert!(anchors.iter().all(Option::is_some));
    let back = state
        .plan_refs_for_anchors(anchors.into_iter().flatten().collect())
        .unwrap();
    assert_eq!(back, refs.into_iter().map(Some).collect::<Vec<_>>());
}

/// THE #310 guarantee at the command boundary: a pin the frontend holds follows its
/// station across a survey edit. The `assert_ne` pins that echoing the input would fail.
#[test]
fn an_anchored_pin_follows_its_station_across_an_upstream_shot_delete() {
    let (mut project, line) = one_line(3);
    let pinned = slot(line, 2);
    let state = state_with(project.clone());
    let anchor = state.plan_anchors_for_refs(vec![pinned]).unwrap()[0]
        .clone()
        .expect("station 2 anchors");
    let first = shot_id(&project, line, 0);
    project.delete_shot(line, first).unwrap();
    state.set_project_for_tests(project);
    let back = state.plan_refs_for_anchors(vec![anchor]).unwrap();
    assert_eq!(back, vec![Some(slot(line, 1))]);
    assert_ne!(back[0], Some(pinned));
}

#[test]
fn an_unanchorable_ref_is_none_without_failing_the_batch() {
    let (project, line) = one_line(3);
    let state = state_with(project);
    let anchors = state
        .plan_anchors_for_refs(vec![slot(line, 1), slot(line + 99, 0), slot(line, 9)])
        .unwrap();
    assert!(anchors[0].is_some());
    assert!(anchors[1].is_none(), "no such line");
    assert!(anchors[2].is_none(), "no such station in that line");
}

/// The anchors handed out with a loaded plan are index-aligned with the RESOLVED lists, and
/// place again exactly on the refs the snapshot carries.
#[test]
fn a_loaded_plan_carries_the_anchors_of_its_stations() {
    let dir = TempDir::new().unwrap();
    let (project, line) = one_line(6);
    let state = saved_state(&dir, project);
    let snapshot = DivePlanSnapshot {
        origin: Some(slot(line, 0)),
        endpoint: Some(slot(line, 6)),
        waypoints: vec![slot(line, 1), slot(line, 3)],
        diver: default_diver(),
        cylinders: Vec::new(),
        gas_config: GasConfig::default(),
        stage_drop_overrides: vec![None, Some(slot(line, 5))],
        marked_points: vec![MarkedPoint {
            station: slot(line, 4),
            label: None,
        }],
        segments: Vec::new(),
    };
    let saved = state
        .save_dive_plan(None, "Plan".to_string(), snapshot)
        .unwrap();
    let plan = state.load_dive_plan(saved.id).unwrap();
    assert_eq!(plan.stale_ref_count, 0);
    assert!(plan.anchors.origin.is_some() && plan.anchors.endpoint.is_some());
    assert_eq!(plan.anchors.waypoints.len(), plan.snapshot.waypoints.len());
    assert_eq!(
        plan.anchors.marked_points.len(),
        plan.snapshot.marked_points.len()
    );
    assert_eq!(
        plan.anchors.stage_drop_overrides.len(),
        plan.snapshot.stage_drop_overrides.len()
    );
    assert!(plan.anchors.stage_drop_overrides[0].is_none());
    assert!(plan.anchors.stage_drop_overrides[1].is_some());
    let waypoint_anchors: Vec<_> = plan.anchors.waypoints.iter().flatten().cloned().collect();
    assert_eq!(
        state.plan_refs_for_anchors(waypoint_anchors).unwrap(),
        plan.snapshot
            .waypoints
            .iter()
            .copied()
            .map(Some)
            .collect::<Vec<_>>()
    );
}

/// A dropped way point leaves BOTH lists together.
#[test]
fn a_dropped_waypoint_leaves_the_anchor_list_aligned() {
    let (mut project, line) = one_line(6);
    let a = |p: &Project, i| anchor(p, slot(line, i));
    let persisted = persisted_with_waypoints(vec![a(&project, 1), a(&project, 3), a(&project, 5)]);
    strand_station_3(&mut project, line);
    let state = state_with(project);
    let (resolved, anchors, stale) = state.resolve_persisted(persisted).unwrap();
    assert_eq!(stale, 1);
    assert_eq!(resolved.waypoints.len(), 2);
    assert_eq!(anchors.waypoints.len(), 2);
    let placed = state
        .plan_refs_for_anchors(anchors.waypoints.into_iter().flatten().collect())
        .unwrap();
    assert_eq!(
        placed,
        resolved
            .waypoints
            .iter()
            .copied()
            .map(Some)
            .collect::<Vec<_>>()
    );
}

/// What a `u64` becomes after a trip through JavaScript: a double, written back as the
/// shortest decimal that identifies it.
fn through_javascript(bits: u64) -> u64 {
    let as_double = bits as f64;
    format!("{as_double}").parse::<u64>().unwrap_or(bits)
}

/// `a` as the frontend hands it back: `terminal_shot_length_bits` mangled in its low bits.
fn mangled_by_javascript(a: &StoredAnchor) -> StoredAnchor {
    let mut value = serde_json::to_value(a).unwrap();
    let field = value
        .get_mut("terminal_shot_length_bits")
        .expect("a terminal anchor records its shot's length");
    let original = field.as_u64().expect("stored as an integer");
    let mangled = through_javascript(original);
    assert_ne!(mangled, original, "JavaScript grew a 64-bit integer?");
    *field = serde_json::Value::from(mangled);
    serde_json::from_value(value).unwrap()
}

/// An anchor handed to the frontend and straight back must still place after the line is
/// extended past it (the length is compared within a tolerance, never bit for bit); and the
/// guard still catches the split it exists for, on the 0.6.0 form that relies on it.
#[test]
fn an_anchor_survives_the_round_trip_through_javascript() {
    let (project, line) = one_line(2);
    let state = state_with(project.clone());
    let entrance = state.plan_anchors_for_refs(vec![slot(line, 0)]).unwrap()[0]
        .clone()
        .expect("the entrance anchors");
    let returned = mangled_by_javascript(&entrance);
    let returned_old = mangled_by_javascript(&without_station(&entrance));
    // The station id is a u32: JavaScript holds it exactly.
    assert_eq!(pair(&returned).station, pair(&entrance).station);

    let mut extended = project.clone();
    extended.prepend_shot(line, shot(200.0, 4.0)).unwrap();
    state.set_project_for_tests(extended);
    assert_eq!(
        state
            .plan_refs_for_anchors(vec![returned.clone(), returned_old.clone()])
            .unwrap(),
        vec![Some(slot(line, 1)), Some(slot(line, 1))],
        "a pin must not go stale merely because it was held by the frontend for a moment"
    );

    let mut split = project;
    let first = shot_id(&split, line, 0);
    split
        .split_shot_at_distance(line, first, 0.5, None)
        .unwrap();
    state.set_project_for_tests(split);
    assert_eq!(
        state
            .plan_refs_for_anchors(vec![returned, returned_old])
            .unwrap(),
        vec![Some(slot(line, 0)), None],
        "the station is still there; the 0.6.0 form cannot name it and must not guess"
    );
}

#[test]
fn plan_anchor_commands_refuse_when_no_project_is_open() {
    let state = AppState::new().unwrap();
    assert!(state.plan_anchors_for_refs(Vec::new()).is_err());
    assert!(state.plan_refs_for_anchors(Vec::new()).is_err());
}

/// Both anchor commands are bounded, like every other list a plan can hold; one past the
/// cap is refused outright rather than answered short.
#[test]
fn the_anchor_commands_refuse_an_over_long_batch() {
    let (project, line) = one_line(3);
    let pinned = anchor(&project, slot(line, 2));
    let state = state_with(project);
    let station = slot(line, 2);
    assert_eq!(
        state
            .plan_anchors_for_refs(vec![station; MAX_PLAN_ANCHOR_BATCH])
            .unwrap()
            .len(),
        MAX_PLAN_ANCHOR_BATCH
    );
    assert_eq!(
        state
            .plan_refs_for_anchors(vec![pinned.clone(); MAX_PLAN_ANCHOR_BATCH])
            .unwrap()
            .len(),
        MAX_PLAN_ANCHOR_BATCH
    );
    let err = state
        .plan_anchors_for_refs(vec![station; MAX_PLAN_ANCHOR_BATCH + 1])
        .unwrap_err();
    assert!(err.contains("Too many plan anchors"), "got: {err}");
    let err = state
        .plan_refs_for_anchors(vec![pinned; MAX_PLAN_ANCHOR_BATCH + 1])
        .unwrap_err();
    assert!(err.contains("Too many plan anchors"), "got: {err}");
}

// ----- a project migrated from 0.6.0 ------------------------------------------

/// A project 0.6.0 wrote, built with the old model and written by its SQLite writer (both
/// still in the tree, the first save upgrades such a file), opened by this build: the
/// migration carries 0.6.0's record parity for the session, 0.6.0's pins read against it,
/// and the upgrade rewrites them before the parity is gone.
mod legacy {
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::super::{written_by_legacy, PersistedDivePlanSnapshot, PersistedMarkedPoint};
    use super::{
        combine_options, default_diver, project_of, shot, turning, xy, AppState, Command,
        GasConfig, LineId, PlanAnchor, PlnxStore, ShotData, StationRef, StoredAnchor,
    };
    use crate::models::disk_names::{from_disk_str, to_disk_string};
    use stalagmight_legacy::models as old;
    use stalagmight_legacy::LegacyProject as Legacy;

    /// A live value as the frozen 0.6.0 type of the same name (both
    /// serialize alike).
    fn frozen<T: serde::Serialize, U: serde::de::DeserializeOwned>(value: &T) -> U {
        serde_json::from_value(serde_json::to_value(value).unwrap()).unwrap()
    }

    fn legacy_line(p: &mut Legacy, shots: Vec<ShotData>) -> LineId {
        p.add_line(old::LineData {
            shots: shots.iter().map(frozen).collect(),
            ..old::LineData::default()
        })
        .id
    }

    /// The anchor 0.6.0 wrote for `at` (its `to_anchor`).
    fn legacy_anchor(p: &Legacy, at: StationRef) -> StoredAnchor {
        let line = p.get_line(at.line_id).unwrap();
        let i = at.station_index as usize;
        let before = i.checked_sub(1).and_then(|k| line.shots.get(k));
        let after = line.shots.get(i);
        let a = before.and_then(|s| s.uuid.clone());
        let b = after.and_then(|s| s.uuid.clone());
        let terminal_shot_length_bits = match (before, after) {
            (Some(shot), None) | (None, Some(shot)) => Some(shot.length.to_bits()),
            _ => None,
        };
        let parity = p.shot_record_parity(line, a.as_deref().or(b.as_deref()));
        let parity_b = b.as_deref().map(|b| p.shot_record_parity(line, Some(b)));
        StoredAnchor::Pair(PlanAnchor {
            line_id: at.line_id,
            a,
            b,
            after_a: true,
            parity,
            parity_b,
            terminal_shot_length_bits,
            station: None,
        })
    }

    fn legacy_xy(p: &Legacy, at: StationRef) -> [f64; 2] {
        let c = p
            .compute_station_raw_coords(old::StationRef::new(at.line_id, at.station_index))
            .unwrap();
        [c.x, c.y]
    }

    /// Migrated positions match 0.6.0's within its per-hop dust (plan 7.2.7).
    fn near(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-5
    }

    struct Written {
        path: PathBuf,
        k: LineId,
        created_at: String,
        /// Every station of K, pinned before K was reversed.
        before: Vec<(StoredAnchor, [f64; 2])>,
        /// ... and after.
        after: Vec<(StoredAnchor, [f64; 2])>,
    }

    /// K (3 shots) and O (2 shots, reversed) joined end to start and combined into K by
    /// 0.6.0, so O's shots carry counter parity in K; every station of K pinned; K
    /// reversed; every station pinned again; written as 0.6.0 wrote it.
    fn write_legacy(dir: &TempDir) -> Written {
        let mut p = Legacy::new("Legacy".to_string(), None);
        let k = legacy_line(&mut p, turning(3));
        let o = legacy_line(&mut p, vec![shot(250.0, 6.0), shot(210.0, 7.0)]);
        p.ensure_shot_uuids();
        assert!(p.reverse_line(o, false));
        p.merge_stations(old::StationRef::new(k, 3), old::StationRef::new(o, 0))
            .unwrap();
        p.combine_lines(k, o, &frozen(&combine_options())).unwrap();
        assert_eq!(p.get_line(k).unwrap().shots.len(), 5);
        assert!(
            !p.counter_parity_shots.is_empty(),
            "the fixture must carry counter parity"
        );
        let pins = |p: &Legacy| {
            (0..=5)
                .map(|i| {
                    let at = StationRef::new(k, i);
                    (legacy_anchor(p, at), legacy_xy(p, at))
                })
                .collect::<Vec<_>>()
        };
        let before = pins(&p);
        assert!(p.reverse_line(k, false));
        let after = pins(&p);
        let path = dir.path().join("cave.stalx");
        let bytes = stalagmight_legacy::write_v18(&p, &Default::default()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        Written {
            path,
            k,
            created_at: p.created_at.to_rfc3339(),
            before,
            after,
        }
    }

    fn open(path: &Path) -> AppState {
        let state = AppState::new().unwrap();
        let r = state.execute(Command::OpenProject {
            path: path.to_path_buf(),
        });
        assert!(r.success, "{:?}", r.error);
        state
    }

    /// The pins `pins` place on, through the IPC pair.
    fn placed(state: &AppState, pins: &[(StoredAnchor, [f64; 2])]) -> Vec<Option<StationRef>> {
        state
            .plan_refs_for_anchors(pins.iter().map(|(a, _)| a.clone()).collect())
            .unwrap()
    }

    /// 0.6.0's pins land on their stations in a migrated session: the record parity it
    /// carries (K reversed, O's shots against K's parity) flips exactly the one-neighbour
    /// pins 0.6.0 flipped.
    #[test]
    fn a_migrated_project_reads_0_6_0_pins_with_its_record_parity() {
        crate::app_dirs::init_for_test();
        let dir = TempDir::new().unwrap();
        let w = write_legacy(&dir);
        let state = open(&w.path);
        let parity = state
            .with_file(|f| f.legacy_parity.clone())
            .expect("a migrated session carries 0.6.0's record parity");
        assert!(parity.reversed_lines.contains(&w.k));
        assert!(!parity.counter_parity_shots.is_empty());
        let project = project_of(&state);
        for (what, pins) in [("before the reverse", &w.before), ("after it", &w.after)] {
            for (i, (slot, (_, at))) in placed(&state, pins).iter().zip(pins).enumerate() {
                assert!(slot.is_some(), "{what}: pin {i} is stale");
                let slot = slot.unwrap();
                assert!(near(xy(&project, slot), *at), "{what}: pin {i} on {slot:?}");
            }
        }

        // Without it (a version 2 reopen of the upgraded file has none) a 0.6.0 pin's
        // saved side is read literally: right for the pins written after the reverse, one
        // station off for the line ends pinned before it. Why the upgrade rewrites them.
        state.with_file(|f| f.legacy_parity = None);
        for (i, (slot, (_, at))) in placed(&state, &w.after).iter().zip(&w.after).enumerate() {
            assert!(near(xy(&project, slot.unwrap()), *at), "after: pin {i}");
        }
        let off: Vec<usize> = placed(&state, &w.before)
            .iter()
            .zip(&w.before)
            .enumerate()
            .filter(|(_, (slot, (_, at)))| !near(xy(&project, slot.unwrap()), *at))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            off,
            vec![0, 5],
            "only the one-neighbour pins need the parity"
        );
    }

    /// The upgrade rewrites a migrated project's 0.6.0 pins in the station-graph form while
    /// the parity is known, keeps each plan's `updated_at`, and the plan still lands after
    /// the project is saved as version 2 and reopened without any parity.
    #[test]
    fn the_upgrade_rewrites_0_6_0_pins_while_the_parity_is_known() {
        crate::app_dirs::init_for_test();
        let dir = TempDir::new().unwrap();
        let w = write_legacy(&dir);
        let pin = |set: &[(StoredAnchor, [f64; 2])], i: usize| set[i].0.clone();
        let plan = PersistedDivePlanSnapshot {
            origin: Some(pin(&w.before, 0)),
            endpoint: Some(pin(&w.before, 5)),
            waypoints: vec![pin(&w.before, 2)],
            diver: default_diver(),
            cylinders: Vec::new(),
            gas_config: GasConfig::default(),
            stage_drop_overrides: vec![None, Some(pin(&w.after, 1))],
            marked_points: vec![PersistedMarkedPoint {
                station: pin(&w.after, 4),
                label: Some("photo".to_string()),
            }],
            segments: Vec::new(),
        };
        let expected = [
            w.before[0].1,
            w.before[5].1,
            w.before[2].1,
            w.after[1].1,
            w.after[4].1,
        ];
        let stamped = "2026-01-02T03:04:05+00:00";
        let plnx = dir.path().join("cave.plnx");
        let id = {
            let store = PlnxStore::open(&plnx).unwrap();
            store.ensure_identity(&w.created_at, "Legacy").unwrap();
            store
                .insert("Old plan", stamped, 2, &to_disk_string(&plan).unwrap())
                .unwrap()
                .id
        };
        let check = |state: &AppState, what: &str| {
            let loaded = state.load_dive_plan(id).unwrap();
            assert_eq!(loaded.stale_ref_count, 0, "{what}");
            assert_eq!(loaded.meta.updated_at, stamped, "{what}: not a user edit");
            let s = &loaded.snapshot;
            let got = [
                s.origin.unwrap(),
                s.endpoint.unwrap(),
                s.waypoints[0],
                s.stage_drop_overrides[1].unwrap(),
                s.marked_points[0].station,
            ];
            assert!(s.stage_drop_overrides[0].is_none());
            let project = project_of(state);
            for (i, (slot, at)) in got.iter().zip(expected).enumerate() {
                assert!(near(xy(&project, *slot), at), "{what}: pin {i} on {slot:?}");
            }
        };

        let state = open(&w.path);
        assert_eq!(
            project_of(&state).created_at.to_rfc3339(),
            w.created_at,
            "the migration keeps the creation instant to the nanosecond"
        );
        assert_eq!(
            state.list_dive_plans().unwrap().len(),
            1,
            "the migrated project owns 0.6.0's plan file"
        );
        check(&state, "migrated");

        assert_eq!(state.upgrade_dive_plan_anchors().unwrap(), 1);
        assert!(state.with_file(|f| f.legacy_parity.is_none()));
        assert_eq!(
            state.upgrade_dive_plan_anchors().unwrap(),
            0,
            "nothing left"
        );
        let row = PlnxStore::open(&plnx).unwrap().get(id).unwrap().unwrap();
        let upgraded: PersistedDivePlanSnapshot = from_disk_str(&row.snapshot_json).unwrap();
        assert!(upgraded.anchors().all(|a| !written_by_legacy(a)));
        assert_eq!(upgraded.marked_points[0].label.as_deref(), Some("photo"));
        check(&state, "upgraded");

        let saved = state.execute(Command::SaveProject { path: None });
        assert!(saved.success, "{:?}", saved.error);
        let reopened = open(&w.path);
        assert!(reopened.with_file(|f| f.legacy_parity.is_none()));
        assert_eq!(reopened.list_dive_plans().unwrap().len(), 1);
        check(&reopened, "reopened as version 2");
    }

    /// The save that upgrades a migrated project runs the anchor upgrade itself
    /// (`save_with`): a 0.6.0 one-neighbour pin on a line reversed after the
    /// plan was saved still lands on its station once the version 2 file is
    /// reopened without any record parity.
    #[test]
    fn the_upgrading_save_rewrites_0_6_0_pins() {
        crate::app_dirs::init_for_test();
        let dir = TempDir::new().unwrap();
        let w = write_legacy(&dir);
        let plan = PersistedDivePlanSnapshot {
            origin: Some(w.before[0].0.clone()),
            endpoint: Some(w.before[5].0.clone()),
            waypoints: Vec::new(),
            diver: default_diver(),
            cylinders: Vec::new(),
            gas_config: GasConfig::default(),
            stage_drop_overrides: Vec::new(),
            marked_points: Vec::new(),
            segments: Vec::new(),
        };
        let plnx = dir.path().join("cave.plnx");
        let id = {
            let store = PlnxStore::open(&plnx).unwrap();
            store.ensure_identity(&w.created_at, "Legacy").unwrap();
            store
                .insert(
                    "Old plan",
                    "2026-01-02T03:04:05+00:00",
                    2,
                    &to_disk_string(&plan).unwrap(),
                )
                .unwrap()
                .id
        };
        let state = open(&w.path);
        let saved = state.execute(Command::SaveProject { path: None });
        assert!(saved.success, "{:?}", saved.error);
        assert!(
            state.with_file(|f| f.legacy_parity.is_none()),
            "the upgrade ran and dropped the parity"
        );
        let row = PlnxStore::open(&plnx).unwrap().get(id).unwrap().unwrap();
        let upgraded: PersistedDivePlanSnapshot = from_disk_str(&row.snapshot_json).unwrap();
        assert!(upgraded.anchors().all(|a| !written_by_legacy(a)));

        let reopened = open(&w.path);
        let loaded = reopened.load_dive_plan(id).unwrap();
        assert_eq!(loaded.stale_ref_count, 0);
        let project = project_of(&reopened);
        let s = &loaded.snapshot;
        assert!(
            near(xy(&project, s.origin.unwrap()), w.before[0].1),
            "origin"
        );
        assert!(
            near(xy(&project, s.endpoint.unwrap()), w.before[5].1),
            "endpoint"
        );
    }
}
