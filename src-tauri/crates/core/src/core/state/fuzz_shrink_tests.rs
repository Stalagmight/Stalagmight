//! SIGNATURE-MATCHED delta debugging for the Rust-tier undo/redo fuzzer.
//!
//! `fuzz_undo_tests.rs` finds failures; this minimizes them. It replays
//! SUBSEQUENCES of a recorded move stream through the same engine, keeps the
//! ones that still fail THE SAME WAY, and prints what is left.
//!
//! # The three pieces
//!
//! | piece | what it is |
//! |---|---|
//! | the TRACE | the fuzzer's move stream, recorded as values ([`TraceMove`]) |
//! | the SIGNATURE | what "the same failure" means (`failure_signature`), so a shrink step cannot wander onto a different bug |
//! | ddmin | remove a chunk, replay, keep it if the signature survives; halve the chunk when nothing can go |
//!
//! # What a signature is
//!
//! A failing run PANICS with the fuzzer's full payload. The signature is its
//! headline with every run of digits collapsed to `#`, plus an app panic's
//! own operation (the discriminator), plus the first differing member of the
//! v2 text, collapsed the same way - so "did not restore the state recorded
//! at depth 23 | at lines.shot" is one bucket at every depth. A step that
//! produces a DIFFERENT signature, or no failure at all, is a NON-MATCH and is
//! rejected: without that rule a shrinker drifts onto a shallower bug.
//!
//! RETIRED with the old engine: the CLASSIFIED target kind (a known open bug
//! counted in `Outcome::hits`). No classifier exists any more - every oracle
//! is bit-exact and every finding fails the run - so a target is always a
//! panic signature. A trace file written with a classified target no longer
//! parses; nothing on disk has one (the classifier table was empty when the
//! old engine was retired).
//!
//! # How an inapplicable move is handled
//!
//! Removing a move can orphan the ones after it. The trace names entities the
//! way that survives this - lines by id, shots by `Shot::uuid`, stations by
//! `StationId`, closures by id - and `Refs` maps the recorded name onto the
//! live one, learning each pairing from what a move CREATED. A name it cannot
//! map makes the move INAPPLICABLE: skipped with a note, never fatal. A
//! history move is clamped to what is reachable.
//!
//! # The ddmin schedule
//!
//! Complement-only ddmin: split into n chunks, probe every complement (in
//! parallel, `run_seed_jobs`), take the FIRST that reproduces in index order
//! (so the parallel and the one-worker shrink return the same minimum), else
//! refine; finally probe the empty trace.
//!
//! # Knobs
//!
//! Prefix `STALAGMIGHT_RUST_FUZZ_` on all of them.
//!
//! | env | what it does |
//! |---|---|
//! | `SHRINK=<seed>` | record that seed under the plan below, then shrink it |
//! | `SHRINK=<path.json>` | shrink a trace file a previous run wrote |
//! | `SHRINK_TARGET=<signature>` | the signature to shrink FOR; default is the failure the run produced |
//! | `SHRINK_OUT=<path>` | where to write the minimized trace; default is a temp path, printed |
//! | `SHRINK_PROBES=<n>` | probe budget, default 2000 |
//! | `REPLAY=<path.json>` | replay a trace file and assert it still reproduces its recorded target |
//! | `ITERATIONS` `PERSISTENCE` `JUMPS` `CLOSURES` `DIVERGENCE` `CLOSURE_SHAPES` `CROSSINGS` `HORIZON` `CONFIG` `COMBINED` `LEGACY_ORIGIN` and the coverage knobs | the plan, exactly as `fuzz_seed_survey` reads them |
//! | `THREADS` | probes per round in parallel; results do not depend on it |
//!
//! ```text
//! STALAGMIGHT_RUST_FUZZ_SHRINK=1 STALAGMIGHT_RUST_FUZZ_JUMPS=1 \
//!   STALAGMIGHT_RUST_FUZZ_CLOSURES=1 STALAGMIGHT_RUST_FUZZ_ITERATIONS=200 \
//!   TELEMETRY_ENDPOINT=http://localhost/dummy cargo test \
//!   --manifest-path src-tauri/Cargo.toml -p stalagmight-core \
//!   --lib fuzz_shrink -- --ignored --nocapture
//! ```
//!
//! It does not shrink the PLAN nor WITHIN a move (the data a move carries is
//! replayed verbatim).

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test module: see docs/linting.md for the per-test-module expect idiom"
)]
#![expect(
    clippy::print_stdout,
    reason = "a shrinker's output IS the repro: the minimized move list is what a reader acts on, \
              and `cargo test -- --nocapture` is the only channel a test has"
)]
#![expect(
    clippy::panic,
    reason = "a shrink that cannot reproduce its own input has to say so loudly, and a panic IS \
              how a test fails"
)]

