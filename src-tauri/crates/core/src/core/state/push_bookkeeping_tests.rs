//! [#576 B35(d)] Done's record of a landed upload never interleaves with an
//! autosave.
//!
//! After a 200 (or a 304) Done advances BASE and clears the dirty bit on the open
//! copy's `speleo_link` row. An autosave runs through `execute`, copies that row
//! at its start, writes a fresh file and renames it over the live one, with the
//! database handle out of the slot in between. Before the fix Done's write took
//! only the `database` lock, so a write landing inside an autosave went into the
//! file about to be replaced (BASE and the dirty bit came back stale, which the
//! next push reads as a conflict with the user's own revision) or failed with
//! "no remote project" (Done reported an error for a revision that had landed).
//!
//! Each round autosaves continuously on a worker while this thread records a
//! landed upload once, then reads the link back. All data is synthetic.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::{Project, SectionData, ShotData, SpeleoLink};
use crate::persistence::Database;

const ROUNDS: usize = 30;
const LANDED: &str = "def456";

fn linked_file(dir: &Path) -> PathBuf {
    let path = dir.join("linked.stalx");
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
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    db.save_speleo_link(Some(&SpeleoLink {
        endpoint: "http://speleodb.invalid/".to_string(),
        project_id: "11111111-1111-1111-1111-111111111111".to_string(),
        project_name: "CaveA".to_string(),
        base_commit_sha: Some("abc123".to_string()),
        has_unpushed_edits: true,
        last_edit_at: Some("2026-07-27T10:00:00Z".to_string()),
        last_verify_at: None,
        undo_mode: None,
    }))
    .unwrap();
    path
}

#[test]
fn a_landed_upload_is_recorded_whatever_an_autosave_is_doing() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    let file = linked_file(temp.path());
    assert!(state.open_project(file.clone()).success);
    // [#576 Q-W44-6-D1] The file and link the push was made from (the open copy here).
    let made_for_link = state.speleodb_open_link().unwrap();
    // The race needs saves that really rewrite the file; a refused save is no autosave.
    let probe = state.execute(Command::SaveProject { path: None });
    assert!(probe.success, "the autosave runs: {:?}", probe.error);

    let mut lost = Vec::new();
    for round in 0..ROUNDS {
        // The copy as Done finds it: dirty, at the old BASE. No autosave runs yet.
        state
            .speleodb_mutate_link(|l| {
                l.base_commit_sha = Some("abc123".to_string());
                l.has_unpushed_edits = true;
                l.last_edit_at = Some("2026-07-27T10:00:00Z".to_string());
            })
            .unwrap();

        let stop = AtomicBool::new(false);
        let saves = AtomicUsize::new(0);
        let recorded = std::thread::scope(|scope| {
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
            // The counter wakes this thread just as a save ENDS; stagger the record
            // across the next save, so rounds land before, inside and after the span
            // in which it holds a copy of the link.
            std::thread::sleep(std::time::Duration::from_micros(
                250 * u64::try_from(round % 12).unwrap(),
            ));
            let push_start = chrono::Utc::now().to_rfc3339();
            // [#576 A8] Recording a landing takes the proof that one was concluded.
            let landed = state.speleodb_revision_landed("push-bookkeeping");
            let recorded = state.speleodb_record_landed_upload(
                &landed,
                &super::speleodb_push::UploadMadeFor {
                    file: &file,
                    link: &made_for_link,
                },
                Some(LANDED),
                &push_start,
                &push_start,
                None,
            );
            let after = saves.load(Ordering::Relaxed) + 2;
            while saves.load(Ordering::Relaxed) < after {
                std::thread::yield_now();
            }
            stop.store(true, Ordering::Relaxed);
            recorded
        });

        let link = state.speleodb_open_link().unwrap();
        let ok = recorded.is_ok()
            && link.base_commit_sha.as_deref() == Some(LANDED)
            && !link.has_unpushed_edits;
        if !ok {
            lost.push(format!(
                "round {round}: record -> {:?}; link afterwards: base {:?}, dirty {}",
                recorded.as_ref().map_err(ToString::to_string),
                link.base_commit_sha,
                link.has_unpushed_edits
            ));
        }
    }
    println!(
        "{} of {ROUNDS} round(s) lost the landed upload under a concurrent autosave",
        lost.len()
    );
    for line in lost.iter().take(6) {
        println!("    {line}");
    }
    assert!(
        lost.is_empty(),
        "an autosave undid (or refused) the record of a revision that landed"
    );
}
