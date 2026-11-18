//! Two coverage knobs (see `Knobs` in `fuzz_undo_tests.rs`) that widen what the
//! roundtrip engine drives, with every draw from the coverage stream.
//!
//! # `EXTENDED=1`: the actions no generator drew
//!
//! A tenth of the mutations are one of the actions below, each through the
//! entry point the app itself uses, with its targets picked from the live
//! project. The oracle is the engine's own - the exact snapshot per depth,
//! through every history move, jump and crossing - because each of these pushes
//! an ordinary undo entry and nothing about it may be special.
//!
//! | action | entry point | what it brings the engine |
//! |---|---|---|
//! | Edit Shot Credit | `Command::EditShotCredit`, one section or a multi-shot selection | the `ShotCredit` typed entry |
//! | Unmerge Stations (whole station) | `Command::UnmergeStations` | every merge at a station removed by one entry |
//! | Move Comment | `Command::MoveComment`, station or shot holders, with swap | a comment moving between holders |
//! | Project Settings | `AppState::update_project_settings` | a delta that edits `name` / `validation` |
//! | Entrance elevation | `AppState::set_surface_elevation` | a delta that edits `surface_elevation`, including back to unknown |
//! | Untrack Import | `Command::UntrackImport` | positional edits of the tracked-hash lists |
//! | a synthetic import | `AppState::execute_import_sections`, the route every importer ends in | several sections in one entry, `station_names` bag edits, the import hash lists |
//! | Add .stalx | `Command::AddProjectFile` on a second synthetic project saved to disk | the `AddedProjectContent` entry, cross-file uuid repeats, `station_names` / `station_offsets` brought in, redo after a reload with no file to read |
//!
//! Not drawn, and why: Merge Projects writes a NEW output project and opens it,
//! which ends the run's history by design (there is nothing left to walk);
//! Repair Integrity has nothing to repair on a project only the app's own
//! commands built, so it would push no entry; SpeleoDB tree replacement needs a
//! linked project and a server.
//!
//! # `LARGE=1`: a project past the whole-list threshold
//!
//! A top-level list of 64 elements or fewer is replaced whole by a delta
//! (`SMALL_LIST`, core/undo/delta.rs); only a longer one is edited by position,
//! with its source length recorded and checked on apply. No ordinary run ever
//! held such a list (the peak was 32 merges), so that whole path went unrun.
//! The large project adds, after the ordinary builder has run:
//!
//! * a chain of `LARGE_CHAIN` sections, each merged to the one before - so
//!   `station_merges` is well past 64 - with a loop closure across every two of
//!   them, so `loop_closures` is too;
//! * `LARGE_ISLANDS` unmerged one-shot islands, each holding its own gauge in
//!   `held_gauges` (at most one per component is kept, so the islands are what
//!   lets the list pass 64);
//! * `LARGE_HASHES` tracked import hashes of each kind.
//!
//! It is built on the live project, then saved and reopened through the app,
//! so the run starts from exactly what the loader makes of it.

use std::path::PathBuf;

use tempfile::TempDir;

use super::{random_shot, Drawn, Fuzzer, Op, Refs, Rng, ShotRefT, StationRefT, Targets, COLORS};
use crate::core::{Command, CommentHolder};
use crate::models::{
    ClosureStrategy, CreditEdit, CreditTargets, HeldGauge, ImportPlacement, ImportSectionConfig,
    Person, Project, SectionData, SectionId, ShotData, StationRef,
};
use crate::persistence::Database;

/// Sections in the LARGE chain.
const LARGE_CHAIN: usize = 80;
/// Unmerged islands in the LARGE project, each holding a gauge.
const LARGE_ISLANDS: usize = 70;
/// Tracked import hashes of each kind in the LARGE project.
const LARGE_HASHES: usize = 70;

/// A station or a shot that can hold a comment, as a trace names it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(in crate::core::state) enum Holder {
    Station(StationRefT),
    Shot(ShotRefT),
}

