//! [#576 NF-A8R-GAPS, decision NF-A8R-GAPS-D1] The Ariane adoption driven end to
//! end, so an EXTRA close earlier in the verb fails a test.
//!
//! `adopt_order` types the adoption's chain - the lock decision, the guarded fetch,
//! the stamp, the hand-over, the type settle and the release-verdict correction, and
//! the close as the last step - but the verb runs on `&AppState`, whose
//! `close_project` is the general teardown every flow uses, so no proof can forbid a
//! second, earlier call to it. Everything after the local span reads the OPEN
//! project: the hand-over's Start editing reads the open link, and its Done saves,
//! snapshots and renders the sidecar from the open survey. A close written ahead of
//! them leaves the conversion local and the lock held - which this test sees.
//!
//! Driven against a loopback stub that answers like SpeleoDB and keeps the server's
//! state across the exchange: the mutex (unheld until the adoption acquires it, freed
//! by the release), the project type (`ariane` until a PATCH says `stalagmight`) and the
//! revision history (the Ariane tip, then the pushed `.stalx` on top of it). The dump
//! at the tip holds one `ariane.tml`, a synthetic survey built in this test. After
//! the verb: the upload landed and is reported `Pushed`, the type flipped, the lock
//! went back and its claim is gone, the cache copy is clean at the pushed commit, and
//! nothing is left open (the verb's own close, last).
//!
//! The verb resolves the cache path through the process-wide app dirs, a `OnceLock`
//! other tests in this binary rely on staying unset, so - as in
//! `take_remote_success_tests`, whose child-process and deadline helpers this reuses -
//! the body runs in a process of its own.
//!
//! All data is synthetic, built in code.

use std::io::{Read as _, Write as _};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tempfile::TempDir;

use super::speleodb::{authed_session_at, REMOTE_ID};
use super::take_remote_success_tests::{output_within, CHILD_DEADLINE, TIP_SHA};
use crate::parsers::tml::test_helpers::{wrap_xml, TripShot};
use crate::persistence::Database;
use crate::speleodb::models::AdoptUpload;
use crate::test_fixtures::state_with_null_sink;

/// The environment variable that lets the child-process body run.
const CHILD_ENV: &str = "STALAGMIGHT_ADOPT_E2E_CHILD";
/// The body's full test name, for the child's exact filter.
const CHILD_TEST: &str = "core::state::sdb_adopt_e2e_tests::adoption_body_in_a_process_of_its_own";

/// The commit the stub answers the adoption's upload with.
const PUSHED_SHA: &str = "fee1230000000000000000000000000000000000";

/// Shots in the synthetic remote survey.
const REMOTE_SHOTS: i64 = 3;

/// What the stub's server holds, as the exchange changes it.
struct Server {
    /// Who holds the write mutex.
    holder: Option<&'static str>,
    /// The project's `type`.
    kind: &'static str,
    /// Whether the adoption's upload has landed (its commit is on top of the tip).
    pushed: bool,
}

/// The project as the server describes it now: every JSON answer but the upload's
/// (the client reads the fields each endpoint needs from the same shape).
fn project_json(server: &Server) -> String {
    let mutex = server
        .holder
        .map(|user| format!(r#""active_mutex":{{"user":"{user}"}},"#))
        .unwrap_or_default();
    let pushed = if server.pushed {
        format!(r#"{{"id":"{PUSHED_SHA}","parent_ids":["{TIP_SHA}"],"author_email":"u@x"}},"#)
    } else {
        String::new()
    };
    format!(
        r#"{{"id":"{REMOTE_ID}","name":"Remote","type":"{kind}",{mutex}
            "commits":[{pushed}{{"id":"{TIP_SHA}","parent_ids":[],"author_email":"other@x"}}]}}"#,
        kind = server.kind
    )
}

/// `bytes` as the one entry `name` of a ZIP archive.
fn zip_of(name: &str, bytes: &[u8]) -> Vec<u8> {
    let mut writer = zip::write::ZipWriter::new(std::io::Cursor::new(Vec::<u8>::new()));
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    writer.start_file(name, opts).expect("start entry");
    writer.write_all(bytes).expect("write entry");
    writer.finish().expect("finish zip").into_inner()
}

/// The server's tree at the tip: one `ariane.tml` - an Ariane archive whose
/// `Data.xml` is a synthetic three-shot survey - as SpeleoDB zips an Ariane project.
fn ariane_dump() -> Vec<u8> {
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
    let tml = zip_of("Data.xml", wrap_xml(&shots.join("\n")).as_bytes());
    zip_of("ariane.tml", &tml)
}

/// The request log and the server state the stub answers from.
type Seen = Arc<Mutex<Vec<String>>>;

/// A loopback stub answering each request from [`Server`] and updating it the way
/// SpeleoDB does: an acquire takes the mutex for our account, a release frees it, a
/// PATCH retypes the project `stalagmight`, the upload lands [`PUSHED_SHA`]. The dump is
/// `zip`. Records every request line. Keep the listener bound for the whole test.
fn adoption_stub(zip: Vec<u8>) -> (String, std::net::TcpListener, Seen, Arc<Mutex<Server>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let server = Arc::new(Mutex::new(Server {
        holder: None,
        kind: "ariane",
        pushed: false,
    }));
    let accept_on = listener.try_clone().expect("clone listener");
    let (log, state) = (Arc::clone(&seen), Arc::clone(&server));
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = accept_on.accept() {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(150)));
            let mut request = Vec::new();
            let mut chunk = vec![0u8; 64 * 1024];
            while let Ok(n) = stream.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                request.extend_from_slice(chunk.get(..n).unwrap_or_default());
            }
            let line = String::from_utf8_lossy(&request)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned();
            let path = line
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_owned();
            let (content_type, body): (&str, Vec<u8>) = {
                let Ok(mut server) = state.lock() else {
                    return;
                };
                if path.contains("/download/dump/") {
                    ("application/zip", zip.clone())
                } else if line.starts_with("PUT ") {
                    server.pushed = true;
                    let envelope = format!(
                        r#"{{"hexsha":"{PUSHED_SHA}","project":{{"id":"{REMOTE_ID}","name":"Remote"}}}}"#
                    );
                    ("application/json", envelope.into_bytes())
                } else {
                    if line.starts_with("POST ") && path.ends_with("/acquire/") {
                        server.holder = Some("u@x");
                    } else if line.starts_with("POST ") && path.ends_with("/release/") {
                        server.holder = None;
                    } else if line.starts_with("PATCH ") {
                        server.kind = "stalagmight";
                    }
                    ("application/json", project_json(&server).into_bytes())
                }
            };
            if let Ok(mut lines) = log.lock() {
                lines.push(line);
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{port}"), listener, seen, server)
}

