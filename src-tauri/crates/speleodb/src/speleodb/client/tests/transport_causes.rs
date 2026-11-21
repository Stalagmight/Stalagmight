//! F1: the typed cause of every way a request can fail to get its reply,
//! through the real SDK client against a loopback listener that misbehaves on
//! purpose. A read that got no reply at all (a connection that could not be
//! made, one closed or reset before the whole reply, a name that does not
//! resolve) must say so (`TransportCause::got_no_reply`), since that is what
//! lets an open fall back to the cached copy; a write fails the same way but is
//! never sent twice. Nothing leaves loopback, except one DNS lookup of a name
//! under `.invalid`, which never resolves (RFC 6761).
use super::*;

/// How the loopback listener misbehaves on every connection.
#[derive(Clone, Copy, Debug)]
enum Fault {
    /// Read the whole request, then close without a word: no reply at all.
    CloseBeforeReply,
    /// Read the request but its last byte, then close: the unread byte makes
    /// the kernel reset the connection instead of closing it.
    ResetBeforeReply,
    /// Close in the middle of the reply head, before its blank line.
    HeadCutShort,
    /// A 200 whose head promises 1000 bytes, then 10 of them and a close.
    BodyCutShort,
    /// A 200 head promising a body, part of it, then a reset (the request's
    /// last byte left unread).
    ResetInBody,
    /// Bytes that are not an HTTP reply at all, then a close.
    NotHttp,
}

