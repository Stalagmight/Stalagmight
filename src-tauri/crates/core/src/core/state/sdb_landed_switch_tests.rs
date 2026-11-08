//! [#576 Q-W44-6-D1] Done/Push mid-switch: a push whose upload reached the server
//! before a project switch landed still records its landing in the link of the
//! project it was made for.
//!
//! The push's local bookkeeping after the upload (BASE to the new commit, the dirty
//! mark cleared when no edit came after the push started) runs in a `command_lock`
//! span that checks the open project first (#576 RVF-c-C1). A switch landing between
//! the upload and that span made the check answer `PROJECT_CHANGED` and write
//! nothing: CaveA's link kept its old BASE and its dirty mark although the server
//! held the commit, so the next open of CaveA read our own landed push as somebody
//! else's. Decided: record it in that project's link anyway. The answer stays
//! `PROJECT_CHANGED` (nothing is written to the project now open), and the record
//! goes into the file the push was made from.
//!
//! Driven at the bookkeeping step itself, inside `for_project` made for CaveA while
//! CaveB is open - the order a switch leaves behind. All data is synthetic.
//!
//! [#576 SDB-ACCT-D1] The switch's silent release ran while CaveA was still dirty, so
//! the un-pushed-edits veto kept its lock. A landing that leaves CaveA clean releases
//! that lock, as the switch releases a clean copy's; a lock the user chose to keep,
//! or a copy still dirty, keeps it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::speleodb::{
    authed_session_at, local_dto, recording_stub, HELD_BY_SAME_ACCOUNT_AT_BASE_BODY, REMOTE_ID,
};
use super::speleodb_push::UploadMadeFor;
use super::switch_identity_tests::{identity_of, new_section, saved};
use super::{for_project, AppState, ProjectIdentity};
use crate::core::Command;
use crate::models::{CommandErrorCode, SpeleoLink};
use crate::persistence::Database;
use crate::test_fixtures::state_with_null_sink;

/// CaveA's BASE before the push.
const OLD_BASE: &str = "abc123";
/// The commit the push landed.
const LANDED: &str = "def456";
/// The last edit CaveA recorded before the push started.
const EDITED_BEFORE: &str = "2026-07-27T10:00:00Z";

/// `path`'s SpeleoDB link row as it is on disk.
fn link_on_disk(path: &Path) -> Option<SpeleoLink> {
    Database::open_for_metadata(path)
        .and_then(|db| db.load_speleo_link())
        .ok()
        .flatten()
}

/// Link the project at `path` to `REMOTE_ID`, at the old BASE, dirty since
/// `last_edit_at`.
fn link_dirty(path: &Path, last_edit_at: &str) {
    link_dirty_on(path, "http://speleodb.invalid/", last_edit_at);
}

/// [`link_dirty`] on the deployment at `endpoint`.
fn link_dirty_on(path: &Path, endpoint: &str, last_edit_at: &str) {
    Database::open_existing(path)
        .expect("reopen")
        .save_speleo_link(Some(&SpeleoLink {
            endpoint: endpoint.to_owned(),
            project_id: REMOTE_ID.to_owned(),
            project_name: "CaveA".to_owned(),
            base_commit_sha: Some(OLD_BASE.to_owned()),
            has_unpushed_edits: true,
            last_edit_at: Some(last_edit_at.to_owned()),
            last_verify_at: None,
            undo_mode: None,
        }))
        .expect("link");
}

/// What a switch leaves behind mid-push: linked CaveA opened (its identity and link
/// read as the push read them), then CaveB opened over it.
struct SwitchedMidPush {
    _temp: TempDir,
    state: AppState,
    a: PathBuf,
    b: PathBuf,
    made_for_a: Option<ProjectIdentity>,
    link_a: SpeleoLink,
}

fn switched_mid_push(last_edit_at: &str) -> SwitchedMidPush {
    let temp = TempDir::new().expect("temp");
    let a = saved(temp.path(), "CaveA.stalx", "CaveA");
    let b = saved(temp.path(), "CaveB.stalx", "CaveB");
    link_dirty(&a, last_edit_at);
    let state = state_with_null_sink();
    state.do_load_project_full(a.clone()).expect("open CaveA");
    let made_for_a = identity_of(&state);
    let link_a = state.speleodb_open_link().expect("CaveA's link");
    state.do_load_project_full(b.clone()).expect("open CaveB");
    SwitchedMidPush {
        _temp: temp,
        state,
        a,
        b,
        made_for_a,
        link_a,
    }
}

