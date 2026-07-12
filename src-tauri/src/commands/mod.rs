pub mod cache;
pub mod collab;
pub mod config;
pub mod debug;
pub mod dive_plan;
pub mod export;
pub mod gas;
pub mod history;
pub mod import;
pub mod logging;
pub mod marker;
pub mod memory;
pub mod project;
pub mod raw_data;
pub mod recovery;
pub mod section;
pub mod shot;
pub mod speleodb;
pub mod station;
pub mod survey_geometry;
pub mod telemetry;
pub mod validation;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr,
    unsafe_code,
    clippy::float_cmp
)]
mod command_tests;
