//! A raw HTTP/1.1 request over std, for the one upload the client crate cannot
//! make: an Ariane project's `ariane.tml` alone (the client always uploads a
//! `.stalx` with its sidecar). Loopback only, like everything in the fuzzer.

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

fn loopback_addr(base: &str) -> Result<SocketAddr, String> {
    let (_, addr) = super::target::check_target_url(base)?;
    Ok(addr)
}

/// `PUT base/path` as multipart/form-data with the given file parts and a
/// `message` text field. Returns the status and the raw body.
pub(super) fn multipart_put(
    base: &str,
    path: &str,
    token: &str,
    files: &[(&str, &str, &str, &[u8])],
    message: &str,
) -> Result<(u16, Vec<u8>), String> {
    let addr = loopback_addr(base)?;
    let boundary = "fuzzboundary7f3a9c1e";
    let mut body = Vec::new();
    for (field, file_name, mime, bytes) in files {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; \
                 filename=\"{file_name}\"\r\nContent-Type: {mime}\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"message\"\r\n\r\n{message}\r\n\
             --{boundary}--\r\n"
        )
        .as_bytes(),
    );
    let head = format!(
        "PUT /{path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10))
        .map_err(|e| format!("connect {addr}: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_mins(2)))
        .map_err(|e| format!("timeout: {e}"))?;
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(&body))
        .map_err(|e| format!("send: {e}"))?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| format!("read: {e}"))?;
    parse_response(&raw)
}

/// Split a raw `Connection: close` response into its status and (de-chunked) body.
fn parse_response(raw: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("no header terminator in the response")?;
    let head = String::from_utf8_lossy(raw.get(..split).unwrap_or_default());
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or("no status code in the response")?;
    let body = raw.get(split + 4..).unwrap_or_default().to_vec();
    let chunked = head
        .lines()
        .any(|l| l.to_ascii_lowercase().starts_with("transfer-encoding:") && l.contains("chunked"));
    Ok((status, if chunked { dechunk(&body) } else { body }))
}

fn dechunk(mut rest: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let Some(eol) = rest.windows(2).position(|w| w == b"\r\n") else {
            return out;
        };
        let size_text = String::from_utf8_lossy(rest.get(..eol).unwrap_or_default()).to_string();
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("0").trim(), 16)
            .unwrap_or(0);
        if size == 0 {
            return out;
        }
        let start = eol + 2;
        out.extend_from_slice(rest.get(start..start + size).unwrap_or_default());
        rest = rest.get(start + size + 2..).unwrap_or_default();
    }
}

#[cfg(test)]
mod tests {
    use super::parse_response;

    #[test]
    fn responses_parse_with_and_without_chunking() {
        let plain = b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\nok";
        assert_eq!(parse_response(plain), Ok((201, b"ok".to_vec())));
        let chunked =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n";
        assert_eq!(parse_response(chunked), Ok((200, b"abcde".to_vec())));
        assert!(parse_response(b"garbage").is_err());
    }
}
