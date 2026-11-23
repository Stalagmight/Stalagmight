//! The rarer actions of the undo fuzzer (`fuzz_undo_tests.rs`), the starting
//! shapes no command builds in a few moves, and the findings they pinned.
//!
//! # The rarer actions
//!
//! Every one is in the DEFAULT draw (the last block of `draw_op`, from the
//! run's own generator); `EXTENDED=1` draws a tenth more of them from the
//! coverage stream. Each goes through the entry point the app itself uses,
//! with its targets picked from the live survey, and is held to the engine's
//! own oracles: an ordinary undo entry, nothing special about it.
//!
//! | action | entry point |
//! |---|---|
//! | Edit Shot Credit, one line or a multi-shot selection | `Command::EditShotCredit` |
//! | Unmerge a whole station (every line but the first leaves the junction) | `Command::UnmergeStations` |
//! | Move Comment, station or shot holders, with swap | `Command::MoveComment` |
//! | Project Settings (name, validation) | `AppState::update_project_settings` |
//! | Entrance elevation, including back to unknown | `AppState::set_surface_elevation` |
//! | Untrack Import, alone and batched with a line delete (the DMP withdrawal) | `Command::UntrackImport`, `Command::Batch` |
//! | a synthetic import, placed at the origin or on a live station | `AppState::execute_import_lines`, the route every importer ends in |
//! | Add .stalx of a second synthetic v2 project, repeating live uuids | `Command::AddProjectFile` |
//! | Rename station, sometimes to a name the station's lines already hold (refused) | `Command::SetStationName` |
//! | a Section command (#327) | `Command::{Create,Update,Delete}CaveSection`, `MoveLinesToCaveSection`, `ApplyCaveSectionColor` |
//! | a new line created ON a live station, optionally into a Section | `AppState::execute_create_and_merge_in_cave_section` |
//!
//! Not drawn: Merge Projects writes and opens a NEW project, which ends the
//! run's history by design; Repair has nothing to repair on a survey only the
//! app's commands built; SpeleoDB replacements need a server.
//!
//! # The starting shapes
//!
//! * `LARGE=1`: an 80-line chain on junctions with a closure across every two
//!   links, 70 islands each placed by its own `Placement`, and 70 tracked
//!   import hashes of each kind - lists long enough that every history hunk
//!   lands well inside them - built on the live survey, then saved and
//!   reopened so the run starts from what the loader makes of it.
//! * `LEGACY_ORIGIN=1` (`Plan::legacy_origin`): a seeded synthetic 0.6.0
//!   survey (junction merges, a raw cycle-closing merge the migration demotes
//!   to a `Tie`, hand moves, a datum, closures, comments, a piece moved by
//!   hand) written by the frozen 0.6.0 save and opened through the migration.
//! * `DIVERGENCE=1`: session ids out of positional order, the id-addressed
//!   edits on top, then a crossing (the loader re-mints, persisted entries
//!   mint).
//! * `CLOSURE_SHAPES=1`: two closures, so list order matters to every undo
//!   of a removal; at the first crossing, two datum-held pieces tied by a
//!   closure, one datum then removed (the orphan sweep's promotion).

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::{random_shot, Drawn, Fuzzer, Op, Refs, Rng, ShotRefT, StationT, Targets, COLORS};
use crate::core::{Command, CommentHolder};
use crate::models::survey::{LineMeta, Project, StartAt};
use crate::models::{
    ClosureStrategy, CreditEdit, CreditTargets, GpsCoordinates, ImportLineConfig, ImportPlacement,
    LineId, Person, ShotData, ShotUpdates, StationRef,
};
use crate::persistence::stalx_v2::{self, Extras, Mode};

/// Lines in the LARGE chain.
const LARGE_CHAIN: usize = 80;
/// Islands in the LARGE project, each with its own placement.
const LARGE_ISLANDS: usize = 70;
/// Tracked import hashes of each kind in the LARGE project.
const LARGE_HASHES: usize = 70;

/// A station or a shot that can hold a comment, as a trace names it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(in crate::core::state) enum Holder {
    Station(StationT),
    Shot(ShotRefT),
}

/// One rarer action, as a value: everything drawn, every reference symbolic.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(in crate::core::state) enum ExtOp {
    EditCredit {
        /// The whole line when set, else `shots`.
        line: Option<LineId>,
        shots: Vec<ShotRefT>,
        /// 0 set surveyors, 1 add explorers, 2 remove surveyors, 3 set a
        /// date, 4 clear the date.
        role: u8,
        person: u32,
    },
    UnmergeStation {
        station: StationT,
    },
    MoveComment {
        source: Holder,
        target: Holder,
        swap: bool,
    },
    ProjectSettings {
        rename: Option<u32>,
        nudge: u32,
    },
    SurfaceElevation {
        millimetres: Option<i64>,
    },
    UntrackImport {
        delete: Option<LineId>,
        file_hashes: Vec<String>,
        line_hashes: Vec<String>,
        /// Inside a batch with the delete (the DMP wizard's route); alone
        /// otherwise.
        batched: bool,
    },
    Import {
        seed: u32,
        lines: u8,
        anchor: Option<StationT>,
        names: bool,
    },
    AddProjectFile {
        seed: u32,
        lines: u8,
        /// Live shots whose uuids the added file repeats.
        repeats: Vec<ShotRefT>,
    },
    /// Rename a station to one of a few names, so a draw can land on a name
    /// its lines already hold and exercise the refusal.
    RenameStation {
        station: StationT,
        name: u32,
    },
    /// A Section command on the project's Sections by position (`at` modulo
    /// their count): `kind` 0 creates one holding `lines`, 1 renames, 2 moves
    /// `lines` into it (out of any when `ungroup`), 3 applies its colour, 4
    /// deletes it.
    Section {
        kind: u8,
        at: u32,
        lines: Vec<LineId>,
        name: u32,
        ungroup: bool,
    },
    /// A new line whose station `at` (clamped to the line) is the live
    /// station `target`, optionally into the Section at that position.
    CreateAndMerge {
        shots: Vec<ShotData>,
        at: u32,
        target: StationT,
        section: Option<u32>,
    },
}