use std::path::PathBuf;

use super::fuzz_undo_tests::{
    env_flag, env_number, failure_signature, fuzz_threads, panic_message, recorder_arm,
    recorder_take, run_fuzz, run_seed_jobs, run_trace, summarize, Fault, Outcome, Plan, Trace,
    TraceMove, GATE_ITERATIONS,
};
use crate::test_helpers::isolated_config;

// ===========================================================================
// What "the same failure" means
// ===========================================================================

/// The failure a shrink step has to keep reproducing: a panic signature.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(super) enum Target {
    Panic(String),
}

impl Target {
    pub(super) fn describe(&self) -> String {
        match self {
            Self::Panic(signature) => format!("panic {signature}"),
        }
    }
}

/// What one probe did: the run's own verdict, or the panic that stopped it.
type Probe = Result<Outcome, String>;
type ProbeRef<'a> = Result<&'a Outcome, &'a str>;

fn borrow(probe: &Probe) -> ProbeRef<'_> {
    probe.as_ref().map_err(String::as_str)
}

/// Does this probe reproduce the target failure? A completed run is no
/// failure at all.
fn reproduces(target: &Target, probe: ProbeRef<'_>) -> bool {
    match (target, probe) {
        (Target::Panic(signature), Err(message)) => &failure_signature(message) == signature,
        (Target::Panic(_), Ok(_)) => false,
    }
}

fn signature_of(probe: ProbeRef<'_>) -> String {
    match probe {
        Ok(_) => "<no failure>".to_string(),
        Err(message) => failure_signature(message),
    }
}

fn target_of(probe: ProbeRef<'_>) -> Option<Target> {
    match probe {
        Err(message) => Some(Target::Panic(failure_signature(message))),
        Ok(_) => None,
    }
}

// ===========================================================================
// Recording a failing run
// ===========================================================================

pub(super) struct Recorded {
    pub(super) trace: Trace,
    pub(super) probe: Probe,
}

/// Run `(seed, plan)` with the recorder armed, and keep the trace whether the
/// run returned or panicked.
pub(super) fn record_run(seed: u32, plan: Plan) -> Recorded {
    recorder_arm();
    let probe = quietly(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_fuzz(seed, plan)))
            .map_err(|payload| panic_message(&*payload))
    });
    let (prelude, moves) = recorder_take().expect("the recorder was armed for this run");
    Recorded {
        trace: Trace {
            seed,
            plan,
            prelude,
            moves,
            fault: None,
        },
        probe,
    }
}

/// Is a human driving the shrinker right now? Only then is the
/// process-global panic hook silenced (a hook installed during an ordinary
/// `cargo test` would blind every other test's panic reporting).
fn driven_deliberately() -> bool {
    text("SHRINK").is_some() || text("REPLAY").is_some()
}

