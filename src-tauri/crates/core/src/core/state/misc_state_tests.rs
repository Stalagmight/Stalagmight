//! The last of the 0.6.0 `state/tests.rs` on the station-graph core: the
//! sections no command family took, and the cases the family packages left
//! for this triage (package C20; its report maps every section of the old
//! file to the suite that now holds it, or to the reason it was retired).
//!
//! What 0.6.0 pinned on positional bookkeeping is pinned here on what
//! replaced it:
//! - "Station Ref Shifting" re-keyed merges, datums, closures and station
//!   offsets after an insert. A reference is a station id now, so it follows
//!   its station by construction; what the user sees is the slot the wire
//!   shows it on, and that is what these tests read.
//! - "Line offset adjustment for insert_before at pos=0" compensated a line
//!   offset so a prepend moved nothing. Positions are derived now; the
//!   property is the same: the stations that were there do not move.
//! - #413 findings 1, 7, 24 and 26 were drift, wedge and overflow bugs of the
//!   snapshot engine. The user-visible contracts behind them (a redo puts
//!   every uuid back on its own shot, a jump lands where single steps land,
//!   ids never overflow or repeat, a shot edit never re-georeferences the
//!   survey) are checked on the changeset engine.
//!
//! All data is synthetic, built in memory, or a temporary copy of a tracked
//! e2e fixture (the legacy-open cases).
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::float_cmp,
    clippy::indexing_slicing,
    reason = "test module: panics on a failed assumption are the point; ids and \
              positions are compared bit for bit"
)]

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::synthetic_stalx_tests::{link, write_v2_project};
use super::topology_kit::{
    assert_near, assert_round_trip, assert_same, assert_same_xy, empty, line_from, meta, north,
    open, origin, project, raw_xy, run, shot, slot, snapshot, st, xy, Opened,
};
use super::AppState;
use crate::core::command::EDIT_DISABLED_CODE;
use crate::core::{Command, CommandResult, CommentHolder};
use crate::models::survey::{ClosureStrategy, Project, StartAt, ZeroDatum};
use crate::models::{
    Coordinates, GpsCoordinates, LineId, LineUpdates, ShotData, ShotId, ShotKind, ShotUpdates,
    SpeleoLink, StationRef, UndoMode, FEATURE_ID_STRIDE,
};
use crate::persistence::stalx_v2::{self, DecodeOptions, Decoded};
use crate::persistence::ResolvedWarning;

// ===========================================================================
// Shared readers
// ===========================================================================

/// The session ids of `line`'s shots, in order.
fn shot_ids(state: &AppState, line: LineId) -> Vec<ShotId> {
    project(state)
        .line(line)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.id)
        .collect()
}

/// `(uuid, length)` of every shot of `line`, in order: which identity ended up
/// on which measurement.
fn uuid_lengths(state: &AppState, line: LineId) -> Vec<(String, f64)> {
    project(state)
        .line(line)
        .unwrap()
        .shots
        .iter()
        .map(|s| (s.uuid.clone(), s.length))
        .collect()
}

fn decoded(path: &Path) -> Decoded {
    stalx_v2::decode(&std::fs::read(path).unwrap(), &DecodeOptions::default()).unwrap()
}

/// A free line on a new piece of its own, its start placed at `at`.
fn free_line(p: &mut Project, name: &str, at: [f64; 2], shots: Vec<ShotData>) -> LineId {
    super::topology_kit::line_at(p, name, at, shots)
}

/// A line on a new piece with no anchor yet (the caller adds a datum).
fn bare_line(p: &mut Project, name: &str, shots: Vec<ShotData>) -> LineId {
    p.add_line(meta(name), StartAt::New, shots).unwrap()
}

/// `CreateShot` of `data` on `line`, after or before an anchor shot (neither:
/// appended).
fn create_shot(
    state: &AppState,
    line: LineId,
    data: ShotData,
    after: Option<ShotId>,
    before: Option<ShotId>,
) -> CommandResult {
    run(
        state,
        Command::CreateShot {
            line_id: line,
            shot: data,
            after_shot_id: after,
            before_shot_id: before,
        },
    )
}

// ===========================================================================
// Station Ref Shifting: every attachment follows its station
// ===========================================================================

/// Where the wire shows each attachment of the survey: the derived junction
/// pairs, each datum's slot, each closure's ends, each legacy hand move.
#[derive(Debug, Clone, PartialEq)]
struct Shown {
    merges: Vec<(StationRef, StationRef)>,
    datums: Vec<StationRef>,
    closures: Vec<(StationRef, StationRef)>,
    hand_moves: Vec<(StationRef, [f64; 3])>,
}

fn shown(state: &AppState) -> Shown {
    let p = project(state);
    let wire = p.project_wire();
    let mut hand_moves = Vec::new();
    for line in p.lines() {
        for (station, delta) in &line.hand_moves {
            let at = p.topology().slot_on(*station, line.id).unwrap();
            hand_moves.push((at, *delta));
        }
    }
    hand_moves.sort_by_key(|(at, _)| *at);
    Shown {
        merges: wire
            .station_merges
            .iter()
            .map(|m| (m.station_a, m.station_b))
            .collect(),
        datums: wire.zero_datums.iter().map(|d| d.station_ref).collect(),
        closures: wire
            .loop_closures
            .iter()
            .map(|c| (c.station_a, c.station_b))
            .collect(),
        hand_moves,
    }
}

/// `shown` with every slot of `line` at index `from` or beyond one index
/// along: what an insert of one station there must show.
fn shifted(shown: &Shown, line: LineId, from: u32) -> Shown {
    let along = |at: StationRef| {
        if at.line_id == line && at.station_index >= from {
            StationRef::new(at.line_id, at.station_index + 1)
        } else {
            at
        }
    };
    Shown {
        merges: shown
            .merges
            .iter()
            .map(|(a, b)| (along(*a), along(*b)))
            .collect(),
        datums: shown.datums.iter().map(|d| along(*d)).collect(),
        closures: shown
            .closures
            .iter()
            .map(|(a, b)| (along(*a), along(*b)))
            .collect(),
        hand_moves: shown
            .hand_moves
            .iter()
            .map(|(at, d)| (along(*at), *d))
            .collect(),
    }
}

/// Line A (three stations, held by a datum on A:2) with B starting on A:0 and
/// C starting on A:2, a loop closure between the far ends of B and C, and a
/// legacy hand move of A at A:1 (0.6.0's `station_offsets`; no command makes
/// one, only a migrated file has them). Returns `(opened, a, b, c)`.
fn hub_survey() -> (Opened, LineId, LineId, LineId) {
    let mut p = empty("Shift Test");
    let a = bare_line(&mut p, "Line A", north(2, 10.0));
    let (a0, a1, a2) = (st(&p, a, 0), st(&p, a, 1), st(&p, a, 2));
    p.push_datum(ZeroDatum::new(a2, origin()));
    let b = line_from(
        &mut p,
        "Line B",
        a0,
        vec![shot(90.0, 15.0), shot(90.0, 25.0)],
    );
    let c = line_from(&mut p, "Line C", a2, vec![shot(270.0, 12.0)]);
    let (b2, c1) = (st(&p, b, 2), st(&p, c, 1));
    p.push_closure(b2, Some(b), c1, Some(c), ClosureStrategy::Batch);
    p.line_mut(a)
        .unwrap()
        .hand_moves
        .insert(a1, [1.0, 2.0, 3.0]);
    (open(&p), a, b, c)
}

/// 0.6.0's `test_insert_before_shifts_station_merge`, `_zero_datum`,
/// `_station_offset`, `_loop_closure`, `_undo_restores_refs` and
/// `_redo_shifts_refs_again` in one: a shot inserted before A's first shot
/// shows every attachment of A one slot along (the junctions with B and C,
/// the datum, the hand move), leaves the other lines' slots and the closure
/// where they were, and one undo and one redo land on the two states.
#[test]
fn a_prepended_shot_shows_every_attachment_of_its_line_one_slot_along() {
    let (o, a, b, c) = hub_survey();
    let before_shown = shown(&o.state);
    assert_eq!(before_shown.merges.len(), 2, "setup: two junctions");
    assert_eq!(before_shown.datums, vec![slot(a, 2)]);
    assert_eq!(before_shown.closures, vec![(slot(b, 2), slot(c, 1))]);
    assert_eq!(before_shown.hand_moves, vec![(slot(a, 1), [1.0, 2.0, 3.0])]);
    let stations_before = project(&o.state).topology().chain(a).unwrap().to_vec();
    let before = snapshot(&o.state);

    let first = shot_ids(&o.state, a)[0];
    create_shot(&o.state, a, shot(45.0, 5.0), None, Some(first));

    let after_shown = shown(&o.state);
    assert_eq!(after_shown, shifted(&before_shown, a, 0));
    let p = project(&o.state);
    assert_eq!(
        &p.topology().chain(a).unwrap()[1..],
        &stations_before[..],
        "the old stations are the same stations, one index along"
    );
    assert_eq!(
        p.line(b).unwrap().start,
        stations_before[0],
        "B still starts on A's old start"
    );

    assert_round_trip(&o.state, &before, "a prepended shot");
    assert_eq!(
        shown(&o.state),
        after_shown,
        "the redo shows them along again"
    );
    assert!(o.state.undo().success);
    assert_eq!(shown(&o.state), before_shown, "the undo shows them back");
}

/// 0.6.0's `test_insert_after_only_shifts_refs_beyond`: a shot inserted after
/// A's first shot makes a station at index 2, so A:0 (the junction with B)
/// and A:1 (the hand move) stay where they are and only A:2 onwards moves.
#[test]
fn a_shot_inserted_after_the_first_shows_only_the_slots_beyond_it_along() {
    let (o, a, _, _) = hub_survey();
    let before_shown = shown(&o.state);
    let before = snapshot(&o.state);

    let first = shot_ids(&o.state, a)[0];
    create_shot(&o.state, a, shot(45.0, 7.0), Some(first), None);

    let after_shown = shown(&o.state);
    assert_eq!(after_shown, shifted(&before_shown, a, 2));
    assert!(after_shown.merges.iter().any(|(x, _)| *x == slot(a, 0)));
    assert_eq!(after_shown.hand_moves[0].0, slot(a, 1));
    assert_round_trip(&o.state, &before, "an inserted shot");
}

