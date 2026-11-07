//! The tile engine's tests, moved with it from the app crate's `lib.rs`
//! (#576 C8). No network: every fetch goes through a mock `fetch_fn`, and
//! the overzoom tests pre-warm the tilemap cache.

use super::*;
use std::sync::atomic::AtomicU64;
use std::sync::Mutex;

// === parse_tile_uri tests ===

#[test]
fn test_parse_tile_uri_path_based() {
    let uri: http::Uri = "stalagmight-tiles://localhost/esri/10/500/250"
        .parse()
        .unwrap();
    let result = parse_tile_uri(&uri);
    assert_eq!(result, Some(("esri".to_string(), 10, 500, 250)));
}

#[test]
fn test_parse_tile_uri_path_based_with_extension() {
    let uri: http::Uri = "stalagmight-tiles://localhost/esri/10/500/250.jpg"
        .parse()
        .unwrap();
    let result = parse_tile_uri(&uri);
    assert_eq!(result, Some(("esri".to_string(), 10, 500, 250)));
}

#[test]
fn test_parse_tile_uri_host_based() {
    let uri: http::Uri = "stalagmight-tiles://esri/10/500/250".parse().unwrap();
    let result = parse_tile_uri(&uri);
    assert_eq!(result, Some(("esri".to_string(), 10, 500, 250)));
}

#[test]
fn test_parse_tile_uri_malformed_no_host() {
    // Malformed URI with numeric first segment but no valid host
    let uri: http::Uri = "stalagmight-tiles://localhost/10/500/250".parse().unwrap();
    let result = parse_tile_uri(&uri);
    // First segment is "10" which parses as z, host is "localhost" which is rejected
    assert_eq!(result, None);
}

#[test]
fn test_parse_tile_uri_invalid_coordinates() {
    let uri: http::Uri = "stalagmight-tiles://localhost/esri/abc/500/250"
        .parse()
        .unwrap();
    let result = parse_tile_uri(&uri);
    assert_eq!(result, None);
}

#[test]
fn test_parse_tile_uri_too_few_segments() {
    let uri: http::Uri = "stalagmight-tiles://localhost/esri/10/500".parse().unwrap();
    let result = parse_tile_uri(&uri);
    assert_eq!(result, None);
}

#[test]
fn test_parse_tile_uri_empty_path() {
    let uri: http::Uri = "stalagmight-tiles://localhost/".parse().unwrap();
    let result = parse_tile_uri(&uri);
    assert_eq!(result, None);
}

#[expect(clippy::unnecessary_wraps)]
fn expect(provider: &str, z: u32, y: u32, x: u32) -> Option<(String, u32, u32, u32)> {
    Some((provider.to_string(), z, y, x))
}

macro_rules! parse_case {
    ($name:ident, $uri:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let uri: http::Uri = $uri.parse().unwrap();
            let result = parse_tile_uri(&uri);
            assert_eq!(result, $expected);
        }
    };
}

parse_case!(
    test_parse_tile_uri_path_based_png_extension,
    "stalagmight-tiles://localhost/esri/10/500/250.png",
    expect("esri", 10, 500, 250)
);
parse_case!(
    test_parse_tile_uri_path_based_jpeg_extension,
    "stalagmight-tiles://localhost/esri/10/500/250.jpeg",
    expect("esri", 10, 500, 250)
);
parse_case!(
    test_parse_tile_uri_path_based_leading_zeros,
    "stalagmight-tiles://localhost/esri/01/002/0003",
    expect("esri", 1, 2, 3)
);
parse_case!(
    test_parse_tile_uri_path_based_uppercase_provider,
    "stalagmight-tiles://localhost/ESRI/10/500/250",
    expect("ESRI", 10, 500, 250)
);
parse_case!(
    test_parse_tile_uri_path_based_provider_with_dash,
    "stalagmight-tiles://localhost/esri-v2/10/500/250",
    expect("esri-v2", 10, 500, 250)
);
parse_case!(
    test_parse_tile_uri_path_based_provider_with_dot,
    "stalagmight-tiles://localhost/esri.v1/10/500/250",
    expect("esri.v1", 10, 500, 250)
);
parse_case!(
    test_parse_tile_uri_path_based_extra_segments,
    "stalagmight-tiles://localhost/esri/10/500/250/extra",
    expect("esri", 10, 500, 250)
);
parse_case!(
    test_parse_tile_uri_path_based_trailing_slash,
    "stalagmight-tiles://localhost/esri/10/500/250/",
    expect("esri", 10, 500, 250)
);
parse_case!(
    test_parse_tile_uri_path_based_query,
    "stalagmight-tiles://localhost/esri/10/500/250?token=abc",
    expect("esri", 10, 500, 250)
);
parse_case!(
    test_parse_tile_uri_path_based_zero_coords,
    "stalagmight-tiles://localhost/esri/0/0/0",
    expect("esri", 0, 0, 0)
);
parse_case!(
    test_parse_tile_uri_path_based_large_coords,
    "stalagmight-tiles://localhost/esri/4294967295/4294967295/4294967295",
    expect("esri", 4_294_967_295, 4_294_967_295, 4_294_967_295)
);
parse_case!(
    test_parse_tile_uri_path_based_with_port,
    "stalagmight-tiles://localhost:1234/esri/10/500/250",
    expect("esri", 10, 500, 250)
);
parse_case!(
    test_parse_tile_uri_path_based_x_double_extension,
    "stalagmight-tiles://localhost/esri/10/500/250.png.jpg",
    expect("esri", 10, 500, 250)
);
parse_case!(
    test_parse_tile_uri_path_based_x_trailing_dot,
    "stalagmight-tiles://localhost/esri/10/500/250.",
    expect("esri", 10, 500, 250)
);
parse_case!(
    test_parse_tile_uri_path_based_x_decimal_truncates,
    "stalagmight-tiles://localhost/esri/10/500/250.5",
    expect("esri", 10, 500, 250)
);
parse_case!(
    test_parse_tile_uri_path_based_invalid_y,
    "stalagmight-tiles://localhost/esri/10/abc/250",
    None
);
parse_case!(
    test_parse_tile_uri_path_based_invalid_x,
    "stalagmight-tiles://localhost/esri/10/500/abc",
    None
);
parse_case!(
    test_parse_tile_uri_path_based_invalid_z,
    "stalagmight-tiles://localhost/esri/abc/500/250",
    None
);
parse_case!(
    test_parse_tile_uri_path_based_negative_z,
    "stalagmight-tiles://localhost/esri/-1/500/250",
    None
);
parse_case!(
    test_parse_tile_uri_path_based_negative_y,
    "stalagmight-tiles://localhost/esri/10/-1/250",
    None
);
parse_case!(
    test_parse_tile_uri_path_based_negative_x,
    "stalagmight-tiles://localhost/esri/10/500/-1",
    None
);
parse_case!(
    test_parse_tile_uri_path_based_missing_x_trailing_slash,
    "stalagmight-tiles://localhost/esri/10/500/",
    None
);
parse_case!(
    test_parse_tile_uri_path_based_only_provider,
    "stalagmight-tiles://localhost/esri",
    None
);
parse_case!(
    test_parse_tile_uri_path_based_provider_and_z,
    "stalagmight-tiles://localhost/esri/10",
    None
);
parse_case!(
    test_parse_tile_uri_path_based_double_slash,
    "stalagmight-tiles://localhost/esri//500/250",
    None
);
parse_case!(
    test_parse_tile_uri_path_based_z_float,
    "stalagmight-tiles://localhost/esri/10.5/500/250",
    None
);
parse_case!(
    test_parse_tile_uri_path_based_y_float,
    "stalagmight-tiles://localhost/esri/10/500.1/250",
    None
);
parse_case!(
    test_parse_tile_uri_host_based_png_extension,
    "stalagmight-tiles://esri/10/500/250.png",
    expect("esri", 10, 500, 250)
);
parse_case!(
    test_parse_tile_uri_host_based_uppercase_provider,
    "stalagmight-tiles://ESRI/10/500/250",
    expect("ESRI", 10, 500, 250)
);
parse_case!(
    test_parse_tile_uri_host_based_provider_with_dot,
    "stalagmight-tiles://esri.v1/10/500/250",
    expect("esri.v1", 10, 500, 250)
);
parse_case!(
    test_parse_tile_uri_host_based_provider_numeric,
    "stalagmight-tiles://123/10/500/250",
    expect("123", 10, 500, 250)
);
parse_case!(
    test_parse_tile_uri_host_based_with_port,
    "stalagmight-tiles://esri:1234/10/500/250",
    expect("esri", 10, 500, 250)
);
parse_case!(
    test_parse_tile_uri_host_based_extra_segments,
    "stalagmight-tiles://esri/10/500/250/extra",
    expect("esri", 10, 500, 250)
);
parse_case!(
    test_parse_tile_uri_host_based_trailing_slash,
    "stalagmight-tiles://esri/10/500/250/",
    expect("esri", 10, 500, 250)
);
parse_case!(
    test_parse_tile_uri_host_based_query,
    "stalagmight-tiles://esri/10/500/250?x=1",
    expect("esri", 10, 500, 250)
);
parse_case!(
    test_parse_tile_uri_host_based_zero_coords,
    "stalagmight-tiles://esri/0/0/0",
    expect("esri", 0, 0, 0)
);
parse_case!(
    test_parse_tile_uri_host_based_large_coords,
    "stalagmight-tiles://esri/4294967295/4294967295/4294967295",
    expect("esri", 4_294_967_295, 4_294_967_295, 4_294_967_295)
);
parse_case!(
    test_parse_tile_uri_host_based_missing_y,
    "stalagmight-tiles://esri/10",
    None
);
parse_case!(
    test_parse_tile_uri_host_based_missing_x,
    "stalagmight-tiles://esri/10/500",
    None
);
parse_case!(
    test_parse_tile_uri_host_based_invalid_y,
    "stalagmight-tiles://esri/10/abc/250",
    None
);
parse_case!(
    test_parse_tile_uri_host_based_invalid_x,
    "stalagmight-tiles://esri/10/500/abc",
    None
);
parse_case!(
    test_parse_tile_uri_host_based_negative_x,
    "stalagmight-tiles://esri/10/500/-1",
    None
);
parse_case!(
    test_parse_tile_uri_host_based_negative_y,
    "stalagmight-tiles://esri/10/-1/250",
    None
);
parse_case!(
    test_parse_tile_uri_host_based_negative_z,
    "stalagmight-tiles://esri/-1/500/250",
    None
);
parse_case!(
    test_parse_tile_uri_host_based_z_float,
    "stalagmight-tiles://esri/10.5/500/250",
    None
);
parse_case!(
    test_parse_tile_uri_host_based_numeric_first_segment_non_localhost,
    "stalagmight-tiles://tiles/10/20/30",
    expect("tiles", 10, 20, 30)
);

