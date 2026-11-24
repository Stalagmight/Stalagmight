//! The four SpeleoDB sync fixes against each other (plan
//! `2026-11-21_speleodb-sync-fixes.md`, "How the fixes meet"): each one is
//! pinned by its own suite, and these are the paths where two of them decide
//! the same outcome.
//!
//! - F1 x F2: an open whose reads get no reply opens the cached copy offline;
//!   a copy whose un-uploaded mark is stale (an edit lost in a crash) opens
//!   there as the clean copy it is, and a saved edit still opens marked.
//! - F2 x the #193 recovery that R3 narrowed: an upload whose answer is lost
//!   may have landed, so until one is confirmed the copy's BASE digest no
//!   longer says the server holds nothing of it beyond BASE. An edit taken
//!   back after such an upload is still uploaded, on top of our own revision.
//! - F2 x R3: a stale mark is no conflict when another session of the account
//!   pushed meanwhile: the copy is behind, nothing is uploaded, nothing refused.
//! - F4 x R3 on the exit and sign-out paths: an exit upload refused because
//!   another session's revision is HEAD keeps the lock through the quit's
//!   release and the sign-out; one that landed over a revision pushed meanwhile
//!   hands the lock back once and leaves no claim behind.
//!
//! All data is synthetic: the survey is built by the encoder, the server is a
//! loopback listener that keeps a real history, a real lock holder, and can
//! lose its replies on purpose.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::speleodb_cache::LocalEditsPolicy;
use super::speleodb_local_state::{cache_path_in, TestCacheRoot};
use super::stalx_fixtures::{decoded, extras_for, line_survey, link_on_disk};
use super::AppState;
use crate::core::Command;
use crate::models::{SpeleoLink, UndoMode};
use crate::persistence::stalx_v2::{self, Mode, PushOptions};
use crate::speleodb::auth::TokenStore;
use crate::speleodb::{
    DoneResult, PushResult, RemoteState, SpeleoErrorDto, SpeleoOpenResult, SpeleoReadOnlyOpen,
};

const ID: &str = "11111111-1111-1111-1111-111111111111";
const ME: &str = "diver@example.com";
/// The revision the copy is downloaded at.
const BASE: &str = "abc1234";
/// The name on the server and in every survey here, so the pre-push name fold
/// (#216) never renames the copy.
const NAME: &str = "Remote";

/// One revision on the loopback server.
#[derive(Clone, Debug)]
struct Rev {
    id: String,
    parent: Option<String>,
    message: String,
    file: Vec<u8>,
}

/// How the server answers the next upload.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Next {
    /// Land it and answer 200 with its revision.
    #[default]
    Accept,
    /// Land it, then close the connection without an answer: the upload is on
    /// the server and the client cannot know.
    LandSilently,
}

/// The server's state, shared with the test.
#[derive(Default)]
struct Remote {
    /// Oldest first.
    history: Vec<Rev>,
    holder: Option<String>,
    uploads: usize,
    lines: Vec<String>,
    next: Next,
    /// Lands just before the next accepted upload does: another session's
    /// revision, pushed while that upload was on its way (R3, the lost race).
    meanwhile: Option<(String, Vec<u8>)>,
    /// Every GET under `/api/v2/projects/` is closed without a reply (F1).
    lose_reads: bool,
    minted: u32,
}

impl Remote {
    fn tip(&self) -> Option<String> {
        self.history.last().map(|r| r.id.clone())
    }

    fn tip_rev(&self) -> Rev {
        self.history.last().cloned().expect("a history")
    }

    /// Land a revision on the tip, as any session of the account would.
    fn land(&mut self, message: &str, file: Vec<u8>) -> String {
        self.minted += 1;
        let id = format!("fed{:04}", self.minted);
        let parent = self.tip();
        self.history.push(Rev {
            id: id.clone(),
            parent,
            message: message.to_owned(),
            file,
        });
        id
    }

    fn releases(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| l.starts_with("POST") && l.contains("/release/"))
            .count()
    }

    fn commits_json(&self) -> String {
        let commits: Vec<_> = self
            .history
            .iter()
            .rev()
            .map(|r| {
                serde_json::json!({
                    "id": r.id,
                    "parent_ids": r.parent.iter().collect::<Vec<_>>(),
                    "author_email": ME,
                    "message": r.message,
                })
            })
            .collect();
        serde_json::json!({ "commits": commits }).to_string()
    }
}

