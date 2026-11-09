//! [FZ-SDB] A fault-injecting HTTP/1.1 reverse proxy.
//!
//! It sits between the app's SpeleoDB client and a SpeleoDB server, both on
//! loopback, and turns an armed [`FaultRule`] into what a flaky network or
//! server does: an error status the server never saw, a lost confirmation of a
//! request the server applied, a dropped upload, a lost, cut or garbled reply,
//! a reset connection, or a request held back (before or after the server
//! applies it) until the fuzzer lets it go or aborts it.
//!
//! One request per client connection, one thread per connection. Every reply
//! carries `Connection: close` and the socket closes after it. Upstream gets a
//! fresh connection per request, with `Connection: close`: the request body
//! (Content-Length or chunked) is read whole and forwarded with a
//! Content-Length, every other request header line (Host included) unchanged.
//! The upstream reply (Content-Length, chunked or close-delimited) is read
//! whole and relayed with a Content-Length.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How long one upstream exchange may stall: a dump download or an upload.
const UPSTREAM_TIMEOUT: Duration = Duration::from_mins(3);
/// How long the proxy waits on a client that stops sending or reading.
const CLIENT_TIMEOUT: Duration = Duration::from_mins(3);
/// How long dialling upstream may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a hold parks a request that nothing releases.
const HOLD_TIMEOUT: Duration = Duration::from_mins(3);
/// The pause between two peeks at a request head that has not fully arrived.
const PEEK_PAUSE: Duration = Duration::from_millis(2);
/// The largest request head the proxy accepts.
const HEAD_LIMIT: usize = 64 * 1024;
/// How long `Refuse` waits for the request's last byte to arrive.
const REFUSE_WAIT: Duration = Duration::from_secs(2);
/// The reply head `Refuse` writes before the reset: a body is promised, so the
/// client is reading one when the reset lands.
const REFUSE_HEAD: &[u8] = b"HTTP/1.1 502 Fault Proxy\r\nContent-Type: application/json\r\n\
                             Content-Length: 64\r\nConnection: close\r\n\r\n";
/// `ScrambleBody` keeps this many leading bytes (a SQLite header) intact.
const SCRAMBLE_KEEP: usize = 100;
/// `ScrambleBody` XORs every later byte with this.
const SCRAMBLE_MASK: u8 = 0x5A;
/// Request headers the proxy rewrites on the way upstream.
const REQUEST_REFRAMED: [&str; 5] = [
    "connection",
    "keep-alive",
    "content-length",
    "transfer-encoding",
    "expect",
];
/// Reply headers the proxy rewrites on the way to the client.
const REPLY_REFRAMED: [&str; 4] = [
    "connection",
    "keep-alive",
    "content-length",
    "transfer-encoding",
];
/// Headers dropped from a reply that carries no body (it keeps its own
/// Content-Length, which describes the body it would have had).
const HOP_BY_HOP: [&str; 2] = ["connection", "keep-alive"];

/// What an armed rule does to the request it fires on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FaultKind {
    /// Answer this status with a small `{"detail": ...}` JSON body, never
    /// forwarding: the server never sees the request.
    Status(u16),
    /// Forward and let upstream apply it, then answer this status instead: the
    /// confirmation is lost.
    StatusAfter(u16),
    /// Sleep, then forward normally.
    Latency(Duration),
    /// Read the head and about half the body, then close the client socket
    /// without forwarding: the upload drops midway.
    DropBeforeForward,
    /// Forward fully, then close the client socket without writing anything:
    /// the reply is lost.
    DropAfterForward,
    /// Forward, then write upstream's status line and headers (stating the full
    /// Content-Length) and only the first half of the body, then close.
    TruncateBody,
    /// Forward, then answer upstream's status with the body cut to half its
    /// length and a Content-Length that matches the cut: malformed JSON or ZIP.
    CorruptBody,
    /// Forward, then answer upstream's status with a body of the same length
    /// whose bytes from offset 100 on are XORed with 0x5A.
    ScrambleBody,
    /// Hold the client socket open without answering for this long (the client
    /// times out), close it, THEN forward: the server applies it late.
    HangThenForward(Duration),
    /// Reset the connection: the client's read fails with `ConnectionReset`,
    /// which the app's client reports as `TransportCause::Refused`. Never
    /// forwarded. The proxy takes the request but its last byte, writes a 502
    /// head that promises a body, and closes with that byte unread, so the
    /// kernel sends a RST. The head is needed: the client reports a failure
    /// before any reply head (a send error) as `TransportCause::Unknown`, and
    /// only a reset while it reads a body as `Refused`.
    Refuse,
    /// Read the full request, park until released, then forward and relay
    /// normally. An aborted park closes the client socket without forwarding.
    HoldBefore,
    /// Forward at once and read the full upstream reply, park until released,
    /// then write the reply. An aborted park closes the client socket without
    /// writing it.
    HoldReply,
}

