//! Reading another project file as a SOURCE for "Add .stalx to current project"
//! and "Merge projects" (#674, #680): through a private copy, normalized the
//! way opening it would be, refusing input that cannot be trusted. One reader,
//! so the two features cannot drift apart on what a source looks like.

use std::path::Path;

use crate::models::{ClassifyScope, GpsCoordinates, Project, SpeleoLink};
use crate::persistence::{Database, ResolvedWarning};

/// A source project as "Add .stalx" and "Merge projects" read it.
pub(super) struct SourceProject {
    pub(super) project: Project,
    /// Its `resolved_warnings` rows; `None` when the table could not be read.
    pub(super) ignores: Option<Vec<ResolvedWarning>>,
    pub(super) link: Option<SpeleoLink>,
}

/// Read `path` WITHOUT touching it, normalized the way opening it would be.
/// `label` names the file in every refusal; `verb` ("added", "merged") says
/// what was refused.
///
/// Through a private copy: opening an older file migrates it in place, and the
/// source - possibly a SpeleoDB-managed cache copy whose bytes are compared
/// against its server commit - must stay exactly as it was. The source's undo
/// history is never read.
pub(super) fn read_source_project(
    path: &Path,
    label: &str,
    verb: &str,
) -> Result<SourceProject, String> {
    let dir = tempfile::tempdir()
        .map_err(|e| format!("Failed to prepare a private copy of '{label}': {e}"))?;
    let copy = dir.path().join("source.stalx");
    copy_regular_file(path, &copy, label, verb)?;
    copy_sqlite_sidecars(path, &copy, label, verb)?;
    let db = Database::open_existing(&copy)
        .map_err(|e| format!("Failed to open project '{label}': {e}"))?;
    let mut project = db
        .load_project()
        .map_err(|e| format!("Failed to load project '{label}': {e}"))?;
    let ignores = db.load_resolved_warnings_verbatim().ok();
    // An unreadable link table on a file whose project DID load is treated as
    // no link: the copy carries survey data only either way, so the link only
    // ever decides one receipt line.
    let link = db.load_speleo_link().ok().flatten();
    // The v12->v13 migration that just ran on the copy seeded every legacy
    // reference shot as Jump and left this flag for the open path to finish.
    // Read before the copy is dropped: it goes with the temp database, so an
    // add that skipped it would leave those shots swimmable for good.
    let kind_classification_pending = db.is_kind_classification_pending();
    drop(db);
    refuse_non_finite_placement(&project, label, verb)?;

    // What an open does to a freshly read project, in the same order
    // (`do_load_project_full`): every shot gets a uuid (the identity ignores
    // and cross-references are re-keyed onto), legacy closures get ids, the
    // derived offsets are re-solved rather than trusted, and a pending #27
    // Jump/Reference backfill is run on the source's own graph. Nothing is
    // written back: the copy is discarded and the source stays untouched.
    project.ensure_shot_uuids();
    project.assign_closure_ids();
    project.redrive_offsets_after_load();
    if kind_classification_pending {
        project.classify_virtual_shots(&ClassifyScope::WholeProject);
    }
    Ok(SourceProject {
        project,
        ignores,
        link,
    })
}

/// Refuse a source whose placement data cannot be trusted: the file reader
/// builds zero-datum GPS without `GpsCoordinates::new`'s validation, and reads
/// offsets as stored. A NaN or infinite value would pass every tolerance
/// comparison downstream (NaN is never greater than a tolerance) and be written
/// into the open project as section offsets; an out-of-range datum would place
/// the copy somewhere meaningless. Checked on the raw read, before anything
/// derives from it.
fn refuse_non_finite_placement(project: &Project, label: &str, verb: &str) -> Result<(), String> {
    let refuse = |what: String| {
        Err(format!(
            "'{label}' was not {verb}: {what}. The file is damaged or was written by \
             another tool. No changes were made."
        ))
    };
    for datum in &project.zero_datums {
        let gps = &datum.gps;
        if let Err(e) = GpsCoordinates::validate(gps.latitude, gps.longitude, gps.depth_reference) {
            return refuse(format!("a zero datum has invalid GPS coordinates ({e})"));
        }
    }
    if project
        .section_offsets
        .values()
        .any(|(x, y)| !x.is_finite() || !y.is_finite())
    {
        return refuse("a section offset is not a finite number".to_string());
    }
    if project
        .station_offsets
        .values()
        .any(|(x, y, z)| !x.is_finite() || !y.is_finite() || !z.is_finite())
    {
        return refuse("a station offset is not a finite number".to_string());
    }
    Ok(())
}

/// The largest `.stalx` an add or a merge reads: the ceiling a SpeleoDB download of a
/// project file is already held to. The read runs under `command_lock`, so an
/// unbounded one would stall every edit behind it (and fill the disk).
const MAX_SOURCE_BYTES: u64 = crate::speleodb::client::NATIVE_LIMIT;

/// Copy `path` to `dest`, refusing anything that is not a regular file of at
/// most [`MAX_SOURCE_BYTES`]: a FIFO or a device named `*.stalx` would
/// block or never end, and a directory cannot be a project.
///
/// The metadata check runs before the file is opened (opening a FIFO for
/// reading blocks), and the copy itself is bounded too, so a file that grows
/// or is swapped after the check still cannot exceed the ceiling.
fn copy_regular_file(path: &Path, dest: &Path, label: &str, verb: &str) -> Result<(), String> {
    let limit_mb = MAX_SOURCE_BYTES / (1024 * 1024);
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("File not found: {label}"));
        }
        Err(e) => return Err(format!("Failed to read '{label}': {e}")),
    };
    if !metadata.is_file() {
        return Err(format!("'{label}' is not a project file"));
    }
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err(format!(
            "'{label}' is larger than {limit_mb} MB, the largest project file that can be {verb}"
        ));
    }
    let source = std::fs::File::open(path).map_err(|e| format!("Failed to read '{label}': {e}"))?;
    let mut target =
        std::fs::File::create(dest).map_err(|e| format!("Failed to read '{label}': {e}"))?;
    let copied = std::io::copy(
        &mut std::io::Read::take(source, MAX_SOURCE_BYTES + 1),
        &mut target,
    )
    .map_err(|e| format!("Failed to read '{label}': {e}"))?;
    if copied > MAX_SOURCE_BYTES {
        return Err(format!(
            "'{label}' grew past {limit_mb} MB while it was being read, so it was not {verb}"
        ));
    }
    Ok(())
}

/// Copy the SQLite sidecars of `path` that exist beside it - a `-wal` holding
/// committed pages not yet checkpointed into the database, a hot `-journal`
/// left by an interrupted write - to the same names beside `dest`.
///
/// Both are named after the database and only mean anything next to it: a copy
/// of the main file alone would read a stale database (the WAL's commits
/// missing) or a half-written one (the journal never rolled back) where opening
/// the original would not. With them beside the copy, SQLite replays or rolls
/// back exactly as it would for the original. `-shm` is not copied: it is an
/// index SQLite rebuilds from the WAL.
fn copy_sqlite_sidecars(path: &Path, dest: &Path, label: &str, verb: &str) -> Result<(), String> {
    for suffix in ["-wal", "-journal"] {
        let from = with_suffix(path, suffix);
        if std::fs::metadata(&from).is_ok_and(|m| m.is_file()) {
            copy_regular_file(&from, &with_suffix(dest, suffix), label, verb)?;
        }
    }
    Ok(())
}

fn with_suffix(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    std::path::PathBuf::from(name)
}
