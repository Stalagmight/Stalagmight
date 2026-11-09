//! [FZ-SDB] A stateful SpeleoDB protocol fuzzer.
//!
//! Drives the app's real SpeleoDB verbs (the `AppState::speleodb_*` methods the
//! IPC commands wrap, in the order the frontend calls them) against a REAL
//! SpeleoDB server, in random sequences interleaved with network faults, a
//! second client's actions and two app verbs racing each other, from many
//! seeds, and checks the invariants listed in [`oracle`] after every step.
//!
//! ```text
//!   AppState (owner) --> fault proxy (in-process, 127.0.0.1) --> SpeleoDB instance
//!   second client (collaborator / another session of the owner) ----^
//!   oracle (superuser + each account: locks, history, every commit) -^
//! ```
//!
//! It only ever runs against a private test instance: everything is env-gated
//! and refuses to start unless the URL is a loopback IP literal and every
//! account is under the reserved `.invalid` domain ([`target`]). No hostname,
//! address or credential lives in this repo. See docs/speleodb-dev-testing.md,
//! "Protocol fuzzer", for the env vars and how to run a campaign.
//!
//! Offline (normal `cargo test`): the allowlist, the generator, the proxy, the
//! other-server stub, the raw-HTTP helper and parts of the oracle have unit
//! tests; the campaign, replay, regression-seed and seed-writer entry points
//! below are `#[ignore]` and fail naming the missing variable when run on
//! purpose without one.

mod app;
mod data;
mod flows;
mod gen;
mod history;
mod interleave;
mod model;
mod oracle;
mod oracle_data;
mod oracle_lock;
mod oracle_proto;
#[cfg(test)]
mod oracle_tests;
mod proxy;
mod run;
mod snap;
mod stub;
mod target;
mod wire;
mod world;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use model::{Rng, Step};
use run::{shrink, Harness, Source};

/// Seeds that once failed, replayed by [`speleodb_fuzz_regression_seeds`].
/// Each entry: (seed, steps, fault percent). The list of the pre-port fuzzer
/// named seeds of another generator, so it starts empty.
const REGRESSION_SEEDS: &[(u64, usize, u64)] = &[];

/// Directed step lists, replayed by [`speleodb_fuzz_regression_seeds`] beside the
/// seeds: shapes the generator draws rarely or cannot draw.
fn directed_steps() -> Vec<(&'static str, Vec<Step>)> {
    vec![
        ("same-account-overwrite", same_account_overwrite_steps()),
        ("saved-unpushed-close", saved_unpushed_close_steps()),
        ("crash-residue-reopen", crash_residue_reopen_steps()),
    ]
}

/// [R6] A crash mid-save leaves a hot journal beside the cache copy; reopening
/// from SpeleoDB installs the server copy next to it, and the next open must
/// still read exactly the server revision.
fn crash_residue_reopen_steps() -> Vec<Step> {
    use model::{Acct, Act};
    vec![
        Step::plain(Act::Login { acct: Acct::Owner }),
        Step::plain(Act::Open {
            p: 0,
            take_server: false,
        }),
        Step::plain(Act::Edit),
        Step::plain(Act::Autosave),
        Step::plain(Act::Crash { residue: true }),
        Step::plain(Act::Open {
            p: 0,
            take_server: true,
        }),
        Step::plain(Act::Refresh),
    ]
}

/// [R3] Another session of the app's own account pushes while the app edits;
/// the app's next upload must not supersede that revision without a prompt.
fn same_account_overwrite_steps() -> Vec<Step> {
    use model::{Acct, Act};
    vec![
        Step::plain(Act::Login { acct: Acct::Owner }),
        Step::plain(Act::Open {
            p: 0,
            take_server: false,
        }),
        Step::plain(Act::ActorPush {
            who: Acct::Owner,
            p: 0,
            keep_lock: true,
        }),
        Step::plain(Act::Edit),
        Step::plain(Act::SaveChanges {
            long_message: false,
        }),
    ]
}

