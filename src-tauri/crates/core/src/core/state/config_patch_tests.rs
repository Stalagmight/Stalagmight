//! [#576 D27-32(c), A18(c), A18(g), A18(h)] A settings save changes only what it
//! changed, and every config change - a Settings save, a partial save, Reload
//! Config - reaches the running session.
//!
//! D27-32(c): the Settings dialog posted the whole `Config` from the snapshot it
//! took on open, so a value the backend recorded since (the panel layout, a recent
//! project) was reverted by an unrelated save. `patch_config` applies only the
//! changed settings onto the live config.
//!
//! A18: the undo history limit, the tile-cache size and retention, and (on Reload
//! Config) the tile cache itself were applied only at startup. One helper,
//! `sync_config_side_effects`, now runs after every config change.
//!
//! All data is synthetic; the config file is an isolated temp dir.

use super::AppState;
use crate::cache::TileCache;
use crate::config::{Config, ConfigPatch};

fn patch(path: &[&str], value: serde_json::Value) -> ConfigPatch {
    ConfigPatch {
        path: path.iter().map(|s| (*s).to_owned()).collect(),
        value,
    }
}

/// A live tile cache of `max_mb` megabytes, as the startup open would leave it.
fn with_tile_cache(state: &AppState, max_mb: u32) {
    *state.tile_cache.lock() =
        Some(TileCache::in_memory(max_mb, 30).expect("in-memory tile cache"));
}

fn live_cache_mb(state: &AppState) -> Option<u64> {
    state
        .tile_cache
        .lock()
        .as_ref()
        .map(|cache| cache.max_bytes() / (1024 * 1024))
}

#[test]
fn a_settings_patch_keeps_what_the_backend_recorded_since_the_snapshot() {
    let _cfg = crate::test_helpers::isolated_config();
    let state = AppState::new().expect("state");
    let snapshot = state.get_config();
    assert!(
        snapshot.panels.data_table_open,
        "fixture: the table starts open"
    );

    // The user closes the data table: a partial write the dialog never sees.
    state
        .modify_config(|config| {
            config.panels.data_table_open = false;
            Ok(())
        })
        .expect("panel layout saved");

    // The dialog opened before that; the user changes only the theme.
    let theme = if snapshot.appearance.theme == crate::config::ThemeMode::Dark {
        "light"
    } else {
        "dark"
    };
    state
        .patch_config(&[patch(&["appearance", "theme"], serde_json::json!(theme))])
        .expect("settings saved");

    let live = state.get_config();
    let on_disk = Config::load().expect("reload the saved file");
    println!(
        "after the Settings save: theme={:?} data_table_open live={} on disk={}",
        live.appearance.theme, live.panels.data_table_open, on_disk.panels.data_table_open
    );
    assert_eq!(
        serde_json::to_value(live.appearance.theme).expect("theme"),
        serde_json::json!(theme),
        "the changed setting is saved"
    );
    assert!(
        !live.panels.data_table_open && !on_disk.panels.data_table_open,
        "the table the user closed stays closed"
    );
}

#[test]
fn a_patch_naming_no_setting_or_misfitting_its_setting_writes_nothing() {
    let _cfg = crate::test_helpers::isolated_config();
    let state = AppState::new().expect("state");
    let before = serde_json::to_value(state.get_config()).expect("config");

    for (what, bad) in [
        (
            "unknown key",
            patch(&["appearance", "no_such_setting"], serde_json::json!(1)),
        ),
        (
            "unknown table",
            patch(&["no_such_table", "theme"], serde_json::json!(1)),
        ),
        (
            "wrong type",
            patch(
                &["general", "undo_history_limit"],
                serde_json::json!("many"),
            ),
        ),
        ("empty path", patch(&[], serde_json::json!(1))),
    ] {
        let refused = state.patch_config(&[
            patch(&["general", "auto_save"], serde_json::json!(false)),
            bad,
        ]);
        println!(
            "{what}: {:?}",
            refused.as_ref().map(|_| ()).map_err(ToString::to_string)
        );
        assert!(refused.is_err(), "{what} is refused");
        assert_eq!(
            serde_json::to_value(state.get_config()).expect("config"),
            before,
            "{what}: nothing of that save lands, not even its valid part"
        );
    }
}