/// One armed fault. The first armed rule matching a request fires.
#[derive(Debug, Clone)]
pub(super) struct FaultRule {
    /// The request method it fires on (`Some("PUT")`); `None` matches any.
    pub(super) method: Option<&'static str>,
    /// A substring of the request target, e.g. `"/upload/stalagmight/"`.
    pub(super) path_contains: String,
    pub(super) kind: FaultKind,
    /// How many matching requests it fires on before it is removed.
    pub(super) times: u32,
}

impl FaultRule {
    fn matches(&self, method: &str, path: &str) -> bool {
        self.method.is_none_or(|m| m.eq_ignore_ascii_case(method))
            && path.contains(self.path_contains.as_str())
    }
}

/// One client request the proxy handled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProxyEvent {
    pub(super) method: String,
    /// The request target as the client sent it (path and query).
    pub(super) path: String,
    /// What upstream answered; `None` when the request was not forwarded or the
    /// forward failed.
    pub(super) upstream_status: Option<u16>,
    /// The `Debug` form of the fault that fired (`"StatusAfter(502)"`), with
    /// `" aborted"` appended for a hold that [`FaultProxy::abort_holds`] ended.
    pub(super) fault: Option<String>,
    /// How long a hold parked it.
    pub(super) held_ms: Option<u64>,
    /// The full reply was written to a client that was still connected,
    /// without an I/O error.
    pub(super) delivered: bool,
}

/// The proxy. Dropping it stops the accept thread and releases every parked
/// request; a request already being handled runs to its end on its own thread
/// (a pending `HangThenForward` or a released hold still reaches upstream),
/// which never keeps the process from exiting.
pub(super) struct FaultProxy {
    addr: SocketAddr,
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
}

impl FaultProxy {
    /// Bind `127.0.0.1:0` and forward to `upstream`, which must be loopback.
    pub(super) fn start(upstream: SocketAddr) -> io::Result<Self> {
        if !upstream.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("the fault proxy forwards only to loopback, not to {upstream}"),
            ));
        }
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            upstream,
            stop: AtomicBool::new(false),
            rules: Mutex::new(Vec::new()),
            log: Mutex::new(Vec::new()),
            in_flight: Mutex::new(0),
            idle: Condvar::new(),
            holds: Mutex::new(Holds::default()),
            hold_change: Condvar::new(),
        });
        let accept = thread::Builder::new()
            .name("fz-sdb-proxy-accept".to_owned())
            .spawn({
                let shared = Arc::clone(&shared);
                move || accept_loop(&listener, &shared)
            })?;
        Ok(Self {
            addr,
            shared,
            accept: Some(accept),
        })
    }

    /// `"http://127.0.0.1:<port>/"`.
    pub(super) fn base_url(&self) -> String {
        format!("http://{}/", self.addr)
    }

    /// Append `rule` (a rule with `times == 0` is exhausted already and is not
    /// kept). The first armed rule matching a request fires and its `times`
    /// decrements; a rule whose `times` reaches 0 is removed.
    pub(super) fn arm(&self, rule: FaultRule) {
        if rule.times > 0 {
            lock(&self.shared.rules).push(rule);
        }
    }

    /// Drop every armed rule and release every parked request.
    pub(super) fn clear(&self) {
        lock(&self.shared.rules).clear();
        self.shared.decide(Verdict::Release);
    }

    /// Every request handled to its end since the last take, in arrival order.
    /// A request still in flight (parked, say) is not in it yet: it comes with a
    /// later take, so call [`Self::quiesce`] first for a complete log.
    pub(super) fn take_log(&self) -> Vec<ProxyEvent> {
        lock(&self.shared.log)
            .drain(..)
            .map(|(_, event)| event)
            .collect()
    }

    /// Wait until no connection is in flight (a `HangThenForward`'s late
    /// forward included). `false` when `max` ran out first.
    pub(super) fn quiesce(&self, max: Duration) -> bool {
        wait_until(&self.shared.in_flight, &self.shared.idle, max, |n| *n == 0)
    }

    /// Wait until at least one request is parked by a hold and not yet released
    /// or aborted. `false` when `max` ran out first.
    pub(super) fn wait_held(&self, max: Duration) -> bool {
        wait_until(
            &self.shared.holds,
            &self.shared.hold_change,
            max,
            Holds::any_waiting,
        )
    }

    /// Let every currently parked request continue.
    pub(super) fn release_holds(&self) {
        self.shared.decide(Verdict::Release);
    }

    /// End every currently parked request as if its client died: a
    /// `HoldBefore` request is never forwarded, a `HoldReply` request's reply is
    /// discarded, and either client socket is closed without a reply. Requests
    /// parked later are unaffected.
    pub(super) fn abort_holds(&self) {
        self.shared.decide(Verdict::Abort);
    }
}