impl ExtOp {
    pub(in crate::core::state) fn kind(&self) -> &'static str {
        match self {
            Self::EditCredit { .. } => "edit_shot_credit",
            Self::UnmergeStation { .. } => "unmerge_station",
            Self::MoveComment { .. } => "move_comment",
            Self::ProjectSettings { .. } => "project_settings",
            Self::SurfaceElevation { .. } => "surface_elevation",
            Self::UntrackImport { .. } => "untrack_import",
            Self::Import { .. } => "import_lines",
            Self::AddProjectFile { .. } => "add_project_file",
            Self::RenameStation { .. } => "rename_station",
            Self::Section { .. } => "cave_section",
            Self::CreateAndMerge { .. } => "create_and_merge",
        }
    }

    pub(in crate::core::state) fn describe(&self) -> String {
        let holder = |h: &Holder| match h {
            Holder::Station(s) => format!("station {}", s.describe()),
            Holder::Shot(s) => format!("shot {}:{}", s.line, s.id),
        };
        match self {
            Self::EditCredit {
                line, shots, role, ..
            } => match line {
                Some(id) => format!("editShotCredit(line {id},role={role})"),
                None => format!("editShotCredit({} shot(s),role={role})", shots.len()),
            },
            Self::UnmergeStation { station } => {
                format!("unmergeStations({})", station.describe())
            }
            Self::MoveComment {
                source,
                target,
                swap,
            } => format!(
                "moveComment({} -> {},swap={swap})",
                holder(source),
                holder(target)
            ),
            Self::ProjectSettings { rename, nudge } => format!(
                "updateProjectSettings(rename={},nudge={nudge})",
                rename.is_some()
            ),
            Self::SurfaceElevation { millimetres } => {
                format!("setSurfaceElevation({millimetres:?}mm)")
            }
            Self::UntrackImport {
                delete,
                file_hashes,
                line_hashes,
                batched,
            } => format!(
                "untrackImport(deleteLine={delete:?},files={},lines={},batched={batched})",
                file_hashes.len(),
                line_hashes.len()
            ),
            Self::Import {
                seed,
                lines,
                anchor,
                names,
            } => format!(
                "importLines(seed={seed},n={lines},anchor={},names={names})",
                anchor.map_or_else(|| "origin".to_string(), StationT::describe)
            ),
            Self::AddProjectFile {
                seed,
                lines,
                repeats,
            } => format!(
                "addProjectFile(seed={seed},n={lines},repeats={})",
                repeats.len()
            ),
            Self::RenameStation { station, name } => {
                format!("setStationName({},r{name})", station.describe())
            }
            Self::Section {
                kind,
                at,
                lines,
                ungroup,
                ..
            } => format!(
                "caveSection(kind={kind},at={at},lines={},ungroup={ungroup})",
                lines.len()
            ),
            Self::CreateAndMerge {
                shots,
                at,
                target,
                section,
            } => format!(
                "createAndMerge(shots={},at={at},target={},section={section:?})",
                shots.len(),
                target.describe()
            ),
        }
    }
}

/// The default draw's rare block, from the run's own generator.
pub(super) fn draw_rare(fuzzer: &mut Fuzzer, targets: &Targets) -> Option<Op> {
    let project = fuzzer.live();
    draw_ext(&mut fuzzer.rng, targets, &project).map(|ext| Op::Extended(Box::new(ext)))
}

/// `EXTENDED=1`'s extra draws, from the coverage stream.
pub(super) fn draw_extended(fuzzer: &mut Fuzzer, targets: &Targets) -> Option<Op> {
    let project = fuzzer.live();
    draw_ext(&mut fuzzer.aux, targets, &project).map(|ext| Op::Extended(Box::new(ext)))
}

fn pick_shot_ref(rng: &mut Rng, targets: &Targets) -> Option<ShotRefT> {
    if targets.shot_uuids.is_empty() {
        return None;
    }
    let (line, id, uuid) = &targets.shot_uuids[rng.below(targets.shot_uuids.len())];
    Some(ShotRefT {
        line: *line,
        id: *id,
        uuid: uuid.clone(),
    })
}

fn pick_station(rng: &mut Rng, targets: &Targets) -> Option<StationT> {
    (!targets.slots.is_empty()).then(|| targets.station_at(rng.below(targets.slots.len())))
}

