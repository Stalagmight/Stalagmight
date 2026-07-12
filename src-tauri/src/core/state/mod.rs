mod app_state;
mod collab;
mod command_comment;
mod command_delete_split;
mod command_import;
mod command_marker;
mod command_move;
mod command_query;
mod command_section;
mod command_shot;
mod command_split_shot;
mod command_station;
mod commands;
mod config;
mod display_settings;
mod dive_plan;
mod import_tracking;
mod project_create;
mod project_io;
mod project_load;
mod project_merge;
mod raw_data;
mod recent_projects;
mod speleodb;
mod speleodb_admin;
mod undo;
mod undo_capture;
mod undo_restore;
mod warnings;

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
mod tests_support;

pub use app_state::AppState;
#[cfg(test)]
pub(crate) use collab::apply_changeset;
