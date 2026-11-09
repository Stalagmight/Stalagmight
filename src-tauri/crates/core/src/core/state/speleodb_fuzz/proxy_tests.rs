//! Offline tests of the fault proxy: a tiny loopback upstream answers canned
//! replies, and raw sockets as well as the app's SpeleoDB client go through
//! the proxy. Nothing leaves loopback.

use std::io::{self, BufReader, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{lock, read_body, read_head, FaultKind, FaultProxy, FaultRule, ProxyEvent};
use crate::speleodb::client;
use crate::speleodb::{SpeleoDbError, TransportCause};

const ID: &str = "11111111-2222-4333-8444-555555555555";
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const TOKEN: &str = "synthetic";
/// The bound on every wait in these tests; none is expected to take long.
const WAIT: Duration = Duration::from_secs(5);

/// What the in-test upstream answers.
#[derive(Clone)]
enum Canned {
    /// This status and body, framed by Content-Length.
    Length(u16, Vec<u8>),
    /// A 200 whose body goes out in these chunks, then a trailer.
    Chunked(Vec<Vec<u8>>),
    /// A 200 whose body ends when the upstream closes the connection.
    CloseDelimited(Vec<u8>),
}

impl Canned {
    fn json(value: &Value) -> Self {
        Self::Length(200, value.to_string().into_bytes())
    }

    fn wire(&self) -> Vec<u8> {
        let (status, framing, body) = match self {
            Self::Length(status, body) => (
                *status,
                format!("Content-Length: {}\r\n", body.len()),
                body.clone(),
            ),
            Self::Chunked(chunks) => {
                let mut wire = Vec::new();
                for chunk in chunks {
                    wire.extend_from_slice(format!("{:x};ext=1\r\n", chunk.len()).as_bytes());
                    wire.extend_from_slice(chunk);
                    wire.extend_from_slice(b"\r\n");
                }
                wire.extend_from_slice(b"0\r\nX-Trailer: done\r\n\r\n");
                (200, "Transfer-Encoding: chunked\r\n".to_owned(), wire)
            }
            Self::CloseDelimited(body) => (200, String::new(), body.clone()),
        };
        let decoded = match self {
            Self::Chunked(chunks) => chunks.concat(),
            Self::Length(_, body) | Self::CloseDelimited(body) => body.clone(),
        };
        let kind = if serde_json::from_slice::<Value>(&decoded).is_ok() {
            "application/json"
        } else {
            "application/octet-stream"
        };
        let mut wire = format!(
            "HTTP/1.1 {status} Test\r\nContent-Type: {kind}\r\n{framing}Connection: close\r\n\r\n"
        )
        .into_bytes();
        wire.extend_from_slice(&body);
        wire
    }
}

/// One request the in-test upstream received.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    fn line(&self) -> String {
        format!("{} {}", self.method, self.path)
    }
}

type Log = Arc<(Mutex<Vec<Seen>>, Condvar)>;

/// A tiny upstream: one thread per connection, records each request, answers
/// what `router` picks for it, closes.
struct Upstream {
    addr: SocketAddr,
    seen: Log,
    stop: Arc<AtomicBool>,
}

impl Upstream {
    fn start(router: impl Fn(&Seen) -> Canned + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind the test upstream");
        let addr = listener.local_addr().expect("the test upstream's address");
        let seen: Log = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let router = Arc::new(router);
        let (log, halt) = (Arc::clone(&seen), Arc::clone(&stop));
        thread::spawn(move || {
            for conn in listener.incoming() {
                if halt.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(stream) = conn else { continue };
                let (log, router) = (Arc::clone(&log), Arc::clone(&router));
                thread::spawn(move || answer(stream, router.as_ref(), &log));
            }
        });
        Self { addr, seen, stop }
    }

    fn canned(canned: Canned) -> Self {
        Self::start(move |_| canned.clone())
    }

    fn seen(&self) -> Vec<Seen> {
        lock(&self.seen.0).clone()
    }

    fn lines(&self) -> Vec<String> {
        self.seen().iter().map(Seen::line).collect()
    }
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
    }
}

