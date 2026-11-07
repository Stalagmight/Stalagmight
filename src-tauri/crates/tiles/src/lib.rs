//! The map-tile engine behind the `stalagmight-tiles://` protocol and the
//! `fetch_tile` command (#576 C8): the RFC 9111 caching client over
//! [`TileCache`], the retrying fetch, the ArcGIS tilemap probe,
//! stale-if-error, and the overzoom fallback with its decoded-parent cache.
//!
//! Tauri-free on purpose, so `cargo test -p stalagmight-tiles` builds without
//! the app. The app crate keeps the protocol registration and the event /
//! telemetry emission around [`handle_tile_request`] and [`tile_fetch_impl`].
//! The on-disk caches themselves ([`TileCache`], [`TilemapCache`]) stay in
//! `stalagmight-persistence`, where `AppState` (in the core member, which this
//! crate depends on) can own them.

use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageFormat};
use std::collections::HashMap as StdHashMap;
use std::io::Cursor;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use stalagmight_core::models::poison_policy::RecoveringMutex;
use stalagmight_core::tile_stats::TileFetchStats;
use stalagmight_core::{config, tile_providers};
use stalagmight_persistence::{cache, tilemap_cache};

use cache::tiles::CachePolicy;
use cache::TileCache;
use tilemap_cache::TilemapCache;

/// LRU cache for decoded parent tile images.
///
/// When deriving overzoom tiles, multiple child tiles crop from the same parent.
/// Without this cache, each child independently decodes the parent JPEG (~5ms each).
/// With 16 children (2 zoom levels of overzoom), that's 16 redundant decodes.
///
/// The cache is keyed by (provider, z, y, x) and stores the decoded `DynamicImage`.
/// A small capacity (4-8 entries) is sufficient since children of the same parent
/// are requested in bursts.
struct DecodedImageCache {
    entries: StdHashMap<(String, u32, u32, u32), Arc<DynamicImage>>,
    order: Vec<(String, u32, u32, u32)>,
    capacity: usize,
}

impl DecodedImageCache {
    fn new(capacity: usize) -> Self {
        Self {
            entries: StdHashMap::with_capacity(capacity + 1),
            order: Vec::with_capacity(capacity + 1),
            capacity,
        }
    }

    fn get(&mut self, provider: &str, z: u32, y: u32, x: u32) -> Option<Arc<DynamicImage>> {
        let key = (provider.to_string(), z, y, x);
        if let Some(img) = self.entries.get(&key) {
            // Move to end (most recently used)
            if let Some(pos) = self.order.iter().position(|k| k == &key) {
                self.order.remove(pos);
                self.order.push(key);
            }
            Some(Arc::clone(img))
        } else {
            None
        }
    }

    fn put(&mut self, provider: &str, z: u32, y: u32, x: u32, img: Arc<DynamicImage>) {
        let key = (provider.to_string(), z, y, x);
        if self.entries.contains_key(&key) {
            // Update existing
            self.entries.insert(key.clone(), img);
            if let Some(pos) = self.order.iter().position(|k| k == &key) {
                self.order.remove(pos);
                self.order.push(key);
            }
        } else {
            // Evict oldest if at capacity
            if self.entries.len() >= self.capacity {
                if let Some(oldest) = self.order.first().cloned() {
                    self.entries.remove(&oldest);
                    self.order.remove(0);
                }
            }
            self.entries.insert(key.clone(), img);
            self.order.push(key);
        }
    }
}

/// Global decoded image cache, shared across all tile request threads.
/// Small capacity (8 entries) since parent tiles are requested in bursts.
static DECODED_IMAGE_CACHE: std::sync::LazyLock<RecoveringMutex<DecodedImageCache>> =
    std::sync::LazyLock::new(|| {
        RecoveringMutex::new("decoded_image_cache", DecodedImageCache::new(8))
    });

/// HTTP request timeout for tile fetches
const TILE_FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// HTTP request timeout for tilemap API fetches
const TILEMAP_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Allowed content types for tile responses (whitelist for security)
const ALLOWED_CONTENT_TYPES: &[&str] = &["image/jpeg", "image/jpg", "image/png"];

/// Default content type when upstream doesn't provide one or it's invalid
const DEFAULT_CONTENT_TYPE: &str = "image/jpeg";

/// Most bytes a tile response body may hold (#576 W30-7a). A map tile is a
/// 256 or 512 px JPEG/PNG: real ones are tens of KB, and even an
/// incompressible 512 px RGBA PNG is about 1 MiB (512 * 512 * 4 bytes plus
/// framing). 4 MiB is four times that worst case, so no real tile comes near
/// it, while a provider (or anything between us and it) streaming a huge body
/// is cut off there instead of at ureq's silent 10 MiB default. The cap counts
/// the DECODED body ([`read_body_capped`]): ureq's own limit counts the bytes
/// on the wire, which a gzip-encoded body can be a thousandth of.
const TILE_BODY_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// Most bytes a tilemap response body may hold (#576 W30-NF4), for the reason
/// [`TILE_BODY_MAX_BYTES`] gives. A tilemap is 128 x 128 availability flags as
/// compact JSON - `{"data":[1,0,...]}` and a few small fields, about 33 KB -
/// so 1 MiB is some thirty times that, while a body streaming far more is cut
/// off here instead of at ureq's silent 10 MiB default. Like the tile cap it
/// counts the decoded body, not the (possibly gzip) bytes on the wire.
const TILEMAP_BODY_MAX_BYTES: u64 = 1024 * 1024;

/// Maximum retry attempts for transient network errors (total attempts = 1 + TILE_FETCH_MAX_RETRIES).
const TILE_FETCH_MAX_RETRIES: u32 = 2;
/// Backoff delays between retries.
const TILE_FETCH_RETRY_DELAYS: [Duration; 2] =
    [Duration::from_millis(200), Duration::from_millis(500)];

/// Returns true if the error is a transient transport error worth retrying.
/// HTTP 4xx/5xx are NOT retried since the server understood the request.
/// Read EVERY `Cache-Control` field line, joined the way RFC 9110 5.3 defines
/// repeated fields: sending them on separate lines means exactly the same thing
/// as one comma-joined value.
///
/// `HeaderMap::get` returns only the first, so
///
/// ```text
/// Cache-Control: max-age=86400
/// Cache-Control: no-store
/// ```
///
/// read as a plain grant and the `no-store` was never seen - the app would store
/// imagery it had been told not to keep, which is the one failure this cache is
/// built to avoid. Realistic rather than theoretical: it is what an origin and
/// an intermediary each appending a directive produces, and Esri sits behind
/// CloudFront. `parse_cache_control` already splits quote-aware, so the joined
/// form needs no parser change.
fn read_cache_policy(headers: &http::HeaderMap) -> CachePolicy {
    let joined = headers
        .get_all("cache-control")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>()
        .join(", ");
    if joined.is_empty() {
        return CachePolicy::default();
    }
    let mut policy = cache::tiles::parse_cache_control(&joined);
    // `Age` is single-valued; a repeat is malformed, so the first wins.
    policy.age_seconds = headers
        .get("age")
        .and_then(|value| value.to_str().ok())
        .and_then(cache::tiles::parse_age);
    policy
}