// === sanitize_content_type tests ===

#[test]
fn test_sanitize_content_type_valid_jpeg() {
    assert_eq!(sanitize_content_type("image/jpeg"), "image/jpeg");
}

#[test]
fn test_sanitize_content_type_valid_png() {
    assert_eq!(sanitize_content_type("image/png"), "image/png");
}

#[test]
fn test_sanitize_content_type_with_charset() {
    assert_eq!(
        sanitize_content_type("image/jpeg; charset=utf-8"),
        "image/jpeg"
    );
}

#[test]
fn test_sanitize_content_type_invalid_returns_default() {
    assert_eq!(sanitize_content_type("text/html"), DEFAULT_CONTENT_TYPE);
}

#[test]
fn test_sanitize_content_type_uppercase_normalized() {
    assert_eq!(sanitize_content_type("IMAGE/JPEG"), "image/jpeg");
}

#[test]
fn test_sanitize_content_type_application_octet_stream() {
    // Should fall back to default for non-image types
    assert_eq!(
        sanitize_content_type("application/octet-stream"),
        DEFAULT_CONTENT_TYPE
    );
}

macro_rules! sanitize_case {
    ($name:ident, $input:expr, $expected:expr) => {
        #[test]
        fn $name() {
            assert_eq!(sanitize_content_type($input), $expected);
        }
    };
}

sanitize_case!(
    test_sanitize_content_type_valid_jpg,
    "image/jpg",
    "image/jpg"
);
sanitize_case!(
    test_sanitize_content_type_uppercase_png,
    "IMAGE/PNG",
    "image/png"
);
sanitize_case!(
    test_sanitize_content_type_mixed_case_jpeg,
    "Image/Jpeg",
    "image/jpeg"
);
sanitize_case!(
    test_sanitize_content_type_trim_spaces,
    " image/jpeg ",
    "image/jpeg"
);
sanitize_case!(
    test_sanitize_content_type_png_with_charset,
    "image/png; charset=utf-8",
    "image/png"
);
sanitize_case!(
    test_sanitize_content_type_jpg_with_charset,
    "image/jpg; charset=utf-8",
    "image/jpg"
);
sanitize_case!(
    test_sanitize_content_type_jpeg_multi_params,
    "image/jpeg;param=1;param2=2",
    "image/jpeg"
);
sanitize_case!(
    test_sanitize_content_type_invalid_text_plain,
    "text/plain",
    DEFAULT_CONTENT_TYPE
);
sanitize_case!(
    test_sanitize_content_type_invalid_gif,
    "image/gif",
    DEFAULT_CONTENT_TYPE
);
sanitize_case!(
    test_sanitize_content_type_empty_string,
    "",
    DEFAULT_CONTENT_TYPE
);
sanitize_case!(
    test_sanitize_content_type_whitespace_only,
    "   ",
    DEFAULT_CONTENT_TYPE
);
sanitize_case!(
    test_sanitize_content_type_uppercase_jpg_with_param,
    "IMAGE/JPG;foo=bar",
    "image/jpg"
);
sanitize_case!(
    test_sanitize_content_type_uppercase_png_with_param,
    "IMAGE/PNG;foo=bar",
    "image/png"
);
sanitize_case!(
    test_sanitize_content_type_no_space_before_charset,
    "image/jpeg;charset=utf-8",
    "image/jpeg"
);
sanitize_case!(
    test_sanitize_content_type_charset_uppercase,
    "image/jpeg; charset=UTF-8",
    "image/jpeg"
);
sanitize_case!(
    test_sanitize_content_type_charset_with_spaces,
    "image/jpeg; charset=utf-8 ; q=0.9",
    "image/jpeg"
);
sanitize_case!(
    test_sanitize_content_type_uppercase_charset,
    "IMAGE/JPEG;CHARSET=UTF-8",
    "image/jpeg"
);
sanitize_case!(
    test_sanitize_content_type_jpg_multi_params,
    "image/jpg;foo=bar;baz=qux",
    "image/jpg"
);
sanitize_case!(
    test_sanitize_content_type_png_trailing_semicolon,
    "image/png;",
    "image/png"
);
sanitize_case!(
    test_sanitize_content_type_leading_semicolon,
    ";charset=utf-8",
    DEFAULT_CONTENT_TYPE
);
sanitize_case!(
    test_sanitize_content_type_invalid_json,
    "application/json",
    DEFAULT_CONTENT_TYPE
);
sanitize_case!(
    test_sanitize_content_type_invalid_avif,
    "image/avif",
    DEFAULT_CONTENT_TYPE
);
sanitize_case!(
    test_sanitize_content_type_invalid_short_jpeg,
    "image/jpe",
    DEFAULT_CONTENT_TYPE
);
sanitize_case!(
    test_sanitize_content_type_jpg_space_before_semicolon,
    "image/jpg ; charset=utf-8",
    "image/jpg"
);
sanitize_case!(
    test_sanitize_content_type_jpeg_space_before_semicolon,
    "image/jpeg ; charset=utf-8",
    "image/jpeg"
);

// === Provider validation tests ===

#[test]
fn test_registry_contains_esri() {
    assert!(tile_providers::get_provider("esri").is_some());
}

#[test]
fn test_registry_unknown_provider_returns_none() {
    assert!(tile_providers::get_provider("unknown").is_none());
}

#[test]
fn test_allowed_content_types() {
    assert!(ALLOWED_CONTENT_TYPES.contains(&"image/jpeg"));
    assert!(ALLOWED_CONTENT_TYPES.contains(&"image/png"));
    assert!(!ALLOWED_CONTENT_TYPES.contains(&"text/html"));
}

// === handle_tile_request tests (with real cache, no network) ===

fn make_test_cache() -> Arc<RecoveringMutex<Option<TileCache>>> {
    let cache = TileCache::in_memory(10, 30).unwrap();
    Arc::new(RecoveringMutex::new("tile_cache", Some(cache)))
}

fn make_test_tilemap_cache() -> Arc<RecoveringMutex<TilemapCache>> {
    Arc::new(RecoveringMutex::new("tilemap_cache", TilemapCache::new()))
}

/// Cache config with all providers enabled (no per-provider overrides)
fn make_test_cache_config() -> config::CacheConfig {
    config::CacheConfig {
        tile_cache_enabled: true,
        providers: Default::default(),
        ..Default::default()
    }
}

fn make_test_stats() -> Arc<TileFetchStats> {
    Arc::new(TileFetchStats::new())
}

fn assert_cors_header(response: &http::Response<Vec<u8>>) {
    assert_eq!(
        response
            .headers()
            .get(http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .unwrap(),
        "*"
    );
}

#[test]
fn test_handle_tile_request_unknown_provider_returns_400() {
    let cache = make_test_cache();
    let tilemap_cache = make_test_tilemap_cache();
    let stats = make_test_stats();
    let (response, _) = handle_tile_request(
        "unknown_provider",
        10,
        500,
        250,
        &cache,
        &tilemap_cache,
        &stats,
        &make_test_cache_config(),
    );
    assert_eq!(response.status(), http::StatusCode::BAD_REQUEST);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/plain"
    );
    assert!(std::str::from_utf8(response.body())
        .unwrap()
        .contains("Unknown provider"));
    assert_cors_header(&response);
}

