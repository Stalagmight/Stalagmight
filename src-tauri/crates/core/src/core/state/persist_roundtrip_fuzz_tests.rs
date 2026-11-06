//! [#576 FZ-PERSIST] ROUND-TRIP fuzzer: what a save writes is what a load reads.
//!
//! # What it adds to the engines next door
//!
//! The file-format engine ([`super::format_property_tests`]) proves the writer and
//! the reader agree with EACH OTHER (save, load, save: a fixpoint) over projects
//! that have no side tables. This engine checks the stronger, user-facing
//! property - the project the user had open when they saved is the project the
//! next open shows them - over a session that also owns every side table a save
//! has to carry by hand:
//!
//! - the survey (sections, shots, kinds, merges, loop closures, zero datums,
//!   section and station offsets, station comments, project settings, entrance
//!   elevation, the default north reference) - compared as the canonical project
//!   snapshot, the integrity verdict and the adjusted geometry;
//! - `display_settings`, `resolved_warnings` (the ignore list, keyed by shot
//!   identity), `speleo_link`, `speleo_undo_chain`, `speleo_undo_watermark`, and
//!   the undo and redo history (the persisted window of it);
//! - and the FILE: every table, row by row ([`dump_database`]), must survive one
//!   more open and save unchanged.
//!
//! # The session
//!
//! One seed is one session: a random project, random edits, undo and redo,
//! random side-table writes, and saves of every kind the session can make - the
//! first save of a new project, Ctrl+S in place, Save As to a new file (which
//! moves the session), and, once the project is SpeleoDB-linked, a detached copy
//! (which does not). A test-side [`SideModel`] holds what each side table should
//! say. After every save the file is copied aside and opened in a fresh
//! [`AppState`], and compared against the live session and the model.
//!
//! # Documented healing the oracle allows (and nothing else)
//!
//! - Shot ids are renumbered by the loader (`section_id * FEATURE_ID_STRIDE +
//!   sequence_order`): the survey is compared id-agnostically, the allocators not
//!   at all (see `fuzz_undo_tests::drop_key`).
//! - The save's own heals are applied to the LIVE project before it is written
//!   (`apply_save_heals`), so the live project after the save is the reference.
//! - An ignore whose shot no longer exists at a save is dropped by that save
//!   (`carry_resolved_warnings`); the model drops it at the same moment.
//! - The history is persisted as a window: the newest `undo_persist_limit` (20)
//!   entries of each stack, all of them for a Full-mode linked project.
//! - A detached copy sheds the link and both undo-chain markers (docs/saving.md).
//!
//! # Running it
//!
//! `persist_roundtrip_fuzz_gate` runs a fixed seed list in CI. The long run:
//!
//! ```text
//! STALAGMIGHT_RUST_PERSIST_SEEDS=500 STALAGMIGHT_RUST_PERSIST_SEED_START=1 \
//!   cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib \
//!   persist_roundtrip_fuzz_sweep -- --ignored --nocapture
//! ```
//!
//! All data is synthetic, built in code; every file lives in a `TempDir`.

#![expect(
    clippy::indexing_slicing,
    clippy::expect_used,
    reason = "test module: see docs/linting.md for the per-test-module expect idiom"
)]
#![expect(
    clippy::print_stdout,
    reason = "a fuzz sweep's per-seed failure lines and summary ARE its output"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::Utc;
use tempfile::TempDir;

use super::format_property_tests::{dump_database, dump_differences, open_fresh};
use super::fuzz_undo_tests::{
    adjusted_diffs, adjusted_geometry_of, build_project, canonical_snapshot, describe_diffs,
    diff_snapshots, integrity_verdict, random_offset, random_shot, read_targets, Rng, COLORS,
};
use super::AppState;
use crate::core::Command;
use crate::models::{
    AzimuthReference, ClosureStrategy, CorrectionSource, GpsCoordinates, NorthReference,
    SectionUpdates, ShotKind, ShotUpdates, SpeleoLink, StationRef, UndoMode,
};
use crate::persistence::ResolvedWarning;
use crate::test_helpers::isolated_config;

/// The ignore codes the generator uses. The table takes any text; real codes
/// keep a failure payload readable.
const WARNING_CODES: &[&str] = &[
    "SHOT_LENGTH_MAX",
    "DEPTH_CHANGE_MAX",
    "AZIMUTH_DEVIATION",
    "MISSING_LRUD",
];

/// Display-setting keys: a few ordinary ones, and a few a careless writer would
/// mangle (quotes, a separator, non-ASCII, an empty key).
const DISPLAY_KEYS: &[&str] = &[
    "showLabels",
    "labelDensity",
    "colorMode",
    "it's \"quoted\"",
    "a=b;c",
    "z\u{e9}ro",
    "",
];

/// The persisted history window of a local project (`undo_persist_limit`'s
/// default, `config/defaults.rs`).
const LOCAL_HISTORY_WINDOW: usize = 20;

thread_local! {
    /// The steps of the seed running on this thread, when a sweep asked for them
    /// (`STALAGMIGHT_RUST_PERSIST_TRACE=1`): what a failing seed prints to be
    /// replayed and shrunk by hand.
    static TRACE: std::cell::RefCell<Option<Vec<String>>> = const { std::cell::RefCell::new(None) };
}

/// Start (or stop) recording steps on this thread; returns what was recorded.
pub(super) fn trace_take(start_again: bool) -> Vec<String> {
    TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let taken = trace.take().unwrap_or_default();
        if start_again {
            *trace = Some(Vec::new());
        }
        taken
    })
}

/// Record one step, if recording.
pub(super) fn trace(line: impl FnOnce() -> String) {
    TRACE.with(|trace| {
        if let Some(lines) = trace.borrow_mut().as_mut() {
            lines.push(line());
        }
    });
}

fn tracing() -> bool {
    TRACE.with(|trace| trace.borrow().is_some())
}

/// What each side table should say, kept by the test.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct SideModel {
    pub(super) display: BTreeMap<String, String>,
    /// `(shot uuid, warning code)` -> comment.
    pub(super) ignores: BTreeMap<(String, String), Option<String>>,
    pub(super) link: Option<SpeleoLink>,
    pub(super) chain: Option<String>,
    pub(super) watermark: Option<String>,
}

impl SideModel {
    /// What a file written in a mode that sheds the link holds.
    pub(super) fn unlinked(&self) -> Self {
        Self {
            link: None,
            chain: None,
            watermark: None,
            ..self.clone()
        }
    }

    /// A save drops every ignore whose shot is gone (`carry_resolved_warnings`).
    pub(super) fn prune_dead_ignores(&mut self, state: &AppState) {
        let live = live_uuids(state);
        self.ignores.retain(|(uuid, _), _| live.contains(uuid));
    }
}

