//! Transition fuzz at the `AppState` level (C24, plan 11.4 and 11.6): the
//! saving fuzz and the private-corpus fixture fuzz.
//!
//! **Saving fuzz** (`saving_fuzz_window`, `saving_fuzz_wide`). Each seed opens
//! one origin: a version 2 file built from a random walk through the survey
//! operations (`changeset::testkit::Walk`), sometimes SpeleoDB-linked,
//! sometimes carrying unknown tier-2 members; a tracked legacy fixture (the
//! first save upgrades it); or a project created in the session. Then a
//! random mix of: commands over every ported family (shots, lines, stations,
//! merges, closures, datums, moves, credits, Sections, batches, split,
//! combine, settings, elevation, warning ignores), undo, redo, saves (with
//! redo entries on the stack too), Save a Copy / Save As, display toggles
//! (the local-only durable save), changes made to the file behind the app's
//! back, and a reopen that continues on the reopened project. Properties:
//! - every step leaves a valid project; a refused command changes nothing
//!   (model, stacks, dirty flag);
//! - a save writes exactly the in-memory survey and the persisted window of
//!   the stacks (the newest `general.undo_persist_limit` entries of each, or
//!   both whole on a project in Full SpeleoDB mode, as 0.6.0 saved them): the
//!   file decodes to the same body and history counts, a fresh open shows the
//!   same survey and stacks, and saving that unchanged is a byte fixpoint;
//!   the reopened history redoes and undoes end to end;
//! - Save a Copy of a linked project is detached (no link) and leaves the
//!   session on its file; Save As moves the session to the new file;
//! - a display toggle rewrites only the per-machine members (the survey
//!   bytes on disk stay those of the last save) and the next save passes the
//!   guard;
//! - a file touched between saves refuses the save (`EXTERNALLY_MODIFIED`),
//!   leaves the file and the in-memory edit alone, and the Overwrite answer
//!   then writes the edit;
//! - unknown tier-2 members survive every edit and save verbatim;
//! - a forward block of commands undoes to every recorded state and redoes
//!   back, exactly (`modified_at` masked; the id allocators may stand
//!   higher after an undo, plan 11.4).
//!
//! **Read-only** (`read_only_files_are_never_written`): a file with an
//! unknown required feature refuses every edit, undo and save over random
//! sequences, and its survey bytes never change (display toggles rewrite
//! only the per-machine members).
//!
//! **Fixture fuzz** (`transition_fuzz_corpus`, ignored): every `*.stalx`
//! under `STALAGMIGHT_MIGRATION_CORPUS` (real files, on the fuzz host only)
//! is copied to a temp dir and opened through `AppState`; a legacy file must
//! open as `migrate` gives it, with no unexplained position mismatch; then
//! `STALAGMIGHT_FUZZ_WALKS` seeded command sequences each undo and redo
//! exactly, save, reopen to the same survey and save to the same bytes. It
//! prints file-name hashes, counts and seed repros only, never a name, a
//! coordinate or a failure's text (the panic hook is silenced too).
//!
//! **Two command generators.** A step draws from this file's own generator
//! ([`operate`]) five times in six and from the undo fuzzer's
//! (`fuzz_undo_tests`: `Fuzzer::draw_op` and `apply_op` on its `Targets`)
//! once in six ([`shared_op`]). The shared one brings the families this file
//! has no arm for (split shot, delete and split, the range deletes, delete
//! the last shot and its line, detach, move comment, imports, Add .stalx,
//! create and merge) and its targeting (closures between tieable stations,
//! Combine at the join a split made), so the save properties run over them
//! too. The own generator stays, because it deliberately differs from the
//! undo fuzzer's where the save is the oracle:
//! - its floats are full-precision draws of a 64-bit stream, not the undo
//!   fuzzer's integer-quantized ones, so the encoder's byte fixpoint sees
//!   arbitrary bit patterns;
//! - its updates carry hostile values (NaN, infinities, negative lengths,
//!   `-0.0`) whose refusal must leave the dirty flag alone;
//! - its new shots carry client uuids, which a save must keep;
//! - its comments carry quotes, accents and line breaks, and its station
//!   names collide;
//! - it covers warning ignores, which ride the save;
//! - its seeds are the farm's 64-bit `STALAGMIGHT_FUZZ_SEED_*` windows,
//!   while the undo fuzzer's streams, references and replayable traces are
//!   pinned by its own gates (`the_fuzzer_is_deterministic`, the shrinker),
//!   which the saving fuzz must not move.
//!
//! The undo fuzzer's generator runs here on its own stream (seeded
//! `seed ^ SHARED_STREAM`) through `Fuzzer::driving`, so it is not changed
//! by being shared.
//!
//! Windows: `STALAGMIGHT_FUZZ_SEED_START` / `STALAGMIGHT_FUZZ_SEEDS`; a failure
//! prints one line with the seed and the test that replays it alone.

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use chrono::Utc;
use tempfile::TempDir;

use super::fuzz_undo_tests::{read_targets, Fuzzer, Refs};
use super::{AppState, EXTERNALLY_MODIFIED_CODE, READ_ONLY_FILE_CODE};
use crate::core::{Command, CommandResult};
use crate::models::survey::changeset::testkit::Walk;
use crate::models::survey::Project;
use crate::models::{
    ClosureStrategy, CombineLinesOptions, CombinePick, Coordinates, CreditEdit, CreditTargets,
    GpsCoordinates, LineUpdates, Person, ShotData, ShotKind, ShotUpdates, SplitAt,
    SplitLineOptions, StationRef,
};
use crate::persistence::stalx_v2::{self, decode, encode, DecodeOptions, Extras, Format, Mode};
use crate::persistence::ResolvedWarning;

/// Seeds the CI window runs when no window is set.
const CI_SEEDS: u64 = 12;
/// Seeds the wide run takes when no window is set.
const WIDE_SEEDS: u64 = 2_000;
/// Mixed steps per seed.
const STEPS: usize = 40;
/// Commands in the forward block of the exactness check.
const FORWARD: usize = 16;
/// Command sequences per corpus file (`STALAGMIGHT_FUZZ_WALKS` overrides).
const CORPUS_WALKS: u64 = 4;
/// Commands per corpus sequence.
const CORPUS_STEPS: usize = 30;
/// A step draws one of `OWN_ARMS + SHARED_ARMS` arms: this file's own
/// command families ([`operate`]), or a move of the undo fuzzer's generator
/// ([`shared_op`]).
const OWN_ARMS: usize = 30;
const SHARED_ARMS: usize = 6;
/// The undo fuzzer's generator is seeded `seed ^ SHARED_STREAM` (folded to
/// its 32-bit seed), so its stream is not the undo fuzzer's own for the same
/// seed number.
const SHARED_STREAM: u32 = 0x0C24_0C15;

const TEST_PATH: &str = "core::state::transition_fuzz_tests";

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

fn window(default_count: u64) -> (u64, u64) {
    (
        env_u64("STALAGMIGHT_FUZZ_SEED_START").unwrap_or(0),
        env_u64("STALAGMIGHT_FUZZ_SEEDS").unwrap_or(default_count),
    )
}