fn answer(stream: TcpStream, router: &dyn Fn(&Seen) -> Canned, log: &(Mutex<Vec<Seen>>, Condvar)) {
    let _ = stream.set_read_timeout(Some(WAIT));
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(read_half);
    let Ok(head) = read_head(&mut reader) else {
        return;
    };
    let Ok(body) = read_body(&mut reader, &head.headers) else {
        return;
    };
    let seen = Seen {
        headers: head
            .headers
            .iter()
            .map(|h| (h.name.clone(), h.value.clone()))
            .collect(),
        method: head.method,
        path: head.path,
        body,
    };
    let wire = router(&seen).wire();
    lock(&log.0).push(seen);
    log.1.notify_all();
    let mut stream = stream;
    let _ = stream.write_all(&wire);
    let _ = stream.shutdown(Shutdown::Both);
}

fn project_json() -> Value {
    json!({"id": ID, "name": "Synthetic cave", "description": "proxy test", "country": "US",
        "type": "STALAGMIGHT", "visibility": "PRIVATE", "permission": "ADMIN",
        "color": "#123456", "commit_count": 0, "created_by": "fuzz-proxy-test",
        "creation_date": "2026-01-01T00:00:00Z", "modified_date": "2026-01-01T00:00:00Z",
        "exclude_geojson": false, "fork_from": null, "is_active": true,
        "active_mutex": null, "latest_commit": null})
}

/// Synthetic native-file bytes, longer than the 100 bytes `ScrambleBody` keeps.
fn stalx() -> Vec<u8> {
    let mut bytes = b"SQLite format 3\0".to_vec();
    bytes.extend((0..400_u32).map(|i| (i % 251) as u8));
    bytes
}

/// The SDK routes the client tests use, answered like SpeleoDB would.
fn sdk_routes(seen: &Seen) -> Canned {
    if seen.path.contains("/upload/") {
        Canned::json(&json!({"project": project_json(), "hexsha": SHA,
            "files": ["project.stalx"], "geojson_status": "skipped", "message": "Saved",
            "browser_url": null}))
    } else if seen.path.contains("/download/") {
        Canned::Length(200, stalx())
    } else {
        Canned::json(&project_json())
    }
}

fn start(upstream: &Upstream) -> FaultProxy {
    FaultProxy::start(upstream.addr).expect("start the proxy")
}

fn rule(method: Option<&'static str>, path: &str, kind: FaultKind, times: u32) -> FaultRule {
    FaultRule {
        method,
        path_contains: path.to_owned(),
        kind,
        times,
    }
}

fn settle(proxy: &FaultProxy) {
    assert!(proxy.quiesce(WAIT), "the proxy went idle");
}

fn event(
    method: &str,
    path: &str,
    upstream_status: Option<u16>,
    fault: Option<&str>,
    delivered: bool,
) -> ProxyEvent {
    ProxyEvent {
        method: method.to_owned(),
        path: path.to_owned(),
        upstream_status,
        fault: fault.map(str::to_owned),
        held_ms: None,
        delivered,
    }
}

/// `event` without its `held_ms`, which must be set.
fn unheld(mut event: ProxyEvent) -> ProxyEvent {
    assert!(event.held_ms.is_some(), "a held request records its hold");
    event.held_ms = None;
    event
}

fn get(path: &str) -> Vec<u8> {
    format!("GET {path} HTTP/1.1\r\nHost: sdb.test:8000\r\nConnection: keep-alive\r\n\r\n")
        .into_bytes()
}

/// A request with a Content-Length body, asking for keep-alive (which the
/// proxy must replace).
fn with_body(method: &str, path: &str, body: &str) -> Vec<u8> {
    format!(
        "{method} {path} HTTP/1.1\r\nHost: sdb.test:8000\r\nConnection: keep-alive\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// Send `request` to `addr` and read until the connection closes.
fn exchange(addr: SocketAddr, request: &[u8], timeout: Duration) -> io::Result<Vec<u8>> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.write_all(request)?;
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply)?;
    Ok(reply)
}

/// A reply as the client got it: the status, the headers (names lowercased)
/// and every byte after the head.
struct Got {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Got {
    fn parse(raw: &[u8]) -> Self {
        let end = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("a reply head");
        let head = String::from_utf8_lossy(&raw[..end]);
        let mut lines = head.split("\r\n");
        let status = lines
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .expect("a status line");
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_owned()))
            .collect();
        Self {
            status,
            headers,
            body: raw[end + 4..].to_vec(),
        }
    }

    fn all(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    fn content_length(&self) -> Option<usize> {
        self.all("content-length")
            .first()
            .and_then(|v| v.parse().ok())
    }
}

fn ask(proxy: &FaultProxy, request: &[u8]) -> Got {
    Got::parse(&exchange(proxy.addr, request, WAIT).expect("a reply"))
}