fn quietly<T>(body: impl FnOnce() -> T) -> T {
    if !driven_deliberately() {
        return body();
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    std::panic::set_hook(previous);
    match out {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

// ===========================================================================
// ddmin
// ===========================================================================

pub(super) struct Shrunk {
    pub(super) moves: Vec<TraceMove>,
    pub(super) original: usize,
    pub(super) probes: usize,
    pub(super) rounds: usize,
    pub(super) exhausted: bool,
}

const DEFAULT_PROBE_BUDGET: usize = 2000;

fn probe(trace: &Trace, moves: &[TraceMove]) -> Probe {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_trace(trace, moves).0))
        .map_err(|payload| panic_message(&*payload))
}

/// Minimize `trace`'s move stream to a subsequence that still reproduces
/// `target`.
pub(super) fn shrink(trace: &Trace, target: &Target, budget: usize, verbose: bool) -> Shrunk {
    let original = trace.moves.len();
    let mut best = trace.moves.clone();
    let mut granularity = 2;
    let mut probes = 0;
    let mut rounds = 0;
    let mut exhausted = false;
    while best.len() >= 2 {
        if probes >= budget {
            exhausted = true;
            break;
        }
        rounds += 1;
        let chunk = best.len().div_ceil(granularity);
        let candidates: Vec<Vec<TraceMove>> = (0..best.len())
            .step_by(chunk)
            .map(|start| {
                let end = (start + chunk).min(best.len());
                let mut candidate = Vec::with_capacity(best.len() - (end - start));
                candidate.extend_from_slice(&best[..start]);
                candidate.extend_from_slice(&best[end..]);
                candidate
            })
            .collect();
        let results = quietly(|| {
            run_seed_jobs(&candidates, fuzz_threads(candidates.len()), |candidate| {
                probe(trace, candidate)
            })
        });
        probes += candidates.len();
        let accepted = results.iter().position(|result| {
            let flattened: ProbeRef<'_> = match result {
                Ok(Ok(outcome)) => Ok(outcome),
                Ok(Err(message)) | Err(message) => Err(message.as_str()),
            };
            reproduces(target, flattened)
        });
        if let Some(index) = accepted {
            let before = best.len();
            best = candidates[index].clone();
            if verbose {
                println!(
                    "  ddmin round {rounds}: granularity {granularity}, {} probe(s), \
                     {before} -> {} move(s) [probes {probes}]",
                    candidates.len(),
                    best.len()
                );
            }
            granularity = granularity.saturating_sub(1).max(2);
        } else {
            if granularity >= best.len() {
                break;
            }
            granularity = (granularity * 2).min(best.len());
        }
    }
    // "No moves at all" is a real answer worth one probe: the plan's
    // prologues and the closing walk are the whole repro.
    if !best.is_empty() && probes < budget {
        probes += 1;
        if reproduces(target, borrow(&probe(trace, &[]))) {
            best.clear();
        }
    }
    Shrunk {
        moves: best,
        original,
        probes,
        rounds,
        exhausted,
    }
}

/// Record `(seed, plan)`, pick a target, and shrink. `fault` is the
/// shrinker's own injection (`None` for a real hunt): it lives in the
/// replay, so the verdict then comes from replaying the whole trace.
pub(super) fn shrink_seed(
    seed: u32,
    plan: Plan,
    fault: Option<Fault>,
    target: Option<Target>,
    budget: usize,
    verbose: bool,
) -> Option<(Trace, Target, Shrunk)> {
    let recorded = record_run(seed, plan);
    let trace = Trace {
        fault,
        ..recorded.trace
    };
    let verdict: Probe = if fault.is_some() {
        probe(&trace, &trace.moves)
    } else {
        recorded.probe
    };
    let target = target.or_else(|| target_of(borrow(&verdict)))?;
    if !reproduces(&target, borrow(&verdict)) {
        return None;
    }
    let shrunk = shrink(&trace, &target, budget, verbose);
    Some((trace, target, shrunk))
}

// ===========================================================================
// The trace file
// ===========================================================================

/// A trace and the failure it is a repro OF: self-verifying (`fuzz_replay`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct TraceFile {
    pub(super) target: Target,
    pub(super) trace: Trace,
}

fn write_trace(path: &PathBuf, file: &TraceFile) {
    let json = serde_json::to_string_pretty(file).expect("a trace should serialize");
    std::fs::write(path, json)
        .unwrap_or_else(|error| panic!("could not write {}: {error}", path.display()));
}

fn read_trace(path: &PathBuf) -> TraceFile {
    let json = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
    serde_json::from_str(&json)
        .unwrap_or_else(|error| panic!("{} is not a trace file: {error}", path.display()))
}

fn text(name: &str) -> Option<String> {
    std::env::var(format!("STALAGMIGHT_RUST_FUZZ_{name}")).ok()
}

fn describe_plan(plan: Plan) -> String {
    let mut modes = Vec::new();
    if plan.persistence {
        modes.push("persistence".to_string());
    }
    if plan.crossings > 1 {
        modes.push(format!("crossings={}", plan.crossings));
    }
    for (on, name) in [
        (plan.jumps, "jumps"),
        (plan.closures, "closures"),
        (plan.divergence, "divergence"),
        (plan.closure_shapes, "closure-shapes"),
        (plan.horizon, "horizon"),
        (plan.config_fuzz, "drawn-config"),
        (plan.combined, "combined"),
        (plan.legacy_origin, "legacy-origin"),
    ] {
        if on {
            modes.push(name.to_string());
        }
    }
    if plan.knobs.any() {
        modes.push(format!("knobs={}", plan.knobs.describe()));
    }
    format!(
        "{} move(s), {}",
        plan.iterations,
        if modes.is_empty() {
            "in-memory".to_string()
        } else {
            modes.join(" ")
        }
    )
}