/// SplitMix64.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0xC24_5A7E_F022_0000)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }
}

// ----- state helpers -------------------------------------------------------------

/// A step's failure: what broke, and (for synthetic data) the detail.
type Fail = String;

fn check(ok: bool, what: impl FnOnce() -> String) -> Result<(), Fail> {
    if ok {
        Ok(())
    } else {
        Err(what())
    }
}

fn open(path: &Path) -> Result<AppState, Fail> {
    let state = AppState::new().map_err(|e| format!("AppState::new: {e}"))?;
    let r = state.execute(Command::OpenProject {
        path: path.to_path_buf(),
    });
    check(r.success, || format!("open failed: {:?}", r.error))?;
    Ok(state)
}

fn project_of(state: &AppState) -> Result<Project, Fail> {
    state
        .get_project_state()
        .ok_or_else(|| "no project open".to_string())
}

/// The survey as a file holds it (no per-file state, no session ids).
fn body(p: &Project) -> Result<Vec<u8>, Fail> {
    let mut p = p.clone();
    p.file_path = None;
    encode(&p, &Extras::default(), Mode::Normal).map_err(|e| format!("encode: {e}"))
}

/// `live` stepped onto `expected` (plan 11.4): the same survey once
/// `modified_at` (undo and redo stamp it) is masked, every id allocator at
/// least the recorded one (an undo leaves the counters standing).
fn same_state(live: &Project, expected: &Project) -> Result<(), Fail> {
    let (l, e) = (live.counters, expected.counters);
    check(
        l.next_station_id >= e.next_station_id
            && l.next_closure_id >= e.next_closure_id
            && l.next_section_id >= e.next_section_id,
        || format!("a counter went down: live {l:?}, recorded {e:?}"),
    )?;
    let mut normalized = live.clone();
    normalized.counters = e;
    normalized.modified_at = expected.modified_at;
    let (a, b) = (body(&normalized)?, body(expected)?);
    check(a == b, || diff_at(&a, &b))
}

fn valid(state: &AppState, what: &str) -> Result<Project, Fail> {
    let p = project_of(state)?;
    if let Some(v) = p.validate().first() {
        return Err(format!("{what}: the project is invalid: {v}"));
    }
    Ok(p)
}

fn read(path: &Path) -> Result<Vec<u8>, Fail> {
    std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))
}

fn decoded(path: &Path) -> Result<stalx_v2::Decoded, Fail> {
    decode(&read(path)?, &DecodeOptions::default()).map_err(|e| format!("decode: {e}"))
}

/// The text of every member but `local` (the per-machine one) and
/// `summary` (a cache every write recomputes, plan 6.4; its unknown keys are
/// kept, after the known ones).
fn survey_members(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|l| !l.starts_with("\"local\":") && !l.starts_with("\"summary\":"))
        .collect::<Vec<_>>()
        .join("\n")
}

// ----- the command generator -------------------------------------------------------

fn shot_data(rng: &mut Rng, uuid: String) -> ShotData {
    let azimuth = rng.range(0.0, 360.0);
    let depth = rng.range(0.0, 30.0);
    ShotData {
        uuid: Some(uuid),
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length: rng.range(0.5, 20.0),
        depth_in: depth,
        depth_out: depth + rng.range(-1.0, 1.0),
        left: rng.chance(0.5).then(|| rng.range(0.0, 3.0)),
        right: rng.chance(0.3).then(|| rng.range(0.0, 3.0)),
        comment: rng.chance(0.1).then(|| "note \u{e9}\"".to_string()),
        kind: if rng.chance(0.1) {
            ShotKind::Jump
        } else {
            ShotKind::Normal
        },
        ..ShotData::default()
    }
}

fn slot(p: &Project, rng: &mut Rng) -> Option<StationRef> {
    let lines = p.lines();
    if lines.is_empty() {
        return None;
    }
    let line = &lines[rng.below(lines.len())];
    Some(StationRef::new(
        line.id,
        rng.below(line.shots.len() + 1) as u32,
    ))
}

fn shot_of(p: &Project, rng: &mut Rng) -> Option<(u32, u32)> {
    let with: Vec<_> = p.lines().iter().filter(|l| !l.shots.is_empty()).collect();
    if with.is_empty() {
        return None;
    }
    let line = with[rng.below(with.len())];
    Some((line.id, line.shots[rng.below(line.shots.len())].id))
}

fn line_of(p: &Project, rng: &mut Rng) -> Option<u32> {
    let lines = p.lines();
    (!lines.is_empty()).then(|| lines[rng.below(lines.len())].id)
}

fn gps(rng: &mut Rng) -> GpsCoordinates {
    GpsCoordinates {
        latitude: 20.5 + rng.range(-0.01, 0.01),
        longitude: -87.3 + rng.range(-0.01, 0.01),
        depth_reference: 0.0,
    }
}