fn transport(result: Result<impl std::fmt::Debug, SpeleoDbError>) -> TransportCause {
    match result {
        Err(SpeleoDbError::Transport { cause, .. }) => cause,
        other => panic!("expected a transport error, got {other:?}"),
    }
}

#[test]
fn pass_through_relays_every_upstream_framing_with_a_content_length() {
    let body = br#"{"framing":"test","n":12345}"#.to_vec();
    let framings = [
        Canned::Length(200, body.clone()),
        Canned::Chunked(vec![body[..7].to_vec(), body[7..].to_vec()]),
        Canned::CloseDelimited(body.clone()),
    ];
    for canned in framings {
        let upstream = Upstream::canned(canned);
        let proxy = start(&upstream);
        let got = ask(&proxy, &get("/api/v2/projects/"));
        assert_eq!(got.status, 200);
        assert_eq!(got.body, body);
        assert_eq!(got.content_length(), Some(body.len()));
        assert_eq!(got.all("transfer-encoding"), Vec::<&str>::new());
        assert_eq!(got.all("connection"), ["close"]);
        assert_eq!(got.all("content-type"), ["application/json"]);
        settle(&proxy);
        let log = proxy.take_log();
        assert_eq!(
            log,
            [event("GET", "/api/v2/projects/", Some(200), None, true)]
        );
        assert!(proxy.take_log().is_empty(), "a take drains the log");
    }
}

