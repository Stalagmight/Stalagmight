//! #576 W30-NF4: a tilemap response body is read under an explicit ceiling, as
//! a tile's is (W30-7a, `body_limit_tests`). It was read with only ureq's
//! default 10 MiB limit, though a real tilemap - 128 x 128 availability flags
//! as compact JSON - is about 33 KB.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;

use super::*;

/// The tilemap a provider sends for a region: every flag set, compact JSON,
/// then `padding` bytes of whitespace (which JSON allows after the value).
fn tilemap_body(padding: usize) -> Vec<u8> {
    let cells = (tilemap_cache::TILEMAP_GRID_SIZE * tilemap_cache::TILEMAP_GRID_SIZE) as usize;
    let flags = vec!["1"; cells].join(",");
    let mut body = format!("{{\"adjusted\":false,\"data\":[{flags}]}}").into_bytes();
    body.resize(body.len() + padding, b' ');
    body
}

/// A loopback stub answering every request with `200 OK` and `body` as JSON.
/// Returns a provider whose tilemap URL points at it.
fn serve_tilemap(body: Vec<u8>) -> tile_providers::TileProvider {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                match stream.read(&mut byte) {
                    Ok(1) => head.extend_from_slice(&byte),
                    _ => break,
                }
            }
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n",
                body.len()
            );
            // The client hangs up once it hits its ceiling; a failed write
            // is that, not a stub fault.
            let _ = stream.write_all(reply.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    let mut provider = tile_providers::get_provider("esri").unwrap().clone();
    provider.tilemap_url_template = Some(format!("{base}/tilemap/{{z}}/{{y}}/{{x}}/128/128"));
    provider
}

/// A tilemap body of 2 MiB - a well-formed tilemap followed by whitespace, so
/// only its size is wrong - is refused, with the limit named. It used to be
/// read whole and accepted.
#[test]
fn a_tilemap_body_over_the_ceiling_is_refused() {
    let provider = serve_tilemap(tilemap_body(2 * 1024 * 1024));
    let result = fetch_tilemap(&provider, 10, 0, 0);
    println!(
        "2 MiB tilemap body: {}",
        match &result {
            Ok(flags) => format!("Ok: {} flags read", flags.len()),
            Err(e) => format!("Err: {e}"),
        }
    );
    let err = result.err().unwrap();
    assert!(
        err.contains("1 MiB"),
        "the refusal must name the limit, got {err}"
    );
}

/// An ordinary tilemap is read whole: the ceiling refuses only bodies no
/// tilemap can be.
#[test]
fn a_tilemap_body_under_the_ceiling_is_read_whole() {
    let body = tilemap_body(0);
    let len = body.len();
    let provider = serve_tilemap(body);
    let flags = fetch_tilemap(&provider, 10, 0, 0).unwrap();
    println!("{len}-byte tilemap body: {} flags read", flags.len());
    assert_eq!(
        flags.len(),
        (tilemap_cache::TILEMAP_GRID_SIZE * tilemap_cache::TILEMAP_GRID_SIZE) as usize
    );
    assert!(flags.iter().all(|available| *available));
}