fn draw_ext(rng: &mut Rng, targets: &Targets, project: &Project) -> Option<ExtOp> {
    match rng.below(28) {
        0..=3 => {
            let role = rng.below(5) as u8;
            let person = rng.below(4) as u32;
            if rng.chance(30) {
                // A survey with no lines has no line to credit (the saving
                // fuzz starts from a new, empty project).
                let at = rng.below(targets.lines.len().max(1));
                let (line, _) = targets.lines.get(at)?;
                return Some(ExtOp::EditCredit {
                    line: Some(*line),
                    shots: Vec::new(),
                    role,
                    person,
                });
            }
            let count = 1 + rng.below(4);
            let shots: Vec<ShotRefT> = (0..count)
                .filter_map(|_| pick_shot_ref(rng, targets))
                .collect();
            (!shots.is_empty()).then_some(ExtOp::EditCredit {
                line: None,
                shots,
                role,
                person,
            })
        }
        4..=6 => {
            if targets.junctions.is_empty() {
                return None;
            }
            let (station, slots) = &targets.junctions[rng.below(targets.junctions.len())];
            Some(ExtOp::UnmergeStation {
                station: StationT {
                    station: *station,
                    line: slots[rng.below(slots.len())].0,
                },
            })
        }
        7..=8 => draw_move_comment(rng, targets, project),
        9..=10 => Some(ExtOp::ProjectSettings {
            rename: rng.chance(50).then(|| rng.below(1000) as u32),
            nudge: rng.below(40) as u32,
        }),
        11..=12 => Some(ExtOp::SurfaceElevation {
            millimetres: (!rng.chance(25)).then(|| rng.range(-50_000, 500_000)),
        }),
        13..=14 => {
            let delete = (!targets.lines.is_empty() && rng.chance(40))
                .then(|| targets.lines[rng.below(targets.lines.len())].0);
            let mut pick = |all: &[String]| -> Vec<String> {
                all.iter().filter(|_| rng.chance(40)).cloned().collect()
            };
            let file_hashes = pick(&targets.file_hashes);
            let line_hashes = pick(&targets.line_hashes);
            let batched = rng.chance(50);
            (!file_hashes.is_empty() || !line_hashes.is_empty()).then_some(ExtOp::UntrackImport {
                delete: delete.filter(|_| batched),
                file_hashes,
                line_hashes,
                batched,
            })
        }
        15..=17 => Some(ExtOp::Import {
            seed: rng.below(1 << 30) as u32,
            lines: 1 + rng.below(3) as u8,
            anchor: (rng.chance(70))
                .then(|| pick_station(rng, targets))
                .flatten(),
            names: rng.chance(60),
        }),
        18..=19 => {
            let repeats = (0..rng.below(3))
                .filter_map(|_| pick_shot_ref(rng, targets))
                .collect();
            Some(ExtOp::AddProjectFile {
                seed: rng.below(1 << 30) as u32,
                lines: 1 + rng.below(3) as u8,
                repeats,
            })
        }
        20..=22 => Some(ExtOp::RenameStation {
            station: pick_station(rng, targets)?,
            name: rng.below(4) as u32,
        }),
        23..=25 => {
            let count = rng.below(4);
            let lines = if targets.lines.is_empty() {
                Vec::new()
            } else {
                (0..count)
                    .map(|_| targets.lines[rng.below(targets.lines.len())].0)
                    .collect()
            };
            Some(ExtOp::Section {
                kind: rng.below(5) as u8,
                at: rng.below(8) as u32,
                lines,
                name: rng.below(6) as u32,
                ungroup: rng.chance(25),
            })
        }
        _ => {
            let target = pick_station(rng, targets)?;
            let shots: Vec<ShotData> = (0..=rng.below(3)).map(|_| random_shot(rng)).collect();
            Some(ExtOp::CreateAndMerge {
                at: rng.below(shots.len() + 1) as u32,
                shots,
                target,
                section: (!targets.sections.is_empty() && rng.chance(30))
                    .then(|| rng.below(8) as u32),
            })
        }
    }
}

fn draw_move_comment(rng: &mut Rng, targets: &Targets, project: &Project) -> Option<ExtOp> {
    // Sources that hold a comment, so most moves move something.
    let mut sources: Vec<Holder> = Vec::new();
    for &station in &targets.commented {
        if let Some(line) = targets
            .slots
            .iter()
            .find(|(_, _, s)| *s == station)
            .map(|(l, _, _)| *l)
        {
            sources.push(Holder::Station(StationT { station, line }));
        }
    }
    for line in project.lines() {
        for shot in &line.shots {
            if shot.comment.is_some() {
                sources.push(Holder::Shot(ShotRefT {
                    line: line.id,
                    id: shot.id,
                    uuid: shot.uuid.clone(),
                }));
            }
        }
    }
    if sources.is_empty() || targets.slots.is_empty() {
        return None;
    }
    let source = sources.swap_remove(rng.below(sources.len()));
    let target = if rng.chance(50) {
        Holder::Station(pick_station(rng, targets)?)
    } else {
        Holder::Shot(pick_shot_ref(rng, targets)?)
    };
    Some(ExtOp::MoveComment {
        source,
        target,
        swap: rng.chance(30),
    })
}

