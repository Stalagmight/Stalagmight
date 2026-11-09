//! A raw HTTP/1.1 request over std, for the endpoints the client crate does not
//! offer (project delete, the per-user permission grants, the generic
//! `upload/auto/` route). Loopback only, like everything in the fuzzer: the base
//! URL must pass [`super::target::check_target_url`] before a socket is opened.
//!
//! One request per connection (`Connection: close`), a request body sent with a
//! Content-Length, the reply read to EOF and de-chunked when the server answers
//! `Transfer-Encoding: chunked`. The token goes out as
//! `Authorization: Bearer <token>`, exactly as the published SDK sends it
//! (reqwest's `bearer_auth`, which `HttpClient::with_api_key` drives).

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IO_TIMEOUT: Duration = Duration::from_mins(2);
/// The multipart boundary; a part that contains it is refused.
const BOUNDARY: &str = "stalagmight-fuzz-7f3a9c1e5b20d4";

/// What the server answered.
pub(super) struct Reply {
    pub(super) status: u16,
    /// The body, de-chunked.
    pub(super) body: Vec<u8>,
}

/// A request body and its media type.
struct Body {
    content_type: String,
    bytes: Vec<u8>,
}

/// Send `method base/path` with an optional Bearer token and an optional JSON
/// body, and return the server's reply, whatever its status. `path` is relative
/// to the base (`api/v2/projects/<id>/permission/user/detail/`; a leading `/` is
/// tolerated). `Err` only when the request was refused before sending (a base
/// URL that is not a loopback instance, a malformed method, path or token) or
/// the exchange itself failed (connect, send, read, an unparseable reply).
pub(super) fn request(
    base: &str,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    json: Option<&serde_json::Value>,
) -> Result<Reply, String> {
    let (_, addr) = super::target::check_target_url(base)?;
    let body = json.map(|value| Body {
        content_type: "application/json".to_owned(),
        bytes: value.to_string().into_bytes(),
    });
    let bytes = encode_request(addr, method, path, bearer, body)?;
    exchange(addr, &bytes, method, path)
}

/// `PUT base/path` as `multipart/form-data`: one part per `files` entry
/// (`(field, file name, media type, bytes)`), then one text part per `fields`
/// entry (`(name, value)`), with a Bearer token. Same refusals and reply as
/// [`request`]; also refused when a name, file name or media type would break
/// the part header, or when any content holds the boundary.
pub(super) fn multipart_put(
    base: &str,
    path: &str,
    bearer: &str,
    files: &[(&str, &str, &str, &[u8])],
    fields: &[(&str, &str)],
) -> Result<Reply, String> {
    let (_, addr) = super::target::check_target_url(base)?;
    let body = multipart_body(files, fields)?;
    let bytes = encode_request(addr, "PUT", path, Some(bearer), Some(body))?;
    exchange(addr, &bytes, "PUT", path)
}

/// Connect, send `bytes`, read the reply to EOF and parse it.
fn exchange(addr: SocketAddr, bytes: &[u8], method: &str, path: &str) -> Result<Reply, String> {
    let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .map_err(|e| format!("connect {addr}: {e}"))?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(IO_TIMEOUT)))
        .map_err(|e| format!("timeout: {e}"))?;
    stream
        .write_all(bytes)
        .and_then(|()| stream.flush())
        .map_err(|e| format!("send {method} {path}: {e}"))?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| format!("read {method} {path}: {e}"))?;
    parse_response(&raw)
}

/// Visible ASCII only: no space, control character or non-ASCII byte that could
/// split the request line or smuggle a header.
fn is_wire_safe(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_graphic())
}

