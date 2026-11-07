//! [#576 NF-W4T-3] The per-provider cache figures come from running totals.
//!
//! `get_provider_stats` (Settings > Cache) ran `GROUP BY provider SUM(size_bytes)`
//! over the whole table - a pass that walks every tile's BLOB overflow chain,
//! because `size_bytes` is stored after `data` - under the mutex every map tile
//! request waits on. The totals are now kept per provider by every write that
//! adds or frees bytes, as `total_bytes` already was (#576 B19), so the panel
//! reads them without touching the table.
//!
//! Every tile here is synthetic bytes; no imagery and no fixture is read.

use std::collections::HashMap;

use super::TileCache;

/// What the table itself says, per provider - the figure the totals must equal.
fn on_disk(cache: &TileCache) -> HashMap<String, (u64, u64)> {
    let mut stmt = cache
        .conn
        .prepare(
            "SELECT provider, COUNT(*), COALESCE(SUM(size_bytes), 0) FROM tiles GROUP BY provider",
        )
        .expect("prepare");
    stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            (row.get::<_, i64>(1)? as u64, row.get::<_, i64>(2)? as u64),
        ))
    })
    .expect("query")
    .collect::<Result<HashMap<_, _>, _>>()
    .expect("rows")
}

#[test]
fn provider_stats_track_every_write_without_reading_the_table() {
    let mut cache = TileCache::in_memory(1, 30).expect("cache");
    cache.max_bytes = 3000;
    for x in 0..4 {
        cache
            .put_granted("esri", 1, 0, x, &[0xAA; 500], "image/jpeg")
            .expect("put esri");
        cache
            .put_granted("osm", 1, 0, x, &[0xBB; 300], "image/png")
            .expect("put osm");
    }
    // A replacement (same tile, new size) and an eviction past the cap.
    cache
        .put_granted("osm", 1, 0, 0, &[0xCC; 700], "image/png")
        .expect("replace osm");
    cache.clear_provider("esri").expect("clear esri");
    cache
        .put_granted("esri", 2, 0, 0, &[0xDD; 900], "image/jpeg")
        .expect("put esri again");

    let totals = cache.get_provider_stats();
    let disk = on_disk(&cache);
    println!("running totals: {totals:?}");
    println!("on disk:        {disk:?}");
    assert_eq!(totals, disk, "the totals follow every write");

    // And they are read without the table: with it gone, the panel still answers.
    cache
        .conn
        .execute_batch("DROP TABLE tiles")
        .expect("drop the table");
    let without_table = cache.get_provider_stats();
    println!("with the tiles table gone: {without_table:?}");
    assert_eq!(without_table, disk, "no pass over the table");
}
