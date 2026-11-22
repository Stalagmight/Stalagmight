//! S2 (F2-phantom-unpushed), end to end against a loopback SpeleoDB: the
//! un-uploaded mark is decided by the survey a copy holds, not by the bit
//! alone.
//!
//! The protocol fuzzer's trace: an edit of an open SpeleoDB copy makes the
//! mark durable at once (a local-only save), the edit itself only the next
//! save; a crash in between leaves the mark over a file that holds exactly its
//! BASE revision, and the next open refused "this project has local edits
//! that were never pushed". 0.6.0 did the same (one UPDATE per edit).
//!
//! Each test drives the real verbs (the pull, the open, an edit, the save, the
//! push) and checks every reader that asks "does this copy have un-uploaded
//! work": the open link, the file on disk, the local-state listing the
//! sign-out and held-lock prompts read, and the replace guard of the pull.
//!
//! All data is synthetic: the files are built by the encoder and the legacy
//! fixture kit, the server is a loopback listener.

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::speleodb_cache::LocalEditsPolicy;
use super::speleodb_local_state::{cache_path_in, TestCacheRoot};
use super::stalx_fixtures::{
    decoded, extras_for, legacy_linked_project, line_survey, link_on_disk,
};
use super::AppState;
use crate::core::Command;
use crate::models::{SpeleoLink, UndoMode};
use crate::persistence::stalx_v2::{self, Mode, PushOptions};
use crate::speleodb::auth::TokenStore;
use crate::speleodb::{PushResult, SpeleoDbAction, StartEditingResult};

const ID: &str = "11111111-1111-1111-1111-111111111111";
const ME: &str = "diver@example.com";
/// The revision the copy is downloaded at.
const BASE: &str = "abc1234";
/// Another session's push on top of it.
const NEWER: &str = "bcd2345";
/// What this machine's upload lands as.
const PUSHED: &str = "cde3456";
/// The name on the server and in every survey here: the pre-push name fold
/// (#216) then never renames the copy.
const NAME: &str = "Remote";

/// The loopback server's history and files.
#[derive(Default)]
struct Repo {
    /// Newest first: (id, parent).
    commits: Vec<(String, Option<String>)>,
    files: HashMap<String, Vec<u8>>,
    lines: Vec<String>,
    uploads: usize,
    /// Answer uploads with 304 (the server already holds what went up), landing
    /// nothing.
    upload_304: bool,
}

impl Repo {
    fn push(&mut self, id: &str, bytes: Vec<u8>) {
        let parent = self.commits.first().map(|(c, _)| c.clone());
        self.commits.insert(0, (id.to_owned(), parent));
        self.files.insert(id.to_owned(), bytes);
    }
}

/// Run while the server holds an upload, before it answers (an edit made while
/// the upload is in flight).
type DuringUpload = Box<dyn Fn() + Send>;

/// A loopback SpeleoDB whose lock [`ME`] holds: the history and the files are
/// `repo`'s, a download answers the revision its request names (else the tip),
/// and an accepted upload lands as [`PUSHED`] after running `during_upload`.
fn server(repo: Arc<Mutex<Repo>>, during_upload: Option<DuringUpload>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let request = read_request(&mut stream);
            let line = request.lines().next().unwrap_or_default().to_owned();
            let project = super::tests_support::sdk_fixture(&format!(
                r#"{{"id":"{ID}","name":"{NAME}","permission":"READ_AND_WRITE","active_mutex":{{"user":"{ME}"}}}}"#
            ));
            let json = |body: String| (200, "application/json", body.into_bytes());
            let (status, kind, body) = if line.contains("/user/") {
                json(super::tests_support::sdk_fixture(&format!(
                    r#"{{"email":"{ME}"}}"#
                )))
            } else if line.contains("/revisions/") {
                let repo = repo.lock().expect("repo");
                let commits: Vec<String> = repo
                    .commits
                    .iter()
                    .map(|(id, parent)| {
                        let parents = parent
                            .as_ref()
                            .map(|p| format!("\"{p}\""))
                            .unwrap_or_default();
                        format!(r#"{{"id":"{id}","parent_ids":[{parents}],"author_email":"{ME}"}}"#)
                    })
                    .collect();
                json(super::tests_support::sdk_fixture(&format!(
                    r#"{{"commits":[{}]}}"#,
                    commits.join(",")
                )))
            } else if line.contains("/download/") {
                let repo = repo.lock().expect("repo");
                let named = repo
                    .files
                    .iter()
                    .find(|(id, _)| line.contains(id.as_str()))
                    .map(|(_, bytes)| bytes.clone());
                let tip = repo
                    .commits
                    .first()
                    .and_then(|(id, _)| repo.files.get(id).cloned());
                (
                    200,
                    "application/octet-stream",
                    named.or(tip).unwrap_or_default(),
                )
            } else if line.contains("/upload/") && repo.lock().expect("repo").upload_304 {
                if let Some(during) = &during_upload {
                    during();
                }
                repo.lock().expect("repo").uploads += 1;
                (304, "application/json", Vec::new())
            } else if line.contains("/upload/") {
                if let Some(during) = &during_upload {
                    during();
                }
                let start = request.find("{\"format\":\"stalx\"").unwrap_or(0);
                let end = request
                    .find("\"end\":true}\n")
                    .map_or(request.len(), |at| at + "\"end\":true}\n".len());
                let mut repo = repo.lock().expect("repo");
                repo.uploads += 1;
                repo.push(PUSHED, request.as_bytes()[start..end].to_vec());
                json(format!(
                    r#"{{"project":{project},"hexsha":"{PUSHED}","files":["project.stalx"],"message":"Saved","browser_url":null}}"#
                ))
            } else {
                json(project)
            };
            if let Ok(mut repo) = repo.lock() {
                repo.lines.push(line);
            }
            let head = format!(
                "HTTP/1.1 {status} Test\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
        }
    });
    format!("http://127.0.0.1:{port}")
}