/// Split a `ureq` failure into "the provider answered" and "we could not reach
/// it", preserving the status code in the first case.
///
/// This is the same distinction `is_retryable_error` already drew; the result
/// used to be thrown away at the `Result<_, String>` boundary, which is what let
/// a 403/404 be served off the stale path as though the provider were down.
fn classify_fetch_error(err: &ureq::Error) -> TileFetchError {
    match err {
        ureq::Error::StatusCode(code) => {
            TileFetchError::status(*code, format!("Network error: {err}"))
        }
        _ => TileFetchError::transport(format!("Network error: {err}")),
    }
}

/// Classify a failed read of a tile's body.
///
/// A body over `limit` is ureq's `BodyExceedsLimit`, matched structurally (it
/// is not wrapped in `Io`: `From<io::Error> for ureq::Error` unwraps its own
/// error). That is the provider answering with more than any tile can be, not
/// the connection failing, so it is reported as a status - like an unknown
/// provider - and therefore never served off a stored copy, and never counted
/// as "existence unknown", which would leave the tile to be downloaded again
/// on every request. Any other read failure is a truncated transfer: transport.
fn classify_body_error(err: &ureq::Error, limit: u64) -> TileFetchError {
    match err {
        ureq::Error::BodyExceedsLimit(_) => TileFetchError::status(
            0,
            format!(
                "Tile refused: the response body is larger than the {} limit for a map tile",
                describe_body_limit(limit)
            ),
        ),
        _ => TileFetchError::transport(format!("Failed to read response body: {err}")),
    }
}

/// Read a response body whole, refusing one that DECODES to more than `limit`
/// bytes with ureq's own `BodyExceedsLimit(limit)` (#576 RV19a-1).
///
/// ureq's `with_config().limit()` alone does not bound what the caller gets:
/// ureq asks for `accept-encoding: gzip` (its default `gzip` feature) and
/// inflates ABOVE that limit, so it counts compressed bytes - 65 KB of gzip
/// decoded to 64 MiB under a 4 MiB limit. The wire limit stays, so a stream
/// that never ends but inflates to almost nothing (empty deflate blocks) is
/// still cut off; the decoded stream is capped on top of it.
pub fn read_body_capped(body: ureq::Body, limit: u64) -> Result<Vec<u8>, ureq::Error> {
    use std::io::Read as _;
    let mut decoded = Vec::new();
    body.into_with_config()
        .limit(limit)
        .reader()
        .take(limit.saturating_add(1))
        .read_to_end(&mut decoded)
        // ureq's own errors (the wire limit, a timeout) arrive wrapped in an
        // io::Error; `From` unwraps them, as ureq's `read_to_vec` does.
        .map_err(ureq::Error::from)?;
    if decoded.len() as u64 > limit {
        return Err(ureq::Error::BodyExceedsLimit(limit));
    }
    Ok(decoded)
}

/// A body limit as a person reads it: whole MiB from 1 MiB up, exact bytes
/// otherwise (where the test limits live).
fn describe_body_limit(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    if bytes >= MIB && bytes.is_multiple_of(MIB) {
        format!("{} MiB", bytes / MIB)
    } else {
        format!("{bytes} bytes")
    }
}

fn is_retryable_error(err: &ureq::Error) -> bool {
    match err {
        ureq::Error::StatusCode(_) => false, // Server responded - don't retry
        _ => true, // Transport errors (DNS, timeout, connection reset, etc.)
    }
}

/// Fetches a tile from the network based on provider.
///
/// If `stats` is provided, records in-flight / success / error metrics.
/// Retries up to `TILE_FETCH_MAX_RETRIES` times on transient transport errors
/// with exponential backoff (200ms, 500ms).
///
/// `if_none_match` turns this into a conditional request. Esri honours it: a
/// matching validator comes back `304` with no body, which over real imagery
/// saves ~20 KB per tile - and imagery changes on the order of yearly while
/// `max-age` expires daily, so past the first day nearly every revalidation is
/// a 304.
fn fetch_tile_from_network(
    provider: &str,
    z: u32,
    y: u32,
    x: u32,
    stats: Option<&TileFetchStats>,
    if_none_match: Option<&str>,
) -> Result<TileFetchResponse, TileFetchError> {
    // Look up provider in registry
    let tile_provider = tile_providers::get_provider(provider).ok_or_else(|| {
        // Not a network condition, but not something a stored copy should paper
        // over either - a request for an unknown provider is a bug, not an
        // outage. Reported as a status so it cannot reach the stale path.
        TileFetchError::status(0, format!("Unknown provider: {provider}"))
    })?;
    let url = tile_providers::expand_url(tile_provider, z, y, x);
    fetch_tile_from_url(
        &url,
        provider,
        z,
        y,
        x,
        stats,
        if_none_match,
        TILE_BODY_MAX_BYTES,
    )
}

/// [`fetch_tile_from_network`] once the provider's URL is known, with the body
/// ceiling spelled out, so a test can serve a tile from a loopback stub and
/// trip the ceiling without sending megabytes.
fn fetch_tile_from_url(
    url: &str,
    provider: &str,
    z: u32,
    y: u32,
    x: u32,
    stats: Option<&TileFetchStats>,
    if_none_match: Option<&str>,
    body_limit: u64,
) -> Result<TileFetchResponse, TileFetchError> {
    tracing::debug!(url = %url, provider = %provider, "Fetching tile from network");

    if let Some(s) = stats {
        s.record_start();
    }

    let start = std::time::Instant::now();
    let mut last_err: Option<TileFetchError> = None;

    for attempt in 0..=TILE_FETCH_MAX_RETRIES {
        if attempt > 0 {
            let delay = TILE_FETCH_RETRY_DELAYS
                .get((attempt - 1) as usize)
                .copied()
                .unwrap_or(TILE_FETCH_RETRY_DELAYS[TILE_FETCH_RETRY_DELAYS.len() - 1]);
            tracing::debug!(
                attempt = attempt + 1,
                delay_ms = delay.as_millis() as u64,
                provider = %provider,
                z = z,
                y = y,
                x = x,
                "Retrying tile fetch"
            );
            std::thread::sleep(delay);
        }

        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(TILE_FETCH_TIMEOUT))
            .build()
            .new_agent();

        let mut request = agent.get(url);
        if let Some(validator) = if_none_match {
            request = request.header("If-None-Match", validator);
        }

        match request.call() {
            Ok(response) => {
                let latency_ms = start.elapsed().as_millis() as u64;

                // 304 arrives here as an ordinary Ok response - ureq only turns
                // 4xx/5xx into errors, and its redirect handling leaves a
                // body-less 3xx alone (verified against the live service).
                if response.status() == 304 {
                    // A 304 carries `Cache-Control` and `Age` too, and it is the
                    // response that runs every day after the first - so both the
                    // multi-value read and the age subtraction matter most here.
                    let policy = read_cache_policy(response.headers());
                    if let Some(s) = stats {
                        s.record_success(latency_ms);
                    }
                    tracing::debug!(
                        provider = %provider,
                        z = z, y = y, x = x,
                        "Cached tile revalidated (304), no body transferred"
                    );
                    return Ok(TileFetchResponse::NotModified { policy });
                }

                // Sanitize content type - only allow whitelisted image types
                let raw_content_type = response
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or(DEFAULT_CONTENT_TYPE);

                let content_type = sanitize_content_type(raw_content_type);

                // Read before `into_body` consumes the response.
                let policy = read_cache_policy(response.headers());
                let etag = response
                    .headers()
                    .get("etag")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);

                // A failed read returns from here rather than reaching the
                // retry arm below, so an oversized body is downloaded once.
                let body = read_body_capped(response.into_body(), body_limit).map_err(|e| {
                    if let Some(s) = stats {
                        s.record_error();
                    }
                    classify_body_error(&e, body_limit)
                })?;

                if let Some(s) = stats {
                    s.record_success(latency_ms);
                }

                return Ok(TileFetchResponse::Fetched(FetchedTile {
                    data: body,
                    content_type,
                    policy,
                    etag,
                }));
            }
            Err(e) => {
                if attempt < TILE_FETCH_MAX_RETRIES && is_retryable_error(&e) {
                    tracing::debug!(
                        error = %e,
                        attempt = attempt + 1,
                        provider = %provider,
                        "Tile fetch failed, will retry"
                    );
                    last_err = Some(classify_fetch_error(&e));
                    continue;
                }

                // Final attempt or non-retryable error
                if let Some(s) = stats {
                    s.record_error();
                }
                return Err(classify_fetch_error(&e));
            }
        }
    }

    // Should not be reached, but just in case
    if let Some(s) = stats {
        s.record_error();
    }
    Err(last_err.unwrap_or_else(|| TileFetchError::transport("Tile fetch failed".to_string())))
}