/// One extended action, as a value: everything drawn, every reference symbolic.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(in crate::core::state) enum ExtOp {
    EditCredit {
        /// The whole section when set, else `shots`.
        section: Option<SectionId>,
        shots: Vec<ShotRefT>,
        /// 0 set surveyors, 1 add explorers, 2 remove surveyors, 3 set a date,
        /// 4 clear the date.
        role: u8,
        person: u32,
    },
    UnmergeStation {
        station: StationRefT,
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
        /// Millimetres, so the value is an integer draw.
        millimetres: Option<i64>,
    },
    /// The withdrawal the DMP wizard sends: one batch of an optional section
    /// delete and the untrack. A bare `UntrackImport` through `execute()` pushes
    /// no entry (see `untrack_import_alone_is_undoable`), so the route the app
    /// uses is the one drawn.
    UntrackImport {
        delete: Option<SectionId>,
        file_hashes: Vec<String>,
        section_hashes: Vec<String>,
    },
    Import {
        seed: u32,
        sections: u8,
        anchor: Option<StationRefT>,
        names: bool,
    },
    AddProjectFile {
        seed: u32,
        sections: u8,
        /// Live shots whose uuids the added file repeats.
        repeats: Vec<ShotRefT>,
    },
}

impl ExtOp {
    pub(super) fn describe(&self) -> String {
        let station = |s: &StationRefT| format!("{}/{}", s.section, s.index);
        let holder = |h: &Holder| match h {
            Holder::Station(s) => format!("station {}", station(s)),
            Holder::Shot(s) => format!("shot {}:{}", s.section, s.id),
        };
        match self {
            Self::EditCredit {
                section,
                shots,
                role,
                ..
            } => match section {
                Some(id) => format!("editShotCredit(section {id},role={role})"),
                None => format!("editShotCredit({} shot(s),role={role})", shots.len()),
            },
            Self::UnmergeStation { station: s } => format!("unmergeStations({})", station(s)),
            Self::MoveComment {
                source,
                target,
                swap,
            } => format!(
                "moveComment({} -> {},swap={swap})",
                holder(source),
                holder(target)
            ),
            Self::ProjectSettings { rename, nudge } => {
                format!(
                    "updateProjectSettings(rename={},nudge={nudge})",
                    rename.is_some()
                )
            }
            Self::SurfaceElevation { millimetres } => {
                format!("setSurfaceElevation({millimetres:?}mm)")
            }
            Self::UntrackImport {
                delete,
                file_hashes,
                section_hashes,
            } => format!(
                "batch(deleteSection={delete:?},untrackImport(files={},sections={}))",
                file_hashes.len(),
                section_hashes.len()
            ),
            Self::Import {
                seed,
                sections,
                anchor,
                names,
            } => format!(
                "importSections(seed={seed},n={sections},anchor={},names={names})",
                anchor
                    .as_ref()
                    .map_or_else(|| "origin".to_string(), station)
            ),
            Self::AddProjectFile {
                seed,
                sections,
                repeats,
            } => format!(
                "addProjectFile(seed={seed},n={sections},repeats={})",
                repeats.len()
            ),
        }
    }
}

impl Fuzzer {
    /// Draw one extended action from the coverage stream. `None` when the
    /// project has no target of the drawn kind right now.
    pub(super) fn draw_extended(&mut self, targets: &Targets) -> Option<Op> {
        let ext = match self.aux.below(8) {
            0 => self.draw_credit(targets),
            1 => {
                if targets.merges.is_empty() {
                    return None;
                }
                let (a, b) = targets.merges[self.aux.below(targets.merges.len())];
                let (section, index) = if self.aux.chance(50) { a } else { b };
                Some(ExtOp::UnmergeStation {
                    station: StationRefT { section, index },
                })
            }
            2 => self.draw_move_comment(targets),
            3 => Some(ExtOp::ProjectSettings {
                rename: self
                    .aux
                    .chance(50)
                    .then(|| u32::try_from(self.aux.below(1000)).unwrap_or(0)),
                nudge: u32::try_from(self.aux.below(40)).unwrap_or(0),
            }),
            4 => Some(ExtOp::SurfaceElevation {
                millimetres: (!self.aux.chance(25)).then(|| self.aux.range(-50_000, 500_000)),
            }),
            5 => self.draw_untrack(targets),
            6 => Some(ExtOp::Import {
                seed: u32::try_from(self.aux.below(1 << 30)).unwrap_or(0),
                sections: 1 + u8::try_from(self.aux.below(3)).unwrap_or(0),
                anchor: (!targets.stations.is_empty() && self.aux.chance(70)).then(|| {
                    let (section, index) = targets.stations[self.aux.below(targets.stations.len())];
                    StationRefT { section, index }
                }),
                names: self.aux.chance(60),
            }),
            _ => {
                let repeats = (0..self.aux.below(3))
                    .filter_map(|_| self.pick_shot_ref(targets))
                    .collect();
                Some(ExtOp::AddProjectFile {
                    seed: u32::try_from(self.aux.below(1 << 30)).unwrap_or(0),
                    sections: 1 + u8::try_from(self.aux.below(3)).unwrap_or(0),
                    repeats,
                })
            }
        }?;
        Some(Op::Extended(Box::new(ext)))
    }

