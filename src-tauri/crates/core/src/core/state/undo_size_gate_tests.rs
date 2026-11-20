//! The undo size gate at the `AppState` level (plan 8.1, Deviation U11): on
//! the 5,000- and 20,000-shot gate surveys (`changeset::testkit::gate_survey`,
//! written as a v2 file and opened the way the app opens one), every entry
//! the execute pipeline records is at most 1% of that file with an empty
//! history, on the undo stack and on the redo stack once undone, and a paste
//! costs a fixed part plus at most 40 bytes per changed cell.
//!
//! The persistence crate's `history_size_gate_tests.rs` already gates the
//! changeset of each action through the model operations with a fixed label.
//! This gate measures what the pipeline itself adds on top: the label worded
//! at record time, the stable refs, the recorded preview rows, the real
//! command families (their own epilogues, recipes and batches). An entry is
//! measured as the history persists it: its canonical text
//! (`UndoEntry::bytes`, what a save writes and a push carries).
//!
//! On the 5,000-shot survey every undo and redo also lands on the recorded
//! state to the bit (the v2 text and the session shot ids; the id allocators
//! may stand higher). Exempt by construction, as before: the actions that
//! bring whole-file data in (Add .stalx, an import, Merge Projects).
//!
//! `STALAGMIGHT_UNDO_SIZE_REPORT=<path>` appends the measured table there.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test module: a failed setup step is a failed test"
)]

use std::fmt::Write as _;
use std::path::Path;

use tempfile::TempDir;

use super::topology_kit::{assert_same, snapshot};
use super::AppState;
use crate::core::{Command, CommandResult, CommentHolder};
use crate::models::survey::changeset::testkit::{gate_shot, gate_survey, gps, SHOTS_PER_LINE};
use crate::models::{
    ClosureStrategy, CombineLinesOptions, CombinePick, Coordinates, CreditEdit, CreditTargets,
    LineId, LineUpdates, Person, SectionId, ShotId, ShotKind, ShotUpdates, SplitAt,
    SplitLineOptions, SplitSide, StationRef,
};
use crate::persistence::stalx_v2::{self, Extras, Mode};

/// The cap: an entry may be at most this fraction of the v2 file.
const MAX_FRACTION: f64 = 0.01;

struct Gate {
    state: AppState,
    _dir: TempDir,
    file_bytes: usize,
    /// The tree's lines, in creation order.
    lines: Vec<LineId>,
    island: LineId,
    leaf: LineId,
    long: Option<LineId>,
    section: SectionId,
}

fn open(lines: usize) -> Gate {
    open_with(lines, None)
}

fn open_with(lines: usize, long: Option<usize>) -> Gate {
    let g = gate_survey(lines, long);
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("gate.stalx");
    let bytes = stalx_v2::encode(&g.project, &Extras::default(), Mode::Normal).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let state = AppState::new().unwrap();
    let r = state.execute(Command::OpenProject { path: path.clone() });
    assert!(r.success, "open the gate survey: {:?}", r.error);
    assert!(state.get_undo_stack().is_empty());
    Gate {
        state,
        _dir: dir,
        file_bytes: bytes.len(),
        lines: g.lines,
        island: g.island,
        leaf: g.leaf,
        long: g.long,
        section: g.section,
    }
}

impl Gate {
    /// A tree line in the middle that no closure touches.
    fn mid(&self) -> LineId {
        self.lines[self.mid_index()]
    }

    fn mid_index(&self) -> usize {
        self.lines.len() / 2 + 1
    }

    fn shot_ids(&self, line: LineId) -> Vec<ShotId> {
        self.state
            .get_project_state()
            .unwrap()
            .line(line)
            .unwrap()
            .shots
            .iter()
            .map(|s| s.id)
            .collect()
    }

    fn closure_id(&self) -> u64 {
        self.state.get_project_state().unwrap().closures[0].id
    }
}

/// The newest entry of one stack: `(kind, label, persisted bytes)`.
fn newest(state: &AppState, redo: bool) -> (String, String, usize) {
    let stack = state.undo_stack.lock().unwrap();
    let entry = if redo {
        stack.peek_redo()
    } else {
        stack.peek_undo()
    }
    .expect("an entry");
    (
        entry.entry().kind.clone(),
        entry.entry().label.clone(),
        entry.bytes(),
    )
}