/// One whole request (headers and the `Content-Length` body).
fn read_request(stream: &mut std::net::TcpStream) -> String {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
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
    String::from_utf8_lossy(&request).into_owned()
}

/// The link a revision on `endpoint` carries as the server holds it.
fn server_link(endpoint: &str) -> SpeleoLink {
    SpeleoLink {
        endpoint: endpoint.to_owned(),
        project_id: ID.to_owned(),
        project_name: NAME.to_owned(),
        base_commit_sha: None,
        has_unpushed_edits: false,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: Some(UndoMode::Full),
    }
}

/// A revision as an upload carries it: [`line_survey`], its entrance elevation
/// `elevation`.
fn revision(endpoint: &str, elevation: Option<f64>) -> Vec<u8> {
    let mut survey = line_survey(NAME);
    survey.settings.surface_elevation = elevation;
    stalx_v2::encode(
        &survey,
        &extras_for(Some(&server_link(endpoint))),
        Mode::ForPush(PushOptions::default()),
    )
    .expect("encode a revision")
}

/// A server holding [`BASE`] and a cache root; returns the endpoint, the repo
/// and the copy's path.
fn setup(root: &Path, during_upload: Option<DuringUpload>) -> (String, Arc<Mutex<Repo>>, PathBuf) {
    let repo = Arc::new(Mutex::new(Repo::default()));
    let endpoint = server(Arc::clone(&repo), during_upload);
    repo.lock()
        .expect("repo")
        .push(BASE, revision(&endpoint, None));
    (endpoint, repo, cache_path_in(root, ID))
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
fn pull(
    state: &AppState,
    dest: &Path,
) -> Result<crate::speleodb::SpeleoOpenResult, crate::speleodb::SpeleoErrorDto> {
    state.speleodb_pull_project_to(
        dest,
        ID.to_owned(),
        NAME.to_owned(),
        None,
        LocalEditsPolicy::Protect,
    )
}

/// Download the copy, open it and hold the lock, as Open then Start editing
/// leave it.
fn open_for_editing(endpoint: &str, dest: &Path) -> AppState {
    let state = signed_in(endpoint);
    pull(&state, dest).expect("the first pull downloads");
    assert!(
        state.open_project(dest.to_path_buf()).success,
        "open the copy"
    );
    state.set_edit_enabled(true);
    state.speleodb_note_lock_acquired(Some(ID));
    state
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

/// What the listing the sign-out and held-lock prompts read says about [`ID`].
fn listed_unpushed(state: &AppState) -> bool {
    state
        .speleodb_local_project_states(Some(vec![ID.to_owned()]))
        .get(ID)
        .is_some_and(|row| row.has_unpushed_edits || row.state_unknown)
}

/// The survey the copy holds, with the edit time left out (an edit and its
/// edit back change it and nothing else).
fn survey_at(path: &Path) -> crate::models::survey::Project {
    let mut project = decoded(path).project;
    project.modified_at = project.created_at;
    project
}

/// The survey of the revision `sha` on the server, the edit time left out.
fn downloaded_survey(sha: &str, repo: &Mutex<Repo>) -> crate::models::survey::Project {
    let bytes = repo.lock().expect("repo").files[sha].clone();
    let mut project = stalx_v2::decode(&bytes, &stalx_v2::DecodeOptions::default())
        .expect("a revision")
        .project;
    project.modified_at = project.created_at;
    project
}

/// The fuzzer's trace (seed 900426): Edit, a crash that loses the unsaved
/// edit, another session pushes, and the next Open of the copy refused over
/// "un-uploaded" work although the copy holds exactly its BASE.
///
/// Injection: drop the content rule from the link probe
/// (`speleodb_base_survey::probe_settled` returning the header as read), or
/// record no digest at the install (`PreparedDownload::cache_bytes`), and the
/// listing reports un-uploaded work and the open refuses with
/// `UnpushedLocalEdits` again.
#[test]
fn an_edit_lost_in_a_crash_leaves_no_un_uploaded_work_behind() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, repo, dest) = setup(temp.path(), None);
        let state = open_for_editing(&endpoint, &dest);
        set_elevation(&state, Some(12.5));
        assert!(
            raw_mark(&dest),
            "the mark is durable at the edit (a crash after an edit still knows)"
        );
        assert!(listed_unpushed(&state), "and the open session says so");
        // The crash: the edit was never saved.
        drop(state);
        assert_eq!(
            survey_at(&dest),
            downloaded_survey(BASE, &repo),
            "the copy holds exactly its BASE"
        );

        // Another session pushes; this machine opens the project again.
        repo.lock()
            .expect("repo")
            .push(NEWER, revision(&endpoint, Some(3.0)));
        let state = signed_in(&endpoint);
        assert!(
            !listed_unpushed(&state),
            "the sign-out and held-lock prompts see no un-uploaded work"
        );
        let opened = pull(&state, &dest)
            .unwrap_or_else(|e| panic!("the open refused ({:?}): {}", e.action, e.message));
        assert_ne!(opened.has_unpushed_edits, Some(true));
        let on_disk = link_on_disk(&dest).expect("linked");
        assert_eq!(on_disk.base_commit_sha.as_deref(), Some(NEWER), "updated");
        assert!(!on_disk.has_unpushed_edits);
        assert_eq!(survey_at(&dest), downloaded_survey(NEWER, &repo));
    });
}