/// Response from the ArcGIS tilemap API.
#[derive(serde::Deserialize)]
struct TilemapResponse {
    /// Availability data: 0 = no data, 1 = real imagery.
    data: Vec<u8>,
}

/// Why [`fetch_tilemap`] produced no tilemap.
#[derive(Debug)]
enum TilemapFetchError {
    /// The provider answered with a body over [`TILEMAP_BODY_MAX_BYTES`]:
    /// more than any tilemap can be, so asking again would only be refused
    /// again (#576 RV19a-10).
    TooLarge(String),
    /// Anything else - network, status, a body that did not parse.
    Failed(String),
}

impl std::fmt::Display for TilemapFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge(message) | Self::Failed(message) => f.write_str(message),
        }
    }
}

/// Fetch tilemap data from the provider's tilemap API.
///
/// Returns a Vec<bool> where true = tile available, false = no data.
/// The vector contains TILEMAP_GRID_SIZE × TILEMAP_GRID_SIZE (16,384) entries.
fn fetch_tilemap(
    provider: &tile_providers::TileProvider,
    z: u32,
    base_y: u32,
    base_x: u32,
) -> Result<Vec<bool>, TilemapFetchError> {
    let url = tile_providers::expand_tilemap_url(provider, z, base_y, base_x).ok_or_else(|| {
        TilemapFetchError::Failed("Provider has no tilemap URL configured".to_string())
    })?;

    tracing::debug!(url = %url, provider = %provider.name, z = z, base_y = base_y, base_x = base_x, "Fetching tilemap");

    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(TILEMAP_FETCH_TIMEOUT))
        .build()
        .new_agent();

    let response = agent
        .get(&url)
        .call()
        .map_err(|e| TilemapFetchError::Failed(format!("Tilemap network error: {e}")))?;

    let body =
        read_body_capped(response.into_body(), TILEMAP_BODY_MAX_BYTES).map_err(|e| match e {
            ureq::Error::BodyExceedsLimit(_) => TilemapFetchError::TooLarge(format!(
                "Tilemap refused: the response body is larger than the {} limit for a tilemap",
                describe_body_limit(TILEMAP_BODY_MAX_BYTES)
            )),
            _ => TilemapFetchError::Failed(format!("Failed to read tilemap response: {e}")),
        })?;

    let tilemap_response: TilemapResponse = serde_json::from_slice(&body)
        .map_err(|e| TilemapFetchError::Failed(format!("Failed to parse tilemap JSON: {e}")))?;

    // Convert u8 (0/1) to bool
    let data: Vec<bool> = tilemap_response.data.into_iter().map(|v| v != 0).collect();

    let expected_len =
        (tilemap_cache::TILEMAP_GRID_SIZE * tilemap_cache::TILEMAP_GRID_SIZE) as usize;
    if data.len() != expected_len {
        return Err(TilemapFetchError::Failed(format!(
            "Unexpected tilemap data length: got {}, expected {}",
            data.len(),
            expected_len
        )));
    }

    Ok(data)
}

/// Check tile availability using the tilemap cache and API.
///
/// Returns:
/// - `Some(true)` if the tile is known to have real imagery
/// - `Some(false)` if the tile is known to have no data
/// - `None` if availability is unknown (API unavailable, provider has no tilemap, etc.)
fn check_tile_availability(
    tilemap_cache: &Arc<RecoveringMutex<TilemapCache>>,
    provider: &tile_providers::TileProvider,
    z: u32,
    y: u32,
    x: u32,
) -> Option<bool> {
    // Only check tilemap if provider supports it
    provider.tilemap_url_template.as_ref()?;

    // Check cache first
    {
        let mut cache = tilemap_cache.lock();
        if let Some(available) = cache.is_tile_available(&provider.name, z, y, x) {
            tracing::trace!(
                provider = %provider.name,
                z = z,
                y = y,
                x = x,
                available = available,
                "Tilemap cache hit"
            );
            return Some(available);
        }
        // A region whose tilemap was refused for its size stays unknown until
        // that verdict expires, instead of being downloaded again for every
        // tile in it (#576 RV19a-10).
        if cache.is_refused(&provider.name, z, y, x) {
            tracing::trace!(
                provider = %provider.name,
                z = z,
                y = y,
                x = x,
                "Tilemap refused earlier for this region, availability unknown"
            );
            return None;
        }
    }

    // Cache miss - fetch tilemap for this region
    let (base_y, base_x) = TilemapCache::get_region_origin(y, x);

    match fetch_tilemap(provider, z, base_y, base_x) {
        Ok(data) => {
            // Cache the result
            {
                tilemap_cache
                    .lock()
                    .insert(&provider.name, z, base_y, base_x, data.clone());
            }

            // Calculate index and return availability
            let offset_y = y - base_y;
            let offset_x = x - base_x;
            let index = (offset_y * tilemap_cache::TILEMAP_GRID_SIZE + offset_x) as usize;
            let available = data.get(index).copied().unwrap_or(true);

            tracing::debug!(
                provider = %provider.name,
                z = z,
                y = y,
                x = x,
                available = available,
                "Tilemap fetched and cached"
            );

            Some(available)
        }
        Err(e) => {
            // Only the size refusal is remembered: a network or parse failure
            // may clear up, so the region is asked again next time.
            if matches!(e, TilemapFetchError::TooLarge(_)) {
                tilemap_cache
                    .lock()
                    .insert_refused(&provider.name, z, base_y, base_x);
            }
            tracing::warn!(
                provider = %provider.name,
                z = z,
                y = y,
                x = x,
                error = %e,
                "Failed to fetch tilemap, availability unknown"
            );
            None
        }
    }
}

/// Sanitize content type to only allow whitelisted image types
fn sanitize_content_type(raw: &str) -> String {
    // Extract the base content type (before any ; parameters like charset)
    let base = raw.split(';').next().unwrap_or(raw).trim().to_lowercase();

    if ALLOWED_CONTENT_TYPES.contains(&base.as_str()) {
        base
    } else {
        DEFAULT_CONTENT_TYPE.to_string()
    }
}

