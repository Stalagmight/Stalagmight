//! RARITY-KEYED corpus admission, and the second measurement saying a corpus
//! is not what the undo fuzzer is short of.
//!
//! # The question
//!
//! `fuzz_corpus_sweep` (fuzz_undo_tests.rs) measured corpus guidance keyed on
//! whole-signature NOVELTY and found it worth nothing: nearly every random
//! run lands on a novel signature, so the predicate admits nearly everything
//! and "guided" only means half the budget replaying a prefix. The named
//! alternative was admission keyed on a RARE structural bit. This module
//! measures it against the random walk and against novelty. Nothing here is
//! wired into any gate.
//!
//! # What a bit is
//!
//! One `topology_signature` feature at or above a bucket threshold ([`BITS`]);
//! RARE means "seen on at most `RARITY_THRESHOLD` percent of a calibration
//! sample". On the station graph the features are lines, empty and one-shot
//! lines, shots, junctions and the most lines at one junction, closures and
//! migrated `Tie`s, datums, placements, commented stations and aliases.
//!
//! RETIRED with the old model (their machinery is gone): `merge-edge`,
//! `merges-3+`, `merge-star` (merge records; their station-graph forms are
//! `junction`, `junctions-3+`, `junction-star`), `cyclic-merges` (a merge
//! cycle is impossible on a forest; its migrated form is the `tie` bit),
//! `line-offset` (stored offsets; the datum-less pieces' anchors are the
//! `placements-3+` bit) and `diverged-ids` (session ids are not part of the
//! survey's shape).
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
//! `prefix` exists because a prologue replays `(seed, n)` as n MUTATIONS,
//! while an admitted run also undid and redid: the state an entry delivers is
//! not the one admitted (`fuzz_rarity_carryover` measures the gap). Its
//! probes are paid for out of the same budget, and a continuation's own
//! stream is CHARGED for its prologue (every run costs `iterations` moves) -
//! both accountings are reported.
//!
//! # The record
//!
//! On the old model, while the #413 findings were open, six repetitions of
//! 80 runs x 60 moves per arm gave (distinct signatures, then time to the
//! regime's rare classified findings in runs / moves / ms):
//!
//! | regime | arm | sigs | first (runs) | first (moves) | first (ms) |
//! |---|---|---|---|---|---|
//! | plain | unguided | **73.5** | 38.4 | 3071 | **7770** |
//! | plain | prefix | 62.3 | **28.8** | **2832** | 8968 |
//! | closures | unguided | 60.5 | 40.1 | 2515 | **7409** |
//! | closures | prefix | 57.5 | **23.7** | **2079** | 7706 |
//!
//! Counted in runs `prefix` led by 41%, in moves by 17%, in wall time not at
//! all; on distinct signatures guidance lost outright. The verdict: the
//! random walk is not this tier's bottleneck, and a shape worth having is
//! cheaper to CONSTRUCT (`Plan::closure_shapes`, `Plan::divergence`,
//! `Plan::legacy_origin`) than to rediscover. There are no classifiers any
//! more, so the port scores time to each rare BIT instead (`bit-first`): how
//! soon an arm reaches the structure guidance was meant to compound.
//!
//! # Re-running it
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
//! Every column but the wall-clock one is a pure function of the knobs.
//! Everything is generated in code.

#![expect(
    clippy::print_stdout,
    reason = "a measurement's output IS its deliverable: the per-repetition table is what a \
              reviewer reads to check the verdict"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::panic::AssertUnwindSafe;
use std::time::Instant;

use super::fuzz_undo_tests::{
    env_number, failure_signature, fuzz_threads, panic_message, prologue_signatures, run_fuzz,
    run_seed_jobs, Outcome, Plan, Rng, GATE_ITERATIONS, SIGNATURE_FEATURES,
};
use crate::test_helpers::isolated_config;

// ===========================================================================
// The bits
// ===========================================================================

/// A structural BIT: one signature feature at or above a bucket value
/// (`bucket` maps 0, 1, 2, 3..=4, 5..=8, 9..=16, 17..=32, more to 0..=7).
struct BitSpec {
    name: &'static str,
    /// Index into [`SIGNATURE_FEATURES`].
    feature: usize,
    least: u8,
}