type Shared = Arc<Mutex<Remote>>;

/// The upload's description: the multipart `message` part.
fn upload_message(request: &[u8]) -> String {
    String::from_utf8_lossy(request)
        .split("name=\"message\"")
        .nth(1)
        .and_then(|part| part.split_once("\r\n\r\n"))
        .and_then(|(_, rest)| rest.split_once("\r\n--"))
        .map(|(message, _)| message.to_owned())
        .unwrap_or_default()
}

/// The `.stalx` an upload carries: its file part, from the first member to
/// the end marker.
fn upload_file(request: &[u8]) -> Vec<u8> {
    let find = |needle: &[u8]| request.windows(needle.len()).position(|w| w == needle);
    let start = find(b"{\"format\":\"stalx\"").unwrap_or(0);
    let marker = b"\"end\":true}\n";
    let end = find(marker).map_or(request.len(), |at| at + marker.len());
    request.get(start..end).unwrap_or_default().to_vec()
}

/// A loopback SpeleoDB holding [`BASE`] (`base_file`), whose lock [`ME`] holds.
fn server(base_file: impl FnOnce(&str) -> Vec<u8>) -> (String, Shared) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let endpoint = format!(
        "http://127.0.0.1:{}",
        listener.local_addr().expect("addr").port()
    );
    let remote = Arc::new(Mutex::new(Remote {
        history: vec![Rev {
            id: BASE.to_owned(),
            parent: None,
            message: "First survey".to_owned(),
            file: base_file(&endpoint),
        }],
        holder: Some(ME.to_owned()),
        ..Remote::default()
    }));
    let shared = Arc::clone(&remote);
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let request = read_request(&mut stream);
            let head = String::from_utf8_lossy(&request);
            let line = head.lines().next().unwrap_or_default().to_owned();
            let Ok(mut remote) = shared.lock() else {
                return;
            };
            remote.lines.push(line.clone());
            if remote.lose_reads && line.starts_with("GET /api/v2/projects/") {
                // Read, then closed without a word (the F1 trace).
                continue;
            }
            let mutex = remote
                .holder
                .as_ref()
                .map_or_else(|| "null".to_owned(), |h| format!(r#"{{"user":"{h}"}}"#));
            let project = super::tests_support::sdk_fixture(&format!(
                r#"{{"id":"{ID}","name":"{NAME}","permission":"READ_AND_WRITE","active_mutex":{mutex}}}"#
            ));
            let json = |body: String| (200, "application/json", body.into_bytes());
            let (status, kind, body) = if line.contains("/user/") {
                json(super::tests_support::sdk_fixture(&format!(
                    r#"{{"email":"{ME}"}}"#
                )))
            } else if line.contains("/revisions/") {
                json(super::tests_support::sdk_fixture(&remote.commits_json()))
            } else if line.contains("/download/") {
                let named = remote
                    .history
                    .iter()
                    .find(|r| line.contains(r.id.as_str()))
                    .map(|r| r.file.clone());
                let file = named.unwrap_or_else(|| remote.tip_rev().file);
                (200, "application/octet-stream", file)
            } else if line.contains("/upload/") {
                remote.uploads += 1;
                if let Some((message, file)) = remote.meanwhile.take() {
                    remote.land(&message, file);
                }
                let landed = remote.land(&upload_message(&request), upload_file(&request));
                if std::mem::take(&mut remote.next) == Next::LandSilently {
                    continue;
                }
                json(format!(
                    r#"{{"project":{project},"hexsha":"{landed}","files":["project.stalx"],"message":"Saved","browser_url":null}}"#
                ))
            } else if line.contains("/release/") {
                remote.holder = None;
                json(super::tests_support::sdk_fixture(&format!(
                    r#"{{"id":"{ID}","name":"{NAME}","permission":"READ_AND_WRITE"}}"#
                )))
            } else if line.contains("/acquire/") {
                remote.holder = Some(ME.to_owned());
                json(project)
            } else {
                json(project)
            };
            drop(remote);
            let reply = format!(
                "HTTP/1.1 {status} Test\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
        }
    });
    (endpoint, remote)
}

