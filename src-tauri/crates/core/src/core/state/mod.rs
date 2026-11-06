mod app_state;
mod command_comment;
mod command_delete_split;
mod command_epilogue;
mod command_import;
mod command_move;
mod command_query;
mod command_section;
mod command_shot;
mod command_split_shot;
mod command_station;
mod commands;
mod config;
mod display_settings;
pub mod dive_plan;
mod import_tracking;
mod project_create;
mod project_io;
mod project_load;
mod project_merge;
mod project_slot;
mod raw_data;
mod recent_projects;
mod save_sidecars;
mod session_commit;
#[cfg(any(test, feature = "test-support"))]
pub mod settle_gate;
mod settled_adjusted;
mod speleodb;
mod speleodb_admin;
mod speleodb_adopt;
mod speleodb_cache;
mod speleodb_credentials;
mod speleodb_foreign;
mod speleodb_local_state;
mod speleodb_lock_release;
mod speleodb_logout;
mod speleodb_project;
mod speleodb_pull;
mod speleodb_push;
mod speleodb_revisions;
mod speleodb_session;
mod undo;
mod undo_capture;
mod undo_policy;
mod undo_restore;
mod warnings;
mod workspace_mode;

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
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    clippy::result_large_err,
    reason = "test module: panics on a failed assumption are the point, the printed \
              values are the evidence a run captures, and the closures return the \
              loads' own large LoadError"
)]
mod cancelled_load_lock_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::float_cmp,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, lengths are \
              compared exactly as written, and the printed values are the evidence a \
              run captures"
)]
mod command_ordering_tests;
#[cfg(test)]
mod concurrency_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod config_patch_tests;
#[cfg(test)]
mod crash_property_tests;
#[cfg(test)]
mod determinism_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod display_setting_identity_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              messages are the evidence a run captures"
)]
mod error_marker_tests;
#[cfg(test)]
#[expect(
    clippy::panic,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod exhaustive_undo_arms_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              readings are the evidence a run captures"
)]
mod f32_edit_tests;
#[cfg(test)]
mod format_property_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              runs are the evidence a run captures"
)]
mod forward_epilogue_tests;
#[cfg(test)]
mod fuzz_crossing_tests;
#[cfg(test)]
mod fuzz_rarity_tests;
#[cfg(test)]
mod fuzz_shrink_tests;
#[cfg(test)]
mod fuzz_undo_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, one panics ON \
              PURPOSE to poison a mutex, and the printed answers are the evidence a run \
              captures"
)]
mod geometry_query_tests;
#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::expect_used)]
mod import_pipeline_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::indexing_slicing,
    reason = "test module: panics on a failed assumption are the point, the injected crash \
              panics ON PURPOSE to poison a cell, and the printed reports are the evidence a \
              run captures"
)]
mod interleave_fuzz_tests;
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
    reason = "test module: panics on a failed assumption are the point"
)]
mod issue_517_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point"
)]
mod issue_555_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              counts are the evidence a run captures"
)]
mod lifecycle_lock_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              counts are the evidence a run captures"
)]
mod lifecycle_park_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod load_json_newer_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod load_repairs_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod merge_block_reason_tests;
#[cfg(test)]
mod metamorphic_undo_tests;
#[cfg(test)]
mod mock_narrowing_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod newer_schema_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod north_reference_mode_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod older_schema_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod open_file_flags_settings_tests;
#[cfg(test)]
mod open_undo_bugs_tests;
#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod perf_probe_tests;
#[cfg(test)]
mod persist_roundtrip_fuzz_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, each test panics \
              ON PURPOSE to poison a lock, and the printed answers are the evidence a run \
              captures"
)]
mod poison_policy_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod protected_folder_load_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              counts are the evidence a run captures"
)]
mod push_bookkeeping_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod recents_config_lock_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              positions are the evidence a run captures"
)]
mod reference_change_datum_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod reference_change_geometry_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod rename_during_save_tests;
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
    reason = "test module: panics on a failed assumption are the point"
)]
mod roundtrip_fidelity_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod save_as_unreadable_tests;
#[cfg(test)]
mod save_fault_fuzz_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              counts are the evidence a run captures"
)]
mod save_history_cache_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod save_sidecar_tests;
#[cfg(test)]
mod schema_history_fuzz_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod sdb_adopt_edit_gate_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod sdb_adopt_gate_lift_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod sdb_adopt_guard_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              counts are the evidence a run captures"
)]
mod sdb_adopt_span_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod sdb_delete_release_claim_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod sdb_done_here_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod sdb_endpoint_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::result_large_err,
    reason = "test module: panics on a failed assumption are the point, the printed \
              values are the evidence a run captures, and the closures return the \
              verbs' own large SpeleoErrorDto"
)]
mod sdb_failed_push_save_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod sdb_flush_warnings_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod sdb_fold_refusal_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod sdb_lock_rule_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              counts are the evidence a run captures"
)]
mod sdb_lock_span_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod sdb_newer_pull_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              answers are the evidence a run captures"
)]
mod sdb_publish_gate_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod sdb_publish_refusal_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod sdb_refusal_wording_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod sdb_resume_claim_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod sdb_resume_licence_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod sdb_revision_gate_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod sdb_silent_release_claim_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod sdb_swap_gate_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod section_created_at_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              cells are the evidence a run captures"
)]
mod session_commit_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              timings and counts are the evidence a run captures"
)]
mod settled_lookup_tests;
#[cfg(test)]
#[expect(
    clippy::panic,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod settled_read_panic_tests;
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
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod side_table_write_during_save_tests;
#[cfg(test)]
#[expect(
    clippy::print_stdout,
    reason = "test module: the printed keys are the evidence a run captures"
)]
mod sidecar_key_coverage_tests;
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
#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod undo_classification_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod unlinked_save_as_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod unreadable_link_open_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod unreadable_settings_lift_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod upgrade_copy_refusal_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              outcomes are the evidence a run captures"
)]
mod workspace_mode_tests;
// `any(test, feature = "test-support")`: the root crate's tests reach the
// AppState test setters in here (via the `test-support` feature the root's
// [dev-dependencies] enable); the source-scan tripwires inside stay
// `cfg(test)`-gated.
#[cfg(any(test, feature = "test-support"))]
mod tests_support;