/// One operation: a `Command` through `execute`, or a recipe the IPC calls
/// directly. Returns its name and result.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per command family; splitting it scatters the generator"
)]
fn operate(
    state: &AppState,
    p: &Project,
    arm: usize,
    rng: &mut Rng,
    n: &mut u64,
) -> (&'static str, CommandResult) {
    *n += 1;
    let tag = *n;
    let none = || CommandResult::failure("nothing to do".to_string());
    let exec = |name: &'static str, cmd: Command| (name, state.execute(cmd));
    match arm {
        0 | 1 => {
            let shots = (0..=rng.below(4))
                .map(|k| shot_data(rng, format!("tf-{tag}-{k}")))
                .collect();
            exec(
                "create line",
                Command::CreateLine {
                    name: Some(format!("F{tag}")),
                    surveyor: None,
                    date: None,
                    color: "#123abc".to_string(),
                    direction: None,
                    shots,
                    cave_section_id: (!p.sections.is_empty() && rng.chance(0.3))
                        .then(|| p.sections[rng.below(p.sections.len())].id),
                },
            )
        }
        2..=4 => match line_of(p, rng) {
            Some(line_id) => {
                let after = p
                    .line(line_id)
                    .filter(|l| !l.shots.is_empty() && rng.chance(0.6))
                    .map(|l| l.shots[rng.below(l.shots.len())].id);
                exec(
                    "create shot",
                    Command::CreateShot {
                        line_id,
                        shot: shot_data(rng, format!("tf-{tag}")),
                        after_shot_id: after,
                        before_shot_id: None,
                    },
                )
            }
            None => ("create shot", none()),
        },
        5..=8 => match shot_of(p, rng) {
            Some((line_id, shot_id)) => {
                let value = match rng.below(10) {
                    0 => f64::NAN,
                    1 => -1.0,
                    2 => f64::INFINITY,
                    _ => rng.range(0.5, 30.0),
                };
                let updates = match rng.below(4) {
                    0 => ShotUpdates {
                        length: Some(value),
                        ..ShotUpdates::default()
                    },
                    1 => ShotUpdates {
                        azimuth_in: Some(value * 7.0 % 360.0),
                        azimuth_out: Some(value * 7.0 % 360.0),
                        ..ShotUpdates::default()
                    },
                    2 => ShotUpdates {
                        depth_out: Some(value),
                        ..ShotUpdates::default()
                    },
                    _ => ShotUpdates {
                        left: Some(rng.chance(0.5).then_some(value)),
                        pitch_in: Some(Some(-0.0)),
                        ..ShotUpdates::default()
                    },
                };
                exec(
                    "update shot",
                    Command::UpdateShot {
                        line_id,
                        shot_id,
                        updates,
                    },
                )
            }
            None => ("update shot", none()),
        },
        9 => match shot_of(p, rng) {
            Some((line_id, shot_id)) => {
                exec("delete shot", Command::DeleteShot { line_id, shot_id })
            }
            None => ("delete shot", none()),
        },
        10 => match shot_of(p, rng) {
            Some((line_id, shot_id)) => exec(
                "set shot kind",
                Command::SetShotKind {
                    line_id,
                    shot_id,
                    kind: [ShotKind::Normal, ShotKind::Jump, ShotKind::Reference][rng.below(3)],
                },
            ),
            None => ("set shot kind", none()),
        },
        11 => match line_of(p, rng) {
            Some(line_id) if p.lines().len() > 2 && rng.chance(0.4) => {
                exec("delete line", Command::DeleteLine { line_id })
            }
            Some(line_id) => exec(
                "update line",
                Command::UpdateLineMetadata {
                    line_id,
                    updates: LineUpdates {
                        name: Some(Some(format!("Renamed {tag}"))),
                        color: Some("#abcdef".to_string()),
                        quality: Some([1.0, 0.5, 2.0][rng.below(3)]),
                        ..LineUpdates::default()
                    },
                },
            ),
            None => ("update line", none()),
        },
        12 => match line_of(p, rng) {
            Some(line_id) => exec(
                "reverse line",
                Command::ReverseLine {
                    line_id,
                    swap_left_right: rng.chance(0.5),
                },
            ),
            None => ("reverse line", none()),
        },
        13 | 14 => match (slot(p, rng), slot(p, rng)) {
            (Some(station_a), Some(station_b)) => exec(
                "merge stations",
                Command::MergeStations {
                    station_a,
                    station_b,
                },
            ),
            _ => ("merge stations", none()),
        },
        15 => match slot(p, rng) {
            Some(station_ref) => exec("unmerge stations", Command::UnmergeStations { station_ref }),
            None => ("unmerge stations", none()),
        },
        16 => match (slot(p, rng), slot(p, rng)) {
            (Some(station_a), Some(station_b)) => exec(
                "create closure",
                Command::CreateLoopClosure {
                    station_a,
                    station_b,
                    closure_strategy: if rng.chance(0.3) {
                        ClosureStrategy::Move
                    } else {
                        ClosureStrategy::Batch
                    },
                },
            ),
            _ => ("create closure", none()),
        },
        17 => match p.closures.get(rng.below(p.closures.len().max(1))) {
            Some(c) => exec(
                "remove closure",
                Command::RemoveLoopClosure { closure_id: c.id },
            ),
            None => ("remove closure", none()),
        },
        18 => match slot(p, rng) {
            Some(station_ref) if rng.chance(0.7) => exec(
                "set datum",
                Command::SetZeroDatum {
                    station_ref,
                    coordinates: gps(rng),
                    accuracy_m: rng.chance(0.5).then_some(3.0),
                    entrance: [None, Some(true), Some(false)][rng.below(3)],
                },
            ),
            Some(station_ref) => {
                // Mostly a datum's own slot, so the removal has something to remove.
                let datum_slot = p
                    .datums
                    .get(rng.below(p.datums.len().max(1)))
                    .and_then(|d| p.slots_of(d.station).first().copied());
                let station_ref = datum_slot
                    .filter(|_| rng.chance(0.8))
                    .unwrap_or(station_ref);
                exec("remove datum", Command::RemoveZeroDatum { station_ref })
            }
            None => ("set datum", none()),
        },
        19 => match slot(p, rng) {
            Some(station_ref) if rng.chance(0.5) => exec(
                "station comment",
                Command::SetStationComment {
                    station_ref,
                    comment: rng.chance(0.8).then(|| format!("comment {tag}\nline two")),
                },
            ),
            Some(station_ref) => exec(
                "station name",
                Command::SetStationName {
                    station_ref,
                    name: if rng.chance(0.2) {
                        "A1".to_string()
                    } else {
                        format!("N{tag}")
                    },
                },
            ),
            None => ("station meta", none()),
        },
        20 => {
            let offset = Coordinates::new(rng.range(-5.0, 5.0), rng.range(-5.0, 5.0), 0.0);
            match (rng.below(3), line_of(p, rng), slot(p, rng)) {
                (0, Some(line_id), _) => exec("move line", Command::MoveLine { line_id, offset }),
                (1, Some(line_id), _) => {
                    exec("move segment", Command::MoveSegment { line_id, offset })
                }
                (_, _, Some(station_ref)) => exec(
                    "move station",
                    Command::MoveStation {
                        station_ref,
                        offset,
                    },
                ),
                _ => ("move (no target)", none()),
            }
        }
        21 => match line_of(p, rng) {
            Some(line_id) => exec(
                "edit credit",
                Command::EditShotCredit {
                    targets: CreditTargets::Line { line_id },
                    edit: CreditEdit {
                        set_surveyors: Some(vec![Person {
                            name: format!("Diver {}", rng.below(3)),
                            email: None,
                        }]),
                        ..CreditEdit::default()
                    },
                },
            ),
            None => ("edit credit", none()),
        },
        22 => {
            let lines: Vec<u32> = p.lines().iter().take(2).map(|l| l.id).collect();
            match (rng.below(3), p.sections.first()) {
                (0, _) | (_, None) => exec(
                    "create section",
                    Command::CreateCaveSection {
                        name: format!("Section {tag}"),
                        color: "#336699".to_string(),
                        line_ids: lines,
                    },
                ),
                (1, Some(s)) => exec(
                    "move lines to section",
                    Command::MoveLinesToCaveSection {
                        line_ids: lines,
                        section_id: rng.chance(0.7).then_some(s.id),
                    },
                ),
                (_, Some(s)) => exec(
                    "delete section",
                    Command::DeleteCaveSection { section_id: s.id },
                ),
            }
        }
        23 => match (shot_of(p, rng), shot_of(p, rng)) {
            (Some(a), Some(b)) => {
                let edit = |(line_id, shot_id): (u32, u32), length: f64| Command::UpdateShot {
                    line_id,
                    shot_id,
                    updates: ShotUpdates {
                        length: Some(length),
                        ..ShotUpdates::default()
                    },
                };
                exec(
                    "batch edits",
                    Command::Batch {
                        commands: vec![edit(a, rng.range(1.0, 9.0)), edit(b, rng.range(1.0, 9.0))],
                        label: None,
                    },
                )
            }
            _ => ("batch edits", none()),
        },
        24 => match p.lines().iter().find(|l| l.shots.len() >= 2) {
            Some(line) => {
                let index = 1 + rng.below(line.shots.len() - 1) as u32;
                (
                    "split line",
                    state.execute_split_line(
                        line.id,
                        SplitAt::Station {
                            station_index: index,
                        },
                        SplitLineOptions {
                            name_a: format!("A{tag}"),
                            name_b: format!("B{tag}"),
                            color_a: "#111111".to_string(),
                            color_b: "#222222".to_string(),
                        },
                    ),
                )
            }
            None => ("split line", none()),
        },
        25 => match (line_of(p, rng), line_of(p, rng)) {
            (Some(kept), Some(other)) if kept != other => (
                "combine lines",
                state.execute_combine_lines(
                    kept,
                    other,
                    CombineLinesOptions {
                        name: format!("C{tag}"),
                        color: "#333333".to_string(),
                        swap_lr: rng.chance(0.5),
                        comment_pick: CombinePick::Both,
                        name_pick: CombinePick::Kept,
                        join: None,
                    },
                ),
            ),
            _ => ("combine lines", none()),
        },
        26 => {
            let mut validation = p.settings.validation.clone();
            validation.warn_shot_length_max = rng.range(30.0, 80.0);
            (
                "project settings",
                state.update_project_settings(format!("Project {tag}"), validation),
            )
        }
        27 => (
            "surface elevation",
            state.set_surface_elevation(rng.chance(0.7).then(|| rng.range(-10.0, 200.0))),
        ),
        28 => match shot_of(p, rng) {
            Some((line_id, shot_id)) => {
                let uuid = p
                    .line(line_id)
                    .and_then(|l| l.shots.iter().find(|s| s.id == shot_id))
                    .map(|s| s.uuid.clone());
                let warning = ResolvedWarning {
                    line_id,
                    shot_id,
                    warning_code: "SHORT_SHOT".to_string(),
                    resolved_at: Utc::now(),
                    resolved_comment: rng.chance(0.5).then(|| "checked".to_string()),
                    shot_uuid: uuid,
                };
                let r = state.save_resolved_warning_gated(&warning);
                (
                    "ignore warning",
                    r.map_or_else(CommandResult::failure, |()| CommandResult::success()),
                )
            }
            None => ("ignore warning", none()),
        },
        _ => {
            let hashes = p.imported_file_hashes.clone();
            exec(
                "untrack import",
                Command::UntrackImport {
                    file_hashes: hashes,
                    line_hashes: Vec::new(),
                },
            )
        }
    }
}

