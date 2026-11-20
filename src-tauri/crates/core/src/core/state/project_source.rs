//! Reading another project file as a SOURCE for "Add .stalx to current project"
//! and "Merge projects" (#674, #680; plan 5.8).
//!
//! The source is read by the open path's own reader
//! ([`AppState::read_project_file`]): the bytes sniffed, a version 2 file
//! decoded and validated, a legacy SQLite file migrated in memory with the
//! user's least-squares config. The legacy reader works on a private copy of
//! the file and its `-wal` / `-journal` sidecars, so a committed WAL is read
//! and a hot journal rolled back exactly as an open would, while the user's
//! file - possibly a SpeleoDB cache copy compared byte for byte against its
//! server commit - is never written. One reader, so the add, the merge and
//! the open cannot drift apart on what a project file holds.
//!
//! Around it, only what a source needs: the refusals that must come before
//! the file is opened (not a regular file, too large), a source that would
//! open read-only (made with a newer Stalagmight: copying it would drop what
//! this build does not understand), and datum GPS out of range (the model
//! checks only that numbers are finite). The source's undo history is never
//! used.

use std::path::Path;

use super::AppState;
use crate::models::survey::Project;
use crate::models::GpsCoordinates;
use crate::persistence::stalx_v2::IgnoredWarning;

/// A source project as "Add .stalx" and "Merge projects" read it.
pub(super) struct SourceProject {
    /// The survey, exactly as an open of the file would hold it.
    pub(super) project: Project,
    /// Its ignored warnings, keyed by the source's shot uuids (a legacy
    /// file's re-keyed by the migration).
    pub(super) ignores: Vec<IgnoredWarning>,
    /// The SpeleoDB project the source is linked to, by name. Never copied:
    /// it only words one receipt line.
    pub(super) link_name: Option<String>,
}

/// The largest `.stalx` an add or a merge reads: the ceiling a SpeleoDB
/// download of a project file is held to. The read runs under
/// `command_lock`, so an unbounded one would stall every edit behind it.
const MAX_SOURCE_BYTES: u64 = crate::speleodb::client::NATIVE_LIMIT;

/// The SQLite files that carry a legacy database's data beside it, named
/// after it (`x.stalx-wal`). Moved with an archived source; the legacy
/// reader copies them with the source.
pub(super) const SQLITE_SIDECARS: [&str; 2] = ["-wal", "-journal"];

/// `path` with `suffix` appended to its file name (`x.stalx` -> `x.stalx-wal`).
pub(super) fn with_suffix(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    std::path::PathBuf::from(name)
}

impl AppState {
    /// Read `path` as a source, WITHOUT touching it. `label` names the file in
    /// every refusal; `verb` ("added", "merged") says what was refused.
    pub(super) fn read_source_project(
        &self,
        path: &Path,
        label: &str,
        verb: &str,
    ) -> Result<SourceProject, String> {
        check_source_file(path, label, verb)?;
        let opened = self
            .read_project_file(path)
            .map_err(|e| format!("Failed to load project '{label}': {}", e.message))?;
        if opened.file.extras.read_only.is_some() {
            return Err(format!(
                "'{label}' was made with a newer Stalagmight, so it cannot be {verb} \
                 without losing what this version does not understand. No changes were made."
            ));
        }
        refuse_invalid_gps(&opened.project, label, verb)?;
        let link_name = opened.file.speleo_link().map(|link| link.project_name);
        Ok(SourceProject {
            project: opened.project,
            ignores: opened.file.extras.ignored_warnings,
            link_name,
        })
    }
}

/// Refuse a source that is not a regular file of at most
/// [`MAX_SOURCE_BYTES`] (with its SQLite sidecars), from its metadata alone:
/// a FIFO or a device named `*.stalx` would block the read or never end, and
/// a directory cannot be a project. Checked before the file is opened
/// (opening a FIFO for reading blocks).
fn check_source_file(path: &Path, label: &str, verb: &str) -> Result<(), String> {
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
    let sidecars: u64 = SQLITE_SIDECARS
        .iter()
        .filter_map(|suffix| std::fs::metadata(with_suffix(path, suffix)).ok())
        .filter(std::fs::Metadata::is_file)
        .map(|m| m.len())
        .sum();
    if metadata.len().saturating_add(sidecars) > MAX_SOURCE_BYTES {
        return Err(format!(
            "'{label}' is larger than {limit_mb} MB, the largest project file that can be {verb}"
        ));
    }
    Ok(())
}

/// Refuse a source whose datum GPS is out of range: `Project::validate`
/// checks that it is finite, not that it names a place, and an out-of-range
/// datum would place the copy somewhere meaningless (and frame the target
/// with it when the target has no datum).
fn refuse_invalid_gps(project: &Project, label: &str, verb: &str) -> Result<(), String> {
    for datum in &project.datums {
        let gps = &datum.gps;
        if let Err(e) = GpsCoordinates::validate(gps.latitude, gps.longitude, gps.depth_reference) {
            return Err(format!(
                "'{label}' was not {verb}: a zero datum has invalid GPS coordinates ({e}). \
                 The file is damaged or was written by another tool. No changes were made."
            ));
        }
    }
    Ok(())
}
