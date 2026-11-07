//! SIGNATURE-MATCHED delta debugging for the Rust-tier undo/redo fuzzer.
//!
//! `fuzz_undo_tests.rs` finds failures; this minimizes them. A 200-move seed
//! that reaches finding 8 is a repro in the sense that it fails, and in no other
//! sense: nobody can read 271 moves and say which three of them are the bug. So
//! this replays SUBSEQUENCES of the recorded move stream through the same
//! engine, keeps the ones that still fail THE SAME WAY, and prints what is left.
//!
//! # The three pieces
//!
//! | piece | what it is |
//! |---|---|
//! | the TRACE | `Fuzzer`'s move stream, recorded as values ([`TraceMove`]) rather than as text |
//! | the SIGNATURE | what "the same failure" means, so a shrink step cannot wander onto a different bug |
//! | ddmin | remove a chunk, replay, keep it if the signature survives; halve the chunk when nothing can go |
//!
//! # What a signature is
//!
//! Two kinds, because this module's failures come in two kinds.
//!
//! * **Classified.** A known open bug does not fail the run - it is counted, by
//!   id, in `Outcome::hits`. The signature is that id, and a candidate reproduces
//!   when the id is in its hits. There are NO classified failures today: every
//!   #413 finding is fixed and `fuzz_undo_tests.rs`'s `CLASSIFIERS` table is
//!   empty, so this arm is here for the next open bug rather than for a current
//!   one - and the gate below shrinks an INJECTED failure instead, see `Fault`.
//! * **Unclassified.** Anything else PANICS, with the fuzzer's full payload. The
//!   signature is `failure_signature` - the headline with every run of digits
//!   collapsed to `#`, plus any DISCRIMINATOR the payload carries below its
//!   headline, plus the first differing snapshot path, collapsed the same way.
//!   That is the same normalization the classifiers use, and it is what keeps
//!   "did not restore the state recorded at depth 23 | at sections[#].shots[#].length"
//!   from splitting into one bucket per depth.
//!
//! The discriminator exists because a headline can name an INVARIANT rather than
//! a defect: "the app's own merge-alignment assert fired" is one headline for
//! every call site that can trip that assert, and issue #464 filed three
//! separate defects behind it. Shrinking the second drifted onto the third. So
//! the signature also carries the assert's own context string - the call site,
//! a `&'static str` literal - and nothing else from that message: see
//! `DISCRIMINATORS` in `fuzz_undo_tests.rs` for why the distance it also reports
//! must stay out.
//!
//! A step that produces a DIFFERENT signature is a NON-MATCH and is rejected,
//! whether it is a different finding, a different panic, or no failure at all.
//! Without that rule a shrinker drifts: the first accepted step lands on a
//! shallower, more common bug and every later step minimizes for THAT, and the
//! minimum it reports is a repro of something nobody asked about.
//!
//! # How an inapplicable move is handled
//!
//! Removing a move can orphan the ones after it: an `updateShot` whose shot was
//! created by the `createShot` that just went away has nothing to address. The
//! trace names entities the way that survives this - sections by id, shots by
//! `Shot::uuid`, closures by id, stations positionally - and [`Refs`] maps the
//! recorded name onto the live one, learning each pairing from what a move
//! CREATED. A name it cannot map makes the move INAPPLICABLE, and an
//! inapplicable move is SKIPPED with a note rather than fatal. ddmin needs
//! replay to be a total function: a candidate that killed the harness would be
//! indistinguishable from one that reproduced the bug.
//!
//! A history move is clamped rather than skipped - `step_history` already takes
//! `steps.min(reach)`, which is the same thing the drawn path does when the
//! stack is shorter than the draw.
//!
//! # The ddmin schedule
//!
//! Classic ddmin, complement-only (Zeller & Hildebrandt's `ddmin`, without the
//! subset phase - the subsets of a move stream are almost never repros on their
//! own, so the phase costs probes and returns nothing):
//!
//! ```text
//! granularity n = 2
//! while len >= 2:
//!     split the trace into n chunks
//!     probe every COMPLEMENT (the trace with one chunk removed)
//!     if one reproduces: take the FIRST such, in index order; n = max(n-1, 2)
//!     else if n >= len: stop      // single moves have been tried, nothing goes
//!     else: n = min(2n, len)      // finer chunks
//! ```
//!
//! The complements of one round are INDEPENDENT, so they run on
//! [`run_seed_jobs`] - the same worker pool the sweeps use - and the accepted
//! one is the first in INDEX order, which is exactly what a sequential loop
//! would have taken. So the parallel shrink and the one-worker shrink return the
//! same minimum, and `STALAGMIGHT_RUST_FUZZ_THREADS=1` is a speed knob rather
//! than a mode.
//!
//! # Knobs
//!
//! Prefix `STALAGMIGHT_RUST_FUZZ_` on all of them.
//!
//! | env | what it does |
//! |---|---|
//! | `SHRINK=<seed>` | record that seed under the plan below, then shrink it |
//! | `SHRINK=<path.json>` | shrink a trace file a previous run wrote |
//! | `SHRINK_TARGET=<id>` | the finding id to shrink FOR; default is the failure the run produced |
//! | `SHRINK_OUT=<path>` | where to write the minimized trace; default is a temp path, printed |
//! | `SHRINK_PROBES=<n>` | probe budget, default 2000 |
//! | `REPLAY=<path.json>` | replay a trace file and assert it still reproduces its recorded target |
//! | `ITERATIONS` `PERSISTENCE` `JUMPS` `CLOSURES` `DIVERGENCE` `CLOSURE_SHAPES` `CROSSINGS` `HORIZON` `CONFIG` `COMBINED` | the plan, exactly as `fuzz_seed_survey` reads them |
//! | `THREADS` | probes per round in parallel; results do not depend on it |
//!
//! `SHRINK_TARGET` takes a classified id, and none exists today - leave it off
//! and the shrinker takes the panic the run produced, which is what a hunt for a
//! NEW defect wants anyway:
//!
//! ```text
//! STALAGMIGHT_RUST_FUZZ_SHRINK=1 STALAGMIGHT_RUST_FUZZ_JUMPS=1 \
//!   STALAGMIGHT_RUST_FUZZ_CLOSURES=1 STALAGMIGHT_RUST_FUZZ_ITERATIONS=200 \
//!   TELEMETRY_ENDPOINT=http://localhost/dummy cargo test \
//!   --manifest-path src-tauri/Cargo.toml -p stalagmight-core \
//!   --lib fuzz_shrink -- --ignored --nocapture
//! ```
//!
//! # What it does NOT do
//!
//! It does not shrink the PLAN (a mode is on or off) and it does not shrink
//! WITHIN a move: the shot data, offsets and updates a move carries are replayed
//! verbatim. Both are worth having and neither is needed to read a repro, which
//! is what the move count buys.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
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

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::fuzz_undo_tests::{
    failure_signature, fuzz_threads, panic_message, recorder_arm, recorder_take, run_fuzz,
    run_seed_jobs, run_trace, summarize, Fault, Outcome, Plan, Trace, TraceMove, GATE_ITERATIONS,
};
// CONFIG ISOLATION. Every test in this module opens with
// `let _config = isolated_config();`: `AppState::new()` LOADS the global config,
// and every save reaches `note_recent_project` -> `Config::save()`, which WRITES
// it. Config resolution is per-thread (`crate::test_helpers` documents what that
// does and does not protect, and why it is not opt-in), so the guard is no longer
// what keeps these tests off other files; what it buys is ONE dir for the whole
// test, whose path the test can name and hand to a worker thread
// (`ConfigHandoff`). It is held for the whole test because the saves are.
use crate::models::test_support::missing_prerequisite;
use crate::test_helpers::isolated_config;