impl Drop for FaultProxy {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.decide(Verdict::Release);
        // The accept loop blocks in `accept`; a throwaway connection wakes it to
        // see the stop flag. Joined only when the wake landed, so a failed wake
        // can never hang the drop.
        let woke = TcpStream::connect_timeout(&self.addr, Duration::from_secs(1)).is_ok();
        if let Some(accept) = self.accept.take().filter(|_| woke) {
            let _ = accept.join();
        }
    }
}

/// How a park ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Release,
    Abort,
}

/// The parked requests: a ticket each, with its verdict once one came.
#[derive(Default)]
struct Holds {
    next_ticket: u64,
    parked: Vec<(u64, Option<Verdict>)>,
}

impl Holds {
    fn any_waiting(&self) -> bool {
        self.parked.iter().any(|(_, verdict)| verdict.is_none())
    }
}

/// State the accept thread and the request threads share with the handle.
struct Shared {
    upstream: SocketAddr,
    stop: AtomicBool,
    rules: Mutex<Vec<FaultRule>>,
    /// Finished requests keyed by their arrival number, in arrival order.
    log: Mutex<Vec<(u64, ProxyEvent)>>,
    in_flight: Mutex<usize>,
    /// Signalled when `in_flight` drops to 0.
    idle: Condvar,
    holds: Mutex<Holds>,
    /// Signalled when a request parks and when parked requests get a verdict.
    hold_change: Condvar,
}

impl Shared {
    /// The kind of the first armed rule matching the request, consuming one of
    /// its `times`.
    fn take_rule(&self, method: &str, path: &str) -> Option<FaultKind> {
        let mut rules = lock(&self.rules);
        let at = rules.iter().position(|rule| rule.matches(method, path))?;
        let rule = &mut rules[at];
        let kind = rule.kind;
        rule.times = rule.times.saturating_sub(1);
        if rule.times == 0 {
            rules.remove(at);
        }
        Some(kind)
    }

    /// Log a finished request at its arrival position.
    fn record(&self, seq: u64, event: ProxyEvent) {
        let mut log = lock(&self.log);
        let at = log.partition_point(|(earlier, _)| *earlier < seq);
        log.insert(at, (seq, event));
    }

