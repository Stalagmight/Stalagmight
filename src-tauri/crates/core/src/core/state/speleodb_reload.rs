//! Install an already parsed project while retaining the current tree on failure.
use std::path::Path;

use super::speleodb_sidecars::SetAsideSidecars;
use super::AppState;
use crate::models::Project;
use crate::persistence::Database;
use crate::speleodb::SpeleoDbError;

impl AppState {
    /// Caller holds project through file installation and the prepared state commit.
    pub(super) fn install_prepared_database(
        &self,
        source: &Path,
        dest: &Path,
        prepared: Database,
        current: Option<&Project>,
    ) -> Result<Database, SpeleoDbError> {
        let io_error = |error: std::io::Error| SpeleoDbError::Io(error.to_string());
        let parent = dest.parent().unwrap_or_else(|| Path::new("."));
        let backup = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
        let had_file = dest.try_exists().map_err(io_error)?;
        if had_file {
            std::fs::copy(dest, backup.path()).map_err(io_error)?;
        }
        // Windows requires both SQLite handles closed before replacing their files.
        drop(prepared);
        self.database
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let reopen_current = |error: String| -> Result<Database, SpeleoDbError> {
            if let Some(path) = current.and_then(|project| project.file_path.as_deref()) {
                let original = Database::open_existing(path).map_err(|e| {
                    SpeleoDbError::Io(format!("could not reopen original project: {e}"))
                })?;
                *self
                    .database
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(original);
            }
            Err(SpeleoDbError::Io(format!(
                "could not replace project: {error}"
            )))
        };
        // With both handles closed, a sidecar beside `dest` is the outgoing file's;
        // see `speleodb_sidecars`. A failure to move it replaces nothing.
        let sidecars = match SetAsideSidecars::take(dest) {
            Ok(sidecars) => sidecars,
            Err(error) => return reopen_current(error.to_string()),
        };
        if let Err(error) = std::fs::rename(source, dest) {
            sidecars.restore();
            return reopen_current(error.to_string());
        }
        match Database::open_existing(dest) {
            Ok(db) => {
                sidecars.discard();
                super::speleodb_cache_install::sync_cache_directory(dest);
                Ok(db)
            }
            Err(error) => {
                // The outgoing sidecars may only go back beside the outgoing bytes:
                // beside the NEW file they are exactly the replay being prevented.
                if !had_file {
                    sidecars.discard();
                } else if let Err(copy) = std::fs::copy(backup.path(), dest) {
                    sidecars.discard();
                    return Err(io_error(copy));
                } else {
                    sidecars.restore();
                }
                reopen_current(error.to_string())
            }
        }
    }
}
