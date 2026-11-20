//! Files on disk: the two tracked old fixtures (schema 4 and 14, the only
//! files in the tree an old build really wrote), the private-corpus runner,
//! and the timing of large synthetic surveys.
//!
//! These print COUNTS ONLY (the report summary, error classes, hashes of
//! file names). Never a name, a coordinate or any other content of a file:
//! the corpus runner is run on private data on another host.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use stalagmight_legacy::models::{self as m, GpsCoordinates, LineData, ShotData, StationRef};
use stalagmight_legacy::{write_v18, LegacyExtras, OpenOptions};

use super::test_support::{check, Rng};
use super::{migrate, migrate_path, ExpectedClass, Migrated};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../e2e/fixtures")
        .join(name)
}

fn migrate_fixture(name: &str) -> Migrated {
    let path = fixture(name);
    let before = std::fs::read(&path).unwrap();
    let legacy = stalagmight_legacy::open_path(&path, &OpenOptions::default()).unwrap();
    let migrated = migrate(&legacy).unwrap();
    if let Err(e) = check(&legacy, &migrated) {
        panic!("{name}: {e}");
    }
    assert_eq!(std::fs::read(&path).unwrap(), before, "{name} untouched");
    eprintln!("{name}:\n{}", migrated.report.summary());
    migrated
}

#[test]
fn the_schema_4_fixture_migrates_with_every_station_in_place() {
    let out = migrate_fixture("fractal_plant_iter3.stalx");
    assert_eq!(out.report.schema_version, 4);
    // Pre-v6: every shot uuid was derived by the legacy load and is kept.
    assert_eq!(out.report.legacy_derived_shot_uuids, out.report.shots);
    assert_eq!(out.report.shot_uuids_minted, 0);
    assert!(out.report.verification.is_clean());
}

#[test]
fn the_schema_14_fixture_migrates_with_every_station_in_place() {
    let out = migrate_fixture("naharon.stalx");
    assert_eq!(out.report.schema_version, 14);
    assert!(out.report.verification.is_clean());
}

/// A short, stable, non-reversible tag for a file name.
fn name_hash(name: &str) -> String {
    const NAMESPACE: uuid::Uuid = uuid::Uuid::from_u128(0x1b0e_4a2c_77d9_4c31_9f05_2e6a_c8d4_5b17);
    uuid::Uuid::new_v5(&NAMESPACE, name.as_bytes())
        .simple()
        .to_string()[..12]
        .to_string()
}

fn stalx_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            stalx_files(&path, out);
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("stalx"))
        {
            out.push(path);
        }
    }
}

/// `STALAGMIGHT_MIGRATION_CORPUS=<dir> cargo test -p stalagmight-persistence
/// --lib migrate_corpus -- --ignored --nocapture`: migrates every `*.stalx`
/// under `<dir>` and prints, per file, a hash of its name, the outcome
/// (an error class, never its text) and the report's counts; then totals.
/// Fails when any file has an unexplained position mismatch or panics.
#[test]
#[ignore = "runs on a private corpus: set STALAGMIGHT_MIGRATION_CORPUS"]
fn migrate_corpus() {
    let Some(dir) = std::env::var_os("STALAGMIGHT_MIGRATION_CORPUS") else {
        eprintln!("STALAGMIGHT_MIGRATION_CORPUS is not set");
        return;
    };
    let dir = PathBuf::from(dir);
    let mut files = Vec::new();
    stalx_files(&dir, &mut files);
    // A panic message could quote a file: silence the hook for the run.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let (mut ok, mut failed, mut panicked, mut unclean) = (0, 0, 0, 0);
    let mut classes: BTreeMap<String, usize> = BTreeMap::default();
    let mut expected_max: BTreeMap<ExpectedClass, f64> = BTreeMap::default();
    for path in &files {
        let relative = path.strip_prefix(&dir).unwrap_or(path);
        let tag = name_hash(&relative.to_string_lossy());
        let started = Instant::now();
        let outcome = std::panic::catch_unwind(|| migrate_path(path));
        let millis = started.elapsed().as_millis();
        match outcome {
            Ok(Ok(migrated)) => {
                ok += 1;
                let v = &migrated.report.verification;
                if !v.is_clean() {
                    unclean += 1;
                }
                for (class, stats) in &v.expected {
                    let worst = expected_max.entry(*class).or_insert(0.0);
                    *worst = worst.max(stats.max_displacement_m);
                }
                eprintln!(
                    "file={tag} status=ok ms={millis} schema={} history={}\n{}",
                    migrated.report.schema_version,
                    migrated.had_history,
                    migrated.report.summary()
                );
            }
            Ok(Err(error)) => {
                failed += 1;
                *classes.entry(error.class().to_string()).or_default() += 1;
                eprintln!("file={tag} status=error class={}", error.class());
            }
            Err(_) => {
                panicked += 1;
                eprintln!("file={tag} status=panic");
            }
        }
    }
    std::panic::set_hook(hook);
    eprintln!(
        "corpus: files={} ok={ok} errors={failed} panics={panicked} unexplained_mismatch_files={unclean}",
        files.len()
    );
    for (class, n) in &classes {
        eprintln!("corpus: error.{class}={n}");
    }
    for (class, worst) in &expected_max {
        eprintln!("corpus: expected.{}.max_m={worst:e}", class.as_str());
    }
    // A file the legacy reader opens must migrate: any other error is a
    // migration bug (or a model limit that loses a file the old app opened).
    let lost: usize = classes
        .iter()
        .filter(|(class, _)| !class.starts_with("legacy-"))
        .map(|(_, n)| n)
        .sum();
    assert_eq!(panicked, 0, "a migration panicked");
    assert_eq!(lost, 0, "files the legacy reader opened failed to migrate");
    assert_eq!(unclean, 0, "unexplained position mismatches");
}

