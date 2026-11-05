//! [#576 fuzzers, FZ-SDB] A stateful SpeleoDB protocol fuzzer.
//!
//! Drives the app's real SpeleoDB verbs (the `AppState::speleodb_*` methods the
//! IPC commands wrap, in the order the frontend calls them) against a REAL
//! SpeleoDB server, in random sequences interleaved with network faults and a
//! second client's actions, from many seeds, and checks the invariants listed in
//! [`oracle`] after every step.
//!
//! ```text
//!   AppState (owner) --> fault proxy (in-process, 127.0.0.1) --> SpeleoDB instance
//!   second client (collaborator / another session of the owner) ----^
//!   oracle (superuser: locks, history, every commit's content) ------^
//! ```
//!
//! It only ever runs against a private test instance: everything is env-gated
//! and refuses to start unless the URL is a loopback IP literal and every
//! account is under the reserved `.invalid` domain ([`target`]). No hostname,
//! address or credential lives in this repo. See docs/speleodb-dev-testing.md,
//! "Protocol fuzzer", for the env vars and how to run a campaign.
//!
//! Offline (normal `cargo test`): the allowlist, the generator, the proxy, the
//! other-server stub and the raw-HTTP helper have unit tests; the campaign,
//! replay and regression-seed entry points below are `#[ignore]` and fail
//! naming the missing variable when run on purpose without an instance.

mod app;
mod model;
mod oracle;
mod proxy;
mod run;
mod stub;
mod target;
mod wire;
mod world;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use model::{Rng, Step};
use run::{shrink, Harness, Source};

/// Seeds that once failed, replayed by [`speleodb_fuzz_regression_seeds`].
/// Each entry: (seed, steps, fault percent).
const REGRESSION_SEEDS: &[(u64, usize, u64)] = &[];

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_owned())
}

fn parse_range(text: &str) -> std::ops::Range<u64> {
    let mut parts = text.split("..");
    let a = parts
        .next()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(1);
    let b = parts
        .next()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(a + 1);
    a..b
}

/// The folders a run uses: the app's dirs (process-wide, set once, before any
/// `AppState`) and the trace dir. Refuses when this process's app dirs were
/// already set elsewhere (another test ran first in this process).
fn harness() -> Harness {
    let target = target::target_from_env().unwrap_or_else(|e| panic!("FZ-SDB: {e}"));
    let base = PathBuf::from(env_or(
        "STALAGMIGHT_SPELEODB_FUZZ_DIR",
        &std::env::temp_dir()
            .join(format!("stalagmight-sdb-fuzz-{}", std::process::id()))
            .to_string_lossy(),
    ));
    let cache = base.join("app/cache");
    crate::app_dirs::init_paths(
        base.join("app/data"),
        base.join("app/config"),
        cache.clone(),
        Some(base.join("app/documents")),
    );
    assert_eq!(
        crate::app_dirs::cache_dir(),
        Some(cache.as_path()),
        "FZ-SDB: the app dirs of this process were set before the fuzzer; run it alone (--exact)"
    );
    let server = world::Server::connect(&target).unwrap_or_else(|e| panic!("FZ-SDB: sign-in: {e}"));
    Harness {
        target,
        server,
        cache_root: cache.join("speleodb"),
        config_dir: base.join("app/config"),
        copies: base.join("copies"),
    }
}

