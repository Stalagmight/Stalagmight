//! Writes that upgrade a legacy file while it is closed (a Recents rename, a
//! SpeleoDB link strip) owe what the upgrading save does: the conversion
//! notice reaches the next open of the file (review finding
//! D-migration-upgrade-4). The dive-plan half is in `dive_plan_tests.rs`
//! (`mod legacy`), beside its 0.6.0 pin fixture. Synthetic files only.

use std::path::Path;

use tempfile::TempDir;

use super::legacy_upgrade_tests::{forget_backups, unique_stem};
use super::project_save_tests::write_legacy_drift;
use super::stalx_fixtures::write_legacy_link_only;
use super::AppState;
use crate::core::loading::SpeleoManagedFile;
use crate::models::SpeleoLink;
use crate::persistence::stalx_v2::{self, Format};

const ID: &str = "11111111-1111-1111-1111-111111111111";

fn link() -> SpeleoLink {
    SpeleoLink {
        endpoint: "http://x/".to_owned(),
        project_id: ID.to_owned(),
        project_name: "Linked".to_owned(),
        base_commit_sha: Some("abc123".to_owned()),
        has_unpushed_edits: false,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: None,
    }
}

fn is_v2(path: &Path) -> bool {
    stalx_v2::sniff(&std::fs::read(path).unwrap()) == Format::V2
}

/// Open `path` in `state` the way the executor does; the notice it raised.
fn notice_of_open(state: &AppState, path: &Path) -> Option<String> {
    let opened = state.open_project(path.to_path_buf());
    assert!(opened.success, "{:?}", opened.error);
    state.take_conversion_notice()
}

/// A Recents rename upgrades a closed 0.6.0 file whose conversion moved two
/// stations: the open of that file later in the session says so, once, even
/// when another project was opened in between.
#[test]
fn a_recents_rename_upgrade_keeps_the_conversion_notice_for_the_next_open() {
    crate::app_dirs::init_for_test();
    crate::test_helpers::with_isolated_config(|| {
        let dir = TempDir::new().unwrap();
        let stem = unique_stem("closed-rename-notice");
        let drifted = dir.path().join(format!("{stem}.stalx"));
        write_legacy_drift(&drifted, true);
        let other = dir.path().join(format!("{stem}-other.stalx"));
        write_legacy_drift(&other, false);
        let state = AppState::new().unwrap();

        let renamed = state.set_project_name_for_path(drifted.clone(), "Renamed".to_owned());
        assert!(renamed.success, "{:?}", renamed.error);
        assert!(is_v2(&drifted), "the rename upgraded the file");

        assert_eq!(
            notice_of_open(&state, &other),
            None,
            "another file's open neither shows nor drops it"
        );
        let notice = notice_of_open(&state, &drifted).expect("the conversion notice");
        assert!(
            notice.starts_with(
                "Converted from the old file format; 2 stations moved by up to 5.00 m"
            ),
            "{notice}"
        );
        assert_eq!(notice_of_open(&state, &drifted), None, "offered once");
        forget_backups(&stem);
    });
}

/// Stripping a closed 0.6.0 copy's SpeleoDB link upgrades it; the open that
/// follows (the prompt opens the file next) shows the conversion notice.
#[test]
fn a_link_strip_upgrade_hands_the_conversion_notice_to_the_open_that_follows() {
    crate::app_dirs::init_for_test();
    crate::test_helpers::with_isolated_config(|| {
        let dir = TempDir::new().unwrap();
        let stem = unique_stem("closed-strip-notice");
        let path = dir.path().join(format!("{stem}.stalx"));
        write_legacy_drift(&path, true);
        write_legacy_link_only(&path, &link());
        let state = AppState::new().unwrap();

        state
            .speleodb_remove_link_at(
                path.clone(),
                SpeleoManagedFile {
                    path: path.display().to_string(),
                    project_id: ID.to_owned(),
                    project_name: "Linked".to_owned(),
                    endpoint: "http://x/".to_owned(),
                    can_open_remote: true,
                },
            )
            .unwrap();
        assert!(is_v2(&path), "the strip upgraded the file");

        let notice = notice_of_open(&state, &path).expect("the conversion notice");
        assert!(
            notice.contains("2 stations moved by up to 5.00 m"),
            "{notice}"
        );
        assert!(state.speleo_link().is_none());
        forget_backups(&stem);
    });
}

/// A restored 0.6.0 original at the path is a legacy file again: its own
/// migration decides the notice, and the one the closed upgrade left is
/// dropped, never shown for a file it does not describe.
#[test]
fn a_legacy_file_back_at_the_path_drops_the_closed_upgrade_notice() {
    crate::app_dirs::init_for_test();
    crate::test_helpers::with_isolated_config(|| {
        let dir = TempDir::new().unwrap();
        let stem = unique_stem("closed-restore-notice");
        let path = dir.path().join(format!("{stem}.stalx"));
        write_legacy_drift(&path, true);
        let state = AppState::new().unwrap();
        assert!(
            state
                .set_project_name_for_path(path.clone(), "Renamed".to_owned())
                .success
        );
        // The clean survey, as a 0.6.0 file, back at the path.
        std::fs::remove_file(&path).unwrap();
        write_legacy_drift(&path, false);

        assert_eq!(notice_of_open(&state, &path), None);
        assert!(state.close_project().success);
        forget_backups(&stem);
    });
}