pub use app_state::AppState;
pub use command_import::VirtualShotMode;
pub use display_settings::DisplaySettingTarget;
pub use settled_adjusted::SettledProject;
pub use workspace_mode::WorkspaceMode;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              refusal is the evidence a run captures"
)]
mod adoption_gate_code_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, each test panics \
              ON PURPOSE to poison a lock, and the printed answers are the evidence a run \
              captures"
)]
mod crash_history_save_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              warnings are the evidence a run captures"
)]
mod first_save_side_tables_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              answers are the evidence a run captures"
)]
mod frozen_shot_patch_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, the helper panics \
              ON PURPOSE to poison a lock, and the printed answers are the evidence a run \
              captures"
)]
mod internal_error_status_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              listings are the evidence a run captures"
)]
mod merge_source_read_only_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              answers are the evidence a run captures"
)]
mod newer_download_refusal_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              warnings are the evidence a run captures"
)]
mod publish_save_losses_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              value is the evidence a run captures"
)]
mod pull_view_lock_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              answers are the evidence a run captures"
)]
mod sdb_adopt_e2e_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              answers are the evidence a run captures"
)]
mod sdb_ff_arm_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              answers are the evidence a run captures"
)]
mod settled_reads_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "test module: the SpeleoDB protocol fuzzer (#576 fuzzers) - a broken invariant \
              is a panic by design, and the printed trace is the evidence a run captures"
)]
mod speleodb_fuzz;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              answers are the evidence a run captures"
)]
mod take_remote_success_tests;