fn trace_dir() -> PathBuf {
    let dir = PathBuf::from(env_or(
        "STALAGMIGHT_SPELEODB_FUZZ_TRACE_DIR",
        &std::env::temp_dir()
            .join("stalagmight-sdb-fuzz-traces")
            .to_string_lossy(),
    ));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn save_steps(path: &Path, steps: &[Step]) {
    if let Ok(json) = serde_json::to_string_pretty(steps) {
        let _ = std::fs::write(path, json);
    }
}

/// Shrink a failure, write the shrunk steps and their trace; returns the path.
fn shrink_and_save(
    h: &mut Harness,
    dir: &Path,
    label: &str,
    steps: Vec<Step>,
    id: &str,
) -> PathBuf {
    let budget: usize = env_or("STALAGMIGHT_SPELEODB_FUZZ_SHRINK_BUDGET", "40")
        .parse()
        .unwrap_or(40);
    let mut log = |line: &str| println!("FZ-SDB {label}: {line}");
    let shrunk = shrink(h, steps, id, budget, &mut log);
    let path = dir.join(format!("{label}-shrunk.json"));
    save_steps(&path, &shrunk);
    let trace = dir.join(format!("{label}-shrunk.jsonl"));
    let _ = std::fs::remove_file(&trace);
    let replay = h.run(
        &format!("{label}-shrunk"),
        Source::Fixed(shrunk.clone()),
        shrunk.len(),
        Some(&trace),
    );
    println!(
        "FZ-SDB {label}: shrunk to {} steps -> {} (replay {})",
        shrunk.len(),
        path.display(),
        if replay.failure.is_some() {
            "fails"
        } else {
            "PASSES: flaky"
        }
    );
    path
}

/// The long campaign: `STALAGMIGHT_SPELEODB_FUZZ_SEEDS=a..b` (default 1..4),
/// `_STEPS` per seed (40), `_FAULT_PCT` (25), `_MINUTES` (stop starting seeds
/// after this long), `_SHRINK=0` to skip shrinking, `_SHRINK_BUDGET` (40).
#[test]
#[ignore = "env-gated: needs a private SpeleoDB instance (STALAGMIGHT_SPELEODB_FUZZ_URL etc.)"]
fn speleodb_fuzz_campaign() {
    let mut h = harness();
    let seeds = parse_range(&env_or("STALAGMIGHT_SPELEODB_FUZZ_SEEDS", "1..4"));
    let steps: usize = env_or("STALAGMIGHT_SPELEODB_FUZZ_STEPS", "40")
        .parse()
        .unwrap_or(40);
    let fault_pct: u64 = env_or("STALAGMIGHT_SPELEODB_FUZZ_FAULT_PCT", "25")
        .parse()
        .unwrap_or(25);
    let minutes: u64 = env_or("STALAGMIGHT_SPELEODB_FUZZ_MINUTES", "0")
        .parse()
        .unwrap_or(0);
    let do_shrink = env_or("STALAGMIGHT_SPELEODB_FUZZ_SHRINK", "1") != "0";
    let dir = trace_dir();
    let started = Instant::now();
    let mut failed = Vec::new();
    for seed in seeds {
        if minutes > 0 && started.elapsed() > Duration::from_secs(minutes * 60) {
            println!("FZ-SDB: time budget reached before seed {seed}");
            break;
        }
        let label = format!("s{seed}");
        let trace = dir.join(format!("{label}.jsonl"));
        let _ = std::fs::remove_file(&trace);
        let source = Source::Seed {
            rng: Rng::new(seed),
            fault_pct,
        };
        let r = h.run(&label, source, steps, Some(&trace));
        save_steps(&dir.join(format!("{label}.json")), &r.steps);
        if let Some(e) = &r.setup_error {
            println!("FZ-SDB seed {seed}: SETUP ERROR {e}");
            continue;
        }
        match r.failure {
            None => {
                println!(
                    "FZ-SDB seed {seed}: ok ({} steps, {:.1}s)",
                    r.steps.len(),
                    r.elapsed.as_secs_f64()
                );
                let _ = std::fs::remove_file(&trace);
                let _ = std::fs::remove_file(dir.join(format!("{label}.json")));
            }
            Some((i, v)) => {
                let first = v.first().map(|x| x.id.clone()).unwrap_or_default();
                println!(
                    "FZ-SDB seed {seed}: FAIL at step {i}: {}",
                    serde_json::to_string(&v).unwrap_or_default()
                );
                let shrunk = do_shrink
                    .then(|| shrink_and_save(&mut h, &dir, &label, r.steps.clone(), &first));
                failed.push((seed, first, trace.clone(), shrunk));
            }
        }
    }
    for (seed, id, trace, shrunk) in &failed {
        println!(
            "FZ-SDB failing seed {seed} [{id}] trace {} shrunk {:?}",
            trace.display(),
            shrunk
        );
    }
    assert!(
        failed.is_empty(),
        "FZ-SDB: {} seed(s) broke an invariant",
        failed.len()
    );
}

/// Replay a saved step list (`STALAGMIGHT_SPELEODB_FUZZ_REPLAY=<steps.json>`);
/// `_SHRINK=1` shrinks it against the first violation it shows.
#[test]
#[ignore = "env-gated: needs a private SpeleoDB instance and STALAGMIGHT_SPELEODB_FUZZ_REPLAY"]
fn speleodb_fuzz_replay() {
    let path = PathBuf::from(
        std::env::var("STALAGMIGHT_SPELEODB_FUZZ_REPLAY")
            .unwrap_or_else(|_| panic!("FZ-SDB: STALAGMIGHT_SPELEODB_FUZZ_REPLAY is not set")),
    );
    let steps: Vec<Step> = serde_json::from_slice(&std::fs::read(&path).expect("read the steps"))
        .expect("parse the steps");
    let mut h = harness();
    let dir = trace_dir();
    let label = path
        .file_stem()
        .map_or_else(|| "replay".to_owned(), |s| s.to_string_lossy().into_owned());
    let trace = dir.join(format!("{label}-replay.jsonl"));
    let _ = std::fs::remove_file(&trace);
    let r = h.run(
        &format!("{label}-replay"),
        Source::Fixed(steps.clone()),
        steps.len(),
        Some(&trace),
    );
    println!(
        "FZ-SDB replay {}: trace {}",
        path.display(),
        trace.display()
    );
    if let Some((i, v)) = r.failure {
        println!(
            "FZ-SDB replay: FAIL at step {i}: {}",
            serde_json::to_string(&v).unwrap_or_default()
        );
        if env_or("STALAGMIGHT_SPELEODB_FUZZ_SHRINK", "0") == "1" {
            let first = v.first().map(|x| x.id.clone()).unwrap_or_default();
            shrink_and_save(&mut h, &dir, &label, r.steps, &first);
        }
        panic!("FZ-SDB replay broke an invariant");
    }
}

/// Every seed that once failed, re-run (on the instance only).
#[test]
#[ignore = "env-gated: needs a private SpeleoDB instance (STALAGMIGHT_SPELEODB_FUZZ_URL etc.)"]
fn speleodb_fuzz_regression_seeds() {
    let mut h = harness();
    let dir = trace_dir();
    let mut failed = Vec::new();
    for &(seed, steps, fault_pct) in REGRESSION_SEEDS {
        let label = format!("reg{seed}");
        let trace = dir.join(format!("{label}.jsonl"));
        let _ = std::fs::remove_file(&trace);
        let r = h.run(
            &label,
            Source::Seed {
                rng: Rng::new(seed),
                fault_pct,
            },
            steps,
            Some(&trace),
        );
        if let Some((i, v)) = r.failure {
            failed.push(format!(
                "seed {seed} step {i}: {}",
                serde_json::to_string(&v).unwrap_or_default()
            ));
        }
    }
    assert!(
        failed.is_empty(),
        "FZ-SDB regression seeds failed: {failed:?}"
    );
}
