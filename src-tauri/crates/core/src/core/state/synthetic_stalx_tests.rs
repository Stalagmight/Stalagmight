//! Synthetic `.stalx` files for the SpeleoDB tests.
//!
//! - [`write_v2`]: a version 2 file written by the encoder, linked or not, the
//!   way a save or a pull leaves it.
//! - [`write_legacy_link_only`]: a minimal SQLite file holding nothing but a
//!   `speleo_link` row. The link readers never read the project tables, so
//!   this is enough to drive their legacy branch (hot journals, narrow
//!   tables) without the legacy writer, which only the persistence crate's
//!   tests have.
//! - [`legacy_linked_project`]: the tracked legacy fixture (e2e/fixtures),
//!   copied and given a `speleo_link` row, for the paths that open or
//!   migrate a whole legacy project.

use std::path::{Path, PathBuf};

use super::open_file::FileState;
use crate::models::survey::Project;
use crate::models::SpeleoLink;
use crate::persistence::stalx_v2::{self, Mode};

/// A link to `project_id` on `endpoint` with the given base and dirty bit.
pub(super) fn link(
    endpoint: &str,
    project_id: &str,
    name: &str,
    base: Option<&str>,
    dirty: bool,
) -> SpeleoLink {
    SpeleoLink {
        endpoint: endpoint.to_owned(),
        project_id: project_id.to_owned(),
        project_name: name.to_owned(),
        base_commit_sha: base.map(str::to_owned),
        has_unpushed_edits: dirty,
        last_edit_at: dirty.then(|| "2026-11-09T10:00:00Z".to_owned()),
        last_verify_at: None,
        undo_mode: None,
    }
}

/// Write an empty project named `name` as a version 2 file at `path` (parent
/// directories created), carrying `link` (identity in `speleodb`, sync state
/// in `local.speleodb`).
pub(super) fn write_v2(path: &Path, name: &str, link: Option<&SpeleoLink>) {
    write_v2_project(path, &Project::new(name, None), link);
}

/// [`write_v2`] for a given project.
pub(super) fn write_v2_project(path: &Path, project: &Project, link: Option<&SpeleoLink>) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create the fixture's directory");
    }
    let mut file = FileState::default();
    file.set_speleo_link(link);
    let bytes = stalx_v2::encode(project, &file.extras, Mode::Normal)
        .expect("encode the synthetic project");
    std::fs::write(path, bytes).expect("write the synthetic project");
}

/// The link the file at `path` carries now, read through the shared probe.
pub(super) fn link_on_disk(path: &Path) -> Option<SpeleoLink> {
    match super::speleodb_local_state::probe_link(
        path,
        super::speleodb_local_state::LegacyRead::ReadOnly,
    ) {
        super::speleodb_local_state::FileLink::Read(link) => link,
        other => panic!("{} could not be read: {other:?}", path.display()),
    }
}

/// The instant an RFC 3339 time names (the stored spelling is not the one a
/// test wrote: a version 2 file stores instants).
pub(super) fn instant(text: Option<&str>) -> Option<chrono::DateTime<chrono::Utc>> {
    text.map(|t| {
        chrono::DateTime::parse_from_rfc3339(t)
            .expect("an RFC 3339 time")
            .with_timezone(&chrono::Utc)
    })
}

/// The `speleo_link` table 0.6.0 wrote (schema v13 on).
const LINK_TABLE: &str = "CREATE TABLE IF NOT EXISTS speleo_link (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    endpoint TEXT NOT NULL,
    project_id TEXT NOT NULL,
    project_name TEXT NOT NULL,
    base_commit_sha TEXT,
    has_unpushed_edits INTEGER NOT NULL DEFAULT 0,
    last_edit_at TEXT,
    last_verify_at TEXT,
    undo_mode TEXT
)";

fn insert_link(conn: &rusqlite::Connection, link: &SpeleoLink) {
    conn.execute_batch(LINK_TABLE).expect("link table");
    conn.execute("DELETE FROM speleo_link", [])
        .expect("clear the link row");
    conn.execute(
        "INSERT INTO speleo_link VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            link.endpoint,
            link.project_id,
            link.project_name,
            link.base_commit_sha,
            i64::from(link.has_unpushed_edits),
            link.last_edit_at,
            link.last_verify_at,
            link.undo_mode.map(crate::models::UndoMode::as_db_str),
        ],
    )
    .expect("insert the link row");
}

/// A SQLite file at `path` holding only `link`'s `speleo_link` row.
pub(super) fn write_legacy_link_only(path: &Path, link: &SpeleoLink) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create the fixture's directory");
    }
    let conn = rusqlite::Connection::open(path).expect("create the SQLite file");
    insert_link(&conn, link);
}

/// The tracked legacy fixture copied to `path`, carrying `link`.
pub(super) fn legacy_linked_project(path: &Path, link: Option<&SpeleoLink>) -> PathBuf {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../e2e/fixtures/fractal_plant_iter3.stalx");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create the fixture's directory");
    }
    std::fs::copy(&fixture, path).expect("copy the tracked legacy fixture");
    if let Some(link) = link {
        let conn = rusqlite::Connection::open(path).expect("open the legacy copy");
        insert_link(&conn, link);
    }
    path.to_path_buf()
}