/// One whole request (headers and the `Content-Length` body).
fn read_request(stream: &mut std::net::TcpStream) -> Vec<u8> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut request = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    while let Ok(count) = stream.read(&mut chunk) {
        if count == 0 {
            break;
        }
        request.extend_from_slice(chunk.get(..count).unwrap_or_default());
        if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
            let length = headers
                .lines()
                .find_map(|l| {
                    l.strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if request.len() >= end + 4 + length {
                break;
            }
        }
    }
    request
}

/// A revision as an upload carries it: [`line_survey`], its entrance elevation
/// `elevation`.
fn revision(endpoint: &str, elevation: Option<f64>) -> Vec<u8> {
    let mut survey = line_survey(NAME);
    survey.settings.surface_elevation = elevation;
    let link = SpeleoLink {
        endpoint: endpoint.to_owned(),
        project_id: ID.to_owned(),
        project_name: NAME.to_owned(),
        base_commit_sha: None,
        has_unpushed_edits: false,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: Some(UndoMode::Full),
    };
    stalx_v2::encode(
        &survey,
        &extras_for(Some(&link)),
        Mode::ForPush(PushOptions::default()),
    )
    .expect("encode a revision")
}

/// The entrance elevation a revision's survey holds: what tells the edit, the
/// edit taken back and the other session's revision apart.
fn elevation_of(file: &[u8]) -> Option<f64> {
    stalx_v2::decode(file, &stalx_v2::DecodeOptions::default())
        .expect("a revision")
        .project
        .settings
        .surface_elevation
}

/// A server holding [`BASE`], and the path of the copy under `root`.
fn setup(root: &Path) -> (String, Shared, PathBuf) {
    let (endpoint, remote) = server(|endpoint| revision(endpoint, None));
    (endpoint, remote, cache_path_in(root, ID))
}

/// A state signed in as [`ME`] on `endpoint`.
fn signed_in(endpoint: &str) -> AppState {
    let state = AppState::new().expect("state");
    let mut session = state.speleodb.lock().expect("session");
    session.endpoint = Some(endpoint.to_owned());
    session.email = Some(ME.to_owned());
    session.token_store = TokenStore::from_in_memory_token("tok-1");
    session.rehydrate_attempted = true;
    drop(session);
    state
}

/// The pull the SpeleoDB browser's Open runs.
#[expect(
    clippy::result_large_err,
    reason = "the pull's own error type, returned as the pull returns it"
)]
fn pull(state: &AppState, dest: &Path) -> Result<SpeleoOpenResult, SpeleoErrorDto> {
    state.speleodb_pull_project_to(
        dest,
        ID.to_owned(),
        NAME.to_owned(),
        None,
        LocalEditsPolicy::Protect,
    )
}

/// Editing resumed on the open copy: the gate open and the lock claimed, as
/// Start editing leaves it.
fn resume_editing(state: &AppState) {
    state.set_edit_enabled(true);
    state.speleodb_note_lock_acquired(Some(ID));
}

/// Download the copy, open it and hold the lock, as Open then Start editing
/// leave it.
fn open_for_editing(state: &AppState, dest: &Path) {
    pull(state, dest).expect("the first pull downloads");
    assert!(
        state.open_project(dest.to_path_buf()).success,
        "open the copy"
    );
    resume_editing(state);
}

fn set_elevation(state: &AppState, elevation: Option<f64>) {
    let edited = state.set_surface_elevation(elevation);
    assert!(edited.success, "edit: {:?}", edited.error);
}

fn save(state: &AppState) -> crate::core::CommandResult {
    let saved = state.execute(Command::SaveProject { path: None });
    assert!(saved.success, "save: {:?}", saved.error);
    saved
}

fn open_link(state: &AppState) -> SpeleoLink {
    state
        .speleodb_open_link()
        .expect("an open SpeleoDB project")
}

/// The mark as the file at `path` holds it, read raw (every link reader reads
/// it through the content rule).
fn raw_mark(path: &Path) -> bool {
    stalx_v2::probe_path(path)
        .expect("probe")
        .local
        .is_some_and(|local| local.speleodb.unpushed_edits)
}

