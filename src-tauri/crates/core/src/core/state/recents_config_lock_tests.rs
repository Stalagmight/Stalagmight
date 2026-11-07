//! [#576 RV4-R-M2] A settings save cannot undo a recent-projects write.
//!
//! A partial settings save (`modify_config`, the path `patch_config` takes)
//! clones the live config under `config_write_lock`, normalizes and saves it,
//! then writes the clone back. The recent-projects writers (open, save, Remove
//! from recent, Delete project) wrote `config.general.recent_projects` under the
//! `config` mutex only, so a recents write landing inside that window was
//! reverted - in the live config AND in `config.toml` - while the in-memory
//! recents list kept the change. They now take the config write lock too.
//!
//! The settings save is held open (a sleep inside its mutate closure) while the
//! removal runs on another thread; on the fixed code the removal waits for it,
//! so the test cannot pass by timing. All data is synthetic, in temp dirs.

use std::time::Duration;

use tempfile::TempDir;

use super::AppState;
use crate::models::Project;
use crate::persistence::Database;
use crate::test_helpers::ConfigHandoff;

#[test]
fn a_settings_save_does_not_undo_a_concurrent_remove_from_recent() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("CaveR.stalx");
    let mut db = Database::open(&path).unwrap();
    db.save_project(&Project::new("CaveR".to_string(), None))
        .unwrap();
    drop(db);
    let state = AppState::new().unwrap();
    assert!(state.open_project(path.clone()).success);
    assert_eq!(
        state.get_config().general.recent_projects,
        vec![path.clone()]
    );

    let handoff = ConfigHandoff::capture();
    std::thread::scope(|scope| {
        let saved = state.modify_config(|config| {
            // The removal starts while this settings save holds its clone.
            scope.spawn(|| {
                let _config = handoff.adopt();
                state.remove_from_recent_projects(path.clone())
            });
            std::thread::sleep(Duration::from_millis(300));
            config.general.auto_save = !config.general.auto_save;
            Ok(())
        });
        assert!(saved.is_ok(), "{saved:?}");
    });

    let live = state.get_config().general.recent_projects;
    let on_disk = crate::config::Config::load()
        .unwrap()
        .general
        .recent_projects;
    let in_memory = state.recent_projects.lock().len();
    println!(
        "after Remove from recent raced a settings save: config recents live={live:?} \
         on disk={on_disk:?}; in-memory recents list len={in_memory}"
    );
    assert!(
        live.is_empty(),
        "the live config got the removed entry back: {live:?}"
    );
    assert!(
        on_disk.is_empty(),
        "config.toml got the removed entry back: {on_disk:?}"
    );
    assert_eq!(in_memory, 0);
}
