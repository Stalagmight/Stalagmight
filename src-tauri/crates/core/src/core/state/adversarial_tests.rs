//! ADVERSARIAL arguments: what the command layer does with input it should
//! refuse, on the station-graph pipeline.
//!
//! # The hole this fills
//!
//! Every other generator is valid by construction: it picks a shot that
//! exists, in a line that exists, and gives it a length in a sane range. So the
//! command layer's VALIDATION, which is most of what it is, is fuzzed only
//! here. The `execute_*` recipes are the sharpest edge: they do their own
//! argument checking, and several take the command lock and resolve slots
//! before they reach the pipeline.
//!
//! # Three invariants
//!
//! **A rejected command is a PERFECT no-op.** Every oracle is compared: the
//! canonical v2 text of the survey (bit for bit), the integrity verdict, the
//! Closures Computed View (bit for bit: the solve is deterministic), the id
//! allocators (stored counters, the derived next line id, every line's session
//! shot allocator), both history stacks, and the unsaved-changes flag. A
//! command that mutates and THEN fails is the worst shape a validation bug
//! can take, because the failure tells the user nothing happened. A Batch
//! whose later sub-command is hostile is in the mix on purpose: its earlier,
//! valid sub-commands must roll back too.
//!
//! **An accepted command leaves a valid survey.** The pipeline validates after
//! every mutation and rolls back on a violation; an accepted command whose
//! result breaks an invariant (plan section 2) means some path bypassed it.
//!
//! **NO COMMAND EVER PANICS.** Every move runs inside `catch_unwind`, and the
//! panic site is recorded by a hook. No site is exempt: the old suite's
//! classifier went with the findings it classified (all fixed before the
//! switch), and none is carried over.
//!
//! # What "adversarial" means here
//!
//! `Command` values are built directly, so this tests what the TYPE admits
//! rather than what the IPC deserializer would accept: `f64` fields admit NaN
//! and infinity, `u32` slot indices admit values far past any line's length,
//! strings admit 64 kB and NULs (which the v2 encoder must escape, not choke
//! on). The IPC layer's own validation is the harness's job.
//!
//! # Data
//!
//! Everything is generated in code. No fixture is read, copied or derived from.

use std::collections::BTreeMap;

use super::robustness_kit::Targets;
use super::robustness_kit::{
    build_project, differences, observe, panic_text, random_shot, read_targets, Observed, Rng,
    COLORS, GEOMETRY_EXTREME,
};
use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::survey::Project;
use crate::models::{
    ClosureStrategy, CombineLinesOptions, CombinePick, Coordinates, GpsCoordinates, LineId,
    LineUpdates, ShotData, ShotId, ShotKind, ShotUpdates, SplitAt, SplitLineOptions, StationId,
    StationRef, StationRename, FEATURE_ID_STRIDE,
};
use crate::test_helpers::isolated_config;

// ===========================================================================
// Hostile values
// ===========================================================================

/// A line id that is not a line: past the allocator, the extremes, zero.
fn hostile_line_id(rng: &mut Rng, live: &[LineId]) -> LineId {
    match rng.below(6) {
        0 => LineId::MAX,
        1 => LineId::MIN,
        2 => 999_999,
        3 => live.last().map_or(0, |id| id.wrapping_add(1)),
        4 => 0,
        _ => live.first().map_or(0, |id| id.wrapping_sub(1)),
    }
}

/// A shot id that is not a shot in the line it is handed to. One stride away
/// is "the same position in the next line": the session ids are
/// `line * FEATURE_ID_STRIDE + seq`, so it looks plausible.
fn hostile_shot_id(rng: &mut Rng, live: &[ShotId]) -> ShotId {
    match rng.below(6) {
        0 => ShotId::MAX,
        1 => ShotId::MIN,
        2 => 0,
        3 => live.last().map_or(0, |id| id.wrapping_add(1)),
        4 => live.first().map_or(0, |id| id.wrapping_sub(1)),
        _ => live
            .first()
            .map_or(0, |id| id.wrapping_add(FEATURE_ID_STRIDE)),
    }
}

