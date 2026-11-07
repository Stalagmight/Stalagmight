//! [#576 B35(j), B19] Size accounting of the tile cache: what `put` holds against
//! `max_bytes`, checked against what is actually on disk.
//!
//! Every tile here is synthetic bytes; no imagery and no fixture is read.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use rusqlite::params;

use super::{CachePolicy, TileCache};

/// Push one stored tile to an exact `accessed_at`, so LRU order is decided by
/// the test rather than by which second the puts happened to land in.
fn set_accessed_at(cache: &TileCache, provider: &str, z: u32, y: u32, x: u32, accessed_at: i64) {
    cache
        .conn
        .execute(
            "UPDATE tiles SET accessed_at = ?1
             WHERE provider = ?2 AND z = ?3 AND y = ?4 AND x = ?5",
            params![accessed_at, provider, z, y, x],
        )
        .unwrap();
}

#[test]
fn replacing_the_lru_tile_with_a_bigger_one_stays_under_the_cap() {
    // The cache is exactly full: A (400 B, least recently used) + B (600 B).
    // A comes back at 600 B. Eviction must not count A's bytes twice - once as
    // "being replaced" and once as "evicted" - or the insert lands 200 B over.
    let mut cache = TileCache::in_memory(1, 30).unwrap();
    cache.max_bytes = 1000;
    cache
        .put_granted("esri", 1, 0, 0, &[0xAA; 400], "image/jpeg")
        .unwrap();
    cache
        .put_granted("esri", 1, 0, 1, &[0xBB; 600], "image/jpeg")
        .unwrap();
    set_accessed_at(&cache, "esri", 1, 0, 0, TileCache::now() - 100);
    assert_eq!(cache.get_stats().total_bytes, 1000, "precondition: full");

    cache
        .put_granted("esri", 1, 0, 0, &[0xCC; 600], "image/jpeg")
        .unwrap();

    let total = cache.get_stats().total_bytes;
    assert!(
        total <= cache.max_bytes,
        "after replacing the LRU tile the cache holds {total} bytes; the cap is {}",
        cache.max_bytes
    );
    // The new copy is what was asked for.
    let entry = cache.get("esri", 1, 0, 0).unwrap();
    assert_eq!(entry.data, vec![0xCC; 600]);
}

// ---- [#576 B19] The cost of a put must not grow with the cache. ----

/// Fill a cache file with `rows` granted tiles of `size` bytes, straight into
/// the table in one transaction (the fill is setup, not what is measured), and
/// reopen it so the cache starts from what is on disk.
fn full_cache(dir: &Path, rows: u32, size: usize) -> TileCache {
    {
        let cache = TileCache::open(dir, 1, 30).unwrap();
        let tile = vec![0x5Au8; size];
        let now = TileCache::now();
        let tx = cache.conn.unchecked_transaction().unwrap();
        {
            let mut stmt = cache
                .conn
                .prepare(
                    "INSERT INTO tiles (provider, z, y, x, data, content_type, size_bytes,
                                        fetched_at, accessed_at, expires_at, etag, no_cache)
                     VALUES ('esri', 17, ?1, ?2, ?3, 'image/jpeg', ?4, ?5, ?5, ?6, NULL, 0)",
                )
                .unwrap();
            for i in 0..rows {
                stmt.execute(params![
                    i / 256,
                    i % 256,
                    tile,
                    size as i64,
                    now,
                    now + 86_400
                ])
                .unwrap();
            }
        }
        tx.commit().unwrap();
    }
    let mut cache = TileCache::open(dir, 1, 30).unwrap();
    // Exactly full, so every new tile has to evict one.
    cache.max_bytes = u64::from(rows) * size as u64;
    cache
}

/// SQLite virtual-machine steps run on `cache`'s connection while `f` runs.
///
/// A full-table pass over N rows costs at least N steps (one `Next` per row),
/// so this counts work the way a timing would, without a timing's noise.
fn vm_steps(cache: &mut TileCache, f: impl FnOnce(&mut TileCache)) -> u64 {
    let steps = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&steps);
    cache
        .conn
        .progress_handler(
            1,
            Some(move || {
                counter.fetch_add(1, Ordering::Relaxed);
                false
            }),
        )
        .unwrap();
    f(cache);
    cache
        .conn
        .progress_handler(1, None::<fn() -> bool>)
        .unwrap();
    steps.load(Ordering::Relaxed)
}