/// 0.6.0's `test_delete_shot_undo_redo_with_snapshot`: deleting A's first
/// shot takes A off the junction it started on (B keeps the station, and the
/// wire shows no junction any more), shows A's datum one slot back, and one
/// undo and one redo land on the two states exactly.
#[test]
fn deleting_the_first_shot_at_a_junction_leaves_it_to_the_other_line() {
    let mut p = empty("Delete Shift");
    let a = bare_line(&mut p, "Line A", vec![shot(0.0, 10.0), shot(0.0, 20.0)]);
    let (a0, a2) = (st(&p, a, 0), st(&p, a, 2));
    p.push_datum(ZeroDatum::new(a2, origin()));
    let b = line_from(
        &mut p,
        "Line B",
        a0,
        vec![shot(90.0, 15.0), shot(90.0, 25.0)],
    );
    let o = open(&p);
    let before_shown = shown(&o.state);
    assert_eq!(before_shown.merges, vec![(slot(a, 0), slot(b, 0))]);
    assert_eq!(before_shown.datums, vec![slot(a, 2)]);
    let before = snapshot(&o.state);
    let b_before = raw_xy(&project(&o.state));

    let first = shot_ids(&o.state, a)[0];
    run(
        &o.state,
        Command::DeleteShot {
            line_id: a,
            shot_id: first,
        },
    );

    let after_shown = shown(&o.state);
    assert!(after_shown.merges.is_empty(), "{:?}", after_shown.merges);
    assert_eq!(after_shown.datums, vec![slot(a, 1)]);
    let p = project(&o.state);
    assert_eq!(p.line(a).unwrap().shots.len(), 1);
    assert_eq!(p.line(b).unwrap().start, a0, "B keeps the station");
    for i in 0..3 {
        assert_near(
            xy(&p, slot(b, i)),
            b_before[&slot(b, i)],
            1e-9,
            "B stays where it was",
        );
    }

    assert_round_trip(&o.state, &before, "the delete");
    assert_eq!(shown(&o.state), after_shown);
}

// ===========================================================================
// Line offset adjustment for insert_before at pos=0: nothing that was there
// moves
// ===========================================================================

/// 0.6.0's `test_insert_before_pos0_adjusts_line_offsets`,
/// `_adjusts_merged_line_offsets` and `_undo_restores_offsets`: a shot
/// prepended to a line puts the new start one shot vector behind the old one
/// and moves no station that was there, on the line or on a line through its
/// old start; the undo puts every slot back and the redo again.
#[test]
fn a_prepended_shot_starts_behind_the_old_start_and_moves_nothing() {
    let mut p = empty("Offset Test");
    let a = free_line(
        &mut p,
        "Line A",
        [0.0, 0.0],
        vec![shot(0.0, 10.0), shot(0.0, 5.0)],
    );
    let a0 = st(&p, a, 0);
    let b = line_from(&mut p, "Line B", a0, vec![shot(90.0, 8.0)]);
    let o = open(&p);
    let before_xy = raw_xy(&project(&o.state));
    assert_near(before_xy[&slot(a, 2)], [0.0, 15.0], 1e-9, "setup: A:2");
    let before = snapshot(&o.state);

    let first = shot_ids(&o.state, a)[0];
    create_shot(&o.state, a, shot(90.0, 3.0), None, Some(first));

    let p = project(&o.state);
    assert_near(xy(&p, slot(a, 0)), [-3.0, 0.0], 1e-9, "the new start");
    for i in 0..3 {
        assert_near(
            xy(&p, slot(a, i + 1)),
            before_xy[&slot(a, i)],
            1e-9,
            &format!("A's old station {i}"),
        );
    }
    for i in 0..2 {
        assert_near(
            xy(&p, slot(b, i)),
            before_xy[&slot(b, i)],
            1e-9,
            &format!("B:{i}"),
        );
    }

    assert!(o.state.undo().success);
    assert_same_xy(&raw_xy(&project(&o.state)), &before_xy);
    assert_same(&snapshot(&o.state), &before, "the undo");
    assert!(o.state.redo().success);
    assert_near(
        xy(&project(&o.state), slot(a, 0)),
        [-3.0, 0.0],
        1e-9,
        "the redo",
    );
}

/// 0.6.0's `test_insert_before_pos_gt0_no_offset_adjustment`: a shot inserted
/// inside a line keeps the line's head where it was and carries the tail by
/// the new shot's vector.
#[test]
fn a_shot_inserted_inside_a_line_keeps_its_head_and_carries_its_tail() {
    let mut p = empty("No Offset Test");
    let a = free_line(
        &mut p,
        "Line A",
        [4.0, 2.0],
        vec![shot(0.0, 10.0), shot(0.0, 5.0)],
    );
    let o = open(&p);
    let before_xy = raw_xy(&project(&o.state));

    let second = shot_ids(&o.state, a)[1];
    create_shot(&o.state, a, shot(90.0, 3.0), None, Some(second));

    let p = project(&o.state);
    assert_near(xy(&p, slot(a, 0)), before_xy[&slot(a, 0)], 1e-9, "A:0");
    assert_near(xy(&p, slot(a, 1)), before_xy[&slot(a, 1)], 1e-9, "A:1");
    assert_near(xy(&p, slot(a, 2)), [7.0, 12.0], 1e-9, "the new station");
    assert_near(
        xy(&p, slot(a, 3)),
        [7.0, 17.0],
        1e-9,
        "the old end, carried",
    );
}

// ===========================================================================
// Redo of a created shot keeps its uuid in every insert mode
// ===========================================================================

/// 0.6.0's `redo_create_shot_preserves_uuid_all_insert_modes`: append, insert
/// after and prepend each come back on redo with the shot's own uuid (the
/// identity cross-references and the Ariane export resolve by), and the redo
/// lands on the recorded state to the bit.
#[test]
fn a_redone_shot_keeps_its_uuid_in_every_insert_mode() {
    let mut p = empty("X");
    let a = free_line(&mut p, "A", [0.0, 0.0], north(2, 5.0));
    let o = open(&p);
    let ids = shot_ids(&o.state, a);
    let (first, last) = (ids[0], ids[ids.len() - 1]);

    for (mode, after, before) in [
        ("append", None, None),
        ("insert after", Some(last), None),
        ("prepend", None, Some(first)),
    ] {
        let created = create_shot(&o.state, a, shot(30.0, 3.0), after, before);
        let new_id = created.shot_id.unwrap();
        let p = project(&o.state);
        let uuid = p
            .line(a)
            .unwrap()
            .shots
            .iter()
            .find(|s| s.id == new_id)
            .unwrap()
            .uuid
            .clone();
        let at = uuid_lengths(&o.state, a);
        let state_after = snapshot(&o.state);

        assert!(o.state.undo().success, "{mode}: undo");
        assert!(
            !uuid_lengths(&o.state, a).iter().any(|(u, _)| *u == uuid),
            "{mode}: the undo took the shot"
        );
        assert!(o.state.redo().success, "{mode}: redo");
        assert_same(&snapshot(&o.state), &state_after, mode);
        assert_eq!(
            uuid_lengths(&o.state, a),
            at,
            "{mode}: every uuid on its shot"
        );
    }
}

// ===========================================================================
// #191: the read-only edit gate, the dirty bit, the chokepoint enumeration.
// A linked version 2 file opened without an edit session is read-only (no
// lock claim is bound to it, `speleodb_edit_gate`).
// ===========================================================================

/// Nothing listens here: every request fails fast.
const UNREACHABLE: &str = "http://127.0.0.1:1/";
const REMOTE_ID: &str = "11111111-1111-1111-1111-111111111111";

fn remote_link() -> SpeleoLink {
    link(UNREACHABLE, REMOTE_ID, "Remote Cave", Some("abc123"), false)
}

/// An open linked project with one two-shot line, read-only (opened without
/// a lock claim). Returns `(state, line, first shot, dir)`; keep `dir` alive.
fn remote_fixture() -> (AppState, LineId, ShotId, TempDir) {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("remote.stalx");
    let mut p = Project::new("Remote Cave", None);
    let line = free_line(
        &mut p,
        "Remote",
        [0.0, 0.0],
        vec![shot(45.0, 2.0), shot(45.0, 2.0)],
    );
    write_v2_project(&path, &p, Some(&remote_link()));
    let state = AppState::new().unwrap();
    assert!(state.open_project(path).success, "open the remote fixture");
    assert!(!state.edit_enabled(), "a linked project opens read-only");
    let first = shot_ids(&state, line)[0];
    (state, line, first, temp)
}

/// [`remote_fixture`] in an edit session (its claim bound).
fn remote_fixture_editable() -> (AppState, LineId, ShotId, TempDir) {
    let (state, line, first, temp) = remote_fixture();
    state.set_edit_enabled(true);
    assert!(state.edit_enabled());
    (state, line, first, temp)
}

/// The open project's SpeleoDB dirty bit (`has_unpushed_edits`), in memory.
fn dirty_bit(state: &AppState) -> bool {
    state.speleodb_open_link().unwrap().has_unpushed_edits
}

/// A local project with one line, created through the commands.
fn local_state() -> (AppState, LineId) {
    let state = AppState::new().unwrap();
    run(
        &state,
        Command::CreateProject {
            name: "Local".to_string(),
            location: None,
        },
    );
    let line = run(
        &state,
        Command::CreateLine {
            name: Some("S".to_string()),
            surveyor: None,
            date: None,
            color: "#abcdef".to_string(),
            direction: None,
            shots: vec![shot(45.0, 1.0)],
            cave_section_id: None,
        },
    )
    .line_id
    .unwrap();
    (state, line)
}

fn length_edit(line: LineId, shot_id: ShotId, length: f64) -> Command {
    Command::UpdateShot {
        line_id: line,
        shot_id,
        updates: ShotUpdates {
            length: Some(length),
            ..ShotUpdates::default()
        },
    }
}

#[test]
fn unwound_pushes_require_an_explicit_backend_acknowledgement() {
    let (state, _, _, _temp) = remote_fixture_editable();
    state.mark_remote_dirty();
    state.speleodb_set_unwound_from(Some("abc123"));

    let refused = state
        .speleodb_push_only_acknowledged(None, false)
        .expect_err("an unacknowledged older-version push fails closed");
    assert!(
        refused.message.contains("older unwound revision"),
        "unexpected refusal: {refused:?}"
    );
    let acknowledged = state
        .speleodb_push_only_acknowledged(None, true)
        .expect_err("nothing listens at the fixture's endpoint");
    assert!(
        !acknowledged.message.contains("older unwound revision"),
        "the explicit acknowledgement passes the older-version guard: {acknowledged:?}"
    );
}

#[test]
fn edit_enabled_defaults_true_and_local_project_passes() {
    let state = AppState::new().unwrap();
    assert!(state.edit_enabled(), "nothing open: the gate is open");
    let (state, line) = local_state();
    assert!(state.edit_enabled(), "a local project is always editable");
    let first = shot_ids(&state, line)[0];
    run(&state, length_edit(line, first, 3.0));
}

#[test]
fn remote_open_without_lock_disables_edit() {
    let (state, _, _, _temp) = remote_fixture();
    assert!(!state.edit_enabled());
    assert!(state.close_project().success);
    assert!(state.edit_enabled(), "close resets the gate");
}

#[test]
fn logout_leaves_local_project_editable() {
    // Logout writes the config (it clears the persisted account email), so it
    // needs a config file of its own.
    crate::test_helpers::with_isolated_config(|| {
        let (state, _) = local_state();
        state.speleodb_logout().expect("logout");
        assert!(
            state.edit_enabled(),
            "logout never disables editing on a local-only project"
        );
    });
}

/// How `execute` treats each command on a read-only remote project.
#[derive(Debug, PartialEq)]
enum Gate {
    /// Passes the gate: the project lifecycle (autosave must keep working).
    Lifecycle,
    /// Refused by `execute`: the command has its own gated entry point.
    OwnEntryPoint,
    /// Refused with the EDIT_DISABLED code before anything is read.
    Gated,
}

