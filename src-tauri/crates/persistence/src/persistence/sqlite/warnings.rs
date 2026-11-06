use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};
use std::collections::HashMap;

use super::{Database, ResolvedWarning};

/// The columns without which a `resolved_warnings` row says nothing: which shot
/// it is about, and which warning on it was ignored. `resolved_comment` and
/// `shot_uuid` are optional - a file that never ran the migration adding them
/// still carries usable ignores - but these four are the row itself.
const REQUIRED_RESOLVED_COLUMNS: [&str; 4] =
    ["section_id", "shot_id", "warning_code", "resolved_at"];

/// Ceiling on a carried `resolved_comment`, in chars. The resolve dialog writes
/// a sentence; this is three orders of magnitude above that, and exists only so
/// a hand-written 100 MB cell cannot be re-serialized into every future save and
/// every SpeleoDB push (`resolved_warnings` is NOT stripped from the push
/// snapshot - see `core::state::warnings::save_resolved_warning_gated`).
const MAX_RESOLVED_COMMENT_CHARS: usize = 10_000;

/// Ceiling on a carried `warning_code` / `shot_uuid`, in chars. Real values are
/// ~30 and 36. Deliberately a LENGTH clamp and nothing else: a code this build
/// does not recognise may well come from a newer version, and dropping it is
/// precisely the loss this whole path exists to prevent.
const MAX_RESOLVED_KEY_CHARS: usize = 512;

/// Ceiling on how many rows one carry-over will EXAMINE. Above this the file is
/// not a survey's worth of "Ignore" clicks any more, and the cost is paid on
/// every save, every autosave and every push. Generous by ~2 orders of magnitude
/// against any real project.
///
/// Deliberately counted over rows PROCESSED, not rows kept: a table of a million
/// rows that all fail [`coerce_id`] keeps none of them, so a cap on the kept
/// count would never trip and the whole table would be scanned on every save -
/// which is the work this exists to bound. The consequence, stated plainly: past
/// this many rows the read stops, so usable rows sitting BEHIND a wall of junk
/// are not carried. That trade only ever applies to a file no user made.
pub const MAX_CARRIED_RESOLVED_WARNINGS: usize = 100_000;

/// Ceiling on an id cell's TEXT, in bytes, applied before it is decoded.
///
/// A `u32` is at most 10 digits, so anything past this is not an id under any
/// reading. Checked first because `from_utf8` + `trim` + `parse` are each O(n)
/// over the cell: without it, one multi-megabyte cell costs that much work per
/// row, per save, to reach the same `None`.
const MAX_ID_TEXT_BYTES: usize = 64;

/// Read a text-ish cell as a bounded `String`, or `None` for SQL NULL.
///
/// SQLite columns are UNTYPED - a declared `TEXT` column holds whatever was put
/// in it - so a hand-written or hostile file can park a BLOB or an INTEGER here.
/// `row.get::<String>` answers that with `InvalidColumnType`, which on the save
/// path would mean a permanently unsaveable project (the read failure refuses
/// the save). Coerce instead: the value is carried inert rather than costing the
/// user their file.
///
/// The byte slice is truncated BEFORE the lossy conversion so a huge cell is
/// never materialised in full; `from_utf8_lossy` turns any partial trailing
/// char into U+FFFD, which is exactly what a clamp should do.
fn coerce_text(value: rusqlite::types::ValueRef<'_>, max_chars: usize) -> Option<String> {
    let bounded = |bytes: &[u8]| -> String {
        let budget = bytes.len().min(max_chars.saturating_mul(4));
        String::from_utf8_lossy(bytes.get(..budget).unwrap_or(bytes))
            .chars()
            .take(max_chars)
            .collect()
    };
    match value {
        rusqlite::types::ValueRef::Null => None,
        rusqlite::types::ValueRef::Text(t) | rusqlite::types::ValueRef::Blob(t) => Some(bounded(t)),
        rusqlite::types::ValueRef::Integer(i) => Some(i.to_string()),
        rusqlite::types::ValueRef::Real(f) => Some(f.to_string()),
    }
}