#[test]
fn a_revalidation_is_not_counted_as_imagery_pulled_from_the_provider() {
    // #31 measures how much imagery a session consumes from the provider. A
    // 304 transfers none: the pixels came off our own disk. Past the first
    // day nearly every request revalidates, so folding those into
    // `tile.fetch` would report near-total consumption for a session that
    // downloaded nothing at all.
    let fetched = Ok(TileFetchResponse::Fetched(FetchedTile {
        data: b"bytes".to_vec(),
        content_type: "image/jpeg".to_string(),
        policy: CachePolicy::default(),
        etag: None,
    }));
    assert_eq!(
        TileServeSource::from_fetch(&fetched),
        Some(TileServeSource::Network)
    );
    assert_eq!(TileServeSource::Network.telemetry_kind(), "fetch");

    let not_modified = Ok(TileFetchResponse::NotModified {
        policy: CachePolicy::default(),
    });
    assert_eq!(
        TileServeSource::from_fetch(&not_modified),
        Some(TileServeSource::Revalidated)
    );
    assert_eq!(TileServeSource::Revalidated.telemetry_kind(), "revalidate");

    // A failed call says nothing about provenance: whatever the cache path
    // then serves (a stale copy, or nothing) decides it.
    let failed: Result<TileFetchResponse, TileFetchError> =
        Err(TileFetchError::transport("down".to_string()));
    assert_eq!(TileServeSource::from_fetch(&failed), None);
    assert_eq!(TileServeSource::Cache.telemetry_kind(), "cache");
}

#[test]
fn the_serve_source_survives_the_round_trip_through_the_atomic() {
    // It crosses a thread boundary as a u8; a bad mapping would silently
    // retag every tile.
    for source in [
        TileServeSource::Cache,
        TileServeSource::Revalidated,
        TileServeSource::Network,
    ] {
        assert_eq!(TileServeSource::from_u8(source.as_u8()), source);
    }
}

#[test]
fn test_handle_tile_request_cache_hit_returns_200() {
    let cache = make_test_cache();
    let tilemap_cache = make_test_tilemap_cache();
    let stats = make_test_stats();

    // Pre-populate cache
    {
        let mut guard = cache.lock();
        let c = guard.as_mut().unwrap();
        c.put_granted("esri", 10, 500, 250, b"fake_tile_data", "image/jpeg")
            .unwrap();
    }

    let (response, _) = handle_tile_request(
        "esri",
        10,
        500,
        250,
        &cache,
        &tilemap_cache,
        &stats,
        &make_test_cache_config(),
    );
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(response.body(), b"fake_tile_data");
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "image/jpeg"
    );
    assert_cors_header(&response);
}

#[test]
fn test_handle_tile_request_cache_hit_content_type() {
    let cache = make_test_cache();
    let tilemap_cache = make_test_tilemap_cache();
    let stats = make_test_stats();

    // Pre-populate cache with PNG
    {
        let mut guard = cache.lock();
        let c = guard.as_mut().unwrap();
        c.put_granted("esri", 10, 500, 250, b"png_data", "image/png")
            .unwrap();
    }

    let (response, source) = handle_tile_request(
        "esri",
        10,
        500,
        250,
        &cache,
        &tilemap_cache,
        &stats,
        &make_test_cache_config(),
    );
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(response.headers().get("content-type").unwrap(), "image/png");
    // No network call was made at all, so this is provider consumption of
    // exactly nothing.
    assert_eq!(source, TileServeSource::Cache);
    assert_cors_header(&response);
}

#[test]
fn test_handle_tile_request_cache_miss_network_fails_returns_404() {
    // With no cached tile and network failing, should return 404
    let cache = make_test_cache();
    let tilemap_cache = make_test_tilemap_cache();

    // Mock fetch that always fails
    let mock_fetch = |_: &str,
                      _: u32,
                      _: u32,
                      _: u32,
                      _: Option<&str>|
     -> Result<TileFetchResponse, TileFetchError> {
        Err(TileFetchError::transport("Network unavailable".to_string()))
    };

    let response = handle_tile_request_with_fetcher(
        "esri",
        10,
        500,
        250,
        &cache,
        &tilemap_cache,
        mock_fetch,
        true,
    );
    assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/plain"
    );
    assert!(std::str::from_utf8(response.body())
        .unwrap()
        .contains("Tile not available"));
    assert_cors_header(&response);
}

#[test]
fn test_handle_tile_request_cache_miss_network_succeeds_returns_200() {
    // Cache miss but network fetch succeeds
    let cache = make_test_cache();
    let tilemap_cache = make_test_tilemap_cache();

    // Mock fetch that returns tile data
    let mock_fetch = |_: &str,
                      _: u32,
                      _: u32,
                      _: u32,
                      _: Option<&str>|
     -> Result<TileFetchResponse, TileFetchError> {
        Ok(TileFetchResponse::Fetched(FetchedTile {
            data: b"fetched_tile_data".to_vec(),
            content_type: "image/jpeg".to_string(),
            policy: CachePolicy::default(),
            etag: None,
        }))
    };

    let response = handle_tile_request_with_fetcher(
        "esri",
        10,
        500,
        250,
        &cache,
        &tilemap_cache,
        mock_fetch,
        true,
    );
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(response.body(), b"fetched_tile_data");
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "image/jpeg"
    );
    assert_cors_header(&response);
}

#[test]
fn test_handle_tile_request_fetched_tile_is_cached() {
    // Verify that a successfully fetched tile is stored in cache. The mock
    // grants storage explicitly, because the cache stores nothing without a
    // `Cache-Control` that says it may - see the no-grant test below.
    let cache = make_test_cache();
    let tilemap_cache = make_test_tilemap_cache();

    // Pre-populate tilemap cache so we don't make network calls
    // Mark all tiles in the region as available
    // Tile (15, 100, 200) belongs to region (0, 128) since 100/128=0, 200/128=1
    {
        let mut tc = tilemap_cache.lock();
        let data = vec![true; 16_384]; // All tiles available
        tc.insert("esri", 15, 0, 128, data);
    }

    let mock_fetch = |_: &str,
                      _: u32,
                      _: u32,
                      _: u32,
                      _: Option<&str>|
     -> Result<TileFetchResponse, TileFetchError> {
        Ok(TileFetchResponse::Fetched(FetchedTile {
            data: b"fetched_tile".to_vec(),
            content_type: "image/jpeg".to_string(),
            policy: CachePolicy {
                max_age_seconds: Some(86_400),
                no_store: false,
                age_seconds: None,
                no_cache: false,
                malformed: false,
            },
            etag: None,
        }))
    };

    let response = handle_tile_request_with_fetcher(
        "esri",
        15,
        100,
        200,
        &cache,
        &tilemap_cache,
        mock_fetch,
        true,
    );
    assert_eq!(
        response.status(),
        http::StatusCode::OK,
        "Response should be OK"
    );

    // Check that tile is now in cache
    let cached = {
        let mut guard = cache.lock();
        guard.as_mut().unwrap().get("esri", 15, 100, 200)
    };
    assert!(
        cached.is_some(),
        "Tile should be cached after successful fetch"
    );
    assert_eq!(&cached.unwrap().data, b"fetched_tile");
}

#[test]
fn test_handle_tile_request_with_no_cache_network_succeeds() {
    // Cache is None (disabled) but network works
    let cache = Arc::new(RecoveringMutex::new("tile_cache", None));
    let tilemap_cache = make_test_tilemap_cache();

    let mock_fetch = |_: &str,
                      _: u32,
                      _: u32,
                      _: u32,
                      _: Option<&str>|
     -> Result<TileFetchResponse, TileFetchError> {
        Ok(TileFetchResponse::Fetched(FetchedTile {
            data: b"tile_data".to_vec(),
            content_type: "image/jpeg".to_string(),
            policy: CachePolicy::default(),
            etag: None,
        }))
    };

    let response = handle_tile_request_with_fetcher(
        "esri",
        10,
        500,
        250,
        &cache,
        &tilemap_cache,
        mock_fetch,
        true,
    );
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(response.body(), b"tile_data");
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "image/jpeg"
    );
    assert_cors_header(&response);
}

#[test]
fn test_handle_tile_request_with_no_cache_network_fails() {
    // Cache is None (disabled) and network fails -> 404
    let cache = Arc::new(RecoveringMutex::new("tile_cache", None));
    let tilemap_cache = make_test_tilemap_cache();

    let mock_fetch = |_: &str,
                      _: u32,
                      _: u32,
                      _: u32,
                      _: Option<&str>|
     -> Result<TileFetchResponse, TileFetchError> {
        Err(TileFetchError::transport("Connection refused".to_string()))
    };

    let response = handle_tile_request_with_fetcher(
        "esri",
        10,
        500,
        250,
        &cache,
        &tilemap_cache,
        mock_fetch,
        true,
    );
    assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/plain"
    );
    assert!(std::str::from_utf8(response.body())
        .unwrap()
        .contains("Tile not available"));
    assert_cors_header(&response);
}