/// The undo fuzzer's generator for `seed`, unattached (see [`shared_op`]).
fn shared_generator(seed: u64) -> Fuzzer {
    Fuzzer::driving((seed ^ (seed >> 32)) as u32 ^ SHARED_STREAM)
}

/// One move of the undo fuzzer's generator (`fuzz_undo_tests`): drawn from
/// the live survey's targets and applied through its own execution path,
/// with `state` swapped into the generator's app for the draw and the apply.
/// Named by its family (`Op::kind`): the undo fuzzer names its families in
/// snake case and [`operate`] in words, so the coverage counts keep the two
/// generators apart. A draw with no target is a refusal.
fn shared_op(state: &mut AppState, shared: &mut Fuzzer) -> (&'static str, CommandResult) {
    std::mem::swap(state, &mut shared.state);
    let targets = read_targets(&shared.state);
    let drawn = shared.draw_op(&targets).map(|op| {
        let result = shared
            .apply_op(&op, &mut Refs::identity(), &targets)
            .map(|drawn| drawn.result);
        (op.kind(), result)
    });
    std::mem::swap(state, &mut shared.state);
    match drawn {
        Some((kind, Some(result))) => (kind, result),
        Some((kind, None)) => (kind, CommandResult::failure("no target".to_string())),
        None => (
            "shared_no_target",
            CommandResult::failure("nothing to do".to_string()),
        ),
    }
}

/// One operation from either generator.
fn draw(
    state: &mut AppState,
    p: &Project,
    shared: &mut Fuzzer,
    rng: &mut Rng,
    n: &mut u64,
) -> (&'static str, CommandResult) {
    let arm = rng.below(OWN_ARMS + SHARED_ARMS);
    if arm < OWN_ARMS {
        operate(state, p, arm, rng, n)
    } else {
        shared_op(state, shared)
    }
}

/// Run one operation and check the step properties: the project stays
/// valid, and a refusal changes nothing (survey, stacks, dirty flag).
fn step(
    state: &mut AppState,
    shared: &mut Fuzzer,
    rng: &mut Rng,
    n: &mut u64,
) -> Result<(&'static str, bool), Fail> {
    let before = project_of(state)?;
    let stacks = (state.get_undo_stack().len(), state.get_redo_stack().len());
    let dirty = state.has_unsaved_changes();
    let (name, result) = draw(state, &before, shared, rng, n);
    let after = valid(state, name)?;
    if !result.success {
        check(body(&after)? == body(&before)?, || {
            format!("a refused {name} changed the survey ({:?})", result.error)
        })?;
        check(
            (state.get_undo_stack().len(), state.get_redo_stack().len()) == stacks,
            || format!("a refused {name} changed the stacks"),
        )?;
        check(state.has_unsaved_changes() == dirty, || {
            format!("a refused {name} changed the dirty flag")
        })?;
    }
    Ok((name, result.success))
}

// ----- the save properties ---------------------------------------------------------

