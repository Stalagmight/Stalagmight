//! [#576 TML-DEPTH-D1, D2] Junction depths, end to end: a project exported
//! with `export_tml`, read back through our own parser and importer.
//!
//! A junction whose readings disagree is written at their mean, every other
//! station at its true depth, and each shot touching the junction with the
//! length that keeps its measured horizontal run. These go through the public
//! export alone, so they run unchanged against the re-hang this replaced.

#![expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: a failed assumption panics, and the printed values are the evidence"
)]

use std::path::Path;

use crate::models::test_fixtures::straight_shot;
use crate::models::{
    ImportSectionConfig, Project, SectionData, ShotData, StationMerge, StationRef,
};
use crate::parsers::tml::schema::DEPTH_IN_SAME_AS_PREVIOUS;
use crate::parsers::tml::{
    convert_tml_with_options, parse_tml, TmlFile, TmlImportOptions, TmlShot,
};

use super::export_tml;

fn uuid(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

/// A shot from `depth_in` to `depth_out` along `azimuth`, identified by `n`.
fn shot(n: u32, azimuth: f64, length: f64, depth_in: f64, depth_out: f64) -> ShotData {
    straight_shot(azimuth, length)
        .with_depths(depth_in, depth_out)
        .with_uuid(&uuid(n))
}

fn section(project: &mut Project, name: &str, shots: Vec<ShotData>) {
    project.add_section(SectionData {
        name: Some(name.to_string()),
        color: "#336699".to_string(),
        shots,
        ..Default::default()
    });
}

fn merge(project: &mut Project, a: (u32, u32), b: (u32, u32)) {
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a.0, a.1).into(),
        station_b: StationRef::new(b.0, b.1).into(),
        merged_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
    });
}

/// What one export says: its notes, the rows as written, the re-import.
struct Exported {
    notes: Vec<String>,
    file: TmlFile,
    configs: Vec<ImportSectionConfig>,
}

impl Exported {
    fn of(project: &Project) -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("junctions.tmlu");
        let notes = export_tml(project, &path).unwrap();
        Self::read(notes, &path)
    }

    fn read(notes: Vec<String>, path: &Path) -> Self {
        let file = parse_tml(path).unwrap();
        let all = TmlImportOptions {
            include_excluded: true,
            ..TmlImportOptions::default()
        };
        let (configs, _, _) = convert_tml_with_options(&file, &all).unwrap();
        Self {
            notes,
            file,
            configs,
        }
    }

    /// The written row of shot `n`.
    fn row(&self, n: u32) -> &TmlShot {
        let id = uuid(n);
        self.file
            .sections
            .iter()
            .flat_map(|s| &s.shots)
            .find(|s| s.uuid.as_deref() == Some(id.as_str()))
            .unwrap()
    }

    /// Shot `n` as the importer reads it back.
    fn imported(&self, n: u32) -> &ShotData {
        let id = uuid(n);
        self.configs
            .iter()
            .flat_map(|c| &c.shots)
            .find(|s| s.uuid.as_deref() == Some(id.as_str()))
            .unwrap()
    }
}

fn assert_close(what: &str, got: f64, want: f64) {
    assert!(
        // Six decimals in the file.
        (got - want).abs() < 6e-7,
        "{what}: expected {want}, got {got}"
    );
}

/// The new length keeps the measured run `h`: `sqrt(h^2 + dz^2)`, rounded up
/// to the next written micrometre and no further.
fn assert_keeps_run(what: &str, length: f64, run: f64, exported_change: f64) {
    let exact = run.hypot(exported_change);
    assert!(
        length >= exact - 1e-12 && length <= exact + 1e-6 + 1e-12,
        "{what}: a length of {length} does not keep the {run} m run over a {exported_change} m \
         change (expected {exact}, rounded up by at most a micrometre)"
    );
}

/// The measured horizontal run of a shot, as the model derives it.
fn run_of(length: f64, depth_in: f64, depth_out: f64) -> f64 {
    let dz = depth_out - depth_in;
    (length * length - dz * dz).max(0.0).sqrt()
}

/// The decided example: main-line station A at 10.0 m, a branch's first shot
/// from 12.0 to 12.5 m.
fn example_project() -> Project {
    let mut project = Project::new("CaveA".to_string(), None);
    section(
        &mut project,
        "Main",
        vec![
            shot(1, 0.0, 4.0, 9.0, 10.0),
            shot(2, 90.0, 3.0, 10.0, 9.5),
            shot(3, 90.0, 2.0, 9.5, 10.0),
        ],
    );
    section(
        &mut project,
        "Branch",
        vec![
            shot(11, 180.0, 2.0, 12.0, 12.5),
            shot(12, 200.0, 3.0, 12.5, 13.0),
        ],
    );
    merge(&mut project, (0, 1), (1, 0));
    project
}