fn normalize_content_type(content_type: &str) -> &str {
    match content_type {
        "image/jpg" => "image/jpeg",
        other => other,
    }
}

fn output_format_from_hint(content_type_hint: &str) -> ImageFormat {
    match normalize_content_type(content_type_hint) {
        "image/png" => ImageFormat::Png,
        _ => ImageFormat::Jpeg,
    }
}

fn output_content_type_for_format(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "image/png",
        _ => "image/jpeg",
    }
}

/// Parses tile coordinates from a URI.
/// Supports both path-based (/esri/z/y/x) and host-based (esri/{z}/{y}/{x}) formats.
///
/// Path-based: `stalagmight-tiles://localhost/esri/10/500/250`
/// Host-based: `stalagmight-tiles://esri/10/500/250`
///
/// Returns None for malformed URIs.
pub fn parse_tile_uri(uri: &http::Uri) -> Option<(String, u32, u32, u32)> {
    let path = uri.path().trim_start_matches('/');
    let mut segments = path.split('/');

    // Try path-based format first: /esri/{z}/{y}/{x}
    let first = segments.next()?;

    // Check if first segment is a provider or a number (z coordinate)
    if let Ok(z) = first.parse::<u32>() {
        // Host-based format: provider is in host, path is /{z}/{y}/{x}
        // Host is REQUIRED for this format - reject malformed URIs without host
        let host = uri.host()?;
        if host.is_empty() || host == "localhost" {
            // No valid provider in host, reject
            return None;
        }
        let provider = host.to_string();

        let y: u32 = segments.next()?.parse().ok()?;
        let x_str = segments.next()?.split('.').next()?;
        let x: u32 = x_str.parse().ok()?;

        Some((provider, z, y, x))
    } else {
        // Path-based format: /esri/{z}/{y}/{x}
        let provider = first.to_string();
        let z: u32 = segments.next()?.parse().ok()?;
        let y: u32 = segments.next()?.parse().ok()?;
        let x_str = segments.next()?.split('.').next()?;
        let x: u32 = x_str.parse().ok()?;

        Some((provider, z, y, x))
    }
}

/// Build a successful HTTP response with proper error handling
fn build_ok_response(data: Vec<u8>, content_type: &str) -> http::Response<Vec<u8>> {
    http::Response::builder()
        .status(http::StatusCode::OK)
        .header(http::header::CONTENT_TYPE, content_type)
        .header(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(data)
        .unwrap_or_else(|_| {
            let mut resp = http::Response::new(b"Internal error building response".to_vec());
            *resp.status_mut() = http::StatusCode::INTERNAL_SERVER_ERROR;
            resp
        })
}

/// Build an error HTTP response with proper error handling
pub fn build_error_response(status: http::StatusCode, message: &str) -> http::Response<Vec<u8>> {
    http::Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "text/plain")
        .header(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(message.as_bytes().to_vec())
        .unwrap_or_else(|_| {
            let mut resp = http::Response::new(b"Internal error".to_vec());
            *resp.status_mut() = http::StatusCode::INTERNAL_SERVER_ERROR;
            resp
        })
}

struct TileFetchResult {
    data: Vec<u8>,
    content_type: String,
    /// Freshness left on this tile, so an overzoomed crop of it cannot be cached
    /// for longer than the imagery it was cut from.
    policy: CachePolicy,
}

#[derive(Debug)]
struct FetchedTile {
    data: Vec<u8>,
    content_type: String,
    /// Freshness directives the provider sent with this tile. Esri answers
    /// `max-age=86400`, which is what bounds how long we may serve it as current.
    policy: CachePolicy,
    /// Validator for a future conditional re-fetch, stored alongside the tile.
    etag: Option<String>,
}

/// What a network round trip produced.
///
/// A conditional request can succeed without a body: `304 Not Modified` means
/// the copy we already hold is still current, which is the whole point of
/// sending the validator.
#[derive(Debug)]
enum TileFetchResponse {
    Fetched(FetchedTile),
    /// The provider confirmed our cached copy. Carries the refreshed directives
    /// off the 304 (Esri re-sends `max-age=86400`), so the stored deadline moves
    /// forward without a byte of imagery crossing the wire.
    NotModified {
        policy: CachePolicy,
    },
}

/// Why a tile fetch failed, in the only distinction the caller acts on.
///
/// `stale-if-error` (RFC 5861) applies to a cache that "cannot reach" the
/// origin: a transport failure, or a `500`/`502`/`503`/`504`. It does NOT apply
/// to a `403` or `404` - those are the provider answering, and answering that
/// this tile is not to be served. Flattening both into one string made them
/// indistinguishable, so a tile Esri had removed kept being displayed off the
/// stored copy until retention ran out.
#[derive(Debug, Clone)]
pub(crate) struct TileFetchError {
    /// Human-readable text, unchanged from what callers already surface.
    message: String,
    /// `Some(code)` when the provider answered with an HTTP status; `None` for a
    /// transport failure (DNS, timeout, connection reset, TLS).
    status: Option<u16>,
}

impl TileFetchError {
    fn transport(message: String) -> Self {
        Self {
            message,
            status: None,
        }
    }

    fn status(code: u16, message: String) -> Self {
        Self {
            message,
            status: Some(code),
        }
    }

    /// Whether a retained copy may answer this failure.
    ///
    /// True for transport failures and 5xx. False for everything else the
    /// provider says deliberately - notably 403 and 404.
    fn allows_stale_fallback(&self) -> bool {
        match self.status {
            None => true,
            Some(code) => (500..600).contains(&code),
        }
    }

    /// Whether this failure leaves it unknown if the provider has the tile.
    ///
    /// True for everything that allows the stale fallback (transport, 5xx),
    /// and also for `408 Request Timeout` and `429 Too Many Requests`: both
    /// say "ask later", not "absent" (#576 RV4-R-T1). Any other status is the
    /// provider answering - a `404` that the tile is absent, a `403` that it
    /// is refused. Decides whether an overzoomed crop may be stored under this
    /// tile's key; it does not widen the stale fallback, which stays at
    /// transport failures and 5xx.
    fn left_existence_unknown(&self) -> bool {
        self.allows_stale_fallback() || matches!(self.status, Some(408 | 429))
    }
}

impl std::fmt::Display for TileFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<TileFetchError> for String {
    fn from(err: TileFetchError) -> Self {
        err.message
    }
}

enum TileFetchOutcome {
    Tile {
        tile: TileFetchResult,
    },
    Missing {
        error: String,
        /// The provider refused this imagery outright, so climbing to a parent
        /// zoom must stop rather than serve an upscaled crop of the area it just
        /// denied. `401`/`403`/`410` only. A `404` still climbs: for most tile
        /// services that is how they say "no imagery at this zoom here", which
        /// is precisely the case overzoom exists to handle, so treating it as
        /// terminal would disable the feature. `410 Gone` gets no such
        /// carve-out - nobody uses it to mean "not at this zoom"; it says the
        /// imagery was deliberately removed, which is a statement like `403`
        /// rather than an absence. (Esri returns none of these - it answers 200
        /// with a blank tile and lets the tilemap probe decide - so this is for
        /// the providers the registry is built to take, not the one it ships.)
        denied: bool,
        /// We never learned whether the provider has this tile: the request
        /// did not get through (transport failure, a 5xx), the provider said
        /// "ask later" (408, 429) or the protocol misbehaved. A `404` is the
        /// opposite - an answer that it is absent.
        ///
        /// Decides whether an overzoomed crop may be STORED under this tile's
        /// key. A crop for a tile that does not exist is the best that tile
        /// will be until the parent's grant runs out; a crop for one we merely
        /// could not reach would keep answering as a fresh cache hit after the
        /// network came back, blurry, for up to that whole grant (#576 C16).
        transient: bool,
    },
}

/// Whether a failure is the provider refusing the imagery, as opposed to it
/// being absent or unreachable.
fn is_provider_denial(err: &TileFetchError) -> bool {
    matches!(err.status, Some(401 | 403 | 410))
}

/// Where the bytes we served actually came from, for the per-provider
/// consumption telemetry (#31).
///
/// The three are disjoint, and the middle one is why this is not a bool. A `304`
/// is a network round trip that transfers NO imagery: the pixels come off our
/// own disk. Counting it as a fetch would report near-total provider
/// consumption for a session that downloaded nothing, since past the first day
/// nearly every request revalidates - and consumption is precisely what the
/// metric exists to measure.
///
/// This tracks BYTES, not network contact. `Cache` also covers a failed attempt:
/// stale-if-error tries a revalidation first, and the overzoom fallback reaches
/// a cached parent only after the child fetch failed. Both are right for
/// consumption - no imagery was pulled - but neither means the provider went
/// untouched. `TileFetchStats` counts attempts, and is what the offline
/// indicator reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileServeSource {
    /// Bytes came from cache with no successful network response: a clean cache
    /// hit (which really does make no network call - see `cache_has_fresh`), a
    /// stale copy under stale-if-error, or an overzoomed crop of a cached
    /// parent.
    Cache,
    /// Revalidated with `If-None-Match`; the provider answered `304` and the
    /// body came from cache.
    Revalidated,
    /// Imagery bytes pulled from the provider.
    Network,
}