/// Save to the open file and check what it wrote; then open it again, check
/// the reopened survey and stacks, the byte fixpoint of an unchanged save,
/// and that the reopened history steps end to end. Returns the reopened
/// state (the caller may continue on it).
fn save_and_reopen(state: &AppState, path: &Path) -> Result<AppState, Fail> {
    let r = state.execute(Command::SaveProject { path: None });
    check(r.success, || format!("save failed: {:?}", r.error))?;
    check(!state.has_unsaved_changes(), || {
        "dirty after a save".to_string()
    })?;
    let p = valid(state, "after save")?;
    let file = decoded(path)?;
    check(file.read_only().is_none(), || {
        "the saved file opens read-only".to_string()
    })?;
    check(body(&file.project)? == body(&p)?, || {
        "the file's survey is not the in-memory one".to_string()
    })?;
    // What a save keeps of each stack: the newest `undo_persist_limit`
    // entries (the lists are most recent first), unless the history rides to
    // SpeleoDB (Full mode).
    let window = (!state
        .undo_policy_for(state.speleo_link().as_ref())
        .unlimited_depth())
    .then(|| state.get_config().general.undo_persist_limit);
    let (mut undo, mut redo) = (state.get_undo_stack(), state.get_redo_stack());
    if let Some(n) = window {
        undo.truncate(n);
        redo.truncate(n);
    }
    check(
        file.extras.history.undo.len() == undo.len()
            && file.extras.history.redo.len() == redo.len(),
        || "the file's history counts are not the stacks'".to_string(),
    )?;
    let bytes = read(path)?;

    let reopened = open(path)?;
    let q = valid(&reopened, "reopened")?;
    check(body(&q)? == body(&p)?, || {
        "the reopened survey differs".to_string()
    })?;
    let labels = |items: Vec<crate::core::undo::UndoHistoryItem>| {
        items
            .into_iter()
            .map(|i| (i.description, i.command_type))
            .collect::<Vec<_>>()
    };
    check(
        labels(reopened.get_undo_stack()) == labels(undo)
            && labels(reopened.get_redo_stack()) == labels(redo),
        || "the reopened stacks differ".to_string(),
    )?;
    check(!reopened.history_discarded(), || {
        "the reopened history was discarded".to_string()
    })?;
    let r = reopened.execute(Command::SaveProject { path: None });
    check(r.success, || {
        format!("the unchanged save failed: {:?}", r.error)
    })?;
    check(read(path)? == bytes, || {
        "an unchanged save is not a byte fixpoint".to_string()
    })?;

    // The persisted history steps on the reopened project: redo all, undo
    // all, redo back to the saved state.
    let redo_n = reopened.get_redo_stack().len();
    for k in 0..redo_n {
        let r = reopened.redo();
        check(r.success, || format!("reopened redo {k}: {:?}", r.error))?;
        valid(&reopened, "reopened redo")?;
    }
    let total = reopened.get_undo_stack().len();
    for k in 0..total {
        let r = reopened.undo();
        check(r.success, || format!("reopened undo {k}: {:?}", r.error))?;
        valid(&reopened, "reopened undo")?;
    }
    for k in 0..total - redo_n {
        let r = reopened.redo();
        check(r.success, || {
            format!("reopened redo back {k}: {:?}", r.error)
        })?;
    }
    same_state(&project_of(&reopened)?, &q)
        .map_err(|e| format!("the reopened history does not step back to the saved state: {e}"))?;
    // Leave the file as saved for the caller.
    let r = reopened.save_project_overwriting(None);
    check(r.success, || format!("restoring the save: {:?}", r.error))?;
    Ok(reopened)
}

/// A display toggle (and, on a linked project, a sync-state change) is
/// durable at once and rewrites only `local`; the next save still passes the
/// guard.
fn display_toggle(state: &AppState, path: &Path, rng: &mut Rng) -> Result<(), Fail> {
    let before = read(path)?;
    let key = format!("zz.view{}", rng.below(3));
    state
        .save_display_setting(&key, if rng.chance(0.5) { "true" } else { "false" })
        .map_err(|e| format!("display toggle: {e}"))?;
    if state.speleo_link().is_some() && rng.chance(0.5) {
        // A SpeleoDB sync-state change is local-only too (plan 6.5 mode 4).
        let verified = Utc::now().to_rfc3339();
        state
            .mutate_speleo_link(|link| {
                link.as_mut().is_some_and(|l| {
                    l.last_verify_at = Some(verified.clone());
                    true
                })
            })
            .map_err(|e| format!("sync-state change: {e}"))?;
    }
    let after = read(path)?;
    if stalx_v2::sniff(&before) == Format::V2 {
        let (a, b) = (survey_members(&after), survey_members(&before));
        check(a == b, || {
            format!(
                "a display toggle rewrote a survey member, {}",
                diff_at(a.as_bytes(), b.as_bytes())
            )
        })?;
        let file = decode(&after, &DecodeOptions::default()).map_err(|e| e.to_string())?;
        check(file.extras.local.display.contains_key(&key), || {
            "the display toggle is not on disk".to_string()
        })?;
    } else {
        check(after == before, || {
            "a display toggle wrote a legacy file".to_string()
        })?;
    }
    Ok(())
}

/// The file changes behind the app's back: the save is refused and touches
/// nothing, the edit stays in memory, and the Overwrite answer writes it.
fn external_change(state: &AppState, path: &Path) -> Result<(), Fail> {
    let p = project_of(state)?;
    let dirty = state.has_unsaved_changes();
    let mut other = read(path)?;
    other.extend_from_slice(b"\n");
    std::fs::write(path, &other).map_err(|e| e.to_string())?;
    let r = state.execute(Command::SaveProject { path: None });
    check(!r.success, || {
        "a save over a changed file succeeded".to_string()
    })?;
    check(
        r.error
            .as_deref()
            .unwrap_or_default()
            .starts_with(EXTERNALLY_MODIFIED_CODE),
        || format!("the refusal is not EXTERNALLY_MODIFIED: {:?}", r.error),
    )?;
    check(read(path)? == other, || {
        "the refused save touched the file".to_string()
    })?;
    check(body(&project_of(state)?)? == body(&p)?, || {
        "the refused save changed the in-memory survey".to_string()
    })?;
    check(state.has_unsaved_changes() == dirty, || {
        "the refused save changed the dirty flag".to_string()
    })?;
    let r = state.save_project_overwriting(None);
    check(r.success, || format!("the overwrite failed: {:?}", r.error))?;
    check(body(&decoded(path)?.project)? == body(&p)?, || {
        "the overwrite did not write the in-memory edit".to_string()
    })?;
    Ok(())
}

/// Save a Copy (linked: detached, session stays) or Save As (unlinked: the
/// session moves). Returns the path the session is on afterwards.
fn save_copy(state: &AppState, path: &Path, dir: &Path, k: usize) -> Result<PathBuf, Fail> {
    let copy = dir.join(format!("copy-{k}.stalx"));
    let p = project_of(state)?;
    let linked = state.speleo_link().is_some();
    let before = read(path).ok();
    let r = state.execute(Command::SaveProject {
        path: Some(copy.clone()),
    });
    check(r.success, || format!("save a copy failed: {:?}", r.error))?;
    let file = decoded(&copy)?;
    check(body(&file.project)? == body(&p)?, || {
        "the copy's survey differs".to_string()
    })?;
    if linked {
        check(
            file.extras.speleodb.is_none() && file.extras.local.speleodb.is_empty(),
            || "a copy of a linked project kept its link".to_string(),
        )?;
        check(read(path).ok() == before, || {
            "a detached copy wrote the open file".to_string()
        })?;
        Ok(path.to_path_buf())
    } else {
        check(
            project_of(state)?.file_path.as_deref() == Some(copy.as_path()),
            || "Save As did not move the session".to_string(),
        )?;
        Ok(copy)
    }
}

// ----- origins ------------------------------------------------------------------------

/// Tier-2 unknown content a newer writer could add; each fragment must be in
/// every saved file.
const UNKNOWN: [(&str, &str, &str); 3] = [
    (
        "\n\"end\":true",
        "\n\"zz_future\":{\"a\":[1,2.50,\"s\"]},\n\"end\":true",
        "\"zz_future\":{\"a\":[1,2.50,\"s\"]}",
    ),
    (
        "\"project\":{",
        "\"project\":{\"zz_p\":{\"b\":-0},",
        "\"zz_p\":{\"b\":-0}",
    ),
    ("\"summary\":{", "\"summary\":{\"zz_m\":7,", "\"zz_m\":7"),
];

/// How a seed starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    V2,
    V2Linked,
    V2Unknown,
    Legacy,
    New,
}

fn origin_of(seed: u64) -> Origin {
    match seed % 10 {
        0..=3 => Origin::V2,
        4 | 5 => Origin::V2Linked,
        6 | 7 => Origin::V2Unknown,
        8 => Origin::Legacy,
        _ => Origin::New,
    }
}