/// A slot index at or past a boundary (`u32`: a caller computing
/// `index - 1` wraps).
fn hostile_station_index(rng: &mut Rng, length: u32) -> u32 {
    match rng.below(6) {
        0 => 0,
        1 => length,
        2 => length.wrapping_add(1),
        3 => u32::MAX,
        4 => u32::MAX - 1,
        _ => length.wrapping_add(u32::MAX / 2),
    }
}

/// A float the type admits and the survey cannot mean.
fn hostile_float(rng: &mut Rng) -> f64 {
    *rng.pick(&[
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        0.0,
        -0.0,
        f64::MIN_POSITIVE,
        f64::MAX,
        f64::MIN,
        -1.0,
        GEOMETRY_EXTREME,
        GEOMETRY_EXTREME * 1.000_000_001,
        1e308,
    ])
}

/// A string the type admits: empty, enormous, control characters, a NUL, bidi
/// and zero-width characters, and SQL (harmless now: nothing parses it).
fn hostile_string(rng: &mut Rng) -> String {
    match rng.below(7) {
        0 => String::new(),
        1 => "\0".to_string(),
        2 => format!("before\0after-{}", rng.range(0, 99)),
        3 => "\u{1}\u{2}\u{7f}\r\n\t\"\\".to_string(),
        4 => "A".repeat(65_536),
        5 => "\u{fffd}\u{202e}\u{200b}".repeat(64),
        _ => "'; DROP TABLE shots; --".to_string(),
    }
}

/// A keep the type admits and no dialog offers (RS 4.1, 4.2, 5.2): the
/// extremes, the station counter, an archived record's Station ID, a live
/// station that is (most likely) neither end.
fn hostile_keep(rng: &mut Rng, project: &Project) -> StationId {
    let live = project.topology().stations().to_vec();
    let record = project
        .stations
        .values()
        .flat_map(|s| s.archived.iter().map(|r| r.id))
        .next();
    match rng.below(6) {
        0 => StationId::MAX,
        1 => 0,
        2 => project.counters.next_station_id,
        3 => record.unwrap_or(StationId::MAX - 1),
        4 => live.last().copied().unwrap_or(1),
        _ => live.get(live.len() / 2).copied().unwrap_or(2),
    }
}

/// Renames the type admits and no dialog sends (RS 4.1 step 6): unknown and
/// archived Station IDs, hostile names, a name another station already
/// shows, the same Station ID twice.
fn hostile_renames(rng: &mut Rng, project: &Project) -> Option<Vec<StationRename>> {
    if rng.chance(10) {
        return None;
    }
    let live = project.topology().stations().to_vec();
    let record = project
        .stations
        .values()
        .flat_map(|s| s.archived.iter().map(|r| r.id))
        .next();
    let mut out: Vec<StationRename> = Vec::new();
    for _ in 0..1 + rng.below(3) {
        let station_id = match rng.below(5) {
            0 => StationId::MAX,
            1 => record.unwrap_or(StationId::MAX - 1),
            2 => project.counters.next_station_id,
            _ => live.get(rng.below(live.len().max(1))).copied().unwrap_or(0),
        };
        let name = match rng.below(4) {
            0 => hostile_string(rng),
            1 => live
                .get(rng.below(live.len().max(1)))
                .and_then(|s| project.shown_name(*s))
                .unwrap_or_default()
                .to_string(),
            2 => String::new(),
            _ => format!("ren-{}", rng.range(0, 99)),
        };
        out.push(StationRename { station_id, name });
    }
    if rng.chance(25) {
        let twice = StationRename {
            station_id: out[0].station_id,
            name: format!("twice-{}", rng.range(0, 99)),
        };
        out.push(twice);
    }
    Some(out)
}

/// Two live slots, on different lines when there are two lines.
fn live_pair(rng: &mut Rng, targets: &Targets) -> Option<(StationRef, StationRef)> {
    let slots = &targets.stations;
    if slots.is_empty() {
        return None;
    }
    let (la, ia) = slots[rng.below(slots.len())];
    let others: Vec<&(LineId, u32)> = slots.iter().filter(|(l, _)| *l != la).collect();
    let (lb, ib) = if others.is_empty() {
        slots[rng.below(slots.len())]
    } else {
        *others[rng.below(others.len())]
    };
    Some((StationRef::new(la, ia), StationRef::new(lb, ib)))
}