/// Execute one rarer action: the `apply_op` of `Op::Extended`.
pub(super) fn apply_extended(
    fuzzer: &mut Fuzzer,
    ext: &ExtOp,
    refs: &mut Refs,
    targets: &Targets,
) -> Option<Drawn> {
    fuzzer.outcome.coverage.extended += 1;
    let label = ext.describe();
    let state = &fuzzer.state;
    let result = match ext {
        ExtOp::EditCredit {
            line,
            shots,
            role,
            person,
        } => {
            let credit_targets = if let Some(id) = line {
                let Some(line_id) = refs.line(*id, targets) else {
                    return refs.skip(&label);
                };
                CreditTargets::Line { line_id }
            } else {
                let mut resolved: Vec<_> =
                    shots.iter().filter_map(|s| refs.shot(s, targets)).collect();
                resolved.dedup();
                if resolved.is_empty() {
                    return refs.skip(&label);
                }
                CreditTargets::Shots { shots: resolved }
            };
            state.execute(Command::EditShotCredit {
                targets: credit_targets,
                edit: credit_edit(*role, *person),
            })
        }
        ExtOp::UnmergeStation { station } => {
            let Some(station_ref) = refs.station(*station, targets) else {
                return refs.skip(&label);
            };
            state.execute(Command::UnmergeStations {
                station_ref,
                renames: None,
                rest_keep: None,
            })
        }
        ExtOp::MoveComment {
            source,
            target,
            swap,
        } => {
            let (Some(source), Some(target)) = (
                resolve_holder(source, refs, targets),
                resolve_holder(target, refs, targets),
            ) else {
                return refs.skip(&label);
            };
            state.execute(Command::MoveComment {
                source,
                target,
                swap: *swap,
            })
        }
        ExtOp::ProjectSettings { rename, nudge } => {
            let project = state.get_project_state()?;
            let name = rename.map_or(project.name.clone(), |n| format!("RustFuzz-{n}"));
            let mut validation = project.settings.validation;
            validation.warn_shot_length_max = 50.0 + f64::from(*nudge);
            state.update_project_settings(name, validation)
        }
        ExtOp::SurfaceElevation { millimetres } => {
            state.set_surface_elevation(millimetres.map(|mm| mm as f64 / 1000.0))
        }
        ExtOp::UntrackImport {
            delete,
            file_hashes,
            line_hashes,
            batched,
        } => {
            let untrack = Command::UntrackImport {
                file_hashes: file_hashes.clone(),
                line_hashes: line_hashes.clone(),
            };
            if *batched {
                let mut commands = Vec::new();
                if let Some(line_id) = delete.and_then(|id| refs.line(id, targets)) {
                    commands.push(Command::DeleteLine { line_id });
                }
                commands.push(untrack);
                state.execute(Command::Batch {
                    commands,
                    label: None,
                })
            } else {
                state.execute(untrack)
            }
        }
        ExtOp::Import {
            seed,
            lines,
            anchor,
            names,
        } => {
            let anchor = match anchor {
                Some(at) => match refs.station(*at, targets) {
                    Some(resolved) => Some(resolved),
                    None => return refs.skip(&label),
                },
                None => None,
            };
            let (configs, file_hash, line_hashes) =
                import_configs(*seed, usize::from(*lines), anchor, *names);
            state.execute_import_lines(
                configs,
                Vec::new(),
                Some(file_hash),
                line_hashes,
                Vec::new(),
                super::super::command_import::VirtualShotMode::default(),
            )
        }
        ExtOp::AddProjectFile {
            seed,
            lines,
            repeats,
        } => {
            let uuids: Vec<String> = repeats
                .iter()
                .filter_map(|r| {
                    let (line, id) = refs.shot(r, targets)?;
                    targets.uuid_of(line, id)
                })
                .collect();
            let temp = fuzzer.temp.get_or_insert_with(|| TempDir::new().unwrap());
            let path: PathBuf = temp.path().join(format!("added-{seed}.stalx"));
            let added = added_project(*seed, usize::from(*lines), &uuids);
            std::fs::write(
                &path,
                stalx_v2::encode(&added, &Extras::default(), Mode::Normal)
                    .expect("the project to add encodes"),
            )
            .expect("write the project to add");
            let result = fuzzer
                .state
                .execute(Command::add_project_file(path.clone()));
            let _ = std::fs::remove_file(&path);
            result
        }
        ExtOp::RenameStation { station, name } => {
            let Some(station_ref) = refs.station(*station, targets) else {
                return refs.skip(&label);
            };
            state.execute(Command::SetStationName {
                station_ref,
                name: format!("r{name}"),
            })
        }
        ExtOp::Section {
            kind,
            at,
            lines,
            name,
            ungroup,
        } => {
            let line_ids: Vec<LineId> = lines
                .iter()
                .filter_map(|id| refs.line(*id, targets))
                .collect();
            let section = (!targets.sections.is_empty()).then(|| {
                let len = targets.sections.len() as u32;
                targets.sections[(*at % len) as usize]
            });
            // With no Section yet every kind creates one.
            let command = match (*kind, section) {
                (0, _) | (_, None) => Command::CreateCaveSection {
                    name: format!("Region {name}"),
                    color: COLORS[*name as usize % COLORS.len()].to_string(),
                    line_ids,
                },
                (1, Some(section_id)) => Command::UpdateCaveSection {
                    section_id,
                    name: Some(format!("Region {name}")),
                    color: None,
                },
                (2, Some(section_id)) => Command::MoveLinesToCaveSection {
                    line_ids,
                    section_id: (!*ungroup).then_some(section_id),
                },
                (3, Some(section_id)) => Command::ApplyCaveSectionColor { section_id },
                (_, Some(section_id)) => Command::DeleteCaveSection { section_id },
            };
            state.execute(command)
        }
        ExtOp::CreateAndMerge {
            shots,
            at,
            target,
            section,
        } => {
            let Some(target_station) = refs.station(*target, targets) else {
                return refs.skip(&label);
            };
            let cave_section_id = section.and_then(|at| {
                let len = targets.sections.len() as u32;
                (len > 0).then(|| targets.sections[(at % len) as usize])
            });
            state.execute_create_and_merge_in_cave_section(
                Some(format!("joined-{}", shots.len())),
                None,
                None,
                COLORS[shots.len() % COLORS.len()].to_string(),
                None,
                shots.clone(),
                (*at).min(shots.len() as u32),
                target_station,
                ClosureStrategy::Move,
                cave_section_id,
            )
        }
    };
    Some(Drawn::new(label, result))
}

/// The edit a credit draw names.
fn credit_edit(role: u8, person: u32) -> CreditEdit {
    let named = |prefix: &str| {
        Some(vec![Person {
            name: format!("{prefix}{person}"),
            email: None,
        }])
    };
    match role {
        0 => CreditEdit {
            set_surveyors: named("Surveyor "),
            ..CreditEdit::default()
        },
        1 => CreditEdit {
            add_explorers: named("Explorer "),
            ..CreditEdit::default()
        },
        2 => CreditEdit {
            remove_surveyors: named("Surveyor "),
            ..CreditEdit::default()
        },
        3 => CreditEdit {
            set_date: Some(chrono::NaiveDate::from_ymd_opt(
                2026,
                1 + person % 12,
                1 + person,
            )),
            ..CreditEdit::default()
        },
        _ => CreditEdit {
            set_date: Some(None),
            ..CreditEdit::default()
        },
    }
}

fn resolve_holder(holder: &Holder, refs: &Refs, targets: &Targets) -> Option<CommentHolder> {
    Some(match holder {
        Holder::Station(at) => CommentHolder::Station {
            station_ref: refs.station(*at, targets)?,
        },
        Holder::Shot(shot) => {
            let (line_id, shot_id) = refs.shot(shot, targets)?;
            CommentHolder::Shot { line_id, shot_id }
        }
    })
}

