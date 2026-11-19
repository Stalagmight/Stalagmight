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
    ClosureStrategy, CombineLinesOptions, CombinePick, Coordinates, CreditEdit, CreditTargets,
    GpsCoordinates, LineData, LineId, LineUpdates, Person, Project, SectionId, ShotData, ShotId,
    ShotKind, ShotUpdates, SplitAt, SplitLineOptions, SplitSide, StationRef,
};
use crate::persistence::Database;

/// The cap: an entry may be at most this fraction of the saved file.
const MAX_FRACTION: f64 = 0.01;

const SHOTS_PER_LINE: usize = 50;

fn shot(line: usize, i: usize) -> ShotData {
    let azimuth = ((line * 37 + i * 11) % 360) as f64 + 0.25;
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length: 3.0 + ((line + i * 7) % 9) as f64 * 1.125,
        depth_in: 5.0 + (i as f64) * 0.1,
        depth_out: 5.1 + (i as f64) * 0.1,
        left: Some(1.5),
        right: Some(2.0),
        up: Some(0.75),
        down: Some(1.25),
        comment: i
            .is_multiple_of(10)
            .then(|| format!("station note {line}-{i}")),
        marker_index: Some(i as u32),
        ..Default::default()
    }
}

fn line_data(line: usize, shots: usize) -> LineData {
    LineData {
        name: Some(format!("Line {line}")),
        color: "#4ecdc4".to_string(),
        shots: (0..shots).map(|i| shot(line, i)).collect(),
        ..Default::default()
    }
}