#[test]
fn the_example_junction_is_exported_at_the_mean_of_its_readings() {
    let out = Exported::of(&example_project());

    // A (the station shot 1 ends at) is the mean of 10.0 and 12.0.
    assert_close("junction A", out.row(1).depth, 11.0);
    // Into A: 9.0 -> 11.0, so it absorbs +1.0 m over the same 3.873 m run.
    assert_keeps_run(
        "main shot into A",
        out.row(1).length,
        run_of(4.0, 9.0, 10.0),
        2.0,
    );
    // Out of A: 11.0 -> 9.5, absorbing -1.0 m.
    assert_close("main shot out of A", out.row(2).depth, 9.5);
    assert_eq!(out.row(2).depth_in, DEPTH_IN_SAME_AS_PREVIOUS);
    assert_keeps_run(
        "main shot out of A",
        out.row(2).length,
        run_of(3.0, 10.0, 9.5),
        -1.5,
    );
    // The branch's first shot starts at A's one depth and ends at its true one.
    assert_close("branch first shot", out.row(11).depth, 12.5);
    assert_eq!(out.row(11).depth_in, DEPTH_IN_SAME_AS_PREVIOUS);
    assert_keeps_run(
        "branch first shot",
        out.row(11).length,
        run_of(2.0, 12.0, 12.5),
        1.5,
    );

    // Every later shot keeps its true depth and its measured tape.
    for (n, depth, length) in [(3, 10.0, 2.0), (12, 13.0, 3.0)] {
        assert_close(&format!("later shot {n}"), out.row(n).depth, depth);
        assert_close(&format!("later shot {n} length"), out.row(n).length, length);
    }

    // Our importer reads the same geometry back: A at its mean, the branch
    // shot from it with its plan intact, the later shot exactly as measured.
    let branch = out.imported(11);
    assert_close("re-imported branch start", branch.depth_in, 11.0);
    assert_close("re-imported branch end", branch.depth_out, 12.5);
    assert!((branch.horizontal_length() - run_of(2.0, 12.0, 12.5)).abs() < 1e-5);
    let later = out.imported(12);
    assert_close("re-imported later start", later.depth_in, 12.5);
    assert_close("re-imported later end", later.depth_out, 13.0);
    assert_close("re-imported later length", later.length, 3.0);
    assert_close("re-imported main out of A", out.imported(2).depth_in, 11.0);

    // D2: the receipt says how many junctions and the widest disagreement.
    assert!(
        out.notes
            .contains(&"junction-depths-averaged?count=1&largest=2.00".to_string()),
        "the export receipt names the averaged junction, got {:?}",
        out.notes
    );
}

#[test]
fn several_branches_at_one_junction_average_every_start() {
    let mut project = Project::new("CaveA".to_string(), None);
    section(
        &mut project,
        "Main",
        vec![shot(1, 0.0, 5.0, 8.0, 10.0), shot(2, 0.0, 5.0, 10.0, 11.0)],
    );
    // Three branches at Main station 1: one agrees, two do not.
    for (i, start) in [10.0, 12.0, 14.0].into_iter().enumerate() {
        let n = 10 * (i as u32 + 1);
        section(
            &mut project,
            &format!("Branch{i}"),
            vec![
                shot(n + 1, 45.0, 4.0, start, start + 1.0),
                shot(n + 2, 45.0, 4.0, start + 1.0, start + 2.0),
            ],
        );
        merge(&mut project, (0, 1), (i as u32 + 1, 0));
    }

    let out = Exported::of(&project);
    let mean = (10.0 + 10.0 + 12.0 + 14.0) / 4.0;
    assert_close("junction", out.row(1).depth, mean);
    for (i, start) in [10.0, 12.0, 14.0].into_iter().enumerate() {
        let n = 10 * (i as u32 + 1);
        let first = out.row(n + 1);
        assert_close("branch end", first.depth, start + 1.0);
        assert_eq!(first.depth_in, DEPTH_IN_SAME_AS_PREVIOUS);
        assert_keeps_run(
            &format!("branch {i} first shot"),
            first.length,
            run_of(4.0, start, start + 1.0),
            start + 1.0 - mean,
        );
        assert_close("later branch shot", out.row(n + 2).depth, start + 2.0);
        assert_close("later branch length", out.row(n + 2).length, 4.0);
    }
    assert!(
        out.notes
            .contains(&"junction-depths-averaged?count=1&largest=4.00".to_string()),
        "got {:?}",
        out.notes
    );
}