/// The head of one request, and its `Content-Length` body unless `keep_last`
/// leaves the last byte of the head unread.
fn read_request(stream: &mut std::net::TcpStream, keep_last: bool) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let mut request = Vec::new();
    let mut byte = [0_u8; 1];
    let end = if keep_last {
        &b"\r\n\r"[..]
    } else {
        &b"\r\n\r\n"[..]
    };
    while !request.ends_with(end) {
        match stream.read(&mut byte) {
            Ok(1) => request.push(byte[0]),
            _ => break,
        }
    }
    if !keep_last {
        let head = String::from_utf8_lossy(&request).to_lowercase();
        let length = head
            .lines()
            .find_map(|h| {
                h.strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        let mut body = vec![0_u8; length];
        let _ = stream.read_exact(&mut body);
    }
    String::from_utf8_lossy(&request)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// A loopback listener answering `connections` connections with `fault`, then
/// closing (so a request past them is refused). `finish` returns the request
/// lines it saw; a connection that never comes ends the wait after 30 s, so a
/// client that tries fewer times fails the count instead of hanging the test.
struct Faulty {
    base: String,
    worker: Option<std::thread::JoinHandle<Vec<String>>>,
}
impl Faulty {
    fn start(fault: Fault, connections: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listen");
        let base = format!("http://{}", listener.local_addr().expect("address"));
        listener.set_nonblocking(true).expect("nonblocking");
        let worker = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            let mut seen = Vec::new();
            for _ in 0..connections {
                let Some(mut stream) = (loop {
                    match listener.accept() {
                        Ok((stream, _)) => break Some(stream),
                        Err(_) if Instant::now() < deadline => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break None,
                    }
                }) else {
                    break;
                };
                stream
                    .set_nonblocking(false)
                    .expect("blocking accepted stream");
                let keep_last = matches!(fault, Fault::ResetBeforeReply | Fault::ResetInBody);
                let line = read_request(&mut stream, keep_last);
                // The type the route answers, so the SDK reads the body it expects.
                let kind = if line.contains("/download/") {
                    "application/octet-stream"
                } else {
                    "application/json"
                };
                let reply = match fault {
                    Fault::CloseBeforeReply | Fault::ResetBeforeReply => String::new(),
                    Fault::HeadCutShort => {
                        format!("HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nCont")
                    }
                    Fault::BodyCutShort | Fault::ResetInBody => format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: 1000\r\n\
                         Connection: close\r\n\r\n{{\"id\": \"1"
                    ),
                    Fault::NotHttp => "SSH-2.0-OpenSSH_9.6\r\nProtocol mismatch.\r\n".to_owned(),
                };
                seen.push(line);
                let _ = stream.write_all(reply.as_bytes());
                let _ = stream.flush();
                // Give the client the bytes before the close (or the reset).
                std::thread::sleep(Duration::from_millis(50));
            }
            seen
        });
        Self {
            base,
            worker: Some(worker),
        }
    }
    fn finish(mut self) -> Vec<String> {
        self.worker
            .take()
            .expect("worker")
            .join()
            .expect("listener")
    }
}

fn cause_of(error: &SpeleoDbError) -> Option<TransportCause> {
    match error {
        SpeleoDbError::Transport { cause, .. } => Some(*cause),
        _ => None,
    }
}

/// Every read the open path makes (the project, its history, the download at
/// HEAD and at a revision) under `fault`, retried twice as a safe read is, and
/// the cause each comes back with.
fn reads_under(fault: Fault) -> Vec<(&'static str, Option<TransportCause>)> {
    type Read = fn(&str) -> Result<(), SpeleoDbError>;
    let reads: [(&'static str, Read); 4] = [
        ("project", |base| {
            get_project(base, "t", false, ID).map(drop)
        }),
        ("revisions", |base| {
            get_revisions(base, "t", false, ID).map(drop)
        }),
        ("download", |base| {
            download_stalx(base, "t", false, ID).map(drop)
        }),
        ("download at a revision", |base| {
            download_stalx_at_hash(base, "t", false, ID, SHA).map(drop)
        }),
    ];
    reads
        .into_iter()
        .map(|(name, read)| {
            let server = Faulty::start(fault, 3);
            let error = read(&server.base).expect_err("every attempt fails");
            let seen = server.finish();
            assert_eq!(seen.len(), 3, "{name}: a read is tried three times");
            assert!(seen.iter().all(|line| line.starts_with("GET ")), "{seen:?}");
            (name, cause_of(&error))
        })
        .collect()
}

#[test]
fn a_connection_closed_before_the_reply_is_dropped() {
    // The fuzzer's trace (seed 900366): every GET under /api/v2/projects/
    // dropped before any reply. hyper reports an incomplete message with no
    // I/O error under it, which read as `Unknown` and so never fell back.
    for (read, cause) in reads_under(Fault::CloseBeforeReply) {
        assert_eq!(cause, Some(TransportCause::Dropped), "{read}");
    }
}

#[test]
fn a_connection_reset_before_the_reply_is_dropped() {
    // A reset of the SDK's send was `Unknown`: only a body read was walked for
    // its I/O error.
    for (read, cause) in reads_under(Fault::ResetBeforeReply) {
        assert_eq!(cause, Some(TransportCause::Dropped), "{read}");
    }
}

#[test]
fn a_reply_cut_short_is_dropped() {
    // A head cut short is an incomplete message; a body shorter than its
    // length is an early end of file, which read as `Unknown` too.
    for fault in [Fault::HeadCutShort, Fault::BodyCutShort] {
        for (read, cause) in reads_under(fault) {
            assert_eq!(cause, Some(TransportCause::Dropped), "{fault:?}: {read}");
        }
    }
}

#[test]
fn a_reset_while_the_reply_is_read_is_dropped_not_refused() {
    // A connection that answered is not one that could not be made: this read
    // as `Refused` ("could not connect"). It still has no reply to act on.
    for (read, cause) in reads_under(Fault::ResetInBody) {
        assert_eq!(cause, Some(TransportCause::Dropped), "{read}");
    }
}

#[test]
fn a_connection_that_cannot_be_made_is_refused() {
    // Nothing listens on the port: the SDK's send fails to connect, which read
    // as `Unknown` because only a body read was walked for its I/O error.
    let listener = TcpListener::bind("127.0.0.1:0").expect("listen");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    drop(listener);
    let error = get_project(&base, "t", false, ID).expect_err("nothing listens");
    assert_eq!(cause_of(&error), Some(TransportCause::Refused), "{error:?}");
    let error = download_stalx(&base, "t", false, ID).expect_err("nothing listens");
    assert_eq!(cause_of(&error), Some(TransportCause::Refused), "{error:?}");
}

#[test]
fn an_https_name_that_does_not_resolve_is_a_dns_failure() {
    // An https host is resolved by the SDK's send (an http one is resolved by
    // the transport policy first, `tls::enforce`), so its DNS failure came back
    // through the send's own classifier, which knew only timeouts and TLS.
    let base = "https://speleodb-offline.invalid";
    let error = get_project(base, "t", false, ID).expect_err("never resolves");
    assert_eq!(cause_of(&error), Some(TransportCause::Dns), "{error:?}");
}

#[test]
fn a_reply_that_is_not_http_stays_unknown() {
    // The server said something, just not HTTP: not a failure to reach it, so
    // not a reason to open the cached copy.
    for (read, cause) in reads_under(Fault::NotHttp) {
        assert_eq!(cause, Some(TransportCause::Unknown), "{read}");
        assert!(cause.is_some_and(|c| !c.got_no_reply()), "{read}");
    }
}

#[test]
fn only_a_failure_without_any_reply_counts_as_no_reply() {
    for (cause, no_reply) in [
        (TransportCause::Refused, true),
        (TransportCause::Dropped, true),
        (TransportCause::Timeout, true),
        (TransportCause::Dns, true),
        (TransportCause::Tls, false),
        (TransportCause::Unknown, false),
    ] {
        assert_eq!(cause.got_no_reply(), no_reply, "{cause:?}");
    }
}

#[test]
fn a_write_whose_reply_is_lost_is_dropped_and_never_sent_again() {
    // The same lost reply on a write: classified alike, and never replayed (a
    // write may have landed). The caller keeps the outcome as unknown.
    let file = tempfile::NamedTempFile::new().expect("file");
    std::fs::write(file.path(), b"{\"format\":\"stalx\",\"version\":2}").expect("write");
    let path = file.path().to_owned();
    type Write = Box<dyn Fn(&str) -> Result<(), SpeleoDbError>>;
    let writes: [(&str, Write); 5] = [
        (
            "acquire",
            Box::new(|base: &str| acquire_lock(base, "t", false, ID).map(drop)),
        ),
        (
            "release",
            Box::new(|base: &str| release_lock(base, "t", false, ID).map(drop)),
        ),
        (
            "create",
            Box::new(|base: &str| {
                create_project_with(
                    base,
                    "t",
                    false,
                    &json!({"name":"Synthetic","description":"Test","country":"US",
                        "type":"STALAGMIGHT"})
                    .to_string(),
                )
                .map(drop)
            }),
        ),
        (
            "patch",
            Box::new(|base: &str| patch_project(base, "t", false, ID, r#"{"name":"R"}"#).map(drop)),
        ),
        (
            "upload",
            Box::new(move |base: &str| {
                upload_stalx(base, "t", false, ID, &path, "Synthetic").map(drop)
            }),
        ),
    ];
    for fault in [Fault::CloseBeforeReply, Fault::BodyCutShort] {
        for (name, write) in &writes {
            let server = Faulty::start(fault, 1);
            let error = write(&server.base).expect_err("the reply is lost");
            assert_eq!(server.finish().len(), 1, "{fault:?}: {name} is sent once");
            assert_eq!(
                cause_of(&error),
                Some(TransportCause::Dropped),
                "{fault:?}: {name}: {error:?}"
            );
        }
    }
}
