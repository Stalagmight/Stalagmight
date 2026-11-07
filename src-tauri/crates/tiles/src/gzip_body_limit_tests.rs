//! #576 RV19a-1: the tile and tilemap body ceilings (`body_limit_tests`,
//! `tilemap_body_limit_tests`) bound the DECODED body. ureq, with its default
//! `gzip` feature, sends `accept-encoding: gzip` and inflates above the byte
//! limit `with_config().limit()` sets, so that limit counts compressed bytes:
//! a few KB of gzip decoding to far more than either ceiling was read whole.
//!
//! No compression crate is a dependency, so the gzip is built here by hand:
//! one final fixed-Huffman deflate block (RFC 1951 3.2.6) holding a prefix as
//! literals and then a long run of one byte as repeated (length 258,
//! distance 1) matches, inside a gzip member (RFC 1952) with its CRC-32 and
//! size trailer. ureq's own decoder checks both, so the under-ceiling tests
//! below are what prove the encoder right before the over-ceiling ones rely
//! on it.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::tilemap_body_limit_tests::tilemap_body;
use super::*;

/// Deflate's bit order: bits fill each byte from its least significant end.
struct BitWriter {
    out: Vec<u8>,
    acc: u8,
    filled: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            out: Vec::new(),
            acc: 0,
            filled: 0,
        }
    }

    fn bit(&mut self, bit: u32) {
        self.acc |= ((bit & 1) as u8) << self.filled;
        self.filled += 1;
        if self.filled == 8 {
            self.out.push(self.acc);
            self.acc = 0;
            self.filled = 0;
        }
    }

    /// A header field or extra-bits value: least significant bit first.
    fn value(&mut self, value: u32, len: u32) {
        for i in 0..len {
            self.bit(value >> i);
        }
    }

    /// A Huffman code, which deflate packs MOST significant bit first.
    fn code(&mut self, code: u32, len: u32) {
        for i in (0..len).rev() {
            self.bit(code >> i);
        }
    }

    /// A literal byte in the fixed code: 0-143 are the 8-bit codes from
    /// 0x30, 144-255 the 9-bit codes from 0x190.
    fn literal(&mut self, byte: u8) {
        if byte < 144 {
            self.code(0x30 + u32::from(byte), 8);
        } else {
            self.code(0x190 + u32::from(byte) - 144, 9);
        }
    }

    /// Copy the previous byte 258 more times: length 258 is symbol 285 (the
    /// 8-bit code 0xC5, no extra bits), distance 1 the 5-bit code 0.
    fn repeat_previous_258(&mut self) {
        self.code(0xC5, 8);
        self.code(0, 5);
    }

    fn finish(mut self) -> Vec<u8> {
        if self.filled > 0 {
            self.out.push(self.acc);
        }
        self.out
    }
}

/// CRC-32 as gzip's trailer carries it (IEEE, reflected polynomial).
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// `prefix` followed by `run_len` copies of `run_byte`: the decoded body, and
/// its gzip encoding.
fn gzip_prefix_then_run(prefix: &[u8], run_byte: u8, run_len: usize) -> (Vec<u8>, Vec<u8>) {
    let mut deflate = BitWriter::new();
    deflate.value(1, 1); // BFINAL: the only block
    deflate.value(1, 2); // BTYPE 01: fixed Huffman codes
    for &byte in prefix {
        deflate.literal(byte);
    }
    if run_len > 0 {
        deflate.literal(run_byte);
        let mut left = run_len - 1;
        while left >= 258 {
            deflate.repeat_previous_258();
            left -= 258;
        }
        for _ in 0..left {
            deflate.literal(run_byte);
        }
    }
    deflate.code(0, 7); // end of block: symbol 256, the 7-bit code 0

    let mut decoded = prefix.to_vec();
    decoded.resize(prefix.len() + run_len, run_byte);

    // ID1 ID2, CM 8 (deflate), no flags, mtime 0, xfl 0, OS 255 (unknown).
    let mut gzip = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255];
    gzip.extend_from_slice(&deflate.finish());
    gzip.extend_from_slice(&crc32(&decoded).to_le_bytes());
    gzip.extend_from_slice(&(decoded.len() as u32).to_le_bytes());
    (decoded, gzip)
}

/// A loopback stub answering every request with `200 OK`, `content_type`,
/// `Content-Encoding: gzip` and `gzip` as the body (`Content-Length` is the
/// compressed length, as a real server sends it).
struct GzipStub {
    base: String,
    accepted: Arc<AtomicUsize>,
    /// Whether a request advertised `accept-encoding: gzip`, i.e. whether a
    /// real server would have been invited to answer this way.
    asked_for_gzip: Arc<AtomicBool>,
}

fn serve_gzip(content_type: &'static str, gzip: Vec<u8>) -> GzipStub {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let accepted = Arc::new(AtomicUsize::new(0));
    let asked_for_gzip = Arc::new(AtomicBool::new(false));
    let (counter, asked) = (Arc::clone(&accepted), Arc::clone(&asked_for_gzip));
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
            if String::from_utf8_lossy(&head)
                .to_ascii_lowercase()
                .contains("accept-encoding: gzip")
            {
                asked.store(true, Ordering::SeqCst);
            }
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\n\
                 Content-Encoding: gzip\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n",
                gzip.len()
            );
            // The client hangs up once it hits its ceiling; a failed write
            // is that, not a stub fault.
            let _ = stream.write_all(reply.as_bytes());
            let _ = stream.write_all(&gzip);
        }
    });
    GzipStub {
        base,
        accepted,
        asked_for_gzip,
    }
}

