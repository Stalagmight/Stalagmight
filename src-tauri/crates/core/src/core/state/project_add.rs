//! "Add .stalx to current project" (#674): append another project file's
//! survey into the OPEN project, as one undoable command.
//!
//! PARKED by the station-graph switch (C0): `Command::AddProjectFile` refuses
//! with "not yet ported" in the execute pipeline (`commands.rs`). The port
//! (onto `survey::append_project` and the pipeline's `run_recorded`, with the
//! source's ignores carried into the in-memory `ignored_warnings`) is a work
//! package of the C0 breakdown; the 0.6.0 module is
//! `git show ee49b1299:src-tauri/crates/core/src/core/state/project_add.rs`.

/// The execute pipeline's mutation for the Add .stalx family (`commands.rs`
/// dispatches here; plan 9.1). Runs inside `run_recorded`, which snapshots,
/// validates, captures and records around it; `record` is the entry being
/// recorded (worded before the edit), for rows only the result can say.
pub(super) fn apply(
    project: &mut crate::models::survey::Project,
    record: &mut super::commands::Record,
    cmd: &crate::core::Command,
) -> Result<super::commands::Applied, String> {
    let _ = (&mut *project, &mut *record);
    match cmd {
        crate::core::Command::AddProjectFile { .. } => {
            Err(super::commands::not_ported("execute::add_project_file"))
        }
        _ => Err(format!(
            "{} is not a Add .stalx command",
            crate::core::command_kind(cmd)
        )),
    }
}
