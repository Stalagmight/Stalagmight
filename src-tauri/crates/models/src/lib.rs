//! Stalagmight's survey data model.
//!
//! Internal to the Stalagmight app. It is published to crates.io only because
//! `stalx-reader` depends on it, and makes no semver promise: depend on
//! `stalx-reader`, whose API is the supported one.

pub mod angle;
pub mod app_dirs;
pub mod azimuth_reference;
#[cfg(test)]
mod azimuth_reference_tests;
pub mod command_error;
pub mod comment_split;
pub mod config;
pub mod creation_stamp;
pub mod dive_plan;
pub mod geo;
pub mod id_stride;
pub mod import_receipt;
/// [#576 FZ-INT] The deterministic lock-granular scheduler the interleaving
/// fuzzers run under: see the module doc.
#[cfg(any(test, feature = "test-support"))]
pub mod interleave;
pub mod placement_suggest;
pub mod poison_policy;
pub mod process_memory;
pub mod project;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              findings are the evidence a run captures"
)]
mod raw_lock_scan_tests;
pub mod section;
pub mod shot;
/// [#576 A8] The one way a test reads production source: see the module doc.
#[cfg(any(test, feature = "test-support"))]
pub mod source_scan;
pub mod speleo_link;
pub mod station;
/// [#576 E1] The shared survey-data builders (`shot`, `straight_shot`, ...): see
/// the module doc.
#[cfg(any(test, feature = "test-support"))]
pub mod test_fixtures;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
#[cfg(test)]
mod test_support_probe_tests;
pub mod undo_mode;
pub mod uuid_mint;
pub mod warning;

pub use angle::*;
pub use azimuth_reference::*;
pub use command_error::*;
pub use creation_stamp::CreationStamp;
pub use dive_plan::*;
pub use geo::*;
pub use id_stride::*;
pub use import_receipt::*;
pub use placement_suggest::*;
pub use project::*;
pub use section::*;
pub use shot::*;
pub use speleo_link::*;
pub use station::ClosureStrategy;
pub use station::*;
pub use undo_mode::*;
pub use warning::*;
