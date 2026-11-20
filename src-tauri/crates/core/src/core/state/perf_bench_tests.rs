//! A release-build benchmark of the `AppState` entry points the IPC commands
//! call, on the 5,000- and 20,000-shot gate surveys (D2). Every number is the
//! median of several runs; nothing is asserted (timings say nothing on a
//! loaded host or in a debug build). Ignored by default.
//!
//! ```text
//! cargo test --release -p stalagmight-core --lib perf_bench -- --ignored --nocapture
//! ```
//!
//! - `STALAGMIGHT_PERF_RUNS`: runs per measurement (default 5).
//! - `STALAGMIGHT_PERF_LEGACY_DIR`: a folder holding `gate-100.stalx` and
//!   `gate-400.stalx`, the gate surveys as a 0.6.0 build saved them (the
//!   0.6.0 side of the comparison writes them there). Without it the legacy
//!   files are written here by the frozen 0.6.0 model and its save.
//! - `STALAGMIGHT_PERF_REPORT`: a file the results are appended to, one
//!   `name<TAB>do<TAB>undo<TAB>redo` line each (milliseconds).
//!
//! The surveys are the size gate's (`undo_size_gate_tests.rs`): a tree of
//! 50-shot lines, a snap every 25 lines, a GPS datum at the entrance, a
//! Section, an island and a leaf. The large import is a synthetic Compass
//! file of the same shape. Fully synthetic data only.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: a failed setup step is a failed run, and the table is the output"
)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use tempfile::TempDir;

use super::legacy_fixture_kit::{frozen, old, slot, write_legacy_file, Legacy};
use super::AppState;
use crate::config::GasConfig;
use crate::core::events::EventSink;
use crate::core::{Command, CommandResult};
use crate::models::{
    ClosureStrategy, CombineLinesOptions, CombinePick, Coordinates, CylRole, Cylinder, Diver,
    GpsCoordinates, LineId, ShotId, ShotUpdates, SplitAt, SplitLineOptions, SplitSide, StationRef,
    TankMaterial,
};

const SHOTS_PER_LINE: usize = 50;

struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}

    fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        false
    }
}

fn runs() -> usize {
    std::env::var("STALAGMIGHT_PERF_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5)
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn millis<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let started = Instant::now();
    let value = f();
    (value, started.elapsed().as_secs_f64() * 1000.0)
}

#[derive(Default)]
struct Report {
    rows: Vec<(String, [f64; 3])>,
}

impl Report {
    fn add(&mut self, name: &str, times: [f64; 3]) {
        println!(
            "  {name:<52} {:>10.2} {:>10.2} {:>10.2}",
            times[0], times[1], times[2]
        );
        self.rows.push((name.to_string(), times));
    }

    fn write(&self) {
        let Ok(path) = std::env::var("STALAGMIGHT_PERF_REPORT") else {
            return;
        };
        let mut text = std::fs::read_to_string(&path).unwrap_or_default();
        for (name, t) in &self.rows {
            let _ = writeln!(text, "{name}\t{:.3}\t{:.3}\t{:.3}", t[0], t[1], t[2]);
        }
        std::fs::write(path, text).unwrap();
    }
}

// ----- the gate survey as 0.6.0 saved it --------------------------------------

fn gate_shot(line: usize, i: usize) -> old::ShotData {
    let azimuth = ((line * 37 + i * 11) % 360) as f64 + 0.25;
    old::ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length: 3.0 + ((line + i * 7) % 9) as f64 * 1.125,
        depth_in: 5.0 + (i as f64) * 0.1,
        depth_out: 5.1 + (i as f64) * 0.1,
        left: Some(1.5),
        right: Some(2.0),
        up: Some(0.75),
        down: Some(1.25),
        comment: i
            .is_multiple_of(10)
            .then(|| format!("station note {line}-{i}")),
        marker_index: Some(i as u32),
        ..Default::default()
    }
}

fn line_data(line: usize, shots: usize) -> old::LineData {
    old::LineData {
        name: Some(format!("Line {line}")),
        color: "#4ecdc4".to_string(),
        shots: (0..shots).map(|i| gate_shot(line, i)).collect(),
        ..Default::default()
    }
}

