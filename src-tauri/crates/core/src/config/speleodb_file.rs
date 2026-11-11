//! Disk representation of SpeleoDB configuration, including its private token.
use crate::config::SpeleoDbConfig;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
struct Document {
    #[serde(flatten)]
    settings: SpeleoDbConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token: Option<String>,
}

fn file_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("speleodb")
        .join("speleodb.conf")
}

/// Only genuine absence permits importing preferences from the old general file.
pub(super) fn load_legacy(
    settings: &mut SpeleoDbConfig,
    config_path: &Path,
    legacy: Option<toml::Value>,
) {
    if !std::fs::symlink_metadata(file_path(config_path))
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    {
        return;
    }
    let Some(mut legacy) = legacy else {
        return;
    };
    // Import preferences only; obsolete password/keychain/token fields are ignored.
    if let Some(table) = legacy.as_table_mut() {
        table.retain(|key, _| {
            matches!(
                key,
                "endpoint" | "email" | "insecure_http" | "poll_interval_seconds" | "undo_mode"
            )
        });
    }
    match legacy.try_into::<SpeleoDbConfig>() {
        Ok(migrated) => *settings = migrated,
        Err(_) => {
            settings.preserve_unreadable_file = true;
            tracing::warn!(
                "Malformed legacy SpeleoDB settings; preserving the legacy table for recovery"
            );
        }
    }
}

pub(super) fn load(settings: &mut SpeleoDbConfig, config_path: &Path) -> Result<()> {
    let path = file_path(config_path);
    if let Some(parent) = path.parent() {
        super::atomic_write::sweep_stale_temp_files_named(parent, "speleodb.toml.");
    }
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && std::fs::symlink_metadata(&path)
                    .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(())
        }
        Err(error) => return Err(error).context("Failed to read SpeleoDB configuration"),
    };
    // Parser diagnostics can contain the source line, which may contain a token.
    let mut document: Document = toml::from_str(&contents)
        .map_err(|_| anyhow::anyhow!("Failed to parse SpeleoDB configuration"))?;
    document.settings.token = crate::speleodb::auth::AuthToken(document.token);
    *settings = document.settings;
    Ok(())
}

/// Keep the independent general preferences usable if this document is damaged.
pub(super) fn load_or_preserve(settings: &mut SpeleoDbConfig, config_path: &Path) {
    if let Err(error) = load(settings, config_path) {
        settings.preserve_unreadable_file = true;
        tracing::warn!(error = %error, "SpeleoDB configuration could not be loaded; preserving its file and using signed-out defaults");
    }
}

/// A failed general-config write must not leave a different durable account.
/// Preserve old bytes/absence, and report a failed rollback explicitly.
pub(super) fn save_with_general(
    settings: &SpeleoDbConfig,
    config_path: &Path,
    general: &str,
) -> Result<()> {
    if settings.preserve_unreadable_file {
        // Preserve recovery information in the old general document as well.
        let mut document: toml::Value = toml::from_str(general)?;
        let previous = match std::fs::read_to_string(config_path) {
            Ok(old) => Some(old),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => {
                anyhow::bail!("Could not read general configuration to preserve legacy settings")
            }
        };
        if let Some(old) = previous {
            let old: toml::Value = toml::from_str(&old).map_err(|_| {
                anyhow::anyhow!("Could not parse general configuration to preserve legacy settings")
            })?;
            if let Some(legacy) = old.get("speleodb") {
                if let Some(table) = document.as_table_mut() {
                    table.insert("speleodb".into(), legacy.clone());
                }
            }
        }
        return super::atomic_write::write_atomically(
            config_path,
            &toml::to_string_pretty(&document)?,
        );
    }
    let path = file_path(config_path);
    let previous = match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("Failed to read prior SpeleoDB configuration"),
    };
    save(settings, config_path)?;
    if let Err(error) = super::atomic_write::write_atomically(config_path, general) {
        let rollback = match previous {
            Some(bytes) => super::atomic_write::write_private_bytes_atomically(&path, &bytes),
            None => {
                std::fs::remove_file(&path).context("Failed to remove new SpeleoDB configuration")
            }
        };
        if rollback.is_err() {
            anyhow::bail!("Failed to save general configuration and to restore prior SpeleoDB configuration; reload settings before continuing");
        }
        return Err(error);
    }
    Ok(())
}