/// A random survey from the model's own walk (junctions, closures, datums,
/// placements, credits, Sections, names, comments).
fn walk_survey(seed: u64, steps: usize) -> Project {
    let mut walk = Walk::new(seed);
    for _ in 0..steps {
        walk.step();
    }
    walk.project
}

fn linked_extras() -> Extras {
    Extras {
        speleodb: Some(stalx_v2::SpeleoDbLink {
            endpoint: "https://speleodb.example".to_string(),
            project_id: "0f6c1d2e-3b4a-4c5d-8e9f-a0b1c2d3e4f5".to_string(),
            project_name: "Synthetic".to_string(),
            undo_mode: Some(crate::models::UndoMode::Full),
        }),
        local: stalx_v2::LocalState {
            speleodb: stalx_v2::LocalSync {
                base_commit: Some("ab".repeat(20)),
                ..stalx_v2::LocalSync::default()
            },
            ..stalx_v2::LocalState::default()
        },
        ..Extras::default()
    }
}

/// Write the seed's starting file into `dir` and open it. Returns the
/// state, the open path and the unknown fragments the file carries.
fn start(
    seed: u64,
    origin: Origin,
    dir: &Path,
) -> Result<(AppState, PathBuf, Vec<&'static str>), Fail> {
    let path = dir.join(format!("c24-{seed}.stalx"));
    let mut fragments = Vec::new();
    match origin {
        Origin::V2 | Origin::V2Linked | Origin::V2Unknown => {
            let p = walk_survey(seed, 30);
            let extras = if origin == Origin::V2Linked {
                linked_extras()
            } else {
                Extras::default()
            };
            let mut text =
                String::from_utf8(encode(&p, &extras, Mode::Normal).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            if origin == Origin::V2Unknown {
                for (from, to, needle) in UNKNOWN {
                    let edited = text.replacen(from, to, 1);
                    if edited != text {
                        fragments.push(needle);
                        text = edited;
                    }
                }
            }
            std::fs::write(&path, text).map_err(|e| e.to_string())?;
            Ok((open(&path)?, path, fragments))
        }
        Origin::Legacy => {
            crate::app_dirs::init_for_test();
            let name = ["fractal_plant_iter3.stalx", "naharon.stalx"][(seed / 10 % 2) as usize];
            let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../e2e/fixtures")
                .join(name);
            std::fs::copy(&fixture, &path).map_err(|e| e.to_string())?;
            Ok((open(&path)?, path, fragments))
        }
        Origin::New => {
            let state = AppState::new().map_err(|e| e.to_string())?;
            let r = state.execute(Command::CreateProject {
                name: format!("Fresh {seed}"),
                location: None,
            });
            check(r.success, || format!("create: {:?}", r.error))?;
            let r = state.execute(Command::SaveProject {
                path: Some(path.clone()),
            });
            check(r.success, || format!("first save: {:?}", r.error))?;
            Ok((state, path, fragments))
        }
    }
}

/// Remove the upgrade backups a legacy seed left (named after its file).
fn drop_backups(stem: &str) {
    let Some(dir) = super::project_io::upgrade_backup_dir() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(&format!("{stem}."))
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The undo entries by their (strictly increasing) times: an entry pushed
/// is a time that was not there, whatever the limits trimmed.
fn entry_times(state: &AppState) -> Vec<Option<String>> {
    state
        .get_undo_stack()
        .into_iter()
        .map(|i| i.timestamp)
        .collect()
}

/// Where two texts first differ, with some context (synthetic data only).
fn diff_at(a: &[u8], b: &[u8]) -> String {
    let at = a
        .iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()));
    let from = at.saturating_sub(60);
    let show = |t: &[u8]| {
        String::from_utf8_lossy(&t[from.min(t.len())..(at + 60).min(t.len())]).into_owned()
    };
    format!(
        "at byte {at}: live ...{}... recorded ...{}...",
        show(a),
        show(b)
    )
}

/// A forward block of `count` commands, then undo all and redo all: every
/// depth lands on its recorded state ([`same_state`]).
fn forward_exactness(
    state: &mut AppState,
    shared: &mut Fuzzer,
    rng: &mut Rng,
    n: &mut u64,
    count: usize,
) -> Result<usize, Fail> {
    let mut states = vec![valid(state, "forward start")?];
    let mut ops: Vec<&'static str> = Vec::new();
    let mut times: Vec<Option<String>> = Vec::new();
    for _ in 0..count {
        let before = entry_times(state);
        let (name, ok) = step(state, shared, rng, n)?;
        let after = entry_times(state);
        let pushed = after.iter().find(|t| !before.contains(t)).cloned();
        match (ok, pushed) {
            (true, Some(time)) => {
                states.push(project_of(state)?);
                ops.push(name);
                times.push(time);
            }
            (true, None) => {
                // Recorded nothing: a no-op (or an ignore, which is not
                // survey content): the survey is the last state.
                let now = project_of(state)?;
                let last = states.last().ok_or_else(|| "no start state".to_string())?;
                check(body(&now)? == body(last)?, || {
                    format!("{name} changed the survey without an entry")
                })?;
            }
            (false, _) => {}
        }
    }
    let recorded = ops.len();
    // The limits may have trimmed the oldest: step as far as entries remain.
    let live = entry_times(state);
    let reachable = times.iter().rev().take_while(|t| live.contains(t)).count();
    for k in 0..reachable {
        let r = state.undo();
        let op = ops[recorded - k - 1];
        check(r.success, || format!("undo {k} ({op}): {:?}", r.error))?;
        let expected = &states[recorded - k - 1];
        same_state(&valid(state, "undo")?, expected).map_err(|e| {
            format!("undo {k} of {recorded} ({op}) did not land on its recorded state, {e}")
        })?;
    }
    for k in 0..reachable {
        let index = recorded - reachable + k;
        let op = ops[index];
        let r = state.redo();
        check(r.success, || format!("redo {k} ({op}): {:?}", r.error))?;
        let expected = &states[index + 1];
        same_state(&valid(state, "redo")?, expected).map_err(|e| {
            format!("redo {k} of {reachable} ({op}) did not land on its recorded state, {e}")
        })?;
    }
    Ok(recorded)
}