#[test]
fn test_handle_tile_request_cache_miss_network_fails_uses_stale_tile() {
    // A real retention TTL, with the tile pushed past its max-age. Zeroing
    // ttl_days used to produce the same "not fresh" state, but retention 0
    // now means nothing is retained at all - which is a different case, and
    // one where no copy should be served. See
    // `a_tile_past_retention_is_not_served_even_as_a_last_resort`.
    let cache = Arc::new(RecoveringMutex::new(
        "tile_cache",
        Some(TileCache::in_memory(1, 30).unwrap()),
    ));
    let tilemap_cache = make_test_tilemap_cache();

    {
        let mut guard = cache.lock();
        let c = guard.as_mut().unwrap();
        c.put_granted("esri", 1, 2, 3, b"stale_tile", "image/png")
            .unwrap();
        c.expire_for_test("esri", 1, 2, 3).unwrap();
    }

    let mock_fetch = |_: &str,
                      _: u32,
                      _: u32,
                      _: u32,
                      _: Option<&str>|
     -> Result<TileFetchResponse, TileFetchError> {
        Err(TileFetchError::transport("Network down".to_string()))
    };

    let response =
        handle_tile_request_with_fetcher("esri", 1, 2, 3, &cache, &tilemap_cache, mock_fetch, true);
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(response.body(), b"stale_tile");
    assert_eq!(response.headers().get("content-type").unwrap(), "image/png");
    assert_cors_header(&response);
}

// === tile_fetch_impl tests ===

#[test]
fn test_tile_fetch_impl_unknown_provider_errors() {
    let cache = make_test_cache();
    let tilemap_cache = make_test_tilemap_cache();
    let stats = make_test_stats();
    let err = tile_fetch_impl(
        "unknown",
        1,
        2,
        3,
        &cache,
        &tilemap_cache,
        &stats,
        &make_test_cache_config(),
    )
    .unwrap_err();
    assert!(err.contains("Unknown provider"));
}

#[test]
fn test_tile_fetch_impl_cache_hit_returns_bytes() {
    let cache = make_test_cache();
    let tilemap_cache = make_test_tilemap_cache();
    let stats = make_test_stats();
    {
        let mut guard = cache.lock();
        let c = guard.as_mut().unwrap();
        c.put_granted("esri", 1, 2, 3, b"tile_bytes", "image/png")
            .unwrap();
    }
    let data = tile_fetch_impl(
        "esri",
        1,
        2,
        3,
        &cache,
        &tilemap_cache,
        &stats,
        &make_test_cache_config(),
    )
    .unwrap();
    assert_eq!(data.0, b"tile_bytes");
    // Served entirely from cache, so nothing was consumed from the provider.
    assert_eq!(data.1, TileServeSource::Cache);
}

// === Per-provider cache toggle tests ===

#[test]
fn test_cache_disabled_skips_cache_read_and_write() {
    let cache = make_test_cache();
    let tilemap_cache = make_test_tilemap_cache();

    // One switch decides whether the cache is touched at all.
    let cache_config = config::CacheConfig {
        tile_cache_enabled: false,
        ..Default::default()
    };
    let cache_enabled = cache_config.tile_cache_enabled;
    assert!(!cache_enabled);

    // Mock fetch that succeeds
    let mock_fetch = |_: &str,
                      _: u32,
                      _: u32,
                      _: u32,
                      _: Option<&str>|
     -> Result<TileFetchResponse, TileFetchError> {
        Ok(TileFetchResponse::Fetched(FetchedTile {
            data: b"network_tile".to_vec(),
            content_type: "image/jpeg".to_string(),
            policy: CachePolicy::default(),
            etag: None,
        }))
    };

    // Fetch should succeed via network
    let response = handle_tile_request_with_fetcher(
        "esri",
        10,
        500,
        250,
        &cache,
        &tilemap_cache,
        mock_fetch,
        cache_enabled,
    );
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(response.body(), b"network_tile");

    // Verify tile was NOT written to cache
    let cached = {
        let mut guard = cache.lock();
        guard.as_mut().unwrap().get("esri", 10, 500, 250)
    };
    assert!(
        cached.is_none(),
        "Tile should not be cached when provider caching is disabled"
    );
}

#[test]
fn every_cache_control_field_line_is_read_not_just_the_first() {
    // RFC 9110 5.3: repeating a field is identical to comma-joining it. An
    // origin and an intermediary each adding a directive produces exactly
    // this, and `HeaderMap::get` returns only the first - so a `no-store`
    // sent on its own line was invisible while an earlier `max-age` still
    // let the tile be stored. The one failure this cache must not have.
    let mut headers = http::HeaderMap::new();
    headers.append("cache-control", "max-age=86400".parse().unwrap());
    headers.append("cache-control", "no-store".parse().unwrap());

    let policy = read_cache_policy(&headers);
    assert!(policy.no_store, "the second field line must be seen");
    assert_eq!(
        policy.granted_seconds(),
        None,
        "and no-store must beat the max-age that arrived first"
    );

    // The joined form is unchanged, and quote-aware splitting still holds
    // across the join.
    let mut joined = http::HeaderMap::new();
    joined.append(
        "cache-control",
        "private=\"x,max-age=999\"".parse().unwrap(),
    );
    joined.append("cache-control", "max-age=600".parse().unwrap());
    assert_eq!(read_cache_policy(&joined).granted_seconds(), Some(600));

    // No header at all is still "no grant", not a default one.
    assert_eq!(
        read_cache_policy(&http::HeaderMap::new()).granted_seconds(),
        None
    );
}

#[test]
fn age_is_read_alongside_the_joined_directives() {
    let mut headers = http::HeaderMap::new();
    headers.append("cache-control", "max-age=86400".parse().unwrap());
    headers.append("age", "80000".parse().unwrap());
    assert_eq!(read_cache_policy(&headers).granted_seconds(), Some(6_400));
}

/// A real, decodable 256x256 JPEG. The overzoom path decodes the parent and
/// crops it, so a parent of arbitrary bytes fails for the wrong reason.
fn make_jpeg_bytes() -> Vec<u8> {
    let img = DynamicImage::new_rgb8(256, 256);
    let mut buf = Cursor::new(Vec::new());
    // `unwrap`, not `expect`: this module allows the former (see the
    // `#[expect(clippy::unwrap_used)]` on it) and denies the latter.
    img.write_to(&mut buf, ImageFormat::Jpeg).unwrap();
    buf.into_inner()
}

#[test]
fn a_no_cache_tile_still_revalidates_conditionally() {
    // The point of storing a `no-cache` tile at all: its ETag makes the next
    // request a cheap 304 instead of a full download. Hiding those rows from
    // the retained lookup removed the validator along with them, so every
    // request re-downloaded the whole body - the opposite of the intent.
    let cache = Arc::new(RecoveringMutex::new(
        "tile_cache",
        Some(TileCache::in_memory(10, 30).unwrap()),
    ));
    let tilemap_cache = make_test_tilemap_cache();
    {
        let mut guard = cache.lock();
        guard
            .as_mut()
            .unwrap()
            .put(
                "esri",
                1,
                2,
                3,
                b"held",
                "image/png",
                cache::tiles::parse_cache_control("no-cache"),
                Some("\"v1\""),
            )
            .unwrap();
    }

    let seen = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
    let seen_ref = Arc::clone(&seen);
    let mock_fetch = move |_: &str,
                           _: u32,
                           _: u32,
                           _: u32,
                           validator: Option<&str>|
          -> Result<TileFetchResponse, TileFetchError> {
        seen_ref.lock().unwrap().push(validator.map(str::to_string));
        Ok(TileFetchResponse::NotModified {
            policy: cache::tiles::parse_cache_control("no-cache"),
        })
    };

    let response =
        handle_tile_request_with_fetcher("esri", 1, 2, 3, &cache, &tilemap_cache, mock_fetch, true);

    assert_eq!(
        seen.lock().unwrap().as_slice(),
        &[Some("\"v1\"".to_string())],
        "the stored validator must reach the request"
    );
    // Validation SUCCEEDED, so serving it is exactly what RFC 9111 permits.
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(response.body(), b"held");
}

