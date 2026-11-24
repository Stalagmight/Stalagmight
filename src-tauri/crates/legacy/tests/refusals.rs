//! (c) What the legacy open refuses, and that it never writes to the input.
#![expect(clippy::unwrap_used, reason = "test setup and assertions")]

use std::path::Path;

use stalagmight_legacy::models as m;
use stalagmight_legacy::{
    open_bytes, open_path, write_v18, LegacyError, LegacyExtras, OpenOptions, Positions, Slot,
    SUPPORTED_SCHEMA_VERSION,
};

#[path = "support/builder.rs"]
mod builder;

fn bits(positions: &Positions) -> Vec<(Slot, [u64; 3])> {
    positions
        .iter()
        .map(|(slot, p)| (*slot, p.map(f64::to_bits)))
        .collect()
}

/// The kitchen-sink project, as 0.6.0 saves it.
fn written() -> Vec<u8> {
    let (_, project): (_, m::Project) = builder::all().remove(0);
    write_v18(&project, &LegacyExtras::default()).unwrap()
}

#[test]
fn a_non_sqlite_input_is_refused_as_not_sqlite() {
    for bytes in [
        &b""[..],
        b"SQLite",
        b"{\"format\":\"stalx\",\"version\":2}\n",
        &[0u8; 4096],
    ] {
        let err = open_bytes(bytes, &OpenOptions::default()).unwrap_err();
        assert!(matches!(err, LegacyError::NotSqlite), "{err}");
    }
    // A valid header over garbage pages: SQLite itself says "not a database".
    let mut bytes = b"SQLite format 3\0".to_vec();
    bytes.extend(std::iter::repeat_n(0xA5, 8192));
    let err = open_bytes(&bytes, &OpenOptions::default()).unwrap_err();
    assert!(
        matches!(err, LegacyError::NotSqlite | LegacyError::Corrupt(_)),
        "{err}"
    );
}

#[test]
fn a_file_stamped_newer_than_18_is_refused_as_newer_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("newer.stalx");
    std::fs::write(&path, written()).unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", SUPPORTED_SCHEMA_VERSION + 1)
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    let err = open_path(&path, &OpenOptions::default()).unwrap_err();
    assert!(
        matches!(
            err,
            LegacyError::NewerSchema {
                found: 19,
                supported: 18
            }
        ),
        "{err}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn an_input_over_the_limit_is_refused_before_it_is_read() {
    let bytes = written();
    let opts = OpenOptions {
        max_file_bytes: bytes.len() as u64 - 1,
        ..OpenOptions::default()
    };
    let err = open_bytes(&bytes, &opts).unwrap_err();
    assert!(matches!(err, LegacyError::TooLarge { .. }), "{err}");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.stalx");
    std::fs::write(&path, &bytes).unwrap();
    let err = open_path(&path, &opts).unwrap_err();
    assert!(matches!(err, LegacyError::TooLarge { .. }), "{err}");
}

#[test]
fn a_hostile_schema_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hostile.stalx");
    std::fs::write(&path, written()).unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE VIEW sneaky AS SELECT 1;")
        .unwrap();
    let err = open_path(&path, &OpenOptions::default()).unwrap_err();
    assert!(
        matches!(&err, LegacyError::RefusedSchema(refused) if refused.name == "sneaky"),
        "{err}"
    );
}

#[test]
fn the_original_file_is_byte_identical_after_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plain.stalx");
    let bytes = written();
    std::fs::write(&path, &bytes).unwrap();
    let survey = open_path(&path, &OpenOptions::default()).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(survey.project.file_path.as_deref(), Some(path.as_path()));
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 1, "no sidecar or temp file appears: {names:?}");
}

fn sidecar(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

/// A crash mid-save leaves the database with some pages rewritten and a hot
/// `-journal` holding the originals. Build that pair from a real transaction:
/// spill dirty pages into the file before commit, then copy both files while
/// the transaction is still open.
fn crashed_mid_transaction(original: &[u8], dir: &Path) -> std::path::PathBuf {
    let scratch = dir.join("scratch.stalx");
    std::fs::write(&scratch, original).unwrap();
    let conn = rusqlite::Connection::open(&scratch).unwrap();
    conn.execute_batch(
        "PRAGMA cache_size = 10;
         BEGIN IMMEDIATE;
         UPDATE shots SET length = length * 3;
         CREATE TABLE ballast (blob BLOB);
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 400)
         INSERT INTO ballast SELECT zeroblob(4000) FROM n;",
    )
    .unwrap();
    let crashed = dir.join("crashed.stalx");
    std::fs::copy(&scratch, &crashed).unwrap();
    std::fs::copy(sidecar(&scratch, "-journal"), sidecar(&crashed, "-journal")).unwrap();
    drop(conn);
    crashed
}

#[test]
fn a_hot_journal_is_rolled_back_on_the_copy_and_both_originals_stay_as_they_were() {
    let original = written();
    let dir = tempfile::tempdir().unwrap();
    let crashed = crashed_mid_transaction(&original, dir.path());
    let journal = sidecar(&crashed, "-journal");
    let db_before = std::fs::read(&crashed).unwrap();
    let journal_before = std::fs::read(&journal).unwrap();
    assert_ne!(db_before, original, "the transaction reached the file");
    assert!(!journal_before.is_empty());

    let survey = open_path(&crashed, &OpenOptions::default()).unwrap();

    assert_eq!(std::fs::read(&crashed).unwrap(), db_before);
    assert_eq!(std::fs::read(&journal).unwrap(), journal_before);
    // Rolled back: the geometry is the committed file's, not the tripled one.
    let committed = open_bytes(&original, &OpenOptions::default()).unwrap();
    assert_eq!(bits(&survey.raw_positions), bits(&committed.raw_positions));
    assert_eq!(
        serde_json::to_string(&survey.project.lines).unwrap(),
        serde_json::to_string(&committed.project.lines).unwrap()
    );
}
