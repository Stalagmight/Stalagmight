//! [#576 NF-W4C-3] The Ariane adoption's local half is ONE `command_lock` span.
//!
//! `speleodb_adopt_ariane` builds the adopted project (create, import, save),
//! promotes it over the cache copy (close, rename, reopen at `dest`) and stamps the
//! SpeleoDB link onto it. Each of those steps used to take `command_lock` on its
//! own, so a command - a frontend edit, an autosave - could run between two of
//! them against the half-built project: an edit whose small section/shot ids
//! resolve landed in the adopted copy and was saved into it, and an autosave
//! between the reopen and the stamp copied the (absent) link and renamed a fresh
//! file over the live one.
//!
//! Driven through `adopt_import_and_save`, the local half, with a synthetic remote
//! file written under a temp dir; nothing here reaches a server. All data is
//! synthetic.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::{Project, SectionData, ShotData, ShotUpdates, SpeleoLink, UndoMode};
use crate::parsers::tml::test_helpers::{wrap_xml, TripShot};
use crate::persistence::Database;

const ROUNDS: usize = 30;
const PROJECT_ID: &str = "33333333-3333-3333-3333-333333333333";
/// Shots in the synthetic remote file.
const REMOTE_SHOTS: i64 = 6;

/// Counts the `loading-progress` events the adoption's create emits - its first
/// step - and can hold the first one (bounded) until `released`, so a test can act
/// at exactly that point.
#[derive(Default)]
struct Probe {
    progress_events: AtomicUsize,
    hold_first: AtomicBool,
    released: AtomicBool,
}

struct ProbeSink(Arc<Probe>);