#[test]
fn a_no_cache_tile_is_not_served_when_revalidation_fails() {
    // The other half. For every other retained row a failed fetch is the
    // reason to serve the stored copy; for this one it is the reason not to,
    // because RFC 9111 5.2.2.4 requires a SUCCESSFUL validation first.
    let cache = Arc::new(RecoveringMutex::new(
        "tile_cache",
        Some(TileCache::in_memory(10, 30).unwrap()),
    ));
    let tilemap_cache = make_test_tilemap_cache();
    {
        let mut guard = cache.lock();
        guard
            .as_mut()
            .unwrap()
            .put(
                "esri",
                1,
                2,
                3,
                b"held",
                "image/png",
                cache::tiles::parse_cache_control("no-cache"),
                Some("\"v1\""),
            )
            .unwrap();
    }

    let offline = move |_: &str,
                        _: u32,
                        _: u32,
                        _: u32,
                        _: Option<&str>|
          -> Result<TileFetchResponse, TileFetchError> {
        Err(TileFetchError::transport("Connection reset".to_string()))
    };

    let response =
        handle_tile_request_with_fetcher("esri", 1, 2, 3, &cache, &tilemap_cache, offline, true);
    assert_eq!(
        response.status(),
        http::StatusCode::NOT_FOUND,
        "a transport failure is exactly when a no-cache copy may NOT be used"
    );
}

#[test]
fn a_gone_tile_is_not_served_as_an_overzoomed_parent() {
    // `410 Gone` gets no 404 carve-out: nobody uses it to mean "not at this
    // zoom", it says the imagery was deliberately removed. Serving a parent
    // crop would hand back the withdrawn area at a coarser resolution.
    let cache = Arc::new(RecoveringMutex::new(
        "tile_cache",
        Some(TileCache::in_memory(10, 30).unwrap()),
    ));
    let tilemap_cache = make_test_tilemap_cache();
    {
        let mut guard = cache.lock();
        guard
            .as_mut()
            .unwrap()
            .put_granted("esri", 9, 250, 125, &make_jpeg_bytes(), "image/jpeg")
            .unwrap();
    }

    let gone = move |_: &str,
                     _: u32,
                     _: u32,
                     _: u32,
                     _: Option<&str>|
          -> Result<TileFetchResponse, TileFetchError> {
        Err(TileFetchError::status(410, "HTTP 410".to_string()))
    };

    let response =
        handle_tile_request_with_fetcher("esri", 10, 500, 250, &cache, &tilemap_cache, gone, true);
    assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
}

#[test]
fn a_denied_tile_is_not_served_as_an_overzoomed_parent() {
    // Refusing to answer a 403 off the stale cache is only half a guard: the
    // overzoom loop treated "missing" as permission to climb, so a cached
    // parent still produced an upscaled crop of the very area the provider
    // had just denied.
    let cache = Arc::new(RecoveringMutex::new(
        "tile_cache",
        Some(TileCache::in_memory(10, 30).unwrap()),
    ));
    let tilemap_cache = make_test_tilemap_cache();
    {
        // A parent one zoom up, fresh, ready to be cropped.
        let mut guard = cache.lock();
        guard
            .as_mut()
            .unwrap()
            .put_granted("esri", 9, 250, 125, &make_jpeg_bytes(), "image/jpeg")
            .unwrap();
    }

    let denied = move |_: &str,
                       _: u32,
                       _: u32,
                       _: u32,
                       _: Option<&str>|
          -> Result<TileFetchResponse, TileFetchError> {
        Err(TileFetchError::status(403, "HTTP 403".to_string()))
    };

    let response = handle_tile_request_with_fetcher(
        "esri",
        10,
        500,
        250,
        &cache,
        &tilemap_cache,
        denied,
        true,
    );
    assert_eq!(
        response.status(),
        http::StatusCode::NOT_FOUND,
        "a denial must not be answered at a coarser resolution either"
    );
}

#[test]
fn a_missing_tile_still_falls_back_to_an_overzoomed_parent() {
    // The other side, and the reason "any non-5xx is terminal" would be
    // wrong: for most tile services a 404 means "no imagery at this zoom
    // here", which is the entire purpose of overzoom. Making that terminal
    // would disable the feature for any provider that uses it.
    let cache = Arc::new(RecoveringMutex::new(
        "tile_cache",
        Some(TileCache::in_memory(10, 30).unwrap()),
    ));
    let tilemap_cache = make_test_tilemap_cache();
    {
        let mut guard = cache.lock();
        guard
            .as_mut()
            .unwrap()
            .put_granted("esri", 9, 250, 125, &make_jpeg_bytes(), "image/jpeg")
            .unwrap();
    }

    let absent = move |_: &str,
                       _: u32,
                       _: u32,
                       _: u32,
                       _: Option<&str>|
          -> Result<TileFetchResponse, TileFetchError> {
        Err(TileFetchError::status(404, "HTTP 404".to_string()))
    };

    let response = handle_tile_request_with_fetcher(
        "esri",
        10,
        500,
        250,
        &cache,
        &tilemap_cache,
        absent,
        true,
    );
    assert_eq!(
        response.status(),
        http::StatusCode::OK,
        "a 404 is an absence, not a refusal - the parent may still serve"
    );
}

/// Mark a tilemap region as "imagery everywhere", so the overzoom tests
/// below never send a real availability probe to the provider.
fn warm_tilemap(tilemap_cache: &Arc<RecoveringMutex<TilemapCache>>, z: u32, y: u32, x: u32) {
    let (base_y, base_x) = TilemapCache::get_region_origin(y, x);
    let cells = (tilemap_cache::TILEMAP_GRID_SIZE * tilemap_cache::TILEMAP_GRID_SIZE) as usize;
    tilemap_cache
        .lock()
        .insert("esri", z, base_y, base_x, vec![true; cells]);
}

/// A tile cache holding one fresh parent at z9 (250, 125), and a tilemap
/// that says imagery exists at z9..=z11 there.
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
        .put_granted("esri", 9, 250, 125, &make_jpeg_bytes(), "image/jpeg")
        .unwrap();
    let tilemap_cache = make_test_tilemap_cache();
    warm_tilemap(&tilemap_cache, 9, 250, 125);
    warm_tilemap(&tilemap_cache, 10, 500, 250);
    warm_tilemap(&tilemap_cache, 11, 1000, 500);
    (cache, tilemap_cache)
}

#[test]
fn an_overzoomed_crop_after_a_transient_failure_is_served_but_not_stored() {
    // [#576 C16] Offline for a moment while zoomed past the imagery held:
    // the z10 fetch fails (transport, or a single 5xx), so the loop climbs
    // to the cached z9 parent and serves an upscaled crop - which is right.
    // Storing that crop under the z10 key as a FRESH tile is not: once the
    // network is back, the blurry crop kept answering as a cache hit, with
    // no request made, until the parent's grant ran out (a day for Esri),
    // across restarts.
    for outage in [
        TileFetchError::transport("Connection reset".to_string()),
        TileFetchError::status(503, "HTTP 503".to_string()),
    ] {
        let (cache, tilemap_cache) = cache_with_a_z9_parent();
        let label = outage.to_string();
        let offline =
            move |_: &str,
                  _: u32,
                  _: u32,
                  _: u32,
                  _: Option<&str>|
                  -> Result<TileFetchResponse, TileFetchError> { Err(outage.clone()) };
        let response = handle_tile_request_with_fetcher(
            "esri",
            10,
            500,
            250,
            &cache,
            &tilemap_cache,
            offline,
            true,
        );
        assert_eq!(
            response.status(),
            http::StatusCode::OK,
            "{label}: the parent crop still serves"
        );
        let crop_len = response.body().len();

        // The network is back and the provider has the real z10 tile.
        let z10_requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&z10_requests);
        let online = move |_: &str,
                           z: u32,
                           _: u32,
                           _: u32,
                           _: Option<&str>|
              -> Result<TileFetchResponse, TileFetchError> {
            if z == 10 {
                counter.fetch_add(1, Ordering::Relaxed);
            }
            Ok(TileFetchResponse::Fetched(FetchedTile {
                data: b"sharp z10 imagery".to_vec(),
                content_type: "image/jpeg".to_string(),
                policy: cache::tiles::parse_cache_control("max-age=86400"),
                etag: None,
            }))
        };
        let response = handle_tile_request_with_fetcher(
            "esri",
            10,
            500,
            250,
            &cache,
            &tilemap_cache,
            online,
            true,
        );
        let requests = z10_requests.load(Ordering::Relaxed);
        let fetched = response.body().as_slice() == b"sharp z10 imagery";
        assert!(
            fetched && requests == 1,
            "{label}: after reconnecting, z10 was answered by {} with {requests} request(s) \
             to the provider",
            if fetched {
                "the provider's own z10 tile".to_string()
            } else {
                format!(
                    "a stored {}-byte tile (the {crop_len}-byte crop)",
                    response.body().len()
                )
            }
        );
    }
}