    /// A live shot as a trace reference, drawn from the coverage stream.
    fn pick_shot_ref(&mut self, targets: &Targets) -> Option<ShotRefT> {
        if targets.shot_uuids.is_empty() {
            return None;
        }
        let (section, id, uuid) = &targets.shot_uuids[self.aux.below(targets.shot_uuids.len())];
        Some(ShotRefT {
            section: *section,
            id: *id,
            uuid: Some(uuid.clone()),
        })
    }

    fn draw_credit(&mut self, targets: &Targets) -> Option<ExtOp> {
        let role = u8::try_from(self.aux.below(5)).unwrap_or(0);
        let person = u32::try_from(self.aux.below(4)).unwrap_or(0);
        if self.aux.chance(30) {
            if targets.sections.is_empty() {
                return None;
            }
            let (section, _) = &targets.sections[self.aux.below(targets.sections.len())];
            return Some(ExtOp::EditCredit {
                section: Some(*section),
                shots: Vec::new(),
                role,
                person,
            });
        }
        // A multi-shot selection, possibly across sections - the shape the
        // grid's credit editor sends.
        let count = 1 + self.aux.below(4);
        let shots: Vec<ShotRefT> = (0..count)
            .filter_map(|_| self.pick_shot_ref(targets))
            .collect();
        (!shots.is_empty()).then_some(ExtOp::EditCredit {
            section: None,
            shots,
            role,
            person,
        })
    }

    fn draw_move_comment(&mut self, targets: &Targets) -> Option<ExtOp> {
        let project = self.state.get_project_state()?;
        // Sources that hold a comment, so most moves move something: station
        // comments, then shot comments, both in a stable order.
        let mut sources: Vec<Holder> = Vec::new();
        for &(section, index) in &targets.stations {
            let holds = project
                .key_for_ref(StationRef::new(section, index))
                .is_some_and(|key| project.station_comments.contains_key(&key));
            if holds {
                sources.push(Holder::Station(StationRefT { section, index }));
            }
        }
        for (section, id, uuid) in &targets.shot_uuids {
            let holds = project
                .get_section(*section)
                .and_then(|s| s.shots.iter().find(|shot| shot.id == *id))
                .is_some_and(|shot| shot.comment.is_some());
            if holds {
                sources.push(Holder::Shot(ShotRefT {
                    section: *section,
                    id: *id,
                    uuid: Some(uuid.clone()),
                }));
            }
        }
        if sources.is_empty() || targets.stations.is_empty() {
            return None;
        }
        let source = sources.swap_remove(self.aux.below(sources.len()));
        let target = if self.aux.chance(50) {
            let (section, index) = targets.stations[self.aux.below(targets.stations.len())];
            Holder::Station(StationRefT { section, index })
        } else {
            Holder::Shot(self.pick_shot_ref(targets)?)
        };
        Some(ExtOp::MoveComment {
            source,
            target,
            swap: self.aux.chance(30),
        })
    }

    fn draw_untrack(&mut self, targets: &Targets) -> Option<ExtOp> {
        let project = self.state.get_project_state()?;
        let delete = (!targets.sections.is_empty() && self.aux.chance(50))
            .then(|| targets.sections[self.aux.below(targets.sections.len())].0);
        let mut pick = |all: &[String]| -> Vec<String> {
            all.iter()
                .filter(|_| self.aux.chance(30))
                .cloned()
                .collect()
        };
        let file_hashes = pick(&project.imported_file_hashes);
        let section_hashes = pick(&project.imported_section_hashes);
        (!file_hashes.is_empty() || !section_hashes.is_empty()).then_some(ExtOp::UntrackImport {
            delete,
            file_hashes,
            section_hashes,
        })
    }