/// A shot whose every field is drawn from the hostile pool. The uuid is left
/// out: the command doors mint their own identity (#423).
fn hostile_shot(rng: &mut Rng) -> ShotData {
    ShotData {
        uuid: None,
        azimuth_in: hostile_float(rng),
        azimuth_out: hostile_float(rng),
        length: hostile_float(rng),
        depth_in: hostile_float(rng),
        depth_out: hostile_float(rng),
        pitch_in: rng.chance(50).then(|| hostile_float(rng)),
        pitch_out: rng.chance(50).then(|| hostile_float(rng)),
        left: rng.chance(50).then(|| hostile_float(rng)),
        right: rng.chance(50).then(|| hostile_float(rng)),
        up: rng.chance(50).then(|| hostile_float(rng)),
        down: rng.chance(50).then(|| hostile_float(rng)),
        comment: rng.chance(50).then(|| hostile_string(rng)),
        kind: *rng.pick(&[ShotKind::Normal, ShotKind::Jump, ShotKind::Reference]),
        marker_index: rng
            .chance(50)
            .then(|| u32::try_from(rng.range(0, 9)).unwrap_or(0)),
        temperature: rng.chance(50).then(|| hostile_float(rng)),
        credit: None,
    }
}

fn hostile_offset(rng: &mut Rng) -> Coordinates {
    Coordinates::new(hostile_float(rng), hostile_float(rng), hostile_float(rng))
}

// ===========================================================================
// The oracles
// ===========================================================================

/// [`Observed`] plus the unsaved-changes flag: a refused edit must not mark
/// the project dirty either.
struct Seen {
    observed: Observed,
    dirty: bool,
}

fn see(state: &AppState) -> Seen {
    Seen {
        observed: observe(state),
        dirty: state.has_unsaved_changes(),
    }
}

/// Every way a REJECTED command failed to be a no-op.
fn not_a_no_op(before: &Seen, after: &Seen) -> Vec<String> {
    let mut out = differences(&before.observed, &after.observed);
    if before.dirty != after.dirty {
        out.push(format!(
            "it changed the unsaved-changes flag: {} -> {}",
            before.dirty, after.dirty
        ));
    }
    out
}

// ===========================================================================
// The moves
// ===========================================================================

/// One adversarial call, with the label a failure payload needs.
struct Move {
    label: String,
    result: CommandResult,
}

fn split_options(rng: &mut Rng) -> SplitLineOptions {
    SplitLineOptions {
        name_a: hostile_string(rng),
        name_b: hostile_string(rng),
        color_a: if rng.chance(50) {
            hostile_string(rng)
        } else {
            (*rng.pick(COLORS)).to_string()
        },
        color_b: (*rng.pick(COLORS)).to_string(),
    }
}