/// Safe inside a quoted `Content-Disposition` parameter: visible ASCII or
/// space, and no quote or backslash.
fn is_quotable(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| (b.is_ascii_graphic() || b == b' ') && b != b'"' && b != b'\\')
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// The multipart body and its `Content-Type`.
fn multipart_body(
    files: &[(&str, &str, &str, &[u8])],
    fields: &[(&str, &str)],
) -> Result<Body, String> {
    let delimiter = format!("--{BOUNDARY}");
    let mut bytes = Vec::new();
    for (field, file_name, mime, content) in files {
        if !is_quotable(field) || !is_quotable(file_name) || !is_wire_safe(mime) {
            return Err(format!(
                "refused: the part {field:?} ({file_name:?}, {mime:?}) has an unsafe header"
            ));
        }
        if contains(content, delimiter.as_bytes()) {
            return Err(format!(
                "refused: {file_name:?} contains the multipart boundary"
            ));
        }
        bytes.extend_from_slice(
            format!(
                "{delimiter}\r\nContent-Disposition: form-data; name=\"{field}\"; \
                 filename=\"{file_name}\"\r\nContent-Type: {mime}\r\n\r\n"
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(content);
        bytes.extend_from_slice(b"\r\n");
    }
    for (name, value) in fields {
        if !is_quotable(name) {
            return Err(format!("refused: the field name {name:?} is unsafe"));
        }
        if value.contains(&delimiter) {
            return Err(format!(
                "refused: the field {name:?} contains the multipart boundary"
            ));
        }
        bytes.extend_from_slice(
            format!(
                "{delimiter}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    bytes.extend_from_slice(format!("{delimiter}--\r\n").as_bytes());
    Ok(Body {
        content_type: format!("multipart/form-data; boundary={BOUNDARY}"),
        bytes,
    })
}

/// The request bytes: head and body.
fn encode_request(
    host: SocketAddr,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    body: Option<Body>,
) -> Result<Vec<u8>, String> {
    if method.is_empty() || !method.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(format!("refused: {method:?} is not an HTTP method"));
    }
    let path = path.trim_start_matches('/');
    if !path.is_empty() && !is_wire_safe(path) {
        return Err(format!(
            "refused: the path {path:?} is not plain visible ASCII"
        ));
    }
    if bearer.is_some_and(|t| !is_wire_safe(t)) {
        return Err("refused: the token is not plain visible ASCII".to_owned());
    }
    let auth = bearer
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    let framing = match &body {
        Some(body) => format!(
            "Content-Type: {}\r\nContent-Length: {}\r\n",
            body.content_type,
            body.bytes.len()
        ),
        None if method != "GET" && method != "HEAD" => "Content-Length: 0\r\n".to_owned(),
        None => String::new(),
    };
    let head = format!(
        "{method} /{path} HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\n\
         Connection: close\r\n{auth}{framing}\r\n"
    );
    let mut bytes = head.into_bytes();
    if let Some(body) = body {
        bytes.extend(body.bytes);
    }
    Ok(bytes)
}

/// Split a raw `Connection: close` response into its status and body: a chunked
/// body is de-chunked, a Content-Length body is cut to that length (a shorter
/// one is an error), any other body runs to EOF.
fn parse_response(raw: &[u8]) -> Result<Reply, String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("no header terminator in the response")?;
    let head = String::from_utf8_lossy(raw.get(..split).unwrap_or_default());
    let rest = raw.get(split + 4..).unwrap_or_default();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut words = status_line.split_whitespace();
    let (version, code) = (words.next(), words.next());
    let status = code
        .filter(|_| version.is_some_and(|v| v.starts_with("HTTP/1.")))
        .and_then(|code| code.parse::<u16>().ok())
        .filter(|code| (100..=599).contains(code))
        .ok_or_else(|| format!("no HTTP/1.x status line in the response: {status_line:?}"))?;
    let mut chunked = false;
    let mut content_length = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let (name, value) = (name.trim(), value.trim());
        if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.to_ascii_lowercase().contains("chunked");
        } else if name.eq_ignore_ascii_case("content-length") {
            content_length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| format!("a bad Content-Length {value:?}"))?,
            );
        }
    }
    let body = if chunked {
        dechunk(rest)?
    } else if let Some(len) = content_length {
        rest.get(..len)
            .ok_or_else(|| format!("the body is {} bytes of a declared {len}", rest.len()))?
            .to_vec()
    } else {
        rest.to_vec()
    };
    Ok(Reply { status, body })
}