/// The same crash with nobody pushing since. Reopened from Recents, the copy
/// opens clean and the stale mark is cleared on disk; opened from the SpeleoDB
/// browser, the pull reports no un-uploaded work (the fast-forward arm, which
/// opens a marked copy on the tip as it is, does not take it).
///
/// Injection: drop the open's clear (`settle_on_open` returning `false`) and the
/// reopened link is marked; drop the probe's rule and the pull reports
/// `has_unpushed_edits: Some(true)` from the fast-forward arm.
#[test]
fn a_crash_after_an_edit_reopens_clean() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, repo, dest) = setup(temp.path(), None);
        let state = open_for_editing(&endpoint, &dest);
        set_elevation(&state, Some(12.5));
        drop(state);

        // Recents: the file opened as it is.
        let state = signed_in(&endpoint);
        assert!(!listed_unpushed(&state));
        assert!(state.open_project(dest.clone()).success);
        assert!(!open_link(&state).has_unpushed_edits, "opens clean");
        assert!(!state.has_unsaved_changes());
        assert!(!raw_mark(&dest), "and the stale mark is cleared on disk");
        assert_eq!(survey_at(&dest), downloaded_survey(BASE, &repo));
        drop(state);

        // The SpeleoDB browser, on a copy whose stale mark is still on disk.
        let state = open_for_editing(&endpoint, &dest);
        set_elevation(&state, Some(12.5));
        drop(state);
        let state = signed_in(&endpoint);
        let opened = pull(&state, &dest).expect("open");
        assert_ne!(opened.has_unpushed_edits, Some(true), "{opened:?}");
        assert!(state.open_project(dest.clone()).success);
        assert!(!open_link(&state).has_unpushed_edits);
        assert_eq!(survey_at(&dest), downloaded_survey(BASE, &repo));
    });
}

/// A saved edit is un-uploaded work: it keeps its mark through a crash and the
/// next open, and the pull still refuses to replace it (the guard every test of
/// `guard_local_edits` pins stays as it was).
#[test]
fn a_saved_edit_keeps_its_mark() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, repo, dest) = setup(temp.path(), None);
        let state = open_for_editing(&endpoint, &dest);
        set_elevation(&state, Some(12.5));
        let saved = save(&state);
        assert_eq!(saved.speleodb_unpushed_cleared, None);
        assert!(open_link(&state).has_unpushed_edits);
        drop(state);

        repo.lock()
            .expect("repo")
            .push(NEWER, revision(&endpoint, Some(3.0)));
        let state = signed_in(&endpoint);
        assert!(listed_unpushed(&state));
        let refused = pull(&state, &dest).expect_err("the saved edit is protected");
        assert_eq!(refused.action, SpeleoDbAction::UnpushedLocalEdits);
        assert!(state.open_project(dest.clone()).success);
        assert!(open_link(&state).has_unpushed_edits);
        assert!(link_on_disk(&dest).expect("linked").has_unpushed_edits);
    });
}

