//! RARITY-KEYED corpus admission, and the SECOND measurement saying a corpus is
//! not what this fuzzer is short of.
//!
//! # The question this closes
//!
//! `fuzz_corpus_sweep` (fuzz_undo_tests.rs) measured corpus guidance keyed on
//! whole-signature NOVELTY and found it worth nothing: 78 of 80 random runs land
//! on a novel signature, so the predicate admits nearly everything and "guided"
//! only means half the budget replaying a prefix. That write-up left one named
//! alternative - admission keyed on a RARE feature rather than on novelty,
//! because the shapes findings 4 and 10 needed are rare rather than new, and
//! `c`/`y`/`v` in the signature are exactly those bits.
//!
//! This module measures that alternative, against the random walk and against
//! the predicate it was supposed to beat. **The answer is no again**, and this
//! time with the mechanism rather than only the number. Nothing here is wired
//! into any gate.
//!
//! # What a bit is
//!
//! A bit is one signature feature at or above a bucket threshold - thirteen of
//! them, in [`BITS`] - and RARE means "measured at or under 25% of a calibration
//! sample", not "picked because it sounds rare". Measured (80 runs of 60 moves,
//! `STALAGMIGHT_RUST_FUZZ_RARITY_THRESHOLD=25`):
//!
//! | bit | plain, end states | closures, end states | closures, prefix states |
//! |---|---|---|---|
//! | `merges-3+` | 3.8% | 10.4% | 17.9% |
//! | `merge-star` | 1.3% | 6.5% | 2.2% |
//! | `closures-2+` | 0.0% | 3.9% | 0.4% |
//! | `datums-2+` | 7.7% | 6.5% | 0.6% |
//! | `loop-closure` | 0.0% | 27.3% | 50.0% |
//! | `diverged-ids` | 80.8% | 51.9% | 74.4% |
//! | `section-offset` | 100.0% | 68.8% | 99.6% |
//!
//! Two things fall out before any arm runs. `loop-closure`, `closures-2+` and
//! `cyclic-merges` are IDENTICALLY ZERO under the plain plan - the generators
//! cannot make a closure with `Plan::closures` off - so the plain regime cannot
//! test the hypothesis the write-up actually made, which is why the whole
//! comparison runs under `closures: true` as well. And `cyclic-merges` is zero
//! in BOTH: 60 moves never closes a cycle in the merge graph, so a bit that
//! costs nothing to key on can still be unreachable.
//!
//! # The four arms
//!
//! | arm | admits | corpus entry |
//! |---|---|---|
//! | `unguided` | nothing | - |
//! | `novel` | a run whose whole signature was new | `(lineage, iterations / 2)` |
//! | `rare` | a run whose END state carries a rare bit | `(lineage, iterations / 2)` |
//! | `prefix` | a STATE on the mutate-only trajectory carrying a rare bit | `(seed, first such prefix)` |
//!
//! `rare` is `novel` with one line changed, which is what makes it the honest
//! successor to the first experiment. `prefix` exists because of the defect that
//! experiment never looked for - see "The two trajectories" below - and it pays
//! for its probes out of the same budget.
//!
//! # What "the same budget" has to mean
//!
//! Three ways to count, and they do not agree, which is the second finding here.
//!
//! * **Runs.** The first experiment's accounting. A `Plan::prologue` replays
//!   `iterations / 2` mutations BEFORE the run's own stream, so a guided run
//!   does half again as much work as an unguided one: over 80 runs of the plain
//!   regime, 9665 engine moves against 6780 and 31.6 s against 17.6 s. Guidance
//!   was not being measured at the same budget, it was being subsidised by 80%.
//! * **Moves.** `Setup::charge` shortens a continuation's own stream by what its
//!   prologue cost, so every run is `iterations` moves. Still a subsidy, for two
//!   reasons: a continuation inherits a BIGGER project, so it rejects fewer
//!   draws and its closing unwind walks deeper (8136 moves against 6780), and
//!   every one of those moves costs more, because every oracle in this engine
//!   canonicalizes the WHOLE project (2.87 ms per move against 2.34).
//! * **Wall time.** 23.3 s against 15.9 s in the same charged plain regime. The
//!   only measure that charges guidance for both, and therefore the only one a
//!   verdict can rest on.
//!
//! Both accountings are run and reported. An effect that only exists when
//! guidance is subsidised is not an effect.
//!
//! # The numbers
//!
//! MEASURED WHILE THE ISSUE-413 FINDINGS WERE OPEN, and kept as the record of
//! the experiment rather than re-run. Every one of those findings is fixed now
//! and `fuzz_undo_tests.rs`'s `CLASSIFIERS` table is EMPTY, so re-running this
//! today measures the `sigs` column and nothing else: `ids` and `rare-ids` are
//! zero by construction, and the three `first` columns have no event to be
//! first to. That does not weaken the conclusion - the conclusion is NEGATIVE
//! and the signature column, the metric the first experiment used, is the one
//! that carries it. The classifier-keyed columns become measurable again the
//! moment there is another open bug to key on, which is why the machinery
//! stays.
//!
//! Six repetitions of 80 runs x 60 moves per arm, disjoint seed blocks per
//! repetition, prologue CHARGED, `STALAGMIGHT_RUST_FUZZ_THREADS=8`. `sigs` is
//! distinct topology signatures (the first experiment's primary metric), `ids`
//! is distinct classifier ids, `rare-ids` is how many of the regime's rare
//! classified findings the arm reached at all, and the three `first` columns are
//! the same time-to-first-hit over those findings counted three ways:
//!
//! | regime | arm | sigs | ids | rare-ids | first (runs) | first (moves) | first (ms) |
//! |---|---|---|---|---|---|---|---|
//! | plain | unguided | **73.5** | 7.7 | 2.7/3 | 38.4 | 3071 | **7770** |
//! | plain | novel | 74.2 | 7.8 | 2.8/3 | 36.2 | 3622 | 10657 |
//! | plain | rare | 70.3 | 7.8 | 2.8/3 | 34.9 | 3263 | 9729 |
//! | plain | prefix | 62.3 | 7.7 | 2.7/3 | **28.8** | **2832** | 8968 |
//! | closures | unguided | 60.5 | 9.3 | 4.3/6 | 40.1 | 2515 | **7409** |
//! | closures | novel | **65.0** | 9.8 | 4.8/6 | 34.7 | 2519 | 10269 |
//! | closures | rare | 60.2 | 9.8 | 4.8/6 | 35.8 | 2572 | 11072 |
//! | closures | prefix | 57.5 | **10.2** | **5.2/6** | **23.7** | **2079** | 7706 |
//!
//! Read the `prefix` row across and the whole experiment is in it. Counted in
//! RUNS it reaches the rare findings 41% sooner than the random walk (23.7
//! against 40.1) and does so in five of six repetitions. Counted in MOVES the
//! lead falls to 17%. Counted in WALL TIME it is GONE (7706 against 7409, and
//! worse than the random walk in four of the six repetitions). The lead was the
//! metric, not the guidance: a continuation run is not one run's worth of
//! compute, and scoring guidance per run pays it for being expensive.
//!
//! On the metric the first experiment used, guidance loses outright: 62.3
//! distinct signatures against 73.5 in the plain regime, which is the same
//! effect it found - budget spent replaying is budget not spent exploring.
//!
//! And the whole guided family is more expensive per repetition even after the
//! charge, because of the per-move cost above: 18.7 s (`prefix`) to 23.4 s
//! (`rare`) against 15.9 s unguided in the plain regime, 18.9 s to 24.3 s
//! against 14.6 s in the closures one. No arm bought anything with it.
//!
//! Distinct FAILURE signatures - the UNCLASSIFIED panics, which is what a fuzzer
//! is finally scored on - separate no arm from any other. Over six repetitions
//! each arm reaches 3 to 5 buckets of a 6-bucket union in the plain regime and 3
//! to 5 of a 5-bucket union in the closures one, and the arm holding the extra
//! bucket is a different one each time: `novel` alone reaches
//! `undo at section_offsets.#` and `prefix` alone `undo at station_merges.length`
//! in plain, while in closures it is `unguided` that reaches one neither `novel`
//! nor `rare` does. Every extra bucket is a depth-collapsed variant of a family
//! every arm already reaches. Only 2 to 6 runs in 80 fail at all here, so this
//! metric has little resolution to give at any budget this experiment can
//! afford; it is reported, and it is not what the verdict rests on.
//!
//! # The two trajectories, and why `rare` cannot work at all
//!
//! `Plan::prologue` replays `(seed, n)` as n MUTATIONS of that seed's own
//! stream. A run that is being ADMITTED walked a different trajectory: it drew
//! undos and redos too, from an RNG that has therefore advanced differently
//! since its second move. The state a corpus entry delivers is not the state the
//! admission decision was made on, and nothing in the first experiment measured
//! the gap. `fuzz_rarity_carryover` does:
//!
//! ```text
//! plain:    9 admitted run(s): 1 prologue still carries a rare bit, 8 carry none
//! closures: 21 admitted run(s): 9 still carry one, 12 carry none
//!             merge-star  admitted 8, replayed 0
//!             merges-3+   admitted 19, replayed 7
//! ```
//!
//! So end-state keying is 11% faithful in the plain regime and 43% in the
//! closures one: the `rare` arm mostly compounds shapes that are not there any
//! more. That is a real defect and it is FIXED in the `prefix` arm, which
//! observes the trajectory a prologue actually replays and is exact by
//! construction - and the fixed version still does not beat the random walk once
//! its compute is counted. The negative result is not an artifact of the
//! plumbing; the plumbing was repaired and the result held.
//!
//! # What this leaves
//!
//! Nothing here is wired in, for the same reason `fuzz_corpus_sweep` is not: the
//! measurement IS the deliverable. Two negative results now say the same thing
//! from different directions - the random walk is not this tier's bottleneck.
//! Every finding in the module header's matrix was reached by a seed and a mode,
//! and the ones that needed a shape the walk does not reach (4, 10, 8, 12, 13)
//! got it from a CONSTRUCTED prologue (`Plan::closure_shapes`, `Plan::divergence`)
//! rather than from a remembered one. Constructing the shape you want is cheaper
//! and more reliable than hoping a corpus rediscovers it.
//!
//! What is NOT closed, and would be measured rather than argued about: guidance
//! over a regime whose interesting states are hours away rather than seconds
//! (`Plan::horizon` at ten thousand moves), where a prologue's cost is a
//! rounding error against the depth it saves. That is a different experiment
//! with a different budget, and this one says nothing about it.
//!
//! # Re-running it
//!
//! Every table above comes from these two commands and nothing else. About six
//! minutes for the first and one for the second on sixteen cores.
//!
//! ```text
//! STALAGMIGHT_RUST_FUZZ_THREADS=8 STALAGMIGHT_RUST_FUZZ_RARITY_BUDGET=80 \
//!   STALAGMIGHT_RUST_FUZZ_RARITY_REPS=6 STALAGMIGHT_RUST_FUZZ_ITERATIONS=60 \
//!   TELEMETRY_ENDPOINT=http://localhost/dummy cargo test \
//!   --manifest-path src-tauri/Cargo.toml -p stalagmight-core \
//!   --lib fuzz_rarity_sweep -- --ignored --nocapture
//!
//! STALAGMIGHT_RUST_FUZZ_RARITY_BUDGET=80 STALAGMIGHT_RUST_FUZZ_ITERATIONS=60 \
//!   TELEMETRY_ENDPOINT=http://localhost/dummy cargo test \
//!   --manifest-path src-tauri/Cargo.toml -p stalagmight-core \
//!   --lib fuzz_rarity_carryover -- --ignored --nocapture
//! ```
//!
//! Every column but the two wall-clock ones is a pure function of the knobs:
//! runs are pure functions of `(seed, Plan)` as everywhere else in this
//! programme, the repetitions take disjoint seed blocks, and the only randomness
//! this module adds is the prologue picker, which lives in its own [`Rng`] on
//! [`PICKER_STREAM`] and touches no run's own stream. Re-running the first
//! command reproduces `sigs`, `ids`, `hits`, `fails`, `rare-bits`, `rare-ids`,
//! `first-hit`, `mv-first` and `moves` exactly; `ms-first` and `ms` are wall
//! time and will not, which is why `STALAGMIGHT_RUST_FUZZ_THREADS=8` is in the
//! command - measure them with the machine left some headroom.
//!
//! Everything is generated in code. No fixture is read, copied or derived from.