/// Exhaustive on purpose (no `_` arm): a new `Command` variant does not
/// compile until it is classified here, and a gated one must also get a probe
/// in [`gated_commands`] (whose variant count is pinned below).
fn gate_of(cmd: &Command) -> Gate {
    match cmd {
        Command::CreateProject { .. }
        | Command::OpenProject { .. }
        | Command::SaveProject { .. }
        | Command::CloseProject
        | Command::MergeProjects { .. } => Gate::Lifecycle,
        Command::RepairIntegrity
        | Command::UpdateProjectSettings
        | Command::SetSurfaceElevation => Gate::OwnEntryPoint,
        Command::CreateLine { .. }
        | Command::DeleteLine { .. }
        | Command::UpdateLineMetadata { .. }
        | Command::ReverseLine { .. }
        | Command::CreateShot { .. }
        | Command::UpdateShot { .. }
        | Command::DeleteShot { .. }
        | Command::SetShotKind { .. }
        | Command::EditShotCredit { .. }
        | Command::MergeStations { .. }
        | Command::UnmergeStations { .. }
        | Command::UnmergeStationPair { .. }
        | Command::CreateLoopClosure { .. }
        | Command::RemoveLoopClosure { .. }
        | Command::SetZeroDatum { .. }
        | Command::RemoveZeroDatum { .. }
        | Command::SetStationComment { .. }
        | Command::SetStationName { .. }
        | Command::MoveLine { .. }
        | Command::MoveSegment { .. }
        | Command::MoveStation { .. }
        | Command::MoveComment { .. }
        | Command::UntrackImport { .. }
        | Command::CreateCaveSection { .. }
        | Command::UpdateCaveSection { .. }
        | Command::DeleteCaveSection { .. }
        | Command::MoveLinesToCaveSection { .. }
        | Command::ApplyCaveSectionColor { .. }
        | Command::Batch { .. }
        | Command::AddProjectFile { .. } => Gate::Gated,
    }
}

/// The gated variants of [`gate_of`].
const GATED_VARIANTS: usize = 30;

/// One probe per gated `Command` variant (plus the Batch shapes the move and
/// multi-select tools send), named as the tripwire reports them.
fn gated_commands(line: LineId, first: ShotId) -> Vec<(&'static str, Command)> {
    let at = |i| StationRef::new(line, i);
    let offset = Coordinates::new(1.0, 2.0, 0.0);
    let gps = GpsCoordinates::new(20.5, -87.25, 0.0).unwrap();
    vec![
        (
            "execute:CreateLine",
            Command::CreateLine {
                name: Some("New".to_string()),
                surveyor: None,
                date: None,
                color: "#00ff00".to_string(),
                direction: None,
                shots: vec![shot(0.0, 3.0)],
                cave_section_id: None,
            },
        ),
        ("execute:DeleteLine", Command::DeleteLine { line_id: line }),
        (
            "execute:UpdateLineMetadata",
            Command::UpdateLineMetadata {
                line_id: line,
                updates: LineUpdates::default(),
            },
        ),
        (
            "execute:ReverseLine",
            Command::ReverseLine {
                line_id: line,
                swap_left_right: false,
            },
        ),
        (
            "execute:CreateShot",
            Command::CreateShot {
                line_id: line,
                shot: shot(0.0, 3.0),
                after_shot_id: None,
                before_shot_id: None,
            },
        ),
        ("execute", length_edit(line, first, 9.0)),
        (
            "execute:DeleteShot",
            Command::DeleteShot {
                line_id: line,
                shot_id: first,
            },
        ),
        (
            "execute:SetShotKind",
            Command::SetShotKind {
                line_id: line,
                shot_id: first,
                kind: ShotKind::Reference,
            },
        ),
        (
            "execute:EditShotCredit",
            Command::EditShotCredit {
                targets: crate::models::CreditTargets::Line { line_id: line },
                edit: crate::models::CreditEdit {
                    set_date: Some(None),
                    ..Default::default()
                },
            },
        ),
        (
            "execute:MergeStations",
            Command::MergeStations {
                station_a: at(0),
                station_b: at(2),
            },
        ),
        (
            "execute:UnmergeStations",
            Command::UnmergeStations { station_ref: at(0) },
        ),
        (
            "execute:UnmergeStationPair",
            Command::UnmergeStationPair {
                station_a: at(0),
                station_b: at(1),
            },
        ),
        (
            "execute:CreateLoopClosure",
            Command::CreateLoopClosure {
                station_a: at(0),
                station_b: at(2),
                closure_strategy: crate::models::ClosureStrategy::Batch,
            },
        ),
        (
            "execute:RemoveLoopClosure",
            Command::RemoveLoopClosure { closure_id: 7 },
        ),
        (
            "execute:SetZeroDatum",
            Command::SetZeroDatum {
                station_ref: at(0),
                coordinates: gps,
                accuracy_m: None,
                entrance: None,
            },
        ),
        (
            "execute:RemoveZeroDatum",
            Command::RemoveZeroDatum { station_ref: at(0) },
        ),
        (
            "execute:SetStationComment",
            Command::SetStationComment {
                station_ref: at(1),
                comment: Some("sump".to_string()),
            },
        ),
        (
            "execute:SetStationName",
            Command::SetStationName {
                station_ref: at(1),
                name: "X1".to_string(),
            },
        ),
        (
            "execute:MoveLine",
            Command::MoveLine {
                line_id: line,
                offset,
            },
        ),
        (
            "execute:MoveSegment",
            Command::MoveSegment {
                line_id: line,
                offset,
            },
        ),
        (
            "execute:MoveStation",
            Command::MoveStation {
                station_ref: at(1),
                offset,
            },
        ),
        (
            "execute:MoveComment",
            Command::MoveComment {
                source: CommentHolder::Shot {
                    line_id: line,
                    shot_id: first,
                },
                target: CommentHolder::Station { station_ref: at(1) },
                swap: false,
            },
        ),
        (
            "execute:UntrackImport",
            Command::UntrackImport {
                file_hashes: vec!["hash".to_string()],
                line_hashes: Vec::new(),
            },
        ),
        (
            "execute:CreateCaveSection",
            Command::CreateCaveSection {
                name: "Upper".to_string(),
                color: "#112233".to_string(),
                line_ids: vec![line],
            },
        ),
        (
            "execute:UpdateCaveSection",
            Command::UpdateCaveSection {
                section_id: 1,
                name: Some("Lower".to_string()),
                color: None,
            },
        ),
        (
            "execute:DeleteCaveSection",
            Command::DeleteCaveSection { section_id: 1 },
        ),
        (
            "execute:MoveLinesToCaveSection",
            Command::MoveLinesToCaveSection {
                line_ids: vec![line],
                section_id: None,
            },
        ),
        (
            "execute:ApplyCaveSectionColor",
            Command::ApplyCaveSectionColor { section_id: 1 },
        ),
        (
            "execute:Batch",
            Command::Batch {
                commands: vec![Command::MoveStation {
                    station_ref: at(1),
                    offset,
                }],
                label: None,
            },
        ),
        (
            "execute:Batch:ReverseLine",
            Command::Batch {
                commands: vec![Command::ReverseLine {
                    line_id: line,
                    swap_left_right: false,
                }],
                label: None,
            },
        ),
        // The gate runs before the file is read: a path that does not exist
        // still answers EDIT_DISABLED, not "file not found".
        (
            "execute:AddProjectFile",
            Command::add_project_file(PathBuf::from("/nonexistent/added.stalx")),
        ),
    ]
}

/// The chokepoint enumeration tripwire (D-B6): every survey-content mutation
/// entry point rejects on a read-only remote project with the EDIT_DISABLED
/// code (the undo-mode setting with its own words), and changes nothing.
///
/// When a new state-mutating entry is added, route it through
/// `guard_mutation`, add its name here and a call to the test below;
/// [`every_public_mutation_method_is_enumerated`] fails until the name is
/// here.
const MUTATION_ENTRYPOINTS: &[&str] = &[
    // `execute`, one probe per gated command variant (`gated_commands`).
    "execute",
    // History.
    "undo",
    "redo",
    "undo_to",
    "redo_to",
    // The recipes and bypass methods with their own lock.
    "update_project_settings",
    "set_surface_elevation",
    "execute_delete_shot_and_split",
    "execute_delete_shot_range_split",
    "execute_delete_shot_range_shift",
    "execute_delete_last_shot_and_line",
    "execute_split_shot_at_distance",
    "execute_split_line",
    "execute_combine_lines",
    "execute_create_and_merge",
    "execute_create_and_merge_in_cave_section",
    "execute_import_lines",
    // The DMP wizard's route (#814: the one import that still fails whole).
    "execute_import_lines_all_or_nothing",
    "execute_tml_import_sections",
    // Import tracking is a recorded edit now (C7).
    "track_imported_file",
    "track_line_hashes",
    // By path: gated when the path is the OPEN project.
    "set_project_name_for_path",
    // Warning ignores ride the push (#197).
    "save_resolved_warning_gated",
    "delete_resolved_warning_gated",
    // The SpeleoDB undo-history setting needs an edit session.
    "speleodb_set_undo_mode",
];

fn assert_disabled(label: &str, result: &CommandResult) {
    assert!(
        !result.success,
        "{label} must be refused when editing is off"
    );
    assert!(
        result
            .error
            .as_deref()
            .is_some_and(|e| e.starts_with(EDIT_DISABLED_CODE)),
        "{label} must carry the EDIT_DISABLED code, got: {:?}",
        result.error
    );
}

