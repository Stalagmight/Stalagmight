//! [#576 SDB-POLL-D1] An acquire whose reply was lost, with the holder re-read after
//! it failing too, is settled by the first successful status poll.
//!
//! Start Editing sends the acquire, the server applies it and the reply is lost (a
//! 200 with an unreadable body); the re-read that FZ-SDB-6-D1 sends at once fails
//! (503). Start Editing fails with the gate shut and no claim, while the server holds
//! the lock for this account. The status poll then reopened the gate over it as an
//! INHERITED lock - no claim - so the quit's silent release (which hands back only a
//! lock this session took) left it held (the fuzzer's I2b).
//!
//! Now the acquire is marked unconfirmed, and the first successful poll settles it:
//! our account holding the lock records the claim (so the quit hands it back);
//! another holder, or none, drops the mark; a close drops it too.
//!
//! [#576 SDB-ACCT-D1] The poll's reopen adopts a lock the account holds (a resumed
//! claim: the release, not the authorship shortcut) whether or not anything is
//! marked, so the quit hands it back either way; what the mark adds is the kind of
//! claim the lost reply cost - this session's acquisition when the probe before the
//! acquire saw the lock free. And the acquire is marked in every case now: also when
//! the probe saw our own account holding the lock (W29's narrowing removed), as the
//! adopted kind.
//!
//! Driven against the loopback routing stub; each test has its own static phase,
//! since the stub routes by a plain `fn`. All data is synthetic, built in code.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::speleodb::{
    authed_session_at, remote_readonly_fixture, routing_stub, FREE_MUTEX_AT_BASE_BODY,
    HELD_BY_ANOTHER_ACCOUNT_AT_BASE_BODY, HELD_BY_SAME_ACCOUNT_AT_BASE_BODY, REMOTE_ID,
};
use super::AppState;
use crate::core::Command;