#[test]
fn a_station_merged_across_sections_counts_every_section_hung_there() {
    // C is merged onto B's first station, which is itself merged onto Main
    // station 1: one junction, three readings.
    let mut project = Project::new("CaveA".to_string(), None);
    section(
        &mut project,
        "Main",
        vec![shot(1, 0.0, 5.0, 9.0, 10.0), shot(2, 0.0, 5.0, 10.0, 10.0)],
    );
    section(&mut project, "B", vec![shot(11, 90.0, 3.0, 11.0, 11.5)]);
    section(&mut project, "C", vec![shot(21, 270.0, 3.0, 13.0, 12.0)]);
    merge(&mut project, (0, 1), (1, 0));
    merge(&mut project, (1, 0), (2, 0));

    let out = Exported::of(&project);
    let mean = (10.0 + 11.0 + 13.0) / 3.0;
    assert_close("junction", out.row(1).depth, mean);
    assert_keeps_run(
        "B",
        out.row(11).length,
        run_of(3.0, 11.0, 11.5),
        11.5 - mean,
    );
    assert_keeps_run(
        "C",
        out.row(21).length,
        run_of(3.0, 13.0, 12.0),
        12.0 - mean,
    );
    assert_close("B end", out.row(11).depth, 11.5);
    assert_close("C end", out.row(21).depth, 12.0);
    assert!(
        out.notes
            .contains(&"junction-depths-averaged?count=1&largest=3.00".to_string()),
        "got {:?}",
        out.notes
    );
}

#[test]
fn a_section_merged_part_way_along_is_one_reading_of_its_junction() {
    // Mid is merged at its station 1, so it leaves the junction twice: a
    // forward block (shot 22 onwards) and a reversed one (shot 21 walked
    // back). Mid arrives there at 12.0 and leaves at 12.4, a second reading
    // inside the section. The section counts once, with the depth it arrives
    // with: (10 + 12) / 2, not (10 + 12.4 + 12) / 3.
    let mut project = Project::new("CaveA".to_string(), None);
    section(
        &mut project,
        "Main",
        vec![shot(1, 0.0, 5.0, 9.0, 10.0), shot(2, 0.0, 5.0, 10.0, 10.0)],
    );
    section(
        &mut project,
        "Mid",
        vec![
            shot(21, 90.0, 3.0, 11.0, 12.0),
            shot(22, 90.0, 3.0, 12.4, 11.5),
            shot(23, 90.0, 3.0, 11.5, 11.0),
        ],
    );
    merge(&mut project, (0, 1), (1, 1));

    let out = Exported::of(&project);
    let mean = f64::midpoint(10.0, 12.0);
    assert_close("junction", out.row(1).depth, mean);
    // The forward piece: 12.4 -> 11.5 measured, now from the mean.
    assert_close("forward piece end", out.row(22).depth, 11.5);
    assert_eq!(out.row(22).depth_in, DEPTH_IN_SAME_AS_PREVIOUS);
    assert_keeps_run(
        "forward piece",
        out.row(22).length,
        run_of(3.0, 12.4, 11.5),
        11.5 - mean,
    );
    // The reversed piece: shot 21 walked back, 12.0 -> 11.0, now from the mean.
    assert_close("reversed piece end", out.row(21).depth, 11.0);
    assert_eq!(out.row(21).depth_in, DEPTH_IN_SAME_AS_PREVIOUS);
    assert_keeps_run(
        "reversed piece",
        out.row(21).length,
        run_of(3.0, 11.0, 12.0),
        11.0 - mean,
    );
    // Further along, Mid keeps its true depths and its tape.
    assert_close("later Mid shot", out.row(23).depth, 11.0);
    assert_close("later Mid shot length", out.row(23).length, 3.0);
    assert!(
        out.notes
            .contains(&"junction-depths-averaged?count=1&largest=2.00".to_string()),
        "got {:?}",
        out.notes
    );
}

#[test]
fn a_zero_length_tie_at_a_junction_takes_its_new_drop_as_its_length() {
    // A tie has no horizontal run (h = 0), so its new length is the whole
    // exported change: L' = |dz'|.
    let mut project = Project::new("CaveA".to_string(), None);
    section(
        &mut project,
        "Main",
        vec![shot(1, 0.0, 5.0, 9.0, 10.0), shot(2, 0.0, 5.0, 10.0, 10.0)],
    );
    section(
        &mut project,
        "Tie",
        vec![
            shot(11, 0.0, 0.0, 12.0, 12.0),
            shot(12, 90.0, 4.0, 12.0, 13.0),
        ],
    );
    merge(&mut project, (0, 1), (1, 0));

    let out = Exported::of(&project);
    assert_close("junction", out.row(1).depth, 11.0);
    let tie = out.row(11);
    assert_close("tie end", tie.depth, 12.0);
    assert_close("tie length", tie.length, 1.0);
    assert_close("after the tie", out.row(12).depth, 13.0);
    assert_close("after the tie, length", out.row(12).length, 4.0);
    // Still a tie when read back: no horizontal run appeared.
    assert!(out.imported(11).horizontal_length() < 1e-6);
}