    /// Park the calling request until a verdict comes, the proxy is dropped
    /// (a release) or [`HOLD_TIMEOUT`] passes (a release). Returns how long it
    /// was parked and how the park ended.
    fn park(&self) -> (Duration, Verdict) {
        let started = Instant::now();
        let deadline = started + HOLD_TIMEOUT;
        let mut holds = lock(&self.holds);
        let ticket = holds.next_ticket;
        holds.next_ticket += 1;
        holds.parked.push((ticket, None));
        self.hold_change.notify_all();
        let verdict = loop {
            let decided = holds
                .parked
                .iter()
                .find(|(parked, _)| *parked == ticket)
                .and_then(|(_, verdict)| *verdict);
            if let Some(verdict) = decided {
                break verdict;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if self.stop.load(Ordering::SeqCst) || left.is_zero() {
                break Verdict::Release;
            }
            holds = self
                .hold_change
                .wait_timeout(holds, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        };
        holds.parked.retain(|(parked, _)| *parked != ticket);
        (started.elapsed(), verdict)
    }

    /// Give every parked request that has none yet this verdict.
    fn decide(&self, verdict: Verdict) {
        let mut holds = lock(&self.holds);
        for (_, decided) in &mut holds.parked {
            if decided.is_none() {
                *decided = Some(verdict);
            }
        }
        self.hold_change.notify_all();
    }
}

/// Counts one connection as in flight for as long as it lives.
struct InFlight(Arc<Shared>);

impl InFlight {
    fn enter(shared: &Arc<Shared>) -> Self {
        *lock(&shared.in_flight) += 1;
        Self(Arc::clone(shared))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let mut in_flight = lock(&self.0.in_flight);
        *in_flight = in_flight.saturating_sub(1);
        if *in_flight == 0 {
            self.0.idle.notify_all();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Wait on `changed` until `done` holds for the guarded value; `false` when
/// `max` ran out first.
fn wait_until<T>(
    mutex: &Mutex<T>,
    changed: &Condvar,
    max: Duration,
    done: impl Fn(&T) -> bool,
) -> bool {
    let deadline = Instant::now() + max;
    let mut guard = lock(mutex);
    while !done(&guard) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        guard = changed
            .wait_timeout(guard, left)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
    true
}

fn millis(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_owned())
}

/// Accept connections until stopped, one thread per connection. A connection
/// counts as in flight from its accept on, so a `quiesce` racing the handler's
/// start cannot miss it.
fn accept_loop(listener: &TcpListener, shared: &Arc<Shared>) {
    for (seq, conn) in (0_u64..).zip(listener.incoming()) {
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        let Ok(stream) = conn else {
            // A failed accept (out of descriptors, say) must not spin.
            thread::sleep(Duration::from_millis(10));
            continue;
        };
        let guard = InFlight::enter(shared);
        // A failed spawn drops the closure, and with it the guard and the stream.
        let _ = thread::Builder::new()
            .name(format!("fz-sdb-proxy-{seq}"))
            .spawn(move || serve(&guard.0, stream, seq));
    }
}

/// How one request ended, for its [`ProxyEvent`].
#[derive(Default)]
struct Outcome {
    upstream_status: Option<u16>,
    held: Option<Duration>,
    aborted: bool,
    delivered: bool,
}

impl Outcome {
    /// Park the request; `true` when the park was aborted.
    fn park(&mut self, shared: &Shared) -> bool {
        let (held, verdict) = shared.park();
        self.held = Some(held);
        self.aborted = verdict == Verdict::Abort;
        self.aborted
    }
}

/// Handle one connection: match its head against the rules, apply the fault,
/// log the event. The head is matched on a peek, so a `Refuse` can leave
/// request bytes unread.
fn serve(shared: &Shared, stream: TcpStream, seq: u64) {
    let _ = stream.set_read_timeout(Some(CLIENT_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CLIENT_TIMEOUT));
    let Ok(head) = peek_head(&stream) else {
        let _ = stream.shutdown(Shutdown::Both);
        return;
    };
    let fault = shared.take_rule(&head.method, &head.path);
    let outcome = if fault == Some(FaultKind::Refuse) {
        refuse(stream, &head);
        Outcome::default()
    } else {
        Client::open(stream, head.len).map_or_else(
            |_| Outcome::default(),
            |client| handle(shared, &head, fault, client),
        )
    };
    let fault = fault.map(|kind| {
        if outcome.aborted {
            format!("{kind:?} aborted")
        } else {
            format!("{kind:?}")
        }
    });
    shared.record(
        seq,
        ProxyEvent {
            method: head.method,
            path: head.path,
            upstream_status: outcome.upstream_status,
            fault,
            held_ms: outcome.held.map(millis),
            delivered: outcome.delivered,
        },
    );
}

/// `Refuse`: take the request but its last byte (read unbuffered, so nothing
/// past it is consumed), wait for that byte, so the client is done sending and
/// reads the reply, write [`REFUSE_HEAD`], and close with the byte unread.
/// Closing a socket whose receive queue still holds data makes the kernel send
/// a RST instead of a FIN: the client's body read fails with ECONNRESET. A
/// chunked request keeps its whole body unread (its end is not known up
/// front), so a chunked upload larger than the socket buffers may instead fail
/// while still sending.
fn refuse(mut stream: TcpStream, head: &RequestHead) {
    let body = if is_chunked(&head.headers) {
        0
    } else {
        content_length(&head.headers).unwrap_or(0)
    };
    let all_but_last = head.len.saturating_add(body).saturating_sub(1);
    send_continue(&stream, head);
    let _ = io::copy(&mut (&stream).take(all_but_last as u64), &mut io::sink());
    let _ = stream.set_read_timeout(Some(REFUSE_WAIT));
    let _ = stream.peek(&mut [0_u8; 1]);
    let _ = stream.set_nodelay(true);
    let _ = stream.write_all(REFUSE_HEAD);
    drop(stream);
}

/// Answer `Expect: 100-continue`, so a client that waits for it sends the body.
fn send_continue(mut stream: &TcpStream, head: &RequestHead) {
    if header(&head.headers, "expect").is_some_and(|v| v.eq_ignore_ascii_case("100-continue")) {
        let _ = stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
    }
}

/// Run the request through `fault` (or straight through) and answer the client.
fn handle(
    shared: &Shared,
    head: &RequestHead,
    fault: Option<FaultKind>,
    mut client: Client,
) -> Outcome {
    let mut outcome = Outcome::default();
    send_continue(&client.writer, head);
    if fault == Some(FaultKind::DropBeforeForward) {
        read_part_of_body(&mut client.reader, &head.headers);
        client.close();
        return outcome;
    }
    let Ok(body) = read_body(&mut client.reader, &head.headers) else {
        client.close();
        return outcome;
    };
    let request = upstream_request(head, body.as_deref());
    let forward_now = || forward(shared.upstream, &head.method, &request);
    match fault {
        Some(FaultKind::Status(code)) => {
            let detail = format!("fault proxy: injected HTTP {code}");
            outcome.delivered = client.reply(&status_reply(code, &detail));
            return outcome;
        }
        Some(FaultKind::HangThenForward(hang)) => {
            thread::sleep(hang);
            client.close();
            outcome.upstream_status = forward_now().ok().map(|reply| reply.status);
            return outcome;
        }
        Some(FaultKind::Latency(delay)) => thread::sleep(delay),
        _ => {}
    }
    if fault == Some(FaultKind::HoldBefore) && outcome.park(shared) {
        client.close();
        return outcome;
    }
    let forwarded = forward_now();
    outcome.upstream_status = forwarded.as_ref().ok().map(|reply| reply.status);
    if fault == Some(FaultKind::HoldReply) && outcome.park(shared) {
        client.close();
        return outcome;
    }
    let (bytes, complete) = match (fault, forwarded) {
        (Some(FaultKind::DropAfterForward), _) => {
            client.close();
            return outcome;
        }
        (_, Err(err)) => {
            let detail = format!("fault proxy: forwarding failed: {err}");
            (status_reply(502, &detail), true)
        }
        (Some(FaultKind::StatusAfter(code)), Ok(reply)) => {
            let detail = format!(
                "fault proxy: upstream answered {}, injected HTTP {code}",
                reply.status
            );
            (status_reply(code, &detail), true)
        }
        (Some(FaultKind::TruncateBody), Ok(reply)) => truncated(&reply),
        (Some(FaultKind::CorruptBody), Ok(reply)) => (corrupted(&reply), true),
        (Some(FaultKind::ScrambleBody), Ok(reply)) => (scrambled(&reply), true),
        (_, Ok(reply)) => (passed_through(&reply), true),
    };
    outcome.delivered = client.reply(&bytes) && complete;
    outcome
}

/// The client side of one connection: a buffered reader and the writer, two
/// handles on one socket.
struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Client {
    /// Take the connection over, consuming the `head_len` head bytes that were
    /// already parsed from a peek.
    fn open(mut stream: TcpStream, head_len: usize) -> io::Result<Self> {
        stream.read_exact(&mut vec![0; head_len])?;
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Self {
            reader,
            writer: stream,
        })
    }

    /// Whether the client is still connected: a client that gave up (closed
    /// its socket) shows as end of stream or an error on a non-blocking peek.
    fn alive(&self) -> bool {
        if self.writer.set_nonblocking(true).is_err() {
            return true;
        }
        let alive = match self.writer.peek(&mut [0_u8; 1]) {
            Ok(got) => got > 0,
            Err(err) => err.kind() == io::ErrorKind::WouldBlock,
        };
        let _ = self.writer.set_nonblocking(false);
        alive
    }

    /// Write `bytes`, then close. `true` when the client was still connected
    /// and took every byte.
    fn reply(mut self, bytes: &[u8]) -> bool {
        let delivered = self.alive()
            && self
                .writer
                .write_all(bytes)
                .and_then(|()| self.writer.flush())
                .is_ok();
        self.close();
        delivered
    }

    /// Close the connection. `shutdown`, because dropping one of the two
    /// handles alone would leave the socket open.
    fn close(self) {
        let _ = self.writer.shutdown(Shutdown::Both);
    }
}

/// One header line: its bytes as received (no line ending), and its name
/// (lowercased) and value (trimmed) for lookups.
struct Header {
    raw: Vec<u8>,
    name: String,
    value: String,
}

impl Header {
    fn parse(raw: &[u8]) -> Self {
        let text = String::from_utf8_lossy(raw);
        let (name, value) = text.split_once(':').unwrap_or((text.as_ref(), ""));
        Self {
            raw: raw.to_vec(),
            name: name.trim().to_ascii_lowercase(),
            value: value.trim().to_owned(),
        }
    }
}

fn header<'a>(headers: &'a [Header], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|header| header.name == name)
        .map(|header| header.value.as_str())
}

fn content_length(headers: &[Header]) -> Option<usize> {
    header(headers, "content-length").and_then(|value| value.parse().ok())
}

fn is_chunked(headers: &[Header]) -> bool {
    header(headers, "transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"))
}

