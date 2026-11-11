pub mod app_update;
pub mod cache;
pub mod config;
pub mod debug;
pub mod dive_plan;
pub mod event_sink;
pub mod exit_wait;
pub mod export;
pub mod gas;
pub mod history;
pub mod import;
pub mod launch;
pub mod logging;
pub mod memory;
pub mod project;
pub mod raw_data;
pub mod recovery;
pub mod section;
pub mod shot;
pub mod speleodb;
pub mod speleodb_cache;
pub mod station;
pub mod survey_geometry;
pub mod telemetry;
pub mod validation;

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::expect_used)]
mod app_contract_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr,
    unsafe_code,
    clippy::float_cmp
)]
mod command_tests;
#[cfg(all(test, feature = "telemetry"))]
#[expect(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod telemetry_coverage_tests;
