use anyhow::{Context, Result};
use chrono::Utc;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use rusqlite::Connection;
use std::io::{Read as _, Write as _};

// Items below are used only by the sibling `tests` module via `use super::*`;
// production code in this residual file no longer references them after the
// `impl Database` split into child modules, so gate them to the test build.
#[cfg(test)]
use crate::models::{
    ClosureStrategy, GpsCoordinates, LoopClosure, Project, SectionData, StationKey, StationMerge,
    SurveyDirection, ZeroDatum,
};
#[cfg(test)]
use chrono::NaiveDate;
#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::path::Path;

/// Schema version for migrations
///
/// v8 adds a nullable `shot_uuid` column to `resolved_warnings` (issue #57):
/// "Ignore" suppression is keyed by the shot's stable UUID so it survives shot
/// inserts/deletes/reorders. Backfilled lazily from the legacy
/// `(section_id, shot_id)` key - no rewrite of existing rows at migration time.
///
/// v9 adds the additive single-row `speleo_link` table (SpeleoDB hand-off
/// identity + dirty bit, #189). No existing column or row is touched;
/// forward-looking only - no backfill. The table is created in BOTH
/// `init_schema` and `migrate_schema`, so legacy v8 files gain it on
/// `open_existing`. `is_remote` is DERIVED from `EXISTS(speleo_link)`, never
/// stored. See `PRESERVED_SPELEOLINK_COLUMNS` (the carry-forward fence) for the
/// columns every future bump must preserve.
///
/// v10 adds the additive `speleo_link.last_edit_at` column (RFC-3339 UTC of the
/// most recent accepted edit; #193 guardrail-7 push-start comparison). Added in
/// BOTH `init_schema` and `migrate_schema` (a conditional `ALTER` for existing v9
/// tables, mirroring the shot-uuid pattern); carried forward by the fence.
/// v12 normalises stored pitch to the +down convention (#296). DMP import used
/// to keep the MNemo's +up reading, so pre-v12 files mix conventions: DMP-sourced
/// shots are +up while Walls/Compass-sourced shots are already +down, and nothing
/// records which importer produced a section. `migrate_pitch_to_down_convention`
/// infers it from the geometry and negates only the sections that need it. No
/// column changes, so the carry-forward fence is unaffected.
///
/// v14 normalises stored azimuth readings into `[0, 360)` (#24). Nothing changed
/// meaning - `Shot::average_azimuth` simply used Rust's truncated `%`, which returned
/// the ANTIPODE of the circular mean for a reading pair more than 540 degrees apart,
/// and now wraps with `rem_euclid`. Normalising takes the ambiguity out of the FILE, so
/// a `.stalx` renders identically either side of that fix. See
/// `migrate_azimuths_to_canonical_range`. No column changes, so the carry-forward
/// fence is unaffected.
///
/// v15 drops the single-row `collab_state` table: Ariane collaboration mode was
/// removed, so nothing reads or writes it. Destructive, but inert - the table only
/// ever held local sidecar wiring (the sibling `.tml` path, the per-shot export
/// baseline and a per-machine autosave toggle), never survey data. Existing files
/// take the migration once on open; an older build opening a v15 file still works
/// (it warns about the newer version and recreates the table on its own writes).
///
/// v16 adds the additive `speleo_link.undo_mode` column (#303: whether this project's
/// undo history rides to the server; nullable only as a legacy initialization marker)
/// and the additive single-row local-only
/// `speleo_undo_chain` table (which server revision the working copy was unwound
/// to, so the next Unwind can walk to its parent). Both are created in BOTH
/// `init_schema` and `migrate_schema` - a conditional `ALTER` for existing tables,
/// mirroring the v10/v11 pattern - and the column is carried forward by the fence.
/// `speleo_undo_chain` is per-machine progress through history, so it is stripped
/// from every push snapshot (`PUSH_SNAPSHOT_LOCAL_TABLES`).
///
/// v17 gives `SectionUpdates`' three nullable fields (`name`, `surveyor`, `date`)
/// the `double_option` + `skip_serializing_if` pair `ShotUpdates` has carried
/// since v7 (#413 finding 5). No column changes, so the carry-forward fence is
/// unaffected - but it is a meaning-flip of a PERSISTED payload, exactly the v7
/// hazard one struct over: a pre-v17 writer serialized every untouched nullable
/// field as an explicit `null`, which now reads as "clear the field". The
/// migration drops the persisted undo/redo stacks for the same reason v7 did;
/// see the v17 arm of `ensure_schema_version`.
///
/// v18 adds four additive, nullable `sections` columns - `azimuth_north`,
/// `azimuth_correction`, `azimuth_source`, `azimuth_epoch_year` - plus the
/// optional `project_settings` key `default_azimuth_reference` (#438: the
/// stored azimuth is the measured one, and the section declares how it relates
/// to true north). All NULL, which is what an existing file has after the
/// `ALTER`, means "no reference recorded" and resolves to a ZERO correction, so
/// an upgraded file computes exactly the geometry it did before - the property
/// that makes this bump safe without a data migration. No column changes to any
/// fenced table, so the carry-forward fence is unaffected, and the persisted
/// undo payloads keep their meaning (`SectionIdentity` gained an OPTIONAL field,
/// absent in every pre-v18 entry), so the stacks are NOT dropped.
///
/// Public so readers outside the app (the `stalx-reader` crate) can refuse a
/// file written by a newer schema instead of misreading it.
pub const SCHEMA_VERSION: i32 = 18;