impl crate::core::events::EventSink for ProbeSink {
    fn emit(&self, event: &str, _payload: serde_json::Value) {
        if event != "loading-progress" {
            return;
        }
        let first = self.0.progress_events.fetch_add(1, Ordering::SeqCst) == 0;
        if first && self.0.hold_first.load(Ordering::SeqCst) {
            let deadline = Instant::now() + Duration::from_millis(500);
            while !self.0.released.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        false
    }
}

/// The local project open before the adoption. Its first section and shot carry
/// the same small ids the adopted survey's do, so an edit aimed at it resolves in
/// the adopted copy too.
fn previous_project(dir: &Path) -> PathBuf {
    let path = dir.join("previous.stalx");
    let mut project = Project::new("CaveB".to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionB".to_string()),
        shots: (0..4)
            .map(|i| ShotData {
                length: 3.0 + f64::from(i),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    });
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    path
}

/// The remote Ariane file as the adoption extracts it beside the cache copy: one
/// chain of `REMOTE_SHOTS` shots. Plain XML (`.tmlu`); the reader is picked by
/// extension, and the import downstream of it is the same.
fn remote_file(dir: &Path) -> PathBuf {
    let shots: Vec<String> = (1..=REMOTE_SHOTS)
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

fn link(undo_mode: UndoMode) -> SpeleoLink {
    SpeleoLink {
        endpoint: "http://speleodb.invalid/".to_string(),
        project_id: PROJECT_ID.to_string(),
        project_name: "CaveA".to_string(),
        base_commit_sha: Some("abc123".to_string()),
        has_unpushed_edits: true,
        last_edit_at: Some("2026-07-27T10:00:00Z".to_string()),
        last_verify_at: None,
        undo_mode: Some(undo_mode),
    }
}

struct Fixture {
    _temp: TempDir,
    state: AppState,
    probe: Arc<Probe>,
    previous: PathBuf,
    remote: PathBuf,
    dest: PathBuf,
}

/// A fresh state with the previous project open, and the remote file extracted
/// into the cache directory the adoption builds `dest` in.
fn fixture() -> Fixture {
    let temp = TempDir::new().unwrap();
    let cache = temp.path().join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    let previous = previous_project(temp.path());
    let remote = remote_file(&cache);
    let state = AppState::new().unwrap();
    let probe = Arc::new(Probe::default());
    state.set_event_sink(Arc::new(ProbeSink(Arc::clone(&probe))));
    assert!(
        state.open_project(previous.clone()).success,
        "the project the adoption replaces opens"
    );
    Fixture {
        dest: cache.join("project.stalx"),
        _temp: temp,
        state,
        probe,
        previous,
        remote,
    }
}

/// Run the adoption's local half, with `observe` called at the stamp (inside the
/// span once it is one). `Ok` only when the build AND the stamp succeeded.
fn adopt(f: &Fixture, observe: impl FnOnce()) -> Result<(), String> {
    let adopted = f.state.adopt_import_and_save(
        None,
        &f.remote,
        "ariane.tml",
        &f.dest,
        "CaveA".to_string(),
        |undo_mode| {
            observe();
            link(undo_mode)
        },
    );
    match adopted {
        Ok((_, Ok(()))) => Ok(()),
        Ok((_, Err(stamp))) => Err(format!("built, but the stamp failed: {stamp}")),
        Err(e) => Err(format!("the build failed: {}", e.message)),
    }
}

fn link_id_on_disk(path: &Path) -> Option<String> {
    Database::open_existing(path)
        .ok()
        .and_then(|db| db.load_speleo_link().ok().flatten())
        .map(|l| l.project_id)
}

fn lengths(project: &Project) -> Vec<f64> {
    project
        .sections
        .iter()
        .flat_map(|section| section.shots.iter().map(|shot| shot.length))
        .collect()
}

/// What the adoption had built at the moment it stamped the link.
#[derive(Debug)]
struct AtStamp {
    open: Option<PathBuf>,
    in_memory: Vec<f64>,
    on_disk: Vec<f64>,
}

fn at_stamp(f: &Fixture) -> AtStamp {
    AtStamp {
        open: f.state.open_project_path(),
        in_memory: f
            .state
            .get_project_state()
            .map(|p| lengths(&p))
            .unwrap_or_default(),
        on_disk: Database::open_existing(&f.dest)
            .ok()
            .and_then(|db| db.load_project().ok())
            .map(|p| lengths(&p))
            .unwrap_or_default(),
    }
}

/// While a command is in flight (this thread holds `command_lock`, as an autosave
/// does) the adoption does not begin: its first step, the create, emits no
/// progress until the command ends.
#[test]
fn the_adoption_waits_for_a_command_in_flight() {
    let _cfg = crate::test_helpers::isolated_config();
    let f = fixture();

    let (during_hold, open_during_hold, adopted) = std::thread::scope(|scope| {
        let commands = f.state.hold_commands();
        let worker = scope.spawn(|| adopt(&f, || {}));
        std::thread::sleep(Duration::from_millis(300));
        let during_hold = f.probe.progress_events.load(Ordering::SeqCst);
        let open_during_hold = f.state.open_project_path();
        drop(commands);
        (during_hold, open_during_hold, worker.join().unwrap())
    });
    let after = f.probe.progress_events.load(Ordering::SeqCst);

    println!(
        "while a command held command_lock the adoption emitted {during_hold} create \
         progress event(s) (open project {open_during_hold:?}); {after} by the end"
    );
    assert_eq!(
        adopted,
        Ok(()),
        "the adoption completes once the lock is free"
    );
    assert!(after > 0, "the probe sees the create's progress at all");
    assert_eq!(
        during_hold, 0,
        "the adoption ran its create while another command held command_lock"
    );
    assert_eq!(open_during_hold.as_deref(), Some(f.previous.as_path()));
}

/// The converse: once the adoption has begun it holds `command_lock` through the
/// stamp. An autosave issued at its first step (the create's first progress event)
/// runs only after the adopted project is open at `dest` with its link stamped -
/// and so copies that link into the file it writes.
#[test]
fn a_command_issued_once_the_adoption_began_runs_after_its_stamp() {
    let _cfg = crate::test_helpers::isolated_config();
    let f = fixture();
    f.probe.hold_first.store(true, Ordering::SeqCst);

    let (adopted, (saved, open_when_it_ran, link_when_it_ran)) = std::thread::scope(|scope| {
        let edit = scope.spawn(|| {
            while f.probe.progress_events.load(Ordering::SeqCst) == 0 {
                std::thread::yield_now();
            }
            let saved = f.state.execute(Command::SaveProject { path: None });
            let seen = (
                saved.success,
                f.state.open_project_path(),
                f.state.speleodb_open_link().ok().map(|l| l.project_id),
            );
            f.probe.released.store(true, Ordering::SeqCst);
            seen
        });
        let adopted = adopt(&f, || {});
        (adopted, edit.join().unwrap())
    });
    let on_disk = link_id_on_disk(&f.dest);

    println!(
        "an autosave issued at the adoption's first step ran with open project \
         {open_when_it_ran:?} and open link {link_when_it_ran:?} (saved: {saved}); \
         the adopted file's link afterwards: {on_disk:?}"
    );
    assert_eq!(adopted, Ok(()));
    assert!(saved, "the autosave itself succeeds");
    assert_eq!(
        open_when_it_ran.as_deref(),
        Some(f.dest.as_path()),
        "the autosave ran before the adoption had reopened its project at dest"
    );
    assert_eq!(
        link_when_it_ran.as_deref(),
        Some(PROJECT_ID),
        "the autosave ran before the adoption stamped the link"
    );
    assert_eq!(on_disk.as_deref(), Some(PROJECT_ID));
}

/// The race itself. A worker edits a shot whose ids resolve in both projects and
/// autosaves, back to back, while this thread adopts once per round, staggered
/// across them. At the stamp the adopted file - and the project open on it - must
/// hold exactly what a quiet adoption of the same file builds, and afterwards the
/// link must be on the open project and in the file.
#[test]
fn an_adoption_under_continuous_edits_and_autosaves_keeps_its_survey_and_link() {
    let _cfg = crate::test_helpers::isolated_config();

    let expected = {
        let f = fixture();
        let seen = Mutex::new(None);
        assert_eq!(
            adopt(&f, || *seen.lock().unwrap() = Some(at_stamp(&f))),
            Ok(())
        );
        let seen = seen.into_inner().unwrap().unwrap();
        assert_eq!(seen.in_memory, seen.on_disk);
        seen.on_disk
    };
    assert_eq!(
        expected.len(),
        usize::try_from(REMOTE_SHOTS).unwrap(),
        "a quiet adoption imports every remote shot: {expected:?}"
    );

    let mut bad = Vec::new();
    for round in 0..ROUNDS {
        let f = fixture();
        let (section_id, shot_id) = {
            let project = f.state.get_project_state().unwrap();
            (project.sections[0].id, project.sections[0].shots[1].id)
        };
        let seen = Mutex::new(None);
        let stop = AtomicBool::new(false);
        let edits = AtomicUsize::new(0);
        let adopted = std::thread::scope(|scope| {
            scope.spawn(|| {
                let mut length = 50.0;
                while !stop.load(Ordering::Relaxed) {
                    length += 0.5;
                    let _ = f.state.execute(Command::UpdateShot {
                        section_id,
                        shot_id,
                        updates: ShotUpdates {
                            length: Some(length),
                            ..ShotUpdates::default()
                        },
                    });
                    let _ = f.state.execute(Command::SaveProject { path: None });
                    edits.fetch_add(1, Ordering::Relaxed);
                    std::thread::yield_now();
                }
            });
            while edits.load(Ordering::Relaxed) < 1 + round % 3 {
                std::thread::yield_now();
            }
            std::thread::sleep(Duration::from_micros(
                250 * u64::try_from(round % 8).unwrap(),
            ));
            let adopted = adopt(&f, || *seen.lock().unwrap() = Some(at_stamp(&f)));
            stop.store(true, Ordering::Relaxed);
            adopted
        });

        let seen = seen.into_inner().unwrap();
        let open_link = f.state.speleodb_open_link().ok().map(|l| l.project_id);
        let on_disk = link_id_on_disk(&f.dest);
        let survey_ok = seen.as_ref().is_some_and(|s| {
            s.open.as_deref() == Some(f.dest.as_path())
                && s.in_memory == expected
                && s.on_disk == expected
        });
        let link_ok =
            open_link.as_deref() == Some(PROJECT_ID) && on_disk.as_deref() == Some(PROJECT_ID);
        if adopted.is_err() || !survey_ok || !link_ok {
            bad.push(format!(
                "round {round}: adopt -> {adopted:?}; at the stamp {seen:?}; link afterwards: \
                 open {open_link:?}, in the file {on_disk:?}"
            ));
        }
    }

    println!(
        "{} of {ROUNDS} round(s) let a concurrent edit or autosave into the adoption \
         (expected survey {expected:?})",
        bad.len()
    );
    for line in bad.iter().take(6) {
        println!("    {line}");
    }
    assert!(
        bad.is_empty(),
        "a command interleaved with the adoption's local half"
    );
}
