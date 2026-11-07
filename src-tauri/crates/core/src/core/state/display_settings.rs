use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use ts_rs::TS;

use super::AppState;
use crate::core::{CommandError, CommandErrorCode};

/// [#576 RV6b-2, NF-W8FE-3] The open project a display-setting write was MADE
/// for, as the frontend names it when it makes the write.
///
/// `created_at` alone used to be the whole identity, and two copies of one project
/// file share it: a write still in flight when the user switched from one copy to
/// the other landed in the other. The file the frontend has the project open from
/// tells the copies apart.
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct DisplaySettingTarget {
    /// The project's `created_at`, in the RFC 3339 form the wire carries it in.
    pub created_at: String,
    /// The file the project is open from (`Project::file_path`); `None` for a
    /// project that has never been saved.
    pub file_path: Option<String>,
}

impl AppState {
    /// Load display settings from database or in-memory cache
    pub fn load_display_settings(&self) -> Result<HashMap<String, String>> {
        // Try database first
        let db_guard = self.database.lock();
        if let Some(db) = db_guard.as_ref() {
            return db.load_display_settings();
        }
        drop(db_guard);

        // Fall back to in-memory cache for new unsaved projects
        let cache = self.display_settings_cache.lock();
        Ok(cache.clone())
    }

    /// Save a display setting to database or in-memory cache
    ///
    /// [#576 RV5-1] Between commands, under `command_lock`. A save snapshots the
    /// display settings from the open file (or this cache), writes a fresh temp
    /// file and renames it over the open one, all under `command_lock`; a setting
    /// written through the open handle in between went into the file the rename
    /// replaced (and one written to the cache was cleared by the save), lost with
    /// `Ok` returned. Holding the lock puts the write wholly before or after the
    /// save. Never call it while holding `command_lock`.
    pub fn save_display_setting(&self, key: &str, value: &str) -> Result<()> {
        let _between_commands = self.hold_commands();
        self.write_display_setting(key, value)
    }

    /// [#576 RV6b-2] [`Self::save_display_setting`] for the project the write was
    /// MADE for ([`DisplaySettingTarget`]; `None` when the frontend had none open).
    ///
    /// A display setting belongs to one project, and the IPC carries no other
    /// identity: a write queued or still in flight when the user opened another
    /// project used to land in whichever project was open by the time it ran -
    /// invisible this session (the new project's settings came with its load) and
    /// shown on its next open. Refused with `PROJECT_CHANGED` when the identity is
    /// not the open project's; nothing is written then. The check and the write
    /// share one `command_lock` span, and every project-slot commit takes that
    /// lock, so no open can land between them.
    ///
    /// [#576 NF-W8FE-3] The same project means the same `created_at` AND the same
    /// file: the path the open project was committed from, or the one it has now.
    /// Either, because a Save / Save As moves the open project to another file
    /// without a new open, and a write made just before it names the old one.
    ///
    /// [#576 RV10b-4] Or any file a Save As has moved it off since that commit
    /// ([`Self::note_open_project_moved`]): a write made between two of them names
    /// the file in between. A file the project was never at this session - another
    /// copy - is still refused.
    pub fn save_display_setting_for(
        &self,
        made_for: Option<&DisplaySettingTarget>,
        key: &str,
        value: &str,
    ) -> Result<(), CommandError> {
        let _between_commands = self.hold_commands();
        let open = self
            .project
            .lock()
            .as_ref()
            .map(|p| (p.created_at.utc(), p.file_path.clone()));
        let same_project = match (made_for, open) {
            (None, None) => true,
            (Some(made_for), Some((open_created_at, open_path))) => {
                let same_created_at = DateTime::parse_from_rfc3339(&made_for.created_at)
                    .is_ok_and(|at| at.with_timezone(&Utc) == open_created_at);
                let made_for_path = made_for.file_path.as_ref().map(PathBuf::from);
                same_created_at
                    && (made_for_path == open_path
                        || self.opened_from.lock().contains(&made_for_path))
            }
            _ => false,
        };
        if !same_project {
            return Err(CommandError::new(
                CommandErrorCode::ProjectChanged,
                "The display setting was not saved: another project is open now.",
            ));
        }
        self.write_display_setting(key, value)
            .map_err(|e| CommandError::failed(e.to_string()))
    }

    /// [#576 RV10b-4] A save is moving the open project from `from` (its
    /// `file_path` before the save) to `to`, without a new open: keep `from` among
    /// the files this session's project has been at, so a display-setting write
    /// made for it still lands ([`Self::save_display_setting_for`]). The next
    /// project-slot commit starts the list again.
    pub(super) fn note_open_project_moved(&self, from: Option<&Path>, to: &Path) {
        if from == Some(to) {
            return;
        }
        let from = from.map(Path::to_path_buf);
        let mut files = self.opened_from.lock();
        if !files.contains(&from) {
            files.push(from);
        }
    }

    /// The write itself, for a caller already holding `command_lock`.
    fn write_display_setting(&self, key: &str, value: &str) -> Result<()> {
        // Try database first
        let db_guard = self.database.lock();
        if let Some(db) = db_guard.as_ref() {
            return db.save_display_setting(key, value);
        }
        drop(db_guard);

        // Fall back to in-memory cache for new unsaved projects
        let mut cache = self.display_settings_cache.lock();
        cache.insert(key.to_string(), value.to_string());
        Ok(())
    }

    /// Get a single display setting from database or in-memory cache
    pub fn get_display_setting(&self, key: &str) -> Result<Option<String>> {
        // Try database first
        let db_guard = self.database.lock();
        if let Some(db) = db_guard.as_ref() {
            return db.get_display_setting(key);
        }
        drop(db_guard);

        // Fall back to in-memory cache for new unsaved projects
        let cache = self.display_settings_cache.lock();
        Ok(cache.get(key).cloned())
    }

    /// Clear the in-memory display settings cache (called after save when settings are persisted to DB)
    pub(super) fn clear_display_settings_cache(&self) {
        self.display_settings_cache.lock().clear();
    }
}