const BITS: [BitSpec; 13] = [
    BitSpec {
        name: "empty-line",
        feature: 1,
        least: 1,
    },
    BitSpec {
        name: "one-shot-line",
        feature: 2,
        least: 1,
    },
    BitSpec {
        name: "shots-17+",
        feature: 3,
        least: 6,
    },
    BitSpec {
        name: "junction",
        feature: 4,
        least: 1,
    },
    BitSpec {
        name: "junctions-3+",
        feature: 4,
        least: 3,
    },
    BitSpec {
        name: "junction-star",
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
        name: "tie",
        feature: 7,
        least: 1,
    },
    BitSpec {
        name: "datums-2+",
        feature: 8,
        least: 2,
    },
    BitSpec {
        name: "placements-3+",
        feature: 9,
        least: 3,
    },
    BitSpec {
        name: "station-comment",
        feature: 10,
        least: 1,
    },
    BitSpec {
        name: "aliases",
        feature: 11,
        least: 1,
    },
];

/// The twelve bucket values behind a signature, or `None` when the string is
/// not one (`no-project`).
fn features_of(signature: &str) -> Option<[u8; 12]> {
    let bytes = signature.as_bytes();
    if bytes.len() != SIGNATURE_FEATURES.len() * 2 {
        return None;
    }
    let mut values = [0_u8; 12];
    for (index, &letter) in SIGNATURE_FEATURES.iter().enumerate() {
        if bytes.get(index * 2) != Some(&letter) {
            return None;
        }
        let digit = *bytes.get(index * 2 + 1)?;
        if !digit.is_ascii_digit() {
            return None;
        }
        values[index] = digit - b'0';
    }
    Some(values)
}

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

    /// The bits seen at least once and on at most `threshold` percent.
    fn rare_mask(&self, threshold: usize) -> u16 {
        let mut mask = 0;
        for index in 0..BITS.len() {
            if self.counts[index] > 0 && self.rate(index) <= threshold as f64 / 100.0 {
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Arm {
    Unguided,
    Novel,
    Rare,
    Prefix,
}

impl Arm {
    const fn label(self) -> &'static str {
        match self {
            Self::Unguided => "unguided",
            Self::Novel => "novel",
            Self::Rare => "rare",
            Self::Prefix => "prefix",
        }
    }
}

/// Seed blocks far from every gate and documented seed.
const SEED_BLOCK: u32 = 100_000;
const CALIB_BLOCK: u32 = 900_000;
/// The prologue picker's stream: the only randomness this adds.
const PICKER_STREAM: u32 = 0x00CA_11B0;
const REP_STRIDE: u32 = 0x9E37_79B9;

fn plan_for(regime: Regime, iterations: usize, prologue: Option<(u32, usize)>) -> Plan {
    let mut plan = Plan::plain(iterations);
    plan.closures = regime.closures();
    plan.prologue = prologue;
    plan
}

struct Setup {
    regime: Regime,
    iterations: usize,
    budget: usize,
    /// Is a prologue CHARGED to the run that replays it?
    charge: bool,
    probes: usize,
    rare_end: u16,
    rare_prefix: u16,
}

// ===========================================================================
// What a sweep is scored on
// ===========================================================================

#[derive(Default)]
struct Tally {
    runs: usize,
    moves: usize,
    signatures: BTreeSet<String>,
    failures: BTreeMap<String, usize>,
    bits_seen: u16,
    /// Run index and moves consumed when each bit was first seen.
    first_bit: BTreeMap<usize, (usize, usize)>,
    admitted: usize,
    continuations: usize,
    prologue_moves: usize,
    millis: u128,
}

impl Tally {
    fn observe(&mut self, index: usize, outcome: &Outcome) {
        let shape = outcome.shape();
        self.runs += 1;
        self.moves += shape.mutations + shape.undos + shape.redos;
        self.signatures.insert(shape.signature.to_string());
        let bits = bits_of(shape.signature);
        self.bits_seen |= bits;
        for bit in 0..BITS.len() {
            if bits & (1 << bit) != 0 {
                self.first_bit.entry(bit).or_insert((index, self.moves));
            }
        }
    }

    fn observe_failure(&mut self, message: &str) {
        self.runs += 1;
        *self.failures.entry(failure_signature(message)).or_insert(0) += 1;
    }

    /// Mean run index (and moves) to the first sight of each `rare` bit,
    /// counting a miss as the whole budget.
    fn mean_first(&self, rare: u16, budget: usize, censor: usize) -> (f64, f64) {
        let bits: Vec<usize> = (0..BITS.len()).filter(|b| rare & (1 << b) != 0).collect();
        if bits.is_empty() {
            return (0.0, 0.0);
        }
        let (runs, moves) = bits.iter().fold((0, 0), |(r, m), bit| {
            let (run, mv) = self.first_bit.get(bit).copied().unwrap_or((budget, censor));
            (r + run, m + mv)
        });
        (
            runs as f64 / bits.len() as f64,
            moves as f64 / bits.len() as f64,
        )
    }
}

fn run_arm(setup: &Setup, arm: Arm, rep: usize) -> Tally {
    let started = Instant::now();
    let block = SEED_BLOCK + rep as u32 * setup.budget as u32;
    let mut tally = Tally::default();
    let mut corpus: Vec<(u32, usize)> = Vec::new();
    let mut picker = Rng::new(PICKER_STREAM ^ (rep as u32).wrapping_mul(REP_STRIDE));
    let mut runs = setup.budget;
    if arm == Arm::Prefix {
        runs -= setup.probes;
        for index in 0..setup.probes {
            let seed = block + index as u32 + 1;
            let signatures = prologue_signatures(seed, setup.regime.closures(), setup.iterations);
            tally.moves += signatures.len();
            let mut taken = 0;
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
        let prologue = if corpus.is_empty() || step % 2 == 0 {
            None
        } else {
            Some(corpus[picker.below(corpus.len())])
        };
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
                let admit = match arm {
                    Arm::Unguided | Arm::Prefix => false,
                    Arm::Novel => !tally.signatures.contains(outcome.shape().signature),
                    Arm::Rare => bits_of(outcome.shape().signature) & setup.rare_end != 0,
                };
                tally.observe(step, &outcome);
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
    tally.millis = started.elapsed().as_millis();
    tally
}

struct Calibration {
    rare_end: u16,
    rare_prefix: u16,
}

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
    let ends = run_seed_jobs(&seeds, fuzz_threads(seeds.len()), |&seed| {
        run_fuzz(seed, plan_for(regime, iterations, None)).signature
    });
    let mut end_rates = BaseRates::default();
    for signature in ends.into_iter().flatten() {
        end_rates.observe(&signature);
    }
    let probe_seeds: Vec<u32> = (1..=probes as u32)
        .map(|index| CALIB_BLOCK + 500_000 + index)
        .collect();
    let walks = run_seed_jobs(&probe_seeds, fuzz_threads(probe_seeds.len()), |&seed| {
        prologue_signatures(seed, regime.closures(), iterations)
    });
    let mut prefix_rates = BaseRates::default();
    for walk in walks.into_iter().flatten() {
        for signature in walk {
            prefix_rates.observe(&signature);
        }
    }
    println!("calibration ({}):", regime.label());
    end_rates.print("end");
    prefix_rates.print("prefix");
    let rare_end = end_rates.rare_mask(threshold);
    let rare_prefix = prefix_rates.rare_mask(threshold);
    println!("  rare END bits    (<= {threshold}%): {:?}", bit_names(rare_end));
    println!(
        "  rare PREFIX bits (<= {threshold}%): {:?}",
        bit_names(rare_prefix)
    );
    Calibration {
        rare_end,
        rare_prefix,
    }
}

/// RARITY-KEYED corpus admission against the random walk and novelty: four
/// arms x `RARITY_REPS` repetitions x two accountings x two regimes.
#[test]
#[ignore = "measurement: compares rarity-keyed corpus admission against unguided and novelty"]
fn fuzz_rarity_sweep() {
    let _config = isolated_config();
    let budget: usize = env_number("RARITY_BUDGET", 80);
    let reps: usize = env_number("RARITY_REPS", 3);
    let threshold: usize = env_number("RARITY_THRESHOLD", 25);
    let iterations: usize = env_number("ITERATIONS", GATE_ITERATIONS);
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
        for charge in [true, false] {
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
            report(&setup, &jobs, &results);
        }
    }
}

fn report(setup: &Setup, jobs: &[(Arm, usize)], results: &[Result<Tally, String>]) {
    println!(
        "regime {} ({}):",
        setup.regime.label(),
        if setup.charge {
            "prologue CHARGED - equal moves per arm"
        } else {
            "prologue FREE - equal runs per arm"
        }
    );
    println!(
        "  {:<9} {:>3} {:>5} {:>5} {:>9} {:>9} {:>9} {:>7} {:>7}",
        "arm", "rep", "sigs", "fails", "rare-bits", "bit-first", "mv-first", "moves", "ms"
    );
    let rare = setup.rare_end;
    let mut means: BTreeMap<&'static str, Vec<[f64; 4]>> = BTreeMap::new();
    for (&(arm, rep), result) in jobs.iter().zip(results) {
        let Ok(tally) = result else {
            println!("  {} rep {rep}: the arm itself panicked", arm.label());
            continue;
        };
        let covered = (tally.bits_seen & rare).count_ones();
        let fails: usize = tally.failures.values().sum();
        let (first, move_first) =
            tally.mean_first(rare, setup.budget, setup.budget * setup.iterations);
        println!(
            "  {:<9} {rep:>3} {:>5} {:>5} {:>4}/{:<4} {:>9.1} {:>9.0} {:>7} {:>7}   admit {} \
             cont {} replayed {}",
            arm.label(),
            tally.signatures.len(),
            fails,
            covered,
            rare.count_ones(),
            first,
            move_first,
            tally.moves,
            tally.millis,
            tally.admitted,
            tally.continuations,
            tally.prologue_moves
        );
        for failure in tally.failures.keys() {
            println!("      failure: {failure}");
        }
        means.entry(arm.label()).or_default().push([
            tally.signatures.len() as f64,
            first,
            move_first,
            tally.millis as f64,
        ]);
    }
    for (label, rows) in &means {
        let n = rows.len().max(1) as f64;
        let mean = |c: usize| rows.iter().map(|r| r[c]).sum::<f64>() / n;
        println!(
            "  MEAN {label:<9} sigs {:>6.1}  bit-first {:>6.1}  mv-first {:>8.0}  ms {:>8.0}",
            mean(0),
            mean(1),
            mean(2),
            mean(3)
        );
    }
}

/// How often a corpus entry admitted on a run's END state replays the bit it
/// was admitted for (the prologue walks a mutate-only trajectory).
#[test]
#[ignore = "measurement: how often a corpus entry replays the bit it was admitted for"]
fn fuzz_rarity_carryover() {
    let _config = isolated_config();
    let budget: usize = env_number("RARITY_BUDGET", 80);
    let threshold: usize = env_number("RARITY_THRESHOLD", 25);
    let iterations: usize = env_number("ITERATIONS", GATE_ITERATIONS);
    for regime in [Regime::Plain, Regime::Closures] {
        let setup = calibrate(regime, iterations, budget, (budget / 8).max(1), threshold);
        let seeds: Vec<u32> = (1..=budget as u32)
            .map(|index| SEED_BLOCK + index)
            .collect();
        let pairs = run_seed_jobs(&seeds, fuzz_threads(seeds.len()), |&seed| {
            let outcome = run_fuzz(seed, plan_for(regime, iterations, None));
            let admitted = bits_of(&outcome.signature) & setup.rare_end;
            let walk = prologue_signatures(seed, regime.closures(), iterations / 2);
            let replayed = walk.last().map_or(0, |signature| bits_of(signature));
            (admitted, replayed)
        });
        let (mut admitted_runs, mut carried, mut lost) = (0, 0, 0);
        let mut per_bit: BTreeMap<&'static str, (usize, usize)> = BTreeMap::new();
        for (admitted, replayed) in pairs.into_iter().flatten() {
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

/// The bit parser reads what `topology_signature` writes. Cheap and NOT
/// ignored: a feature added to that signature, or its letters reordered,
/// would silently turn every bit into zero.
#[test]
fn the_rarity_bits_read_the_topology_signature() {
    let _config = isolated_config();
    let outcome = run_fuzz(1, plan_for(Regime::Closures, 20, None));
    let signature = outcome.shape().signature.to_string();
    assert!(
        features_of(&signature).is_some(),
        "a real run's topology signature ({signature}) no longer parses as twelve \
         letter-and-digit pairs - see SIGNATURE_FEATURES"
    );
    assert!(
        bits_of(&signature) != 0,
        "a real run's signature carries no bit at all: {signature}"
    );
    assert_eq!(features_of("no-project"), None);
    assert_eq!(features_of("s0e0o0h0j0d0c0t0z0p0k0a0"), Some([0; 12]));
    assert_eq!(bits_of("s0e0o0h0j0d0c0t0z0p0k0a0"), 0);
    assert_eq!(
        bit_names(bits_of("s0e1o0h0j0d3c2t1z0p0k0a1")),
        vec![
            "empty-line",
            "junction-star",
            "loop-closure",
            "closures-2+",
            "tie",
            "aliases",
        ]
    );
}
