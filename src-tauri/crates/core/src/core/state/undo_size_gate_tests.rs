//! The undo size gate: on large synthetic surveys, every undoable action
//! records an entry - on the undo stack, and again on the redo stack once it
//! is undone - that is a small fraction of the saved file.
//!
//! Each action runs once against the same loaded project and is then undone,
//! so every measurement starts from the same state. An entry is measured as
//! the history persists it: the serialized command plus the serialized
//! payload, uncompressed. The file is the `.stalx` a save of the loaded
//! project writes with an empty history: the survey alone.
//!
//! Exempt by construction: the actions that bring whole-file data in (Add
//! .stalx, an import, Merge Projects) are not here - their entry IS the data
//! they add.
//!
//! `STALAGMIGHT_UNDO_SIZE_REPORT=<path>` writes the measured table there.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test module: a failed setup step is a failed test"
)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::AppState;
use crate::core::{Command, CommentHolder, UndoStack};
use crate::models::{
    ClosureStrategy, CombinePick, CombineSectionsOptions, Coordinates, CreditEdit, CreditTargets,
    GpsCoordinates, Person, Project, SectionData, SectionId, SectionUpdates, ShotData, ShotId,
    ShotKind, ShotUpdates, SplitAt, SplitSectionOptions, SplitSide, StationRef,
};
use crate::persistence::Database;

/// The cap: an entry may be at most this fraction of the saved file.
const MAX_FRACTION: f64 = 0.01;

const SHOTS_PER_SECTION: usize = 50;

fn shot(section: usize, i: usize) -> ShotData {
    let azimuth = ((section * 37 + i * 11) % 360) as f64 + 0.25;
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length: 3.0 + ((section + i * 7) % 9) as f64 * 1.125,
        depth_in: 5.0 + (i as f64) * 0.1,
        depth_out: 5.1 + (i as f64) * 0.1,
        left: Some(1.5),
        right: Some(2.0),
        up: Some(0.75),
        down: Some(1.25),
        comment: (i % 10 == 0).then(|| format!("station note {section}-{i}")),
        marker_index: Some(i as u32),
        ..Default::default()
    }
}

fn section_data(section: usize, shots: usize) -> SectionData {
    SectionData {
        name: Some(format!("Line {section}")),
        color: "#4ecdc4".to_string(),
        shots: (0..shots).map(|i| shot(section, i)).collect(),
        ..Default::default()
    }
}

/// A tree of `sections` lines of `SHOTS_PER_SECTION` shots, each line's
/// start tied to a station of its parent, a loop closure every 25 lines, a
/// GPS datum at the entrance, plus an unmerged 30-shot island and a 1-shot
/// line hanging off the end of the root line (so the two can combine).
fn survey(sections: usize) -> Project {
    let mut project = Project::new("Gate".to_string(), None);
    for s in 0..sections {
        project.add_section(section_data(s, SHOTS_PER_SECTION));
    }
    let ids: Vec<SectionId> = project.sections.iter().map(|s| s.id).collect();
    for s in 1..sections {
        let parent = ids[(s - 1) / 2];
        let at = 1 + ((s * 7) % (SHOTS_PER_SECTION - 1)) as u32;
        project
            .merge_stations(StationRef::new(ids[s], 0), StationRef::new(parent, at))
            .unwrap();
    }
    for s in (25..sections).step_by(25) {
        let _ = project.create_loop_closure(
            StationRef::new(ids[s], SHOTS_PER_SECTION as u32),
            StationRef::new(ids[s - 3], 2),
            ClosureStrategy::Move,
        );
    }
    project
        .set_zero_datum(
            StationRef::new(ids[0], 0),
            GpsCoordinates {
                latitude: 20.5,
                longitude: -87.25,
                depth_reference: 0.0,
            },
        )
        .unwrap();
    project.add_section(section_data(9_000, 30));
    let leaf = project.add_section(section_data(9_001, 1)).id;
    project
        .merge_stations(
            StationRef::new(leaf, 0),
            StationRef::new(ids[0], SHOTS_PER_SECTION as u32),
        )
        .unwrap();
    project
}

struct Gate {
    state: AppState,
    _dir: TempDir,
    file_bytes: u64,
    /// The lines of the tree, in id order.
    lines: Vec<SectionId>,
    island: SectionId,
    leaf: SectionId,
}

fn open(sections: usize) -> Gate {
    let dir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("gate.stalx");
    let project = survey(sections);
    Database::open(&path).unwrap().save_project(&project).unwrap();
    let state = AppState::new().unwrap();
    assert!(state.open_project(path.clone()).success, "open the survey");
    // Rewrite it through the app's own save, history empty: the survey alone.
    assert!(state.execute(Command::SaveProject { path: None }).success);
    state.undo_stack.lock().unwrap().clear();
    assert!(state.execute(Command::SaveProject { path: None }).success);
    let file_bytes = std::fs::metadata(&path).unwrap().len();
    let live = state.get_project_state().unwrap();
    let mut lines: Vec<SectionId> = live.sections.iter().map(|s| s.id).collect();
    let leaf = lines.pop().unwrap();
    let island = lines.pop().unwrap();
    Gate {
        state,
        _dir: dir,
        file_bytes,
        lines,
        island,
        leaf,
    }
}