/// A synthetic import of `count` lines, a function of `seed`: the first at
/// `anchor` (or the origin), each later one hung off the one before, with
/// station names and comments on some stations.
fn import_configs(
    seed: u32,
    count: usize,
    anchor: Option<StationRef>,
    names: bool,
) -> (Vec<ImportLineConfig>, String, Vec<String>) {
    let mut rng = Rng::new(seed);
    let file_hash = format!("fuzz-file-{seed:08x}");
    let mut configs = Vec::new();
    let mut hashes = Vec::new();
    for index in 0..count {
        let shots: Vec<ShotData> = (0..=rng.below(4)).map(|_| random_shot(&mut rng)).collect();
        let stations = shots.len() as u32;
        let placement = match (index, anchor) {
            (0, Some(existing_station)) => ImportPlacement::FromExisting {
                import_station: 0,
                existing_station,
            },
            (0, None) => ImportPlacement::Origin,
            _ => ImportPlacement::FromImporting {
                import_station: 0,
                target_line_index: index as u32 - 1,
                target_station_index: 1,
            },
        };
        let station_names = if names {
            (0..=stations)
                .filter(|_| rng.chance(50))
                .map(|at| (at, format!("I{seed:x}-{index}-{at}")))
                .collect()
        } else {
            Vec::new()
        };
        let station_comments = (0..=stations)
            .filter(|_| rng.chance(20))
            .map(|at| (at, format!("imported note {at}")))
            .collect();
        hashes.push(format!("{file_hash}-s{index}"));
        configs.push(ImportLineConfig {
            reverse: false,
            line_index: index as u32,
            name: Some(format!("imported-{seed:x}-{index}")),
            surveyor: None,
            date: None,
            color: (*rng.pick(COLORS)).to_string(),
            direction: None,
            quality: 1.0,
            shots,
            station_comments,
            station_names,
            station_external: Vec::new(),
            placement,
            azimuth_reference: None,
            explorers: None,
            override_date: None,
            surveyors: None,
            cave_section: None,
        });
    }
    (configs, file_hash, hashes)
}

fn meta(name: String, color: &str) -> LineMeta {
    LineMeta {
        name: Some(name),
        color: color.to_string(),
        ..LineMeta::default()
    }
}

/// The second project an Add .stalx brings in, a function of `seed`: a
/// chain of `count` lines on junctions, its last shots carrying uuids the
/// open project already holds, a station name on every line.
fn added_project(seed: u32, count: usize, repeats: &[String]) -> Project {
    let mut rng = Rng::new(seed);
    let mut project = Project::new(format!("Added {seed}"), None);
    let mut start = StartAt::New;
    for index in 0..count {
        let shots: Vec<ShotData> = (0..2 + rng.below(3))
            .map(|_| random_shot(&mut rng))
            .collect();
        let id = project
            .add_line(
                meta(format!("added-{seed:x}-{index}"), rng.pick(COLORS)),
                start,
                shots,
            )
            .expect("the added project builds");
        let line = project.line(id).expect("the line just added");
        if index == 0 {
            let first = line.start;
            project.set_placement(first, 0.0, 0.0).expect("a placement");
        }
        let line = project.line(id).expect("the line just added");
        let second = line.station(1).expect("two stations");
        start = StartAt::Station(line.stations().last().expect("a last station"));
        if let Some(station) = project.station_mut(second) {
            station.name = format!("A{seed:x}-{index}");
        }
    }
    // The repeats: later shots of the file take uuids the open project holds.
    let mut slots: Vec<(usize, usize)> = project
        .lines()
        .iter()
        .enumerate()
        .flat_map(|(s, line)| (0..line.shots.len()).map(move |i| (s, i)))
        .collect();
    slots.reverse();
    // Each repeated uuid once: a draw can name the same live shot twice.
    let mut distinct: Vec<&String> = Vec::new();
    for uuid in repeats {
        if !distinct.contains(&uuid) {
            distinct.push(uuid);
        }
    }
    for (uuid, (s, i)) in distinct.into_iter().zip(slots) {
        project.lines_mut()[s].shots[i].uuid = uuid.clone();
    }
    assert!(
        project.validate().is_empty(),
        "the added project is invalid: {:?}",
        project.validate()
    );
    project
}

/// Grow the live project into the LARGE one, then save and reopen it so the
/// run starts from what the loader makes of it.
pub(super) fn grow_large(state: &super::AppState, aux: &mut Rng) -> TempDir {
    {
        let mut guard = state.project.lock().unwrap();
        let project = guard.as_mut().expect("the builder opened a project");
        let root = project.lines()[0].start;
        let mut at = StartAt::Station(root);
        let mut chain = Vec::new();
        for index in 0..LARGE_CHAIN {
            let shots: Vec<ShotData> = (0..3).map(|_| random_shot(aux)).collect();
            let id = project
                .add_line(meta(format!("chain-{index}"), aux.pick(COLORS)), at, shots)
                .expect("a chain link");
            let line = project.line(id).expect("the link just added");
            at = StartAt::Station(line.stations().last().expect("a last station"));
            chain.push(id);
        }
        for index in 2..LARGE_CHAIN {
            let far = project.line(chain[index]).and_then(|l| l.station(3));
            let back = project.line(chain[index - 2]).and_then(|l| l.station(1));
            if let (Some(a), Some(b)) = (far, back) {
                project.push_closure(
                    a,
                    Some(chain[index]),
                    b,
                    Some(chain[index - 2]),
                    crate::models::survey::ClosureStrategy::Move,
                );
            }
        }
        for index in 0..LARGE_ISLANDS {
            let id = project
                .add_line(
                    meta(format!("island-{index}"), aux.pick(COLORS)),
                    StartAt::New,
                    vec![random_shot(aux), random_shot(aux)],
                )
                .expect("an island");
            let start = project.line(id).expect("the island").start;
            project
                .set_placement(start, 500.0 + index as f64 * 25.0, -300.0)
                .expect("the island's placement");
        }
        project
            .imported_file_hashes
            .extend((0..LARGE_HASHES).map(|i| format!("large-file-{i:03}")));
        project
            .imported_line_hashes
            .extend((0..LARGE_HASHES).map(|i| format!("large-line-{i:03}")));
        assert!(
            project.validate().is_empty(),
            "the LARGE project is invalid: {:?}",
            project.validate()
        );
    }
    state.bump_revision();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("large.stalx");
    assert!(
        state.save_project(Some(path.clone())).success,
        "save the LARGE project"
    );
    assert!(state.execute(Command::CloseProject).success);
    let opened = state.execute(Command::OpenProject { path });
    assert!(
        opened.success,
        "reopen the LARGE project: {:?}",
        opened.error
    );
    let project = state
        .get_project_state()
        .expect("the LARGE project is open");
    assert!(
        project.closures.len() > 64
            && project.placements.len() > 64
            && project.imported_file_hashes.len() > 64,
        "the LARGE project lost its long lists on the way through the file"
    );
    dir
}