#![expect(
    clippy::indexing_slicing,
    reason = "test module: see docs/linting.md for the per-test-module expect idiom"
)]
#![expect(
    clippy::print_stdout,
    reason = "a measurement's output IS its deliverable: the per-repetition table is what a \
              reviewer reads to check the verdict, and `cargo test -- --nocapture` is the only \
              channel a test has"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::panic::AssertUnwindSafe;
use std::time::Instant;

use super::fuzz_undo_tests::{
    failure_signature, fuzz_threads, panic_message, prologue_signatures, run_fuzz, run_seed_jobs,
    Knobs, Outcome, Plan, Rng, GATE_ITERATIONS,
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
// The bits
// ===========================================================================

/// The feature letters `topology_signature` prints, in its order.
///
/// Parsing depends on this being exactly the producer's format, which
/// `the_rarity_bits_read_the_topology_signature` pins against a real run.
const FEATURES: &[u8; 12] = b"seohmdcyzgkv";

/// A structural BIT: one signature feature, at or above a bucket threshold.
///
/// Bits rather than the whole signature is the entire hypothesis. The previous
/// experiment (`fuzz_corpus_sweep`) admitted a state because its twelve-feature
/// signature was NOVEL, and 78 of 80 random runs are novel - so the predicate
/// carried no information. A bit is a single structural possibility the project
/// has acquired, and the ones that matter are rare rather than new.
struct BitSpec {
    name: &'static str,
    /// Index into [`FEATURES`].
    feature: usize,
    /// The smallest BUCKET value that counts as having the bit. `bucket` maps
    /// 0 -> 0, 1 -> 1, 2 -> 2, 3..=4 -> 3, 5..=8 -> 4, 9..=16 -> 5, 17..=32 -> 6,
    /// so `least: 2` on a count feature reads "at least two".
    least: u8,
}

/// The thirteen candidate bits.
///
/// Chosen from what this programme has actually needed: findings 4 and 10 need
/// a second closure and an orphaned one, finding 3 needs an EMPTY section,
/// findings 8, 12 and 13 need ids that have diverged from positional order, and
/// findings 14 and 20 need a merge graph with somewhere for an alignment to go
/// wrong. Whether any of them is RARE is measured, not assumed.
const BITS: [BitSpec; 13] = [
    BitSpec {
        name: "empty-section",
        feature: 1,
        least: 1,
    },
    BitSpec {
        name: "one-shot-section",
        feature: 2,
        least: 1,
    },
    BitSpec {
        name: "shots-17+",
        feature: 3,
        least: 6,
    },
    BitSpec {
        name: "merge-edge",
        feature: 4,
        least: 1,
    },
    BitSpec {
        name: "merges-3+",
        feature: 4,
        least: 3,
    },
    BitSpec {
        name: "merge-star",
        feature: 5,
        least: 3,
    },
    BitSpec {
        name: "loop-closure",
        feature: 6,
        least: 1,
    },
    BitSpec {
        name: "closures-2+",
        feature: 6,
        least: 2,
    },
    BitSpec {
        name: "cyclic-merges",
        feature: 7,
        least: 1,
    },
    BitSpec {
        name: "datums-2+",
        feature: 8,
        least: 2,
    },
    BitSpec {
        name: "section-offset",
        feature: 9,
        least: 1,
    },
    BitSpec {
        name: "station-comment",
        feature: 10,
        least: 1,
    },
    BitSpec {
        name: "diverged-ids",
        feature: 11,
        least: 1,
    },
];

/// The twelve bucket values behind a signature, or `None` when the string is
/// not one (`topology_signature` returns `no-project` for a state with no
/// project open).
fn features_of(signature: &str) -> Option<[u8; 12]> {
    let bytes = signature.as_bytes();
    if bytes.len() != FEATURES.len() * 2 {
        return None;
    }
    let mut values = [0_u8; 12];
    for (index, &letter) in FEATURES.iter().enumerate() {
        if bytes[index * 2] != letter {
            return None;
        }
        let digit = bytes[index * 2 + 1];
        if !digit.is_ascii_digit() {
            return None;
        }
        values[index] = digit - b'0';
    }
    Some(values)
}

/// Which of [`BITS`] a signature exhibits, as a mask.
fn bits_of(signature: &str) -> u16 {
    let Some(values) = features_of(signature) else {
        return 0;
    };
    let mut mask = 0;
    for (index, bit) in BITS.iter().enumerate() {
        if values[bit.feature] >= bit.least {
            mask |= 1 << index;
        }
    }
    mask
}

fn bit_names(mask: u16) -> Vec<&'static str> {
    BITS.iter()
        .enumerate()
        .filter(|(index, _)| mask & (1 << index) != 0)
        .map(|(_, bit)| bit.name)
        .collect()
}