/// Decode a chunked body (chunk extensions ignored, trailers dropped).
fn dechunk(mut rest: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let eol = rest
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or("a chunk size line without CRLF")?;
        let size_line = String::from_utf8_lossy(rest.get(..eol).unwrap_or_default()).into_owned();
        let size_text = size_line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| format!("a bad chunk size {size_text:?}"))?;
        if size == 0 {
            return Ok(out);
        }
        let start = eol + 2;
        let chunk = rest.get(start..start + size).ok_or("a truncated chunk")?;
        out.extend_from_slice(chunk);
        if rest.get(start + size..start + size + 2) != Some(b"\r\n".as_slice()) {
            return Err("a chunk without its closing CRLF".to_owned());
        }
        rest = rest.get(start + size + 2..).unwrap_or_default();
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead as _, BufReader, Read as _, Write as _};
    use std::net::{Ipv4Addr, TcpListener};
    use std::thread;

    use super::{encode_request, multipart_body, multipart_put, parse_response, request, BOUNDARY};

    fn parsed(raw: &[u8]) -> Result<(u16, Vec<u8>), String> {
        parse_response(raw).map(|reply| (reply.status, reply.body))
    }

    #[test]
    fn responses_parse_by_length_by_chunks_and_to_eof() {
        let sized = b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\nokEXTRA";
        assert_eq!(parsed(sized), Ok((201, b"ok".to_vec())));
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2;ext=1\r\nde\r\n0\r\nX-Trailer: t\r\n\r\n";
        assert_eq!(parsed(chunked), Ok((200, b"abcde".to_vec())));
        let to_eof = b"HTTP/1.0 404 Not Found\r\nContent-Type: application/json\r\n\r\n{\"detail\":\"Not found.\"}";
        assert_eq!(
            parsed(to_eof),
            Ok((404, br#"{"detail":"Not found."}"#.to_vec()))
        );
        assert_eq!(
            parsed(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"),
            Ok((204, Vec::new()))
        );
    }

    #[test]
    fn garbage_responses_are_errors() {
        for bad in [
            b"garbage".as_slice(),
            b"",
            b"HTTP/1.1 abc OK\r\n\r\n",
            b"HTTP/1.1 999 Odd\r\n\r\n",
            b"SPDY/3 200 OK\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort",
            b"HTTP/1.1 200 OK\r\nContent-Length: ten\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\nabc\r\n0\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n9\r\nabc\r\n0\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabcXX0\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n",
        ] {
            assert!(
                parse_response(bad).is_err(),
                "{:?} must not parse",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn nothing_but_a_loopback_instance_is_ever_contacted() {
        for base in [
            "https://www.speleodb.org/",
            "http://www.speleodb.org/",
            "http://10.0.0.5:8000/",
            "http://localhost:18000/",
        ] {
            let err = request(base, "GET", "api/v2/projects/", Some("t"), None)
                .err()
                .expect("refused before connecting");
            assert!(err.contains("refused"), "{base}: {err}");
            let err = multipart_put(base, "api/v2/projects/p/upload/auto/", "t", &[], &[])
                .err()
                .expect("refused before connecting");
            assert!(err.contains("refused"), "{base}: {err}");
        }
        let host = "127.0.0.1:9".parse().expect("addr");
        assert!(encode_request(host, "get", "x/", None, None).is_err());
        assert!(encode_request(host, "GET", "a b/", None, None).is_err());
        assert!(encode_request(host, "GET", "a/\r\nX: y", None, None).is_err());
        assert!(encode_request(host, "GET", "a/", Some("t\r\nX: y"), None).is_err());
        assert!(encode_request(host, "GET", "a/", Some(""), None).is_err());
        let boundary = format!("--{BOUNDARY}");
        let one = b"1".as_slice();
        assert!(multipart_body(&[("a\"b", "f", "x/y", one)], &[]).is_err());
        assert!(multipart_body(&[("a", "f\r\n", "x/y", one)], &[]).is_err());
        assert!(multipart_body(&[("a", "f", "x y", one)], &[]).is_err());
        assert!(multipart_body(&[("a", "f", "x/y", one)], &[("m", "v")]).is_ok());
        assert!(multipart_body(&[("a", "f", "x/y", boundary.as_bytes())], &[]).is_err());
        assert!(multipart_body(&[], &[("m", boundary.as_str())]).is_err());
    }

    /// One request as a listener saw it: the request line, the headers
    /// (lowercased names) and the body.
    struct Seen {
        line: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl Seen {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
        }
    }

    /// Accept one connection on a fresh loopback listener, record the request
    /// and answer `reply`. Returns the listener's address and the thread.
    fn serve_once(reply: &'static [u8]) -> (String, thread::JoinHandle<Seen>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let addr = listener.local_addr().expect("addr").to_string();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut line = String::new();
            reader.read_line(&mut line).expect("request line");
            let mut headers = Vec::new();
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).expect("header");
                let header = header.trim_end();
                if header.is_empty() {
                    break;
                }
                let (name, value) = header.split_once(':').expect("a name: value header");
                headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
            }
            let len = headers
                .iter()
                .find(|(name, _)| name == "content-length")
                .map_or(0, |(_, v)| v.parse::<usize>().expect("length"));
            let mut body = vec![0; len];
            reader.read_exact(&mut body).expect("body");
            let mut writer = stream;
            writer.write_all(reply).expect("reply");
            Seen {
                line: line.trim_end().to_owned(),
                headers,
                body,
            }
        });
        (addr, server)
    }

    #[test]
    fn a_json_request_round_trips_through_a_loopback_listener() {
        let (addr, server) = serve_once(
            b"HTTP/1.1 201 Created\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n",
        );
        let body = serde_json::json!({ "user": "collab@example.invalid", "level": "READ_ONLY" });
        let reply = request(
            &format!("http://{addr}"),
            "POST",
            "/api/v2/projects/p1/permission/user/detail/",
            Some("tok123"),
            Some(&body),
        )
        .expect("round trip");
        assert_eq!(
            (reply.status, reply.body.as_slice()),
            (201, br#"{"a":1}"#.as_slice())
        );
        let seen = server.join().expect("the listener thread");
        assert_eq!(
            seen.line,
            "POST /api/v2/projects/p1/permission/user/detail/ HTTP/1.1"
        );
        assert_eq!(seen.header("host"), Some(addr.as_str()));
        assert_eq!(seen.header("authorization"), Some("Bearer tok123"));
        assert_eq!(seen.header("content-type"), Some("application/json"));
        assert_eq!(seen.header("connection"), Some("close"));
        let len = seen.body.len().to_string();
        assert_eq!(seen.header("content-length"), Some(len.as_str()));
        let sent: serde_json::Value = serde_json::from_slice(&seen.body).expect("a JSON body");
        assert_eq!(sent, body);
    }

    #[test]
    fn a_bodyless_get_sends_no_length_and_no_token_when_none_is_given() {
        let (addr, server) = serve_once(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 2\r\n\r\nno");
        let reply = request(
            &format!("http://{addr}/"),
            "GET",
            "api/v2/user/",
            None,
            None,
        )
        .expect("round trip");
        assert_eq!(
            (reply.status, reply.body.as_slice()),
            (403, b"no".as_slice())
        );
        let seen = server.join().expect("the listener thread");
        assert_eq!(seen.line, "GET /api/v2/user/ HTTP/1.1");
        assert_eq!(seen.body, b"");
        for absent in ["authorization", "content-length", "content-type"] {
            assert!(
                seen.header(absent).is_none(),
                "{absent} in {:?}",
                seen.headers
            );
        }
    }

    /// Split a multipart body into `(part headers, part content)` pairs.
    fn parts(body: &[u8], boundary: &str) -> Vec<(String, Vec<u8>)> {
        let first = format!("--{boundary}\r\n");
        let close = format!("\r\n--{boundary}--\r\n");
        let inner = body
            .strip_prefix(first.as_bytes())
            .and_then(|rest| rest.strip_suffix(close.as_bytes()))
            .expect("the body opens and closes with the boundary");
        let separator = format!("\r\n--{boundary}\r\n");
        let mut out = Vec::new();
        let mut rest = inner;
        loop {
            let end = rest
                .windows(separator.len())
                .position(|w| w == separator.as_bytes())
                .unwrap_or(rest.len());
            let part = rest.get(..end).expect("part");
            let split = part
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .expect("part headers end");
            out.push((
                String::from_utf8_lossy(part.get(..split).expect("headers")).into_owned(),
                part.get(split + 4..).expect("content").to_vec(),
            ));
            match rest.get(end + separator.len()..) {
                Some(next) => rest = next,
                None => return out,
            }
        }
    }

    #[test]
    fn a_multipart_put_round_trips_its_parts() {
        let (addr, server) = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
        );
        let artifact: &[u8] = b"SQLite format 3\0\r\n\xff\x00binary";
        let reply = multipart_put(
            &format!("http://{addr}/"),
            "api/v2/projects/p1/upload/auto/",
            "tok9",
            &[(
                "artifact",
                "project.stalx",
                "application/octet-stream",
                artifact,
            )],
            &[("message", "seed: legacy layout")],
        )
        .expect("round trip");
        assert_eq!(
            (reply.status, reply.body.as_slice()),
            (200, b"{}".as_slice())
        );
        let seen = server.join().expect("the listener thread");
        assert_eq!(seen.line, "PUT /api/v2/projects/p1/upload/auto/ HTTP/1.1");
        assert_eq!(seen.header("authorization"), Some("Bearer tok9"));
        let boundary = seen
            .header("content-type")
            .and_then(|v| v.strip_prefix("multipart/form-data; boundary="))
            .expect("a multipart content type")
            .to_owned();
        let len = seen.body.len().to_string();
        assert_eq!(seen.header("content-length"), Some(len.as_str()));
        let got = parts(&seen.body, &boundary);
        assert_eq!(got.len(), 2, "{got:?}");
        let (file_head, file_body) = got.first().expect("the file part");
        assert_eq!(
            file_head,
            "Content-Disposition: form-data; name=\"artifact\"; filename=\"project.stalx\"\r\n\
             Content-Type: application/octet-stream"
        );
        assert_eq!(file_body.as_slice(), artifact);
        let (field_head, field_body) = got.get(1).expect("the message part");
        assert_eq!(
            field_head,
            "Content-Disposition: form-data; name=\"message\""
        );
        assert_eq!(field_body.as_slice(), b"seed: legacy layout");
    }
}
