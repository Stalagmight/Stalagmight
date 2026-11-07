//! [#576 RV2-R-NITS] The save coverage guard, down to `project_settings` keys.
//!
//! `every_schema_table_is_written_by_the_core_save_or_carried_by_the_registry`
//! checks tables. `project_settings` counts as core-written, but the core save
//! writes only its own keys into the fresh file: any other key in it is not
//! carried, and is gone after the next save. Today the one other key,
//! `pending_kind_classification`, is consumed by the load before any save - but a
//! future key the schema or a migration puts there would vanish on every save
//! with the table-level guard none the wiser. This guard names every key the
//! persistence code writes, and fails on one that is neither saved by the core
//! save nor consumed by the load.

/// The keys `Database::save_project` writes (`write.rs`).
const CORE_SAVED_KEYS: &[&str] = &[
    "validation",
    "surface_elevation",
    "default_azimuth_reference",
];

/// Keys a migration seeds that the load reads and deletes before any save can run.
const CONSUMED_ON_LOAD: &[&str] = &["pending_kind_classification"];

const SCHEMA: &str = include_str!("../../../../persistence/src/persistence/sqlite/schema.rs");
const READ: &str = include_str!("../../../../persistence/src/persistence/sqlite/read.rs");
const WRITE: &str = include_str!("../../../../persistence/src/persistence/sqlite/write.rs");

/// Every key literal an `INSERT ... INTO project_settings (key, value) VALUES ('<key>', ...)`
/// in `source` writes.
fn inserted_keys(source: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find("INTO project_settings") {
        rest = &rest[at + "INTO project_settings".len()..];
        let Some(values) = rest.find("VALUES") else {
            break;
        };
        let after = rest[values..].trim_start_matches("VALUES").trim_start();
        let after = after.trim_start_matches('(').trim_start();
        if let Some(quoted) = after.strip_prefix('\'') {
            if let Some(end) = quoted.find('\'') {
                keys.push(quoted[..end].to_owned());
            }
        }
    }
    keys
}

#[test]
fn every_project_settings_key_is_saved_by_the_core_save_or_consumed_on_load() {
    let written_by_save = inserted_keys(WRITE);
    let elsewhere: Vec<String> = inserted_keys(SCHEMA)
        .into_iter()
        .chain(inserted_keys(READ))
        .collect();
    println!("keys the core save writes: {written_by_save:?}");
    println!("keys the schema/migrations/load write: {elsewhere:?}");

    let mut saved_sorted = written_by_save;
    saved_sorted.sort();
    saved_sorted.dedup();
    let mut core: Vec<String> = CORE_SAVED_KEYS.iter().map(|k| (*k).to_owned()).collect();
    core.sort();
    assert_eq!(
        saved_sorted, core,
        "the core save's keys changed: update CORE_SAVED_KEYS (and check the carry)"
    );
    for key in &elsewhere {
        let saved = CORE_SAVED_KEYS.contains(&key.as_str());
        let consumed = CONSUMED_ON_LOAD.contains(&key.as_str());
        assert!(
            saved || consumed,
            "`project_settings` key `{key}` is written outside the core save but neither saved \
             by it nor consumed on load: every save would drop it"
        );
        if consumed {
            assert!(
                READ.contains(&format!("DELETE FROM project_settings WHERE key = '{key}'")),
                "`{key}` is listed as consumed on load, but the load never deletes it"
            );
        }
    }
}