struct Row {
    action: &'static str,
    kind: String,
    undo_bytes: usize,
    redo_bytes: usize,
    /// Wall time of the action, its undo and its redo (reported, never
    /// asserted: a debug build and a loaded host say nothing about release).
    millis: [u128; 3],
}

type Action = fn(&Gate) -> CommandResult;

fn shots(seed: usize) -> Vec<crate::models::ShotData> {
    (0..20).map(|i| gate_shot(seed, i)).collect()
}

fn create_line(name: &str, seed: usize, section: Option<SectionId>) -> Command {
    Command::CreateLine {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: shots(seed),
        cave_section_id: section,
    }
}

fn update(g: &Gate, line: LineId, index: usize, updates: ShotUpdates) -> CommandResult {
    g.state.execute(Command::UpdateShot {
        line_id: line,
        shot_id: g.shot_ids(line)[index],
        updates,
    })
}

fn surveyor_edit() -> CreditEdit {
    CreditEdit {
        set_surveyors: Some(vec![Person {
            name: "Surveyor".to_string(),
            email: None,
        }]),
        ..Default::default()
    }
}

fn actions() -> Vec<(&'static str, Action)> {
    vec![
        ("create line (20 shots)", |g| {
            g.state.execute(create_line("New", 7_777, None))
        }),
        ("rename line", |g| {
            g.state.execute(Command::UpdateLineMetadata {
                line_id: g.mid(),
                updates: LineUpdates {
                    name: Some(Some("Renamed".to_string())),
                    ..Default::default()
                },
            })
        }),
        ("delete line", |g| {
            g.state.execute(Command::DeleteLine { line_id: g.mid() })
        }),
        ("reverse line", |g| {
            g.state.execute(Command::ReverseLine {
                line_id: g.mid(),
                swap_left_right: false,
            })
        }),
        ("insert shot", |g| {
            g.state.execute(Command::CreateShot {
                line_id: g.mid(),
                shot: gate_shot(1, 1),
                after_shot_id: Some(g.shot_ids(g.mid())[20]),
                before_shot_id: None,
            })
        }),
        ("edit shot length", |g| {
            update(
                g,
                g.lines[1],
                5,
                ShotUpdates {
                    length: Some(12.5),
                    ..Default::default()
                },
            )
        }),
        ("edit shot comment", |g| {
            update(
                g,
                g.mid(),
                5,
                ShotUpdates {
                    comment: Some(Some("edited".to_string())),
                    ..Default::default()
                },
            )
        }),
        ("set shot kind", |g| {
            g.state.execute(Command::SetShotKind {
                line_id: g.mid(),
                shot_id: g.shot_ids(g.mid())[7],
                kind: ShotKind::Jump,
            })
        }),
        ("delete shot", |g| {
            g.state.execute(Command::DeleteShot {
                line_id: g.mid(),
                shot_id: g.shot_ids(g.mid())[25],
            })
        }),
        ("edit credit (one line)", |g| {
            g.state.execute(Command::EditShotCredit {
                targets: CreditTargets::Line { line_id: g.mid() },
                edit: surveyor_edit(),
            })
        }),
        ("merge stations", |g| {
            g.state.execute(Command::MergeStations {
                station_a: StationRef::new(g.island, 0),
                station_b: StationRef::new(g.mid(), 10),
            })
        }),
        ("unmerge stations", |g| {
            g.state.execute(Command::UnmergeStations {
                station_ref: StationRef::new(g.mid(), 0),
            })
        }),
        ("unmerge station pair", |g| {
            let parent = g.lines[(g.mid_index() - 1) / 2];
            let at = 1 + ((g.mid_index() * 7) % (SHOTS_PER_LINE - 1)) as u32;
            g.state.execute(Command::UnmergeStationPair {
                station_a: StationRef::new(g.mid(), 0),
                station_b: StationRef::new(parent, at),
            })
        }),
        ("create loop closure", |g| {
            g.state.execute(Command::CreateLoopClosure {
                station_a: StationRef::new(g.mid(), SHOTS_PER_LINE as u32),
                station_b: StationRef::new(g.lines[g.mid_index() + 1], 4),
                closure_strategy: ClosureStrategy::Move,
            })
        }),
        ("remove loop closure", |g| {
            g.state.execute(Command::RemoveLoopClosure {
                closure_id: g.closure_id(),
            })
        }),
        ("set zero datum (GPS fix)", |g| {
            g.state.execute(Command::SetZeroDatum {
                station_ref: StationRef::new(g.mid(), 20),
                coordinates: gps(20.501, -87.249),
                accuracy_m: Some(5.0),
                entrance: None,
            })
        }),
        ("set zero datum (places the island)", |g| {
            g.state.execute(Command::SetZeroDatum {
                station_ref: StationRef::new(g.island, 0),
                coordinates: gps(20.6, -87.3),
                accuracy_m: None,
                entrance: None,
            })
        }),
        ("remove zero datum", |g| {
            g.state.execute(Command::RemoveZeroDatum {
                station_ref: StationRef::new(g.lines[0], 0),
            })
        }),
        ("station comment", |g| {
            g.state.execute(Command::SetStationComment {
                station_ref: StationRef::new(g.mid(), 12),
                comment: Some("restriction".to_string()),
            })
        }),
        ("move line", |g| {
            g.state.execute(Command::MoveLine {
                line_id: g.island,
                offset: Coordinates::new(3.0, -2.0, 0.0),
            })
        }),
        // The tree holds the datum, which pins it: the island moves.
        ("move segment", |g| {
            g.state.execute(Command::MoveSegment {
                line_id: g.island,
                offset: Coordinates::new(1.5, 0.5, 0.0),
            })
        }),
        ("move station", |g| {
            g.state.execute(Command::MoveStation {
                station_ref: StationRef::new(g.mid(), 30),
                offset: Coordinates::new(0.4, 0.3, 0.0),
            })
        }),
        ("move comment", |g| {
            let mid = g.mid();
            let ids = g.shot_ids(mid);
            g.state.execute(Command::MoveComment {
                source: CommentHolder::Shot {
                    line_id: mid,
                    shot_id: ids[10],
                },
                target: CommentHolder::Shot {
                    line_id: mid,
                    shot_id: ids[11],
                },
                swap: false,
            })
        }),
        ("paste 30 cells (batch)", |g| {
            let mid = g.mid();
            let commands = g.shot_ids(mid)[..30]
                .iter()
                .map(|shot_id| Command::UpdateShot {
                    line_id: mid,
                    shot_id: *shot_id,
                    updates: ShotUpdates {
                        left: Some(Some(2.25)),
                        ..Default::default()
                    },
                })
                .collect();
            g.state.execute(Command::Batch {
                commands,
                label: None,
            })
        }),
        ("project settings", |g| {
            let validation = g.state.get_project_state().unwrap().settings.validation;
            g.state
                .update_project_settings("Gate renamed".to_string(), validation)
        }),
        ("surface elevation", |g| {
            g.state.set_surface_elevation(Some(12.0))
        }),
        ("split line here", |g| {
            g.state.execute_split_line(
                g.mid(),
                SplitAt::Station { station_index: 25 },
                SplitLineOptions {
                    name_a: "A".to_string(),
                    name_b: "B".to_string(),
                    color_a: "#111111".to_string(),
                    color_b: "#222222".to_string(),
                    comment_side: SplitSide::A,
                    datum_side: SplitSide::A,
                },
            )
        }),
        ("combine lines", |g| {
            let candidate = g
                .state
                .combine_candidates(StationRef::new(g.leaf, 0))
                .unwrap()
                .into_iter()
                .find(|c| c.blocked_reason.is_none())
                .expect("the leaf hangs off the root line's end, so it can combine");
            g.state.execute_combine_lines(
                candidate.line_a,
                candidate.line_b,
                CombineLinesOptions {
                    name: "Combined".to_string(),
                    color: "#333333".to_string(),
                    swap_lr: false,
                    comment_pick: CombinePick::Kept,
                    name_pick: CombinePick::Kept,
                    join: None,
                },
            )
        }),
        ("insert station mid-shot", |g| {
            g.state
                .execute_split_shot_at_distance(g.mid(), g.shot_ids(g.mid())[15], 1.0)
        }),
        ("delete shot and split", |g| {
            g.state.execute_delete_shot_and_split(
                g.mid(),
                g.shot_ids(g.mid())[25],
                "A".to_string(),
                "B".to_string(),
            )
        }),
        ("delete shot range (split)", |g| {
            let ids = g.shot_ids(g.mid());
            g.state.execute_delete_shot_range_split(
                g.mid(),
                ids[20],
                ids[24],
                "A".to_string(),
                "B".to_string(),
            )
        }),
        ("delete shot range (shift)", |g| {
            let ids = g.shot_ids(g.mid());
            g.state
                .execute_delete_shot_range_shift(g.mid(), ids[20], ids[24])
        }),
        ("delete last shot and line", |g| {
            g.state
                .execute_delete_last_shot_and_line(g.leaf, g.shot_ids(g.leaf)[0])
        }),
        // Station identity and Sections (#327).
        ("rename station", |g| {
            g.state.execute(Command::SetStationName {
                station_ref: StationRef::new(g.mid(), 10),
                name: "10 renamed".to_string(),
            })
        }),
        ("create line into a section", |g| {
            g.state
                .execute(create_line("Grouped", 7_778, Some(g.section)))
        }),
        ("create line and merge into a section", |g| {
            g.state.execute_create_and_merge_in_cave_section(
                Some("Snapped".to_string()),
                None,
                None,
                "#00ff00".to_string(),
                None,
                shots(7_779),
                0,
                StationRef::new(g.mid(), 30),
                ClosureStrategy::Batch,
                Some(g.section),
            )
        }),
        ("create section (with a line)", |g| {
            g.state.execute(Command::CreateCaveSection {
                name: "Lower level".to_string(),
                color: "#123456".to_string(),
                line_ids: vec![g.mid()],
            })
        }),
        ("rename section", |g| {
            g.state.execute(Command::UpdateCaveSection {
                section_id: g.section,
                name: Some("Renamed level".to_string()),
                color: None,
            })
        }),
        ("move 20 lines to a section", |g| {
            g.state.execute(Command::MoveLinesToCaveSection {
                line_ids: g.lines[20..40].to_vec(),
                section_id: Some(g.section),
            })
        }),
        ("apply section colour", |g| {
            g.state.execute(Command::ApplyCaveSectionColor {
                section_id: g.section,
            })
        }),
        ("delete section", |g| {
            g.state.execute(Command::DeleteCaveSection {
                section_id: g.section,
            })
        }),
    ]
}

