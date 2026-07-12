pub mod app_dirs;
pub mod cache;
pub mod commands;
pub mod config;
pub mod core;
pub mod export;
pub mod logging;
pub mod models;
pub mod parsers;
pub mod persistence;
pub mod speleodb;
pub mod telemetry;
#[cfg(test)]
pub mod test_helpers;
pub mod tile_providers;
pub mod tilemap_cache;

use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageFormat};
use std::collections::HashMap as StdHashMap;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{Emitter, Manager};

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

/// Lock-free tile fetch statistics using atomic counters.
///
/// Shared across the protocol handler and IPC commands via `Arc`.
/// All methods use `Relaxed` ordering since we only need approximate
/// monotonic counters, not cross-field consistency.
pub struct TileFetchStats {
    total_fetched: AtomicU64,
    total_errors: AtomicU64,
    /// Cumulative sum of all successful fetch latencies (ms)
    total_latency_ms: AtomicU64,
    last_latency_ms: AtomicU64,
    in_flight: AtomicU64,
    /// true = we've notified the frontend that tiles are failing
    notified_offline: AtomicBool,
}

/// Snapshot of tile fetch statistics at a point in time.
pub struct TileFetchStatsSnapshot {
    pub in_flight: u64,
    pub total_fetched: u64,
    pub total_errors: u64,
    pub total_latency_ms: u64,
    pub last_latency_ms: u64,
}

impl Default for TileFetchStats {
    fn default() -> Self {
        Self::new()
    }
}

impl TileFetchStats {
    pub fn new() -> Self {
        Self {
            total_fetched: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
            total_latency_ms: AtomicU64::new(0),
            last_latency_ms: AtomicU64::new(0),
            in_flight: AtomicU64::new(0),
            notified_offline: AtomicBool::new(false),
        }
    }

    /// Call before starting a network fetch.
    pub fn record_start(&self) {
        self.in_flight.fetch_add(1, Ordering::Relaxed);
    }

    /// Call after a successful network fetch.
    pub fn record_success(&self, latency_ms: u64) {
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
        self.total_fetched.fetch_add(1, Ordering::Relaxed);
        self.total_latency_ms
            .fetch_add(latency_ms, Ordering::Relaxed);
        self.last_latency_ms.store(latency_ms, Ordering::Relaxed);
    }

    /// Call after a failed network fetch (all retries exhausted).
    pub fn record_error(&self) {
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
        self.total_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Take a snapshot of the current counters.
    pub fn snapshot(&self) -> TileFetchStatsSnapshot {
        TileFetchStatsSnapshot {
            in_flight: self.in_flight.load(Ordering::Relaxed),
            total_fetched: self.total_fetched.load(Ordering::Relaxed),
            total_errors: self.total_errors.load(Ordering::Relaxed),
            total_latency_ms: self.total_latency_ms.load(Ordering::Relaxed),
            last_latency_ms: self.last_latency_ms.load(Ordering::Relaxed),
        }
    }

    /// Emit `tile-transport-error` once per online→offline transition.
    pub fn notify_transport_error<R: tauri::Runtime>(&self, handle: &tauri::AppHandle<R>) {
        if self
            .notified_offline
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let _ = handle.emit("tile-transport-error", ());
        }
    }

    /// Emit `tile-transport-success` once per offline→online transition.
    pub fn notify_transport_success<R: tauri::Runtime>(&self, handle: &tauri::AppHandle<R>) {
        if self
            .notified_offline
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let _ = handle.emit("tile-transport-success", ());
        }
    }
}

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
fn is_retryable_error(err: &ureq::Error) -> bool {
    match err {
        ureq::Error::StatusCode(_) => false, // Server responded — don't retry
        _ => true, // Transport errors (DNS, timeout, connection reset, etc.)
    }
}

