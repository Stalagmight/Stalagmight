// Extracted to the `stalagmight-models` workspace member; re-exported under the
// old names so every `crate::app_dirs::*` / `crate::models::*` path (and the
// `stalagmight_lib::…` paths the integration tests and gen_ipc_types example
// use) keeps compiling unchanged.
pub use stalagmight_models::app_dirs;
pub use stalagmight_persistence::cache;
#[cfg(target_os = "macos")]
mod app_menu;
pub mod commands;
// Extracted to the `stalagmight-core` workspace member; re-exported under the
// old names, same deal as `models` below (`tile_providers` and, for the test
// builds, `test_helpers` ride along further down).
pub use stalagmight_core::config;
pub use stalagmight_core::core;
// Extracted to the `stalagmight-export` workspace member; re-exported under
// the old name, same deal as `models` below.
pub use stalagmight_export::export;
// Extracted to the `stalagmight-telemetry` workspace member; re-exported under
// the old names, same deal as `models` above.
pub use stalagmight_models as models;
pub use stalagmight_telemetry::logging;
// Extracted to the `stalagmight-parsers` workspace member; re-exported under
// the old name, same deal as `models` above.
pub use stalagmight_parsers::parsers;
// Extracted to the `stalagmight-persistence` workspace member (together with
// `cache` and `tilemap_cache`); re-exported under the old names, same deal.
pub use stalagmight_persistence::persistence;
// Extracted to the `stalagmight-speleodb` workspace member; re-exported under
// the old name, same deal as `models` above.
pub use stalagmight_speleodb::speleodb;
pub use stalagmight_telemetry::telemetry;
// The core member compiles `test_helpers` under its `test-support` feature
// (enabled by this crate's [dev-dependencies]), so every
// `crate::test_helpers::*` path in the tests keeps resolving.
#[cfg(test)]
pub use stalagmight_core::test_helpers;
pub mod tile_attribution;
pub use stalagmight_core::tile_providers;
pub use stalagmight_persistence::tilemap_cache;
/// Linux-only WebKit memory-pressure wiring (see the module docs and the
/// patched wry fork: <https://github.com/Stalagmight/wry>).
#[cfg(target_os = "linux")]
pub mod webkit_memory;

use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageFormat};
use std::collections::HashMap as StdHashMap;
use std::io::Cursor;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{Emitter, Manager};

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
static DECODED_IMAGE_CACHE: std::sync::LazyLock<Mutex<DecodedImageCache>> =
    std::sync::LazyLock::new(|| Mutex::new(DecodedImageCache::new(8)));

// Moved to the `stalagmight-core` member (AppState owns the shared instance,
// and members stay tauri-free): the counters and the once-per-transition
// latch live there; THIS crate keeps the actual `tile-transport-*` event
// emission, at the call site in the tile protocol handler below.
pub use stalagmight_core::tile_stats::{TileFetchStats, TileFetchStatsSnapshot};

/// HTTP request timeout for tile fetches
const TILE_FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// HTTP request timeout for tilemap API fetches
const TILEMAP_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Allowed content types for tile responses (whitelist for security)
const ALLOWED_CONTENT_TYPES: &[&str] = &["image/jpeg", "image/jpg", "image/png"];