#[test]
fn a_junction_whose_readings_agree_is_written_as_measured() {
    let mut project = example_project();
    let branch = &mut project.sections[1].shots;
    (branch[0].depth_in, branch[0].depth_out) = (10.0, 10.5);
    (branch[1].depth_in, branch[1].depth_out) = (10.5, 11.0);

    let out = Exported::of(&project);
    assert_close("junction", out.row(1).depth, 10.0);
    for (n, length) in [(1, 4.0), (2, 3.0), (11, 2.0), (12, 3.0)] {
        assert_close(&format!("shot {n} length"), out.row(n).length, length);
    }
    assert!(
        !out.notes.iter().any(|n| n.starts_with("junction-depths")),
        "nothing to report, got {:?}",
        out.notes
    );
}

/// [#576 RVF-d-7] A vertical shot goes out vertical: its tape as written is its
/// drop as written, so it comes back with no run. In binary 13.3 - 10.1 is
/// 3.200000000000001, a hair over the 3.2 m tape, which sent the shot down the
/// broken-data path and wrote it 3.200001 long: a 2.5 mm run on re-import where
/// the survey has none.
#[test]
fn a_vertical_shot_is_written_with_its_drop_as_its_tape_and_no_run() {
    let mut project = Project::new("CaveA".to_string(), None);
    section(
        &mut project,
        "Main",
        vec![
            shot(1, 0.0, 12.0, 0.0, 10.1),
            shot(2, 90.0, 3.2, 10.1, 13.3),
        ],
    );

    let out = Exported::of(&project);
    let (row, back) = (out.row(2), out.imported(2));
    println!(
        "[RVF-d-7] vertical shot: written length {} over {} -> {}; re-imported run {:e} m",
        row.length,
        back.depth_in,
        row.depth,
        back.horizontal_length()
    );
    assert_close("vertical shot end", row.depth, 13.3);
    assert_eq!(row.length, 3.2, "written with its 3.2 m drop as its tape");
    assert_eq!(back.horizontal_length(), 0.0, "comes back vertical");
}

/// [#576 RVF-d-8] A refitted shot with a recorded pitch states its new slope.
/// The main-line shot out of junction A (10.0 -> 11.0 on a 5 m tape, pitched
/// 11.537 degrees down, as its own geometry says) is refitted to 12.0 -> 11.0
/// when A goes out at its mean: same run, now climbing. It kept its old
/// Inclination, so the re-import read it as pitched down while its depths
/// climb, 23 degrees apart, which the validator flags as
/// INCLINATION_DEPTH_MISMATCH (15). A refitted shot with no pitch keeps none.
#[test]
fn a_refitted_shot_with_a_recorded_pitch_is_written_with_its_new_slope() {
    let mut project = Project::new("CaveA".to_string(), None);
    let pitch = crate::models::implied_pitch_deg(5.0, 10.0, 11.0);
    section(
        &mut project,
        "Main",
        vec![
            shot(1, 0.0, 4.0, 9.0, 10.0),
            shot(2, 90.0, 5.0, 10.0, 11.0).with_pitches(Some(pitch), Some(pitch)),
        ],
    );
    section(
        &mut project,
        "Branch",
        vec![shot(11, 180.0, 2.0, 14.0, 14.5)],
    );
    merge(&mut project, (0, 1), (1, 0));

    let out = Exported::of(&project);
    let back = out.imported(2);
    let implied = crate::models::implied_pitch_deg(back.length, back.depth_in, back.depth_out);
    println!(
        "[RVF-d-8] refitted shot: {} -> {} over {}, pitch {:?}, implied {implied}",
        back.depth_in, back.depth_out, back.length, back.pitch_in
    );
    assert_close("refitted start", back.depth_in, 12.0);
    assert_close("refitted end", back.depth_out, 11.0);
    let written = back.pitch_in.unwrap();
    assert!(
        (written - implied).abs() < 1e-3,
        "the refitted shot states a pitch of {written} where its depths and tape imply {implied}"
    );
    assert!(implied < 0.0, "the refitted shot climbs");
    // Refitted too (9.0 -> 12.0), with no pitch recorded: still none.
    assert_eq!(out.imported(1).pitch_in, None);
}
