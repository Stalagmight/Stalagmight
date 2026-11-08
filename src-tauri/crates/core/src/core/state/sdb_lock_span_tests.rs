//! [#576 NF-W4C-1 / NF-W4C-2] SpeleoDB link writers outside Done never
//! interleave with an autosave.
//!
//! An autosave runs through `execute`, copies the open file's `speleo_link` row at
//! its start, writes a fresh file and renames it over the live one, with the
//! database handle out of the slot in between. A link write that takes only the
//! `database` lock can land inside that span: it goes into the file about to be
//! replaced (the autosave then writes the copy it took before the write), or it
//! finds no handle at all. B35(d) closed that for Done; these are the other
//! writers - Publish's relocation and stamp, and Start Editing's commit stamp.
//!
//! Each round autosaves continuously on a worker while this thread runs the writer
//! once, staggered across the save, then reads the link back. All data is
//! synthetic.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::{Project, SectionData, ShotData, SpeleoLink};
use crate::persistence::Database;

const ROUNDS: usize = 30;
const PROJECT_ID: &str = "11111111-1111-1111-1111-111111111111";

fn survey() -> Project {
    let mut project = Project::new("CaveA".to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionA".to_string()),
        shots: (0..20)
            .map(|i| ShotData {
                length: 2.0 + f64::from(i),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    });
    project
}

fn link() -> SpeleoLink {
    SpeleoLink {
        endpoint: "http://speleodb.invalid/".to_string(),
        project_id: PROJECT_ID.to_string(),
        project_name: "CaveA".to_string(),
        base_commit_sha: Some("abc123".to_string()),
        has_unpushed_edits: true,
        last_edit_at: Some("2026-07-27T10:00:00Z".to_string()),
        last_verify_at: None,
        undo_mode: None,
    }
}

fn local_file(path: &Path) {
    let mut db = Database::open(path).unwrap();
    db.save_project(&survey()).unwrap();
}

fn linked_file(path: &Path) {
    let mut db = Database::open(path).unwrap();
    db.save_project(&survey()).unwrap();
    db.save_speleo_link(Some(&link())).unwrap();
}

/// Run `write` once on this thread while a worker autosaves continuously, with the
/// write staggered across the save by `round`. Returns what `write` returned.
fn under_autosave<T>(state: &AppState, round: usize, write: impl FnOnce() -> T) -> T {
    let stop = AtomicBool::new(false);
    let saves = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !stop.load(Ordering::Relaxed) {
                let _ = state.execute(Command::SaveProject { path: None });
                saves.fetch_add(1, Ordering::Relaxed);
                std::thread::yield_now();
            }
        });
        while saves.load(Ordering::Relaxed) < 1 + round % 3 {
            std::thread::yield_now();
        }
        // The counter wakes this thread just as a save ENDS; stagger the write
        // across the next save, so rounds land before, inside and after the span
        // in which it holds a copy of the link.
        std::thread::sleep(std::time::Duration::from_micros(
            250 * u64::try_from(round % 12).unwrap(),
        ));
        let written = write();
        let after = saves.load(Ordering::Relaxed) + 2;
        while saves.load(Ordering::Relaxed) < after {
            std::thread::yield_now();
        }
        stop.store(true, Ordering::Relaxed);
        written
    })
}

fn report(what: &str, lost: &[String]) {
    println!(
        "{} of {ROUNDS} round(s) lost {what} under a concurrent autosave",
        lost.len()
    );
    for line in lost.iter().take(6) {
        println!("    {line}");
    }
}

#[test]
fn publish_relocates_and_stamps_whatever_an_autosave_is_doing() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let state = AppState::new().unwrap();

    let mut lost = Vec::new();
    for round in 0..ROUNDS {
        // A fresh LOCAL project each round: publish relocates only an unlinked one.
        let local = temp.path().join(format!("local-{round}.stalx"));
        local_file(&local);
        assert!(state.open_project(local.clone()).success);
        let probe = state.execute(Command::SaveProject { path: None });
        assert!(probe.success, "the autosave runs: {:?}", probe.error);

        let dest: PathBuf = temp.path().join(format!("cache-{round}.stalx"));
        let published = under_autosave(&state, round, || {
            state.speleodb_publish_relocate(&dest, |undo_mode| SpeleoLink {
                undo_mode: Some(undo_mode),
                ..link()
            })
        });

        let open_link = state.speleodb_open_link();
        let on_disk = Database::open_existing(&dest)
            .ok()
            .and_then(|db| db.load_speleo_link().ok().flatten());
        let ok = published.is_ok()
            && open_link.as_ref().is_ok_and(|l| l.project_id == PROJECT_ID)
            && on_disk.as_ref().is_some_and(|l| l.project_id == PROJECT_ID);
        if !ok {
            lost.push(format!(
                "round {round}: publish -> {:?}; open link {}; link in the cache file {}",
                published.as_ref().map(|_| ()).map_err(ToString::to_string),
                if open_link.is_ok() {
                    "present"
                } else {
                    "MISSING"
                },
                if on_disk.is_some() {
                    "present"
                } else {
                    "MISSING"
                },
            ));
        }
    }
    report("the published link", &lost);
    assert!(
        lost.is_empty(),
        "an autosave dropped (or refused) the link publish stamped on the cache file"
    );
}

