//! [#576 FZ-FZ-INT-1, FZ-FZ-INT-R2] A load that overlaps a cancel, or a save of
//! the file it read, does not commit.
//!
//! - FZ-FZ-INT-1: the load re-checks its token at the last point before its
//!   commit's first side effect.
//! - FZ-FZ-INT-R2: a load whose file a save replaced while it read it is refused
//!   with `FILE_REPLACED_DURING_LOAD` (the scheduled replay of the campaign seed is
//!   in `interleave_fuzz_tests.rs`).
//!
//! The orderings are forced with the load's `BEFORE_SESSION_COMMIT` hook (it runs
//! on the loading thread holding `command_lock`), so none depends on timing. All
//! data is synthetic, in a temp dir.

use std::sync::mpsc;

use tempfile::TempDir;

use super::project_load::BEFORE_SESSION_COMMIT;
use super::switch_identity_tests::{names_on_disk, new_section, saved, section_names};
use crate::core::{Command, CommandErrorCode};
use crate::test_fixtures::state_with_null_sink;

/// [FZ-FZ-INT-1] A cancel that lands after the load's first re-check under
/// `command_lock` (here: inside the checks that follow it) is honoured: the load
/// is refused and the open project stays.
#[test]
fn a_load_cancelled_after_its_first_lock_check_does_not_commit() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let a = saved(temp.path(), "CaveA.stalx", "CaveA");
    let b = saved(temp.path(), "CaveB.stalx", "CaveB");
    let state = state_with_null_sink();
    state.do_load_project_full(a.clone()).unwrap();

    let (at_commit, reached) = mpsc::channel::<()>();
    let (go, wait) = mpsc::channel::<()>();
    let load = std::thread::scope(|scope| {
        let load = scope.spawn(|| {
            BEFORE_SESSION_COMMIT.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    at_commit.send(()).unwrap();
                    wait.recv().unwrap();
                }));
            });
            state.do_load_project_full(b.clone())
        });
        reached.recv().unwrap();
        state.cancel_loading();
        go.send(()).unwrap();
        load.join().unwrap()
    });

    let open = state.project_snapshot().map(|p| p.name.clone());
    println!(
        "load of CaveB cancelled at its commit -> {}; open {open:?}",
        match &load {
            Ok(_) => "Ok (committed)".to_owned(),
            Err(e) => format!("Err({})", e.code),
        }
    );
    assert_eq!(
        load.err().map(|e| e.code),
        Some(CommandErrorCode::Cancelled)
    );
    assert_eq!(open.as_deref(), Some("CaveA"), "the open project stays");
}

/// [FZ-FZ-INT-R2] Directed, outside the scheduler: a load of CaveA reads the file,
/// and before it commits another session of the same file commits, saves (the
/// save renames a fresh file over the path) and closes. The load is refused with
/// the retryable code, and a load started afterwards opens the file at the path.
/// Unix only: the other session runs on a second state, and off unix the stamp is
/// a state's own save epoch (`every_save_rename_moves_the_save_epoch`; the
/// one-state scheduled replay covers every platform).
#[cfg(unix)]
#[test]
fn a_load_whose_file_a_save_replaced_before_it_committed_is_refused() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let a = saved(temp.path(), "CaveA.stalx", "CaveA");
    let state = state_with_null_sink();

    // The racing load stops at its commit, holding `command_lock`, so the other
    // session's load / edit / save / close run on another state's commit lock:
    // they are driven against the FILE through a second state, which is exactly
    // what a second session of the file does to it.
    let other = state_with_null_sink();
    let (at_commit, reached) = mpsc::channel::<()>();
    let (go, wait) = mpsc::channel::<()>();
    let load = std::thread::scope(|scope| {
        let load = scope.spawn(|| {
            BEFORE_SESSION_COMMIT.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    at_commit.send(()).unwrap();
                    wait.recv().unwrap();
                }));
            });
            state.do_load_project_full(a.clone())
        });
        reached.recv().unwrap();
        other.do_load_project_full(a.clone()).unwrap();
        assert!(other.execute(new_section("SavedByTheOtherSession")).success);
        assert!(other.execute(Command::SaveProject { path: None }).success);
        assert!(other.execute(Command::CloseProject).success);
        go.send(()).unwrap();
        load.join().unwrap()
    });

    println!(
        "load of CaveA whose file was replaced before it committed -> {}; on disk {:?}",
        match &load {
            Ok(_) => "Ok (committed a session on the replaced file)".to_owned(),
            Err(e) => format!("Err({})", e.code),
        },
        names_on_disk(&a)
    );
    assert_eq!(
        load.err().map(|e| e.code),
        Some(CommandErrorCode::FileReplacedDuringLoad)
    );
    assert!(state.project_snapshot().is_none(), "nothing committed");
    let retried = state.do_load_project_full(a.clone());
    assert!(retried.is_ok(), "the retry opens the file at the path");
    assert!(section_names(&state).contains(&"SavedByTheOtherSession".to_owned()));
}

