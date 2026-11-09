//! [FZ-SDB] A stateful SpeleoDB protocol fuzzer.
//!
//! Instance tooling lives here too: [`speleodb_fuzz_write_seed_stalx`] writes a
//! synthetic `.stalx` for seeding a private instance's baseline projects. See
//! docs/speleodb-dev-testing.md, "Protocol fuzzer".

mod data;

use std::path::PathBuf;

/// The default content of the seed project: three sections, nothing from any
/// real survey.
const DEFAULT_SEED_SECTIONS: &str = "A:3,B:2,C:2";

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_owned())
}

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
