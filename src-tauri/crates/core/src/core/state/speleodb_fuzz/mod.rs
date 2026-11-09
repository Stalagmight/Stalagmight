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

mod data;
mod proxy;
mod stub;
mod target;
mod wire;
mod world;

use std::path::PathBuf;

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_owned())
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
