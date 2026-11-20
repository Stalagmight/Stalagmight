//! Cross-platform application directory registry.
//!
//! On desktop, the `directories` crate works fine. On Android it returns `None`
//! for every path, crashing the app at startup. The app layer resolves paths
//! through Tauri's `path` API (which delegates to platform-native APIs - see
//! `commands::event_sink::init_app_dirs`) and stores them here in a `OnceLock`
//! so they're available for the lifetime of the process.
//!
//! Call [`init_paths`] once during app setup before any other code that needs
//! paths. The getter functions return `None` if called before [`init_paths`].

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Resolved application directories.
#[derive(Debug)]
struct Dirs {
    data_local: PathBuf,
    config: PathBuf,
    cache: PathBuf,
    document: Option<PathBuf>,
}

static DIRS: OnceLock<Dirs> = OnceLock::new();

/// Populate application directories from already-resolved paths.
///
/// Must be called once during app setup, before `AppState::new()` or any
/// other code that reads paths. Subsequent calls are no-ops. The app layer
/// owns the resolution (and its fallbacks): `data_local`/`config`/`cache`
/// arrive with the `"."` fallback already applied, while `document` stays
/// `None` when unresolved so callers fall through to their own resolution.
pub fn init_paths(data_local: PathBuf, config: PathBuf, cache: PathBuf, document: Option<PathBuf>) {
    DIRS.get_or_init(|| Dirs {
        data_local,
        config,
        cache,
        document,
    });
}

/// Local data directory (logs, telemetry buffer).
pub fn data_local_dir() -> Option<&'static Path> {
    DIRS.get().map(|d| d.data_local.as_path())
}

/// Configuration directory.
pub fn config_dir() -> Option<&'static Path> {
    DIRS.get().map(|d| d.config.as_path())
}

/// Cache directory (tile cache).
pub fn cache_dir() -> Option<&'static Path> {
    DIRS.get().map(|d| d.cache.as_path())
}

/// User documents directory (default project location).
///
/// `None` if not yet initialized, or if the platform could not resolve a
/// documents directory - callers should fall back rather than use a relative path.
pub fn document_dir() -> Option<&'static Path> {
    DIRS.get().and_then(|d| d.document.as_deref())
}