#[test]
fn an_overzoomed_crop_is_not_stored_when_any_level_on_the_way_up_was_unreachable() {
    // z11 is a 404 (absent - that alone would allow storing a crop), but
    // z10 was unreachable, so the z11 crop came from z9 only for want of
    // z10. Stored, it would outlive the outage at the coarser resolution.
    let (cache, tilemap_cache) = cache_with_a_z9_parent();
    let fetch = move |_: &str,
                      z: u32,
                      _: u32,
                      _: u32,
                      _: Option<&str>|
          -> Result<TileFetchResponse, TileFetchError> {
        match z {
            11 => Err(TileFetchError::status(404, "HTTP 404".to_string())),
            _ => Err(TileFetchError::transport("Connection reset".to_string())),
        }
    };
    let response = handle_tile_request_with_fetcher(
        "esri",
        11,
        1000,
        500,
        &cache,
        &tilemap_cache,
        fetch,
        true,
    );
    assert_eq!(response.status(), http::StatusCode::OK);
    let guard = cache.lock();
    let held = guard.as_ref().unwrap();
    assert!(
        !held.has_fresh_tile("esri", 11, 1000, 500) && !held.has_stale_tile("esri", 11, 1000, 500),
        "the z11 crop was stored although z10 was only unreachable"
    );
}

#[test]
fn an_overzoomed_crop_for_a_tile_the_provider_does_not_have_is_still_stored() {
    // The case caching a crop exists for: a 404 says there is no imagery
    // at this zoom here, so the crop IS the best this tile will be until
    // the parent's grant runs out, and re-deriving it per request buys
    // nothing.
    let (cache, tilemap_cache) = cache_with_a_z9_parent();
    let absent = move |_: &str,
                       _: u32,
                       _: u32,
                       _: u32,
                       _: Option<&str>|
          -> Result<TileFetchResponse, TileFetchError> {
        Err(TileFetchError::status(404, "HTTP 404".to_string()))
    };
    let response = handle_tile_request_with_fetcher(
        "esri",
        10,
        500,
        250,
        &cache,
        &tilemap_cache,
        absent,
        true,
    );
    assert_eq!(response.status(), http::StatusCode::OK);
    let guard = cache.lock();
    assert!(
        guard.as_ref().unwrap().has_fresh_tile("esri", 10, 500, 250),
        "a crop for a tile the provider does not have is stored, under the parent's grant"
    );
}

#[test]
fn a_no_store_revalidation_does_not_serve_the_copy_it_just_deleted() {
    // A 304 carrying `no-store` revokes the grant the copy was held under.
    // `refresh_validated` deletes the row; serving those bytes anyway would
    // hand out a copy in the same breath as dropping it. We ask again
    // without a validator and serve what comes back - which the gate also
    // refuses to store, so nothing is retained either way.
    let cache = Arc::new(RecoveringMutex::new(
        "tile_cache",
        Some(TileCache::in_memory(10, 30).unwrap()),
    ));
    let tilemap_cache = make_test_tilemap_cache();
    {
        let mut guard = cache.lock();
        let c = guard.as_mut().unwrap();
        // An ETag is what makes the next request conditional, which is the
        // only way to reach a 304 at all.
        c.put(
            "esri",
            1,
            2,
            3,
            b"revoked",
            "image/png",
            cache::tiles::parse_cache_control("max-age=86400"),
            Some("\"v1\""),
        )
        .unwrap();
        c.expire_for_test("esri", 1, 2, 3).unwrap();
    }

    let calls = Arc::new(AtomicU64::new(0));
    let calls_seen = Arc::clone(&calls);
    let mock_fetch = move |_: &str,
                           _: u32,
                           _: u32,
                           _: u32,
                           validator: Option<&str>|
          -> Result<TileFetchResponse, TileFetchError> {
        calls_seen.fetch_add(1, Ordering::Relaxed);
        if validator.is_some() {
            // The conditional request: still current, but stop storing it.
            Ok(TileFetchResponse::NotModified {
                policy: cache::tiles::parse_cache_control("no-store"),
            })
        } else {
            // The unconditional re-ask.
            Ok(TileFetchResponse::Fetched(FetchedTile {
                data: b"refetched".to_vec(),
                content_type: "image/png".to_string(),
                policy: cache::tiles::parse_cache_control("no-store"),
                etag: None,
            }))
        }
    };

    let response =
        handle_tile_request_with_fetcher("esri", 1, 2, 3, &cache, &tilemap_cache, mock_fetch, true);
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(
        response.body(),
        b"refetched",
        "must not be the bytes the 304 just revoked"
    );
    assert_eq!(
        calls.load(Ordering::Relaxed),
        2,
        "conditional, then a re-ask"
    );

    // And nothing was kept: `no-store` is refused by the storage gate.
    let guard = cache.lock();
    let c = guard.as_ref().unwrap();
    assert!(c.get_stale("esri", 1, 2, 3).is_none());
}

#[test]
fn a_fresh_cache_hit_does_not_probe_the_tilemap_endpoint() {
    // The probe is a NETWORK call when the tilemap cache is cold, and it ran
    // before anything looked at the tile cache - so a request answered
    // entirely from disk could still reach the provider, while being
    // recorded as `tile.cache.<provider>` ("no network call at all").
    //
    // `make_test_tilemap_cache` is empty, and esri HAS a tilemap template,
    // so reaching `check_tile_availability` here means a real fetch attempt.
    // The assertion that matters is the source: a probe would be network
    // traffic on a request that must report as pure cache.
    let cache = Arc::new(RecoveringMutex::new(
        "tile_cache",
        Some(TileCache::in_memory(10, 30).unwrap()),
    ));
    let tilemap_cache = make_test_tilemap_cache();
    {
        let mut guard = cache.lock();
        guard
            .as_mut()
            .unwrap()
            .put_granted("esri", 10, 500, 250, b"cached", "image/png")
            .unwrap();
    }
    assert!(
        tilemap_cache
            .lock()
            .is_tile_available("esri", 10, 500, 250)
            .is_none(),
        "precondition: the tilemap cache is cold, so a probe would hit the network"
    );

    let stats = Arc::new(TileFetchStats::default());
    let before = stats.snapshot();
    let (response, source) = handle_tile_request(
        "esri",
        10,
        500,
        250,
        &cache,
        &tilemap_cache,
        &stats,
        &make_test_cache_config(),
    );

    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(response.body(), b"cached");
    assert_eq!(source, TileServeSource::Cache);
    // The tilemap cache is still cold: nothing went looking for it.
    assert!(tilemap_cache
        .lock()
        .is_tile_available("esri", 10, 500, 250)
        .is_none());
    let after = stats.snapshot();
    assert_eq!(
        (after.total_fetched, after.total_errors),
        (before.total_fetched, before.total_errors),
        "a pure cache hit must not move the network counters"
    );
}

#[test]
fn a_forbidden_or_missing_tile_does_not_fall_back_to_the_stored_copy() {
    // stale-if-error (RFC 5861) is for a provider we cannot REACH. A 403 or
    // 404 is the provider answering, and answering that this tile is not to
    // be served - so serving it anyway would keep withdrawn imagery on
    // screen until retention ran out.
    for status in [403u16, 404, 410] {
        let cache = Arc::new(RecoveringMutex::new(
            "tile_cache",
            Some(TileCache::in_memory(10, 30).unwrap()),
        ));
        let tilemap_cache = make_test_tilemap_cache();
        {
            let mut guard = cache.lock();
            guard
                .as_mut()
                .unwrap()
                .put_granted("esri", 1, 2, 3, b"withdrawn", "image/png")
                .unwrap();
            // Past max-age, so the request reaches the network at all.
            guard
                .as_mut()
                .unwrap()
                .expire_for_test("esri", 1, 2, 3)
                .unwrap();
        }

        let mock_fetch = move |_: &str,
                               _: u32,
                               _: u32,
                               _: u32,
                               _: Option<&str>|
              -> Result<TileFetchResponse, TileFetchError> {
            Err(TileFetchError::status(status, format!("HTTP {status}")))
        };

        let response = handle_tile_request_with_fetcher(
            "esri",
            1,
            2,
            3,
            &cache,
            &tilemap_cache,
            mock_fetch,
            true,
        );
        assert_eq!(
            response.status(),
            http::StatusCode::NOT_FOUND,
            "a {status} must not be answered off the cache"
        );
    }
}

#[test]
fn a_provider_outage_still_falls_back_to_the_stored_copy() {
    // The other side of the same gate: 5xx and transport failures are
    // exactly what stale-if-error exists for, so a maintenance window must
    // not blank the map.
    for outage in [
        TileFetchError::status(503, "HTTP 503".to_string()),
        TileFetchError::status(500, "HTTP 500".to_string()),
        TileFetchError::transport("Connection reset".to_string()),
    ] {
        let cache = Arc::new(RecoveringMutex::new(
            "tile_cache",
            Some(TileCache::in_memory(10, 30).unwrap()),
        ));
        let tilemap_cache = make_test_tilemap_cache();
        {
            let mut guard = cache.lock();
            guard
                .as_mut()
                .unwrap()
                .put_granted("esri", 1, 2, 3, b"stored", "image/png")
                .unwrap();
            guard
                .as_mut()
                .unwrap()
                .expire_for_test("esri", 1, 2, 3)
                .unwrap();
        }

        let mock_fetch =
            move |_: &str,
                  _: u32,
                  _: u32,
                  _: u32,
                  _: Option<&str>|
                  -> Result<TileFetchResponse, TileFetchError> { Err(outage.clone()) };

        let response = handle_tile_request_with_fetcher(
            "esri",
            1,
            2,
            3,
            &cache,
            &tilemap_cache,
            mock_fetch,
            true,
        );
        assert_eq!(response.status(), http::StatusCode::OK);
        assert_eq!(response.body(), b"stored");
    }
}

