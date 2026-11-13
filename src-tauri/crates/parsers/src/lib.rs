//! Survey-file importers (Ariane TML/TMLU, MNemo DMP, Compass, Walls, GPX)
//! extracted from the root crate. The root re-exports [`parsers`] under its old
//! `crate::parsers` path, so dependents keep compiling unchanged.

pub mod parsers;

/// The moved tree reaches the shared data model through `crate::models::*`
/// exactly as it did in the root crate; alias the models member under the same
/// name so those paths keep resolving.
pub use stalagmight_models as models;