/// An edit, a save, the edit taken back by hand or by Undo, and a save again:
/// the copy holds its BASE survey, so it has nothing to upload - at once in the
/// open session, on disk, in the listing and for the next open.
///
/// Injection: drop the save's clear (`clear_stale_mark` returning `None`) and
/// the link stays marked after the second save.
#[test]
fn an_edit_taken_back_and_saved_counts_as_clean() {
    for by_undo in [false, true] {
        crate::test_helpers::with_isolated_config(|| {
            let temp = tempfile::tempdir().expect("temp");
            let _cache = TestCacheRoot::set(temp.path());
            let (endpoint, repo, dest) = setup(temp.path(), None);
            let state = open_for_editing(&endpoint, &dest);
            set_elevation(&state, Some(12.5));
            save(&state);
            assert!(open_link(&state).has_unpushed_edits);
            if by_undo {
                assert!(state.undo().success);
            } else {
                set_elevation(&state, None);
            }
            assert!(
                open_link(&state).has_unpushed_edits,
                "unsaved: still marked"
            );
            let saved = save(&state);
            assert_eq!(
                saved.speleodb_unpushed_cleared,
                Some(true),
                "the frontend is told (undo: {by_undo})"
            );
            assert!(!open_link(&state).has_unpushed_edits, "undo: {by_undo}");
            assert!(!link_on_disk(&dest).expect("linked").has_unpushed_edits);
            assert!(!listed_unpushed(&state));
            assert_eq!(survey_at(&dest), downloaded_survey(BASE, &repo));

            // A new edit marks it again, as any edit does.
            set_elevation(&state, Some(1.0));
            assert!(open_link(&state).has_unpushed_edits);
            assert!(raw_mark(&dest));
            assert!(listed_unpushed(&state));
            set_elevation(&state, None);
            save(&state);
            drop(state);

            // Another session pushes: the copy is replaced without a prompt.
            repo.lock()
                .expect("repo")
                .push(NEWER, revision(&endpoint, Some(3.0)));
            let state = signed_in(&endpoint);
            pull(&state, &dest).expect("nothing un-uploaded to protect");
            assert_eq!(
                link_on_disk(&dest)
                    .expect("linked")
                    .base_commit_sha
                    .as_deref(),
                Some(NEWER)
            );
        });
    }
}

/// A confirmed upload records the digest of what went up: the copy's BASE is
/// now that revision, so a crash after the next edit leaves nothing un-uploaded
/// either.
///
/// Injection: record no digest at the landing (the 200 arm passing `None` to
/// `speleodb_record_landing`) and the reopened copy is marked.
#[test]
fn after_a_confirmed_upload_a_lost_edit_leaves_no_un_uploaded_work() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, repo, dest) = setup(temp.path(), None);
        let state = open_for_editing(&endpoint, &dest);
        set_elevation(&state, Some(12.5));
        let pushed = state
            .speleodb_push_only(Some("Synthetic push".to_owned()))
            .expect("push");
        assert!(matches!(pushed, PushResult::Pushed { .. }), "{pushed:?}");
        assert_eq!(repo.lock().expect("repo").uploads, 1);
        assert_eq!(
            link_on_disk(&dest)
                .expect("linked")
                .base_commit_sha
                .as_deref(),
            Some(PUSHED)
        );
        set_elevation(&state, Some(20.0));
        assert!(raw_mark(&dest));
        drop(state);

        let state = signed_in(&endpoint);
        assert!(!listed_unpushed(&state));
        let opened = pull(&state, &dest).expect("open");
        assert_ne!(opened.has_unpushed_edits, Some(true), "{opened:?}");
        assert!(state.open_project(dest.clone()).success);
        assert!(!open_link(&state).has_unpushed_edits);
        assert_eq!(survey_at(&dest), downloaded_survey(PUSHED, &repo));
    });
}

/// The push fence holds for an edit taken back while an upload is in flight:
/// the edit taken back (and saved) during the upload is not in what went up,
/// so the copy stays marked over the new BASE. The save during the upload
/// clears nothing: BASE's digest went with the upload (an upload out may have
/// made a revision of ours past BASE), so the mark is taken at its word until
/// the landing records the new BASE's digest. The landing's late-edit fence
/// (`has_unpushed_edits = true`) keeps it marked as well.
///
/// Injection: keep the digest when the upload is sent (drop the clear in
/// `speleodb_upload`) and the save during the upload clears the mark.
#[test]
fn an_edit_taken_back_during_an_upload_stays_un_uploaded() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let shared: Arc<Mutex<Option<Arc<AppState>>>> = Arc::new(Mutex::new(None));
        let during = Arc::clone(&shared);
        // What the save made during the upload said (asserted on this thread).
        let cleared: Arc<Mutex<Option<Option<bool>>>> = Arc::new(Mutex::new(None));
        let saw = Arc::clone(&cleared);
        let (endpoint, repo, dest) = setup(
            temp.path(),
            Some(Box::new(move || {
                let state = during.lock().expect("state slot").clone();
                if let Some(state) = state {
                    let edited = state.set_surface_elevation(None).success;
                    let saved = state.execute(Command::SaveProject { path: None });
                    *saw.lock().expect("slot") = Some(
                        (edited && saved.success)
                            .then_some(saved.speleodb_unpushed_cleared)
                            .flatten(),
                    );
                }
            })),
        );
        let state = Arc::new(open_for_editing(&endpoint, &dest));
        set_elevation(&state, Some(12.5));
        *shared.lock().expect("state slot") = Some(Arc::clone(&state));
        let pushed = state
            .speleodb_push_only(Some("Synthetic push".to_owned()))
            .expect("push");
        // The upload landed, but an edit came after it started: not all of the
        // work is on the server, which the push reports as unconfirmed.
        assert!(
            matches!(pushed, PushResult::PushUnconfirmed { .. }),
            "{pushed:?}"
        );
        *shared.lock().expect("state slot") = None;
        assert_eq!(
            *cleared.lock().expect("slot"),
            Some(None),
            "the edit was taken back and saved during the upload, which held the \
             (old) BASE again, and cleared nothing: the digest went with the upload"
        );

        let link = open_link(&state);
        assert_eq!(link.base_commit_sha.as_deref(), Some(PUSHED));
        assert!(
            link.has_unpushed_edits,
            "the edit taken back during the upload is not on the server"
        );
        assert!(link_on_disk(&dest).expect("linked").has_unpushed_edits);
        assert_ne!(
            survey_at(&dest),
            downloaded_survey(PUSHED, &repo),
            "the copy differs from what went up"
        );
        drop(state);
        let state = signed_in(&endpoint);
        assert!(
            listed_unpushed(&state),
            "and stays marked for the next open"
        );
    });
}