/// A data-table paste of one LRUD cell into every shot of `lines`: a batch
/// of one `UpdateShot` per shot, each value different.
fn paste_left(g: &Gate, lines: &[LineId]) -> CommandResult {
    let mut commands = Vec::new();
    for line in lines {
        for shot_id in g.shot_ids(*line) {
            let k = commands.len();
            commands.push(Command::UpdateShot {
                line_id: *line,
                shot_id,
                updates: ShotUpdates {
                    left: Some(Some(1.0 + k as f64 * 0.01)),
                    ..Default::default()
                },
            });
        }
    }
    g.state.execute(Command::Batch {
        commands,
        label: None,
    })
}

/// Edits of a thousand shots or stations at once, measured on the
/// 20,000-shot survey only (on the smaller one a thousand shots are a fifth
/// of the survey).
fn large_actions() -> Vec<(&'static str, Action)> {
    vec![
        ("paste 1,000 cells (batch)", |g| {
            paste_left(g, &g.lines[180..200])
        }),
        ("rename 1,000 stations (batch)", |g| {
            let mut commands = Vec::new();
            for line in &g.lines[200..220] {
                for index in 1..=SHOTS_PER_LINE as u32 {
                    commands.push(Command::SetStationName {
                        station_ref: StationRef::new(*line, index),
                        name: format!("P{line}.{index}"),
                    });
                }
            }
            g.state.execute(Command::Batch {
                commands,
                label: None,
            })
        }),
    ]
}