#[test]
fn request_bodies_reach_upstream_decoded_with_a_content_length() {
    let upstream = Upstream::canned(Canned::json(&json!({"ok": true})));
    let proxy = start(&upstream);

    assert_eq!(
        ask(
            &proxy,
            &with_body("POST", "/api/v2/projects/", r#"{"n":1}"#)
        )
        .status,
        200
    );
    let chunked = "PUT /api/v2/projects/p1/upload/stalagmight/ HTTP/1.1\r\nHost: sdb.test:8000\r\n\
                   Transfer-Encoding: chunked\r\n\r\n5;name=v\r\nhello\r\n6\r\n world\r\n\
                   0\r\nX-Checksum: 1\r\n\r\n";
    assert_eq!(ask(&proxy, chunked.as_bytes()).status, 200);
    assert_eq!(ask(&proxy, &get("/api/v2/projects/")).status, 200);
    let expecting = "PUT /api/v2/projects/p1/ HTTP/1.1\r\nHost: sdb.test:8000\r\n\
                     Expect: 100-continue\r\nContent-Length: 2\r\n\r\n{}";
    let raw = exchange(proxy.addr, expecting.as_bytes(), WAIT).expect("a reply");
    let interim = b"HTTP/1.1 100 Continue\r\n\r\n";
    assert!(
        raw.starts_with(interim),
        "{}",
        String::from_utf8_lossy(&raw)
    );
    assert_eq!(Got::parse(&raw[interim.len()..]).status, 200);

    settle(&proxy);
    let seen = upstream.seen();
    assert_eq!(seen.len(), 4);
    for request in &seen {
        assert_eq!(request.header("connection"), Some("close"));
        assert_eq!(request.header("host"), Some("sdb.test:8000"));
        assert_eq!(request.header("transfer-encoding"), None);
        assert_eq!(request.header("expect"), None);
    }
    assert_eq!(seen[0].body.as_deref(), Some(br#"{"n":1}"#.as_slice()));
    assert_eq!(seen[0].header("content-length"), Some("7"));
    assert_eq!(seen[0].header("content-type"), Some("application/json"));
    assert_eq!(seen[1].body.as_deref(), Some(b"hello world".as_slice()));
    assert_eq!(seen[1].header("content-length"), Some("11"));
    assert_eq!(seen[2].body, None, "a GET without a body gets none");
    assert_eq!(seen[2].header("content-length"), None);
    assert_eq!(seen[3].body.as_deref(), Some(b"{}".as_slice()));
}

#[test]
fn the_sdk_client_works_through_the_proxy() {
    let upstream = Upstream::start(sdk_routes);
    let proxy = start(&upstream);
    let base = proxy.base_url();

    let project = client::get_project(&base, TOKEN, false, ID).expect("get through the proxy");
    assert_eq!(
        (project.id.as_str(), project.name.as_str()),
        (ID, "Synthetic cave")
    );
    let file = tempfile::NamedTempFile::new().expect("a temp file");
    std::fs::write(file.path(), stalx()).expect("write the upload");
    let uploaded = client::upload_stalx(&base, TOKEN, false, ID, file.path(), "Proxy test")
        .expect("upload through the proxy");
    assert_eq!(uploaded.latest_commit.map(|c| c.id).as_deref(), Some(SHA));
    let native = client::download_stalx(&base, TOKEN, false, ID).expect("download");
    assert_eq!(std::fs::read(native.path()).expect("the download"), stalx());
    client::release_lock(&base, TOKEN, false, ID).expect("release through the proxy");

    settle(&proxy);
    let paths = [
        format!("GET /api/v2/projects/{ID}/"),
        format!("PUT /api/v2/projects/{ID}/upload/stalagmight/"),
        format!("GET /api/v2/projects/{ID}/download/stalagmight/"),
        format!("POST /api/v2/projects/{ID}/release/"),
    ];
    assert_eq!(upstream.lines(), paths);
    let seen = upstream.seen();
    let upload = seen[1].body.as_deref().unwrap_or_default();
    assert!(upload.windows(stalx().len()).any(|w| w == stalx()));
    assert_eq!(
        seen[0].header("authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    let log = proxy.take_log();
    assert_eq!(log.len(), 4);
    assert!(log
        .iter()
        .all(|e| e.delivered && e.fault.is_none() && e.upstream_status == Some(200)));

    // The client parses chunked and close-delimited replies relayed by the proxy.
    let text = project_json().to_string().into_bytes();
    let (head, tail) = text.split_at(40);
    for canned in [
        Canned::Chunked(vec![head.to_vec(), tail.to_vec()]),
        Canned::CloseDelimited(text.clone()),
    ] {
        let upstream = Upstream::canned(canned);
        let proxy = start(&upstream);
        let project = client::get_project(&proxy.base_url(), TOKEN, false, ID)
            .expect("a re-framed reply parses");
        assert_eq!(project.id, ID);
    }
}

#[test]
fn status_answers_without_forwarding() {
    let upstream = Upstream::start(sdk_routes);
    let proxy = start(&upstream);
    proxy.arm(rule(None, "/user/", FaultKind::Status(503), 1));
    let got = ask(&proxy, &with_body("POST", "/api/v2/user/auth-token/", "{}"));
    assert_eq!(got.status, 503);
    assert_eq!(got.content_length(), Some(got.body.len()));
    let body: Value = serde_json::from_slice(&got.body).expect("a JSON body");
    assert!(body.get("detail").is_some_and(Value::is_string), "{body}");

    proxy.arm(rule(Some("POST"), "/release/", FaultKind::Status(409), 1));
    let err = client::release_lock(&proxy.base_url(), TOKEN, false, ID).expect_err("a 409");
    assert!(
        matches!(&err, SpeleoDbError::Status { status: 409, message } if message.contains("409")),
        "{err:?}"
    );
    settle(&proxy);
    assert!(upstream.seen().is_empty(), "upstream never saw either");
    assert_eq!(
        proxy.take_log(),
        [
            event(
                "POST",
                "/api/v2/user/auth-token/",
                None,
                Some("Status(503)"),
                true
            ),
            event(
                "POST",
                &format!("/api/v2/projects/{ID}/release/"),
                None,
                Some("Status(409)"),
                true
            ),
        ]
    );
}

#[test]
fn status_after_lets_upstream_apply_then_answers_the_status() {
    let upstream = Upstream::start(sdk_routes);
    let proxy = start(&upstream);
    proxy.arm(rule(
        Some("POST"),
        "/release/",
        FaultKind::StatusAfter(502),
        1,
    ));
    let err = client::release_lock(&proxy.base_url(), TOKEN, false, ID).expect_err("a 502");
    assert!(
        matches!(err, SpeleoDbError::Status { status: 502, .. }),
        "{err:?}"
    );
    settle(&proxy);
    assert_eq!(
        upstream.lines(),
        [format!("POST /api/v2/projects/{ID}/release/")]
    );
    assert_eq!(
        proxy.take_log(),
        [event(
            "POST",
            &format!("/api/v2/projects/{ID}/release/"),
            Some(200),
            Some("StatusAfter(502)"),
            true
        )]
    );
}

#[test]
fn drop_after_forward_and_truncate_body_fail_the_client_after_upstream_applied() {
    let upstream = Upstream::start(sdk_routes);
    let proxy = start(&upstream);
    let base = proxy.base_url();
    proxy.arm(rule(None, "/release/", FaultKind::DropAfterForward, 1));
    transport(client::release_lock(&base, TOKEN, false, ID));
    // A download is a safe read the client retries twice: three cuts fail it.
    proxy.arm(rule(None, "/download/", FaultKind::TruncateBody, 3));
    transport(client::download_stalx(&base, TOKEN, false, ID));
    settle(&proxy);
    assert_eq!(upstream.seen().len(), 4, "upstream applied every one");
    let log = proxy.take_log();
    assert_eq!(log.len(), 4);
    assert_eq!(
        log[0],
        event(
            "POST",
            &format!("/api/v2/projects/{ID}/release/"),
            Some(200),
            Some("DropAfterForward"),
            false
        )
    );
    for cut in &log[1..] {
        assert_eq!(cut.fault.as_deref(), Some("TruncateBody"));
        assert_eq!((cut.upstream_status, cut.delivered), (Some(200), false));
    }

    // On the wire: the full Content-Length, half the body.
    proxy.arm(rule(None, "/download/", FaultKind::TruncateBody, 1));
    let got = ask(
        &proxy,
        &get(&format!("/api/v2/projects/{ID}/download/stalagmight/")),
    );
    assert_eq!(got.status, 200);
    assert_eq!(got.content_length(), Some(stalx().len()));
    assert_eq!(got.body, stalx()[..stalx().len() / 2]);
}

#[test]
fn corrupt_and_scramble_keep_their_length_semantics() {
    let original = stalx();
    let upstream = Upstream::canned(Canned::Length(200, original.clone()));
    let proxy = start(&upstream);
    proxy.arm(rule(None, "/a/", FaultKind::CorruptBody, 1));
    let got = ask(&proxy, &get("/a/"));
    assert_eq!(got.content_length(), Some(original.len() / 2));
    assert_eq!(got.body, original[..original.len() / 2]);
    proxy.arm(rule(None, "/a/", FaultKind::ScrambleBody, 1));
    let got = ask(&proxy, &get("/a/"));
    assert_eq!(got.content_length(), Some(original.len()));
    assert_eq!(got.body.len(), original.len());
    assert_eq!(got.body[..100], original[..100], "the first 100 bytes stay");
    assert!(got.body[100..]
        .iter()
        .zip(&original[100..])
        .all(|(got, sent)| *got == sent ^ 0x5A));

    // A chunked reply is cut on its decoded body and re-framed.
    let chunks = vec![b"0123456789".to_vec(), b"abcdefghij".to_vec()];
    let upstream = Upstream::canned(Canned::Chunked(chunks));
    let proxy = start(&upstream);
    proxy.arm(rule(None, "/", FaultKind::CorruptBody, 1));
    let got = ask(&proxy, &get("/b/"));
    assert_eq!(got.all("transfer-encoding"), Vec::<&str>::new());
    assert_eq!(
        (got.content_length(), got.body.as_slice()),
        (Some(10), b"0123456789".as_slice())
    );

    // Through the client: a cut or scrambled JSON reply does not parse, a
    // scrambled download arrives scrambled.
    let upstream = Upstream::start(sdk_routes);
    let proxy = start(&upstream);
    let base = proxy.base_url();
    for kind in [FaultKind::CorruptBody, FaultKind::ScrambleBody] {
        proxy.arm(rule(Some("GET"), ID, kind, 1));
        let err = client::get_project(&base, TOKEN, false, ID).expect_err("a garbled reply");
        assert!(matches!(err, SpeleoDbError::Parse(_)), "{kind:?}: {err:?}");
    }
    proxy.arm(rule(None, "/download/", FaultKind::ScrambleBody, 1));
    let native = client::download_stalx(&base, TOKEN, false, ID).expect("a full-length download");
    let mut expected = stalx();
    expected[100..].iter_mut().for_each(|byte| *byte ^= 0x5A);
    assert_eq!(
        std::fs::read(native.path()).expect("the download"),
        expected
    );
}

#[test]
fn refuse_resets_the_connection_and_the_client_reports_refused() {
    let upstream = Upstream::start(sdk_routes);
    let proxy = start(&upstream);
    let base = proxy.base_url();

    proxy.arm(rule(None, "/raw/", FaultKind::Refuse, 1));
    let err = exchange(proxy.addr, &get("/raw/"), WAIT).expect_err("a reset, not a reply");
    assert_eq!(err.kind(), io::ErrorKind::ConnectionReset, "{err:?}");

    // A read retries twice: three refusals fail it outright.
    proxy.arm(rule(Some("GET"), ID, FaultKind::Refuse, 3));
    let err = client::get_project(&base, TOKEN, false, ID).expect_err("refused three times");
    println!("FZ-SDB proxy: get_project under Refuse x3 -> {err:?}");
    assert!(
        matches!(
            err,
            SpeleoDbError::Transport {
                cause: TransportCause::Refused,
                ..
            }
        ),
        "{err:?}"
    );
    proxy.arm(rule(Some("POST"), "/release/", FaultKind::Refuse, 1));
    let err = client::release_lock(&base, TOKEN, false, ID).expect_err("a refused mutation");
    println!("FZ-SDB proxy: release_lock under Refuse x1 -> {err:?}");
    assert_eq!(transport(Err::<(), _>(err)), TransportCause::Refused);
    settle(&proxy);
    assert!(
        upstream.seen().is_empty(),
        "nothing refused reaches upstream"
    );

    // Two refusals are absorbed by the retries: the rule is spent on the third try.
    proxy.arm(rule(Some("GET"), ID, FaultKind::Refuse, 2));
    client::get_project(&base, TOKEN, false, ID).expect("the third try passes");
    settle(&proxy);
    assert_eq!(upstream.lines(), [format!("GET /api/v2/projects/{ID}/")]);

    let log = proxy.take_log();
    assert_eq!(log.len(), 1 + 3 + 1 + 3);
    let project = format!("/api/v2/projects/{ID}/");
    let refused = |path: &str, method: &str| event(method, path, None, Some("Refuse"), false);
    assert_eq!(log[0], refused("/raw/", "GET"));
    assert!(log[1..4].iter().all(|e| *e == refused(&project, "GET")));
    assert_eq!(log[4], refused(&format!("{project}release/"), "POST"));
    assert!(log[5..7].iter().all(|e| *e == refused(&project, "GET")));
    assert_eq!(log[7], event("GET", &project, Some(200), None, true));
}

#[test]
fn hold_before_parks_the_request_until_released_while_others_pass() {
    let upstream = Upstream::start(sdk_routes);
    let proxy = start(&upstream);
    proxy.arm(rule(Some("POST"), "/release/", FaultKind::HoldBefore, 1));
    let base = proxy.base_url();
    let held = thread::spawn(move || client::release_lock(&base, TOKEN, false, ID));
    assert!(proxy.wait_held(WAIT), "the release is parked");
    assert!(
        upstream.seen().is_empty(),
        "a held request has not reached upstream"
    );

    let project = client::get_project(&proxy.base_url(), TOKEN, false, ID)
        .expect("a GET passes while the release is parked");
    assert_eq!(project.id, ID);
    assert_eq!(upstream.lines(), [format!("GET /api/v2/projects/{ID}/")]);
    assert!(!held.is_finished(), "the parked release has no reply yet");

    proxy.release_holds();
    held.join()
        .expect("the release thread")
        .expect("the release completes once let go");
    settle(&proxy);
    assert_eq!(
        upstream.lines(),
        [
            format!("GET /api/v2/projects/{ID}/"),
            format!("POST /api/v2/projects/{ID}/release/"),
        ]
    );
    let log = proxy.take_log();
    assert_eq!(log.len(), 2);
    assert_eq!(
        unheld(log[0].clone()),
        event(
            "POST",
            &format!("/api/v2/projects/{ID}/release/"),
            Some(200),
            Some("HoldBefore"),
            true
        ),
        "logged in arrival order"
    );
    assert_eq!(
        log[1],
        event(
            "GET",
            &format!("/api/v2/projects/{ID}/"),
            Some(200),
            None,
            true
        )
    );
}

#[test]
fn hold_reply_parks_the_reply_after_upstream_applied() {
    let upstream = Upstream::start(sdk_routes);
    let proxy = start(&upstream);
    proxy.arm(rule(Some("POST"), "/release/", FaultKind::HoldReply, 1));
    let base = proxy.base_url();
    let held = thread::spawn(move || client::release_lock(&base, TOKEN, false, ID));
    assert!(proxy.wait_held(WAIT), "the reply is parked");
    assert_eq!(
        upstream.lines(),
        [format!("POST /api/v2/projects/{ID}/release/")],
        "upstream applied it before the park"
    );
    assert!(!held.is_finished(), "the client has no reply yet");
    proxy.release_holds();
    held.join()
        .expect("the release thread")
        .expect("the reply arrives once let go");
    settle(&proxy);
    let log = proxy.take_log();
    assert_eq!(log.len(), 1);
    assert_eq!(
        unheld(log[0].clone()),
        event(
            "POST",
            &format!("/api/v2/projects/{ID}/release/"),
            Some(200),
            Some("HoldReply"),
            true
        )
    );
}

#[test]
fn abort_holds_ends_parked_requests_as_if_their_client_died() {
    let upstream = Upstream::start(sdk_routes);
    let proxy = start(&upstream);
    let release = format!("/api/v2/projects/{ID}/release/");
    for kind in [FaultKind::HoldBefore, FaultKind::HoldReply] {
        proxy.arm(rule(Some("POST"), "/release/", kind, 1));
        let base = proxy.base_url();
        let held = thread::spawn(move || client::release_lock(&base, TOKEN, false, ID));
        assert!(proxy.wait_held(WAIT), "{kind:?} parked");
        proxy.abort_holds();
        transport(held.join().expect("the release thread"));
        settle(&proxy);
    }
    assert_eq!(
        upstream.lines(),
        [format!("POST {release}")],
        "only the HoldReply request reached upstream"
    );
    // Requests after an abort are unaffected.
    client::release_lock(&proxy.base_url(), TOKEN, false, ID).expect("a plain release");
    settle(&proxy);
    let log = proxy.take_log();
    assert_eq!(log.len(), 3);
    assert_eq!(
        unheld(log[0].clone()),
        event("POST", &release, None, Some("HoldBefore aborted"), false)
    );
    assert_eq!(
        unheld(log[1].clone()),
        event(
            "POST",
            &release,
            Some(200),
            Some("HoldReply aborted"),
            false
        )
    );
    assert_eq!(log[2], event("POST", &release, Some(200), None, true));
}

#[test]
fn clear_and_drop_release_parked_requests() {
    let upstream = Upstream::canned(Canned::json(&json!({"ok": true})));
    let proxy = start(&upstream);
    proxy.arm(rule(None, "/projects/", FaultKind::HoldBefore, 5));
    let addr = proxy.addr;
    let held = thread::spawn(move || exchange(addr, &get("/api/v2/projects/"), WAIT));
    assert!(proxy.wait_held(WAIT));
    proxy.clear();
    let raw = held
        .join()
        .expect("the client thread")
        .expect("a reply after the clear");
    assert_eq!(Got::parse(&raw).status, 200);
    assert_eq!(
        ask(&proxy, &get("/api/v2/projects/")).status,
        200,
        "the clear dropped the rule: no second park"
    );
    settle(&proxy);
    let log = proxy.take_log();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].fault.as_deref(), Some("HoldBefore"));
    assert!(log[0].delivered && log[0].held_ms.is_some());
    assert_eq!(log[1].fault, None);

    proxy.arm(rule(None, "/projects/", FaultKind::HoldReply, 1));
    let held = thread::spawn(move || exchange(addr, &get("/api/v2/projects/"), WAIT));
    assert!(proxy.wait_held(WAIT));
    drop(proxy);
    let raw = held
        .join()
        .expect("the client thread")
        .expect("a reply after the drop");
    assert_eq!(Got::parse(&raw).status, 200);
}

#[test]
fn a_held_request_whose_client_gave_up_still_reaches_upstream() {
    let upstream = Upstream::canned(Canned::json(&json!({"ok": true})));
    let proxy = start(&upstream);
    proxy.arm(rule(None, "/acquire/", FaultKind::HoldBefore, 1));
    let mut stream = TcpStream::connect(proxy.addr).expect("connect");
    stream
        .write_all(&with_body("POST", "/api/v2/projects/p1/acquire/", ""))
        .expect("send");
    assert!(proxy.wait_held(WAIT));
    drop(stream);
    proxy.release_holds();
    settle(&proxy);
    assert_eq!(upstream.lines(), ["POST /api/v2/projects/p1/acquire/"]);
    let log = proxy.take_log();
    assert_eq!(
        unheld(log[0].clone()),
        event(
            "POST",
            "/api/v2/projects/p1/acquire/",
            Some(200),
            Some("HoldBefore"),
            false
        )
    );
}

#[test]
fn rules_fire_in_arm_order_and_expire_after_their_times() {
    let upstream = Upstream::canned(Canned::json(&json!({"ok": true})));
    let proxy = start(&upstream);
    proxy.arm(rule(Some("PUT"), "/p1/", FaultKind::Status(409), 1));
    proxy.arm(rule(None, "/p1/", FaultKind::Status(503), 2));
    proxy.arm(rule(None, "/p1/", FaultKind::Status(500), 1));
    proxy.arm(rule(None, "/p1/", FaultKind::Status(418), 0));

    let status = |request: &[u8]| ask(&proxy, request).status;
    let p1 = "/api/v2/projects/p1/";
    assert_eq!(status(&get(p1)), 503, "the PUT rule skips a GET");
    assert_eq!(
        status(&get("/api/v2/projects/p2/")),
        200,
        "no rule matches p2"
    );
    assert_eq!(
        status(&with_body("PUT", p1, "")),
        409,
        "the first match fires"
    );
    assert_eq!(status(&get(p1)), 503, "the 503 rule's second time");
    assert_eq!(status(&get(p1)), 500, "the 503 rule is spent");
    assert_eq!(
        status(&get(p1)),
        200,
        "a rule armed with times 0 is not kept"
    );
    proxy.arm(rule(None, "/", FaultKind::Status(503), 5));
    proxy.clear();
    assert_eq!(status(&get(p1)), 200, "the clear dropped every rule");

    settle(&proxy);
    let faults: Vec<Option<String>> = proxy.take_log().into_iter().map(|e| e.fault).collect();
    let expected = [
        Some("Status(503)"),
        None,
        Some("Status(409)"),
        Some("Status(503)"),
        Some("Status(500)"),
        None,
        None,
    ];
    assert_eq!(faults, expected.map(|f| f.map(str::to_owned)));
}

#[test]
fn drop_before_forward_cuts_the_upload_and_never_forwards() {
    let upstream = Upstream::canned(Canned::json(&json!({"ok": true})));
    let proxy = start(&upstream);
    proxy.arm(rule(None, "/upload/", FaultKind::DropBeforeForward, 1));
    let big = "x".repeat(256 * 1024);
    let got = exchange(
        proxy.addr,
        &with_body("PUT", "/api/v2/projects/p1/upload/stalagmight/", &big),
        WAIT,
    );
    assert!(
        !matches!(&got, Ok(bytes) if !bytes.is_empty()),
        "the client got an I/O error or EOF, not a reply"
    );
    settle(&proxy);
    assert!(upstream.seen().is_empty(), "upstream never saw it");
    assert_eq!(
        proxy.take_log(),
        [event(
            "PUT",
            "/api/v2/projects/p1/upload/stalagmight/",
            None,
            Some("DropBeforeForward"),
            false
        )]
    );
}

#[test]
fn hang_then_forward_times_the_client_out_then_applies_late() {
    let upstream = Upstream::canned(Canned::json(&json!({"ok": true})));
    let proxy = start(&upstream);
    let hang = Duration::from_millis(1000);
    proxy.arm(rule(None, "/acquire/", FaultKind::HangThenForward(hang), 1));
    let sent = Instant::now();
    let got = exchange(
        proxy.addr,
        &with_body("POST", "/api/v2/projects/p1/acquire/", ""),
        Duration::from_millis(300),
    );
    let err = got.expect_err("the client's read timed out");
    assert!(
        matches!(
            err.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ),
        "{err:?}"
    );
    assert!(
        upstream.seen().is_empty(),
        "nothing forwarded before the hang ends"
    );
    settle(&proxy);
    assert!(sent.elapsed() >= hang);
    assert_eq!(upstream.seen().len(), 1, "upstream applied it late");
    assert_eq!(
        proxy.take_log(),
        [event(
            "POST",
            "/api/v2/projects/p1/acquire/",
            Some(200),
            Some("HangThenForward(1s)"),
            false
        )]
    );
}

#[test]
fn latency_delays_then_forwards() {
    let upstream = Upstream::canned(Canned::json(&json!({"ok": true})));
    let proxy = start(&upstream);
    let delay = Duration::from_millis(300);
    proxy.arm(rule(None, "/user/", FaultKind::Latency(delay), 1));
    let sent = Instant::now();
    let got = ask(&proxy, &get("/api/v2/user/"));
    assert!(sent.elapsed() >= delay);
    assert_eq!(
        (got.status, got.body.as_slice()),
        (200, br#"{"ok":true}"#.as_slice())
    );
}

#[test]
fn a_non_loopback_upstream_is_refused() {
    let err = FaultProxy::start(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 80)))
        .err()
        .expect("a non-loopback upstream is refused");
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn dropping_the_proxy_stops_accepting() {
    let upstream = Upstream::canned(Canned::json(&json!({"ok": true})));
    let proxy = start(&upstream);
    let addr = proxy.addr;
    drop(proxy);
    assert!(TcpStream::connect(addr).is_err(), "the listener closed");
}