/// One fuzz session: the live state, its model, and where its files go.
pub(super) struct Session {
    pub(super) state: AppState,
    pub(super) model: SideModel,
    pub(super) dir: TempDir,
    /// The file the session is on, once it has one.
    pub(super) file: Option<PathBuf>,
    files_made: usize,
    /// What the session did, for the sweep's coverage line.
    pub(super) counts: BTreeMap<&'static str, usize>,
}

impl Session {
    /// A random project with history, not saved yet.
    pub(super) fn new(rng: &mut Rng) -> Self {
        let state = AppState::new().expect("AppState::new");
        let closures = rng.chance(50);
        build_project(&state, rng, closures);
        let dir = TempDir::new().expect("temp dir");
        Self {
            state,
            model: SideModel::default(),
            dir,
            file: None,
            files_made: 0,
            counts: BTreeMap::new(),
        }
    }

    /// A fresh file name in the session's folder.
    pub(super) fn next_path(&mut self) -> PathBuf {
        self.files_made += 1;
        self.dir
            .path()
            .join(format!("Cave{}.stalx", self.files_made))
    }

    /// Count one thing the session did.
    pub(super) fn count(&mut self, what: &'static str) {
        trace(|| what.to_owned());
        *self.counts.entry(what).or_insert(0) += 1;
    }

    /// What the file a save just wrote must hold - and, unless it was a detached
    /// copy, the session's model from here on.
    ///
    /// A save rewrites the open file (or moves the session onto the new one)
    /// without the ignores of shots that are gone. A DETACHED copy is written
    /// beside an open file it leaves as it was, dead ignore rows and all - and an
    /// undo that brings the shot back brings its ignore back with it.
    pub(super) fn model_after_save(&mut self, detached: bool) -> SideModel {
        let mut written = self.model.clone();
        written.prune_dead_ignores(&self.state);
        if detached {
            written.unlinked()
        } else {
            self.model = written.clone();
            written
        }
    }

    /// Whether the session has a database to write side tables into.
    pub(super) fn has_database(&self) -> bool {
        self.state.database.lock().is_some()
    }
}

// ===========================================================================
// Generators
// ===========================================================================

/// One random survey edit through the command layer. A command the app refuses
/// (a merge it rejects, a closure with no loop) is fine: the session goes on.
pub(super) fn edit(state: &AppState, rng: &mut Rng) {
    let targets = read_targets(state);
    let station = |rng: &mut Rng| {
        (!targets.stations.is_empty()).then(|| targets.stations[rng.below(targets.stations.len())])
    };
    let populated: Vec<&(u32, Vec<u32>)> = targets
        .sections
        .iter()
        .filter(|(_, shots)| !shots.is_empty())
        .collect();
    let shot = |rng: &mut Rng| {
        (!populated.is_empty()).then(|| {
            let (section_id, shots) = populated[rng.below(populated.len())];
            (*section_id, shots[rng.below(shots.len())])
        })
    };
    let section = |rng: &mut Rng| {
        (!targets.sections.is_empty())
            .then(|| targets.sections[rng.below(targets.sections.len())].0)
    };
    let command = match rng.below(17) {
        0 => Command::CreateSection {
            name: rng.chance(70).then(|| format!("rt-{}", rng.range(0, 9999))),
            surveyor: rng.chance(50).then(|| "Diver A".to_owned()),
            date: None,
            color: (*rng.pick(COLORS)).to_owned(),
            direction: None,
            shots: (0..rng.below(4)).map(|_| random_shot(rng)).collect(),
        },
        1 | 2 => {
            let Some(section_id) = section(rng) else {
                return;
            };
            let after = targets
                .shots_of(section_id)
                .filter(|shots| !shots.is_empty() && rng.chance(60))
                .map(|shots| shots[rng.below(shots.len())]);
            Command::CreateShot {
                section_id,
                shot: random_shot(rng),
                after_shot_id: after,
                before_shot_id: None,
            }
        }
        3 | 4 => {
            let Some((section_id, shot_id)) = shot(rng) else {
                return;
            };
            Command::UpdateShot {
                section_id,
                shot_id,
                updates: ShotUpdates {
                    length: rng.chance(60).then(|| rng.float(0.5, 20.0)),
                    azimuth_in: rng.chance(30).then(|| rng.float(0.0, 359.9)),
                    depth_out: rng.chance(30).then(|| rng.float(0.0, 40.0)),
                    pitch_in: rng
                        .chance(20)
                        .then(|| rng.chance(50).then(|| rng.float(-80.0, 80.0))),
                    left: rng
                        .chance(20)
                        .then(|| rng.chance(70).then(|| rng.float(0.0, 6.0))),
                    ..ShotUpdates::default()
                },
            }
        }
        5 => {
            let Some((section_id, shot_id)) = shot(rng) else {
                return;
            };
            Command::DeleteShot {
                section_id,
                shot_id,
            }
        }
        6 => {
            let Some((section_id, shot_id)) = shot(rng) else {
                return;
            };
            Command::SetShotKind {
                section_id,
                shot_id,
                kind: *rng.pick(&[ShotKind::Normal, ShotKind::Jump, ShotKind::Reference]),
            }
        }
        7 => {
            let Some((section_id, index)) = station(rng) else {
                return;
            };
            Command::SetStationComment {
                station_ref: StationRef::new(section_id, index),
                comment: rng
                    .chance(80)
                    .then(|| format!("note {} \"q\" \u{e9}", rng.range(0, 999))),
            }
        }
        8 => {
            let (Some(a), Some(b)) = (station(rng), station(rng)) else {
                return;
            };
            if a.0 == b.0 {
                return;
            }
            Command::MergeStations {
                station_a: StationRef::new(a.0, a.1),
                station_b: StationRef::new(b.0, b.1),
            }
        }
        9 => {
            if targets.merges.is_empty() {
                return;
            }
            let (a, _) = targets.merges[rng.below(targets.merges.len())];
            Command::UnmergeStations {
                station_ref: StationRef::new(a.0, a.1),
            }
        }
        10 => {
            if !targets.closures.is_empty() && rng.chance(40) {
                Command::RemoveLoopClosure {
                    closure_id: targets.closures[rng.below(targets.closures.len())],
                }
            } else {
                let (Some(a), Some(b)) = (station(rng), station(rng)) else {
                    return;
                };
                Command::CreateLoopClosure {
                    station_a: StationRef::new(a.0, a.1),
                    station_b: StationRef::new(b.0, b.1),
                    closure_strategy: *rng.pick(&[
                        ClosureStrategy::Move,
                        ClosureStrategy::ProportionalByLength,
                        ClosureStrategy::EqualDistribution,
                    ]),
                }
            }
        }
        11 => {
            if !targets.datums.is_empty() && rng.chance(40) {
                let (section_id, index) = targets.datums[rng.below(targets.datums.len())];
                Command::RemoveZeroDatum {
                    station_ref: StationRef::new(section_id, index),
                }
            } else {
                let Some((section_id, index)) = station(rng) else {
                    return;
                };
                Command::SetZeroDatum {
                    station_ref: StationRef::new(section_id, index),
                    coordinates: GpsCoordinates {
                        latitude: rng.float(-60.0, 60.0),
                        longitude: rng.float(-170.0, 170.0),
                        depth_reference: rng.float(0.0, 5.0),
                    },
                }
            }
        }
        12 => {
            let Some(section_id) = section(rng) else {
                return;
            };
            Command::MoveSection {
                section_id,
                offset: random_offset(rng),
            }
        }
        13 => {
            let Some((section_id, index)) = station(rng) else {
                return;
            };
            Command::MoveStation {
                station_ref: StationRef::new(section_id, index),
                offset: random_offset(rng),
            }
        }
        14 => {
            let Some(section_id) = section(rng) else {
                return;
            };
            Command::UpdateSectionMetadata {
                section_id,
                updates: SectionUpdates {
                    name: rng.chance(60).then(|| {
                        rng.chance(80)
                            .then(|| format!("renamed-{}", rng.range(0, 99)))
                    }),
                    surveyor: rng
                        .chance(30)
                        .then(|| rng.chance(50).then(|| "Diver B".to_owned())),
                    color: rng.chance(30).then(|| (*rng.pick(COLORS)).to_owned()),
                    ..SectionUpdates::default()
                },
            }
        }
        15 => {
            let Some(section_id) = section(rng) else {
                return;
            };
            Command::ReverseSection {
                section_id,
                swap_left_right: rng.chance(50),
            }
        }
        _ => {
            if rng.chance(50) {
                let mut validation = state
                    .get_project_state()
                    .map(|p| p.validation)
                    .unwrap_or_default();
                validation.warn_shot_length_max = rng.float(10.0, 80.0);
                let reference = rng.chance(60).then(|| AzimuthReference {
                    north: *rng.pick(&[NorthReference::Magnetic, NorthReference::True]),
                    correction: rng.float(-20.0, 20.0),
                    source: *rng.pick(&[CorrectionSource::Unknown, CorrectionSource::FileDeclared]),
                });
                // Drawn after the reference, as the call's first argument was.
                let name = format!("Cave {}", rng.range(0, 99));
                // [#576 FZP3 triage] Traced like every other edit: it returns before
                // the trace push below, so a seed's trace never showed this call.
                let described = tracing().then(|| {
                    format!(
                        "UpdateProjectSettings {{ name: {name:?}, validation: {validation:?}, \
                         reference: {reference:?} }}"
                    )
                });
                let result = state.update_project_settings(name, validation, reference);
                if let Some(described) = described {
                    trace(|| format!("edit {} -> {}", described, result.success));
                }
            } else {
                let elevation = rng.chance(70).then(|| rng.float(-50.0, 3000.0));
                let result = state.set_surface_elevation(elevation);
                trace(|| {
                    format!(
                        "edit SetSurfaceElevation {{ elevation: {elevation:?} }} -> {}",
                        result.success
                    )
                });
            }
            return;
        }
    };
    let described = tracing().then(|| format!("{command:?}"));
    let result = if rng.chance(8) {
        state.execute(Command::Batch {
            commands: vec![command],
        })
    } else {
        state.execute(command)
    };
    if let Some(described) = described {
        trace(|| format!("edit {} -> {}", described, result.success));
    }
}

