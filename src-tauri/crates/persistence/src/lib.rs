//! Project persistence (sqlite `.stalx` files, plnx import, undo history
//! limits) plus the tile caches, extracted from the root crate.
//!
//! Internal to the Stalagmight app. It is published to crates.io only because
//! `stalx-reader` depends on it, and makes no semver promise: depend on
//! `stalx-reader`, whose API is the supported one.
//!
//! The root crate re-exports [`persistence`], [`cache`] and [`tilemap_cache`]
//! under their old `crate::…` paths, so dependents keep compiling unchanged.

pub mod cache;
pub mod persistence;
pub mod tilemap_cache;

/// The moved trees reach the shared data model through `crate::models::*`
/// exactly as they did in the root crate; alias the models member under the
/// same name so those paths keep resolving.
pub use stalagmight_models as models;