/// Read an id cell as a `u32`, or `None` when it names nothing usable.
///
/// TEXT is parsed because a hand-edited file plausibly quotes its numbers. REAL,
/// BLOB and NULL are not: an id is a lookup key into the live project, and a
/// value that is not one identifies no shot and no section, so the row it keys
/// can never suppress anything. Those rows are counted and dropped rather than
/// guessed at - see `read_resolved_warnings`.
fn coerce_id(value: rusqlite::types::ValueRef<'_>) -> Option<u32> {
    match value {
        rusqlite::types::ValueRef::Integer(i) => u32::try_from(i).ok(),
        // Length first, so an oversized cell is rejected in O(1) instead of
        // being validated and parsed in full on the way to the same answer.
        rusqlite::types::ValueRef::Text(t) if t.len() <= MAX_ID_TEXT_BYTES => {
            std::str::from_utf8(t).ok()?.trim().parse::<u32>().ok()
        }
        _ => None,
    }
}

impl Database {
    /// Load resolved warnings from database
    pub fn load_resolved_warnings(&self) -> Result<Vec<ResolvedWarning>> {
        // Check if table exists (for backwards compatibility with older files)
        let table_exists: bool = self.conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='resolved_warnings'",
            [],
            |row| row.get::<_, i64>(0),
        ).map(|c| c > 0).unwrap_or(false);

        if !table_exists {
            return Ok(Vec::new());
        }

