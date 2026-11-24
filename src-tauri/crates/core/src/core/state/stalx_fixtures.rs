//! Synthetic `.stalx` files for the tests, the SpeleoDB suites above all.
//!
//! - **Version 2 files**, written by the encoder (`stalx_v2::encode`), linked
//!   or not, the way a save or a pull leaves them: [`write_v2`] (an empty
//!   project), [`write_v2_project`] (a given one), [`write_file`] (any
//!   extras) and [`seed_with_history`]. They are read back through the
//!   decoder or the header probe ([`decoded`], [`name_at`], [`link_on_disk`],
//!   [`display_at`], [`history_counts`]), never through SQLite, and their
//!   per-machine members are rewritten the way the app does it ([`relink`],
//!   [`set_display`]). A cache copy, a download and a push snapshot are all
//!   version 2 files.
//! - **Legacy (SQLite) files.** [`write_legacy_link_only`] writes a minimal
//!   SQLite file holding nothing but a `speleo_link` row. The link readers
//!   never read the project tables, so this is enough to drive their legacy
//!   branch (hot journals, narrow tables). [`legacy_linked_project`] writes a
//!   whole synthetic project with the frozen 0.6.0 save
//!   (`legacy_fixture_kit`), for the paths that open or migrate one.
//!
//! All data is synthetic; nothing reads or copies a fixture.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::open_file::FileState;
use crate::models::survey::{LineMeta, Project, StartAt};
use crate::models::{ShotData, SpeleoLink, UndoMode};
use crate::persistence::stalx_v2::{
    self, decode, DecodeOptions, Decoded, Extras, History, Mode, RawJson,
};

// ----- links -------------------------------------------------------------------

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

/// The project id every [`fixture_link`] links to.
pub(super) const FIXTURE_ID: &str = "11111111-1111-1111-1111-111111111111";

/// The cache suites' link: [`FIXTURE_ID`] on `endpoint`, named "Remote
/// project", based on `abc123`, in Full undo mode, with the dirty bit (and
/// its edit time) set or not.
pub(super) fn fixture_link(endpoint: &str, dirty: bool) -> SpeleoLink {
    SpeleoLink {
        last_edit_at: dirty.then(|| "2026-07-27T10:00:00+00:00".to_owned()),
        undo_mode: Some(UndoMode::Full),
        ..link(
            endpoint,
            FIXTURE_ID,
            "Remote project",
            Some("abc123"),
            dirty,
        )
    }
}

/// The instant an RFC 3339 time names (the stored spelling is not the one a
/// test wrote: a version 2 file stores instants).
pub(super) fn instant(text: Option<&str>) -> Option<DateTime<Utc>> {
    text.map(|t| {
        DateTime::parse_from_rfc3339(t)
            .expect("an RFC 3339 time")
            .with_timezone(&Utc)
    })
}

// ----- version 2 files -------------------------------------------------------------

/// A one-line survey named `name` (three shots), for tests that need a body
/// with shots in it.
pub(super) fn line_survey(name: &str) -> Project {
    let shot = |length: f64, azimuth: f64| ShotData {
        length,
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        depth_in: 1.0,
        depth_out: 1.5,
        ..ShotData::default()
    };
    let mut project = Project::new(name, None);
    let line = project
        .add_line(
            LineMeta {
                name: Some("Main".to_owned()),
                ..LineMeta::default()
            },
            StartAt::New,
            vec![shot(10.0, 0.0), shot(12.0, 45.0), shot(8.0, 90.0)],
        )
        .expect("a fixture line");
    let start = project.line(line).expect("the line").start;
    project.set_placement(start, 0.0, 0.0).expect("a placement");
    project
}

/// The extras of a file linked by `link` (identity in `speleodb`, sync state
/// in `local.speleodb`), or of an unlinked file.
pub(super) fn extras_for(link: Option<&SpeleoLink>) -> Extras {
    let mut file = FileState::default();
    file.set_speleo_link(link);
    file.extras
}

/// Encode `project` with `extras` (normal mode) to `path`, parent
/// directories created.
pub(super) fn write_file(path: &Path, project: &Project, extras: &Extras) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture directory");
    }
    let bytes = stalx_v2::encode(project, extras, Mode::Normal).expect("encode a fixture");
    std::fs::write(path, bytes).expect("write a fixture");
}

/// [`write_file`] for `project`, carrying `link` (or none).
pub(super) fn write_v2_project(path: &Path, project: &Project, link: Option<&SpeleoLink>) {
    write_file(path, project, &extras_for(link));
}

/// An empty project named `name` at `path`, carrying `link` (or none).
pub(super) fn write_v2(path: &Path, name: &str, link: Option<&SpeleoLink>) {
    write_v2_project(path, &Project::new(name, None), link);
}

/// `count` undo entries the format carries (only `"t"` is read by the
/// format; the core drops a history it cannot decode, so these suit tests
/// about what a FILE holds, not about undoing).
pub(super) fn raw_entries(first: DateTime<Utc>, count: usize, pad: usize) -> Vec<RawJson> {
    (0..count)
        .map(|i| {
            let at = first + chrono::Duration::seconds(i64::try_from(i).unwrap_or(i64::MAX));
            let text = format!(
                r#"{{"v":1,"t":"{}","k":"fixture","label":"edit {i}","pad":"{}"}}"#,
                at.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
                noise(i as u64, pad)
            );
            RawJson::parse(&text).expect("a fixture history entry")
        })
        .collect()
}