/// Schema version at which persisted undo/redo payloads became safe to replay.
///
/// v7 changed the `ShotUpdates` JSON wire format: nullable fields now use a
/// `double_option` deserializer where an explicit `null` means "clear the
/// field" (`Some(None)`), while pre-v7 writers serialized every UNTOUCHED
/// nullable field as an explicit `null`. Replaying a pre-v7 undo/redo entry
/// under the new semantics would silently clear pitch/LRUD/comment/marker/
/// temperature on the target shot. Legacy JSON cannot distinguish "untouched"
/// from "restore-to-empty", so files below this version get their persisted
/// undo/redo stacks dropped on open (see `drop_legacy_undo_history`).
///
/// Deliberately a separate constant from `SCHEMA_VERSION`, but note what that
/// buys and what it does not. It is the floor for exactly ONE purge - the
/// unconditional pre-v7 one above - and it is NOT a promise that later
/// migrations leave persisted history alone. Later migrations drop the stacks
/// too, and none of them bumped this constant, because none of them changed the
/// undo payload FORMAT; each removed a shape the payloads can still name:
///
/// - **v12→v13** removed the legacy `is_reference` boolean in favour of
///   `ShotKind`. Old payloads still deserialize: `de_shot_kind` maps a bare
///   `true` to `Jump`, because a boolean cannot say which virtual kind it meant.
///   Replaying one after the #27 backfill settled that shot on `Reference` would
///   make a non-swimmable tie swimmable again, with no classifier pass on an
///   undo to correct it. Silent MISinterpretation, not a parse failure.
/// - **v14→v15** removed the `ApplyCollabChanges` `Command` variant with Ariane
///   collaboration mode. `Command` is internally tagged (`tag = "type"`) with no
///   catch-all, so a stored `apply_collab_changes` entry stops deserializing
///   outright - and `UndoStack::load_from_persisted_with_timestamps` SKIPS a bad
///   entry rather than truncating, keeping the entries on BOTH sides. The stack
///   loads with a HOLE, and undoing past it replays surviving older entries
///   (position-keyed `ShotSnapshot`s, the apply's own `ProjectSnapshotPair`)
///   against a tree they no longer describe.
///
/// v11→v12 drops the stacks as well, but conditionally - only when the pitch
/// flip actually moved a section (see `invalidate_pre_pitch_flip_state`).
///
/// So the rule for a future bump is narrower than "any time history is
/// dropped": bump this constant only when the payload format changes such that
/// pre-existing entries still PARSE but mean something different across the
/// whole stack. Removing a `Command` variant, or a field the payloads carry, is
/// handled instead by calling `drop_legacy_undo_history` from that migration's
/// own arm in `ensure_schema_version` - which is what v13 and v15 do, and what
/// any future removal has to do as well.
const UNDO_FORMAT_SAFE_VERSION: i32 = 7;