/// A seeded synthetic 0.6.0 survey, written by the frozen 0.6.0 save into
/// `dir`: what `Plan::legacy_origin` opens through the migration.
pub(super) fn legacy_origin_file(seed: u32, dir: &Path) -> PathBuf {
    use stalagmight_legacy::models as m;
    let mut rng = Rng::new(seed ^ 0x0001_E6AC);
    let mut minted = 0_u64;
    let mut uuid = || {
        minted += 1;
        format!("{seed:08x}-0000-4000-8000-{minted:012x}")
    };
    let at = chrono::DateTime::from_timestamp(1_700_000_000 + i64::from(seed), 0)
        .expect("a valid instant");
    let mut p = m::Project::new(format!("Legacy {seed}"), None);
    p.created_at = at;
    p.modified_at = at;
    let shape: [usize; 7] = [3, 0, 1, 4, 2, 3, 2];
    for (n, count) in shape.iter().enumerate() {
        let shots = (0..*count)
            .map(|_| {
                let fresh = random_shot(&mut rng);
                m::ShotData {
                    uuid: Some(uuid()),
                    azimuth_in: fresh.azimuth_in,
                    azimuth_out: fresh.azimuth_out,
                    length: fresh.length,
                    depth_in: fresh.depth_in,
                    depth_out: fresh.depth_out,
                    left: fresh.left,
                    right: fresh.right,
                    comment: fresh.comment,
                    ..m::ShotData::default()
                }
            })
            .collect();
        p.add_line(m::LineData {
            name: (n % 3 != 0).then(|| format!("L{n}")),
            surveyor: (n % 2 == 0).then(|| "Ada; Bo".to_string()),
            date: None,
            color: (*rng.pick(COLORS)).to_string(),
            direction: Some(m::SurveyDirection::In),
            quality: 1.0,
            shots,
        });
    }
    let ids: Vec<m::LineId> = p.lines.iter().map(|l| l.id).collect();
    let key = |p: &m::Project, line: usize, index: u32| {
        p.key_for_ref(m::StationRef::new(ids[line], index))
    };
    // A hand move before the merges (a merge holds the shown position).
    if let Some(k) = key(&p, 0, 1) {
        p.station_offsets.insert(
            k,
            (
                rng.float(-1.0, 1.0),
                rng.float(-1.0, 1.0),
                rng.float(-0.5, 0.5),
            ),
        );
    }
    let _ = p.set_zero_datum(
        m::StationRef::new(ids[0], 0),
        m::GpsCoordinates::new(20.5, -87.3, 0.0).expect("valid GPS"),
    );
    // Junctions: 3 hangs off 0's end, 4 off 3's end, 6 off 5's middle.
    let _ = p.merge_stations(m::StationRef::new(ids[3], 0), m::StationRef::new(ids[0], 3));
    let _ = p.merge_stations(m::StationRef::new(ids[4], 0), m::StationRef::new(ids[3], 4));
    let _ = p.merge_stations(m::StationRef::new(ids[6], 0), m::StationRef::new(ids[5], 1));
    // A raw cycle-closing merge (0.6.0 files can hold one): a `Tie` once
    // migrated.
    if rng.chance(60) {
        if let (Some(station_a), Some(station_b)) = (key(&p, 4, 2), key(&p, 0, 1)) {
            p.station_merges.push(m::StationMerge {
                station_a,
                station_b,
                merged_at: at,
            });
        }
    }
    let strategy = if rng.chance(50) {
        m::ClosureStrategy::Move
    } else {
        m::ClosureStrategy::Batch
    };
    let _ = p.create_loop_closure(
        m::StationRef::new(ids[0], 0),
        m::StationRef::new(ids[4], 1),
        strategy,
    );
    for (line, index, text) in [(2, 1, "legacy note"), (5, 0, "tie-off")] {
        if let Some(k) = key(&p, line, index) {
            p.station_comments.insert(k, text.to_string());
        }
    }
    // The free piece moved by hand.
    p.adjust_rigid_group_offsets(ids[5], rng.float(-200.0, 200.0), rng.float(-200.0, 200.0));
    let bytes = stalagmight_legacy::write_v18(&p, &stalagmight_legacy::LegacyExtras::default())
        .expect("the frozen 0.6.0 save writes the legacy origin");
    let path = dir.join("legacy.stalx");
    std::fs::write(&path, bytes).expect("write the legacy origin");
    path
}

/// `DIVERGENCE=1`: a fresh line, an insert BEFORE its first shot (session
/// ids stop following positional order), then every id-addressed edit
/// contiguous on top - all through the ordinary per-move oracles.
pub(super) fn expose_stale_ids(fuzzer: &mut Fuzzer) {
    let shots: Vec<ShotData> = (0..3).map(|_| random_shot(&mut fuzzer.rng)).collect();
    let create = Op::CreateLine {
        name: Some("diverged".to_string()),
        surveyor: None,
        color: COLORS[0].to_string(),
        direction: None,
        shots,
        section: None,
    };
    fuzzer.step_apply(
        &create,
        &mut Refs::identity(),
        &super::read_targets(&fuzzer.state),
    );
    let Some(line) = fuzzer.live().lines().last().map(|l| l.id) else {
        return;
    };
    let first = |fuzzer: &Fuzzer| -> Option<ShotRefT> {
        let targets = super::read_targets(&fuzzer.state);
        let id = *targets.shots_of(line)?.first()?;
        Some(targets.shot_ref(line, id))
    };
    let Some(anchor) = first(fuzzer) else {
        return;
    };
    let insert = Op::CreateShot {
        line,
        shot: Box::new(random_shot(&mut fuzzer.rng)),
        after: None,
        before: Some(anchor),
    };
    fuzzer.step_apply(
        &insert,
        &mut Refs::identity(),
        &super::read_targets(&fuzzer.state),
    );
    let edits: Vec<Box<dyn Fn(ShotRefT) -> Op>> = vec![
        Box::new(|shot| Op::UpdateShot {
            shot,
            updates: Box::new(ShotUpdates {
                length: Some(7.25),
                ..ShotUpdates::default()
            }),
        }),
        Box::new(|shot| Op::SetShotKind {
            shot,
            kind: crate::models::ShotKind::Jump,
        }),
        Box::new(|shot| Op::SplitShotAtDistance {
            shot,
            distance: 1.5,
        }),
        Box::new(|shot| Op::DeleteShot { shot }),
    ];
    for edit in edits {
        let Some(shot) = first(fuzzer) else {
            return;
        };
        fuzzer.step_apply(
            &edit(shot),
            &mut Refs::identity(),
            &super::read_targets(&fuzzer.state),
        );
    }
}