/// Draw and run one hostile call against `state`. Weighted towards the
/// hand-written recipes, which do their own argument checking.
fn draw(state: &AppState, rng: &mut Rng) -> Option<Move> {
    let targets = read_targets(state)?;
    let lines: Vec<LineId> = targets.lines.iter().map(|(id, _)| *id).collect();
    if lines.is_empty() {
        return None;
    }
    let live_line = lines[rng.below(lines.len())];
    let live_shots = targets
        .lines
        .iter()
        .find(|(id, _)| *id == live_line)
        .map(|(_, shots)| shots.clone())
        .unwrap_or_default();
    // Half aim a hostile id at a real line, half any id at a hostile line.
    let line_id = if rng.chance(50) {
        live_line
    } else {
        hostile_line_id(rng, &lines)
    };
    let shot_id = if rng.chance(35) && !live_shots.is_empty() {
        live_shots[rng.below(live_shots.len())]
    } else {
        hostile_shot_id(rng, &live_shots)
    };
    let length = u32::try_from(live_shots.len()).unwrap_or(u32::MAX);
    let index = hostile_station_index(rng, length);

    let call = match rng.below(30) {
        // ---- the hand-written recipes ----
        0 => Move {
            label: format!("execute_delete_shot_range_shift(s={line_id}, {shot_id}..{shot_id})"),
            result: state.execute_delete_shot_range_shift(line_id, shot_id, shot_id),
        },
        1 => {
            let other = hostile_shot_id(rng, &live_shots);
            Move {
                label: format!("execute_delete_shot_range_shift(s={line_id}, {shot_id}..{other})"),
                result: state.execute_delete_shot_range_shift(line_id, shot_id, other),
            }
        }
        2 => {
            let other = hostile_shot_id(rng, &live_shots);
            Move {
                label: format!("execute_delete_shot_range_split(s={line_id}, {shot_id}..{other})"),
                result: state.execute_delete_shot_range_split(
                    line_id,
                    shot_id,
                    other,
                    hostile_string(rng),
                    hostile_string(rng),
                ),
            }
        }
        3 => Move {
            label: format!("execute_delete_shot_and_split(s={line_id}, shot={shot_id})"),
            result: state.execute_delete_shot_and_split(
                line_id,
                shot_id,
                hostile_string(rng),
                hostile_string(rng),
            ),
        },
        4 => Move {
            label: format!("execute_delete_last_shot_and_line(s={line_id}, shot={shot_id})"),
            result: state.execute_delete_last_shot_and_line(line_id, shot_id),
        },
        5 => {
            let distance = hostile_float(rng);
            Move {
                label: format!(
                    "execute_split_shot_at_distance(s={line_id}, shot={shot_id}, d={distance})"
                ),
                result: state.execute_split_shot_at_distance(line_id, shot_id, distance),
            }
        }
        6 => {
            let at = if rng.chance(50) {
                SplitAt::Station {
                    station_index: index,
                }
            } else {
                SplitAt::Shot {
                    shot_id,
                    distance: hostile_float(rng),
                }
            };
            Move {
                label: format!("execute_split_line(s={line_id}, {at:?}, hostile names)"),
                result: state.execute_split_line(line_id, at, split_options(rng)),
            }
        }
        7 => {
            let other = hostile_line_id(rng, &lines);
            Move {
                label: format!("execute_combine_lines(kept={line_id}, other={other})"),
                result: state.execute_combine_lines(
                    line_id,
                    other,
                    CombineLinesOptions {
                        name: hostile_string(rng),
                        color: hostile_string(rng),
                        swap_lr: rng.chance(50),
                        comment_pick: CombinePick::Kept,
                        name_pick: CombinePick::Kept,
                        join: None,
                    },
                ),
            }
        }
        // ---- the ordinary command surface ----
        8 => Move {
            label: format!("CreateShot(s={line_id}, hostile shot)"),
            result: state.execute(Command::CreateShot {
                line_id,
                shot: hostile_shot(rng),
                after_shot_id: rng.chance(50).then_some(shot_id),
                before_shot_id: rng.chance(25).then_some(shot_id),
            }),
        },
        9 => Move {
            label: format!("UpdateShot(s={line_id}, shot={shot_id}, hostile updates)"),
            result: state.execute(Command::UpdateShot {
                line_id,
                shot_id,
                updates: ShotUpdates {
                    length: rng.chance(60).then(|| hostile_float(rng)),
                    azimuth_in: rng.chance(40).then(|| hostile_float(rng)),
                    depth_out: rng.chance(40).then(|| hostile_float(rng)),
                    pitch_in: rng
                        .chance(30)
                        .then(|| rng.chance(50).then(|| hostile_float(rng))),
                    left: rng
                        .chance(30)
                        .then(|| rng.chance(50).then(|| hostile_float(rng))),
                    comment: rng
                        .chance(30)
                        .then(|| rng.chance(70).then(|| hostile_string(rng))),
                    ..ShotUpdates::default()
                },
            }),
        },
        10 => Move {
            label: "CreateLine(hostile metadata, hostile shots)".to_string(),
            result: state.execute(Command::CreateLine {
                name: rng.chance(60).then(|| hostile_string(rng)),
                surveyor: rng.chance(50).then(|| hostile_string(rng)),
                date: rng.chance(50).then(|| {
                    *rng.pick(&[
                        chrono::NaiveDate::MIN,
                        chrono::NaiveDate::MAX,
                        chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap_or_default(),
                    ])
                }),
                color: if rng.chance(50) {
                    hostile_string(rng)
                } else {
                    (*rng.pick(COLORS)).to_string()
                },
                direction: None,
                shots: (0..rng.below(3)).map(|_| hostile_shot(rng)).collect(),
                cave_section_id: rng.chance(20).then(|| *rng.pick(&[0, u32::MAX, 999_999])),
            }),
        },
        11 => Move {
            label: format!("UpdateLineMetadata(s={line_id}, hostile)"),
            result: state.execute(Command::UpdateLineMetadata {
                line_id,
                updates: LineUpdates {
                    name: rng
                        .chance(50)
                        .then(|| rng.chance(70).then(|| hostile_string(rng))),
                    surveyor: rng
                        .chance(40)
                        .then(|| rng.chance(70).then(|| hostile_string(rng))),
                    color: rng.chance(40).then(|| hostile_string(rng)),
                    quality: rng.chance(40).then(|| hostile_float(rng)),
                    ..LineUpdates::default()
                },
            }),
        },
        12 => Move {
            label: format!("MoveLine(s={line_id}, hostile offset)"),
            result: state.execute(Command::MoveLine {
                line_id,
                offset: hostile_offset(rng),
            }),
        },
        13 => Move {
            label: format!("MoveSegment(s={line_id}, hostile offset)"),
            result: state.execute(Command::MoveSegment {
                line_id,
                offset: hostile_offset(rng),
            }),
        },
        14 => Move {
            label: format!("MoveStation({line_id}/{index}, hostile offset)"),
            result: state.execute(Command::MoveStation {
                station_ref: StationRef::new(line_id, index),
                offset: hostile_offset(rng),
            }),
        },
        15 => {
            let other = hostile_line_id(rng, &lines);
            Move {
                label: format!("MergeStations({line_id}/{index}, {other}/{index})"),
                result: state.execute(Command::MergeStations {
                    station_a: StationRef::new(line_id, index),
                    station_b: StationRef::new(other, index),
                    keep: None,
                    renames: None,
                }),
            }
        }
        16 => {
            let other = hostile_line_id(rng, &lines);
            Move {
                label: format!("UnmergeStationPair({line_id}/{index}, {other}/0)"),
                result: state.execute(Command::UnmergeStationPair {
                    station_a: StationRef::new(line_id, index),
                    station_b: StationRef::new(other, 0),
                    renames: None,
                    rest_keep: None,
                }),
            }
        }
        17 => Move {
            label: format!("UnmergeStations({line_id}/{index})"),
            result: state.execute(Command::UnmergeStations {
                station_ref: StationRef::new(line_id, index),
                renames: None,
                rest_keep: None,
            }),
        },
        18 => {
            let other = hostile_line_id(rng, &lines);
            Move {
                label: format!("CreateLoopClosure({line_id}/{index}, {other}/{index})"),
                result: state.execute(Command::CreateLoopClosure {
                    station_a: StationRef::new(line_id, index),
                    station_b: StationRef::new(other, index),
                    closure_strategy: *rng.pick(&[
                        ClosureStrategy::Move,
                        ClosureStrategy::LeastSquaresWeighted,
                        ClosureStrategy::ProportionalByLength,
                    ]),
                    keep: None,
                    renames: None,
                }),
            }
        }
        19 => {
            let id = *rng.pick(&[0, u64::MAX, 1 << 53, targets.closures.len() as u64 + 99]);
            Move {
                label: format!("RemoveLoopClosure({id})"),
                result: state.execute(Command::RemoveLoopClosure {
                    closure_id: id,
                    renames: None,
                }),
            }
        }
        20 => {
            // `GpsCoordinates::new` validates, so an out-of-range pair yields
            // no command at all: confirmed rather than worked around.
            match GpsCoordinates::new(hostile_float(rng), hostile_float(rng), hostile_float(rng)) {
                Ok(coordinates) => Move {
                    label: format!("SetZeroDatum({line_id}/{index}, hostile gps)"),
                    result: state.execute(Command::SetZeroDatum {
                        station_ref: StationRef::new(line_id, index),
                        coordinates,
                        accuracy_m: rng.chance(50).then(|| hostile_float(rng)),
                        entrance: None,
                    }),
                },
                Err(_) => Move {
                    label: format!("SetStationComment({line_id}/{index}, hostile)"),
                    result: state.execute(Command::SetStationComment {
                        station_ref: StationRef::new(line_id, index),
                        comment: rng.chance(70).then(|| hostile_string(rng)),
                    }),
                },
            }
        }
        21 => Move {
            label: format!("RemoveZeroDatum({line_id}/{index})"),
            result: state.execute(Command::RemoveZeroDatum {
                station_ref: StationRef::new(line_id, index),
            }),
        },
        22 => Move {
            label: format!("SetStationName({line_id}/{index}, hostile)"),
            result: state.execute(Command::SetStationName {
                station_ref: StationRef::new(line_id, index),
                name: hostile_string(rng),
            }),
        },
        23 => Move {
            label: format!("ReverseLine(s={line_id})"),
            result: state.execute(Command::ReverseLine {
                line_id,
                swap_left_right: rng.chance(50),
            }),
        },
        24 => {
            // A valid edit first, then a hostile one: the batch must roll the
            // valid half back too.
            let valid = Command::CreateShot {
                line_id: live_line,
                shot: random_shot(rng),
                after_shot_id: None,
                before_shot_id: None,
            };
            let hostile = Command::UpdateShot {
                line_id,
                shot_id: hostile_shot_id(rng, &live_shots),
                updates: ShotUpdates {
                    length: Some(hostile_float(rng)),
                    ..ShotUpdates::default()
                },
            };
            Move {
                label: format!(
                    "Batch[CreateShot(s={live_line}), UpdateShot(s={line_id}, hostile)]"
                ),
                result: state.execute(Command::Batch {
                    commands: vec![valid, hostile],
                    label: rng.chance(50).then(|| hostile_string(rng)),
                }),
            }
        }
        // ---- RS: the dialogs' answers, hostile, on live slots ----
        26 => {
            let project = state.get_project_state()?;
            let (a, b) = live_pair(rng, &targets)?;
            let keep = rng.chance(70).then(|| hostile_keep(rng, &project));
            let renames = hostile_renames(rng, &project);
            Move {
                label: format!("MergeStations({a}, {b}, keep={keep:?}, hostile renames)"),
                result: state.execute(Command::MergeStations {
                    station_a: a,
                    station_b: b,
                    keep,
                    renames,
                }),
            }
        }
        27 => {
            let project = state.get_project_state()?;
            let topology = project.topology();
            let junctions: Vec<_> = topology
                .stations()
                .iter()
                .copied()
                .filter(|&s| topology.is_junction(s))
                .collect();
            let junction = *junctions.get(rng.below(junctions.len().max(1)))?;
            let slots = topology.slots_of(junction).to_vec();
            let slot = slots[rng.below(slots.len())];
            let rest_keep = rng.chance(70).then(|| hostile_keep(rng, &project));
            let renames = hostile_renames(rng, &project);
            if rng.chance(50) {
                Move {
                    label: format!("UnmergeStations({slot}, rest_keep={rest_keep:?}, hostile)"),
                    result: state.execute(Command::UnmergeStations {
                        station_ref: slot,
                        renames,
                        rest_keep,
                    }),
                }
            } else {
                let other = slots[rng.below(slots.len())];
                Move {
                    label: format!(
                        "UnmergeStationPair({other}, {slot}, rest_keep={rest_keep:?}, hostile)"
                    ),
                    result: state.execute(Command::UnmergeStationPair {
                        station_a: other,
                        station_b: slot,
                        renames,
                        rest_keep,
                    }),
                }
            }
        }
        28 => {
            let project = state.get_project_state()?;
            let (a, b) = live_pair(rng, &targets)?;
            let keep = rng.chance(70).then(|| hostile_keep(rng, &project));
            let renames = hostile_renames(rng, &project);
            Move {
                label: format!("CreateLoopClosure({a}, {b}, keep={keep:?}, hostile renames)"),
                result: state.execute(Command::CreateLoopClosure {
                    station_a: a,
                    station_b: b,
                    closure_strategy: ClosureStrategy::Batch,
                    keep,
                    renames,
                }),
            }
        }
        29 => {
            let project = state.get_project_state()?;
            let id = *targets
                .closures
                .get(rng.below(targets.closures.len().max(1)))?;
            let renames = hostile_renames(rng, &project);
            Move {
                label: format!("RemoveLoopClosure({id}, hostile renames)"),
                result: state.execute(Command::RemoveLoopClosure {
                    closure_id: id,
                    renames,
                }),
            }
        }
        _ => Move {
            label: format!("SetShotKind(s={line_id}, shot={shot_id})"),
            result: state.execute(Command::SetShotKind {
                line_id,
                shot_id,
                kind: *rng.pick(&[ShotKind::Normal, ShotKind::Jump, ShotKind::Reference]),
            }),
        },
    };
    Some(call)
}