/// Maximum number of rows a single raw-table page request may return.
pub const MAX_RAW_PAGE_ROWS: u32 = 10_000;

/// Render a blob value as a size placeholder, e.g. `<blob 4.2 KB>`.
fn format_blob_placeholder(len: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let len = len as f64;
    if len < KB {
        format!("<blob {len:.0} B>")
    } else if len < MB {
        format!("<blob {:.1} KB>", len / KB)
    } else if len < GB {
        format!("<blob {:.1} MB>", len / MB)
    } else {
        format!("<blob {:.1} GB>", len / GB)
    }
}

fn compress_json(json: &str) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    // write_all and finish are infallible on Vec<u8>
    encoder
        .write_all(json.as_bytes())
        .and_then(|()| encoder.finish())
        .unwrap_or_else(|_| json.as_bytes().to_vec())
}

/// [#473] Hard ceiling on how many bytes ONE persisted history blob may inflate to.
///
/// Binds BOTH blob shapes: the check sits above the gzip/legacy branch in
/// [`decompress_json_under`], so a legacy plain-text row is bound by it too - at its REAL
/// `from_utf8_lossy` output, which is the row's own length when it is valid UTF-8 and up
/// to 3x only when it is not (see [`declared_inflated_size`]).
///
/// **What this bounds is a single row's decompressed size.** The whole-load bound is
/// [`MAX_HISTORY_LOAD_BYTES`], and the two are needed together: this one alone is what
/// the first attempt at #473 shipped, and it does not stop the OOM, because a load
/// reads many rows and nothing summed them. A measured 14 MB file of 6 rows, each blob
/// declaring 250 MiB - every one of them inside this ceiling - allocated 3.1 GB and the
/// load SUCCEEDED. See [`InflationBudget`].
///
/// The persisted tables are attacker-controlled (a `.stalx` is a file the user opens,
/// and nothing in the schema bounds `undo_stack` / `redo_stack`), so one row can carry a
/// gzip bomb: a few hundred kilobytes of BLOB that inflates to gigabytes. Such a blob is
/// refused by its declared size before a byte is inflated (see
/// [`declared_inflated_size`]), and the `take` in [`decompress_json_capped`] is the
/// backstop for a blob that lies about that size.
///
/// The number is picked to be far above any legitimate payload. The largest a history
/// row can honestly be is a `UndoData::ProjectSnapshotPair` (stalagmight-core) - TWO
/// whole-project JSON snapshots - which the table on
/// [`crate::persistence::undo_limits::UNDO_BYTE_BUDGET_BYTES`] measures at ~4.2 MB for a 5,000-shot
/// project and ~17 MB for a 20,000-shot one. That grows roughly linearly in shot count,
/// so 256 MiB is more than ten times the biggest payload any plausible survey produces,
/// and reaching it means the blob is not one we wrote.
///
/// The shape has precedent in this codebase: `MAX_ENTRY_BYTES` (512 MiB) in
/// `speleodb/dump.rs` bounds one hostile ZIP entry the same way, with a fixed ceiling
/// rather than a ratio, and rejects on the declared size first for the same reason.
/// This one is tighter because a history blob is JSON WE produced, where a dump entry
/// is a whole `.stalx`.
///
/// Independent of [`crate::persistence::undo_limits::MAX_PERSISTED_HISTORY_ROWS`], and neither implies
/// the other: that constant bounds how many rows one load reads, this one bounds how
/// large one row may inflate. A file can be well inside either bound and hostile under
/// the other - which is exactly why neither is sufficient and the load also carries a
/// running total.
///
/// Tripping it errors the WHOLE history load, and project open then degrades to empty
/// history - the existing `Err` arms in `core/state/project_load.rs` log and continue
/// with `Vec::new()`. That is the intended outcome: a blob this size is corrupt or
/// crafted, and opening the project without its history beats not opening it. Since
/// #473 the user is TOLD when that happens (`AppState::history_discarded`, surfaced as
/// a banner) rather than only the log file learning about it.
///
/// **And the loss is permanent, not per-session.** Every save is a full rewrite that
/// begins `DELETE FROM undo_stack` (`sqlite/undo.rs`), so the rows this refused are gone
/// from disk as soon as the user saves. That is why the degrade is worth a banner rather
/// than a log line, and why the banner's own copy says the next save replaces the
/// unreadable history: it is the only warning the user gets before it happens.
const MAX_DECOMPRESSED_BLOB_BYTES: u64 = 256 * 1024 * 1024;

