//! [#576 RV4-R-NITS T-2] The running totals are read with ONE pass over the
//! table, at open and at every resync.
//!
//! `total_bytes` (#576 B19) and `provider_totals` (NF-W4T-3) were each read
//! with their own full pass - a `SUM` and a `GROUP BY` that both walk every
//! tile's BLOB overflow chain - although the first is exactly the sum of the
//! second's byte column. `total_bytes` is now derived from the per-provider
//! pass.
//!
//! Every tile here is synthetic bytes; no imagery and no fixture is read.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rusqlite::params;

use super::TileCache;

/// SQLite virtual-machine instructions run on `cache`'s connection while `f`
/// runs. A full-table pass over N rows costs at least N of them.
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

/// A cache file holding `rows` tiles split over two providers, written
/// straight into the table, then reopened so the totals come from disk.
fn reopened_cache(dir: &Path, rows: u32) -> TileCache {
    {
        let cache = TileCache::open(dir, 64, 30).unwrap();
        let now = TileCache::now();
        let tx = cache.conn.unchecked_transaction().unwrap();
        {
            let mut stmt = cache
                .conn
                .prepare(
                    "INSERT INTO tiles (provider, z, y, x, data, content_type, size_bytes,
                                        fetched_at, accessed_at, expires_at, etag, no_cache)
                     VALUES (?1, 17, ?2, ?3, ?4, 'image/jpeg', ?5, ?6, ?6, ?7, NULL, 0)",
                )
                .unwrap();
            for i in 0..rows {
                let (provider, size) = if i % 3 == 0 {
                    ("osm", 70)
                } else {
                    ("esri", 110)
                };
                stmt.execute(params![
                    provider,
                    i / 256,
                    i % 256,
                    vec![0x5Au8; size],
                    size as i64,
                    now,
                    now + 86_400
                ])
                .unwrap();
            }
        }
        tx.commit().unwrap();
    }
    TileCache::open(dir, 64, 30).unwrap()
}

fn summed_provider_bytes(cache: &TileCache) -> u64 {
    cache.provider_totals.values().map(|(_, bytes)| bytes).sum()
}

#[test]
fn the_total_is_the_sum_of_the_provider_totals_after_open_and_resync() {
    let dir = tempfile::tempdir().unwrap();
    let mut cache = reopened_cache(dir.path(), 300);
    let on_disk = cache.sum_size_bytes().unwrap();
    println!(
        "T-2 open: total_bytes {} / provider sum {} / SUM(size_bytes) {on_disk}",
        cache.total_bytes,
        summed_provider_bytes(&cache)
    );
    assert_eq!(cache.total_bytes, on_disk);
    assert_eq!(cache.total_bytes, summed_provider_bytes(&cache));
    assert_eq!(cache.provider_totals.len(), 2);

    // Knock both running figures off, then resync from disk.
    cache.total_bytes = 1;
    cache.provider_totals.clear();
    cache.resync_total_bytes();
    println!(
        "T-2 resync: total_bytes {} / provider sum {} / SUM(size_bytes) {on_disk}",
        cache.total_bytes,
        summed_provider_bytes(&cache)
    );
    assert_eq!(cache.total_bytes, on_disk);
    assert_eq!(cache.total_bytes, summed_provider_bytes(&cache));
}

#[test]
fn a_resync_reads_the_table_once() {
    const ROWS: u32 = 3_000;
    let dir = tempfile::tempdir().unwrap();
    let mut cache = reopened_cache(dir.path(), ROWS);
    let one_pass = vm_steps(&mut cache, |cache| {
        cache.sum_by_provider().unwrap();
    });
    let resync = vm_steps(&mut cache, TileCache::resync_total_bytes);
    println!(
        "T-2 {ROWS}-row cache: one GROUP BY pass {one_pass} SQLite VM steps, a resync \
         {resync} (a second full pass would add at least {ROWS})"
    );
    assert!(
        resync < one_pass + u64::from(ROWS),
        "a resync ran {resync} SQLite VM steps, at least one full {ROWS}-row pass more than \
         the {one_pass} of the per-provider pass alone"
    );
}
