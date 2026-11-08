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

use std::fmt::Write as _;
use std::panic::{catch_unwind, AssertUnwindSafe};

use stalagmight_fuzz_gen::fileset::FileSet;
use stalagmight_fuzz_gen::oracle::{self, Depth, Format, Knobs};
use stalagmight_fuzz_gen::targets::{run, seed_input, Target};

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// Seeds that once failed, replayed on every run on top of the fixed range
/// (the bug each one found is named; its own regression test lives with the
/// fix).
fn regression_seeds(target: Target) -> &'static [u64] {
    match target {
        // FZ-PARSE-2: a project setting stored as TEXT that is not UTF-8
        // panicked the load (persistence project_settings_utf8_tests).
        Target::StalxSql => &[147, 283],
        // FZ-PARSE-5: Walls readings that are huge but finite (1e308) were
        // accepted and the computed station positions overflowed to infinity
        // (parsers walls::reading_bound_tests). The later ones are the fuzz
        // farm's rolling windows 1000-5189 on 8062377e2.
        Target::Gen(Format::Srv) => &[105, 1149, 4113, 4777],
        Target::Gen(Format::Wpj) => &[303, 3266, 3916, 4780],
        Target::Raw(Format::Srv) => &[1952, 2420],
        Target::Raw(Format::Wpj) => &[1426, 1445, 1662, 4457, 5116],
        // FZ-PARSE-5, Compass shot length: once a length had no missing
        // marker (RVF-d-2), a leg of about f64::MAX feet was accepted and the
        // import was refused for a station at infinity (parsers
        // compass::reading_bound_tests). The fuzz farm's rolling windows on
        // 074ab799e.
        Target::Raw(Format::Dat) => &[272_644],
        Target::Gen(Format::Dat) => &[283_470, 352_509],
        Target::Gen(Format::Mak) => &[351_636],
        _ => &[],
    }
}