// ===========================================================================
// The run
// ===========================================================================

#[derive(Default)]
struct Outcome {
    attempted: usize,
    rejected: usize,
    accepted: usize,
    problems: Vec<String>,
}

// Where the last panic came from, captured by a hook: a `catch_unwind`
// payload carries the message and not the site. Thread-local, because libtest
// runs tests in parallel.
thread_local! {
    static LAST_PANIC_SITE: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// Install a panic hook that records the site, ONCE, and delegates to the
/// previous hook (silencing it would silence every other test's panics too).
fn install_location_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let site = info
                .location()
                .map(|location| format!("{}:{}", location.file(), location.line()));
            LAST_PANIC_SITE.with(|cell| *cell.borrow_mut() = site);
            previous(info);
        }));
    });
}

fn last_panic_site() -> Option<String> {
    LAST_PANIC_SITE.with(|cell| cell.borrow().clone())
}

fn run_adversarial(seed: u32, moves: usize, closures: bool) -> Outcome {
    let mut outcome = Outcome::default();
    install_location_hook();
    let Ok(state) = AppState::new() else {
        return outcome;
    };
    let mut rng = Rng::new(seed);
    build_project(&state, &mut rng, closures);
    // A little valid history first, so a rejected command has something real
    // to fail to disturb.
    for _ in 0..4 {
        if let Some((line_id, _)) = read_targets(&state).and_then(|t| t.lines.first().cloned()) {
            state.execute(Command::CreateShot {
                line_id,
                shot: random_shot(&mut rng),
                after_shot_id: None,
                before_shot_id: None,
            });
        }
    }

    for step in 0..moves {
        let before = see(&state);
        let drawn =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| draw(&state, &mut rng)));
        let drawn = match drawn {
            Ok(value) => value,
            Err(payload) => {
                outcome.problems.push(format!(
                    "seed {seed} step {step}: a command PANICKED at {}\n  {}",
                    last_panic_site().unwrap_or_else(|| "<unknown site>".to_string()),
                    panic_text(payload.as_ref())
                ));
                // The unwind may have gone through a guard; nothing after
                // this measures the app rather than a crippled state.
                break;
            }
        };
        let Some(call) = drawn else { continue };
        outcome.attempted += 1;
        if call.result.success {
            outcome.accepted += 1;
            let messages = state.integrity_violation_messages();
            if !messages.is_empty() {
                outcome.problems.push(format!(
                    "seed {seed} step {step}: an ACCEPTED command left an invalid survey\n  \
                     call: {}\n  violations: {messages:?}",
                    call.label
                ));
                break;
            }
            continue;
        }
        outcome.rejected += 1;
        let differences = not_a_no_op(&before, &see(&state));
        if !differences.is_empty() {
            outcome.problems.push(format!(
                "seed {seed} step {step}: a REJECTED command was not a no-op\n  call: {}\n  \
                 error: {:?}\n  {}",
                call.label,
                call.result.error,
                differences.join("\n  ")
            ));
        }
    }
    outcome
}

