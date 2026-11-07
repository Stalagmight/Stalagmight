use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};

use super::{
    compress_json, declared_inflated_size, decompress_json_under, Database, InflationBudget,
    MAX_HISTORY_LOAD_BYTES,
};
use crate::persistence::undo_limits::MAX_PERSISTED_HISTORY_ROWS;

type CompressedHistoryRow = (i64, Vec<u8>, Vec<u8>, Option<String>);
type HistoryRow = (i64, String, String, Option<String>);

/// [#576 B20] One history row as it is stored: the command and undo-data JSON
/// already gzipped (exactly as [`compress_history_json`] gzips them), plus the
/// timestamp column.
///
/// Shared (`Arc`) so a caller can keep the compressed form of an entry it has
/// not changed and hand it to every later save without compressing it again -
/// a save rewrites the whole persisted stack (see
/// `UndoStack::rows_to_persist`), and re-gzipping megabyte snapshot pairs on
/// every autosave is the cost this exists to avoid.
#[derive(Debug, Clone)]
pub struct StoredHistoryRow {
    pub command: std::sync::Arc<[u8]>,
    pub undo_data: std::sync::Arc<[u8]>,
    pub timestamp: Option<String>,
}

/// gzip one history JSON blob exactly as a history save stores it.
pub fn compress_history_json(json: &str) -> Vec<u8> {
    compress_json(json)
}

const MAX_DECOMPRESS_THREADS: usize = 8;

fn rows_per_decompress_batch() -> usize {
    let available = std::thread::available_parallelism().map_or(2, usize::from);
    available.clamp(2, MAX_DECOMPRESS_THREADS) / 2
}

/// [#473] Refuse a window whose rows ALREADY ADD UP to more than the open has left,
/// before a single byte is decompressed.
///
/// **This is the check that actually stops a hostile file**, and the running
/// `InflationBudget` behind it is defense in depth (see its doc for why it is unreachable
/// while this runs first). Every blob can state its own inflated size without allocating
/// (`declared_inflated_size` - a trailer read for gzip, a validity scan for legacy), so
/// the whole window is priced by walking the compressed rows sqlite already handed us,
/// with an error
/// that names the real problem ("this file claims 51 GiB of history") rather than "row 37
/// did not fit".
///
/// Declared sizes are attacker-controlled and may UNDERSTATE, so passing here is not on
/// its own a promise about the real total. What closes that gap is the per-blob cap:
/// every row is then read capped at exactly what it declared, so actual <= declared for
/// every row, and therefore actual total <= this sum. The two together are the bound.
/// `saturating_add` because the point is to refuse an absurd total, not to panic on one.
fn declared_window_bytes(raw: &[CompressedHistoryRow]) -> u64 {
    raw.iter().fold(0_u64, |total, (_, cmd, undo, _)| {
        total
            .saturating_add(declared_inflated_size(cmd))
            .saturating_add(declared_inflated_size(undo))
    })
}