#[test]
fn a_patch_adds_and_replaces_map_entries() {
    let _cfg = crate::test_helpers::isolated_config();
    let state = AppState::new().expect("state");
    state
        .patch_config(&[patch(
            &["cache", "providers", "osm.tiles"],
            serde_json::json!({ "ttl_days": 3 }),
        )])
        .expect("a map entry keyed with a dot is one path segment");
    let providers = state.get_config().cache.providers;
    assert_eq!(providers.get("osm.tiles").and_then(|p| p.ttl_days), Some(3));

    // A new entry may also arrive as its settings one by one.
    state
        .patch_config(&[patch(
            &["cache", "providers", "local", "ttl_days"],
            serde_json::json!(1),
        )])
        .expect("a new map entry, setting by setting");
    let providers = state.get_config().cache.providers;
    assert_eq!(providers.get("local").and_then(|p| p.ttl_days), Some(1));

    state
        .patch_config(&[patch(&["cache", "providers"], serde_json::json!({}))])
        .expect("replacing the map removes its entries");
    assert!(state.get_config().cache.providers.is_empty());
}

#[test]
fn the_undo_limit_reaches_the_running_session_on_save() {
    // A18(c): it used to apply only at startup or on Reload Config.
    let _cfg = crate::test_helpers::isolated_config();
    let state = AppState::new().expect("state");
    let mut config = state.get_config();
    config.general.undo_history_limit = 20;
    state.update_config(config).expect("settings saved");

    let limit = state.undo_stack.lock().history_limit();
    println!("undo history limit in the session after saving 20: {limit}");
    assert_eq!(limit, 20);

    state
        .patch_config(&[patch(
            &["general", "undo_history_limit"],
            serde_json::json!(35),
        )])
        .expect("settings saved");
    assert_eq!(state.undo_stack.lock().history_limit(), 35);
}

#[test]
fn the_tile_cache_size_reaches_the_live_cache_on_save() {
    // A18(g): size and retention reached the cache only when it was opened.
    let _cfg = crate::test_helpers::isolated_config();
    let state = AppState::new().expect("state");
    with_tile_cache(&state, 500);
    let mut config = state.get_config();
    config.cache.tile_cache_enabled = true;
    config.cache.tile_cache_max_mb = 100;
    state.update_config(config).expect("settings saved");

    let live = live_cache_mb(&state);
    println!("live tile cache cap after saving 100 MB: {live:?} MB");
    assert_eq!(live, Some(100));
}

#[test]
fn reload_config_resyncs_the_tile_cache_and_the_undo_limit() {
    // A18(h): Reload Config replaced the config but never touched the cache.
    let _cfg = crate::test_helpers::isolated_config();
    let state = AppState::new().expect("state");
    with_tile_cache(&state, 500);

    // config.toml edited by hand, then Reload Config.
    let mut on_disk = state.get_config();
    on_disk.cache.tile_cache_enabled = true;
    on_disk.cache.tile_cache_max_mb = 64;
    on_disk.general.undo_history_limit = 25;
    on_disk.save().expect("write config.toml");
    state.reload_config().expect("reload");
    let live = live_cache_mb(&state);
    let limit = state.undo_stack.lock().history_limit();
    println!("after Reload Config with 64 MB: live cap {live:?} MB, undo limit {limit}");
    assert_eq!(live, Some(64));
    assert_eq!(limit, 25);

    on_disk.cache.tile_cache_enabled = false;
    on_disk.save().expect("write config.toml");
    state.reload_config().expect("reload");
    let live = live_cache_mb(&state);
    println!("after Reload Config with the cache off: live cache {live:?}");
    assert_eq!(live, None, "caching off closes the live cache");
}