// ===========================================================================
// What "the same failure" means
// ===========================================================================

/// The failure a shrink step has to keep reproducing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(super) enum Target {
    /// A KNOWN open bug, by the id its classifier gives it. A classified hit
    /// does not fail the run - it is counted - so this is a predicate on
    /// `Outcome::hits` rather than on a panic.
    Classified(String),
    /// Anything else: the run panicked, and this is `failure_signature` of the
    /// payload - the headline and the first differing path, with every run of
    /// digits collapsed to `#` so one defect does not split into one bucket per
    /// depth.
    Panic(String),
}

impl Target {
    pub(super) fn describe(&self) -> String {
        match self {
            Self::Classified(id) => format!("classified {id}"),
            Self::Panic(signature) => format!("panic {signature}"),
        }
    }
}

/// What one probe did: the run's own verdict, or the panic that stopped it.
type Probe = Result<Outcome, String>;
/// The same, borrowed - which is what every predicate below takes, because an
/// `Outcome` is not `Clone` and a shrink asks these questions a few hundred
/// times per round.
type ProbeRef<'a> = Result<&'a Outcome, &'a str>;

fn borrow(probe: &Probe) -> ProbeRef<'_> {
    probe.as_ref().map_err(String::as_str)
}

/// Does this probe reproduce the target failure?
fn reproduces(target: &Target, probe: ProbeRef<'_>) -> bool {
    match (target, probe) {
        (Target::Classified(id), Ok(outcome)) => outcome.hits.keys().any(|hit| hit == &id.as_str()),
        (Target::Panic(signature), Err(message)) => &failure_signature(message) == signature,
        // A classified target that PANICKED is a different failure, and a panic
        // target that completed is no failure at all. Both are non-matches, and
        // saying so here rather than at the call sites is what keeps the two
        // kinds of signature one rule.
        (Target::Classified(_), Err(_)) | (Target::Panic(_), Ok(_)) => false,
    }
}