/// The push's bookkeeping after a landed upload, made for CaveA, run with CaveB open.
/// Answers the refusal code, if any.
fn record_for_a(world: &SwitchedMidPush, push_start: &str) -> Option<CommandErrorCode> {
    record_landing_made_for(
        &world.state,
        &world.a,
        &world.link_a,
        world.made_for_a.clone(),
        push_start,
    )
}

/// The push's bookkeeping after a landed upload, made for the file `a` (its link
/// `link_a`, its identity `made_for_a`), on whatever `state` has open now.
fn record_landing_made_for(
    state: &AppState,
    a: &Path,
    link_a: &SpeleoLink,
    made_for_a: Option<ProjectIdentity>,
    push_start: &str,
) -> Option<CommandErrorCode> {
    let world = MadeFor {
        state,
        a,
        link_a,
        made_for_a,
    };
    world.record(push_start, push_start, None)
}

/// What a push read when it was made for CaveA, and the state it records into.
struct MadeFor<'a> {
    state: &'a AppState,
    a: &'a Path,
    link_a: &'a SpeleoLink,
    made_for_a: Option<ProjectIdentity>,
}

impl MadeFor<'_> {
    /// The push's bookkeeping after a landed upload, with the push's undo cutoff and
    /// the newest undo stamp its snapshot carried.
    fn record(
        self,
        push_start: &str,
        undo_cutoff: &str,
        captured_through: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Option<CommandErrorCode> {
        let landed = self.state.speleodb_revision_landed(REMOTE_ID);
        let made_for = UploadMadeFor {
            file: self.a,
            link: self.link_a,
        };
        for_project(self.made_for_a, || {
            self.state.speleodb_record_landed_upload(
                &landed,
                &made_for,
                Some(LANDED),
                push_start,
                undo_cutoff,
                captured_through,
            )
        })
        .err()
        .map(|e| local_dto(e).code)
    }
}

/// [#576 SDB-ACCT-D1] The app's managed state, lent to a detached release the way the
/// running app lends it (`EventSink::with_app_state`): the test's state, leaked so it
/// outlives every thread it is lent to.
struct LendingSink(&'static AppState);

impl crate::core::events::EventSink for LendingSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}

    fn with_app_state(&self, f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        f(self.0);
        true
    }
}

/// How CaveA was left when the session moved on.
#[derive(Clone, Copy)]
enum LeftBy {
    /// A switch to CaveB: its silent release, which the un-pushed-edits veto declines.
    Switch,
    /// The close dialog's "Keep lock", then CaveB opened.
    KeepLockClose,
}

/// CaveA edited under its lock - on a signed-in session against a stub that names our
/// account as the holder and answers a release - dirty since `last_edit_at`, then left
/// as `left_by` says, with CaveB open.
struct EditedThenLeft {
    _temp: TempDir,
    _listener: std::net::TcpListener,
    base: String,
    seen: Arc<Mutex<Vec<String>>>,
    state: &'static AppState,
    a: PathBuf,
    made_for_a: Option<ProjectIdentity>,
    link_a: SpeleoLink,
}