impl TileServeSource {
    /// The `tile.<kind>.<provider>` event name this maps to. Every variant must
    /// stay registered in `telemetry::registry` - a test enforces the trio per
    /// provider.
    pub fn telemetry_kind(self) -> &'static str {
        match self {
            Self::Cache => "cache",
            Self::Revalidated => "revalidate",
            Self::Network => "fetch",
        }
    }

    /// What a completed network call means for provenance, or `None` when it
    /// tells us nothing (an error leaves whatever the cache path decides).
    fn from_fetch(result: &Result<TileFetchResponse, TileFetchError>) -> Option<Self> {
        match result {
            Ok(TileFetchResponse::Fetched(_)) => Some(Self::Network),
            // A round trip, but no imagery. Recorded separately so `tile.fetch`
            // keeps meaning "consumed from the provider".
            Ok(TileFetchResponse::NotModified { .. }) => Some(Self::Revalidated),
            Err(_) => None,
        }
    }

    fn from_u8(raw: u8) -> Self {
        match raw {
            1 => Self::Revalidated,
            2 => Self::Network,
            _ => Self::Cache,
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            Self::Cache => 0,
            Self::Revalidated => 1,
            Self::Network => 2,
        }
    }
}

fn cache_put_best_effort(
    tile_cache: &Arc<RecoveringMutex<Option<TileCache>>>,
    provider: &str,
    z: u32,
    y: u32,
    x: u32,
    data: &[u8],
    content_type: &str,
    policy: CachePolicy,
    etag: Option<&str>,
    cache_enabled: bool,
) {
    if !cache_enabled {
        return;
    }
    {
        let mut cache_guard = tile_cache.lock();
        if let Some(cache) = cache_guard.as_mut() {
            if let Err(e) = cache.put(provider, z, y, x, data, content_type, policy, etag) {
                tracing::warn!(error = %e, "Failed to cache tile");
            }
        }
    }
}

/// Cache a freshly-fetched tile and hand it back. Shared by the ordinary fetch
/// path and the "304 with nothing cached" recovery, which must not diverge.
fn store_and_serve(
    tile_cache: &Arc<RecoveringMutex<Option<TileCache>>>,
    provider: &str,
    z: u32,
    y: u32,
    x: u32,
    fetched: FetchedTile,
    cache_enabled: bool,
) -> TileFetchOutcome {
    let FetchedTile {
        data,
        content_type,
        policy,
        etag,
    } = fetched;
    cache_put_best_effort(
        tile_cache,
        provider,
        z,
        y,
        x,
        &data,
        &content_type,
        policy,
        etag.as_deref(),
        cache_enabled,
    );
    TileFetchOutcome::Tile {
        tile: TileFetchResult {
            data,
            content_type,
            policy,
        },
    }
}