#[test]
fn a_put_into_a_full_cache_does_not_scan_the_table() {
    // Every put used to run the default retention DELETE (no index on
    // fetched_at), then COUNT/SUM/COUNT DISTINCT over the whole table, and the
    // same three again for each tile it evicted - all under the one tile-cache
    // mutex every map tile request waits on.
    const ROWS: u32 = 2_000;
    let dir = tempfile::tempdir().unwrap();
    let mut cache = full_cache(dir.path(), ROWS, 100);

    let steps = vm_steps(&mut cache, |cache| {
        cache
            .put_granted("esri", 18, 0, 0, &[0x11; 100], "image/jpeg")
            .unwrap();
    });

    println!("one put into a full {ROWS}-row cache: {steps} SQLite VM steps");
    let stats = cache.get_stats();
    assert_eq!(stats.tile_count, u64::from(ROWS), "one in, one out");
    assert!(stats.total_bytes <= cache.max_bytes);
    assert!(
        steps < u64::from(ROWS),
        "one put into a full {ROWS}-row cache ran {steps} SQLite VM steps; a single \
         full-table pass alone costs at least {ROWS}"
    );
}

#[test]
fn per_provider_retention_does_not_scan_the_table_either() {
    // With a TTL override configured, retention is one DELETE per overridden
    // provider plus a `fetched_at <= ? AND provider NOT IN (...)` sweep for
    // the rest. Both must walk an index, whichever side the written tile is on.
    const ROWS: u32 = 2_000;
    for overridden in ["osm", "esri"] {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = full_cache(dir.path(), ROWS, 100);
        cache.set_provider_ttls(&HashMap::from([(overridden.to_string(), 7)]));

        let steps = vm_steps(&mut cache, |cache| {
            cache
                .put_granted("esri", 18, 0, 0, &[0x11; 100], "image/jpeg")
                .unwrap();
        });

        assert_eq!(cache.get_stats().tile_count, u64::from(ROWS));
        assert!(
            steps < u64::from(ROWS),
            "with an override for {overridden}, one put into a full {ROWS}-row cache ran \
             {steps} SQLite VM steps"
        );
    }
}

/// The running total against the table, after every step.
fn assert_in_step(cache: &TileCache, step: &str) {
    assert_eq!(
        cache.total_bytes,
        cache.sum_size_bytes().unwrap(),
        "running total drifted from SUM(size_bytes) after: {step}"
    );
}

#[test]
fn the_running_total_follows_every_write_path() {
    let mut cache = TileCache::in_memory(1, 30).unwrap();
    cache.max_bytes = 5_000;
    assert_in_step(&cache, "open");

    // A fixed pseudo-random walk of inserts and same-key replacements, enough
    // to fill the cache several times over so eviction runs throughout.
    let mut seed: u32 = 0x2545_F491;
    for i in 0..300u32 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let size = 50 + (seed % 700) as usize;
        let x = seed % 40;
        cache
            .put_granted("esri", 12, 1, x, &vec![0xA5; size], "image/jpeg")
            .unwrap();
        if i % 7 == 0 {
            set_accessed_at(&cache, "esri", 12, 1, x, TileCache::now() - i64::from(i));
        }
        assert_in_step(&cache, &format!("put #{i} ({size} B at x={x})"));
        assert!(
            cache.total_bytes <= cache.max_bytes,
            "over the cap at put #{i}"
        );
    }

    cache
        .put_granted("osm", 3, 3, 3, &[1; 300], "image/png")
        .unwrap();
    cache
        .put_granted("osm", 3, 3, 4, &[1; 200], "image/png")
        .unwrap();
    assert_in_step(&cache, "osm puts");

    // A response with no grant drops what was held.
    cache
        .put(
            "osm",
            3,
            3,
            3,
            &[2; 10],
            "image/png",
            CachePolicy::default(),
            None,
        )
        .unwrap();
    assert_in_step(&cache, "no-grant put");

    // A 304 that revokes storage drops it too.
    cache
        .refresh_validated(
            "osm",
            3,
            3,
            4,
            CachePolicy {
                no_store: true,
                ..CachePolicy::default()
            },
        )
        .unwrap();
    assert_in_step(&cache, "no-store 304");

    // Retention, at startup and from the explicit eviction pass.
    cache
        .conn
        .execute(
            "UPDATE tiles SET fetched_at = ?1 WHERE x < 10",
            params![TileCache::now() - 31 * 24 * 60 * 60],
        )
        .unwrap();
    cache.purge_retired().unwrap();
    assert_in_step(&cache, "purge_retired");

    cache.max_bytes = 1_000;
    cache.evict_lru().unwrap();
    assert_in_step(&cache, "evict_lru under a lowered cap");
    assert!(cache.total_bytes <= 1_000);

    cache.max_bytes = 5_000;
    cache
        .put_granted("osm", 4, 4, 4, &[3; 400], "image/png")
        .unwrap();
    cache.clear_provider("esri").unwrap();
    assert_in_step(&cache, "clear_provider");
    assert_eq!(cache.total_bytes, 400);

    cache.clear_all().unwrap();
    assert_in_step(&cache, "clear_all");
    assert_eq!(cache.total_bytes, 0);
}

