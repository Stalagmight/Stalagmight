use std::path::PathBuf;

use tauri::State;

use crate::core::AppState;
use crate::persistence::{BackupInfo, RepairResult, SalvagedData};

/// Attempt to repair a project file the load refused (the recovery dialog's
/// "Attempt Repair").
///
/// A version 2 file is salvaged: the damaged file is kept as a numbered
/// `.backup` and the repaired project (`stalx_v2::decode_salvage`, every
/// repair listed in the message) takes its place. A legacy SQLite file gets
/// 0.6.0's repair (numbered backup, REINDEX + VACUUM). See
/// `AppState::repair_project_file`. Refused while the file is the open
/// project.
#[tauri::command]
pub fn attempt_repair(state: State<'_, AppState>, path: PathBuf) -> Result<RepairResult, String> {
    attempt_repair_impl(state.inner(), path)
}

/// [`attempt_repair`] without the Tauri state wrapper.
pub fn attempt_repair_impl(state: &AppState, path: PathBuf) -> Result<RepairResult, String> {
    tracing::info!(path = ?path, "IPC: attempt_repair");
    state.repair_project_file(&path)
}

/// List available backup files for a project.
///
/// Scans for `.backup`, `.backup.N` and `.tmp` files alongside the project,
/// and the pre-upgrade original the first version 2 save kept in the app-data
/// backups folder. See `AppState::list_project_backups`.
#[tauri::command]
pub fn list_backups(path: PathBuf) -> Vec<BackupInfo> {
    tracing::info!(path = ?path, "IPC: list_backups");
    AppState::list_project_backups(&path)
}

/// Restore a project from a backup file (the recovery dialog's "Restore from
/// Backup").
///
/// Keeps a copy of the replaced file as `<project>.stalx.pre-restore`, never
/// writes into a file in place, and refuses (changing nothing) while the
/// project, that copy or the backup is open. See
/// `AppState::restore_project_from_backup` (#720).
#[tauri::command]
pub fn restore_from_backup(
    state: State<'_, AppState>,
    project_path: PathBuf,
    backup_path: PathBuf,
) -> Result<String, String> {
    restore_from_backup_impl(state.inner(), project_path, backup_path)
}

/// [`restore_from_backup`] without the Tauri state wrapper.
pub fn restore_from_backup_impl(
    state: &AppState,
    project_path: PathBuf,
    backup_path: PathBuf,
) -> Result<String, String> {
    tracing::info!(project = ?project_path, backup = ?backup_path, "IPC: restore_from_backup");
    state.restore_project_from_backup(&project_path, &backup_path)
}