fn print_moves(moves: &[TraceMove]) {
    for (index, mv) in moves.iter().enumerate() {
        println!("    {:>3}. {}", index + 1, mv.describe());
    }
}

// ===========================================================================
// The entry points
// ===========================================================================

/// THE SHRINKER. Env-gated and `#[ignore]`d. Prints the minimized move list
/// and writes a replayable trace file whose path is the last line.
#[test]
#[ignore = "shrinker: set STALAGMIGHT_RUST_FUZZ_SHRINK=<seed|trace.json> - see the module header"]
fn fuzz_shrink() {
    let _config = isolated_config();
    let Some(spec) = text("SHRINK") else {
        println!(
            "fuzz_shrink does nothing without STALAGMIGHT_RUST_FUZZ_SHRINK=<seed|trace.json>; \
             see the module header"
        );
        return;
    };
    let budget = env_number("SHRINK_PROBES", DEFAULT_PROBE_BUDGET);
    let asked = text("SHRINK_TARGET").map(Target::Panic);
    let (trace, target) = if let Ok(seed) = spec.parse::<u32>() {
        let plan = Plan::from_env(
            env_number("ITERATIONS", GATE_ITERATIONS),
            env_flag("PERSISTENCE"),
        );
        println!("shrink: seed {seed}, plan {} (recording)", describe_plan(plan));
        let recorded = record_run(seed, plan);
        println!(
            "  the run {} - signature: {}",
            if recorded.probe.is_ok() {
                "completed"
            } else {
                "PANICKED"
            },
            signature_of(borrow(&recorded.probe))
        );
        let target = asked
            .or_else(|| target_of(borrow(&recorded.probe)))
            .unwrap_or_else(|| {
                panic!("seed {seed} reached no failure under this plan, so there is nothing to shrink")
            });
        assert!(
            reproduces(&target, borrow(&recorded.probe)),
            "the recorded run does not reproduce {} (it produced: {})",
            target.describe(),
            signature_of(borrow(&recorded.probe))
        );
        (recorded.trace, target)
    } else {
        let path = PathBuf::from(&spec);
        let file = read_trace(&path);
        println!(
            "shrink: {} ({} move(s))",
            path.display(),
            file.trace.moves.len()
        );
        (file.trace, asked.unwrap_or(file.target))
    };
    println!(
        "  target: {} over {} recorded move(s), probe budget {budget}",
        target.describe(),
        trace.moves.len()
    );
    let shrunk = shrink(&trace, &target, budget, true);
    let (outcome, skipped) = quietly(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_trace(&trace, &shrunk.moves)
        }))
        .map_err(|payload| panic_message(&*payload))
    })
    .map_or_else(
        |message| (Err(message), Vec::new()),
        |(outcome, skipped)| (Ok(outcome), skipped),
    );
    assert!(
        reproduces(&target, borrow(&outcome)),
        "the minimized trace stopped reproducing {} - it produced: {}",
        target.describe(),
        signature_of(borrow(&outcome))
    );
    println!(
        "\n  MINIMIZED: trace {} moves -> {} moves ({} probe(s), {} round(s){})",
        shrunk.original,
        shrunk.moves.len(),
        shrunk.probes,
        shrunk.rounds,
        if shrunk.exhausted {
            ", BUDGET EXHAUSTED"
        } else {
            ""
        }
    );
    println!("  the minimal repro, move by move:");
    print_moves(&shrunk.moves);
    for note in skipped.iter().take(8) {
        println!("    skipped: {note}");
    }
    let out = text("SHRINK_OUT").map_or_else(
        || {
            std::env::temp_dir().join(format!(
                "stalagmight-shrink-seed{}-{}moves.json",
                trace.seed,
                shrunk.moves.len()
            ))
        },
        PathBuf::from,
    );
    write_trace(
        &out,
        &TraceFile {
            target,
            trace: Trace {
                moves: shrunk.moves,
                ..trace
            },
        },
    );
    println!("\n  wrote the minimized trace to: {}", out.display());
}