/// 0.6.0's size-gate survey (`undo_size_gate_tests::survey`), built with the
/// frozen 0.6.0 model.
fn legacy_gate_survey(lines: usize) -> Legacy {
    let mut project = Legacy::new("Gate".to_string(), None);
    for s in 0..lines {
        project.add_line(line_data(s, SHOTS_PER_LINE));
    }
    let ids: Vec<LineId> = project.lines.iter().map(|s| s.id).collect();
    for s in 1..lines {
        let parent = ids[(s - 1) / 2];
        let at = 1 + ((s * 7) % (SHOTS_PER_LINE - 1)) as u32;
        project
            .merge_stations(slot(ids[s], 0), slot(parent, at))
            .unwrap();
    }
    for s in (25..lines).step_by(25) {
        let _ = project.create_loop_closure(
            slot(ids[s], SHOTS_PER_LINE as u32),
            slot(ids[s - 3], 2),
            frozen(&ClosureStrategy::Move),
        );
    }
    project
        .set_zero_datum(
            slot(ids[0], 0),
            frozen(&GpsCoordinates {
                latitude: 20.5,
                longitude: -87.25,
                depth_reference: 0.0,
            }),
        )
        .unwrap();
    let upper = project.add_section("Upper level".to_string(), "#ff6b6b".to_string());
    for id in ids.iter().take(10) {
        project.set_line_section(*id, Some(upper));
    }
    project.add_line(line_data(9_000, 30));
    let leaf = project.add_line(line_data(9_001, 1)).id;
    project
        .merge_stations(slot(leaf, 0), slot(ids[0], SHOTS_PER_LINE as u32))
        .unwrap();
    project
}

/// The legacy gate file for `lines` lines: 0.6.0's own, or written here.
fn legacy_file(dir: &Path, lines: usize) -> PathBuf {
    let name = format!("gate-{lines}.stalx");
    let path = dir.join(&name);
    if let Ok(shared) = std::env::var("STALAGMIGHT_PERF_LEGACY_DIR") {
        std::fs::copy(Path::new(&shared).join(&name), &path).expect("the 0.6.0 gate file");
    } else {
        write_legacy_file(&path, &legacy_gate_survey(lines));
    }
    path
}

// ----- the open survey and its handles ------------------------------------------

struct Bench {
    state: AppState,
    _dir: TempDir,
    path: PathBuf,
    lines: Vec<LineId>,
    island: LineId,
    leaf: LineId,
}

fn fresh_state() -> AppState {
    // The upgrading save backs the legacy original up into the app data
    // folder.
    crate::app_dirs::init_for_test();
    let state = AppState::new().unwrap();
    state.set_event_sink(Arc::new(NullSink));
    state
}

impl Bench {
    /// The gate survey of `lines` lines, upgraded once from its legacy file
    /// and opened from the version 2 file that upgrade wrote.
    fn open(lines: usize) -> Self {
        let dir = TempDir::new().unwrap();
        let legacy = legacy_file(dir.path(), lines);
        let path = dir.path().join("gate-v2.stalx");
        let state = fresh_state();
        state.do_load_project_full(legacy).unwrap();
        let saved = state.execute(Command::SaveProject {
            path: Some(path.clone()),
        });
        assert!(saved.success, "save as v2: {:?}", saved.error);
        let state = fresh_state();
        state.do_load_project_full(path.clone()).unwrap();
        let p = state.get_project_state().unwrap();
        let mut ids: Vec<LineId> = p.lines().iter().map(|l| l.id).collect();
        let leaf = ids.pop().unwrap();
        let island = ids.pop().unwrap();
        Self {
            state,
            _dir: dir,
            path,
            lines: ids,
            island,
            leaf,
        }
    }

    fn mid(&self) -> LineId {
        self.lines[self.lines.len() / 2 + 1]
    }

    fn shot_ids(&self, line: LineId) -> Vec<ShotId> {
        self.state
            .get_project_state()
            .unwrap()
            .line(line)
            .unwrap()
            .shots
            .iter()
            .map(|s| s.id)
            .collect()
    }

    /// The JSON the `get_project_state` IPC sends.
    fn project_state_json(&self) -> usize {
        let wire = self.state.project_state_wire().unwrap();
        serde_json::to_vec(&wire).unwrap().len()
    }
}