/// A copy without a digest keeps today's behaviour: its mark is taken at its
/// word. A 0.6.0 (SQLite) cache copy, and a version 2 copy installed before the
/// digest existed, both still refuse to be replaced.
#[test]
fn a_copy_without_a_digest_keeps_its_mark() {
    for legacy in [true, false] {
        crate::test_helpers::with_isolated_config(|| {
            let temp = tempfile::tempdir().expect("temp");
            let _cache = TestCacheRoot::set(temp.path());
            let (endpoint, repo, dest) = setup(temp.path(), None);
            let mut marked = server_link(&endpoint);
            marked.base_commit_sha = Some(BASE.to_owned());
            marked.has_unpushed_edits = true;
            marked.last_edit_at = Some("2026-11-09T10:00:00Z".to_owned());
            if legacy {
                legacy_linked_project(&dest, Some(&marked));
            } else {
                // Exactly BASE's survey, marked, with no digest recorded.
                super::stalx_fixtures::write_file(
                    &dest,
                    &line_survey(NAME),
                    &extras_for(Some(&marked)),
                );
            }
            repo.lock()
                .expect("repo")
                .push(NEWER, revision(&endpoint, Some(3.0)));
            let state = signed_in(&endpoint);
            assert!(listed_unpushed(&state), "legacy: {legacy}");
            let refused = pull(&state, &dest).expect_err("protected");
            assert_eq!(refused.action, SpeleoDbAction::UnpushedLocalEdits);
            assert!(state.open_project(dest).success);
            assert!(open_link(&state).has_unpushed_edits, "legacy: {legacy}");
        });
    }
}

/// The digest describes the BASE it was recorded for: a link change to another
/// BASE or another project drops it, an edit (same BASE) keeps it.
///
/// Injection: drop the clear in `FileState::set_speleo_link` and the digest
/// outlives the BASE it describes (binding the hash to the BASE id keeps that
/// from ever matching; this keeps the state from carrying it at all).
#[test]
fn the_digest_goes_with_its_base() {
    use super::open_file::FileState;
    let mut link = server_link("https://sdb.example");
    link.base_commit_sha = Some(BASE.to_owned());
    let mut file = FileState::default();
    file.set_speleo_link(Some(&link));
    file.extras.local.speleodb.base_survey = Some("sha256:recorded".to_owned());

    link.has_unpushed_edits = true;
    link.last_edit_at = Some("2026-11-21T10:00:00Z".to_owned());
    file.set_speleo_link(Some(&link));
    assert!(
        file.extras.local.speleodb.base_survey.is_some(),
        "an edit keeps it"
    );

    link.base_commit_sha = Some(NEWER.to_owned());
    file.set_speleo_link(Some(&link));
    assert_eq!(
        file.extras.local.speleodb.base_survey, None,
        "a new BASE drops it"
    );

    file.extras.local.speleodb.base_survey = Some("sha256:recorded".to_owned());
    link.project_id = "22222222-2222-2222-2222-222222222222".to_owned();
    file.set_speleo_link(Some(&link));
    assert_eq!(
        file.extras.local.speleodb.base_survey, None,
        "another project drops it"
    );

    file.extras.local.speleodb.base_survey = Some("sha256:recorded".to_owned());
    file.set_speleo_link(None);
    assert_eq!(
        file.extras.local.speleodb.base_survey, None,
        "an unlink drops it"
    );
}

/// The BASE survey digest as the file at `path` holds it.
fn digest_on_disk(path: &Path) -> Option<String> {
    stalx_v2::probe_path(path)
        .expect("probe")
        .local
        .and_then(|local| local.speleodb.base_survey)
}

#[expect(
    clippy::result_large_err,
    reason = "the restore's own error type, returned as the restore returns it"
)]
fn restore(
    state: &AppState,
    hexsha: &str,
) -> Result<crate::speleodb::SpeleoOpenResult, crate::speleodb::SpeleoErrorDto> {
    state.speleodb_restore_revision(
        ID.to_owned(),
        NAME.to_owned(),
        hexsha.to_owned(),
        true,
        false,
        false,
        false,
        false,
    )
}

