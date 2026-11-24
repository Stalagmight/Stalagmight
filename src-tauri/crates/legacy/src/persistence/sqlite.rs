use chrono::Utc;
#[cfg(feature = "writer")]
use flate2::write::GzEncoder;
#[cfg(feature = "writer")]
use flate2::Compression;
use rusqlite::Connection;
#[cfg(feature = "writer")]
use std::io::Write as _;

// Items below are used only by the sibling `tests` module via `use super::*`;
// production code in this residual file no longer references them after the
// `impl Database` split into child modules, so gate them to the test build.

/// Schema version for migrations
///
/// v8 adds a nullable `shot_uuid` column to `resolved_warnings` (issue #57):
/// "Ignore" suppression is keyed by the shot's stable UUID so it survives shot
/// inserts/deletes/reorders. Backfilled lazily from the legacy
/// `(line_id, shot_id)` key - no rewrite of existing rows at migration time.
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
/// records which importer produced a line. `migrate_pitch_to_down_convention`
/// infers it from the geometry and negates only the lines that need it. No
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
/// v17 gives `LineUpdates`' three nullable fields (`name`, `surveyor`, `date`)
/// the `double_option` + `skip_serializing_if` pair `ShotUpdates` has carried
/// since v7 (#413 finding 5). No column changes, so the carry-forward fence is
/// unaffected - but it is a meaning-flip of a PERSISTED payload, exactly the v7
/// hazard one struct over: a pre-v17 writer serialized every untouched nullable
/// field as an explicit `null`, which now reads as "clear the field". The
/// migration drops the persisted undo/redo stacks for the same reason v7 did;
/// see the v17 arm of `ensure_schema_version`.
///
/// v18 adds four additive, nullable `lines` columns - `azimuth_north`,
/// `azimuth_correction`, `azimuth_source`, `azimuth_epoch_year` - plus the
/// optional `project_settings` key `default_azimuth_reference` (#438: the
/// stored azimuth is the measured one, and the line declares how it relates
/// to true north). All NULL, which is what an existing file has after the
/// `ALTER`, means "no reference recorded" and resolves to a ZERO correction, so
/// an upgraded file computes exactly the geometry it did before - the property
/// that makes this bump safe without a data migration. No column changes to any
/// fenced table, so the carry-forward fence is unaffected, and the persisted
/// undo payloads keep their meaning (`LineIdentity` gained an OPTIONAL field,
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
///   outright - and the loader of that time SKIPPED a bad entry rather than
///   truncating, keeping the entries on BOTH sides. The stack loaded with a
///   HOLE, and undoing past it replayed surviving older entries
///   (position-keyed shot snapshots, the apply's own `ProjectSnapshotPair`)
///   against a tree they no longer described. (Today's loader drops a history
///   with any unreadable row whole, so a removal costs the history, not its
///   meaning; the migration still drops it explicitly.)
///
/// v11→v12 drops the stacks as well, but conditionally - only when the pitch
/// flip actually moved a line (see `invalidate_pre_pitch_flip_state`).
///
/// So the rule for a future bump is narrower than "any time history is
/// dropped": bump this constant only when the payload format changes such that
/// pre-existing entries still PARSE but mean something different across the
/// whole stack. Removing a `Command` variant, or a field the payloads carry, is
/// handled instead by calling `drop_legacy_undo_history` from that migration's
/// own arm in `ensure_schema_version` - which is what v13 and v15 do, and what
/// any future removal has to do as well.
const UNDO_FORMAT_SAFE_VERSION: i32 = 7;

#[cfg(feature = "writer")]
fn compress_json(json: &str) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    // write_all and finish are infallible on Vec<u8>
    encoder
        .write_all(json.as_bytes())
        .and_then(|()| encoder.finish())
        .unwrap_or_else(|_| json.as_bytes().to_vec())
}

/// Database operations for projects
pub struct Database {
    conn: Connection,
}

mod cave_sections;
mod datum_extras;
mod integrity;
mod newer_schema;
mod open;
mod read;
mod schema;
mod schema_guard;
mod settings;
mod shot_credits;
mod station_ids;
mod station_texts;
mod stored_times;
#[cfg(feature = "writer")]
mod undo;
mod warnings;
#[cfg(feature = "writer")]
mod write;
// Added by the legacy crate (not frozen 0.6.0 code): the writer's 0.5.5 shape.
#[cfg(feature = "writer")]
mod legacy_shape;

#[cfg(feature = "writer")]
pub use datum_extras::holds_datum_extras;
/// How many `resolved_warnings` rows a carry-over will examine (#517). Exported
/// so a caller's test can size a fixture against the real ceiling instead of
/// hard-coding a copy of it.
pub use newer_schema::NewerSchema;
pub use schema_guard::{
    check_schema, declares_virtual_table, refused_schema_object, RefusedSchemaObject,
};
pub use warnings::MAX_CARRIED_RESOLVED_WARNINGS;

/// Resolved warning record
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWarning {
    pub line_id: u32,
    pub shot_id: u32,
    pub warning_code: String,
    pub resolved_at: chrono::DateTime<Utc>,
    pub resolved_comment: Option<String>,
    /// Stable UUID of the suppressed shot (issue #57). `Some` for all warnings
    /// raised against shots that have a UUID (the normal case); `None` only for
    /// pre-v8 rows or shots loaded from pre-v6 .stalx files without a UUID yet.
    /// The suppression is matched by `(shot_uuid, warning_code)` when present,
    /// falling back to the legacy `(line_id, shot_id, warning_code)` key
    /// otherwise. See `AppState::compute_project_warnings`.
    pub shot_uuid: Option<String>,
}