/// Fetches a tile from the network based on provider.
///
/// If `stats` is provided, records in-flight / success / error metrics.
/// Retries up to `TILE_FETCH_MAX_RETRIES` times on transient transport errors
/// with exponential backoff (200ms, 500ms).
fn fetch_tile_from_network(
    provider: &str,
    z: u32,
    y: u32,
    x: u32,
    stats: Option<&TileFetchStats>,
) -> Result<FetchedTile, String> {
    // Look up provider in registry
    let tile_provider = tile_providers::get_provider(provider)
        .ok_or_else(|| format!("Unknown provider: {provider}"))?;
    let url = tile_providers::expand_url(tile_provider, z, y, x);

    tracing::debug!(url = %url, provider = %provider, "Fetching tile from network");

    if let Some(s) = stats {
        s.record_start();
    }

    let start = std::time::Instant::now();
    let mut last_err: Option<String> = None;

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

        match agent.get(&url).call() {
            Ok(response) => {
                let latency_ms = start.elapsed().as_millis() as u64;

                // Sanitize content type - only allow whitelisted image types
                let raw_content_type = response
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or(DEFAULT_CONTENT_TYPE);

                let content_type = sanitize_content_type(raw_content_type);

                let body = response.into_body().read_to_vec().map_err(|e| {
                    if let Some(s) = stats {
                        s.record_error();
                    }
                    format!("Failed to read response body: {e}")
                })?;

                if let Some(s) = stats {
                    s.record_success(latency_ms);
                }

                return Ok(FetchedTile {
                    data: body,
                    content_type,
                });
            }
            Err(e) => {
                if attempt < TILE_FETCH_MAX_RETRIES && is_retryable_error(&e) {
                    tracing::debug!(
                        error = %e,
                        attempt = attempt + 1,
                        provider = %provider,
                        "Tile fetch failed, will retry"
                    );
                    last_err = Some(format!("Network error: {e}"));
                    continue;
                }

                // Final attempt or non-retryable error
                if let Some(s) = stats {
                    s.record_error();
                }
                return Err(format!("Network error: {e}"));
            }
        }
    }

    // Should not be reached, but just in case
    if let Some(s) = stats {
        s.record_error();
    }
    Err(last_err.unwrap_or_else(|| "Tile fetch failed".to_string()))
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
}

#[derive(Debug)]
struct FetchedTile {
    data: Vec<u8>,
    content_type: String,
}

enum TileFetchOutcome {
    Tile { tile: TileFetchResult },
    Missing { error: String },
}