/// Replay a trace file, and ASSERT it still reproduces the failure it was
/// minimized for.
#[test]
#[ignore = "replay: set STALAGMIGHT_RUST_FUZZ_REPLAY=<trace.json> - see the module header"]
fn fuzz_replay() {
    let _config = isolated_config();
    let Some(path) = text("REPLAY").map(PathBuf::from) else {
        println!("fuzz_replay does nothing without STALAGMIGHT_RUST_FUZZ_REPLAY=<trace.json>");
        return;
    };
    let file = read_trace(&path);
    println!(
        "replay: {} - seed {}, plan {}, {} move(s), target {}",
        path.display(),
        file.trace.seed,
        describe_plan(file.trace.plan),
        file.trace.moves.len(),
        file.target.describe()
    );
    print_moves(&file.trace.moves);
    let moves = file.trace.moves.clone();
    let outcome = quietly(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_trace(&file.trace, &moves).0
        }))
        .map_err(|payload| panic_message(&*payload))
    });
    if let Ok(result) = &outcome {
        println!("  {}", summarize("replay", file.trace.seed, result));
    }
    assert!(
        reproduces(&file.target, borrow(&outcome)),
        "the trace no longer reproduces {} - it produced: {}",
        file.target.describe(),
        signature_of(borrow(&outcome))
    );
    println!("  REPRODUCED: {}", file.target.describe());
}

// ===========================================================================
// The gates
// ===========================================================================

/// Recording must not change what a run does.
#[test]
fn a_recorded_run_matches_an_unrecorded_one() {
    let _config = isolated_config();
    for persistence in [false, true] {
        let plan = if persistence {
            Plan::persisted(GATE_ITERATIONS, 1)
        } else {
            Plan::plain(GATE_ITERATIONS)
        };
        let plain = run_fuzz(3, plan);
        let recorded = record_run(3, plan);
        let taped = recorded.probe.as_ref().expect("seed 3 is a gate seed");
        assert_eq!(
            plain.shape(),
            taped.shape(),
            "recording changed what seed 3 did (persistence={persistence})"
        );
        assert!(!recorded.trace.moves.is_empty());
    }
}

/// A FULL trace replays to the same run it was recorded from: the
/// load-bearing test of the whole module.
#[test]
fn a_full_trace_replays_to_the_same_run() {
    let _config = isolated_config();
    let mut legacy = Plan::persisted(GATE_ITERATIONS, 2);
    legacy.legacy_origin = true;
    for plan in [
        Plan::plain(GATE_ITERATIONS),
        Plan::persisted(GATE_ITERATIONS, 1),
        legacy,
    ] {
        let recorded = record_run(4, plan);
        let original = recorded.probe.as_ref().expect("seed 4 is a gate seed");
        let moves = recorded.trace.moves.clone();
        let (replayed, skipped) = run_trace(&recorded.trace, &moves);
        assert!(
            skipped.is_empty(),
            "replaying a COMPLETE trace skipped {} move(s): {skipped:?}",
            skipped.len()
        );
        assert_eq!(
            original.shape(),
            replayed.shape(),
            "a full trace did not replay to its own run ({})",
            describe_plan(plan)
        );
    }
}

/// The ddmin schedule itself, over a predicate with a KNOWN minimum.
#[test]
fn ddmin_reaches_the_minimum_of_a_known_predicate() {
    let _config = isolated_config();
    let holds = |candidate: &[usize]| candidate.contains(&3) && candidate.contains(&7);
    let mut best: Vec<usize> = (0..24).collect();
    let original = best.len();
    let mut granularity = 2;
    let mut probes = 0;
    while best.len() >= 2 {
        let chunk = best.len().div_ceil(granularity);
        let candidates: Vec<Vec<usize>> = (0..best.len())
            .step_by(chunk)
            .map(|start| {
                let end = (start + chunk).min(best.len());
                let mut candidate = best[..start].to_vec();
                candidate.extend_from_slice(&best[end..]);
                candidate
            })
            .collect();
        probes += candidates.len();
        if let Some(index) = candidates.iter().position(|candidate| holds(candidate)) {
            best = candidates[index].clone();
            granularity = granularity.saturating_sub(1).max(2);
        } else {
            if granularity >= best.len() {
                break;
            }
            granularity = (granularity * 2).min(best.len());
        }
    }
    assert_eq!(best, vec![3, 7], "ddmin did not reach the minimum");
    assert!(
        probes < 200,
        "ddmin took {probes} probes over {original} elements"
    );
}

/// The whole flow over an INJECTED failure (the app has none to offer): the
/// replay fails once the project holds this many lines. The starting project
/// has seven, so a minimum keeps the moves that add two.
const FAULT_AT_LINES: usize = 9;