/// `CLOSURE_SHAPES=1`: two closures, so list order matters to the undo of
/// a removal.
pub(super) fn expose_closure_shapes(fuzzer: &mut Fuzzer) {
    for strategy in [ClosureStrategy::Batch, ClosureStrategy::Move] {
        let targets = super::read_targets(&fuzzer.state);
        let Some((a, b)) = fuzzer.pick_cross_pair(&targets) else {
            return;
        };
        fuzzer.step_apply(
            &Op::CreateClosure { a, b, strategy },
            &mut Refs::identity(),
            &targets,
        );
    }
}

/// At the first crossing of a `CLOSURE_SHAPES` persistence run: two new
/// pieces each held by a datum, a closure between them, then one datum
/// removed - the orphan sweep must promote or keep the closure, and the
/// undo of the removal must put everything back.
pub(super) fn expose_orphan_closure(fuzzer: &mut Fuzzer) {
    let mut ends = Vec::new();
    for (index, (lat, lon)) in [(20.45, -87.25), (20.46, -87.26)].into_iter().enumerate() {
        let create = Op::CreateLine {
            name: Some(format!("orphan-{index}")),
            surveyor: None,
            color: COLORS[index].to_string(),
            direction: None,
            shots: vec![random_shot(&mut fuzzer.rng), random_shot(&mut fuzzer.rng)],
            section: None,
        };
        fuzzer.step_apply(
            &create,
            &mut Refs::identity(),
            &super::read_targets(&fuzzer.state),
        );
        let Some(line) = fuzzer.live().lines().last().cloned() else {
            return;
        };
        let station = StationT {
            station: line.start,
            line: line.id,
        };
        let datum = Op::SetZeroDatum {
            station,
            coordinates: GpsCoordinates::new(lat, lon, 0.0).expect("valid GPS"),
            accuracy_mm: None,
            entrance: None,
        };
        fuzzer.step_apply(
            &datum,
            &mut Refs::identity(),
            &super::read_targets(&fuzzer.state),
        );
        ends.push(StationT {
            station: line.stations().last().unwrap_or(line.start),
            line: line.id,
        });
        ends.push(station);
    }
    if ends.len() < 4 {
        return;
    }
    let closure = Op::CreateClosure {
        a: ends[0],
        b: ends[2],
        strategy: ClosureStrategy::Batch,
    };
    fuzzer.step_apply(
        &closure,
        &mut Refs::identity(),
        &super::read_targets(&fuzzer.state),
    );
    let remove = Op::RemoveZeroDatum { station: ends[3] };
    fuzzer.step_apply(
        &remove,
        &mut Refs::identity(),
        &super::read_targets(&fuzzer.state),
    );
}

// ===========================================================================
// What these actions pinned
// ===========================================================================

/// RESOLVED (was an `#[ignore]`d open finding of the old engine): a
/// standalone `UntrackImport` through `execute()` changed the import
/// tracking and cleared the redo branch but pushed no undo entry. On the
/// station-graph pipeline it records one, and its undo restores both lists
/// exactly.
/// Every draw on a survey with no lines is a move or nothing, never a panic:
/// the saving fuzz (`transition_fuzz_tests`) runs this generator on a new,
/// empty project, where a line credit indexed an empty target list.
#[test]
fn the_generator_draws_on_a_survey_with_no_lines() {
    let _config = crate::test_helpers::isolated_config();
    let mut fuzzer = Fuzzer::driving(1);
    let created = fuzzer.state.execute(Command::CreateProject {
        name: "Empty".to_string(),
        location: None,
    });
    assert!(created.success, "{:?}", created.error);
    let project = fuzzer.live();
    assert!(project.lines().is_empty(), "setup: no lines");
    let targets = Targets::of(&project);
    let drawn = (0..2_000).filter_map(|_| fuzzer.draw_op(&targets)).count();
    assert!(drawn > 0, "the generator drew nothing at all");
    for seed in 0..2_000 {
        let _ = draw_ext(&mut Rng::new(seed), &targets, &project);
    }
}

#[test]
fn untrack_import_alone_is_undoable() {
    let _config = crate::test_helpers::isolated_config();
    let state = super::AppState::new().unwrap();
    assert!(
        state
            .execute(Command::CreateProject {
                name: "Untrack".to_string(),
                location: None,
            })
            .success
    );
    let (configs, file_hash, line_hashes) = import_configs(7, 2, None, false);
    let imported = state.execute_import_lines(
        configs,
        Vec::new(),
        Some(file_hash.clone()),
        line_hashes.clone(),
        Vec::new(),
        super::super::command_import::VirtualShotMode::default(),
    );
    assert!(imported.success, "{:?}", imported.error);
    let tracked = state.get_project_state().unwrap();
    let before = state.get_undo_stack().len();
    let untracked = state.execute(Command::UntrackImport {
        file_hashes: vec![file_hash],
        line_hashes: line_hashes[..1].to_vec(),
    });
    assert!(untracked.success, "{:?}", untracked.error);
    let project = state.get_project_state().unwrap();
    assert!(project.imported_file_hashes.is_empty(), "the untrack ran");
    assert_eq!(project.imported_line_hashes, line_hashes[1..].to_vec());
    assert_eq!(
        state.get_undo_stack().len(),
        before + 1,
        "an untrack that changed the project pushed no undo entry"
    );
    assert!(state.undo().success);
    let restored = state.get_project_state().unwrap();
    assert_eq!(restored.imported_file_hashes, tracked.imported_file_hashes);
    assert_eq!(restored.imported_line_hashes, tracked.imported_line_hashes);
}

