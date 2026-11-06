//! Write the synthetic seed corpus of one fuzz target.
//!
//! Usage: fuzz_seeds <target> <dir> [count=64] [first=0]
//!
//! Seeds are generated in code (`stalagmight_fuzz_gen::targets::seed_input`),
//! never copied from a survey file, and named `seed-<n>` with no survey-file
//! extension. Run by src-tauri/fuzz/run.sh when a corpus directory is empty.

#![expect(clippy::print_stdout, clippy::print_stderr)]

use std::path::PathBuf;
use std::process::ExitCode;

use stalagmight_fuzz_gen::targets::{seed_input, Target};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (Some(name), Some(dir)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: fuzz_seeds <target> <dir> [count] [first]");
        return ExitCode::from(2);
    };
    let Some(target) = Target::from_name(name) else {
        let names: Vec<String> = Target::all().into_iter().map(Target::name).collect();
        eprintln!("unknown target {name}; known: {}", names.join(" "));
        return ExitCode::from(2);
    };
    let count: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(64);
    let first: u64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
    let dir = PathBuf::from(dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("cannot create {}: {e}", dir.display());
        return ExitCode::FAILURE;
    }
    for seed in first..first.saturating_add(count) {
        let bytes = match seed_input(target, seed) {
            Ok(bytes) => bytes,
            Err(v) => {
                eprintln!("{name}: seed {seed}: {v}");
                return ExitCode::FAILURE;
            }
        };
        let path = dir.join(format!("seed-{seed:06}"));
        if let Err(e) = std::fs::write(&path, bytes) {
            eprintln!("cannot write {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    }
    println!("{name}: wrote {count} seeds to {}", dir.display());
    ExitCode::SUCCESS
}