// ===========================================================================
// The gate
// ===========================================================================

/// PINNED. `adversarial_sweep` re-derives the set.
const GATE_SEEDS: &[u32] = &[1, 2, 3, 4, 5, 6, 7, 8];
const GATE_MOVES: usize = 120;

/// THE ADVERSARIAL GATE. Not `#[ignore]`d.
///
/// The run has to have REJECTED something: a generator that became valid by
/// construction would pass both invariants vacuously.
#[test]
fn adversarial_gate() {
    let _config = isolated_config();
    let mut problems = Vec::new();
    let (mut attempted, mut rejected, mut accepted) = (0, 0, 0);
    for &seed in GATE_SEEDS {
        let outcome = run_adversarial(seed, GATE_MOVES, seed % 2 == 0);
        attempted += outcome.attempted;
        rejected += outcome.rejected;
        accepted += outcome.accepted;
        problems.extend(outcome.problems);
    }
    println!("adversarial: {attempted} call(s), {rejected} rejected, {accepted} accepted");
    assert!(
        rejected > attempted / 10,
        "only {rejected} of {attempted} adversarial calls were rejected - the hostile generators \
         are producing valid input and the invariants are passing vacuously"
    );
    assert!(
        accepted > 0,
        "no adversarial call was accepted, so the accepted-leaves-a-valid-survey invariant is \
         vacuous"
    );
    assert!(
        problems.is_empty(),
        "{} adversarial failure(s):\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

/// Determinism, so a failing seed is a repro.
#[test]
fn the_adversarial_engine_is_deterministic() {
    let _config = isolated_config();
    for &seed in &GATE_SEEDS[..3] {
        let first = run_adversarial(seed, 60, false);
        let second = run_adversarial(seed, 60, false);
        assert_eq!(
            (
                first.attempted,
                first.rejected,
                first.accepted,
                first.problems.len()
            ),
            (
                second.attempted,
                second.rejected,
                second.accepted,
                second.problems.len()
            ),
            "seed {seed} did not replay identically"
        );
    }
}

/// The SWEEP, and the tool for re-choosing `GATE_SEEDS`.
///
/// ```text
/// STALAGMIGHT_RUST_ADV_SEEDS=200 STALAGMIGHT_RUST_ADV_MOVES=400 \
///   STALAGMIGHT_RUST_ADV_CLOSURES=1 \
///   TELEMETRY_ENDPOINT=http://localhost/dummy cargo test \
///   --manifest-path src-tauri/Cargo.toml -p stalagmight-core \
///   --lib adversarial_sweep -- --ignored --nocapture
/// ```
///
/// `STALAGMIGHT_RUST_ADV_FIRST_SEED=n` starts the seed window at `n` (default 1).
#[test]
#[ignore = "sweep: prints per-seed status, used to choose the adversarial GATE_SEEDS"]
fn adversarial_sweep() {
    let _config = isolated_config();
    let seeds: u32 = std::env::var("STALAGMIGHT_RUST_ADV_SEEDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(60);
    let moves: usize = std::env::var("STALAGMIGHT_RUST_ADV_MOVES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(200);
    let closures = std::env::var("STALAGMIGHT_RUST_ADV_CLOSURES").is_ok();

    let mut dirty = Vec::new();
    let mut kinds: BTreeMap<&'static str, usize> = BTreeMap::new();
    let (mut attempted, mut rejected, mut accepted) = (0, 0, 0);
    for seed in crate::test_helpers::sweep_seeds("STALAGMIGHT_RUST_ADV_FIRST_SEED", seeds) {
        let outcome = run_adversarial(seed, moves, closures);
        attempted += outcome.attempted;
        rejected += outcome.rejected;
        accepted += outcome.accepted;
        if !outcome.problems.is_empty() {
            dirty.push(seed);
            for problem in &outcome.problems {
                let kind = if problem.contains("PANICKED") {
                    "panic"
                } else if problem.contains("ACCEPTED") {
                    "accepted-invalid"
                } else {
                    "rejected-not-a-no-op"
                };
                *kinds.entry(kind).or_insert(0) += 1;
                println!("{problem}");
            }
        }
    }
    println!(
        "adversarial sweep: {seeds} seed(s) x {moves} moves, closures={closures}: {attempted} \
         call(s), {rejected} rejected, {accepted} accepted; failures by kind: {kinds:?}"
    );
    println!("seeds with a failure ({}): {dirty:?}", dirty.len());
}