// ----- the actions --------------------------------------------------------------

type Run = Box<dyn FnOnce(&AppState) -> CommandResult>;
type Prepare = fn(&Bench) -> Run;

fn command(cmd: Command) -> Run {
    Box::new(move |state| state.execute(cmd))
}

fn update(b: &Bench, line: LineId, index: usize, updates: ShotUpdates) -> Run {
    command(Command::UpdateShot {
        line_id: line,
        shot_id: b.shot_ids(line)[index],
        updates,
    })
}

/// A data-table paste into every shot of `lines`: one `UpdateShot` per
/// cell, each value different.
fn paste(b: &Bench, lines: &[LineId], geometry: bool) -> Run {
    let mut commands = Vec::new();
    for line in lines {
        for shot_id in b.shot_ids(*line) {
            let k = commands.len() as f64;
            let updates = if geometry {
                ShotUpdates {
                    length: Some(4.0 + k * 0.001),
                    ..Default::default()
                }
            } else {
                ShotUpdates {
                    left: Some(Some(1.0 + k * 0.01)),
                    ..Default::default()
                }
            };
            commands.push(Command::UpdateShot {
                line_id: *line,
                shot_id,
                updates,
            });
        }
    }
    command(Command::Batch {
        commands,
        label: None,
    })
}

/// The 1,000 cells a paste covers: 20 tree lines.
fn paste_lines(b: &Bench) -> &[LineId] {
    let first = if b.lines.len() > 300 { 180 } else { 10 };
    &b.lines[first..first + 20]
}

fn actions() -> Vec<(&'static str, Prepare)> {
    vec![
        ("update shot (comment)", |b| {
            update(
                b,
                b.mid(),
                5,
                ShotUpdates {
                    comment: Some(Some("edited".to_string())),
                    ..Default::default()
                },
            )
        }),
        ("update shot (length)", |b| {
            update(
                b,
                b.mid(),
                5,
                ShotUpdates {
                    length: Some(12.5),
                    ..Default::default()
                },
            )
        }),
        ("insert shot", |b| {
            command(Command::CreateShot {
                line_id: b.mid(),
                shot: frozen(&gate_shot(1, 1)),
                after_shot_id: Some(b.shot_ids(b.mid())[20]),
                before_shot_id: None,
            })
        }),
        ("delete shot", |b| {
            command(Command::DeleteShot {
                line_id: b.mid(),
                shot_id: b.shot_ids(b.mid())[25],
            })
        }),
        ("split line", |b| {
            let line = b.mid();
            Box::new(move |state| {
                state.execute_split_line(
                    line,
                    SplitAt::Station { station_index: 25 },
                    SplitLineOptions {
                        name_a: "A".to_string(),
                        name_b: "B".to_string(),
                        color_a: "#111111".to_string(),
                        color_b: "#222222".to_string(),
                        comment_side: SplitSide::A,
                        datum_side: SplitSide::A,
                    },
                )
            })
        }),
        ("combine lines", |b| {
            let candidate = b
                .state
                .combine_candidates(StationRef::new(b.leaf, 0))
                .unwrap()
                .into_iter()
                .find(|c| c.blocked_reason.is_none())
                .expect("the leaf can combine");
            Box::new(move |state| {
                state.execute_combine_lines(
                    candidate.line_a,
                    candidate.line_b,
                    CombineLinesOptions {
                        name: "Combined".to_string(),
                        color: "#333333".to_string(),
                        swap_lr: false,
                        comment_pick: CombinePick::Kept,
                        name_pick: CombinePick::Kept,
                        join: None,
                    },
                )
            })
        }),
        ("merge stations", |b| {
            command(Command::MergeStations {
                station_a: StationRef::new(b.island, 0),
                station_b: StationRef::new(b.mid(), 10),
            })
        }),
        ("detach (unmerge stations)", |b| {
            command(Command::UnmergeStations {
                station_ref: StationRef::new(b.mid(), 0),
            })
        }),
        ("move station", |b| {
            command(Command::MoveStation {
                station_ref: StationRef::new(b.mid(), 30),
                offset: Coordinates::new(0.4, 0.3, 0.0),
            })
        }),
        ("datum edit (entrance GPS)", |b| {
            command(Command::SetZeroDatum {
                station_ref: StationRef::new(b.lines[0], 0),
                coordinates: GpsCoordinates {
                    latitude: 20.5001,
                    longitude: -87.2501,
                    depth_reference: 0.0,
                },
                accuracy_m: None,
                entrance: None,
            })
        }),
        ("reverse line", |b| {
            command(Command::ReverseLine {
                line_id: b.mid(),
                swap_left_right: false,
            })
        }),
        ("paste 1,000 cells (LRUD)", |b| {
            paste(b, paste_lines(b), false)
        }),
        ("paste 1,000 cells (length)", |b| {
            paste(b, paste_lines(b), true)
        }),
    ]
}

