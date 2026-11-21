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
use super::project_save_tests::{write_legacy_drift, write_legacy_unchanged};
use super::stalx_fixtures::write_legacy_link_only;
use super::AppState;
use super::{ConversionFile, ConversionNoticeView};
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
fn notice_of_open(state: &AppState, path: &Path) -> Option<ConversionNoticeView> {
    let opened = state.open_project(path.to_path_buf());
    assert!(opened.success, "{:?}", opened.error);
    state.take_conversion_notice()
}

/// A Recents rename upgrades a closed 0.6.0 file whose conversion moved two
/// stations: the open of that file later in the session says so, once, even
/// when another project was opened in between, and says the file is already
/// saved in the new format (demo issue I-5: never a future save that already
/// happened). Injection: take the notice with `view(false)` and it promises
/// the next save.
#[test]
fn a_recents_rename_upgrade_keeps_the_conversion_notice_for_the_next_open() {
    crate::app_dirs::init_for_test();
    crate::test_helpers::with_isolated_config(|| {
        let dir = TempDir::new().unwrap();
        let stem = unique_stem("closed-rename-notice");
        let drifted = dir.path().join(format!("{stem}.stalx"));
        write_legacy_drift(&drifted, true);
        let other = dir.path().join(format!("{stem}-other.stalx"));
        write_legacy_unchanged(&other);
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
        assert_eq!(
            notice.changes.first().map(String::as_str),
            Some("2 stations moved by up to 5.00 m"),
            "{notice:?}"
        );
        assert_eq!(notice.file, ConversionFile::Saved, "the rename wrote it");
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
            notice
                .changes
                .contains(&"2 stations moved by up to 5.00 m".to_owned()),
            "{notice:?}"
        );
        assert_eq!(notice.file, ConversionFile::Saved, "the strip wrote it");
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
        // A survey whose conversion changes nothing a line shows, as a 0.6.0
        // file, back at the path.
        std::fs::remove_file(&path).unwrap();
        write_legacy_unchanged(&path);

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

// ----- local state of an open legacy project (D-migration-upgrade-5) ---------

/// A display toggle on an open 0.6.0 project waits for the next save, which
/// writes it with the conversion (0.6.0 wrote it in place; this build writes
/// a legacy file only by upgrading it). It alone does not make the project
/// unsaved (demo issue I-6): the frontend's autosave follows that flag, and
/// would convert a file the user only looked at; a close then leaves the
/// 0.6.0 file as it was. Local state that is not view state (a materialized
/// undo mode, SpeleoDB bookkeeping) still asks. Injection: write the toggle
/// with `persist_local_state` again (or answer `has_unsaved_changes` from
/// `local_pending`) and the toggle makes the project unsaved.
#[test]
fn a_display_toggle_on_a_legacy_project_waits_for_a_save_without_making_it_unsaved() {
    use super::robustness_kit::{open_fresh, write_legacy, Rng};

    crate::app_dirs::init_for_test();
    crate::test_helpers::with_isolated_config(|| {
        let dir = TempDir::new().unwrap();
        let stem = unique_stem("legacy-toggle");
        let path = dir.path().join(format!("{stem}.stalx"));
        write_legacy(&path, &mut Rng::new(21), "Toggle");
        let original = std::fs::read(&path).unwrap();
        let state = open_fresh(&path).unwrap();
        assert!(!state.has_unsaved_changes());

        state.save_display_setting("showGrid", "true").unwrap();
        assert!(!is_v2(&path), "a toggle does not convert the file");
        assert!(
            !state.has_unsaved_changes(),
            "a display toggle alone must not arm autosave or the close prompt"
        );
        assert!(
            state.with_file(|f| f.local_pending),
            "it waits for the next save"
        );
        // A close with nothing else pending leaves the 0.6.0 file untouched.
        assert!(state.close_project().success);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            original,
            "never converted silently"
        );

        // The next save the user asks for writes it with the conversion.
        let state = open_fresh(&path).unwrap();
        state.save_display_setting("showGrid", "true").unwrap();
        assert!(state.save_project(None).success);
        assert!(is_v2(&path));
        assert!(!state.has_unsaved_changes());
        assert!(state.close_project().success);
        let reopened = open_fresh(&path).unwrap();
        assert_eq!(
            reopened.get_display_setting("showGrid").unwrap().as_deref(),
            Some("true")
        );
        forget_backups(&stem);
    });
}

/// The exemption is the display toggle's only: any other local state an open
/// 0.6.0 project keeps for its upgrading save still makes it unsaved (review
/// finding D-migration-upgrade-5), so the close prompt asks. Injection:
/// answer `has_unsaved_changes` from `dirty` alone and this fails.
#[test]
fn other_local_state_on_a_legacy_project_is_still_unsaved() {
    use super::robustness_kit::{open_fresh, write_legacy, Rng};

    crate::app_dirs::init_for_test();
    crate::test_helpers::with_isolated_config(|| {
        let dir = TempDir::new().unwrap();
        let stem = unique_stem("legacy-local");
        let path = dir.path().join(format!("{stem}.stalx"));
        write_legacy(&path, &mut Rng::new(22), "Local");
        let state = open_fresh(&path).unwrap();
        state.save_display_setting("showGrid", "true").unwrap();
        assert!(!state.has_unsaved_changes());

        state.with_file(|file| file.extras.local.speleodb.unpushed_edits = true);
        state.persist_local_state().unwrap();
        assert!(!is_v2(&path), "kept for the upgrading save");
        assert!(state.has_unsaved_changes(), "the close prompt must ask");

        assert!(state.save_project(None).success);
        assert!(!state.has_unsaved_changes());
        assert!(!state.with_file(|f| f.local_pending || f.local_pending_asks));
        forget_backups(&stem);
    });
}

/// A SpeleoDB unlink of an open 0.6.0 copy is durable at once, as it was in
/// 0.6.0 (and as stripping a closed copy's link is, plan N11): the file is
/// upgraded with the survey it holds on disk, never the session's unsaved
/// edits, and the original kept in Recovery. Save as an unmanaged file
/// relies on it: it opens the detached copy right after the unlink, which
/// used to drop the pending unlink and leave the cache copy linked.
/// Injection: write the unlink with `persist_local_state`, as before.
#[test]
fn an_unlink_of_an_open_legacy_copy_is_written_at_once() {
    use super::stalx_fixtures::{decoded, legacy_linked_project, link_on_disk};

    crate::app_dirs::init_for_test();
    crate::test_helpers::with_isolated_config(|| {
        let dir = TempDir::new().unwrap();
        let stem = unique_stem("legacy-unlink");
        let path = legacy_linked_project(&dir.path().join(format!("{stem}.stalx")), Some(&link()));
        let state = AppState::new().unwrap();
        assert!(state.open_project(path.clone()).success);
        assert!(state.speleo_link().is_some());
        // An unsaved edit of the session.
        {
            let mut project = state.project.lock().unwrap();
            "Edited".clone_into(&mut project.as_mut().unwrap().name);
        }
        state.with_file(|file| file.dirty = true);

        state.speleodb_unlink().unwrap();

        assert!(is_v2(&path), "the unlink upgraded the file");
        assert_eq!(link_on_disk(&path), None, "without the link");
        assert_eq!(
            decoded(&path).project.name,
            "Legacy",
            "the survey on disk, not the unsaved edit"
        );
        assert!(state.has_unsaved_changes(), "the edit is still unsaved");
        let kept = AppState::list_project_backups(&path)
            .into_iter()
            .filter(|b| b.path.ends_with(".pre-v2.stalx"))
            .count();
        assert_eq!(kept, 1, "the 0.6.0 original is kept");
        assert!(state.close_project().success);
        assert!(state.open_project(path).success);
        assert!(state.speleo_link().is_none());
        forget_backups(&stem);
    });
}

/// The 0.6.0 originals of `path` kept in Recovery.
fn originals_kept(path: &Path) -> usize {
    AppState::list_project_backups(path)
        .into_iter()
        .filter(|b| b.path.ends_with(".pre-v2.stalx"))
        .count()
}

/// Review finding O2 (package U5): the unlink of an open 0.6.0 copy migrates
/// the file (seconds on a large one) without the `project` guard, which every
/// command waits on, and commits under it only while the project is the one
/// it migrated for. A save of the session that lands during the migration
/// (it upgrades the file with the session's edit) makes the unlink start
/// over: it is then the local-only rewrite of that version 2 file, never the
/// stale migration written over the save.
/// Injection: migrate under the guard again and the first assertion inside
/// the migration fails; skip the check at the commit and the upgrade is
/// refused as changed by another program (or, with the guard read at the
/// commit as before, writes the stale survey over the save).
#[test]
fn an_unlink_of_an_open_legacy_copy_migrates_outside_the_project_lock() {
    use super::stalx_fixtures::{decoded, legacy_linked_project, link_on_disk};

    crate::app_dirs::init_for_test();
    crate::test_helpers::with_isolated_config(|| {
        let dir = TempDir::new().unwrap();
        let stem = unique_stem("legacy-unlink-unlocked");
        let path = legacy_linked_project(&dir.path().join(format!("{stem}.stalx")), Some(&link()));
        let state = AppState::new().unwrap();
        assert!(state.open_project(path.clone()).success);
        {
            let mut project = state.project.lock().unwrap();
            "Edited".clone_into(&mut project.as_mut().unwrap().name);
        }
        state.with_file(|file| file.dirty = true);
        // What `speleodb_unlink` does before it makes the unlink durable.
        state.with_file(|file| file.set_speleo_link(None));
        let adjustment = state.get_config().closure_adjustment;

        let mut migrations = 0;
        state
            .persist_local_state_upgrading_with(|at| {
                migrations += 1;
                assert!(
                    state.project.try_lock().is_ok(),
                    "the migration runs without the project lock"
                );
                let migrated = crate::persistence::migrate::migrate_path_for_app(at, &adjustment);
                // A command that runs meanwhile: the session's save.
                let saved = state.save_project(None);
                assert!(saved.success, "{:?}", saved.error);
                migrated
            })
            .unwrap();

        assert_eq!(migrations, 1, "the file was version 2 at the second look");
        assert!(is_v2(&path));
        assert_eq!(
            decoded(&path).project.name,
            "Edited",
            "the save's survey, not the migration's"
        );
        assert_eq!(link_on_disk(&path), None, "the unlink is written");
        assert!(!state.has_unsaved_changes());
        assert_eq!(originals_kept(&path), 1, "the save kept the original once");
        forget_backups(&stem);
    });
}

/// O2's other half: a file another program changes while the unlink's
/// migration runs is not overwritten. The commit's write checks the file
/// against the guard, as every save does, and the file stays as the other
/// program left it. Injection: commit with `Guard::Overwrite` and the
/// changed file is replaced.
#[test]
fn an_unlink_whose_file_another_program_changed_meanwhile_is_refused() {
    use super::stalx_fixtures::legacy_linked_project;

    crate::app_dirs::init_for_test();
    crate::test_helpers::with_isolated_config(|| {
        let dir = TempDir::new().unwrap();
        let stem = unique_stem("legacy-unlink-changed");
        let path = legacy_linked_project(&dir.path().join(format!("{stem}.stalx")), Some(&link()));
        let state = AppState::new().unwrap();
        assert!(state.open_project(path.clone()).success);
        state.with_file(|file| file.set_speleo_link(None));
        let adjustment = state.get_config().closure_adjustment;

        let mut changed = Vec::new();
        let refused = state.persist_local_state_upgrading_with(|at| {
            let migrated = crate::persistence::migrate::migrate_path_for_app(at, &adjustment);
            let mut bytes = std::fs::read(at).unwrap();
            bytes.extend_from_slice(&[0; 512]);
            std::fs::write(at, &bytes).unwrap();
            changed = bytes;
            migrated
        });

        assert!(refused.is_err(), "{refused:?}");
        assert_eq!(std::fs::read(&path).unwrap(), changed, "left as it was");
        assert!(!is_v2(&path));
        assert_eq!(originals_kept(&path), 0);
        forget_backups(&stem);
    });
}

/// The save that converts a project opened from a 0.6.0 file says so
/// (demo issue I-5), so the banner never promises a save that already
/// happened: `saved` for the save over the file itself (the original kept in
/// Recovery), nothing for the saves after it, `saved_as_new_file` for a Save
/// As elsewhere (the 0.6.0 file left as it was). A version 2 project's save
/// reports nothing. Injection: leave `legacy_converted` unset in
/// `save_project_to` and every assertion on it fails.
#[test]
fn the_converting_save_says_it_converted_the_file() {
    crate::app_dirs::init_for_test();
    crate::test_helpers::with_isolated_config(|| {
        let dir = TempDir::new().unwrap();
        let stem = unique_stem("converting-save");
        let path = dir.path().join(format!("{stem}.stalx"));
        write_legacy_drift(&path, true);
        let state = AppState::new().unwrap();
        assert!(state.open_project(path.clone()).success);
        let notice = state.take_conversion_notice().expect("a notice");
        assert_eq!(notice.file, ConversionFile::NotYetSaved);

        let first = state.save_project(None);
        assert!(first.success, "{:?}", first.error);
        assert_eq!(first.legacy_converted, Some(ConversionFile::Saved));
        assert!(is_v2(&path));
        let again = state.save_project(None);
        assert!(again.success, "{:?}", again.error);
        assert_eq!(again.legacy_converted, None, "converted once");
        assert!(state.close_project().success);

        let other = dir.path().join(format!("{stem}-old.stalx"));
        write_legacy_drift(&other, true);
        let original = std::fs::read(&other).unwrap();
        assert!(state.open_project(other.clone()).success);
        let elsewhere = dir.path().join(format!("{stem}-new.stalx"));
        let save_as = state.save_project(Some(elsewhere.clone()));
        assert!(save_as.success, "{:?}", save_as.error);
        assert_eq!(
            save_as.legacy_converted,
            Some(ConversionFile::SavedAsNewFile)
        );
        assert!(is_v2(&elsewhere));
        assert_eq!(std::fs::read(&other).unwrap(), original, "left as it was");
        let next = state.save_project(None);
        assert!(next.success, "{:?}", next.error);
        assert_eq!(next.legacy_converted, None);
        forget_backups(&stem);
    });
}
