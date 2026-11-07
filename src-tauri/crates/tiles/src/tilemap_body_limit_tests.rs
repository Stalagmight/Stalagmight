//! #576 W30-NF4: a tilemap response body is read under an explicit ceiling, as
//! a tile's is (W30-7a, `body_limit_tests`). It was read with only ureq's
//! default 10 MiB limit, though a real tilemap - 128 x 128 availability flags
//! as compact JSON - is about 33 KB.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

/// The tilemap a provider sends for a region: every flag set, compact JSON,
/// then `padding` bytes of whitespace (which JSON allows after the value).
pub(super) fn tilemap_body(padding: usize) -> Vec<u8> {
    let cells = (tilemap_cache::TILEMAP_GRID_SIZE * tilemap_cache::TILEMAP_GRID_SIZE) as usize;
    let flags = vec!["1"; cells].join(",");
    let mut body = format!("{{\"adjusted\":false,\"data\":[{flags}]}}").into_bytes();
    body.resize(body.len() + padding, b' ');
    body
}

/// A loopback stub answering every request with `200 OK` and `body` as JSON.
/// Returns a provider whose tilemap URL points at it.
fn serve_tilemap(body: Vec<u8>) -> tile_providers::TileProvider {
    serve_tilemap_counted("200 OK", body).0
}

/// [`serve_tilemap`] with the status line spelled out, also returning the
/// count of connections the stub accepted (each one is a tilemap download).
fn serve_tilemap_counted(
    status: &'static str,
    body: Vec<u8>,
) -> (tile_providers::TileProvider, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
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
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
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
    (provider, accepted)
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
        err.to_string().contains("1 MiB"),
        "the refusal must name the limit, got {err}"
    );
    assert!(
        matches!(err, TilemapFetchError::TooLarge(_)),
        "classified as a size refusal, which the tilemap cache remembers"
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

/// #576 RV19a-10: a region whose tilemap was refused for its size is not
/// downloaded again for every tile in it. The refusal used to be logged and
/// forgotten, so each tile request in the 128 x 128 region pulled up to the
/// 1 MiB ceiling again. Availability stays unknown (`None`) - the tiles are
/// still fetched, just without the tilemap's verdict.
#[test]
fn a_tilemap_refused_for_its_size_is_not_downloaded_again_for_its_region() {
    let (provider, accepted) = serve_tilemap_counted("200 OK", tilemap_body(2 * 1024 * 1024));
    let tilemap_cache = Arc::new(RecoveringMutex::new("tilemap_cache", TilemapCache::new()));

    let first = check_tile_availability(&tilemap_cache, &provider, 10, 5, 7);
    let neighbour = check_tile_availability(&tilemap_cache, &provider, 10, 100, 120);
    let same_again = check_tile_availability(&tilemap_cache, &provider, 10, 5, 7);
    let in_region = accepted.load(Ordering::SeqCst);
    println!(
        "over-cap tilemap, three tiles of one region: {first:?} {neighbour:?} {same_again:?}; \
         tilemap downloads: {in_region}"
    );
    assert_eq!((first, neighbour, same_again), (None, None, None));
    assert_eq!(
        in_region, 1,
        "a refused tilemap must not be downloaded again for its own region"
    );

    // The refusal belongs to that region only: the next region asks.
    let other_region = check_tile_availability(&tilemap_cache, &provider, 10, 5, 7 + 128);
    let total = accepted.load(Ordering::SeqCst);
    println!("a tile of the next region: {other_region:?}; tilemap downloads: {total}");
    assert_eq!(other_region, None);
    assert_eq!(total, 2, "another region's tilemap is still fetched");
}

/// Only a refusal for size is remembered. A failure that may clear up - here
/// a 503 - leaves the region to be asked again, as before.
#[test]
fn a_tilemap_that_failed_otherwise_is_asked_for_again() {
    let (provider, accepted) = serve_tilemap_counted("503 Service Unavailable", Vec::new());
    let tilemap_cache = Arc::new(RecoveringMutex::new("tilemap_cache", TilemapCache::new()));
    let first = check_tile_availability(&tilemap_cache, &provider, 10, 5, 7);
    let again = check_tile_availability(&tilemap_cache, &provider, 10, 5, 7);
    let connections = accepted.load(Ordering::SeqCst);
    println!("503 tilemap, same tile twice: {first:?} {again:?}; tilemap downloads: {connections}");
    assert_eq!((first, again), (None, None));
    assert_eq!(connections, 2);
}