#[test]
fn gate_rejects_every_mutation_entrypoint_with_edit_disabled_code() {
    assert_eq!(
        MUTATION_ENTRYPOINTS.len(),
        25,
        "update both the routing and this list when adding a mutation entry"
    );

    // execute(): every gated variant, probed on one read-only state.
    {
        let (state, line, first, _temp) = remote_fixture();
        let before = snapshot(&state);
        let probes = gated_commands(line, first);
        let variants: HashSet<_> = probes
            .iter()
            .map(|(_, cmd)| std::mem::discriminant(cmd))
            .collect();
        assert_eq!(
            variants.len(),
            GATED_VARIANTS,
            "one probe per gated command variant"
        );
        for (label, cmd) in probes {
            assert_eq!(gate_of(&cmd), Gate::Gated, "{label}");
            assert_disabled(label, &state.execute(cmd));
        }
        assert_same(&snapshot(&state), &before, "the refusals changed nothing");
        assert!(state.get_undo_stack().is_empty(), "and recorded nothing");
        assert!(!dirty_bit(&state), "and dirtied nothing");
    }

    // History.
    {
        let (state, _, _, _temp) = remote_fixture();
        assert_disabled("undo", &state.undo());
        assert_disabled("redo", &state.redo());
        assert_disabled("undo_to", &state.undo_to(0));
        assert_disabled("redo_to", &state.redo_to(0));
    }

    // The recipes and bypass methods.
    {
        let (state, line, first, _temp) = remote_fixture();
        let before = snapshot(&state);
        let at = |i| StationRef::new(line, i);
        assert_disabled(
            "update_project_settings",
            &state.update_project_settings("Renamed".to_string(), state.get_config().validation),
        );
        assert_disabled(
            "set_surface_elevation",
            &state.set_surface_elevation(Some(12.0)),
        );
        assert_disabled(
            "execute_delete_shot_and_split",
            &state.execute_delete_shot_and_split(line, first, "A".to_string(), "B".to_string()),
        );
        assert_disabled(
            "execute_delete_shot_range_split",
            &state.execute_delete_shot_range_split(
                line,
                first,
                first,
                "A".to_string(),
                "B".to_string(),
            ),
        );
        assert_disabled(
            "execute_delete_shot_range_shift",
            &state.execute_delete_shot_range_shift(line, first, first),
        );
        assert_disabled(
            "execute_delete_last_shot_and_line",
            &state.execute_delete_last_shot_and_line(line, first),
        );
        assert_disabled(
            "execute_split_shot_at_distance",
            &state.execute_split_shot_at_distance(line, first, 1.0),
        );
        assert_disabled(
            "execute_split_line",
            &state.execute_split_line(
                line,
                crate::models::SplitAt::Station { station_index: 1 },
                crate::models::SplitLineOptions {
                    name_a: "A".to_string(),
                    name_b: "B".to_string(),
                    color_a: "#111111".to_string(),
                    color_b: "#222222".to_string(),
                    comment_side: crate::models::SplitSide::A,
                    datum_side: crate::models::SplitSide::A,
                },
            ),
        );
        assert_disabled(
            "execute_combine_lines",
            &state.execute_combine_lines(
                line,
                line + 1,
                crate::models::CombineLinesOptions {
                    name: "A".to_string(),
                    color: "#111111".to_string(),
                    swap_lr: false,
                    comment_pick: crate::models::CombinePick::Kept,
                    name_pick: crate::models::CombinePick::Kept,
                    join: None,
                },
            ),
        );
        assert_disabled(
            "execute_create_and_merge",
            &state.execute_create_and_merge(
                Some("Imp".to_string()),
                None,
                None,
                "#00ff00".to_string(),
                None,
                vec![shot(0.0, 3.0)],
                0,
                at(1),
                crate::models::ClosureStrategy::Batch,
            ),
        );
        assert_disabled(
            "execute_create_and_merge_in_cave_section",
            &state.execute_create_and_merge_in_cave_section(
                Some("Imp".to_string()),
                None,
                None,
                "#00ff00".to_string(),
                None,
                vec![shot(0.0, 3.0)],
                0,
                at(1),
                crate::models::ClosureStrategy::Batch,
                None,
            ),
        );
        assert_disabled(
            "execute_import_lines",
            &state.execute_import_lines(
                vec![import_config("Imp")],
                Vec::new(),
                None,
                Vec::new(),
                Vec::new(),
                crate::core::VirtualShotMode::Auto,
            ),
        );
        assert_disabled(
            "execute_import_lines_all_or_nothing",
            &state.execute_import_lines_all_or_nothing(
                vec![import_config("Imp")],
                None,
                Vec::new(),
                Vec::new(),
                crate::core::VirtualShotMode::Auto,
            ),
        );
        assert_disabled(
            "execute_tml_import_sections",
            &state.execute_tml_import_sections(
                vec![import_config("Imp")],
                Vec::new(),
                None,
                Vec::new(),
                Vec::new(),
                crate::core::VirtualShotMode::Auto,
                Some(10.0),
            ),
        );
        assert_disabled(
            "track_imported_file",
            &state.track_imported_file("hash".to_string()),
        );
        assert_disabled(
            "track_line_hashes",
            &state.track_line_hashes(vec!["hash".to_string()]),
        );
        // Repair has no coded failure surface; there is nothing to repair on
        // the station graph, and it changes nothing on a read-only project.
        let repair = state.repair_open_project_integrity();
        assert!(!repair.changed, "repair on a read-only remote is a no-op");
        assert_same(&snapshot(&state), &before, "the refusals changed nothing");
        assert!(state.get_undo_stack().is_empty());
        assert!(!dirty_bit(&state));
    }

    // By path: gated only when the path is the open project.
    {
        let (state, _, _, temp) = remote_fixture();
        let open_path = state.open_project_path().unwrap();
        let before = std::fs::read(&open_path).unwrap();
        assert_disabled(
            "set_project_name_for_path",
            &state.set_project_name_for_path(open_path.clone(), "Renamed Remote".to_string()),
        );
        assert_eq!(
            std::fs::read(&open_path).unwrap(),
            before,
            "the file is untouched"
        );
        drop(temp);
    }

    // The ignore verbs answer `Result<(), String>` with the code as the
    // prefix, exactly as the frontend's read-only handler matches it.
    {
        let (state, line, first, _temp) = remote_fixture();
        let warning = resolved(line, first);
        let resolve = state
            .save_resolved_warning_gated(&warning)
            .expect_err("resolve is refused when editing is off");
        assert!(resolve.starts_with(EDIT_DISABLED_CODE), "{resolve:?}");
        let unresolve = state
            .delete_resolved_warning_gated(line, first, "SHORT_SHOT", None)
            .expect_err("unresolve is refused when editing is off");
        assert!(unresolve.starts_with(EDIT_DISABLED_CODE), "{unresolve:?}");
        assert!(
            !dirty_bit(&state),
            "a refused ignore does not dirty the project"
        );
        assert!(
            state.load_resolved_warnings().unwrap().is_empty(),
            "and stores nothing"
        );
    }

    // The undo-history setting: refused in its own words, link unchanged.
    {
        let (state, _, _, _temp) = remote_fixture();
        let mode = state.speleodb_open_link().unwrap().undo_mode;
        assert_ne!(mode, Some(UndoMode::Disabled), "setup");
        let refused = state
            .speleodb_set_undo_mode(UndoMode::Disabled)
            .expect_err("the setting needs an edit session");
        assert!(refused.message.contains("start editing"), "{refused:?}");
        assert_eq!(state.speleodb_open_link().unwrap().undo_mode, mode);
        assert!(!dirty_bit(&state));
    }
}

/// Every public method of the `state` module that looks like a mutation
/// entry point (`execute_*`, `track_*`, `set_*_for_path`, `*_gated`) is in
/// [`MUTATION_ENTRYPOINTS`], so a new one cannot be added without the
/// tripwire knowing (the follow-up the 0.6.0 list asked for: a hand list
/// alone does not see a new method); and every name in the list is a public
/// method that exists (a rename cannot leave a stale entry).
#[test]
fn every_public_mutation_method_is_enumerated() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/core/state");
    let mut found = BTreeSet::new();
    let mut public = BTreeSet::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let is_test = name == "tests.rs"
            || name.ends_with("_tests.rs")
            || name.ends_with("_kit.rs")
            || name.ends_with("_fixtures.rs")
            || name == "tests_support.rs";
        if path.extension().is_none_or(|ext| ext != "rs") || is_test {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for line in text.lines() {
            let Some(rest) = line.trim_start().strip_prefix("pub fn ") else {
                continue;
            };
            let method: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            public.insert(method.clone());
            let looks_like_a_mutation = method.starts_with("execute_")
                || method.starts_with("track_")
                || (method.starts_with("set_") && method.ends_with("_for_path"))
                || method.ends_with("_gated");
            if looks_like_a_mutation {
                found.insert(method);
            }
        }
    }
    assert!(
        found.contains("execute_split_line") && found.contains("track_line_hashes"),
        "the scan sees the state module: {found:?}"
    );
    let missing: Vec<_> = found
        .iter()
        .filter(|m| !MUTATION_ENTRYPOINTS.contains(&m.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "route these through guard_mutation and add them to MUTATION_ENTRYPOINTS: {missing:?}"
    );
    let stale: Vec<_> = MUTATION_ENTRYPOINTS
        .iter()
        .filter(|m| !public.contains(**m))
        .collect();
    assert!(stale.is_empty(), "no such public method: {stale:?}");
}

fn import_config(name: &str) -> crate::models::ImportLineConfig {
    crate::models::ImportLineConfig {
        cave_section: None,
        reverse: false,
        line_index: 0,
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![shot(0.0, 3.0)],
        station_comments: Vec::new(),
        station_names: Vec::new(),
        station_external: Vec::new(),
        placement: crate::models::ImportPlacement::Origin,
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    }
}

fn resolved(line: LineId, shot_id: ShotId) -> ResolvedWarning {
    ResolvedWarning {
        line_id: line,
        shot_id,
        warning_code: "SHORT_SHOT".to_string(),
        resolved_at: chrono::Utc::now(),
        resolved_comment: None,
        shot_uuid: None,
    }
}

#[test]
fn lifecycle_commands_pass_through_gate() {
    let (state, _, _, _temp) = remote_fixture();
    let save = state.execute(Command::SaveProject { path: None });
    assert!(save.success, "autosave passes the gate: {:?}", save.error);
    assert!(!dirty_bit(&state), "a local save never sets the dirty bit");
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.edit_enabled(), "and close resets the gate");
}

/// #197: an ignore is pushed, so in an edit session the gated verbs dirty
/// the project for the push; the ungated low-level verb does not.
#[test]
fn warning_resolve_unresolve_in_edit_session_dirty_for_push() {
    {
        let (state, line, first, _temp) = remote_fixture_editable();
        assert!(!dirty_bit(&state), "the fixture starts clean");
        state
            .save_resolved_warning_gated(&resolved(line, first))
            .expect("resolve is allowed in an edit session");
        assert!(
            dirty_bit(&state),
            "an edit-session resolve dirties the project"
        );
    }
    {
        let (state, line, first, _temp) = remote_fixture_editable();
        state
            .save_resolved_warning(&resolved(line, first))
            .expect("seed an ignore");
        assert!(
            !dirty_bit(&state),
            "the ungated low-level save_resolved_warning leaves the dirty bit alone"
        );
        state
            .delete_resolved_warning_gated(line, first, "SHORT_SHOT", None)
            .expect("unresolve is allowed in an edit session");
        assert!(
            dirty_bit(&state),
            "an edit-session unresolve dirties the project"
        );
    }
}

#[test]
fn dirty_bit_flips_false_to_true_on_successful_remote_edit() {
    let (state, line, first, _temp) = remote_fixture_editable();
    assert!(!dirty_bit(&state));
    run(&state, length_edit(line, first, 9.0));
    assert!(
        dirty_bit(&state),
        "a successful remote edit sets the dirty bit"
    );
}

/// Set-only and idempotent: a second edit and a local save leave it set, and
/// the save writes it (a crash after the edit still knows the copy has
/// unpushed work).
#[test]
fn dirty_bit_is_idempotent_and_set_only() {
    let (state, line, first, _temp) = remote_fixture_editable();
    run(&state, length_edit(line, first, 9.0));
    assert!(dirty_bit(&state));
    run(&state, length_edit(line, first, 8.0));
    run(&state, Command::SaveProject { path: None });
    assert!(dirty_bit(&state), "the dirty bit stays set after a save");
    let path = state.open_project_path().unwrap();
    let on_disk = super::synthetic_stalx_tests::link_on_disk(&path).unwrap();
    assert!(on_disk.has_unpushed_edits, "and the file says so");
}