fn fetch_tile_candidate<F>(
    _provider_info: &tile_providers::TileProvider,
    provider: &str,
    z: u32,
    y: u32,
    x: u32,
    tile_cache: &Arc<RecoveringMutex<Option<TileCache>>>,
    fetch_fn: &F,
    cache_enabled: bool,
) -> TileFetchOutcome
where
    F: Fn(&str, u32, u32, u32, Option<&str>) -> Result<TileFetchResponse, TileFetchError>,
{
    // Fresh cache first (skip if per-provider caching disabled)
    let cached_tile = if cache_enabled {
        {
            let mut cache_guard = tile_cache.lock();
            if let Some(cache) = cache_guard.as_mut() {
                cache.get(provider, z, y, x)
            } else {
                None
            }
        }
    } else {
        None
    };

    if let Some(entry) = cached_tile {
        tracing::debug!(provider = %provider, z = z, y = y, x = x, "Cache hit (fresh)");
        return TileFetchOutcome::Tile {
            tile: TileFetchResult {
                policy: entry.remaining_policy(cache::tiles::now()),
                data: entry.data,
                content_type: entry.content_type,
            },
        };
    }

    // Not servable as fresh. Any RETAINED copy still earns its keep twice over:
    // its ETag makes the network call conditional, and its bytes answer the
    // request if that call fails. Read once, used for both - a second lookup in
    // the error arm would be a wasted query on the path where the network is
    // already unhappy.
    let retained = if cache_enabled {
        {
            let cache_guard = tile_cache.lock();
            cache_guard
                .as_ref()
                .and_then(|cache| cache.get_stale(provider, z, y, x))
        }
    } else {
        None
    };

    let validator = retained.as_ref().and_then(|entry| entry.etag.as_deref());

    match fetch_fn(provider, z, y, x, validator) {
        Ok(TileFetchResponse::NotModified { policy }) => {
            // The provider confirmed the copy we hold. Move its deadlines
            // forward and serve it - no imagery crossed the wire.
            let Some(entry) = retained else {
                // Only reachable if a 304 came back for a request that carried
                // no validator, which no provider should do. Re-ask
                // unconditionally rather than fail a tile over it.
                tracing::warn!(
                    provider = %provider,
                    z = z, y = y, x = x,
                    "304 for an unconditional request; refetching in full"
                );
                return match fetch_fn(provider, z, y, x, None) {
                    Ok(TileFetchResponse::Fetched(fetched)) => {
                        store_and_serve(tile_cache, provider, z, y, x, fetched, cache_enabled)
                    }
                    other => TileFetchOutcome::Missing {
                        error: "Tile not available: provider returned 304 with no cached copy"
                            .to_string(),
                        // A protocol oddity, not a refusal - a parent may serve.
                        denied: false,
                        transient: refetch_left_it_unknown(&other),
                    },
                };
            };

            {
                let mut cache_guard = tile_cache.lock();
                if let Some(cache) = cache_guard.as_mut() {
                    if let Err(e) = cache.refresh_validated(provider, z, y, x, policy) {
                        tracing::warn!(error = %e, "Failed to re-stamp revalidated tile");
                    }
                }
            }

            // `no-store` on the 304 revokes the grant this copy was held under,
            // and `refresh_validated` has just deleted the row. Serving those
            // bytes anyway would mean handing out a copy we were told to drop,
            // in the same breath as dropping it. Ask again without a validator
            // and serve what comes back - the gate refuses to store that too,
            // so nothing is retained either way. Tested against `policy`
            // directly rather than `refresh_validated`'s bool, which also
            // returns false for a legacy row that simply had nothing to update.
            if policy.no_store {
                tracing::debug!(
                    provider = %provider,
                    z = z, y = y, x = x,
                    "304 revoked the storage grant (no-store); refetching rather than \
                     serving the copy just dropped"
                );
                return match fetch_fn(provider, z, y, x, None) {
                    Ok(TileFetchResponse::Fetched(fetched)) => {
                        store_and_serve(tile_cache, provider, z, y, x, fetched, cache_enabled)
                    }
                    other => TileFetchOutcome::Missing {
                        error: "Tile not available: provider revoked storage on revalidation"
                            .to_string(),
                        // Storage was revoked, not access - a parent may serve.
                        denied: false,
                        transient: refetch_left_it_unknown(&other),
                    },
                };
            }

            TileFetchOutcome::Tile {
                tile: TileFetchResult {
                    policy,
                    data: entry.data,
                    content_type: entry.content_type,
                },
            }
        }
        Ok(TileFetchResponse::Fetched(fetched)) => {
            store_and_serve(tile_cache, provider, z, y, x, fetched, cache_enabled)
        }
        Err(e) => {
            tracing::warn!(error = %e, provider = %provider, z = z, y = y, x = x, "Network fetch failed");

            // The retained copy read above answers the failed request - but only
            // where stale-if-error actually applies. A 403 or 404 is the
            // provider answering, and answering that this tile is not to be
            // served; papering over that with a stored copy would keep showing
            // imagery the provider has withdrawn until retention ran out.
            if !e.allows_stale_fallback() {
                tracing::debug!(
                    provider = %provider,
                    z = z,
                    y = y,
                    x = x,
                    status = e.status.unwrap_or_default(),
                    had_retained = retained.is_some(),
                    "Provider answered with a non-5xx status; not serving a stored copy"
                );
                return TileFetchOutcome::Missing {
                    error: format!("Tile not available: {e}"),
                    denied: is_provider_denial(&e),
                    // The provider answered: absent (404) or refused - unless
                    // it only said "ask later" (408, 429).
                    transient: e.left_existence_unknown(),
                };
            }

            // A `no-cache` row is retained and revalidatable - that is what its
            // ETag above is for - but it may not answer a revalidation that
            // FAILED. RFC 9111 5.2.2.4 requires a successful validation before
            // reuse, so this is the one row type for which "the network is down"
            // forbids serving rather than being the reason to.
            if retained.as_ref().is_some_and(|entry| entry.no_cache) {
                tracing::debug!(
                    provider = %provider,
                    z = z, y = y, x = x,
                    "Held under no-cache and revalidation failed; not serving unvalidated"
                );
                return TileFetchOutcome::Missing {
                    error: format!("Tile not available: {e}"),
                    denied: false,
                    // Only transport failures and 5xx get this far.
                    transient: true,
                };
            }

            if let Some(entry) = retained {
                tracing::info!(
                    provider = %provider,
                    z = z,
                    y = y,
                    x = x,
                    purpose = "answer a request the provider could not",
                    "Provider unreachable; serving the stored copy (stale-if-error)"
                );
                return TileFetchOutcome::Tile {
                    tile: TileFetchResult {
                        // No grant travels with an unvalidated stale tile, so a
                        // crop of it is not written to disk. The grant we had
                        // has run out and the network could not renew it -
                        // minting a fresh one locally is exactly the thing the
                        // explicit-grant rule exists to prevent.
                        policy: CachePolicy::default(),
                        data: entry.data,
                        content_type: entry.content_type,
                    },
                };
            }

            TileFetchOutcome::Missing {
                error: format!("Tile not available: {e}"),
                denied: is_provider_denial(&e),
                // A transport failure or a 5xx with nothing stored: the
                // provider was not reached, so whether the tile exists is
                // unknown.
                transient: true,
            }
        }
    }
}

/// For the two "ask again without a validator" recoveries: did the second
/// attempt leave us not knowing whether the tile exists? Only a status that
/// answers (a 404, a refusal) says so; a transport failure, a 5xx, a 408 or
/// 429, or a second protocol oddity says nothing.
fn refetch_left_it_unknown(result: &Result<TileFetchResponse, TileFetchError>) -> bool {
    match result {
        Err(e) => e.left_existence_unknown(),
        Ok(_) => true,
    }
}

/// Largest width or height a tile may decode to (#576 W30-7b). Tiles are 256
/// or 512 px; 4096 is eight times the larger, room for any high-DPI variant,
/// while a header claiming more is refused before anything is allocated.
const TILE_DECODE_MAX_SIDE: u32 = 4096;

/// Most bytes a tile's decoded image may take (#576 W30-7b): the 8-bit RGBA
/// buffer of a tile at the side cap (4096 * 4096 * 4 bytes = 64 MiB). A 512 px
/// RGBA tile needs 1 MiB; the image crate's own default would allow 512 MiB.
///
/// It bounds the output buffer, which `ImageReader::decode` reserves against
/// it, and a PNG decoder's working memory (the image crate hands it to `png`
/// as its byte limit). It does NOT bound a JPEG decoder's working memory: the
/// image crate passes zune-jpeg only the side cap, so buffers such as a
/// progressive image's coefficients (two bytes per sample of each component:
/// 96 MiB for a 4096 px image with three full-resolution components) sit
/// outside it, bounded by [`TILE_DECODE_MAX_SIDE`] alone.
const TILE_DECODE_MAX_ALLOC: u64 = 64 * 1024 * 1024;

/// Decode tile bytes under [`TILE_DECODE_MAX_SIDE`] and
/// [`TILE_DECODE_MAX_ALLOC`]. `image::load_from_memory` applies only the image
/// crate's defaults - no side cap, up to 512 MiB of output - so a few bytes of
/// header claiming a huge image made it reserve hundreds of MiB before finding
/// no pixel data behind them. The checks run on the header, before the output
/// buffer is reserved.
fn decode_tile_image(bytes: &[u8]) -> image::ImageResult<DynamicImage> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(TILE_DECODE_MAX_SIDE);
    limits.max_image_height = Some(TILE_DECODE_MAX_SIDE);
    limits.max_alloc = Some(TILE_DECODE_MAX_ALLOC);
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(limits);
    reader.decode()
}