/// [#473] Hard ceiling on how many bytes ONE PROJECT OPEN may inflate to IN TOTAL,
/// across BOTH history stacks, every row of each, and both blobs of every row.
///
/// This is the bound the per-blob ceiling could not provide. Without it the two
/// existing bounds MULTIPLY rather than compose: a count-window project reads 100 rows x
/// 2 blobs x 256 MiB = 50 GiB, and a Full-mode one reads
/// [`crate::persistence::undo_limits::MAX_PERSISTED_HISTORY_ROWS`] (65536) rows for 32 TiB nominal -
/// the read window is 100 only in count-window mode, never in Full.
///
/// Read "per open" literally. An open reads the undo stack and then the redo stack and
/// holds both results at once, so a budget scoped to one stack bounds the pair at 2x this
/// number - which is what the first version of this fix shipped, under comments that said
/// 1x. `Database::load_history_budget` exists so the open creates one budget and both
/// loads share it.
///
/// The number bounds RESERVATIONS, which is not quite the process's peak RSS: the
/// compressed rows sqlite already read are resident on top of it, and `read_to_end` grows
/// its `Vec` by doubling, so a single blob transiently costs ~1.5-2x what it reserved.
///
/// 1 GiB, derived from what a save can legitimately produce - and note WHICH knob bounds
/// that, because there are two and they are easy to swap. What a count-window
/// (non-SpeleoDB) project WRITES is bounded by `undo_persist_limit`, default **20**
/// (`config/defaults.rs`, read in `project_io.rs::rows_to_persist`). `undo_history_limit`
/// (100) is the in-memory/read window and never reaches disk. So the honest worst case
/// is 20 `ProjectSnapshotPair` entries on a 20,000-shot survey at ~17 MB each = ~340 MB
/// per stack, ~680 MB for the pair - comfortably under this ceiling, which is the point.
///
/// The write side carries no BYTE bound of its own for such a project, only that count,
/// so a user who raises `undo_persist_limit` far past its default can still author more
/// history than this will read back. That is the residual: bounding the writer in bytes,
/// so this ceiling can be tightened without ever refusing real work, is the follow-up.
/// The exposure meanwhile drops from ~50 GiB to ~1 GiB.
const MAX_HISTORY_LOAD_BYTES: u64 = 1024 * 1024 * 1024;

