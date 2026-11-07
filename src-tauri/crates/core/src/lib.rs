//! The application core - state, the command pattern, undo, project loading,
//! validation - plus the config I/O and the tile-provider registry, extracted
//! from the root crate. The root re-exports [`core`], [`config`] and
//! [`tile_providers`] under their old `crate::*` paths (and, for its tests,
//! [`test_helpers`]), so dependents keep compiling unchanged.

pub mod config;
pub mod core;
/// [#576 E1] The shared `AppState` and survey-data fixtures: see the module doc.
#[cfg(any(test, feature = "test-support"))]
#[expect(
    clippy::expect_used,
    reason = "test fixtures: a failed setup step fails the calling test"
)]
pub mod test_fixtures;
#[cfg(any(test, feature = "test-support"))]
pub mod test_helpers;
pub mod tile_providers;
pub mod tile_stats;

// The moved tree reaches its siblings through the same `crate::*` paths it
// used in the root crate; alias each member under the old name so those paths
// keep resolving.
pub use stalagmight_export::export;
pub use stalagmight_models as models;
pub use stalagmight_models::app_dirs;
pub use stalagmight_parsers::parsers;
pub use stalagmight_persistence::{cache, persistence, tilemap_cache};
pub use stalagmight_speleodb::speleodb;