#[test]
fn the_shrinker_minimizes_an_injected_failure() {
    let _config = isolated_config();
    let fault = Some(Fault {
        at_lines: FAULT_AT_LINES,
    });
    let Some((trace, target, shrunk)) =
        shrink_seed(1, Plan::plain(GATE_ITERATIONS), fault, None, 400, false)
    else {
        panic!("the injected fault did not fire: seed 1 never reaches {FAULT_AT_LINES} lines");
    };
    assert!(
        shrunk.moves.len() < shrunk.original,
        "the shrinker did not shrink: {} -> {} move(s)",
        shrunk.original,
        shrunk.moves.len()
    );
    assert!(
        !shrunk.moves.is_empty(),
        "the empty trace cannot reach {FAULT_AT_LINES} lines from seven"
    );
    assert!(
        reproduces(&target, borrow(&probe(&trace, &shrunk.moves))),
        "the minimized trace stopped reproducing {}",
        target.describe()
    );
    let one_shorter = &shrunk.moves[..shrunk.moves.len() - 1];
    assert!(
        !reproduces(&target, borrow(&probe(&trace, one_shorter))),
        "dropping the last move of the minimum still reproduces {}",
        target.describe()
    );
    println!(
        "shrink gate: {} -> {} move(s) for {} ({} probes)",
        shrunk.original,
        shrunk.moves.len(),
        target.describe(),
        shrunk.probes
    );
}

/// A shrink step that changes the failure is a NON-MATCH, and one defect at
/// two depths is one bucket.
#[test]
fn a_different_signature_is_not_a_match() {
    let _config = isolated_config();
    let panicked: Probe = Err("undo x1 did not restore the state recorded at depth 23\n  \
                               seed=1\n    at lines.shot (line 41):\n      recorded x"
        .to_string());
    let signature = failure_signature(panicked.as_ref().unwrap_err());
    let target = Target::Panic(signature.clone());
    assert!(reproduces(&target, borrow(&panicked)));
    let completed: Probe = Ok(Outcome::default());
    assert!(!reproduces(&target, borrow(&completed)));
    let elsewhere: Probe = Err("undo x1 did not restore the state recorded at depth 23\n  \
                                seed=1\n    at stations (line 12):\n      recorded x"
        .to_string());
    assert!(
        !reproduces(&target, borrow(&elsewhere)),
        "another member is another failure"
    );
    let deeper: Probe = Err("undo x1 did not restore the state recorded at depth 7\n  \
                             seed=9\n    at lines.shot (line 3):\n      recorded y"
        .to_string());
    assert_eq!(
        signature,
        failure_signature(deeper.as_ref().unwrap_err()),
        "two depths of one defect landed in different buckets"
    );
    assert_eq!(
        signature,
        "undo x# did not restore the state recorded at depth # | at lines.shot"
    );
}

/// One app-panic payload, built the way `Fuzzer::fail` builds one after
/// `catching`: a FIXED headline, and the panic's own message under the move.
fn panic_payload(message: &str, depth: usize) -> String {
    format!(
        "the app panicked inside a move\n  seed=162 depth={depth} floor=0 top={depth} saves=0 \
         reloads=0\n  see the last move in the log below for the panic's own message\n  last 2 \
         move(s):\n    mergeStations(4@1,8@3) -> ok (undo {depth}->{depth}, redo 0->0)\n    \
         deleteShotRangeShift(s=4,400000..400002) -> PANICKED (did not return)\n    \
         ^ the app panicked: {message}"
    )
}

/// Two defects behind the ONE app-panic headline are two signatures (the
/// panic's own operation is the discriminator); one defect at another depth
/// and other values is one.
#[test]
fn the_panic_operation_splits_one_headline_into_two_signatures() {
    let _config = isolated_config();
    let range = panic_payload("delete_shot_range: index 7 out of range for 5", 41);
    let combine = panic_payload("combine_lines: line 12 is gone", 41);
    assert_eq!(range.lines().next(), combine.lines().next());
    assert_ne!(failure_signature(&range), failure_signature(&combine));
    assert_eq!(
        failure_signature(&range),
        "the app panicked inside a move | panic=delete_shot_range"
    );
    let target = Target::Panic(failure_signature(&range));
    assert!(!reproduces(&target, borrow(&Err(combine))));
    let elsewhere = panic_payload("delete_shot_range: index 19 out of range for 3", 7);
    assert!(reproduces(&target, borrow(&Err(elsewhere))));
}