pub(super) fn save(settings: &SpeleoDbConfig, config_path: &Path) -> Result<()> {
    let path = file_path(config_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .context("Failed to create SpeleoDB configuration directory")?;
    }
    let document = Document {
        settings: settings.clone(),
        token: settings.token.0.clone(),
    };
    let contents =
        toml::to_string_pretty(&document).context("Failed to serialize SpeleoDB configuration")?;
    super::atomic_write::write_private_atomically(&path, &contents)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "synthetic config test setup")]
mod tests {
    use super::*;
    use crate::config::Config;

    fn signed_in_config() -> Config {
        let mut config = Config::default();
        config.speleodb.endpoint = Some("https://sdb.example.com".to_owned());
        config.speleodb.email = Some("diver@example.com".to_owned());
        config.speleodb.token.0 = Some("synthetic-test-token".to_owned());
        config
    }

    fn general_with_legacy(legacy: &str) -> String {
        let mut general = toml::Value::try_from(Config::default()).unwrap();
        general.as_table_mut().unwrap().remove("speleodb");
        format!("{}\n{legacy}", toml::to_string_pretty(&general).unwrap())
    }

    #[test]
    fn legacy_preferences_migrate_only_when_private_file_is_absent() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let legacy = "[speleodb]\nendpoint = 'https://legacy.example.com'\nemail = 'legacy@example.com'\ninsecure_http = true\nallow_force_edit = true\npoll_interval_seconds = 2\nundo_mode = 'disabled'\npassword = 'obsolete'\n";
        std::fs::write(&path, general_with_legacy(legacy)).unwrap();
        let loaded = Config::load_from_path(&path).unwrap();
        assert_eq!(
            loaded.speleodb.endpoint.as_deref(),
            Some("https://legacy.example.com")
        );
        assert_eq!(loaded.speleodb.email.as_deref(), Some("legacy@example.com"));
        assert!(loaded.speleodb.insecure_http);
        assert!(!toml::to_string(&loaded.speleodb)
            .unwrap()
            .contains("allow_force_edit"));
        assert_eq!(loaded.speleodb.undo_mode, crate::models::UndoMode::Disabled);
        assert!(loaded.speleodb.token.0.is_none());
        loaded.save_to_path(&path).unwrap();
        assert!(!std::fs::read_to_string(&path)
            .unwrap()
            .contains("[speleodb]"));
        std::fs::write(&path, general_with_legacy(legacy)).unwrap();
        save(&signed_in_config().speleodb, &path).unwrap();
        assert_eq!(
            Config::load_from_path(&path).unwrap().speleodb.email,
            signed_in_config().speleodb.email
        );
    }

    #[test]
    fn malformed_legacy_survives_unrelated_save() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(&path, general_with_legacy("[speleodb]\nundo_mode = 12\n")).unwrap();
        let loaded = Config::load_from_path(&path).unwrap();
        assert!(loaded.speleodb.preserve_unreadable_file);
        loaded.save_to_path(&path).unwrap();
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("undo_mode = 12"));
        assert!(!file_path(&path).exists());
    }

    #[test]
    fn all_speleodb_settings_and_token_round_trip_in_one_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let config = signed_in_config();
        config.save_to_path(&path).unwrap();
        let contents = std::fs::read_to_string(file_path(&path)).unwrap();
        let document: toml::Value = toml::from_str(&contents).unwrap();
        assert_eq!(
            document["endpoint"].as_str(),
            Some("https://sdb.example.com")
        );
        assert_eq!(document["token"].as_str(), Some("synthetic-test-token"));
        let general: toml::Value =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(general.get("speleodb").is_none());
        let loaded = Config::load_from_path(&path).unwrap();
        assert_eq!(loaded.speleodb.token.0, config.speleodb.token.0);
        assert_eq!(loaded.speleodb.email, config.speleodb.email);
        assert!(!toml::to_string(&loaded.speleodb)
            .unwrap()
            .contains("allow_force_edit"));
    }

    #[test]
    fn obsolete_preference_in_private_file_is_ignored_and_not_resaved() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let private = file_path(&path);
        std::fs::create_dir_all(private.parent().unwrap()).unwrap();
        std::fs::write(&private, "allow_force_edit = true\n").unwrap();
        let mut settings = SpeleoDbConfig::default();
        load(&mut settings, &path).unwrap();
        save(&settings, &path).unwrap();
        assert!(!std::fs::read_to_string(private)
            .unwrap()
            .contains("allow_force_edit"));
    }

    #[test]
    fn token_stays_out_of_ipc_and_debug_output() {
        let config = signed_in_config();
        for output in [
            serde_json::to_string(&config).unwrap(),
            format!("{config:?}"),
        ] {
            assert!(!output.contains("synthetic-test-token"));
        }
    }

    #[test]
    fn standalone_file_loads_and_logout_removes_token() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let config = signed_in_config();
        save(&config.speleodb, &path).unwrap();
        let mut loaded = Config::load_from_path(&path).unwrap();
        assert_eq!(loaded.speleodb.token.0, config.speleodb.token.0);
        loaded.speleodb.token.0 = None;
        loaded.save_to_path(&path).unwrap();
        assert!(!std::fs::read_to_string(file_path(&path))
            .unwrap()
            .contains("synthetic-test-token"));
        assert!(Config::load_from_path(&path)
            .unwrap()
            .speleodb
            .token
            .0
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn dangling_private_file_is_preserved_and_never_falls_back_to_legacy() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(
            &path,
            general_with_legacy("[speleodb]\nemail = 'legacy@example.com'\n"),
        )
        .unwrap();
        let target = file_path(&path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(temp.path().join("missing"), &target).unwrap();
        let loaded = Config::load_from_path(&path).unwrap();
        assert!(loaded.speleodb.email.is_none());
        assert!(loaded.speleodb.preserve_unreadable_file);
        loaded.save_to_path(&path).unwrap();
        assert!(std::fs::symlink_metadata(target).unwrap().is_symlink());
    }

    #[test]
    fn invalid_document_errors_never_include_token_source() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::create_dir_all(file_path(&path).parent().unwrap()).unwrap();
        std::fs::write(file_path(&path), "token = 'synthetic-test-token' broken").unwrap();
        let error = load(&mut SpeleoDbConfig::default(), &path).unwrap_err();
        assert!(!format!("{error:?}").contains("synthetic-test-token"));
    }

    #[cfg(unix)]
    #[test]
    fn token_file_is_private_on_create_and_replace() {
        use std::os::unix::fs::PermissionsExt as _;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let config = signed_in_config();
        save(&config.speleodb, &path).unwrap();
        let token_path = file_path(&path);
        assert_eq!(
            std::fs::metadata(&token_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        save(&config.speleodb, &path).unwrap();
        assert_eq!(
            std::fs::metadata(&token_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    #[test]
    fn damaged_auth_file_preserves_general_preferences_and_survives_unrelated_saves() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let mut config = signed_in_config();
        config.general.undo_history_limit = 432;
        config.save_to_path(&path).unwrap();
        let damaged = b"token = 'synthetic-test-token' broken";
        std::fs::write(file_path(&path), damaged).unwrap();
        let mut loaded = Config::load_from_path(&path).unwrap();
        assert_eq!(loaded.general.undo_history_limit, 432);
        assert!(loaded.speleodb.token.0.is_none());
        loaded.general.undo_history_limit = 433;
        loaded.save_to_path(&path).unwrap();
        assert_eq!(std::fs::read(file_path(&path)).unwrap(), damaged);
        assert_eq!(
            Config::load_from_path(&path)
                .unwrap()
                .general
                .undo_history_limit,
            433
        );
    }

    #[test]
    fn failed_general_save_restores_previous_auth_bytes_or_absence() {
        for previous in [None, Some(b"token = 'synthetic-old-token'\n".as_slice())] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("config.toml");
            std::fs::create_dir(&path).unwrap(); // Atomic replacement must fail.
            if let Some(bytes) = previous {
                std::fs::create_dir_all(file_path(&path).parent().unwrap()).unwrap();
                std::fs::write(file_path(&path), bytes).unwrap();
            }
            assert!(signed_in_config().save_to_path(&path).is_err());
            match previous {
                Some(bytes) => assert_eq!(std::fs::read(file_path(&path)).unwrap(), bytes),
                None => assert!(!file_path(&path).exists()),
            }
            #[cfg(unix)]
            if previous.is_some() {
                use std::os::unix::fs::PermissionsExt as _;
                assert_eq!(
                    std::fs::metadata(file_path(&path))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
            }
        }
    }

    #[test]
    fn unreadable_auth_file_does_not_reset_or_prevent_saving_general_preferences() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let mut config = signed_in_config();
        config.general.undo_history_limit = 432;
        config.save_to_path(&path).unwrap();
        std::fs::remove_file(file_path(&path)).unwrap();
        std::fs::create_dir(file_path(&path)).unwrap();
        let mut loaded = Config::load_from_path(&path).unwrap();
        assert_eq!(loaded.general.undo_history_limit, 432);
        loaded.general.undo_history_limit = 433;
        loaded.save_to_path(&path).unwrap();
        assert!(file_path(&path).is_dir());
    }
}
