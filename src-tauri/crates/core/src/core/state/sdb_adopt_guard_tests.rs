//! [#576 NF-W5L-3] The Ariane adoption's local-edits guard runs inside its
//! `command_lock` span.
//!
//! Re-adopting over the cache copy rebuilds it from the server's `.tml`, so the
//! adoption refuses while that copy holds un-pushed edits (unless the user
//! confirmed discarding them). The check ran BEFORE the span that builds and
//! promotes the copy, so an edit to the open cache copy landing between the two
//! was overwritten without a word. `adopt_import_and_save` now runs the guard
//! first thing inside its span.
//!
//! Driven through `adopt_import_and_save`, with a synthetic remote file and a
//! synthetic linked cache copy under a temp dir; nothing reaches a server.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::{Project, SectionData, ShotData, ShotUpdates, SpeleoLink};
use crate::parsers::tml::test_helpers::{wrap_xml, TripShot};
use crate::persistence::Database;
use crate::test_fixtures::state_with_null_sink;

const PROJECT_ID: &str = "44444444-4444-4444-4444-444444444444";

fn link(dirty: bool) -> SpeleoLink {
    SpeleoLink {
        endpoint: "http://speleodb.invalid/".to_string(),
        project_id: PROJECT_ID.to_string(),
        project_name: "CaveA".to_string(),
        base_commit_sha: Some("abc123".to_string()),
        has_unpushed_edits: dirty,
        last_edit_at: dirty.then(|| "2026-07-27T10:00:00Z".to_string()),
        last_verify_at: None,
        undo_mode: None,
    }
}

/// The linked cache copy at `dest`, open and editable (a clean edit session).
fn open_cache_copy(dest: &Path) -> AppState {
    let mut project = Project::new("CaveA".to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionA".to_string()),
        shots: vec![ShotData {
            length: 5.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    let mut db = Database::open(dest).unwrap();
    db.save_project(&project).unwrap();
    db.save_speleo_link(Some(&link(false))).unwrap();
    drop(db);
    let state = state_with_null_sink();
    assert!(state.open_project(dest.to_path_buf()).success);
    state.set_edit_enabled(true);
    state
}

/// An edit of the open copy, saved: the first shot's length, which marks it
/// un-pushed.
fn edit(state: &AppState) {
    let project = state.get_project_state().unwrap();
    let section = &project.sections[0];
    let edited = state.execute(Command::UpdateShot {
        section_id: section.id,
        shot_id: section.shots[0].id,
        updates: ShotUpdates {
            length: Some(42.0),
            ..ShotUpdates::default()
        },
    });
    assert!(edited.success, "{:?}", edited.error);
    assert!(state.execute(Command::SaveProject { path: None }).success);
}

fn remote_file(dir: &Path) -> PathBuf {
    let shots: Vec<String> = (1..=3)
        .map(|id| {
            TripShot {
                id,
                from_id: if id == 1 { -1 } else { id - 1 },
                section: "SectionA",
                ..TripShot::default()
            }
            .xml()
        })
        .collect();
    let path = dir.join("adopt-source.tmlu");
    std::fs::write(&path, wrap_xml(&shots.join("\n"))).unwrap();
    path
}

fn first_length_on_disk(path: &Path) -> Option<f64> {
    Database::open_existing(path)
        .ok()
        .and_then(|db| db.load_project().ok())
        .and_then(|p| {
            p.sections
                .first()
                .and_then(|s| s.shots.first())
                .map(|s| s.length)
        })
}

fn adopt(
    state: &AppState,
    guarded: Option<&str>,
    remote: &Path,
    dest: &Path,
) -> Result<(), String> {
    state
        .adopt_import_and_save(
            guarded,
            remote,
            "ariane.tml",
            dest,
            "CaveA".to_string(),
            |_| link(true),
        )
        .map(|_| ())
        .map_err(|e| e.message)
}

#[test]
fn an_edit_made_before_the_span_is_seen_and_the_adoption_refuses() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let dest = temp.path().join("project.stalx");
    let remote = remote_file(temp.path());
    let state = open_cache_copy(&dest);
    // What the old order checked first: the copy was clean then ...
    assert!(state.guard_local_edits(PROJECT_ID, &dest).is_ok());
    // ... and an edit lands before the adoption's span.
    edit(&state);

    let refused = adopt(&state, Some(PROJECT_ID), &remote, &dest);
    let on_disk = first_length_on_disk(&dest);
    println!("adoption over a copy edited just before the span -> {refused:?}; first shot on disk {on_disk:?}");
    assert!(refused.is_err(), "the un-pushed edit must stop the rebuild");
    assert_eq!(on_disk, Some(42.0), "and the edited copy is kept as it was");
}

#[test]
fn a_confirmed_discard_still_rebuilds_over_the_edits() {
    // DO-NOT-REGRESS: "take the server copy and discard my local changes" waives it.
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let dest = temp.path().join("project.stalx");
    let remote = remote_file(temp.path());
    let state = open_cache_copy(&dest);
    edit(&state);

    let rebuilt = adopt(&state, None, &remote, &dest);
    let on_disk = first_length_on_disk(&dest);
    println!("adoption with the discard confirmed -> {rebuilt:?}; first shot on disk {on_disk:?}");
    assert!(rebuilt.is_ok(), "{rebuilt:?}");
    assert_ne!(
        on_disk,
        Some(42.0),
        "the copy was rebuilt from the server's file"
    );
}