fn run_seeds(target: Target, default_count: u64) {
    if let Ok(only) = std::env::var("STALAGMIGHT_FUZZ_TARGET") {
        if only != target.name() {
            return;
        }
    }
    let start = env_u64("STALAGMIGHT_FUZZ_SEED_START", 0);
    let count = env_u64("STALAGMIGHT_FUZZ_SEEDS", default_count);
    let regressions = regression_seeds(target).iter().copied();
    let (parsed, failures) = replay_seeds(
        target,
        (start..start.saturating_add(count)).chain(regressions),
    );
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

/// Run `target` on each seed: how many parsed, and one line per failure.
fn replay_seeds(target: Target, seeds: impl Iterator<Item = u64>) -> (u64, Vec<String>) {
    let mut failures = Vec::new();
    let mut parsed = 0u64;
    for seed in seeds {
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
    (parsed, failures)
}

/// Run one in-code file through the whole pipeline, apply included; a
/// violation fails the test.
fn apply_one(format: Format, text: &str, knobs: Knobs) -> oracle::Outcome {
    let files = FileSet::single(format.entry_name(), text.as_bytes().to_vec());
    match oracle::run(format, &files, knobs, Depth::Apply) {
        Ok(outcome) => outcome,
        Err(v) => panic!("{}: {v}", format.slug()),
    }
}

/// FZ-PARSE-5, Compass case (minimised from a gen_mak artifact; it was the
/// ignored `open_findings` test until the fix): a length correction near
/// f64::MAX was accepted and applied to every shot, and the computed depths
/// overflowed to infinity. It now reads as 0 and the survey imports.
#[test]
fn a_compass_length_correction_beyond_the_limit_imports_uncorrected() {
    let mut survey = String::from(
        "SYNTHETIC CAVE\r\nSURVEY NAME: B1\r\nSURVEY DATE: 2 20 1993\r\n\
         SURVEY TEAM:\r\nAnn\r\n\
         DECLINATION: 0.00  CORRECTIONS: 0.00 0.00 5.9e307\r\n\r\n\
         FROM TO LENGTH BEARING INC LEFT UP DOWN RIGHT\r\n\r\n",
    );
    for i in 1..10 {
        let _ = write!(
            survey,
            "B{i} B{} 1.00 0.00 -60.00 0.00 0.00 0.00 0.00\r\n",
            i + 1
        );
    }
    survey.push_str("\x0c\r\n");
    // Knob bit 3: the dialog's "apply corrections" control on.
    let outcome = apply_one(Format::Dat, &survey, Knobs(0b1000));
    eprintln!("compass correction: {outcome:?}");
    assert!(outcome.applied, "{outcome:?}");
}

/// Run an in-code multi-file import (`files[0]` is the opened one) like
/// [`apply_one`].
fn apply_files(format: Format, files: &[(&str, String)], knobs: Knobs) -> oracle::Outcome {
    let files = FileSet {
        files: files
            .iter()
            .map(|(name, text)| ((*name).to_string(), text.as_bytes().to_vec()))
            .collect(),
    };
    match oracle::run(format, &files, knobs, Depth::Apply) {
        Ok(outcome) => outcome,
        Err(v) => panic!("{}: {v}", format.slug()),
    }
}

/// f64::MAX written out in full with two decimals, as the generators and the
/// fuzzer's mutations wrote it.
fn near_max() -> String {
    format!("{:.2}", f64::MAX)
}

// FZ-PARSE-5, the fuzz farm's crash artifacts on 8062377e2 (gen_srv, raw_srv,
// gen_wpj, gen_mak), each minimised line by line against that build: a
// reading of about f64::MAX was accepted and the import computed a station at
// infinity ("accepted import computes a non-finite station position"). Walls
// now reads such a distance as missing and Compass such a length correction
// as 0, and each file imports.

#[test]
fn farm_gen_srv_a_distance_near_f64_max_imports_as_missing() {
    let srv = format!(
        "A6 A7 {} 28.08 59.94 <4.05 -- 1.59 -999.00>\r\n\
         A7 A8 26.45 101.46 0052.652 *2.87,0.14m,4.46m,--* (R5,3)\r\n",
        near_max()
    );
    let outcome = apply_one(Format::Srv, &srv, Knobs(0));
    eprintln!("farm gen_srv: {outcome:?}");
    assert!(outcome.applied, "{outcome:?}");
}

#[test]
fn farm_raw_srv_a_backsight_shot_near_f64_max_imports_as_missing() {
    let srv = format!(
        "A14 A15 .49\t275.89/95.42\t-11.84\t*2.06 2.34 4.25 --*\n\
         A13 A14 {} --/222.92 27.03\n\
         A14 A15 16.27 72.28g -23.90 <2.602883955138475e-1,4.05,0,1.55>\n",
        near_max()
    );
    let outcome = apply_one(Format::Srv, &srv, Knobs(0));
    eprintln!("farm raw_srv: {outcome:?}");
    assert!(outcome.applied, "{outcome:?}");
}

#[test]
fn farm_gen_wpj_a_project_survey_distance_near_f64_max_imports_as_missing() {
    let wpj = ".SURVEY Synthetic\r\n.NAME S1\r\n".to_string();
    let srv = format!(
        "B4 B5 {} 28.08 59.94 <13.29 -- 13.75 15.51>\r\n\
         \x20 B5 B6 +11.7 198.88 -173.21p <+16.8 5 5 -9.90>\r\n",
        near_max()
    );
    let files = [(Format::Wpj.entry_name(), wpj), ("S1.srv", srv)];
    let outcome = apply_files(Format::Wpj, &files, Knobs(0));
    eprintln!("farm gen_wpj: {outcome:?}");
    assert!(outcome.applied, "{outcome:?}");
}

#[test]
fn farm_gen_mak_a_project_length_correction_near_f64_max_imports_uncorrected() {
    let mak = "&Indian;\r\n@520677.501,8872419.318,828.746,59,0.820;\r\n#s2.dat;\r\n".to_string();
    let mut dat = format!(
        "Synthetic Cave\r\nSURVEY NAME: B55\r\nSURVEY DATE: 2 20 1993  COMMENT:Z\r\n\
         SURVEY TEAM:\r\nAnn Smith, Bob Jones\r\n\
         DECLINATION: 0020.000  FORMAT: RIMMUDRLDLANF  CORRECTIONS: -0.88 0.65 {}\r\n\
         FROM TO LENGTH BEARING INC LEFT UP DOWN RIGHT FLAGS COMMENTS\r\n",
        near_max()
    );
    for i in 26..34 {
        let _ = write!(
            dat,
            "B{i} B{} 1.00 0.00 -60.00 0.00 0.00 0.00 0.00\r\n",
            i + 1
        );
    }
    let files = [(Format::Mak.entry_name(), mak), ("s2.dat", dat)];
    // Knob bit 3: the dialog's "apply corrections" control on.
    let outcome = apply_files(Format::Mak, &files, Knobs(0b1000));
    eprintln!("farm gen_mak: {outcome:?}");
    assert!(outcome.applied, "{outcome:?}");
}

/// FZ-PARSE-5, Compass shot length (the farm's raw_dat, gen_dat and gen_mak
/// findings on 074ab799e): legs of about f64::MAX feet were accepted, the
/// preview said nothing and the import was refused for a station at infinity.
/// Each now reads as missing and the rest of the survey imports.
#[test]
fn farm_dat_a_shot_length_near_f64_max_imports_as_missing() {
    let mut dat = String::from(
        "SYNTHETIC CAVE\r\nSURVEY NAME: B1\r\nSURVEY DATE: 2 20 1993\r\n\
         SURVEY TEAM:\r\nAnn\r\n\
         DECLINATION: 0.00  CORRECTIONS: 0.00 0.00 0.00\r\n\r\n\
         FROM TO LENGTH BEARING INC LEFT UP DOWN RIGHT\r\n\r\n\
         B0 B1 10.00 0.00 -60.00 0.00 0.00 0.00 0.00\r\n",
    );
    for i in 1..5 {
        let _ = write!(
            dat,
            "B{i} B{} {} 0.00 -90.00 0.00 0.00 0.00 0.00\r\n",
            i + 1,
            near_max()
        );
    }
    dat.push_str("\x0c\r\n");
    let outcome = apply_one(Format::Dat, &dat, Knobs(0));
    eprintln!("farm dat length: {outcome:?}");
    assert!(outcome.applied, "{outcome:?}");
    assert_eq!(outcome.shots, 1, "only the 10 ft leg lands: {outcome:?}");
}

/// FZ-PARSE-5, GPX case: an elevation near f64::MAX beside an ordinary one
/// made an infinite slope length, which the preview accepted and the core
/// refused. It now reads as missing and is filled in from its neighbours.
#[test]
fn a_gpx_elevation_beyond_the_limit_imports_as_missing() {
    let gpx = r#"<?xml version="1.0"?><gpx version="1.1"><trk><name>T</name><trkseg>
        <trkpt lat="20.5" lon="-87.3"><ele>10</ele></trkpt>
        <trkpt lat="20.5001" lon="-87.3"><ele>1.7976931348623157e308</ele></trkpt>
        <trkpt lat="20.5002" lon="-87.3"><ele>12</ele></trkpt>
        </trkseg></trk></gpx>"#;
    let outcome = apply_one(Format::Gpx, gpx, Knobs(0));
    eprintln!("gpx elevation: {outcome:?}");
    assert!(outcome.applied, "{outcome:?}");
}

/// FZ-PARSE-5, Ariane case: station depths of 1e308 m averaged to infinity
/// where two shots meet. Each now reads as a missing depth.
#[test]
fn an_ariane_depth_beyond_the_limit_imports_as_missing() {
    let shot = |id: i64, kind: &str, depth: &str| {
        format!(
            "<SurveyData><ID>{id}</ID><FromID>{}</FromID><ClosureToID>-1</ClosureToID>\
             <Type>{kind}</Type><Section>S1</Section><Azimut>45</Azimut>\
             <Length>10</Length><Depth>{depth}</Depth><DepthIn>-1.0</DepthIn>\
             <Inclination>0</Inclination><Left>0</Left><Right>0</Right><Up>0</Up>\
             <Down>0</Down><Color>0x000000ff</Color><Excluded>false</Excluded>\
             <Latitude>0</Latitude><Longitude>0</Longitude></SurveyData>",
            id - 1
        )
    };
    let shots: String = [
        shot(0, "START", "0"),
        shot(1, "REAL", "1e308"),
        shot(2, "REAL", "1e308"),
        shot(3, "REAL", "1e308"),
        shot(4, "REAL", "5"),
    ]
    .concat();
    let tmlu = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><CaveFile><caveName>C</caveName>\
         <unit>m</unit><Data>{shots}</Data></CaveFile>"
    );
    let outcome = apply_one(Format::Tmlu, &tmlu, Knobs(0));
    eprintln!("ariane depth: {outcome:?}");
    assert!(outcome.applied, "{outcome:?}");
}

/// FZ-PARSE-1 (minimised from the gen_gpx sweep, where it refused about a
/// fifth of well-formed files): a GPS standing still logs the same point
/// twice. The repeat used to become a zero-length shot and the core refused
/// the whole track; it is now dropped and the track imports.
#[test]
fn a_gpx_track_with_a_repeated_point_imports() {
    let gpx = r#"<?xml version="1.0"?><gpx version="1.1"><trk><name>T</name><trkseg>
        <trkpt lat="20.5" lon="-87.3"><ele>10</ele></trkpt>
        <trkpt lat="20.5001" lon="-87.3"><ele>10</ele></trkpt>
        <trkpt lat="20.5001" lon="-87.3"><ele>10</ele></trkpt>
        </trkseg></trk></gpx>"#;
    let files = FileSet::single(Format::Gpx.entry_name(), gpx.as_bytes().to_vec());
    // Knobs 0: waypoints, tracks and routes all imported, depth not inverted.
    let outcome = match oracle::run(Format::Gpx, &files, Knobs(0), Depth::Apply) {
        Ok(outcome) => outcome,
        Err(v) => panic!("violation: {v}"),
    };
    eprintln!("repeated GPX point: {outcome:?}");
    assert!(outcome.applied, "the track must import: {outcome:?}");
    assert_eq!(outcome.shots, 1, "3 points, 1 repeat: 1 shot");
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

/// Replay one input file (a cargo-fuzz crash artifact or a corpus entry) on
/// stable: `STALAGMIGHT_FUZZ_REPLAY=<target>:<file>`. A no-op without the
/// variable.
#[test]
fn replay() {
    let Ok(spec) = std::env::var("STALAGMIGHT_FUZZ_REPLAY") else {
        return;
    };
    let Some((name, file)) = spec.split_once(':') else {
        panic!("STALAGMIGHT_FUZZ_REPLAY must be <target>:<file>, got {spec}");
    };
    let Some(target) = Target::from_name(name) else {
        panic!("unknown target {name}");
    };
    let data = std::fs::read(file).unwrap_or_else(|e| panic!("read {file}: {e}"));
    match run(target, &data) {
        Ok(outcome) => eprintln!("{name} {file}: {outcome:?}"),
        Err(violation) => panic!("{name} {file}: {violation}"),
    }
}