/// F2-digest-1. A Restore of an older revision installs that revision's bytes
/// on top of the server tip, marked un-uploaded: it holds another revision than
/// its BASE, so it records no digest. With one, the first save would clear the
/// Restore's mark (its survey is the recorded "BASE" survey), Done would hand
/// the lock back without uploading, and the Restore would never reach SpeleoDB.
/// The mark survives a save, a crash and the reopen, and Done uploads it.
///
/// Injection: `PreparedDownload::cache_bytes` recording the digest whatever
/// revision it holds (or the swap naming the tip as the revision it fetched)
/// records digest(R, tip) and the save clears the mark.
#[test]
fn a_restore_of_an_older_revision_records_no_digest_and_stays_un_uploaded() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, repo, dest) = setup(temp.path(), None);
        repo.lock()
            .expect("repo")
            .push(NEWER, revision(&endpoint, Some(3.0)));
        let state = open_for_editing(&endpoint, &dest);
        assert_eq!(open_link(&state).base_commit_sha.as_deref(), Some(NEWER));

        restore(&state, BASE).expect("restore BASE over the tip");
        let link = open_link(&state);
        assert_eq!(
            link.base_commit_sha.as_deref(),
            Some(NEWER),
            "it rides on the tip"
        );
        assert!(link.has_unpushed_edits, "and is un-uploaded");
        assert_eq!(survey_at(&dest), downloaded_survey(BASE, &repo));
        assert_eq!(digest_on_disk(&dest), None, "a Restore records no digest");

        let saved = save(&state);
        assert_eq!(
            saved.speleodb_unpushed_cleared, None,
            "the save clears nothing"
        );
        assert!(open_link(&state).has_unpushed_edits);
        drop(state);

        let state = signed_in(&endpoint);
        assert!(listed_unpushed(&state), "after a crash it is still listed");
        assert!(state.open_project(dest).success);
        assert!(open_link(&state).has_unpushed_edits, "and reopens marked");
        state.set_edit_enabled(true);
        state.speleodb_note_lock_acquired(Some(ID));
        let pushed = state
            .speleodb_push_only(Some("Restore".to_owned()))
            .expect("push");
        assert!(matches!(pushed, PushResult::Pushed { .. }), "{pushed:?}");
        assert_eq!(repo.lock().expect("repo").uploads, 1, "the Restore went up");
    });
}

/// F2-digest-1, the other half: a Restore of the tip itself is the tip's bytes
/// under the tip, clean, so it records the tip's digest like any install.
#[test]
fn a_restore_of_the_tip_records_its_digest() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, _repo, dest) = setup(temp.path(), None);
        let state = open_for_editing(&endpoint, &dest);
        restore(&state, BASE).expect("restore the tip");
        let link = open_link(&state);
        assert_eq!(link.base_commit_sha.as_deref(), Some(BASE));
        assert!(!link.has_unpushed_edits, "nothing to upload");
        assert!(
            digest_on_disk(&dest).is_some(),
            "the tip's digest is recorded"
        );
        // A crash after an edit then leaves nothing un-uploaded, as after a pull.
        set_elevation(&state, Some(12.5));
        drop(state);
        let state = signed_in(&endpoint);
        assert!(!listed_unpushed(&state));
    });
}