/// Credit edits over a long line and a selection of a thousand shots, on
/// the 5,000-shot survey with a 2,000-shot line added: a credit entry grows
/// with the runs it covers, not with the survey, so the smaller survey (and
/// its stricter cap) is the harder test.
fn large_credit_actions() -> Vec<(&'static str, Action)> {
    vec![
        ("edit credit (2,000-shot line)", |g| {
            let line_id = g.long.expect("the survey's long line");
            g.state.execute(Command::EditShotCredit {
                targets: CreditTargets::Line { line_id },
                edit: surveyor_edit(),
            })
        }),
        ("edit credit (1,000 shots, 40 lines)", |g| {
            let mut shots = Vec::new();
            for line in &g.lines[40..80] {
                shots.extend(g.shot_ids(*line)[10..35].iter().map(|id| (*line, *id)));
            }
            g.state.execute(Command::EditShotCredit {
                targets: CreditTargets::Shots { shots },
                edit: surveyor_edit(),
            })
        }),
    ]
}

/// Every action's row, and every action that could not be measured - or,
/// when `exact`, whose undo or redo did not land exactly on the state it
/// records (a small entry that restores the wrong survey is no saving).
fn measure(gate: &Gate, exact: bool, actions: Vec<(&'static str, Action)>) -> (Vec<Row>, Vec<String>) {
    let mut rows = Vec::new();
    let mut failed = Vec::new();
    for (action, run) in actions {
        let depth = gate.state.get_undo_stack().len();
        let before = exact.then(|| snapshot(&gate.state));
        let started = std::time::Instant::now();
        let result = run(gate);
        let forward_ms = started.elapsed().as_millis();
        if !result.success {
            failed.push(format!("{action}: {:?}", result.error));
            continue;
        }
        if gate.state.get_undo_stack().len() != depth + 1 {
            failed.push(format!("{action}: pushed no entry"));
            continue;
        }
        let after = exact.then(|| snapshot(&gate.state));
        let (kind, label, undo_bytes) = newest(&gate.state, false);
        let started = std::time::Instant::now();
        let undone = gate.state.undo();
        let undo_ms = started.elapsed().as_millis();
        if !undone.success {
            failed.push(format!("{action}: undo: {:?}", undone.error));
            continue;
        }
        if let Some(before) = &before {
            assert_same(&snapshot(&gate.state), before, &format!("{action}: undo"));
        }
        let (_, redo_label, redo_bytes) = newest(&gate.state, true);
        assert_eq!(redo_label, label, "{action}: one entry serves both stacks");
        let mut redo_ms = 0;
        if exact || reporting() {
            let started = std::time::Instant::now();
            let redone = gate.state.redo();
            redo_ms = started.elapsed().as_millis();
            if !redone.success {
                failed.push(format!("{action}: redo: {:?}", redone.error));
                continue;
            }
            if let Some(after) = &after {
                assert_same(&snapshot(&gate.state), after, &format!("{action}: redo"));
            }
            if !gate.state.undo().success {
                failed.push(format!("{action}: the second undo failed"));
                continue;
            }
            if let Some(before) = &before {
                assert_same(
                    &snapshot(&gate.state),
                    before,
                    &format!("{action}: second undo"),
                );
            }
        }
        rows.push(Row {
            action,
            kind,
            undo_bytes,
            redo_bytes,
            millis: [forward_ms, undo_ms, redo_ms],
        });
    }
    (rows, failed)
}

fn table(label: &str, gate: &Gate, rows: &[Row]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{label}: {} lines, v2 file {} bytes, cap {} bytes",
        gate.lines.len(),
        gate.file_bytes,
        (gate.file_bytes as f64 * MAX_FRACTION) as u64
    );
    for row in rows {
        let worst = row.undo_bytes.max(row.redo_bytes);
        let _ = writeln!(
            out,
            "  {:<38} {:<20} undo {:>8} redo {:>8}  {:.4}%   ms do/undo/redo {}/{}/{}",
            row.action,
            row.kind,
            row.undo_bytes,
            row.redo_bytes,
            100.0 * worst as f64 / gate.file_bytes as f64,
            row.millis[0],
            row.millis[1],
            row.millis[2]
        );
    }
    out
}