/// [#473] The bytes ONE PROJECT OPEN has left to inflate into, shared by both history
/// stacks, every row of each, and every decompression thread.
///
/// **Per open, not per stack, and that distinction was a bug once.** An open reads undo
/// and then redo, and holds both results at the same time; giving each its own budget
/// made the real ceiling 2x the constant while every comment still said 1x. The budget
/// is therefore created by the caller that owns the open and passed to both loads - see
/// `Database::load_history_budget`.
///
/// A per-blob ceiling cannot bound a load, and a per-thread one cannot bound a machine:
/// `decompress_history_rows` inflates up to `MAX_DECOMPRESS_THREADS` blobs at
/// once, so a bound held per thread is silently multiplied by the core count and the
/// same file loads on a laptop and dies on a workstation. One atomic counter, decremented
/// before any blob is inflated, is what makes the ceiling mean the same thing everywhere.
///
/// **Where this sits in the order of checks matters, and is easy to misread.** The
/// binding check on a hostile file is the DECLARED-SIZE PRE-SCAN in
/// `decompress_history_rows_within`, which refuses a window before a byte is inflated.
/// Given that pre-scan, `reserve` below is unreachable by construction: the pre-scan
/// establishes `sum(declared) <= remaining`, every gzip row reserves exactly its own
/// `declared`, and every legacy row reserves exactly its own `declared` too, so
/// the running total can never exceed what was already checked. That is deliberate
/// defense in depth, not redundancy to delete: it is what still bounds the load if a
/// future caller reaches `decompress_json_under` without a pre-scan, and it is the only
/// thing that makes the bound hold across the two stacks of one open. Do not describe it
/// as the mechanism that stops the attack - the pre-scan is.
///
/// Reserve-then-refund rather than charge-after-the-fact: charging afterwards would let
/// N threads each read a full budget, inflate concurrently and only then discover the
/// total was blown - exactly the allocation the budget exists to deny. The reservation is
/// the blob's DECLARED size (free to read, see [`declared_inflated_size`]), and the unused
/// remainder is refunded once the real output is known, so an honest small row does not
/// starve the rows behind it. Note the peak this bounds is the sum of outstanding
/// RESERVATIONS; the transient allocation can exceed it, because `read_to_end` grows a
/// `Vec` by doubling and briefly holds the old and new buffers at once (~1.5-2x).
pub struct InflationBudget {
    remaining: std::sync::atomic::AtomicU64,
}

impl InflationBudget {
    pub(super) fn new(total: u64) -> Self {
        Self {
            remaining: std::sync::atomic::AtomicU64::new(total),
        }
    }

    pub(super) fn remaining(&self) -> u64 {
        self.remaining.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Take `bytes` out of the budget, or fail without taking anything.
    ///
    /// `try_update` rather than a load followed by a store: two threads reserving
    /// concurrently off the same observed remainder would each believe they fit, and
    /// the budget would be overspent by exactly the amount it is meant to bound.
    ///
    /// The message deliberately shares NO distinctive phrase with the pre-scan's: a test
    /// that asserts on "load budget" cannot tell which of the two refused it, and one
    /// that meant to exercise this path passed for months while only the pre-scan ever
    /// ran. Assert on "while inflating" for this one, "declares" for the pre-scan.
    fn reserve(&self, bytes: u64) -> Result<()> {
        use std::sync::atomic::Ordering;
        self.remaining
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(bytes)
            })
            .map_err(|left| {
                anyhow::anyhow!(
                    "history exceeded its inflation budget while inflating \
                     (a further {bytes} bytes requested with {left} left)"
                )
            })?;
        Ok(())
    }

    /// Hand back a reservation that turned out to be larger than the real output.
    ///
    /// Load-bearing for the legacy branch, which must reserve its 3x worst case before
    /// allocating and can only afford to do so because the excess comes straight back.
    /// A no-op for a well-formed gzip member, which inflates to exactly its declared
    /// size - kept for that case anyway so the counter means "bytes actually held"
    /// rather than "bytes claimed".
    fn refund(&self, bytes: u64) {
        self.remaining
            .fetch_add(bytes, std::sync::atomic::Ordering::SeqCst);
    }
}

