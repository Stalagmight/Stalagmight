//! ADVERSARIAL arguments: what the command layer does with input it should
//! refuse.
//!
//! # The hole this fills
//!
//! Every generator in this programme is valid-by-construction. The roundtrip
//! fuzzer picks a shot that exists, in a section that exists, and gives it a
//! length in `[0.5, 20.0]`; the metamorphic engine and the format properties
//! inherit that. So the whole of `AppState`'s VALIDATION - which is most of
//! what the command layer is - has never been fuzzed by any of them. The
//! `execute_*` entry points are the sharpest edge: they are hand-written, they
//! do their own argument checking, and several of them do it after taking the
//! project lock and capturing a snapshot.
//!
//! # Two invariants, and the second is the point
//!
//! **A rejected command is a PERFECT no-op.** The roundtrip fuzzer already
//! asserts a rejected command leaves the stacks alone; that is the weaker half.
//! Here every oracle is compared: the canonical project, the app's own
//! integrity verdict, the adjusted geometry, the id allocators and both stacks.
//! A command that mutates and THEN fails is the worst shape a validation bug
//! can take, because the failure tells the user nothing happened.
//!
//! **NO COMMAND EVER PANICS.** Every adversarial move runs inside
//! `catch_unwind`. An index out of range, an arithmetic overflow, an
//! `unwrap` on a `None`, a `debug_assert` - any of them takes the whole app
//! down in a build where they are enabled, and a fuzzer that only checks
//! results would report the run as "the command failed" and move on.
//!
//! No panic site is exempt. Three were classified rather than re-reported while
//! their findings were open - `Project::debug_assert_merge_alignment` firing
//! after an insert or a removal (issue #413's finding 14 and its mirror), the
//! same assert from any other site (finding 20), and an arithmetic overflow in
//! `models/shot.rs` (finding 24). All three are fixed and the classifier is
//! deleted, so ANY panic, from ANY site, is a reportable problem again.
//!
//! # What "adversarial" means here
//!
//! This tier constructs `Command` values directly, so it tests what the TYPE
//! admits rather than what the IPC deserializer would accept - which is the
//! right boundary for it, and a wider one: `f64` fields admit NaN and infinity
//! that a JSON payload could also carry, `u32` station indices admit values far
//! past any section's length, and `String` fields admit 64 kB of NULs. The IPC
//! layer's own validation is the harness's job.
//!
//! # Data
//!
//! Everything is generated in code. No fixture is read, copied or derived from.

#![expect(
    clippy::indexing_slicing,
    reason = "test module: see docs/linting.md for the per-test-module expect idiom"
)]
#![expect(
    clippy::print_stdout,
    reason = "a fuzzer's per-seed summary IS its output, and `cargo test -- --nocapture` is the               only channel a test has"
)]

use std::collections::BTreeMap;

use super::fuzz_undo_tests::{
    adjusted_diffs, adjusted_geometry_of, build_project, canonical_snapshot, describe_diffs,
    diff_snapshots, integrity_verdict, random_shot, read_targets, Rng, COLORS, GEOMETRY_EXTREME,
};
use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::{
    ClosureStrategy, Coordinates, GpsCoordinates, SectionId, SectionUpdates, ShotData, ShotId,
    ShotKind, ShotUpdates, StationRef,
};
// CONFIG ISOLATION. Every test in this module opens with
// `let _config = isolated_config();`: `AppState::new()` LOADS the global config,
// and every save reaches `note_recent_project` -> `Config::save()`, which WRITES
// it. Config resolution is per-thread (`crate::test_helpers` documents what that
// does and does not protect, and why it is not opt-in), so the guard is no longer
// what keeps these tests off other files; what it buys is ONE dir for the whole
// test, whose path the test can name and hand to a worker thread
// (`ConfigHandoff`). It is held for the whole test because the saves are.
use crate::test_helpers::isolated_config;

// ===========================================================================
// Hostile values
// ===========================================================================

/// A section id that is not a section: past the allocator, negative-shaped,
/// and the two extremes the type admits.
fn hostile_section_id(rng: &mut Rng, live: &[SectionId]) -> SectionId {
    match rng.below(6) {
        0 => SectionId::MAX,
        1 => SectionId::MIN,
        2 => 999_999,
        3 => live.last().map_or(0, |id| id.wrapping_add(1)),
        4 => 0,
        _ => live.first().map_or(0, |id| id.wrapping_sub(1)),
    }
}

