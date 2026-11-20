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
        let _ = (&source, &dest, &prepared, &current);
        crate::core::stub::not_ported("speleodb_reload::install_prepared_database")
    }
}