    /// Execute one extended action. The `apply_op` of `Op::Extended`.
    pub(super) fn apply_extended(
        &mut self,
        ext: &ExtOp,
        refs: &mut Refs,
        targets: &Targets,
    ) -> Option<Drawn> {
        self.outcome.coverage.extended += 1;
        let label = ext.describe();
        let result = match ext {
            ExtOp::EditCredit {
                section,
                shots,
                role,
                person,
            } => {
                let credit_targets = if let Some(id) = section {
                    let Some(id) = refs.section(*id, targets) else {
                        return refs.skip(&label);
                    };
                    CreditTargets::Section { section_id: id }
                } else {
                    let mut resolved: Vec<_> =
                        shots.iter().filter_map(|s| refs.shot(s, targets)).collect();
                    resolved.dedup();
                    if resolved.is_empty() {
                        return refs.skip(&label);
                    }
                    CreditTargets::Shots { shots: resolved }
                };
                self.state.execute(Command::EditShotCredit {
                    targets: credit_targets,
                    edit: credit_edit(*role, *person),
                })
            }
            ExtOp::UnmergeStation { station } => {
                let Some((section, index)) = refs.station(*station, targets) else {
                    return refs.skip(&label);
                };
                self.state.execute(Command::UnmergeStations {
                    station_ref: StationRef::new(section, index),
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
                self.state.execute(Command::MoveComment {
                    source,
                    target,
                    swap: *swap,
                })
            }
            ExtOp::ProjectSettings { rename, nudge } => {
                let project = self.state.get_project_state()?;
                let name = rename.map_or(project.name.clone(), |n| format!("RustFuzz-{n}"));
                let mut validation = project.validation;
                validation.warn_shot_length_max = 50.0 + f64::from(*nudge);
                self.state.update_project_settings(name, validation)
            }
            ExtOp::SurfaceElevation { millimetres } => self
                .state
                .set_surface_elevation(millimetres.map(|mm| mm as f64 / 1000.0)),
            ExtOp::UntrackImport {
                delete,
                file_hashes,
                section_hashes,
            } => {
                let mut commands = Vec::new();
                if let Some(section_id) = delete.and_then(|id| refs.section(id, targets)) {
                    commands.push(Command::DeleteSection { section_id });
                }
                commands.push(Command::UntrackImport {
                    file_hashes: file_hashes.clone(),
                    section_hashes: section_hashes.clone(),
                });
                self.state.execute(Command::Batch { commands })
            }
            ExtOp::Import {
                seed,
                sections,
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
                let (configs, file_hash, section_hashes) =
                    import_configs(*seed, usize::from(*sections), anchor, *names);
                self.state.execute_import_sections(
                    configs,
                    Vec::new(),
                    Some(file_hash),
                    section_hashes,
                    Vec::new(),
                    super::super::command_import::VirtualShotMode::default(),
                )
            }
            ExtOp::AddProjectFile {
                seed,
                sections,
                repeats,
            } => {
                let uuids: Vec<String> = repeats
                    .iter()
                    .filter_map(|r| {
                        let (section, id) = refs.shot(r, targets)?;
                        targets.uuid_of(section, id)
                    })
                    .collect();
                let temp = self.temp.get_or_insert_with(|| TempDir::new().unwrap());
                let path: PathBuf = temp.path().join(format!("added-{seed}.stalx"));
                let added = added_project(*seed, usize::from(*sections), &uuids);
                Database::open(&path)
                    .and_then(|mut db| db.save_project(&added))
                    .expect("write the project to add");
                let result = self.state.execute(Command::add_project_file(path.clone()));
                let _ = std::fs::remove_file(&path);
                result
            }
        };
        Some(Drawn::new(label, result))
    }
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
        Holder::Station(at) => {
            let (section, index) = refs.station(*at, targets)?;
            CommentHolder::Station {
                station_ref: StationRef::new(section, index),
            }
        }
        Holder::Shot(shot) => {
            let (section_id, shot_id) = refs.shot(shot, targets)?;
            CommentHolder::Shot {
                section_id,
                shot_id,
            }
        }
    })
}

