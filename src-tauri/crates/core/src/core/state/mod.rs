mod app_state;
mod command_cave_section;
mod command_combine_lines;
mod command_comment;
mod command_credit;
mod command_delete_split;
mod command_import;
mod command_line;
mod command_move;
mod command_query;
mod command_shot;
mod command_split_line;
mod command_split_shot;
mod command_station;
mod command_station_name;
mod commands;
mod config;
mod display_settings;
pub mod dive_plan;
mod import_tracking;
mod labels;
mod open_file;
mod project_add;
mod project_append;
mod project_create;
mod project_io;
mod project_load;
mod project_merge;
mod project_restore;
mod project_source;
mod raw_data;
mod recent_projects;
mod speleodb;
mod speleodb_admin;
mod speleodb_cache;
mod speleodb_cache_install;
mod speleodb_cached;
mod speleodb_collaborators;
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
mod undo_policy;
mod warnings;

#[cfg(sg_unported)]
mod adversarial_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point"
)]
mod azimuth_reference_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod batch_label_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point"
)]
mod c0_foundation_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    reason = "test module: panics on a failed assumption are the point"
)]
mod cave_sections_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "on-demand characterisation dump: fails loudly, and says where it wrote"
)]
mod characterization_dump_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point"
)]
mod combine_lines_tests;
#[cfg(sg_unported)]
mod concurrency_tests;
#[cfg(sg_unported)]
mod crash_property_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test module: panics on a failed assumption are the point"
)]
mod credit_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod datum_entrance_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod datum_ui_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    reason = "live check: fails loudly, and reports each step it proved"
)]
mod exit_prompt_live_tests;
#[cfg(sg_unported)]
mod format_property_tests;
#[cfg(sg_unported)]
mod fuzz_rarity_tests;
#[cfg(sg_unported)]
mod fuzz_shrink_tests;
#[cfg(sg_unported)]
mod fuzz_undo_tests;
#[cfg(sg_unported)]
#[expect(clippy::unwrap_used, clippy::expect_used)]
mod import_pipeline_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "test module: panics on a failed assumption are the point"
)]
mod issue_485_tests;
#[cfg(sg_unported)]
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
#[cfg(sg_unported)]
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
#[cfg(sg_unported)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr,
    reason = "test module: panics on a failed assumption are the point, and the delete fuzz \
              beside snaps prints what it covered"
)]
mod issue_737_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod issue_738_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod issue_756_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "on-demand vector dump: fails loudly, and says where it wrote"
)]
mod legacy_disk_vectors_dump_tests;
#[cfg(sg_unported)]
mod metamorphic_undo_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::print_stderr,
    reason = "fuzz test: panics on a failed property are the point, the sweep prints what it \
              covered and the replay prints each step"
)]
mod multi_datum_fuzz_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "fuzz test helpers: a trail that does not parse is a broken replay string"
)]
mod multi_datum_steps_fuzz;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point"
)]
mod multi_datum_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod name_rules_tests;
#[cfg(sg_unported)]
mod open_undo_bugs_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod pre_line_rename_disk_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point"
)]
mod project_add_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod project_restore_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod project_unique_uuid_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point"
)]
mod reverse_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test module: panics on a failed assumption are the point"
)]
mod shot_uuid_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::float_cmp,
    clippy::indexing_slicing,
    reason = "test module: panics on a failed assumption are the point"
)]
mod snapshot_fixes_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::expect_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod speleodb_acquire_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::expect_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod speleodb_cached_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::expect_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod speleodb_close_release_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::expect_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod speleodb_edit_gate_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::expect_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod speleodb_explicit_release_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod speleodb_install_sidecar_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
mod speleodb_nonblocking_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::expect_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod speleodb_upload_limit_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point"
)]
mod split_line_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "test module: panics on a failed assumption are the point"
)]
mod station_identity_tests;
#[cfg(sg_unported)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod station_names_import_tests;
#[cfg(sg_unported)]
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
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point"
)]
mod topology_commands_tests;
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "test kit: panics on a failed assumption are the point"
)]
mod topology_kit;
#[cfg(sg_unported)]
#[expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "test module: panics on a failed assumption are the point"
)]
mod undo_after_reopen_tests;
#[cfg(sg_unported)]
mod undo_disk_names_tests;
#[cfg(sg_unported)]
mod undo_size_gate_tests;
// `any(test, feature = "test-support")`: the root crate's tests reach the
// AppState test setters in here (via the `test-support` feature the root's
// [dev-dependencies] enable); the source-scan tripwires inside stay
// `cfg(test)`-gated.
#[cfg(any(test, feature = "test-support"))]
mod tests_support;

pub use app_state::AppState;
pub use command_import::VirtualShotMode;
pub use open_file::{EXTERNALLY_MODIFIED_CODE, READ_ONLY_FILE_CODE};
pub use project_io::set_generator;
pub use speleodb_cached::{SpeleoCacheOwnership, SpeleoCachedProject, SpeleoPurgeReport};