/// A shot id that is not a shot in the section it is handed to.
fn hostile_shot_id(rng: &mut Rng, live: &[ShotId]) -> ShotId {
    match rng.below(6) {
        0 => ShotId::MAX,
        1 => ShotId::MIN,
        2 => 0,
        3 => live.last().map_or(0, |id| id.wrapping_add(1)),
        4 => live.first().map_or(0, |id| id.wrapping_sub(1)),
        // The stride is what makes a shot id look plausible: the loader
        // renumbers to `section_id * FEATURE_ID_STRIDE + position`, so an id one
        // stride away addresses "the same position in the next section" and is
        // the shape findings 8/12/13 are about.
        _ => live
            .first()
            .map_or(0, |id| id.wrapping_add(crate::models::FEATURE_ID_STRIDE)),
    }
}

/// A station index at or past a boundary. `u32`, so no negative case exists -
/// which is itself worth knowing, since a caller computing `index - 1` wraps.
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

/// A string the type admits: empty, enormous, control characters, and a NUL -
/// which is the one that reaches SQLite's C string boundary.
fn hostile_string(rng: &mut Rng) -> String {
    match rng.below(7) {
        0 => String::new(),
        1 => "\0".to_string(),
        2 => format!("before\0after-{}", rng.range(0, 99)),
        3 => "\u{1}\u{2}\u{7f}\r\n\t".to_string(),
        // 64 kB: past any column width anyone sized by eye.
        4 => "A".repeat(65_536),
        5 => "\u{fffd}\u{202e}\u{200b}".repeat(64),
        _ => "'; DROP TABLE shots; --".to_string(),
    }
}

/// A shot whose every field is drawn from the hostile pool.
fn hostile_shot(rng: &mut Rng) -> ShotData {
    ShotData {
        credit: None,
        // Deliberately absent, not hostile: the command doors strip a
        // client-named identity anyway (#423), so a hostile uuid exercises
        // nothing this suite's oracles can see.
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
        marker_index: rng.chance(50).then(|| rng.range(0, 9) as u32),
        temperature: rng.chance(50).then(|| hostile_float(rng)),
    }
}

// ===========================================================================
// The oracles
// ===========================================================================

/// Everything a rejected command has to leave exactly as it found it.
struct Observed {
    stored: String,
    verdict: String,
    adjusted: String,
    allocators: BTreeMap<String, u32>,
    undo_len: usize,
    redo_len: usize,
}

fn observe(state: &AppState) -> Observed {
    let mut allocators = BTreeMap::new();
    if let Some(project) = state.get_project_state() {
        allocators.insert("next_section_id".to_string(), project.next_section_id);
        allocators.insert(
            "next_closure_id".to_string(),
            u32::try_from(project.next_closure_id).unwrap_or(u32::MAX),
        );
        for section in &project.sections {
            allocators.insert(
                format!("section {}.next_shot_id", section.id),
                section.next_shot_id,
            );
        }
    }
    Observed {
        stored: canonical_snapshot(state, false),
        verdict: integrity_verdict(state).describe(),
        adjusted: adjusted_geometry_of(state),
        allocators,
        undo_len: state.get_undo_stack().len(),
        redo_len: state.get_redo_stack().len(),
    }
}

/// Every way a REJECTED command failed to be a no-op.
fn not_a_no_op(before: &Observed, after: &Observed) -> Vec<String> {
    let mut out = Vec::new();
    let diffs = diff_snapshots(&before.stored, &after.stored);
    if !diffs.is_empty() {
        out.push(format!(
            "it changed the project in {} path(s):\n{}",
            diffs.len(),
            describe_diffs(&diffs)
        ));
    }
    if before.verdict != after.verdict {
        out.push(format!(
            "it changed the integrity verdict:\n    before: {}\n    after:  {}",
            before.verdict, after.verdict
        ));
    }
    let geometry = adjusted_diffs(&before.adjusted, &after.adjusted);
    if !geometry.is_empty() {
        out.push(format!(
            "it moved the ADJUSTED geometry:\n    {}",
            geometry.join("\n    ")
        ));
    }
    if before.allocators != after.allocators {
        out.push(format!(
            "it advanced an id ALLOCATOR, so an id is now burned:\n    before: {:?}\n    after:  {:?}",
            before.allocators, after.allocators
        ));
    }
    if (before.undo_len, before.redo_len) != (after.undo_len, after.redo_len) {
        out.push(format!(
            "it changed the history stacks: undo {} -> {}, redo {} -> {}",
            before.undo_len, after.undo_len, before.redo_len, after.redo_len
        ));
    }
    out
}