#[test]
fn an_adoption_pushes_flips_and_hands_the_lock_back_before_it_closes() {
    let exe = std::env::current_exe().expect("this test binary");
    let output = output_within(
        std::process::Command::new(exe)
            .args([
                CHILD_TEST,
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_ENV, "1"),
        CHILD_DEADLINE,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("{stdout}");
    assert!(
        output.status.success(),
        "the adoption body failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("1 passed"),
        "the body ran (and was not filtered out): {stdout}"
    );
}

/// The cache copy's link as it stands on disk: (BASE, un-pushed mark).
fn link_on_disk(path: &Path) -> Option<(Option<String>, bool)> {
    Database::open_for_metadata(path)
        .ok()?
        .load_speleo_link()
        .ok()?
        .map(|link| (link.base_commit_sha, link.has_unpushed_edits))
}

/// The shots of the cache copy on disk.
fn shots_on_disk(path: &Path) -> usize {
    Database::open_for_metadata(path)
        .and_then(|db| db.load_project())
        .map(|project| project.sections.iter().map(|s| s.shots.len()).sum())
        .unwrap_or_default()
}

#[test]
#[ignore = "the child-process body of an_adoption_pushes_flips_and_hands_the_lock_back_before_it_\
            closes: it sets the process-wide app dirs, so it runs only in a process of its own"]
fn adoption_body_in_a_process_of_its_own() {
    if std::env::var_os(CHILD_ENV).is_none() {
        println!("not in the child process ({CHILD_ENV} unset): nothing to do");
        return;
    }
    let _cfg = crate::test_helpers::isolated_config();
    let dirs = TempDir::new().expect("app dirs");
    crate::app_dirs::init_paths(
        dirs.path().join("data"),
        dirs.path().join("config"),
        dirs.path().join("cache"),
        None,
    );
    let cache = super::speleodb_local_state::cache_root().expect("a cache root in this process");
    let dest = super::speleodb_local_state::cache_path_in(&cache, REMOTE_ID);
    std::fs::create_dir_all(dest.parent().expect("cache dir")).expect("create cache dir");

    let (base, _listener, seen, server) = adoption_stub(ariane_dump());
    let state = state_with_null_sink();
    *state.speleodb.lock() = authed_session_at(&base);

    let adopted = state.speleodb_adopt_ariane(
        REMOTE_ID.to_owned(),
        "Remote".to_owned(),
        Some(TIP_SHA.to_owned()),
        false,
        None,
    );

    let requests = seen.lock().expect("request log").clone();
    let (holder, kind) = {
        let server = server.lock().expect("server state");
        (server.holder, server.kind)
    };
    let link = link_on_disk(&dest);
    let shots = shots_on_disk(&dest);
    let claim_kept = state.speleodb_lock_acquired_here(REMOTE_ID);
    let still_open = state.get_project_state().is_some();
    let upload_at = requests.iter().position(|r| r.starts_with("PUT "));
    let release_at = requests
        .iter()
        .position(|r| r.starts_with("POST ") && r.contains("/release/"));
    println!(
        "[NF-A8R-GAPS-D1] adoption -> {:?}; server: holder {holder:?}, type {kind:?}; cache copy \
         link (BASE, un-pushed) {link:?}, {shots} shots; claim kept {claim_kept}; a project \
         still open {still_open}; upload at request {upload_at:?}, release at {release_at:?}; \
         requests {requests:?}",
        adopted
            .as_ref()
            .map(|r| (r.upload.clone(), r.type_flipped, r.local_path.clone()))
            .map_err(|e| e.message.clone())
    );

    let adopted = adopted.expect("the adoption converts");
    assert_eq!(
        Path::new(&adopted.local_path),
        dest,
        "it answers the cache path"
    );
    assert!(
        matches!(adopted.upload, AdoptUpload::Pushed),
        "the conversion was pushed and the lock handed back, got {:?}",
        adopted.upload
    );
    assert!(
        upload_at.is_some(),
        "the upload reached the server: {requests:?}"
    );
    assert!(
        release_at > upload_at,
        "and the lock was released after it: {requests:?}"
    );
    assert_eq!(holder, None, "the server's mutex is free");
    assert!(!claim_kept, "and no claim on it is left");
    assert!(adopted.type_flipped, "the server type is reported flipped");
    assert_eq!(kind, "stalagmight", "and it is `stalagmight` on the server");
    assert_eq!(
        link,
        Some((Some(PUSHED_SHA.to_owned()), false)),
        "the cache copy is clean at the pushed commit"
    );
    assert_eq!(
        shots,
        usize::try_from(REMOTE_SHOTS).expect("a small count"),
        "it holds the converted survey"
    );
    assert!(
        !still_open,
        "and the verb's own close, last, left nothing open"
    );
}