/// Export whatever data can be salvaged from a damaged project file.
///
/// A version 2 file: the salvaged project as version 2 JSON, the repairs as
/// warnings. A legacy SQLite file: each table read independently, as 0.6.0.
/// Nothing is written. See `AppState::salvage_project_file`.
#[tauri::command]
pub fn export_salvageable_data(path: PathBuf) -> Result<SalvagedData, String> {
    tracing::info!(path = ?path, "IPC: export_salvageable_data");
    AppState::salvage_project_file(&path)
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod tests {
    use super::{
        attempt_repair_impl, export_salvageable_data, list_backups, restore_from_backup_impl,
    };
    use crate::core::AppState;
    use crate::persistence::stalx_v2::{self, Extras, Mode};
    use crate::persistence::SCHEMA_VERSION;
    use std::path::{Path, PathBuf};

    /// A version 2 file at `dir/name`: an empty project named `label`, its
    /// `"version"` token set to `version` (written as text, so a newer major
    /// version is what a newer Stalagmight would write).
    fn v2_file(dir: &Path, name: &str, label: &str, version: u64) -> PathBuf {
        let path = dir.join(name);
        let project = crate::models::survey::Project::new(label, None);
        let bytes = stalx_v2::encode(&project, &Extras::default(), Mode::Normal).unwrap();
        let text = String::from_utf8(bytes).unwrap().replacen(
            "\"version\":2,",
            &format!("\"version\":{version},"),
            1,
        );
        std::fs::write(&path, text).unwrap();
        path
    }

    /// A version 2 file whose survey breaks an invariant: one line and no
    /// anchor for its piece (the placement member removed).
    fn invalid_v2_file(dir: &Path, name: &str) -> PathBuf {
        use crate::models::survey::{LineMeta, StartAt};
        let mut project = crate::models::survey::Project::new("Damaged", None);
        let line = project
            .add_line(
                LineMeta::default(),
                StartAt::New,
                vec![crate::models::ShotData {
                    length: 5.0,
                    ..Default::default()
                }],
            )
            .unwrap();
        let start = project.line(line).unwrap().start;
        project.set_placement(start, 0.0, 0.0).unwrap();
        let text = String::from_utf8(
            stalx_v2::encode(&project, &Extras::default(), Mode::Normal).unwrap(),
        )
        .unwrap();
        let from = text.find("\"placements\":[").unwrap();
        let to = from + text[from..].find("],\n").unwrap() + 3;
        let path = dir.join(name);
        std::fs::write(&path, format!("{}{}", &text[..from], &text[to..])).unwrap();
        path
    }

    /// A saved synthetic project at `dir/name`, stamped `version`. The stamp is
    /// written straight into the SQLite header's user-version field (bytes
    /// 60..64, big-endian), which is what `PRAGMA user_version` sets.
    fn project_file(dir: &Path, name: &str, version: i32) -> PathBuf {
        let path = dir.join(name);
        let mut bytes = stalagmight_legacy::write_v18(
            &stalagmight_legacy::LegacyProject::new(name.to_string(), None),
            &stalagmight_legacy::LegacyExtras::default(),
        )
        .unwrap();
        bytes
            .get_mut(60..64)
            .unwrap()
            .copy_from_slice(&version.to_be_bytes());
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// The command as the dialog calls it, on a fresh session with nothing open.
    fn restore_from_backup(project: PathBuf, backup: PathBuf) -> Result<String, String> {
        crate::test_helpers::with_isolated_config(|| {
            restore_from_backup_impl(&AppState::new().unwrap(), project, backup)
        })
    }

    /// [#649] Restoring an older backup over a project a newer Stalagmight saved
    /// would downgrade it; the restore refuses before it copies anything.
    #[test]
    fn a_restore_never_puts_a_backup_over_a_newer_project() {
        let dir = tempfile::TempDir::new().unwrap();
        let project = project_file(dir.path(), "cave.stalx", SCHEMA_VERSION + 1);
        let backup = project_file(dir.path(), "cave.stalx.backup", SCHEMA_VERSION);
        let before = std::fs::read(&project).unwrap();

        let err = restore_from_backup(project.clone(), backup).unwrap_err();

        assert!(err.contains("newer version of Stalagmight"), "{err}");
        assert_eq!(std::fs::read(&project).unwrap(), before);
        assert!(
            !project.with_extension("stalx.pre-restore").exists(),
            "refused before the safety copy"
        );
    }

    /// [#649] The roles swapped: a backup a newer Stalagmight saved is not put
    /// over a project this build can open, and the refusal names the backup.
    #[test]
    fn a_restore_never_puts_a_newer_backup_over_a_project() {
        let dir = tempfile::TempDir::new().unwrap();
        let project = project_file(dir.path(), "cave.stalx", SCHEMA_VERSION);
        let backup = project_file(dir.path(), "cave.stalx.backup", SCHEMA_VERSION + 1);
        let before = std::fs::read(&project).unwrap();

        let err = restore_from_backup(project.clone(), backup).unwrap_err();

        assert_eq!(
            err,
            format!(
                "Not restored: \"cave.stalx.backup\" was saved by a newer version of \
                 Stalagmight (file format {}; this version reads up to {SCHEMA_VERSION}). \
                 Update Stalagmight to open it.",
                SCHEMA_VERSION + 1
            )
        );
        assert_eq!(std::fs::read(&project).unwrap(), before);
        assert!(
            !project.with_extension("stalx.pre-restore").exists(),
            "refused before the safety copy"
        );
    }

    /// The restore is still for damaged files: one that is not even a database
    /// is restored over as before.
    #[test]
    fn a_restore_over_a_damaged_project_still_runs() {
        let dir = tempfile::TempDir::new().unwrap();
        let project = dir.path().join("cave.stalx");
        std::fs::write(&project, b"damaged beyond reading").unwrap();
        let backup = project_file(dir.path(), "cave.stalx.backup", SCHEMA_VERSION);

        restore_from_backup(project.clone(), backup.clone()).unwrap();

        assert_eq!(
            std::fs::read(&project).unwrap(),
            std::fs::read(&backup).unwrap()
        );
    }

    /// [#649] On version 2 files: a project a newer Stalagmight saved (a
    /// higher major version) is not restored over, and a newer backup is not
    /// put over a project.
    #[test]
    fn a_restore_never_crosses_a_newer_version_2_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let newer_project = v2_file(dir.path(), "cave.stalx", "Newer", 3);
        let backup = v2_file(dir.path(), "cave.stalx.backup", "Older", 2);
        let before = std::fs::read(&newer_project).unwrap();
        let err = restore_from_backup(newer_project.clone(), backup).unwrap_err();
        assert_eq!(
            err,
            "Not restored: \"cave.stalx\" was saved by a newer version of Stalagmight (file \
             format 3; this version reads up to 2). Update Stalagmight to open it."
        );
        assert_eq!(std::fs::read(&newer_project).unwrap(), before);

        let dir = tempfile::TempDir::new().unwrap();
        let project = v2_file(dir.path(), "cave.stalx", "Current", 2);
        let newer_backup = v2_file(dir.path(), "cave.stalx.backup", "Newer", 3);
        let before = std::fs::read(&project).unwrap();
        let err = restore_from_backup(project.clone(), newer_backup).unwrap_err();
        assert_eq!(
            err,
            "Not restored: \"cave.stalx.backup\" was saved by a newer version of Stalagmight \
             (file format 3; this version reads up to 2). Update Stalagmight to open it."
        );
        assert_eq!(std::fs::read(&project).unwrap(), before);
        assert!(!project.with_extension("stalx.pre-restore").exists());
    }

    /// A damaged version 2 project (one the load refuses) is restored over,
    /// and the damaged file is kept beside it.
    #[test]
    fn a_restore_over_a_damaged_version_2_project_runs() {
        let dir = tempfile::TempDir::new().unwrap();
        let project = invalid_v2_file(dir.path(), "cave.stalx");
        let damaged = std::fs::read(&project).unwrap();
        let backup = v2_file(dir.path(), "cave.stalx.backup", "Older", 2);

        restore_from_backup(project.clone(), backup.clone()).unwrap();

        assert_eq!(
            std::fs::read(&project).unwrap(),
            std::fs::read(&backup).unwrap()
        );
        assert_eq!(
            std::fs::read(project.with_extension("stalx.pre-restore")).unwrap(),
            damaged
        );
    }

    /// The backups the dialog offers: version 2 and SQLite files beside the
    /// project, a complete `.tmp`, never a torn one, nor a sidecar.
    #[test]
    fn list_backups_offers_both_formats_and_skips_a_torn_temp() {
        let dir = tempfile::TempDir::new().unwrap();
        let project = v2_file(dir.path(), "cave.stalx", "Current", 2);
        v2_file(dir.path(), "cave.stalx.backup", "Older", 2);
        project_file(dir.path(), "cave.stalx.backup.1", SCHEMA_VERSION);
        let complete_tmp = v2_file(dir.path(), "cave.stalx.tmp", "Saved", 2);
        let torn = std::fs::read(&complete_tmp).unwrap();
        std::fs::write(
            dir.path().join("cave.stalx.backup.2"),
            &torn[..torn.len() / 2],
        )
        .unwrap();
        std::fs::write(dir.path().join("cave.stalx.backup-journal"), [0u8; 512]).unwrap();

        let offered: Vec<String> =
            crate::test_helpers::with_isolated_config(|| list_backups(project))
                .into_iter()
                .map(|b| {
                    Path::new(&b.path)
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
        let mut sorted = offered;
        sorted.sort();
        assert_eq!(
            sorted,
            ["cave.stalx.backup", "cave.stalx.backup.1", "cave.stalx.tmp"]
        );
    }

    /// Attempt Repair on a version 2 file the load refuses: the damaged file
    /// is kept as `.backup`, the repaired one takes its place and opens.
    #[test]
    fn attempt_repair_salvages_a_version_2_file_in_place_and_keeps_the_original() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = invalid_v2_file(dir.path(), "cave.stalx");
        let damaged = std::fs::read(&path).unwrap();

        let result = crate::test_helpers::with_isolated_config(|| {
            attempt_repair_impl(&AppState::new().unwrap(), path.clone())
        })
        .unwrap();

        assert!(result.success, "{}", result.message);
        assert!(
            result.message.contains("placed the piece of station"),
            "{}",
            result.message
        );
        assert_eq!(
            Path::new(&result.backup_path),
            dir.path().join("cave.stalx.backup")
        );
        assert_eq!(std::fs::read(&result.backup_path).unwrap(), damaged);
        let repaired = std::fs::read(&path).unwrap();
        assert!(stalx_v2::decode(&repaired, &stalx_v2::DecodeOptions::default()).is_ok());
    }

    /// Export Salvageable Data on a damaged version 2 file: the repaired
    /// project as version 2 JSON, the repairs as warnings, nothing written.
    #[test]
    fn export_salvageable_data_returns_the_repaired_project_as_version_2_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = invalid_v2_file(dir.path(), "cave.stalx");
        let before = std::fs::read(&path).unwrap();

        let salvaged = export_salvageable_data(path.clone()).unwrap();

        assert_eq!(salvaged.project_name.as_deref(), Some("Damaged"));
        assert_eq!((salvaged.line_count, salvaged.shot_count), (1, 1));
        let full = salvaged.full_project_json.unwrap();
        assert!(stalx_v2::decode(full.as_bytes(), &stalx_v2::DecodeOptions::default()).is_ok());
        assert!(
            salvaged.errors.iter().any(|e| e.starts_with("Repaired: ")),
            "{:?}",
            salvaged.errors
        );
        assert_eq!(std::fs::read(&path).unwrap(), before, "nothing written");
    }
}