fn derive_tile_from_parent(
    provider: &str,
    provider_info: &tile_providers::TileProvider,
    parent_z: u32,
    parent_x: u32,
    parent_y: u32,
    target_z: u32,
    target_x: u32,
    target_y: u32,
    parent_tile: &TileFetchResult,
) -> Result<TileFetchResult, String> {
    if parent_z >= target_z {
        return Ok(TileFetchResult {
            data: parent_tile.data.clone(),
            content_type: parent_tile.content_type.clone(),
            policy: parent_tile.policy,
        });
    }

    let delta = target_z - parent_z;
    let scale = 1u32
        .checked_shl(delta)
        .ok_or_else(|| "Overzoom scale overflow".to_string())?;

    // Use decoded image cache to avoid redundant JPEG decoding.
    // When overzooming 2 levels, 16 child tiles share the same parent - caching
    // the decoded DynamicImage saves ~15 redundant decodes (~5ms each).
    let img = {
        let mut cache = DECODED_IMAGE_CACHE.lock();
        if let Some(cached) = cache.get(provider, parent_z, parent_y, parent_x) {
            cached
        } else {
            let decoded = decode_tile_image(&parent_tile.data)
                .map_err(|e| format!("Failed to decode parent tile: {e}"))?;
            let arc = Arc::new(decoded);
            cache.put(provider, parent_z, parent_y, parent_x, Arc::clone(&arc));
            arc
        }
    };

    let (img_w, img_h) = img.dimensions();
    let base_size = img_w.min(img_h);
    if base_size == 0 {
        return Err("Parent tile has zero dimensions".to_string());
    }

    if base_size % scale != 0 {
        return Err("Parent tile size not divisible by overzoom scale".to_string());
    }
    let sub_size = base_size / scale;

    let parent_x_scaled = parent_x
        .checked_mul(scale)
        .ok_or_else(|| "Overzoom x overflow".to_string())?;
    let parent_y_scaled = parent_y
        .checked_mul(scale)
        .ok_or_else(|| "Overzoom y overflow".to_string())?;
    let offset_x = target_x
        .checked_sub(parent_x_scaled)
        .ok_or_else(|| "Overzoom x underflow".to_string())?;
    let offset_y = target_y
        .checked_sub(parent_y_scaled)
        .ok_or_else(|| "Overzoom y underflow".to_string())?;

    if offset_x >= scale || offset_y >= scale {
        return Err("Overzoom offset out of range".to_string());
    }

    let crop_x = offset_x * sub_size;
    let crop_y = offset_y * sub_size;
    let cropped = img.crop_imm(crop_x, crop_y, sub_size, sub_size);

    let output_size = if provider_info.tile_size > 0 {
        provider_info.tile_size
    } else {
        base_size
    };

    let final_img = if sub_size == output_size {
        cropped
    } else {
        cropped.resize_exact(output_size, output_size, FilterType::CatmullRom)
    };
    let format = output_format_from_hint(&provider_info.content_type_hint);
    let mut buffer = Vec::new();
    {
        let mut cursor = Cursor::new(&mut buffer);
        final_img
            .write_to(&mut cursor, format)
            .map_err(|e| format!("Failed to encode derived tile: {e}"))?;
    }

    Ok(TileFetchResult {
        data: buffer,
        content_type: output_content_type_for_format(format).to_string(),
        // Inherited, not reset: a crop is only as fresh as what it was cut from.
        policy: parent_tile.policy,
    })
}

fn fetch_best_available_with_fetcher<F>(
    provider: &str,
    z: u32,
    y: u32,
    x: u32,
    tile_cache: &Arc<RecoveringMutex<Option<TileCache>>>,
    tilemap_cache: &Arc<RecoveringMutex<TilemapCache>>,
    fetch_fn: &F,
    cache_enabled: bool,
) -> Result<TileFetchResult, String>
where
    F: Fn(&str, u32, u32, u32, Option<&str>) -> Result<TileFetchResponse, TileFetchError>,
{
    let provider_info = tile_providers::get_provider(provider)
        .ok_or_else(|| format!("Unknown provider: {provider}"))?;

    let min_target = provider_info.min_zoom;

    let mut current_z = z;
    let mut current_x = x;
    let mut current_y = y;
    // Tracks the last error from `fetch_tile_candidate` (always set before loop exits
    // via `break`, since `Tile` outcomes return immediately).
    #[expect(unused_assignments)]
    let mut last_error = None;
    // Some level between the requested tile and the one finally served was
    // unreachable rather than absent; see `TileFetchOutcome::Missing::transient`.
    let mut climbed_past_unreachable = false;

    // Can the cache answer for this exact tile without asking anyone?
    //
    // Consulted BEFORE every tilemap probe, because the probe is itself a
    // NETWORK call whenever the in-memory tilemap cache is cold. Probing first
    // meant a request served entirely from disk could still reach the provider -
    // wasted traffic, and it made `tile.cache.<provider>` (documented as no
    // network call at all) a claim the code did not keep. Non-mutating, so it
    // cannot disturb the LRU ordering that `get` maintains.
    let cache_has_fresh = |cz: u32, cy: u32, cx: u32| -> bool {
        cache_enabled
            && tile_cache
                .lock()
                .as_ref()
                .is_some_and(|cache| cache.has_fresh_tile(provider, cz, cy, cx))
    };

    // Check tilemap for tile availability before entering the fetch loop.
    // If tilemap says tile is unavailable, skip directly to parent.
    if !cache_has_fresh(z, y, x) {
        if let Some(false) = check_tile_availability(tilemap_cache, provider_info, z, y, x) {
            tracing::debug!(
                provider = %provider,
                z = z,
                y = y,
                x = x,
                "Tilemap indicates no data, skipping to parent"
            );
            // Start from parent zoom level
            if z > min_target {
                current_z = z - 1;
                current_x = x / 2;
                current_y = y / 2;
            }
        }
    }

    loop {
        // For tiles at current_z < z, we can also check tilemap before fetching -
        // again only when the cache cannot already answer at that level.
        if current_z < z && !cache_has_fresh(current_z, current_y, current_x) {
            if let Some(false) = check_tile_availability(
                tilemap_cache,
                provider_info,
                current_z,
                current_y,
                current_x,
            ) {
                tracing::debug!(
                    provider = %provider,
                    z = current_z,
                    y = current_y,
                    x = current_x,
                    "Tilemap indicates parent has no data, continuing up"
                );
                if current_z > min_target {
                    current_z -= 1;
                    current_x /= 2;
                    current_y /= 2;
                    continue;
                }
            }
        }

        match fetch_tile_candidate(
            provider_info,
            provider,
            current_z,
            current_y,
            current_x,
            tile_cache,
            fetch_fn,
            cache_enabled,
        ) {
            TileFetchOutcome::Tile { tile } => {
                if current_z == z {
                    return Ok(tile);
                }

                let derived = derive_tile_from_parent(
                    provider,
                    provider_info,
                    current_z,
                    current_x,
                    current_y,
                    z,
                    x,
                    y,
                    &tile,
                )?;
                if climbed_past_unreachable {
                    // Served, not stored: a finer tile may well exist and was
                    // only out of reach. Re-deriving per request is cheap (the
                    // parent is cached, and so is its decode), and it lets the
                    // real tile replace the crop the moment the network is back.
                    tracing::debug!(
                        provider = %provider,
                        z = z,
                        y = y,
                        x = x,
                        fallback_z = current_z,
                        "Overzoomed crop not cached: a finer level was unreachable, not absent"
                    );
                } else {
                    cache_put_best_effort(
                        tile_cache,
                        provider,
                        z,
                        y,
                        x,
                        &derived.data,
                        &derived.content_type,
                        // A derived tile is a crop of the parent, so it lives
                        // under the parent's remaining grant - overzooming
                        // cannot mint a longer life than the imagery it came
                        // from. No etag: this bytes never came off the wire, so
                        // it has no validator.
                        tile.policy,
                        None,
                        cache_enabled,
                    );
                }
                tracing::info!(
                    provider = %provider,
                    z = z,
                    y = y,
                    x = x,
                    fallback_z = current_z,
                    purpose = "use best available tile resolution",
                    "Serving overzoomed parent tile"
                );
                return Ok(derived);
            }
            TileFetchOutcome::Missing {
                error,
                denied,
                transient,
            } => {
                if denied {
                    // The provider refused this imagery. Climbing to a parent
                    // would hand back an upscaled crop of the very area it just
                    // denied, which is the same refusal ignored at a coarser
                    // resolution - and it would walk straight past the guard
                    // that already stops a stored copy answering a non-5xx.
                    tracing::debug!(
                        provider = %provider,
                        z = current_z,
                        y = current_y,
                        x = current_x,
                        "Provider denied this imagery; not falling back to a parent zoom"
                    );
                    return Err(error);
                }
                climbed_past_unreachable |= transient;
                last_error = Some(error);
            }
        }

        if current_z <= min_target {
            break;
        }
        current_z -= 1;
        current_x /= 2;
        current_y /= 2;
    }

    Err(last_error.unwrap_or_else(|| "Tile not available".to_string()))
}

