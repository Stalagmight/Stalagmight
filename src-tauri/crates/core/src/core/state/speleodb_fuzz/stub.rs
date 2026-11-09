//! [FZ-SDB] "Another SpeleoDB deployment": a minimal in-process server the user
//! can sign in to, which knows none of the user's projects.
//!
//! Routes (bare JSON, as SpeleoDB v2 answers; every reply `Connection: close`):
//! - `POST /api/v2/user/auth-token/` -> 200 `{"token": TOKEN, "user": EMAIL}`
//!   (the SDK's `AuthTokenResponse`: both fields required)
//! - `GET /api/v2/user/` with a Bearer -> 200 the SDK's `User` (every required
//!   field, country `US`); 403 without one
//! - `GET /api/v2/projects/` -> 200 `[]`
//! - anything else, every `/api/v2/projects/<id>/...` included -> 404
//!   `{"detail":"Not found."}`
//!
//! Requests are served one at a time on the accept thread; a request body is
//! read by its Content-Length (the client never sends one chunked here).

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// The auth token every sign-in gets: 40 lowercase hex characters, like a real one.
const TOKEN: &str = "c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00";
/// How long the stub waits on a client that stops sending or reading.
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const NOT_FOUND: &str = r#"{"detail":"Not found."}"#;

/// The other deployment. Dropping it stops it.
pub(super) struct OtherServer {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl OtherServer {
    /// Bind `127.0.0.1:0` and serve `email`'s account.
    pub(super) fn start(email: &str) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let addr = listener.local_addr()?;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let replies = Replies {
            token: serde_json::json!({ "token": TOKEN, "user": email }).to_string(),
            user: serde_json::json!({
                "country": "US",
                "email": email,
                "email_on_projects_updates": false,
                "email_on_speleodb_updates": false,
                "name": "Other Server",
            })
            .to_string(),
        };
        let accept = thread::Builder::new()
            .name("fz-sdb-other-server".to_owned())
            .spawn({
                let (seen, stop) = (Arc::clone(&seen), Arc::clone(&stop));
                move || {
                    for conn in listener.incoming() {
                        if stop.load(Ordering::SeqCst) {
                            return;
                        }
                        if let Ok(stream) = conn {
                            serve(stream, &replies, &seen);
                        }
                    }
                }
            })?;
        Ok(Self {
            addr,
            seen,
            stop,
            accept: Some(accept),
        })
    }

    /// `"http://127.0.0.1:<port>/"`.
    pub(super) fn base_url(&self) -> String {
        format!("http://{}/", self.addr)
    }

    /// Every request served, as `"METHOD /path"`, in order.
    pub(super) fn seen(&self) -> Vec<String> {
        lock(&self.seen).clone()
    }
}

impl Drop for OtherServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocking accept; joined only when the wake landed, so a
        // failed wake can never hang the drop.
        let woke = TcpStream::connect_timeout(&self.addr, Duration::from_secs(1)).is_ok();
        if let Some(accept) = self.accept.take().filter(|_| woke) {
            let _ = accept.join();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The two account bodies, built once per stub.
struct Replies {
    token: String,
    user: String,
}

/// The parts of a request the routes look at.
struct Request {
    method: String,
    path: String,
    bearer: bool,
}

/// Read one request, its body drained so closing never resets the reply away.
fn read_request(reader: &mut impl BufRead) -> Option<Request> {
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut words = line.split_whitespace();
    let method = words.next()?.to_owned();
    let path = words.next()?.to_owned();
    let mut content_length = 0;
    let mut bearer = false;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 {
            return None;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.parse().ok()?;
        } else if name.eq_ignore_ascii_case("authorization") {
            bearer = value
                .strip_prefix("Bearer ")
                .is_some_and(|t| !t.trim().is_empty());
        }
    }
    io::copy(&mut reader.take(content_length), &mut io::sink()).ok()?;
    Some(Request {
        method,
        path,
        bearer,
    })
}