/// One random side-table write, mirrored into the model. Only what the app
/// itself can do: an ignore and a link need a database, display settings go to
/// the in-memory cache of a project that has none.
pub(super) fn side_write(session: &mut Session, rng: &mut Rng) {
    let state = &session.state;
    match rng.below(10) {
        0..=2 => {
            let key = (*rng.pick(DISPLAY_KEYS)).to_owned();
            let value = format!("v{}-{}", rng.range(0, 999), "x".repeat(rng.below(4)));
            if state.save_display_setting(&key, &value).is_ok() {
                session.model.display.insert(key, value);
                session.count("display-set");
            }
        }
        3..=5 => {
            if !session.has_database() {
                return;
            }
            let targets = read_targets(state);
            if targets.shot_uuids.is_empty() {
                return;
            }
            let (section_id, shot_id, uuid) =
                targets.shot_uuids[rng.below(targets.shot_uuids.len())].clone();
            let code = (*rng.pick(WARNING_CODES)).to_owned();
            if session
                .model
                .ignores
                .contains_key(&(uuid.clone(), code.clone()))
            {
                return;
            }
            let comment = rng.chance(50).then(|| format!("ok {}", rng.range(0, 99)));
            add_ignore(session, (section_id, shot_id, uuid), code, comment);
        }
        6 => {
            if session.model.ignores.is_empty() || !session.has_database() {
                return;
            }
            let keys: Vec<(String, String)> = session.model.ignores.keys().cloned().collect();
            let (uuid, code) = keys[rng.below(keys.len())].clone();
            let Some((section_id, shot_id)) = read_targets(state).by_uuid(&uuid) else {
                return;
            };
            if state
                .delete_resolved_warning(section_id, shot_id, &code, Some(&uuid))
                .is_ok()
            {
                trace(|| format!("unignore {code} on {section_id}/{shot_id} uuid {uuid}"));
                session.model.ignores.remove(&(uuid, code));
                session.count("ignore-removed");
            }
        }
        7 => {
            let link = rng.chance(75).then(|| random_link(rng));
            let written = {
                let db_guard = state.database.lock();
                let Some(db) = db_guard.as_ref() else {
                    return;
                };
                db.save_speleo_link(link.as_ref()).is_ok()
            };
            if written {
                session.count(if link.is_some() {
                    "link-set"
                } else {
                    "link-cleared"
                });
                session.model.link = link;
            }
        }
        _ => {
            let value = rng
                .chance(70)
                .then(|| format!("{:040x}", rng.range(1, 1 << 40)));
            let chain = rng.chance(50);
            let written = {
                let db_guard = state.database.lock();
                let Some(db) = db_guard.as_ref() else {
                    return;
                };
                if chain {
                    db.save_speleo_undo_chain(value.as_deref()).is_ok()
                } else {
                    db.save_speleo_undo_watermark(value.as_deref()).is_ok()
                }
            };
            match (written, chain) {
                (false, _) => {}
                (true, true) => {
                    session.model.chain = value;
                    session.count("undo-chain-written");
                }
                (true, false) => {
                    session.model.watermark = value;
                    session.count("watermark-written");
                }
            }
        }
    }
}

/// Ignore `code` on the shot `(section, id, uuid)` as the Ignore command does -
/// keyed by the uuid once it is on disk, by position before (issue #57) - and
/// mirror it into the model.
fn add_ignore(
    session: &mut Session,
    (section_id, shot_id, uuid): (u32, u32, String),
    code: String,
    comment: Option<String>,
) {
    let state = &session.state;
    let shot_uuid = state
        .shot_uuid_is_persisted(section_id, &uuid)
        .then(|| uuid.clone());
    let row = ResolvedWarning {
        section_id,
        shot_id,
        warning_code: code.clone(),
        resolved_at: Utc::now(),
        resolved_comment: comment.clone(),
        shot_uuid,
    };
    if state.save_resolved_warning(&row).is_ok() {
        trace(|| format!("ignore {code} on {section_id}/{shot_id} uuid {uuid}"));
        session.model.ignores.insert((uuid, code), comment);
        session.count("ignore-added");
    }
}

