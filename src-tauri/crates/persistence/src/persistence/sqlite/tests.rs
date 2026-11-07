use super::*;
use crate::models::test_fixtures::straight_shot;
use crate::models::ShotData;
use crate::models::ShotKind;
use crate::models::StationAnchor;
use crate::models::StationRef;
use tempfile::TempDir;

use super::schema::PRESERVED_SPELEOLINK_COLUMNS;
use crate::models::SpeleoLink;

/// A SpeleoLink fixture with the given base SHA + dirty bit.
fn sample_speleo_link(base_commit_sha: Option<&str>, dirty: bool) -> SpeleoLink {
    SpeleoLink {
        endpoint: "http://10.10.0.220:8000/".to_string(),
        project_id: "11111111-1111-1111-1111-111111111111".to_string(),
        project_name: "Test Cave".to_string(),
        base_commit_sha: base_commit_sha.map(str::to_owned),
        has_unpushed_edits: dirty,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: None,
    }
}

/// [#576 A3] `SAVE_PROJECT_TABLES` is what `Database::save_project` writes - in
/// both directions. The save pipeline's side-table registry trusts this list to
/// say which tables the core write rebuilds from the `Project`, so a table the
/// transaction stops writing (or starts writing) without the list following it
/// would let a table be dropped, or carried twice, by every save. Structural,
/// because the alternative is a fixture holding a row in every table.
#[test]
fn save_project_tables_is_what_the_core_save_writes() {
    let src = include_str!("write.rs");
    let body = &src[src.find("pub fn save_project").unwrap()..];
    let named = |prefix: &str| -> std::collections::BTreeSet<String> {
        body.match_indices(prefix)
            .map(|(at, _)| {
                body[at + prefix.len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect::<String>()
            })
            .collect()
    };
    let mut written = named("DELETE FROM ");
    written.extend(named("INSERT INTO "));
    written.extend(named("INSERT OR REPLACE INTO "));
    let listed: std::collections::BTreeSet<String> = SAVE_PROJECT_TABLES
        .iter()
        .map(|t| (*t).to_string())
        .collect();
    assert_eq!(
        written, listed,
        "SAVE_PROJECT_TABLES must name exactly the tables save_project writes"
    );
    let schema: std::collections::BTreeSet<String> = Database::in_memory()
        .unwrap()
        .list_raw_table_names()
        .unwrap()
        .into_iter()
        .collect();
    assert!(
        listed.is_subset(&schema),
        "every listed table exists in a fresh schema: {:?}",
        listed.difference(&schema).collect::<Vec<_>>()
    );
}

/// A hostile `.stalx` must not reach FTS, RTREE or DBSTAT code, nor load an
/// extension (#608). The modules are stripped at build time by
/// `LIBSQLITE3_FLAGS`, which the repo-root `.cargo/config.toml` sets for every
/// cargo command run inside the clone. Not ignored, so a build that misses it
/// (cargo started outside the clone, the `[env]` entry dropped) fails here.
///
/// Except in a packaged copy of this crate (#576 RV19a-17): the copy crates.io
/// serves, or `cargo vendor` makes, carries the `Cargo.toml.orig` that `cargo
/// package` writes and none of this repo's `.cargo/config.toml`, so the flags
/// this pins are not in play there and the test would fail for every consumer
/// who runs it. There it says so and returns.
#[test]
fn bundled_sqlite_omits_virtual_table_modules_and_extension_loading() {
    if Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("Cargo.toml.orig")
        .exists()
    {
        println!(
            "packaged copy of stalagmight-persistence: the repo's LIBSQLITE3_FLAGS do not \
             apply here, nothing to pin"
        );
        return;
    }
    let conn = Connection::open_in_memory().unwrap();
    let options: Vec<String> = conn
        .prepare("PRAGMA compile_options")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();

    for stripped in [
        "ENABLE_FTS3",
        "ENABLE_FTS5",
        "ENABLE_RTREE",
        "ENABLE_DBSTAT_VTAB",
    ] {
        assert!(
            !options.iter().any(|o| o == stripped),
            "{stripped} compiled in: {options:?}"
        );
    }
    assert!(
        options.iter().any(|o| o == "OMIT_LOAD_EXTENSION"),
        "extension loading compiled in: {options:?}"
    );
    // And by behaviour, not only by name: none of the modules can be used.
    for module in ["fts3", "fts4", "fts5", "rtree", "dbstat"] {
        let err = conn
            .execute_batch(&format!(
                "CREATE VIRTUAL TABLE t_{module} USING {module}(x)"
            ))
            .unwrap_err();
        assert!(
            err.to_string().contains("no such module"),
            "{module}: {err}"
        );
    }
}

// The topic files (#576 AFK-D3): each is one contiguous run of the former single
// file, and its module doc names the lines it came from. The helpers above are
// the ones more than one of them uses; a helper only one file needs lives in that
// file. Two tests stay here: `save_project_tables_is_what_the_core_save_writes`
// reads `write.rs` through an `include_str!` path relative to this file, and CI
// runs `bundled_sqlite_omits_virtual_table_modules_and_extension_loading` by its
// exact name (`persistence::sqlite::tests::...`). `#[cfg(test)]` on each
// declaration is redundant for the compiler (this whole module is test-only) but
// is what marks them as test files for the Rust file-size ratchet and the source
// scans that skip `*_tests.rs`.
#[cfg(test)]
mod backup_recovery_tests;
#[cfg(test)]
mod compression_tests;
#[cfg(test)]
mod float_round_trip_tests;
#[cfg(test)]
mod legacy_history_tests;
#[cfg(test)]
mod open_integrity_tests;
#[cfg(test)]
mod pitch_migration_tests;
#[cfg(test)]
mod project_rows_tests;
#[cfg(test)]
mod project_setting_tests;
#[cfg(test)]
mod push_snapshot_tests;
#[cfg(test)]
mod raw_table_tests;
#[cfg(test)]
mod round_trip_tests;
#[cfg(test)]
mod save_load_tests;
#[cfg(test)]
mod schema_migration_tests;
#[cfg(test)]
mod section_id_bound_tests;
#[cfg(test)]
mod shot_kind_migration_tests;
#[cfg(test)]
mod shot_rank_tests;
#[cfg(test)]
mod speleo_link_tests;
#[cfg(test)]
mod speleo_undo_chain_tests;
#[cfg(test)]
mod undo_warning_persistence_tests;
#[cfg(test)]
mod version_gate_tests;
