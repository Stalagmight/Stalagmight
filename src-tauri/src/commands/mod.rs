pub mod cache;
pub mod config;
pub mod debug;
pub mod dive_plan;
pub mod event_sink;
pub mod export;
pub mod gas;
pub mod history;
pub mod import;
mod import_runner;
pub mod internal_error;
pub mod launch;
pub mod logging;
pub mod memory;
pub(crate) mod pool;
pub mod project;
pub mod raw_data;
pub mod recovery;
pub mod save_target;
pub mod section;
pub mod shot;
pub mod speleodb;
pub mod station;
pub mod survey_geometry;
pub mod telemetry;
pub mod validation;
pub(crate) mod write_path;

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::expect_used)]
mod app_contract_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr,
    clippy::float_cmp
)]
mod command_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              receipt is the evidence a run captures"
)]
mod compass_import_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              answers are the evidence a run captures"
)]
mod expected_project_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point (moved out of \
              command_tests.rs unchanged)"
)]
mod export_command_tests;
#[cfg(test)]
#[expect(
    clippy::print_stdout,
    reason = "test module: the printed values are the evidence a run captures"
)]
mod export_pairing_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod geometry_query_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod import_cancel_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod import_command_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod import_hash_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod import_offthread_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod import_receipt_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod load_path_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod recovery_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "timing probe: panics on a failed assumption are the point, and the printed \
              values are the measurement"
)]
mod refresh_probe_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, one body panics ON \
              PURPOSE to crash a mutation on the pool, and the printed values are the \
              evidence a run captures"
)]
mod remote_link_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod save_target_tests;
#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod telemetry_coverage_tests;
#[cfg(test)]
mod write_path_fuzz_model_tests;
#[cfg(test)]
mod write_path_fuzz_tests;
#[cfg(test)]
#[expect(
    clippy::print_stdout,
    reason = "test module: the printed verdicts are the evidence a run captures"
)]
mod write_path_match_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod write_path_tests;