// ===========================================================================
// The moves
// ===========================================================================

/// One adversarial call, with the label a failure payload needs to replay it.
struct Move {
    label: String,
    result: CommandResult,
}

/// Draw and run one hostile call against `state`.
///
/// Weighted towards the hand-written `execute_*` entry points, which do their
/// own argument checking, several of them after taking the project lock and
/// capturing a snapshot - so a validation gap there is a mutation that has
/// already happened by the time the caller is told no.
fn draw(state: &AppState, rng: &mut Rng) -> Option<Move> {
    let targets = read_targets(state);
    let sections: Vec<SectionId> = targets.sections.iter().map(|(id, _)| *id).collect();
    if sections.is_empty() {
        return None;
    }
    let live_section = sections[rng.below(sections.len())];
    let live_shots = targets
        .sections
        .iter()
        .find(|(id, _)| *id == live_section)
        .map(|(_, shots)| shots.clone())
        .unwrap_or_default();
    // Half the draws aim a hostile id at a real section, half aim any id at a
    // hostile section: the two reach different guards.
    let section_id = if rng.chance(50) {
        live_section
    } else {
        hostile_section_id(rng, &sections)
    };
    let shot_id = if rng.chance(35) && !live_shots.is_empty() {
        live_shots[rng.below(live_shots.len())]
    } else {
        hostile_shot_id(rng, &live_shots)
    };
    let index = hostile_station_index(rng, live_shots.len() as u32);

    let call = match rng.below(16) {
        // ---- the hand-written entry points, which is where the risk is ----
        0 => Move {
            label: format!("execute_delete_shot_range_shift(s={section_id}, {shot_id}..{shot_id})"),
            result: state.execute_delete_shot_range_shift(section_id, shot_id, shot_id),
        },
        1 => {
            let other = hostile_shot_id(rng, &live_shots);
            Move {
                label: format!(
                    "execute_delete_shot_range_shift(s={section_id}, {shot_id}..{other})"
                ),
                result: state.execute_delete_shot_range_shift(section_id, shot_id, other),
            }
        }
        2 => {
            let other = hostile_shot_id(rng, &live_shots);
            Move {
                label: format!(
                    "execute_delete_shot_range_split(s={section_id}, {shot_id}..{other})"
                ),
                result: state.execute_delete_shot_range_split(
                    section_id,
                    shot_id,
                    other,
                    hostile_string(rng),
                    hostile_string(rng),
                ),
            }
        }
        3 => Move {
            label: format!("execute_delete_shot_and_split(s={section_id}, shot={shot_id})"),
            result: state.execute_delete_shot_and_split(
                section_id,
                shot_id,
                hostile_string(rng),
                hostile_string(rng),
            ),
        },
        4 => Move {
            label: format!("execute_delete_last_shot_and_section(s={section_id}, shot={shot_id})"),
            result: state.execute_delete_last_shot_and_section(section_id, shot_id),
        },
        5 => {
            let distance = hostile_float(rng);
            Move {
                label: format!(
                    "execute_split_shot_at_distance(s={section_id}, shot={shot_id}, d={distance})"
                ),
                result: state.execute_split_shot_at_distance(section_id, shot_id, distance),
            }
        }
        // ---- the ordinary command surface ----
        6 => Move {
            label: format!("CreateShot(s={section_id}, hostile shot)"),
            result: state.execute(Command::CreateShot {
                section_id,
                shot: hostile_shot(rng),
                after_shot_id: rng.chance(50).then_some(shot_id),
                before_shot_id: rng.chance(25).then_some(shot_id),
            }),
        },
        7 => Move {
            label: format!("UpdateShot(s={section_id}, shot={shot_id}, hostile updates)"),
            result: state.execute(Command::UpdateShot {
                section_id,
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
        8 => Move {
            label: format!("CreateSection(hostile metadata, {} hostile shot(s))", 1),
            result: state.execute(Command::CreateSection {
                name: rng.chance(60).then(|| hostile_string(rng)),
                surveyor: rng.chance(50).then(|| hostile_string(rng)),
                // `date` is a `NaiveDate`, not a string - so the hostile
                // values here are the calendar's own edges rather than the
                // string pool's.
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
            }),
        },
        9 => Move {
            label: format!("UpdateSectionMetadata(s={section_id}, hostile)"),
            result: state.execute(Command::UpdateSectionMetadata {
                section_id,
                updates: SectionUpdates {
                    name: rng
                        .chance(50)
                        .then(|| rng.chance(70).then(|| hostile_string(rng))),
                    surveyor: rng
                        .chance(40)
                        .then(|| rng.chance(70).then(|| hostile_string(rng))),
                    color: rng.chance(40).then(|| hostile_string(rng)),
                    quality: rng.chance(40).then(|| hostile_float(rng)),
                    ..SectionUpdates::default()
                },
            }),
        },
        10 => Move {
            label: format!("MoveSection(s={section_id}, hostile offset)"),
            result: state.execute(Command::MoveSection {
                section_id,
                offset: Coordinates::new(
                    hostile_float(rng),
                    hostile_float(rng),
                    hostile_float(rng),
                ),
            }),
        },
        11 => Move {
            label: format!("MoveStation({section_id}/{index}, hostile offset)"),
            result: state.execute(Command::MoveStation {
                station_ref: StationRef::new(section_id, index),
                offset: Coordinates::new(
                    hostile_float(rng),
                    hostile_float(rng),
                    hostile_float(rng),
                ),
            }),
        },
        12 => {
            let other = hostile_section_id(rng, &sections);
            Move {
                label: format!("MergeStations({section_id}/{index}, {other}/{index})"),
                result: state.execute(Command::MergeStations {
                    station_a: StationRef::new(section_id, index),
                    station_b: StationRef::new(other, index),
                }),
            }
        }
        13 => {
            let other = hostile_section_id(rng, &sections);
            Move {
                label: format!("CreateLoopClosure({section_id}/{index}, {other}/{index})"),
                result: state.execute(Command::CreateLoopClosure {
                    station_a: StationRef::new(section_id, index),
                    station_b: StationRef::new(other, index),
                    closure_strategy: *rng.pick(&[
                        ClosureStrategy::Move,
                        ClosureStrategy::LeastSquaresWeighted,
                        ClosureStrategy::ProportionalByLength,
                    ]),
                }),
            }
        }
        14 => {
            // `GpsCoordinates::new` validates, so an out-of-range pair yields no
            // command at all - which is the right answer and worth confirming
            // rather than working around.
            let gps =
                GpsCoordinates::new(hostile_float(rng), hostile_float(rng), hostile_float(rng));
            match gps {
                Ok(coordinates) => Move {
                    label: format!("SetZeroDatum({section_id}/{index}, hostile gps)"),
                    result: state.execute(Command::SetZeroDatum {
                        station_ref: StationRef::new(section_id, index),
                        coordinates,
                    }),
                },
                Err(_) => Move {
                    label: format!("SetStationComment({section_id}/{index}, hostile)"),
                    result: state.execute(Command::SetStationComment {
                        station_ref: StationRef::new(section_id, index),
                        comment: rng.chance(70).then(|| hostile_string(rng)),
                    }),
                },
            }
        }
        _ => Move {
            label: format!("SetShotKind(s={section_id}, shot={shot_id})"),
            result: state.execute(Command::SetShotKind {
                section_id,
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
    hits: BTreeMap<&'static str, usize>,
}

// There WAS a `known_panic` classifier here, and it carried three ids. All three
// are FIXED, so per the delete-when-fixed contract it is gone and EVERY panic
// this engine sees is a reportable problem again:
//
// * finding 24 (`shot_display_position` overflowing before validation), matched
//   by panic LOCATION - the function saturates now, so an overflow panic in
//   models/shot.rs or anywhere else is a new finding;
// * finding 14 (`insert_shot_after`) and its `remove_shot_with_rewire` mirror,
//   matched by the merge-alignment assert's context string. Both sites now
//   decide which side of a structural edit is anchored over the rigid merge
//   forest (`Project::anchored_sides_of_edit`, models/project/shots.rs);
// * finding 20, matched by that same assert from any of the other six sites -
//   the drift an unanchored shot edit left behind, tripped later by an
//   unrelated command. The edit realigns its own group now, and the self-heal
//   gate is the assert's own threshold, so there is no drift left to arm it.
//
// `Project::debug_assert_merge_alignment` has a `#[cfg(test)]` arm, so it exists
// in this tier and no other - which is exactly why this engine must not swallow
// it. `Outcome::hits` stays, empty, as the channel a future classified finding
// would use; the replay comparison over it is then trivially true, which is the
// correct statement when nothing is classified.

// Where the last panic came from, captured by a hook.
//
// A `catch_unwind` payload carries the MESSAGE and not the site, and the site is
// what makes a classifier narrow enough to be honest - so it is recorded here on
// the way past. Thread-local, because libtest runs tests in parallel and a
// shared cell would attribute one test's panic to another's.
thread_local! {
    static LAST_PANIC_SITE: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// Install a panic hook that records the site, ONCE, and DELEGATES.
///
/// Both halves matter. `set_hook` is process-global, so a hook installed per
/// run would be re-wrapped hundreds of times; `Once` installs exactly one.
/// And it calls the previous hook rather than swallowing the panic, because
/// silencing it is global too - every other test in the binary would lose its
/// panic message, which is a high price for tidier output in this one. libtest
/// captures per-test output anyway, so the noise only reaches a reader when a
/// test actually fails.
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
    // A little valid history first, so a rejected command has something real to
    // fail to disturb - and so the stacks are not trivially empty.
    for _ in 0..4 {
        let targets = read_targets(&state);
        if let Some((section_id, _)) = targets.sections.first() {
            state.execute(Command::CreateShot {
                section_id: *section_id,
                shot: random_shot(&mut rng),
                after_shot_id: None,
                before_shot_id: None,
            });
        }
    }

    for step in 0..moves {
        let before = observe(&state);
        let drawn =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| draw(&state, &mut rng)));
        let drawn = match drawn {
            Ok(value) => value,
            Err(payload) => {
                let message = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| {
                        payload
                            .downcast_ref::<&str>()
                            .map(|text| (*text).to_string())
                    })
                    .unwrap_or_else(|| "<non-string panic>".to_string());
                let site = last_panic_site();
                // No panic is tolerated here any more - see the note where
                // `known_panic` used to be.
                outcome.problems.push(format!(
                    "seed {seed} step {step}: a command PANICKED at {}\n  {}",
                    site.unwrap_or_else(|| "<unknown site>".to_string()),
                    message.lines().take(3).collect::<Vec<_>>().join("\n  ")
                ));
                // The run STOPS: the unwind went through the project mutex, so
                // it is poisoned and nothing after this measures the app rather
                // than a crippled `AppState`.
                break;
            }
        };
        let Some(call) = drawn else { continue };
        outcome.attempted += 1;
        if call.result.success {
            outcome.accepted += 1;
            continue;
        }
        outcome.rejected += 1;
        let after = observe(&state);
        let differences = not_a_no_op(&before, &after);
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
/// Two assertions, and a third that keeps them honest: the run has to have
/// REJECTED something. A generator that accidentally became valid-by-
/// construction would pass both invariants vacuously, and that is exactly the
/// way this test would rot.
#[test]
fn adversarial_gate() {
    let _config = isolated_config();
    let mut problems = Vec::new();
    let mut total: BTreeMap<&'static str, usize> = BTreeMap::new();
    let (mut attempted, mut rejected) = (0, 0);
    for &seed in GATE_SEEDS {
        let outcome = run_adversarial(seed, GATE_MOVES, false);
        attempted += outcome.attempted;
        rejected += outcome.rejected;
        problems.extend(outcome.problems);
        for (id, count) in outcome.hits {
            *total.entry(id).or_insert(0) += count;
        }
    }
    println!("adversarial: {attempted} call(s), {rejected} rejected; classified: {total:?}");
    assert!(
        rejected > attempted / 10,
        "only {rejected} of {attempted} adversarial calls were rejected - the hostile generators \
         are producing valid input and the invariants below are passing vacuously"
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
                first.problems.len(),
                first.hits
            ),
            (
                second.attempted,
                second.rejected,
                second.accepted,
                second.problems.len(),
                second.hits
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

    let mut total: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut dirty = Vec::new();
    let (mut attempted, mut rejected, mut accepted) = (0, 0, 0);
    for seed in 1..=seeds {
        let outcome = run_adversarial(seed, moves, closures);
        attempted += outcome.attempted;
        rejected += outcome.rejected;
        accepted += outcome.accepted;
        for (id, count) in outcome.hits {
            *total.entry(id).or_insert(0) += count;
        }
        if !outcome.problems.is_empty() {
            dirty.push(seed);
            for problem in &outcome.problems {
                println!("{problem}");
            }
        }
    }
    println!(
        "adversarial sweep: {seeds} seed(s) x {moves} moves, closures={closures}: {attempted} \
         call(s), {rejected} rejected, {accepted} accepted; classified: {total:?}"
    );
    println!("seeds with a failure ({}): {dirty:?}", dirty.len());
}