fn random_link(rng: &mut Rng) -> SpeleoLink {
    SpeleoLink {
        endpoint: "https://speleodb.example.invalid".to_owned(),
        project_id: format!("00000000-0000-4000-8000-{:012x}", rng.range(0, 1 << 40)),
        project_name: format!("Remote Cave {}", rng.range(0, 99)),
        base_commit_sha: rng
            .chance(80)
            .then(|| format!("{:040x}", rng.range(1, 1 << 40))),
        has_unpushed_edits: rng.chance(50),
        last_edit_at: rng.chance(50).then(|| "2026-01-02T03:04:05Z".to_owned()),
        last_verify_at: rng.chance(50).then(|| "2026-01-02T03:04:06Z".to_owned()),
        // `None` is a legacy initialization marker, not a mode: left out so the
        // history window the oracle expects is the one a real link has.
        undo_mode: Some(*rng.pick(&[UndoMode::Full, UndoMode::Disabled])),
    }
}

fn live_uuids(state: &AppState) -> BTreeSet<String> {
    read_targets(state)
        .shot_uuids
        .into_iter()
        .map(|(_, _, uuid)| uuid)
        .collect()
}

// ===========================================================================
// The oracle
// ===========================================================================

/// Everything a reopened file is compared on, read from a state.
pub(super) struct Observed {
    pub(super) survey: String,
    pub(super) verdict: String,
    pub(super) adjusted: String,
    /// `command_type|description|timestamp`, newest first.
    pub(super) undo: Vec<String>,
    pub(super) redo: Vec<String>,
}