/// Only the flip to dirty rewrites the file (C11): the second edit of a
/// linked project leaves the file as the first one left it (the flag is on
/// disk already), its new edit time is kept in memory as the push fence, and
/// the next save writes it with the survey it dates. Injection: persist on
/// every edit again and the bytes after the second edit differ.
#[test]
fn only_the_flip_to_dirty_rewrites_a_linked_file() {
    let (state, line, first, _temp) = remote_fixture_editable();
    let path = state.open_project_path().unwrap();
    run(&state, length_edit(line, first, 9.0));
    let on_disk = super::synthetic_stalx_tests::link_on_disk(&path).unwrap();
    assert!(on_disk.has_unpushed_edits, "the flip is durable at once");
    let after_first = std::fs::read(&path).unwrap();
    let stamped = state.speleodb_open_link().unwrap().last_edit_at;
    std::thread::sleep(std::time::Duration::from_millis(5));
    run(&state, length_edit(line, first, 8.0));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        after_first,
        "a second edit does not rewrite the file"
    );
    let live = state.speleodb_open_link().unwrap();
    assert!(live.has_unpushed_edits);
    assert_ne!(live.last_edit_at, stamped, "the edit time moves in memory");
    run(&state, Command::SaveProject { path: None });
    let saved = super::synthetic_stalx_tests::link_on_disk(&path).unwrap();
    assert!(saved.has_unpushed_edits);
    assert_eq!(
        super::synthetic_stalx_tests::instant(saved.last_edit_at.as_deref()),
        super::synthetic_stalx_tests::instant(live.last_edit_at.as_deref()),
        "the save writes the latest edit time"
    );
}

#[test]
fn dirty_bit_noop_for_local_only_project() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("local.stalx");
    let mut p = Project::new("Local", None);
    let line = free_line(&mut p, "S", [0.0, 0.0], vec![shot(45.0, 2.0)]);
    write_v2_project(&path, &p, None);
    let state = AppState::new().unwrap();
    assert!(state.open_project(path.clone()).success);
    assert!(state.edit_enabled(), "a local project stays editable");
    let first = shot_ids(&state, line)[0];
    run(&state, length_edit(line, first, 5.0));
    assert!(state.speleodb_open_link().is_err(), "still no link");
    run(&state, Command::SaveProject { path: None });
    assert!(
        decoded(&path).extras.speleodb.is_none(),
        "the save writes no link either"
    );
}

/// The EDIT_DISABLED code is distinct from the filesystem permission code,
/// so the frontend can match it on its own (#204).
#[test]
fn edit_disabled_code_prefix_is_distinct() {
    assert_eq!(EDIT_DISABLED_CODE, "EDIT_DISABLED");
    assert_ne!(
        EDIT_DISABLED_CODE,
        crate::core::loading::PERMISSION_DENIED_CODE
    );
    let (state, line, first, _temp) = remote_fixture();
    let error = state.execute(length_edit(line, first, 1.0)).error.unwrap();
    assert!(error.starts_with(EDIT_DISABLED_CODE));
    assert!(!error.starts_with(crate::core::loading::PERMISSION_DENIED_CODE));
}

#[test]
fn repair_and_settings_gated_on_locked_remote() {
    let (state, _, _, _temp) = remote_fixture();
    let repair = state.repair_open_project_integrity();
    assert!(!repair.changed);
    assert_eq!(repair.fixed, 0);
    let settings =
        state.update_project_settings("New Name".to_string(), state.get_config().validation);
    assert_disabled("update_project_settings", &settings);
    assert_eq!(project(&state).name, "Remote Cave");
}

/// A read-only refusal names the command the way the history does (lines
/// and stations by name), never by a positional id (`Update shot 3:0`,
/// `Remove loop closure #7`); a batch refusal likewise.
#[test]
fn a_read_only_refusal_names_the_command_without_positional_ids() {
    let (state, line, first, _temp) = remote_fixture();
    let name = project(&state).line(line).unwrap().name.clone().unwrap();
    let refusal = |cmd: Command| state.execute(cmd).error.unwrap_or_default();
    let update = refusal(length_edit(line, first, 1.0));
    assert!(
        update.contains(&format!("Update shot in line \"{name}\" is disabled")),
        "{update}"
    );
    let merge = refusal(Command::MergeStations {
        station_a: StationRef::new(line, 0),
        station_b: StationRef::new(line, 2),
    });
    assert!(
        merge.contains(&format!(
            "Merge stations {name} / 0 and {name} / 2 is disabled"
        )),
        "{merge}"
    );
    let closure = refusal(Command::RemoveLoopClosure { closure_id: 7 });
    assert!(
        closure.contains("Remove loop closure is disabled"),
        "{closure}"
    );
    for text in [&update, &merge, &closure] {
        assert!(!text.contains(&format!("{line}:")), "{text}");
        assert!(!text.contains('#'), "{text}");
    }

    state.set_edit_enabled(true);
    let batch = refusal(Command::Batch {
        commands: vec![Command::SetStationComment {
            station_ref: StationRef::new(line, 1),
            comment: Some("sump".to_string()),
        }],
        label: None,
    });
    assert_eq!(
        batch,
        format!("Command 'Set comment on station {name} / 1' is not allowed in batch context")
    );
}

// ===========================================================================
// Save a Copy: an explicit destination on a LINKED project writes a detached,
// unlinked copy and leaves the session on the managed file.
// ===========================================================================

#[test]
fn save_as_on_a_linked_project_writes_an_unlinked_copy_and_stays_put() {
    let (state, line, _, temp) = remote_fixture();
    let open_path = state.open_project_path().unwrap();
    let copy_path = temp.path().join("copy.stalx");

    let result = state.save_project(Some(copy_path.clone()));

    assert!(result.success, "copy save: {:?}", result.error);
    assert_eq!(
        result.unlinked_copy_path.as_deref(),
        Some(copy_path.display().to_string().as_str()),
        "the copy is reported through unlinked_copy_path"
    );
    assert!(
        result.saved_path.is_none(),
        "saved_path stays None: the frontend re-points file_path on it"
    );
    assert_eq!(state.open_project_path().unwrap(), open_path);
    assert!(
        state.speleodb_open_link().is_ok(),
        "the managed file keeps its link"
    );
    let copy = decoded(&copy_path);
    assert!(copy.extras.speleodb.is_none(), "the copy carries no link");
    assert!(
        copy.extras.local.speleodb.is_empty(),
        "nor the link's sync state"
    );
    assert_eq!(copy.project.lines().len(), 1, "the survey is in the copy");
    assert_eq!(copy.project.line(line).unwrap().shots.len(), 2);
}

/// 0.6.0 refused an explicit destination that IS the managed file (writing
/// the unlinked copy there would strip its link in place). On v2 that
/// destination is a normal save (plan deviation C6): the link stays.
#[test]
fn save_as_onto_the_managed_file_itself_is_a_normal_save_that_keeps_the_link() {
    let (state, _, _, _temp) = remote_fixture();
    let open_path = state.open_project_path().unwrap();

    let result = state.save_project(Some(open_path.clone()));

    assert!(result.success, "{:?}", result.error);
    assert!(result.unlinked_copy_path.is_none(), "not a copy save");
    let on_disk = super::synthetic_stalx_tests::link_on_disk(&open_path);
    assert_eq!(
        on_disk.map(|l| l.project_id),
        Some(REMOTE_ID.to_string()),
        "the managed file keeps its link"
    );
    assert_eq!(state.open_project_path().unwrap(), open_path);
}

/// Pins the upload's pre-push flush (`save_project(None)`): an implicit save
/// is never a copy save; it keeps the link and the dirty bit.
#[test]
fn implicit_save_on_a_linked_project_still_carries_the_link_and_the_dirty_bit() {
    let (state, line, first, _temp) = remote_fixture_editable();
    run(&state, length_edit(line, first, 9.0));
    assert!(dirty_bit(&state));

    let result = state.save_project(None);

    assert!(result.success, "{:?}", result.error);
    assert!(
        result.saved_path.is_some(),
        "an implicit save reports saved_path"
    );
    assert!(result.unlinked_copy_path.is_none());
    assert!(
        state.speleodb_open_link().is_ok(),
        "the link survived the save"
    );
    assert!(
        dirty_bit(&state),
        "a local save never clears the dirty bit, only a push does"
    );
    let path = state.open_project_path().unwrap();
    assert!(
        super::synthetic_stalx_tests::link_on_disk(&path)
            .unwrap()
            .has_unpushed_edits
    );
}

/// The Save As contract of a plain local project: it relocates the session.
#[test]
fn save_as_on_an_unlinked_project_still_relocates_the_session() {
    let temp = TempDir::new().unwrap();
    let (state, _) = local_state();
    let dest = temp.path().join("relocated.stalx");

    let result = state.save_project(Some(dest.clone()));

    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        result.saved_path.as_deref(),
        Some(dest.display().to_string().as_str()),
        "an unlinked Save As reports saved_path"
    );
    assert!(result.unlinked_copy_path.is_none());
    assert_eq!(state.open_project_path().unwrap(), dest);
}

/// The detach flow reopens the copy and must not lose the edit trail.
#[test]
fn the_copy_carries_the_undo_history() {
    let (state, line, first, temp) = remote_fixture_editable();
    run(&state, length_edit(line, first, 9.0));
    let copy_path = temp.path().join("copy.stalx");
    let result = state.save_project(Some(copy_path.clone()));
    assert!(result.success, "{:?}", result.error);
    assert_eq!(decoded(&copy_path).extras.history.undo.len(), 1);

    let copy = AppState::new().unwrap();
    assert!(copy.open_project(copy_path).success);
    assert_eq!(copy.get_undo_stack().len(), 1);
    assert!(copy.undo().success, "the copy's edit undoes");
    assert_eq!(uuid_lengths(&copy, line)[0].1, 2.0);
}

// ===========================================================================
// What is left of "wedge healing" (#413 Q8): no entry can be unappliable on
// the changeset engine, so the wedges are gone; the redo-branch rules stay.
// ===========================================================================

/// A command that changes nothing records nothing and keeps the redo branch,
/// and the redo still lands exactly (the state it replays onto is the one it
/// was recorded on). 0.6.0 cleared the branch here, because its epilogue
/// reconciled derived state without an entry; nothing derived is stored now
/// (plan C0 pipeline rule: an empty changeset is a no-op).
#[test]
fn a_command_that_changes_nothing_keeps_the_redo_branch_and_its_redo_is_exact() {
    let state = AppState::new().unwrap();
    run(
        &state,
        Command::CreateProject {
            name: "Redo Keep".to_string(),
            location: None,
        },
    );
    let mut ids = Vec::new();
    for name in ["A", "B", "C"] {
        let created = run(
            &state,
            Command::CreateLine {
                name: Some(name.to_string()),
                surveyor: None,
                date: None,
                color: "#4ecdc4".to_string(),
                direction: None,
                shots: vec![shot(0.0, 5.0)],
                cave_section_id: None,
            },
        );
        ids.push(created.line_id.unwrap());
    }
    let all = snapshot(&state);
    assert!(state.undo().success);
    assert!(state.undo().success);
    let depth = state.get_undo_stack().len();

    // A:0 is on no junction: Unmerge is a no-op.
    run(
        &state,
        Command::UnmergeStations {
            station_ref: StationRef::new(ids[0], 0),
        },
    );
    assert_eq!(state.get_undo_stack().len(), depth, "no entry");
    assert!(state.can_redo(), "the redo branch is kept");
    assert!(state.redo().success);
    assert!(state.redo().success);
    assert_same(&snapshot(&state), &all, "the kept branch redoes exactly");
}

