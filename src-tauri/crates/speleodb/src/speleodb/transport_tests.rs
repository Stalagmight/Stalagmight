//! [#576 B5 / B25 / NF-AN3-3] The SpeleoDB transport, observed on a real socket.
//!
//! Every test here drives a public `client::*` function against a loopback stub
//! that speaks HTTP/1.1 keep-alive (unlike the `Connection: close` stub in
//! `client.rs`), so what the stub counts is what the client actually did:
//! connections opened, requests sent, and how a reply was classified.

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::client;
use super::{Permission, SpeleoDbError};

/// What the stub does with one request.
#[derive(Clone, Copy)]
enum Reply {
    /// Answer `status` with a body of `len` bytes (`{}` when `len == 2`) and keep
    /// the connection open for the next request.
    Status { status: u16, len: usize },
    /// Read the request, then close the socket without answering: the reply the
    /// client waits for is lost.
    Drop,
}

/// A keep-alive loopback server: answers the N-th request (across all
/// connections) with `replies[N]` (the last entry repeats), counting the
/// connections it accepted and recording each request line.
struct KeepAliveStub {
    base: String,
    accepted: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<String>>>,
    _listener: TcpListener,
}

impl KeepAliveStub {
    fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let next = Arc::new(AtomicUsize::new(0));
        let replies = Arc::new(replies);
        let accept_on = listener.try_clone().expect("clone listener");
        let (acc, reqs) = (Arc::clone(&accepted), Arc::clone(&requests));
        std::thread::spawn(move || {
            while let Ok((stream, _)) = accept_on.accept() {
                acc.fetch_add(1, Ordering::SeqCst);
                let (reqs, next, replies) =
                    (Arc::clone(&reqs), Arc::clone(&next), Arc::clone(&replies));
                std::thread::spawn(move || serve(stream, &reqs, &next, &replies));
            }
        });
        Self {
            base: format!("http://127.0.0.1:{port}"),
            accepted,
            requests,
            _listener: listener,
        }
    }

    fn connections(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("request log").clone()
    }
}

/// Serve requests on one connection until the client closes it.
fn serve(
    mut stream: TcpStream,
    requests: &Mutex<Vec<String>>,
    next: &AtomicUsize,
    replies: &[Reply],
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let Some(line) = read_request(&mut stream, &mut buf) else {
            return;
        };
        requests.lock().expect("request log").push(line);
        let n = next.fetch_add(1, Ordering::SeqCst);
        let reply = replies
            .get(n)
            .or_else(|| replies.last())
            .copied()
            .unwrap_or(Reply::Drop);
        match reply {
            Reply::Drop => return,
            Reply::Status { status, len } => {
                let body = if len == 2 {
                    b"{}".to_vec()
                } else {
                    vec![b'x'; len]
                };
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\n\r\n",
                    body.len()
                );
                if stream.write_all(head.as_bytes()).is_err()
                    || stream.write_all(&body).is_err()
                    || stream.flush().is_err()
                {
                    return;
                }
            }
        }
    }
}