/// A request line and its headers.
struct RequestHead {
    method: String,
    path: String,
    request_line: Vec<u8>,
    headers: Vec<Header>,
    /// The head's length on the wire, its closing empty line included.
    len: usize,
}

/// Where the head ends in `bytes` (just past its closing empty line).
fn head_end(bytes: &[u8]) -> Option<usize> {
    (0..bytes.len()).find_map(|at| match bytes.get(at..) {
        Some([b'\n', b'\n', ..]) => Some(at + 2),
        Some([b'\n', b'\r', b'\n', ..]) => Some(at + 3),
        _ => None,
    })
}

/// Parse the request head from a peek, leaving its bytes in the socket's
/// receive queue.
fn peek_head(stream: &TcpStream) -> io::Result<RequestHead> {
    let mut buf = vec![0_u8; HEAD_LIMIT];
    let deadline = Instant::now() + CLIENT_TIMEOUT;
    loop {
        // Blocks (up to the read timeout) only while nothing has arrived.
        let got = stream.peek(&mut buf)?;
        if got == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        if let Some(len) = head_end(&buf[..got]) {
            let mut head = read_head(&mut &buf[..len])?;
            head.len = len;
            return Ok(head);
        }
        if got == buf.len() || Instant::now() >= deadline {
            return Err(invalid("no complete request head"));
        }
        thread::sleep(PEEK_PAUSE);
    }
}