impl Gate {
    /// A line in the middle of the tree that no loop closure touches (the
    /// closures sit on every 25th line and three before it).
    fn mid(&self) -> SectionId {
        self.lines[self.mid_index()]
    }

    fn mid_index(&self) -> usize {
        self.lines.len() / 2 + 1
    }

    fn shot_ids(&self, section: SectionId) -> Vec<ShotId> {
        self.state
            .get_project_state()
            .unwrap()
            .get_section(section)
            .unwrap()
            .shots
            .iter()
            .map(|s| s.id)
            .collect()
    }

    fn closure_id(&self) -> u64 {
        self.state.get_project_state().unwrap().loop_closures[0].id
    }
}

/// Persisted size of the newest entry of one stack.
fn newest(state: &AppState, redo: bool) -> (String, usize) {
    let stack = state.undo_stack.lock().unwrap();
    let entry = if redo {
        stack.redo_entries().last()
    } else {
        stack.undo_entries().last()
    }
    .expect("an entry");
    let (command, payload) = UndoStack::serialize_entry(entry).unwrap();
    (
        entry.undo_data.variant_name().to_string(),
        command.len() + payload.len(),
    )
}

struct Row {
    action: &'static str,
    variant: String,
    undo_bytes: usize,
    redo_bytes: usize,
}

type Action = fn(&Gate) -> crate::core::CommandResult;

