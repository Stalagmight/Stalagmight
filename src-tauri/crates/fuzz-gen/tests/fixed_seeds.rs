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

use stalagmight_fuzz_gen::oracle::Format;
use stalagmight_fuzz_gen::targets::{run, seed_input, Target};

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

fn run_seeds(target: Target, default_count: u64) {
    if let Ok(only) = std::env::var("STALAGMIGHT_FUZZ_TARGET") {
        if only != target.name() {
            return;
        }
    }
    let start = env_u64("STALAGMIGHT_FUZZ_SEED_START", 0);
    let count = env_u64("STALAGMIGHT_FUZZ_SEEDS", default_count);
    let mut failures = Vec::new();
    let mut parsed = 0u64;
    for seed in start..start.saturating_add(count) {
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