/// One line, its `\n` (and any `\r`) included; an error at EOF or when the
/// connection closes mid-line.
fn read_line(reader: &mut impl BufRead) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line)?;
    if line.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the connection closed mid-line",
        ));
    }
    Ok(line)
}

fn trim_eol(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn read_head(reader: &mut impl BufRead) -> io::Result<RequestHead> {
    let request_line = trim_eol(&read_line(reader)?).to_vec();
    let text = String::from_utf8_lossy(&request_line);
    let mut words = text.split_whitespace();
    let (Some(method), Some(path)) = (words.next(), words.next()) else {
        return Err(invalid("malformed request line"));
    };
    let (method, path) = (method.to_owned(), path.to_owned());
    let headers = read_headers(reader)?;
    Ok(RequestHead {
        method,
        path,
        request_line,
        headers,
        len: 0,
    })
}

/// Header lines up to and including the empty line that ends them.
fn read_headers(reader: &mut impl BufRead) -> io::Result<Vec<Header>> {
    let mut headers = Vec::new();
    loop {
        let line = read_line(reader)?;
        let line = trim_eol(&line);
        if line.is_empty() {
            return Ok(headers);
        }
        headers.push(Header::parse(line));
    }
}

/// Append exactly `len` bytes from `reader` to `out`.
fn read_exact_into(reader: &mut impl Read, len: usize, out: &mut Vec<u8>) -> io::Result<()> {
    let got = reader.by_ref().take(len as u64).read_to_end(out)?;
    if got < len {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the connection closed mid-body",
        ));
    }
    Ok(())
}