#[test]
fn a_failed_write_rolls_back_and_resyncs_the_total() {
    // put retires, evicts and inserts in one transaction. If the insert fails,
    // the evictions roll back with it, so booking them would leave the total
    // short; and an SQL error means the deltas cannot be trusted anyway, so the
    // total is re-read from the table.
    let mut cache = TileCache::in_memory(1, 30).unwrap();
    cache.max_bytes = 1_000;
    cache
        .put_granted("esri", 1, 0, 0, &[1; 600], "image/jpeg")
        .unwrap();
    cache
        .put_granted("esri", 1, 0, 1, &[1; 400], "image/jpeg")
        .unwrap();
    cache
        .conn
        .execute_batch(
            "CREATE TRIGGER refuse_insert BEFORE INSERT ON tiles
             BEGIN SELECT RAISE(ABORT, 'disk full (simulated)'); END;",
        )
        .unwrap();
    // Pretend the total had drifted (a second process, say): the resync after
    // the failure must replace it with what is on disk, not keep it.
    cache.total_bytes = 123;

    let err = cache
        .put_granted("esri", 1, 0, 2, &[1; 500], "image/jpeg")
        .unwrap_err();
    assert!(err.to_string().contains("disk full"), "{err}");

    assert_eq!(cache.get_stats().tile_count, 2, "the eviction rolled back");
    assert_eq!(cache.total_bytes, 1_000, "re-read from the table");
    assert_in_step(&cache, "failed put");
}

#[test]
#[ignore = "benchmark: cargo test --release ... -- --ignored --nocapture put_into_a_full_90mb_cache"]
fn bench_put_into_a_full_90mb_cache() {
    // 3072 tiles x 30 KiB = 90 MiB, the shape a real Esri imagery cache has
    // (~20-40 KB JPEGs). TILE_BENCH_DIR picks the filesystem (the default
    // temp dir is often tmpfs, i.e. RAM).
    const ROWS: u32 = 3_072;
    const SIZE: usize = 30 * 1024;
    const PUTS: u32 = 200;
    let dir = match std::env::var_os("TILE_BENCH_DIR") {
        Some(base) => tempfile::tempdir_in(base).unwrap(),
        None => tempfile::tempdir().unwrap(),
    };

    let fill = Instant::now();
    drop(full_cache(dir.path(), ROWS, SIZE));
    let fill = fill.elapsed();

    // The startup path, as `AppState::init_tile_cache` runs it: open, then
    // drop whatever is past retention (nothing is, here).
    let reopen = Instant::now();
    let mut cache = TileCache::open(dir.path(), 1, 30).unwrap();
    let reopen = reopen.elapsed();
    let purge = Instant::now();
    cache.purge_retired().unwrap();
    let purge = purge.elapsed();
    cache.max_bytes = u64::from(ROWS) * SIZE as u64;

    let tile = vec![0x77u8; SIZE];
    let mut each = Vec::with_capacity(PUTS as usize);
    let total = Instant::now();
    for i in 0..PUTS {
        let one = Instant::now();
        cache
            .put_granted("esri", 18, i / 256, i % 256, &tile, "image/jpeg")
            .unwrap();
        each.push(one.elapsed());
    }
    let total = total.elapsed();
    each.sort();

    let stats = cache.get_stats();
    let db_bytes = std::fs::metadata(dir.path().join("tile_cache.db")).map_or(0, |m| m.len());
    println!(
        "cache: {} tiles, {:.1} MiB of tile bytes, db file {:.1} MiB, dir {}",
        stats.tile_count,
        stats.total_bytes as f64 / 1_048_576.0,
        db_bytes as f64 / 1_048_576.0,
        dir.path().display()
    );
    println!("setup fill (raw SQL, not measured): {fill:.2?}");
    println!("startup on the full cache: TileCache::open {reopen:.2?} + purge_retired {purge:.2?}");
    println!(
        "{PUTS} puts into the full cache (each evicts one tile): total {total:.2?}, \
         mean {:.2?}, p50 {:.2?}, p95 {:.2?}, max {:.2?}",
        total / PUTS,
        each[each.len() / 2],
        each[each.len() * 95 / 100],
        each[each.len() - 1]
    );
    assert!(stats.total_bytes <= cache.max_bytes);
}
