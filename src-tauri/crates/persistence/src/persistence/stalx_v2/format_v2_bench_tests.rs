//! Size and speed of the v2 format on the bench shape (25 shots per line,
//! junction starts, LRUD on 70% of shots), for comparison with the SQLite
//! numbers of the persistence map (E3 section 7.4). Ignored; run on demand,
//! in release, on a real disk:
//!
//! ```text
//! STALX_BENCH_DIR=$HOME/.cache/stalx-v2-bench cargo test --release \
//!   -p stalagmight-persistence --lib stalx_v2::format_v2_bench_tests -- --ignored --nocapture
//! ```

use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::format_v2_diff_tests::survey;
use super::{
    decode, encode, probe_path, survey_digest, write_atomic, DecodeOptions, Extras, Guard, Mode,
};

fn median(mut v: Vec<Duration>) -> f64 {
    v.sort();
    v[v.len() / 2].as_secs_f64() * 1e3
}

fn time(reps: usize, mut f: impl FnMut()) -> f64 {
    f();
    median(
        (0..reps)
            .map(|_| {
                let t = Instant::now();
                f();
                t.elapsed()
            })
            .collect(),
    )
}

fn gzip_len(bytes: &[u8]) -> usize {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
    e.write_all(bytes).unwrap();
    e.finish().unwrap().len()
}

#[test]
#[ignore = "bench: run on demand in release"]
fn sizes_and_speeds() {
    let dir: PathBuf = std::env::var("STALX_BENCH_DIR").map_or_else(
        |_| std::env::temp_dir().join("stalx-v2-bench"),
        PathBuf::from,
    );
    std::fs::create_dir_all(&dir).unwrap();
    println!("| shots | v2 bytes | gzip | encode ms | decode ms | validate ms | probe us | durable write ms | survey digest ms |");
    println!("|---|---|---|---|---|---|---|---|---|");
    for (lines, reps) in [(12, 25), (200, 11), (800, 7), (4000, 3)] {
        let p = survey(lines, 25, 7);
        let extras = Extras::default();
        let bytes = encode(&p, &extras, Mode::Normal).unwrap();
        let encode_ms = time(reps, || {
            let _ = encode(&p, &extras, Mode::Normal).unwrap();
        });
        let decode_ms = time(reps, || {
            let _ = decode(&bytes, &DecodeOptions::default()).unwrap();
        });
        let validate_ms = time(reps, || {
            let _ = p.validate();
        });
        let path = dir.join(format!("bench-{lines}.stalx"));
        let _ = std::fs::remove_file(&path);
        let write_ms = time(reps, || {
            write_atomic(&path, &bytes, Guard::Overwrite).unwrap();
        });
        // The link check of the (unlinked) file on disk, as every link reader
        // makes it: it stops at the first body member (review finding O1).
        let probe_us = time(reps * 20, || {
            let _ = probe_path(&path).unwrap();
        }) * 1e3;
        // What a save, an open or a link read of a SpeleoDB copy marked
        // un-uploaded adds to tell a stale mark from un-uploaded work (S2).
        let digest_ms = time(reps, || {
            let _ = survey_digest(&bytes, "3a7bd3e2360a3d29eea436fcfb7e44c735d117c4").unwrap();
        });
        println!(
            "| {} | {} | {} | {encode_ms:.2} | {decode_ms:.2} | {validate_ms:.2} | {probe_us:.1} | {write_ms:.2} | {digest_ms:.2} |",
            lines * 25,
            bytes.len(),
            gzip_len(&bytes),
        );
    }
}