fn chunk_size(line: &[u8]) -> io::Result<usize> {
    let text = String::from_utf8_lossy(line);
    let hex = text.split(';').next().unwrap_or_default().trim();
    usize::from_str_radix(hex, 16).map_err(|_| invalid("malformed chunk size"))
}

/// Read a chunked body through its last chunk and trailers; the data is
/// returned with the framing removed.
fn read_chunked(reader: &mut impl BufRead) -> io::Result<Vec<u8>> {
    let mut data = Vec::new();
    loop {
        let size = chunk_size(trim_eol(&read_line(reader)?))?;
        if size == 0 {
            while !trim_eol(&read_line(reader)?).is_empty() {}
            return Ok(data);
        }
        read_exact_into(reader, size, &mut data)?;
        if !trim_eol(&read_line(reader)?).is_empty() {
            return Err(invalid("chunk data overruns its size"));
        }
    }
}

/// The request body with its framing removed; `None` when the head frames no
/// body (neither chunked nor a Content-Length).
fn read_body(reader: &mut impl BufRead, headers: &[Header]) -> io::Result<Option<Vec<u8>>> {
    if is_chunked(headers) {
        return read_chunked(reader).map(Some);
    }
    content_length(headers)
        .map(|len| read_len(reader, len))
        .transpose()
}

/// Exactly `len` bytes from `reader`.
fn read_len(reader: &mut impl Read, len: usize) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    read_exact_into(reader, len, &mut out)?;
    Ok(out)
}

/// About half the request body: half the Content-Length, or half the first
/// chunk of a chunked body. Errors are moot: the connection closes next.
fn read_part_of_body(reader: &mut impl BufRead, headers: &[Header]) {
    let half = if is_chunked(headers) {
        match read_line(reader).and_then(|line| chunk_size(trim_eol(&line))) {
            Ok(size) => size / 2,
            Err(_) => return,
        }
    } else {
        content_length(headers).unwrap_or(0) / 2
    };
    let _ = read_exact_into(reader, half, &mut Vec::new());
}

fn push_line(out: &mut Vec<u8>, line: &[u8]) {
    out.extend_from_slice(line);
    out.extend_from_slice(b"\r\n");
}

/// The request as upstream gets it: the body re-framed by a Content-Length
/// and `Connection: close`, every other header line unchanged.
fn upstream_request(head: &RequestHead, body: Option<&[u8]>) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.map_or(0, <[u8]>::len) + 1024);
    push_line(&mut out, &head.request_line);
    for header in &head.headers {
        if !REQUEST_REFRAMED.contains(&header.name.as_str()) {
            push_line(&mut out, &header.raw);
        }
    }
    if let Some(body) = body {
        push_line(
            &mut out,
            format!("Content-Length: {}", body.len()).as_bytes(),
        );
    }
    push_line(&mut out, b"Connection: close");
    push_line(&mut out, b"");
    out.extend_from_slice(body.unwrap_or_default());
    out
}

/// An upstream reply: the final status line (interim 1xx replies skipped), its
/// headers, and its body with the framing removed.
struct Reply {
    status: u16,
    status_line: Vec<u8>,
    headers: Vec<Header>,
    body: Vec<u8>,
    /// A reply that carries no body: to a HEAD, or a 204 / 304.
    bodiless: bool,
}

