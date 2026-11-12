mod app_state;
mod command_comment;
mod command_delete_split;
mod command_import;
mod command_move;
mod command_query;
mod command_section;
mod command_shot;
mod command_split_section;
mod command_split_shot;
mod command_station;
mod commands;
mod config;
mod display_settings;
pub mod dive_plan;
mod import_tracking;
mod project_add;
mod project_append;
mod project_create;
mod project_io;
mod project_load;
mod project_merge;
mod project_source;
mod raw_data;
mod recent_projects;
mod speleodb;
mod speleodb_admin;
mod speleodb_cache;
mod speleodb_cache_install;
mod speleodb_cached;
mod speleodb_credentials;
mod speleodb_edit_gate;
mod speleodb_foreign;
mod speleodb_local_state;
mod speleodb_lock_release;
mod speleodb_logout;
mod speleodb_project;
mod speleodb_pull;
mod speleodb_push;
mod speleodb_reload;
mod speleodb_revision_swap;
mod speleodb_revisions;
mod speleodb_session;
mod speleodb_sidecars;
mod speleodb_write_guard;
mod undo;
mod undo_capture;
mod undo_policy;
mod undo_restore;
mod warnings;

#[cfg(test)]
mod adversarial_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point"
)]
mod azimuth_reference_tests;
#[cfg(test)]
mod concurrency_tests;
#[cfg(test)]
mod crash_property_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    reason = "live check: fails loudly, and reports each step it proved"
)]
mod exit_prompt_live_tests;
#[cfg(test)]
mod format_property_tests;
#[cfg(test)]
mod fuzz_rarity_tests;
#[cfg(test)]
mod fuzz_shrink_tests;
#[cfg(test)]
mod fuzz_undo_tests;
#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::expect_used)]
mod import_pipeline_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "test module: panics on a failed assumption are the point"
)]
mod issue_485_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    // The read-only fixture says on stderr when the OS declined to make it
    // read-only - root ignores mode bits, and CI's check job runs as root - so
    // a pass there is not mistaken for a pass of the guard (the #485 pattern).
    clippy::print_stderr,
    reason = "test module: panics on a failed assumption are the point"
)]
mod issue_517_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod issue_555_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod issue_649_tests;
#[cfg(test)]
mod metamorphic_undo_tests;
#[cfg(test)]
mod open_undo_bugs_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point"
)]
mod project_add_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod project_unique_uuid_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point"
)]
mod reverse_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test module: panics on a failed assumption are the point"
)]
mod shot_uuid_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod speleodb_acquire_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod speleodb_cached_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod speleodb_close_release_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod speleodb_edit_gate_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod speleodb_explicit_release_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod speleodb_install_sidecar_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod speleodb_nonblocking_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod speleodb_upload_limit_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point"
)]
mod split_section_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod station_names_import_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    clippy::print_stderr,
    clippy::print_stdout
)]
mod tests;
// `any(test, feature = "test-support")`: the root crate's tests reach the
// AppState test setters in here (via the `test-support` feature the root's
// [dev-dependencies] enable); the source-scan tripwires inside stay
// `cfg(test)`-gated.
#[cfg(any(test, feature = "test-support"))]
mod tests_support;

pub use app_state::AppState;
pub use command_import::VirtualShotMode;
pub use speleodb_cached::{SpeleoCacheOwnership, SpeleoCachedProject, SpeleoPurgeReport};