/// [R1] An edit saved locally by autosave, then Close: the lock is released over
/// saved, un-uploaded work.
fn saved_unpushed_close_steps() -> Vec<Step> {
    use model::{Acct, Act};
    vec![
        Step::plain(Act::Login { acct: Acct::Owner }),
        Step::plain(Act::Open {
            p: 0,
            take_server: false,
        }),
        Step::plain(Act::Edit),
        Step::plain(Act::Autosave),
        Step::plain(Act::Close),
        Step::plain(Act::Open {
            p: 0,
            take_server: false,
        }),
    ]
}

/// The R3 directed list as a saved step list, for `speleodb_fuzz_replay`.
const SAME_ACCOUNT_OVERWRITE_REPLAY: &str = r#"[
  {"act": {"Login": {"acct": "Owner"}}, "fault": null},
  {"act": {"Open": {"p": 0, "take_server": false}}, "fault": null},
  {"act": {"ActorPush": {"who": "Owner", "p": 0, "keep_lock": true}}, "fault": null},
  {"act": "Edit", "fault": null},
  {"act": {"SaveChanges": {"long_message": false}}, "fault": null}
]"#;

/// The R1 directed list as a saved step list, for `speleodb_fuzz_replay`.
const SAVED_UNPUSHED_CLOSE_REPLAY: &str = r#"[
  {"act": {"Login": {"acct": "Owner"}}, "fault": null},
  {"act": {"Open": {"p": 0, "take_server": false}}, "fault": null},
  {"act": "Edit", "fault": null},
  {"act": "Autosave", "fault": null},
  {"act": "Close", "fault": null},
  {"act": {"Open": {"p": 0, "take_server": false}}, "fault": null}
]"#;

/// The R6 directed list as a saved step list, for `speleodb_fuzz_replay`.
const CRASH_RESIDUE_REOPEN_REPLAY: &str = r#"[
  {"act": {"Login": {"acct": "Owner"}}, "fault": null},
  {"act": {"Open": {"p": 0, "take_server": false}}, "fault": null},
  {"act": "Edit", "fault": null},
  {"act": "Autosave", "fault": null},
  {"act": {"Crash": {"residue": true}}, "fault": null},
  {"act": {"Open": {"p": 0, "take_server": true}}, "fault": null},
  {"act": "Refresh", "fault": null}
]"#;

/// Offline: each directed list is the saved list the replay entry point reads.
#[test]
fn the_directed_lists_are_their_saved_replays() {
    let lists = directed_steps();
    let saved = [
        ("same-account-overwrite", SAME_ACCOUNT_OVERWRITE_REPLAY),
        ("saved-unpushed-close", SAVED_UNPUSHED_CLOSE_REPLAY),
        ("crash-residue-reopen", CRASH_RESIDUE_REOPEN_REPLAY),
    ];
    assert_eq!(lists.len(), saved.len(), "one saved list per directed list");
    for ((name, steps), (saved_name, json)) in lists.iter().zip(saved) {
        let saved_steps: Vec<Step> = serde_json::from_str(json).expect("parse");
        assert_eq!((*name, steps), (saved_name, &saved_steps));
    }
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_owned())
}

fn env_list(name: &str) -> Vec<String> {
    env_or(name, "")
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
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
/// `AppState`) and the instance's accounts. Refuses when this process's app dirs
/// were already set elsewhere (another test ran first in this process).
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
        copies: base.join("copies"),
        mute: env_list("STALAGMIGHT_SPELEODB_FUZZ_MUTE"),
        skip_acts: env_list("STALAGMIGHT_SPELEODB_FUZZ_SKIP_ACTS"),
    }
}

/// Where traces and step lists go: `STALAGMIGHT_SPELEODB_FUZZ_TRACE_DIR`, or by
/// default a folder of this process's own in the temp dir, created once under a
/// random name and owner-only, kept after the run so its traces can be read.
fn trace_dir() -> PathBuf {
    static DEFAULT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let overridden = env_or("STALAGMIGHT_SPELEODB_FUZZ_TRACE_DIR", "");
    if !overridden.is_empty() {
        let dir = PathBuf::from(overridden);
        let _ = std::fs::create_dir_all(&dir);
        return dir;
    }
    DEFAULT
        .get_or_init(|| {
            let mut folder = tempfile::Builder::new();
            folder.prefix("stalagmight-sdb-fuzz-traces-");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                folder.permissions(std::fs::Permissions::from_mode(0o700));
            }
            folder.tempdir().map_or_else(
                |e| panic!("FZ-SDB: could not create a private trace folder: {e}"),
                tempfile::TempDir::keep,
            )
        })
        .clone()
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