/// F2-digest-3. A 304 says SpeleoDB already holds what went up, at BASE: the
/// landing records that digest (so a lost edit afterwards leaves nothing to
/// upload), and an edit made while the upload was in flight (here: the edit
/// taken back to BASE's survey and saved) keeps the mark.
///
/// Injection: the 304 arm passing no bytes to `speleodb_record_landing` fails
/// the first half. The late-edit branch's mark is defence in depth (the digest
/// goes when the upload starts, so the save during it has nothing to clear the
/// mark with): keeping the digest at the upload AND the 304 arm leaving the mark
/// as that save left it fails the second half; either alone does not.
#[test]
fn a_304_landing_records_the_digest_and_keeps_a_late_edit_marked() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, repo, dest) = setup(temp.path(), None);
        let state = open_for_editing(&endpoint, &dest);
        set_elevation(&state, Some(12.5));
        repo.lock().expect("repo").upload_304 = true;
        let pushed = state
            .speleodb_push_only(Some("Synthetic push".to_owned()))
            .expect("push");
        assert!(matches!(pushed, PushResult::Pushed { .. }), "{pushed:?}");
        let link = link_on_disk(&dest).expect("linked");
        assert_eq!(
            link.base_commit_sha.as_deref(),
            Some(BASE),
            "a 304 lands nothing new"
        );
        assert!(!link.has_unpushed_edits);
        assert!(
            digest_on_disk(&dest).is_some(),
            "the 304 arm records what went up"
        );
        set_elevation(&state, Some(20.0));
        drop(state);
        let state = signed_in(&endpoint);
        assert!(
            !listed_unpushed(&state),
            "the lost edit leaves nothing to upload"
        );
        assert!(state.open_project(dest).success);
        assert!(!open_link(&state).has_unpushed_edits);
    });
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let shared: Arc<Mutex<Option<Arc<AppState>>>> = Arc::new(Mutex::new(None));
        let during = Arc::clone(&shared);
        let (endpoint, repo, dest) = setup(
            temp.path(),
            Some(Box::new(move || {
                if let Some(state) = during.lock().expect("state slot").clone() {
                    assert!(state.set_surface_elevation(None).success);
                    assert!(state.execute(Command::SaveProject { path: None }).success);
                }
            })),
        );
        let state = Arc::new(open_for_editing(&endpoint, &dest));
        set_elevation(&state, Some(12.5));
        repo.lock().expect("repo").upload_304 = true;
        *shared.lock().expect("state slot") = Some(Arc::clone(&state));
        let pushed = state
            .speleodb_push_only(Some("Synthetic push".to_owned()))
            .expect("push");
        *shared.lock().expect("state slot") = None;
        assert!(
            matches!(pushed, PushResult::PushUnconfirmed { .. }),
            "{pushed:?}"
        );
        assert!(
            open_link(&state).has_unpushed_edits,
            "the late edit is not up"
        );
        assert!(link_on_disk(&dest).expect("linked").has_unpushed_edits);
        drop(state);
        let state = signed_in(&endpoint);
        assert!(
            listed_unpushed(&state),
            "and stays marked for the next open"
        );
    });
}

/// F2-digest-2. A writer that does not know the digest keeps it as it is,
/// through its own upload. When that upload's answer is lost, SpeleoDB may hold
/// the edit past BASE; taking the edit back then leaves the copy holding BASE's
/// survey, marked, with BASE's digest still on it. That digest must not clear
/// the mark: the edit time it was vouched for is not the copy's any more, so
/// the copy stays un-uploaded and the open asks before replacing it.
///
/// Injection: `digest_vouched` ignoring `base_survey_edit` (trusting any
/// recorded digest) clears the mark: the listing says nothing is un-uploaded
/// and the open replaces the copy with SpeleoDB's revision, the edit back on it.
#[test]
fn a_digest_another_writer_carried_past_its_edit_is_not_trusted() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, repo, dest) = setup(temp.path(), None);
        drop(open_for_editing(&endpoint, &dest));
        let digest = digest_on_disk(&dest).expect("installed with BASE's digest");
        // What a writer that keeps unknown `local.speleodb` keys leaves: it
        // edited (moving `last_edit`), uploaded with the answer lost, took the
        // edit back and saved; the survey is BASE's and the digest is kept.
        let bytes = std::fs::read(&dest).expect("read");
        let mut extras = decoded(&dest).extras;
        extras.local.speleodb.unpushed_edits = true;
        extras.local.speleodb.last_edit_at = Some(chrono::Utc::now());
        assert_eq!(
            extras.local.speleodb.base_survey.as_deref(),
            Some(digest.as_str())
        );
        std::fs::write(
            &dest,
            stalx_v2::rewrite_local(&bytes, &extras).expect("rewrite"),
        )
        .expect("write");
        // Its upload did land, as revision NEWER.
        repo.lock()
            .expect("repo")
            .push(NEWER, revision(&endpoint, Some(12.5)));

        let state = signed_in(&endpoint);
        assert!(listed_unpushed(&state), "the copy is still un-uploaded");
        let refused = pull(&state, &dest).expect_err("the open asks first");
        assert_eq!(refused.action, SpeleoDbAction::UnpushedLocalEdits);
        assert_eq!(
            survey_at(&dest),
            downloaded_survey(BASE, &repo),
            "untouched"
        );
        assert!(state.open_project(dest).success);
        assert!(open_link(&state).has_unpushed_edits, "and opens marked");
    });
}

/// F2-digest-2, the chain itself: this build vouches for every edit it records
/// while the digest still speaks for the copy, so its own crash-lost edit
/// settles as before; an edit recorded after one it did not vouch for drops the
/// digest for good.
#[test]
fn the_digest_counts_only_while_this_build_recorded_every_edit_since() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, _repo, dest) = setup(temp.path(), None);
        let state = open_for_editing(&endpoint, &dest);
        set_elevation(&state, Some(12.5));
        set_elevation(&state, None);
        let saved = save(&state);
        assert_eq!(
            saved.speleodb_unpushed_cleared,
            Some(true),
            "own edits are vouched for"
        );
        drop(state);

        // Another writer's edit time over the kept digest, then this build edits.
        let bytes = std::fs::read(&dest).expect("read");
        let mut extras = decoded(&dest).extras;
        extras.local.speleodb.unpushed_edits = true;
        extras.local.speleodb.last_edit_at = Some(chrono::Utc::now());
        std::fs::write(
            &dest,
            stalx_v2::rewrite_local(&bytes, &extras).expect("rewrite"),
        )
        .expect("write");
        let state = signed_in(&endpoint);
        assert!(state.open_project(dest.clone()).success);
        assert!(open_link(&state).has_unpushed_edits);
        state.set_edit_enabled(true);
        state.speleodb_note_lock_acquired(Some(ID));
        set_elevation(&state, Some(1.0));
        set_elevation(&state, None);
        let saved = save(&state);
        assert_eq!(
            saved.speleodb_unpushed_cleared, None,
            "the digest no longer counts"
        );
        assert_eq!(digest_on_disk(&dest), None, "and is gone");
        assert!(open_link(&state).has_unpushed_edits);
    });
}

