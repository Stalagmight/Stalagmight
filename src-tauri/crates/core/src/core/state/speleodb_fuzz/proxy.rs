//! [#576 fuzzers, FZ-SDB] A fault-injecting HTTP/1.1 reverse proxy.
//!
//! It sits between the SpeleoDB client (ureq) and a real SpeleoDB server, both on
//! loopback, and turns an armed [`FaultRule`] into a failure a flaky network or
//! server produces: an error status the server never saw, a lost confirmation of
//! a request the server applied, a dropped upload, a lost or cut reply, a corrupt
//! body, or a request the server applies after the client gave up.
//!
//! One request per client connection: every reply carries `Connection: close`
//! (any Connection / Keep-Alive header upstream sent is dropped) and the socket
//! is closed after it. Upstream gets a fresh connection per request, with
//! `Connection: close` in place of the client's Connection header; every other
//! header line (Host included) and the body bytes are passed on unchanged, with
//! line endings written as CRLF. The upstream reply is read until upstream
//! closes the connection.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How long one upstream reply may take: a dump download or an upload.
const UPSTREAM_TIMEOUT: Duration = Duration::from_mins(3);
/// How long the proxy waits on a client that stops sending or reading.
const CLIENT_TIMEOUT: Duration = Duration::from_mins(3);
/// How long dialling upstream may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The hop-by-hop headers every reply to the client drops.
const HOP_BY_HOP: [&str; 2] = ["connection", "keep-alive"];

/// What an armed rule does to the request it fires on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FaultKind {
    /// Answer this status with a small JSON body, never forwarding: the server
    /// never sees the request.
    Status(u16),
    /// Forward and let upstream apply it, then answer this status instead: the
    /// confirmation is lost.
    StatusAfter(u16),
    /// Sleep, then forward normally.
    Latency(Duration),
    /// Read the headers and about half the body, then close the client socket
    /// without forwarding: the upload drops midway.
    DropBeforeForward,
    /// Forward fully, then close the client socket without writing anything: the
    /// reply is lost.
    DropAfterForward,
    /// Forward, then write upstream's status line, its headers and only the
    /// first half of the body bytes, then close: the download drops midway.
    TruncateBody,
    /// Forward, then answer upstream's status with the decoded body cut to half
    /// its length, re-framed with a correct Content-Length: malformed JSON / ZIP.
    CorruptBody,
    /// Hold the client socket open without answering for this long (the client
    /// times out), close it, THEN forward: the server applies it late.
    HangThenForward(Duration),
}

/// One armed fault. The first armed rule matching a request fires.
#[derive(Debug, Clone)]
pub(super) struct FaultRule {
    /// The request method it fires on (`Some("PUT")`); `None` matches any.
    pub(super) method: Option<&'static str>,
    /// A substring of the request path, e.g. `"/upload/auto/"`.
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
    /// The `Debug` form of the fault applied (`"StatusAfter(503)"`), if any.
    pub(super) fault: Option<String>,
    /// From the connection's arrival to the end of its handling, a late forward
    /// included.
    pub(super) millis: u64,
}