#[test]
fn test_cache_disabled_no_stale_fallback() {
    // Use a cache with 0 TTL so tiles are immediately stale
    let cache = Arc::new(RecoveringMutex::new(
        "tile_cache",
        Some(TileCache::in_memory(10, 0).unwrap()),
    ));
    let tilemap_cache = make_test_tilemap_cache();

    // Pre-populate cache with a tile
    {
        let mut guard = cache.lock();
        guard
            .as_mut()
            .unwrap()
            .put_granted("esri", 1, 2, 3, b"stale_tile", "image/png")
            .unwrap();
    }

    // Caching off: even a tile already on disk must not be reached for.
    let cache_config = config::CacheConfig {
        tile_cache_enabled: false,
        ..Default::default()
    };
    let cache_enabled = cache_config.tile_cache_enabled;

    // Network fails
    let mock_fetch = |_: &str,
                      _: u32,
                      _: u32,
                      _: u32,
                      _: Option<&str>|
     -> Result<TileFetchResponse, TileFetchError> {
        Err(TileFetchError::transport("Network down".to_string()))
    };

    // Should NOT fall back to stale cache
    let response = handle_tile_request_with_fetcher(
        "esri",
        1,
        2,
        3,
        &cache,
        &tilemap_cache,
        mock_fetch,
        cache_enabled,
    );
    assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
}

#[test]
fn tile_caching_is_on_by_default_with_no_per_provider_exceptions() {
    let config = config::CacheConfig::default();
    // One switch, no provider carve-outs. Note what this flag does and does
    // not decide: it permits the cache to be touched at all. Whether any
    // given tile is actually written is a separate gate in `TileCache::put`,
    // which stores only on an explicit `Cache-Control` grant - so "enabled"
    // here never means "store whatever comes back".
    assert!(config.tile_cache_enabled);
    assert!(
        config.providers.is_empty(),
        "per-provider overrides are opt-in TTLs, not a second on/off switch"
    );
}

// === Response builder tests ===

#[test]
fn test_build_ok_response() {
    let response = build_ok_response(b"test_data".to_vec(), "image/jpeg");
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(response.body(), b"test_data");
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "image/jpeg"
    );
}

#[test]
fn test_build_error_response() {
    let response = build_error_response(http::StatusCode::BAD_REQUEST, "test error");
    assert_eq!(response.status(), http::StatusCode::BAD_REQUEST);
    assert_eq!(response.body(), b"test error");
}

#[test]
fn test_build_ok_response_fallback_on_invalid_content_type() {
    let response = build_ok_response(b"data".to_vec(), "invalid\r\nheader");
    assert_eq!(response.status(), http::StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response.body(), b"Internal error building response");
}

#[test]
fn test_fetch_tile_from_network_unknown_provider_errors() {
    let result = fetch_tile_from_network("unknown", 0, 0, 0, None, None);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Unknown provider"));
}

// === Registry-based fetch tests ===

#[test]
fn test_fetch_tile_from_network_valid_provider_uses_registry_url() {
    // Verify that fetch_tile_from_network for a valid provider constructs the
    // correct URL via registry lookup. We can't test the actual network call,
    // but we verify the function doesn't error on provider lookup.
    let provider = tile_providers::get_provider("esri").unwrap();
    let url = tile_providers::expand_url(provider, 10, 500, 250);
    assert_eq!(
        url,
        "https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/10/500/250"
    );
}

#[test]
fn test_handle_tile_request_valid_provider_uses_registry() {
    // With a mock fetcher, verify that a valid provider passes registry validation.
    // Pre-seed tilemap cache so the test doesn't hit the network.
    let cache = make_test_cache();
    let tilemap_cache = make_test_tilemap_cache();
    {
        let (base_y, base_x) = TilemapCache::get_region_origin(2, 3);
        let mut tc = tilemap_cache.lock();
        tc.insert(
            "esri",
            1,
            base_y,
            base_x,
            vec![
                true;
                (tilemap_cache::TILEMAP_GRID_SIZE * tilemap_cache::TILEMAP_GRID_SIZE) as usize
            ],
        );
    }
    let mock_fetch = |_: &str,
                      _: u32,
                      _: u32,
                      _: u32,
                      _: Option<&str>|
     -> Result<TileFetchResponse, TileFetchError> {
        Ok(TileFetchResponse::Fetched(FetchedTile {
            data: b"tile_data".to_vec(),
            content_type: "image/jpeg".to_string(),
            policy: CachePolicy::default(),
            etag: None,
        }))
    };
    let response =
        handle_tile_request_with_fetcher("esri", 1, 2, 3, &cache, &tilemap_cache, mock_fetch, true);
    assert_eq!(response.status(), http::StatusCode::OK);
}

// === Config normalization tests ===

#[test]
fn test_map_config_normalize_unknown_provider_to_esri() {
    let mut map_config = config::MapConfig {
        satellite_provider: "nonexistent_provider".to_string(),
        satellite_enabled: true,
        satellite_gps_origin: config::GpsOrigin {
            latitude: 24.55,
            longitude: -81.78,
        },
        zoom_default: 15,
        zoom_min: 1,
        zoom_max: 22,
        zoom_step: 1.0,
        controls: config::MapControlsConfig {
            pan_speed: 1.0,
            zoom_speed: 1.0,
            invert_scroll: false,
            show_navigation_tools: false,
        },
        station: config::StationConfig::default(),
        shot: config::ShotConfig::default(),
        snap: config::MapSnapConfig::default(),
        animation: config::MapAnimationConfig::default(),
        highlight_warnings_on_tab_switch: true,
    };
    map_config.normalize();
    assert_eq!(map_config.satellite_provider, "esri");
}

#[test]
fn test_map_config_normalize_valid_provider_unchanged() {
    let mut map_config = config::MapConfig {
        satellite_provider: "esri".to_string(),
        satellite_enabled: true,
        satellite_gps_origin: config::GpsOrigin {
            latitude: 24.55,
            longitude: -81.78,
        },
        zoom_default: 15,
        zoom_min: 1,
        zoom_max: 22,
        zoom_step: 1.0,
        controls: config::MapControlsConfig {
            pan_speed: 1.0,
            zoom_speed: 1.0,
            invert_scroll: false,
            show_navigation_tools: false,
        },
        station: config::StationConfig::default(),
        shot: config::ShotConfig::default(),
        snap: config::MapSnapConfig::default(),
        animation: config::MapAnimationConfig::default(),
        highlight_warnings_on_tab_switch: true,
    };
    map_config.normalize();
    assert_eq!(map_config.satellite_provider, "esri");
}

// === Conditional revalidation through the real fetch path ===

/// Once a tile is past its `max-age`, the next request must carry the stored
/// ETag - that is the whole saving. Esri answers a match with `304` and no
/// body (~20 KB per tile over real imagery), and since imagery changes about
/// yearly while `max-age` expires daily, nearly every revalidation is a 304.
#[test]
fn a_stale_tile_is_revalidated_with_its_etag_and_served_without_new_bytes() {
    let cache = make_test_cache();
    let tilemap_cache = make_test_tilemap_cache();

    {
        let mut guard = cache.lock();
        let c = guard.as_mut().unwrap();
        c.put(
            "esri",
            1,
            2,
            3,
            b"cached bytes",
            "image/jpeg",
            CachePolicy {
                max_age_seconds: Some(60),
                no_store: false,
                age_seconds: None,
                no_cache: false,
                malformed: false,
            },
            Some("\"abc\""),
        )
        .unwrap();
        // Age it past max-age only; the 30-day retention is untouched.
        c.expire_max_age_for_test("esri", 1, 2, 3).unwrap();
    }

    let seen_validator = Arc::new(Mutex::new(None::<Option<String>>));
    let recorder = Arc::clone(&seen_validator);
    let mock = move |_: &str,
                     _: u32,
                     _: u32,
                     _: u32,
                     validator: Option<&str>|
          -> Result<TileFetchResponse, TileFetchError> {
        *recorder.lock().unwrap() = Some(validator.map(str::to_string));
        Ok(TileFetchResponse::NotModified {
            policy: CachePolicy {
                max_age_seconds: Some(86_400),
                no_store: false,
                age_seconds: None,
                no_cache: false,
                malformed: false,
            },
        })
    };

    let result =
        fetch_best_available_with_fetcher("esri", 1, 2, 3, &cache, &tilemap_cache, &mock, true)
            .unwrap();

    assert_eq!(
        seen_validator.lock().unwrap().clone(),
        Some(Some("\"abc\"".to_string())),
        "the stored ETag must travel as If-None-Match"
    );
    assert_eq!(
        result.data, b"cached bytes",
        "the cached bytes are what gets served - no body came back"
    );

    // And the 304's directives put it back in the fresh window, so the next
    // request is a pure cache hit with no round trip at all.
    let mut guard = cache.lock();
    assert!(guard.as_mut().unwrap().get("esri", 1, 2, 3).is_some());
}