// ===========================================================================
// Base rates
// ===========================================================================

/// How often each bit is exhibited over a sample of states.
///
/// The DENOMINATOR is what makes a rate mean anything, and the two arms need
/// two different ones: guidance keyed on a finished run's end state is measured
/// over end states, and guidance keyed on a prologue's own trajectory is
/// measured over the prefix states that trajectory passes through. They are not
/// the same distribution - see the write-up.
#[derive(Default)]
struct BaseRates {
    counts: [usize; BITS.len()],
    total: usize,
}

impl BaseRates {
    fn observe(&mut self, signature: &str) {
        let mask = bits_of(signature);
        self.total += 1;
        for (index, count) in self.counts.iter_mut().enumerate() {
            if mask & (1 << index) != 0 {
                *count += 1;
            }
        }
    }

    fn rate(&self, index: usize) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.counts[index] as f64 / self.total as f64
    }

    /// The bits that are RARE: seen at least once (a bit this regime cannot
    /// reach can guide nothing) and on no more than `threshold` percent of the
    /// sample.
    fn rare_mask(&self, threshold: usize) -> u16 {
        let mut mask = 0;
        for index in 0..BITS.len() {
            let rate = self.rate(index);
            if self.counts[index] > 0 && rate <= threshold as f64 / 100.0 {
                mask |= 1 << index;
            }
        }
        mask
    }

    fn print(&self, label: &str) {
        println!("  base rates over {} {label} state(s):", self.total);
        for (index, bit) in BITS.iter().enumerate() {
            println!(
                "    {:<16} {:>5.1}%  ({}/{})",
                bit.name,
                self.rate(index) * 100.0,
                self.counts[index],
                self.total
            );
        }
    }
}