fn edited_then_left(last_edit_at: &str, left_by: LeftBy) -> EditedThenLeft {
    let (base, listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let temp = TempDir::new().expect("temp");
    let a = saved(temp.path(), "CaveA.stalx", "CaveA");
    let b = saved(temp.path(), "CaveB.stalx", "CaveB");
    link_dirty_on(&a, &base, last_edit_at);
    let state: &'static AppState = Box::leak(Box::new(AppState::new().expect("state")));
    state.set_event_sink(Arc::new(LendingSink(state)));
    *state.speleodb.lock() = authed_session_at(&base);
    state.do_load_project_full(a.clone()).expect("open CaveA");
    // Start editing's commit: the gate open, this session's claim on the lock.
    state.set_edit_enabled(true);
    state.speleodb_note_lock_acquired(Some(REMOTE_ID));
    let made_for_a = identity_of(state);
    let link_a = state.speleodb_open_link().expect("CaveA's link");
    if matches!(left_by, LeftBy::KeepLockClose) {
        state.speleodb_keep_open_lock_deliberately();
        assert!(state.execute(Command::CloseProject).success, "close CaveA");
    }
    state.do_load_project_full(b).expect("open CaveB");
    assert!(
        state.speleodb_claim_on_deployment(&base, REMOTE_ID),
        "premise: CaveA's lock is kept, its claim with it"
    );
    EditedThenLeft {
        _temp: temp,
        _listener: listener,
        base,
        seen,
        state,
        a,
        made_for_a,
        link_a,
    }
}

impl EditedThenLeft {
    fn record(&self, push_start: &str) -> Option<CommandErrorCode> {
        record_landing_made_for(
            self.state,
            &self.a,
            &self.link_a,
            self.made_for_a.clone(),
            push_start,
        )
    }

    /// The release requests the stub has seen for `REMOTE_ID`, waiting up to `wait`
    /// for at least one (the release runs on a detached thread).
    fn releases_within(&self, wait: Duration) -> Vec<String> {
        let deadline = Instant::now() + wait;
        loop {
            let releases: Vec<String> = self
                .seen
                .lock()
                .expect("request log")
                .iter()
                .filter(|line| line.contains(REMOTE_ID) && line.contains("/release/"))
                .cloned()
                .collect();
            if !releases.is_empty() || Instant::now() >= deadline {
                return releases;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn a_landing_that_leaves_the_switched_away_copy_clean_releases_the_lock_the_switch_kept() {
    // The switch's release ran while CaveA was dirty, so the veto kept its lock; the
    // push then landed and CaveA is clean. Its lock is released as the switch would
    // have released a clean copy's: it used to stay held, its claim kept.
    let world = edited_then_left(EDITED_BEFORE, LeftBy::Switch);
    let before_landing = world.releases_within(Duration::ZERO).len();
    let push_start = chrono::Utc::now().to_rfc3339();
    let code = world.record(&push_start);
    let releases = world.releases_within(Duration::from_secs(10));
    let claim_left = world
        .state
        .speleodb_claim_on_deployment(&world.base, REMOTE_ID);
    let dirty = link_on_disk(&world.a).map(|l| l.has_unpushed_edits);
    println!(
        "[SDB-ACCT-D1] switched away dirty, then the push landed -> refusal {code:?}; \
         CaveA dirty {dirty:?}; release requests before the landing {before_landing}, \
         after {releases:?}; claim left {claim_left}"
    );
    assert_eq!(code, Some(CommandErrorCode::ProjectChanged));
    assert_eq!(dirty, Some(false), "premise: the landing left CaveA clean");
    assert_eq!(before_landing, 0, "premise: the switch kept the lock");
    assert_eq!(
        releases.len(),
        1,
        "the kept lock is released once the copy is clean"
    );
    assert!(!claim_left, "and its claim goes with it");
}

/// [#576 W87b] The kept lock's release is reclaim-checked on every path. Without a
/// sink that lends managed state (unit tests, before `setup`) the detached release
/// cannot reach the guard, and it used to send the raw POST, which frees the lock
/// under a session that reopened the project and claimed it meanwhile. Now nothing
/// goes out: the claim is put back, and the stranded-claim janitor hands the lock
/// back through its reclaim-checked release once no project is open.
#[test]
fn without_managed_state_the_kept_lock_is_never_released_unguarded() {
    let world = edited_then_left(EDITED_BEFORE, LeftBy::Switch);
    world
        .state
        .set_event_sink(Arc::new(crate::core::events::NullEventSink));
    let push_start = chrono::Utc::now().to_rfc3339();
    let code = world.record(&push_start);
    let claim_back = world
        .state
        .speleodb_claim_on_deployment(&world.base, REMOTE_ID);
    let unguarded = world.releases_within(Duration::from_millis(500));
    assert!(
        world.state.execute(Command::CloseProject).success,
        "close CaveB"
    );
    let _ = world.state.speleodb_link_status();
    let by_janitor = world.releases_within(Duration::from_secs(10));
    let claim_left = world
        .state
        .speleodb_claim_on_deployment(&world.base, REMOTE_ID);
    println!(
        "[W87b] landed with no managed state -> refusal {code:?}; release requests right \
         after the landing {unguarded:?}; claim put back {claim_back}; after the janitor \
         {by_janitor:?}, claim left {claim_left}"
    );
    assert_eq!(code, Some(CommandErrorCode::ProjectChanged));
    assert!(
        unguarded.is_empty(),
        "no release goes out where the reclaim guard cannot run"
    );
    assert!(claim_back, "the claim is put back for the janitor");
    assert_eq!(
        by_janitor.len(),
        1,
        "which hands the lock back through its reclaim-checked release"
    );
    assert!(!claim_left, "and ends the claim");
}

#[test]
fn a_copy_still_dirty_after_the_landing_keeps_its_lock() {
    // An edit after the push start is still only local: the veto's reason stands.
    let push_start = chrono::Utc::now().to_rfc3339();
    let later = (chrono::Utc::now() + chrono::Duration::seconds(5)).to_rfc3339();
    let world = edited_then_left(&later, LeftBy::Switch);
    let code = world.record(&push_start);
    let releases = world.releases_within(Duration::from_millis(500));
    let claim_left = world
        .state
        .speleodb_claim_on_deployment(&world.base, REMOTE_ID);
    println!(
        "[SDB-ACCT-D1] landed with a later edit -> refusal {code:?}; release requests \
         {releases:?}; claim left {claim_left}"
    );
    assert_eq!(code, Some(CommandErrorCode::ProjectChanged));
    assert!(
        releases.is_empty(),
        "the lock stays held over the un-pushed edit"
    );
    assert!(claim_left);
}

#[test]
fn a_lock_the_user_chose_to_keep_stays_held_after_the_landing() {
    // The close dialog's "Keep lock" is the user's decision, not the veto's: a landing
    // that leaves the copy clean does not undo it.
    let world = edited_then_left(EDITED_BEFORE, LeftBy::KeepLockClose);
    let push_start = chrono::Utc::now().to_rfc3339();
    let code = world.record(&push_start);
    let releases = world.releases_within(Duration::from_millis(500));
    let claim_left = world
        .state
        .speleodb_claim_on_deployment(&world.base, REMOTE_ID);
    let dirty = link_on_disk(&world.a).map(|l| l.has_unpushed_edits);
    println!(
        "[SDB-ACCT-D1] Keep lock, then the push landed -> refusal {code:?}; CaveA dirty \
         {dirty:?}; release requests {releases:?}; claim left {claim_left}"
    );
    assert_eq!(code, Some(CommandErrorCode::ProjectChanged));
    assert_eq!(dirty, Some(false), "premise: the landing left CaveA clean");
    assert!(releases.is_empty(), "the user's Keep lock stands");
    assert!(claim_left);
}

#[test]
fn a_push_switched_away_from_records_its_landing_in_the_link_it_was_made_for() {
    let world = switched_mid_push(EDITED_BEFORE);
    let push_start = chrono::Utc::now().to_rfc3339();
    let code = record_for_a(&world, &push_start);
    let a_link = link_on_disk(&world.a);
    let b_link = link_on_disk(&world.b);
    println!(
        "[Q-W44-6-D1] landed push made for CaveA, CaveB open -> refusal {code:?}; CaveA's \
         link: base {:?}, dirty {:?}; CaveB's link: {b_link:?}; open {:?}",
        a_link.as_ref().map(|l| l.base_commit_sha.clone()),
        a_link.as_ref().map(|l| l.has_unpushed_edits),
        world.state.open_project_path(),
    );
    assert_eq!(
        code,
        Some(CommandErrorCode::ProjectChanged),
        "the verb is still answered PROJECT_CHANGED: another project is open"
    );
    let a_link = a_link.expect("CaveA is still linked");
    assert_eq!(
        a_link.base_commit_sha.as_deref(),
        Some(LANDED),
        "CaveA's BASE is the commit the push landed"
    );
    assert!(
        !a_link.has_unpushed_edits && a_link.last_edit_at.is_none(),
        "and its dirty mark is cleared: every edit was in the upload"
    );
    assert!(b_link.is_none(), "nothing is written into CaveB");
    assert_eq!(
        world.state.open_project_path().as_deref(),
        Some(world.b.as_path()),
        "CaveB stays open"
    );
}

#[test]
fn an_edit_made_after_the_push_started_keeps_the_dirty_mark() {
    // An edit CaveA recorded after the push started (saved by the switch) is not in
    // the upload: BASE advances, the dirty mark stays, as on the open copy.
    let push_start = chrono::Utc::now().to_rfc3339();
    let later = (chrono::Utc::now() + chrono::Duration::seconds(5)).to_rfc3339();
    let world = switched_mid_push(&later);
    let code = record_for_a(&world, &push_start);
    let a_link = link_on_disk(&world.a).expect("CaveA is still linked");
    println!(
        "[Q-W44-6-D1] landed push, an edit after its start -> refusal {code:?}; CaveA's \
         link: base {:?}, dirty {}",
        a_link.base_commit_sha, a_link.has_unpushed_edits
    );
    assert_eq!(code, Some(CommandErrorCode::ProjectChanged));
    assert_eq!(a_link.base_commit_sha.as_deref(), Some(LANDED));
    assert!(
        a_link.has_unpushed_edits,
        "the later edit is still to be pushed"
    );
}

#[test]
fn a_file_that_now_holds_another_project_is_left_alone() {
    // The path the push was made from was replaced by another project's copy before
    // the bookkeeping ran: nothing is written there.
    let world = switched_mid_push(EDITED_BEFORE);
    Database::open_existing(&world.a)
        .expect("reopen")
        .save_speleo_link(Some(&SpeleoLink {
            project_id: "22222222-2222-2222-2222-222222222222".to_owned(),
            ..world.link_a.clone()
        }))
        .expect("relink");
    let push_start = chrono::Utc::now().to_rfc3339();
    let code = record_for_a(&world, &push_start);
    let a_link = link_on_disk(&world.a).expect("still linked");
    println!(
        "[Q-W44-6-D1] the file now holds another project -> refusal {code:?}; its link: \
         base {:?}, dirty {}",
        a_link.base_commit_sha, a_link.has_unpushed_edits
    );
    assert_eq!(code, Some(CommandErrorCode::ProjectChanged));
    assert_eq!(a_link.base_commit_sha.as_deref(), Some(OLD_BASE));
    assert!(a_link.has_unpushed_edits);
}

/// The undo stamps of `path`'s persisted undo stack (oldest first), and how many redo
/// rows it holds.
fn history_on_disk(path: &Path) -> (Vec<Option<String>>, usize) {
    let db = Database::open_for_metadata(path).expect("open for metadata");
    let undo = db
        .load_undo_stack_with_timestamps()
        .expect("undo rows")
        .into_iter()
        .map(|(_, _, _, stamp)| stamp)
        .collect();
    let redo = db
        .load_redo_stack_with_timestamps()
        .expect("redo rows")
        .len();
    (undo, redo)
}

/// What [`landed_after_switch_with_history`] saw.
struct HistoryOutcome {
    code: Option<CommandErrorCode>,
    on_disk_before: (Vec<Option<String>>, usize),
    on_disk_after: (Vec<Option<String>>, usize),
    undo_cutoff: chrono::DateTime<chrono::Utc>,
    open_can_undo: bool,
}

/// [#303] CaveA edited under its lock - `before` sections created and saved, as the
/// push's pre-push save leaves its file - then the push starts (its cutoff and the
/// newest stamp its snapshot carried), `after` more sections are created and saved,
/// and the session switches to CaveB. Records the landing made for CaveA.
fn landed_after_switch_with_history(before: usize, after: usize) -> HistoryOutcome {
    let temp = TempDir::new().expect("temp");
    let a = saved(temp.path(), "CaveA.stalx", "CaveA");
    let b = saved(temp.path(), "CaveB.stalx", "CaveB");
    link_dirty(&a, EDITED_BEFORE);
    let state = state_with_null_sink();
    state.do_load_project_full(a.clone()).expect("open CaveA");
    state.set_edit_enabled(true);
    let made_for_a = identity_of(&state);
    let link_a = state.speleodb_open_link().expect("CaveA's link");
    let edit_and_save = |n: usize, tag: &str| {
        for i in 0..n {
            let created = state.execute(new_section(&format!("{tag}{i}")));
            assert!(created.success, "{:?}", created.error);
        }
        let saved = state.execute(Command::SaveProject { path: None });
        assert!(saved.success, "{:?}", saved.error);
    };
    edit_and_save(before, "pushed");
    std::thread::sleep(Duration::from_millis(5));
    // The push's two instants, as `speleodb_upload` takes them from its snapshot.
    let push_start = chrono::Utc::now();
    let captured_through = Database::open_for_metadata(&a)
        .expect("snapshot")
        .newest_undo_timestamp()
        .expect("newest stamp");
    let undo_cutoff = captured_through.map_or(push_start, |newest| newest.max(push_start));
    std::thread::sleep(Duration::from_millis(5));
    edit_and_save(after, "later");
    let on_disk_before = history_on_disk(&a);
    state.do_load_project_full(b).expect("open CaveB");
    let code = MadeFor {
        state: &state,
        a: &a,
        link_a: &link_a,
        made_for_a,
    }
    .record(
        &push_start.to_rfc3339(),
        &undo_cutoff.to_rfc3339(),
        captured_through,
    );
    HistoryOutcome {
        code,
        on_disk_before,
        on_disk_after: history_on_disk(&a),
        undo_cutoff,
        open_can_undo: state.can_undo(),
    }
}

#[test]
fn a_push_switched_away_from_drops_the_history_it_carried_from_that_file() {
    // The landed commit carries CaveA's two edits; left in CaveA's file, the next
    // push of CaveA carried them again (#303 duplication).
    let outcome = landed_after_switch_with_history(2, 0);
    println!(
        "[#303] landed push made for CaveA, CaveB open -> refusal {:?}; CaveA's undo rows \
         before {} (redo {}), after {} (redo {}); CaveB can undo {}",
        outcome.code,
        outcome.on_disk_before.0.len(),
        outcome.on_disk_before.1,
        outcome.on_disk_after.0.len(),
        outcome.on_disk_after.1,
        outcome.open_can_undo
    );
    assert_eq!(outcome.code, Some(CommandErrorCode::ProjectChanged));
    assert_eq!(
        outcome.on_disk_before.0.len(),
        2,
        "premise: the edits are in the file"
    );
    assert!(
        outcome.on_disk_after.0.is_empty() && outcome.on_disk_after.1 == 0,
        "the history the commit carried is gone from CaveA's file, redo with it"
    );
    assert!(!outcome.open_can_undo, "CaveB's history is not touched");
}

#[test]
fn an_edit_made_after_the_push_started_keeps_its_history_in_that_file() {
    // The later edit is not in the commit: its undo entry stays, as on the open copy.
    let outcome = landed_after_switch_with_history(1, 1);
    let kept: Vec<String> = outcome.on_disk_after.0.iter().flatten().cloned().collect();
    println!(
        "[#303] landed push, an edit after its start -> CaveA's undo rows before {}, after \
         {kept:?}; cutoff {}",
        outcome.on_disk_before.0.len(),
        outcome.undo_cutoff.to_rfc3339()
    );
    assert_eq!(
        outcome.on_disk_before.0.len(),
        2,
        "premise: both edits are in the file"
    );
    assert_eq!(kept.len(), 1, "only the later edit's entry is left");
    let stamp = kept
        .first()
        .and_then(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).ok())
        .map(|stamp| stamp.with_timezone(&chrono::Utc));
    assert!(
        stamp.is_some_and(|stamp| stamp > outcome.undo_cutoff),
        "and it is the later one: {stamp:?}"
    );
}

/// [#576 W87b] The identity check compares paths literally, so the push's own project
/// reopened mid-verb through another spelling of its path (here a `..` segment; a
/// symlink or a case variant does the same) is refused as another project. The file
/// is the open one, which made the landing skip, "its session owns the link" - and
/// nothing recorded it: that copy kept its old BASE and its dirty mark. The same
/// `created_at` says it is the push's project, so the landing is recorded there.
#[test]
fn a_push_whose_project_is_open_again_under_another_spelling_records_its_landing_there() {
    let world = switched_mid_push(EDITED_BEFORE);
    let dir = world.a.parent().expect("the projects folder");
    std::fs::create_dir_all(dir.join("sub")).expect("a sibling folder");
    let alias = dir
        .join("sub")
        .join("..")
        .join(world.a.file_name().expect("CaveA's name"));
    world
        .state
        .do_load_project_full(alias.clone())
        .expect("reopen CaveA through the alias");
    let reopened = identity_of(&world.state);
    let push_start = chrono::Utc::now().to_rfc3339();
    let code = record_for_a(&world, &push_start);
    let a_link = link_on_disk(&world.a).expect("CaveA is still linked");
    let open_link = world.state.speleodb_open_link().ok();
    println!(
        "[W87b] push made for {}, CaveA open again as {:?} -> refusal {code:?}; CaveA's link: \
         base {:?}, dirty {}; the open session's link: base {:?}",
        world.a.display(),
        world.state.open_project_path(),
        a_link.base_commit_sha,
        a_link.has_unpushed_edits,
        open_link.as_ref().map(|l| l.base_commit_sha.clone()),
    );
    assert_eq!(
        reopened.as_ref().map(|r| &r.created_at),
        world.made_for_a.as_ref().map(|m| &m.created_at),
        "premise: the push's own project is open again"
    );
    assert_eq!(
        world.state.open_project_path().as_deref(),
        Some(alias.as_path()),
        "premise: under another spelling of its path"
    );
    assert_eq!(
        code,
        Some(CommandErrorCode::ProjectChanged),
        "the identity check still refuses it"
    );
    assert_eq!(
        a_link.base_commit_sha.as_deref(),
        Some(LANDED),
        "the landing is recorded in CaveA's link"
    );
    assert!(
        !a_link.has_unpushed_edits && a_link.last_edit_at.is_none(),
        "and its dirty mark is cleared"
    );
}

/// [#576 W87b] The other way a refused landing finds its file open: another project
/// was saved over that path mid-verb (another `created_at`), even one linked to the
/// same SpeleoDB project. The push's project is not in that file any more, so nothing
/// is recorded there, and the log says so instead of "its session owns the link".
#[test]
fn a_path_another_project_was_saved_over_records_nothing_and_says_so() {
    let world = switched_mid_push(EDITED_BEFORE);
    // CaveB, open, saved over CaveA's path, then linked to the same remote project.
    let saved_over = world.state.execute(Command::SaveProject {
        path: Some(world.a.clone()),
    });
    assert!(
        saved_over.success,
        "Save As over CaveA: {:?}",
        saved_over.error
    );
    world
        .state
        .database
        .lock()
        .as_ref()
        .expect("CaveB's database")
        .save_speleo_link(Some(&world.link_a))
        .expect("link CaveB to the same remote project");
    let over = identity_of(&world.state);
    let push_start = chrono::Utc::now().to_rfc3339();
    let mut code = None;
    let logs = super::speleodb::logged_events(|| {
        code = record_for_a(&world, &push_start);
    });
    let link = link_on_disk(&world.a).expect("the file at CaveA's path is linked");
    let said: Vec<&String> = logs
        .iter()
        .filter(|line| line.contains("the file the push was made for"))
        .collect();
    println!(
        "[W87b] CaveB saved over {} -> refusal {code:?}; its link: base {:?}, dirty {}; \
         logged {said:?}",
        world.a.display(),
        link.base_commit_sha,
        link.has_unpushed_edits,
    );
    assert_ne!(
        over.as_ref().map(|o| &o.created_at),
        world.made_for_a.as_ref().map(|m| &m.created_at),
        "premise: another project lives at the path now"
    );
    assert_eq!(code, Some(CommandErrorCode::ProjectChanged));
    assert_eq!(
        link.base_commit_sha.as_deref(),
        Some(OLD_BASE),
        "nothing is recorded in the other project's file"
    );
    assert!(link.has_unpushed_edits, "its dirty mark included");
    assert!(
        said.iter()
            .any(|line| line.contains("holds another project now")),
        "and the log says why: {said:?}"
    );
}