/// Read one reply to a `method` request, framed by Content-Length, chunked,
/// or the connection's close.
fn read_reply(reader: &mut impl BufRead, method: &str) -> io::Result<Reply> {
    loop {
        let status_line = trim_eol(&read_line(reader)?).to_vec();
        let status = String::from_utf8_lossy(&status_line)
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse::<u16>().ok())
            .ok_or_else(|| invalid("malformed status line"))?;
        let headers = read_headers(reader)?;
        if (100..200).contains(&status) && status != 101 {
            continue;
        }
        let bodiless = method.eq_ignore_ascii_case("HEAD") || matches!(status, 101 | 204 | 304);
        let body = if bodiless {
            Vec::new()
        } else if is_chunked(&headers) {
            read_chunked(reader)?
        } else if let Some(len) = content_length(&headers) {
            read_len(reader, len)?
        } else {
            let mut body = Vec::new();
            reader.read_to_end(&mut body)?;
            body
        };
        return Ok(Reply {
            status,
            status_line,
            headers,
            body,
            bodiless,
        });
    }
}

/// Send `request` on a fresh upstream connection and read the reply.
fn forward(upstream: SocketAddr, method: &str, request: &[u8]) -> io::Result<Reply> {
    let mut stream = TcpStream::connect_timeout(&upstream, CONNECT_TIMEOUT)?;
    stream.set_read_timeout(Some(UPSTREAM_TIMEOUT))?;
    stream.set_write_timeout(Some(UPSTREAM_TIMEOUT))?;
    // An upstream that answers before taking the whole request (and resets
    // the connection) still leaves that answer to read.
    let sent = stream.write_all(request);
    match (sent, read_reply(&mut BufReader::new(stream), method)) {
        (Err(err), Err(_)) => Err(err),
        (_, reply) => reply,
    }
}

/// Upstream's status line and headers around `body`, re-framed with a
/// Content-Length of `declared` and `Connection: close`. A bodiless reply keeps
/// its own framing headers and sends no body.
fn framed(reply: &Reply, declared: usize, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 1024);
    push_line(&mut out, &reply.status_line);
    let dropped: &[&str] = if reply.bodiless {
        &HOP_BY_HOP
    } else {
        &REPLY_REFRAMED
    };
    for header in &reply.headers {
        if !dropped.contains(&header.name.as_str()) {
            push_line(&mut out, &header.raw);
        }
    }
    if !reply.bodiless {
        push_line(&mut out, format!("Content-Length: {declared}").as_bytes());
    }
    push_line(&mut out, b"Connection: close");
    push_line(&mut out, b"");
    if !reply.bodiless {
        out.extend_from_slice(body);
    }
    out
}

fn passed_through(reply: &Reply) -> Vec<u8> {
    framed(reply, reply.body.len(), &reply.body)
}

/// The full Content-Length, but only the first half of the body. The flag is
/// `true` only when that is the whole body (an empty one).
fn truncated(reply: &Reply) -> (Vec<u8>, bool) {
    let half = &reply.body[..reply.body.len() / 2];
    (
        framed(reply, reply.body.len(), half),
        half.len() == reply.body.len(),
    )
}

/// The body cut to half, with a Content-Length that matches the cut.
fn corrupted(reply: &Reply) -> Vec<u8> {
    let half = &reply.body[..reply.body.len() / 2];
    framed(reply, half.len(), half)
}

/// The body at full length, every byte from [`SCRAMBLE_KEEP`] on XORed with
/// [`SCRAMBLE_MASK`].
fn scrambled(reply: &Reply) -> Vec<u8> {
    let mut body = reply.body.clone();
    for byte in body.iter_mut().skip(SCRAMBLE_KEEP) {
        *byte ^= SCRAMBLE_MASK;
    }
    framed(reply, body.len(), &body)
}

/// A reply the proxy makes up: `code` with a `{"detail": ...}` body (none for a
/// 204 / 304, which may not carry one).
fn status_reply(code: u16, detail: &str) -> Vec<u8> {
    if matches!(code, 204 | 304) {
        return format!("HTTP/1.1 {code} Fault Proxy\r\nConnection: close\r\n\r\n").into_bytes();
    }
    let body = serde_json::json!({ "detail": detail }).to_string();
    format!(
        "HTTP/1.1 {code} Fault Proxy\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

#[cfg(test)]
#[path = "proxy_tests.rs"]
mod tests;
