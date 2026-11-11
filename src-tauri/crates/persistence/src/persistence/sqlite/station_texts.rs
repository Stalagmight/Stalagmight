//! The two per-station free-text tables, `station_comments` and
//! `station_names`.
//!
//! Both have the same positional shape - `(section_id, station_index, <text>)`
//! with that pair as the primary key - and the same lifecycle: written from a
//! `StationKey` map by resolving each key against the current shots (orphans
//! skipped), read back as transitional positional keys that the load re-anchor
//! pass canonicalizes. One enum keeps the SQL for both in one place.
//!
//! `station_names` was added WITHOUT a schema version bump, on purpose. SpeleoDB
//! turns an uploaded `.stalx` into a map through `openspeleo_lib`, which reads it
//! with OpenSpeleo's PYTHON package `stalagmight_lib`
//! (github.com/OpenSpeleo/pytool_stalagmight_lib - not this repo's Rust lib crate
//! of the same name). Verified at pytool_stalagmight_lib @ 23049eabca21
//! (2026-11-09), `stalagmight_lib/parser.py`: `_read` refuses any
//! `PRAGMA user_version != 18` and `_read_project` any `project.schema_version`
//! other than 18, and it reads only `project`, `sections`, `shots`,
//! `station_comments`, `project_settings`, `zero_datums` and `station_merges`,
//! each by an explicit column list. A new table is invisible to it; a bump would
//! lock it out. (The in-repo `stalx-reader` is not the constraint: its supported
//! version is this crate's `SCHEMA_VERSION`, so it moves with any bump - though an
//! older build of it refuses a newer file too.) The price is that a build
//! without this table drops the names on its next save - every save rebuilds the
//! file from `init_schema`.

/// One of the per-station text tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StationTextTable {
    Comments,
    Names,
}

impl StationTextTable {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Comments => "station_comments",
            Self::Names => "station_names",
        }
    }

    pub(super) fn select_sql(self) -> &'static str {
        match self {
            Self::Comments => "SELECT section_id, station_index, comment FROM station_comments",
            Self::Names => "SELECT section_id, station_index, name FROM station_names",
        }
    }

    pub(super) fn insert_sql(self) -> &'static str {
        match self {
            Self::Comments => {
                "INSERT INTO station_comments (section_id, station_index, comment) VALUES (?1, ?2, ?3)"
            }
            Self::Names => {
                "INSERT INTO station_names (section_id, station_index, name) VALUES (?1, ?2, ?3)"
            }
        }
    }
}

/// DDL for `station_names`, shared by `init_schema` (fresh files, and the temp
/// file every save writes) and `migrate_schema` (an opened file of an older
/// shape) so the two cannot drift. Same shape as `station_comments`.
pub(super) const CREATE_STATION_NAMES: &str = "CREATE TABLE IF NOT EXISTS station_names (
                section_id INTEGER NOT NULL,
                station_index INTEGER NOT NULL,
                name TEXT NOT NULL,
                PRIMARY KEY (section_id, station_index),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            )";