        self.read_resolved_warnings("resolved_comment", "shot_uuid")
    }

    /// [#517] Read `resolved_warnings` taking whatever shape the file actually
    /// has, rather than the shape this build writes.
    ///
    /// `load_resolved_warnings` names every CURRENT column, so a single missing
    /// one fails the whole read - most plausibly `shot_uuid` on a file that never
    /// ran the v8 `ALTER`, which a forged `PRAGMA user_version` over pre-v8
    /// tables can still produce (the migration chain fast-paths on the stamp).
    /// A degraded READ is a fair answer there; a degraded read on the SAVE path
    /// is not, because the save REBUILDS this table from what it reads, so
    /// "could not parse" arriving as "no ignores" erases the user's whole list.
    /// This reads the rows verbatim instead: the optional columns are selected
    /// only when present, and their absence yields `None`, exactly as a NULL
    /// would.
    ///
    /// Errors are reserved for STRUCTURAL trouble - a missing
    /// [`REQUIRED_RESOLVED_COLUMNS`] column, a probe or a scan the connection
    /// refuses (a locked database, a corrupt page, an I/O fault) - so the caller
    /// can tell "no ignores" from "cannot tell". A single unreadable VALUE is
    /// explicitly NOT one of them; see [`coerce_text`]. A table that does not
    /// exist at all is not an error either: it is a file from before the
    /// feature, and it truly has no ignores.
    pub fn load_resolved_warnings_verbatim(&self) -> Result<Vec<ResolvedWarning>> {
        // Probe and read under ONE deferred transaction so the shape cannot
        // change between "which columns exist" and "SELECT them" - a race that
        // would surface as a spurious refusal on the save path. Best-effort on
        // purpose: if a transaction cannot be started (one is already open), the
        // read still runs, because turning this hardening into a new failure
        // mode would defeat its own point.
        let _snapshot = self.conn.unchecked_transaction().ok();

        // `table_exists` filters `type='table'`, so a VIEW named
        // `resolved_warnings` reads as absent and the save carries nothing. That
        // is the safe direction and it costs no real data: the save then writes
        // a genuine table into the fresh file, whereas selecting through a view
        // could not be re-inserted into one anyway.
        if !self.table_exists("resolved_warnings")? {
            return Ok(Vec::new());
        }

        let mut columns = std::collections::HashSet::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT name FROM pragma_table_info('resolved_warnings')")?;
            let names = stmt.query_map([], |row| row.get::<_, String>(0))?;
            for name in names {
                columns.insert(name?);
            }
        }

        for required in REQUIRED_RESOLVED_COLUMNS {
            anyhow::ensure!(
                columns.contains(required),
                "the resolved_warnings table has no {required} column"
            );
        }

        // A literal NULL stands in for a column this file does not have, so the
        // row mapping below is identical either way.
        let comment = if columns.contains("resolved_comment") {
            "resolved_comment"
        } else {
            "NULL"
        };
        let uuid = if columns.contains("shot_uuid") {
            "shot_uuid"
        } else {
            "NULL"
        };
        self.read_resolved_warnings(comment, uuid)
    }

    /// Shared row reader for the two loads above. `comment_expr` / `uuid_expr` are
    /// column names this crate chose (or the literal `NULL`), never user input.
    ///
    /// Every CELL is coerced and clamped rather than type-checked (see
    /// [`coerce_text`]): the only failures that escape are STRUCTURAL - a step
    /// error from a corrupt page or an I/O fault - because those are the ones
    /// that mean "this table cannot be read", as opposed to "this row is
    /// nonsense". The save path turns the first into a refusal and must not be
    /// handed the second, or one poisoned row would make a project permanently
    /// unsaveable.
    fn read_resolved_warnings(
        &self,
        comment_expr: &str,
        uuid_expr: &str,
    ) -> Result<Vec<ResolvedWarning>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT section_id, shot_id, warning_code, resolved_at, {comment_expr}, {uuid_expr} \
             FROM resolved_warnings"
        ))?;

        let rows = stmt.query_map([], |row| {
            // A row that names no shot, or no warning, cannot suppress anything
            // on any project - carrying it forward would preserve nothing.
            let (Some(section_id), Some(shot_id), Some(warning_code)) = (
                coerce_id(row.get_ref(0)?),
                coerce_id(row.get_ref(1)?),
                coerce_text(row.get_ref(2)?, MAX_RESOLVED_KEY_CHARS),
            ) else {
                return Ok(None);
            };
            // An unparseable timestamp already fell back to "now"; a non-text
            // one now joins it rather than failing the read.
            let resolved_at = coerce_text(row.get_ref(3)?, MAX_RESOLVED_KEY_CHARS)
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                .map_or_else(Utc::now, |dt| dt.with_timezone(&Utc));
            Ok(Some(ResolvedWarning {
                section_id,
                shot_id,
                warning_code,
                resolved_at,
                resolved_comment: coerce_text(row.get_ref(4)?, MAX_RESOLVED_COMMENT_CHARS),
                // NULL for pre-v8 rows; an empty string is treated as absent too.
                shot_uuid: coerce_text(row.get_ref(5)?, MAX_RESOLVED_KEY_CHARS)
                    .filter(|s| !s.is_empty()),
            }))
        })?;

        let mut resolved = Vec::new();
        let mut unusable = 0usize;
        let mut examined = 0usize;
        let mut capped = false;
        for row in rows {
            examined += 1;
            match row? {
                Some(r) => resolved.push(r),
                None => unusable += 1,
            }
            // Stop READING at the ceiling, rather than reading everything and
            // truncating: the point is to bound the work, not just the output.
            // Counted over rows EXAMINED - a table of nothing but unusable rows
            // grows `resolved` not at all, so a kept-count cap would never trip
            // and would leave the scan unbounded, which is the whole exposure.
            if examined >= MAX_CARRIED_RESOLVED_WARNINGS {
                capped = true;
                break;
            }
        }
        if unusable > 0 {
            tracing::warn!(
                unusable,
                purpose = "read the ignore list from a file of unknown provenance",
                "Skipped resolved-warning rows that name no shot or no warning code"
            );
        }
        if capped {
            tracing::warn!(
                cap = MAX_CARRIED_RESOLVED_WARNINGS,
                examined,
                carried = resolved.len(),
                purpose = "bound the ignore list a save re-serializes",
                "Stopped reading resolved_warnings at the cap; any rows past it were not read"
            );
        }

        Ok(resolved)
    }

    /// Save a resolved warning to database.
    ///
    /// Suppression is keyed by the shot's stable `shot_uuid` (issue #57). To
    /// keep a single row per suppressed `(shot, warning_code)`, we first delete
    /// any prior row for the same identity (the UUID when present, else the
    /// legacy `(section_id, shot_id)` position), then insert the fresh record.
    /// Falls back to the legacy position key when the shot has no UUID. Legacy
    /// NULL-uuid rows are upgraded to the UUID key by the load-time backfill
    /// (`backfill_resolved_warning_uuids`), not here.
    pub fn save_resolved_warning(&self, warning: &ResolvedWarning) -> Result<()> {
        // The legacy UNIQUE(section_id, shot_id, warning_code) keys rows by
        // POSITION, but a UUID-keyed ignore's real identity is its shot_uuid. A
        // plain INSERT OR REPLACE would, on a positional collision, delete a
        // DIFFERENT live shot's by-UUID ignore that occupies that slot - e.g. after
        // a save remapped rows to sequence positions while the open project still
        // uses pre-save shot ids - silently losing it (issue #57 / Codex). Use a
        // conditional upsert that only ever overwrites the SAME shot's row (or
        // upgrades a legacy NULL-uuid row at that position), never a different
        // shot's UUID-keyed row. `excluded` is the row we tried to insert.
        //
        // The dedup-delete + insert run in ONE transaction: if the insert is
        // refused (the slot is held by a different shot), dropping the transaction
        // rolls the delete back too, so this shot's OWN prior ignore is preserved
        // rather than dropped on the way to refusing (issue #57 / Codex).
        let tx = self.conn.unchecked_transaction()?;
        if warning.shot_uuid.is_some() {
            Self::delete_matching_resolved(&tx, warning)?;
        } else {
            // [#576 FZ-PERSIST-1] A POSITION-keyed ignore (its shot's uuid is not
            // on disk yet) replaces only a position-keyed row in its slot. A
            // uuid-keyed row there belongs to whichever shot that uuid names -
            // after a save re-keyed the ignores to saved positions while the
            // session kept its ids, a different shot - and deleting it lost that
            // shot's ignore. Left in place, it makes the guarded upsert below
            // refuse this insert instead, as it does for a uuid-keyed one.
            tx.execute(
                "DELETE FROM resolved_warnings WHERE warning_code = ?1 \
                 AND section_id = ?2 AND shot_id = ?3 AND shot_uuid IS NULL",
                params![warning.warning_code, warning.section_id, warning.shot_id],
            )?;
        }
        let affected = tx.execute(
            "INSERT INTO resolved_warnings (section_id, shot_id, warning_code, resolved_at, resolved_comment, shot_uuid) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(section_id, shot_id, warning_code) DO UPDATE SET \
                 resolved_at = excluded.resolved_at, \
                 resolved_comment = excluded.resolved_comment, \
                 shot_uuid = excluded.shot_uuid \
             WHERE resolved_warnings.shot_uuid IS NULL \
                OR resolved_warnings.shot_uuid = excluded.shot_uuid",
            params![
                warning.section_id,
                warning.shot_id,
                warning.warning_code,
                warning.resolved_at.to_rfc3339(),
                warning.resolved_comment,
                warning.shot_uuid,
            ],
        )?;
        if affected == 0 {
            // The positional slot is held by a DIFFERENT shot's UUID-keyed ignore
            // (a stale position from a prior save, before the ids re-sync on
            // reload). Roll back (drop without commit) so neither ignore is lost,
            // and surface the failure so the caller can prompt a reload + retry.
            anyhow::bail!(
                "could not record this ignore: its position is held by another shot's ignore - reload the project and try again"
            );
        }
        tx.commit()?;
        Ok(())
    }

    /// Whether `uuid` is already stored in the on-disk `shots` table FOR `section_id`. A pre-v6
    /// shot's UUID is minted only in memory until the project is saved, so keying a new ignore
    /// by such an unpersisted UUID would orphan the row on the next open (different UUIDs are
    /// minted) - the resolve path keeps those ignores position-keyed instead (issue #57 / Codex).
    ///
    /// Scoped by section for the same reason `delete_matching_resolved` scopes its UUID branch:
    /// `ensure_unique_shot_uuids` only de-dups WITHIN a section, so two sections can hold the
    /// same shot UUID. An unscoped `EXISTS` would answer "persisted" because the OTHER section's
    /// copy is on disk, and the caller would then key an ignore by a UUID that is still
    /// in-memory-only for the section it actually belongs to - orphaning it on the next open,
    /// which is precisely what this guard exists to prevent.
    pub fn shot_uuid_is_persisted(&self, section_id: u32, uuid: &str) -> Result<bool> {
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM shots WHERE uuid = ?1 AND section_id = ?2)",
            params![uuid, section_id],
            |row| row.get(0),
        )?;
        Ok(exists)
    }

    /// Delete any resolved-warning rows matching `warning`. When a `shot_uuid`
    /// is present we match its own (section_id, uuid) row, plus any *legacy*
    /// NULL-uuid row at the same `(section_id, shot_id)` - but NEVER a *different*
    /// shot's valid by-UUID row that happens to occupy that position after a
    /// renumber (the `shot_uuid IS NULL` guard on the position branch ensures
    /// that). The UUID branch is constrained by `section_id` because
    /// `ensure_unique_shot_uuids` only de-dups within a section, so two sections
    /// can share a shot UUID - without the section guard, ignoring/un-ignoring on
    /// one section would clear the other's suppression too (issue #57 / Codex).
    /// This lets un-ignoring a live shot still clear a legacy position-keyed
    /// suppression of that same shot even on a load path that did not run the
    /// backfill. When no UUID is supplied (a legacy row), match purely by
    /// position. Shared by save (dedup) and unresolve (clear).
    fn delete_matching_resolved(conn: &Connection, warning: &ResolvedWarning) -> Result<()> {
        if let Some(uuid) = warning.shot_uuid.as_deref() {
            conn.execute(
                "DELETE FROM resolved_warnings WHERE warning_code = ?1 \
                 AND section_id = ?3 \
                 AND (shot_uuid = ?2 \
                      OR (shot_uuid IS NULL AND shot_id = ?4))",
                params![
                    warning.warning_code,
                    uuid,
                    warning.section_id,
                    warning.shot_id
                ],
            )?;
        } else {
            conn.execute(
                "DELETE FROM resolved_warnings WHERE warning_code = ?1 \
                 AND section_id = ?2 AND shot_id = ?3",
                params![warning.warning_code, warning.section_id, warning.shot_id],
            )?;
        }
        Ok(())
    }

    /// [#576 FZ-PERSIST-2] Move position-keyed (NULL-uuid) ignores to new shot
    /// ids: each `(section_id, from, to)` re-keys the rows at `from` to `to`, all
    /// moves at once (a swap of two slots is a pair of moves). Rows keyed by a
    /// uuid are not touched. One transaction: if any move cannot be made (its
    /// target slot holds another ignore with the same code), none is, and the
    /// error is returned. Returns how many rows moved.
    pub fn rekey_position_resolved_warnings(&self, moves: &[(u32, u32, u32)]) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        // Park every moving row on a slot no shot id can take (negative), so a
        // swap never meets the UNIQUE(section_id, shot_id, warning_code) key
        // half-way, then land each on its target.
        let park = |id: u32| -1 - i64::from(id);
        for &(section_id, from, _) in moves {
            tx.execute(
                "UPDATE resolved_warnings SET shot_id = ?3 \
                 WHERE section_id = ?1 AND shot_id = ?2 AND shot_uuid IS NULL",
                params![section_id, from, park(from)],
            )?;
        }
        let mut moved = 0;
        for &(section_id, from, to) in moves {
            moved += tx.execute(
                "UPDATE resolved_warnings SET shot_id = ?3 \
                 WHERE section_id = ?1 AND shot_id = ?2 AND shot_uuid IS NULL",
                params![section_id, park(from), to],
            )?;
        }
        tx.commit()?;
        Ok(moved)
    }

    /// Upgrade legacy NULL-uuid resolved-warning rows to UUID keying using the
    /// supplied (section_id, shot_id) -> uuid map (issue #57, Item 4). Idempotent:
    /// only rewrites rows whose shot_uuid IS NULL and whose (section_id, shot_id)
    /// still maps to a live shot. A row with no live shot is left NULL so it keeps
    /// falling back to legacy position matching and is never mis-keyed.
    pub fn backfill_resolved_warning_uuids(
        &self,
        live: &HashMap<(u32, u32), String>,
    ) -> Result<usize> {
        let mut to_fix: Vec<(i64, String)> = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT id, section_id, shot_id FROM resolved_warnings WHERE shot_uuid IS NULL",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, u32>(1)?,
                    r.get::<_, u32>(2)?,
                ))
            })?;
            for row in rows {
                let (id, sec, shot) = row?;
                if let Some(uuid) = live.get(&(sec, shot)) {
                    to_fix.push((id, uuid.clone()));
                }
            }
        }
        for (id, uuid) in &to_fix {
            self.conn.execute(
                "UPDATE resolved_warnings SET shot_uuid = ?1 WHERE id = ?2",
                params![uuid, id],
            )?;
        }
        Ok(to_fix.len())
    }

    /// Delete a resolved warning from database (un-ignore). Matches by UUID when
    /// `shot_uuid` is provided (the normal case), else by the legacy position.
    pub fn delete_resolved_warning(
        &self,
        section_id: u32,
        shot_id: u32,
        warning_code: &str,
        shot_uuid: Option<&str>,
    ) -> Result<()> {
        Self::delete_matching_resolved(
            &self.conn,
            &ResolvedWarning {
                section_id,
                shot_id,
                warning_code: warning_code.to_string(),
                resolved_at: Utc::now(),
                resolved_comment: None,
                shot_uuid: shot_uuid.map(str::to_string),
            },
        )
    }

    /// Save multiple resolved warnings to database in a single transaction
    pub fn save_resolved_warnings_bulk(&self, warnings: &[ResolvedWarning]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for warning in warnings {
            Self::delete_matching_resolved(&tx, warning)?;
        }
        {
            // Conditional upsert (see save_resolved_warning): on a positional
            // collision only the SAME shot's row is overwritten, or a legacy
            // NULL-uuid row at that position is UPGRADED to the new UUID - a
            // different shot's UUID-keyed ignore is never clobbered. This both
            // collapses a batch that maps two warnings to one (shot, code) and
            // absorbs a legacy NULL-uuid row colliding with a remapped UUID row
            // on the save carry-over, order-independently (issue #57 / Codex).
            let mut stmt = tx.prepare(
                "INSERT INTO resolved_warnings (section_id, shot_id, warning_code, resolved_at, resolved_comment, shot_uuid) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                 ON CONFLICT(section_id, shot_id, warning_code) DO UPDATE SET \
                     resolved_at = excluded.resolved_at, \
                     resolved_comment = excluded.resolved_comment, \
                     shot_uuid = excluded.shot_uuid \
                 WHERE resolved_warnings.shot_uuid IS NULL \
                    OR resolved_warnings.shot_uuid = excluded.shot_uuid",
            )?;
            for warning in warnings {
                // A 0-row result means a different shot's UUID row already holds
                // this position; preserving it (not aborting the save) is correct.
                stmt.execute(params![
                    warning.section_id,
                    warning.shot_id,
                    warning.warning_code,
                    warning.resolved_at.to_rfc3339(),
                    warning.resolved_comment,
                    warning.shot_uuid,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}
