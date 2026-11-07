//! SQLite-backed HTTP cache for map tiles, with LRU eviction.
//!
//! This is an ordinary caching client, not a tile store: it holds what the
//! provider's own `Cache-Control` permits, for as long as it permits, and asks
//! again with the validator the provider supplied. It never bulk-downloads a
//! region, and nothing here calls an export API.
//!
//! # Nothing is stored without an explicit grant
//!
//! A tile is written to disk ONLY if its response carried a `Cache-Control`
//! that says how long it may be kept (`max-age`, or `no-cache` for "keep but
//! always revalidate"). A response with no directive is served and discarded.
//!
//! That is a deliberately narrow reading. Silence is not permission: the app
//! stores third-party imagery only where the provider's own headers say it may,
//! and only for as long as they say. Esri answers every tile with
//! `max-age=86400`, so its tiles cache; a provider that says nothing gets no
//! disk at all until it does.
//!
//! # Two clocks, two jobs
//!
//! A cached tile is governed by two independent deadlines, and conflating them
//! would break either the directives we cache under or the ability to answer a
//! failed request:
//!
//! - **Freshness** (`get`) - may this tile be served *instead of* asking the
//!   provider? Bounded by BOTH the configured TTL and the provider's own
//!   `Cache-Control: max-age` (`expires_at`), whichever expires first. Esri's
//!   tiles come back with `max-age=86400`, so a 30-day configured TTL cannot
//!   make us treat a two-day-old tile as current.
//! - **Retention** (`get_stale`, eviction) - may this tile stay on disk at all?
//!   Bounded by the configured TTL alone.
//!
//! The gap between them is deliberate and is standard HTTP: past `max-age` a
//! tile stops being served as fresh, so every request goes to the network and
//! revalidates, but the stored copy survives to answer if that request fails.
//! That is `stale-if-error` (RFC 5861). Expired imagery is never presented as
//! current - it is only ever a last resort when the provider cannot be reached.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Ceiling on a provider's `max-age`, in seconds (one year).
///
/// `max-age` arrives off the network, so it is an untrusted number that this
/// module then adds to the clock. Without a ceiling, `max-age=<i64::MAX>`
/// overflows the addition: a debug build panics *inside* the tile-cache mutex,
/// poisoning it and silently disabling caching for the rest of the session,
/// while a release build wraps to a negative expiry the tile can never satisfy.
/// A year is far beyond any real basemap grant (Esri sends a day), so clamping
/// here costs nothing legitimate.
const MAX_GRANT_SECONDS: i64 = 365 * 24 * 60 * 60;

/// Freshness directives read off a tile response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CachePolicy {
    /// Seconds the provider says the tile may be served without revalidating,
    /// from `Cache-Control: max-age`.
    ///
    /// `None` means the provider said nothing, and is treated as NO PERMISSION
    /// TO STORE - not as "cache it indefinitely". This is the gate the whole
    /// module hangs off; see the note at the top.
    pub max_age_seconds: Option<i64>,
    /// The provider forbade storing this response at all (`no-store`).
    pub no_store: bool,
    /// Seconds this response had already spent in an intermediary cache when it
    /// reached us, from the `Age` header.
    ///
    /// `max-age` counts from when the ORIGIN generated the response, not from
    /// when we received it. Esri is behind CloudFront and does send `Age` on an
    /// edge hit, so ignoring it would let a tile that has 1 hour of its grant
    /// left be recorded as fresh for a further 24 - serving imagery as current
    /// past the window the provider actually gave.
    pub age_seconds: Option<i64>,
    /// The provider granted storage but forbade unvalidated reuse (`no-cache`).
    ///
    /// Kept separate from `max_age_seconds: Some(0)`, which it also produces,
    /// because on disk the two are otherwise identical - and they are not the
    /// same permission. A grant that merely ran out may still answer a failed
    /// request (`stale-if-error`); `no-cache` may not. RFC 9111 5.2.2.4 requires
    /// successful validation on the origin BEFORE reuse, so a `no-cache` row is
    /// excluded from the stale path even when the network is down.
    pub no_cache: bool,
    /// The header could not be parsed, so we know NOTHING about permission.
    ///
    /// Distinct from "the provider sent no directive", which is a statement -
    /// permission withdrawn, drop the copy. This is the absence of one: a proxy
    /// mangled a header, which says nothing about whether the grant a stored
    /// copy was written under still stands. Both write paths therefore skip
    /// writing AND skip deleting, leaving any existing row to age out on the
    /// deadline it already recorded.
    pub malformed: bool,
}

impl CachePolicy {
    /// Seconds of storage the provider granted, or `None` for no permission.
    ///
    /// **The single gate.** Both write paths - `put` for a fetched body and
    /// `refresh_validated` for a `304` - ask this and nothing else. They used to
    /// carry the rule separately and drifted: the revalidation path never
    /// checked `no_store`, so a provider revoking permission on a 304 had its
    /// tile kept AND its retention extended.
    ///
    /// `Age` is subtracted here for the same reason the ceiling is: this is the
    /// one place every write path passes through, so making it the gate's job
    /// means a producer cannot forget it. A grant fully spent in a CDN comes
    /// back as `Some(0)` rather than `None` - the provider DID permit storage,
    /// the freshness is just used up, so the tile is kept (with its ETag, for a
    /// cheap revalidation) and never served as current. That is exactly the
    /// shape `no-cache` already maps to.
    pub fn granted_seconds(&self) -> Option<i64> {
        if self.malformed {
            return None;
        }
        // Clamped HERE rather than only at the parser: `TileEntry::
        // remaining_policy` also mints a max-age (for an overzoomed crop) and it
        // reaches `put` through the same door. Enforcing the ceiling on the gate
        // makes MAX_GRANT_SECONDS a real invariant of everything written to
        // disk, instead of a property of one producer.
        self.max_age_seconds
            .filter(|_| !self.no_store)
            .map(|seconds| seconds.min(MAX_GRANT_SECONDS))
            .map(|seconds| {
                seconds
                    .saturating_sub(self.age_seconds.unwrap_or(0).max(0))
                    .max(0)
            })
    }
}

/// Entry returned from the cache
#[derive(Debug, Clone)]
pub struct TileEntry {
    /// Raw tile image data
    pub data: Vec<u8>,
    /// MIME content type (e.g., "image/jpeg", "image/png")
    pub content_type: String,
    /// Size in bytes
    pub size_bytes: u64,
    /// Unix timestamp when tile was fetched
    pub fetched_at: i64,
    /// Unix timestamp when tile was last accessed
    pub accessed_at: i64,
    /// Unix timestamp the provider's `max-age` runs out, if it sent one.
    /// `None` = the provider said nothing, so only the configured TTL applies.
    pub expires_at: Option<i64>,
    /// Validator from the response this tile came in, for a conditional
    /// re-fetch. `None` = no `ETag`, so revalidation is not possible and a
    /// re-fetch has to pull the whole body.
    pub etag: Option<String>,
    /// This row was stored under `no-cache`: it may be revalidated, but never
    /// served without a successful validation first (RFC 9111 5.2.2.4).
    ///
    /// Carried on the entry rather than filtered out of the lookup because the
    /// two questions differ, and `no-cache` is exactly where they diverge:
    /// "do we hold bytes to validate against?" (yes - that is why the ETag was
    /// kept) versus "may we serve these unvalidated?" (no). Filtering hid the
    /// validator too, so every request re-downloaded the whole body - the
    /// opposite of why the row was stored.
    pub no_cache: bool,
}

impl TileEntry {
    /// The freshness this entry has LEFT, as a policy a derived tile can inherit.
    ///
    /// Overzoomed tiles are crops of a parent, and re-deriving one must not hand
    /// it a longer life than the imagery it came from - so the remaining seconds
    /// travel with it rather than the original `max-age`.
    pub fn remaining_policy(&self, now: i64) -> CachePolicy {
        CachePolicy {
            max_age_seconds: self.expires_at.map(|at| at.saturating_sub(now).max(0)),
            no_store: false,
            // Already accounted for: `expires_at` was stamped net of the
            // parent's `Age`, so subtracting it again would age the crop twice.
            age_seconds: None,
            no_cache: false,
            malformed: false,
        }
    }
}

/// Split a `Cache-Control` value on the commas that actually separate
/// directives, returning `None` if the quoting is malformed.
///
/// `no-cache="Set-Cookie,X-Foo"` is ONE directive. A naive `split(',')` cuts it
/// in two, which loses the `no-cache` half - and, in the other direction, hands
/// back a bare `max-age=999` lifted out of `private="x,max-age=999,y"`, i.e. a
/// storage grant the provider never issued.
///
/// Quoting has two further traps, and both let a grant through that was never
/// given:
///
/// - **Quoted-pair escapes.** RFC 9110 5.6.4 lets a quoted-string contain `\"`.
///   Toggling on every `"` ends the string early, so
///   `private="x\", max-age=86400, y"` - one `private` directive - split into
///   three, and the `max-age` inside it was read as a standalone grant.
/// - **Unbalanced quotes.** A stray `"` swallows everything after it, so a
///   `no-store` that arrived later simply vanished while an earlier `max-age`
///   still stood. That is the same hole pointing the other way.
///
/// The first is parsed properly. The second cannot be - the header is malformed
/// and there is no way to know what was meant - so it fails closed: `None` here
/// becomes "the provider said nothing", and nothing is stored.
fn split_directives(header: &str) -> Option<Vec<&str>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    let mut escaped = false;
    for (idx, ch) in header.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            // A backslash is only an escape INSIDE a quoted-string; outside one
            // it is an ordinary character.
            '\\' if in_quotes => escaped = true,
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                parts.push(&header[start..idx]);
                start = idx + 1;
            }
            _ => {}
        }
    }
    if in_quotes || escaped {
        return None;
    }
    parts.push(&header[start..]);
    Some(parts)
}

/// Parse the freshness directives out of a `Cache-Control` header value.
///
/// Only the directives that change what this cache may do are read; everything
/// else (`public`, `must-revalidate`, ...) is inert here. `no-cache` maps to
/// `max_age_seconds: Some(0)` - store it, never serve it as fresh, which is
/// exactly the stale-if-error shape the rest of the module is built around.
pub fn parse_cache_control(header: &str) -> CachePolicy {
    let mut policy = CachePolicy::default();
    // Tracked separately and applied last: `no-cache` means "always revalidate"
    // regardless of any `max-age` sent alongside it, and headers do not promise
    // an order. Setting max_age directly from the `no-cache` arm let a later
    // `max-age=86400` in the same header silently overwrite it.
    let mut no_cache = false;
    let Some(tokens) = split_directives(header) else {
        // Malformed quoting: no grant, and no information either. Flagged rather
        // than returned as a plain default, because "said nothing" DELETES an
        // existing row (permission withdrawn) and a header we simply could not
        // read is no reason to drop a copy stored under a grant that was valid
        // when it was given.
        tracing::debug!(
            header,
            "Unbalanced quoting in Cache-Control; no grant, and no revocation either"
        );
        return CachePolicy {
            malformed: true,
            ..CachePolicy::default()
        };
    };
    for token in tokens {
        let token = token.trim().to_ascii_lowercase();
        let (name, value) = match token.split_once('=') {
            Some((name, value)) => (name.trim(), Some(value.trim())),
            None => (token.as_str(), None),
        };
        match name {
            "no-store" => policy.no_store = true,
            "no-cache" => no_cache = true,
            "max-age" => {
                // A negative or unparseable max-age is treated as "said nothing"
                // rather than as zero: guessing "immediately stale" from a
                // malformed header would silently disable caching for a whole
                // provider.
                if let Some(Ok(seconds)) = value.map(str::parse::<i64>) {
                    if seconds >= 0 {
                        // Clamped here as well as in `granted_seconds`: the gate
                        // is the invariant, this just stops an absurd value
                        // travelling any further than it has to.
                        policy.max_age_seconds = Some(seconds.min(MAX_GRANT_SECONDS));
                    }
                }
            }
            _ => {}
        }
    }
    if no_cache {
        policy.max_age_seconds = Some(0);
        policy.no_cache = true;
    }
    policy
}

/// Parse an `Age` header value: seconds the response has already spent in
/// intermediary caches.
///
/// Note the polarity, because it is the reverse of `max-age` and that is the
/// trap. For a grant, "unreadable" means NO grant, so the tile is not stored -
/// the safe direction. For `Age`, "unreadable" means NOT AGED, i.e. maximum
/// freshness - the unsafe one. So the two cannot share a rule:
///
/// - A number too large for us is still a number, and a huge one means the
///   response is very old. RFC 9111 1.2.2 is explicit that an over-large
///   `delta-seconds` MUST be clamped to the largest value the cache can
///   conveniently represent, not ignored. Clamping to `MAX_GRANT_SECONDS`
///   spends any real grant outright, so the tile is stored but never served as
///   current - which is what "we cannot tell how stale this is" should mean.
/// - Genuine junk (`soon`, `12.5`, a negative) says nothing about age at all,
///   and is treated as absent. Reading it as maximally stale would let one
///   malformed header from a proxy force a revalidation on every single
///   request for that provider, forever.
pub fn parse_age(header: &str) -> Option<i64> {
    let raw = header.trim();
    match raw.parse::<i64>() {
        Ok(seconds) if seconds >= 0 => Some(seconds.min(MAX_GRANT_SECONDS)),
        // Overflowed `i64` while being all digits: too old to represent.
        Err(_) if !raw.is_empty() && raw.bytes().all(|b| b.is_ascii_digit()) => {
            Some(MAX_GRANT_SECONDS)
        }
        _ => None,
    }
}

/// Current Unix timestamp, on the same clock the cache stamps entries with.
pub fn now() -> i64 {
    TileCache::now()
}

/// Cache statistics
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// Total number of tiles in cache
    pub tile_count: u64,
    /// Total size of cached tiles in bytes
    pub total_bytes: u64,
    /// Number of distinct providers
    pub provider_count: u64,
}

