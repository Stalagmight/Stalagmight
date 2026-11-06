//! The export commands through the registered commands (moved out of
//! `command_tests.rs`, #576 E1/E8 file-size split; unchanged).

use std::fs;
use std::path::PathBuf;

use tempfile::TempDir;

use super::command_tests::{
    app_with_state, assert_success, create_project, create_section_with_shots, h, made_for, run,
    save_project_to_temp, shot_data,
};
use super::{export, shot};
use crate::models::{ShotData, ShotKind};

#[test]
fn test_export_csv_fails_no_project() {
    let app = app_with_state();
    let result = run(export::export_csv(h(&app), PathBuf::from("/tmp/out.csv")));
    assert!(result.is_err());
}

#[test]
fn test_export_json_fails_no_project() {
    let app = app_with_state();
    let result = run(export::export_json(h(&app), PathBuf::from("/tmp/out.json")));
    assert!(result.is_err());
}

#[test]
fn test_export_image_fails_no_project() {
    let app = app_with_state();
    let options = crate::export::ExportImageOptions::default();
    let result = run(export::export_image(
        h(&app),
        PathBuf::from("/tmp/out.png"),
        options,
    ));
    assert!(result.is_err());
}

#[test]
fn test_export_csv_fails_when_directory_missing() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let missing_dir = dir.path().join("missing");
    let path = missing_dir.join("out.csv");
    let result = run(export::export_csv(h(&app), path.clone()));
    assert!(result.is_err());
    assert!(!path.exists());
}

#[test]
fn test_export_json_fails_when_directory_missing() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let missing_dir = dir.path().join("missing");
    let path = missing_dir.join("out.json");
    let result = run(export::export_json(h(&app), path.clone()));
    assert!(result.is_err());
    assert!(!path.exists());
}

#[test]
fn test_export_image_fails_when_directory_missing() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let missing_dir = dir.path().join("missing");
    let path = missing_dir.join("out.svg");
    let mut options = crate::export::ExportImageOptions::default();
    options.format = crate::export::ImageFormat::Svg;
    let result = run(export::export_image(h(&app), path.clone(), options));
    assert!(result.is_err());
    assert!(!path.exists());
}

#[test]
fn test_export_image_refuses_to_overwrite_open_project_file() {
    // Exporting an image over the open project's own `.stalx` would corrupt the
    // live database (image export does a raw file write). The guard must reject
    // that path before any write, leaving the project file intact.
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let saved_path = save_project_to_temp(&app, &dir);

    let mut options = crate::export::ExportImageOptions::default();
    options.format = crate::export::ImageFormat::Svg;
    let result = run(export::export_image(h(&app), saved_path.clone(), options));
    assert!(
        result.is_err(),
        "must refuse to export an image over the open project file"
    );
    assert!(
        result.unwrap_err().message.contains("open project"),
        "error must explain the clobber guard"
    );
    // The project database on disk is untouched (still present, not an SVG).
    assert!(saved_path.exists());
}

#[test]
fn test_export_image_refuses_to_overwrite_the_plnx_sibling() {
    // `<project>.plnx` holds EVERY saved dive plan for the open project. The
    // text exports and the dive-profile export have always refused it via
    // `is_project_owned_file`; the image export used the narrower
    // `is_open_project_file`, so a PNG/SVG/PDF aimed at that path was written
    // and took the whole plan store with it. The path is not reachable through
    // the save dialog (the frontend forces the extension), but the IPC boundary
    // takes an arbitrary path from the webview - which is the boundary the
    // guard exists for, and which the export fuzzer drives.
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let saved_path = save_project_to_temp(&app, &dir);
    let plnx = saved_path.with_extension("plnx");
    // A project with no saved plan has no sibling to protect, so the guard has
    // nothing to canonicalize - the refusal only applies to a `.plnx` that is
    // really there, exactly as for the `.stalx`.
    fs::write(&plnx, b"dive plans").unwrap();

    let mut options = crate::export::ExportImageOptions::default();
    options.format = crate::export::ImageFormat::Svg;
    let result = run(export::export_image(h(&app), plnx.clone(), options));
    assert!(
        result.is_err(),
        "an image export must refuse the open project's .plnx sibling"
    );
    assert!(
        result.unwrap_err().message.contains("open project"),
        "error must explain the clobber guard"
    );
    assert_eq!(
        fs::read(&plnx).unwrap(),
        b"dive plans",
        "the plan store must be byte-for-byte untouched"
    );
}