/// Two free pieces with a placement each, merged. The merged component holds
/// ONE placement (invariant 6), the save and the reopen keep the survey to
/// the byte, and the undo puts both placements back - the station-graph form
/// of the old `a_merge_of_two_held_components_survives_a_reopen` (whose
/// held-gauge list the redesign deleted; the anchor of a datum-less piece is
/// its `Placement` now).
#[test]
fn a_merge_of_two_placed_pieces_survives_a_reopen() {
    let _config = crate::test_helpers::isolated_config();
    let (state, dir, ids) = two_placed_pieces();
    let original = state.get_project_state().unwrap();
    assert_eq!(original.placements.len(), 2);
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(ids[0], 2),
                station_b: StationRef::new(ids[1], 0),
                keep: None,
                renames: None,
            })
            .success
    );
    let merged = state.get_project_state().unwrap();
    assert_eq!(merged.placements.len(), 1, "one placement per component");
    let path = dir.path().join("held.stalx");
    assert!(state.save_project(Some(path.clone())).success);
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.execute(Command::OpenProject { path }).success);
    let reopened = state.get_project_state().unwrap();
    assert_eq!(
        super::canonical_bytes(&reopened, reopened.counters),
        super::canonical_bytes(&merged, merged.counters),
        "the reopen changed what the session had"
    );
    assert!(state.undo().success);
    let undone = state.get_project_state().unwrap();
    assert_eq!(
        super::canonical_bytes(&undone, original.counters),
        super::canonical_bytes(&original, original.counters)
    );
}

/// After that merge, an Add .stalx and its undo are exact, and the undo of
/// the merge after it too (the old `an_add_after_a_merge_of_two_held_components_undoes_exactly`).
#[test]
fn an_add_after_a_merge_of_two_placed_pieces_undoes_exactly() {
    let _config = crate::test_helpers::isolated_config();
    let (state, dir, ids) = two_placed_pieces();
    let original = state.get_project_state().unwrap();
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(ids[0], 2),
                station_b: StationRef::new(ids[1], 0),
                keep: None,
                renames: None,
            })
            .success
    );
    let merged = state.get_project_state().unwrap();
    let added = dir.path().join("added.stalx");
    std::fs::write(
        &added,
        stalx_v2::encode(&added_project(11, 2, &[]), &Extras::default(), Mode::Normal).unwrap(),
    )
    .unwrap();
    let r = state.execute(Command::add_project_file(added));
    assert!(r.success, "{:?}", r.error);
    assert!(state.get_project_state().unwrap().lines().len() > merged.lines().len());
    assert!(state.undo().success);
    let undone = state.get_project_state().unwrap();
    assert_eq!(
        super::canonical_bytes(&undone, merged.counters),
        super::canonical_bytes(&merged, merged.counters),
        "the undo of the add did not put back what it found"
    );
    assert!(state.undo().success);
    let undone = state.get_project_state().unwrap();
    assert_eq!(
        super::canonical_bytes(&undone, original.counters),
        super::canonical_bytes(&original, original.counters),
        "the undo of the merge did not put back both placements"
    );
}

/// An app with two separate two-shot lines, each placed by its own
/// `Placement`.
fn two_placed_pieces() -> (super::AppState, TempDir, Vec<LineId>) {
    let state = super::AppState::new().unwrap();
    assert!(
        state
            .execute(Command::CreateProject {
                name: "Held".to_string(),
                location: None,
            })
            .success
    );
    let mut rng = Rng::new(3);
    let mut ids = Vec::new();
    for index in 0..2 {
        let r = state.execute(Command::CreateLine {
            name: Some(format!("island-{index}")),
            surveyor: None,
            date: None,
            color: "#123456".to_string(),
            direction: None,
            shots: vec![random_shot(&mut rng), random_shot(&mut rng)],
            cave_section_id: None,
        });
        assert!(r.success, "{:?}", r.error);
        ids.push(r.line_id.unwrap());
    }
    (state, TempDir::new().unwrap(), ids)
}

/// The legacy origin reaches the shapes it exists for over a few seeds: a
/// `Tie` (the demoted cycle-closing merge), junctions, a hand move, aliases
/// - and every one opens as a valid survey.
#[test]
fn the_legacy_origin_is_a_migrated_survey_with_the_old_shapes() {
    let _config = crate::test_helpers::isolated_config();
    let (mut ties, mut hands, mut junctions, mut aliases) = (0, 0, 0, 0);
    for seed in 1..=6 {
        let dir = TempDir::new().unwrap();
        let path = legacy_origin_file(seed, dir.path());
        let state = super::AppState::new().unwrap();
        let opened = state.execute(Command::OpenProject { path });
        assert!(opened.success, "seed {seed}: {:?}", opened.error);
        let project = state.get_project_state().unwrap();
        assert!(
            project.validate().is_empty(),
            "seed {seed}: {:?}",
            project.validate()
        );
        ties += project
            .closures
            .iter()
            .filter(|c| c.strategy == crate::models::survey::ClosureStrategy::Tie)
            .count();
        hands += project
            .lines()
            .iter()
            .map(|l| l.hand_moves.len())
            .sum::<usize>();
        junctions += project
            .stations
            .keys()
            .filter(|s| project.topology().is_junction(**s))
            .count();
        aliases += project
            .stations
            .values()
            .map(|s| s.archived.len())
            .sum::<usize>();
    }
    assert!(ties > 0, "no seed migrated a Tie");
    assert!(hands > 0, "no seed kept a hand move");
    assert!(
        junctions > 0 && aliases > 0,
        "no seed made a junction with aliases"
    );
}
