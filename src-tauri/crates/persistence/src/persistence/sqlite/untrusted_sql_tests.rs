//! [#619] Every open of an existing file refuses the SQLite features the repo's
//! build leaves out, whether or not the linked SQLite has them: creating a
//! virtual table, reading DBSTAT, and the functions FTS, R-tree and extension
//! loading register. The repo's `LIBSQLITE3_FLAGS` do not travel with a
//! published crate, so a crates.io build has every module compiled in, and the
//! authorizer is what keeps a hostile file away from them there. The probes
//! are `untrusted_sql_probes`, which `stalx-reader`'s inspect-connection test
//! runs too.
//!
//! CI also runs this test on a build without the flags, with
//! `STALAGMIGHT_EXPECT_UNSTRIPPED_SQLITE` set, which makes it check that the
//! modules really are there. All data is synthetic: a project saved by this
//! build.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;
use tempfile::TempDir;

use super::untrusted_sql_probes::{check_the_linked_sqlite, refusal_failures};
use crate::models::Project;
use crate::persistence::Database;

type Opener = fn(&Path) -> anyhow::Result<Database>;

const OPENERS: &[(&str, Opener)] = &[
    ("open_existing_untrusted", |p| {
        Database::open_existing_untrusted(p, 64 * 1024 * 1024, Duration::from_secs(30))
    }),
    ("open_existing", Database::open_existing),
    ("open", Database::open),
    ("open_existing_to_read", Database::open_existing_to_read),
    ("open_readonly", Database::open_readonly),
    ("open_for_metadata", Database::open_for_metadata),
    (
        "open_for_metadata_recovering",
        Database::open_for_metadata_recovering,
    ),
    ("open_for_recovery", Database::open_for_recovery),
    ("into_read_only", |p| {
        Database::open_existing(p)?.into_read_only(p)
    }),
];

/// A project saved by this build, at `dir`/CaveU.stalx, after `surgery`.
fn project_file(dir: &Path, surgery: &str) -> PathBuf {
    let path = dir.join("CaveU.stalx");
    Database::open(&path)
        .unwrap()
        .save_project(&Project::new("CaveU".to_string(), None))
        .unwrap();
    if !surgery.is_empty() {
        Connection::open(&path)
            .unwrap()
            .execute_batch(surgery)
            .unwrap();
    }
    path
}

#[test]
fn every_untrusted_open_refuses_what_the_build_flags_strip() {
    check_the_linked_sqlite();

    let mut failures = Vec::new();
    for (opener, open) in OPENERS {
        // An older file too, so open_existing_to_read takes its upgraded copy
        // in memory, a connection of its own.
        for (surgery, label) in [
            ("", (*opener).to_string()),
            (
                "UPDATE project SET schema_version = 16; PRAGMA user_version = 16;",
                format!("{opener} (older file)"),
            ),
        ] {
            let dir = TempDir::new().unwrap();
            let path = project_file(dir.path(), surgery);
            let db = open(&path).unwrap();
            failures.extend(refusal_failures(&db.conn, &label));
        }
    }
    assert!(
        failures.is_empty(),
        "not refused when prepared:\n{}",
        failures.join("\n")
    );
}