/// [#473] Decompress one stack's window, charged against the OPEN's budget.
///
/// `budget` is borrowed rather than created here, and that is the fix for a real defect:
/// creating it per call gave the undo stack and the redo stack a full budget each, so a
/// project open could reach 2x the constant while every doc said 1x. The caller that owns
/// the open owns the budget - see `Database::load_history_budget`.
pub(super) fn decompress_history_rows_within(
    raw: Vec<CompressedHistoryRow>,
    stack_name: &'static str,
    budget: &InflationBudget,
) -> Result<Vec<HistoryRow>> {
    // Against what is LEFT, not against the constant: the second stack of an open must
    // only get what the first did not spend, or the pair is unbounded again.
    let remaining = budget.remaining();
    let declared = declared_window_bytes(&raw);
    if declared > remaining {
        anyhow::bail!(
            "{stack_name} history declares {declared} bytes across {} rows, past the \
             {remaining} bytes left of the load budget",
            raw.len()
        );
    }

    // A FAILED stack charges nothing. Rows decoded before the failure - whole earlier
    // batches, plus whichever blobs of the failing batch got through - hold their bytes
    // against the budget, and every one of them is dropped when this returns `Err`: the
    // caller substitutes `Vec::new()` and none of those `String`s survives. Leaving them
    // charged would be an accounting lie with a user-visible cost, because the budget is
    // shared with the OTHER stack of the same open: one bad row late in a large undo
    // history would refuse a perfectly good redo stack for memory that is already free.
    //
    // `remaining` above is the high-water mark to restore. That subtraction is exact
    // rather than approximate because nothing else can be spending this budget at the
    // point we return: the two stacks of an open load sequentially, and `thread::scope`
    // has joined every decompression thread before `decoded` is inspected.
    let restore_on_failure = |budget: &InflationBudget| {
        budget.refund(remaining.saturating_sub(budget.remaining()));
    };

    let mut entries = Vec::with_capacity(raw.len());
    for batch in raw.chunks(rows_per_decompress_batch()) {
        let decoded: Result<Vec<_>> = std::thread::scope(|scope| {
            let handles: Vec<_> = batch
                .iter()
                .map(|(seq, cmd_bytes, undo_bytes, ts)| {
                    let h_cmd = scope.spawn(move || decompress_json_under(cmd_bytes, budget));
                    let h_undo = scope.spawn(move || decompress_json_under(undo_bytes, budget));
                    (*seq, h_cmd, h_undo, ts.clone())
                })
                .collect();
            handles
                .into_iter()
                .map(|(seq, h_cmd, h_undo, ts)| {
                    let cmd = h_cmd.join().map_err(|_| {
                        anyhow::anyhow!("{stack_name} decompress thread panicked")
                    })??;
                    let undo = h_undo.join().map_err(|_| {
                        anyhow::anyhow!("{stack_name} decompress thread panicked")
                    })??;
                    Ok((seq, cmd, undo, ts))
                })
                .collect()
        });
        match decoded {
            Ok(rows) => entries.extend(rows),
            Err(e) => {
                drop(entries);
                restore_on_failure(budget);
                return Err(e);
            }
        }
    }
    Ok(entries)
}

impl Database {
    pub fn replace_history_stacks_with_timestamps(
        &self,
        undo_entries: Vec<(String, String, Option<String>)>,
        redo_entries: Vec<(String, String, Option<String>)>,
    ) -> Result<()> {
        let compress = |rows: Vec<(String, String, Option<String>)>| -> Vec<StoredHistoryRow> {
            rows.into_iter()
                .map(
                    |(command_json, undo_data_json, timestamp)| StoredHistoryRow {
                        command: compress_json(&command_json).into(),
                        undo_data: compress_json(&undo_data_json).into(),
                        timestamp,
                    },
                )
                .collect()
        };
        self.replace_history_stacks_stored(&compress(undo_entries), &compress(redo_entries))
    }

    /// [`Self::replace_history_stacks_with_timestamps`] for rows that are
    /// already compressed ([`StoredHistoryRow`]): the same transaction, the
    /// same `DELETE`-all then `INSERT` with `sequence` renumbered 0..n, the
    /// same bytes.
    pub fn replace_history_stacks_stored(
        &self,
        undo_entries: &[StoredHistoryRow],
        redo_entries: &[StoredHistoryRow],
    ) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .context("begin undo/redo replacement transaction")?;
        tx.execute("DELETE FROM undo_stack", [])?;
        tx.execute("DELETE FROM redo_stack", [])?;