/// [#473] What a blob SAYS it inflates to, without allocating.
///
/// O(1) for gzip (a trailer read). O(n) for the legacy branch, which scans the blob for
/// UTF-8 validity - cheap relative to the bytes sqlite has already read, and no
/// amplification, but not the constant-time answer this doc used to claim.
///
/// A gzip member ends with a 4-byte little-endian ISIZE trailer carrying the
/// uncompressed length, and the blob is already fully in memory (sqlite read it), so
/// this costs a slice index. It is what lets an honest bomb be refused before a byte is
/// inflated, which is the property `extract_entry_capped` in `speleodb/dump.rs` gets
/// from the ZIP central directory.
///
/// **Attacker-controlled, so only ever trusted in the refusing direction.** ISIZE is the
/// real length mod 2^32 and a crafted blob can simply understate it; a low answer is
/// therefore a HINT, made safe by capping the actual read at the declared size, so a
/// blob that inflates past what it declared trips the `take` in
/// [`decompress_json_capped`]. A high answer is safe to act on directly: no honest blob
/// overstates its own size.
///
/// The same caveat covers multi-member streams, where the trailer belongs to the LAST
/// member while `GzDecoder` stops after the first. Nothing in this codebase writes
/// multi-member gzip, so such a blob is not one we produced, and either direction of
/// disagreement is handled above.
///
/// Non-gzip blobs report what `from_utf8_lossy` will ACTUALLY produce, which is decided
/// by validity: valid UTF-8 passes through unchanged (exactly the input length), and only
/// invalid input expands, by at most a 3-byte U+FFFD per maximal invalid subsequence.
/// Answering the 3x worst case unconditionally refused valid legacy rows over a third of
/// the ceiling - see the comment on that branch in [`decompress_json_under`].
fn declared_inflated_size(data: &[u8]) -> u64 {
    // `checked_sub`, not `data.len() - 4`: the magic-byte test only guarantees TWO
    // bytes, and a hostile blob is free to be exactly that long.
    if is_gzip(data) {
        if let Some(trailer) = data.len().checked_sub(4).and_then(|at| data.get(at..)) {
            let mut isize_bytes = [0_u8; 4];
            isize_bytes.copy_from_slice(trailer);
            return u64::from(u32::from_le_bytes(isize_bytes));
        }
    }
    // Legacy plain-text branch. `from_utf8_lossy` returns the input UNCHANGED when it is
    // valid UTF-8 and only substitutes a 3-byte U+FFFD per invalid subsequence otherwise,
    // so validity decides between an exact answer and a worst case - and the check is an
    // O(n) scan with no allocation.
    //
    // Answering `3 * len` unconditionally was wrong in a way that only bit the legacy
    // shape: the per-blob ceiling is compared against this number, so a perfectly valid
    // 90 MB legacy row was refused for a 270 MB output it would never produce - an
    // effective ceiling of 85 MiB where the constant says 256, and the whole stack
    // discarded (with a data-loss banner) for a file this codebase itself wrote.
    if std::str::from_utf8(data).is_ok() {
        return data.len() as u64;
    }
    (data.len() as u64).saturating_mul(3)
}

fn is_gzip(data: &[u8]) -> bool {
    // Gzip magic bytes: 0x1f 0x8b
    data.len() >= 2 && data[0] == 0x1f && data[1] == 0x8b
}

/// [#473] Inflate one history blob, charged against a load's shared budget.
///
/// The chokepoint every history read goes through. Three bounds apply in order, cheapest
/// first: the declared size against the per-blob ceiling (no allocation), the declared
/// size against what is left of the load budget (no allocation), and finally the real
/// output against the declared size (the backstop for a blob that lied).
fn decompress_json_under(data: &[u8], budget: &InflationBudget) -> Result<String> {
    let declared = declared_inflated_size(data);

    // The per-blob ceiling FIRST, and above the branch rather than inside the gzip arm.
    // It used to sit below, which quietly exempted the legacy shape: `declared` there is
    // its `from_utf8_lossy` output, so a large non-gzip blob inflated past the constant
    // whose own doc calls it a hard ceiling on ONE blob, bounded only by the load budget.
    // Binding it here costs nothing legitimate: `declared_inflated_size` prices a valid
    // legacy row at its true length, so only a row that really would exceed the ceiling
    // is refused.
    if declared > MAX_DECOMPRESSED_BLOB_BYTES {
        anyhow::bail!(
            "history blob declares {declared} bytes, past the \
             {MAX_DECOMPRESSED_BLOB_BYTES}-byte decompression ceiling \
             (from {} compressed bytes)",
            data.len()
        );
    }

    // The legacy plain-UTF-8 branch. It reserves its worst case BEFORE allocating and
    // refunds down to the truth, like the gzip branch and for the same reason: charging
    // after `into_owned()` has already run is the charge-afterwards ordering the budget
    // exists to forbid, and with several rows inflating at once it is how a bound gets
    // silently multiplied. The refund is what keeps that affordable - `from_utf8_lossy`
    // triples only fully-invalid input, so an ordinary legacy row hands back ~2/3 of its
    // reservation immediately and does not starve the rows behind it.
    //
    // `saturating_sub` on both refunds, and it is not defensive noise. `out.len()` cannot
    // exceed `declared` today - the legacy branch's `declared` is its exact
    // `from_utf8_lossy` worst case, and the gzip branch is hard-capped at `declared` by
    // `decompress_json_capped`. But `[profile.release]` sets no `overflow-checks`, so a
    // plain `-` that ever did underflow would wrap and hand `refund` ~2^64, leaving the
    // budget effectively unbounded for the rest of the open. That is a bound failing OPEN,
    // silently, in release only - the one direction this whole issue exists to prevent -
    // and the guard against it costs nothing.
    if !is_gzip(data) {
        budget.reserve(declared)?;
        let out = String::from_utf8_lossy(data).into_owned();
        budget.refund(declared.saturating_sub(out.len() as u64));
        return Ok(out);
    }

    budget.reserve(declared)?;
    // Refund on the failure path too, not just on success: THIS blob's reservation goes
    // back when it fails to inflate. That is only half of "a failed load charges nothing"
    // - the rows that already decoded still hold their bytes, and releasing those is
    // `decompress_history_rows_within`'s job at the point it propagates the error.
    match decompress_json_capped(data, declared) {
        Ok(out) => {
            budget.refund(declared.saturating_sub(out.len() as u64));
            Ok(out)
        }
        Err(e) => {
            budget.refund(declared);
            Err(e)
        }
    }
}