/// A synthetic import of `count` sections, a function of `seed`: the first
/// placed on `anchor` (or at the origin), each later one hung off the one
/// before it, with station names and comments on some stations. Returns the
/// configs, the file hash and one section hash per section.
fn import_configs(
    seed: u32,
    count: usize,
    anchor: Option<(SectionId, u32)>,
    names: bool,
) -> (Vec<ImportSectionConfig>, String, Vec<String>) {
    let mut rng = Rng::new(seed);
    let file_hash = format!("fuzz-file-{seed:08x}");
    let mut configs = Vec::new();
    let mut hashes = Vec::new();
    for index in 0..count {
        let shots: Vec<ShotData> = (0..=rng.below(4)).map(|_| random_shot(&mut rng)).collect();
        let stations = u32::try_from(shots.len()).unwrap_or(0);
        let placement = match (index, anchor) {
            (0, Some((section, station))) => ImportPlacement::FromExisting {
                import_station: 0,
                existing_station: StationRef::new(section, station),
            },
            (0, None) => ImportPlacement::Origin,
            _ => ImportPlacement::FromImporting {
                import_station: 0,
                target_section_index: u32::try_from(index - 1).unwrap_or(0),
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
        configs.push(ImportSectionConfig {
            reverse: false,
            section_index: u32::try_from(index).unwrap_or(0),
            name: Some(format!("imported-{seed:x}-{index}")),
            surveyor: None,
            date: None,
            color: (*rng.pick(COLORS)).to_string(),
            direction: None,
            quality: 1.0,
            shots,
            station_comments,
            station_names,
            placement,
            azimuth_reference: None,
            explorers: None,
            override_date: None,
            surveyors: None,
        });
    }
    (configs, file_hash, hashes)
}

/// The second project an Add .stalx brings in, a function of `seed`: a chain of
/// `count` sections, the first `repeats.len()` shots carrying uuids the open
/// project already holds, station names on every section and a station offset
/// on the last one (whose far end nothing is merged to).
fn added_project(seed: u32, count: usize, repeats: &[String]) -> Project {
    let mut rng = Rng::new(seed);
    let mut project = Project::new(format!("Added {seed}"), None);
    let mut ids = Vec::new();
    for index in 0..count {
        let shots: Vec<ShotData> = (0..2 + rng.below(3))
            .map(|_| random_shot(&mut rng))
            .collect();
        let data = SectionData {
            name: Some(format!("added-{seed:x}-{index}")),
            color: (*rng.pick(COLORS)).to_string(),
            shots,
            ..SectionData::default()
        };
        ids.push(project.add_section(data).id);
    }
    // The repeats: a later shot of the file takes a uuid the open project holds.
    let mut slots: Vec<(usize, usize)> = project
        .sections
        .iter()
        .enumerate()
        .flat_map(|(s, section)| (0..section.shots.len()).map(move |i| (s, i)))
        .collect();
    slots.reverse();
    for (uuid, (s, i)) in repeats.iter().zip(slots) {
        project.sections[s].shots[i].uuid = Some(uuid.clone());
    }
    project.rebuild_shot_uuid_index();
    for pair in ids.windows(2) {
        let last =
            u32::try_from(project.get_section(pair[0]).map_or(0, |s| s.shots.len())).unwrap_or(0);
        let _ = project.merge_stations(StationRef::new(pair[1], 0), StationRef::new(pair[0], last));
    }
    for (index, id) in ids.iter().enumerate() {
        if let Some(key) = project.key_for_ref(StationRef::new(*id, 1)) {
            project
                .station_names
                .insert(key, format!("A{seed:x}-{index}"));
        }
    }
    if let Some(key) = ids
        .last()
        .and_then(|id| project.key_for_ref(StationRef::new(*id, 1)))
    {
        project.station_offsets.insert(key, (0.25, -0.5, 0.0));
    }
    project
}

/// Grow the live project into the LARGE one, then save and reopen it so the run
/// starts from what the loader makes of it. Returns the dir holding its file.
pub(super) fn grow_large(state: &super::AppState, aux: &mut Rng) -> TempDir {
    {
        let mut guard = state.project.lock().unwrap();
        let project = guard.as_mut().expect("the builder opened a project");
        let mut chain = Vec::new();
        for index in 0..LARGE_CHAIN {
            let shots: Vec<ShotData> = (0..3).map(|_| random_shot(aux)).collect();
            let data = SectionData {
                name: Some(format!("chain-{index}")),
                color: (*aux.pick(COLORS)).to_string(),
                shots,
                ..SectionData::default()
            };
            chain.push(project.add_section(data).id);
        }
        // Hang the chain off the first ordinary section so it shares its datum.
        let root = project.sections[0].id;
        project
            .merge_stations(StationRef::new(chain[0], 0), StationRef::new(root, 0))
            .expect("hang the chain");
        for pair in chain.windows(2) {
            project
                .merge_stations(StationRef::new(pair[1], 0), StationRef::new(pair[0], 3))
                .expect("chain-merge");
        }
        // A loop across every two links: the far end of a link closes onto the
        // start of the link two back.
        for index in (2..LARGE_CHAIN).step_by(1) {
            let _ = project.create_loop_closure(
                StationRef::new(chain[index], 3),
                StationRef::new(chain[index - 2], 1),
                ClosureStrategy::Move,
            );
        }
        for index in 0..LARGE_ISLANDS {
            let data = SectionData {
                name: Some(format!("island-{index}")),
                color: (*aux.pick(COLORS)).to_string(),
                shots: vec![random_shot(aux), random_shot(aux)],
                ..SectionData::default()
            };
            let id = project.add_section(data).id;
            let section = project.get_section(id).expect("the island just added");
            let shot = section.shots[1].uuid.clone().expect("a minted uuid");
            let parity = project.shot_record_parity(section, Some(&shot));
            project.held_gauges.push(HeldGauge {
                section_id: id,
                shot,
                neighbour: None,
                end: false,
                parity,
            });
        }
        project
            .imported_file_hashes
            .extend((0..LARGE_HASHES).map(|i| format!("large-file-{i:03}")));
        project
            .imported_section_hashes
            .extend((0..LARGE_HASHES).map(|i| format!("large-section-{i:03}")));
        project.touch();
    }
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
        project.station_merges.len() > 64
            && project.loop_closures.len() > 64
            && project.held_gauges.len() > 64
            && project.imported_file_hashes.len() > 64,
        "the LARGE project lost its long lists on the way through the file: merges={} \
         closures={} held gauges={} file hashes={}",
        project.station_merges.len(),
        project.loop_closures.len(),
        project.held_gauges.len(),
        project.imported_file_hashes.len()
    );
    dir
}

// ===========================================================================
// What EXTENDED found
// ===========================================================================

/// OPEN FINDING (EXTENDED, seeds 4, 10, 18, 28 and 30 at 120 moves before the
/// generator was moved onto the batch route): `Command::UntrackImport` run on
/// its own through `execute()` changes the project's import tracking and clears
/// the redo branch, but pushes NO undo entry. `is_undoable` says it is undoable,
/// and `generate_undo_data` has no arm for it, so it falls to `UndoData::None` -
/// the change cannot be undone, and a redo the user had is gone.
///
/// The app's one caller, the DMP wizard's withdrawal, always sends it inside a
/// `Batch` (which records its own snapshot pair), so no UI path reaches it
/// today; the IPC accepts it on its own. Either it records an entry or it is
/// refused outside a batch - this pins the first.
#[test]
#[ignore = "open finding: a standalone UntrackImport pushes no undo entry - see the doc comment"]
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
    let (configs, file_hash, section_hashes) = import_configs(7, 1, None, false);
    let imported = state.execute_import_sections(
        configs,
        Vec::new(),
        Some(file_hash.clone()),
        section_hashes.clone(),
        Vec::new(),
        super::super::command_import::VirtualShotMode::default(),
    );
    assert!(imported.success, "{:?}", imported.error);
    let before = state.get_undo_stack().len();
    let untracked = state.execute(Command::UntrackImport {
        file_hashes: vec![file_hash],
        section_hashes,
    });
    assert!(untracked.success, "{:?}", untracked.error);
    let project = state.get_project_state().unwrap();
    assert!(project.imported_file_hashes.is_empty(), "the untrack ran");
    assert_eq!(
        state.get_undo_stack().len(),
        before + 1,
        "an undoable command that changed the project pushed no undo entry"
    );
}