/// Default content type when upstream doesn't provide one or it's invalid
const DEFAULT_CONTENT_TYPE: &str = "image/jpeg";

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

        let mut request = agent.get(&url);
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

                let body = response.into_body().read_to_vec().map_err(|e| {
                    if let Some(s) = stats {
                        s.record_error();
                    }
                    // The provider answered; the body just did not arrive
                    // intact. Treated as transport, because a truncated read is
                    // a connection problem rather than a decision by the origin.
                    TileFetchError::transport(format!("Failed to read response body: {e}"))
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

/// Fetch tilemap data from the provider's tilemap API.
///
/// Returns a Vec<bool> where true = tile available, false = no data.
/// The vector contains TILEMAP_GRID_SIZE × TILEMAP_GRID_SIZE (16,384) entries.
fn fetch_tilemap(
    provider: &tile_providers::TileProvider,
    z: u32,
    base_y: u32,
    base_x: u32,
) -> Result<Vec<bool>, String> {
    let url = tile_providers::expand_tilemap_url(provider, z, base_y, base_x)
        .ok_or_else(|| "Provider has no tilemap URL configured".to_string())?;

    tracing::debug!(url = %url, provider = %provider.name, z = z, base_y = base_y, base_x = base_x, "Fetching tilemap");

    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(TILEMAP_FETCH_TIMEOUT))
        .build()
        .new_agent();

    let response = agent
        .get(&url)
        .call()
        .map_err(|e| format!("Tilemap network error: {e}"))?;

    let body = response
        .into_body()
        .read_to_vec()
        .map_err(|e| format!("Failed to read tilemap response: {e}"))?;

    let tilemap_response: TilemapResponse =
        serde_json::from_slice(&body).map_err(|e| format!("Failed to parse tilemap JSON: {e}"))?;

    // Convert u8 (0/1) to bool
    let data: Vec<bool> = tilemap_response.data.into_iter().map(|v| v != 0).collect();

    let expected_len =
        (tilemap_cache::TILEMAP_GRID_SIZE * tilemap_cache::TILEMAP_GRID_SIZE) as usize;
    if data.len() != expected_len {
        return Err(format!(
            "Unexpected tilemap data length: got {}, expected {}",
            data.len(),
            expected_len
        ));
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
    tilemap_cache: &Arc<Mutex<TilemapCache>>,
    provider: &tile_providers::TileProvider,
    z: u32,
    y: u32,
    x: u32,
) -> Option<bool> {
    // Only check tilemap if provider supports it
    provider.tilemap_url_template.as_ref()?;

    // Check cache first
    {
        let mut cache = tilemap_cache.lock().ok()?;
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
    }

    // Cache miss - fetch tilemap for this region
    let (base_y, base_x) = TilemapCache::get_region_origin(y, x);

    match fetch_tilemap(provider, z, base_y, base_x) {
        Ok(data) => {
            // Cache the result
            {
                if let Ok(mut cache) = tilemap_cache.lock() {
                    cache.insert(&provider.name, z, base_y, base_x, data.clone());
                }
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
fn parse_tile_uri(uri: &http::Uri) -> Option<(String, u32, u32, u32)> {
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
fn build_error_response(status: http::StatusCode, message: &str) -> http::Response<Vec<u8>> {
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
pub(crate) enum TileServeSource {
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
    pub(crate) fn telemetry_kind(self) -> &'static str {
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
    tile_cache: &Arc<Mutex<Option<TileCache>>>,
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
    if let Ok(mut cache_guard) = tile_cache.lock() {
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
    tile_cache: &Arc<Mutex<Option<TileCache>>>,
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
    tile_cache: &Arc<Mutex<Option<TileCache>>>,
    fetch_fn: &F,
    cache_enabled: bool,
) -> TileFetchOutcome
where
    F: Fn(&str, u32, u32, u32, Option<&str>) -> Result<TileFetchResponse, TileFetchError>,
{
    // Fresh cache first (skip if per-provider caching disabled)
    let cached_tile = if cache_enabled {
        if let Ok(mut cache_guard) = tile_cache.lock() {
            if let Some(cache) = cache_guard.as_mut() {
                cache.get(provider, z, y, x)
            } else {
                None
            }
        } else {
            None
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
        if let Ok(cache_guard) = tile_cache.lock() {
            cache_guard
                .as_ref()
                .and_then(|cache| cache.get_stale(provider, z, y, x))
        } else {
            None
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
                    _ => TileFetchOutcome::Missing {
                        error: "Tile not available: provider returned 304 with no cached copy"
                            .to_string(),
                        // A protocol oddity, not a refusal - a parent may serve.
                        denied: false,
                    },
                };
            };

            if let Ok(mut cache_guard) = tile_cache.lock() {
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
                    _ => TileFetchOutcome::Missing {
                        error: "Tile not available: provider revoked storage on revalidation"
                            .to_string(),
                        // Storage was revoked, not access - a parent may serve.
                        denied: false,
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
            }
        }
    }
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
        let mut cache = DECODED_IMAGE_CACHE
            .lock()
            .map_err(|e| format!("Cache lock poisoned: {e}"))?;
        if let Some(cached) = cache.get(provider, parent_z, parent_y, parent_x) {
            cached
        } else {
            let decoded = image::load_from_memory(&parent_tile.data)
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
    tile_cache: &Arc<Mutex<Option<TileCache>>>,
    tilemap_cache: &Arc<Mutex<TilemapCache>>,
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
                .ok()
                .and_then(|guard| {
                    guard
                        .as_ref()
                        .map(|cache| cache.has_fresh_tile(provider, cz, cy, cx))
                })
                .unwrap_or(false)
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
                cache_put_best_effort(
                    tile_cache,
                    provider,
                    z,
                    y,
                    x,
                    &derived.data,
                    &derived.content_type,
                    // A derived tile is a crop of the parent, so it lives under
                    // the parent's remaining grant - overzooming cannot mint a
                    // longer life than the imagery it came from. No etag: this
                    // bytes never came off the wire, so it has no validator.
                    tile.policy,
                    None,
                    cache_enabled,
                );
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
            TileFetchOutcome::Missing { error, denied } => {
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
fn handle_tile_request(
    provider: &str,
    z: u32,
    y: u32,
    x: u32,
    tile_cache: &Arc<Mutex<Option<TileCache>>>,
    tilemap_cache: &Arc<Mutex<TilemapCache>>,
    stats: &Arc<TileFetchStats>,
    cache_config: &config::CacheConfig,
) -> (http::Response<Vec<u8>>, TileServeSource) {
    let stats_ref = Arc::clone(stats);
    // Per-request provenance for the consumption telemetry, recorded by
    // `fetch_fn` (the real network call) and left at `Cache` when it is never
    // reached. NOT what drives the offline indicator - that runs off the shared
    // `TileFetchStats` snapshot diff below, which a 304 does move, because the
    // network genuinely was reached.
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
    tile_cache: &Arc<Mutex<Option<TileCache>>>,
    tilemap_cache: &Arc<Mutex<TilemapCache>>,
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
pub(crate) fn tile_fetch_impl(
    provider: &str,
    z: u32,
    y: u32,
    x: u32,
    tile_cache: &Arc<Mutex<Option<TileCache>>>,
    tilemap_cache: &Arc<Mutex<TilemapCache>>,
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

/// How long the app-exit handler waits for a best-effort SpeleoDB lock release
/// before exiting anyway (fix M2). Long enough for a healthy round trip, short
/// enough that a hung / unreachable server can't noticeably freeze quit. It
/// bounds only the release itself: an upload or acquire still in flight is
/// waited for first (`commands::exit_wait`), never cut by this deadline. The
/// server has NO lock timeout: a release that does not land leaves the lock
/// held until the next sign-in's held-lock prompt (or the website) releases it.
pub(crate) const EXIT_LOCK_RELEASE_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Export the WebKit memory-pressure environment as the FIRST thing `run()`
    // does, while the process is still genuinely single-threaded: the `set_var`
    // calls inside are a data race once any other thread is live, and
    // `logging::init()` below spawns a `tracing_appender` writer thread. It also
    // must run before the Tauri builder: on Linux the config-defined window's
    // WebContext is constructed inside Tauri's setup step (before our own
    // `.setup()` hook), and `memory-pressure-settings` is a construct-only
    // property. No-op unless enabled (default on). The summary is logged after
    // `logging::init()` (the logger does not exist yet here). See `webkit_memory`
    // and the patched wry fork (https://github.com/Stalagmight/wry).
    #[cfg(target_os = "linux")]
    let webkit_mem_cfg = webkit_memory::apply_env_from_config();

    let _guard = logging::init();
    // Route panics through tracing + a crash-marker file so a crash oracle can
    // detect a backend panic even when a thread panic leaves the app alive.
    logging::panic_hook::install();

    // Now that logging is initialized, emit the WebKit memory summary that
    // `apply_env_from_config` computed before any subscriber existed.
    #[cfg(target_os = "linux")]
    webkit_memory::log_applied_config(&webkit_mem_cfg);

    #[cfg(not(feature = "e2e"))]
    let builder = tauri::Builder::default();
    #[cfg(feature = "e2e")]
    let builder = tauri::Builder::default().plugin(tauri_plugin_webdriver_automation::init());

    // Single instance (#424), registered before every other plugin as the
    // plugin requires. A second launch - Explorer's "Open with", a Linux
    // `.desktop` handler - is killed and its argv forwarded here, so the file
    // opens in the window the user already has. macOS is not built with the
    // plugin at all (see Cargo.toml): LaunchServices routes the second open to
    // the running instance as an Apple event, handled in the `RunEvent::Opened`
    // arm below. Off under `e2e` so the WebDriver harness can still spawn its
    // own instance while a dev build is running.
    //
    // `desktop` (not a hand-rolled `not(android)`/`not(ios)` pair) so this cfg
    // cannot drift from Cargo.toml's `[target.'cfg(...)'.dependencies]` gate on
    // the crate itself - `not(macos)` alone used to be WIDER than that gate
    // (true on Android/iOS too), so a mobile build would try to reference a
    // dependency Cargo never pulled in and fail to resolve it (#445 review).
    #[cfg(all(not(feature = "e2e"), desktop, not(target_os = "macos")))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
        // `cwd` is the SENDER's working directory - a relative argument there
        // means nothing against ours, so it is what relative paths resolve
        // against (and without it they are dropped, not guessed at).
        let cwd = std::path::PathBuf::from(cwd);
        let base = cwd.is_absolute().then_some(cwd.as_path());
        let paths = commands::launch::launch_paths_from_args(
            argv.into_iter().map(std::ffi::OsString::from),
            base,
        );
        if paths.is_empty() {
            return;
        }
        // Raise the window BEFORE queueing: whatever the frontend does with
        // this - a confirm, an open - has to happen somewhere the user can see,
        // or "Open with" on a backgrounded app reads as nothing happening.
        commands::launch::focus_main_window(app);
        commands::launch::queue_and_notify(app, paths);
    }));

    let builder = builder
        .manage(commands::launch::PendingLaunchFiles::default())
        // In-app updater state (#579): the checked `Update`, its downloaded
        // bytes and the one-shot install arm, parked between the four
        // `app_update` commands. Managed unconditionally (the commands answer
        // for themselves when the updater is off), and here rather than in
        // `.setup()` because it needs nothing from the app.
        .manage(commands::app_update::AppUpdateState::default())
        // Exit never cuts an in-flight SpeleoDB operation (lock#6): see `exit_wait`.
        .manage(commands::exit_wait::ExitWait::default())
        // One Import-from-MNemo pull at a time, and its cancel flag (#725).
        .manage(commands::mnemo::MnemoPullState::default())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .register_asynchronous_uri_scheme_protocol(
            "stalagmight-tiles",
            |ctx, request, responder| {
                let uri = request.uri().clone();
                let app_handle = ctx.app_handle().clone();
                let state = ctx.app_handle().state::<core::state::AppState>();
                let tile_cache = state.tile_cache();
                let tilemap_cache = state.tilemap_cache();
                let tile_fetch_stats = state.tile_fetch_stats();
                let tile_fetch_stats_notify = Arc::clone(&tile_fetch_stats);
                let cache_config = state.get_config().cache;

                // Use Tauri's async runtime with spawn_blocking for bounded concurrency
                // This uses a threadpool instead of spawning unbounded threads
                tauri::async_runtime::spawn(async move {
                    // Parse the tile coordinates from the URI
                    let Some((provider, z, y, x)) = parse_tile_uri(&uri) else {
                        tracing::warn!(
                            uri = %uri,
                            purpose = "handle map tile request",
                            "Invalid tile URI format; rejecting request"
                        );
                        responder.respond(build_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "Invalid tile URI format. Expected: /esri/{z}/{y}/{x} or esri/{z}/{y}/{x}",
                    ));
                        return;
                    };

                    tracing::debug!(
                        provider = %provider,
                        z = z,
                        y = y,
                        x = x,
                        purpose = "render map tiles",
                        "Processing tile request"
                    );

                    // Provider for tile-consumption telemetry (#31), kept past the fetch
                    // closure that moves `provider`.
                    let provider_for_telemetry = provider.clone();

                    // Run the blocking tile fetch in the blocking threadpool. The global
                    // stats snapshot drives the (concurrency-tolerant) connectivity
                    // notifications; `network_hit` is the per-request flag used for the
                    // exact tile-consumption telemetry below.
                    let result = tauri::async_runtime::spawn_blocking(move || {
                        let snap_before = tile_fetch_stats.snapshot();
                        let (response, serve_source) = handle_tile_request(
                            &provider,
                            z,
                            y,
                            x,
                            &tile_cache,
                            &tilemap_cache,
                            &tile_fetch_stats,
                            &cache_config,
                        );
                        let snap_after = tile_fetch_stats.snapshot();
                        let had_network_success =
                            snap_after.total_fetched > snap_before.total_fetched;
                        let had_network_error = snap_after.total_errors > snap_before.total_errors;
                        (
                            response,
                            had_network_success,
                            had_network_error,
                            serve_source,
                        )
                    })
                    .await;

                    let (response, had_network_success, had_network_error, serve_source) = result
                        .unwrap_or_else(|e| {
                            tracing::error!(
                                error = %e,
                                purpose = "render map tiles",
                                "Tile request task panicked"
                            );
                            (
                                build_error_response(
                                    http::StatusCode::INTERNAL_SERVER_ERROR,
                                    "Internal error processing tile request",
                                ),
                                false,
                                false,
                                TileServeSource::Cache,
                            )
                        });

                    // Notify connectivity changes (debounced - only emits on state
                    // transitions; the once-per-transition latch lives on
                    // `TileFetchStats` in the core member, the emission stays here
                    // with Tauri).
                    if had_network_success {
                        // Emit `tile-transport-success` once per offline→online transition.
                        if tile_fetch_stats_notify.transition_to_online() {
                            let _ = app_handle.emit("tile-transport-success", ());
                        }
                    } else if had_network_error && tile_fetch_stats_notify.transition_to_offline() {
                        // Emit `tile-transport-error` once per online→offline transition.
                        let _ = app_handle.emit("tile-transport-error", ());
                    }

                    // Tile-consumption telemetry (#31): one event per SERVED tile, tagged by
                    // provider and by where its bytes came from - `tile.fetch.<p>` for
                    // imagery pulled from the provider, `tile.revalidate.<p>` for a 304
                    // (a round trip, but the bytes came off our disk), `tile.cache.<p>`
                    // for no network call at all, stale-if-error included. Uses the
                    // PER-REQUEST source (not the shared stats diff, which concurrent
                    // requests would cross-contaminate). Errors (no tile served) aren't
                    // counted; "tiles used by a session for provider P" is the sum of the
                    // three. No-op unless telemetry is compiled in and enabled.
                    if response.status().is_success() {
                        let event = format!(
                            "tile.{}.{provider_for_telemetry}",
                            serve_source.telemetry_kind()
                        );
                        app_handle
                            .state::<telemetry::TelemetryCollector>()
                            .track(&event);
                    }

                    responder.respond(response);
                });
            },
        );

    // macOS menu bar: Tauri's default menu with a working Help submenu (#665).
    #[cfg(target_os = "macos")]
    let builder = builder
        .menu(app_menu::build)
        .on_menu_event(|app, event| app_menu::on_event(app, &event));

    // Desktop-only: intercept window close to allow frontend confirmation dialogs.
    // Android has no window close button, so this handler is dead code on mobile.
    #[cfg(desktop)]
    let builder = builder.on_window_event(|window, event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            if let Err(e) = window.emit("tauri://close-requested", ()) {
                tracing::error!(
                    error = %e,
                    purpose = "allow frontend to confirm close",
                    "Failed to emit close-requested event"
                );
                if let Err(destroy_err) = window.destroy() {
                    tracing::error!(
                        error = %destroy_err,
                        purpose = "close window after emit failure",
                        "Failed to destroy window after close-requested emit failed"
                    );
                }
            }
        }
    });

    // In-app updater (#579), registered ONLY when a minisign public key is
    // committed. This is not a preference: `tauri_plugin_updater::Config` has a
    // required `pubkey` and no `Default`, so with `plugins.updater` absent from
    // tauri.conf.json Tauri fails to deserialize the plugin's config and
    // `build()` below returns an error - the app does not start at all. The
    // block is deliberately absent while the keypair is being provisioned (see
    // `.github/workflows/release.yml`'s "Determine updater enablement", which
    // reads the same key to decide whether to build updater artifacts), so
    // registering unconditionally would brick every build until it lands.
    //
    // The plugin supplies the public key and the Windows install mode; the
    // ENDPOINT it configures is only a fallback, because `commands::app_update`
    // overrides it per call with the user's release track.
    let builder = if commands::app_update::updater_configured() {
        builder.plugin(tauri_plugin_updater::Builder::new().build())
    } else {
        tracing::info!(
            "In-app updater disabled: no plugins.updater.pubkey in tauri.conf.json (#579)"
        );
        builder
    };

    builder
        .setup(|app| {
            tracing::info!("Stalagmight starting up");

            // Resolve platform directories before anything else needs them.
            // On Android, the `directories` crate returns None; Tauri's
            // PathResolver delegates to the platform-native APIs correctly.
            commands::event_sink::init_app_dirs(app.handle());

            // Initialize app state
            let app_state = core::state::AppState::new()?;
            app.manage(app_state);

            // Set event sink on state for event emission
            let state = app.state::<core::state::AppState>();
            state.set_event_sink(commands::event_sink::tauri_sink(app.handle().clone()));

            // Initialize telemetry (no-op when feature is disabled)
            let telemetry_config = state.get_config();
            let collector =
                telemetry::TelemetryCollector::new(telemetry_config.general.telemetry_enabled);
            app.manage(collector.clone());
            // #210 (F1): give the free-function SpeleoDB client a handle to emit
            // per-endpoint traffic metrics through (it has no AppHandle to reach
            // Tauri-managed state, so it records via this write-once sink).
            speleodb::metrics::install(collector.clone());
            telemetry::start_periodic_flush(collector.clone());
            collector.send_buffered();

            // Bound the log directory once per launch (#319). `cleanup_logs`
            // existed but had no caller, so logs and crash markers grew
            // forever. Runs here because the config is only readable after
            // `AppState::new`; it sweeps the directory `logging::init()`
            // recorded, not `log_directory()` (which now resolves elsewhere -
            // see `ACTIVE_LOG_DIR`). Inline rather than spawned: the work is
            // one stat pass over a directory holding a handful of log files,
            // and a failure must never block startup.
            if let Err(err) =
                logging::cleanup_logs(state.get_config().logging.max_directory_size_mb)
            {
                tracing::warn!(error = %err, "Failed to prune the log directory");
            }

            // Launch-with-file (#424): a cold launch from Explorer or a Linux
            // `.desktop` handler puts the path in argv. Queue it now; the
            // frontend drains the queue once its stores are up. macOS passes
            // nothing here (Finder sends an Apple event instead - see the
            // `RunEvent::Opened` arm below), so this is simply empty there.
            // `args_os`, not `args`: the latter PANICS on a non-Unicode
            // argument, which would make a latin-1 filename crash the launch.
            let launch_cwd = std::env::current_dir().ok();
            commands::launch::queue_and_notify(
                app.handle(),
                commands::launch::launch_paths_from_args(
                    std::env::args_os(),
                    launch_cwd.as_deref(),
                ),
            );

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Cache commands
            commands::cache::get_cache_stats,
            commands::cache::clear_tile_cache,
            commands::cache::get_network_status,
            commands::cache::get_tile_fetch_stats,
            commands::cache::fetch_tile,
            commands::cache::get_tile_providers,
            commands::cache::get_tile_attribution,
            // Gas planning (#38)
            commands::gas::compute_gas_reach_field,
            // Dive-plan persistence (#244, .plnx sibling)
            commands::dive_plan::list_dive_plans,
            commands::dive_plan::load_dive_plan,
            commands::dive_plan::save_dive_plan,
            commands::dive_plan::delete_dive_plan,
            commands::dive_plan::plan_anchors_for_refs,
            commands::dive_plan::plan_refs_for_anchors,
            // Project commands
            commands::project::update_project_settings,
            commands::project::set_surface_elevation,
            commands::project::save_project,
            commands::project::close_project,
            commands::project::speleodb_discard_cached_copy,
            commands::project::take_unrecognised_shot_kinds,
            commands::project::history_discarded,
            commands::project::get_project_state,
            commands::project::get_adjusted_stations,
            commands::project::get_adjustment_report,
            commands::project::get_station_distances_from,
            commands::project::get_along_line_distance,
            commands::project::list_recent_projects,
            commands::project::remove_from_recent_projects,
            commands::project::delete_project,
            commands::project::set_project_name_for_path,
            commands::project::merge_projects,
            commands::project::load_project_full,
            commands::project::load_project_full_binary,
            commands::project::create_project_full,
            commands::project::cancel_loading,
            // SpeleoDB commands (read-only spike, #183)
            commands::speleodb::speleodb_login,
            commands::speleodb::speleodb_list_projects,
            commands::speleodb::speleodb_open_project,
            commands::speleodb::speleodb_open_project_discarding_local,
            commands::speleodb::speleodb_logout,
            commands::speleodb::speleodb_session,
            // SpeleoDB linked-project status for the action shell (#204)
            commands::speleodb::speleodb_link_status,
            commands::speleodb::speleodb_unlink,
            commands::speleodb::speleodb_remove_link_at,
            // SpeleoDB single-writer hand-off (#194/#197)
            commands::speleodb::speleodb_start_editing,
            commands::speleodb::speleodb_done,
            commands::speleodb::speleodb_update,
            commands::speleodb::speleodb_push,
            // SpeleoDB divergence resolution - take remote (#198)
            commands::speleodb::speleodb_take_remote,
            // SpeleoDB held-lock resume/release prompt - release all + per-project (#201)
            commands::speleodb::speleodb_release_lock,
            commands::speleodb::speleodb_release_lock_acquired_here,
            // SpeleoDB per-project LOCAL state (cached copy + un-pushed edits), the
            // local half of the held-lock / GlobalView project joins
            commands::speleodb::speleodb_local_project_states,
            // Cached SpeleoDB copies on the home screen: owner-aware listing,
            // removing one copy, and Settings' "Purge all cached files"
            commands::speleodb_cache::speleodb_cached_projects,
            commands::speleodb_cache::speleodb_remove_cached_copy,
            commands::speleodb_cache::speleodb_purge_cached_copies,
            // SpeleoDB revision-history read + restore (#218)
            commands::speleodb::speleodb_get_revisions,
            commands::speleodb::speleodb_restore_revision,
            // SpeleoDB hybrid undo stack: discard the local copy (#303)
            commands::speleodb::speleodb_discard_changes,
            // SpeleoDB create-on-SDB publish (#207)
            commands::speleodb::speleodb_project_collaborators,
            commands::speleodb::speleodb_publish,
            // SpeleoDB project settings: release the open project lock
            commands::speleodb::speleodb_release_current_lock,
            // A project switch / quit hands the open project's lock back first
            commands::speleodb::speleodb_settle_open_lock,
            commands::speleodb::speleodb_project_access_at,
            // Section commands
            commands::section::create_section,
            commands::section::update_section,
            commands::section::delete_section,
            commands::section::reverse_section,
            commands::section::move_section,
            commands::section::move_segment,
            commands::section::execute_batch,
            commands::section::get_rigid_connected_section_ids,
            commands::section::get_anchored_section_ids,
            commands::section::create_section_and_merge,
            commands::section::get_section_relationships,
            commands::section::combine_candidate,
            commands::section::combine_candidate_for_sections,
            commands::section::combine_sections,
            commands::section::get_segment_geometry,
            // Shot commands
            commands::shot::create_shot,
            commands::shot::update_shot,
            commands::shot::preview_delete_shot,
            commands::shot::delete_shot,
            commands::shot::set_shot_kind,
            commands::shot::edit_shot_credit,
            commands::shot::split_shot_at_distance,
            commands::shot::delete_shot_and_split,
            commands::shot::split_section,
            commands::shot::split_section_preview,
            commands::shot::delete_last_shot_and_section,
            commands::shot::preview_delete_shot_range,
            commands::shot::delete_shot_range_split,
            commands::shot::delete_shot_range_shift,
            // Station commands
            commands::station::merge_stations,
            commands::station::unmerge_stations,
            commands::station::unmerge_station_pair,
            commands::station::set_zero_datum,
            commands::station::remove_zero_datum,
            commands::station::set_station_comment,
            commands::station::move_comment,
            commands::station::move_station,
            commands::station::check_merge_connectivity,
            commands::station::create_loop_closure,
            commands::station::remove_loop_closure,
            // Undo/Redo
            commands::history::undo,
            commands::history::redo,
            commands::history::can_undo,
            commands::history::can_redo,
            commands::history::get_undo_stack,
            commands::history::get_redo_stack,
            commands::history::undo_to,
            commands::history::redo_to,
            // Import/Export
            commands::import::import_dmp,
            commands::mnemo::list_mnemo_ports,
            commands::mnemo::pull_mnemo,
            commands::mnemo::cancel_mnemo_pull,
            commands::import::calculate_file_hash,
            commands::import::check_duplicate_import,
            commands::import::track_imported_file,
            commands::import::check_section_hashes,
            commands::import::import_sections,
            commands::import::add_project_file,
            commands::import::parse_tml,
            commands::import::execute_tml_import,
            commands::import::parse_compass,
            commands::import::execute_compass_import,
            commands::import::parse_walls,
            commands::import::execute_walls_import,
            commands::import::parse_gpx,
            commands::import::execute_gpx_import,
            commands::import::suggest_import_placement,
            commands::import::suggest_all_import_placements,
            commands::import::suggest_orphan_placements,
            commands::import::count_unanchored_sections,
            commands::export::export_csv,
            commands::export::export_json,
            commands::export::export_tml,
            commands::export::export_image,
            commands::export::preview_svg_export,
            commands::export::export_value_histogram,
            commands::export::export_dive_profile,
            commands::export::render_dive_profile_text,
            // Config
            commands::config::get_config,
            commands::config::update_config,
            commands::config::reload_config,
            commands::config::update_panel_sizes,
            commands::config::update_panel_layout,
            commands::config::update_data_table_view,
            commands::config::update_new_section_view,
            commands::config::update_edit_section_view,
            commands::config::update_import_shots_view,
            commands::config::update_gas_config,
            commands::config::update_map_palette,
            commands::config::remember_people,
            // Display settings (per-project)
            commands::config::load_display_settings,
            commands::config::save_display_setting,
            // Raw table browsing (data table fullscreen)
            commands::raw_data::get_raw_table_names,
            commands::raw_data::get_raw_table,
            // Validation
            commands::validation::get_project_warnings,
            commands::validation::check_project_integrity,
            commands::validation::repair_project_integrity,
            commands::validation::resolve_warning,
            commands::validation::unresolve_warning,
            // Survey geometry
            commands::survey_geometry::get_survey_geometry_for_file,
            // OS file association / launch-with-file (#424)
            commands::launch::take_pending_launch_files,
            // In-app updater (#579): check / download / arm / install are
            // separate steps so the install cannot run before the exit guards
            // do - the arm is the backend's own proof that it did.
            commands::app_update::check_for_app_update,
            commands::app_update::download_app_update,
            // Turning "Update automatically" off abandons the automatic
            // download (#645).
            commands::app_update::discard_app_update_download,
            commands::app_update::arm_app_update_restart,
            commands::app_update::unarm_app_update_restart,
            commands::exit_wait::arm_forced_exit,
            commands::exit_wait::disarm_forced_exit,
            commands::app_update::install_app_update_and_restart,
            // Recovery (SPEC 14.4)
            commands::recovery::attempt_repair,
            commands::recovery::list_backups,
            commands::recovery::restore_from_backup,
            commands::recovery::export_salvageable_data,
            // Logging
            commands::logging::get_recent_logs,
            commands::logging::get_recent_logs_since,
            commands::logging::push_frontend_log,
            commands::debug::debug_trigger_panic,
            // Memory probe (debug)
            commands::memory::get_memory_stats,
            // Telemetry
            commands::telemetry::track_events,
            commands::telemetry::flush_telemetry,
            commands::telemetry::is_telemetry_available,
            commands::telemetry::set_telemetry_enabled,
        ])
        .build(tauri::generate_context!())
        .map(|app| {
            app.run(|app_handle, event| {
                // Best-effort release of a held SpeleoDB write lock on app exit so
                // quitting / closing the window doesn't strand the server mutex
                // locked for every other editor (fix 1). This covers the quit +
                // window-close paths that destroy the window WITHOUT routing through
                // `close_project` (which releases on its own). No-op when no remote
                // edit session is active. Bounded (fix M2): this closure runs on the
                // main event-loop thread, so a synchronous release against a hung /
                // unreachable server would freeze quit for the full HTTP timeout - cap
                // the release and exit anyway. An upload or acquire still in flight
                // is NOT cut (lock#6): the exit is held, the operation is waited for
                // off the main thread, and only then does the release run and the
                // app exit - unless the user chose "Quit now" in-app.
                if let tauri::RunEvent::ExitRequested { code, api, .. } = &event {
                    commands::exit_wait::on_exit_requested(app_handle, *code, api);
                }

                // Install on quit (#631). `Exit`, not `ExitRequested`: the latter
                // can still be prevented, and a cancelled exit must not have
                // replaced the app underneath the running process.
                if let tauri::RunEvent::Exit = event {
                    commands::app_update::install_downloaded_update_on_exit(app_handle);
                }

                // macOS "open with" (#424). Finder never uses argv: both a cold
                // launch and a double-click while the app runs arrive as an
                // Apple event, which Tauri surfaces here. On a cold launch this
                // fires long before the webview exists, so the paths are queued
                // rather than pushed - the frontend drains them when ready.
                // `focus_main_window` mirrors the single-instance path above (a
                // no-op with no webview yet, so the cold-launch case above is
                // unaffected): without it, a double-click on a backgrounded app
                // queues the file and raises the confirm in a window the user
                // cannot see - the exact failure raising it exists to prevent
                // (#445 review).
                #[cfg(target_os = "macos")]
                if let tauri::RunEvent::Opened { urls } = &event {
                    commands::launch::focus_main_window(app_handle);
                    commands::launch::queue_and_notify(
                        app_handle,
                        commands::launch::launch_paths_from_urls(urls),
                    );
                }
            });
        })
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "Fatal: Tauri application failed to start");
        });
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

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

    fn make_test_cache() -> Arc<Mutex<Option<TileCache>>> {
        let cache = TileCache::in_memory(10, 30).unwrap();
        Arc::new(Mutex::new(Some(cache)))
    }

    fn make_test_tilemap_cache() -> Arc<Mutex<TilemapCache>> {
        Arc::new(Mutex::new(TilemapCache::new()))
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
            let mut guard = cache.lock().unwrap();
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
            let mut guard = cache.lock().unwrap();
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
            let mut tc = tilemap_cache.lock().unwrap();
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
            let mut guard = cache.lock().unwrap();
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
        let cache = Arc::new(Mutex::new(None));
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
        let cache = Arc::new(Mutex::new(None));
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
        let cache = Arc::new(Mutex::new(Some(TileCache::in_memory(1, 30).unwrap())));
        let tilemap_cache = make_test_tilemap_cache();

        {
            let mut guard = cache.lock().unwrap();
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
            let mut guard = cache.lock().unwrap();
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
            let mut guard = cache.lock().unwrap();
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
        let cache = Arc::new(Mutex::new(Some(TileCache::in_memory(10, 30).unwrap())));
        let tilemap_cache = make_test_tilemap_cache();
        {
            let mut guard = cache.lock().unwrap();
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
        let cache = Arc::new(Mutex::new(Some(TileCache::in_memory(10, 30).unwrap())));
        let tilemap_cache = make_test_tilemap_cache();
        {
            let mut guard = cache.lock().unwrap();
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

        let response = handle_tile_request_with_fetcher(
            "esri",
            1,
            2,
            3,
            &cache,
            &tilemap_cache,
            offline,
            true,
        );
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
        let cache = Arc::new(Mutex::new(Some(TileCache::in_memory(10, 30).unwrap())));
        let tilemap_cache = make_test_tilemap_cache();
        {
            let mut guard = cache.lock().unwrap();
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

        let response = handle_tile_request_with_fetcher(
            "esri",
            10,
            500,
            250,
            &cache,
            &tilemap_cache,
            gone,
            true,
        );
        assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
    }

    #[test]
    fn a_denied_tile_is_not_served_as_an_overzoomed_parent() {
        // Refusing to answer a 403 off the stale cache is only half a guard: the
        // overzoom loop treated "missing" as permission to climb, so a cached
        // parent still produced an upscaled crop of the very area the provider
        // had just denied.
        let cache = Arc::new(Mutex::new(Some(TileCache::in_memory(10, 30).unwrap())));
        let tilemap_cache = make_test_tilemap_cache();
        {
            // A parent one zoom up, fresh, ready to be cropped.
            let mut guard = cache.lock().unwrap();
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
        let cache = Arc::new(Mutex::new(Some(TileCache::in_memory(10, 30).unwrap())));
        let tilemap_cache = make_test_tilemap_cache();
        {
            let mut guard = cache.lock().unwrap();
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

    #[test]
    fn a_no_store_revalidation_does_not_serve_the_copy_it_just_deleted() {
        // A 304 carrying `no-store` revokes the grant the copy was held under.
        // `refresh_validated` deletes the row; serving those bytes anyway would
        // hand out a copy in the same breath as dropping it. We ask again
        // without a validator and serve what comes back - which the gate also
        // refuses to store, so nothing is retained either way.
        let cache = Arc::new(Mutex::new(Some(TileCache::in_memory(10, 30).unwrap())));
        let tilemap_cache = make_test_tilemap_cache();
        {
            let mut guard = cache.lock().unwrap();
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
        let guard = cache.lock().unwrap();
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
        let cache = Arc::new(Mutex::new(Some(TileCache::in_memory(10, 30).unwrap())));
        let tilemap_cache = make_test_tilemap_cache();
        {
            let mut guard = cache.lock().unwrap();
            guard
                .as_mut()
                .unwrap()
                .put_granted("esri", 10, 500, 250, b"cached", "image/png")
                .unwrap();
        }
        assert!(
            tilemap_cache
                .lock()
                .unwrap()
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
            .unwrap()
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
            let cache = Arc::new(Mutex::new(Some(TileCache::in_memory(10, 30).unwrap())));
            let tilemap_cache = make_test_tilemap_cache();
            {
                let mut guard = cache.lock().unwrap();
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
            let cache = Arc::new(Mutex::new(Some(TileCache::in_memory(10, 30).unwrap())));
            let tilemap_cache = make_test_tilemap_cache();
            {
                let mut guard = cache.lock().unwrap();
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

            let mock_fetch = move |_: &str,
                                   _: u32,
                                   _: u32,
                                   _: u32,
                                   _: Option<&str>|
                  -> Result<TileFetchResponse, TileFetchError> {
                Err(outage.clone())
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
            assert_eq!(response.status(), http::StatusCode::OK);
            assert_eq!(response.body(), b"stored");
        }
    }

    #[test]
    fn test_cache_disabled_no_stale_fallback() {
        // Use a cache with 0 TTL so tiles are immediately stale
        let cache = Arc::new(Mutex::new(Some(TileCache::in_memory(10, 0).unwrap())));
        let tilemap_cache = make_test_tilemap_cache();

        // Pre-populate cache with a tile
        {
            let mut guard = cache.lock().unwrap();
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
            let mut tc = tilemap_cache.lock().unwrap();
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
            let mut guard = cache.lock().unwrap();
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
        let mut guard = cache.lock().unwrap();
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
}
