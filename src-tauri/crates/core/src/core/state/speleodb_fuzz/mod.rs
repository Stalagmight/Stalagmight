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
#[cfg(test)]
mod oracle_tests;
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
const REGRESSION_SEEDS: &[(u64, usize, u64)] = &[
    // FZ-SDB-2 (fixed): Publish left the now-linked project editing without its
    // lock (I11).
    (101, 40, 25),
    (2100, 40, 25),
    (3600, 40, 25),
    (4101, 40, 25),
    // FZ-SDB-4 (fixed): a revision View mid-edit dropped the lock claim, and the
    // quit kept the lock without a word (I2b).
    (14503, 40, 25),
    // Oracle false positives, since fixed: a silent release promised for a
    // read-only copy or a lock the second client touched (I2b), and a publish whose
    // best-effort push did not land read as synced (I6).
    (10002, 40, 25),
    (10005, 40, 25),
    (14000, 40, 25),
    (14007, 40, 25),
    (402, 40, 15),
    (200, 40, 15),
    // FZ-SDB-3 (fixed, decision FZ-SDB-3-D1): a sign-in that replaced the session
    // kept the old server's lock without a word (I2b).
    (10000, 40, 25),
    (13506, 40, 25),
    // FZ-SDB-5 (fixed, decisions FZ-SDB-5-D1..D3): re-opening a project from
    // SpeleoDB overwrote an unlinked copy at its cache path (I1). Farm seed 100000.
    (302, 40, 15),
    (12508, 40, 25),
    (11009, 40, 25),
    (100000, 40, 25),
    // FZ-SDB-6 (fixed, decision FZ-SDB-6-D1): a lock verb whose reply was lost after
    // the server applied it left the app's lock state out of step: a lost release (or
    // Done) reply kept the gate open over a free lock (I11; farm seed 100057); a lost
    // acquire reply (Publish, Start Editing) made the lock read as inherited, so a
    // logout or quit never handed it back (I2b; farm seed 100026).
    (12505, 40, 25),
    (12008, 40, 25),
    (203, 40, 15),
    (13004, 40, 25),
    (100026, 40, 25),
    (100057, 40, 25),
];

/// Directed step lists, replayed by [`speleodb_fuzz_regression_seeds`] beside the
/// seeds: shapes the generator cannot draw, because it arms one fault per step.
fn directed_steps() -> Vec<(&'static str, Vec<Step>)> {
    vec![
        ("sdb-poll-d1", sdb_poll_d1_steps()),
        ("sdb-acct-d1-poll", sdb_acct_d1_steps(model::Act::Poll)),
        (
            "sdb-acct-d1-start",
            sdb_acct_d1_steps(model::Act::StartEditing),
        ),
    ]
}

/// [#576 SDB-ACCT-D1, SDB-QUIT-D1] The owner ACCOUNT takes the lock in another session
/// (the second client, as the website or another computer would) while the app has
/// the project open read-only; the app then edits under it - through the status
/// poll's inherited-lock reopen (`Act::Poll`) or a plain Start Editing
/// (`Act::StartEditing`) - and the quit over the clean copy must hand that lock back
/// (I2b): a lock the account holds is the user's, and the quit releases any lock the
/// session edits under. It used to keep it (no claim of this session's).
fn sdb_acct_d1_steps(adopt: model::Act) -> Vec<Step> {
    use model::{Act, LeaveMode, OnUnpushed};
    let plain = |act| Step {
        act,
        fault: None,
        then: Vec::new(),
    };
    vec![
        plain(Act::Login),
        plain(Act::Open {
            p: 0,
            on_unpushed: OnUnpushed::KeepLocal,
        }),
        plain(Act::ActorLock {
            who: world::Who::Owner,
            p: 0,
        }),
        plain(adopt),
        plain(Act::Quit {
            mode: LeaveMode::KeepLock,
        }),
    ]
}

/// [#576 SDB-POLL-D1] Start Editing's acquire lands with its reply lost (a 502 after
/// the server applied it), and every attempt of the holder re-read after it fails
/// (503: the read and the client's two retries); the probe before the acquire gets
/// through (a 1 ms delay takes the first matching rule). The poll then finds the
/// lock ours, and the quit over the clean copy must hand it back (I2b): the claim
/// the lost reply cost is recorded by that poll.
fn sdb_poll_d1_steps() -> Vec<Step> {
    use model::{Act, Fault, FaultSpec, LeaveMode, OnUnpushed};
    let spec = |fault, method: &str, path: &str, times| FaultSpec {
        fault,
        method: Some(method.to_owned()),
        path: path.to_owned(),
        times,
    };
    let plain = |act| Step {
        act,
        fault: None,
        then: Vec::new(),
    };
    vec![
        plain(Act::Login),
        plain(Act::Open {
            p: 0,
            on_unpushed: OnUnpushed::KeepLocal,
        }),
        Step {
            act: Act::StartEditing,
            fault: Some(spec(Fault::LatencyMs(1), "GET", "/projects/", 1)),
            then: vec![
                spec(Fault::StatusAfter(502), "POST", "/acquire/", 1),
                spec(Fault::Status(503), "GET", "/projects/", 3),
            ],
        },
        plain(Act::Poll),
        plain(Act::Quit {
            mode: LeaveMode::KeepLock,
        }),
    ]
}