/// Each action `runs()` times: do, undo, redo (each timed), then an untimed
/// undo so the next run starts from the same survey.
fn measure_actions(b: &Bench, label: &str, report: &mut Report) {
    for (name, prepare) in actions() {
        let mut times = [Vec::new(), Vec::new(), Vec::new()];
        for _ in 0..runs() {
            let run = prepare(b);
            let (result, forward) = millis(|| run(&b.state));
            assert!(result.success, "{name}: {:?}", result.error);
            let (undone, undo) = millis(|| b.state.undo());
            assert!(undone.success, "{name}: undo {:?}", undone.error);
            let (redone, redo) = millis(|| b.state.redo());
            assert!(redone.success, "{name}: redo {:?}", redone.error);
            assert!(b.state.undo().success, "{name}: second undo");
            times[0].push(forward);
            times[1].push(undo);
            times[2].push(redo);
        }
        let [a, u, r] = times;
        report.add(
            &format!("{label} {name}"),
            [median(a), median(u), median(r)],
        );
    }
}

fn measure_reads(b: &Bench, label: &str, report: &mut Report) {
    let mut state = Vec::new();
    let mut gas = Vec::new();
    let diver = Diver::new(20.0, None).unwrap();
    let cylinders = vec![Cylinder::new(
        11.1,
        232.0,
        232.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap()];
    let mut config = GasConfig::default();
    config.normalize();
    let origin = StationRef::new(b.lines[0], 0);
    for _ in 0..runs() {
        // A fresh revision each time, as after an edit (the layout cache
        // is keyed by revision).
        let r = b.state.execute(Command::UpdateShot {
            line_id: b.mid(),
            shot_id: b.shot_ids(b.mid())[3],
            updates: ShotUpdates {
                comment: Some(Some(format!("rev {}", state.len()))),
                ..Default::default()
            },
        });
        assert!(r.success);
        state.push(millis(|| b.project_state_json()).1);
        gas.push(
            millis(|| {
                b.state
                    .gas_reach_field(
                        Some(&origin),
                        None,
                        &[],
                        &[],
                        &diver,
                        &cylinders,
                        &config,
                        &[],
                    )
                    .unwrap()
            })
            .1,
        );
    }
    report.add(
        &format!("{label} get_project_state (+ JSON)"),
        [median(state), 0.0, 0.0],
    );
    report.add(&format!("{label} gas reach field"), [median(gas), 0.0, 0.0]);
}

fn measure_open_save(b: &Bench, lines: usize, label: &str, report: &mut Report) {
    let dir = TempDir::new().unwrap();
    let legacy = legacy_file(dir.path(), lines);
    let mut open_legacy = Vec::new();
    let mut first_save = Vec::new();
    let mut open_v2 = Vec::new();
    let mut envelope = Vec::new();
    for k in 0..runs() {
        let copy = dir.path().join(format!("legacy-{k}.stalx"));
        std::fs::copy(&legacy, &copy).unwrap();
        let state = fresh_state();
        open_legacy.push(millis(|| state.do_load_project_full(copy.clone()).unwrap()).1);
        // The first save of a legacy file: the upgrade (backup, v2 write).
        assert!(
            state
                .execute(Command::UpdateShot {
                    line_id: b.mid(),
                    shot_id: b.shot_ids(b.mid())[3],
                    updates: ShotUpdates {
                        comment: Some(Some("dirty".to_string())),
                        ..Default::default()
                    },
                })
                .success
        );
        let (saved, ms) = millis(|| state.execute(Command::SaveProject { path: None }));
        assert!(saved.success, "upgrade save: {:?}", saved.error);
        first_save.push(ms);

        let state = fresh_state();
        let (result, ms) = millis(|| state.do_load_project_full(b.path.clone()).unwrap());
        open_v2.push(ms);
        envelope.push(millis(|| crate::core::load_binary::encode(result).unwrap().len()).1);
    }
    report.add(
        &format!("{label} open legacy file (upgrade-open)"),
        [median(open_legacy), 0.0, 0.0],
    );
    report.add(
        &format!("{label} open v2 file"),
        [median(open_v2), 0.0, 0.0],
    );
    report.add(
        &format!("{label} binary envelope encode"),
        [median(envelope), 0.0, 0.0],
    );
    report.add(
        &format!("{label} first save of a legacy file"),
        [median(first_save), 0.0, 0.0],
    );

    let mut save = Vec::new();
    for k in 0..runs() {
        assert!(
            b.state
                .execute(Command::UpdateShot {
                    line_id: b.mid(),
                    shot_id: b.shot_ids(b.mid())[4],
                    updates: ShotUpdates {
                        comment: Some(Some(format!("save {k}"))),
                        ..Default::default()
                    },
                })
                .success
        );
        let (saved, ms) = millis(|| b.state.execute(Command::SaveProject { path: None }));
        assert!(saved.success, "save: {:?}", saved.error);
        save.push(ms);
    }
    report.add(&format!("{label} save"), [median(save), 0.0, 0.0]);
}

// ----- the large import -----------------------------------------------------------

/// A synthetic Compass `.dat`: `surveys` surveys of 50 shots in the gate
/// survey's tree shape, each starting on a station of its parent survey.
fn compass_file(path: &Path, surveys: usize) {
    let mut text = String::new();
    for s in 0..surveys {
        let _ = write!(
            text,
            "Synthetic Cave\r\nSURVEY NAME: S{s}\r\nSURVEY DATE: 7 10 1979\r\nSURVEY TEAM:\r\n\
             Nobody\r\nDECLINATION: 0.00  FORMAT: DMMDLRUDLADN  CORRECTIONS: 0.00 0.00 0.00\r\n\r\n\
             FROM TO LENGTH BEARING INC LEFT UP DOWN RIGHT FLAGS COMMENTS\r\n\r\n"
        );
        let start = if s == 0 {
            "S0_0".to_string()
        } else {
            let parent = (s - 1) / 2;
            format!("S{parent}_{}", 1 + (s * 7) % (SHOTS_PER_LINE - 1))
        };
        for i in 0..SHOTS_PER_LINE {
            let from = if i == 0 {
                start.clone()
            } else {
                format!("S{s}_{i}")
            };
            let shot = gate_shot(s, i);
            let _ = write!(
                text,
                "{from} S{s}_{} {:.2} {:.2} {:.2} 1.50 0.75 1.25 2.00\r\n",
                i + 1,
                shot.length * 3.28084,
                shot.azimuth_in,
                -2.0 + (i % 5) as f64,
            );
        }
        text.push_str("\x0c\r\n");
    }
    std::fs::write(path, text).unwrap();
}

fn measure_import(surveys: usize, label: &str, report: &mut Report) {
    use crate::parsers::compass::{
        convert_compass_to_import_configs, parse_compass, CompassImportOptions,
    };
    let dir = TempDir::new().unwrap();
    let dat = dir.path().join("synthetic.dat");
    compass_file(&dat, surveys);
    let mut times = [Vec::new(), Vec::new(), Vec::new()];
    for k in 0..runs() {
        let state = fresh_state();
        let created = state.execute(Command::CreateProject {
            name: format!("Import {k}"),
            location: None,
        });
        assert!(created.success, "create: {:?}", created.error);
        let (result, ms) = millis(|| {
            let parsed = parse_compass(&dat).unwrap();
            let opts = CompassImportOptions {
                include_excluded: false,
                apply_corrections: true,
                comments_on_stations: false,
            };
            let (lines, closures, _) = convert_compass_to_import_configs(&parsed, &opts).unwrap();
            state.execute_import_lines(
                lines,
                closures,
                None,
                Vec::new(),
                Vec::new(),
                super::VirtualShotMode::Auto,
            )
        });
        assert!(result.success, "import: {:?}", result.error);
        times[0].push(ms);
        let (undone, ms) = millis(|| state.undo());
        assert!(undone.success, "import undo: {:?}", undone.error);
        times[1].push(ms);
        let (redone, ms) = millis(|| state.redo());
        assert!(redone.success, "import redo: {:?}", redone.error);
        times[2].push(ms);
    }
    let [a, u, r] = times;
    report.add(
        &format!(
            "{label} Compass import ({} shots)",
            surveys * SHOTS_PER_LINE
        ),
        [median(a), median(u), median(r)],
    );
}

fn bench(lines: usize) {
    let label = if lines > 300 { "20k" } else { "5k" };
    println!(
        "{label}: {lines} lines, {} runs (ms: do / undo / redo)",
        runs()
    );
    let mut report = Report::default();
    let b = Bench::open(lines);
    measure_open_save(&b, lines, label, &mut report);
    measure_reads(&b, label, &mut report);
    measure_actions(&b, label, &mut report);
    measure_import(lines / 2, label, &mut report);
    report.write();
}

#[test]
#[ignore = "a release-build benchmark: cargo test --release ... perf_bench -- --ignored"]
fn perf_bench_5k() {
    bench(100);
}

#[test]
#[ignore = "a release-build benchmark: cargo test --release ... perf_bench -- --ignored"]
fn perf_bench_20k() {
    bench(400);
}

// ----- where one edit's time goes --------------------------------------------------

/// Median milliseconds of `run` over `runs()` runs, each on a fresh `setup`
/// (untimed).
fn timed<T>(mut setup: impl FnMut() -> T, mut run: impl FnMut(T)) -> f64 {
    let mut v = Vec::new();
    for _ in 0..runs() {
        let input = setup();
        let started = Instant::now();
        run(input);
        v.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    median(v)
}

/// The pieces of one recorded edit on the 20,000-shot survey, each timed
/// on its own: the pipeline's clone, validation and capture, the model's
/// anchor pass, the frozen-shot check and the record.
#[test]
#[ignore = "a release-build benchmark: cargo test --release ... perf_parts -- --ignored"]
fn perf_parts_20k() {
    use crate::models::survey::changeset::capture;
    use crate::models::survey::changeset::testkit::gate_survey;
    use crate::models::survey::{EditContext, ShotEditRun};
    use std::hint::black_box;
    let g = gate_survey(400, None);
    let p = g.project;
    let line = g.lines[201];
    let shot = p.line(line).unwrap().shots[5].id;
    let length = ShotUpdates {
        length: Some(9.5),
        ..Default::default()
    };
    let comment = ShotUpdates {
        comment: Some(Some("x".to_string())),
        ..Default::default()
    };
    let mut report = Report::default();
    let mut add = |name: &str, ms: f64| report.add(name, [ms, 0.0, 0.0]);
    add("clone", timed(|| (), |()| drop(black_box(p.clone()))));
    add("validate", timed(|| (), |()| drop(black_box(p.validate()))));
    add(
        "topology (rebuilt)",
        timed(
            || {
                let mut q = p.clone();
                let _ = q.lines_mut();
                q
            },
            |q| {
                black_box(q.topology().components().len());
            },
        ),
    );
    add(
        "raw_positions",
        timed(|| (), |()| drop(black_box(p.raw_positions()))),
    );
    add(
        "anchor_snapshot",
        timed(|| (), |()| drop(black_box(p.anchor_snapshot()))),
    );
    let before = p.anchor_snapshot();
    add(
        "settle_anchors (geometry)",
        timed(
            || p.clone(),
            |mut q| {
                drop(black_box(
                    q.settle_anchors(&before, &EditContext::geometry(line)),
                ));
            },
        ),
    );
    add(
        "frozen check (alignment + frozen_shots)",
        timed(
            || (),
            |()| {
                let aligned = p.closure_alignment(&p.raw_positions());
                drop(black_box(p.frozen_shots(&aligned)));
            },
        ),
    );
    let mut edited = p.clone();
    edited.update_shot(line, shot, &comment).unwrap();
    add(
        "capture (one comment)",
        timed(|| (), |()| drop(black_box(capture(&p, &edited)))),
    );
    add(
        "model update_shot (length)",
        timed(
            || p.clone(),
            |mut q| drop(black_box(q.update_shot(line, shot, &length).unwrap())),
        ),
    );
    add(
        "run: frozen check + update (length) + finish",
        timed(
            || p.clone(),
            |mut q| {
                let mut run = ShotEditRun::new(&mut q);
                black_box(run.is_frozen(line, 5));
                run.update_shot(line, shot, &length).unwrap();
                run.finish().unwrap();
            },
        ),
    );
    let cmd = Command::UpdateShot {
        line_id: line,
        shot_id: shot,
        updates: comment,
    };
    add(
        "record (label, refs, preview)",
        timed(
            || (),
            |()| drop(black_box(super::commands::Record::of(&p, &cmd))),
        ),
    );
    report.write();
}

/// The pieces of Combine Lines and of a legacy upgrade-open on the
/// 20,000-shot survey.
#[test]
#[ignore = "a release-build benchmark: cargo test --release ... perf_parts -- --ignored"]
fn perf_parts_combine_open_20k() {
    use std::hint::black_box;
    let b = Bench::open(400);
    let p = b.state.get_project_state().unwrap();
    let candidate = b
        .state
        .combine_candidates(StationRef::new(b.leaf, 0))
        .unwrap()
        .into_iter()
        .find(|c| c.blocked_reason.is_none())
        .unwrap();
    let options = CombineLinesOptions {
        name: "Combined".to_string(),
        color: "#333333".to_string(),
        swap_lr: false,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Kept,
        join: None,
    };
    let (kept, other) = (candidate.line_a, candidate.line_b);
    let mut report = Report::default();
    let mut add = |name: &str, ms: f64| report.add(name, [ms, 0.0, 0.0]);
    add(
        "combine: model combine_lines",
        timed(
            || p.clone(),
            |mut q| drop(black_box(q.combine_lines(kept, other, &options).unwrap())),
        ),
    );
    let mut q = p.clone();
    let edited = q.combine_lines(kept, other, &options).unwrap();
    add(
        "combine: combine_plan_wire",
        timed(
            || (),
            |()| drop(black_box(p.combine_plan_wire(&edited.value))),
        ),
    );
    add(
        "combine: candidates at the leaf",
        timed(
            || (),
            |()| {
                drop(black_box(
                    b.state.combine_candidates(StationRef::new(b.leaf, 0)),
                ));
            },
        ),
    );
    add(
        "combine: execute_combine_lines + undo",
        timed(
            || (),
            |()| {
                assert!(
                    b.state
                        .execute_combine_lines(kept, other, options.clone())
                        .success
                );
                assert!(b.state.undo().success);
            },
        ),
    );
    let dir = TempDir::new().unwrap();
    let legacy = legacy_file(dir.path(), 400);
    add(
        "open: frozen legacy read",
        timed(
            || (),
            |()| {
                let options = stalagmight_legacy::OpenOptions::default();
                drop(black_box(
                    stalagmight_legacy::open_path(&legacy, &options).unwrap(),
                ));
            },
        ),
    );
    add(
        "open: frozen legacy read, no adjusted view",
        timed(
            || (),
            |()| {
                let options = stalagmight_legacy::OpenOptions {
                    with_adjusted: false,
                    ..Default::default()
                };
                drop(black_box(
                    stalagmight_legacy::open_path(&legacy, &options).unwrap(),
                ));
            },
        ),
    );
    let config = crate::models::config::ClosureAdjustmentConfig::default();
    add(
        "open: migrate_path_for_app (read + convert + verify)",
        timed(
            || (),
            |()| {
                drop(black_box(
                    crate::persistence::migrate::migrate_path_for_app(&legacy, &config).unwrap(),
                ));
            },
        ),
    );
    add(
        "open: v2 read_project_file",
        timed(fresh_state, |state| {
            drop(black_box(state.read_project_file(&b.path).unwrap()));
        }),
    );
    report.write();
}