/// What a delete removed: its bytes, and per provider its tiles and bytes
/// (#576 NF-W4T-3), so the running totals move by exactly the rows deleted.
#[derive(Default)]
struct Freed {
    bytes: u64,
    by_provider: HashMap<String, (u64, u64)>,
}

impl Freed {
    fn note(&mut self, provider: String, size: u64) {
        self.bytes += size;
        let entry = self.by_provider.entry(provider).or_default();
        entry.0 += 1;
        entry.1 += size;
    }

    fn absorb(&mut self, other: Freed) {
        self.bytes += other.bytes;
        for (provider, (tiles, bytes)) in other.by_provider {
            let entry = self.by_provider.entry(provider).or_default();
            entry.0 += tiles;
            entry.1 += bytes;
        }
    }
}

/// SQLite-based tile cache with LRU eviction
pub struct TileCache {
    conn: Connection,
    /// Maximum cache size in bytes
    max_bytes: u64,
    /// Bytes of tile data on disk, kept in step with every insert and delete
    /// this process makes (#576 B19).
    ///
    /// `put` needs this figure on every write, and asking SQLite for it is a
    /// pass over the whole table that walks every row's BLOB overflow chain
    /// (`size_bytes` is stored after `data`) - all under the one mutex every map
    /// tile request waits on. So it is summed once at open, moved by the bytes
    /// each committed write adds or frees, and re-summed after any failed
    /// write, whose rolled-back deltas cannot be trusted. A second process
    /// writing the same file (dev worktrees share one app-data dir) makes it
    /// drift until the next open; the cache is disposable and the app runs
    /// single-instance.
    total_bytes: u64,
    /// Per provider, its tiles and bytes on disk (#576 NF-W4T-3): kept the same way
    /// as `total_bytes`, so the Settings cache panel reads them instead of a
    /// `GROUP BY` pass over every BLOB under the mutex tile requests wait on.
    provider_totals: HashMap<String, (u64, u64)>,
    /// Default time-to-live in seconds, for providers with no override
    ttl_seconds: i64,
    /// Per-provider TTL overrides, in seconds
    provider_ttl_seconds: HashMap<String, i64>,
}

impl TileCache {
    /// Open or create a tile cache at the given directory.
    ///
    /// # Arguments
    /// * `cache_dir` - Directory to store the cache database
    /// * `max_mb` - Maximum cache size in megabytes
    /// * `ttl_days` - Time-to-live for cached tiles in days
    ///
    /// Note: `PRAGMA auto_vacuum = INCREMENTAL` only takes effect on newly created databases.
    /// For existing databases, the vacuum mode is already set.
    pub fn open(cache_dir: &Path, max_mb: u32, ttl_days: u32) -> Result<Self> {
        // Ensure cache directory exists
        std::fs::create_dir_all(cache_dir).context("Failed to create cache directory")?;

        let db_path = cache_dir.join("tile_cache.db");
        let is_new = !db_path.exists();
        let conn = Connection::open(&db_path).context("Failed to open tile cache database")?;

        // Set pragmas (auto_vacuum only effective on new DBs)
        conn.execute_batch(
            "PRAGMA auto_vacuum = INCREMENTAL;
             PRAGMA journal_mode = WAL;",
        )?;

        // Run initial vacuum on new databases to apply auto_vacuum setting
        if is_new {
            let _ = conn.execute("VACUUM", []);
        }

        let mut cache = Self {
            conn,
            max_bytes: u64::from(max_mb) * 1024 * 1024,
            total_bytes: 0,
            provider_totals: HashMap::new(),
            ttl_seconds: i64::from(ttl_days) * 24 * 60 * 60,
            provider_ttl_seconds: HashMap::new(),
        };

        cache.init_schema()?;
        cache.read_totals()?;
        Ok(cache)
    }

    /// Create an in-memory cache (for testing)
    #[cfg(any(test, feature = "test-support"))]
    pub fn in_memory(max_mb: u32, ttl_days: u32) -> Result<Self> {
        let conn = Connection::open_in_memory().context("Failed to create in-memory cache")?;
        let mut cache = Self {
            conn,
            max_bytes: u64::from(max_mb) * 1024 * 1024,
            total_bytes: 0,
            provider_totals: HashMap::new(),
            ttl_seconds: i64::from(ttl_days) * 24 * 60 * 60,
            provider_ttl_seconds: HashMap::new(),
        };
        cache.init_schema()?;
        cache.read_totals()?;
        Ok(cache)
    }

    /// Initialize the database schema
    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r"
            CREATE TABLE IF NOT EXISTS tiles (
                id INTEGER PRIMARY KEY,
                provider TEXT NOT NULL,
                z INTEGER NOT NULL,
                y INTEGER NOT NULL,
                x INTEGER NOT NULL,
                data BLOB NOT NULL,
                content_type TEXT NOT NULL DEFAULT 'image/jpeg',
                size_bytes INTEGER NOT NULL,
                fetched_at INTEGER NOT NULL,
                accessed_at INTEGER NOT NULL,
                UNIQUE(provider, z, y, x)
            );
            CREATE INDEX IF NOT EXISTS idx_tiles_accessed ON tiles(accessed_at);
            -- Retention runs `WHERE provider = ? AND fetched_at <= ?` on every
            -- put; without this it is a full scan of a cache up to 500 MB of
            -- BLOBs. Cheap next to the rows it covers.
            CREATE INDEX IF NOT EXISTS idx_tiles_provider_fetched
                ON tiles(provider, fetched_at);
            -- ...and the far commoner case, no per-provider override at all,
            -- runs `WHERE fetched_at <= ?` (also the NOT IN branch beside
            -- overrides). `provider` leads the index above, so that was a full
            -- scan per put until this one existed (#576 B19).
            CREATE INDEX IF NOT EXISTS idx_tiles_fetched ON tiles(fetched_at);
            ",
        )?;

