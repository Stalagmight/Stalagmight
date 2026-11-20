//! Writes that upgrade a legacy file other than the open project's first
//! save. A Recents rename or a SpeleoDB link strip of a closed file owes what
//! the upgrading save does: the conversion notice reaches the next open of the
//! file (review finding D-migration-upgrade-4; the dive-plan half is in
//! `dive_plan_tests.rs`, `mod legacy`, beside its 0.6.0 pin fixture). A Save
//! As onto another legacy file goes through the upgrade too
//! (D-migration-upgrade-10). Synthetic files only.

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

/// Save As onto ANOTHER 0.6.0 file, one an old build was killed writing (a
/// hot journal beside it), writes the version 2 file through the upgrade: the
/// journal is gone, so no old build can roll it into the JSON file, and the
/// whole 0.6.0 survey it replaced (journal rolled back) is kept in Recovery
/// (review finding D-migration-upgrade-10). Injection: write an explicit
/// destination with `write_atomic` alone, as before, and the journal stays
/// beside the JSON file.
#[test]
fn save_as_over_another_legacy_file_leaves_no_sidecar_and_keeps_it() {
    use super::legacy_upgrade::settled_bytes;
    use super::legacy_upgrade_tests::mid_transaction;
    use super::robustness_kit::{open_fresh, write_legacy, Rng};
    use super::speleodb_sidecars::sidecar_path;
    use crate::core::Command;

    crate::app_dirs::init_for_test();
    crate::test_helpers::with_isolated_config(|| {
        let dir = TempDir::new().unwrap();
        let stem = unique_stem("save-as-over-legacy");
        let source = dir.path().join(format!("{stem}-source.stalx"));
        write_legacy(&source, &mut Rng::new(31), "Source");
        let scratch = dir.path().join(format!("{stem}-scratch.stalx"));
        write_legacy(&scratch, &mut Rng::new(32), "Destination");
        let (main, journal) = mid_transaction(&std::fs::read(&scratch).unwrap()).unwrap();
        let dest = dir.path().join(format!("{stem}.stalx"));
        std::fs::write(&dest, &main).unwrap();
        std::fs::write(sidecar_path(&dest, "-journal"), &journal).unwrap();
        let whole = settled_bytes(&main, &dest).unwrap();

        let state = open_fresh(&source).unwrap();
        let saved = state.execute(Command::SaveProject {
            path: Some(dest.clone()),
        });
        assert!(saved.success, "{:?}", saved.error);
        assert!(is_v2(&dest), "the destination holds version 2");
        for suffix in ["-journal", "-wal", "-shm"] {
            assert!(
                !sidecar_path(&dest, suffix).exists(),
                "no {suffix} beside the JSON file"
            );
        }
        assert!(!is_v2(&source), "the source is untouched");
        let kept = AppState::list_project_backups(&dest)
            .into_iter()
            .filter(|b| b.path.ends_with(".pre-v2.stalx"))
            .map(|b| std::fs::read(b.path).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(kept, vec![whole], "the replaced survey, whole, in Recovery");
        forget_backups(&stem);
    });
}

/// Save As onto a plain (unlinked, settled) 0.6.0 file also keeps it.
#[test]
fn save_as_over_a_plain_legacy_file_keeps_it_in_recovery() {
    use super::robustness_kit::{open_fresh, write_legacy, Rng};
    use crate::core::Command;

    crate::app_dirs::init_for_test();
    crate::test_helpers::with_isolated_config(|| {
        let dir = TempDir::new().unwrap();
        let stem = unique_stem("save-as-over-plain-legacy");
        let source = dir.path().join(format!("{stem}-source.stalx"));
        write_legacy(&source, &mut Rng::new(41), "Source");
        let dest = dir.path().join(format!("{stem}.stalx"));
        write_legacy(&dest, &mut Rng::new(42), "Destination");
        let replaced = std::fs::read(&dest).unwrap();

        let state = open_fresh(&source).unwrap();
        assert!(
            state
                .execute(Command::SaveProject {
                    path: Some(dest.clone()),
                })
                .success
        );
        assert!(is_v2(&dest));
        let kept = AppState::list_project_backups(&dest)
            .into_iter()
            .filter(|b| b.path.ends_with(".pre-v2.stalx"))
            .count();
        assert_eq!(kept, 1);
        let backups = AppState::list_project_backups(&dest);
        assert!(backups
            .iter()
            .any(|b| std::fs::read(&b.path).ok().as_deref() == Some(replaced.as_slice())));
        forget_backups(&stem);
    });
}