/// #24: a reverse of a shot-less line is a provable no-op. It keeps the redo
/// branch, and on a linked project in an edit session it stamps no phantom
/// edit (`has_unpushed_edits` would proliferate it to SpeleoDB).
#[test]
fn a_shotless_reverse_keeps_the_redo_branch_and_stamps_no_remote_edit() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("remote.stalx");
    let mut p = Project::new("Remote Cave", None);
    let waypoint = free_line(&mut p, "Waypoint", [0.0, 0.0], Vec::new());
    let real = free_line(&mut p, "Real", [5.0, 0.0], vec![shot(0.0, 5.0)]);
    write_v2_project(&path, &p, Some(&remote_link()));
    let state = AppState::new().unwrap();
    assert!(state.open_project(path).success);
    state.set_edit_enabled(true);
    run(&state, length_edit(real, shot_ids(&state, real)[0], 6.0));
    assert!(state.undo().success);
    state.with_file(|file| {
        let mut link = file.speleo_link().unwrap();
        link.has_unpushed_edits = false;
        file.set_speleo_link(Some(&link));
    });
    assert!(!dirty_bit(&state), "setup: clean");
    let before = snapshot(&state);

    run(
        &state,
        Command::ReverseLine {
            line_id: waypoint,
            swap_left_right: false,
        },
    );

    assert!(
        state.can_redo(),
        "a provable no-op leaves the redo branch alone"
    );
    assert!(!dirty_bit(&state), "and stamps no edit for the push");
    assert_same(&snapshot(&state), &before, "and changes nothing");
}

// ===========================================================================
// `AppState::update_config` normalization (#357 item 1)
// ===========================================================================

#[test]
fn test_update_config_repairs_a_degenerate_whole_config_payload() {
    // A whole-`Config` payload with junk in unrelated lines is repaired on the
    // way in, as the load path repairs a hand-edited config.toml, not
    // persisted as is for the next launch to fix.
    crate::test_helpers::with_isolated_config(|| {
        let state = AppState::new().unwrap();
        let mut payload = state.get_config();
        payload.panels.sections_panel_width = 5; // out of the 150..400 bound
        payload.placement_suggestion.azimuth_sigma = f64::NAN;
        payload.general.locale = "   ".to_string(); // blank after trim

        state.update_config(payload).expect("settings saved");

        let after = state.get_config();
        assert_eq!(after.panels.sections_panel_width, 150);
        assert_eq!(
            after.placement_suggestion.azimuth_sigma,
            crate::config::default_ps_azimuth_sigma()
        );
        assert_eq!(after.general.locale, "system");
    });
}

#[test]
fn test_update_config_leaves_an_explicitly_cleared_shortcut_empty() {
    // `normalize_shortcut_field` reads an EMPTY binding list as "missing from
    // the file" and re-seeds the stock binding: right on load, wrong on write
    // (the Settings UI sends an empty list when the user clears the last
    // binding of an action). `update_config` keeps a deliberate clear.
    crate::test_helpers::with_isolated_config(|| {
        let state = AppState::new().unwrap();
        let mut payload = state.get_config();
        assert!(
            !payload.shortcuts.select.is_empty(),
            "setup: a stock binding"
        );
        payload.shortcuts.select = Vec::new();

        state.update_config(payload).expect("settings saved");

        assert!(state.get_config().shortcuts.select.is_empty());
    });

    // The load path's plain `normalize()` does re-seed an empty binding.
    let mut loaded = crate::config::Config::default();
    loaded.shortcuts.select = Vec::new();
    loaded.normalize();
    assert_eq!(
        loaded.shortcuts.select,
        crate::config::Config::default().shortcuts.select
    );
}

// ===========================================================================
// #413 finding 26: a shot edit never re-georeferences the survey
// ===========================================================================

/// Every slot's raw position, through the app.
fn slot_xy(state: &AppState) -> std::collections::BTreeMap<StationRef, [f64; 2]> {
    raw_xy(&project(state))
}

/// 0.6.0's reduction: removing the FIRST datum re-defined the frame without
/// moving anything, and the next geometry edit in the promoted group
/// translated the whole group onto the new frame, unundoably. Positions are
/// derived now (the frame change is measured once, plan 4.3): the removal
/// puts the promoted datum on the origin, the shot edit moves only what is
/// downstream of the edited shot, and the undo restores every slot.
#[test]
fn finding_26_a_shot_edit_does_not_translate_a_group_whose_first_datum_was_removed() {
    let state = AppState::new().unwrap();
    run(
        &state,
        Command::CreateProject {
            name: "datum frame".to_string(),
            location: None,
        },
    );
    let line = |name: &str| {
        run(
            &state,
            Command::CreateLine {
                name: Some(name.to_string()),
                surveyor: None,
                date: None,
                color: "#111111".to_string(),
                direction: None,
                shots: vec![shot(30.0, 5.0), shot(60.0, 5.0)],
                cave_section_id: None,
            },
        )
        .line_id
        .unwrap()
    };
    let (anchor, a, b) = (line("anchor"), line("a"), line("b"));
    run(
        &state,
        Command::MergeStations {
            station_a: StationRef::new(a, 2),
            station_b: StationRef::new(b, 0),
        },
    );
    let datum = |at: StationRef, lat: f64, lon: f64| {
        run(
            &state,
            Command::SetZeroDatum {
                station_ref: at,
                coordinates: GpsCoordinates::new(lat, lon, 0.0).unwrap(),
                accuracy_m: None,
                entrance: None,
            },
        )
    };
    datum(StationRef::new(anchor, 0), 20.30, -87.45);
    datum(StationRef::new(a, 0), 20.20, -87.24);
    let placed = slot_xy(&state)[&slot(a, 0)];
    assert!(
        placed[0].abs() > 1000.0,
        "the second datum places the group kilometres out: {placed:?}"
    );

    run(
        &state,
        Command::RemoveZeroDatum {
            station_ref: StationRef::new(anchor, 0),
        },
    );
    let promoted = slot_xy(&state);
    assert_near(
        promoted[&slot(a, 0)],
        [0.0, 0.0],
        1e-6,
        "the promoted datum's station sits on the new origin",
    );
    let before = snapshot(&state);

    let second = shot_ids(&state, a)[1];
    run(&state, length_edit(a, second, 6.0));

    let edited = slot_xy(&state);
    for at in [
        slot(a, 0),
        slot(a, 1),
        slot(anchor, 0),
        slot(anchor, 1),
        slot(anchor, 2),
    ] {
        assert_near(
            edited[&at],
            promoted[&at],
            1e-6,
            &format!("{at:?}: a shot edit re-georeferenced the survey"),
        );
    }
    assert!(
        super::topology_kit::apart(edited[&slot(a, 2)], promoted[&slot(a, 2)]) > 0.5,
        "the edit moved what is downstream of it"
    );

    assert!(state.undo().success);
    assert_same_xy(&slot_xy(&state), &promoted);
    assert_same(&snapshot(&state), &before, "the undo restores every slot");
}

// ===========================================================================
// #413 finding 7, second half: a jump lands where single steps land
// ===========================================================================

/// A survey with a history of six entries: two lines, a datum, a merge and
/// two station moves. Returns the state, the snapshots before each entry
/// (index 0: the empty project) and the line ids.
fn jump_history() -> (AppState, Vec<super::topology_kit::Snapshot>) {
    let state = AppState::new().unwrap();
    run(
        &state,
        Command::CreateProject {
            name: "Jumps".to_string(),
            location: None,
        },
    );
    let mut states = vec![snapshot(&state)];
    let mut line = |name: &str, shots: Vec<ShotData>| {
        let id = run(
            &state,
            Command::CreateLine {
                name: Some(name.to_string()),
                surveyor: None,
                date: None,
                color: "#3388FF".to_string(),
                direction: None,
                shots,
                cave_section_id: None,
            },
        )
        .line_id
        .unwrap();
        states.push(snapshot(&state));
        id
    };
    let a = line("A", north(2, 5.0));
    let b = line("B", vec![shot(90.0, 5.0)]);
    for cmd in [
        Command::SetZeroDatum {
            station_ref: StationRef::new(a, 0),
            coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
            accuracy_m: None,
            entrance: None,
        },
        Command::MergeStations {
            station_a: StationRef::new(a, 2),
            station_b: StationRef::new(b, 0),
        },
        Command::MoveStation {
            station_ref: StationRef::new(a, 1),
            offset: Coordinates::new(1.0, 2.0, 0.0),
        },
        Command::MoveStation {
            station_ref: StationRef::new(a, 1),
            offset: Coordinates::new(1.0, 2.0, 0.0),
        },
    ] {
        run(&state, cmd);
        states.push(snapshot(&state));
    }
    assert_eq!(state.get_undo_stack().len(), 6);
    (state, states)
}

/// In the session: `undo_to(2)` (three steps) lands on the state three
/// single undos land on, and `redo_to(2)` back on the state before them.
#[test]
fn undo_to_and_redo_to_land_where_the_same_number_of_single_steps_land() {
    let (state, states) = jump_history();
    let jump = state.undo_to(2);
    assert!(jump.success, "{:?}", jump.error);
    assert_same(&snapshot(&state), &states[3], "undo_to(2)");
    assert_eq!(state.get_undo_stack().len(), 3);
    let jump = state.redo_to(2);
    assert!(jump.success, "{:?}", jump.error);
    assert_same(&snapshot(&state), &states[6], "redo_to(2)");

    // And the single steps land on the same states.
    for depth in (3..6).rev() {
        assert!(state.undo().success);
        assert_same(&snapshot(&state), &states[depth], "single undo");
    }
}

/// The same through the file: two sessions opened from one saved history,
/// one jumping and one stepping, end on the same survey to the bit (the
/// decoded entries mint the same session ids in both).
#[test]
fn a_jump_and_single_steps_over_a_reopened_history_reach_the_same_survey() {
    let (state, _) = jump_history();
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("jumps.stalx");
    run(
        &state,
        Command::SaveProject {
            path: Some(path.clone()),
        },
    );
    let twin = || {
        let s = AppState::new().unwrap();
        assert!(s.open_project(path.clone()).success);
        assert_eq!(s.get_undo_stack().len(), 6);
        s
    };
    let (jumped, stepped) = (twin(), twin());
    assert_same(
        &snapshot(&jumped),
        &snapshot(&stepped),
        "the twins start equal",
    );

    assert!(jumped.undo_to(2).success);
    for _ in 0..3 {
        assert!(stepped.undo().success);
    }
    assert_same(&snapshot(&jumped), &snapshot(&stepped), "three steps back");

    assert!(jumped.redo_to(1).success);
    for _ in 0..2 {
        assert!(stepped.redo().success);
    }
    assert_same(&snapshot(&jumped), &snapshot(&stepped), "two steps forward");
    assert_eq!(
        jumped.get_undo_stack().len(),
        stepped.get_undo_stack().len()
    );
}

// ===========================================================================
// #413 finding 24: a line's session shot ids never overflow or repeat
// ===========================================================================

/// The ids of `line`'s shots are distinct, inside the line's id window, and
/// below its allocator, which is at most the window's end.
fn assert_ids_sound(state: &AppState, line: LineId, what: &str) {
    let p = project(state);
    let l = p.line(line).unwrap();
    let base = line * FEATURE_ID_STRIDE;
    let end = base + FEATURE_ID_STRIDE;
    let ids: Vec<ShotId> = l.shots.iter().map(|s| s.id).collect();
    let distinct: HashSet<ShotId> = ids.iter().copied().collect();
    assert_eq!(distinct.len(), ids.len(), "{what}: a repeated id {ids:?}");
    assert!(
        ids.iter()
            .all(|&id| id >= base && id < end && id < l.next_shot_id),
        "{what}: ids {ids:?} outside [{base}, {end}) or not below {}",
        l.next_shot_id
    );
    assert!(l.next_shot_id <= end, "{what}: allocator past the window");
}

