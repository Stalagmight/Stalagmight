//! [#555] A project may not mint more line ids than the shot-id encode has
//! room for.
//!
//! A shot id is `line_id * FEATURE_ID_STRIDE + position` in `u32`, so the
//! last line id that can hold a FULL line (`FEATURE_ID_STRIDE - 1` shots)
//! is [`MAX_LINE_ID`] (42_948) and the last usable COUNT is
//! [`MAX_LINES_IN_ID_SPACE`] (42_949).
//!
//! The places that MINT ids are pinned here: `create_line` (the per-line
//! door every command comes through), the import funnel (the batch door every
//! importer comes through), Merge Projects, and the two delete-and-split
//! paths, which mint a line id without going through `create_line` at all.
//!
//! On the station-graph model the next line id is derived (`max + 1`), not a
//! stored counter, so the precondition "N ids are spent" is one line whose id
//! is `N - 1`, re-keyed by [`with_line_id`]; 42k real lines are not needed.
//! The merge test is the exception - its check sums the SOURCES' line counts,
//! which nothing but real lines can supply.
//!
//! The create, import and merge cases are other packages' doors and are
//! `#[ignore]`d until those packages land (their reasons say which).

use tempfile::TempDir;

use super::{AppState, VirtualShotMode};
use crate::core::{Command, CommandResult};
use crate::models::survey::{LineMeta, Project, StartAt};
use crate::models::{
    ImportLineConfig, ImportPlacement, LineId, ShotData, MAX_LINES_IN_ID_SPACE, MAX_LINE_ID,
};
use crate::persistence::stalx_v2::{self, Extras, Mode};

fn shot(length: f64) -> ShotData {
    ShotData {
        azimuth_in: 45.0,
        azimuth_out: 225.0,
        length,
        depth_in: 10.0,
        depth_out: 11.0,
        ..ShotData::default()
    }
}

fn meta(name: &str) -> LineMeta {
    LineMeta {
        name: Some(name.to_string()),
        color: "#4ecdc4".to_string(),
        ..LineMeta::default()
    }
}

/// A free line named `name` (placed at the origin) re-keyed to line id `id`.
fn with_line_id(p: &mut Project, name: &str, shots: Vec<ShotData>, id: LineId) -> LineId {
    let line = p.add_line(meta(name), StartAt::New, shots).unwrap();
    let start = p.line(line).unwrap().start;
    p.set_placement(start, 0.0, 0.0).unwrap();
    let position = p.line_position(line).unwrap();
    let mut taken = p.lines_mut().remove(position);
    taken.id = id;
    p.insert_line(taken).unwrap()
}

fn state_with(project: Project) -> AppState {
    let violations = project.validate();
    assert!(violations.is_empty(), "invalid test survey: {violations:?}");
    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);
    state
}

/// A project whose highest line id is `spent - 1`: `spent` ids are handed
/// out, and the next one minted is `spent`.
fn state_with_spent_line_ids(spent: u32) -> AppState {
    let mut project = Project::new("Bound", None);
    with_line_id(&mut project, "Marker", Vec::new(), spent - 1);
    state_with(project)
}

fn create_one_line(state: &AppState) -> CommandResult {
    state.execute(Command::CreateLine {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
        cave_section_id: None,
    })
}

fn import_config(index: u32) -> ImportLineConfig {
    ImportLineConfig {
        cave_section: None,
        reverse: false,
        line_index: index,
        name: Some(format!("Imported {index}")),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![shot(3.0)],
        station_comments: Vec::new(),
        station_names: Vec::new(),
        placement: ImportPlacement::Origin,
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    }
}

/// The LAST id is usable: 42_948 * 100_000 + 99_999 = 4_294_899_999, which
/// still fits `u32`. An off-by-one guard would refuse this one.
#[test]
fn the_last_encodable_line_id_is_still_minted() {
    let state = state_with_spent_line_ids(MAX_LINE_ID);
    let result = create_one_line(&state);
    assert!(
        result.success,
        "id {MAX_LINE_ID} still encodes and must be allowed: {:?}",
        result.error
    );
    assert_eq!(result.line_id, Some(MAX_LINE_ID));
}

/// One past it is refused.
#[test]
fn minting_one_line_past_the_encode_is_refused() {
    let state = state_with_spent_line_ids(MAX_LINE_ID + 1);
    let result = create_one_line(&state);
    assert!(
        !result.success,
        "line id {} cannot hold shots",
        MAX_LINE_ID + 1
    );
    let error = result.error.unwrap_or_default();
    assert!(
        error.contains("maximum number of lines")
            && error.contains(&MAX_LINES_IN_ID_SPACE.to_string()),
        "the refusal must name the limit, got: {error}"
    );
    assert_eq!(state.get_project_state().unwrap().lines().len(), 1);
}