fn cache_put_best_effort(
    tile_cache: &Arc<Mutex<Option<TileCache>>>,
    provider: &str,
    z: u32,
    y: u32,
    x: u32,
    data: &[u8],
    content_type: &str,
    cache_enabled: bool,
) {
    if !cache_enabled {
        return;
    }
    if let Ok(mut cache_guard) = tile_cache.lock() {
        if let Some(cache) = cache_guard.as_mut() {
            if let Err(e) = cache.put(provider, z, y, x, data, content_type) {
                tracing::warn!(error = %e, "Failed to cache tile");
            }
        }
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
    F: Fn(&str, u32, u32, u32) -> Result<FetchedTile, String>,
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
                data: entry.data,
                content_type: entry.content_type,
            },
        };
    }

    // Cache miss - fetch from network
    match fetch_fn(provider, z, y, x) {
        Ok(fetched) => {
            let FetchedTile { data, content_type } = fetched;
            cache_put_best_effort(
                tile_cache,
                provider,
                z,
                y,
                x,
                &data,
                &content_type,
                cache_enabled,
            );
            TileFetchOutcome::Tile {
                tile: TileFetchResult { data, content_type },
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, provider = %provider, z = z, y = y, x = x, "Network fetch failed");

            // Try stale cached tile as offline fallback (skip if per-provider caching disabled)
            let stale_tile = if cache_enabled {
                if let Ok(cache_guard) = tile_cache.lock() {
                    if let Some(cache) = cache_guard.as_ref() {
                        cache.get_stale(provider, z, y, x)
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(entry) = stale_tile {
                tracing::info!(
                    provider = %provider,
                    z = z,
                    y = y,
                    x = x,
                    purpose = "keep map usable offline",
                    "Serving stale cached tile as offline fallback"
                );
                return TileFetchOutcome::Tile {
                    tile: TileFetchResult {
                        data: entry.data,
                        content_type: entry.content_type,
                    },
                };
            }

            TileFetchOutcome::Missing {
                error: format!("Tile not available: {e}"),
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
        });
    }

    let delta = target_z - parent_z;
    let scale = 1u32
        .checked_shl(delta)
        .ok_or_else(|| "Overzoom scale overflow".to_string())?;

    // Use decoded image cache to avoid redundant JPEG decoding.
    // When overzooming 2 levels, 16 child tiles share the same parent — caching
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
    F: Fn(&str, u32, u32, u32) -> Result<FetchedTile, String>,
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

    // Check tilemap for tile availability before entering the fetch loop.
    // If tilemap says tile is unavailable, skip directly to parent.
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

    loop {
        // For tiles at current_z < z, we can also check tilemap before fetching
        if current_z < z {
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
            TileFetchOutcome::Missing { error } => {
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
) -> (http::Response<Vec<u8>>, bool) {
    let stats_ref = Arc::clone(stats);
    // Per-request network-hit flag, set when fetch_fn (the real network fetch) returns
    // Ok for this request. A cache hit / stale fallback never sets it.
    let network_hit = Arc::new(AtomicBool::new(false));
    let network_hit_setter = Arc::clone(&network_hit);
    let fetch_fn = move |p: &str, z: u32, y: u32, x: u32| -> Result<FetchedTile, String> {
        let result = fetch_tile_from_network(p, z, y, x, Some(&stats_ref));
        if result.is_ok() {
            network_hit_setter.store(true, Ordering::Relaxed);
        }
        result
    };
    let cache_enabled = cache_config.is_provider_cache_enabled(provider);
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
    (response, network_hit.load(Ordering::Relaxed))
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
    F: Fn(&str, u32, u32, u32) -> Result<FetchedTile, String>,
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
    tile_cache: &Arc<Mutex<Option<TileCache>>>,
    tilemap_cache: &Arc<Mutex<TilemapCache>>,
    stats: &Arc<TileFetchStats>,
    cache_config: &config::CacheConfig,
) -> Result<Vec<u8>, String> {
    // Validate provider against registry
    if tile_providers::get_provider(provider).is_none() {
        return Err(format!("Unknown provider: {provider}"));
    }

    let cache_enabled = cache_config.is_provider_cache_enabled(provider);
    let stats_ref = Arc::clone(stats);
    let fetch_fn = move |p: &str, z: u32, y: u32, x: u32| -> Result<FetchedTile, String> {
        fetch_tile_from_network(p, z, y, x, Some(&stats_ref))
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
    .map(|tile| tile.data)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// How long the app-exit handler waits for a best-effort SpeleoDB lock release
/// before exiting anyway (fix M2). Long enough for a healthy round trip, short
/// enough that a hung / unreachable server can't noticeably freeze quit; the
/// server-side mutex times out on its own if the release doesn't land.
const EXIT_LOCK_RELEASE_TIMEOUT: Duration = Duration::from_secs(5);

pub fn run() {
    let _guard = logging::init();
    // Route panics through tracing + a crash-marker file so a crash oracle can
    // detect a backend panic even when a thread panic leaves the app alive.
    logging::panic_hook::install();

    #[cfg(not(feature = "e2e"))]
    let builder = tauri::Builder::default();
    #[cfg(feature = "e2e")]
    let builder = tauri::Builder::default().plugin(tauri_plugin_webdriver_automation::init());
    let builder = builder
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
                        let (response, network_hit) = handle_tile_request(
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
                            network_hit,
                        )
                    })
                    .await;

                    let (response, had_network_success, had_network_error, network_hit) = result
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
                                false,
                            )
                        });

                    // Notify connectivity changes (debounced — only emits on state transitions)
                    if had_network_success {
                        tile_fetch_stats_notify.notify_transport_success(&app_handle);
                    } else if had_network_error {
                        tile_fetch_stats_notify.notify_transport_error(&app_handle);
                    }

                    // Tile-consumption telemetry (#31): one event per SERVED tile, tagged by
                    // provider — `tile.fetch.<provider>` for a fresh network pull (provider
                    // consumption) vs `tile.cache.<provider>` for one served from the local
                    // cache (incl. the stale offline fallback). Uses the PER-REQUEST
                    // `network_hit` (not the shared stats diff, which concurrent requests
                    // would cross-contaminate). Errors (no tile served) aren't counted;
                    // "tiles used by a session for provider P" = fetch + cache. No-op unless
                    // telemetry is compiled in and enabled.
                    if response.status().is_success() {
                        let event = if network_hit {
                            format!("tile.fetch.{provider_for_telemetry}")
                        } else {
                            format!("tile.cache.{provider_for_telemetry}")
                        };
                        app_handle
                            .state::<telemetry::TelemetryCollector>()
                            .track(&event);
                    }

                    responder.respond(response);
                });
            },
        );

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

    builder
        .setup(|app| {
            tracing::info!("Stalagmight starting up");

            // Resolve platform directories before anything else needs them.
            // On Android, the `directories` crate returns None; Tauri's
            // PathResolver delegates to the platform-native APIs correctly.
            app_dirs::init(app.handle());

            // Initialize app state
            let app_state = core::state::AppState::new()?;
            app.manage(app_state);

            // Set app handle on state for event emission
            let state = app.state::<core::state::AppState>();
            state.set_app_handle(app.handle().clone());

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
            // Gas planning (#38)
            commands::gas::compute_gas_reach_field,
            // Dive-plan persistence (#244, .plnx sibling)
            commands::dive_plan::list_dive_plans,
            commands::dive_plan::load_dive_plan,
            commands::dive_plan::save_dive_plan,
            commands::dive_plan::delete_dive_plan,
            // Project commands
            commands::project::update_project_settings,
            commands::project::save_project,
            commands::project::close_project,
            commands::project::get_project_state,
            commands::project::get_adjusted_stations,
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
            commands::speleodb::speleodb_adopt_ariane,
            commands::speleodb::speleodb_logout,
            commands::speleodb::speleodb_session,
            // SpeleoDB linked-project status for the action shell (#204)
            commands::speleodb::speleodb_link_status,
            commands::speleodb::speleodb_unlink,
            // SpeleoDB single-writer hand-off (#194/#197)
            commands::speleodb::speleodb_start_editing,
            commands::speleodb::speleodb_force_edit,
            commands::speleodb::speleodb_done,
            commands::speleodb::speleodb_update,
            commands::speleodb::speleodb_push,
            // SpeleoDB divergence resolution - take remote (#198)
            commands::speleodb::speleodb_take_remote,
            // SpeleoDB held-lock resume/release prompt - release all + per-project (#201)
            commands::speleodb::speleodb_release_all_locks,
            commands::speleodb::speleodb_release_lock,
            // SpeleoDB revision-history read + restore (#218)
            commands::speleodb::speleodb_get_revisions,
            commands::speleodb::speleodb_restore_revision,
            // SpeleoDB Enable-rendering PATCH (#200)
            commands::speleodb::speleodb_set_exclude_geojson,
            // SpeleoDB Project Settings metadata (#206) + create-on-SDB publish (#207)
            commands::speleodb::speleodb_get_project_metadata,
            commands::speleodb::speleodb_patch_project,
            commands::speleodb::speleodb_publish,
            // SpeleoDB full Project Settings: collaborators + lock release + delete (#215)
            commands::speleodb::speleodb_list_collaborators,
            commands::speleodb::speleodb_add_collaborator,
            commands::speleodb::speleodb_set_collaborator,
            commands::speleodb::speleodb_remove_collaborator,
            commands::speleodb::speleodb_add_team,
            commands::speleodb::speleodb_set_team,
            commands::speleodb::speleodb_remove_team,
            commands::speleodb::speleodb_list_teams,
            commands::speleodb::speleodb_search_users,
            commands::speleodb::speleodb_release_current_lock,
            commands::speleodb::speleodb_delete_project,
            // Section commands
            commands::section::create_section,
            commands::section::update_section,
            commands::section::delete_section,
            commands::section::move_section,
            commands::section::move_segment,
            commands::section::execute_batch,
            commands::section::get_rigid_connected_section_ids,
            commands::section::get_anchored_section_ids,
            commands::section::create_section_and_merge,
            commands::section::get_section_relationships,
            commands::section::get_segment_geometry,
            // Shot commands
            commands::shot::create_shot,
            commands::shot::update_shot,
            commands::shot::preview_delete_shot,
            commands::shot::delete_shot,
            commands::shot::set_shot_reference,
            commands::shot::split_shot_at_distance,
            commands::shot::delete_shot_and_split,
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
            // Marker commands
            commands::marker::add_marker,
            commands::marker::update_marker,
            commands::marker::delete_marker,
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
            commands::import::calculate_file_hash,
            commands::import::check_duplicate_import,
            commands::import::track_imported_file,
            commands::import::check_section_hashes,
            commands::import::import_sections,
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
            commands::export::export_csv,
            commands::export::export_json,
            commands::export::export_tml,
            commands::export::export_image,
            commands::export::preview_svg_export,
            // Collaboration mode
            commands::collab::set_collab_enabled,
            commands::collab::set_collab_enabled_for_path,
            commands::collab::get_collab_state,
            commands::collab::detect_collab_changes,
            commands::collab::apply_collab_changes,
            commands::collab::collab_sibling_when_disabled,
            // Config
            commands::config::get_config,
            commands::config::update_config,
            commands::config::reload_config,
            commands::config::update_panel_sizes,
            commands::config::update_panel_layout,
            commands::config::update_data_table_view,
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
                // the wait and exit anyway (the server lock times out on its own).
                if let tauri::RunEvent::ExitRequested { .. } = event {
                    if let Some(state) = app_handle.try_state::<core::state::AppState>() {
                        state.speleodb_release_held_lock_bounded(EXIT_LOCK_RELEASE_TIMEOUT);
                    }
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
    fn test_handle_tile_request_cache_hit_returns_200() {
        let cache = make_test_cache();
        let tilemap_cache = make_test_tilemap_cache();
        let stats = make_test_stats();

        // Pre-populate cache
        {
            let mut guard = cache.lock().unwrap();
            let c = guard.as_mut().unwrap();
            c.put("esri", 10, 500, 250, b"fake_tile_data", "image/jpeg")
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
            c.put("esri", 10, 500, 250, b"png_data", "image/png")
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
        assert_eq!(response.headers().get("content-type").unwrap(), "image/png");
        assert_cors_header(&response);
    }

    #[test]
    fn test_handle_tile_request_cache_miss_network_fails_returns_404() {
        // With no cached tile and network failing, should return 404
        let cache = make_test_cache();
        let tilemap_cache = make_test_tilemap_cache();

        // Mock fetch that always fails
        let mock_fetch = |_: &str, _: u32, _: u32, _: u32| -> Result<FetchedTile, String> {
            Err("Network unavailable".to_string())
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
        let mock_fetch = |_: &str, _: u32, _: u32, _: u32| -> Result<FetchedTile, String> {
            Ok(FetchedTile {
                data: b"fetched_tile_data".to_vec(),
                content_type: "image/jpeg".to_string(),
            })
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
        // Verify that a successfully fetched tile is stored in cache
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

        let mock_fetch = |_: &str, _: u32, _: u32, _: u32| -> Result<FetchedTile, String> {
            Ok(FetchedTile {
                data: b"fetched_tile".to_vec(),
                content_type: "image/jpeg".to_string(),
            })
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

        let mock_fetch = |_: &str, _: u32, _: u32, _: u32| -> Result<FetchedTile, String> {
            Ok(FetchedTile {
                data: b"tile_data".to_vec(),
                content_type: "image/jpeg".to_string(),
            })
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

        let mock_fetch = |_: &str, _: u32, _: u32, _: u32| -> Result<FetchedTile, String> {
            Err("Connection refused".to_string())
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
        let cache = Arc::new(Mutex::new(Some(TileCache::in_memory(1, 0).unwrap())));
        let tilemap_cache = make_test_tilemap_cache();

        {
            let mut guard = cache.lock().unwrap();
            guard
                .as_mut()
                .unwrap()
                .put("esri", 1, 2, 3, b"stale_tile", "image/png")
                .unwrap();
        }

        let mock_fetch = |_: &str, _: u32, _: u32, _: u32| -> Result<FetchedTile, String> {
            Err("Network down".to_string())
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
            c.put("esri", 1, 2, 3, b"tile_bytes", "image/png").unwrap();
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
        assert_eq!(data, b"tile_bytes");
    }

    // === Per-provider cache toggle tests ===

    #[test]
    fn test_provider_cache_disabled_skips_cache_read_and_write() {
        use std::collections::HashMap;

        let cache = make_test_cache();
        let tilemap_cache = make_test_tilemap_cache();

        // Configure esri provider with offline_caching = false
        let mut providers = HashMap::new();
        providers.insert(
            "esri".to_string(),
            config::ProviderCacheConfig {
                offline_caching: false,
            },
        );
        let cache_config = config::CacheConfig {
            tile_cache_enabled: true,
            providers,
            ..Default::default()
        };
        let cache_enabled = cache_config.is_provider_cache_enabled("esri");
        assert!(!cache_enabled);

        // Mock fetch that succeeds
        let mock_fetch = |_: &str, _: u32, _: u32, _: u32| -> Result<FetchedTile, String> {
            Ok(FetchedTile {
                data: b"network_tile".to_vec(),
                content_type: "image/jpeg".to_string(),
            })
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
    fn test_provider_cache_disabled_no_stale_fallback() {
        use std::collections::HashMap;

        // Use a cache with 0 TTL so tiles are immediately stale
        let cache = Arc::new(Mutex::new(Some(TileCache::in_memory(10, 0).unwrap())));
        let tilemap_cache = make_test_tilemap_cache();

        // Pre-populate cache with a tile
        {
            let mut guard = cache.lock().unwrap();
            guard
                .as_mut()
                .unwrap()
                .put("esri", 1, 2, 3, b"stale_tile", "image/png")
                .unwrap();
        }

        // Configure esri with caching disabled
        let mut providers = HashMap::new();
        providers.insert(
            "esri".to_string(),
            config::ProviderCacheConfig {
                offline_caching: false,
            },
        );
        let cache_config = config::CacheConfig {
            tile_cache_enabled: true,
            providers,
            ..Default::default()
        };
        let cache_enabled = cache_config.is_provider_cache_enabled("esri");

        // Network fails
        let mock_fetch = |_: &str, _: u32, _: u32, _: u32| -> Result<FetchedTile, String> {
            Err("Network down".to_string())
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
    fn test_is_provider_cache_enabled_defaults() {
        let config = config::CacheConfig::default();
        // ESRI defaults to disabled
        assert!(!config.is_provider_cache_enabled("esri"));
        // Unlisted provider defaults to enabled
        assert!(config.is_provider_cache_enabled("anything"));
    }

    #[test]
    fn test_is_provider_cache_enabled_global_off() {
        let config = config::CacheConfig {
            tile_cache_enabled: false,
            ..Default::default()
        };
        // Global off overrides everything
        assert!(!config.is_provider_cache_enabled("esri"));
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
        let result = fetch_tile_from_network("unknown", 0, 0, 0, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown provider"));
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
        let mock_fetch = |_: &str, _: u32, _: u32, _: u32| -> Result<FetchedTile, String> {
            Ok(FetchedTile {
                data: b"tile_data".to_vec(),
                content_type: "image/jpeg".to_string(),
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