/// The acquire landed; its reply arrives cut off mid-way (a parse failure).
const TRUNCATED_PROJECT: &str = r#"{"id":"11111111-1111-1111-1111-111111111111","na"#;
/// The holder re-read failing: the server is down.
const UNAVAILABLE: (u16, &str) = (503, r#"{"detail":"service unavailable"}"#);
/// How many requests one failed holder re-read makes: the read and the client's two
/// retries of a 5xx (`MAX_RETRIES` in the SpeleoDB client).
const REREAD_ATTEMPTS: usize = 3;

/// The request line's path.
fn path_of(line: &str) -> &str {
    line.split_whitespace().nth(1).unwrap_or_default()
}

fn is_acquire(line: &str) -> bool {
    line.starts_with("POST ") && path_of(line).ends_with("/acquire/")
}

fn is_release(line: &str) -> bool {
    line.starts_with("POST ") && path_of(line).ends_with("/release/")
}

fn is_project_read(line: &str) -> bool {
    line.starts_with("GET ") && path_of(line) == format!("/api/v2/projects/{REMOTE_ID}/")
}

/// The server's answers for one test: before the first acquire the lock is free;
/// that acquire lands with an unreadable reply and the project read right after it
/// fails, every attempt of it; from then on the holder is `after` (re-acquires
/// answer as `after` too).
struct Phases {
    acquires: AtomicUsize,
    reread_failures: AtomicUsize,
}

impl Phases {
    const fn new() -> Self {
        Self {
            acquires: AtomicUsize::new(0),
            reread_failures: AtomicUsize::new(0),
        }
    }

    fn answer(&self, line: &str, after: &'static str) -> (u16, &'static str) {
        self.answer_from(line, FREE_MUTEX_AT_BASE_BODY, after)
    }

    /// [`Self::answer`] with the holder before the first acquire given (`before`).
    fn answer_from(
        &self,
        line: &str,
        before: &'static str,
        after: &'static str,
    ) -> (u16, &'static str) {
        if is_acquire(line) {
            return if self.acquires.fetch_add(1, Ordering::SeqCst) == 0 {
                (200, TRUNCATED_PROJECT)
            } else {
                (200, after)
            };
        }
        if self.acquires.load(Ordering::SeqCst) == 0 {
            return (200, before);
        }
        if is_project_read(line)
            && self.reread_failures.fetch_add(1, Ordering::SeqCst) < REREAD_ATTEMPTS
        {
            return UNAVAILABLE;
        }
        (200, after)
    }
}

/// Start Editing against `route` on a clean, signed-in, not-editing copy.
fn start_lost_unread(
    route: fn(&str) -> (u16, &'static str),
) -> (
    tempfile::TempDir,
    AppState,
    Arc<Mutex<Vec<String>>>,
    std::net::TcpListener,
) {
    let (base, listener, seen) = routing_stub(route);
    let (temp, state) = remote_readonly_fixture();
    *state.speleodb.lock() = authed_session_at(&base);
    let started = state
        .speleodb_start_editing()
        .map(|_| ())
        .map_err(|e| e.message);
    println!(
        "[SDB-POLL-D1] start editing (acquire reply lost, re-read 503) -> {started:?}; gate {}; \
         claim {}; marked {}",
        state.edit_enabled(),
        state.speleodb_lock_acquired_here(REMOTE_ID),
        marked(&state),
    );
    assert!(started.is_err(), "the acquire is reported failed");
    assert!(
        !state.edit_enabled() && !state.speleodb_lock_acquired_here(REMOTE_ID),
        "gate shut, nothing claimed"
    );
    (temp, state, seen, listener)
}

/// Whether an unconfirmed acquire is marked.
fn marked(state: &AppState) -> bool {
    state.speleodb.lock().unconfirmed_acquire.is_some()
}

fn releases(seen: &Arc<Mutex<Vec<String>>>) -> usize {
    seen.lock()
        .map(|lines| lines.iter().filter(|l| is_release(l)).count())
        .unwrap_or_default()
}

/// The status poll, then what it left.
fn poll(state: &AppState, label: &str) {
    let status = state.speleodb_link_status().map_err(|e| e.message);
    println!(
        "[SDB-POLL-D1] {label}: poll -> lock_holder {:?}; gate {}; claim {}; marked {}",
        status.as_ref().map(|s| s.lock_holder.clone()),
        state.edit_enabled(),
        state.speleodb_lock_acquired_here(REMOTE_ID),
        marked(state),
    );
    assert!(status.is_ok(), "the poll answers");
}

/// The quit's bounded silent release; answers how many releases the stub saw.
fn quit(state: &AppState, seen: &Arc<Mutex<Vec<String>>>, label: &str) -> usize {
    state.speleodb_release_held_lock_bounded(Duration::from_secs(5));
    let n = releases(seen);
    println!("[SDB-POLL-D1] {label}: quit -> release POSTs {n}");
    n
}

static OURS: Phases = Phases::new();

fn lost_unread_then_ours(line: &str) -> (u16, &'static str) {
    OURS.answer(line, HELD_BY_SAME_ACCOUNT_AT_BASE_BODY)
}

/// The acquire landed: the first poll finds our account holding the lock, records
/// the claim as it reopens the gate, and the quit hands the lock back. It used to
/// reopen the gate as an inherited lock with no claim, and the quit kept the lock.
#[test]
fn a_poll_that_finds_our_lock_records_the_claim_and_quit_releases_it() {
    let (_temp, state, seen, _listener) = start_lost_unread(lost_unread_then_ours);
    let was_marked = marked(&state);
    poll(&state, "ours");
    let (gate, claimed, taken, still_marked) = (
        state.edit_enabled(),
        state.speleodb_lock_acquired_here(REMOTE_ID),
        state.speleodb_lock_taken_here(REMOTE_ID),
        marked(&state),
    );
    let released = quit(&state, &seen, "ours");
    assert!(gate, "the gate reopens over our lock");
    assert!(claimed, "and the claim the lost reply cost is recorded");
    assert!(
        taken,
        "as this session's acquisition: the probe saw the lock free"
    );
    assert_eq!(released, 1, "the quit hands the lock back");
    assert!(
        was_marked && !still_marked,
        "the acquire was marked unconfirmed, and the poll settled it"
    );
}

static SECOND_START: Phases = Phases::new();

fn second_start_then_ours(line: &str) -> (u16, &'static str) {
    SECOND_START.answer(line, HELD_BY_SAME_ACCOUNT_AT_BASE_BODY)
}

/// A second Start Editing before any poll takes the lock as inherited (the probe
/// sees it ours) and opens the gate with an ADOPTED claim (#576 SDB-ACCT-D1); the poll
/// then finds our lock with the gate open, re-reads it under the lock-ops span, and
/// records the claim the lost reply cost: this session's acquisition.
#[test]
fn a_poll_after_a_second_start_editing_records_the_claim() {
    let (_temp, state, seen, _listener) = start_lost_unread(second_start_then_ours);
    let again = state
        .speleodb_start_editing()
        .map(|_| ())
        .map_err(|e| e.message);
    println!(
        "[SDB-POLL-D1] second start -> {again:?}; gate {}; claim {}",
        state.edit_enabled(),
        state.speleodb_lock_acquired_here(REMOTE_ID)
    );
    assert!(
        again.is_ok()
            && state.edit_enabled()
            && state.speleodb_lock_resumed_here(REMOTE_ID)
            && !state.speleodb_lock_taken_here(REMOTE_ID),
        "editing over an inherited lock, adopted (for the release only)"
    );
    poll(&state, "second start");
    let (claimed, still_marked) = (state.speleodb_lock_taken_here(REMOTE_ID), marked(&state));
    let released = quit(&state, &seen, "second start");
    assert!(claimed, "the poll records this session's acquisition");
    assert_eq!(released, 1, "the quit hands the lock back");
    assert!(!still_marked, "the mark is settled");
}

static OTHER: Phases = Phases::new();
static OTHER_LET_GO: AtomicBool = AtomicBool::new(false);

fn lost_unread_then_another(line: &str) -> (u16, &'static str) {
    let after = if OTHER_LET_GO.load(Ordering::SeqCst) {
        HELD_BY_SAME_ACCOUNT_AT_BASE_BODY
    } else {
        HELD_BY_ANOTHER_ACCOUNT_AT_BASE_BODY
    };
    OTHER.answer(line, after)
}

/// Control: the poll finds ANOTHER account holding the lock. The acquire did not
/// land (or the lock moved on): nothing is claimed and the mark is dropped, so a
/// later poll that finds the lock ours (taken elsewhere on this account) reopens the
/// gate over an ADOPTED lock (#576 SDB-ACCT-D1: a resumed claim, not this session's
/// acquisition), and the quit hands that lock back.
#[test]
fn a_poll_that_finds_another_holder_drops_the_mark() {
    let (_temp, state, seen, _listener) = start_lost_unread(lost_unread_then_another);
    poll(&state, "another holder");
    assert!(
        !state.edit_enabled() && !state.speleodb_lock_acquired_here(REMOTE_ID),
        "nothing opens, nothing is claimed"
    );
    assert!(!marked(&state), "no mark is left");
    // Had the mark survived the poll above, this one would record this session's
    // acquisition: the adopted kind below is what proves it was dropped.
    OTHER_LET_GO.store(true, Ordering::SeqCst);
    poll(&state, "another holder, then ours");
    assert!(
        state.edit_enabled()
            && state.speleodb_lock_resumed_here(REMOTE_ID)
            && !state.speleodb_lock_taken_here(REMOTE_ID),
        "inherited: the gate reopens over the adopted lock"
    );
    assert_eq!(
        quit(&state, &seen, "another holder"),
        1,
        "the quit hands the adopted lock back (#576 SDB-QUIT-D1)"
    );
}

static NONE: Phases = Phases::new();

fn lost_unread_then_free(line: &str) -> (u16, &'static str) {
    NONE.answer(line, FREE_MUTEX_AT_BASE_BODY)
}

/// The poll finds the lock free: the acquire did not land, and the mark is dropped.
#[test]
fn a_poll_that_finds_the_lock_free_drops_the_mark() {
    let (_temp, state, _seen, _listener) = start_lost_unread(lost_unread_then_free);
    assert!(marked(&state), "the acquire is marked unconfirmed");
    poll(&state, "free");
    assert!(
        !state.edit_enabled() && !state.speleodb_lock_acquired_here(REMOTE_ID),
        "nothing opens, nothing is claimed"
    );
    assert!(!marked(&state), "the mark is dropped");
}

static CLOSED: Phases = Phases::new();

fn lost_unread_then_ours_closed(line: &str) -> (u16, &'static str) {
    CLOSED.answer(line, HELD_BY_SAME_ACCOUNT_AT_BASE_BODY)
}

/// A close drops the mark.
#[test]
fn a_close_drops_the_mark() {
    let (_temp, state, _seen, _listener) = start_lost_unread(lost_unread_then_ours_closed);
    assert!(marked(&state), "the acquire is marked unconfirmed");
    let closed = state.execute(Command::CloseProject);
    println!(
        "[SDB-POLL-D1] close -> {}; marked {}",
        closed.success,
        marked(&state)
    );
    assert!(closed.success, "close");
    assert!(!marked(&state), "the mark is dropped");
}

static OURS_FIRST: Phases = Phases::new();

fn ours_then_lost_unread_then_ours(line: &str) -> (u16, &'static str) {
    OURS_FIRST.answer_from(
        line,
        HELD_BY_SAME_ACCOUNT_AT_BASE_BODY,
        HELD_BY_SAME_ACCOUNT_AT_BASE_BODY,
    )
}

/// [#576 SDB-ACCT-D1] The probe before the acquire saw our OWN account holding the
/// lock (another session of it, the website), and the acquire's reply and the re-read
/// after it were lost. The acquire is marked unconfirmed in this case too (W29 marked
/// only an acquire whose probe saw the lock not ours), as the adopted kind: the poll
/// settles it into an adopted claim - the release, not the authorship of a lock taken
/// here - and the quit hands the lock back.
#[test]
fn a_lost_acquire_over_the_accounts_own_lock_is_marked_too() {
    let (_temp, state, seen, _listener) = start_lost_unread(ours_then_lost_unread_then_ours);
    let mark = state
        .speleodb
        .lock()
        .unconfirmed_acquire
        .as_ref()
        .map(super::speleodb_session::LockClaim::is_resumed);
    println!("[SDB-ACCT-D1] probe saw our own lock: marked as adopted {mark:?}");
    assert_eq!(mark, Some(true), "marked, as an adopted lock");
    poll(&state, "probe saw ours");
    let (gate, adopted, taken, still_marked) = (
        state.edit_enabled(),
        state.speleodb_lock_resumed_here(REMOTE_ID),
        state.speleodb_lock_taken_here(REMOTE_ID),
        marked(&state),
    );
    let released = quit(&state, &seen, "probe saw ours");
    assert!(
        gate && adopted && !taken,
        "the poll adopts the lock, no authorship"
    );
    assert!(!still_marked, "the mark is settled");
    assert_eq!(released, 1, "the quit hands the lock back");
}
