//! In-memory LRU cache for ArcGIS tilemap API responses.
//!
//! The tilemap API returns availability information for a 128×128 grid of tiles
//! at a given zoom level. This cache stores parsed tilemap responses to avoid
//! redundant network requests.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Size of the tilemap grid (128×128 = 16,384 tiles per response).
pub const TILEMAP_GRID_SIZE: u32 = 128;

/// Default TTL for cached tilemap entries.
const DEFAULT_TTL: Duration = Duration::from_secs(3600); // 1 hour

/// Default maximum number of cached tilemap entries.
const DEFAULT_MAX_ENTRIES: usize = 100;

/// Key for a tilemap cache entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TilemapKey {
    /// Provider name (e.g., "esri").
    provider: String,
    /// Zoom level.
    z: u32,
    /// Base Y coordinate (aligned to TILEMAP_GRID_SIZE).
    base_y: u32,
    /// Base X coordinate (aligned to TILEMAP_GRID_SIZE).
    base_x: u32,
}

/// A single cached tilemap entry.
struct TilemapEntry {
    /// Availability data: true = tile has real imagery, false = no data.
    /// Length is TILEMAP_GRID_SIZE × TILEMAP_GRID_SIZE (16,384). `None` marks
    /// a region whose tilemap was refused ([`TilemapCache::insert_refused`]).
    data: Option<Vec<bool>>,
    /// When this entry was cached.
    cached_at: Instant,
    /// Last access time for LRU eviction.
    last_accessed: Instant,
}

/// In-memory LRU cache for tilemap responses.
pub struct TilemapCache {
    entries: HashMap<TilemapKey, TilemapEntry>,
    max_entries: usize,
    ttl: Duration,
}

impl TilemapCache {
    /// Create a new tilemap cache with default settings.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            max_entries: DEFAULT_MAX_ENTRIES,
            ttl: DEFAULT_TTL,
        }
    }

    /// Create a new tilemap cache with custom settings.
    pub fn with_config(max_entries: usize, ttl: Duration) -> Self {
        Self {
            entries: HashMap::new(),
            max_entries,
            ttl,
        }
    }

    /// Align a tile coordinate to the tilemap grid boundary.
    fn align_to_grid(coord: u32) -> u32 {
        (coord / TILEMAP_GRID_SIZE) * TILEMAP_GRID_SIZE
    }

    /// Check if a specific tile is available according to cached tilemap data.
    ///
    /// Returns:
    /// - `Some(true)` if the tile has real imagery
    /// - `Some(false)` if the tile is known to have no data
    /// - `None` if no tilemap data is cached for this tile's region, or the
    ///   region's tilemap was refused ([`Self::is_refused`] tells the two apart)
    pub fn is_tile_available(&mut self, provider: &str, z: u32, y: u32, x: u32) -> Option<bool> {
        let entry = self.live_entry(provider, z, y, x)?;

        // Calculate index within the grid
        let offset_y = y - Self::align_to_grid(y);
        let offset_x = x - Self::align_to_grid(x);
        let index = (offset_y * TILEMAP_GRID_SIZE + offset_x) as usize;

        entry.data.as_ref()?.get(index).copied()
    }

    /// Whether the tilemap for this tile's region was refused
    /// ([`Self::insert_refused`]) and that verdict has not expired. Counts as
    /// a use of the entry for LRU, like [`Self::is_tile_available`].
    pub fn is_refused(&mut self, provider: &str, z: u32, y: u32, x: u32) -> bool {
        self.live_entry(provider, z, y, x)
            .is_some_and(|entry| entry.data.is_none())
    }

    /// The unexpired entry for the region holding tile (`y`, `x`), marked as
    /// just used. An expired one is removed and reads as absent.
    fn live_entry(&mut self, provider: &str, z: u32, y: u32, x: u32) -> Option<&TilemapEntry> {
        let key = TilemapKey {
            provider: provider.to_string(),
            z,
            base_y: Self::align_to_grid(y),
            base_x: Self::align_to_grid(x),
        };

        // Check TTL
        if self.entries.get(&key)?.cached_at.elapsed() > self.ttl {
            // Entry is stale, remove it
            self.entries.remove(&key);
            return None;
        }

        // Update last accessed time for LRU
        let entry = self.entries.get_mut(&key)?;
        entry.last_accessed = Instant::now();
        Some(entry)
    }

    /// Insert tilemap data for a region.
    ///
    /// `base_y` and `base_x` should be aligned to TILEMAP_GRID_SIZE boundaries.
    /// `data` should contain TILEMAP_GRID_SIZE × TILEMAP_GRID_SIZE (16,384) booleans.
    pub fn insert(&mut self, provider: &str, z: u32, base_y: u32, base_x: u32, data: Vec<bool>) {
        // Validate data length
        let expected_len = (TILEMAP_GRID_SIZE * TILEMAP_GRID_SIZE) as usize;
        if data.len() != expected_len {
            tracing::warn!(
                provider = %provider,
                z = z,
                base_y = base_y,
                base_x = base_x,
                expected = expected_len,
                actual = data.len(),
                "Invalid tilemap data length, ignoring"
            );
            return;
        }

        self.put(provider, z, base_y, base_x, Some(data));
    }

    /// Remember that the provider's tilemap for a region was refused - it was
    /// larger than any tilemap can be - so the tiles there are not checked
    /// against a fresh download of it each time (#576 RV19a-10). The entry
    /// expires and is evicted like any other; until then
    /// [`Self::is_tile_available`] answers `None` (unknown) for the region and
    /// [`Self::is_refused`] answers `true`.
    pub fn insert_refused(&mut self, provider: &str, z: u32, base_y: u32, base_x: u32) {
        self.put(provider, z, base_y, base_x, None);
    }

    fn put(&mut self, provider: &str, z: u32, base_y: u32, base_x: u32, data: Option<Vec<bool>>) {
        // Evict if at capacity
        if self.entries.len() >= self.max_entries {
            self.evict_lru();
        }

        let key = TilemapKey {
            provider: provider.to_string(),
            z,
            base_y,
            base_x,
        };

        let now = Instant::now();
        self.entries.insert(
            key,
            TilemapEntry {
                data,
                cached_at: now,
                last_accessed: now,
            },
        );
    }

    /// Evict the least recently used entry.
    fn evict_lru(&mut self) {
        if let Some(oldest_key) = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_accessed)
            .map(|(key, _)| key.clone())
        {
            self.entries.remove(&oldest_key);
        }
    }

    /// Remove all expired entries.
    pub fn cleanup_expired(&mut self) {
        self.entries
            .retain(|_, entry| entry.cached_at.elapsed() <= self.ttl);
    }

    /// Get the number of cached entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clear all cached entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Get the base coordinates for a tile (aligned to grid).
    pub fn get_region_origin(y: u32, x: u32) -> (u32, u32) {
        (Self::align_to_grid(y), Self::align_to_grid(x))
    }
}

