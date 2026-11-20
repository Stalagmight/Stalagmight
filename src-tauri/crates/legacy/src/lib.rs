//! A frozen copy of the Stalagmight 0.6.0 survey model, SQLite `.stalx`
//! reader and load pipeline (`stalagmight-legacy`).
//!
//! The live `models` crate moves to the station-graph model and `.stalx`
//! becomes JSON (docs/plans/2026-11-20_station-graph-model.md). Every old
//! SQLite `.stalx` must still open exactly as 0.6.0 opened it, so this crate
//! keeps that code, unchanged, under its own paths: [`models`] (the 0.6.0
//! `stalagmight-models`, minus the gas planner, placement suggestions and
//! dive plans) and [`persistence`] (the 0.6.0 SQLite layer). It depends on no
//! live crate, so nothing the rewrite does can change what it reads. Change it
//! only to fix a migration bug.
//!
//! Provenance: copied at `feat/survey-graph` d595f4ee7 (release/0.6.0 at
//! 745946507). Mechanical edits only: test modules and `cfg(test)` items
//! removed, ts-rs derives removed (these types must never export TS
//! bindings), `crate::` paths re-rooted. Two files are additions, marked as
//! such: `models/project/legacy_oracle.rs` (component anchors) and
//! `persistence/sqlite/legacy_shape.rs` (the writer's 0.5.5 shape).
//!
//! # Opening a file
//!
//! [`open_path`] and [`open_bytes`] copy the input (and, for a path, any
//! `-journal` or `-wal` beside it) into a private temp directory, so the
//! original is never written, then run the 0.6.0 production open
//! (`AppState::do_load_project_full`) on the copy:
//!
//! 1. refuse a non-SQLite input ([`LegacyError::NotSqlite`]), a schema that
//!    would run file-chosen SQL, or a schema newer than v18
//!    ([`LegacyError::NewerSchema`]); a hot journal is rolled back;
//! 2. the frozen `Database::open_existing_untrusted` (integrity check, the
//!    v2..v18 migration chain, foreign-key check) and `load_project`;
//! 3. the load heal: shot uuids, the resolved-warning uuid backfill,
//!    `assign_closure_ids`, `redrive_offsets_after_load_reporting`, the
//!    unreadable-kind substitution and the one-shot Jump/Reference
//!    classification;
//! 4. measured positions (`compute_station_coordinates`), the Closures
//!    Computed View (`into_adjusted_stations`, as `get_adjusted_stations`),
//!    the adjustment report and the held station of every component.
//!
//! The result is a [`LegacySurvey`]: the settled [`LegacyProject`], per-slot
//! raw and adjusted positions, [`LegacyExtras`] (display settings, ignored
//! warnings, the SpeleoDB link and per-machine state), whether the file held
//! undo history, and a [`LoadReport`] of what the load changed.
//!
//! **Differences from the 0.6.0 app**, all deliberate:
//! - A shot with no uuid (pre-v6 files) or a repeated one in its line gets a
//!   uuid derived from its line id, index and stored id (`stalx-reader`'s
//!   rule and namespace), not a random v4, so every open of a file agrees.
//!   Positions do not depend on these ids.
//! - `project.modified_at` keeps the file's value even when a heal bumped
//!   it in memory ([`LoadReport::touched_modified_at`]), and a merge the load
//!   creates (an orphan closure promoted) is dated by it, not by the wall
//!   clock ([`LoadReport::merges_created`]).
//! - A file without stored validation thresholds keeps the 0.6.0 hardcoded
//!   default; the app used the user's global thresholds
//!   ([`LoadReport::validation_stored`] says which).
//! - The open is the hardened `open_existing_untrusted` (as `stalx-reader`),
//!   not the app's trusted one; both run the same checks and migrations.
//! - Undo history is counted, never loaded.
//!
//! # Writing (tests only)
//!
//! With `feature = "writer"`, [`write_v18`] produces what the 0.6.0 save
//! writes (save heals, `save_project`, the extras), and
//! [`WriteOptions::shape`] can drop the side tables 0.5.5 did not have.

mod error;
#[expect(
    clippy::pedantic,
    clippy::style,
    clippy::complexity,
    reason = "frozen 0.6.0 code: the live crate carries the same warnings, and this copy is \
              never edited to satisfy a lint"
)]
pub mod models;
mod open;
#[expect(
    clippy::pedantic,
    reason = "frozen 0.6.0 code: the live crate carries the same pedantic warnings, and this \
              copy is never edited to satisfy a lint"
)]
pub mod persistence;
mod survey;
#[cfg(feature = "writer")]
mod writer;

pub use error::LegacyError;
pub use open::{
    open_bytes, open_path, OpenOptions, DEFAULT_MAX_FILE_BYTES, SUPPORTED_SCHEMA_VERSION,
};
pub use survey::{
    positions_of, HistoryPresence, LegacyExtras, LegacySurvey, LoadReport, Positions, Slot,
};
#[cfg(feature = "writer")]
pub use writer::{write_v18, write_v18_with, FileShape, WriteOptions};

/// The frozen 0.6.0 project model.
pub type LegacyProject = models::Project;
