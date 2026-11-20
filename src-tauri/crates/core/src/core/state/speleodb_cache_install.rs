//! Stage downloaded files before the short, session-owned cache commit.
use std::collections::HashMap;
use std::path::Path;

use super::speleodb_cache::{
    drop_inherited_undo, preserved_display_settings, InheritedUndo, LocalEditsPolicy,
};
use super::speleodb_sidecars::SetAsideSidecars;
use super::AppState;
use crate::persistence::Database;
use crate::speleodb::SpeleoDbError;

/// Preserve the shared newer-schema refusal through hardened download opens.
pub(super) fn downloaded_file_error(error: anyhow::Error) -> SpeleoDbError {
    if let Some(newer) = error.downcast_ref::<crate::persistence::NewerSchema>() {
        return SpeleoDbError::NewerSchema(crate::persistence::NewerSchema {
            file: "project.stalx".into(),
            ..newer.clone()
        });
    }
    SpeleoDbError::Io(format!("could not open downloaded project: {error}"))
}

pub(super) fn sync_staged_file(path: &Path) -> Result<(), SpeleoDbError> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
        .map_err(|e| SpeleoDbError::Io(e.to_string()))
}

pub(super) fn sync_cache_directory(dest: &Path) {
    #[cfg(unix)]
    if let Some(parent) = dest.parent() {
        if let Err(error) = std::fs::File::open(parent).and_then(|dir| dir.sync_all()) {
            tracing::warn!(%error, "could not synchronize the installed cache directory");
        }
    }
    #[cfg(not(unix))]
    let _ = dest;
}

pub(super) fn stage_download(
    source: &Path,
    dest: &Path,
    undo: InheritedUndo,
    sync: fn(&Path) -> Result<(), SpeleoDbError>,
) -> Result<tempfile::TempPath, SpeleoDbError> {
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| SpeleoDbError::Io(e.to_string()))?;
    let staged = tempfile::Builder::new()
        .prefix(".speleodb-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|e| SpeleoDbError::Io(e.to_string()))?
        .into_temp_path();
    std::fs::copy(source, &staged).map_err(|e| SpeleoDbError::Io(e.to_string()))?;
    Database::check_schema_at(&staged).map_err(|e| {
        SpeleoDbError::Parse(format!("the downloaded project file was refused: {e:#}"))
    })?;
    Database::check_not_newer_at(&staged).map_err(|newer| {
        SpeleoDbError::NewerSchema(crate::persistence::NewerSchema {
            file: dest
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            ..newer
        })
    })?;
    if matches!(undo, InheritedUndo::Drop) {
        drop_inherited_undo(&staged)?;
    }
    sync(&staged)?;
    Ok(staged)
}

impl AppState {
    pub(super) fn reserve_download_generation(&self) -> u64 {
        let _project = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.bump_speleodb_session();
        self.speleodb_session_generation()
    }

    /// Network and validation happen before this commit. A newer open or login
    /// invalidates the download; pending native readers exclude replacement too.
    pub(super) fn replace_cache_for_session(
        &self,
        source: &Path,
        dest: &Path,
        project_id: &str,
        policy: LocalEditsPolicy,
        undo: InheritedUndo,
        generation: u64,
    ) -> Result<HashMap<String, String>, SpeleoDbError> {
        let _ = (&source, &dest, &project_id, &policy, &undo, &generation);
        crate::core::stub::not_ported("speleodb_cache_install::replace_cache_for_session")
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "synthetic cache durability tests")]
mod tests {
    use super::*;
    use crate::models::Project;

    #[test]
    fn failed_stage_sync_preserves_the_cache_and_cleans_the_stage() {
        let dir = tempfile::tempdir().expect("temp");
        let source = dir.path().join("source.stalx");
        let dest = dir.path().join("working.stalx");
        for path in [&source, &dest] {
            Database::open(path)
                .expect("database")
                .save_project(&Project::new("Synthetic".into(), None))
                .expect("project");
        }
        let before = std::fs::read(&dest).expect("old bytes");
        let result = stage_download(&source, &dest, InheritedUndo::Keep, |_| {
            Err(SpeleoDbError::Io("synthetic sync failure".into()))
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(&dest).expect("unchanged"), before);
        assert_eq!(std::fs::read_dir(dir.path()).expect("directory").count(), 2);
    }
}