/// What a probe found, named the way a target names it - for the report, and
/// for choosing a target when the caller did not.
fn signature_of(probe: ProbeRef<'_>) -> String {
    match probe {
        Ok(outcome) if outcome.hits.is_empty() => "<no failure>".to_string(),
        Ok(outcome) => outcome
            .hits
            .keys()
            .copied()
            .collect::<Vec<&str>>()
            .join(", "),
        Err(message) => failure_signature(message),
    }
}

/// The target a recorded run suggests: its panic if it panicked, else its
/// alphabetically first classified hit.
fn target_of(probe: ProbeRef<'_>) -> Option<Target> {
    match probe {
        Err(message) => Some(Target::Panic(failure_signature(message))),
        Ok(outcome) => outcome
            .hits
            .keys()
            .next()
            .map(|id| Target::Classified((*id).to_string())),
    }
}

// ===========================================================================
// Recording a failing run
// ===========================================================================

/// A run, its move stream, and how it ended.
pub(super) struct Recorded {
    pub(super) trace: Trace,
    pub(super) probe: Probe,
}

/// Run `(seed, plan)` with the recorder armed, and keep the trace whether the
/// run returned or panicked.
///
/// Recording cannot change what the run does: the recorder only reads state the
/// engine already computes and clones the `Op` the generator already drew. That
/// is checked rather than asserted in prose - `a_recorded_run_matches_an_unrecorded_one`
/// runs the same seed both ways and compares.
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
            // A RECORDED run never carries one: the fault injection is the
            // shrink gate's own, applied to a replay of an already-recorded
            // trace. See `Fault`.
            fault: None,
        },
        probe,
    }
}

/// Is a human driving the shrinker right now?
///
/// The hook silencing below is only allowed then - see [`quietly`].
fn driven_deliberately() -> bool {
    text("SHRINK").is_some() || text("REPLAY").is_some()
}

/// Run `body` with the panic hook silenced - but ONLY when the shrinker is being
/// driven deliberately by its own env knob.
///
/// Silencing is worth a lot here: a shrink is several hundred probes, and for a
/// PANIC target most of them panic ON PURPOSE, each printing the fuzzer's whole
/// payload to stderr until the minimized repro has scrolled away.
///
/// It is also PROCESS-GLOBAL, which is why the knob gates it. `adversarial_tests`
/// installs a hook that records each panic's SITE (a `catch_unwind` payload
/// carries the message and not the location) and classifies finding 24 by it;
/// libtest runs tests in parallel threads in one process, so a hook this module
/// installed during an ordinary `cargo test` would blind that gate for as long
/// as it was up. It did, exactly once, and this is the fix: under `cargo test`
/// the hook is left alone and libtest's own per-test capture keeps the noise out
/// of a passing run's output.
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

/// What a shrink produced.
pub(super) struct Shrunk {
    /// The minimized move stream, in the original order.
    pub(super) moves: Vec<TraceMove>,
    pub(super) original: usize,
    pub(super) probes: usize,
    pub(super) rounds: usize,
    /// True when the probe budget ran out before the schedule finished, so the
    /// result is A smaller repro rather than the smallest this schedule reaches.
    pub(super) exhausted: bool,
}

/// Default probe budget. A probe is a whole mini-run, so this is seconds to a
/// couple of minutes depending on the trace; a shrink that needs more than this
/// is one to run deliberately with a bigger number.
const DEFAULT_PROBE_BUDGET: usize = 2000;

/// Replay one candidate subsequence and report what it did.
fn probe(trace: &Trace, moves: &[TraceMove]) -> Probe {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_trace(trace, moves).0))
        .map_err(|payload| panic_message(&*payload))
}