/// The status and JSON body a request gets.
fn route(request: &Request, replies: &Replies) -> (u16, String) {
    let path = request.path.split('?').next().unwrap_or_default();
    match (request.method.as_str(), path) {
        ("POST", "/api/v2/user/auth-token/") => (200, replies.token.clone()),
        ("GET", "/api/v2/user/") if request.bearer => (200, replies.user.clone()),
        ("GET", "/api/v2/user/") => (
            403,
            r#"{"detail":"Authentication credentials were not provided."}"#.to_owned(),
        ),
        ("GET", "/api/v2/projects/") => (200, "[]".to_owned()),
        _ => (404, NOT_FOUND.to_owned()),
    }
}

fn serve(stream: TcpStream, replies: &Replies, seen: &Mutex<Vec<String>>) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let Some(request) = read_request(&mut BufReader::new(read_half)) else {
        let _ = stream.shutdown(Shutdown::Both);
        return;
    };
    lock(seen).push(format!("{} {}", request.method, request.path));
    let (status, body) = route(&request, replies);
    let reason = match status {
        200 => "OK",
        403 => "Forbidden",
        _ => "Not Found",
    };
    let reply = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    let mut writer = stream;
    let _ = writer.write_all(reply.as_bytes());
    let _ = writer.flush();
    let _ = writer.shutdown(Shutdown::Both);
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    use super::{OtherServer, TOKEN};
    use crate::speleodb::{client, SpeleoDbError};

    const EMAIL: &str = "other@example.invalid";

    #[test]
    fn the_real_client_signs_in_names_the_user_and_finds_no_projects() {
        let stub = OtherServer::start(EMAIL).expect("start the stub");
        let base = stub.base_url();
        let token = client::login(&base, false, EMAIL, "pw").expect("sign in");
        assert_eq!(token, TOKEN);
        assert_eq!(token.len(), 40);
        assert!(token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        // Country "US", so this is the SDK's own `User` parse, not the client's
        // empty-country fallback.
        assert_eq!(
            client::fetch_user_email(&base, &token, false).expect("whoami"),
            EMAIL
        );
        assert!(client::list_projects(&base, &token, false)
            .expect("list projects")
            .is_empty());
        let err = client::get_project(&base, &token, false, "0b7e8f4c-2f43-4c55-9a43-6f1d2b3c4d5e")
            .expect_err("an unknown project");
        assert!(
            matches!(err, SpeleoDbError::Status { status: 404, ref message } if message == "Not found."),
            "{err:?}"
        );
        assert_eq!(
            stub.seen(),
            vec![
                "POST /api/v2/user/auth-token/",
                "GET /api/v2/user/",
                "GET /api/v2/projects/",
                "GET /api/v2/projects/0b7e8f4c-2f43-4c55-9a43-6f1d2b3c4d5e/",
            ]
        );
    }

    #[test]
    fn whoami_needs_a_bearer() {
        let stub = OtherServer::start(EMAIL).expect("start the stub");
        let mut stream = TcpStream::connect(stub.addr).expect("connect to the stub");
        stream
            .write_all(b"GET /api/v2/user/ HTTP/1.1\r\nHost: stub\r\n\r\n")
            .expect("send a request without a bearer");
        let mut reply = String::new();
        stream.read_to_string(&mut reply).expect("read the reply");
        assert!(reply.starts_with("HTTP/1.1 403 "), "{reply}");
        assert!(reply.contains("Connection: close"), "{reply}");
        assert_eq!(stub.seen(), vec!["GET /api/v2/user/"]);
    }

    #[test]
    fn dropping_the_stub_stops_it() {
        let stub = OtherServer::start(EMAIL).expect("start the stub");
        let addr = stub.addr;
        drop(stub);
        assert!(TcpStream::connect(addr).is_err(), "the listener closed");
    }
}