// ===========================================================================
// The regimes and the arms
// ===========================================================================

/// Which plan the whole comparison runs under.
///
/// PLAIN is the previous experiment's plan, unchanged, so the two measurements
/// are comparable. CLOSURES is here because the hypothesis is specifically
/// about compounding closure shapes, and under the plain plan the generators
/// cannot make a loop closure at all - the `loop-closure` and `closures-2+`
/// bits are identically zero there, so a rarity key has nothing to key on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Regime {
    Plain,
    Closures,
}

impl Regime {
    const fn label(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Closures => "closures",
        }
    }

    const fn closures(self) -> bool {
        matches!(self, Self::Closures)
    }
}

/// The four ways a sweep chooses what to run next.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Arm {
    /// The control: `budget` fresh seeds, nothing remembered.
    Unguided,
    /// The PREVIOUS experiment's predicate, re-measured under this harness:
    /// admit a run whose whole signature was novel.
    Novel,
    /// Admit a run whose END state exhibits a rare bit. Same corpus mechanics
    /// as `Novel` - including the `(seed, iterations / 2)` entry - so the only
    /// variable against it is the predicate.
    Rare,
    /// Admit STATES rather than runs: probe a seed's mutate-only trajectory,
    /// which is exactly what a prologue replays, and admit the earliest prefix
    /// at which each rare bit appears. Exact by construction, and it pays for
    /// the probes out of the same move budget.
    Prefix,
}

impl Arm {
    const fn label(self) -> &'static str {
        match self {
            Self::Unguided => "unguided",
            Self::Novel => "novel   ",
            Self::Rare => "rare    ",
            Self::Prefix => "prefix  ",
        }
    }
}

/// Seed block base. Far from the seeds any gate, survey or documented repro
/// uses, so an experiment run never collides with a pinned one.
const SEED_BLOCK: u32 = 100_000;
/// Seed block for the calibration runs, disjoint from every repetition's block.
const CALIB_BLOCK: u32 = 900_000;
/// The prologue picker's stream, which is the only randomness this experiment
/// adds. Additive by construction: it lives in its own `Rng`, drawn from a
/// meta-seed, and no run's own stream is touched by it.
const PICKER_STREAM: u32 = 0x00CA_11B0;
/// What a repetition moves the picker's seed by. The golden-ratio constant,
/// for the usual reason: consecutive repetitions must not draw correlated
/// pickers, and `rep` alone would give them adjacent seeds.
const REP_STRIDE: u32 = 0x9E37_79B9;

fn plan_for(regime: Regime, iterations: usize, prologue: Option<(u32, usize)>) -> Plan {
    Plan {
        iterations,
        persistence: false,
        crossings: 1,
        prologue,
        jumps: false,
        closures: regime.closures(),
        divergence: false,
        closure_shapes: false,
        horizon: false,
        config_fuzz: false,
        combined: false,
        knobs: Knobs::OFF,
    }
}

/// Everything an arm needs that is a property of the REGIME rather than of the
/// arm: the budget, and what the calibration measured.
struct Setup {
    regime: Regime,
    iterations: usize,
    budget: usize,
    /// Is a prologue CHARGED to the run that replays it?
    ///
    /// The previous experiment's answer was no, and it is the one thing about
    /// its protocol this changes. `Plan::prologue` replays `iterations / 2`
    /// mutations BEFORE the run's own stream, so under that accounting a guided
    /// run does half again as much engine work as an unguided one - measured
    /// over 80 runs of the plain regime at 9665 moves against 6780, and 31.6 s
    /// against 17.6 s. "Same budget" then means the same number of runs, not
    /// the same amount of compute, and guidance is being handed an 80% bonus
    /// rather than being made to pay for itself.
    ///
    /// Charged (the default), a continuation's OWN stream is shortened by what
    /// its prologue cost, so every run is `iterations` moves whatever it
    /// inherited. Both accountings are measured: an effect that only exists
    /// when guidance is subsidised is not an effect.
    charge: bool,
    /// Probes the `Prefix` arm runs, each costing one run's worth of moves.
    probes: usize,
    /// Rare bits over END states - what `Rare` admits on.
    rare_end: u16,
    /// Rare bits over PREFIX states - what `Prefix` admits on.
    rare_prefix: u16,
}

// ===========================================================================
// What a sweep is scored on
// ===========================================================================

#[derive(Default)]
struct Tally {
    runs: usize,
    /// Moves the engine actually executed, probes included. The budget is
    /// matched on this, and it is printed so the match can be checked rather
    /// than trusted.
    moves: usize,
    signatures: BTreeSet<String>,
    hits: BTreeMap<&'static str, usize>,
    /// RUNS that reached each classifier id, which is what an id's incidence is
    /// measured in - a run that hits one finding forty times is one run.
    id_runs: BTreeMap<&'static str, usize>,
    /// Run index at which each classifier id was first reached.
    first_hit: BTreeMap<&'static str, usize>,
    /// Engine moves consumed - probes included - when each classifier id was
    /// first reached. The run INDEX is not a compute measure: a continuation
    /// run inherits a bigger project, so it draws more moves and each of them
    /// costs more oracle work than a fresh run's. This is the same
    /// time-to-first-hit normalized by what it actually cost.
    moves_at_first_hit: BTreeMap<&'static str, usize>,
    /// Microseconds of the arm's own wall clock consumed when each classifier
    /// id was first reached. Moves are not a compute measure either: every
    /// oracle in this engine canonicalizes the WHOLE project, so a move made in
    /// a project a prologue has already grown costs more than a move made in a
    /// fresh one. This is the only measure that charges for that, and the only
    /// number here that is not a pure function of the seeds - measure it at a
    /// thread count that leaves the machine headroom.
    micros_at_first_hit: BTreeMap<&'static str, u128>,
    failures: BTreeMap<String, usize>,
    /// Bits any run's end state exhibited.
    bits_seen: u16,
    admitted: usize,
    /// The arm's own clock, read once per run rather than per move.
    micros: u128,
    /// Continuation runs, and the total length of the prologues they replayed.
    continuations: usize,
    prologue_moves: usize,
    millis: u128,
}