/// Minimize `trace`'s move stream to a subsequence that still reproduces
/// `target`.
///
/// The schedule is in the module header. The one rule worth repeating here: a
/// candidate is accepted ONLY when its signature matches, so the result is a
/// repro of the failure that was asked for and not of whatever the shrink
/// happened to land on.
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
        // Every complement, in index order: the trace with one chunk removed.
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
        // `run_seed_jobs` catches a panic PER JOB, so a probe that panics comes
        // back wrapped twice - once by the probe's own `catch_unwind` and once
        // by the pool's. Either way the message is the payload, which is what
        // the signature is derived from.
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
            // Back off one level rather than restarting at 2: the chunk that
            // just went is gone, and the ones around it are still worth
            // trying at close to this size.
            granularity = granularity.saturating_sub(1).max(2);
        } else {
            if granularity >= best.len() {
                if verbose {
                    println!(
                        "  ddmin round {rounds}: granularity {granularity} = 1 move per \
                         chunk, {} probe(s), nothing else can go [probes {probes}]",
                        candidates.len()
                    );
                }
                break;
            }
            let finer = (granularity * 2).min(best.len());
            if verbose {
                println!(
                    "  ddmin round {rounds}: granularity {granularity}, {} probe(s), \
                     no chunk could go - refining to {finer} [probes {probes}]",
                    candidates.len()
                );
            }
            granularity = finer;
        }
    }

    // ddmin's loop stops at two moves, so the last one is never offered for
    // removal - and "no moves at all" is a real answer worth one probe. It says
    // the PLAN reproduces the failure: the prologues and the closing
    // unwind/rewind walk are enough, and every recorded move was scaffolding.
    // For a mode that constructs a shape (`divergence`, `closure_shapes`) that
    // is the most useful thing a shrink can report.
    if !best.is_empty() && probes < budget {
        probes += 1;
        if reproduces(target, borrow(&probe(trace, &[]))) {
            best.clear();
            if verbose {
                println!(
                    "  ddmin: the EMPTY trace still reproduces - the plan's prologues and the \
                     closing walk are the whole repro [probes {probes}]"
                );
            }
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

/// Record `(seed, plan)`, pick a target, and shrink - the whole flow as one
/// call, for a hunt that wants a minimum rather than a report.
///
/// `fault` is the shrinker's own test-only injection (see `Fault` in
/// `fuzz_undo_tests.rs`) and is `None` for every real hunt. It cannot be read by
/// `run_fuzz`, so the RECORDED run is unaffected by it and says nothing about the
/// failure being shrunk; when one is armed the verdict therefore comes from
/// replaying the whole trace, which is where the injection lives.
///
/// `None` when the run produced no failure at all, or when `target` names a
/// failure the run does not reach.
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

/// What gets written to disk: a trace, and the failure it is a repro OF.
///
/// The target travels with the moves on purpose - it makes the file
/// self-verifying. `fuzz_replay` replays it and asserts the signature still
/// matches, so "is this repro still a repro" is one command rather than a
/// judgement call about a wall of output.
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

// ===========================================================================
// The plan, from the environment
// ===========================================================================

fn flag(name: &str) -> bool {
    std::env::var(format!("STALAGMIGHT_RUST_FUZZ_{name}")).is_ok()
}

fn number<T: std::str::FromStr>(name: &str, fallback: T) -> T {
    std::env::var(format!("STALAGMIGHT_RUST_FUZZ_{name}"))
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

fn text(name: &str) -> Option<String> {
    std::env::var(format!("STALAGMIGHT_RUST_FUZZ_{name}")).ok()
}

/// The plan the survey's knobs describe, so a seed that failed under a sweep is
/// shrunk under exactly the environment that found it.
fn plan_from_env() -> Plan {
    // `COMBINED=1` turns the modes it IS on, exactly as the survey reads it, so
    // a seed that failed under the combined regime is shrunk under it too.
    let combined = flag("COMBINED");
    Plan {
        iterations: number("ITERATIONS", GATE_ITERATIONS),
        persistence: flag("PERSISTENCE"),
        crossings: number("CROSSINGS", 1),
        prologue: None,
        jumps: flag("JUMPS") || combined,
        closures: flag("CLOSURES") || combined,
        divergence: flag("DIVERGENCE"),
        closure_shapes: flag("CLOSURE_SHAPES") || combined,
        horizon: flag("HORIZON") || combined,
        config_fuzz: flag("CONFIG"),
        combined,
    }
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
    ] {
        if on {
            modes.push(name.to_string());
        }
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

/// Print a minimized trace the way a reader reconstructs it: one line per move,
/// numbered, with the recorded ids.
fn print_moves(moves: &[TraceMove]) {
    for (index, mv) in moves.iter().enumerate() {
        println!("    {:>3}. {}", index + 1, mv.describe());
    }
}

// ===========================================================================
// The entry points
// ===========================================================================

/// THE SHRINKER. Env-gated and `#[ignore]`d, like every other tool in this tier.
///
/// See the module header for the knobs. Prints the minimized move list and
/// writes a replayable trace file whose path is the last line.
#[test]
#[ignore = "shrinker: set STALAGMIGHT_RUST_FUZZ_SHRINK=<seed|trace.json> - see the module header"]
fn fuzz_shrink() {
    let _config = isolated_config();
    let Some(spec) = text("SHRINK") else {
        println!(
            "fuzz_shrink needs STALAGMIGHT_RUST_FUZZ_SHRINK=<seed|trace.json>.\n\
             Example:\n  STALAGMIGHT_RUST_FUZZ_SHRINK=1 STALAGMIGHT_RUST_FUZZ_DIVERGENCE=1 \
             STALAGMIGHT_RUST_FUZZ_ITERATIONS=200 \\\n    \
             STALAGMIGHT_RUST_FUZZ_SHRINK_TARGET=finding-8-stale-id-deletes-the-wrong-shot \\\n    \
             TELEMETRY_ENDPOINT=http://localhost/dummy cargo test --manifest-path \
             src-tauri/Cargo.toml \\\n    -p stalagmight-core --lib fuzz_shrink -- --ignored \
             --nocapture"
        );
        missing_prerequisite("STALAGMIGHT_RUST_FUZZ_SHRINK=<seed|trace.json>");
    };
    let budget = number("SHRINK_PROBES", DEFAULT_PROBE_BUDGET);
    let asked = text("SHRINK_TARGET").map(Target::Classified);

    // A seed is RECORDED first; a path is a trace a previous run wrote.
    let (trace, target) = if let Ok(seed) = spec.parse::<u32>() {
        {
            let plan = plan_from_env();
            println!(
                "shrink: seed {seed}, plan {} (recording)",
                describe_plan(plan)
            );
            let recorded = record_run(seed, plan);
            println!(
                "  the run {} - signature: {}",
                match &recorded.probe {
                    Ok(_) => "completed",
                    Err(_) => "PANICKED",
                },
                signature_of(borrow(&recorded.probe))
            );
            if let Ok(outcome) = &recorded.probe {
                println!("  {}", summarize("recorded", seed, outcome));
            }
            let target = asked
                .or_else(|| target_of(borrow(&recorded.probe)))
                .unwrap_or_else(|| {
                    panic!(
                        "seed {seed} reached no failure under this plan, so there is nothing to \
                         shrink - pick a seed and mode from the bug matrix in fuzz_undo_tests.rs"
                    )
                });
            assert!(
                reproduces(&target, borrow(&recorded.probe)),
                "the recorded run does not reproduce {} (it produced: {}) - shrinking it would \
                 minimize for a failure it never had",
                target.describe(),
                signature_of(borrow(&recorded.probe))
            );
            (recorded.trace, target)
        }
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

    // The minimum, re-run once on its own: this is what confirms the printed
    // repro rather than a candidate that happened to be the last accepted one,
    // and it is where the skipped-move notes come from.
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
    if let Ok(outcome) = &outcome {
        println!("  {}", summarize("minimized", trace.seed, outcome));
    }
    if skipped.is_empty() {
        println!("  every move in the minimum applied");
    } else {
        println!(
            "  {} move(s) were inapplicable and skipped (their target was shrunk away):",
            skipped.len()
        );
        for note in skipped.iter().take(8) {
            println!("    {note}");
        }
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
    let file = TraceFile {
        target,
        trace: Trace {
            seed: trace.seed,
            plan: trace.plan,
            prelude: trace.prelude,
            moves: shrunk.moves,
            fault: trace.fault,
        },
    };
    write_trace(&out, &file);
    println!("\n  wrote the minimized trace to: {}", out.display());
    println!(
        "  replay it with:\n    STALAGMIGHT_RUST_FUZZ_REPLAY={} \
         TELEMETRY_ENDPOINT=http://localhost/dummy cargo test \\\n      --manifest-path \
         src-tauri/Cargo.toml -p stalagmight-core --lib fuzz_replay -- --ignored --nocapture",
        out.display()
    );
}

/// Replay a trace file, and ASSERT it still reproduces the failure it was
/// minimized for.
///
/// `STALAGMIGHT_RUST_FUZZ_TRACE=1` alongside this streams every move as the
/// engine makes it, which is how a minimized repro is read against the app's own
/// behaviour rather than against the trace's own summary.
#[test]
#[ignore = "replay: set STALAGMIGHT_RUST_FUZZ_REPLAY=<trace.json> - see the module header"]
fn fuzz_replay() {
    let _config = isolated_config();
    let Some(path) = text("REPLAY").map(PathBuf::from) else {
        println!(
            "fuzz_replay needs STALAGMIGHT_RUST_FUZZ_REPLAY=<trace.json>, which \
             fuzz_shrink writes."
        );
        missing_prerequisite("STALAGMIGHT_RUST_FUZZ_REPLAY=<trace.json>");
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
    let (outcome, skipped) = quietly(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_trace(&file.trace, &moves)
        }))
        .map_err(|payload| panic_message(&*payload))
    })
    .map_or_else(
        |message| (Err(message), Vec::new()),
        |(outcome, skipped)| (Ok(outcome), skipped),
    );
    match &outcome {
        Ok(result) => println!("  {}", summarize("replay", file.trace.seed, result)),
        Err(message) => println!("  the replay PANICKED:\n{message}"),
    }
    for note in skipped.iter().take(8) {
        println!("  skipped: {note}");
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
///
/// The whole shrinker rests on this: a trace is only a repro if the run that
/// produced it is the run everything else measures. The recorder clones the
/// `Op` the generator drew and reads state the engine already computes, so it
/// cannot - and this is the measurement that says so, over a gate seed in both
/// modes.
#[test]
fn a_recorded_run_matches_an_unrecorded_one() {
    let _config = isolated_config();
    for persistence in [false, true] {
        let plan = Plan {
            iterations: GATE_ITERATIONS,
            persistence,
            crossings: 1,
            prologue: None,
            jumps: false,
            closures: false,
            divergence: false,
            closure_shapes: false,
            horizon: false,
            config_fuzz: false,
            combined: false,
        };
        let plain = run_fuzz(3, plan);
        let recorded = record_run(3, plan);
        let taped = recorded
            .probe
            .as_ref()
            .expect("seed 3 is a gate seed and does not panic");
        assert_eq!(
            plain.shape(),
            taped.shape(),
            "recording changed what seed 3 did (persistence={persistence})"
        );
        assert!(
            !recorded.trace.moves.is_empty(),
            "seed 3 recorded no moves at all (persistence={persistence})"
        );
    }
}

/// A FULL trace replays to the same run it was recorded from.
///
/// The shrinker removes moves; this pins the case where it removes none, which
/// is the only one with a known right answer. If replay of a complete trace
/// diverges, every shrink is minimizing a different program from the one that
/// failed - so this is the load-bearing test of the whole module, not a
/// smoke test.
#[test]
fn a_full_trace_replays_to_the_same_run() {
    let _config = isolated_config();
    for persistence in [false, true] {
        let plan = Plan {
            iterations: GATE_ITERATIONS,
            persistence,
            crossings: 1,
            prologue: None,
            jumps: false,
            closures: false,
            divergence: false,
            closure_shapes: false,
            horizon: false,
            config_fuzz: false,
            combined: false,
        };
        let recorded = record_run(4, plan);
        let original = recorded
            .probe
            .as_ref()
            .expect("seed 4 is a gate seed and does not panic");
        let moves = recorded.trace.moves.clone();
        let (replayed, skipped) = run_trace(&recorded.trace, &moves);
        assert!(
            skipped.is_empty(),
            "replaying a COMPLETE trace skipped {} move(s), so a reference did not resolve \
             against the state that produced it: {skipped:?}",
            skipped.len()
        );
        assert_eq!(
            original.shape(),
            replayed.shape(),
            "a full trace did not replay to its own run (persistence={persistence})"
        );
    }
}

/// The ddmin schedule itself, over a predicate with a KNOWN minimum.
///
/// Deliberately not the engine: this pins the search, and a search bug that only
/// showed up through a whole fuzz run would be indistinguishable from a replay
/// bug. The predicate is "the sequence still holds 3 and 7", whose unique
/// 1-minimal subsequence is `[3, 7]`.
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
        "ddmin took {probes} probes over {original} elements - the schedule is not converging"
    );
}

/// The whole flow, over an INJECTED failure: record, shrink, and check the
/// minimum still reproduces - and that it is genuinely smaller.
///
/// It used to shrink a KNOWN finding, targeting "whatever this run classifies"
/// rather than a named id, so that it stayed a test of the SHRINKER rather than a
/// second, weaker copy of the bug matrix. Every #413 finding is fixed now and
/// `CLASSIFIERS` is empty, so there is nothing left for a run to classify and
/// that target could never be chosen again. Waiting for the next real bug would
/// leave the shrinker unguarded until then.
///
/// So the failure is INJECTED into the REPLAY (see `Fault` in
/// `fuzz_undo_tests.rs`): the trace fails once the project holds
/// [`FAULT_AT_SECTIONS`] sections, which is a property of the moves a candidate
/// kept rather than of the app. Everything the gate covers is unchanged - the
/// recorder, `Refs`' reference remapping, the panic-signature match, the ddmin
/// schedule and the verification replay - and the app is not required to be
/// broken for any of it.
///
/// The starting project has SEVEN sections (`build_project`), so the threshold is
/// above it by construction: the empty trace cannot reproduce, and a minimum has
/// to keep the `createSection` moves that get from seven to twelve. That is what
/// the assertions below pin - a non-empty minimum that is strictly smaller than
/// the trace it came from.
const FAULT_AT_SECTIONS: usize = 12;

#[test]
fn the_shrinker_minimizes_an_injected_failure() {
    let _config = isolated_config();
    // The in-memory gate's own seed and length, so the moves being shrunk are
    // ordinary ones rather than a constructed shape.
    let plan = Plan {
        iterations: GATE_ITERATIONS,
        persistence: false,
        crossings: 1,
        prologue: None,
        jumps: false,
        closures: false,
        divergence: false,
        closure_shapes: false,
        horizon: false,
        config_fuzz: false,
        combined: false,
    };
    // Through the PROGRAMMATIC entry point, which is what a future observation
    // hunt would call: record, choose the target from what the run produced,
    // shrink. The only difference from a real hunt is the armed fault.
    let fault = Some(Fault {
        at_sections: FAULT_AT_SECTIONS,
    });
    let Some((trace, target, shrunk)) = shrink_seed(1, plan, fault, None, 400, false) else {
        panic!(
            "the injected fault did not fire over the whole trace, so seed 1 never reaches \
             {FAULT_AT_SECTIONS} sections - re-choose FAULT_AT_SECTIONS"
        );
    };
    assert!(
        matches!(target, Target::Panic(_)),
        "the injected fault must surface as a panic target, not as {}",
        target.describe()
    );
    assert!(
        shrunk.moves.len() < shrunk.original,
        "the shrinker did not shrink: {} -> {} move(s)",
        shrunk.original,
        shrunk.moves.len()
    );
    assert!(
        !shrunk.moves.is_empty(),
        "the empty trace cannot reach {FAULT_AT_SECTIONS} sections from the seven the starting \
         project has, so a minimum of zero moves means the predicate stopped depending on the \
         moves"
    );
    assert!(
        reproduces(&target, borrow(&probe(&trace, &shrunk.moves))),
        "the minimized trace stopped reproducing {}",
        target.describe()
    );
    // ... and one move fewer does NOT, which is what makes it a MINIMUM rather
    // than merely a smaller trace that still fails.
    let one_shorter = &shrunk.moves[..shrunk.moves.len() - 1];
    assert!(
        !reproduces(&target, borrow(&probe(&trace, one_shorter))),
        "dropping the last move of the minimum still reproduces {} - ddmin stopped early",
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

/// A shrink step that changes the failure is a NON-MATCH.
///
/// The rule the whole module rests on, checked directly rather than through a
/// run: the two kinds of signature never satisfy each other, and a classified id
/// only matches the hits it is actually in.
#[test]
fn a_different_signature_is_not_a_match() {
    let _config = isolated_config();
    let mut hits: BTreeMap<&'static str, usize> = BTreeMap::new();
    hits.insert("finding-8-stale-id-deletes-the-wrong-shot", 1);
    let completed: Probe = Ok(Outcome::from_hits(hits));
    let panicked: Probe = Err("did not restore the state recorded at depth 23\n    at \
                               sections[4].shots[2].length"
        .to_string());

    let eight = Target::Classified("finding-8-stale-id-deletes-the-wrong-shot".to_string());
    let twelve = Target::Classified("finding-12-stale-id-edits-the-wrong-shot".to_string());
    assert!(reproduces(&eight, borrow(&completed)));
    assert!(!reproduces(&twelve, borrow(&completed)));
    // A classified target is not reproduced by a panic, however similar.
    assert!(!reproduces(&eight, borrow(&panicked)));

    let signature = failure_signature(panicked.as_ref().unwrap_err());
    assert!(reproduces(
        &Target::Panic(signature.clone()),
        borrow(&panicked)
    ));
    // ... and a panic target is not reproduced by a run that completed, even one
    // carrying classified hits.
    assert!(!reproduces(&Target::Panic(signature), borrow(&completed)));

    // The digit collapse is what makes one defect one bucket: the same failure
    // at a different depth and a different index has the same signature.
    let deeper: Probe = Err("did not restore the state recorded at depth 7\n    at \
                             sections[11].shots[9].length"
        .to_string());
    assert_eq!(
        failure_signature(panicked.as_ref().unwrap_err()),
        failure_signature(deeper.as_ref().unwrap_err()),
        "two depths of one defect landed in different buckets"
    );
}

/// One `Fuzzer::fail` payload for the app's merge-alignment assert, built the
/// way `fail` builds one: a FIXED headline, the run header, the detail line, and
/// the move-log tail with the assert's own message under the move that tripped
/// it.
///
/// The three parameters are exactly the three things that differ between two
/// runs of ONE defect (`depth`, `distance`, `coords`) and the one thing that
/// differs between two DIFFERENT defects (`context`). Written as a builder so
/// the tests below can vary them one at a time rather than eyeballing two large
/// string literals for their differences.
fn alignment_payload(context: &str, depth: usize, distance: f64, coords: (f64, f64)) -> String {
    format!(
        "the app's own merge-alignment assert fired\n  \
         seed=162 depth={depth} top={depth} saves=0 reloads=0\n  \
         see the last move in the log below for the assert's own message\n  \
         last 2 move(s):\n    \
         mergeStations(4/1, 8/3) -> ok (undo {depth}->{depth}, redo 0->0)\n    \
         deleteShotRangeShift(s=4, 400000..400002) -> PANICKED (did not return)\n      \
         ^ the app's own merge-alignment assert fired: Merge alignment violated after \
         {context}: stations StationKey {{ section_id: 3, station_index: 0 }} and \
         StationKey {{ section_id: 8, station_index: 2 }} are {distance:.6}m apart \
         (threshold 0.01m). A=({:.6}, {:.6}), B=(0.000000, 0.000000)",
        coords.0, coords.1,
    )
}

/// Two defects behind ONE app assert are two signatures.
///
/// The gotcha issue #464 was filed with: `Fuzzer::fail`'s headline names the
/// INVARIANT ("the app's own merge-alignment assert fired") and not the call
/// site, so all three of that issue's defects produced the same signature and a
/// shrink of the second one drifted onto the third. The assert's context string
/// is what tells them apart, and this pins that it reaches the signature - and
/// that ddmin's `reproduces` therefore rejects the other defect.
#[test]
fn the_assert_context_splits_one_headline_into_two_signatures() {
    let _config = isolated_config();
    let range_delete = alignment_payload("delete_shot_range_shift", 41, 7.512_345, (1.0, 2.0));
    let rewire = alignment_payload("remove_shot_with_rewire", 41, 7.512_345, (1.0, 2.0));

    // Same headline by construction - which is the whole problem being fixed.
    assert_eq!(
        range_delete.lines().next(),
        rewire.lines().next(),
        "the two payloads must share a headline, else this proves nothing"
    );
    assert_ne!(
        failure_signature(&range_delete),
        failure_signature(&rewire),
        "two defects sharing one assert still share one signature"
    );
    // The discriminator is the call site, and it is in there by name.
    assert!(
        failure_signature(&range_delete).contains("delete_shot_range_shift"),
        "the asserting call site is not in the signature: {}",
        failure_signature(&range_delete)
    );

    // ... and that is what ddmin matches on: a candidate that reproduces the
    // OTHER defect is a NON-MATCH, so a shrink cannot drift onto it.
    let target = Target::Panic(failure_signature(&range_delete));
    let drifted: Probe = Err(rewire);
    let held: Probe = Err(range_delete);
    assert!(reproduces(&target, borrow(&held)));
    assert!(
        !reproduces(&target, borrow(&drifted)),
        "a shrink step that landed on a different call site was accepted as the same failure"
    );
}

/// One defect at two geometries is ONE signature.
///
/// The other half of the rule, and the one that decides whether the shrinker
/// still works at all: every move ddmin removes changes the survey, so the
/// distance and the coordinates the assert reports differ between the original
/// trace and every candidate. A signature carrying them would reject every
/// candidate and shrink nothing. The depth moves for the same reason.
#[test]
fn the_same_defect_at_a_different_geometry_is_one_signature() {
    let _config = isolated_config();
    let small = alignment_payload("remove_shot_with_rewire", 41, 16.7, (1.0, 2.0));
    let large = alignment_payload("remove_shot_with_rewire", 7, 1_111.32, (58_921.5, -412.25));
    assert_eq!(
        failure_signature(&small),
        failure_signature(&large),
        "one defect split into two buckets when the geometry changed"
    );
    // Stated exactly as well, because "equal to each other" would also hold if
    // the signature collapsed to something useless: this is the whole string,
    // and the distance clause the assert carries ("... are 1111.320000m apart")
    // is not in it.
    let signature = failure_signature(&small);
    assert_eq!(
        signature, "the app's own merge-alignment assert fired | after=remove_shot_with_rewire",
        "the signature is not headline + call site"
    );
    let target = Target::Panic(signature);
    assert!(
        reproduces(&target, borrow(&Err(large))),
        "the same defect at a different magnitude stopped reproducing its own target"
    );
}