/// Read one HTTP/1.1 request (head + `Content-Length` body) off `stream`,
/// returning its request line, or `None` once the client has closed it.
fn read_request(stream: &mut TcpStream, buf: &mut Vec<u8>) -> Option<String> {
    let mut chunk = [0u8; 16 * 1024];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        let n = stream.read(&mut chunk).ok().filter(|n| *n > 0)?;
        buf.extend_from_slice(chunk.get(..n)?);
    };
    let head = String::from_utf8_lossy(buf.get(..head_end)?).into_owned();
    let body_len = head
        .lines()
        .find_map(|l| {
            let (name, value) = l.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    while buf.len() < head_end + body_len {
        let n = stream.read(&mut chunk).ok().filter(|n| *n > 0)?;
        buf.extend_from_slice(chunk.get(..n)?);
    }
    buf.drain(..head_end + body_len);
    Some(head.lines().next().unwrap_or_default().to_owned())
}

const PROJECT_ID: &str = "11111111-1111-1111-1111-111111111111";

/// A `[]` project list is the smallest body `list_projects` parses.
fn ok_list() -> Reply {
    Reply::Status {
        status: 200,
        len: 2,
    }
}

#[test]
fn two_reads_in_a_row_share_one_connection() {
    // [#576 B5-D3 "Pool reads"] The status poll makes two back-to-back GETs per
    // tick. A fresh agent per call opened a new TCP (and, against a real server,
    // TLS) connection for each; a shared pooled agent reuses the first.
    //
    // `list_projects` parses `[]`, not `{}`, so the stub's `{}` body is a parse
    // error - which is fine: the connection count is decided before the parse.
    let stub = KeepAliveStub::start(vec![ok_list()]);
    let _ = client::list_projects(&stub.base, "tok", false);
    let _ = client::list_projects(&stub.base, "tok", false);
    let opened = stub.connections();
    println!("connections opened for two reads: {opened} (expected 1)");
    assert_eq!(stub.requests().len(), 2, "both reads reached the server");
    assert_eq!(
        opened, 1,
        "the second read must reuse the first read's connection"
    );
}

#[test]
fn a_write_never_travels_on_a_pooled_connection() {
    // [#576 B5-D3 "fresh writes"] A run-once write on a socket the server already
    // closed fails with a transport error it may not retry; the push would then
    // report "could not be confirmed" for nothing. So writes open their own
    // connection even right after a read left one idle in the pool.
    let stub = KeepAliveStub::start(vec![ok_list()]);
    let _ = client::list_projects(&stub.base, "tok", false);
    let _ = client::patch_project(
        &client::TestWire,
        &stub.base,
        "tok",
        false,
        PROJECT_ID,
        "{}",
    );
    let _ = client::patch_project(
        &client::TestWire,
        &stub.base,
        "tok",
        false,
        PROJECT_ID,
        "{}",
    );
    let opened = stub.connections();
    println!("connections opened for one read then two writes: {opened} (expected 3)");
    assert_eq!(stub.requests().len(), 3);
    assert_eq!(opened, 3, "each write opens a fresh connection");
}

#[test]
fn deleting_a_project_the_server_no_longer_has_counts_as_deleted() {
    // [#576 B5-D2 "Retry; 404 means done" / NF-AN3-3] DELETE is idempotent: a 404
    // from it means the project is already gone (a first DELETE whose reply was
    // lost, or one made from another window). Reported as a hard error, it left
    // the local copy linked to a project the server no longer has.
    let stub = KeepAliveStub::start(vec![Reply::Status {
        status: 404,
        len: 2,
    }]);
    let result = client::delete_project(&stub.base, "tok", false, PROJECT_ID, &stub.base);
    println!("delete answered 404 -> {result:?}");
    assert!(result.is_ok(), "a 404 from DELETE is 'already deleted'");
}

#[test]
fn a_delete_of_a_project_on_another_server_sends_nothing() {
    // [#576 RV4-R-S4-D2] The request would go to `base`, the session's server, but
    // the project lives on another one: refused before the request, naming the
    // project's server, whatever `base` would have answered.
    let stub = KeepAliveStub::start(vec![Reply::Status {
        status: 200,
        len: 2,
    }]);
    let result = client::delete_project(
        &stub.base,
        "tok",
        false,
        PROJECT_ID,
        "https://sdb.example.com/",
    );
    let requests = stub.requests();
    println!("delete for a project on another server -> {result:?}; requests {requests:?}");
    assert!(requests.is_empty(), "nothing was sent: {requests:?}");
    assert!(
        matches!(
            &result,
            Err(SpeleoDbError::Refused { code, message })
                if *code == crate::models::CommandErrorCode::SpeleodbFatal
                    && message.contains("This project lives on https://sdb.example.com/")
        ),
        "{result:?}"
    );
}

#[test]
fn a_delete_whose_reply_was_lost_is_retried_and_its_404_is_done() {
    // [#576 B5-D2] The lost-reply case end to end: the first DELETE reaches the
    // server and its reply never arrives; the automatic retry finds the project
    // gone (404). That is the delete having worked, not a failure.
    let stub = KeepAliveStub::start(vec![
        Reply::Drop,
        Reply::Status {
            status: 404,
            len: 2,
        },
    ]);
    let result = client::delete_project(&stub.base, "tok", false, PROJECT_ID, &stub.base);
    let requests = stub.requests();
    println!("delete attempts: {} -> {result:?}", requests.len());
    assert_eq!(requests.len(), 2, "the lost reply is retried once");
    assert!(
        requests.iter().all(|r| r.starts_with("DELETE ")),
        "{requests:?}"
    );
    assert!(result.is_ok(), "the retry's 404 means the delete landed");
}

#[test]
fn an_oversized_reply_to_a_permission_write_is_a_size_refusal() {
    // [#576 B25] The permission writes bypassed `with_retry` and re-implemented
    // its tail without the body-limit arm, so a reply over ureq's 10 MiB ceiling
    // was reported as "could not reach SpeleoDB" - the misdiagnosis
    // `is_body_limit` exists to prevent. Through the shared transport it is the
    // same size refusal every other endpoint gives, and the write still runs once.
    let stub = KeepAliveStub::start(vec![Reply::Status {
        status: 200,
        len: 11 * 1024 * 1024,
    }]);
    let result = client::add_user_permission(
        &stub.base,
        "tok",
        false,
        PROJECT_ID,
        "a@x",
        Permission::ReadOnly,
    );
    let kind = match &result {
        Err(SpeleoDbError::Parse(m)) => format!("Parse({m})"),
        Err(SpeleoDbError::Transport { cause, detail }) => {
            format!("Transport({cause:?}, {detail})")
        }
        other => format!("{other:?}"),
    };
    println!("permission write with an 11 MiB reply -> {kind}");
    match &result {
        Err(SpeleoDbError::Parse(message)) => assert!(
            message.contains("10 MiB"),
            "the refusal names the ceiling that applied: {message}"
        ),
        other => panic!("expected a Parse size refusal, got: {other:?}"),
    }
    assert_eq!(stub.requests().len(), 1, "a write runs exactly once");
}