/// A tile body of exactly 1 MiB: a PNG signature, then a run of zero bytes.
/// Its gzip is a few KB.
fn one_mib_tile() -> (Vec<u8>, Vec<u8>) {
    const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    gzip_prefix_then_run(PNG_SIGNATURE, 0, 1024 * 1024 - PNG_SIGNATURE.len())
}

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

/// The encoder self-check, and decoding still works: the 1 MiB tile's gzip,
/// read under the production 4 MiB ceiling, comes back byte for byte.
#[test]
fn a_gzip_tile_under_the_ceiling_is_read_whole_and_decoded() {
    let (decoded, gzip) = one_mib_tile();
    let wire = gzip.len();
    let stub = serve_gzip("image/png", gzip);
    let url = format!("{}/tile/1/2/3", stub.base);
    let result = fetch_tile_from_url(&url, "esri", 1, 2, 3, None, None, TILE_BODY_MAX_BYTES);
    println!(
        "gzip tile, {wire} bytes on the wire decoding to {}, 4 MiB ceiling: {}; \
         client asked for gzip: {}",
        decoded.len(),
        describe(&result),
        stub.asked_for_gzip.load(Ordering::SeqCst)
    );
    assert!(
        matches!(&result, Ok(TileFetchResponse::Fetched(tile)) if tile.data == decoded),
        "the gzip body must decode to the bytes it was built from, got {}",
        describe(&result)
    );
    assert_eq!(stub.accepted.load(Ordering::SeqCst), 1);
}

/// The same body against a 16 KiB ceiling. Its gzip is under the ceiling, so
/// only a cap on the decoded bytes can refuse it - with the limit named, after
/// ONE connection, classified as the provider answering (no stale copy, not
/// transient), exactly like a plain body over the ceiling.
#[test]
fn a_gzip_tile_decoding_past_the_ceiling_is_refused_once_not_retried_as_transient() {
    let limit: u64 = 16 * 1024;
    let (decoded, gzip) = one_mib_tile();
    let wire = gzip.len();
    assert!(
        (wire as u64) < limit,
        "the compressed body ({wire} bytes) must be under the ceiling for this test to mean anything"
    );
    let stub = serve_gzip("image/png", gzip);
    let url = format!("{}/tile/1/2/3", stub.base);
    let result = fetch_tile_from_url(&url, "esri", 1, 2, 3, None, None, limit);
    let connections = stub.accepted.load(Ordering::SeqCst);
    println!(
        "gzip tile, {wire} bytes on the wire decoding to {}, 16 KiB ceiling: {}; connections: {connections}",
        decoded.len(),
        describe(&result)
    );

    assert!(
        result.is_err(),
        "a body decoding past the ceiling must be refused, got {}",
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

/// Serve `body` gzip-encoded as a tilemap; returns a provider pointing at it
/// and the compressed length.
fn serve_gzip_tilemap(body: &[u8]) -> (tile_providers::TileProvider, usize) {
    // `tilemap_body` is the JSON followed by spaces: the JSON goes in as
    // literals, the spaces as the run.
    let json_len = body.iter().rposition(|&b| b != b' ').map_or(0, |i| i + 1);
    let (decoded, gzip) = gzip_prefix_then_run(&body[..json_len], b' ', body.len() - json_len);
    assert_eq!(decoded, body);
    let gzip_len = gzip.len();
    let stub = serve_gzip("application/json", gzip);
    let mut provider = tile_providers::get_provider("esri").unwrap().clone();
    provider.tilemap_url_template =
        Some(format!("{}/tilemap/{{z}}/{{y}}/{{x}}/128/128", stub.base));
    (provider, gzip_len)
}

fn describe_tilemap<E: std::fmt::Display>(result: &Result<Vec<bool>, E>) -> String {
    match result {
        Ok(flags) => format!("Ok: {} flags read", flags.len()),
        Err(e) => format!("Err: {e}"),
    }
}

/// A tilemap whose gzip is far under the 1 MiB ceiling but decodes to over
/// 2 MiB (a well-formed tilemap, then whitespace) is refused, naming the limit.
#[test]
fn a_gzip_tilemap_decoding_past_the_ceiling_is_refused() {
    let body = tilemap_body(2 * 1024 * 1024);
    let (provider, gzip_len) = serve_gzip_tilemap(&body);
    assert!((gzip_len as u64) < TILEMAP_BODY_MAX_BYTES);
    let result = fetch_tilemap(&provider, 10, 0, 0);
    let described = describe_tilemap(&result);
    println!(
        "gzip tilemap, {gzip_len} bytes on the wire decoding to {}, 1 MiB ceiling: {described}",
        body.len()
    );
    assert!(
        result.is_err(),
        "a tilemap decoding past the ceiling must be refused, got {described}"
    );
    assert!(
        described.contains("1 MiB"),
        "the refusal must name the limit, got {described}"
    );
}

/// An ordinary gzip tilemap (here padded to half the ceiling) still decodes and
/// parses: the cap refuses only what no tilemap can be.
#[test]
fn a_gzip_tilemap_under_the_ceiling_is_read_whole() {
    let body = tilemap_body(512 * 1024);
    let (provider, gzip_len) = serve_gzip_tilemap(&body);
    let result = fetch_tilemap(&provider, 10, 0, 0);
    println!(
        "gzip tilemap, {gzip_len} bytes on the wire decoding to {}: {}",
        body.len(),
        describe_tilemap(&result)
    );
    let flags = result.unwrap();
    assert_eq!(
        flags.len(),
        (tilemap_cache::TILEMAP_GRID_SIZE * tilemap_cache::TILEMAP_GRID_SIZE) as usize
    );
    assert!(flags.iter().all(|available| *available));
}