/// One seed of the saving fuzz.
fn saving_seed(seed: u64, steps: usize) -> Result<BTreeMap<&'static str, usize>, Fail> {
    let dir = TempDir::new().map_err(|e| e.to_string())?;
    let origin = origin_of(seed);
    let (first, mut path, fragments) = start(seed, origin, dir.path())?;
    let mut state = first;
    let mut rng = Rng::new(seed);
    let mut shared = shared_generator(seed);
    let mut n = 0u64;
    let mut seen: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut copies = 0usize;
    *seen
        .entry(match origin {
            Origin::V2 => "origin v2",
            Origin::V2Linked => "origin v2 linked",
            Origin::V2Unknown => "origin v2 unknown members",
            Origin::Legacy => "origin legacy",
            Origin::New => "origin new",
        })
        .or_default() += 1;
    let mut saved_once = origin != Origin::Legacy;
    for _ in 0..steps {
        let action = rng.below(100);
        let what: &'static str = match action {
            0..=64 => {
                let (name, ok) = step(&mut state, &mut shared, &mut rng, &mut n)?;
                if ok {
                    *seen.entry(name).or_default() += 1;
                    if !name.contains(' ') {
                        *seen.entry("shared generator").or_default() += 1;
                    }
                }
                "command"
            }
            65..=71 => {
                let ok = state.undo().success;
                valid(&state, "undo")?;
                if ok {
                    "undo"
                } else {
                    "undo refused"
                }
            }
            72..=75 => {
                let ok = state.redo().success;
                valid(&state, "redo")?;
                if ok {
                    "redo"
                } else {
                    "redo refused"
                }
            }
            76..=84 => {
                let redo = !state.get_redo_stack().is_empty();
                state = save_and_reopen(&state, &path)?;
                saved_once = true;
                if redo {
                    "save with redo"
                } else {
                    "save"
                }
            }
            85..=89 => {
                display_toggle(&state, &path, &mut rng)?;
                "display toggle"
            }
            90..=93 if saved_once => {
                external_change(&state, &path)?;
                "external change"
            }
            94..=96 if saved_once => {
                copies += 1;
                path = save_copy(&state, &path, dir.path(), copies)?;
                "save a copy"
            }
            _ => continue,
        };
        *seen.entry(what).or_default() += 1;
    }
    let recorded = forward_exactness(&mut state, &mut shared, &mut rng, &mut n, FORWARD)?;
    *seen.entry("forward steps").or_default() += recorded;
    let last = save_and_reopen(&state, &path)?;
    let text = String::from_utf8_lossy(&read(&path)?).into_owned();
    for needle in &fragments {
        check(text.contains(needle), || {
            format!("the unknown member {needle} was lost")
        })?;
    }
    drop(last);
    if origin == Origin::Legacy {
        drop_backups(&format!("c24-{seed}"));
    }
    Ok(seen)
}

fn run_saving(start: u64, count: u64, steps: usize, test: &str) -> BTreeMap<&'static str, usize> {
    let mut totals: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut failures = Vec::new();
    for seed in start..start + count {
        match catch_unwind(AssertUnwindSafe(|| saving_seed(seed, steps))) {
            Ok(Ok(seen)) => {
                for (k, v) in seen {
                    *totals.entry(k).or_default() += v;
                }
            }
            Ok(Err(why)) => failures.push((seed, why)),
            Err(payload) => failures.push((
                seed,
                payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
                    .unwrap_or_else(|| "panicked".to_string()),
            )),
        }
    }
    for (seed, why) in &failures {
        eprintln!(
            "C24 saving fuzz FAILED seed={seed} origin={:?} replay=\"STALAGMIGHT_FUZZ_SEED_START={seed} \
             STALAGMIGHT_FUZZ_SEEDS=1 cargo test -p stalagmight-core --lib {TEST_PATH}::{test} \
             -- --include-ignored\" detail={}",
            origin_of(*seed),
            why.replace('\n', " | ")
        );
    }
    eprintln!("C24 saving fuzz: seeds={count} failed={}", failures.len());
    assert!(
        failures.is_empty(),
        "{} seed(s) failed, see the lines above",
        failures.len()
    );
    totals
}

#[test]
fn saving_fuzz_window() {
    let (start, count) = window(CI_SEEDS);
    let totals = run_saving(start, count, STEPS, "saving_fuzz_window");
    if env_u64("STALAGMIGHT_FUZZ_SEEDS").is_some() {
        return;
    }
    for wanted in [
        "origin v2",
        "origin v2 linked",
        "origin v2 unknown members",
        "origin legacy",
        "origin new",
        "save",
        "display toggle",
        "external change",
        "save a copy",
        "undo",
        "update shot",
        "shared generator",
        "forward steps",
    ] {
        assert!(
            totals.get(wanted).copied().unwrap_or(0) > 0,
            "the CI window never reached {wanted}: {totals:#?}"
        );
    }
}

/// `cargo test -p stalagmight-core --lib saving_fuzz_wide -- --ignored
/// --nocapture`: 2,000 seeds (or the env window); prints the coverage.
#[test]
#[ignore = "wide run: 2,000 seeds by default; set STALAGMIGHT_FUZZ_SEED_START / STALAGMIGHT_FUZZ_SEEDS"]
fn saving_fuzz_wide() {
    let (start, count) = window(WIDE_SEEDS);
    let totals = run_saving(start, count, STEPS + 20, "saving_fuzz_wide");
    for (k, v) in &totals {
        eprintln!("coverage: {k} = {v}");
    }
}

/// Read-only files: an unknown required feature. Over seeded sequences every
/// edit, undo, redo and save is refused, and the survey bytes never change;
/// a display toggle rewrites only `local`.
#[test]
fn read_only_files_are_never_written() {
    let (start, count) = window(8);
    for seed in start..start + count {
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let path = dir.path().join("future.stalx");
        let p = walk_survey(seed, 25);
        let text =
            String::from_utf8(encode(&p, &Extras::default(), Mode::Normal).unwrap_or_default())
                .unwrap_or_default();
        let text = text.replacen(
            "\"requires\":[\"archived-stations\"]",
            "\"requires\":[\"archived-stations\",\"zz-future\"]",
            1,
        );
        std::fs::write(&path, &text).unwrap_or_else(|e| panic!("{e}"));
        let original = survey_members(text.as_bytes());
        let mut state = open(&path).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
        assert!(
            state.read_only_reason().is_some(),
            "seed {seed}: opens read-only"
        );
        let mut rng = Rng::new(seed);
        let mut shared = shared_generator(seed);
        let mut n = 0;
        for _ in 0..30 {
            let before = project_of(&state).unwrap_or_else(|e| panic!("{e}"));
            let (name, result) = draw(&mut state, &before, &mut shared, &mut rng, &mut n);
            assert!(
                !result.success,
                "seed {seed}: {name} succeeded on a read-only file"
            );
            assert!(
                body(&project_of(&state).unwrap_or_else(|e| panic!("{e}"))).ok()
                    == body(&before).ok(),
                "seed {seed}: a refused {name} changed the survey"
            );
            if rng.chance(0.2) {
                let r = state.execute(Command::SaveProject { path: None });
                assert!(
                    !r.success
                        && r.error
                            .as_deref()
                            .unwrap_or_default()
                            .starts_with(READ_ONLY_FILE_CODE),
                    "seed {seed}: a save of a read-only file: {:?}",
                    r.error
                );
            }
            if rng.chance(0.2) {
                let _ = state.save_display_setting("zz.view", "true");
            }
            if rng.chance(0.1) {
                assert!(
                    !state.undo().success,
                    "seed {seed}: an undo on a read-only file"
                );
            }
            let now = std::fs::read(&path).unwrap_or_default();
            assert_eq!(
                survey_members(&now),
                original,
                "seed {seed}: the survey bytes changed"
            );
        }
    }
}