fn muted_lines(seed: &str, muted: &[(usize, String)]) {
    for (step, id) in muted {
        println!("FZ-SDB seed {seed}: muted {id} at step {step}");
    }
}

/// The long campaign: `STALAGMIGHT_SPELEODB_FUZZ_SEEDS=a..b` (default 1..4),
/// `_STEPS` per seed (40), `_FAULT_PCT` (25), `_MINUTES` (stop starting seeds
/// after this long), `_SHRINK=0` to skip shrinking, `_SHRINK_BUDGET` (40),
/// `_MUTE` (invariant ids or id prefixes that do not fail a seed), `_SKIP_ACTS`
/// (act names never drawn).
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
        muted_lines(&seed.to_string(), &r.muted);
        match r.failure {
            None => {
                println!(
                    "FZ-SDB seed {seed}: ok ({} steps, {:.1}s{})",
                    r.steps.len(),
                    r.elapsed.as_secs_f64(),
                    if r.muted.is_empty() {
                        ""
                    } else {
                        ", with muted"
                    }
                );
                if r.muted.is_empty() {
                    let _ = std::fs::remove_file(&trace);
                    let _ = std::fs::remove_file(dir.join(format!("{label}.json")));
                }
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
    muted_lines("replay", &r.muted);
    if let Some(e) = &r.setup_error {
        panic!("FZ-SDB replay: SETUP ERROR {e}");
    }
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

/// The default content of the seed project: three sections, nothing from any
/// real survey.
const DEFAULT_SEED_SECTIONS: &str = "A:3,B:2,C:2";

/// Instance tooling: write a synthetic `.stalx` to
/// `STALAGMIGHT_SPELEODB_FUZZ_SEED_OUT`, for seeding the instance's baseline
/// Stalagmight projects. `STALAGMIGHT_SPELEODB_FUZZ_SEED_SECTIONS` (optional,
/// `name:shots,...`, default `A:3,B:2,C:2`) picks its sections. No network.
#[test]
#[ignore = "instance tooling: writes a synthetic seed .stalx to STALAGMIGHT_SPELEODB_FUZZ_SEED_OUT"]
fn speleodb_fuzz_write_seed_stalx() {
    let out = PathBuf::from(
        std::env::var("STALAGMIGHT_SPELEODB_FUZZ_SEED_OUT")
            .unwrap_or_else(|_| panic!("FZ-SDB: STALAGMIGHT_SPELEODB_FUZZ_SEED_OUT is not set")),
    );
    let spec = env_or(
        "STALAGMIGHT_SPELEODB_FUZZ_SEED_SECTIONS",
        DEFAULT_SEED_SECTIONS,
    );
    let sections = data::parse_sections(&spec).unwrap_or_else(|| {
        panic!("FZ-SDB: STALAGMIGHT_SPELEODB_FUZZ_SEED_SECTIONS {spec:?} is not name:shots,...")
    });
    data::write_stalx(&out, &sections).expect("write the seed");
    println!("FZ-SDB: wrote {} ({spec})", out.display());
}

/// Every seed that once failed, and every directed list, re-run (on the instance only).
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
    for (name, steps) in directed_steps() {
        let label = format!("reg-{name}");
        let trace = dir.join(format!("{label}.jsonl"));
        let _ = std::fs::remove_file(&trace);
        let r = h.run(
            &label,
            Source::Fixed(steps.clone()),
            steps.len(),
            Some(&trace),
        );
        muted_lines(&label, &r.muted);
        println!(
            "FZ-SDB directed {name}: {}",
            r.failure.as_ref().map_or("ok".to_owned(), |(i, v)| format!(
                "FAIL at step {i}: {}",
                serde_json::to_string(v).unwrap_or_default()
            ))
        );
        if let Some((i, v)) = r.failure {
            failed.push(format!(
                "directed {name} step {i}: {}",
                serde_json::to_string(&v).unwrap_or_default()
            ));
        }
    }
    assert!(
        failed.is_empty(),
        "FZ-SDB regression seeds failed: {failed:?}"
    );
}