/// An import is refused as a WHOLE when its line count would cross the
/// bound, before any mutation. One id is left here and the file declares two.
#[test]
fn an_import_that_would_cross_the_bound_is_refused_atomically() {
    let state = state_with_spent_line_ids(MAX_LINE_ID);
    let result = state.execute_import_lines(
        vec![import_config(0), import_config(1)],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(!result.success, "2 lines do not fit into 1 remaining id");
    let error = result.error.unwrap_or_default();
    assert!(error.contains("would add 2 lines"), "got: {error}");
    assert!(
        error.contains(&MAX_LINE_ID.to_string())
            && error.contains(&MAX_LINES_IN_ID_SPACE.to_string()),
        "the refusal must name how many ids are spent and the limit, got: {error}"
    );
    let project = state.get_project_state().unwrap();
    assert_eq!(project.lines().len(), 1, "nothing may be created");
    assert_eq!(project.next_line_id(), MAX_LINE_ID, "and no id spent");
    assert!(
        !state.can_undo(),
        "a refused import is not an undoable unit"
    );
}

/// The same import, one line smaller, lands: the batch check is exact.
#[test]
fn an_import_that_exactly_fills_the_id_space_is_accepted() {
    let state = state_with_spent_line_ids(MAX_LINE_ID);
    let result = state.execute_import_lines(
        vec![import_config(0)],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "import failed: {:?}", result.error);
    let project = state.get_project_state().unwrap();
    assert!(
        project.line(MAX_LINE_ID).is_some(),
        "the last id is spent, once"
    );
}

/// A five-shot line on a project that has spent EVERY line id: the line
/// itself carries the last one, `MAX_LINE_ID`.
fn state_at_full_id_space_with_one_line(shots: usize) -> (AppState, LineId, Vec<u32>) {
    let mut project = Project::new("Full", None);
    let line = with_line_id(
        &mut project,
        "Original",
        (0..shots).map(|i| shot(5.0 + i as f64)).collect(),
        MAX_LINE_ID,
    );
    let shot_ids: Vec<u32> = project
        .line(line)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.id)
        .collect();
    (state_with(project), line, shot_ids)
}

fn assert_split_was_refused(state: &AppState, result: &CommandResult, line: LineId, shots: usize) {
    assert!(
        !result.success,
        "a split with no id to mint must be refused"
    );
    let error = result.error.clone().unwrap_or_default();
    assert!(
        error.contains("split it into two") && error.contains(&MAX_LINES_IN_ID_SPACE.to_string()),
        "the refusal must say what it could not do and name the limit, got: {error}"
    );
    let project = state.get_project_state().unwrap();
    assert_eq!(project.lines().len(), 1, "no second line may be minted");
    let l = project.line(line).unwrap();
    assert_eq!(l.shots.len(), shots, "the line must not be truncated");
    assert_eq!(l.name.as_deref(), Some("Original"), "nor renamed");
    assert!(!state.can_undo(), "a refusal records nothing");
}

/// The single-shot split mints its second line without `create_line`, so it
/// is refused by the split's own budget check, before anything changes.
#[test]
fn a_delete_that_splits_a_line_is_refused_when_no_line_id_is_left() {
    let (state, line, shot_ids) = state_at_full_id_space_with_one_line(5);
    let result =
        state.execute_delete_shot_and_split(line, shot_ids[2], "A".to_string(), "B".to_string());
    assert_split_was_refused(&state, &result, line, 5);
}

/// Same door, the range flavour.
#[test]
fn a_range_delete_that_splits_a_line_is_refused_when_no_line_id_is_left() {
    let (state, line, shot_ids) = state_at_full_id_space_with_one_line(5);
    let result = state.execute_delete_shot_range_split(
        line,
        shot_ids[1],
        shot_ids[2],
        "A".to_string(),
        "B".to_string(),
    );
    assert_split_was_refused(&state, &result, line, 5);
}

/// One id left is enough for a split, which mints exactly one: the guard is
/// the same exact bound the other doors use, not a margin.
#[test]
fn a_split_with_the_last_line_id_left_still_succeeds() {
    let mut project = Project::new("Last", None);
    let line = with_line_id(
        &mut project,
        "Original",
        (0..5).map(|i| shot(5.0 + f64::from(i))).collect(),
        MAX_LINE_ID - 1,
    );
    let split_at = project.line(line).unwrap().shots[2].id;
    let state = state_with(project);

    let result =
        state.execute_delete_shot_and_split(line, split_at, "A".to_string(), "B".to_string());
    assert!(result.success, "split failed: {:?}", result.error);
    assert_eq!(
        result.line_id,
        Some(MAX_LINE_ID),
        "the second line takes the last id"
    );
    let project = state.get_project_state().unwrap();
    assert_eq!(project.lines().len(), 2);
    assert!(project.line(MAX_LINE_ID).is_some());
}