/// 0.6.0's two overflow cases came from a persisted shot id at `u32::MAX`;
/// session ids are never persisted now (U8). What is left of the shape is a
/// line whose session used up its id window: the last id is handed out, the
/// next create is refused in so many words and changes nothing, and an undo
/// and redo of the last create give back the state they recorded, session
/// ids included.
#[test]
fn a_line_that_used_up_its_session_ids_refuses_the_next_shot_and_steps_exactly() {
    let mut p = empty("Ids");
    let a = free_line(&mut p, "A", [0.0, 0.0], north(2, 5.0));
    let o = open(&p);
    let end = (a + 1) * FEATURE_ID_STRIDE;
    // The session minted every id of the window but the last (create and
    // delete 99,997 times): the allocator is session state, set here.
    {
        let mut guard = o.state.project.lock().unwrap();
        guard.as_mut().unwrap().line_mut(a).unwrap().next_shot_id = end - 1;
    }

    let created = create_shot(&o.state, a, shot(0.0, 3.0), None, None);
    assert_eq!(created.shot_id, Some(end - 1), "the window's last id");
    assert_ids_sound(&o.state, a, "after the last create");
    let after = snapshot(&o.state);

    let refused = super::topology_kit::refused(
        &o.state,
        Command::CreateShot {
            line_id: a,
            shot: shot(0.0, 4.0),
            after_shot_id: None,
            before_shot_id: None,
        },
    );
    assert!(refused.contains("used up its shot numbers"), "{refused}");

    assert!(o.state.undo().success);
    assert_ids_sound(&o.state, a, "after the undo");
    assert!(o.state.redo().success);
    assert_ids_sound(&o.state, a, "after the redo");
    assert_same(
        &snapshot(&o.state),
        &after,
        "the redo lands on the recorded state",
    );
}

// ===========================================================================
// #413 finding 1: a redo puts every uuid back on its own shot
// ===========================================================================

/// The undo of a CreateLine with recorded edits after it, and their redo,
/// keep every uuid on the shot that owned it (0.6.0 zipped the recorded
/// shots against the recorded uuids by position).
#[test]
fn undo_and_redo_of_a_created_line_keep_every_uuid_on_its_own_shot() {
    let state = AppState::new().unwrap();
    run(
        &state,
        Command::CreateProject {
            name: "Identity".to_string(),
            location: None,
        },
    );
    let line = run(
        &state,
        Command::CreateLine {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#3388FF".to_string(),
            direction: None,
            shots: vec![shot(0.0, 5.0), shot(0.0, 7.0)],
            cave_section_id: None,
        },
    )
    .line_id
    .unwrap();
    let first = shot_ids(&state, line)[0];
    create_shot(&state, line, shot(180.0, 3.0), None, Some(first));
    let owned = uuid_lengths(&state, line);
    assert_eq!(
        owned.iter().map(|(_, l)| *l).collect::<Vec<_>>(),
        vec![3.0, 5.0, 7.0]
    );
    let after = snapshot(&state);

    assert!(state.undo().success);
    assert!(state.undo().success);
    assert!(project(&state).line(line).is_none(), "the line is gone");
    assert!(state.redo().success);
    assert!(state.redo().success);

    assert_eq!(
        uuid_lengths(&state, line),
        owned,
        "every uuid on its own shot"
    );
    assert_same(&snapshot(&state), &after, "and the survey to the bit");
}

/// The other half of the old shape: a shot that appeared on the line behind
/// the engine's back (0.6.0: an undo-less heal) makes the line differ from
/// what the entry recorded, so the undo is refused, keeps its entry and
/// changes nothing (the line hash precondition, plan deviation U2), instead
/// of re-creating the line with somebody else's identity on redo.
#[test]
fn an_undo_over_a_line_changed_behind_the_engine_is_refused_and_kept() {
    let mut p = empty("Behind");
    let a = free_line(&mut p, "A", [0.0, 0.0], north(1, 5.0));
    let o = open(&p);
    let line = run(
        &o.state,
        Command::CreateLine {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#3388FF".to_string(),
            direction: None,
            shots: vec![shot(0.0, 5.0), shot(0.0, 7.0)],
            cave_section_id: None,
        },
    )
    .line_id
    .unwrap();
    {
        let mut guard = o.state.project.lock().unwrap();
        let project = guard.as_mut().unwrap();
        let l = project.line_mut(line).unwrap();
        l.shots[0].length = 3.0;
    }
    let before = snapshot(&o.state);

    let undone = o.state.undo();

    assert!(
        !undone.success,
        "a step whose precondition fails is refused"
    );
    assert_same(&snapshot(&o.state), &before, "and changes nothing");
    assert_eq!(o.state.get_undo_stack().len(), 1, "and keeps its entry");
    assert!(project(&o.state).line(a).is_some());
}

// ===========================================================================
// Merge Projects: the cases of "Project Operations" (C13's range) that are
// the merge's (C8's merge_projects_tests holds the edge cases)
// ===========================================================================

use super::project_add_tests::{
    at as station_of, gps, line_named, read_v2, shot as fixed_shot, write_v2, xy as station_xy,
};

fn merge_into(dir: &Path, sources: Vec<PathBuf>, output: PathBuf, archive: bool) -> CommandResult {
    let _ = dir;
    AppState::new().unwrap().execute(Command::MergeProjects {
        source_paths: sources,
        output_path: output,
        output_name: "Merged".to_string(),
        archive_originals: archive,
    })
}

/// A project `name` with one free line `line` of one shot (uuid `uuid`).
fn one_line_project(name: &str, line: &str, uuid: &str, length: f64) -> Project {
    let mut p = Project::new(name, None);
    free_line(
        &mut p,
        line,
        [0.0, 0.0],
        vec![fixed_shot(uuid, 45.0, length, 10.0)],
    );
    p
}

/// 0.6.0's `test_merge_projects_archives_sources_and_adds_extension`: an
/// extension-less output gets `.stalx`, and every source is archived beside
/// itself, next to (never over) an archive that was already there.
#[test]
fn a_merge_adds_the_extension_and_archives_every_source() {
    let temp = TempDir::new().unwrap();
    let a = write_v2(
        temp.path(),
        "a.stalx",
        &one_line_project("A", "Line A", "a-0", 1.0),
    );
    let b = write_v2(
        temp.path(),
        "b.stalx",
        &one_line_project("B", "Line B", "b-0", 2.0),
    );
    let existing_archive = temp.path().join("a.stalx.archived");
    std::fs::write(&existing_archive, b"archived").unwrap();
    let output = temp.path().join("merged-output");

    let result = merge_into(
        temp.path(),
        vec![a.clone(), b.clone()],
        output.clone(),
        true,
    );

    assert!(result.success, "{:?}", result.error);
    let written = output.with_extension("stalx");
    assert_eq!(read_v2(&written).project.lines().len(), 2);
    assert!(!a.exists() && !b.exists(), "the sources are archived away");
    assert_eq!(
        std::fs::read(&existing_archive).unwrap(),
        b"archived",
        "an existing archive is never replaced"
    );
    let a_archives = std::fs::read_dir(temp.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("a.stalx.archived")
        })
        .count();
    assert_eq!(a_archives, 2, "the old archive and a new one beside it");
    assert!(temp.path().join("b.stalx.archived").exists());
}

/// #438: a merged line keeps the north reference it was RESOLVED to in its
/// own project, whether it declared one or inherited its project's default,
/// and the geometry follows (station 1 of a due-north shot sits on the
/// corrected bearing).
#[test]
fn each_merged_line_keeps_its_resolved_north_reference() {
    let temp = TempDir::new().unwrap();
    let due_north = |uuid: &str| ShotData {
        uuid: Some(uuid.to_string()),
        azimuth_in: 0.0,
        azimuth_out: 0.0,
        length: 10.0,
        depth_in: 10.0,
        depth_out: 10.0,
        ..ShotData::default()
    };
    let mut declared = Project::new("A", None);
    let line = free_line(
        &mut declared,
        "Declared",
        [0.0, 0.0],
        vec![due_north("d-0")],
    );
    declared.line_mut(line).unwrap().azimuth_reference = Some(
        crate::models::AzimuthReference::magnetic(10.0, crate::models::CorrectionSource::Manual),
    );
    let mut inherited = Project::new("B", None);
    inherited.settings.default_azimuth_reference = Some(crate::models::AzimuthReference::magnetic(
        -4.0,
        crate::models::CorrectionSource::FileDeclared,
    ));
    free_line(
        &mut inherited,
        "Inherited",
        [0.0, 0.0],
        vec![due_north("i-0")],
    );
    let output = temp.path().join("merged.stalx");

    let result = merge_into(
        temp.path(),
        vec![
            write_v2(temp.path(), "a.stalx", &declared),
            write_v2(temp.path(), "b.stalx", &inherited),
        ],
        output.clone(),
        false,
    );

    assert!(result.success, "{:?}", result.error);
    let merged = read_v2(&output).project;
    for (name, correction) in [("Declared", 10.0_f64), ("Inherited", -4.0_f64)] {
        let id = line_named(&merged, name);
        let l = merged.line(id).unwrap();
        assert!(
            (merged.azimuth_reference_for(l).applied_correction() - correction).abs() < 1e-9,
            "{name} lost its north reference: {:?}",
            l.azimuth_reference
        );
        let from = station_xy(&merged, station_of(&merged, id, 0));
        let to = station_xy(&merged, station_of(&merged, id, 1));
        let bearing = (to[0] - from[0]).atan2(to[1] - from[1]).to_degrees();
        assert!(
            (bearing - correction).abs() < 1e-6,
            "{name} walks at {bearing} degrees, expected {correction}"
        );
    }
}

/// Merge two one-line sources with these entrance elevations: the merged
/// elevation and the merge's warnings (code, message).
fn merge_two_elevations(
    first: Option<f64>,
    second: Option<f64>,
) -> (Option<f64>, Vec<(String, String)>) {
    let temp = TempDir::new().unwrap();
    let mut a = one_line_project("A", "Line A", "a-0", 1.0);
    a.settings.surface_elevation = first;
    let mut b = one_line_project("B", "Line B", "b-0", 2.0);
    b.settings.surface_elevation = second;
    let output = temp.path().join("merged.stalx");
    let result = merge_into(
        temp.path(),
        vec![
            write_v2(temp.path(), "a.stalx", &a),
            write_v2(temp.path(), "b.stalx", &b),
        ],
        output.clone(),
        false,
    );
    assert!(result.success, "{:?}", result.error);
    (
        read_v2(&output).project.settings.surface_elevation,
        result
            .warnings
            .into_iter()
            .map(|w| (w.code, w.message))
            .collect(),
    )
}