/// One-blob convenience wrapper: its own full-size budget.
///
/// Test-only, and deliberately so - every production read is part of a LOAD, and a load
/// has to share one budget across its rows or the bound it is under is not the bound it
/// claims. A production caller reaching for this would be re-introducing #473's
/// per-blob-only accounting one call site at a time.
#[cfg(test)]
fn decompress_json(data: &[u8]) -> Result<String> {
    decompress_json_under(data, &InflationBudget::new(MAX_HISTORY_LOAD_BYTES))
}

/// [#473] Inflate one history blob under an explicit output ceiling.
///
/// Split out from [`decompress_json_under`] so the bound can be exercised at a small
/// `max_bytes` in tests instead of only at the real 256 MiB constant.
///
/// The decoder is read through `take(max_bytes + 1)`: the extra byte is what makes a
/// payload of EXACTLY `max_bytes` succeed while `max_bytes + 1` is detectable, since
/// `Take` reports end-of-stream identically to a genuinely finished one. Hence the
/// explicit `buf.len() > limit` check rather than trusting the reader to complain.
///
/// `saturating_add` rather than `+` is what prevents the wrap: a caller asking for a very
/// large ceiling ("no practical limit") would otherwise turn `+ 1` into zero, and since
/// `[profile.release]` sets no `overflow-checks` that becomes `take(0)` - an EMPTY string
/// reported as success - in exactly the build where it is hardest to notice. The
/// `min(usize::MAX)` beside it does NOT help with that (on 64-bit the two are the same
/// number); it earns its place only by keeping the `usize::try_from` below exact on a
/// 32-bit target.
///
/// Read into a `Vec<u8>` and validated afterwards rather than `read_to_string` on the
/// `Take`: the cut is at a byte offset, so on a multi-byte payload it can land mid
/// codepoint, and `read_to_string` would then report a size trip as a UTF-8 error -
/// blaming the wrong thing. For the non-trip case nothing is lost, because
/// `String::from_utf8` is exactly as strict as `read_to_string`.
///
/// `Vec::new()` rather than `Vec::with_capacity(limit)` deliberately: pre-allocating
/// the ceiling would hand a hostile blob the very allocation the ceiling exists to
/// deny. The vector grows to what is actually produced, up to the cap.
///
/// **Gzip only.** It used to carry a legacy plain-UTF-8 `else` arm, which became
/// unreachable once [`decompress_json_under`] started handling that shape itself - and
/// unreachable is the dangerous state for this particular arm, because it took no budget:
/// a future caller routing legacy blobs back through here would have reintroduced
/// uncharged inflation in the function documented as the chokepoint. One legacy path,
/// and it is the charged one.
fn decompress_json_capped(data: &[u8], max_bytes: u64) -> Result<String> {
    debug_assert!(
        is_gzip(data),
        "decompress_json_capped is gzip-only; legacy blobs belong to decompress_json_under"
    );
    if !is_gzip(data) {
        anyhow::bail!("history blob is not gzip ({} bytes)", data.len());
    }
    let max_bytes = max_bytes.min(usize::MAX as u64);
    let limit = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    let mut buf = Vec::new();
    GzDecoder::new(data)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut buf)?;
    if buf.len() > limit {
        anyhow::bail!(
            "history blob expands past the {max_bytes}-byte decompression ceiling \
             (stopped after {} bytes from {} compressed bytes)",
            buf.len(),
            data.len()
        );
    }
    String::from_utf8(buf).context("decompressed history blob is not valid UTF-8")
}