#[test]
fn start_editing_stamps_its_verify_whatever_an_autosave_is_doing() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    // [#576 W88] The commit opens the gate only for a session signed in to the server
    // the copy is linked to.
    *state.speleodb.lock() = super::speleodb::authed_session_at(&link().endpoint);
    let path = temp.path().join("linked.stalx");
    linked_file(&path);
    assert!(state.open_project(path).success);
    let probe = state.execute(Command::SaveProject { path: None });
    assert!(probe.success, "the autosave runs: {:?}", probe.error);

    let mut lost = Vec::new();
    for round in 0..ROUNDS {
        // The copy as Start Editing finds it: never verified. No autosave runs yet.
        {
            let _commands = state.hold_commands();
            state
                .speleodb_mutate_link(|l| l.last_verify_at = None)
                .unwrap();
        }
        let generation = state.speleodb_session_generation();
        let committed = under_autosave(&state, round, || {
            state.speleodb_commit_editing(
                &state.speleodb_lock_ops_guard(),
                Some(generation),
                PROJECT_ID,
                false,
            )
        });
        let verified = state
            .speleodb_open_link()
            .ok()
            .and_then(|l| l.last_verify_at);
        if committed.is_err() || verified.is_none() {
            lost.push(format!(
                "round {round}: commit -> {:?}; last_verify_at afterwards {verified:?}",
                committed.as_ref().map_err(ToString::to_string),
            ));
        }
    }
    report("Start Editing's verify stamp", &lost);
    assert!(
        lost.is_empty(),
        "an autosave undid (or refused) the stamp Start Editing's commit wrote"
    );
}

/// Whether `write`, started on a worker while this thread holds `command_lock` (as
/// an in-flight autosave does), waits for it rather than finishing underneath.
fn waits_for_command_lock(state: &AppState, write: impl FnOnce() + Send) -> bool {
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let commands = state.hold_commands();
        scope.spawn(|| {
            write();
            done.store(true, Ordering::SeqCst);
        });
        std::thread::sleep(std::time::Duration::from_millis(300));
        let finished_underneath = done.load(Ordering::SeqCst);
        drop(commands);
        !finished_underneath
    })
}

/// The deterministic form of the races above, for every link writer outside Done
/// that runs outside `execute`: each one waits for a command (an autosave) in
/// flight. Start Editing's commit holds `project`, which an autosave also holds,
/// but the save drops it before it installs the reopened handle
/// (`save_project`), so only `command_lock` closes that gap.
#[test]
fn link_writers_wait_for_a_command_in_flight() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let state = AppState::new().unwrap();

    let local = temp.path().join("local.stalx");
    local_file(&local);
    assert!(state.open_project(local).success);
    let dest = temp.path().join("cache.stalx");
    let publish = waits_for_command_lock(&state, || {
        let published = state.speleodb_publish_relocate(&dest, |undo_mode| SpeleoLink {
            undo_mode: Some(undo_mode),
            ..link()
        });
        assert!(published.is_ok());
    });

    // [#576 W88] Signed in to the copy's server for the commit only (it opens the gate
    // for no other session), and out again, so the unlink below hands nothing back.
    let signed_out = std::mem::replace(
        &mut *state.speleodb.lock(),
        super::speleodb::authed_session_at(&link().endpoint),
    );
    let generation = state.speleodb_session_generation();
    let commit = waits_for_command_lock(&state, || {
        assert!(state
            .speleodb_commit_editing(
                &state.speleodb_lock_ops_guard(),
                Some(generation),
                PROJECT_ID,
                false
            )
            .is_ok());
    });
    *state.speleodb.lock() = signed_out;

    let unlink = waits_for_command_lock(&state, || {
        assert!(state.speleodb_unlink_in_place().is_ok());
    });

    println!("publish's relocation + stamp waits for command_lock: {publish}");
    println!("Start Editing's commit waits for command_lock: {commit}");
    println!("unlink's link delete waits for command_lock: {unlink}");
    assert!(publish && commit && unlink);
}