/// Whether the file at `path` records a survey digest for its BASE.
fn has_digest(path: &Path) -> bool {
    stalx_v2::probe_path(path)
        .expect("probe")
        .local
        .is_some_and(|local| local.speleodb.base_survey.is_some())
}

/// F1 x F2. The read fallback opens the copy as the shared probe reads it, so
/// the content rule applies offline too: an edit lost in a crash leaves a copy
/// that opens clean (its own or another account's - there is no work of
/// anyone's in it), while a saved edit opens still marked as not uploaded, and
/// another account's saved edit read-only. The offline open writes nothing; the
/// open of the file then clears the stale mark, as any open does.
///
/// Injection: `probe_settled` returning the header as read makes the lost
/// edit's offline open report `has_unpushed_edits: Some(true)` (and read-only
/// for the other account).
#[test]
fn an_offline_open_decides_un_uploaded_work_by_content() {
    let other = "someone@example.com";
    for (saved, owner) in [(false, ME), (false, other), (true, ME), (true, other)] {
        let case = format!("saved: {saved}, owner: {owner}");
        crate::test_helpers::with_isolated_config(|| {
            let temp = tempfile::tempdir().expect("temp");
            let _cache = TestCacheRoot::set(temp.path());
            let (endpoint, remote, dest) = setup(temp.path());
            let state = signed_in(&endpoint);
            open_for_editing(&state, &dest);
            set_elevation(&state, Some(12.5));
            if saved {
                save(&state);
            }
            assert!(raw_mark(&dest), "{case}: the mark is durable at the edit");
            // The crash (an unsaved edit is lost), and whose copy it is.
            drop(state);
            let state = signed_in(&endpoint);
            state.speleodb_record_cache_owner(&dest, &endpoint, owner, NAME);

            remote.lock().expect("remote").lose_reads = true;
            let before = std::fs::read(&dest).expect("copy");
            let opened =
                pull(&state, &dest).unwrap_or_else(|e| panic!("{case}: opens offline, got {e:?}"));
            assert_eq!(opened.offline, Some(true), "{case}");
            assert_eq!(
                opened.has_unpushed_edits,
                Some(saved),
                "{case}: un-uploaded work is decided by what the copy holds"
            );
            let theirs = saved && owner != ME;
            assert_eq!(
                opened.read_only,
                theirs.then_some(SpeleoReadOnlyOpen::OtherAccount),
                "{case}"
            );
            assert_eq!(opened.other_owner.is_some(), theirs, "{case}");
            assert_eq!(
                std::fs::read(&dest).expect("copy"),
                before,
                "{case}: the offline open writes nothing"
            );
            assert!(
                remote
                    .lock()
                    .expect("remote")
                    .lines
                    .iter()
                    .any(|l| l.contains("/download/")),
                "{case}: the download was tried"
            );

            assert!(state.open_project(dest.clone()).success, "{case}");
            assert_eq!(open_link(&state).has_unpushed_edits, saved, "{case}");
            assert_eq!(
                raw_mark(&dest),
                saved,
                "{case}: the open clears a stale mark and keeps a real one"
            );
        });
    }
}

