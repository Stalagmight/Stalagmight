//! cargo-fuzz target `gen_gpx`: see `stalagmight_fuzz_gen::targets` for what
//! the input means and which invariants a run checks.
#![no_main]

use stalagmight_fuzz_gen::oracle::Format;
use stalagmight_fuzz_gen::targets::{run, Target};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    if let Err(violation) = run(Target::Gen(Format::Gpx), data) {
        panic!("gen_gpx: {violation}");
    }
});