/// Whether the run writes the size report (and so wants every timing).
fn reporting() -> bool {
    std::env::var_os("STALAGMIGHT_UNDO_SIZE_REPORT").is_some()
}

fn report(text: &str) {
    if let Ok(path) = std::env::var("STALAGMIGHT_UNDO_SIZE_REPORT") {
        let mut existing = std::fs::read_to_string(&path).unwrap_or_default();
        existing.push_str(text);
        std::fs::write(Path::new(&path), existing).unwrap();
    }
}

fn gate(lines: usize, label: &str, exact: bool, large: bool) {
    let mut actions = actions();
    if large {
        actions.extend(large_actions());
    }
    gate_actions(&open(lines), label, exact, actions);
}

fn gate_actions(gate: &Gate, label: &str, exact: bool, actions: Vec<(&'static str, Action)>) {
    let count = actions.len();
    let (rows, failed) = measure(gate, exact, actions);
    let mut text = table(label, gate, &rows);
    for failure in &failed {
        let _ = writeln!(text, "  NOT MEASURED {failure}");
    }
    report(&text);
    assert!(failed.is_empty(), "actions that did not run:\n{text}");
    assert_eq!(rows.len(), count, "every action measured");
    let cap = gate.file_bytes as f64 * MAX_FRACTION;
    let over: Vec<&Row> = rows
        .iter()
        .filter(|row| row.undo_bytes.max(row.redo_bytes) as f64 > cap)
        .collect();
    assert!(
        over.is_empty(),
        "entries over {}% of the file:\n{text}",
        MAX_FRACTION * 100.0
    );
}

#[test]
fn every_undo_entry_is_a_small_fraction_of_a_5k_shot_file() {
    gate(100, "5k shots", true, false);
}

#[test]
fn every_undo_entry_is_a_small_fraction_of_a_20k_shot_file() {
    gate(400, "20k shots", false, true);
}

#[test]
fn a_large_credit_edit_is_a_small_fraction_of_the_file() {
    gate_actions(
        &open_with(100, Some(2_000)),
        "5k shots and a 2,000-shot line, credit edits",
        true,
        large_credit_actions(),
    );
}

/// A paste's entry grows with the cells it changed, not with the commands
/// that changed them: past a fixed part (the label, the batch's recorded
/// preview rows, the envelope) every changed cell costs at most
/// `BYTES_PER_CELL`, on the undo stack and on the redo stack, from 250
/// cells to 2,000. Each undo lands exactly.
#[test]
fn a_paste_entry_costs_a_bounded_number_of_bytes_per_changed_cell() {
    /// What a batch entry holds whatever its size.
    const FIXED_BYTES: usize = 4_000;
    /// One changed LRUD cell.
    const BYTES_PER_CELL: usize = 40;
    let gate = open(100);
    let mut report_text = String::from("paste entry per changed cell (5k shots)\n");
    for lines in [5, 40] {
        let lines = &gate.lines[10..10 + lines];
        let cells = lines.len() * SHOTS_PER_LINE;
        let before = snapshot(&gate.state);
        let r = paste_left(&gate, lines);
        assert!(r.success, "paste {cells} cells: {:?}", r.error);
        let (_, _, undo_bytes) = newest(&gate.state, false);
        assert!(gate.state.undo().success);
        assert_same(
            &snapshot(&gate.state),
            &before,
            &format!("the undo of {cells} cells"),
        );
        let (_, _, redo_bytes) = newest(&gate.state, true);
        let _ = writeln!(
            report_text,
            "  {cells:>5} cells: undo {undo_bytes:>6} redo {redo_bytes:>6} ({:.1} B/cell)",
            undo_bytes.max(redo_bytes) as f64 / cells as f64
        );
        for (side, bytes) in [("undo", undo_bytes), ("redo", redo_bytes)] {
            assert!(
                bytes <= FIXED_BYTES + BYTES_PER_CELL * cells,
                "a {cells}-cell paste's {side} entry is {bytes} bytes, past \
                 {FIXED_BYTES} + {BYTES_PER_CELL} per cell"
            );
        }
    }
    report(&report_text);
}