/// Deterministic, incompressible lower-case text.
pub(super) fn noise(seed: u64, len: usize) -> String {
    let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    (0..len)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            char::from(b'a' + u8::try_from((state >> 33) % 26).unwrap_or(0))
        })
        .collect()
}

/// A file named `name` linked by `link` carrying `undo` and `redo` raw
/// history entries and an Unwind chain position and watermark (a Full-mode
/// push's snapshot as another machine left it).
pub(super) fn seed_with_history(
    path: &Path,
    name: &str,
    link: &SpeleoLink,
    undo: usize,
    redo: usize,
) {
    let mut extras = extras_for(Some(link));
    let first = DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z")
        .expect("a fixture time")
        .with_timezone(&Utc);
    extras.history = History {
        undo: raw_entries(first, undo, 16),
        redo: raw_entries(first + chrono::Duration::hours(1), redo, 16),
    };
    extras.local.speleodb.unwound_from = Some("older-revision".to_owned());
    extras.local.speleodb.inherited_through = Some(first);
    write_file(path, &Project::new(name, None), &extras);
}

/// The decoded file at `path`.
pub(super) fn decoded(path: &Path) -> Decoded {
    let bytes = std::fs::read(path).expect("read a fixture");
    decode(&bytes, &DecodeOptions::default()).expect("decode a fixture")
}

/// The project name the file at `path` holds.
pub(super) fn name_at(path: &Path) -> String {
    decoded(path).project.name
}

/// The link the file at `path` carries now, version 2 or legacy, read through
/// the shared probe (a version 2 header must be complete).
pub(super) fn link_on_disk(path: &Path) -> Option<SpeleoLink> {
    match super::speleodb_local_state::probe_link(
        path,
        super::speleodb_local_state::LegacyRead::ReadOnly,
    ) {
        super::speleodb_local_state::FileLink::Read(link) => link,
        other => panic!("{} could not be read: {other:?}", path.display()),
    }
}

/// The view toggles the file at `path` holds.
pub(super) fn display_at(path: &Path) -> BTreeMap<String, String> {
    stalx_v2::probe_path(path)
        .expect("probe a fixture")
        .local
        .unwrap_or_default()
        .display
}

/// The history the file at `path` holds: (undo entries, redo entries).
pub(super) fn history_counts(path: &Path) -> (usize, usize) {
    let history = decoded(path).extras.history;
    (history.undo.len(), history.redo.len())
}

/// Rewrite the link of the file at `path` (a local-only rewrite, as the app
/// does).
pub(super) fn relink(path: &Path, link: Option<&SpeleoLink>) {
    let original = std::fs::read(path).expect("read a fixture");
    let mut file = FileState {
        extras: decoded(path).extras,
        ..FileState::default()
    };
    file.set_speleo_link(link);
    let bytes = stalx_v2::rewrite_local(&original, &file.extras).expect("rewrite a fixture");
    std::fs::write(path, bytes).expect("write a fixture");
}

/// Set one view toggle in the file at `path`.
pub(super) fn set_display(path: &Path, key: &str, value: &str) {
    let original = std::fs::read(path).expect("read a fixture");
    let mut extras = decoded(path).extras;
    extras
        .local
        .display
        .insert(key.to_owned(), value.to_owned());
    let bytes = stalx_v2::rewrite_local(&original, &extras).expect("rewrite a fixture");
    std::fs::write(path, bytes).expect("write a fixture");
}

// ----- legacy (SQLite) files ---------------------------------------------------------

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

/// A SQLite file at `path` holding only `link`'s `speleo_link` row (an
/// existing row at `path` is replaced).
pub(super) fn write_legacy_link_only(path: &Path, link: &SpeleoLink) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create the fixture's directory");
    }
    let conn = rusqlite::Connection::open(path).expect("create the SQLite file");
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
            link.undo_mode.map(UndoMode::as_db_str),
        ],
    )
    .expect("insert the link row");
}

/// A whole legacy project at `path` (one line of three shots, its start a
/// datum), written by the 0.6.0 save and carrying `link` (or none).
pub(super) fn legacy_linked_project(path: &Path, link: Option<&SpeleoLink>) -> PathBuf {
    use super::legacy_fixture_kit::{
        frozen, old, slot, write_legacy_file_with, Legacy, LegacyExtras,
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create the fixture's directory");
    }
    let shot = |length: f64, azimuth: f64| ShotData {
        length,
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        depth_in: 2.0,
        depth_out: 3.0,
        ..ShotData::default()
    };
    let mut project = Legacy::new("Legacy".to_owned(), None);
    let line = project
        .add_line(old::LineData {
            name: Some("Main".to_owned()),
            color: "#3388FF".to_owned(),
            shots: [shot(10.0, 0.0), shot(12.0, 45.0), shot(8.0, 90.0)]
                .iter()
                .map(frozen)
                .collect(),
            ..Default::default()
        })
        .id;
    project
        .set_zero_datum(
            slot(line, 0),
            old::GpsCoordinates::new(20.4, -87.3, 0.0).expect("a valid GPS"),
        )
        .expect("a legacy datum");
    let extras = LegacyExtras {
        speleo_link: link.map(frozen),
        ..LegacyExtras::default()
    };
    write_legacy_file_with(path, &project, &extras);
    path.to_path_buf()
}