/// F2-digest-5. A copy with real saved work keeps its mark and its digest for
/// as long as the work stays un-uploaded, so the link probe read it whole and
/// hashed it on every listing fetch. The answer "real work" is remembered while
/// the file is unchanged; a changed file is read again, and a remembered answer
/// can only keep a mark.
///
/// Injection: `probe_settled` never consulting what it remembered reads the copy
/// whole on every listing.
#[test]
fn a_copy_with_real_work_is_hashed_once_while_it_is_unchanged() {
    use super::speleodb_base_survey::whole_reads;
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, _repo, dest) = setup(temp.path(), None);
        let state = open_for_editing(&endpoint, &dest);
        set_elevation(&state, Some(12.5));
        save(&state);
        drop(state);
        let state = signed_in(&endpoint);
        let before = whole_reads();
        for _ in 0..3 {
            assert!(listed_unpushed(&state), "real work stays listed");
        }
        assert_eq!(
            whole_reads() - before,
            1,
            "read whole once, not per listing"
        );

        // The file changes (another save of more work): read again, once.
        assert!(state.open_project(dest).success);
        state.set_edit_enabled(true);
        state.speleodb_note_lock_acquired(Some(ID));
        set_elevation(&state, Some(14.0));
        save(&state);
        let before = whole_reads();
        for _ in 0..3 {
            assert!(listed_unpushed(&state));
        }
        assert_eq!(whole_reads() - before, 1, "a changed file is read again");
    });
}

/// Demo item 5. A copy that is behind SpeleoDB - what an offline open whose
/// download alone was lost leaves: the copy at BASE, SpeleoDB at NEWER - is
/// never edited behind. Resume editing downloads the newer revision first and
/// edits on it; with un-uploaded work it routes to the divergence prompt and
/// downloads nothing. The SpeleoDB tab says so while the copy is behind.
///
/// Pins the reconcile in `speleodb_start_editing_flow` (this held before the
/// demo; the demo did not check it). Injection: `sync::reconcile_verdict`
/// answering `UpToDate` for a fast-forward (editing starts on the copy that is
/// behind) fails the first half.
#[test]
fn resume_editing_on_a_copy_behind_downloads_the_newer_revision_first() {
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, repo, dest) = setup(temp.path(), None);
        pull(&signed_in(&endpoint), &dest).expect("download BASE");
        repo.lock()
            .expect("repo")
            .push(NEWER, revision(&endpoint, Some(3.0)));

        let state = signed_in(&endpoint);
        assert!(state.open_project(dest.clone()).success);
        assert!(!state.edit_enabled(), "opens read-only, behind");
        let started = state.speleodb_start_editing().expect("resume editing");
        assert!(
            matches!(started, StartEditingResult::FastForwarded { .. }),
            "{started:?}"
        );
        let on_disk = link_on_disk(&dest).expect("linked");
        assert_eq!(on_disk.base_commit_sha.as_deref(), Some(NEWER));
        assert_eq!(survey_at(&dest), downloaded_survey(NEWER, &repo));
        assert!(state.edit_enabled(), "editing, on the newer revision");
    });
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::tempdir().expect("temp");
        let _cache = TestCacheRoot::set(temp.path());
        let (endpoint, repo, dest) = setup(temp.path(), None);
        let state = open_for_editing(&endpoint, &dest);
        set_elevation(&state, Some(12.5));
        save(&state);
        drop(state);
        repo.lock()
            .expect("repo")
            .push(NEWER, revision(&endpoint, Some(3.0)));
        let before = survey_at(&dest);
        let downloads = || {
            repo.lock()
                .expect("repo")
                .lines
                .iter()
                .filter(|line| line.contains("/download/"))
                .count()
        };
        let downloaded = downloads();

        let state = signed_in(&endpoint);
        assert!(state.open_project(dest.clone()).success);
        assert!(open_link(&state).has_unpushed_edits, "un-uploaded work");
        let started = state.speleodb_start_editing().expect("resume editing");
        assert!(
            matches!(started, StartEditingResult::DivergenceNeeded),
            "{started:?}"
        );
        assert_eq!(downloads(), downloaded, "nothing downloaded");
        assert_eq!(survey_at(&dest), before, "the work is untouched");
        assert!(!state.edit_enabled(), "still read-only");
    });
}