#[test]
fn test_text_exports_refuse_to_overwrite_the_plnx_sibling() {
    // The control for the test above: the text exports were already correct, so
    // a regression that narrowed THEIR guard would otherwise go unnoticed.
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let saved_path = save_project_to_temp(&app, &dir);
    let plnx = saved_path.with_extension("plnx");
    fs::write(&plnx, b"dive plans").unwrap();

    for res in [
        run(export::export_csv(h(&app), plnx.clone())),
        run(export::export_json(h(&app), plnx.clone())),
        run(export::export_tml(h(&app), plnx.clone())).map(|_| ()),
    ] {
        assert!(res.is_err(), "text export must refuse the .plnx sibling");
        assert!(
            res.unwrap_err().message.contains("open project"),
            "error must explain the clobber guard"
        );
    }
    assert_eq!(fs::read(&plnx).unwrap(), b"dive plans");
}

#[test]
fn test_text_exports_refuse_to_overwrite_open_project_file() {
    // CSV/JSON/TML exports do raw file writes too; exporting over the open
    // project's own `.stalx` would corrupt the live database. The guard must
    // reject before any write, leaving the DB bytes byte-for-byte intact.
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let saved_path = save_project_to_temp(&app, &dir);
    let before = fs::read(&saved_path).unwrap();

    for res in [
        run(export::export_csv(h(&app), saved_path.clone())),
        run(export::export_json(h(&app), saved_path.clone())),
        run(export::export_tml(h(&app), saved_path.clone())).map(|_| ()),
    ] {
        assert!(
            res.is_err(),
            "text export must refuse the open project path"
        );
        assert!(
            res.unwrap_err().message.contains("open project"),
            "error must explain the clobber guard"
        );
    }
    // Byte-for-byte identical: no export path touched the DB.
    assert_eq!(fs::read(&saved_path).unwrap(), before);
}

#[test]
fn test_export_image_rejects_zero_dpi() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("out.png");
    let mut options = crate::export::ExportImageOptions::default();
    options.format = crate::export::ImageFormat::Png;
    options.dpi = 0;
    let result = run(export::export_image(h(&app), path.clone(), options));
    assert!(result.is_err());
    assert!(!path.exists());
}

#[test]
fn test_export_json_overwrites_existing_file() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("out.json");
    fs::write(&path, "sentinel").unwrap();
    run(export::export_json(h(&app), path.clone())).unwrap();
    let contents = fs::read_to_string(&path).unwrap();
    assert!(contents.contains("\"project\""));
    assert!(!contents.contains("sentinel"));
}

#[test]
fn test_export_csv_success_creates_file() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("out.csv");
    run(export::export_csv(h(&app), path.clone())).unwrap();
    assert!(path.exists());
}

#[test]
fn test_export_json_success_creates_file() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("out.json");
    run(export::export_json(h(&app), path.clone())).unwrap();
    assert!(path.exists());
}

#[test]
fn test_export_image_success_creates_file() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("out.svg");
    let mut options = crate::export::ExportImageOptions::default();
    options.format = crate::export::ImageFormat::Svg;
    run(export::export_image(h(&app), path.clone(), options)).unwrap();
    assert!(path.exists());
}

#[test]
fn test_export_roundtrip_includes_reference_and_comment() {
    let app = app_with_state();
    create_project(&app);
    let shot = ShotData {
        azimuth_in: 12.0,
        azimuth_out: 15.0,
        length: 8.25,
        depth_in: 1.0,
        depth_out: 2.0,
        left: Some(0.3),
        right: Some(0.4),
        up: Some(0.5),
        down: Some(0.6),
        comment: Some("note,with comma".to_string()),
        ..Default::default()
    };
    let section_id = create_section_with_shots(&app, vec![shot]);
    assert_success(&run(shot::set_shot_kind(
        h(&app),
        made_for(&app),
        section_id,
        0,
        ShotKind::Jump,
    )));

    let dir = TempDir::new().unwrap();
    let csv_path = dir.path().join("export.csv");
    let json_path = dir.path().join("export.json");

    run(export::export_csv(h(&app), csv_path.clone())).unwrap();
    run(export::export_json(h(&app), json_path.clone())).unwrap();

    let csv_contents = fs::read_to_string(&csv_path).unwrap();
    assert!(csv_contents.contains("section_id,section_name"));
    assert!(csv_contents.contains("\"note,with comma\""));
    // #27 replaced the boolean `is_reference` column with the 3-way `kind`,
    // so the export now names the kind instead of emitting "true".
    assert!(csv_contents.contains(",kind,comment"));
    assert!(csv_contents.contains("Jump"));

    let json_contents = fs::read_to_string(&json_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_contents).unwrap();
    let shot_json = &parsed["sections"][0]["shots"][0];
    assert_eq!(shot_json["kind"], "Jump");
    assert_eq!(shot_json["comment"], "note,with comma");
    assert!(shot_json["lrud"].is_array());
}