/// Handle a tile request - check cache, fetch from network, return response
/// Returns the tile response plus a per-request flag that is `true` only when THIS
/// request pulled a fresh tile from the provider over the network (its `fetch_fn`
/// returned a tile). Unlike diffing the shared `TileFetchStats` counter, this is
/// accurate under the concurrent tile loads the map issues, so it drives the exact
/// per-provider consumption telemetry (#31).
pub fn handle_tile_request(
    provider: &str,
    z: u32,
    y: u32,
    x: u32,
    tile_cache: &Arc<RecoveringMutex<Option<TileCache>>>,
    tilemap_cache: &Arc<RecoveringMutex<TilemapCache>>,
    stats: &Arc<TileFetchStats>,
    cache_config: &config::CacheConfig,
) -> (http::Response<Vec<u8>>, TileServeSource) {
    let stats_ref = Arc::clone(stats);
    // Per-request provenance for the consumption telemetry, recorded by
    // `fetch_fn` (the real network call) and left at `Cache` when it is never
    // reached. NOT what drives the offline indicator - that runs off the shared
    // `TileFetchStats` snapshot diff the app's protocol handler takes around
    // this call, which a 304 does move, because the network genuinely was
    // reached.
    let serve_source = Arc::new(AtomicU8::new(TileServeSource::Cache.as_u8()));
    let source_setter = Arc::clone(&serve_source);
    let fetch_fn = move |p: &str,
                         z: u32,
                         y: u32,
                         x: u32,
                         validator: Option<&str>|
          -> Result<TileFetchResponse, TileFetchError> {
        let result = fetch_tile_from_network(p, z, y, x, Some(&stats_ref), validator);
        if let Some(source) = TileServeSource::from_fetch(&result) {
            source_setter.store(source.as_u8(), Ordering::Relaxed);
        }
        result
    };
    let cache_enabled = cache_config.tile_cache_enabled;
    let response = handle_tile_request_with_fetcher(
        provider,
        z,
        y,
        x,
        tile_cache,
        tilemap_cache,
        fetch_fn,
        cache_enabled,
    );
    (
        response,
        TileServeSource::from_u8(serve_source.load(Ordering::Relaxed)),
    )
}

/// Internal implementation that accepts a fetch function for testability.
/// The fetch function signature: (provider, z, y, x) -> Result<(data, content_type), error_message>
fn handle_tile_request_with_fetcher<F>(
    provider: &str,
    z: u32,
    y: u32,
    x: u32,
    tile_cache: &Arc<RecoveringMutex<Option<TileCache>>>,
    tilemap_cache: &Arc<RecoveringMutex<TilemapCache>>,
    fetch_fn: F,
    cache_enabled: bool,
) -> http::Response<Vec<u8>>
where
    F: Fn(&str, u32, u32, u32, Option<&str>) -> Result<TileFetchResponse, TileFetchError>,
{
    // Validate provider against registry
    if tile_providers::get_provider(provider).is_none() {
        tracing::warn!(provider = %provider, "Unknown tile provider requested");
        return build_error_response(
            http::StatusCode::BAD_REQUEST,
            &format!("Unknown provider: {provider}"),
        );
    }

    match fetch_best_available_with_fetcher(
        provider,
        z,
        y,
        x,
        tile_cache,
        tilemap_cache,
        &fetch_fn,
        cache_enabled,
    ) {
        Ok(tile) => build_ok_response(tile.data, &tile.content_type),
        Err(e) => build_error_response(http::StatusCode::NOT_FOUND, &e),
    }
}

/// Fetch a tile and return its bytes directly.
/// Used by the `fetch_tile` IPC command.
pub fn tile_fetch_impl(
    provider: &str,
    z: u32,
    y: u32,
    x: u32,
    tile_cache: &Arc<RecoveringMutex<Option<TileCache>>>,
    tilemap_cache: &Arc<RecoveringMutex<TilemapCache>>,
    stats: &Arc<TileFetchStats>,
    cache_config: &config::CacheConfig,
) -> Result<(Vec<u8>, TileServeSource), String> {
    // Validate provider against registry
    if tile_providers::get_provider(provider).is_none() {
        return Err(format!("Unknown provider: {provider}"));
    }

    let cache_enabled = cache_config.tile_cache_enabled;
    let stats_ref = Arc::clone(stats);
    // Same provenance recording as the desktop protocol handler. Android does
    // not use that handler at all - MapLibre there calls the `fetch_tile` IPC
    // command, which lands here - so without this the #31 consumption events
    // were simply absent on that platform rather than wrong. Returned to the
    // caller, since only the command layer has the `AppHandle` the collector
    // hangs off.
    let serve_source = Arc::new(AtomicU8::new(TileServeSource::Cache.as_u8()));
    let source_setter = Arc::clone(&serve_source);
    let fetch_fn = move |p: &str,
                         z: u32,
                         y: u32,
                         x: u32,
                         validator: Option<&str>|
          -> Result<TileFetchResponse, TileFetchError> {
        let result = fetch_tile_from_network(p, z, y, x, Some(&stats_ref), validator);
        if let Some(source) = TileServeSource::from_fetch(&result) {
            source_setter.store(source.as_u8(), Ordering::Relaxed);
        }
        result
    };
    fetch_best_available_with_fetcher(
        provider,
        z,
        y,
        x,
        tile_cache,
        tilemap_cache,
        &fetch_fn,
        cache_enabled,
    )
    .map(|tile| {
        (
            tile.data,
            TileServeSource::from_u8(serve_source.load(Ordering::Relaxed)),
        )
    })
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests;

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod crop_storage_rv4_tests;

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod body_limit_tests;

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod tilemap_body_limit_tests;

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod gzip_body_limit_tests;

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod decode_limits_tests;
