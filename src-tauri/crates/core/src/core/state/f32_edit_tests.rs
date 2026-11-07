//! [#576 F32-EDIT] Editing one field of a shot leaves every other stored reading
//! bit-identical across save and reopen.
//!
//! The frontend's copy of the project comes from the binary envelope, which
//! carries every shot reading as an f32 (`core::load_binary`, the PRECISION note).
//! Its edit paths now send only the fields the user changed; this pins the
//! backend half of that contract: a one-field `update_shot` touches that field
//! and nothing else in the saved file. The second test shows what the old
//! whole-row edit did - the f32 copy of each untouched reading, written back -
//! so the first one cannot pass for a reason other than the patch's shape.
//!
//! All data is synthetic.

use std::path::Path;

use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::{Project, SectionData, Shot, ShotData, ShotUpdates};
use crate::persistence::Database;
use crate::test_fixtures::bare_shot;

/// A shot whose readings are not exactly representable as f32.
fn exact_readings() -> ShotData {
    bare_shot(1.2)
        .with_azimuths(123.4, 303.3)
        .with_depths(10.1, 11.7)
        .with_pitches(Some(-3.3), Some(3.1))
        .with_lrud(Some(0.7), Some(1.3), Some(0.2), Some(2.9))
        .with_comment("StationA1 lead")
        .with_marker_index(4)
        .with_temperature(21.3)
}

/// Every float reading of `shot` by name, as raw bits (`None` = not recorded).
fn reading_bits(shot: &Shot) -> Vec<(&'static str, Option<u64>)> {
    let bits = |v: Option<f64>| v.map(f64::to_bits);
    vec![
        ("azimuth_in", bits(Some(shot.azimuth_in))),
        ("azimuth_out", bits(Some(shot.azimuth_out))),
        ("length", bits(Some(shot.length))),
        ("depth_in", bits(Some(shot.depth_in))),
        ("depth_out", bits(Some(shot.depth_out))),
        ("pitch_in", bits(shot.pitch_in)),
        ("pitch_out", bits(shot.pitch_out)),
        ("left", bits(shot.left)),
        ("right", bits(shot.right)),
        ("up", bits(shot.up)),
        ("down", bits(shot.down)),
        ("temperature", bits(shot.temperature)),
    ]
}

fn first_shot(path: &Path) -> Shot {
    let project = Database::open_existing(path)
        .expect("reopen")
        .load_project()
        .expect("load");
    project.sections[0].shots[0].clone()
}

/// Write the synthetic project, open it, apply `updates` to its one shot, save,
/// and return the shot as the file held it before and after.
fn edit_save_reopen(updates: ShotUpdates) -> (Shot, Shot) {
    // open_project and the save record the path in recent projects (global config).
    let _config = crate::test_helpers::isolated_config();
    let temp = TempDir::new().expect("temp dir");
    let path = temp.path().join("CaveA.stalx");
    let mut project = Project::new("CaveA".to_owned(), None);
    project.add_section(SectionData {
        color: "#abcdef".to_owned(),
        shots: vec![exact_readings()],
        ..Default::default()
    });
    Database::open(&path)
        .expect("create")
        .save_project(&project)
        .expect("write");
    let before = first_shot(&path);

    let state = AppState::new().expect("state");
    let opened = state.open_project(path.clone());
    assert!(opened.success, "open failed: {:?}", opened.error);
    let (section_id, shot_id) = {
        let open = state.project_snapshot().expect("a project is open");
        (open.sections[0].id, open.sections[0].shots[0].id)
    };
    let edited = state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates,
    });
    assert!(edited.success, "update_shot failed: {:?}", edited.error);
    let saved = state.save_project(Some(path.clone()));
    assert!(saved.success, "save failed: {:?}", saved.error);
    drop(state);

    (before, first_shot(&path))
}

/// The readings whose bits differ between `before` and `after`, but `length`.
fn changed_besides_length(before: &Shot, after: &Shot) -> Vec<String> {
    reading_bits(before)
        .into_iter()
        .zip(reading_bits(after))
        .filter(|((name, a), (_, b))| *name != "length" && a != b)
        .map(|((name, a), (_, b))| {
            let show =
                |v: Option<u64>| v.map_or("unset".to_owned(), |b| f64::from_bits(b).to_string());
            format!("{name}: {} -> {}", show(a), show(b))
        })
        .collect()
}

#[test]
fn a_one_field_edit_leaves_every_other_reading_bit_identical() {
    let (before, after) = edit_save_reopen(ShotUpdates {
        length: Some(5.0),
        ..Default::default()
    });
    let changed = changed_besides_length(&before, &after);
    println!(
        "length {} -> {}; other readings changed: {changed:?}",
        before.length, after.length
    );
    assert_eq!(
        after.length.to_bits(),
        5.0_f64.to_bits(),
        "the edited field is saved"
    );
    assert!(
        changed.is_empty(),
        "untouched readings changed: {changed:?}"
    );
    assert_eq!(after.comment, before.comment);
    assert_eq!(after.marker_index, before.marker_index);
    assert_eq!(after.kind, before.kind);
}

#[test]
fn the_whole_row_resent_from_its_f32_copy_rewrites_the_untouched_readings() {
    let f32_copy = |v: f64| f64::from(v as f32);
    let sent = exact_readings();
    let (before, after) = edit_save_reopen(ShotUpdates {
        azimuth_in: Some(f32_copy(sent.azimuth_in)),
        azimuth_out: Some(f32_copy(sent.azimuth_out)),
        length: Some(5.0),
        depth_in: Some(f32_copy(sent.depth_in)),
        depth_out: Some(f32_copy(sent.depth_out)),
        pitch_in: Some(sent.pitch_in.map(f32_copy)),
        pitch_out: Some(sent.pitch_out.map(f32_copy)),
        left: Some(sent.left.map(f32_copy)),
        right: Some(sent.right.map(f32_copy)),
        up: Some(sent.up.map(f32_copy)),
        down: Some(sent.down.map(f32_copy)),
        temperature: Some(sent.temperature.map(f32_copy)),
        ..Default::default()
    });
    let changed = changed_besides_length(&before, &after);
    println!("whole-row edit from the f32 copy changed: {changed:?}");
    assert_eq!(
        changed.len(),
        11,
        "every untouched reading picks up the f32 rounding: {changed:?}"
    );
}