/// F2 x the #193 recovery. An upload whose answer is lost may have landed (here
/// it did): the server's HEAD may be our own revision with the edit. Taking the
/// edit back and saving then leaves a copy whose survey is BASE's but not the
/// server's, so it is not clean: the digest went with the upload, the mark
/// stays, and Done uploads the take-back on top of our own revision. With the
/// digest kept, the save cleared the mark and Done released without uploading,
/// leaving the edit the user took back as SpeleoDB's latest revision.
///
/// Injection: keep the digest when the upload is sent (drop the clear in
/// `speleodb_upload`) and the save reports the mark cleared and Done uploads
/// nothing.
#[test]
fn an_edit_taken_back_after_an_unconfirmed_upload_is_still_uploaded() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, remote, dest) = setup(temp.path());
        let state = signed_in(&endpoint);
        open_for_editing(&state, &dest);
        assert!(
            has_digest(&dest),
            "fixture: the install recorded BASE's digest"
        );
        set_elevation(&state, Some(12.5));

        remote.lock().expect("remote").next = Next::LandSilently;
        let pushed = state
            .speleodb_push_only(Some("Synthetic push".to_owned()))
            .expect("a lost answer is no error");
        assert!(
            matches!(pushed, PushResult::PushUnconfirmed { .. }),
            "{pushed:?}"
        );
        let first = {
            let remote = remote.lock().expect("remote");
            assert_eq!(remote.uploads, 1);
            assert_eq!(
                elevation_of(&remote.tip_rev().file),
                Some(12.5),
                "it landed"
            );
            remote.tip()
        };
        assert!(
            !has_digest(&dest),
            "the digest went with the upload, on disk too (a crash keeps it gone)"
        );

        assert!(state.undo().success, "take the edit back");
        let saved = save(&state);
        assert_eq!(
            saved.speleodb_unpushed_cleared, None,
            "the survey is BASE's, but SpeleoDB may hold our edit"
        );
        assert!(open_link(&state).has_unpushed_edits);
        assert!(link_on_disk(&dest).expect("linked").has_unpushed_edits);

        let done = state
            .speleodb_done(Some("Taken back".to_owned()))
            .expect("Done answers");
        assert!(matches!(done, DoneResult::Released { .. }), "{done:?}");
        let remote = remote.lock().expect("remote");
        assert_eq!(remote.uploads, 2, "the take-back was uploaded");
        let top = remote.tip_rev();
        assert_eq!(top.parent, first, "on top of our own landed upload (#193)");
        assert_eq!(
            elevation_of(&top.file),
            None,
            "SpeleoDB's latest revision is the survey the user kept"
        );
        assert!(remote.holder.is_none(), "and the lock is back");
        drop(remote);
        let on_disk = link_on_disk(&dest).expect("linked");
        assert!(!on_disk.has_unpushed_edits);
        assert_eq!(on_disk.base_commit_sha.as_deref(), Some(top.id.as_str()));
        assert!(
            has_digest(&dest),
            "the confirmed upload records the new digest"
        );
    });
}

/// F2 x R3. A copy whose mark is stale (an edit lost in a crash) holds exactly
/// its BASE, so another session of the account pushing meanwhile leaves it
/// behind, not in conflict: the status says an update is waiting, Save Changes
/// and Done upload nothing and refuse nothing, and the lock is handed back.
///
/// Injection: the open's clear off (`settle_on_open` returning `false`) and the
/// status reads Conflict, Save Changes `DivergenceNeeded`.
#[test]
fn a_stale_mark_is_no_conflict_when_another_session_of_the_account_pushed() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, remote, dest) = setup(temp.path());
        let state = signed_in(&endpoint);
        open_for_editing(&state, &dest);
        set_elevation(&state, Some(12.5));
        drop(state);

        let theirs = {
            let mut remote = remote.lock().expect("remote");
            remote.land("From the other session", revision(&endpoint, Some(3.0)))
        };
        let state = signed_in(&endpoint);
        assert!(state.open_project(dest).success, "reopened from Recents");
        resume_editing(&state);
        assert!(!open_link(&state).has_unpushed_edits);

        let status = state.speleodb_link_status().expect("status");
        assert_eq!(status.remote_state, RemoteState::RemoteAhead, "{status:?}");
        let pushed = state
            .speleodb_push_only(Some("Nothing to upload".to_owned()))
            .expect("Save Changes answers");
        assert!(
            matches!(
                pushed,
                PushResult::Pushed {
                    replaced_revisions: None,
                    ..
                }
            ),
            "{pushed:?}"
        );
        let done = state
            .speleodb_done(Some("Nothing to upload".to_owned()))
            .expect("Done answers");
        assert!(matches!(done, DoneResult::Released { .. }), "{done:?}");
        let remote = remote.lock().expect("remote");
        assert_eq!(remote.uploads, 0, "nothing uploaded");
        assert_eq!(remote.tip().as_deref(), Some(theirs.as_str()));
        assert!(remote.holder.is_none(), "the lock is back");
    });
}

/// A sink that gives a release thread this process's managed state, as the app
/// does: without one a deferred release has no claims to consult, and skips
/// the reclaim check (F4) this file means to cross.
struct ManagedSink(&'static AppState);

impl crate::core::events::EventSink for ManagedSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}

    fn with_app_state(&self, f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        f(self.0);
        true
    }
}