/// The encoder's text for a project of `lines` shot-less placed lines named
/// `L<i>`, written row by row. Building that many lines through the model
/// re-derives the topology and rescans the placements on every
/// `set_placement`, which is quadratic and takes minutes at the id-space
/// bound; [`the_bulk_source_text_is_the_encoders`] pins this text to the
/// encoder's own output, and the merge's read decodes and validates it.
fn bulk_source_text(name: &str, lines: usize) -> String {
    let rows = |row: &dyn Fn(usize) -> String| (0..lines).map(row).collect::<Vec<_>>().join(",\n");
    let settings = r#"{"validation":{"warn_shot_length_min":0.3,"warn_shot_length_max":50.0,"warn_depth_change_per_shot_max":10.0,"warn_azimuth_deviation_max":30.0,"azimuth_diff_threshold":20.0,"pitch_diff_threshold":10.0,"inclination_depth_diff_threshold":15.0}}"#;
    format!(
        concat!(
            "{{\"format\":\"stalx\",\"version\":2,\"solver\":1,\"requires\":[],\n",
            "\"conventions\":{{\"length\":\"m\",\"angles\":\"deg\",\"azimuth\":\"cw-from-north\",",
            "\"inclination\":\"+down\",\"depth\":\"+down\",\"local\":\"enu-m\",\"gps\":\"wgs84-deg\",",
            "\"temperature\":\"degC\"}},\n",
            "\"generator\":\"Stalagmight\",\n",
            "\"summary\":{{\"name\":\"{name}\",\"lines\":{lines},\"stations\":{lines},\"shots\":0,\"length\":-0.0}},\n",
            "\"project\":{{\"name\":\"{name}\",\"created\":\"2026-01-01T00:00:00.000000000Z\",",
            "\"modified\":\"2026-01-01T00:00:00.000000000Z\"}},\n",
            "\"settings\":{settings},\n",
            "\"counters\":{{\"station\":{lines},\"closure\":1,\"section\":0}},\n",
            "\"stations\":[\n{stations}\n],\n",
            "\"lines\":[\n{line_rows}\n],\n",
            "\"placements\":[\n{placements}\n],\n",
            "\"end\":true}}\n"
        ),
        name = name,
        lines = lines,
        settings = settings,
        stations = rows(&|i| format!("[{i},\"0\"]")),
        line_rows = rows(&|i| format!(
            "{{\"id\":{i},\"name\":\"L{i}\",\"color\":\"#4ecdc4\",\"direction\":\"in\",\"quality\":1.0,\"start\":{i}}}"
        )),
        placements = rows(&|i| format!("{{\"station\":{i},\"x\":0.0,\"y\":0.0}}")),
    )
}

/// [`bulk_source_text`] is byte for byte what the encoder writes for the same
/// survey built through the model (small enough for the builder).
#[test]
fn the_bulk_source_text_is_the_encoders() {
    let created = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let mut project = Project::new_at("a", None, created);
    for i in 0..3 {
        let line = project
            .add_line(meta(&format!("L{i}")), StartAt::New, Vec::new())
            .unwrap();
        let start = project.line(line).unwrap().start;
        project.set_placement(start, 0.0, 0.0).unwrap();
    }
    let bytes = stalx_v2::encode(&project, &Extras::default(), Mode::Normal).unwrap();
    assert_eq!(String::from_utf8(bytes).unwrap(), bulk_source_text("a", 3));
}

/// A merge whose sources hold more lines between them than the id space
/// allows is refused before any line is appended, and writes no output file.
/// Real lines, because the check sums the sources' line counts. They are
/// shot-less, so the cost is one station each.
#[test]
fn a_merge_past_the_id_space_is_refused_and_writes_no_output() {
    fn source(name: &str, lines: usize, dir: &TempDir) -> std::path::PathBuf {
        let path = dir.path().join(format!("{name}.stalx"));
        std::fs::write(&path, bulk_source_text(name, lines)).unwrap();
        path
    }

    let temp_dir = TempDir::new().unwrap();
    // 42_949 + 1: the smallest pair that crosses the bound at all.
    let source_a = source("a", MAX_LINES_IN_ID_SPACE, &temp_dir);
    let source_b = source("b", 1, &temp_dir);
    let output_path = temp_dir.path().join("merged.stalx");

    let state = AppState::new().unwrap();
    let result = state.execute(Command::MergeProjects {
        source_paths: vec![source_a, source_b],
        output_path: output_path.clone(),
        output_name: "Merged".to_string(),
        archive_originals: false,
    });
    assert!(!result.success, "the merged project cannot hold that many");
    let error = result.error.unwrap_or_default();
    assert!(
        error.contains(&(MAX_LINES_IN_ID_SPACE + 1).to_string())
            && error.contains(&MAX_LINES_IN_ID_SPACE.to_string()),
        "the refusal must name the combined count and the limit, got: {error}"
    );
    assert!(
        !output_path.exists(),
        "a refused merge leaves no output file"
    );
    assert!(
        !output_path.with_extension("stalx.tmp").exists(),
        "nor a temp file"
    );
}

