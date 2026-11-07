//! #576 W30-7b: tile bytes are decoded under explicit limits. The overzoom
//! path decodes a cached parent tile; a body that merely CLAIMS huge
//! dimensions in its header used to be decoded under the image crate's
//! defaults only - no side cap, and up to 512 MiB of output buffer - so a
//! hostile or broken tile could make the app allocate hundreds of MiB.

use super::*;

/// CRC-32 (IEEE), the checksum PNG chunks carry, done bitwise: the tests
/// build PNG headers by hand, and the decoder checks every chunk's CRC.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

/// Append one PNG chunk (length, type, data, CRC of type + data).
fn push_chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut typed = kind.to_vec();
    typed.extend_from_slice(data);
    png.extend_from_slice(&typed);
    png.extend_from_slice(&crc32(&typed).to_be_bytes());
}

/// A PNG that is all header: an IHDR claiming `width` x `height` at
/// `bit_depth` / `color_type`, then an IDAT holding an EMPTY zlib stream and
/// IEND. A few dozen bytes that claim an image of any size - exactly what
/// the limits must refuse before a buffer that size is allocated.
fn png_claiming(width: u32, height: u32, bit_depth: u8, color_type: u8) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    // Bit depth, colour type, then deflate / adaptive filtering / no interlace.
    ihdr.extend_from_slice(&[bit_depth, color_type, 0, 0, 0]);
    push_chunk(&mut png, b"IHDR", &ihdr);
    // zlib header, one final stored block of length 0, Adler-32 of nothing.
    push_chunk(
        &mut png,
        b"IDAT",
        &[
            0x78, 0x01, 0x01, 0x00, 0x00, 0xff, 0xff, 0x00, 0x00, 0x00, 0x01,
        ],
    );
    push_chunk(&mut png, b"IEND", &[]);
    png
}

/// A real, decodable image of `width` x `height`.
fn encoded(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    DynamicImage::new_rgb8(width, height)
        .write_to(&mut buf, format)
        .unwrap();
    buf.into_inner()
}

/// Overzoom `parent` (a z9 tile) to its top-left z10 child, through the
/// production path. `cache_key` keys the process-wide decoded-image cache,
/// so each case gets its own entry and parallel tests cannot see each other's.
fn derive_child(cache_key: &str, parent: Vec<u8>) -> Result<TileFetchResult, String> {
    let provider_info = tile_providers::get_provider("esri").unwrap();
    let parent = TileFetchResult {
        data: parent,
        content_type: "image/png".to_string(),
        policy: CachePolicy::default(),
    };
    derive_tile_from_parent(cache_key, provider_info, 9, 4, 6, 10, 8, 12, &parent)
}

/// What a derive produced, in words for the test log.
fn describe(result: &Result<TileFetchResult, String>) -> String {
    match result {
        Ok(tile) => format!("Ok: derived a {}-byte child tile", tile.data.len()),
        Err(e) => format!("Err: {e}"),
    }
}

/// A header claiming 100000 x 100000, or 8192 x 8192 RGBA (256 MiB, inside
/// the image crate's own 512 MiB default), is refused on its dimensions
/// before any buffer is reserved.
#[test]
fn a_tile_whose_header_claims_huge_dimensions_is_refused_before_allocating() {
    let outcomes: Vec<_> = [
        ("w30-7b-100000", 100_000, 100_000, 2),
        ("w30-7b-8192-rgba", 8192, 8192, 6),
    ]
    .into_iter()
    .map(|(key, width, height, color_type)| {
        let result = derive_child(key, png_claiming(width, height, 8, color_type));
        println!("{width} x {height} header: {}", describe(&result));
        (width, height, result.err().unwrap_or_default())
    })
    .collect();
    for (width, height, err) in outcomes {
        assert!(
            err.contains("Image size exceeds limit"),
            "{width} x {height} must be refused on its dimensions, got {err:?}"
        );
    }
}

/// A real image wider than the side cap is refused although it is small in
/// memory (4352 x 256 RGB is 3.3 MB): the cap is on the shape a tile can
/// have, not only on bytes.
#[test]
fn a_tile_wider_than_the_side_cap_is_refused() {
    let result = derive_child("w30-7b-wide", encoded(4352, 256, ImageFormat::Png));
    println!("4352 x 256 PNG: {}", describe(&result));
    let err = result.err().unwrap_or_default();
    assert!(
        err.contains("Image size exceeds limit"),
        "a 4352 px wide tile must be refused, got {err:?}"
    );
}

/// Within the side cap but over the allocation cap: 4096 x 4096 RGBA at 16
/// bits is 128 MiB of output, refused before it is reserved.
#[test]
fn a_tile_over_the_allocation_cap_is_refused_before_allocating() {
    let result = derive_child("w30-7b-4096-rgba16", png_claiming(4096, 4096, 16, 6));
    println!("4096 x 4096 RGBA16 header: {}", describe(&result));
    let err = result.err().unwrap_or_default();
    assert!(
        err.contains("Memory limit exceeded"),
        "128 MiB of output must be refused on the allocation cap, got {err:?}"
    );
}

/// Real tiles - 256 px PNG and JPEG, and a 512 px PNG - still decode and
/// overzoom to a 256 px child.
#[test]
fn a_normal_tile_still_decodes() {
    for (key, size, format) in [
        ("w30-7b-256-png", 256, ImageFormat::Png),
        ("w30-7b-256-jpeg", 256, ImageFormat::Jpeg),
        ("w30-7b-512-png", 512, ImageFormat::Png),
    ] {
        let result = derive_child(key, encoded(size, size, format));
        println!("{size} px {format:?}: {}", describe(&result));
        let child = image::load_from_memory(&result.unwrap().data).unwrap();
        assert_eq!(child.dimensions(), (256, 256), "{size} px {format:?}");
    }
}