/// A state the release threads can reach through the sink, editing the copy.
fn leaked_editing(endpoint: &str, dest: &Path) -> &'static AppState {
    let state: &'static AppState = Box::leak(Box::new(signed_in(endpoint)));
    state.set_event_sink(Arc::new(ManagedSink(state)));
    open_for_editing(state, dest);
    state
}

/// F4 x R3 on the exit and sign-out paths. Another session of the account
/// pushed, so the exit's upload (Done, which the close, quit and sign-out
/// prompts run) is refused and uploads nothing. The quit's release and the
/// sign-out that follow it must not hand the lock back over the work that is
/// still only here: no release is sent, the lock stays the account's, the
/// claim is kept on purpose and the copy stays marked at its BASE.
///
/// Injection: drop the un-pushed-edits veto from
/// `speleodb_lock_release_decision` and the quit's release frees the lock.
#[test]
fn an_exit_upload_refused_for_another_sessions_revision_keeps_the_lock() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, remote, dest) = setup(temp.path());
        let state = leaked_editing(&endpoint, &dest);
        set_elevation(state, Some(12.5));
        save(state);
        remote
            .lock()
            .expect("remote")
            .land("From the other session", revision(&endpoint, Some(3.0)));

        let done = state
            .speleodb_done(Some("From this session".to_owned()))
            .expect("Done answers");
        assert!(matches!(done, DoneResult::DivergenceNeeded), "{done:?}");
        state.speleodb_release_held_lock_bounded(Duration::from_secs(5));
        assert!(
            state.speleodb_claim_is_kept(ID),
            "the quit's veto keeps the claim on purpose"
        );
        state.speleodb_logout().expect("sign out");

        let remote = remote.lock().expect("remote");
        assert_eq!(remote.uploads, 0, "nothing of this session's was uploaded");
        assert_eq!(remote.releases(), 0, "no release was sent");
        assert_eq!(remote.holder.as_deref(), Some(ME), "the lock is still held");
        drop(remote);
        let on_disk = link_on_disk(&dest).expect("linked");
        assert!(on_disk.has_unpushed_edits, "the work is still un-uploaded");
        assert_eq!(on_disk.base_commit_sha.as_deref(), Some(BASE));
        assert_eq!(
            elevation_of(&std::fs::read(&dest).expect("copy")),
            Some(12.5),
            "and still here"
        );
    });
}

/// F4 x R3, the other arm: the exit's upload landed on top of a revision
/// another session pushed between the check and the upload. It is reported by
/// name, and the lock was handed back by Done itself, so the quit and the
/// sign-out that follow send nothing more and no claim is left for the
/// stranded-claim cleanup to hand back again.
///
/// Injection: keep the claim after Done's release (drop its
/// `speleodb_forget_lock_acquired_for`) and the claim outlives the lock.
#[test]
fn an_exit_upload_that_replaced_a_revision_hands_the_lock_back_once() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, remote, dest) = setup(temp.path());
        let state = leaked_editing(&endpoint, &dest);
        set_elevation(state, Some(12.5));
        remote.lock().expect("remote").meanwhile = Some((
            "From the other session, meanwhile".to_owned(),
            revision(&endpoint, Some(3.0)),
        ));

        let done = state
            .speleodb_done(Some("From this session".to_owned()))
            .expect("Done answers");
        let DoneResult::Released {
            replaced_revisions: Some(replaced),
            ..
        } = done
        else {
            panic!("Done names what its upload replaced: {done:?}");
        };
        assert_eq!(
            replaced
                .iter()
                .map(|r| r.message.as_deref())
                .collect::<Vec<_>>(),
            [Some("From the other session, meanwhile")]
        );
        assert!(!state.speleodb_holds_claim(ID), "the claim ended with Done");
        state.speleodb_release_held_lock_bounded(Duration::from_secs(5));
        state.speleodb_logout().expect("sign out");

        let remote = remote.lock().expect("remote");
        assert_eq!(remote.uploads, 1);
        assert_eq!(
            remote.releases(),
            1,
            "the lock was handed back once, by Done"
        );
        assert!(remote.holder.is_none());
        assert_eq!(elevation_of(&remote.tip_rev().file), Some(12.5));
        drop(remote);
        assert!(!link_on_disk(&dest).expect("linked").has_unpushed_edits);
        assert_eq!(
            decoded(&dest).project.settings.surface_elevation,
            Some(12.5)
        );
    });
}