impl Default for TilemapCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_align_to_grid() {
        assert_eq!(TilemapCache::align_to_grid(0), 0);
        assert_eq!(TilemapCache::align_to_grid(1), 0);
        assert_eq!(TilemapCache::align_to_grid(127), 0);
        assert_eq!(TilemapCache::align_to_grid(128), 128);
        assert_eq!(TilemapCache::align_to_grid(129), 128);
        assert_eq!(TilemapCache::align_to_grid(255), 128);
        assert_eq!(TilemapCache::align_to_grid(256), 256);
    }

    #[test]
    fn test_get_region_origin() {
        assert_eq!(TilemapCache::get_region_origin(50, 200), (0, 128));
        assert_eq!(TilemapCache::get_region_origin(300, 300), (256, 256));
    }

    #[test]
    fn test_insert_and_lookup() {
        let mut cache = TilemapCache::new();

        // Create test data: alternating true/false
        let data: Vec<bool> = (0..16384).map(|i| i % 2 == 0).collect();

        cache.insert("esri", 18, 0, 0, data);

        // Check specific tiles
        assert_eq!(cache.is_tile_available("esri", 18, 0, 0), Some(true));
        assert_eq!(cache.is_tile_available("esri", 18, 0, 1), Some(false));
        assert_eq!(cache.is_tile_available("esri", 18, 0, 2), Some(true));

        // Row 1 starts at index 128
        assert_eq!(cache.is_tile_available("esri", 18, 1, 0), Some(true)); // index 128
        assert_eq!(cache.is_tile_available("esri", 18, 1, 1), Some(false)); // index 129
    }

    #[test]
    fn test_cache_miss() {
        let mut cache = TilemapCache::new();

        // No data cached
        assert_eq!(cache.is_tile_available("esri", 18, 50, 50), None);

        // Different provider
        let data: Vec<bool> = vec![true; 16384];
        cache.insert("esri", 18, 0, 0, data);
        assert_eq!(cache.is_tile_available("other", 18, 50, 50), None);

        // Different zoom
        assert_eq!(cache.is_tile_available("esri", 17, 50, 50), None);
    }

    #[test]
    fn test_region_alignment() {
        let mut cache = TilemapCache::new();

        // Insert data for region starting at (128, 256)
        let data: Vec<bool> = vec![true; 16384];
        cache.insert("esri", 18, 128, 256, data);

        // Tiles within this region should be found
        assert_eq!(cache.is_tile_available("esri", 18, 128, 256), Some(true));
        assert_eq!(cache.is_tile_available("esri", 18, 200, 300), Some(true));
        assert_eq!(cache.is_tile_available("esri", 18, 255, 383), Some(true));

        // Tiles outside this region should not be found
        assert_eq!(cache.is_tile_available("esri", 18, 127, 256), None); // y too low
        assert_eq!(cache.is_tile_available("esri", 18, 256, 256), None); // y too high
        assert_eq!(cache.is_tile_available("esri", 18, 128, 255), None); // x too low
        assert_eq!(cache.is_tile_available("esri", 18, 128, 384), None); // x too high
    }

    #[test]
    fn test_invalid_data_length_ignored() {
        let mut cache = TilemapCache::new();

        // Try to insert data with wrong length
        let bad_data: Vec<bool> = vec![true; 100];
        cache.insert("esri", 18, 0, 0, bad_data);

        // Should not be cached
        assert!(cache.is_empty());
        assert_eq!(cache.is_tile_available("esri", 18, 0, 0), None);
    }

    #[test]
    fn test_lru_eviction() {
        let mut cache = TilemapCache::with_config(2, DEFAULT_TTL);

        let data: Vec<bool> = vec![true; 16384];

        // Insert first entry
        cache.insert("esri", 18, 0, 0, data.clone());
        assert_eq!(cache.len(), 1);

        // Insert second entry
        cache.insert("esri", 18, 0, 128, data.clone());
        assert_eq!(cache.len(), 2);

        // Access first entry to make it more recent
        cache.is_tile_available("esri", 18, 0, 0);

        // Insert third entry, should evict second (LRU)
        cache.insert("esri", 18, 128, 0, data);
        assert_eq!(cache.len(), 2);

        // First should still exist (was accessed)
        assert!(cache.is_tile_available("esri", 18, 0, 0).is_some());
        // Second should be evicted
        assert!(cache.is_tile_available("esri", 18, 0, 128).is_none());
        // Third should exist
        assert!(cache.is_tile_available("esri", 18, 128, 0).is_some());
    }

    #[test]
    fn test_ttl_expiration() {
        let mut cache = TilemapCache::with_config(100, Duration::from_millis(10));

        let data: Vec<bool> = vec![true; 16384];
        cache.insert("esri", 18, 0, 0, data);

        // Should be available immediately
        assert!(cache.is_tile_available("esri", 18, 0, 0).is_some());

        // Wait for TTL to expire
        std::thread::sleep(Duration::from_millis(20));

        // Should be expired now
        assert!(cache.is_tile_available("esri", 18, 0, 0).is_none());
    }

    #[test]
    fn test_clear() {
        let mut cache = TilemapCache::new();
        let data: Vec<bool> = vec![true; 16384];
        cache.insert("esri", 18, 0, 0, data);
        assert_eq!(cache.len(), 1);

        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn test_cleanup_expired() {
        let mut cache = TilemapCache::with_config(100, Duration::from_millis(10));

        let data: Vec<bool> = vec![true; 16384];
        cache.insert("esri", 18, 0, 0, data.clone());
        cache.insert("esri", 18, 0, 128, data);
        assert_eq!(cache.len(), 2);

        // Wait for TTL
        std::thread::sleep(Duration::from_millis(20));

        cache.cleanup_expired();
        assert!(cache.is_empty());
    }

    #[test]
    fn a_refused_region_reads_as_refused_and_unknown() {
        let mut cache = TilemapCache::new();
        cache.insert_refused("esri", 18, 128, 256);
        assert_eq!(cache.len(), 1);

        // Every tile of the region, and only that region.
        assert!(cache.is_refused("esri", 18, 128, 256));
        assert!(cache.is_refused("esri", 18, 255, 383));
        assert!(!cache.is_refused("esri", 18, 127, 256));
        assert!(!cache.is_refused("esri", 18, 128, 384));
        assert!(!cache.is_refused("other", 18, 128, 256));
        assert!(!cache.is_refused("esri", 17, 128, 256));
        // Availability there is unknown, not "no data".
        assert_eq!(cache.is_tile_available("esri", 18, 200, 300), None);

        // A region with real data is not refused.
        cache.insert("esri", 18, 0, 0, vec![true; 16384]);
        assert!(!cache.is_refused("esri", 18, 0, 0));
        assert_eq!(cache.is_tile_available("esri", 18, 0, 0), Some(true));
    }

    #[test]
    fn a_refusal_expires_with_the_ttl() {
        let mut cache = TilemapCache::with_config(100, Duration::from_millis(10));
        cache.insert_refused("esri", 18, 0, 0);
        assert!(cache.is_refused("esri", 18, 0, 0));

        std::thread::sleep(Duration::from_millis(20));

        assert!(!cache.is_refused("esri", 18, 0, 0));
        assert!(cache.is_empty(), "the expired refusal is dropped");
    }

    #[test]
    fn a_refusal_takes_part_in_lru_eviction() {
        let mut cache = TilemapCache::with_config(2, DEFAULT_TTL);
        cache.insert_refused("esri", 18, 0, 0);
        cache.insert("esri", 18, 0, 128, vec![true; 16384]);

        // Checking the refusal counts as a use, so the data entry is older.
        assert!(cache.is_refused("esri", 18, 0, 0));
        cache.insert("esri", 18, 128, 0, vec![true; 16384]);
        assert_eq!(cache.len(), 2);
        assert!(cache.is_refused("esri", 18, 0, 0));
        assert!(cache.is_tile_available("esri", 18, 0, 128).is_none());

        // And an unused refusal is evicted like any entry.
        cache.is_tile_available("esri", 18, 128, 0);
        cache.insert("esri", 18, 256, 0, vec![true; 16384]);
        assert!(!cache.is_refused("esri", 18, 0, 0));
        assert!(cache.is_tile_available("esri", 18, 256, 0).is_some());
    }
}