/// A synthetic survey of about `shots` shots: lines of 50 shots, each
/// starting on a random station of an earlier line, one datum, some loops.
fn large_survey(shots: usize, seed: u64) -> m::Project {
    let mut rng = Rng::new(seed);
    let mut p = m::Project::new(format!("Large {shots}"), None);
    let mut n = 0u64;
    let lines = shots.div_ceil(50);
    for l in 0..lines {
        let mut depth = rng.range(0.0, 40.0);
        let data = (0..50)
            .map(|_| {
                n += 1;
                let azimuth = rng.range(0.0, 360.0);
                let length = rng.range(1.0, 12.0);
                let depth_in = depth;
                depth = (depth + rng.range(-1.0, 1.0)).max(0.0);
                ShotData {
                    uuid: Some(format!("00000000-0000-4000-8000-{n:012x}")),
                    azimuth_in: azimuth,
                    azimuth_out: azimuth,
                    length,
                    depth_in,
                    depth_out: depth,
                    left: Some(1.0),
                    right: Some(1.0),
                    ..ShotData::default()
                }
            })
            .collect();
        let id = p
            .add_line(LineData {
                name: Some(format!("L{l}")),
                surveyor: None,
                date: None,
                color: "#336699".to_string(),
                direction: None,
                quality: 1.0,
                shots: data,
            })
            .id;
        if l > 0 {
            let target = p.lines[rng.below(l)].id;
            let index = rng.below(51) as u32;
            let _ = p.merge_stations(StationRef::new(id, 0), StationRef::new(target, index));
        }
    }
    let first = p.lines[0].id;
    p.set_zero_datum(
        StationRef::new(first, 0),
        GpsCoordinates::new(20.5, -87.3, 0.0).unwrap(),
    )
    .unwrap();
    for _ in 0..lines / 10 {
        let a = StationRef::new(p.lines[rng.below(lines)].id, 50);
        let b = StationRef::new(p.lines[rng.below(lines)].id, 25);
        let _ = p.create_loop_closure(a, b, m::ClosureStrategy::Batch);
    }
    p
}

/// `cargo test -p stalagmight-persistence --release --lib
/// migration_timing -- --ignored --nocapture`: legacy open and migration
/// times of 5k- and 20k-shot synthetic surveys.
#[test]
#[ignore = "a measurement: run with --release --ignored --nocapture"]
fn migration_timing() {
    for shots in [5_000, 20_000] {
        let project = large_survey(shots, 7);
        let bytes = write_v18(&project, &LegacyExtras::default()).unwrap();
        let options = OpenOptions {
            with_adjusted: false,
            ..OpenOptions::default()
        };
        let started = Instant::now();
        let legacy = stalagmight_legacy::open_bytes(&bytes, &options).unwrap();
        let open = started.elapsed();
        let started = Instant::now();
        let migrated = migrate(&legacy).unwrap();
        let total = started.elapsed();
        let started = Instant::now();
        let positions = migrated.project.raw_positions();
        let solve = started.elapsed();
        assert!(migrated.report.verification.is_clean());
        assert_eq!(positions.iter().count(), migrated.report.stations);
        eprintln!(
            "shots={} lines={} stations={} legacy_open_ms={} migrate_ms={} (incl. verification) raw_solve_ms={}",
            migrated.report.shots,
            migrated.report.lines,
            migrated.report.stations,
            open.as_millis(),
            total.as_millis(),
            solve.as_millis()
        );
    }
}