        // Added after the table shipped, so existing caches need the columns
        // bolted on. SQLite has no `ADD COLUMN IF NOT EXISTS`, hence the probe.
        //
        // Both are nullable with no default, so every pre-upgrade row carries
        // NULL - which `get` reads as "no recorded grant" and therefore never
        // serves as fresh (`expires_at IS NOT NULL AND expires_at > ?`). Those
        // rows are reachable only through `get_stale`, i.e. after a network
        // attempt has already failed. In practice that means a one-time
        // re-fetch of whatever was on disk, in full, since a legacy row has no
        // `etag` to revalidate with either.
        //
        // That is deliberate rather than an oversight: a row cached before this
        // column existed has no record of what the provider actually permitted,
        // and the module's rule is that silence is not consent. Backfilling a
        // grant we never received would be inventing one.
        //
        // The blast radius is small: Esri shipped with caching OFF, so an
        // upgrading install almost certainly has no legacy rows at all.
        let existing: Vec<String> = {
            let mut stmt = self.conn.prepare("PRAGMA table_info(tiles)")?;
            let names = stmt
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?;
            names
        };
        if !existing.iter().any(|c| c == "expires_at") {
            self.conn
                .execute("ALTER TABLE tiles ADD COLUMN expires_at INTEGER", [])?;
        }
        if !existing.iter().any(|c| c == "etag") {
            self.conn
                .execute("ALTER TABLE tiles ADD COLUMN etag TEXT", [])?;
        }
        // `no-cache` rows must never answer a failed request (RFC 9111 5.2.2.4),
        // and on disk they are otherwise indistinguishable from a grant that
        // simply ran out. Rows written before this column existed default to 0:
        // they were stored under a plain `max-age`, since `no-cache` was mapped
        // to the same shape and is not something Esri sends.
        if !existing.iter().any(|c| c == "no_cache") {
            self.conn.execute(
                "ALTER TABLE tiles ADD COLUMN no_cache INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }

        Ok(())
    }

    /// Replace the per-provider TTL overrides.
    ///
    /// Separate from the constructor so the existing `open`/`in_memory`
    /// signatures (and their call sites) stay put, and so a config reload can
    /// re-apply them without rebuilding the cache.
    pub fn set_provider_ttls(&mut self, ttl_days_by_provider: &HashMap<String, u32>) {
        self.provider_ttl_seconds = ttl_days_by_provider
            .iter()
            .map(|(provider, days)| (provider.clone(), i64::from(*days) * 24 * 60 * 60))
            .collect();
    }

    /// Apply new size and retention settings to the open cache (#576 A18(g)):
    /// the settings' size cap, default TTL and per-provider TTL overrides.
    /// Returns whether anything changed. When something did, tiles past the new
    /// retention and anything over the new cap are dropped now (`evict_lru`),
    /// rather than at the next write - a lowered cap or TTL is in force at once.
    pub fn reconfigure(
        &mut self,
        max_mb: u32,
        ttl_days: u32,
        ttl_days_by_provider: &HashMap<String, u32>,
    ) -> Result<bool> {
        let max_bytes = u64::from(max_mb) * 1024 * 1024;
        let ttl_seconds = i64::from(ttl_days) * 24 * 60 * 60;
        let provider_ttl_seconds: HashMap<String, i64> = ttl_days_by_provider
            .iter()
            .map(|(provider, days)| (provider.clone(), i64::from(*days) * 24 * 60 * 60))
            .collect();
        if max_bytes == self.max_bytes
            && ttl_seconds == self.ttl_seconds
            && provider_ttl_seconds == self.provider_ttl_seconds
        {
            return Ok(false);
        }
        self.max_bytes = max_bytes;
        self.ttl_seconds = ttl_seconds;
        self.provider_ttl_seconds = provider_ttl_seconds;
        self.evict_lru()?;
        Ok(true)
    }

    /// The size cap in bytes, for a caller checking which settings are in force.
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// Retention TTL for a provider, in seconds - its override, else the default.
    fn ttl_for(&self, provider: &str) -> i64 {
        self.provider_ttl_seconds
            .get(provider)
            .copied()
            .unwrap_or(self.ttl_seconds)
    }

    /// Drop everything past retention, now, rather than waiting for a write.
    ///
    /// Eviction otherwise only runs on `put`, so a session that never caches a
    /// tile - offline, or imagery switched off - leaves retired rows on disk for
    /// as long as it lasts. `get_stale` already refuses to serve them, but
    /// "kept for `ttl_days`" should mean the bytes are gone too, not merely
    /// unreachable.
    ///
    /// Call AFTER `set_provider_ttls`: with the overrides unset every provider
    /// would be swept against the default TTL, retiring tiles early for any
    /// provider configured to keep them longer.
    pub fn purge_retired(&mut self) -> Result<u64> {
        let freed = match self.delete_retired() {
            Ok(freed) => freed,
            Err(e) => {
                self.resync_total_bytes();
                return Err(e.into());
            }
        };
        self.book_freed(&freed);
        let freed = freed.bytes;
        if freed > 0 {
            tracing::debug!(
                freed_bytes = freed,
                "Dropped tiles past their retention TTL at startup"
            );
        }
        Ok(freed)
    }

    /// Delete every tile past its own provider's retention TTL, returning the
    /// bytes freed. Leaves `total_bytes` to the caller, which books the figure
    /// only once its own transaction (if any) has committed.
    ///
    /// One statement per overridden provider plus one for everything else,
    /// rather than a single global cutoff: with per-provider TTLs a single cutoff
    /// would either evict a provider's tiles early or keep another's past its
    /// configured life. Retention only - `expires_at` is not consulted, so tiles
    /// past the provider's max-age survive here to serve `get_stale`.
    ///
    /// The bytes come back through `RETURNING` from the very rows deleted, so
    /// the figure cannot drift from the deletion and costs no extra pass; each
    /// statement walks an index (`idx_tiles_provider_fetched` or
    /// `idx_tiles_fetched`) and touches only the rows it retires. `put` runs
    /// this on every tile write.
    fn delete_retired(&self) -> rusqlite::Result<Freed> {
        let now = Self::now();
        let mut freed_bytes = Freed::default();

        for (provider, ttl) in &self.provider_ttl_seconds {
            freed_bytes.absorb(self.delete_returning_bytes(
                "DELETE FROM tiles WHERE provider = ?1 AND fetched_at <= ?2
                 RETURNING provider, size_bytes",
                params![provider, now - ttl],
            )?);
        }

        // Everything with no override, against the default TTL. Building the
        // NOT IN list from the same map keeps the two branches disjoint.
        let overridden: Vec<&String> = self.provider_ttl_seconds.keys().collect();
        let cutoff = now - self.ttl_seconds;
        if overridden.is_empty() {
            freed_bytes.absorb(self.delete_returning_bytes(
                "DELETE FROM tiles WHERE fetched_at <= ?1 RETURNING provider, size_bytes",
                params![cutoff],
            )?);
        } else {
            let placeholders = vec!["?"; overridden.len()].join(",");
            let mut args: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(overridden.len() + 1);
            args.push(&cutoff);
            for provider in &overridden {
                args.push(provider);
            }
            freed_bytes.absorb(self.delete_returning_bytes(
                &format!(
                    "DELETE FROM tiles WHERE fetched_at <= ?1 AND provider NOT IN ({placeholders})
                     RETURNING provider, size_bytes"
                ),
                args.as_slice(),
            )?);
        }

        Ok(freed_bytes)
    }

    /// `SUM(size_bytes)` over the whole table: the tests' independent oracle
    /// for `total_bytes`. Production reads it off [`Self::read_totals`].
    #[cfg(test)]
    fn sum_size_bytes(&self) -> rusqlite::Result<u64> {
        self.conn
            .query_row(
                "SELECT COALESCE(SUM(size_bytes), 0) FROM tiles",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|bytes| bytes.max(0) as u64)
    }

    /// Re-read the running totals from disk after a write failed: its
    /// transaction rolled back (or half-ran), so the in-memory deltas cannot be
    /// trusted.
    fn resync_total_bytes(&mut self) {
        if let Err(e) = self.read_totals() {
            tracing::warn!(error = %e, "Could not re-read the tile cache size");
        }
    }

    /// Read `provider_totals` from the table and derive `total_bytes` from
    /// it: one full pass, at open and to resync. `total_bytes` is exactly the
    /// sum of the per-provider byte column, so a separate `SUM` pass over every
    /// BLOB would only repeat the walk (#576 RV4-R-NITS T-2).
    fn read_totals(&mut self) -> rusqlite::Result<()> {
        let totals = self.sum_by_provider()?;
        self.total_bytes = totals
            .values()
            .fold(0u64, |sum, (_, bytes)| sum.saturating_add(*bytes));
        self.provider_totals = totals;
        Ok(())
    }

    /// Tiles and bytes per provider, from the table: the full pass
    /// `provider_totals` and `total_bytes` exist to keep off the panel and the
    /// write path. Run by [`Self::read_totals`] only.
    fn sum_by_provider(&self) -> rusqlite::Result<HashMap<String, (u64, u64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT provider, COUNT(*), COALESCE(SUM(size_bytes), 0) FROM tiles GROUP BY provider",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (
                    row.get::<_, i64>(1)?.max(0) as u64,
                    row.get::<_, i64>(2)?.max(0) as u64,
                ),
            ))
        })?;
        rows.collect()
    }

    /// Book what a committed delete freed off the running totals.
    fn book_freed(&mut self, freed: &Freed) {
        self.total_bytes = self.total_bytes.saturating_sub(freed.bytes);
        for (provider, (tiles, bytes)) in &freed.by_provider {
            if let Some(entry) = self.provider_totals.get_mut(provider) {
                entry.0 = entry.0.saturating_sub(*tiles);
                entry.1 = entry.1.saturating_sub(*bytes);
                if entry.0 == 0 {
                    self.provider_totals.remove(provider);
                }
            }
        }
    }

    /// Run a `DELETE ... RETURNING provider, size_bytes` and add up what it removed.
    fn delete_returning_bytes(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> rusqlite::Result<Freed> {
        let mut stmt = self.conn.prepare_cached(sql)?;
        let mut rows = stmt.query(params)?;
        let mut freed = Freed::default();
        while let Some(row) = rows.next()? {
            freed.note(
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?.max(0) as u64,
            );
        }
        Ok(freed)
    }

    /// Drop one tile and book its bytes off `total_bytes`. Returns the bytes
    /// freed (0 when there was no such tile).
    fn delete_tile(&mut self, provider: &str, z: u32, y: u32, x: u32) -> Result<u64> {
        match self.delete_returning_bytes(
            "DELETE FROM tiles WHERE provider = ?1 AND z = ?2 AND y = ?3 AND x = ?4
             RETURNING provider, size_bytes",
            params![provider, z, y, x],
        ) {
            Ok(freed) => {
                self.book_freed(&freed);
                Ok(freed.bytes)
            }
            Err(e) => {
                self.resync_total_bytes();
                Err(e.into())
            }
        }
    }

    /// Get the current Unix timestamp
    pub(crate) fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    /// Get a tile from the cache, if it may still be served *without* asking the
    /// provider.
    ///
    /// Fresh means inside BOTH deadlines (see the module docs): the configured
    /// retention TTL, and the provider's own `Cache-Control: max-age` recorded in
    /// `expires_at`. A tile past `max-age` is not returned here even though it is
    /// still on disk - `get_stale` is the path that reaches it, and only when the
    /// network has already failed.
    ///
    /// Updates the `accessed_at` timestamp on cache hit.
    ///
    /// Note: Returns `None` on DB errors (logged at warn level).
    pub fn get(&mut self, provider: &str, z: u32, y: u32, x: u32) -> Option<TileEntry> {
        let now = Self::now();
        // A tile is fresh if it was fetched within the last ttl_seconds
        // Use strict < for staleness: tile is stale when fetched_at <= now - ttl_seconds
        let min_fetched_at = now - self.ttl_for(provider);

        // Update accessed_at first and return tile data in one query
        // This ensures the returned accessed_at is current
        let result: Result<TileEntry, rusqlite::Error> = self.conn.query_row(
            "UPDATE tiles SET accessed_at = ?1
             WHERE provider = ?2 AND z = ?3 AND y = ?4 AND x = ?5
               AND fetched_at > ?6
               AND expires_at IS NOT NULL AND expires_at > ?1
             RETURNING data, content_type, size_bytes, fetched_at, accessed_at, expires_at, etag",
            params![now, provider, z, y, x, min_fetched_at],
            |row| {
                Ok(TileEntry {
                    data: row.get(0)?,
                    content_type: row.get(1)?,
                    size_bytes: row.get::<_, i64>(2)? as u64,
                    fetched_at: row.get(3)?,
                    accessed_at: row.get(4)?,
                    expires_at: row.get(5)?,
                    etag: row.get(6)?,
                    no_cache: false,
                })
            },
        );

        match result {
            Ok(entry) => Some(entry),
            Err(rusqlite::Error::QueryReturnedNoRows) => None, // Cache miss
            Err(e) => {
                // Log actual DB errors but don't propagate - treat as miss
                tracing::warn!(
                    provider,
                    z,
                    y,
                    x,
                    error = %e,
                    "Database error during tile cache lookup"
                );
                None
            }
        }
    }

    /// Get a stale tile from the cache - one `get` declined to serve.
    ///
    /// This answers a request the provider could not, reached only after a fetch
    /// has already failed. It is bounded by BOTH clocks, in opposite directions:
    /// the tile must still be within retention (or it should not be on disk at
    /// all), and must NOT be fresh (or `get` would have served it).
    ///
    /// Retention has to be enforced here and not left to eviction. Retired rows
    /// are swept on `put`, but a fully offline session never writes a tile, and
    /// offline is precisely when this lookup runs - so without this bound a
    /// stored copy could outlive `ttl_days` by however long the session lasts.
    /// The `expires_at IS NULL` arm covers rows cached before the column
    /// existed: never servable as fresh, still usable as a last resort while
    /// they are inside retention.
    ///
    /// A `no-cache` row IS returned, with `TileEntry::no_cache` set. It may be
    /// revalidated - that is what its ETag is for - and a successful `304` may
    /// then serve it. What it may not do is answer a FAILED revalidation, so the
    /// caller checks the flag before using it as a stale-if-error fallback.
    /// Filtering it out here instead removed the validator along with it, which
    /// turned every request for such a tile into a full download.
    ///
    /// Does NOT update `accessed_at` since we're serving stale data.
    ///
    /// Returns `None` if the tile is not cached at all (fresh or stale).
    pub fn get_stale(&self, provider: &str, z: u32, y: u32, x: u32) -> Option<TileEntry> {
        let now = Self::now();
        let min_fetched_at = now - self.ttl_for(provider);

        // Inside retention, and not fresh. `no-cache` rows are INCLUDED, with
        // the flag carried out on the entry - see the note above.
        let result: Result<TileEntry, rusqlite::Error> = self.conn.query_row(
            "SELECT data, content_type, size_bytes, fetched_at, accessed_at, expires_at, etag, no_cache FROM tiles
             WHERE provider = ?1 AND z = ?2 AND y = ?3 AND x = ?4
               AND fetched_at > ?5
               AND (expires_at IS NULL OR expires_at <= ?6)",
            params![provider, z, y, x, min_fetched_at, now],
            |row| {
                Ok(TileEntry {
                    data: row.get(0)?,
                    content_type: row.get(1)?,
                    size_bytes: row.get::<_, i64>(2)? as u64,
                    fetched_at: row.get(3)?,
                    accessed_at: row.get(4)?,
                    expires_at: row.get(5)?,
                    etag: row.get(6)?,
                    no_cache: row.get::<_, i64>(7)? != 0,
                })
            },
        );

        match result {
            Ok(entry) => Some(entry),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => {
                tracing::warn!(
                    provider,
                    z,
                    y,
                    x,
                    error = %e,
                    "Database error during stale tile lookup"
                );
                None
            }
        }
    }

    /// Store a tile in the cache.
    ///
    /// If storing this tile would exceed the cache size limit, expired tiles are removed
    /// first, then LRU eviction is performed until there's space. Tiles larger than
    /// max_bytes are rejected.
    pub fn put(
        &mut self,
        provider: &str,
        z: u32,
        y: u32,
        x: u32,
        data: &[u8],
        content_type: &str,
        policy: CachePolicy,
        etag: Option<&str>,
    ) -> Result<()> {
        let now = Self::now();
        let new_size = data.len() as u64;

        // No grant, no disk. `no-store` is an explicit refusal; a missing
        // `max-age` is simply silence, and this cache does not read silence as
        // consent. Either way any copy we already hold goes with it - a provider
        // that stops granting should not leave us holding yesterday's grant.
        if policy.malformed {
            // We could not read the header, so we learned nothing - not that
            // permission was withdrawn. Skip the write, and leave any existing
            // row to age out on the deadline it already recorded.
            tracing::debug!(
                provider,
                z,
                y,
                x,
                "Unreadable Cache-Control; not caching, and not dropping what is held"
            );
            return Ok(());
        }

        let Some(max_age) = policy.granted_seconds() else {
            let dropped_bytes = self.delete_tile(provider, z, y, x)?;
            // Worth a line: if a provider stops sending Cache-Control (a CDN
            // change, a proxy stripping headers) caching turns off AND the
            // existing cache drains, and the caller only logs on Err.
            tracing::debug!(
                provider,
                z,
                y,
                x,
                dropped_bytes,
                "No storage grant on this response; not cached"
            );
            return Ok(());
        };

        let expires_at = now.saturating_add(max_age);

        // Reject tiles larger than the entire cache
        if new_size > self.max_bytes {
            anyhow::bail!(
                "Tile size ({} bytes) exceeds max cache size ({} bytes)",
                new_size,
                self.max_bytes
            );
        }

        match self.store(
            provider,
            z,
            y,
            x,
            data,
            content_type,
            now,
            expires_at,
            etag,
            policy.no_cache,
        ) {
            Ok(freed) => {
                self.book_freed(&freed);
                self.total_bytes += new_size;
                let entry = self.provider_totals.entry(provider.to_owned()).or_default();
                entry.0 += 1;
                entry.1 += new_size;
                Ok(())
            }
            Err(e) => {
                self.resync_total_bytes();
                Err(e.into())
            }
        }
    }

    /// The write half of `put`, as ONE transaction: retire, drop the copy being
    /// replaced, evict, insert. Returns the bytes it freed; `put` books them
    /// (and the new tile) against `total_bytes` only once this has committed.
    ///
    /// No step reads more of the table than it changes: retention walks an
    /// index to the rows it retires, the replaced copy is a unique-key lookup,
    /// and eviction is one ordered walk that stops at the bytes it needs. It
    /// used to re-run COUNT/SUM over the whole table once per put and again
    /// per evicted tile (#576 B19), each its own autocommit transaction.
    fn store(
        &self,
        provider: &str,
        z: u32,
        y: u32,
        x: u32,
        data: &[u8],
        content_type: &str,
        now: i64,
        expires_at: i64,
        etag: Option<&str>,
        no_cache: bool,
    ) -> rusqlite::Result<Freed> {
        let tx = self.conn.unchecked_transaction()?;

        // Retention first, so nothing past its TTL is kept at the expense of a
        // live tile.
        let mut freed = self.delete_retired()?;

        // The copy being replaced leaves BEFORE eviction is sized, so the new
        // tile needs room for all of its bytes. Crediting the old copy's bytes
        // instead ("space needed = new - old") counted them twice whenever the
        // LRU eviction below picked that same row (#576 B35(j)).
        freed.absorb(self.delete_returning_bytes(
            "DELETE FROM tiles WHERE provider = ?1 AND z = ?2 AND y = ?3 AND x = ?4
             RETURNING provider, size_bytes",
            params![provider, z, y, x],
        )?);

        let held = self.total_bytes.saturating_sub(freed.bytes);
        let over = (held + data.len() as u64).saturating_sub(self.max_bytes);
        freed.absorb(self.evict_oldest(over)?);

        // A plain INSERT, not OR REPLACE: the row it would replace is gone, and
        // a REPLACE here would drop bytes `total_bytes` never hears about.
        self.conn.execute(
            "INSERT INTO tiles (provider, z, y, x, data, content_type, size_bytes, fetched_at, accessed_at, expires_at, etag, no_cache)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9, ?10, ?11)",
            params![
                provider,
                z,
                y,
                x,
                data,
                content_type,
                data.len() as i64,
                now,
                Some(expires_at),
                etag,
                i64::from(no_cache)
            ],
        )?;

        tx.commit()?;
        Ok(freed)
    }

    /// Re-stamp a tile the provider just confirmed is still current (a `304`).
    ///
    /// Moves both clocks forward without touching the bytes: `expires_at` from
    /// the freshly-returned directives, and `fetched_at` because the copy WAS
    /// just validated - it is current imagery, not an old file being kept alive.
    /// That is what lets a revalidating client hold a tile indefinitely while
    /// the provider keeps saying it has not changed.
    ///
    /// Returns whether a row was actually updated.
    pub fn refresh_validated(
        &mut self,
        provider: &str,
        z: u32,
        y: u32,
        x: u32,
        policy: CachePolicy,
    ) -> Result<bool> {
        let now = Self::now();

        // `no-store` on a revalidation is an explicit revocation: drop the copy
        // rather than extend a grant the provider just withdrew.
        if policy.no_store {
            self.delete_tile(provider, z, y, x)?;
            return Ok(false);
        }

        // A 304 that carries NO directives is a different case, and deleting on
        // it was a bug: RFC 9111 4.3.4 makes a 304 an UPDATE of the stored
        // headers, so silence leaves the grant we already recorded standing.
        // Deleting threw away the bytes we had just spent a conditional request
        // to avoid re-fetching - along with the etag, forcing a full download
        // next time and leaving nothing to answer a failed fetch in between.
        //
        // Retention is re-stamped because the content WAS confirmed current;
        // freshness is not, because no new grant arrived. The tile stays
        // stale-but-retained and revalidates again next request. Rows with no
        // recorded grant at all (pre-migration) are left alone, so silence can
        // never keep a grantless row alive indefinitely.
        //
        // A malformed header lands here too, and correctly: unreadable is not
        // revocation either, and `granted_seconds` already returns `None` for
        // it. Nothing is deleted on this path.
        let Some(max_age) = policy.granted_seconds() else {
            let updated = self.conn.execute(
                "UPDATE tiles SET fetched_at = ?1, accessed_at = ?1
                 WHERE provider = ?2 AND z = ?3 AND y = ?4 AND x = ?5
                   AND expires_at IS NOT NULL",
                params![now, provider, z, y, x],
            )?;
            return Ok(updated > 0);
        };

        let expires_at = now.saturating_add(max_age);
        // `no_cache` is re-stamped from THIS response, not left as it was: the
        // 304 restates the directives, so a provider that has stopped sending
        // `no-cache` releases the row from the stale-path exclusion, and one
        // that has started sending it imposes the exclusion on a row already
        // held.
        let updated = self.conn.execute(
            "UPDATE tiles SET fetched_at = ?1, accessed_at = ?1, expires_at = ?2, no_cache = ?3
             WHERE provider = ?4 AND z = ?5 AND y = ?6 AND x = ?7",
            params![
                now,
                Some(expires_at),
                i64::from(policy.no_cache),
                provider,
                z,
                y,
                x
            ],
        )?;
        Ok(updated > 0)
    }

    /// Push a tile past its provider `max-age` without touching retention.
    /// Lets a test reach the revalidation window without sleeping through it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn expire_max_age_for_test(
        &mut self,
        provider: &str,
        z: u32,
        y: u32,
        x: u32,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE tiles SET expires_at = ?1
                 WHERE provider = ?2 AND z = ?3 AND y = ?4 AND x = ?5",
            params![Self::now() - 1, provider, z, y, x],
        )?;
        Ok(())
    }

    /// Push a stored tile past its `max-age` without touching retention.
    ///
    /// The state every stale-if-error test needs: not servable as fresh, still
    /// retained. Winding `fetched_at` back instead would cross the retention
    /// bound as well, which is a different case entirely (and, since
    /// `get_stale` began enforcing retention, one where nothing is served).
    #[cfg(any(test, feature = "test-support"))]
    pub fn expire_for_test(&self, provider: &str, z: u32, y: u32, x: u32) -> Result<()> {
        self.conn.execute(
            "UPDATE tiles SET expires_at = ?1
             WHERE provider = ?2 AND z = ?3 AND y = ?4 AND x = ?5",
            params![Self::now() - 1, provider, z, y, x],
        )?;
        Ok(())
    }

    /// `put` with a generous grant already supplied, for the tests that predate
    /// provider cache directives.
    ///
    /// They exercise LRU, sizing, retention and provider isolation - none of
    /// which the directives touch - so they need a tile that actually lands on
    /// disk. Since the cache stores only on an explicit grant, "no directives"
    /// is no longer a way to store anything; the gate itself has its own tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn put_granted(
        &mut self,
        provider: &str,
        z: u32,
        y: u32,
        x: u32,
        data: &[u8],
        content_type: &str,
    ) -> Result<()> {
        self.put(
            provider,
            z,
            y,
            x,
            data,
            content_type,
            CachePolicy {
                max_age_seconds: Some(30 * 24 * 60 * 60),
                no_store: false,
                age_seconds: None,
                no_cache: false,
                malformed: false,
            },
            None,
        )
    }

    /// Delete least-recently-used tiles, oldest first (id breaks ties, so the
    /// order is deterministic), until at least `bytes_to_free` are gone.
    /// Returns the bytes freed, which may fall short only when the table runs
    /// out. Leaves `total_bytes` to the caller.
    ///
    /// One ordered walk down `idx_tiles_accessed` that stops as soon as it has
    /// enough, so it reads the rows it deletes and no others.
    fn evict_oldest(&self, bytes_to_free: u64) -> rusqlite::Result<Freed> {
        let mut freed = Freed::default();
        if bytes_to_free == 0 {
            return Ok(freed);
        }
        let mut victims = Vec::new();
        {
            let mut stmt = self.conn.prepare_cached(
                "SELECT id, provider, size_bytes FROM tiles ORDER BY accessed_at ASC, id ASC",
            )?;
            let mut rows = stmt.query([])?;
            while freed.bytes < bytes_to_free {
                let Some(row) = rows.next()? else {
                    break;
                };
                victims.push(row.get::<_, i64>(0)?);
                freed.note(
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?.max(0) as u64,
                );
            }
        }
        let mut delete = self
            .conn
            .prepare_cached("DELETE FROM tiles WHERE id = ?1")?;
        for id in victims {
            delete.execute(params![id])?;
        }
        Ok(freed)
    }

    /// Evict least-recently-used tiles until cache is under the size limit.
    /// Also removes any expired tiles (TTL exceeded).
    ///
    /// Returns the number of bytes freed.
    pub fn evict_lru(&mut self) -> Result<u64> {
        match self.retire_and_evict_to_cap() {
            Ok(freed) => {
                self.book_freed(&freed);
                let freed = freed.bytes;
                // Run incremental vacuum to reclaim space if we freed anything
                if freed > 0 {
                    let _ = self.conn.execute("PRAGMA incremental_vacuum(100)", []);
                }
                Ok(freed)
            }
            Err(e) => {
                self.resync_total_bytes();
                Err(e.into())
            }
        }
    }

    /// `evict_lru`'s write half, as one transaction.
    fn retire_and_evict_to_cap(&self) -> rusqlite::Result<Freed> {
        let tx = self.conn.unchecked_transaction()?;
        let mut freed = self.delete_retired()?;
        let over = self
            .total_bytes
            .saturating_sub(freed.bytes)
            .saturating_sub(self.max_bytes);
        freed.absorb(self.evict_oldest(over)?);
        tx.commit()?;
        Ok(freed)
    }

    /// Clear all tiles from the cache.
    ///
    /// Uses incremental vacuum instead of full VACUUM to avoid blocking
    /// the database connection for extended periods on large caches.
    pub fn clear_all(&mut self) -> Result<()> {
        if let Err(e) = self.conn.execute("DELETE FROM tiles", []) {
            self.resync_total_bytes();
            return Err(e.into());
        }
        self.total_bytes = 0;
        self.provider_totals.clear();
        // Use incremental vacuum (non-blocking) instead of full VACUUM
        // to prevent UI freezes when clearing large caches
        let _ = self.conn.execute("PRAGMA incremental_vacuum(100)", []);
        Ok(())
    }

    /// Clear tiles from a specific provider.
    pub fn clear_provider(&mut self, provider: &str) -> Result<()> {
        match self.delete_returning_bytes(
            "DELETE FROM tiles WHERE provider = ?1 RETURNING provider, size_bytes",
            params![provider],
        ) {
            Ok(freed) => self.book_freed(&freed),
            Err(e) => {
                self.resync_total_bytes();
                return Err(e.into());
            }
        }
        let _ = self.conn.execute("PRAGMA incremental_vacuum(100)", []);
        Ok(())
    }

    /// Get cache statistics, read from the table itself.
    ///
    /// Three passes over the whole table, so nothing on the write path calls
    /// it; `put` sizes eviction off the running `total_bytes` instead.
    pub fn get_stats(&self) -> CacheStats {
        let tile_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM tiles", [], |row| row.get(0))
            .unwrap_or(0);

        let total_bytes: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(size_bytes), 0) FROM tiles",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let provider_count: i64 = self
            .conn
            .query_row("SELECT COUNT(DISTINCT provider) FROM tiles", [], |row| {
                row.get(0)
            })
            .unwrap_or(0);

        CacheStats {
            tile_count: tile_count as u64,
            total_bytes: total_bytes as u64,
            provider_count: provider_count as u64,
        }
    }

    /// Get per-provider statistics.
    ///
    /// Returns a map of provider name to (tile_count, total_bytes) - the running
    /// totals (#576 NF-W4T-3), so opening the Settings cache panel no longer walks
    /// the whole table under the mutex every tile request waits on.
    pub fn get_provider_stats(&self) -> HashMap<String, (u64, u64)> {
        self.provider_totals.clone()
    }

    /// Check if a tile exists and is fresh (not expired).
    ///
    /// Returns `false` on DB errors (logs at warn level).
    pub fn has_fresh_tile(&self, provider: &str, z: u32, y: u32, x: u32) -> bool {
        let now = Self::now();
        let min_fetched_at = now - self.ttl_for(provider);

        // Mirrors `get`'s predicate exactly - a divergence here would report a
        // tile as servable that `get` then declines to serve.
        match self.conn.query_row(
            "SELECT COUNT(*) FROM tiles
             WHERE provider = ?1 AND z = ?2 AND y = ?3 AND x = ?4
               AND fetched_at > ?5
               AND expires_at IS NOT NULL AND expires_at > ?6",
            params![provider, z, y, x, min_fetched_at, now],
            |row| row.get::<_, i64>(0),
        ) {
            Ok(count) => count > 0,
            Err(e) => {
                tracing::warn!(
                    provider,
                    z,
                    y,
                    x,
                    error = %e,
                    "Database error checking fresh tile"
                );
                false
            }
        }
    }

    /// Check if a tile exists but is stale (expired TTL).
    ///
    /// Returns `false` on DB errors (logs at warn level).
    pub fn has_stale_tile(&self, provider: &str, z: u32, y: u32, x: u32) -> bool {
        let now = Self::now();
        let min_fetched_at = now - self.ttl_for(provider);

        // "May a stored copy answer a FAILED fetch?" - so unlike `get_stale`,
        // which also feeds the validator lookup, this one does exclude
        // `no-cache`. The pair is deliberately not symmetric; the asymmetry is
        // the point.
        match self.conn.query_row(
            "SELECT COUNT(*) FROM tiles
             WHERE provider = ?1 AND z = ?2 AND y = ?3 AND x = ?4
               AND fetched_at > ?5
               AND (expires_at IS NULL OR expires_at <= ?6)
               AND no_cache = 0",
            params![provider, z, y, x, min_fetched_at, now],
            |row| row.get::<_, i64>(0),
        ) {
            Ok(count) => count > 0,
            Err(e) => {
                tracing::warn!(
                    provider,
                    z,
                    y,
                    x,
                    error = %e,
                    "Database error checking stale tile"
                );
                false
            }
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod accounting_tests;

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    clippy::cast_sign_loss,
    reason = "test module: panics on a failed assumption are the point, the printed values \
              are the evidence a run captures, and SQLite counts are never negative"
)]
mod provider_totals_tests;

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod totals_rv4_tests;

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::thread;
    // Helper to create test tile data
    fn make_tile_data(size: usize) -> Vec<u8> {
        vec![0xFFu8; size]
    }

    // Helper to set accessed_at directly in DB for deterministic LRU testing
    fn set_accessed_at(
        cache: &TileCache,
        provider: &str,
        z: u32,
        y: u32,
        x: u32,
        accessed_at: i64,
    ) {
        cache
            .conn
            .execute(
                "UPDATE tiles SET accessed_at = ?1 WHERE provider = ?2 AND z = ?3 AND y = ?4 AND x = ?5",
                params![accessed_at, provider, z, y, x],
            )
            .unwrap();
    }

    // Helper to set fetched_at directly in DB for deterministic TTL testing
    fn set_fetched_at(cache: &TileCache, provider: &str, z: u32, y: u32, x: u32, fetched_at: i64) {
        cache
            .conn
            .execute(
                "UPDATE tiles SET fetched_at = ?1 WHERE provider = ?2 AND z = ?3 AND y = ?4 AND x = ?5",
                params![fetched_at, provider, z, y, x],
            )
            .unwrap();
    }

    // Helper to get accessed_at directly from DB
    fn get_accessed_at_from_db(cache: &TileCache, provider: &str, z: u32, y: u32, x: u32) -> i64 {
        cache
            .conn
            .query_row(
                "SELECT accessed_at FROM tiles WHERE provider = ?1 AND z = ?2 AND y = ?3 AND x = ?4",
                params![provider, z, y, x],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn drop_tiles_table(cache: &TileCache) {
        cache.conn.execute("DROP TABLE tiles", []).unwrap();
    }

    // === Test 1: put and get roundtrip ===
    #[test]
    fn test_put_get_roundtrip() {
        let mut cache = TileCache::in_memory(100, 30).unwrap();

        let tile_data = b"fake jpeg data here";
        cache
            .put_granted("esri", 10, 500, 250, tile_data, "image/jpeg")
            .unwrap();

        let entry = cache
            .get("esri", 10, 500, 250)
            .expect("Should get cached tile");
        assert_eq!(entry.data, tile_data);
        assert_eq!(entry.content_type, "image/jpeg");
        assert_eq!(entry.size_bytes, tile_data.len() as u64);
    }

    // === Test 2: cache miss returns None ===
    #[test]
    fn test_cache_miss_returns_none() {
        let mut cache = TileCache::in_memory(100, 30).unwrap();

        // Try to get a tile that was never cached
        let result = cache.get("esri", 10, 500, 250);
        assert!(result.is_none());

        // Add a different tile and confirm the original is still missing
        cache
            .put_granted("esri", 10, 500, 251, b"other tile", "image/png")
            .unwrap();
        let result = cache.get("esri", 10, 500, 250);
        assert!(result.is_none());
    }

    // === Test 3: LRU eviction (exceed max_mb, verify oldest evicted) ===
    #[test]
    fn test_lru_eviction() {
        // Create a tiny cache (1 KB)
        let mut cache = TileCache::in_memory(1, 30).unwrap();
        cache.max_bytes = 1024; // Override to 1KB for testing

        // Add tiles that will exceed capacity
        // Each tile is ~400 bytes, so 3 tiles = 1200 bytes > 1024 bytes
        let tile1 = make_tile_data(400);
        let tile2 = make_tile_data(400);
        let tile3 = make_tile_data(400);

        cache
            .put_granted("esri", 1, 0, 0, &tile1, "image/jpeg")
            .unwrap();
        cache
            .put_granted("esri", 1, 0, 1, &tile2, "image/jpeg")
            .unwrap();
        cache
            .put_granted("esri", 1, 0, 2, &tile3, "image/jpeg")
            .unwrap();

        // Set explicit timestamps to make LRU deterministic
        // tile1 is oldest, tile2 is middle, tile3 is newest
        let base_time = TileCache::now();
        set_accessed_at(&cache, "esri", 1, 0, 0, base_time - 100); // oldest
        set_accessed_at(&cache, "esri", 1, 0, 1, base_time - 50); // middle
        set_accessed_at(&cache, "esri", 1, 0, 2, base_time); // newest

        // Trigger eviction explicitly
        let _freed = cache.evict_lru().unwrap();

        // The oldest tile (tile1) should have been evicted
        let stats = cache.get_stats();
        assert!(
            stats.total_bytes <= cache.max_bytes,
            "Cache should be under limit after eviction"
        );

        // tile1 (oldest) should be gone
        assert!(
            cache.get("esri", 1, 0, 0).is_none(),
            "Oldest tile should be evicted"
        );

        // tile2 and tile3 should still exist
        assert!(
            cache.get("esri", 1, 0, 1).is_some(),
            "Middle tile should remain"
        );
        assert!(
            cache.get("esri", 1, 0, 2).is_some(),
            "Newest tile should remain"
        );
    }

    // === Test 4: TTL expiration (stale tiles not returned) ===
    #[test]
    fn test_ttl_expiration() {
        // Create cache with very short TTL
        let mut cache = TileCache::in_memory(100, 30).unwrap();
        cache.ttl_seconds = 2; // 2 second TTL

        cache
            .put_granted("esri", 10, 500, 250, b"tile data", "image/jpeg")
            .unwrap();

        // Tile should be fresh immediately after put
        let result = cache.get("esri", 10, 500, 250);
        assert!(result.is_some(), "Tile should be fresh right after put");

        // Force the tile to be stale without sleeping
        let stale_time = TileCache::now() - cache.ttl_seconds - 1;
        set_fetched_at(&cache, "esri", 10, 500, 250, stale_time);

        // Should return None because TTL has expired
        let result = cache.get("esri", 10, 500, 250);
        assert!(
            result.is_none(),
            "Stale tile should not be returned by get()"
        );

        // And past RETENTION it is not reachable as a last resort either: that
        // is the difference between the two clocks. A tile past max-age but
        // inside retention still answers a failed fetch - see
        // `a_tile_inside_retention_but_past_max_age_still_answers` - but this
        // one is past the TTL that says it may be on disk at all.
        assert!(
            !cache.has_stale_tile("esri", 10, 500, 250),
            "past ttl_days, so not servable by any path"
        );
        assert!(cache.get_stale("esri", 10, 500, 250).is_none());
    }

    // === Test 5: clear_all removes all tiles ===
    #[test]
    fn test_clear_all() {
        let mut cache = TileCache::in_memory(100, 30).unwrap();

        cache
            .put_granted("esri", 1, 0, 0, b"tile1", "image/jpeg")
            .unwrap();
        cache
            .put_granted("osm", 2, 1, 1, b"tile2", "image/png")
            .unwrap();
        cache
            .put_granted("mapbox", 3, 2, 2, b"tile3", "image/jpeg")
            .unwrap();

        let stats_before = cache.get_stats();
        assert_eq!(stats_before.tile_count, 3);

        cache.clear_all().unwrap();

        let stats_after = cache.get_stats();
        assert_eq!(stats_after.tile_count, 0);
        assert_eq!(stats_after.total_bytes, 0);
    }

    // === Test 6: clear_provider only removes specified provider ===
    #[test]
    fn test_clear_provider() {
        let mut cache = TileCache::in_memory(100, 30).unwrap();

        cache
            .put_granted("esri", 1, 0, 0, b"esri1", "image/jpeg")
            .unwrap();
        cache
            .put_granted("esri", 1, 0, 1, b"esri2", "image/jpeg")
            .unwrap();
        cache
            .put_granted("osm", 1, 0, 0, b"osm1", "image/png")
            .unwrap();

        cache.clear_provider("esri").unwrap();

        // ESRI tiles should be gone
        assert!(cache.get("esri", 1, 0, 0).is_none());
        assert!(cache.get("esri", 1, 0, 1).is_none());

        // OSM tile should remain
        assert!(cache.get("osm", 1, 0, 0).is_some());

        let stats = cache.get_stats();
        assert_eq!(stats.tile_count, 1);
    }

    // === Test 7: cache_stats returns correct counts ===
    #[test]
    fn test_cache_stats() {
        let mut cache = TileCache::in_memory(100, 30).unwrap();

        assert_eq!(cache.get_stats().tile_count, 0);
        assert_eq!(cache.get_stats().total_bytes, 0);
        assert_eq!(cache.get_stats().provider_count, 0);

        let tile1 = make_tile_data(100);
        let tile2 = make_tile_data(200);
        let tile3 = make_tile_data(150);

        cache
            .put_granted("esri", 1, 0, 0, &tile1, "image/jpeg")
            .unwrap();
        cache
            .put_granted("esri", 1, 0, 1, &tile2, "image/jpeg")
            .unwrap();
        cache
            .put_granted("osm", 1, 0, 0, &tile3, "image/png")
            .unwrap();

        let stats = cache.get_stats();
        assert_eq!(stats.tile_count, 3);
        assert_eq!(stats.total_bytes, 450);
        assert_eq!(stats.provider_count, 2);
    }

    #[test]
    fn test_get_returns_none_on_db_error() {
        let mut cache = TileCache::in_memory(100, 30).unwrap();
        drop_tiles_table(&cache);

        let entry = cache.get("esri", 1, 2, 3);
        assert!(entry.is_none());
    }

    #[test]
    fn test_get_stale_returns_none_on_db_error() {
        let cache = TileCache::in_memory(100, 30).unwrap();
        drop_tiles_table(&cache);

        let entry = cache.get_stale("esri", 1, 2, 3);
        assert!(entry.is_none());
    }

    #[test]
    fn test_has_fresh_tile_returns_false_on_db_error() {
        let cache = TileCache::in_memory(100, 30).unwrap();
        drop_tiles_table(&cache);

        assert!(!cache.has_fresh_tile("esri", 1, 2, 3));
    }

    #[test]
    fn test_has_stale_tile_returns_false_on_db_error() {
        let cache = TileCache::in_memory(100, 30).unwrap();
        drop_tiles_table(&cache);

        assert!(!cache.has_stale_tile("esri", 1, 2, 3));
    }

    #[test]
    fn test_get_provider_stats_returns_empty_on_db_error() {
        let cache = TileCache::in_memory(100, 30).unwrap();
        drop_tiles_table(&cache);

        let stats = cache.get_provider_stats();
        assert!(stats.is_empty());
    }

    #[test]
    fn test_evict_lru_returns_error_when_table_missing() {
        let mut cache = TileCache::in_memory(100, 30).unwrap();
        drop_tiles_table(&cache);

        let result = cache.evict_lru();
        assert!(result.is_err());
    }

    // === Test 8: concurrent access (multi-threaded) ===
    // Note: This test uses a Mutex to serialize access, primarily testing
    // that TileCache is Send. For true concurrent SQLite access, separate
    // connections would be needed.
    #[test]
    fn test_concurrent_access() {
        use std::sync::{Arc, Mutex};

        // Use a real temp directory for thread safety
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Mutex::new(
            TileCache::open(temp_dir.path(), 100, 30).unwrap(),
        ));

        let mut handles = vec![];

        // Spawn multiple threads writing different tiles
        for i in 0..10 {
            let cache_clone = Arc::clone(&cache);
            let handle = thread::spawn(move || {
                let tile_data = make_tile_data(50);
                let mut cache = cache_clone.lock().unwrap();
                cache
                    .put_granted("esri", 10, i, 0, &tile_data, "image/jpeg")
                    .unwrap();
            });
            handles.push(handle);
        }

        // Wait for all threads
        for handle in handles {
            handle.join().unwrap();
        }

        // Verify all tiles were cached
        let cache = cache.lock().unwrap();
        let stats = cache.get_stats();
        assert_eq!(stats.tile_count, 10);
    }

    // === Test 9: different providers isolation ===
    #[test]
    fn test_provider_isolation() {
        let mut cache = TileCache::in_memory(100, 30).unwrap();

        // Same z/y/x but different providers
        cache
            .put_granted("esri", 10, 500, 250, b"esri data", "image/jpeg")
            .unwrap();
        cache
            .put_granted("osm", 10, 500, 250, b"osm data", "image/png")
            .unwrap();
        cache
            .put_granted("mapbox", 10, 500, 250, b"mapbox data", "image/webp")
            .unwrap();

        let esri = cache.get("esri", 10, 500, 250).unwrap();
        let osm = cache.get("osm", 10, 500, 250).unwrap();
        let mapbox = cache.get("mapbox", 10, 500, 250).unwrap();

        assert_eq!(esri.data, b"esri data");
        assert_eq!(osm.data, b"osm data");
        assert_eq!(mapbox.data, b"mapbox data");

        assert_eq!(esri.content_type, "image/jpeg");
        assert_eq!(osm.content_type, "image/png");
        assert_eq!(mapbox.content_type, "image/webp");
    }

    // === Test 10: zero-byte tile handling ===
    #[test]
    fn test_zero_byte_tile() {
        let mut cache = TileCache::in_memory(100, 30).unwrap();

        // Store an empty tile (valid "no data" marker)
        cache
            .put_granted("esri", 10, 500, 250, &[], "image/jpeg")
            .unwrap();

        let entry = cache.get("esri", 10, 500, 250).unwrap();
        assert!(entry.data.is_empty());
        assert_eq!(entry.size_bytes, 0);
    }

    // === Test 11: content-type preservation ===
    #[test]
    fn test_content_type_preservation() {
        let mut cache = TileCache::in_memory(100, 30).unwrap();

        let content_types = [
            "image/jpeg",
            "image/jpg",
            "image/png",
            "image/webp",
            "application/octet-stream",
        ];

        for (i, ct) in content_types.iter().enumerate() {
            cache
                .put_granted("test", 10, 0, i as u32, b"data", ct)
                .unwrap();
        }

        for (i, ct) in content_types.iter().enumerate() {
            let entry = cache.get("test", 10, 0, i as u32).unwrap();
            assert_eq!(&entry.content_type, *ct, "Content type should be preserved");
        }
    }

    // === Test 12: accessed_at updates on get ===
    #[test]
    fn test_accessed_at_updates_on_get() {
        let mut cache = TileCache::in_memory(100, 30).unwrap();

        cache
            .put_granted("esri", 10, 500, 250, b"tile data", "image/jpeg")
            .unwrap();

        // Force accessed_at to an older value without sleeping
        let base_time = TileCache::now();
        set_accessed_at(&cache, "esri", 10, 500, 250, base_time - 10);
        let first_accessed_at = get_accessed_at_from_db(&cache, "esri", 10, 500, 250);

        // Access the tile - this should update accessed_at
        let entry = cache.get("esri", 10, 500, 250).unwrap();

        // Verify the returned entry has the updated accessed_at
        assert!(
            entry.accessed_at > first_accessed_at,
            "Returned accessed_at should be updated (got {}, expected > {})",
            entry.accessed_at,
            first_accessed_at
        );

        // Also verify DB was updated
        let db_accessed_at = get_accessed_at_from_db(&cache, "esri", 10, 500, 250);
        assert!(
            db_accessed_at > first_accessed_at,
            "DB accessed_at should be updated"
        );
    }

    // === Test 13: put overwrites existing tile ===
    #[test]
    fn test_put_overwrites_existing() {
        let mut cache = TileCache::in_memory(100, 30).unwrap();

        cache
            .put_granted("esri", 10, 500, 250, b"original data", "image/jpeg")
            .unwrap();
        cache
            .put_granted("esri", 10, 500, 250, b"updated data!!!", "image/png")
            .unwrap();

        let entry = cache.get("esri", 10, 500, 250).unwrap();
        assert_eq!(entry.data, b"updated data!!!");
        assert_eq!(entry.content_type, "image/png");

        // Should still be just one tile
        let stats = cache.get_stats();
        assert_eq!(stats.tile_count, 1);
    }

    // === Test 14: has_fresh_tile and has_stale_tile ===
    #[test]
    fn test_has_fresh_and_stale_tile() {
        let mut cache = TileCache::in_memory(100, 30).unwrap();

        // Fresh tile with long TTL
        cache
            .put_granted("esri", 10, 0, 0, b"fresh", "image/jpeg")
            .unwrap();
        assert!(cache.has_fresh_tile("esri", 10, 0, 0));
        assert!(!cache.has_stale_tile("esri", 10, 0, 0));

        // Non-existent tile
        assert!(!cache.has_fresh_tile("esri", 10, 0, 1));
        assert!(!cache.has_stale_tile("esri", 10, 0, 1));
    }

    // === Test 15: eviction respects TTL ===
    #[test]
    fn test_evict_removes_expired() {
        let mut cache = TileCache::in_memory(100, 30).unwrap();
        cache.ttl_seconds = 2; // 2 second TTL

        cache
            .put_granted("esri", 10, 0, 0, b"tile1", "image/jpeg")
            .unwrap();
        cache
            .put_granted("esri", 10, 0, 1, b"tile2", "image/jpeg")
            .unwrap();

        // Verify tiles are present
        let stats_before = cache.get_stats();
        assert_eq!(stats_before.tile_count, 2);

        // Force tiles to be stale without sleeping
        let stale_time = TileCache::now() - cache.ttl_seconds - 1;
        set_fetched_at(&cache, "esri", 10, 0, 0, stale_time);
        set_fetched_at(&cache, "esri", 10, 0, 1, stale_time);

        // Eviction should remove expired tiles
        let freed = cache.evict_lru().unwrap();
        assert!(freed > 0, "Should have freed expired tiles");

        let stats = cache.get_stats();
        assert_eq!(stats.tile_count, 0, "All expired tiles should be removed");
    }

    // === Test 16: put enforces max_bytes limit ===
    #[test]
    fn test_put_enforces_max_bytes() {
        let mut cache = TileCache::in_memory(1, 30).unwrap();
        cache.max_bytes = 500; // 500 bytes max

        // Add tiles that together exceed limit
        let tile1 = make_tile_data(200);
        let tile2 = make_tile_data(200);
        let tile3 = make_tile_data(200);

        cache
            .put_granted("esri", 1, 0, 0, &tile1, "image/jpeg")
            .unwrap();
        cache
            .put_granted("esri", 1, 0, 1, &tile2, "image/jpeg")
            .unwrap();

        // Set explicit timestamps
        set_accessed_at(&cache, "esri", 1, 0, 0, TileCache::now() - 100);
        set_accessed_at(&cache, "esri", 1, 0, 1, TileCache::now());

        // This put should trigger eviction to make space
        cache
            .put_granted("esri", 1, 0, 2, &tile3, "image/jpeg")
            .unwrap();

        let stats = cache.get_stats();
        assert!(
            stats.total_bytes <= cache.max_bytes,
            "Cache should never exceed max_bytes after put"
        );

        // The oldest tile should have been evicted
        assert!(
            cache.get("esri", 1, 0, 0).is_none(),
            "Oldest tile should be evicted"
        );
    }

    // === Test 17: tile larger than cache is rejected ===
    #[test]
    fn test_oversized_tile_rejected() {
        let mut cache = TileCache::in_memory(1, 30).unwrap();
        cache.max_bytes = 100; // 100 bytes max

        let oversized = make_tile_data(200); // Larger than cache
        let result = cache.put_granted("esri", 1, 0, 0, &oversized, "image/jpeg");

        assert!(result.is_err(), "Oversized tile should be rejected");
    }

    // === Test 18: LRU eviction has deterministic tie-breaking ===
    #[test]
    fn test_lru_deterministic_tiebreaker() {
        let mut cache = TileCache::in_memory(1, 30).unwrap();
        cache.max_bytes = 500; // 500 bytes capacity

        // Add tiles with same timestamp - 3 * 200 = 600 bytes exceeds 500
        let tile = make_tile_data(200);
        cache
            .put_granted("esri", 1, 0, 0, &tile, "image/jpeg")
            .unwrap();
        cache
            .put_granted("esri", 1, 0, 1, &tile, "image/jpeg")
            .unwrap();
        cache
            .put_granted("esri", 1, 0, 2, &tile, "image/jpeg")
            .unwrap();

        // Set all to same timestamp
        let same_time = TileCache::now();
        set_accessed_at(&cache, "esri", 1, 0, 0, same_time);
        set_accessed_at(&cache, "esri", 1, 0, 1, same_time);
        set_accessed_at(&cache, "esri", 1, 0, 2, same_time);

        // Force eviction - should evict to under max_bytes (500)
        cache.evict_lru().unwrap();

        let stats = cache.get_stats();
        // Should have evicted at least one tile to get under 500 bytes
        assert!(
            stats.total_bytes <= cache.max_bytes,
            "Cache should be under limit after eviction, got {} bytes",
            stats.total_bytes
        );

        // With deterministic tie-breaking (by id), lower id tiles should be evicted first
        // Tile at (1,0,0) has lowest id, should be evicted first
        assert!(
            cache.get("esri", 1, 0, 0).is_none(),
            "Tile with lowest id should be evicted first when timestamps tie"
        );
    }

    // === Test 19: get_provider_stats returns per-provider breakdown ===
    #[test]
    fn test_get_provider_stats() {
        let mut cache = TileCache::in_memory(100, 30).unwrap();

        // Add tiles from multiple providers
        let tile100 = make_tile_data(100);
        let tile200 = make_tile_data(200);
        let tile150 = make_tile_data(150);

        cache
            .put_granted("esri", 1, 0, 0, &tile100, "image/jpeg")
            .unwrap();
        cache
            .put_granted("esri", 1, 0, 1, &tile200, "image/jpeg")
            .unwrap();
        cache
            .put_granted("osm", 1, 0, 0, &tile150, "image/png")
            .unwrap();

        let provider_stats = cache.get_provider_stats();

        // Should have 2 providers
        assert_eq!(provider_stats.len(), 2);

        // ESRI: 2 tiles, 300 bytes
        let esri = provider_stats.get("esri").unwrap();
        assert_eq!(esri.0, 2, "ESRI should have 2 tiles");
        assert_eq!(esri.1, 300, "ESRI should have 300 bytes");

        // OSM: 1 tile, 150 bytes
        let osm = provider_stats.get("osm").unwrap();
        assert_eq!(osm.0, 1, "OSM should have 1 tile");
        assert_eq!(osm.1, 150, "OSM should have 150 bytes");
    }

    // === Test 20: get_provider_stats returns empty for empty cache ===
    #[test]
    fn test_get_provider_stats_empty_cache() {
        let cache = TileCache::in_memory(100, 30).unwrap();

        let provider_stats = cache.get_provider_stats();
        assert!(provider_stats.is_empty());
    }

    // === Cache-Control (Esri sends `max-age=86400` on every tile) ===

    #[test]
    fn parses_the_header_esri_actually_sends() {
        let policy = parse_cache_control("max-age=86400");
        assert_eq!(policy.max_age_seconds, Some(86_400));
        assert!(!policy.no_store);
    }

    #[test]
    fn parses_directives_case_insensitively_and_among_others() {
        let policy = parse_cache_control("Public, MAX-AGE=600, must-revalidate");
        assert_eq!(policy.max_age_seconds, Some(600));
        assert!(!policy.no_store);
    }

    #[test]
    fn treats_no_cache_as_store_but_never_fresh() {
        // Keeping it is what lets it answer a failed fetch; serving it as
        // current is what the directive forbids.
        let policy = parse_cache_control("no-cache");
        assert_eq!(policy.max_age_seconds, Some(0));
        assert!(!policy.no_store);
    }

    #[test]
    fn reads_no_store() {
        assert!(parse_cache_control("no-store").no_store);
    }

    #[test]
    fn ignores_a_malformed_max_age_rather_than_reading_it_as_zero() {
        // Reading garbage as "immediately stale" would silently kill caching for
        // a whole provider; "said nothing" leaves the configured TTL in charge.
        assert_eq!(parse_cache_control("max-age=banana").max_age_seconds, None);
        assert_eq!(parse_cache_control("max-age=-5").max_age_seconds, None);
        assert_eq!(parse_cache_control("").max_age_seconds, None);
    }

    #[test]
    fn max_age_expiry_stops_a_tile_being_served_as_fresh() {
        let mut cache = TileCache::in_memory(10, 30).unwrap();
        let policy = CachePolicy {
            max_age_seconds: Some(60),
            no_store: false,
            age_seconds: None,
            no_cache: false,
            malformed: false,
        };
        cache
            .put(
                "esri",
                1,
                2,
                3,
                b"tile",
                "image/jpeg",
                policy,
                Some("W/\"abc\""),
            )
            .unwrap();

        assert!(cache.get("esri", 1, 2, 3).is_some());

        // Walk the stored expiry into the past - the configured 30-day TTL is
        // untouched, so only the provider's max-age can be doing the work here.
        cache
            .conn
            .execute(
                "UPDATE tiles SET expires_at = ?1",
                params![TileCache::now() - 1],
            )
            .unwrap();

        assert!(
            cache.get("esri", 1, 2, 3).is_none(),
            "past max-age the tile must not be served as current"
        );
        assert!(
            cache.get_stale("esri", 1, 2, 3).is_some(),
            "but it must survive to answer a failed fetch (stale-if-error)"
        );
        assert!(!cache.has_fresh_tile("esri", 1, 2, 3));
        assert!(cache.has_stale_tile("esri", 1, 2, 3));
    }

    #[test]
    fn a_provider_that_grants_nothing_gets_no_disk() {
        // Silence is not consent. Without a Cache-Control saying how long the
        // tile may be kept, it is served and dropped - the configured TTL is a
        // ceiling on a grant, never a substitute for one.
        let mut cache = TileCache::in_memory(10, 30).unwrap();
        cache
            .put(
                "other",
                1,
                2,
                3,
                b"tile",
                "image/jpeg",
                CachePolicy::default(),
                None,
            )
            .unwrap();

        assert!(cache.get("other", 1, 2, 3).is_none());
        assert!(cache.get_stale("other", 1, 2, 3).is_none());
        assert_eq!(cache.get_stats().tile_count, 0);
    }

    #[test]
    fn losing_the_grant_drops_the_copy_we_were_holding_under_it() {
        // A provider that stops sending a grant should not leave us sitting on
        // yesterday's. The re-fetch that carried no directive evicts it.
        let mut cache = TileCache::in_memory(10, 30).unwrap();
        cache
            .put_granted("esri", 1, 2, 3, b"granted", "image/jpeg")
            .unwrap();
        assert!(cache.get("esri", 1, 2, 3).is_some());

        cache
            .put(
                "esri",
                1,
                2,
                3,
                b"ungranted",
                "image/jpeg",
                CachePolicy::default(),
                None,
            )
            .unwrap();

        assert_eq!(cache.get_stats().tile_count, 0);
    }

    #[test]
    fn no_cache_is_a_grant_to_store_even_though_it_is_never_served() {
        // "Keep it, but always ask" is still permission to KEEP it - the bytes
        // and the validator are what make the next request a cheap 304 instead
        // of a full download. What it is not is permission to serve: this test
        // used to assert the copy answered stale-if-error, which RFC 9111
        // 5.2.2.4 forbids. See
        // `a_no_cache_tile_is_never_served_as_a_last_resort`.
        let mut cache = TileCache::in_memory(10, 30).unwrap();
        cache
            .put(
                "esri",
                1,
                2,
                3,
                b"tile",
                "image/jpeg",
                parse_cache_control("no-cache"),
                None,
            )
            .unwrap();

        assert_eq!(cache.get_stats().tile_count, 1, "stored");
        assert!(
            cache.get("esri", 1, 2, 3).is_none(),
            "never served as fresh"
        );
        // The row IS reachable - that is what lets it be revalidated - but it
        // carries the flag that stops it answering a FAILED revalidation.
        let held = cache.get_stale("esri", 1, 2, 3).expect("retained");
        assert!(held.no_cache);
        assert!(
            !cache.has_stale_tile("esri", 1, 2, 3),
            "and reports as not servable"
        );
    }

    #[test]
    fn a_directive_that_grants_nothing_is_not_a_grant() {
        // `granted_seconds` is the live gate; there was a second `permits_storage`
        // helper saying the same thing that no production path called, which is
        // how the two copies of this rule drifted the first time.
        assert!(parse_cache_control("max-age=86400")
            .granted_seconds()
            .is_some());
        assert_eq!(parse_cache_control("no-cache").granted_seconds(), Some(0));
        assert_eq!(parse_cache_control("no-store").granted_seconds(), None);
        assert_eq!(parse_cache_control("public").granted_seconds(), None);
        assert_eq!(CachePolicy::default().granted_seconds(), None);
    }

    #[test]
    fn no_store_is_not_written_and_drops_any_prior_copy() {
        let mut cache = TileCache::in_memory(10, 30).unwrap();
        cache
            .put_granted("esri", 1, 2, 3, b"first", "image/jpeg")
            .unwrap();
        assert!(cache.get("esri", 1, 2, 3).is_some());

        cache
            .put(
                "esri",
                1,
                2,
                3,
                b"second",
                "image/jpeg",
                CachePolicy {
                    max_age_seconds: None,
                    no_store: true,
                    age_seconds: None,
                    no_cache: false,
                    malformed: false,
                },
                None,
            )
            .unwrap();

        assert!(cache.get("esri", 1, 2, 3).is_none());
        assert!(cache.get_stale("esri", 1, 2, 3).is_none());
    }

    #[test]
    fn remaining_policy_carries_what_is_left_never_the_original_grant() {
        // An overzoomed crop inherits this; handing it the full max-age would
        // let a derived tile outlive the imagery it was cut from.
        let now = 1_000;
        let entry = TileEntry {
            data: vec![],
            content_type: "image/jpeg".to_string(),
            size_bytes: 0,
            fetched_at: now - 100,
            accessed_at: now,
            expires_at: Some(now + 25),
            etag: None,
            no_cache: false,
        };
        assert_eq!(entry.remaining_policy(now).max_age_seconds, Some(25));
        // Never negative: an already-expired parent yields an immediately-stale child.
        assert_eq!(entry.remaining_policy(now + 100).max_age_seconds, Some(0));
    }

    // === Per-provider TTL ===

    #[test]
    fn a_provider_override_shortens_retention_without_touching_others() {
        let mut cache = TileCache::in_memory(10, 30).unwrap();
        cache.set_provider_ttls(&HashMap::from([("esri".to_string(), 1u32)]));

        cache
            .put_granted("esri", 1, 0, 0, b"a", "image/jpeg")
            .unwrap();
        cache
            .put_granted("other", 1, 0, 0, b"b", "image/jpeg")
            .unwrap();

        // Age both by two days: past esri's 1-day override, inside the 30-day default.
        let two_days_ago = TileCache::now() - 2 * 24 * 60 * 60;
        cache
            .conn
            .execute("UPDATE tiles SET fetched_at = ?1", params![two_days_ago])
            .unwrap();

        assert!(cache.get("esri", 1, 0, 0).is_none(), "past its own TTL");
        assert!(
            cache.get("other", 1, 0, 0).is_some(),
            "still inside the default"
        );
    }

    #[test]
    fn eviction_retires_each_provider_on_its_own_clock() {
        let mut cache = TileCache::in_memory(10, 30).unwrap();
        cache.set_provider_ttls(&HashMap::from([("esri".to_string(), 1u32)]));
        cache
            .put_granted("esri", 1, 0, 0, b"a", "image/jpeg")
            .unwrap();
        cache
            .put_granted("other", 1, 0, 0, b"b", "image/jpeg")
            .unwrap();
        let two_days_ago = TileCache::now() - 2 * 24 * 60 * 60;
        cache
            .conn
            .execute("UPDATE tiles SET fetched_at = ?1", params![two_days_ago])
            .unwrap();

        cache.evict_lru().unwrap();

        // A single global cutoff would have taken both, or neither.
        assert!(!cache.has_stale_tile("esri", 1, 0, 0));
        assert!(cache.get("other", 1, 0, 0).is_some());
    }

    #[test]
    fn schema_migration_is_idempotent_on_an_existing_cache() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut cache = TileCache::open(dir.path(), 10, 30).unwrap();
            cache
                .put_granted("esri", 1, 2, 3, b"tile", "image/jpeg")
                .unwrap();
        }
        // Re-opening runs init_schema again; the ADD COLUMNs must not double-apply.
        let mut reopened = TileCache::open(dir.path(), 10, 30).unwrap();
        assert!(reopened.get("esri", 1, 2, 3).is_some());
    }

    // === Conditional revalidation (Esri answers If-None-Match with a 304) ===

    #[test]
    fn the_validator_is_stored_and_readable_for_a_conditional_refetch() {
        let mut cache = TileCache::in_memory(10, 30).unwrap();
        cache
            .put(
                "esri",
                1,
                2,
                3,
                b"tile",
                "image/jpeg",
                CachePolicy {
                    max_age_seconds: Some(60),
                    no_store: false,
                    age_seconds: None,
                    no_cache: false,
                    malformed: false,
                },
                Some("\"sjii0vvv1sqvf\""),
            )
            .unwrap();
        let entry = cache.get("esri", 1, 2, 3).unwrap();
        assert_eq!(entry.etag.as_deref(), Some("\"sjii0vvv1sqvf\""));
    }

    #[test]
    fn a_validator_survives_into_the_stale_window_where_it_is_actually_needed() {
        // Revalidation only happens once a tile is PAST max-age, so the etag has
        // to still be readable from the stale path - that is what supplies
        // If-None-Match.
        let mut cache = TileCache::in_memory(10, 30).unwrap();
        cache
            .put(
                "esri",
                1,
                2,
                3,
                b"tile",
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
        cache
            .conn
            .execute(
                "UPDATE tiles SET expires_at = ?1",
                params![TileCache::now() - 1],
            )
            .unwrap();

        assert!(cache.get("esri", 1, 2, 3).is_none());
        let stale = cache.get_stale("esri", 1, 2, 3).unwrap();
        assert_eq!(stale.etag.as_deref(), Some("\"abc\""));
    }

    #[test]
    fn revalidation_makes_a_stale_tile_fresh_again_without_new_bytes() {
        let mut cache = TileCache::in_memory(10, 30).unwrap();
        cache
            .put(
                "esri",
                1,
                2,
                3,
                b"original bytes",
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
        cache
            .conn
            .execute(
                "UPDATE tiles SET expires_at = ?1",
                params![TileCache::now() - 1],
            )
            .unwrap();
        assert!(cache.get("esri", 1, 2, 3).is_none(), "precondition: stale");

        let refreshed = cache
            .refresh_validated(
                "esri",
                1,
                2,
                3,
                CachePolicy {
                    max_age_seconds: Some(86_400),
                    no_store: false,
                    age_seconds: None,
                    no_cache: false,
                    malformed: false,
                },
            )
            .unwrap();
        assert!(refreshed);

        let entry = cache
            .get("esri", 1, 2, 3)
            .expect("a 304 puts the tile back in the fresh window");
        // The bytes are untouched - that is the entire saving.
        assert_eq!(entry.data, b"original bytes");
        assert!(entry.expires_at.unwrap() > TileCache::now());
    }

    #[test]
    fn revalidating_a_tile_we_do_not_have_reports_it_rather_than_inventing_a_row() {
        let mut cache = TileCache::in_memory(10, 30).unwrap();
        let refreshed = cache
            .refresh_validated("esri", 9, 9, 9, CachePolicy::default())
            .unwrap();
        assert!(!refreshed);
        assert!(cache.get_stale("esri", 9, 9, 9).is_none());
    }

    // === Remote max-age is untrusted arithmetic input ===

    #[test]
    fn an_absurd_max_age_is_clamped_rather_than_overflowing_the_clock() {
        // `max-age` comes off the network and is then ADDED to the clock. Left
        // unbounded, i64::MAX panics the addition in a debug build - inside the
        // tile-cache mutex, which poisons it and silently kills caching for the
        // session - and wraps to a negative expiry in release.
        let policy = parse_cache_control(&format!("max-age={}", i64::MAX));
        assert_eq!(policy.max_age_seconds, Some(MAX_GRANT_SECONDS));

        let ordinary = parse_cache_control("max-age=86400");
        assert_eq!(
            ordinary.max_age_seconds,
            Some(86_400),
            "a real grant must pass through untouched"
        );
    }

    #[test]
    fn a_hostile_max_age_neither_panics_nor_makes_the_tile_unservable() {
        let mut cache = TileCache::in_memory(10, 30).unwrap();
        // Straight past the parser, as a hand-built policy - the belt to the
        // clamp's braces, and the shape the existing tests already construct.
        cache
            .put(
                "esri",
                1,
                2,
                3,
                b"tile",
                "image/jpeg",
                CachePolicy {
                    max_age_seconds: Some(i64::MAX),
                    no_store: false,
                    age_seconds: None,
                    no_cache: false,
                    malformed: false,
                },
                None,
            )
            .unwrap();

        let entry = cache
            .get("esri", 1, 2, 3)
            .expect("a saturated expiry is far-future, so the tile stays servable");
        assert!(entry.expires_at.unwrap() > TileCache::now());
    }

    #[test]
    fn a_saturated_parent_hands_a_child_a_bounded_grant() {
        // remaining_policy subtracts; an i64::MAX expiry must not wrap it.
        let entry = TileEntry {
            data: vec![],
            content_type: "image/jpeg".to_string(),
            size_bytes: 0,
            fetched_at: 0,
            accessed_at: 0,
            expires_at: Some(i64::MAX),
            etag: None,
            no_cache: false,
        };
        // Saturating rather than wrapping...
        assert_eq!(entry.remaining_policy(0).max_age_seconds, Some(i64::MAX));
        assert_eq!(
            entry.remaining_policy(i64::MIN).max_age_seconds,
            Some(i64::MAX)
        );
        // ...and bounded by the time it reaches a write path, which is what the
        // ceiling has to mean if it is an invariant rather than a parser quirk.
        assert_eq!(
            entry.remaining_policy(0).granted_seconds(),
            Some(MAX_GRANT_SECONDS)
        );
    }

    // === One grant gate, applied on BOTH write paths ===

    #[test]
    fn revalidation_that_revokes_the_grant_drops_the_copy() {
        // The 304 path used to skip this check entirely: a provider answering
        // "still current, but stop storing it" had its tile kept AND its
        // retention extended, the exact opposite of what put() promises.
        let mut cache = TileCache::in_memory(10, 30).unwrap();
        cache
            .put_granted("esri", 1, 2, 3, b"tile", "image/jpeg")
            .unwrap();
        assert!(cache.get("esri", 1, 2, 3).is_some());

        let refreshed = cache
            .refresh_validated(
                "esri",
                1,
                2,
                3,
                CachePolicy {
                    max_age_seconds: Some(86_400),
                    no_store: true,
                    age_seconds: None,
                    no_cache: false,
                    malformed: false,
                },
            )
            .unwrap();

        assert!(!refreshed, "nothing was refreshed - the row is gone");
        assert_eq!(cache.get_stats().tile_count, 0);
    }

    #[test]
    fn granted_seconds_is_the_one_gate_both_paths_ask() {
        assert_eq!(
            parse_cache_control("max-age=86400").granted_seconds(),
            Some(86_400)
        );
        assert_eq!(parse_cache_control("no-cache").granted_seconds(), Some(0));
        assert_eq!(parse_cache_control("no-store").granted_seconds(), None);
        assert_eq!(
            parse_cache_control("no-store, max-age=86400").granted_seconds(),
            None,
            "no-store beats a max-age sent alongside it"
        );
        assert_eq!(CachePolicy::default().granted_seconds(), None);
    }

    // === Retention bounds `get_stale`, not just eviction ===

    #[test]
    fn a_tile_past_retention_is_not_served_even_as_a_last_resort() {
        // Eviction only runs on `put`. A session that never caches anything -
        // offline, which is exactly when `get_stale` is reached - leaves retired
        // rows sitting there, so the bound has to be enforced at the lookup too.
        let mut cache = TileCache::in_memory(10, 1).unwrap();
        cache
            .put_granted("esri", 1, 2, 3, b"tile", "image/jpeg")
            .unwrap();

        // Two days old against a one-day retention, and no `put` in between.
        set_fetched_at(&cache, "esri", 1, 2, 3, TileCache::now() - 2 * 24 * 60 * 60);

        assert!(cache.get("esri", 1, 2, 3).is_none(), "not fresh either");
        assert!(
            cache.get_stale("esri", 1, 2, 3).is_none(),
            "past ttl_days means past ttl_days - a stored copy cannot outlive it"
        );
        assert!(!cache.has_stale_tile("esri", 1, 2, 3), "and reports so");
    }

    #[test]
    fn a_tile_inside_retention_but_past_max_age_still_answers() {
        // The other half of the same predicate, and the reason it is not simply
        // "delete anything not fresh": this is the whole stale-if-error case.
        let mut cache = TileCache::in_memory(10, 30).unwrap();
        cache
            .put(
                "esri",
                1,
                2,
                3,
                b"tile",
                "image/jpeg",
                parse_cache_control("max-age=1"),
                Some("\"v1\""),
            )
            .unwrap();
        cache.expire_for_test("esri", 1, 2, 3).unwrap();

        assert!(cache.get("esri", 1, 2, 3).is_none(), "past its max-age");
        assert!(
            cache.get_stale("esri", 1, 2, 3).is_some(),
            "but well inside retention, so it can answer a failed fetch"
        );
        assert!(cache.has_stale_tile("esri", 1, 2, 3));
    }

    #[test]
    fn purge_retired_drops_the_bytes_not_just_the_visibility() {
        let mut cache = TileCache::in_memory(10, 1).unwrap();
        cache
            .put_granted("esri", 1, 2, 3, b"retired", "image/jpeg")
            .unwrap();
        cache
            .put_granted("esri", 4, 5, 6, b"current", "image/jpeg")
            .unwrap();
        set_fetched_at(&cache, "esri", 1, 2, 3, TileCache::now() - 2 * 24 * 60 * 60);

        let freed = cache.purge_retired().unwrap();
        assert_eq!(freed, b"retired".len() as u64);
        assert_eq!(
            cache.get_stats().tile_count,
            1,
            "the retired row is gone from disk, not merely unreachable"
        );
        assert!(
            cache.get("esri", 4, 5, 6).is_some(),
            "and the rest survives"
        );
    }

    #[test]
    fn purge_retired_honours_a_provider_override() {
        // Why `purge_retired` must be called AFTER `set_provider_ttls`: swept
        // against the default alone, a provider configured to keep tiles LONGER
        // would have them retired early.
        let mut cache = TileCache::in_memory(10, 1).unwrap();
        cache.set_provider_ttls(&HashMap::from([("esri".to_string(), 30u32)]));
        cache
            .put_granted("esri", 1, 2, 3, b"tile", "image/jpeg")
            .unwrap();
        set_fetched_at(&cache, "esri", 1, 2, 3, TileCache::now() - 2 * 24 * 60 * 60);
        cache.expire_for_test("esri", 1, 2, 3).unwrap();

        assert_eq!(cache.purge_retired().unwrap(), 0);
        assert!(
            cache.get_stale("esri", 1, 2, 3).is_some(),
            "two days old, but this provider is configured for thirty"
        );
    }

    // === `Age`: max-age counts from the origin, not from our clock ===

    #[test]
    fn parses_an_age_header_and_rejects_nonsense() {
        assert_eq!(parse_age("0"), Some(0));
        assert_eq!(parse_age("80000"), Some(80_000));
        assert_eq!(parse_age("  42  "), Some(42), "surrounding space is legal");
        // Absent rather than zero: guessing "brand new" from a malformed header
        // would invent freshness the provider never granted.
        assert_eq!(parse_age("-5"), None);
        assert_eq!(parse_age("12.5"), None);
        assert_eq!(parse_age("soon"), None);
        assert_eq!(parse_age(""), None);
        assert_eq!(
            parse_age(&format!("{}", i64::MAX)),
            Some(MAX_GRANT_SECONDS),
            "clamped like a grant, so it cannot underflow anything downstream"
        );
    }

    #[test]
    fn an_age_too_large_to_parse_means_ancient_not_brand_new() {
        // The polarity trap. For `max-age`, unreadable means NO grant - safe.
        // For `Age`, "absent" means NOT AGED, i.e. full freshness, so the same
        // rule would let a hostile or broken intermediary hand back the whole
        // grant again. RFC 9111 1.2.2 says an over-large delta-seconds MUST be
        // clamped, not ignored.
        let huge = "99999999999999999999999";
        assert_eq!(parse_age(huge), Some(MAX_GRANT_SECONDS));

        let mut policy = parse_cache_control("max-age=86400");
        policy.age_seconds = parse_age(huge);
        assert_eq!(
            policy.granted_seconds(),
            Some(0),
            "an unrepresentably old response has no freshness left"
        );

        // Genuine junk still says nothing about age. Reading it as maximally
        // stale would let one malformed proxy header force a revalidation on
        // every request for that provider, forever.
        assert_eq!(parse_age("soon"), None);
        assert_eq!(parse_age("-99999999999999999999999"), None);
        assert_eq!(parse_age("12.5"), None);
    }

    #[test]
    fn age_is_subtracted_from_the_grant() {
        // The live shape: Esri sends max-age=86400 through CloudFront, which
        // adds Age once its edge has been holding the response.
        let mut policy = parse_cache_control("max-age=86400");
        policy.age_seconds = Some(80_000);
        assert_eq!(
            policy.granted_seconds(),
            Some(6_400),
            "only what is LEFT of the origin's window may be treated as fresh"
        );

        // No Age header at all: nothing to subtract.
        let untouched = parse_cache_control("max-age=86400");
        assert_eq!(untouched.granted_seconds(), Some(86_400));
    }

    #[test]
    fn a_fully_spent_grant_is_kept_but_never_fresh() {
        // Age >= max-age. The provider DID permit storage, so the tile is still
        // stored - with its ETag, which is what makes the next request a cheap
        // 304 rather than a full download. It is simply never servable as
        // current, the same shape `no-cache` maps to.
        let mut policy = parse_cache_control("max-age=86400");
        policy.age_seconds = Some(90_000);
        assert_eq!(
            policy.granted_seconds(),
            Some(0),
            "spent, not revoked - Some(0) stores; None would drop the bytes"
        );

        let mut cache = TileCache::in_memory(10, 30).unwrap();
        cache
            .put("esri", 1, 2, 3, b"tile", "image/jpeg", policy, None)
            .unwrap();
        assert!(
            cache.get("esri", 1, 2, 3).is_none(),
            "a spent grant is not freshness"
        );
        assert!(
            cache.get_stale("esri", 1, 2, 3).is_some(),
            "but the bytes are kept to revalidate against"
        );
    }

    #[test]
    fn no_store_still_beats_a_zero_age() {
        let mut policy = parse_cache_control("no-store, max-age=86400");
        policy.age_seconds = Some(0);
        assert_eq!(policy.granted_seconds(), None);
    }

    #[test]
    fn a_derived_crop_is_not_aged_twice() {
        // `expires_at` was already stamped net of the parent's Age, so the
        // remaining seconds are the whole truth. Re-subtracting would shorten
        // every crop by the parent's CDN residency a second time.
        let entry = TileEntry {
            data: vec![1],
            content_type: "image/jpeg".to_string(),
            size_bytes: 1,
            fetched_at: 0,
            accessed_at: 0,
            expires_at: Some(6_400),
            etag: None,
            no_cache: false,
        };
        let derived = entry.remaining_policy(0);
        assert_eq!(derived.age_seconds, None);
        assert_eq!(derived.granted_seconds(), Some(6_400));
    }

    #[test]
    fn a_quoted_field_list_does_not_hide_the_directive() {
        // `no-cache="Set-Cookie,X-Foo"` embeds a comma, so splitting the header
        // on commas cuts the directive in half. Matching the whole token missed
        // it entirely and served the tile as fresh under the max-age alongside.
        let policy = parse_cache_control("no-cache=\"Set-Cookie,X-Foo\", max-age=86400");
        assert_eq!(
            policy.max_age_seconds,
            Some(0),
            "no-cache must win over the max-age beside it"
        );

        let store = parse_cache_control("no-store=\"Set-Cookie,X-Foo\"");
        assert!(store.no_store);
    }

    #[test]
    fn a_no_cache_tile_is_never_served_as_a_last_resort() {
        // `no-cache` grants storage but forbids reuse WITHOUT successful
        // validation on the origin (RFC 9111 5.2.2.4). "The network failed" is
        // therefore the one condition under which such a tile may not be
        // served - the opposite of every other retained row. On disk it looks
        // exactly like a grant that ran out, which is why the flag is stored.
        let mut cache = TileCache::in_memory(10, 30).unwrap();
        cache
            .put(
                "esri",
                1,
                2,
                3,
                b"revalidate-me",
                "image/jpeg",
                parse_cache_control("no-cache"),
                Some("\"v1\""),
            )
            .unwrap();

        // Stored - the bytes and the ETag are what make revalidation cheap, and
        // the lookup must still reach them or the conditional request cannot be
        // built at all. Excluding the row here instead hid the validator too,
        // turning every request into a full download.
        assert_eq!(cache.get_stats().tile_count, 1);
        assert!(cache.get("esri", 1, 2, 3).is_none(), "never fresh");
        let retained = cache
            .get_stale("esri", 1, 2, 3)
            .expect("reachable, so it can be revalidated");
        assert_eq!(retained.etag.as_deref(), Some("\"v1\""));
        assert!(
            retained.no_cache,
            "flagged, so the caller refuses to serve it unvalidated"
        );
        assert!(
            !cache.has_stale_tile("esri", 1, 2, 3),
            "which is the question has_stale_tile answers"
        );

        // A plain expired grant, by contrast, is exactly what stale-if-error is
        // for. Same on-disk shape, different permission.
        cache
            .put(
                "esri",
                4,
                5,
                6,
                b"ordinary",
                "image/jpeg",
                parse_cache_control("max-age=86400"),
                Some("\"v2\""),
            )
            .unwrap();
        cache.expire_for_test("esri", 4, 5, 6).unwrap();
        let ordinary = cache.get_stale("esri", 4, 5, 6).expect("retained");
        assert!(!ordinary.no_cache);
        assert!(cache.has_stale_tile("esri", 4, 5, 6));
    }

    #[test]
    fn a_304_restates_no_cache_rather_than_leaving_it_set() {
        // The 304 carries the current directives, so a provider that has stopped
        // sending `no-cache` releases the row, and one that has started sending
        // it imposes the exclusion on a row already held.
        let mut cache = TileCache::in_memory(10, 30).unwrap();
        cache
            .put(
                "esri",
                1,
                2,
                3,
                b"tile",
                "image/jpeg",
                parse_cache_control("no-cache"),
                Some("\"v1\""),
            )
            .unwrap();
        assert!(!cache.has_stale_tile("esri", 1, 2, 3));

        // Revalidated, and now under an ordinary grant.
        cache
            .refresh_validated("esri", 1, 2, 3, parse_cache_control("max-age=86400"))
            .unwrap();
        cache.expire_for_test("esri", 1, 2, 3).unwrap();
        assert!(
            cache.has_stale_tile("esri", 1, 2, 3),
            "the no-cache exclusion must lift when the provider stops sending it"
        );

        // And back the other way.
        cache
            .refresh_validated("esri", 1, 2, 3, parse_cache_control("no-cache"))
            .unwrap();
        assert!(!cache.has_stale_tile("esri", 1, 2, 3));
    }

    #[test]
    fn an_unreadable_header_does_not_drop_what_is_already_held() {
        // "The provider sent no directive" is a statement - permission
        // withdrawn, drop the copy. "We could not parse what it sent" is the
        // absence of one, and says nothing about the grant the stored copy was
        // written under. Collapsing both to "no grant" let one mangled header
        // from a proxy drain the tile.
        let mut cache = TileCache::in_memory(10, 30).unwrap();
        cache
            .put_granted("esri", 1, 2, 3, b"held", "image/jpeg")
            .unwrap();

        let malformed = parse_cache_control("max-age=86400, x=\"unterminated, no-store");
        assert!(malformed.malformed);
        assert_eq!(malformed.granted_seconds(), None, "still grants nothing");

        cache
            .put("esri", 1, 2, 3, b"new", "image/jpeg", malformed, None)
            .unwrap();

        let held = cache.get("esri", 1, 2, 3).expect("the old row survives");
        assert_eq!(held.data, b"held", "and the new body was not written");

        // The contrast: a response that genuinely carries no directive IS a
        // revocation, and still drops it.
        cache
            .put(
                "esri",
                1,
                2,
                3,
                b"new",
                "image/jpeg",
                CachePolicy::default(),
                None,
            )
            .unwrap();
        assert!(cache.get("esri", 1, 2, 3).is_none());
        assert!(cache.get_stale("esri", 1, 2, 3).is_none());
    }

    #[test]
    fn an_escaped_quote_does_not_end_the_quoted_string() {
        // RFC 9110 5.6.4 allows `\"` inside a quoted-string. Toggling on every
        // quote ended it early, so this ONE `private` directive split into
        // three and the `max-age` buried inside was read as a standalone grant -
        // the exact hole quote-awareness was added to close, one level down.
        let policy = parse_cache_control(r#"private="x\", max-age=86400, y""#);
        assert_eq!(
            policy.granted_seconds(),
            None,
            "the max-age is inside the quoted field list, not a grant"
        );

        // An escaped quote must not stop a REAL directive alongside it parsing.
        let alongside = parse_cache_control(r#"private="x\", y", max-age=600"#);
        assert_eq!(alongside.granted_seconds(), Some(600));
    }

    #[test]
    fn unbalanced_quoting_grants_nothing() {
        // A stray quote swallows everything after it, so a `no-store` arriving
        // later vanished while an earlier `max-age` still stood - the same hole
        // pointing the other way. The header is malformed and there is no way to
        // know what was meant, so it fails closed.
        let swallowed = parse_cache_control("max-age=86400, x=\"unterminated, no-store");
        assert_eq!(
            swallowed.granted_seconds(),
            None,
            "a max-age cannot survive a header we could not parse"
        );

        // A trailing escape is malformed the same way.
        assert_eq!(
            parse_cache_control("max-age=86400, x=\"y\\").granted_seconds(),
            None
        );

        // Not `no_store` though: that would also evict a copy already held under
        // a grant that was perfectly valid when it was given.
        assert!(!swallowed.no_store);
    }

    #[test]
    fn directives_with_spaces_around_the_equals_still_parse() {
        assert_eq!(
            parse_cache_control("max-age = 600").max_age_seconds,
            Some(600)
        );
    }

    #[test]
    fn eviction_reports_the_bytes_it_actually_freed() {
        // The figure used to be a diff of two whole-table SUMs - two full scans
        // of a cache up to 500 MB, on the put path. It is now summed against the
        // same predicate the DELETE uses, so it must still be exact.
        let mut cache = TileCache::in_memory(10, 30).unwrap();
        cache
            .put_granted("esri", 1, 0, 0, &vec![0u8; 4096], "image/jpeg")
            .unwrap();
        cache
            .put_granted("other", 1, 0, 0, &vec![0u8; 2048], "image/jpeg")
            .unwrap();

        // Age everything past the default 30-day retention.
        let long_ago = TileCache::now() - 31 * 24 * 60 * 60;
        cache
            .conn
            .execute("UPDATE tiles SET fetched_at = ?1", params![long_ago])
            .unwrap();

        let freed = cache.evict_lru().unwrap();
        assert_eq!(freed, 4096 + 2048);
        assert_eq!(cache.get_stats().tile_count, 0);
    }

    #[test]
    fn eviction_reports_freed_bytes_per_provider_clock() {
        // With a per-provider override the two branches free different rows;
        // the total must still add up.
        let mut cache = TileCache::in_memory(10, 30).unwrap();
        cache.set_provider_ttls(&HashMap::from([("esri".to_string(), 1u32)]));
        cache
            .put_granted("esri", 1, 0, 0, &vec![0u8; 1024], "image/jpeg")
            .unwrap();
        cache
            .put_granted("other", 1, 0, 0, &vec![0u8; 512], "image/jpeg")
            .unwrap();

        // Two days old: past esri's 1-day override, inside the 30-day default.
        let two_days_ago = TileCache::now() - 2 * 24 * 60 * 60;
        cache
            .conn
            .execute("UPDATE tiles SET fetched_at = ?1", params![two_days_ago])
            .unwrap();

        assert_eq!(cache.evict_lru().unwrap(), 1024, "only esri's row retired");
        assert!(cache.get("other", 1, 0, 0).is_some());
    }

    #[test]
    fn a_quoted_list_cannot_mint_a_grant_the_provider_never_issued() {
        // The inverse of the directive-hiding bug. `private="x,max-age=999,y"`
        // is ONE directive named `private`; splitting on its inner commas lifted
        // out a bare `max-age=999` and treated it as permission to store.
        let policy = parse_cache_control(r#"private="x,max-age=999,y""#);
        assert_eq!(policy.max_age_seconds, None);
        assert_eq!(policy.granted_seconds(), None);

        // And a quoted list must not smuggle a no-store either.
        let smuggled = parse_cache_control(r#"private="x,no-store,y", max-age=86400"#);
        assert!(!smuggled.no_store);
        assert_eq!(smuggled.granted_seconds(), Some(86_400));
    }

    #[test]
    fn the_ceiling_binds_every_write_path_not_just_the_parser() {
        // remaining_policy mints a max-age for overzoomed crops and it reaches
        // `put` through the same gate, so the clamp has to live on the gate.
        let hand_built = CachePolicy {
            max_age_seconds: Some(i64::MAX),
            no_store: false,
            age_seconds: None,
            no_cache: false,
            malformed: false,
        };
        assert_eq!(hand_built.granted_seconds(), Some(MAX_GRANT_SECONDS));
    }

    #[test]
    fn a_304_with_no_directives_keeps_the_tile_it_just_confirmed() {
        // RFC 9111 4.3.4: a 304 UPDATES the stored headers, so silence is not
        // revocation. Deleting here threw away bytes we had just spent a
        // conditional request to avoid re-fetching, along with the etag needed
        // to ask again cheaply.
        let mut cache = TileCache::in_memory(10, 30).unwrap();
        cache
            .put(
                "esri",
                1,
                2,
                3,
                b"tile",
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
        cache.expire_max_age_for_test("esri", 1, 2, 3).unwrap();

        let refreshed = cache
            .refresh_validated("esri", 1, 2, 3, CachePolicy::default())
            .unwrap();

        assert!(refreshed, "the row was updated, not dropped");
        let entry = cache
            .get_stale("esri", 1, 2, 3)
            .expect("still retained, still revalidatable");
        assert_eq!(entry.data, b"tile");
        assert_eq!(entry.etag.as_deref(), Some("\"abc\""), "etag survives");
        // Retention re-stamped (content confirmed), freshness NOT extended
        // (no new grant arrived), so it revalidates again next request.
        assert!(cache.get("esri", 1, 2, 3).is_none());
    }

    #[test]
    fn a_304_without_directives_does_not_resurrect_a_grantless_row() {
        // Pre-migration rows have no recorded grant; silence must not keep them
        // alive forever by re-stamping retention on every revalidation.
        let mut cache = TileCache::in_memory(10, 30).unwrap();
        cache
            .put_granted("esri", 1, 2, 3, b"tile", "image/jpeg")
            .unwrap();
        cache
            .conn
            .execute("UPDATE tiles SET expires_at = NULL", [])
            .unwrap();

        let refreshed = cache
            .refresh_validated("esri", 1, 2, 3, CachePolicy::default())
            .unwrap();
        assert!(!refreshed, "no grant on record, nothing to keep alive");
    }
}
