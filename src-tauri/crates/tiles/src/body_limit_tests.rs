//! #576 W30-7a: a tile response body is read under an explicit ceiling. One
//! over it is a refusal - the provider answered, with more than any tile can
//! be - so it is not retried, not served off a stored copy, and not treated
//! as "the tile may exist" (which would re-download it on every request).

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

/// A loopback stub answering every request with `200 OK` and a `len`-byte
/// PNG-typed body. Returns a tile URL on it and the count of connections it
/// accepted, so a test can see whether the client came back for a re-download.
fn serve_body(len: usize) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/tile/1/2/3", listener.local_addr().unwrap());
    let accepted = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&accepted);
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            counter.fetch_add(1, Ordering::SeqCst);
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                match stream.read(&mut byte) {
                    Ok(1) => head.extend_from_slice(&byte),
                    _ => break,
                }
            }
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {len}\r\n\
                 Connection: close\r\n\r\n"
            );
            // The client hangs up once it hits its ceiling; a failed write
            // is that, not a stub fault.
            let _ = stream.write_all(reply.as_bytes());
            let _ = stream.write_all(&vec![0u8; len]);
        }
    });
    (url, accepted)
}

/// What a fetch produced, in words for the test log.
fn describe(result: &Result<TileFetchResponse, TileFetchError>) -> String {
    match result {
        Ok(TileFetchResponse::Fetched(tile)) => format!("Ok: fetched {} bytes", tile.data.len()),
        Ok(TileFetchResponse::NotModified { .. }) => "Ok: 304".to_string(),
        Err(e) => format!(
            "Err: {e} (status {:?}, stale fallback {}, existence unknown {})",
            e.status,
            e.allows_stale_fallback(),
            e.left_existence_unknown()
        ),
    }
}

/// The ceiling bites: a 64 KiB body against a 16 KiB ceiling is refused with
/// the limit named, after ONE connection, and is classified as an answer -
/// no stale copy may paper over it, and it does not leave the tile's
/// existence unknown (the `transient` that would make every later request
/// fetch it again).
#[test]
fn a_tile_body_over_the_ceiling_is_refused_once_not_retried_as_transient() {
    let limit: u64 = 16 * 1024;
    let (url, accepted) = serve_body(64 * 1024);
    let result = fetch_tile_from_url(&url, "esri", 1, 2, 3, None, None, limit);
    let connections = accepted.load(Ordering::SeqCst);
    println!(
        "64 KiB body, 16 KiB ceiling: {}; connections: {connections}",
        describe(&result)
    );

    assert!(
        result.is_err(),
        "a body over the ceiling must be refused, got {}",
        describe(&result)
    );
    let err = result.err().unwrap();
    assert!(
        err.to_string().contains("16384 bytes"),
        "the refusal must name the limit, got {err}"
    );
    assert!(
        !err.allows_stale_fallback(),
        "an over-limit body is an answer, not an outage: no stale copy"
    );
    assert!(
        !err.left_existence_unknown(),
        "an over-limit body must not be transient, or it is re-downloaded per request"
    );
    assert!(!is_provider_denial(&err), "it is not a refusal of access");
    assert_eq!(
        connections, 1,
        "the oversized body must not be re-downloaded"
    );
}

/// A body under the ceiling is read whole, so the ceiling is not refusing
/// ordinary tiles; and production reads under the 4 MiB constant.
#[test]
fn a_tile_body_under_the_ceiling_is_read_whole() {
    let (url, accepted) = serve_body(8 * 1024);
    let result = fetch_tile_from_url(&url, "esri", 1, 2, 3, None, None, 16 * 1024);
    println!("8 KiB body, 16 KiB ceiling: {}", describe(&result));
    assert!(
        matches!(&result, Ok(TileFetchResponse::Fetched(tile)) if tile.data.len() == 8 * 1024),
        "{}",
        describe(&result)
    );
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    assert_eq!(TILE_BODY_MAX_BYTES, 4 * 1024 * 1024);
}