/// A tree of `lines` lines of `SHOTS_PER_LINE` shots, each line's
/// start tied to a station of its parent, a loop closure every 25 lines, a
/// GPS datum at the entrance, a Section grouping the first ten lines, plus an
/// unmerged 30-shot island and a 1-shot line hanging off the end of the root
/// line (so the two can combine). With `long`, an unmerged line of that many
/// shots too, before the island.
fn survey(lines: usize, long: Option<usize>) -> Project {
    let mut project = Project::new("Gate".to_string(), None);
    for s in 0..lines {
        project.add_line(line_data(s, SHOTS_PER_LINE));
    }
    let ids: Vec<LineId> = project.lines.iter().map(|s| s.id).collect();
    for s in 1..lines {
        let parent = ids[(s - 1) / 2];
        let at = 1 + ((s * 7) % (SHOTS_PER_LINE - 1)) as u32;
        project
            .merge_stations(StationRef::new(ids[s], 0), StationRef::new(parent, at))
            .unwrap();
    }
    for s in (25..lines).step_by(25) {
        let _ = project.create_loop_closure(
            StationRef::new(ids[s], SHOTS_PER_LINE as u32),
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
    let upper = project.add_section("Upper level".to_string(), "#ff6b6b".to_string());
    for id in ids.iter().take(10) {
        project.set_line_section(*id, Some(upper));
    }
    if let Some(shots) = long {
        project.add_line(line_data(8_000, shots));
    }
    project.add_line(line_data(9_000, 30));
    let leaf = project.add_line(line_data(9_001, 1)).id;
    project
        .merge_stations(
            StationRef::new(leaf, 0),
            StationRef::new(ids[0], SHOTS_PER_LINE as u32),
        )
        .unwrap();
    project
}

struct Gate {
    state: AppState,
    _dir: TempDir,
    file_bytes: u64,
    /// The lines of the tree, in id order.
    lines: Vec<LineId>,
    island: LineId,
    leaf: LineId,
    /// The long line, when the survey has one.
    long: Option<LineId>,
    /// The survey's one Section.
    section: SectionId,
}

fn open(lines: usize) -> Gate {
    open_with(lines, None)
}

fn open_with(lines: usize, long: Option<usize>) -> Gate {
    let dir = TempDir::new().unwrap();
    let path: PathBuf = dir.path().join("gate.stalx");
    let project = survey(lines, long);
    Database::open(&path)
        .unwrap()
        .save_project(&project)
        .unwrap();
    let state = AppState::new().unwrap();
    assert!(state.open_project(path.clone()).success, "open the survey");
    // Rewrite it through the app's own save, history empty: the survey alone.
    assert!(state.execute(Command::SaveProject { path: None }).success);
    state.undo_stack.lock().unwrap().clear();
    assert!(state.execute(Command::SaveProject { path: None }).success);
    let file_bytes = std::fs::metadata(&path).unwrap().len();
    let live = state.get_project_state().unwrap();
    let mut lines: Vec<LineId> = live.lines.iter().map(|s| s.id).collect();
    let leaf = lines.pop().unwrap();
    let island = lines.pop().unwrap();
    let long = long.map(|_| lines.pop().unwrap());
    let section = live.sections.first().expect("the survey's Section").id;
    Gate {
        state,
        _dir: dir,
        file_bytes,
        lines,
        island,
        leaf,
        long,
        section,
    }
}

impl Gate {
    /// A line in the middle of the tree that no loop closure touches (the
    /// closures sit on every 25th line and three before it).
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
            .get_line(line)
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

/// Persisted size of the newest entry of one stack (the one the next undo or
/// redo takes: `undo_entries` lists the most recent first).
fn newest(state: &AppState, redo: bool) -> (String, usize) {
    let stack = state.undo_stack.lock().unwrap();
    let entry = if redo {
        stack.redo_entries().next()
    } else {
        stack.undo_entries().next()
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
    /// Wall time of the action, its undo and its redo (reported, never
    /// asserted: a debug build and a loaded host say nothing about release).
    millis: [u128; 3],
}

type Action = fn(&Gate) -> crate::core::CommandResult;

fn actions() -> Vec<(&'static str, Action)> {
    vec![
        ("create line (20 shots)", |g| {
            g.state.execute(Command::CreateLine {
                name: Some("New".to_string()),
                surveyor: None,
                date: None,
                color: "#ff0000".to_string(),
                direction: None,
                shots: (0..20).map(|i| shot(7_777, i)).collect(),
                cave_section_id: None,
            })
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
                shot: shot(1, 1),
                after_shot_id: Some(g.shot_ids(g.mid())[20]),
                before_shot_id: None,
            })
        }),
        ("edit shot length", |g| {
            g.state.execute(Command::UpdateShot {
                line_id: g.lines[1],
                shot_id: g.shot_ids(g.lines[1])[5],
                updates: ShotUpdates {
                    length: Some(12.5),
                    ..Default::default()
                },
            })
        }),
        ("edit shot comment", |g| {
            g.state.execute(Command::UpdateShot {
                line_id: g.mid(),
                shot_id: g.shot_ids(g.mid())[5],
                updates: ShotUpdates {
                    comment: Some(Some("edited".to_string())),
                    ..Default::default()
                },
            })
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
        ("move line", |g| {
            g.state.execute(Command::MoveLine {
                line_id: g.island,
                offset: Coordinates::new(3.0, -2.0, 0.0),
            })
        }),
        // The tree's rigid group holds the datum, which pins it: the island moves.
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
            g.state.execute(Command::MoveComment {
                source: CommentHolder::Shot {
                    line_id: mid,
                    shot_id: g.shot_ids(mid)[10],
                },
                target: CommentHolder::Shot {
                    line_id: mid,
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
            let validation = g.state.get_project_state().unwrap().validation;
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
                .next()
                .expect("the leaf hangs off a line, so it can combine");
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
            g.state.execute(Command::CreateLine {
                name: Some("Grouped".to_string()),
                surveyor: None,
                date: None,
                color: "#ff0000".to_string(),
                direction: None,
                shots: (0..20).map(|i| shot(7_778, i)).collect(),
                cave_section_id: Some(g.section),
            })
        }),
        ("create line and merge into a section", |g| {
            g.state.execute_create_and_merge_in_cave_section(
                Some("Snapped".to_string()),
                None,
                None,
                "#00ff00".to_string(),
                None,
                (0..20).map(|i| shot(7_779, i)).collect(),
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
fn paste_left(g: &Gate, lines: &[LineId]) -> crate::core::CommandResult {
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
/// 20,000-shot survey only: on the smaller one a thousand shots are a fifth
/// of the survey, and an entry that holds what they changed is no small
/// fraction of the file.
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

/// `Surveyor` set on `targets`.
fn credit_edit(g: &Gate, targets: CreditTargets) -> crate::core::CommandResult {
    g.state.execute(Command::EditShotCredit {
        targets,
        edit: CreditEdit {
            set_surveyors: Some(vec![Person {
                name: "Surveyor".to_string(),
                email: None,
            }]),
            ..Default::default()
        },
    })
}

/// Credit edits over a long line and a selection of a thousand shots, on the
/// 20,000-shot survey with a 2,000-shot line added.
fn large_credit_actions() -> Vec<(&'static str, Action)> {
    vec![
        ("edit credit (2,000-shot line)", |g| {
            let line_id = g.long.expect("the survey's long line");
            credit_edit(g, CreditTargets::Line { line_id })
        }),
        ("edit credit (1,000 shots, 40 lines)", |g| {
            let mut shots = Vec::new();
            for line in &g.lines[40..80] {
                shots.extend(g.shot_ids(*line)[10..35].iter().map(|id| (*line, *id)));
            }
            credit_edit(g, CreditTargets::Shots { shots })
        }),
    ]
}

/// The project as the undo fuzzer compares it (`canonical_snapshot`): what no
/// history move restores - `modified_at`, the monotonic id allocators - left out.
fn state_text(state: &AppState) -> String {
    super::fuzz_undo_tests::canonical_snapshot(state, false)
}

/// The id allocators: each only grows (an id once handed out is not handed
/// out again, so an undo of what took one leaves it standing), so a history
/// move may leave one higher than it recorded, never lower.
const ALLOCATORS: [&str; 4] = [
    "next_line_id",
    "next_closure_id",
    "next_station_id",
    "next_cave_section_id",
];

/// The live project's snapshot text, its edit time blanked: equal texts are
/// equal states to the bit, and most steps compare equal without a parse.
fn raw_state(state: &AppState) -> String {
    let project = state.get_project_state().unwrap();
    let mut text = AppState::serialize_project_snapshot(&project).unwrap();
    let key = "\"modified_at\":\"";
    if let Some(at) = text.find(key) {
        let start = at + key.len();
        if let Some(len) = text[start..].find('"') {
            text.replace_range(start..start + len, "");
        }
    }
    text
}

/// The snapshot text member by member, every number to the bit: its top-level
/// members as written, each line on its own with its shot-id allocator apart.
/// Left out is the edit time, which every step stamps afresh.
fn bitwise_members(text: &str) -> std::collections::BTreeMap<String, String> {
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    let mut out = std::collections::BTreeMap::new();
    for (key, member) in value.as_object().unwrap() {
        match key.as_str() {
            "modified_at" => {}
            "lines" => {
                for line in member.as_array().unwrap() {
                    let mut line = line.clone();
                    let id = line["id"].to_string();
                    let next = line.as_object_mut().unwrap().remove("next_shot_id");
                    out.insert(format!("line {id}"), line.to_string());
                    out.insert(
                        format!("line {id} next_shot_id"),
                        next.map(|n| n.to_string()).unwrap_or_default(),
                    );
                }
            }
            _ => {
                out.insert(key.clone(), member.to_string());
            }
        }
    }
    out
}

/// Where `now` is not `recorded` to the bit. The one difference allowed is an
/// id allocator standing higher than recorded ([`ALLOCATORS`], and a line's
/// shot-id allocator): an undo of an insert leaves the line's raised past the
/// shot it removed, an undo of a new line or Section the project's, so a redo
/// cannot hand an id out twice.
fn bitwise_differences(
    recorded: &std::collections::BTreeMap<String, String>,
    now: &std::collections::BTreeMap<String, String>,
) -> Vec<String> {
    let raised = |key: &str, was: &str, is: &str| {
        (key.ends_with(" next_shot_id") || ALLOCATORS.contains(&key))
            && matches!((was.parse::<u64>(), is.parse::<u64>()), (Ok(was), Ok(is)) if is > was)
    };
    recorded
        .keys()
        .chain(now.keys())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter(|key| {
            let (was, is) = (recorded.get(*key), now.get(*key));
            was != is && !matches!((was, is), (Some(was), Some(is)) if raised(key, was, is))
        })
        .cloned()
        .collect()
}

/// Every action's row, and every action that could not be measured - or, when
/// `exact`, whose undo or redo did not land exactly on the state it records,
/// since a small entry that restores the wrong project is no saving at all.
/// Exact to the bit ([`raw_state`], [`bitwise_members`]), which is stricter than the fuzzer's
/// comparison (numbers to 12 significant digits). (The comparisons cost most
/// of the run on the larger survey, whose entries take the same paths, so
/// only the smaller one asks for them; without them an action is measured
/// from its step and one undo, and redone only for a report's timings.)
fn measure(
    gate: &Gate,
    exact: bool,
    actions: Vec<(&'static str, Action)>,
) -> (Vec<Row>, Vec<String>) {
    let mut rows = Vec::new();
    let mut failed = Vec::new();
    for (action, run) in actions {
        let depth = gate.state.undo_stack.lock().unwrap().len();
        let bits = |gate: &Gate| exact.then(|| raw_state(&gate.state));
        let off_by_bits = |recorded: &Option<String>, gate: &Gate| {
            let recorded = recorded.as_ref()?;
            let now = raw_state(&gate.state);
            if now == *recorded {
                return None;
            }
            Some(bitwise_differences(
                &bitwise_members(recorded),
                &bitwise_members(&now),
            ))
            .filter(|differences| !differences.is_empty())
        };
        let before_bits = bits(gate);
        let started = std::time::Instant::now();
        let result = run(gate);
        let forward_ms = started.elapsed().as_millis();
        if !result.success {
            failed.push(format!("{action}: {:?}", result.error));
            continue;
        }
        if gate.state.undo_stack.lock().unwrap().len() != depth + 1 {
            failed.push(format!("{action}: pushed no entry"));
            continue;
        }
        let after_bits = bits(gate);
        let (variant, undo_bytes) = newest(&gate.state, false);
        let started = std::time::Instant::now();
        let undone = gate.state.undo();
        let undo_ms = started.elapsed().as_millis();
        if !undone.success {
            failed.push(format!("{action}: undo: {:?}", undone.error));
            continue;
        }
        if let Some(differences) = off_by_bits(&before_bits, gate) {
            failed.push(format!(
                "{action}: the undo did not restore the state before it to the bit: {differences:?}"
            ));
            continue;
        }
        let (_, redo_bytes) = newest(&gate.state, true);
        if !exact && !reporting() {
            rows.push(Row {
                action,
                variant,
                undo_bytes,
                redo_bytes,
                millis: [forward_ms, undo_ms, 0],
            });
            continue;
        }
        // Redo and undo again, so the next action starts from the same state
        // and the redo is timed too.
        let started = std::time::Instant::now();
        let redone = gate.state.redo();
        let redo_ms = started.elapsed().as_millis();
        if !redone.success {
            failed.push(format!("{action}: redo: {:?}", redone.error));
            continue;
        }
        if let Some(differences) = off_by_bits(&after_bits, gate) {
            failed.push(format!(
                "{action}: the redo did not restore the state after it to the bit: {differences:?}"
            ));
            continue;
        }
        if !gate.state.undo().success {
            failed.push(format!("{action}: the second undo failed"));
            continue;
        }
        if let Some(differences) = off_by_bits(&before_bits, gate) {
            failed.push(format!(
                "{action}: the second undo did not restore the state before it to the bit: \
                 {differences:?}"
            ));
            continue;
        }
        rows.push(Row {
            action,
            variant,
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
        "{label}: {} lines, file {} bytes, cap {} bytes",
        gate.lines.len(),
        gate.file_bytes,
        (gate.file_bytes as f64 * MAX_FRACTION) as u64
    );
    for row in rows {
        let worst = row.undo_bytes.max(row.redo_bytes);
        let _ = writeln!(
            out,
            "  {:<36} {:<24} undo {:>8} redo {:>8}  {:.4}%   ms do/undo/redo {}/{}/{}",
            row.action,
            row.variant,
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
    let (rows, failed) = measure(gate, exact, actions);
    let mut text = table(label, gate, &rows);
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
    gate(100, "5k shots", true, false);
}

#[test]
fn every_undo_entry_is_a_small_fraction_of_a_20k_shot_file() {
    gate(400, "20k shots", false, true);
}

/// On the 5k survey: a credit entry grows with the runs it covers, not with
/// the survey, so the smaller one (and its stricter cap) is the harder test.
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
/// that changed them: past a fixed part (the label, the first sub-commands
/// the history previews) every changed cell costs at most
/// `BYTES_PER_CELL`, on the undo stack and on the redo stack, from 250 cells
/// to 2,000.
#[test]
fn a_paste_entry_costs_a_bounded_number_of_bytes_per_changed_cell() {
    /// What a batch entry holds whatever its size: the stored command's
    /// first sub-commands and label, the envelope.
    const FIXED_BYTES: usize = 4_000;
    /// One changed LRUD cell: its copy's offset and the value it held.
    const BYTES_PER_CELL: usize = 40;
    let gate = open(100);
    let mut report_text = String::from("paste entry per changed cell (5k shots)\n");
    for lines in [5, 40] {
        let lines = &gate.lines[10..10 + lines];
        let cells = lines.len() * SHOTS_PER_LINE;
        let before = state_text(&gate.state);
        assert!(paste_left(&gate, lines).success, "paste {cells} cells");
        let (_, undo_bytes) = newest(&gate.state, false);
        assert!(gate.state.undo().success);
        assert_eq!(state_text(&gate.state), before, "the undo of {cells} cells");
        let (_, redo_bytes) = newest(&gate.state, true);
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

/// The live project serialized in its parts (`serialize_project_parts`, what a
/// compact step and a capture use) is the whole-text snapshot split
/// (`serialize_project_snapshot` then `Doc::parse`), part for part and byte
/// for byte, and the project is left exactly as it was: on both gate surveys
/// (opened from a file, so `file_path` is set) and with a GPS fix, which the
/// snapshot keeps beside `zero_datums`.
#[test]
fn a_project_serialized_in_parts_is_its_whole_snapshot_split() {
    fn check(state: &AppState, what: &str) {
        let mut guard = state.project.lock().unwrap();
        let project = guard.as_mut().unwrap();
        assert!(
            project.file_path.is_some(),
            "{what}: premise, the path is set"
        );
        let whole = AppState::serialize_project_snapshot(project).unwrap();
        let parts = AppState::serialize_project_parts(project).unwrap();
        assert_eq!(
            parts.doc().unwrap(),
            crate::core::Doc::parse(&whole).unwrap(),
            "{what}: the parts are the whole text's"
        );
        assert_eq!(
            AppState::serialize_project_snapshot(project).unwrap(),
            whole,
            "{what}: serializing in parts gave everything back"
        );
        assert!(project.file_path.is_some(), "{what}: the path is back");
        let mut from_whole = None;
        let mut from_parts = None;
        assert!(AppState::restore_snapshot_into(&mut from_whole, &whole));
        assert!(AppState::restore_parts_into(&mut from_parts, &parts));
        assert_eq!(
            AppState::serialize_project_snapshot(from_parts.as_ref().unwrap()).unwrap(),
            AppState::serialize_project_snapshot(from_whole.as_ref().unwrap()).unwrap(),
            "{what}: the parts restore the project the whole text restores"
        );
    }
    // The 5k survey only: the 20k one takes no other path.
    let gate = open(100);
    check(&gate.state, "100 lines");
    // A second datum in the tree's component is a GPS fix there.
    assert!(
        gate.state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(gate.mid(), 20),
                coordinates: GpsCoordinates {
                    latitude: 20.501,
                    longitude: -87.249,
                    depth_reference: 0.0,
                },
                accuracy_m: Some(5.0),
                entrance: None,
            })
            .success
    );
    {
        let guard = gate.state.project.lock().unwrap();
        assert!(
            crate::core::undo::snapshot_fixes::holds_fix(guard.as_ref().unwrap()),
            "premise: the project holds a GPS fix"
        );
    }
    check(&gate.state, "100 lines and a GPS fix");
}