impl Tally {
    fn observe(&mut self, index: usize, outcome: &Outcome, micros: u128) {
        self.micros = micros;
        let shape = outcome.shape();
        self.runs += 1;
        self.moves += shape.mutations + shape.undos + shape.redos;
        self.signatures.insert(shape.signature.to_string());
        for (&id, &count) in shape.hits {
            *self.hits.entry(id).or_insert(0) += count;
            *self.id_runs.entry(id).or_insert(0) += 1;
            self.first_hit.entry(id).or_insert(index);
            self.moves_at_first_hit.entry(id).or_insert(self.moves);
            self.micros_at_first_hit.entry(id).or_insert(micros);
        }
        self.bits_seen |= bits_of(shape.signature);
    }

    fn observe_failure(&mut self, message: &str) {
        self.runs += 1;
        *self.failures.entry(failure_signature(message)).or_insert(0) += 1;
    }

    /// Mean run index at which the arm first reached each of the rare
    /// classifier ids, counting a MISS as the whole budget: an arm that never
    /// reaches an id must not score better than one that reached it late.
    ///
    /// The target set is pooled over every arm of the regime rather than taken
    /// from the calibration, because the ids that matter here are exactly the
    /// ones a calibration of 80 runs does not see: finding 17 turns up on one
    /// run in eighty. A set drawn from the calibration would be empty in the
    /// plain regime and would make the metric vacuous.
    fn mean_first_hit(&self, rare_ids: &[&'static str], budget: usize) -> f64 {
        if rare_ids.is_empty() {
            return 0.0;
        }
        let total: usize = rare_ids
            .iter()
            .map(|id| self.first_hit.get(id).copied().unwrap_or(budget))
            .sum();
        total as f64 / rare_ids.len() as f64
    }

    /// [`Self::mean_first_hit`], in engine moves rather than in runs, censored
    /// at the arm's nominal move budget so every arm is censored at the same
    /// number whatever it actually spent.
    fn mean_moves_to_first(&self, rare_ids: &[&'static str], censor: usize) -> f64 {
        if rare_ids.is_empty() {
            return 0.0;
        }
        let total: usize = rare_ids
            .iter()
            .map(|id| self.moves_at_first_hit.get(id).copied().unwrap_or(censor))
            .sum();
        total as f64 / rare_ids.len() as f64
    }

    /// [`Self::mean_moves_to_first`], in the arm's own microseconds, censored
    /// at what the arm spent in total.
    fn mean_micros_to_first(&self, rare_ids: &[&'static str]) -> f64 {
        if rare_ids.is_empty() {
            return 0.0;
        }
        let total: u128 = rare_ids
            .iter()
            .map(|id| {
                self.micros_at_first_hit
                    .get(id)
                    .copied()
                    .unwrap_or(self.micros)
            })
            .sum();
        total as f64 / rare_ids.len() as f64
    }

    fn total_hits(&self) -> usize {
        self.hits.values().sum()
    }
}

// ===========================================================================
// One arm of one repetition
// ===========================================================================

fn run_arm(setup: &Setup, arm: Arm, rep: usize) -> Tally {
    let started = Instant::now();
    let block = SEED_BLOCK + rep as u32 * setup.budget as u32;
    let mut tally = Tally::default();
    let mut corpus: Vec<(u32, usize)> = Vec::new();
    // One picker per (arm, repetition), so an arm's continuation choices are a
    // pure function of the repetition and every arm of a repetition draws the
    // same picker stream.
    let mut picker = Rng::new(PICKER_STREAM ^ (rep as u32).wrapping_mul(REP_STRIDE));
    let mut runs = setup.budget;

    if arm == Arm::Prefix {
        // The probes come out of the SAME move budget: each walks `iterations`
        // mutations, which is what a run costs, so the arm gets that many
        // fewer runs. Guidance that has to be paid for is measured paying.
        runs -= setup.probes;
        for index in 0..setup.probes {
            let seed = block + index as u32 + 1;
            let signatures = prologue_signatures(seed, setup.regime.closures(), setup.iterations);
            tally.moves += signatures.len();
            // The EARLIEST prefix carrying each rare bit: a shorter prologue
            // leaves the continuation more of its own budget to explore with.
            let mut taken = 0;
            // Capped at half a run, which is `fuzz_corpus_sweep`'s own entry
            // length: a prologue longer than that leaves a charged continuation
            // with almost no stream of its own.
            for (offset, signature) in signatures.iter().take(setup.iterations / 2).enumerate() {
                let fresh = bits_of(signature) & setup.rare_prefix & !taken;
                if fresh != 0 {
                    taken |= fresh;
                    corpus.push((seed, offset + 1));
                }
            }
        }
    }

    for step in 0..runs {
        let seed = block + step as u32 + 1;
        // Alternate, exactly as `fuzz_corpus_sweep` does: a fresh seed keeps
        // feeding the corpus new material, a corpus entry compounds what is
        // already there, and all-corpus converges on one lineage.
        let prologue = if corpus.is_empty() || step % 2 == 0 {
            None
        } else {
            Some(corpus[picker.below(corpus.len())])
        };
        // A charged continuation gives up as much of its own stream as its
        // prologue replayed, so the run costs `iterations` moves either way.
        let replayed = prologue.map_or(0, |(_, moves)| moves);
        let own = if setup.charge {
            setup.iterations.saturating_sub(replayed).max(1)
        } else {
            setup.iterations
        };
        if replayed > 0 {
            tally.continuations += 1;
            tally.prologue_moves += replayed;
        }
        let plan = plan_for(setup.regime, own, prologue);
        match std::panic::catch_unwind(AssertUnwindSafe(|| run_fuzz(seed, plan))) {
            Ok(outcome) => {
                // Measured BEFORE the tally sees it, because novelty is a
                // property of what came before this run.
                let admit = match arm {
                    Arm::Unguided | Arm::Prefix => false,
                    Arm::Novel => !tally.signatures.contains(outcome.shape().signature),
                    Arm::Rare => bits_of(outcome.shape().signature) & setup.rare_end != 0,
                };
                tally.observe(step, &outcome, started.elapsed().as_micros());
                if admit {
                    tally.admitted += 1;
                    corpus.push((
                        prologue.map_or(seed, |(ancestor, _)| ancestor),
                        setup.iterations / 2,
                    ));
                }
            }
            Err(payload) => tally.observe_failure(&panic_message(&*payload)),
        }
    }
    tally.micros = started.elapsed().as_micros();
    tally.millis = started.elapsed().as_millis();
    tally
}

// ===========================================================================
// The measurement
// ===========================================================================

fn usize_env(name: &str, fallback: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

/// What one regime's calibration measured: which bits are rare over end
/// states, which are rare over prefix states, and which classifier ids are rare
/// per run.
struct Calibration {
    rare_end: u16,
    rare_prefix: u16,
}

/// Calibrate one regime: base rates over end states, base rates over prefix
/// states, and which classifier ids are rare.
///
/// One-time per regime and reused by every repetition, which is how a rarity
/// key would be deployed: rarity is a property of the regime, not of a sweep.
fn calibrate(
    regime: Regime,
    iterations: usize,
    budget: usize,
    probes: usize,
    threshold: usize,
) -> Calibration {
    let seeds: Vec<u32> = (1..=budget as u32)
        .map(|index| CALIB_BLOCK + index)
        .collect();
    let outcomes = run_seed_jobs(&seeds, fuzz_threads(seeds.len()), |&seed| {
        std::panic::catch_unwind(AssertUnwindSafe(|| {
            let outcome = run_fuzz(seed, plan_for(regime, iterations, None));
            let shape = outcome.shape();
            (
                shape.signature.to_string(),
                shape.hits.keys().copied().collect::<Vec<&'static str>>(),
            )
        }))
        .map_err(|payload| panic_message(&*payload))
    });
    let mut ends = BaseRates::default();
    let mut id_runs: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut sampled = 0;
    for outcome in outcomes.into_iter().flatten() {
        let Ok((signature, ids)) = outcome else {
            continue;
        };
        sampled += 1;
        ends.observe(&signature);
        for id in ids {
            *id_runs.entry(id).or_insert(0) += 1;
        }
    }

    let probe_seeds: Vec<u32> = (1..=probes as u32)
        .map(|index| CALIB_BLOCK + 500_000 + index)
        .collect();
    let walks = run_seed_jobs(&probe_seeds, fuzz_threads(probe_seeds.len()), |&seed| {
        std::panic::catch_unwind(AssertUnwindSafe(|| {
            prologue_signatures(seed, regime.closures(), iterations)
        }))
        .map_err(|payload| panic_message(&*payload))
    });
    let mut prefixes = BaseRates::default();
    for walk in walks.into_iter().flatten() {
        for signature in walk.into_iter().flatten() {
            prefixes.observe(&signature);
        }
    }

    println!("calibration ({}):", regime.label());
    ends.print("end");
    prefixes.print("prefix");
    let rare_end = ends.rare_mask(threshold);
    let rare_prefix = prefixes.rare_mask(threshold);
    println!(
        "  rare END bits    (<= {threshold}%): {:?}",
        bit_names(rare_end)
    );
    println!(
        "  rare PREFIX bits (<= {threshold}%): {:?}",
        bit_names(rare_prefix)
    );
    println!(
        "  classifier ids over {sampled} run(s): {:?}",
        id_runs
            .iter()
            .map(|(id, runs)| format!("{id}={runs}"))
            .collect::<Vec<_>>()
    );

    Calibration {
        rare_end,
        rare_prefix,
    }
}

/// RARITY-KEYED corpus admission, measured against the random walk and against
/// the previous experiment's predicate.
///
/// Four arms x `REPS` repetitions x two accountings x two regimes, each arm a
/// job on the pool. The module header carries the numbers this printed and the
/// verdict they support; the knobs are `RARITY_BUDGET` (runs per arm),
/// `RARITY_REPS`, `RARITY_THRESHOLD` (the rarity percentage) and the module-wide
/// `ITERATIONS`.
///
/// ```text
/// STALAGMIGHT_RUST_FUZZ_THREADS=8 STALAGMIGHT_RUST_FUZZ_RARITY_BUDGET=80 \
///   STALAGMIGHT_RUST_FUZZ_RARITY_REPS=6 STALAGMIGHT_RUST_FUZZ_ITERATIONS=60 \
///   TELEMETRY_ENDPOINT=http://localhost/dummy cargo test \
///   --manifest-path src-tauri/Cargo.toml -p stalagmight-core \
///   --lib fuzz_rarity_sweep -- --ignored --nocapture
/// ```
#[test]
#[ignore = "measurement: compares rarity-keyed corpus admission against unguided and novelty"]
fn fuzz_rarity_sweep() {
    let _config = isolated_config();
    let budget = usize_env("STALAGMIGHT_RUST_FUZZ_RARITY_BUDGET", 80);
    let reps = usize_env("STALAGMIGHT_RUST_FUZZ_RARITY_REPS", 3);
    let threshold = usize_env("STALAGMIGHT_RUST_FUZZ_RARITY_THRESHOLD", 25);
    let iterations = usize_env("STALAGMIGHT_RUST_FUZZ_ITERATIONS", GATE_ITERATIONS);
    let probes = (budget / 8).max(1);
    let arms = [Arm::Unguided, Arm::Novel, Arm::Rare, Arm::Prefix];

    println!(
        "rarity sweep: {reps} repetition(s) x {budget} run(s) x {iterations} moves per arm, \
         rarity threshold {threshold}%, {probes} probe(s) for the prefix arm"
    );

    for regime in [Regime::Plain, Regime::Closures] {
        let calibration = calibrate(regime, iterations, budget, probes, threshold);
        let jobs: Vec<(Arm, usize)> = (0..reps)
            .flat_map(|rep| arms.map(|arm| (arm, rep)))
            .collect();
        // Both accountings are run BEFORE either is reported, because the
        // time-to-first-hit target set is pooled over all of them: a rare id
        // has to mean the same thing in every block being compared.
        let blocks: Vec<(bool, Vec<Result<Tally, String>>)> = [true, false]
            .into_iter()
            .map(|charge| {
                let setup = Setup {
                    regime,
                    iterations,
                    budget,
                    charge,
                    probes,
                    rare_end: calibration.rare_end,
                    rare_prefix: calibration.rare_prefix,
                };
                let results = run_seed_jobs(&jobs, fuzz_threads(jobs.len()), |&(arm, rep)| {
                    run_arm(&setup, arm, rep)
                });
                (charge, results)
            })
            .collect();

        let mut pooled: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut pooled_runs = 0;
        for (_, results) in &blocks {
            for tally in results.iter().flatten() {
                pooled_runs += tally.runs;
                for (&id, &runs) in &tally.id_runs {
                    *pooled.entry(id).or_insert(0) += runs;
                }
            }
        }
        let rare_ids: Vec<&'static str> = pooled
            .iter()
            .filter(|(_, &runs)| runs * 100 <= threshold * pooled_runs)
            .map(|(&id, _)| id)
            .collect();
        println!(
            "  pooled classifier incidence over {pooled_runs} run(s) of {}: {:?}",
            regime.label(),
            pooled
                .iter()
                .map(|(id, runs)| format!("{id}={runs}"))
                .collect::<Vec<_>>()
        );
        println!("  RARE classifier ids (<= {threshold}% of pooled runs): {rare_ids:?}");

        for (charge, results) in &blocks {
            let setup = Setup {
                regime,
                iterations,
                budget,
                charge: *charge,
                probes,
                rare_end: calibration.rare_end,
                rare_prefix: calibration.rare_prefix,
            };
            report(&setup, &jobs, results, reps, &rare_ids);
        }
    }
}

/// One `(regime, accounting)` block of the table.
fn report(
    setup: &Setup,
    jobs: &[(Arm, usize)],
    results: &[Result<Tally, String>],
    reps: usize,
    rare_ids: &[&'static str],
) {
    println!(
        "regime {} ({}):",
        setup.regime.label(),
        if setup.charge {
            "prologue CHARGED to the run - equal moves per arm"
        } else {
            "prologue FREE - equal runs per arm, the previous experiment's accounting"
        }
    );
    println!(
        "  {:<10} {:>3} {:>5} {:>4} {:>6} {:>5} {:>9} {:>8} {:>9} {:>8} {:>7} {:>7}",
        "arm",
        "rep",
        "sigs",
        "ids",
        "hits",
        "fails",
        "rare-bits",
        "rare-ids",
        "first-hit",
        "mv-first",
        "ms-first",
        "ms"
    );
    let mut rows: BTreeMap<&'static str, Vec<[f64; 10]>> = BTreeMap::new();
    let mut union: BTreeMap<
        &'static str,
        (BTreeSet<String>, BTreeSet<&'static str>, BTreeSet<String>),
    > = BTreeMap::new();
    for (&(arm, rep), result) in jobs.iter().zip(results) {
        let Ok(tally) = result else {
            println!("  {} rep {rep}: the arm itself panicked", arm.label());
            continue;
        };
        let covered = (tally.bits_seen & setup.rare_end).count_ones() as usize;
        let first = tally.mean_first_hit(rare_ids, setup.budget);
        let reached = rare_ids
            .iter()
            .filter(|id| tally.first_hit.contains_key(*id))
            .count();
        let fails: usize = tally.failures.values().sum();
        let move_first = tally.mean_moves_to_first(rare_ids, setup.budget * setup.iterations);
        let ms_first = tally.mean_micros_to_first(rare_ids) / 1000.0;
        println!(
            "  {:<10} {rep:>3} {:>5} {:>4} {:>6} {:>5} {:>4}/{:<4} {:>4}/{:<3} {:>9.1} \
             {:>8.0} {:>8.0} {:>7}   moves {} replayed {} admit {} cont {}",
            arm.label(),
            tally.signatures.len(),
            tally.hits.len(),
            tally.total_hits(),
            fails,
            covered,
            setup.rare_end.count_ones(),
            reached,
            rare_ids.len(),
            first,
            move_first,
            ms_first,
            tally.millis,
            tally.moves,
            tally.prologue_moves,
            tally.admitted,
            tally.continuations
        );
        rows.entry(arm.label()).or_default().push([
            tally.signatures.len() as f64,
            tally.hits.len() as f64,
            tally.total_hits() as f64,
            fails as f64,
            covered as f64,
            reached as f64,
            first,
            move_first,
            ms_first,
            tally.moves as f64,
        ]);
        let entry = union.entry(arm.label()).or_default();
        entry.0.extend(tally.signatures.iter().cloned());
        entry.1.extend(tally.hits.keys().copied());
        entry.2.extend(tally.failures.keys().cloned());
    }
    println!("  MEANS over {reps} repetition(s):");
    for (label, samples) in &rows {
        let count = samples.len().max(1) as f64;
        let mean = |column: usize| samples.iter().map(|row| row[column]).sum::<f64>() / count;
        println!(
            "  {label:<10} sigs {:>6.1}  ids {:>4.1}  hits {:>6.1}  fails {:>4.1}  rare-bits \
             {:>4.1}  rare-ids {:>4.1}  first-hit {:>6.1}  mv-first {:>8.0}  ms-first {:>8.0}  \
             moves {:>7.1}",
            mean(0),
            mean(1),
            mean(2),
            mean(3),
            mean(4),
            mean(5),
            mean(6),
            mean(7),
            mean(8),
            mean(9),
        );
    }
    // The union over repetitions, which is what "reached at all" means for a
    // sweep somebody would actually run.
    println!("  UNION over repetitions:");
    for (label, (signatures, ids, failures)) in &union {
        println!(
            "  {label:<10} {} distinct signature(s), {} classifier id(s), {} failure signature(s)",
            signatures.len(),
            ids.len(),
            failures.len()
        );
        for failure in failures {
            println!("      failure: {failure}");
        }
    }
    println!("  EARLIEST run index per rare classifier id (`-` is never reached):");
    for (&(arm, rep), result) in jobs.iter().zip(results) {
        let Ok(tally) = result else { continue };
        let detail: Vec<String> = rare_ids
            .iter()
            .map(|id| {
                tally
                    .first_hit
                    .get(id)
                    .map_or_else(|| format!("{id}=-"), |index| format!("{id}={index}"))
            })
            .collect();
        println!("  {} rep {rep}: {detail:?}", arm.label());
    }
}

/// What a corpus entry admitted on a run's END state actually replays.
///
/// The diagnostic the first experiment never ran, and the reason its successor
/// is measured rather than argued about: `Plan::prologue` replays `(seed, n)`
/// as n MUTATIONS of that seed's own stream, while the admission decision was
/// made on the end state of a run that also undid and redid. The two are
/// different trajectories, so the state a corpus entry actually delivers need
/// not carry the bit it was admitted for. This measures how often it does.
///
/// ```text
/// STALAGMIGHT_RUST_FUZZ_RARITY_BUDGET=80 STALAGMIGHT_RUST_FUZZ_ITERATIONS=60 \
///   TELEMETRY_ENDPOINT=http://localhost/dummy cargo test \
///   --manifest-path src-tauri/Cargo.toml -p stalagmight-core \
///   --lib fuzz_rarity_carryover -- --ignored --nocapture
/// ```
#[test]
#[ignore = "measurement: how often a corpus entry replays the bit it was admitted for"]
fn fuzz_rarity_carryover() {
    let _config = isolated_config();
    let budget = usize_env("STALAGMIGHT_RUST_FUZZ_RARITY_BUDGET", 80);
    let threshold = usize_env("STALAGMIGHT_RUST_FUZZ_RARITY_THRESHOLD", 25);
    let iterations = usize_env("STALAGMIGHT_RUST_FUZZ_ITERATIONS", GATE_ITERATIONS);

    for regime in [Regime::Plain, Regime::Closures] {
        let setup = calibrate(regime, iterations, budget, (budget / 8).max(1), threshold);
        let seeds: Vec<u32> = (1..=budget as u32)
            .map(|index| SEED_BLOCK + index)
            .collect();
        let pairs = run_seed_jobs(&seeds, fuzz_threads(seeds.len()), |&seed| {
            std::panic::catch_unwind(AssertUnwindSafe(|| {
                let outcome = run_fuzz(seed, plan_for(regime, iterations, None));
                let admitted = bits_of(outcome.shape().signature) & setup.rare_end;
                // Exactly the state `(seed, iterations / 2)` hands a
                // continuation - `fuzz_corpus_sweep`'s own entry length.
                let walk = prologue_signatures(seed, regime.closures(), iterations / 2);
                let replayed = walk.last().map_or(0, |signature| bits_of(signature));
                (admitted, replayed)
            }))
            .map_err(|payload| panic_message(&*payload))
        });

        let (mut admitted_runs, mut carried, mut lost) = (0, 0, 0);
        let mut per_bit: BTreeMap<&'static str, (usize, usize)> = BTreeMap::new();
        for pair in pairs.into_iter().flatten() {
            let Ok((admitted, replayed)) = pair else {
                continue;
            };
            if admitted == 0 {
                continue;
            }
            admitted_runs += 1;
            for name in bit_names(admitted) {
                per_bit.entry(name).or_insert((0, 0)).0 += 1;
            }
            for name in bit_names(admitted & replayed) {
                per_bit.entry(name).or_insert((0, 0)).1 += 1;
            }
            if admitted & replayed == 0 {
                lost += 1;
            } else {
                carried += 1;
            }
        }
        println!("carry-over ({}):", regime.label());
        println!(
            "  {admitted_runs} admitted run(s): {carried} whose prologue still carries a rare \
             bit, {lost} that carry none"
        );
        for (name, (admits, kept)) in &per_bit {
            println!("    {name:<16} admitted {admits:>3}, replayed {kept:>3}");
        }
    }
}

/// The bit parser reads what `topology_signature` writes.
///
/// Cheap and NOT `#[ignore]`d, because the whole experiment is keyed on a
/// string another module formats: a feature added to that signature, or its
/// letters reordered, would silently turn every bit into zero and the
/// measurement into a comparison of four identical arms.
#[test]
fn the_rarity_bits_read_the_topology_signature() {
    let _config = isolated_config();
    let outcome = run_fuzz(1, plan_for(Regime::Plain, 20, None));
    let signature = outcome.shape().signature.to_string();
    assert!(
        features_of(&signature).is_some(),
        "a real run's topology signature ({signature}) no longer parses as twelve \
         letter-and-digit pairs - see FEATURES"
    );
    assert_eq!(features_of("no-project"), None);
    assert_eq!(features_of("s0e0o0h0m0d0c0y0z0g0k0v0"), Some([0; 12]));
    assert_eq!(bits_of("s0e0o0h0m0d0c0y0z0g0k0v0"), 0);
    // One bit per feature position, checked where it is easy to get wrong: `c`
    // is a bucketed COUNT (so 2 means two closures) and `y`/`v` are flags.
    assert_eq!(
        bit_names(bits_of("s0e1o0h0m0d0c2y1z0g0k0v1")),
        vec![
            "empty-section",
            "loop-closure",
            "closures-2+",
            "cyclic-merges",
            "diverged-ids",
        ]
    );
}