/// The SDB-POLL-D1 directed list as a saved step list, for `speleodb_fuzz_replay`.
const SDB_POLL_D1_REPLAY: &str = r#"[
  {"act": "Login", "fault": null},
  {"act": {"Open": {"p": 0, "on_unpushed": "KeepLocal"}}, "fault": null},
  {"act": "StartEditing",
   "fault": {"fault": {"LatencyMs": 1}, "method": "GET", "path": "/projects/", "times": 1},
   "then": [
     {"fault": {"StatusAfter": 502}, "method": "POST", "path": "/acquire/", "times": 1},
     {"fault": {"Status": 503}, "method": "GET", "path": "/projects/", "times": 3}
   ]},
  {"act": "Poll", "fault": null},
  {"act": {"Quit": {"mode": "KeepLock"}}, "fault": null}
]"#;

/// The SDB-ACCT-D1 directed lists as saved step lists, for `speleodb_fuzz_replay`.
const SDB_ACCT_D1_POLL_REPLAY: &str = r#"[
  {"act": "Login", "fault": null},
  {"act": {"Open": {"p": 0, "on_unpushed": "KeepLocal"}}, "fault": null},
  {"act": {"ActorLock": {"who": "Owner", "p": 0}}, "fault": null},
  {"act": "Poll", "fault": null},
  {"act": {"Quit": {"mode": "KeepLock"}}, "fault": null}
]"#;
const SDB_ACCT_D1_START_REPLAY: &str = r#"[
  {"act": "Login", "fault": null},
  {"act": {"Open": {"p": 0, "on_unpushed": "KeepLocal"}}, "fault": null},
  {"act": {"ActorLock": {"who": "Owner", "p": 0}}, "fault": null},
  {"act": "StartEditing", "fault": null},
  {"act": {"Quit": {"mode": "KeepLock"}}, "fault": null}
]"#;

/// [#576 SDB-POLL-D1, SDB-ACCT-D1] Offline: each directed list is the saved list the
/// replay entry point reads, so a replay of that file runs exactly the regression
/// set's steps.
#[test]
fn the_directed_list_is_its_saved_replay() {
    let lists = directed_steps();
    let saved = [
        ("sdb-poll-d1", SDB_POLL_D1_REPLAY),
        ("sdb-acct-d1-poll", SDB_ACCT_D1_POLL_REPLAY),
        ("sdb-acct-d1-start", SDB_ACCT_D1_START_REPLAY),
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

/// Where traces and step lists go: `STALAGMIGHT_SPELEODB_FUZZ_TRACE_DIR`, or by
/// default a folder of this process's own in the temp dir, kept after the run so
/// its traces can be read.
///
/// [#576 W87b] The default used to be one fixed name,
/// `<temp>/stalagmight-sdb-fuzz-traces`, written through `create_dir_all` and
/// `fs::write`, which follow links: on a shared temp dir another user could
/// create that folder, or a link at it, ahead of a run and have the traces
/// written where they chose. The default is now made once per process, under a
/// random name and owner-only (`tempfile`, which refuses a name that exists).
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

/// [#576 W87b] The default trace folder is this process's own: not the fixed
/// `<temp>/stalagmight-sdb-fuzz-traces` another user could plant ahead of a run,
/// a real folder rather than a link, owner-only, and one folder for every entry
/// point of the process. No other test of this binary writes there, so the empty
/// folder is removed at the end.
#[test]
fn the_default_trace_folder_is_private_to_this_process() {
    if std::env::var_os("STALAGMIGHT_SPELEODB_FUZZ_TRACE_DIR").is_some() {
        println!("FZ-SDB: skipped: STALAGMIGHT_SPELEODB_FUZZ_TRACE_DIR is set");
        return;
    }
    let dir = trace_dir();
    let again = trace_dir();
    let fixed = std::env::temp_dir().join("stalagmight-sdb-fuzz-traces");
    let meta = std::fs::symlink_metadata(&dir).expect("the trace folder exists");
    #[cfg(unix)]
    let (mode, owner, me) = {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        let probe = tempfile::tempfile().expect("a file of this user's");
        let me = probe.metadata().expect("its metadata").uid();
        (meta.permissions().mode() & 0o777, meta.uid(), me)
    };
    #[cfg(unix)]
    println!(
        "FZ-SDB: default trace folder {} (again {}); mode {mode:o}, owner {owner}, this \
         user {me}",
        dir.display(),
        again.display()
    );
    let _ = std::fs::remove_dir(&dir);
    assert_ne!(dir, fixed, "not the fixed name every run used to share");
    assert_eq!(dir, again, "one folder for the whole process");
    assert!(meta.is_dir(), "a real folder, not a link");
    #[cfg(unix)]
    {
        assert_eq!(mode & 0o077, 0, "owner-only: {mode:o}");
        assert_eq!(owner, me, "owned by this user");
    }
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

/// Instance tooling: write a synthetic `.stalx` (three sections, nothing from any
/// real survey) to `STALAGMIGHT_SPELEODB_FUZZ_SEED_OUT`, for seeding the
/// instance's baseline Stalagmight projects. No network.
#[test]
#[ignore = "instance tooling: writes a synthetic seed .stalx to STALAGMIGHT_SPELEODB_FUZZ_SEED_OUT"]
fn speleodb_fuzz_write_seed_stalx() {
    let out = PathBuf::from(
        std::env::var("STALAGMIGHT_SPELEODB_FUZZ_SEED_OUT")
            .unwrap_or_else(|_| panic!("FZ-SDB: STALAGMIGHT_SPELEODB_FUZZ_SEED_OUT is not set")),
    );
    let project = world::project_with(&[
        ("A".to_owned(), 3),
        ("B".to_owned(), 2),
        ("C".to_owned(), 2),
    ]);
    let _ = std::fs::remove_file(&out);
    let mut db = crate::persistence::Database::open(&out).expect("create the seed");
    db.save_project(&project).expect("save the seed");
    println!("FZ-SDB: wrote {}", out.display());
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
