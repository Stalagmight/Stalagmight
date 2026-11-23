//! Stalagmight's survey data model.
//!
//! Internal to the Stalagmight app. It is published to crates.io only because
//! `stalx-reader` depends on it, and makes no semver promise: depend on
//! `stalx-reader`, whose API is the supported one.

pub mod adjustment;
pub mod angle;
pub mod app_dirs;
pub mod azimuth_reference;
#[cfg(test)]
mod azimuth_reference_tests;
pub mod comment_split;
pub mod config;
pub mod credit;
pub mod disk_names;
pub mod dive_plan;
pub mod gas;
pub mod geo;
pub mod id_stride;
pub mod identity;
pub mod import_receipt;
pub mod line;
pub mod line_combine;
pub mod line_split;
pub mod newer_schema;
pub mod placement_suggest;
pub mod previews;
pub mod project_info;
pub mod section;
pub mod shot;
pub mod shot_reverse;
#[cfg(test)]
mod shot_reverse_tests;
pub mod shot_uuid;
pub mod speleo_link;
pub mod station;
pub mod station_naming;
#[cfg(test)]
mod station_naming_tests;
pub mod survey;
pub mod undo_mode;
pub mod warning;

pub use adjustment::*;
pub use angle::*;
pub use azimuth_reference::*;
pub use credit::*;
pub use dive_plan::*;
pub use gas::{
    CylRole, Cylinder, Diver, GasReachField, PlanMode, PlanSegment, RouteLeg, RouteProfilePoint,
    TankMaterial, Team, MAX_SEGMENT_SPEED_M_PER_MIN, MIN_SEGMENT_SPEED_M_PER_MIN,
};
pub use geo::*;
pub use id_stride::*;
pub use identity::*;
pub use import_receipt::*;
pub use line::*;
pub use line_combine::*;
pub use line_split::*;
pub use newer_schema::NewerSchema;
pub use placement_suggest::*;
pub use previews::*;
pub use project_info::*;
pub use section::*;
pub use shot::*;
pub use shot_reverse::{reverse_shots, reversed_azimuths, shot_run_displacement};
pub use shot_uuid::*;
pub use speleo_link::*;
pub use station::ClosureStrategy;
pub use station::*;
pub use undo_mode::*;
pub use warning::*;