/// [#576 RVF-c-A2-1] A load the user cancelled is answered as cancelled even when
/// its file was ALSO replaced before it committed: `FILE_REPLACED_DURING_LOAD` is
/// the one refusal the frontend repeats, and a repeat is a fresh load the earlier
/// cancel never reaches, so a cancelled load answered that way opened anyway.
/// Unix only, as above (the other session runs on a second state).
#[cfg(unix)]
#[test]
fn a_cancelled_load_whose_file_was_also_replaced_is_answered_cancelled() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let a = saved(temp.path(), "CaveA.stalx", "CaveA");
    let state = state_with_null_sink();
    let other = state_with_null_sink();
    let (at_commit, reached) = mpsc::channel::<()>();
    let (go, wait) = mpsc::channel::<()>();
    let load = std::thread::scope(|scope| {
        let load = scope.spawn(|| {
            BEFORE_SESSION_COMMIT.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    at_commit.send(()).unwrap();
                    wait.recv().unwrap();
                }));
            });
            state.do_load_project_full(a.clone())
        });
        reached.recv().unwrap();
        // Both at once: the user cancels, and another session saves the file.
        state.cancel_loading();
        other.do_load_project_full(a.clone()).unwrap();
        assert!(other.execute(new_section("SavedByTheOtherSession")).success);
        assert!(other.execute(Command::SaveProject { path: None }).success);
        assert!(other.execute(Command::CloseProject).success);
        go.send(()).unwrap();
        load.join().unwrap()
    });

    println!(
        "cancelled load of CaveA whose file was also replaced -> {}",
        match &load {
            Ok(_) => "Ok (committed)".to_owned(),
            Err(e) => format!("Err({})", e.code),
        }
    );
    assert_eq!(
        load.err().map(|e| e.code),
        Some(CommandErrorCode::Cancelled),
        "a cancelled load is never answered with the retryable code"
    );
    assert!(state.project_snapshot().is_none(), "nothing committed");
}

/// [FZ-FZ-INT-R2] Off unix the stamp is the save epoch: every save that renames a
/// file into place moves it, the detached copy included.
#[test]
fn every_save_rename_moves_the_save_epoch() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let a = saved(temp.path(), "CaveA.stalx", "CaveA");
    let state = state_with_null_sink();
    state.do_load_project_full(a).unwrap();
    let before = state.saves_renamed_so_far();
    assert!(state.execute(Command::SaveProject { path: None }).success);
    let after_save = state.saves_renamed_so_far();
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(temp.path().join("Other.stalx")),
            })
            .success
    );
    let after_save_as = state.saves_renamed_so_far();
    println!("save epoch: {before} -> save {after_save} -> Save As {after_save_as}");
    assert_eq!(after_save, before + 1);
    assert_eq!(after_save_as, before + 2);
}