/// 0.6.0's five finite `test_merge_projects_surface_elevation_*` cases: the
/// first source that has an elevation wins; a later one agreeing within
/// the tolerance (strictly greater than 0.005 m disagrees, matching the
/// message's `{:.2}`) is quiet; a disagreeing one keeps the first and says
/// so, once.
#[test]
fn the_merged_entrance_elevation_is_the_first_one_and_a_disagreement_is_reported() {
    assert_eq!(
        merge_two_elevations(Some(123.5), None),
        (Some(123.5), vec![])
    );
    assert_eq!(
        merge_two_elevations(None, Some(-7.25)),
        (Some(-7.25), vec![])
    );
    assert_eq!(
        merge_two_elevations(Some(100.0), Some(100.002)),
        (Some(100.0), vec![])
    );
    let (kept, warnings) = merge_two_elevations(Some(100.0), Some(100.005));
    assert_eq!(kept, Some(100.0));
    assert!(
        warnings.is_empty(),
        "0.005 m is AT the tolerance, not beyond it: {warnings:?}"
    );
    let (kept, warnings) = merge_two_elevations(Some(100.0), Some(150.0));
    assert_eq!(kept, Some(100.0));
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let (code, message) = &warnings[0];
    assert_eq!(code, "SURFACE_ELEVATION_MISMATCH");
    assert!(message.contains("100.00"), "{message}");
    assert!(message.contains("150.00"), "{message}");
}

/// 0.6.0's `test_merge_projects_places_by_datum_and_keeps_station_offsets_
/// datums_and_closure_ids` and `test_merge_projects_preserves_station_comments`:
/// each georeferenced source is placed by its datum (the first's station on
/// the frame origin, the second's at its GPS in that frame), and every
/// source keeps its hand moves, datums, closures (re-minted, distinct ids)
/// and station comments.
#[test]
fn a_merge_places_each_source_by_its_datum_and_keeps_what_its_stations_carry() {
    let temp = TempDir::new().unwrap();
    let (gps_a, gps_b) = (gps(20.6, -87.3, 0.0), gps(20.55, -87.25, 2.0));
    let source =
        |name: &str, prefix: &str, at: GpsCoordinates, hand: [f64; 3], comment_at: usize| {
            let mut p = Project::new(name, None);
            let line = bare_line(
                &mut p,
                &format!("Line {name}"),
                vec![
                    fixed_shot(&format!("{prefix}-0"), 30.0, 4.0, 1.0),
                    fixed_shot(&format!("{prefix}-1"), 80.0, 6.0, 2.0),
                ],
            );
            let (s0, s1, s2) = (st(&p, line, 0), st(&p, line, 1), st(&p, line, 2));
            p.push_datum(ZeroDatum::new(s0, at));
            p.line_mut(line).unwrap().hand_moves.insert(s1, hand);
            p.push_closure(s0, Some(line), s2, Some(line), ClosureStrategy::Batch);
            let commented = st(&p, line, comment_at as u32);
            p.stations.get_mut(&commented).unwrap().comment = Some(format!("{name} note"));
            assert!(p.validate().is_empty(), "{:?}", p.validate());
            p
        };
    let a = source("A", "a", gps_a, [0.5, -0.5, 1.0], 0);
    let b = source("B", "b", gps_b, [-1.0, 1.5, 0.0], 1);
    let output = temp.path().join("merged.stalx");

    let result = merge_into(
        temp.path(),
        vec![
            write_v2(temp.path(), "a.stalx", &a),
            write_v2(temp.path(), "b.stalx", &b),
        ],
        output.clone(),
        false,
    );

    assert!(result.success, "{:?}", result.error);
    let merged = read_v2(&output).project;
    let (la, lb) = (line_named(&merged, "Line A"), line_named(&merged, "Line B"));
    let a0 = station_xy(&merged, station_of(&merged, la, 0));
    assert!(
        a0[0].abs() < 1e-9 && a0[1].abs() < 1e-9,
        "the frame origin: {a0:?}"
    );
    let expected = crate::models::gps_to_local(&gps_b, &gps_a);
    let b0 = station_xy(&merged, station_of(&merged, lb, 0));
    assert!(
        (b0[0] - expected.0).abs() < 1e-6 && (b0[1] - expected.1).abs() < 1e-6,
        "the second datum station sits at its GPS: {b0:?} vs {expected:?}"
    );
    for (line, hand) in [(la, [0.5, -0.5, 1.0]), (lb, [-1.0, 1.5, 0.0])] {
        let s1 = station_of(&merged, line, 1);
        assert_eq!(
            merged.line(line).unwrap().hand_moves.get(&s1),
            Some(&hand),
            "line {line} keeps its hand move"
        );
    }
    let latitudes: Vec<f64> = merged.datums.iter().map(|d| d.gps.latitude).collect();
    assert_eq!(latitudes, vec![20.6, 20.55], "both datums, in source order");
    let ids: HashSet<u64> = merged.closures.iter().map(|c| c.id).collect();
    assert_eq!(ids.len(), 2, "two closures with distinct ids");
    for line in [la, lb] {
        let (s0, s2) = (station_of(&merged, line, 0), station_of(&merged, line, 2));
        assert!(
            merged.closures.iter().any(|c| (c.a, c.b) == (s0, s2)),
            "line {line}'s closure kept its ends"
        );
    }
    let comment = |line, index| {
        merged.stations[&station_of(&merged, line, index)]
            .comment
            .clone()
    };
    assert_eq!(comment(la, 0).as_deref(), Some("A note"));
    assert_eq!(comment(lb, 1).as_deref(), Some("B note"));
}

// ===========================================================================
// Group 1: export round trips (C13's range), on the station-graph exporters
// ===========================================================================

fn line_with(state: &AppState, name: &str, shots: Vec<ShotData>) -> LineId {
    run(
        state,
        Command::CreateLine {
            name: Some(name.to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            shots,
            cave_section_id: None,
        },
    )
    .line_id
    .unwrap()
}

fn created_state(name: &str) -> AppState {
    let state = AppState::new().unwrap();
    run(
        &state,
        Command::CreateProject {
            name: name.to_string(),
            location: None,
        },
    );
    state
}

/// Export the open project to Ariane and read it back the way the import
/// wizard does: the import configs.
fn tml_round_trip(state: &AppState) -> Vec<crate::models::ImportLineConfig> {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("roundtrip.tmlu");
    crate::export::survey::export_tml(&project(state), &path).unwrap();
    let parsed = crate::parsers::tml::parse_tml(&path).unwrap();
    let (configs, _closures, _warnings) =
        crate::parsers::tml::convert_tml_to_import_configs(&parsed, true, false).unwrap();
    configs
}

#[test]
fn test_tml_roundtrip_preserves_shot_data() {
    let state = created_state("TML Roundtrip");
    let first = ShotData {
        azimuth_in: 12.5,
        azimuth_out: 192.3,
        length: 8.75,
        depth_in: 3.2,
        depth_out: 5.8,
        left: Some(1.1),
        right: Some(0.9),
        up: Some(2.3),
        down: Some(0.4),
        comment: Some("test note".to_string()),
        ..ShotData::default()
    };
    let second = ShotData {
        azimuth_in: 280.0,
        azimuth_out: 100.0,
        length: 3.25,
        depth_in: 5.8,
        depth_out: 4.0,
        ..ShotData::default()
    };
    line_with(&state, "Main Passage", vec![first, second]);

    let configs = tml_round_trip(&state);

    assert!(!configs.is_empty(), "at least one line");
    let shots = &configs[0].shots;
    assert_eq!(shots.len(), 2);
    let eps = 0.05; // the Ariane format's precision
    let close = |got: f64, want: f64, what: &str| {
        assert!((got - want).abs() < eps, "{what}: {got} vs {want}");
    };
    close(shots[0].length, 8.75, "length");
    close(shots[0].azimuth_in, 12.5, "azimuth_in");
    close(shots[0].depth_in, 3.2, "depth_in");
    close(shots[0].depth_out, 5.8, "depth_out");
    close(shots[1].length, 3.25, "second length");
    close(shots[1].azimuth_in, 280.0, "second azimuth_in");
}

#[test]
fn test_tml_roundtrip_preserves_multiple_sections() {
    let state = created_state("TML Multi");
    line_with(&state, "Entrance", vec![shot(10.0, 5.0), shot(20.0, 6.0)]);
    line_with(&state, "Side", vec![shot(100.0, 4.0)]);

    let configs = tml_round_trip(&state);

    assert_eq!(configs.len(), 2, "two lines");
    let total: usize = configs.iter().map(|c| c.shots.len()).sum();
    assert_eq!(total, 3, "every shot");
}

#[test]
fn test_csv_export_preserves_azimuth_precision() {
    let state = created_state("CSV Precision");
    line_with(
        &state,
        "Boundary",
        vec![ShotData {
            azimuth_in: 359.9,
            azimuth_out: 0.1,
            length: 4.567,
            depth_in: 12.345,
            depth_out: 12.678,
            left: Some(0.01),
            right: Some(99.99),
            ..ShotData::default()
        }],
    );
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("precision.csv");

    crate::export::survey::export_csv(&project(&state), &path).unwrap();

    let contents = std::fs::read_to_string(&path).unwrap();
    let row: Vec<&str> = contents.lines().nth(1).unwrap().split(',').collect();
    // Line, From, To, From ID, To ID, Length, Azimuth In, Azimuth Out, ...
    assert_eq!(row[5], "4.57", "the length at two decimals: {row:?}");
    assert_eq!(
        row[6], "359.9",
        "azimuth in, not truncated to 359 or 360: {row:?}"
    );
    assert_eq!(row[7], "0.1", "azimuth out: {row:?}");
}

// ===========================================================================
// The legacy open seeds the validation thresholds (C2's range)
// ===========================================================================

/// A temporary copy of the tracked e2e fixture `name`.
fn fixture_copy(dir: &Path, name: &str) -> PathBuf {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../e2e/fixtures")
        .join(name);
    let path = dir.join(name);
    std::fs::copy(&fixture, &path).unwrap();
    path
}

/// Issue #57 item 5: an old file saved before per-project validation (no
/// stored thresholds) opens with the user's GLOBAL thresholds (the
/// "defaults for new projects"), not the built-in defaults, and those drive
/// the warnings; a file that stores its own keeps them. The tracked
/// fixtures are one of each (schema 4 without, schema 14 with).
#[test]
fn test_legacy_project_seeds_validation_from_global_config() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = TempDir::new().unwrap();
        let state = AppState::new().unwrap();
        let mut config = state.get_config();
        // Every shot of the schema 4 fixture is 5 m: short under a 6 m
        // minimum, fine under the built-in 0.3 m.
        config.validation.warn_shot_length_min = 6.0;
        state.update_config(config).unwrap();

        assert!(
            state
                .open_project(fixture_copy(temp.path(), "fractal_plant_iter3.stalx"))
                .success
        );
        let opened = project(&state);
        assert_eq!(
            opened.settings.validation.warn_shot_length_min, 6.0,
            "an old file without thresholds inherits the global ones"
        );
        assert!(
            state
                .compute_project_warnings(&opened)
                .iter()
                .any(|w| w.code == "SHORT_SHOT"),
            "the inherited threshold drives the warnings"
        );

        assert!(
            state
                .open_project(fixture_copy(temp.path(), "naharon.stalx"))
                .success
        );
        assert_eq!(
            project(&state).settings.validation.warn_shot_length_min,
            0.1,
            "a file that stores its thresholds keeps them"
        );
    });
}
