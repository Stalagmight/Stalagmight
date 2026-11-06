//! Fixed-seed run of every parser fuzz target (stable Rust, part of the normal
//! `cargo test --workspace`): the CI slice of the coverage-guided campaign in
//! `src-tauri/fuzz`. Each target replays a small fixed set of synthetic seeds
//! through the same `targets::run` the fuzzer calls.
//!
//! Long runs on demand (the fuzz farm uses these):
//!   STALAGMIGHT_FUZZ_SEEDS=<count per target>   (default 24)
//!   STALAGMIGHT_FUZZ_SEED_START=<first seed>     (default 0)
//!   STALAGMIGHT_FUZZ_TARGET=<target name>        (run only that target)
//! A failing seed prints its target and seed number; replay it with
//! `STALAGMIGHT_FUZZ_SEED_START=<seed> STALAGMIGHT_FUZZ_SEEDS=1`.

#![expect(clippy::panic, clippy::print_stderr)]

use std::panic::{catch_unwind, AssertUnwindSafe};

use stalagmight_fuzz_gen::fileset::FileSet;
use stalagmight_fuzz_gen::oracle::{self, Depth, Format, Knobs};
use stalagmight_fuzz_gen::targets::{run, seed_input, Target};

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// Seeds that once failed, replayed on every run on top of the fixed range
/// (the bug each one found is named; its own regression test lives with the
/// fix).
fn regression_seeds(target: Target) -> &'static [u64] {
    match target {
        // FZ-PARSE-2: a project setting stored as TEXT that is not UTF-8
        // panicked the load (persistence project_settings_utf8_tests).
        Target::StalxSql => &[147, 283],
        _ => &[],
    }
}

fn run_seeds(target: Target, default_count: u64) {
    if let Ok(only) = std::env::var("STALAGMIGHT_FUZZ_TARGET") {
        if only != target.name() {
            return;
        }
    }
    let start = env_u64("STALAGMIGHT_FUZZ_SEED_START", 0);
    let count = env_u64("STALAGMIGHT_FUZZ_SEEDS", default_count);
    let regressions = regression_seeds(target).iter().copied();
    let (parsed, failures) = replay_seeds(
        target,
        (start..start.saturating_add(count)).chain(regressions),
    );
    eprintln!(
        "{}: {count} seeds from {start}, {parsed} parsed, {} failed",
        target.name(),
        failures.len()
    );
    assert!(
        failures.is_empty(),
        "{} failed on {} seed(s):\n{}",
        target.name(),
        failures.len(),
        failures.join("\n")
    );
}

/// Run `target` on each seed: how many parsed, and one line per failure.
fn replay_seeds(target: Target, seeds: impl Iterator<Item = u64>) -> (u64, Vec<String>) {
    let mut failures = Vec::new();
    let mut parsed = 0u64;
    for seed in seeds {
        let input = match seed_input(target, seed) {
            Ok(input) => input,
            Err(v) => panic!("{}: cannot build seed {seed}: {v}", target.name()),
        };
        match catch_unwind(AssertUnwindSafe(|| run(target, &input))) {
            Ok(Ok(outcome)) => {
                parsed += u64::from(outcome.parsed);
                if std::env::var_os("STALAGMIGHT_FUZZ_VERBOSE").is_some() {
                    let why = outcome.rejected.as_deref().unwrap_or("-");
                    let why: String = why.chars().take(160).collect();
                    eprintln!(
                        "{} seed {seed}: parsed={} converted={} applied={} shots={} rejected={why}",
                        target.name(),
                        outcome.parsed,
                        outcome.converted,
                        outcome.applied,
                        outcome.shots
                    );
                }
            }
            Ok(Err(violation)) => failures.push(format!("seed {seed}: {violation}")),
            Err(payload) => {
                let msg = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
                    .unwrap_or_default();
                failures.push(format!("seed {seed}: panic: {msg}"));
            }
        }
    }
    (parsed, failures)
}

/// Seeds of findings that are still OPEN (reported, not fixed: they need a
/// product decision). Ignored so the CI slice stays green; run them with
/// `cargo test -p stalagmight-fuzz-gen --test fixed_seeds -- --ignored open_findings`.
/// When a finding is fixed, move its seeds to `regression_seeds`.
#[test]
#[ignore = "open findings: fails until FZ-PARSE-5 is fixed"]
fn open_findings() {
    use stalagmight_fuzz_gen::oracle::Format::{Srv, Wpj};
    // FZ-PARSE-5: Walls readings that are huge but finite (1e308) are accepted
    // and the computed station positions overflow to infinity.
    let open: &[(Target, &[u64])] = &[
        (Target::Gen(Srv), &[105, 1149]),
        (Target::Gen(Wpj), &[303]),
        (Target::Raw(Srv), &[1952]),
        (Target::Raw(Wpj), &[1426]),
    ];
    let mut failures = Vec::new();
    for &(target, seeds) in open {
        let (_, f) = replay_seeds(target, seeds.iter().copied());
        failures.extend(
            f.into_iter()
                .map(|line| format!("{}: {line}", target.name())),
        );
    }
    // FZ-PARSE-5, Compass case (minimised from a gen_mak artifact): a length
    // correction near f64::MAX is accepted and applied to every shot, and the
    // computed depths overflow to infinity.
    let survey = "SYNTHETIC CAVE\r\nSURVEY NAME: B1\r\nSURVEY DATE: 2 20 1993\r\n\
                  SURVEY TEAM:\r\nAnn\r\n\
                  DECLINATION: 0.00  CORRECTIONS: 0.00 0.00 5.9e307\r\n\r\n\
                  FROM TO LENGTH BEARING INC LEFT UP DOWN RIGHT\r\n\r\n\
                  B1 B2 1.00 0.00 -60.00 0.00 0.00 0.00 0.00\r\n\
                  B2 B3 1.00 0.00 -60.00 0.00 0.00 0.00 0.00\r\n\
                  B3 B4 1.00 0.00 -60.00 0.00 0.00 0.00 0.00\r\n\
                  B4 B5 1.00 0.00 -60.00 0.00 0.00 0.00 0.00\r\n\
                  B5 B6 1.00 0.00 -60.00 0.00 0.00 0.00 0.00\r\n\
                  B6 B7 1.00 0.00 -60.00 0.00 0.00 0.00 0.00\r\n\
                  B7 B8 1.00 0.00 -60.00 0.00 0.00 0.00 0.00\r\n\
                  B8 B9 1.00 0.00 -60.00 0.00 0.00 0.00 0.00\r\n\
                  B9 B10 1.00 0.00 -60.00 0.00 0.00 0.00 0.00\r\n\x0c\r\n";
    let files = FileSet::single(Format::Dat.entry_name(), survey.as_bytes().to_vec());
    // Knob bit 3: the dialog's "apply corrections" control on.
    match oracle::run(Format::Dat, &files, Knobs(0b1000), Depth::Apply) {
        Err(v) => failures.push(format!("compass correction: {v}")),
        Ok(outcome) => eprintln!("compass correction: {outcome:?}"),
    }
    assert!(
        failures.is_empty(),
        "open findings still fail:\n{}",
        failures.join("\n")
    );
}