// ----- the private corpus --------------------------------------------------------------

fn name_hash(name: &str) -> String {
    const NAMESPACE: uuid::Uuid = uuid::Uuid::from_u128(0x1b0e_4a2c_77d9_4c31_9f05_2e6a_c8d4_5b17);
    uuid::Uuid::new_v5(&NAMESPACE, name.as_bytes())
        .simple()
        .to_string()[..12]
        .to_string()
}

fn stalx_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            stalx_files(&path, out);
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("stalx"))
        {
            out.push(path);
        }
    }
}

/// One corpus file: the open matches the migration, then `walks` command
/// sequences each undo and redo exactly and save to a reopenable fixpoint.
/// Returns the sequence that failed (its seed) with a class, never a text.
fn corpus_file(
    original: &Path,
    scratch: &Path,
    first: u64,
    walks: u64,
) -> Result<u64, (Option<u64>, &'static str)> {
    let copy = scratch.join("corpus.stalx");
    std::fs::copy(original, &copy).map_err(|_| (None, "copy"))?;
    let bytes = std::fs::read(&copy).map_err(|_| (None, "read"))?;
    let legacy = stalx_v2::sniff(&bytes) == Format::Legacy;
    let state = open(&copy).map_err(|_| (None, "open"))?;
    let opened = project_of(&state).map_err(|_| (None, "open"))?;
    if !opened.validate().is_empty() {
        return Err((None, "open-invalid"));
    }
    if legacy {
        let config = state.get_config().closure_adjustment;
        let migrated = crate::persistence::migrate::migrate_path_for_app(&copy, &config)
            .map_err(|_| (None, "migrate"))?;
        if !migrated.report.verification.is_clean() {
            return Err((None, "unexplained-position-mismatch"));
        }
        let (a, b) = (body(&opened), body(&migrated.project));
        if a.is_err() || a != b {
            return Err((None, "open-differs-from-migrate"));
        }
    }
    drop(state);
    let mut steps = 0;
    for seed in first..first + walks {
        std::fs::copy(original, &copy).map_err(|_| (Some(seed), "copy"))?;
        let mut state = open(&copy).map_err(|_| (Some(seed), "open"))?;
        let mut rng = Rng::new(seed);
        let mut shared = shared_generator(seed);
        let mut n = 0;
        steps += forward_exactness(&mut state, &mut shared, &mut rng, &mut n, CORPUS_STEPS)
            .map_err(|_| (Some(seed), "undo-redo-exactness"))?;
        save_and_reopen(&state, &copy).map_err(|_| (Some(seed), "save-reopen-fixpoint"))?;
        drop_backups("corpus");
    }
    Ok(steps as u64)
}

/// `STALAGMIGHT_MIGRATION_CORPUS=<dir> cargo test -p stalagmight-core --lib
/// transition_fuzz_corpus -- --ignored --nocapture`: see the module doc.
/// `STALAGMIGHT_CORPUS_FILE=<hash>` runs one file; `STALAGMIGHT_FUZZ_WALKS`
/// and `STALAGMIGHT_FUZZ_SEED_START` choose the sequences.
#[test]
#[ignore = "runs on a private corpus: set STALAGMIGHT_MIGRATION_CORPUS"]
fn transition_fuzz_corpus() {
    let Some(dir) = std::env::var_os("STALAGMIGHT_MIGRATION_CORPUS") else {
        eprintln!("STALAGMIGHT_MIGRATION_CORPUS is not set");
        return;
    };
    crate::app_dirs::init_for_test();
    let dir = PathBuf::from(dir);
    let only = std::env::var("STALAGMIGHT_CORPUS_FILE").ok();
    let walks = env_u64("STALAGMIGHT_FUZZ_WALKS").unwrap_or(CORPUS_WALKS);
    let first = env_u64("STALAGMIGHT_FUZZ_SEED_START").unwrap_or(0);
    let mut files = Vec::new();
    stalx_files(&dir, &mut files);
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let (mut ok, mut failed, mut steps) = (0usize, 0usize, 0u64);
    let mut classes: BTreeMap<&'static str, usize> = BTreeMap::new();
    for path in &files {
        let relative = path.strip_prefix(&dir).unwrap_or(path);
        let tag = name_hash(&relative.to_string_lossy());
        if only.as_deref().is_some_and(|h| h != tag) {
            continue;
        }
        let Ok(scratch) = TempDir::new() else {
            eprintln!("file={tag} status=no-scratch");
            continue;
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            corpus_file(path, scratch.path(), first, walks)
        }));
        match outcome {
            Ok(Ok(n)) => {
                ok += 1;
                steps += n;
                eprintln!("file={tag} status=ok walks={walks} recorded_steps={n}");
            }
            other => {
                failed += 1;
                let (seed, class) = match other {
                    Ok(Err(pair)) => pair,
                    _ => (None, "panic"),
                };
                *classes.entry(class).or_default() += 1;
                let seed_env = seed.map_or(String::new(), |s| {
                    format!("STALAGMIGHT_FUZZ_SEED_START={s} STALAGMIGHT_FUZZ_WALKS=1 ")
                });
                eprintln!(
                    "file={tag} status=FAILED class={class} replay=\"STALAGMIGHT_MIGRATION_CORPUS=<dir> \
                     STALAGMIGHT_CORPUS_FILE={tag} {seed_env}cargo test -p stalagmight-core --lib \
                     {TEST_PATH}::transition_fuzz_corpus -- --ignored --nocapture\""
                );
            }
        }
    }
    std::panic::set_hook(hook);
    eprintln!(
        "corpus: files={} ok={ok} failed={failed} recorded_steps={steps}",
        files.len()
    );
    for (class, n) in &classes {
        eprintln!("corpus: failed.{class}={n}");
    }
    assert_eq!(failed, 0, "corpus files failed a transition property");
}

/// The generator itself: a sequence of operations on a fresh walk survey
/// reaches most command families successfully (a vacuous fuzz would pass
/// everything).
#[test]
fn the_command_generator_reaches_the_command_families() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let path = dir.path().join("g.stalx");
    let p = walk_survey(5, 30);
    std::fs::write(
        &path,
        encode(&p, &Extras::default(), Mode::Normal).unwrap_or_default(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let mut state = open(&path).unwrap_or_else(|e| panic!("{e}"));
    let mut rng = Rng::new(5);
    let mut shared = shared_generator(5);
    let mut n = 0;
    let mut ok: BTreeMap<&'static str, usize> = BTreeMap::new();
    for _ in 0..400 {
        let (name, success) =
            step(&mut state, &mut shared, &mut rng, &mut n).unwrap_or_else(|e| panic!("{e}"));
        if success {
            *ok.entry(name).or_default() += 1;
        }
    }
    let (own, shared): (Vec<&str>, Vec<&str>) = ok.keys().partition(|name| name.contains(' '));
    assert!(
        own.len() >= 18,
        "only {} of this file's families succeeded: {ok:#?}",
        own.len()
    );
    assert!(
        shared.len() >= 8,
        "only {} of the undo fuzzer's families succeeded: {ok:#?}",
        shared.len()
    );
}