/// The proxy. Dropping it stops the accept thread; a request already being
/// handled runs to its end on its own thread (a pending `HangThenForward` still
/// reaches upstream), which never keeps the process from exiting.
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

    /// Drop every armed rule.
    pub(super) fn clear(&self) {
        lock(&self.shared.rules).clear();
    }

    /// How many rules are still armed.
    pub(super) fn armed(&self) -> usize {
        lock(&self.shared.rules).len()
    }

    /// Every request handled to its end since the last take, in arrival order. A
    /// request still in flight (a `HangThenForward` waiting to forward, say) is
    /// not in it yet: it comes with a later take, so call [`Self::quiesce`]
    /// first for a complete log.
    pub(super) fn take_log(&self) -> Vec<ProxyEvent> {
        lock(&self.shared.log)
            .drain(..)
            .map(|(_, event)| event)
            .collect()
    }

    /// Wait until no request is in flight, `HangThenForward`'s late forward
    /// included. `true` when the proxy went idle within `timeout`.
    pub(super) fn quiesce(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut in_flight = lock(&self.shared.in_flight);
        while *in_flight > 0 {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            in_flight = self
                .shared
                .idle
                .wait_timeout(in_flight, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

impl Drop for FaultProxy {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        // The accept loop blocks in `accept`; a throwaway connection wakes it to
        // see the stop flag. Joined only when the wake landed, so a failed wake
        // can never hang the drop.
        let woke = TcpStream::connect_timeout(&self.addr, Duration::from_secs(1)).is_ok();
        if let Some(accept) = self.accept.take().filter(|_| woke) {
            let _ = accept.join();
        }
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
        let arrived = Instant::now();
        let guard = InFlight::enter(shared);
        // A failed spawn drops the closure, and with it the guard and the stream.
        let _ = thread::Builder::new()
            .name(format!("fz-sdb-proxy-{seq}"))
            .spawn(move || serve(&guard.0, stream, seq, arrived));
    }
}

/// The client side of one connection: a buffered reader and the writer, two
/// handles on one socket.
struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Client {
    /// Write `bytes`, then close.
    fn reply(mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
        self.close();
    }

    /// Close the connection. `shutdown`, because dropping one of the two handles
    /// alone would leave the socket open.
    fn close(self) {
        let _ = self.writer.shutdown(Shutdown::Both);
    }
}

/// Handle one connection: read the request, apply the fault, log the event.
fn serve(shared: &Shared, stream: TcpStream, seq: u64, arrived: Instant) {
    let _ = stream.set_read_timeout(Some(CLIENT_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CLIENT_TIMEOUT));
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut client = Client {
        reader: BufReader::new(read_half),
        writer: stream,
    };
    let Ok(head) = read_head(&mut client.reader) else {
        client.close();
        return;
    };
    let fault = shared.take_rule(&head.method, &head.path);
    let upstream_status = apply(shared.upstream, fault, &head, client);
    shared.record(
        seq,
        ProxyEvent {
            method: head.method,
            path: head.path,
            upstream_status,
            fault: fault.map(|kind| format!("{kind:?}")),
            millis: millis(arrived.elapsed()),
        },
    );
}

/// Run the request through `fault` (or straight through) and answer the client.
/// Returns upstream's status when the request was forwarded.
fn apply(
    upstream: SocketAddr,
    fault: Option<FaultKind>,
    head: &RequestHead,
    mut client: Client,
) -> Option<u16> {
    if fault == Some(FaultKind::DropBeforeForward) {
        read_part_of_body(&mut client.reader, &head.headers);
        client.close();
        return None;
    }
    let Ok(body) = read_body(&mut client.reader, &head.headers) else {
        client.close();
        return None;
    };
    let request = upstream_request(head, &body);
    match fault {
        Some(FaultKind::Status(code)) => {
            client.reply(&status_reply(
                code,
                &format!("fault proxy: injected HTTP {code}"),
            ));
            None
        }
        Some(FaultKind::HangThenForward(hold)) => {
            thread::sleep(hold);
            client.close();
            forward(upstream, &request).ok().map(|reply| reply.status)
        }
        _ => {
            if let Some(FaultKind::Latency(delay)) = fault {
                thread::sleep(delay);
            }
            let reply = match forward(upstream, &request) {
                Ok(reply) => reply,
                Err(_) if fault == Some(FaultKind::DropAfterForward) => {
                    client.close();
                    return None;
                }
                Err(err) => {
                    client.reply(&status_reply(
                        502,
                        &format!("fault proxy: forwarding failed: {err}"),
                    ));
                    return None;
                }
            };
            match fault {
                Some(FaultKind::StatusAfter(code)) => client.reply(&status_reply(
                    code,
                    &format!(
                        "fault proxy: upstream answered {}, injected HTTP {code}",
                        reply.status
                    ),
                )),
                Some(FaultKind::DropAfterForward) => client.close(),
                Some(FaultKind::TruncateBody) => client.reply(&truncated(&reply)),
                Some(FaultKind::CorruptBody) => client.reply(&corrupted(&reply)),
                _ => client.reply(&passed_through(&reply)),
            }
            Some(reply.status)
        }
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
            name: name.trim().to_ascii_lowercase(),
            value: value.trim().to_owned(),
            raw: raw.to_vec(),
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

/// Read a chunked body through its last chunk and trailers: the wire bytes are
/// appended to `raw`, the decoded data returned.
fn read_chunked(reader: &mut impl BufRead, raw: &mut Vec<u8>) -> io::Result<Vec<u8>> {
    let mut data = Vec::new();
    loop {
        let line = read_line(reader)?;
        raw.extend_from_slice(&line);
        let size = chunk_size(trim_eol(&line))?;
        if size == 0 {
            loop {
                let trailer = read_line(reader)?;
                raw.extend_from_slice(&trailer);
                if trim_eol(&trailer).is_empty() {
                    return Ok(data);
                }
            }
        }
        let start = raw.len();
        // The chunk's data and the CRLF after it.
        let framed = size
            .checked_add(2)
            .ok_or_else(|| invalid("chunk size overflows"))?;
        read_exact_into(reader, framed, raw)?;
        data.extend_from_slice(&raw[start..start + size]);
    }
}

/// The request body's wire bytes, framed by `Transfer-Encoding: chunked` or
/// `Content-Length` (neither: no body).
fn read_body(reader: &mut impl BufRead, headers: &[Header]) -> io::Result<Vec<u8>> {
    let mut raw = Vec::new();
    if is_chunked(headers) {
        read_chunked(reader, &mut raw)?;
    } else if let Some(len) = content_length(headers) {
        read_exact_into(reader, len, &mut raw)?;
    }
    Ok(raw)
}

/// About half the request body: half the Content-Length, or half the first
/// chunk of a chunked body. Errors are moot - the connection closes next.
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

/// The request as upstream gets it: `Connection: close` in place of the
/// client's Connection header, everything else unchanged.
fn upstream_request(head: &RequestHead, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 1024);
    push_line(&mut out, &head.request_line);
    for header in head.headers.iter().filter(|h| h.name != "connection") {
        push_line(&mut out, &header.raw);
    }
    push_line(&mut out, b"Connection: close");
    push_line(&mut out, b"");
    out.extend_from_slice(body);
    out
}

/// An upstream reply: the final status line (interim 1xx replies skipped), its
/// headers, and every byte after them.
struct Reply {
    status: u16,
    status_line: Vec<u8>,
    headers: Vec<Header>,
    body: Vec<u8>,
}

fn parse_reply(raw: &[u8]) -> io::Result<Reply> {
    let mut rest = raw;
    loop {
        let status_line = trim_eol(&read_line(&mut rest)?).to_vec();
        let status = String::from_utf8_lossy(&status_line)
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse::<u16>().ok())
            .ok_or_else(|| invalid("malformed status line"))?;
        let headers = read_headers(&mut rest)?;
        if (100..200).contains(&status) && status != 101 {
            continue;
        }
        return Ok(Reply {
            status,
            status_line,
            headers,
            body: rest.to_vec(),
        });
    }
}

/// Send `request` on a fresh connection and read the reply until upstream
/// closes it.
fn forward(upstream: SocketAddr, request: &[u8]) -> io::Result<Reply> {
    let mut stream = TcpStream::connect_timeout(&upstream, CONNECT_TIMEOUT)?;
    stream.set_read_timeout(Some(UPSTREAM_TIMEOUT))?;
    stream.set_write_timeout(Some(UPSTREAM_TIMEOUT))?;
    stream.write_all(request)?;
    let mut raw = Vec::new();
    // A reset after the reply arrived still leaves that reply to pass on.
    if let Err(err) = stream.read_to_end(&mut raw) {
        if raw.is_empty() {
            return Err(err);
        }
    }
    parse_reply(&raw)
}

/// The body bytes as framed on the wire: a Content-Length body is cut to its
/// length, a chunked or close-delimited body is everything.
fn wire_body(reply: &Reply) -> &[u8] {
    match content_length(&reply.headers) {
        Some(len) if !is_chunked(&reply.headers) => &reply.body[..len.min(reply.body.len())],
        _ => &reply.body,
    }
}

/// The body with any chunked framing removed (a malformed one is used as is).
fn decoded_body(reply: &Reply) -> Vec<u8> {
    if is_chunked(&reply.headers) {
        let mut wire = reply.body.as_slice();
        return read_chunked(&mut wire, &mut Vec::new()).unwrap_or_else(|_| reply.body.clone());
    }
    wire_body(reply).to_vec()
}

/// Upstream's status line, its headers minus `dropped`, then `extra`,
/// `Connection: close`, and `body`.
fn reply_bytes(reply: &Reply, dropped: &[&str], extra: Option<String>, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 1024);
    push_line(&mut out, &reply.status_line);
    for header in &reply.headers {
        if !dropped.contains(&header.name.as_str()) {
            push_line(&mut out, &header.raw);
        }
    }
    if let Some(extra) = extra {
        push_line(&mut out, extra.as_bytes());
    }
    push_line(&mut out, b"Connection: close");
    push_line(&mut out, b"");
    out.extend_from_slice(body);
    out
}

fn passed_through(reply: &Reply) -> Vec<u8> {
    reply_bytes(reply, &HOP_BY_HOP, None, &reply.body)
}

/// The head as upstream sent it and the first half of the body's wire bytes. A
/// close-delimited body gets the Content-Length it had, so the cut shows.
fn truncated(reply: &Reply) -> Vec<u8> {
    let framed = is_chunked(&reply.headers) || content_length(&reply.headers).is_some();
    let wire = wire_body(reply);
    let extra = (!framed).then(|| format!("Content-Length: {}", wire.len()));
    reply_bytes(reply, &HOP_BY_HOP, extra, &wire[..wire.len() / 2])
}

/// The decoded body cut to half, with a Content-Length that matches the cut.
fn corrupted(reply: &Reply) -> Vec<u8> {
    let body = decoded_body(reply);
    let half = &body[..body.len() / 2];
    reply_bytes(
        reply,
        &[
            "connection",
            "keep-alive",
            "content-length",
            "transfer-encoding",
        ],
        Some(format!("Content-Length: {}", half.len())),
        half,
    )
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
mod tests {
    use std::io::{self, BufReader, Read, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{
        content_length, header, is_chunked, lock, parse_reply, read_body, read_head, FaultKind,
        FaultProxy, FaultRule, Reply,
    };

    /// What the in-test upstream answers.
    #[derive(Clone, Copy)]
    enum Canned {
        /// This status and JSON body, framed by Content-Length.
        Json(u16, &'static str),
        /// A 200 whose body is sent in these chunks.
        Chunked(&'static [&'static str]),
    }

    impl Canned {
        fn wire(self) -> String {
            match self {
                Self::Json(status, body) => format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                ),
                Self::Chunked(chunks) => {
                    let mut wire = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                                    Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
                        .to_owned();
                    wire.extend(
                        chunks
                            .iter()
                            .map(|chunk| format!("{:x}\r\n{chunk}\r\n", chunk.len())),
                    );
                    wire.push_str("0\r\n\r\n");
                    wire
                }
            }
        }
    }

    /// One request the in-test upstream received.
    #[derive(Clone)]
    struct Seen {
        method: String,
        path: String,
        connection: Option<String>,
        host: Option<String>,
        raw_body: Vec<u8>,
    }

    /// A tiny upstream: records each request, answers `canned`, closes.
    struct Upstream {
        addr: SocketAddr,
        seen: Arc<Mutex<Vec<Seen>>>,
        stop: Arc<AtomicBool>,
    }

    impl Upstream {
        fn start(canned: Canned) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind the test upstream");
            let addr = listener.local_addr().expect("the test upstream's address");
            let seen = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let (log, halt) = (Arc::clone(&seen), Arc::clone(&stop));
            thread::spawn(move || {
                for conn in listener.incoming() {
                    if halt.load(Ordering::SeqCst) {
                        return;
                    }
                    if let Ok(stream) = conn {
                        answer(stream, canned, &log);
                    }
                }
            });
            Self { addr, seen, stop }
        }

        fn seen(&self) -> Vec<Seen> {
            lock(&self.seen).clone()
        }
    }

    impl Drop for Upstream {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            let _ = TcpStream::connect(self.addr);
        }
    }

    fn answer(mut stream: TcpStream, canned: Canned, log: &Mutex<Vec<Seen>>) {
        let Ok(read_half) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(read_half);
        let Ok(head) = read_head(&mut reader) else {
            return;
        };
        let Ok(raw_body) = read_body(&mut reader, &head.headers) else {
            return;
        };
        lock(log).push(Seen {
            connection: header(&head.headers, "connection").map(str::to_owned),
            host: header(&head.headers, "host").map(str::to_owned),
            method: head.method,
            path: head.path,
            raw_body,
        });
        let _ = stream.write_all(canned.wire().as_bytes());
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

    /// A request with a Content-Length body, asking for keep-alive (the proxy
    /// must replace that).
    fn request(method: &str, path: &str, body: &str) -> Vec<u8> {
        format!(
            "{method} {path} HTTP/1.1\r\nHost: sdb.test:8000\r\nConnection: keep-alive\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    /// Send `request` through the proxy and read until it closes.
    fn exchange(proxy: &FaultProxy, request: &[u8], timeout: Duration) -> io::Result<Vec<u8>> {
        let mut stream = TcpStream::connect(proxy.addr)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        stream.write_all(request)?;
        let mut reply = Vec::new();
        stream.read_to_end(&mut reply)?;
        Ok(reply)
    }

    fn ask(proxy: &FaultProxy, request: &[u8]) -> Reply {
        let raw = exchange(proxy, request, Duration::from_secs(5)).expect("a reply");
        parse_reply(&raw).expect("a well-formed reply")
    }

    fn settle(proxy: &FaultProxy) {
        assert!(proxy.quiesce(Duration::from_secs(5)), "the proxy went idle");
    }

    fn connection_headers(reply: &Reply) -> Vec<&str> {
        reply
            .headers
            .iter()
            .filter(|h| h.name == "connection")
            .map(|h| h.value.as_str())
            .collect()
    }

    const TOKEN_JSON: &str = r#"{"token":"0123456789abcdef"}"#;

    #[test]
    fn pass_through_keeps_status_body_and_request_bytes() {
        let upstream = Upstream::start(Canned::Json(201, TOKEN_JSON));
        let proxy = start(&upstream);

        let reply = ask(
            &proxy,
            &request("POST", "/api/v2/projects/", r#"{"name":"x"}"#),
        );
        assert_eq!(reply.status, 201);
        assert_eq!(reply.body, TOKEN_JSON.as_bytes());
        assert_eq!(connection_headers(&reply), vec!["close"]);

        let chunked_body = "5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let chunked = format!(
            "PUT /api/v2/projects/p1/upload/auto/ HTTP/1.1\r\nHost: sdb.test:8000\r\n\
             Transfer-Encoding: chunked\r\n\r\n{chunked_body}"
        );
        assert_eq!(ask(&proxy, chunked.as_bytes()).status, 201);

        settle(&proxy);
        let seen = upstream.seen();
        assert_eq!(seen.len(), 2);
        assert_eq!(
            (seen[0].method.as_str(), seen[0].path.as_str()),
            ("POST", "/api/v2/projects/")
        );
        assert_eq!(seen[0].raw_body, br#"{"name":"x"}"#);
        assert_eq!(seen[0].connection.as_deref(), Some("close"));
        assert_eq!(seen[0].host.as_deref(), Some("sdb.test:8000"));
        assert_eq!(seen[1].raw_body, chunked_body.as_bytes());
        assert_eq!(seen[1].connection.as_deref(), Some("close"));

        let log = proxy.take_log();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].method, "POST");
        assert_eq!(log[1].path, "/api/v2/projects/p1/upload/auto/");
        assert!(log
            .iter()
            .all(|e| e.upstream_status == Some(201) && e.fault.is_none()));
        assert!(proxy.take_log().is_empty(), "a take drains the log");
    }

    #[test]
    fn a_chunked_reply_passes_through_unchanged() {
        let chunks: &[&str] = &[r#"{"a":"#, "1}"];
        let upstream = Upstream::start(Canned::Chunked(chunks));
        let proxy = start(&upstream);
        let reply = ask(&proxy, &request("GET", "/api/v2/projects/", ""));
        assert_eq!(reply.status, 200);
        assert!(is_chunked(&reply.headers));
        let upstream_wire = Canned::Chunked(chunks).wire();
        let (_, upstream_body) = upstream_wire
            .split_once("\r\n\r\n")
            .expect("the canned reply has a head");
        assert_eq!(reply.body, upstream_body.as_bytes());
    }

    #[test]
    fn status_answers_without_forwarding() {
        let upstream = Upstream::start(Canned::Json(200, TOKEN_JSON));
        let proxy = start(&upstream);
        proxy.arm(rule(None, "/user/", FaultKind::Status(503), 1));
        let reply = ask(&proxy, &request("POST", "/api/v2/user/auth-token/", "{}"));
        assert_eq!(reply.status, 503);
        let body: serde_json::Value = serde_json::from_slice(&reply.body).expect("a JSON body");
        assert!(body.get("detail").is_some(), "{body}");
        settle(&proxy);
        assert!(upstream.seen().is_empty(), "upstream never saw it");
        let log = proxy.take_log();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].fault.as_deref(), Some("Status(503)"));
        assert_eq!(log[0].upstream_status, None);
    }

    #[test]
    fn status_after_forwards_then_answers_the_injected_status() {
        let upstream = Upstream::start(Canned::Json(200, TOKEN_JSON));
        let proxy = start(&upstream);
        proxy.arm(rule(
            Some("PUT"),
            "/upload/",
            FaultKind::StatusAfter(500),
            1,
        ));
        let reply = ask(
            &proxy,
            &request("PUT", "/api/v2/projects/p1/upload/auto/", "{}"),
        );
        assert_eq!(reply.status, 500);
        settle(&proxy);
        assert_eq!(upstream.seen().len(), 1, "upstream applied it");
        assert_eq!(proxy.take_log()[0].upstream_status, Some(200));
    }

    #[test]
    fn drop_before_forward_cuts_the_upload_and_never_forwards() {
        let upstream = Upstream::start(Canned::Json(200, TOKEN_JSON));
        let proxy = start(&upstream);
        proxy.arm(rule(None, "/upload/", FaultKind::DropBeforeForward, 1));
        let big = "x".repeat(256 * 1024);
        let got = exchange(
            &proxy,
            &request("PUT", "/api/v2/projects/p1/upload/auto/", &big),
            Duration::from_secs(5),
        );
        assert!(
            got.as_ref().map_or(true, Vec::is_empty),
            "the client got an IO error or EOF, not a reply: {got:?}"
        );
        settle(&proxy);
        assert!(upstream.seen().is_empty(), "upstream never saw it");
        let log = proxy.take_log();
        assert_eq!(log[0].fault.as_deref(), Some("DropBeforeForward"));
        assert_eq!(log[0].upstream_status, None);
    }

    #[test]
    fn drop_after_forward_applies_but_answers_nothing() {
        let upstream = Upstream::start(Canned::Json(200, TOKEN_JSON));
        let proxy = start(&upstream);
        proxy.arm(rule(None, "/projects/", FaultKind::DropAfterForward, 1));
        let got = exchange(
            &proxy,
            &request("DELETE", "/api/v2/projects/p1/", ""),
            Duration::from_secs(5),
        );
        assert!(
            got.as_ref().map_or(true, Vec::is_empty),
            "the client got EOF: {got:?}"
        );
        settle(&proxy);
        assert_eq!(upstream.seen().len(), 1, "upstream applied it");
        assert_eq!(proxy.take_log()[0].upstream_status, Some(200));
    }

    #[test]
    fn truncate_body_sends_fewer_bytes_than_content_length() {
        let body = r#"{"detail":"a reply long enough to cut in half, forty-odd bytes"}"#;
        let upstream = Upstream::start(Canned::Json(200, body));
        let proxy = start(&upstream);
        proxy.arm(rule(None, "/download/", FaultKind::TruncateBody, 1));
        let reply = ask(
            &proxy,
            &request("GET", "/api/v2/projects/p1/download/dump/", ""),
        );
        assert_eq!(reply.status, 200);
        assert_eq!(content_length(&reply.headers), Some(body.len()));
        assert_eq!(reply.body, body.as_bytes()[..body.len() / 2]);
        assert_eq!(connection_headers(&reply), vec!["close"]);
    }

    #[test]
    fn corrupt_body_halves_the_decoded_body_with_a_correct_length() {
        let chunks: &[&str] = &[r#"{"token":"#, r#""abcdef"}"#];
        let upstream = Upstream::start(Canned::Chunked(chunks));
        let proxy = start(&upstream);
        proxy.arm(rule(None, "auth-token", FaultKind::CorruptBody, 1));
        let reply = ask(&proxy, &request("POST", "/api/v2/user/auth-token/", "{}"));
        let decoded = chunks.concat();
        let half = &decoded.as_bytes()[..decoded.len() / 2];
        assert_eq!(reply.status, 200);
        assert!(!is_chunked(&reply.headers), "re-framed by Content-Length");
        assert_eq!(content_length(&reply.headers), Some(half.len()));
        assert_eq!(reply.body, half);
        assert!(serde_json::from_slice::<serde_json::Value>(&reply.body).is_err());
    }

    #[test]
    fn hang_then_forward_times_the_client_out_then_applies_late() {
        let upstream = Upstream::start(Canned::Json(200, TOKEN_JSON));
        let proxy = start(&upstream);
        let hold = Duration::from_millis(1500);
        proxy.arm(rule(None, "/acquire/", FaultKind::HangThenForward(hold), 1));
        let sent = Instant::now();
        let got = exchange(
            &proxy,
            &request("POST", "/api/v2/projects/p1/acquire/", ""),
            Duration::from_secs(1),
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
            "nothing forwarded before the hold ends"
        );
        assert!(
            proxy.take_log().is_empty(),
            "the request is still in flight"
        );
        settle(&proxy);
        assert!(sent.elapsed() >= hold);
        assert_eq!(upstream.seen().len(), 1, "upstream applied it late");
        let log = proxy.take_log();
        assert_eq!(log[0].upstream_status, Some(200));
        assert!(log[0].millis >= 1500, "{:?}", log[0]);
    }

    #[test]
    fn latency_delays_then_forwards() {
        let upstream = Upstream::start(Canned::Json(200, TOKEN_JSON));
        let proxy = start(&upstream);
        let delay = Duration::from_millis(200);
        proxy.arm(rule(None, "/user/", FaultKind::Latency(delay), 1));
        let sent = Instant::now();
        let reply = ask(&proxy, &request("GET", "/api/v2/user/", ""));
        assert!(sent.elapsed() >= delay);
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body, TOKEN_JSON.as_bytes());
    }

    #[test]
    fn rules_fire_in_arm_order_and_expire_after_their_times() {
        let upstream = Upstream::start(Canned::Json(200, TOKEN_JSON));
        let proxy = start(&upstream);
        proxy.arm(rule(Some("PUT"), "/p1/", FaultKind::Status(409), 1));
        proxy.arm(rule(None, "/p1/", FaultKind::Status(503), 2));
        proxy.arm(rule(None, "/p1/", FaultKind::Status(500), 1));
        proxy.arm(rule(None, "/p1/", FaultKind::Status(418), 0));
        assert_eq!(proxy.armed(), 3, "a rule armed with times 0 is not kept");

        let status = |method: &str, path: &str| ask(&proxy, &request(method, path, "")).status;
        assert_eq!(
            status("GET", "/api/v2/projects/p1/"),
            503,
            "the PUT rule skips a GET"
        );
        assert_eq!(proxy.armed(), 3);
        assert_eq!(
            status("GET", "/api/v2/projects/p2/"),
            200,
            "no rule matches p2"
        );
        assert_eq!(
            status("PUT", "/api/v2/projects/p1/"),
            409,
            "the first match fires"
        );
        assert_eq!(proxy.armed(), 2);
        assert_eq!(status("GET", "/api/v2/projects/p1/"), 503);
        assert_eq!(proxy.armed(), 1, "the 503 rule used its two times");
        assert_eq!(status("GET", "/api/v2/projects/p1/"), 500);
        assert_eq!(proxy.armed(), 0);
        assert_eq!(status("GET", "/api/v2/projects/p1/"), 200);

        proxy.arm(rule(None, "/", FaultKind::Status(503), 5));
        proxy.clear();
        assert_eq!(proxy.armed(), 0);
        assert_eq!(status("GET", "/api/v2/projects/p1/"), 200);

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
    fn a_non_loopback_upstream_is_refused() {
        let addr: SocketAddr = "192.0.2.1:80".parse().expect("a TEST-NET-1 address");
        let err = FaultProxy::start(addr)
            .err()
            .expect("a non-loopback upstream is refused");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn dropping_the_proxy_stops_accepting() {
        let upstream = Upstream::start(Canned::Json(200, TOKEN_JSON));
        let proxy = start(&upstream);
        let addr = proxy.addr;
        drop(proxy);
        assert!(TcpStream::connect(addr).is_err(), "the listener closed");
    }

    /// The real SpeleoDB client (ureq) through the proxy, both ways.
    #[test]
    fn the_speleodb_client_signs_in_through_the_proxy() {
        let upstream = Upstream::start(Canned::Json(200, TOKEN_JSON));
        let proxy = start(&upstream);
        let token =
            crate::speleodb::client::login(&proxy.base_url(), false, "diver@example.com", "pw")
                .expect("login through the proxy");
        assert_eq!(token, "0123456789abcdef");
        settle(&proxy);
        let seen = upstream.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            (seen[0].method.as_str(), seen[0].path.as_str()),
            ("POST", "/api/v2/user/auth-token/")
        );
        assert!(String::from_utf8_lossy(&seen[0].raw_body).contains("diver@example.com"));
        assert_eq!(seen[0].connection.as_deref(), Some("close"));

        proxy.arm(rule(None, "auth-token", FaultKind::CorruptBody, 1));
        let err =
            crate::speleodb::client::login(&proxy.base_url(), false, "diver@example.com", "pw")
                .expect_err("a cut JSON body does not parse");
        assert!(
            matches!(err, crate::speleodb::SpeleoDbError::Parse(_)),
            "{err:?}"
        );
    }
}
