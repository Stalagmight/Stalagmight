//! Write a checked download into the cache slot, leaving the current copy in
//! place on any failure.
//!
//! The bytes are already a decoded, validated version 2 file
//! (`PreparedDownload`), so an install cannot produce a copy that fails to
//! open; what is left is the write itself. It goes through `write_atomic`
//! (temp beside the destination, data fsync, rename, directory fsync): the
//! cache holds either the old copy or the new one, never a mix, and a failure
//! leaves the old copy byte-identical.
//!
//! A legacy (SQLite) copy an older build left in the cache can have SQLite
//! sidecars beside it. They are moved out of SQLite's sight before the new file
//! takes the name and deleted once it has (`speleodb_sidecars`): a stale journal
//! beside a version 2 file is harmless to this build, but an older build that
//! opened the file would roll it into the JSON. Unlike every other write over
//! a legacy file (`legacy_upgrade`), this one keeps no backup of the 0.6.0
//! copy: what it held is on the server.
use std::collections::BTreeMap;
use std::path::Path;

use super::speleodb_cache_install::PreparedDownload;
use super::speleodb_sidecars::SetAsideSidecars;
use super::AppState;
use crate::persistence::stalx_v2::{write_atomic, Fingerprint, Guard};
use crate::speleodb::SpeleoDbError;

/// Write `bytes` over the cache file at `dest` (creating its directory),
/// setting any SQLite sidecar of the outgoing copy aside first. Returns the
/// new file's fingerprint (the save guard of a project opened from it).
pub(super) fn install_cache_bytes(dest: &Path, bytes: &[u8]) -> Result<Fingerprint, SpeleoDbError> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| SpeleoDbError::Io(format!("could not create the cache directory: {e}")))?;
    }
    // No handle of ours is ever open on a cache file, so whatever sidecar sits
    // beside it is the outgoing legacy copy's, and must not outlive it.
    let sidecars = SetAsideSidecars::take(dest)?;
    // The caller decided to replace this copy (the local-edits guard ran, or the
    // user confirmed the discard), so the destination is overwritten as it is.
    match write_atomic(dest, bytes, Guard::Overwrite) {
        Ok(fingerprint) => {
            sidecars.discard();
            Ok(fingerprint)
        }
        Err(error) => {
            sidecars.restore();
            Err(SpeleoDbError::Io(format!(
                "could not replace the project: {error}"
            )))
        }
    }
}

impl AppState {
    /// Install `prepared` at `dest` with `display` as its view toggles. The
    /// caller holds `project` through the install and the commit of the opened
    /// project (`open_project_download`), so no other open or save can land
    /// between them.
    pub(super) fn install_prepared(
        &self,
        prepared: &mut PreparedDownload,
        dest: &Path,
        display: BTreeMap<String, String>,
    ) -> Result<Fingerprint, SpeleoDbError> {
        let bytes = prepared.cache_bytes(display)?;
        install_cache_bytes(dest, &bytes)
    }
}
