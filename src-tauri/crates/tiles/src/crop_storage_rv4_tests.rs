//! #576 RV4-R-T1: an overzoomed crop is stored under the child's key only
//! when the child is known to be absent. A `429 Too Many Requests` or a
//! `408 Request Timeout` says "ask later", not "absent", so a crop derived
//! through one must be served but not stored: stored, it answers as a fresh
//! cache hit (blurry, with no request made) until the parent's grant runs
//! out, across restarts - the #576 C16 symptom.

use super::*;

/// A real, decodable 256x256 JPEG: the overzoom path decodes the parent.
fn jpeg_bytes() -> Vec<u8> {
    let img = DynamicImage::new_rgb8(256, 256);
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Jpeg).unwrap();
    buf.into_inner()
}

/// Mark a tilemap region as "imagery everywhere", so no availability probe
/// goes to the provider.
fn warm_tilemap(tilemap_cache: &Arc<RecoveringMutex<TilemapCache>>, z: u32, y: u32, x: u32) {
    let (base_y, base_x) = TilemapCache::get_region_origin(y, x);
    let cells = (tilemap_cache::TILEMAP_GRID_SIZE * tilemap_cache::TILEMAP_GRID_SIZE) as usize;
    tilemap_cache
        .lock()
        .insert("esri", z, base_y, base_x, vec![true; cells]);
}

/// A tile cache holding one fresh parent at z9 (250, 125), and a tilemap
/// that says imagery exists at z9 and z10 there.
fn cache_with_a_z9_parent() -> (
    Arc<RecoveringMutex<Option<TileCache>>>,
    Arc<RecoveringMutex<TilemapCache>>,
) {
    let cache = Arc::new(RecoveringMutex::new(
        "tile_cache",
        Some(TileCache::in_memory(10, 30).unwrap()),
    ));
    cache
        .lock()
        .as_mut()
        .unwrap()
        .put_granted("esri", 9, 250, 125, &jpeg_bytes(), "image/jpeg")
        .unwrap();
    let tilemap_cache = Arc::new(RecoveringMutex::new("tilemap_cache", TilemapCache::new()));
    warm_tilemap(&tilemap_cache, 9, 250, 125);
    warm_tilemap(&tilemap_cache, 10, 500, 250);
    (cache, tilemap_cache)
}

/// Request z10 (500, 250) with every fetch answering `code`; report whether
/// the response served and whether a fresh z10 row was stored.
fn crop_after_status(code: u16) -> (http::StatusCode, bool) {
    let (cache, tilemap_cache) = cache_with_a_z9_parent();
    let answer = move |_: &str,
                       _: u32,
                       _: u32,
                       _: u32,
                       _: Option<&str>|
          -> Result<TileFetchResponse, TileFetchError> {
        Err(TileFetchError::status(code, format!("HTTP {code}")))
    };
    let response = handle_tile_request_with_fetcher(
        "esri",
        10,
        500,
        250,
        &cache,
        &tilemap_cache,
        answer,
        true,
    );
    let guard = cache.lock();
    let stored = guard.as_ref().unwrap().has_fresh_tile("esri", 10, 500, 250);
    (response.status(), stored)
}

#[test]
fn a_rate_limited_or_timed_out_child_serves_the_crop_without_storing_it() {
    let outcomes: Vec<_> = [429_u16, 408]
        .into_iter()
        .map(|code| (code, crop_after_status(code)))
        .collect();
    for (code, (status, stored)) in &outcomes {
        println!(
            "RV4-R-T1: HTTP {code} on z10 -> served {status}, crop stored as fresh z10: {stored}"
        );
    }
    for (code, (status, stored)) in outcomes {
        assert_eq!(
            status,
            http::StatusCode::OK,
            "HTTP {code}: the parent crop still serves"
        );
        assert!(
            !stored,
            "HTTP {code} says 'ask later', not 'absent', yet the overzoomed crop was stored \
             as a fresh z10 tile"
        );
    }
}

#[test]
fn a_child_the_provider_says_is_absent_still_stores_the_crop() {
    // The 'answered' side is unchanged: a 404 is an absence, so the crop is
    // the best that tile will be and is stored under the parent's grant.
    let (status, stored) = crop_after_status(404);
    println!("RV4-R-T1: HTTP 404 on z10 -> served {status}, crop stored as fresh z10: {stored}");
    assert_eq!(status, http::StatusCode::OK);
    assert!(stored, "a 404 child still stores the crop");
}

#[test]
fn retry_later_statuses_leave_existence_unknown_and_answers_do_not() {
    for code in [408_u16, 429, 500, 503] {
        assert!(
            TileFetchError::status(code, String::new()).left_existence_unknown(),
            "HTTP {code} does not say whether the tile exists"
        );
    }
    assert!(TileFetchError::transport(String::new()).left_existence_unknown());
    for code in [400_u16, 401, 403, 404, 410] {
        assert!(
            !TileFetchError::status(code, String::new()).left_existence_unknown(),
            "HTTP {code} is the provider answering"
        );
    }
    // The stale fallback is a separate rule and keeps its RFC 5861 scope.
    assert!(!TileFetchError::status(429, String::new()).allows_stale_fallback());
}