/// Database operations for projects
pub struct Database {
    conn: Connection,
}

mod integrity;
mod newer_schema;
mod open;
mod read;
mod repair;
mod schema;
mod schema_guard;
mod settings;
mod shot_credits;
mod station_texts;
mod undo;
mod warnings;
mod write;

/// How many `resolved_warnings` rows a carry-over will examine (#517). Exported
/// so a caller's test can size a fixture against the real ceiling instead of
/// hard-coding a copy of it.
pub use newer_schema::NewerSchema;
pub use schema_guard::{
    check_schema, declares_virtual_table, refused_schema_object, RefusedSchemaObject,
};
pub use warnings::MAX_CARRIED_RESOLVED_WARNINGS;

/// A stringified page of rows from a raw sqlite table.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct RawTableData {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub total_rows: u32,
}

/// Information about a backup file.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../bindings/")]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    pub path: String,
    pub size_bytes: u64,
    pub modified_at: String,
}

/// Result of attempting to repair a corrupt database.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../bindings/")]
#[serde(rename_all = "camelCase")]
pub struct RepairResult {
    pub success: bool,
    pub backup_path: String,
    pub message: String,
}

/// Data salvaged from a corrupt database.
#[derive(Debug, Clone, Default, serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../bindings/")]
#[serde(rename_all = "camelCase")]
pub struct SalvagedData {
    pub project_name: Option<String>,
    pub project_location: Option<String>,
    pub section_count: usize,
    pub shot_count: usize,
    pub merge_count: usize,
    /// Sections as JSON (for display/re-import)
    pub sections_json: Option<String>,
    /// Full project as JSON (if loadable despite corruption)
    pub full_project_json: Option<String>,
    /// Errors encountered while reading individual tables
    pub errors: Vec<String>,
}

/// Minimal section info for salvage display.
#[derive(Debug, Clone, serde::Serialize)]
struct SalvagedSection {
    id: u32,
    name: Option<String>,
    surveyor: Option<String>,
    date: Option<String>,
    color: String,
}

/// Resolved warning record
#[derive(Debug, Clone)]
pub struct ResolvedWarning {
    pub section_id: u32,
    pub shot_id: u32,
    pub warning_code: String,
    pub resolved_at: chrono::DateTime<Utc>,
    pub resolved_comment: Option<String>,
    /// Stable UUID of the suppressed shot (issue #57). `Some` for all warnings
    /// raised against shots that have a UUID (the normal case); `None` only for
    /// pre-v8 rows or shots loaded from pre-v6 .stalx files without a UUID yet.
    /// The suppression is matched by `(shot_uuid, warning_code)` when present,
    /// falling back to the legacy `(section_id, shot_id, warning_code)` key
    /// otherwise. See `AppState::compute_project_warnings`.
    pub shot_uuid: Option<String>,
}

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stderr)]
mod diagnostic_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod newer_schema_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod station_names_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test module: panics on a failed assumption are the point"
)]
mod shot_credits_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    // The permission-bit tests (#485) say on stderr when the OS declined to make
    // the fixture read-only - root ignores mode bits, and CI's check job runs as
    // root - so a pass there is not read as a pass of the guard.
    clippy::print_stderr,
    clippy::float_cmp,
    clippy::panic
)]
mod tests;