        for (table, rows) in [("undo_stack", undo_entries), ("redo_stack", redo_entries)] {
            let sql = format!(
                "INSERT INTO {table} (sequence, command_json, undo_data_json, executed_at) \
                 VALUES (?1, ?2, ?3, ?4)"
            );
            for (seq, row) in rows.iter().enumerate() {
                tx.execute(
                    &sql,
                    params![
                        seq as i64,
                        &row.command[..],
                        &row.undo_data[..],
                        row.timestamp
                            .clone()
                            .unwrap_or_else(|| Utc::now().to_rfc3339()),
                    ],
                )?;
            }
        }
        tx.commit().context("commit undo/redo replacement")?;
        Ok(())
    }

    pub fn clear_history_stacks(&self) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .context("begin undo/redo clear transaction")?;
        tx.execute("DELETE FROM undo_stack", [])?;
        tx.execute("DELETE FROM redo_stack", [])?;
        tx.commit().context("commit undo/redo clear")?;
        Ok(())
    }

    /// Newest undo timestamp persisted in this database, without decompressing payloads.
    pub fn newest_undo_timestamp(&self) -> Result<Option<DateTime<Utc>>> {
        let raw = self
            .conn
            .query_row(
                "SELECT executed_at FROM undo_stack ORDER BY sequence DESC LIMIT 1",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        raw.map(|stamp| {
            DateTime::parse_from_rfc3339(&stamp)
                .map(|parsed| parsed.with_timezone(&Utc))
                .with_context(|| format!("invalid newest undo timestamp `{stamp}`"))
        })
        .transpose()
    }

    /// [#469] The newest `max_rows` rows of a history table, OLDEST-first.
    ///
    /// The `ORDER BY sequence DESC LIMIT ?1` is the whole point: an ascending `SELECT`
    /// with a `LIMIT` would keep the OLDEST window, which is the half a restore throws
    /// away, and an unlimited one is what let a crafted table be read whole. The result
    /// is reversed back to ascending so every consumer downstream sees the order it
    /// always saw.
    fn newest_history_rows(
        &self,
        table: &'static str,
        max_rows: usize,
    ) -> Result<Vec<CompressedHistoryRow>> {
        // Clamp HERE rather than trusting the caller: `max_rows` is config-derived on the
        // load paths (`undo_history_limit` is an unclamped `usize` in `config.toml`), so
        // the ceiling only binds a future caller if the loader applies it itself.
        let limit = i64::try_from(max_rows.min(MAX_PERSISTED_HISTORY_ROWS)).unwrap_or(i64::MAX);

        // [#473] The COMPRESSED bytes of this window are NOT bounded here, and an attempt
        // to bound them was reverted rather than repaired. Recording why, because the
        // gap is real and the obvious fix is worse than it looks.
        //
        // The gap: every other bound in this file is about inflation, and none can see
        // this term - the window pre-scan and the shared budget both run on `raw`, which
        // is already fully materialised by the time they get it. A hostile file whose
        // blobs are large but declare a tiny ISIZE evades every inflation bound and still
        // costs one copy of its history tables in RAM, bounded only by the row cap times
        // sqlite's ~1 GB `SQLITE_MAX_LENGTH`. It is linear in file size - no
        // amplification, so no bomb - and it predates #473 (#469 introduced this reader).
        //
        // The attempted fix was `SUM(length(command_json) + length(undo_data_json))` over
        // `(SELECT ... ORDER BY sequence DESC LIMIT ?1)`. It cost 4238x what it looked
        // like. sqlite's header-only `length()` optimisation needs a DIRECT table-column
        // reference; applied to a subquery output it cannot flatten, so the co-routine
        // materialises every blob and the whole history is read TWICE per open. Measured
        // on a 400 MB history: 46,616 us for that shape against 11 us for
        // `WHERE id IN (SELECT id ... LIMIT ?1)`, identical sums.
        //
        // So if this is ever bounded, it must be by the id-set shape, must charge the
        // shared `InflationBudget` rather than the bare constant (or it is per-stack and
        // an open reaches 2x), must propagate its error instead of failing open, must use
        // `octet_length` (plain `length` counts CHARACTERS on a TEXT-typed row), and must
        // have a test - the reverted version had none, and deleting it whole broke
        // nothing. Streaming the sum inside the existing row loop avoids the second read
        // entirely and is the better starting point.
        let mut stmt = self.conn.prepare(&format!(
            "SELECT sequence, command_json, undo_data_json, executed_at FROM {table} \
             ORDER BY sequence DESC LIMIT ?1"
        ))?;
        let rows = stmt.query_map(params![limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        let mut raw: Vec<CompressedHistoryRow> = Vec::new();
        for row in rows {
            raw.push(row?);
        }
        raw.reverse();
        Ok(raw)
    }

    /// Load undo stack from database
    ///
    /// [#469] Bounded by [`MAX_PERSISTED_HISTORY_ROWS`]; see
    /// [`Self::load_undo_stack_capped`].
    ///
    /// [#473] NOT for production use - it mints its own [`InflationBudget`].
    ///
    /// Every loader that mints its own budget bounds only itself, so two of them in one
    /// open is 2 GiB under a constant that says 1 GiB - the defect this issue fixed. A
    /// project open must use the `_within` variants and share one budget across both
    /// stacks (`Database::load_history_budget`).
    ///
    /// This carried `#[cfg(test)]` while the backend was one crate, which made a
    /// production caller a COMPILE error. The workspace split ended that: `#[cfg(test)]`
    /// is per-crate, and `stalagmight-core`'s own tests call these across the crate
    /// boundary, where the gate makes them invisible rather than test-only. The guard is
    /// therefore the structural tripwire in `core/state/undo_policy.rs`, which pins the
    /// `_within` call count and the one-budget-per-load-path count in `project_load.rs` -
    /// weaker than a compile error, and the reason this doc says so out loud.
    pub fn load_undo_stack(&self) -> Result<Vec<(i64, String, String)>> {
        self.load_undo_stack_capped(MAX_PERSISTED_HISTORY_ROWS)
    }

    /// [#469] The legacy (no-timestamp) loader, reading at most the newest `max_rows`.
    ///
    /// Bounded for the same reason as its timestamped sibling. Its QUERY is separate
    /// rather than delegated - it names only the three columns the legacy shape
    /// guarantees, so a file old enough to lack `executed_at` still loads - but its
    /// DECOMPRESSION is not.
    ///
    /// [#473] It used to inflate the rows in a loop of its own, which is the shape where
    /// one path quietly acquires a bound the other lacks: the window pre-scan and the
    /// shared budget both live in `decompress_history_rows`, and a second implementation
    /// had neither until someone remembered to add them twice. Rows are collected, given
    /// an absent timestamp, and handed to the same helper as every other load.
    /// [#473] NOT for production use - see [`Self::load_undo_stack`].
    pub fn load_undo_stack_capped(&self, max_rows: usize) -> Result<Vec<(i64, String, String)>> {
        // Clamped to the ceiling inside the loader, exactly as `newest_history_rows` does.
        let limit = i64::try_from(max_rows.min(MAX_PERSISTED_HISTORY_ROWS)).unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare(
            "SELECT sequence, command_json, undo_data_json FROM undo_stack \
             ORDER BY sequence DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?;

        let mut raw: Vec<CompressedHistoryRow> = Vec::new();
        for row in rows {
            let (seq, cmd_bytes, undo_bytes) = row?;
            raw.push((seq, cmd_bytes, undo_bytes, None));
        }
        // Read newest-first for the LIMIT; hand back ascending sequence order.
        raw.reverse();
        Ok(
            decompress_history_rows_within(raw, "undo", &Self::load_history_budget())?
                .into_iter()
                .map(|(seq, command_json, undo_data_json, _)| (seq, command_json, undo_data_json))
                .collect(),
        )
    }

    /// [#473] A fresh inflation budget for ONE project open.
    ///
    /// Hand the SAME one to every stack of that open - `load_*_with_timestamps_within`
    /// take it for exactly this reason. A caller that instead lets each stack default to
    /// its own budget doubles the real ceiling while every doc states the single one, which is
    /// the defect this function exists to make hard to reintroduce.
    pub fn load_history_budget() -> InflationBudget {
        InflationBudget::new(MAX_HISTORY_LOAD_BYTES)
    }

    /// Load undo stack from database with timestamps
    ///
    /// [#469] Bounded by [`MAX_PERSISTED_HISTORY_ROWS`]. The load paths pass their own,
    /// tighter window to [`Self::load_undo_stack_with_timestamps_capped`]; this default is
    /// what a caller with no window of its own gets. The ceiling binds either way, because
    /// the capped loaders clamp `max_rows` to it themselves - a future caller handing them
    /// a config-derived number is bounded too.
    ///
    /// One reader does take the whole table, deliberately: `Database::cut_inherited_undo_rows`
    /// (persistence/sqlite/repair.rs) selects `id, executed_at` and nothing else, because its
    /// cut has to consider every row. That scan is bounded by the row count times two small
    /// columns and gunzips nothing, which is the property this ceiling exists to protect.
    ///
    /// [#473] NOT for production use - it mints its own budget; see [`Self::load_undo_stack`]
    /// for why the compile-time gate did not survive the workspace split.
    pub fn load_undo_stack_with_timestamps(
        &self,
    ) -> Result<Vec<(i64, String, String, Option<String>)>> {
        self.load_undo_stack_with_timestamps_capped(MAX_PERSISTED_HISTORY_ROWS)
    }

    /// [#469] Load at most the newest `max_rows` undo rows, oldest-first.
    ///
    /// `max_rows` is the count bound that will be in force when these rows are restored:
    /// the configured `undo_history_limit` for a count-window project, or
    /// [`MAX_PERSISTED_HISTORY_ROWS`] for a byte-budgeted (Full-mode) one. Rows past it
    /// are ones the restore's own `enforce_depth_limit` would evict, so not reading them
    /// costs no history - and stops a table with a million rows in it from being
    /// gunzipped into memory first.
    ///
    /// Dropping the oldest rows cannot corrupt sequence bookkeeping: the returned
    /// sequences are discarded by both load paths, and every save renumbers the table
    /// 0..n (`save_undo_stack_with_timestamps`).
    /// [#473] NOT for production use - see [`Self::load_undo_stack`]. Production opens use
    /// [`Self::load_undo_stack_with_timestamps_within`] and share the open's budget.
    pub fn load_undo_stack_with_timestamps_capped(
        &self,
        max_rows: usize,
    ) -> Result<Vec<(i64, String, String, Option<String>)>> {
        self.load_undo_stack_with_timestamps_within(max_rows, &Self::load_history_budget())
    }

    /// [#473] As [`Self::load_undo_stack_with_timestamps_capped`], sharing the open's
    /// inflation budget with the redo stack rather than taking a whole one to itself.
    ///
    /// A project open reads BOTH stacks and holds both results at once, so the budget has
    /// to span the pair; see [`Self::load_history_budget`].
    pub fn load_undo_stack_with_timestamps_within(
        &self,
        max_rows: usize,
        budget: &InflationBudget,
    ) -> Result<Vec<(i64, String, String, Option<String>)>> {
        // Decompress independent blobs in bounded batches. Large snapshot payloads
        // still use multiple cores, but a deep Full-mode history cannot create two
        // operating-system threads for every row at once.
        let raw = self.newest_history_rows("undo_stack", max_rows)?;
        decompress_history_rows_within(raw, "undo", budget)
    }

    /// TEST FIXTURE: replace the undo table with `entries` in the legacy (no-timestamp)
    /// shape, stamping every row with the current time.
    ///
    /// Not transactional - a DELETE, then one autocommit INSERT per row - so a failure
    /// part-way leaves half a history on disk. Production writes both stacks through
    /// [`Self::replace_history_stacks_with_timestamps`], in one transaction; this exists
    /// only to seed fixtures and is compiled for tests alone (B27(e), #576).
    #[cfg(any(test, feature = "test-support"))]
    pub fn save_undo_stack(&self, entries: Vec<(String, String)>) -> Result<()> {
        // Clear existing
        self.conn.execute("DELETE FROM undo_stack", [])?;

        // Save new entries
        for (seq, (command_json, undo_data_json)) in entries.iter().enumerate() {
            let cmd_compressed = compress_json(command_json);
            let undo_compressed = compress_json(undo_data_json);
            self.conn.execute(
                "INSERT INTO undo_stack (sequence, command_json, undo_data_json, executed_at) VALUES (?1, ?2, ?3, ?4)",
                params![seq as i64, cmd_compressed, undo_compressed, Utc::now().to_rfc3339()],
            )?;
        }

        tracing::debug!(count = entries.len(), "Saved undo stack to database");
        Ok(())
    }

    /// [#303] Empty the undo/redo tables in the `.stalx` at `path`, without opening it
    /// as a full [`Database`].
    ///
    /// For the download-replace chokepoint: a freshly-extracted server copy carries the
    /// pusher's history (Full mode ships it deliberately), and every implicit adopt path
    /// must drop it - see `InheritedUndo`. Deliberately a BARE connection rather than
    /// `open_existing`: this runs on a file that is about to be opened properly, and
    /// `open_existing` would run the integrity check and the whole migration chain on it
    /// twice, as well as taking a read-write connection with its 256 MB mmap.
    ///
    /// A missing table is not an error - an older snapshot shape simply has none.
    pub fn clear_undo_history_at(path: &std::path::Path) -> Result<()> {
        let conn = rusqlite::Connection::open(path)
            .with_context(|| format!("open {} to clear inherited undo", path.display()))?;
        // The file may come from a server: refuse a hostile schema before a
        // DELETE fires a trigger it carries (#608).
        super::check_schema(&conn)?;
        let tx = conn
            .unchecked_transaction()
            .with_context(|| format!("begin inherited-history cleanup in {}", path.display()))?;
        for table in [
            "undo_stack",
            "redo_stack",
            "speleo_undo_chain",
            "speleo_undo_watermark",
        ] {
            let exists: i64 = tx.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                params![table],
                |row| row.get(0),
            )?;
            if exists > 0 {
                tx.execute(&format!("DELETE FROM {table}"), [])
                    .with_context(|| format!("clear {table} in {}", path.display()))?;
            }
        }
        tx.commit()
            .with_context(|| format!("commit inherited-history cleanup in {}", path.display()))?;
        Ok(())
    }

    /// TEST FIXTURE: replace the undo table with `entries`, `sequence` renumbered 0..n.
    ///
    /// Not transactional - a DELETE, then one autocommit INSERT per row - so a failure
    /// part-way leaves half a history on disk. The save pipeline does not use it: it
    /// writes both stacks through [`Self::replace_history_stacks_with_timestamps`], in
    /// one transaction (see `UndoStack::rows_to_persist` in core for why every save is a
    /// full rewrite). Compiled for tests alone (B27(e), #576).
    #[cfg(any(test, feature = "test-support"))]
    pub fn save_undo_stack_with_timestamps(
        &self,
        entries: Vec<(String, String, Option<String>)>,
    ) -> Result<()> {
        // Clear existing
        self.conn.execute("DELETE FROM undo_stack", [])?;

        // Save new entries
        for (seq, (command_json, undo_data_json, timestamp_str)) in entries.iter().enumerate() {
            // Use provided timestamp or fall back to current time
            let executed_at = timestamp_str
                .clone()
                .unwrap_or_else(|| Utc::now().to_rfc3339());
            let cmd_compressed = compress_json(command_json);
            let undo_compressed = compress_json(undo_data_json);
            self.conn.execute(
                "INSERT INTO undo_stack (sequence, command_json, undo_data_json, executed_at) VALUES (?1, ?2, ?3, ?4)",
                params![seq as i64, cmd_compressed, undo_compressed, executed_at],
            )?;
        }

        tracing::debug!(
            count = entries.len(),
            "Saved undo stack with timestamps to database"
        );
        Ok(())
    }

    /// Load redo stack from database with timestamps
    ///
    /// [#469] Bounded by [`MAX_PERSISTED_HISTORY_ROWS`], exactly as the undo side.
    ///
    /// [#473] NOT for production use - see [`Self::load_undo_stack`].
    pub fn load_redo_stack_with_timestamps(
        &self,
    ) -> Result<Vec<(i64, String, String, Option<String>)>> {
        self.load_redo_stack_with_timestamps_capped(MAX_PERSISTED_HISTORY_ROWS)
    }

    /// [#469] Load at most the newest `max_rows` redo rows, oldest-first.
    ///
    /// The redo table gets the same treatment as the undo one because the bound it is
    /// read under is the same: `enforce_depth_limit` applies the active count or byte
    /// limit to BOTH stacks independently, and `.stalx` persists both.
    /// [#473] NOT for production use - see [`Self::load_undo_stack`].
    pub fn load_redo_stack_with_timestamps_capped(
        &self,
        max_rows: usize,
    ) -> Result<Vec<(i64, String, String, Option<String>)>> {
        self.load_redo_stack_with_timestamps_within(max_rows, &Self::load_history_budget())
    }

    /// [#473] As above, sharing the open's inflation budget with the undo stack.
    ///
    /// This is the half that made the defect visible: an open that loads undo and then
    /// redo with a full budget each is bounded at 2x the constant, not 1x.
    pub fn load_redo_stack_with_timestamps_within(
        &self,
        max_rows: usize,
        budget: &InflationBudget,
    ) -> Result<Vec<(i64, String, String, Option<String>)>> {
        // Parallel gzip decompression of the per-entry blobs - see
        // load_undo_stack_with_timestamps_capped for the rationale.
        let raw = self.newest_history_rows("redo_stack", max_rows)?;
        decompress_history_rows_within(raw, "redo", budget)
    }

    /// TEST FIXTURE: the redo twin of [`Self::save_undo_stack_with_timestamps`] - same
    /// shape, same non-transactional caveat. Production writes the redo table through
    /// [`Self::replace_history_stacks_with_timestamps`]. Compiled for tests alone
    /// (B27(e), #576).
    #[cfg(any(test, feature = "test-support"))]
    pub fn save_redo_stack_with_timestamps(
        &self,
        entries: Vec<(String, String, Option<String>)>,
    ) -> Result<()> {
        self.conn.execute("DELETE FROM redo_stack", [])?;

        for (seq, (command_json, undo_data_json, timestamp_str)) in entries.iter().enumerate() {
            let executed_at = timestamp_str
                .clone()
                .unwrap_or_else(|| Utc::now().to_rfc3339());
            let cmd_compressed = compress_json(command_json);
            let undo_compressed = compress_json(undo_data_json);
            self.conn.execute(
                "INSERT INTO redo_stack (sequence, command_json, undo_data_json, executed_at) VALUES (?1, ?2, ?3, ?4)",
                params![seq as i64, cmd_compressed, undo_compressed, executed_at],
            )?;
        }

        Ok(())
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "unit test fixtures fail loudly on setup and readback errors"
)]
mod tests {
    use super::*;

    /// B27(e) (#576): the per-stack history writers are test fixtures, not production API.
    ///
    /// The save pipeline writes history through `replace_history_stacks_with_timestamps`:
    /// both stacks, one transaction. The per-stack writers DELETE and then INSERT row by
    /// row on autocommit, so a production caller would leave half a history behind on any
    /// mid-loop failure. They survive only to seed test fixtures, so they compile only
    /// under `test` / `test-support`, and the two with no caller at all are gone. The
    /// compiler is the real guard - a production caller no longer builds - and this pins
    /// the gate itself so it cannot be dropped quietly.
    #[test]
    fn the_non_transactional_history_writers_are_test_only() {
        const GATE: &str = "#[cfg(any(test, feature = \"test-support\"))]";
        let source = include_str!("undo.rs");
        let production = source
            .split_once("\nmod tests {")
            .map_or(source, |(before, _)| before);
        for removed in ["save_undo_entry", "clear_undo_after"] {
            assert!(
                !production.contains(&format!("pub fn {removed}(")),
                "{removed} has no caller and must stay deleted"
            );
        }
        for fixture in [
            "save_undo_stack",
            "save_undo_stack_with_timestamps",
            "save_redo_stack_with_timestamps",
        ] {
            let found = production.find(&format!("pub fn {fixture}("));
            assert!(
                found.is_some(),
                "{fixture} is no longer defined in this file"
            );
            let Some(at) = found else { continue };
            let attribute = production[..at]
                .lines()
                .rev()
                .map(str::trim)
                .find(|line| !line.is_empty());
            assert_eq!(
                attribute,
                Some(GATE),
                "{fixture} is a non-transactional writer and must be compiled for tests only"
            );
        }
    }

    #[test]
    fn decompression_batch_never_exceeds_the_thread_cap() {
        assert!(rows_per_decompress_batch() >= 1);
        assert!(rows_per_decompress_batch() * 2 <= MAX_DECOMPRESS_THREADS);
    }

    /// [#469] A table with far more rows than the bound loads only the NEWEST window,
    /// in ascending sequence order.
    ///
    /// Newest, because that is the end a restore keeps: `enforce_depth_limit` evicts
    /// from the front. Ascending, because every consumer downstream reads stack order
    /// off the returned vector, and a `DESC` result would restore the history reversed.
    #[test]
    fn a_bloated_history_table_loads_only_the_newest_window_in_ascending_order() {
        const ROWS: i64 = 250;
        let db = Database::in_memory().unwrap();
        let rows: Vec<_> = (0..ROWS)
            .map(|i| (format!(r#"{{"n":{i}}}"#), format!(r#"{{"u":{i}}}"#), None))
            .collect();
        db.save_undo_stack_with_timestamps(rows.clone()).unwrap();
        db.save_redo_stack_with_timestamps(rows).unwrap();

        let undo = db.load_undo_stack_with_timestamps_capped(10).unwrap();
        assert_eq!(
            undo.iter().map(|(seq, ..)| *seq).collect::<Vec<_>>(),
            (240..250).collect::<Vec<_>>(),
            "the cap must keep the newest rows, in ascending sequence order"
        );
        assert_eq!(undo.first().unwrap().1, r#"{"n":240}"#);
        assert_eq!(undo.last().unwrap().1, r#"{"n":249}"#);

        let redo = db.load_redo_stack_with_timestamps_capped(3).unwrap();
        assert_eq!(
            redo.iter().map(|(seq, ..)| *seq).collect::<Vec<_>>(),
            vec![247, 248, 249],
            "the redo table is bounded the same way as the undo one"
        );

        let legacy = db.load_undo_stack_capped(4).unwrap();
        assert_eq!(
            legacy.iter().map(|(seq, ..)| *seq).collect::<Vec<_>>(),
            vec![246, 247, 248, 249],
            "the legacy no-timestamp loader is bounded too, and returns ascending"
        );
        assert_eq!(legacy.last().unwrap().1, r#"{"n":249}"#);

        // A cap the table does not reach changes nothing - the exact-restore behaviour
        // every other test in the suite relies on.
        assert_eq!(
            db.load_undo_stack_with_timestamps_capped(250)
                .unwrap()
                .len(),
            250
        );
        assert_eq!(db.load_undo_stack_with_timestamps().unwrap().len(), 250);
        assert_eq!(db.load_redo_stack_with_timestamps().unwrap().len(), 250);
        assert_eq!(db.load_undo_stack().unwrap().len(), 250);
        assert_eq!(
            db.load_undo_stack().unwrap().first().unwrap().1,
            r#"{"n":0}"#,
            "an unbounded-in-practice load still starts at the oldest row"
        );
        // A zero window reads nothing rather than everything.
        assert!(db
            .load_undo_stack_with_timestamps_capped(0)
            .unwrap()
            .is_empty());
    }

    // The [#469] "ceiling is derived from the byte budget" test lives with
    // `core::undo` in the root crate now: it asserts against `UndoStack`'s
    // entry costing, which this crate cannot see.

    #[test]
    fn replacing_both_history_stacks_rolls_back_when_redo_insert_fails() {
        let db = Database::in_memory().unwrap();
        let old = |name: &str| {
            (
                format!(r#"{{"type":"{name}"}}"#),
                r#"{"kind":"old"}"#.to_owned(),
                Some("2026-08-01T00:00:00+00:00".to_owned()),
            )
        };
        db.save_undo_stack_with_timestamps(vec![old("OldUndo")])
            .unwrap();
        db.save_redo_stack_with_timestamps(vec![old("OldRedo")])
            .unwrap();
        db.conn
            .execute_batch(
                "CREATE TRIGGER reject_redo_insert BEFORE INSERT ON redo_stack \
                 BEGIN SELECT RAISE(FAIL, 'redo rejected'); END;",
            )
            .unwrap();

        let result =
            db.replace_history_stacks_with_timestamps(vec![old("NewUndo")], vec![old("NewRedo")]);

        assert!(result.is_err());
        assert_eq!(
            db.load_undo_stack_with_timestamps().unwrap()[0].1,
            r#"{"type":"OldUndo"}"#
        );
        assert_eq!(
            db.load_redo_stack_with_timestamps().unwrap()[0].1,
            r#"{"type":"OldRedo"}"#
        );
    }
}