pub(super) fn observe_state(state: &AppState) -> Observed {
    let history = |items: Vec<crate::core::undo::history_view::UndoHistoryItem>| {
        items
            .into_iter()
            .map(|item| {
                format!(
                    "{}|{}|{}",
                    item.command_type,
                    item.description,
                    item.timestamp.unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
    };
    Observed {
        survey: canonical_snapshot(state, true),
        verdict: integrity_verdict(state).describe(),
        adjusted: adjusted_geometry_of(state),
        undo: history(state.get_undo_stack()),
        redo: history(state.get_redo_stack()),
    }
}

/// What a file's side tables hold, read the way the app reads them, and the
/// IRREGULAR ignore rows, one line each.
///
/// An ignore is matched the way `compute_project_warnings` matches it: a
/// uuid-keyed row by `(section, uuid)`, wherever that shot now is; a legacy
/// position-keyed row by `(section, shot id)`. A row is irregular when it names
/// no shot of the file (a position row for a shot that only ever existed in
/// memory, a uuid row for a deleted shot), or when a uuid row's stored position
/// is not where its shot is: the open file is written with the session's shot
/// ids, which drift from the file's positions after an insert and a save. A file
/// a save COMPLETED holds none (`carry_resolved_warnings` re-keys and drops);
/// a file a kill interrupted can, the guarded upsert (`save_resolved_warning`)
/// keeps a stale slot from overwriting another shot's ignore, and the next save
/// re-keys or drops them.
pub(super) fn side_tables_of(state: &AppState) -> Result<(SideModel, Vec<String>), String> {
    let display = state
        .load_display_settings()
        .map_err(|e| format!("display settings unreadable: {e}"))?
        .into_iter()
        .collect();
    let project = state.get_project_state().ok_or("no project open")?;
    let mut ignores = BTreeMap::new();
    let mut dead = Vec::new();
    for row in state
        .load_resolved_warnings()
        .map_err(|e| format!("ignores unreadable: {e}"))?
    {
        let section = project.sections.iter().find(|s| s.id == row.section_id);
        let at_position = section
            .and_then(|s| s.shots.iter().find(|sh| sh.id == row.shot_id))
            .and_then(|sh| sh.uuid.clone());
        let in_section = |uuid: &str| {
            section.is_some_and(|s| s.shots.iter().any(|sh| sh.uuid.as_deref() == Some(uuid)))
        };
        let described = format!(
            "{} on section {} shot {} (row uuid {:?}, shot there {at_position:?})",
            row.warning_code, row.section_id, row.shot_id, row.shot_uuid
        );
        let uuid = match (row.shot_uuid, at_position) {
            (Some(stored), Some(found)) if stored == found => found,
            (Some(stored), _) if in_section(&stored) => {
                dead.push(format!("stale position: {described}"));
                stored
            }
            (None, Some(found)) => found,
            (Some(_) | None, _) => {
                dead.push(format!("names no shot: {described}"));
                continue;
            }
        };
        if ignores
            .insert((uuid, row.warning_code.clone()), row.resolved_comment)
            .is_some()
        {
            return Err(format!(
                "ignore {} stored twice for one shot",
                row.warning_code
            ));
        }
    }
    let db_guard = state.database.lock();
    let db = db_guard.as_ref().ok_or("no database open")?;
    let model = SideModel {
        display,
        ignores,
        link: db
            .load_speleo_link()
            .map_err(|e| format!("link unreadable: {e}"))?,
        chain: db
            .load_speleo_undo_chain()
            .map_err(|e| format!("undo chain unreadable: {e}"))?,
        watermark: db
            .load_speleo_undo_watermark()
            .map_err(|e| format!("watermark unreadable: {e}"))?,
    };
    Ok((model, dead))
}

/// The newest entries of a history a save keeps: all of a Full-mode linked
/// project's, the local window of everyone else's.
fn persisted_window(history: &[String], link: Option<&SpeleoLink>) -> Vec<String> {
    let unlimited = link.is_some_and(|l| l.undo_mode == Some(UndoMode::Full));
    let keep = if unlimited {
        history.len()
    } else {
        history.len().min(LOCAL_HISTORY_WINDOW)
    };
    history[..keep].to_vec()
}

/// Every way the file at `path` differs from what the session saved into it.
///
/// `expected` is the live observation taken right after the save, `model` what
/// the side tables must say. The file is copied first, so neither the reopen nor
/// the re-save below touches the file the live session has open.
pub(super) fn check_file(path: &Path, expected: &Observed, model: &SideModel) -> Vec<String> {
    let mut problems = Vec::new();
    let Ok(scratch) = TempDir::new() else {
        return vec!["no scratch dir".to_owned()];
    };
    let copy = scratch.path().join("Reopened.stalx");
    if let Err(e) = std::fs::copy(path, &copy) {
        return vec![format!("the saved file cannot be copied: {e}")];
    }
    let Some(reopened) = open_fresh(&copy) else {
        return vec!["the saved file does not open".to_owned()];
    };
    let got = observe_state(&reopened);
    let diffs = diff_snapshots(&expected.survey, &got.survey);
    if !diffs.is_empty() {
        problems.push(format!(
            "the reopened survey differs from the saved one in {} path(s):\n{}",
            diffs.len(),
            describe_diffs(&diffs)
        ));
    }
    if expected.verdict != got.verdict {
        problems.push(format!(
            "integrity verdict changed across the save: saved {}, reopened {}",
            expected.verdict, got.verdict
        ));
    }
    let geometry = adjusted_diffs(&expected.adjusted, &got.adjusted);
    if !geometry.is_empty() {
        problems.push(format!(
            "adjusted geometry changed: {}",
            geometry.join("; ")
        ));
    }
    let want_undo = persisted_window(&expected.undo, model.link.as_ref());
    let want_redo = persisted_window(&expected.redo, model.link.as_ref());
    if got.undo != want_undo || got.redo != want_redo {
        problems.push(format!(
            "history differs: undo saved {} kept {} (want {}), redo saved {} kept {} (want {}); \
             first undo saved {:?} reopened {:?}",
            expected.undo.len(),
            got.undo.len(),
            want_undo.len(),
            expected.redo.len(),
            got.redo.len(),
            want_redo.len(),
            want_undo.first(),
            got.undo.first()
        ));
    }
    match side_tables_of(&reopened) {
        Ok((sides, dead)) => {
            if !dead.is_empty() {
                problems.push(format!(
                    "a completed save left irregular ignore rows: {dead:?}"
                ));
            }
            if sides.display != model.display {
                problems.push(format!(
                    "display settings: want {:?}, file {:?}",
                    model.display, sides.display
                ));
            }
            if sides.ignores != model.ignores {
                problems.push(format!(
                    "ignore list: want {:?}, file {:?}",
                    model.ignores, sides.ignores
                ));
            }
            if sides.link != model.link {
                problems.push(format!(
                    "link: want {:?}, file {:?}",
                    model.link, sides.link
                ));
            }
            if (sides.chain.as_ref(), sides.watermark.as_ref())
                != (model.chain.as_ref(), model.watermark.as_ref())
            {
                problems.push(format!(
                    "undo markers: want chain {:?} watermark {:?}, file {:?} {:?}",
                    model.chain, model.watermark, sides.chain, sides.watermark
                ));
            }
        }
        Err(e) => problems.push(e),
    }
    // The file itself: one more open and save in place changes no row.
    let before = dump_database(&copy);
    let resaved = reopened.save_project(None);
    drop(reopened);
    if resaved.success {
        match (before, dump_database(&copy)) {
            (Ok(before), Ok(after)) => {
                let rows = dump_differences(&before, &after, ("saved", "re-saved"));
                if !rows.is_empty() {
                    problems.push(format!(
                        "an open and save in place changed the file's rows:\n{}",
                        rows.join("\n")
                    ));
                }
            }
            (Err(e), _) | (_, Err(e)) => problems.push(format!("cannot dump: {e}")),
        }
    } else if model.link.is_none() {
        // A linked file reopened outside the managed cache may be refused a
        // save by design; an unlinked one never is.
        problems.push(format!(
            "the reopened file cannot be saved in place: {:?}",
            resaved.error
        ));
    }
    problems
}

// ===========================================================================
// One seed
// ===========================================================================

/// What a session's save was.
#[derive(Clone, Copy, Debug)]
enum SaveKind {
    InPlace,
    SaveAs,
}

/// Run one seed; every problem found, each prefixed with the step it was found at.
pub(super) fn run_seed(seed: u32, steps: usize) -> (Vec<String>, BTreeMap<&'static str, usize>) {
    let mut rng = Rng::new(seed);
    let mut session = Session::new(&mut rng);
    let mut problems = Vec::new();
    // Display settings before the first save live in the in-memory cache.
    for _ in 0..rng.below(3) {
        side_write(&mut session, &mut rng);
    }
    let first = session.next_path();
    save_and_check(&mut session, Some(first), "first save", &mut problems);
    for step in 0..steps {
        if problems.len() > 4 {
            break;
        }
        trace(|| format!("-- step {step}"));
        match rng.below(20) {
            0..=7 => {
                edit(&session.state, &mut rng);
                session.count("edit");
            }
            8 | 9 => {
                if session.state.undo().success {
                    session.count("undo");
                }
            }
            10 => {
                if session.state.redo().success {
                    session.count("redo");
                }
            }
            11..=14 => side_write(&mut session, &mut rng),
            15 | 16 => save_and_check(
                &mut session,
                None,
                &format!("step {step} save"),
                &mut problems,
            ),
            17 => {
                // A new file, or over an older file of this session that is not
                // the open one: nothing of what it held may bleed into the save.
                let older = (session.files_made > 1 && rng.chance(35))
                    .then(|| {
                        let k = rng.range(1, session.files_made as i64);
                        session.dir.path().join(format!("Cave{k}.stalx"))
                    })
                    .filter(|p| p.exists() && session.file.as_ref() != Some(p));
                if older.is_some() {
                    session.count("save-as-over-older-file");
                }
                let to = older.unwrap_or_else(|| session.next_path());
                save_and_check(
                    &mut session,
                    Some(to),
                    &format!("step {step} save as"),
                    &mut problems,
                );
            }
            _ => reopen_session(&mut session, &format!("step {step} reopen"), &mut problems),
        }
    }
    save_and_check(&mut session, None, "final save", &mut problems);
    let problems = problems
        .into_iter()
        .map(|p| format!("seed {seed}: {p}"))
        .collect();
    (problems, session.counts)
}

/// Save, then check the file against the live session and the model.
fn save_and_check(
    session: &mut Session,
    to: Option<PathBuf>,
    label: &str,
    problems: &mut Vec<String>,
) {
    let kind = match (&to, &session.file) {
        (Some(to), Some(open)) if to != open => SaveKind::SaveAs,
        (Some(_), None) => SaveKind::SaveAs,
        _ => SaveKind::InPlace,
    };
    if let Some(problem) = adopt_live_bookkeeping(session) {
        problems.push(format!("{label}: before the save, {problem}"));
    }
    let linked = session.model.link.is_some();
    // A project with an unresolvable cross-reference is refused by design
    // (`model_write_refusal`) and must write nothing; the session carries on.
    let blocked = session
        .state
        .get_project_state()
        .is_some_and(|p| p.has_unresolvable_cross_references());
    let first = session.file.is_none();
    let target = to.clone().or_else(|| session.file.clone());
    let bytes_before = target.as_deref().and_then(|t| std::fs::read(t).ok());
    let result = session.state.save_project(to.clone());
    if blocked {
        session.count("save-refused-dangling-ref");
        let bytes_after = target.as_deref().and_then(|t| std::fs::read(t).ok());
        if result.success {
            problems.push(format!(
                "{label}: a project with a dangling reference was saved"
            ));
        } else if bytes_before != bytes_after {
            problems.push(format!("{label}: a refused save changed the file on disk"));
        }
        return;
    }
    if !result.success {
        problems.push(format!(
            "{label}: the save failed: {:?} {:?}",
            result.code, result.error
        ));
        return;
    }
    let lost: Vec<&str> = result.warnings.iter().map(|w| w.code.as_str()).collect();
    if !lost.is_empty() {
        problems.push(format!("{label}: a clean save warned {lost:?}"));
    }
    let detached = linked && matches!(kind, SaveKind::SaveAs);
    session.count(match (first, kind, detached) {
        (true, ..) => "save-first",
        (_, _, true) => "save-detached-copy",
        (_, SaveKind::SaveAs, _) => "save-as",
        (_, SaveKind::InPlace, _) => "save-in-place",
    });
    if linked && !detached {
        session.count("save-linked");
    }
    let Some(written) = to.or_else(|| session.file.clone()) else {
        problems.push(format!("{label}: an in-place save with no file"));
        return;
    };
    let model = session.model_after_save(detached);
    let expected = observe_state(&session.state);
    for problem in check_file(&written, &expected, &model) {
        problems.push(format!(
            "{label} ({kind:?}{}): {problem}",
            if detached { ", detached copy" } else { "" }
        ));
    }
    if !detached {
        session.file = Some(written);
    }
}

/// Bring the model's SpeleoDB bookkeeping up to what the app itself did since the
/// last save, and nothing more: every accepted edit of a linked project sets the
/// dirty bit and refreshes `last_edit_at` (`mark_remote_dirty`), and every forward
/// edit forgets the Unwind position (`speleodb_forget_unwind_position`). Any other
/// difference between the model and the open database is a write that did not
/// land, reported.
pub(super) fn adopt_live_bookkeeping(session: &mut Session) -> Option<String> {
    let db_guard = session.state.database.lock();
    let db = db_guard.as_ref()?;
    let link = db.load_speleo_link().ok()?;
    let chain = db.load_speleo_undo_chain().ok()?;
    let watermark = db.load_speleo_undo_watermark().ok()?;
    drop(db_guard);
    let model = &mut session.model;
    let link_ok = match (&model.link, &link) {
        (Some(want), Some(got)) => {
            let mut dirtied = want.clone();
            dirtied.has_unpushed_edits = got.has_unpushed_edits;
            dirtied.last_edit_at.clone_from(&got.last_edit_at);
            dirtied == *got && (got.has_unpushed_edits || !want.has_unpushed_edits)
        }
        (want, got) => want == got,
    };
    let chain_ok = chain.is_none() || chain == model.chain;
    if !link_ok || !chain_ok || watermark != model.watermark {
        return Some(format!(
            "the open database's SpeleoDB rows are not what was written: link {:?} vs {:?}, \
             chain {:?} vs {:?}, watermark {:?} vs {:?}",
            model.link, link, model.chain, chain, model.watermark, watermark
        ));
    }
    model.link = link;
    model.chain = chain;
    None
}

/// Close the project and open its file again: the session goes on from what the
/// file says. Unsaved edits are saved first, as the close prompt would.
fn reopen_session(session: &mut Session, label: &str, problems: &mut Vec<String>) {
    let Some(file) = session.file.clone() else {
        return;
    };
    save_and_check(
        session,
        None,
        &format!("{label} (save before close)"),
        problems,
    );
    let Some(state) = open_fresh(&file) else {
        problems.push(format!("{label}: the session's file does not open"));
        return;
    };
    session.state = state;
    session.count("reopen");
}

// ===========================================================================
// Directed regressions for what the sweep found
// ===========================================================================

/// [#576 FZ-PERSIST-1] (sweep seed 3526, before the generator changed) An Ignore
/// on a shot an undo brought back never deletes the ignore of the shot that a
/// save moved into its slot.
///
/// Shot B is ignored; shot A, in front of it, is deleted and the project saved,
/// so the save re-keys B's ignore to the slot A had; the delete is undone, so A
/// is back in the session under that slot's id, its uuid no longer on disk.
/// Ignoring A is then keyed by position - and used to delete B's ignore on its
/// way in.
#[test]
fn an_ignore_on_a_restored_shot_keeps_the_ignore_of_the_shot_in_its_slot() {
    let _config = isolated_config();
    let state = AppState::new().expect("AppState::new");
    assert!(
        state
            .execute(Command::CreateProject {
                name: "CaveA".to_owned(),
                location: None,
            })
            .success
    );
    let mut rng = Rng::new(7);
    let created = state.execute(Command::CreateSection {
        name: Some("SectionA".to_owned()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_owned(),
        direction: None,
        shots: (0..3).map(|_| random_shot(&mut rng)).collect(),
    });
    let section_id = created.section_id.expect("a section");
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("CaveA.stalx");
    assert!(state.save_project(Some(path.clone())).success);

    let targets = read_targets(&state);
    let shots = targets.shots_of(section_id).expect("the section").to_vec();
    let (shot_a, shot_b) = (shots[0], shots[1]);
    let uuid_b = targets.uuid_of(section_id, shot_b).expect("B has a uuid");
    let ignore = |shot_id, shot_uuid: Option<String>, comment: &str| ResolvedWarning {
        section_id,
        shot_id,
        warning_code: "AZIMUTH_DEVIATION".to_owned(),
        resolved_at: Utc::now(),
        resolved_comment: Some(comment.to_owned()),
        shot_uuid,
    };
    state
        .save_resolved_warning(&ignore(shot_b, Some(uuid_b.clone()), "B's"))
        .expect("ignore B");
    assert!(
        state
            .execute(Command::DeleteShot {
                section_id,
                shot_id: shot_a,
            })
            .success
    );
    assert!(state.save_project(None).success);
    assert!(state.undo().success, "undo the delete");
    let uuid_a = read_targets(&state)
        .uuid_of(section_id, shot_a)
        .expect("A is back");
    assert!(
        !state.shot_uuid_is_persisted(section_id, &uuid_a),
        "precondition: A's uuid is not on disk, so its ignore is keyed by position"
    );

    // Ignore A as the Ignore command does. It is refused (its slot is held);
    // deleting B's ignore is not allowed.
    let refusal = state
        .save_resolved_warning(&ignore(shot_a, None, "A's"))
        .err()
        .map(|e| e.to_string());

    let kept = |rows: &[ResolvedWarning], uuid: &str| {
        rows.iter().any(|r| r.shot_uuid.as_deref() == Some(uuid))
    };
    let rows = state.load_resolved_warnings().expect("read the ignores");
    assert!(kept(&rows, &uuid_b), "B's ignore was deleted: {rows:?}");
    // [#576 FZ-PERSIST-1-D1] The refusal asks for a save, and a save is what
    // clears it: it writes A's uuid, so the same Ignore, keyed by it now, goes
    // through.
    assert!(
        refusal
            .as_deref()
            .is_some_and(|m| m.ends_with("save the project and try again")),
        "the refusal asks for a save: {refusal:?}"
    );
    assert!(state.save_project(None).success);
    assert!(
        state.shot_uuid_is_persisted(section_id, &uuid_a),
        "the save wrote A's uuid"
    );
    state
        .save_resolved_warning(&ignore(shot_a, Some(uuid_a.clone()), "A's"))
        .expect("after the save, the same Ignore goes through");
    let reopened = open_fresh(&path).expect("reopen");
    let rows = reopened.load_resolved_warnings().expect("read the ignores");
    assert!(
        kept(&rows, &uuid_b) && kept(&rows, &uuid_a),
        "B's ignore did not survive a save, or A's retried one is missing: {rows:?}"
    );
}

/// [#576 FZP3 triage] The settings edit - the project settings, or the surface
/// elevation - is traced like every other edit. It returned before the trace
/// push, so a seed's trace never showed it, which misled the FZ-PERSIST-3
/// triage.
#[test]
fn the_settings_edit_is_traced_like_every_other_edit() {
    let _config = isolated_config();
    let state = AppState::new().expect("AppState::new");
    assert!(
        state
            .execute(Command::CreateProject {
                name: "CaveA".to_owned(),
                location: None,
            })
            .success
    );
    // A seed whose first edit draws the settings kind, and the sub-branch asked for.
    let settings_seed = |project_settings: bool| {
        (0..u32::MAX)
            .find(|&seed| {
                let mut probe = Rng::new(seed);
                probe.below(17) == 16 && probe.chance(50) == project_settings
            })
            .expect("a seed that takes the settings edit")
    };
    let project_fields: &[&str] = &["name: ", "validation: ", "reference: "];
    let elevation_fields: &[&str] = &["elevation: "];
    for (project_settings, call, set) in [
        (true, "UpdateProjectSettings", project_fields),
        (false, "SetSurfaceElevation", elevation_fields),
    ] {
        let mut rng = Rng::new(settings_seed(project_settings));
        trace_take(true);
        edit(&state, &mut rng);
        let lines = trace_take(false);
        assert!(
            lines.len() == 1
                && lines[0].starts_with(&format!("edit {call} "))
                && set.iter().all(|field| lines[0].contains(field)),
            "the {call} edit is traced with what it set: {lines:?}"
        );
    }
}

/// [#576 FZ-PERSIST-2] (fault sweep seed 27349) An Ignore on a shot not yet saved
/// stays on that shot when its section is reversed and reversed back.
///
/// The ignore is keyed by position (the shot's uuid is not on disk). The first
/// reverse cannot pin it to the shot (`pin_position_keyed_warnings_after_reverse`
/// pins only onto a uuid the file holds), so by position it now names the
/// mirrored shot; the reverse back DOES pin it - to that mirrored shot, whose
/// uuid is on disk. The section is as it was, and the ignore is on another shot
/// for good. `back_by_undo` takes the way back through Undo instead.
///
/// `mirror_code` also ignores the shot the new one mirrors (saved, so keyed by
/// its uuid) for that code. With the SAME code its row held the slot the new
/// shot's ignore had to move to, the move failed, and the old pin came back.
fn reverse_and_back_keeps_an_unsaved_shots_ignore(
    back_by_undo: bool,
    mirror_code: Option<&str>,
) -> Result<(), String> {
    let state = AppState::new().map_err(|e| e.to_string())?;
    state.execute(Command::CreateProject {
        name: "CaveA".to_owned(),
        location: None,
    });
    let mut rng = Rng::new(11);
    let created = state.execute(Command::CreateSection {
        name: Some("SectionA".to_owned()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_owned(),
        direction: None,
        shots: (0..2).map(|_| random_shot(&mut rng)).collect(),
    });
    let section_id = created.section_id.ok_or("no section")?;
    let dir = TempDir::new().map_err(|e| e.to_string())?;
    let path = dir.path().join("CaveA.stalx");
    if !state.save_project(Some(path.clone())).success {
        return Err("first save".to_owned());
    }
    // A shot added since the save: its uuid is not on disk.
    let before = read_targets(&state);
    state.execute(Command::CreateShot {
        section_id,
        shot: random_shot(&mut rng),
        after_shot_id: None,
        before_shot_id: None,
    });
    let targets = read_targets(&state);
    let (_, new_shot, new_uuid) = targets
        .shot_uuids
        .iter()
        .find(|(_, _, uuid)| before.by_uuid(uuid).is_none())
        .cloned()
        .ok_or("no new shot")?;
    let mut want = BTreeSet::from([(new_uuid.clone(), "MISSING_LRUD".to_owned())]);
    let mut ignores = vec![(new_shot, new_uuid.clone(), "MISSING_LRUD")];
    if let Some(code) = mirror_code {
        let (mirror_shot, mirror_uuid) =
            mirror_of(&state, section_id, &new_uuid).ok_or("no mirrored shot")?;
        if !state.shot_uuid_is_persisted(section_id, &mirror_uuid) {
            return Err("the mirrored shot is not on disk".to_owned());
        }
        want.insert((mirror_uuid.clone(), code.to_owned()));
        ignores.push((mirror_shot, mirror_uuid, code));
    }
    for (shot_id, uuid, code) in ignores {
        let shot_uuid = state
            .shot_uuid_is_persisted(section_id, &uuid)
            .then_some(uuid);
        state
            .save_resolved_warning(&ResolvedWarning {
                section_id,
                shot_id,
                warning_code: code.to_owned(),
                resolved_at: Utc::now(),
                resolved_comment: None,
                shot_uuid,
            })
            .map_err(|e| e.to_string())?;
    }
    let reverse = || {
        state.execute(Command::ReverseSection {
            section_id,
            swap_left_right: false,
        })
    };
    if !reverse().success {
        return Err("reverse".to_owned());
    }
    let back = if back_by_undo {
        state.undo()
    } else {
        reverse()
    };
    if !back.success {
        return Err("reverse back".to_owned());
    }
    if !state.save_project(None).success {
        return Err("save".to_owned());
    }
    let reopened = open_fresh(&path).ok_or("reopen")?;
    let (sides, _) = side_tables_of(&reopened)?;
    let on: BTreeSet<(String, String)> = sides.ignores.into_keys().collect();
    if on == want {
        Ok(())
    } else {
        Err(format!(
            "ignores after reopen {on:?}, want {want:?} (new shot {new_uuid}, back by undo: \
             {back_by_undo}, mirrored shot ignored for {mirror_code:?})"
        ))
    }
}

/// The shot a reverse of `section_id` swaps with the one carrying `uuid`, as
/// `(shot id, uuid)`; `None` when there is none, or it is that shot itself.
fn mirror_of(state: &AppState, section_id: u32, uuid: &str) -> Option<(u32, String)> {
    let project = state.get_project_state()?;
    let shots = &project.sections.iter().find(|s| s.id == section_id)?.shots;
    let at = shots.iter().position(|s| s.uuid.as_deref() == Some(uuid))?;
    let mirror = shots.get(shots.len() - 1 - at)?;
    let mirror_uuid = mirror.uuid.clone().filter(|u| u != uuid)?;
    Some((mirror.id, mirror_uuid))
}

#[test]
fn an_unsaved_shots_ignore_stays_on_it_across_a_reverse_and_back() {
    let _config = isolated_config();
    let mut found = Vec::new();
    // Only the new shot ignored; the shot it mirrors ignored for the same code;
    // for another code.
    for mirror_code in [None, Some("MISSING_LRUD"), Some("SHORT_SHOT")] {
        for by_undo in [false, true] {
            found
                .extend(reverse_and_back_keeps_an_unsaved_shots_ignore(by_undo, mirror_code).err());
        }
    }
    assert!(found.is_empty(), "{found:#?}");
}

/// [#576 FZ-PERSIST-2] A directed session the gate runs beside its seeds, as the
/// generator makes it too rarely to rely on: in a random project, a shot added
/// since the save and the saved shot it mirrors are both ignored for ONE code,
/// their section is reversed and brought back by a second reverse, then reversed
/// and brought back by Undo, and the save is checked by the round-trip oracle -
/// which holds the model's two ignores against the file.
fn reverse_and_back_session(seed: u32) -> Vec<String> {
    let mut rng = Rng::new(seed);
    let mut session = Session::new(&mut rng);
    let mut problems = Vec::new();
    let first = session.next_path();
    save_and_check(&mut session, Some(first), "first save", &mut problems);
    let label = format!("directed seed {seed} (reverse and back, by reverse and by undo)");
    let before = read_targets(&session.state);
    let Some(&(section_id, _)) = before.sections.iter().max_by_key(|(_, shots)| shots.len()) else {
        return vec![format!("{label}: no section")];
    };
    session.state.execute(Command::CreateShot {
        section_id,
        shot: random_shot(&mut rng),
        after_shot_id: None,
        before_shot_id: None,
    });
    let Some(new) = read_targets(&session.state)
        .shot_uuids
        .into_iter()
        .find(|(_, _, uuid)| before.by_uuid(uuid).is_none())
    else {
        return vec![format!("{label}: no new shot")];
    };
    let Some((mirror_shot, mirror_uuid)) = mirror_of(&session.state, section_id, &new.2) else {
        return vec![format!("{label}: no mirrored shot")];
    };
    let keys = [&new.2, &mirror_uuid].map(|uuid| (uuid.clone(), "MISSING_LRUD".to_owned()));
    // Keyed by position and by uuid respectively, or this is not the case.
    let on_disk = |uuid: &str| session.state.shot_uuid_is_persisted(section_id, uuid);
    if on_disk(&new.2) || !on_disk(&mirror_uuid) {
        return vec![format!(
            "{label}: the new shot is on disk, or its mirror is not"
        )];
    }
    for target in [new, (section_id, mirror_shot, mirror_uuid)] {
        add_ignore(&mut session, target, "MISSING_LRUD".to_owned(), None);
    }
    if !keys
        .iter()
        .all(|key| session.model.ignores.contains_key(key))
    {
        return vec![format!("{label}: an ignore was refused")];
    }
    let reverse = || {
        session
            .state
            .execute(Command::ReverseSection {
                section_id,
                swap_left_right: false,
            })
            .success
    };
    let there_and_back = reverse() && reverse() && reverse() && session.state.undo().success;
    if !there_and_back {
        return vec![format!("{label}: a reverse or the undo was refused")];
    }
    save_and_check(&mut session, None, &label, &mut problems);
    problems
}

// ===========================================================================
// Gate and sweep
// ===========================================================================

/// The CI seed list, `(seed, steps)`. Seeds that ever failed are added here as
/// regressions, with the step count they failed at.
const GATE_SEEDS: &[(u32, usize)] = &[
    (1, 30),
    (2, 30),
    (3, 30),
    (4, 30),
    (5, 30),
    (6, 30),
    (7, 30),
    (8, 30),
    // An ignore on a shot deleted before a DETACHED copy, brought back by an
    // undo: the copy drops the row, the session's own file keeps it (found by
    // the sweep; the model had pruned the session too).
    (1285, 60),
];

#[test]
fn persist_roundtrip_fuzz_gate() {
    let _config = isolated_config();
    let mut problems = Vec::new();
    for &(seed, steps) in GATE_SEEDS {
        problems.extend(run_seed(seed, steps).0);
    }
    problems.extend(reverse_and_back_session(11));
    assert!(
        problems.is_empty(),
        "round-trip fuzz found {} problem(s):\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// Read a numeric env knob.
pub(super) fn knob(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

#[test]
#[ignore = "long run: STALAGMIGHT_RUST_PERSIST_SEEDS / _SEED_START / _STEPS"]
fn persist_roundtrip_fuzz_sweep() {
    let _config = isolated_config();
    let start = knob("STALAGMIGHT_RUST_PERSIST_SEED_START", 1) as u32;
    let count = knob("STALAGMIGHT_RUST_PERSIST_SEEDS", 200) as u32;
    let steps = knob("STALAGMIGHT_RUST_PERSIST_STEPS", 60) as usize;
    let began = Instant::now();
    let mut failed = 0;
    let mut coverage: BTreeMap<&'static str, usize> = BTreeMap::new();
    let want_trace = knob("STALAGMIGHT_RUST_PERSIST_TRACE", 0) == 1;
    for seed in start..start.saturating_add(count) {
        trace_take(want_trace);
        let (problems, counts) = run_seed(seed, steps);
        if (seed - start + 1) % 100 == 0 {
            // A run cut short by a timeout still says how far it got.
            println!(
                "FZ-PERSIST PROGRESS roundtrip seeds={start}..{seed} failed={failed} secs={}",
                began.elapsed().as_secs()
            );
        }
        let steps_taken = trace_take(false);
        for (what, n) in counts {
            *coverage.entry(what).or_insert(0) += n;
        }
        if let Some(first) = problems.first() {
            failed += 1;
            println!(
                "FZ-PERSIST FAIL roundtrip seed={seed} {}",
                first.lines().next().unwrap_or_default()
            );
            for problem in &problems {
                println!("  {problem}");
            }
            for line in &steps_taken {
                println!("  trace: {line}");
            }
        }
    }
    println!("FZ-PERSIST COVERAGE roundtrip {coverage:?}");
    println!(
        "FZ-PERSIST SUMMARY roundtrip seeds={start}..{} failed={failed} secs={}",
        start.saturating_add(count).saturating_sub(1),
        began.elapsed().as_secs()
    );
    assert_eq!(failed, 0, "round-trip sweep: {failed} failing seed(s)");
}
