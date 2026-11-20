//! Synthetic version 2 `.stalx` fixtures for the SpeleoDB tests.
//!
//! Every file is written by the encoder (`stalx_v2::encode`) and read back
//! through the decoder or the header probe, never through SQLite: a cache
//! copy, a download and a push snapshot are all version 2 files now. The
//! legacy (SQLite) cache path is tested with synthetic sidecar files only.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Utc};

use super::open_file::FileState;
use crate::models::survey::{LineMeta, Project, StartAt};
use crate::models::{ShotData, SpeleoLink, UndoMode};
use crate::persistence::stalx_v2::{
    self, decode, probe_path, DecodeOptions, Decoded, Extras, History, Mode, RawJson,
};

/// The project id every fixture links to unless a test says otherwise.
pub(super) const FIXTURE_ID: &str = "11111111-1111-1111-1111-111111111111";

/// A linked SpeleoDB link to [`FIXTURE_ID`] on `endpoint`, with the dirty bit
/// (and its edit time) set or not.
pub(super) fn fixture_link(endpoint: &str, dirty: bool) -> SpeleoLink {
    SpeleoLink {
        endpoint: endpoint.to_owned(),
        project_id: FIXTURE_ID.to_owned(),
        project_name: "Remote project".to_owned(),
        base_commit_sha: Some("abc123".to_owned()),
        has_unpushed_edits: dirty,
        last_edit_at: dirty.then(|| "2026-07-27T10:00:00+00:00".to_owned()),
        last_verify_at: None,
        undo_mode: Some(UndoMode::Full),
    }
}

/// An empty survey named `name`.
pub(super) fn empty_survey(name: &str) -> Project {
    Project::new(name, None)
}

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

/// Encode `project` with `extras` (normal mode) to `path`.
pub(super) fn write_file(path: &Path, project: &Project, extras: &Extras) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture directory");
    }
    let bytes = stalx_v2::encode(project, extras, Mode::Normal).expect("encode a fixture");
    std::fs::write(path, bytes).expect("write a fixture");
}

/// An unlinked empty project named `name` at `path`.
pub(super) fn seed_unlinked(path: &Path, name: &str) {
    write_file(path, &empty_survey(name), &Extras::default());
}

/// An empty project named `name` at `path`, linked by `link`.
pub(super) fn seed_linked(path: &Path, name: &str, link: &SpeleoLink) {
    write_file(path, &empty_survey(name), &extras_for(Some(link)));
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
    write_file(path, &empty_survey(name), &extras);
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

/// The SpeleoDB link the file at `path` holds (read through the header probe).
pub(super) fn link_at(path: &Path) -> Option<SpeleoLink> {
    let header = probe_path(path).expect("probe a fixture");
    let mut file = FileState::default();
    file.extras.speleodb = header.speleodb;
    file.extras.local = header.local.unwrap_or_default();
    file.speleo_link()
}

/// The view toggles the file at `path` holds.
pub(super) fn display_at(path: &Path) -> BTreeMap<String, String> {
    probe_path(path)
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
    let mut extras = decoded(path).extras;
    let mut file = FileState {
        extras,
        ..FileState::default()
    };
    file.set_speleo_link(link);
    extras = file.extras;
    let bytes = stalx_v2::rewrite_local(&original, &extras).expect("rewrite a fixture");
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