#[test]
fn a_cold_cache_asks_unconditionally() {
    let cache = make_test_cache();
    let tilemap_cache = make_test_tilemap_cache();

    let seen = Arc::new(Mutex::new(None::<Option<String>>));
    let recorder = Arc::clone(&seen);
    let mock = move |_: &str,
                     _: u32,
                     _: u32,
                     _: u32,
                     validator: Option<&str>|
          -> Result<TileFetchResponse, TileFetchError> {
        *recorder.lock().unwrap() = Some(validator.map(str::to_string));
        Ok(TileFetchResponse::Fetched(FetchedTile {
            data: b"fresh".to_vec(),
            content_type: "image/jpeg".to_string(),
            policy: CachePolicy::default(),
            etag: None,
        }))
    };

    fetch_best_available_with_fetcher("esri", 4, 5, 6, &cache, &tilemap_cache, &mock, true)
        .unwrap();

    // Nothing to validate against, so no If-None-Match - sending a stale or
    // borrowed validator would risk a 304 for imagery we do not hold.
    assert_eq!(seen.lock().unwrap().clone(), Some(None));
}

/// A provider that answers 304 to a request that carried no validator would
/// otherwise leave us with nothing to serve. Recover by asking in full
/// rather than failing the tile.
#[test]
fn a_304_with_nothing_cached_falls_back_to_a_full_fetch() {
    let cache = make_test_cache();
    let tilemap_cache = make_test_tilemap_cache();

    let calls = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&calls);
    let mock = move |_: &str,
                     _: u32,
                     _: u32,
                     _: u32,
                     _: Option<&str>|
          -> Result<TileFetchResponse, TileFetchError> {
        if counter.fetch_add(1, Ordering::Relaxed) == 0 {
            Ok(TileFetchResponse::NotModified {
                policy: CachePolicy::default(),
            })
        } else {
            Ok(TileFetchResponse::Fetched(FetchedTile {
                data: b"recovered".to_vec(),
                content_type: "image/jpeg".to_string(),
                policy: CachePolicy::default(),
                etag: None,
            }))
        }
    };

    let result =
        fetch_best_available_with_fetcher("esri", 7, 8, 9, &cache, &tilemap_cache, &mock, true)
            .unwrap();
    assert_eq!(result.data, b"recovered");
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

// === get_tile_providers IPC tests ===

#[test]
fn test_get_tile_providers_returns_non_empty() {
    let providers = tile_providers::get_all_provider_info();
    assert!(!providers.is_empty());
}

#[test]
fn test_get_tile_providers_esri_available() {
    let providers = tile_providers::get_all_provider_info();
    let esri = providers.iter().find(|p| p.name == "esri").unwrap();
    assert!(esri.available);
}

#[test]
fn test_get_tile_providers_response_serializes() {
    let providers = tile_providers::get_all_provider_info();
    let json = serde_json::to_value(&providers).unwrap();
    let arr = json.as_array().unwrap();
    assert!(!arr.is_empty());
    let esri = arr.iter().find(|v| v["name"] == "esri").unwrap();
    assert_eq!(esri["available"], true);
    assert!(esri.get("display_label").is_some());
    assert!(esri.get("url_template").is_some());
    assert!(esri.get("tile_size").is_some());
}

// === TileFetchStats tests ===

#[test]
fn test_tile_fetch_stats_new_all_zeros() {
    let stats = TileFetchStats::new();
    let snap = stats.snapshot();
    assert_eq!(snap.in_flight, 0);
    assert_eq!(snap.total_fetched, 0);
    assert_eq!(snap.total_errors, 0);
    assert_eq!(snap.total_latency_ms, 0);
    assert_eq!(snap.last_latency_ms, 0);
}

#[test]
fn test_tile_fetch_stats_record_start_increments_in_flight() {
    let stats = TileFetchStats::new();
    stats.record_start();
    assert_eq!(stats.snapshot().in_flight, 1);
    stats.record_start();
    assert_eq!(stats.snapshot().in_flight, 2);
}

#[test]
fn test_tile_fetch_stats_record_success() {
    let stats = TileFetchStats::new();
    stats.record_start();
    stats.record_success(150);
    let snap = stats.snapshot();
    assert_eq!(snap.in_flight, 0);
    assert_eq!(snap.total_fetched, 1);
    assert_eq!(snap.total_latency_ms, 150);
    assert_eq!(snap.last_latency_ms, 150);
}

#[test]
fn test_tile_fetch_stats_record_success_accumulates_latency() {
    let stats = TileFetchStats::new();
    stats.record_start();
    stats.record_success(100);
    stats.record_start();
    stats.record_success(200);
    let snap = stats.snapshot();
    assert_eq!(snap.total_fetched, 2);
    assert_eq!(snap.total_latency_ms, 300);
    assert_eq!(snap.last_latency_ms, 200); // last overwrites
}

#[test]
fn test_tile_fetch_stats_record_error() {
    let stats = TileFetchStats::new();
    stats.record_start();
    stats.record_error();
    let snap = stats.snapshot();
    assert_eq!(snap.in_flight, 0);
    assert_eq!(snap.total_errors, 1);
    assert_eq!(snap.total_fetched, 0); // not counted as fetched
}

#[test]
fn test_tile_fetch_stats_mixed_operations() {
    let stats = TileFetchStats::new();
    // 3 starts
    stats.record_start();
    stats.record_start();
    stats.record_start();
    assert_eq!(stats.snapshot().in_flight, 3);
    // 1 success, 1 error
    stats.record_success(50);
    stats.record_error();
    let snap = stats.snapshot();
    assert_eq!(snap.in_flight, 1); // 3 - 1 - 1
    assert_eq!(snap.total_fetched, 1);
    assert_eq!(snap.total_errors, 1);
    assert_eq!(snap.total_latency_ms, 50);
}

// === normalize_content_type tests ===

#[test]
fn test_normalize_content_type_jpg_to_jpeg() {
    assert_eq!(normalize_content_type("image/jpg"), "image/jpeg");
}

#[test]
fn test_normalize_content_type_jpeg_passthrough() {
    assert_eq!(normalize_content_type("image/jpeg"), "image/jpeg");
}

#[test]
fn test_normalize_content_type_png_passthrough() {
    assert_eq!(normalize_content_type("image/png"), "image/png");
}

#[test]
fn test_normalize_content_type_other_passthrough() {
    assert_eq!(normalize_content_type("text/html"), "text/html");
}

// === output_format_from_hint / output_content_type_for_format tests ===

#[test]
fn test_output_format_from_hint_png() {
    assert_eq!(output_format_from_hint("image/png"), ImageFormat::Png);
}

#[test]
fn test_output_format_from_hint_jpeg_default() {
    assert_eq!(output_format_from_hint("image/jpeg"), ImageFormat::Jpeg);
}

#[test]
fn test_output_format_from_hint_jpg_normalizes_to_jpeg() {
    // "image/jpg" normalizes to "image/jpeg" which maps to Jpeg
    assert_eq!(output_format_from_hint("image/jpg"), ImageFormat::Jpeg);
}

#[test]
fn test_output_format_from_hint_unknown_defaults_to_jpeg() {
    assert_eq!(output_format_from_hint("text/html"), ImageFormat::Jpeg);
}

#[test]
fn test_output_content_type_for_format_png() {
    assert_eq!(
        output_content_type_for_format(ImageFormat::Png),
        "image/png"
    );
}

#[test]
fn test_output_content_type_for_format_jpeg() {
    assert_eq!(
        output_content_type_for_format(ImageFormat::Jpeg),
        "image/jpeg"
    );
}

// === is_retryable_error tests ===

#[test]
fn test_is_retryable_error_status_code_not_retryable() {
    // HTTP status codes should NOT be retried
    let err = ureq::Error::StatusCode(404);
    assert!(!is_retryable_error(&err));
    let err = ureq::Error::StatusCode(500);
    assert!(!is_retryable_error(&err));
}

#[test]
fn test_is_retryable_error_transport_is_retryable() {
    // Transport errors (connection issues) SHOULD be retried
    let err = ureq::Error::Io(std::io::Error::new(
        std::io::ErrorKind::ConnectionReset,
        "reset",
    ));
    assert!(is_retryable_error(&err));
}