fn actions() -> Vec<(&'static str, Action)> {
    vec![
        ("create section (20 shots)", |g| {
            g.state.execute(Command::CreateSection {
                name: Some("New".to_string()),
                surveyor: None,
                date: None,
                color: "#ff0000".to_string(),
                direction: None,
                shots: (0..20).map(|i| shot(7_777, i)).collect(),
            })
        }),
        ("rename section", |g| {
            g.state.execute(Command::UpdateSectionMetadata {
                section_id: g.mid(),
                updates: SectionUpdates {
                    name: Some(Some("Renamed".to_string())),
                    ..Default::default()
                },
            })
        }),
        ("delete section", |g| {
            g.state.execute(Command::DeleteSection {
                section_id: g.mid(),
            })
        }),
        ("reverse section", |g| {
            g.state.execute(Command::ReverseSection {
                section_id: g.mid(),
                swap_left_right: false,
            })
        }),
        ("insert shot", |g| {
            g.state.execute(Command::CreateShot {
                section_id: g.mid(),
                shot: shot(1, 1),
                after_shot_id: Some(g.shot_ids(g.mid())[20]),
                before_shot_id: None,
            })
        }),
        ("edit shot length", |g| {
            g.state.execute(Command::UpdateShot {
                section_id: g.lines[1],
                shot_id: g.shot_ids(g.lines[1])[5],
                updates: ShotUpdates {
                    length: Some(12.5),
                    ..Default::default()
                },
            })
        }),
        ("edit shot comment", |g| {
            g.state.execute(Command::UpdateShot {
                section_id: g.mid(),
                shot_id: g.shot_ids(g.mid())[5],
                updates: ShotUpdates {
                    comment: Some(Some("edited".to_string())),
                    ..Default::default()
                },
            })
        }),
        ("set shot kind", |g| {
            g.state.execute(Command::SetShotKind {
                section_id: g.mid(),
                shot_id: g.shot_ids(g.mid())[7],
                kind: ShotKind::Jump,
            })
        }),
        ("delete shot", |g| {
            g.state.execute(Command::DeleteShot {
                section_id: g.mid(),
                shot_id: g.shot_ids(g.mid())[25],
            })
        }),
        ("edit credit (one line)", |g| {
            g.state.execute(Command::EditShotCredit {
                targets: CreditTargets::Section {
                    section_id: g.mid(),
                },
                edit: CreditEdit {
                    set_surveyors: Some(vec![Person {
                        name: "Surveyor".to_string(),
                        email: None,
                    }]),
                    ..Default::default()
                },
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
            let at = 1 + ((g.mid_index() * 7) % (SHOTS_PER_SECTION - 1)) as u32;
            g.state.execute(Command::UnmergeStationPair {
                station_a: StationRef::new(g.mid(), 0),
                station_b: StationRef::new(parent, at),
            })
        }),
        ("create loop closure", |g| {
            g.state.execute(Command::CreateLoopClosure {
                station_a: StationRef::new(g.mid(), SHOTS_PER_SECTION as u32),
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
                coordinates: GpsCoordinates {
                    latitude: 20.501,
                    longitude: -87.249,
                    depth_reference: 0.0,
                },
                accuracy_m: Some(5.0),
                entrance: None,
            })
        }),
        ("set zero datum (places the island)", |g| {
            g.state.execute(Command::SetZeroDatum {
                station_ref: StationRef::new(g.island, 0),
                coordinates: GpsCoordinates {
                    latitude: 20.6,
                    longitude: -87.3,
                    depth_reference: 0.0,
                },
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
        ("move section", |g| {
            g.state.execute(Command::MoveSection {
                section_id: g.island,
                offset: Coordinates::new(3.0, -2.0, 0.0),
            })
        }),
        // The tree's rigid group holds the datum, which pins it: the island moves.
        ("move segment", |g| {
            g.state.execute(Command::MoveSegment {
                section_id: g.island,
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
            g.state.execute(Command::MoveComment {
                source: CommentHolder::Shot {
                    section_id: mid,
                    shot_id: g.shot_ids(mid)[10],
                },
                target: CommentHolder::Shot {
                    section_id: mid,
                    shot_id: g.shot_ids(mid)[11],
                },
                swap: false,
            })
        }),
        ("paste 30 cells (batch)", |g| {
            let mid = g.mid();
            let commands = g.shot_ids(mid)[..30]
                .iter()
                .map(|shot_id| Command::UpdateShot {
                    section_id: mid,
                    shot_id: *shot_id,
                    updates: ShotUpdates {
                        left: Some(Some(2.25)),
                        ..Default::default()
                    },
                })
                .collect();
            g.state.execute(Command::Batch { commands })
        }),
        ("project settings", |g| {
            let validation = g.state.get_project_state().unwrap().validation;
            g.state
                .update_project_settings("Gate renamed".to_string(), validation)
        }),
        ("surface elevation", |g| g.state.set_surface_elevation(Some(12.0))),
        ("split section here", |g| {
            g.state.execute_split_section(
                g.mid(),
                SplitAt::Station { station_index: 25 },
                SplitSectionOptions {
                    name_a: "A".to_string(),
                    name_b: "B".to_string(),
                    color_a: "#111111".to_string(),
                    color_b: "#222222".to_string(),
                    comment_side: SplitSide::A,
                    datum_side: SplitSide::A,
                },
            )
        }),
        ("combine sections", |g| {
            let candidate = g
                .state
                .combine_candidates(StationRef::new(g.leaf, 0))
                .unwrap()
                .into_iter()
                .next()
                .expect("the leaf hangs off a line, so it can combine");
            g.state.execute_combine_sections(
                candidate.section_a,
                candidate.section_b,
                CombineSectionsOptions {
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
        ("delete last shot and section", |g| {
            g.state
                .execute_delete_last_shot_and_section(g.leaf, g.shot_ids(g.leaf)[0])
        }),
    ]
}

/// Every action's row, and every action that could not be measured.
fn measure(gate: &Gate) -> (Vec<Row>, Vec<String>) {
    let mut rows = Vec::new();
    let mut failed = Vec::new();
    for (action, run) in actions() {
        let depth = gate.state.undo_stack.lock().unwrap().len();
        let result = run(gate);
        if !result.success {
            failed.push(format!("{action}: {:?}", result.error));
            continue;
        }
        if gate.state.undo_stack.lock().unwrap().len() != depth + 1 {
            failed.push(format!("{action}: pushed no entry"));
            continue;
        }
        let (variant, undo_bytes) = newest(&gate.state, false);
        let undone = gate.state.undo();
        if !undone.success {
            failed.push(format!("{action}: undo: {:?}", undone.error));
            continue;
        }
        let (_, redo_bytes) = newest(&gate.state, true);
        rows.push(Row {
            action,
            variant,
            undo_bytes,
            redo_bytes,
        });
    }
    (rows, failed)
}

fn table(label: &str, gate: &Gate, rows: &[Row]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{label}: {} lines, file {} bytes, cap {} bytes",
        gate.lines.len(),
        gate.file_bytes,
        (gate.file_bytes as f64 * MAX_FRACTION) as u64
    );
    for row in rows {
        let worst = row.undo_bytes.max(row.redo_bytes);
        let _ = writeln!(
            out,
            "  {:<36} {:<24} undo {:>8} redo {:>8}  {:.4}%",
            row.action,
            row.variant,
            row.undo_bytes,
            row.redo_bytes,
            100.0 * worst as f64 / gate.file_bytes as f64
        );
    }
    out
}

fn report(text: &str) {
    if let Ok(path) = std::env::var("STALAGMIGHT_UNDO_SIZE_REPORT") {
        let mut existing = std::fs::read_to_string(&path).unwrap_or_default();
        existing.push_str(text);
        std::fs::write(Path::new(&path), existing).unwrap();
    }
}

fn gate(sections: usize, label: &str) {
    let gate = open(sections);
    let (rows, failed) = measure(&gate);
    let mut text = table(label, &gate, &rows);
    for failure in &failed {
        let _ = writeln!(text, "  NOT MEASURED {failure}");
    }
    report(&text);
    assert!(failed.is_empty(), "actions that did not run:\n{text}");
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
    gate(100, "5k shots");
}

#[test]
fn every_undo_entry_is_a_small_fraction_of_a_20k_shot_file() {
    gate(400, "20k shots");
}