/// FZ-PARSE-1 (minimised from the gen_gpx sweep, where it refused about a
/// fifth of well-formed files): a GPS standing still logs the same point
/// twice. The repeat used to become a zero-length shot and the core refused
/// the whole track; it is now dropped and the track imports.
#[test]
fn a_gpx_track_with_a_repeated_point_imports() {
    let gpx = r#"<?xml version="1.0"?><gpx version="1.1"><trk><name>T</name><trkseg>
        <trkpt lat="20.5" lon="-87.3"><ele>10</ele></trkpt>
        <trkpt lat="20.5001" lon="-87.3"><ele>10</ele></trkpt>
        <trkpt lat="20.5001" lon="-87.3"><ele>10</ele></trkpt>
        </trkseg></trk></gpx>"#;
    let files = FileSet::single(Format::Gpx.entry_name(), gpx.as_bytes().to_vec());
    // Knobs 0: waypoints, tracks and routes all imported, depth not inverted.
    let outcome = match oracle::run(Format::Gpx, &files, Knobs(0), Depth::Apply) {
        Ok(outcome) => outcome,
        Err(v) => panic!("violation: {v}"),
    };
    eprintln!("repeated GPX point: {outcome:?}");
    assert!(outcome.applied, "the track must import: {outcome:?}");
    assert_eq!(outcome.shots, 1, "3 points, 1 repeat: 1 shot");
}

macro_rules! seed_tests {
    ($($name:ident => $target:expr, $count:expr;)*) => {
        $(
            #[test]
            fn $name() {
                run_seeds($target, $count);
            }
        )*
    };
}

seed_tests! {
    raw_dmp => Target::Raw(Format::Dmp), 24;
    raw_tml => Target::Raw(Format::Tml), 24;
    raw_tmlu => Target::Raw(Format::Tmlu), 24;
    raw_srv => Target::Raw(Format::Srv), 24;
    raw_wpj => Target::Raw(Format::Wpj), 24;
    raw_dat => Target::Raw(Format::Dat), 24;
    raw_mak => Target::Raw(Format::Mak), 24;
    raw_gpx => Target::Raw(Format::Gpx), 24;
    gen_dmp => Target::Gen(Format::Dmp), 24;
    gen_tml => Target::Gen(Format::Tml), 24;
    gen_tmlu => Target::Gen(Format::Tmlu), 24;
    gen_srv => Target::Gen(Format::Srv), 24;
    gen_wpj => Target::Gen(Format::Wpj), 24;
    gen_dat => Target::Gen(Format::Dat), 24;
    gen_mak => Target::Gen(Format::Mak), 24;
    gen_gpx => Target::Gen(Format::Gpx), 24;
    stalx_sql => Target::StalxSql, 24;
    stalx_raw => Target::StalxRaw, 4;
}

#[test]
fn every_target_has_a_fixed_seed_test() {
    // Keep the macro list above in step with `Target::all()`.
    let listed = 18;
    assert_eq!(
        Target::all().len(),
        listed,
        "add the new target to seed_tests!"
    );
}

/// Replay one input file (a cargo-fuzz crash artifact or a corpus entry) on
/// stable: `STALAGMIGHT_FUZZ_REPLAY=<target>:<file>`. A no-op without the
/// variable.
#[test]
fn replay() {
    let Ok(spec) = std::env::var("STALAGMIGHT_FUZZ_REPLAY") else {
        return;
    };
    let Some((name, file)) = spec.split_once(':') else {
        panic!("STALAGMIGHT_FUZZ_REPLAY must be <target>:<file>, got {spec}");
    };
    let Some(target) = Target::from_name(name) else {
        panic!("unknown target {name}");
    };
    let data = std::fs::read(file).unwrap_or_else(|e| panic!("read {file}: {e}"));
    match run(target, &data) {
        Ok(outcome) => eprintln!("{name} {file}: {outcome:?}"),
        Err(violation) => panic!("{name} {file}: {violation}"),
    }
}
