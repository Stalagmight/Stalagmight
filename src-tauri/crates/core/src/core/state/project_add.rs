//! "Add .stalx to current project" (#674; plan 5.8): append another project
//! file's survey into the OPEN project, as one undoable command.
//!
//! A recipe behind `execute`'s edit gates: the source is read OUTSIDE the
//! project lock (`read_source_project`: a version 2 file decoded, a legacy
//! one migrated), then one `run_recorded` step copies it with the model's
//! `append_project` (station ids offset, lines and closures re-minted,
//! Sections and line names made unique, colliding shot uuids re-minted, the
//! copy moved rigidly by `place_by_datum`'s vector) and extends what only an
//! add carries:
//! - the source's import hashes and its own identity
//!   (`stalx-project:<created_at>`), so adding it again, or re-importing the
//!   file it was built from, warns;
//! - the source's ignored warnings, re-keyed onto the copy's (re-minted)
//!   shot uuids, into the open file's in-memory ignores (they ride the next
//!   save; an undo leaves them matching nothing, and the save drops those);
//! - the receipt (counts and warnings) on `CommandResult::import_receipt`,
//!   and the added lines on `CommandResult::line_ids` for the orphan
//!   placement dialog.
//!
//! The undo entry is the generic changeset of the step (what the add brought
//! in, nothing else), with 0.6.0's preview rows: the file, the lines and the
//! shots added.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::commands::Record;
use super::project_io::same_file;
use super::project_source::SourceProject;
use super::AppState;
use crate::core::command::{sanitize_display_text, ImportReceipt, MAX_FILE_NAME_LEN};
use crate::core::{Command, CommandResult};
use crate::models::survey::changeset::{PreviewRow, RecordedPreview};
use crate::models::survey::{AppendSummary, Project};
use crate::models::{
    warning_is_ignorable, FEATURE_ID_STRIDE, MAX_LINES_IN_ID_SPACE, MAX_LINE_ID, MAX_STATION_ID,
};
use crate::persistence::stalx_v2::IgnoredWarning;

/// The import-dedup entry an add records for its source, in
/// `Project::imported_file_hashes` beside the content hashes the importers
/// record there. Prefixed so it can never collide with a hex SHA-256.
///
/// Keyed on the source PROJECT's identity (`created_at`, stable across every
/// edit and save of that project), not on the file's bytes: a `.stalx` is
/// rewritten on every save, so a byte hash would miss exactly the common
/// case of adding the same project again after it was opened and saved. A
/// Save As copy keeps the identity, and holds the same survey, so it warns
/// too. The spelling is 0.6.0's (`to_rfc3339`), so an entry a 0.6.0 add
/// recorded still matches.
const STALX_IDENTITY_PREFIX: &str = "stalx-project:";

fn stalx_identity(project: &Project) -> String {
    format!("{STALX_IDENTITY_PREFIX}{}", project.created_at.to_rfc3339())
}

/// Append each of `items` to `list` unless `list` already holds it.
fn extend_unique(list: &mut Vec<String>, items: &[String]) {
    let mut held: BTreeSet<String> = list.iter().cloned().collect();
    for item in items {
        if held.insert(item.clone()) {
            list.push(item.clone());
        }
    }
}

/// The name a file goes by in the receipt and in every refusal: its own name,
/// no directory, sanitized and clamped (as `Command::add_project_file`).
fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|n| sanitize_display_text(&n.to_string_lossy(), MAX_FILE_NAME_LEN))
        .unwrap_or_default()
}

/// What the copy did, for the receipt and the history entry.
struct AddReport {
    summary: AppendSummary,
    /// Why the source is a survey this project already holds, if it is.
    duplicate: Option<&'static str>,
    shot_count: usize,
}

/// The copy itself and the dedup state it carries; on `Err` the caller
/// (`run_recorded`) restores the project.
fn append_into(project: &mut Project, source: &SourceProject) -> Result<AddReport, String> {
    // The id budgets, checked before anything is copied and worded for the
    // user (the model refuses the same cases with a terser text).
    let incoming = source.project.lines().len();
    let spent = project.next_line_id() as usize;
    if incoming.saturating_add(spent) > MAX_LINES_IN_ID_SPACE {
        return Err(format!(
            "Add refused: the file holds {incoming} lines and this project has \
             already used {spent} line ids, past the limit of \
             {MAX_LINES_IN_ID_SPACE} for one project. Shot ids encode as \
             line_id * {FEATURE_ID_STRIDE} + position in 32 bits, so line ids \
             past {MAX_LINE_ID} cannot hold shots. No changes were made."
        ));
    }
    // E5: the copy keeps its Station IDs, offset past this project's; when
    // they do not fit the add is refused (0.6.0 renumbered every station).
    let base = u64::from(project.counters.next_station_id);
    let needed = u64::from(source.project.counters.next_station_id);
    if base.saturating_add(needed) > u64::from(MAX_STATION_ID) {
        return Err(format!(
            "Add refused: the file's Station IDs run to {needed} and this project has \
             already used {base}, past the limit of {MAX_STATION_ID} Station IDs for \
             one project. No changes were made."
        ));
    }

    let identity = stalx_identity(&source.project);
    let duplicate = if identity == stalx_identity(project) {
        Some("is a copy of the open project")
    } else if project.imported_file_hashes.contains(&identity) {
        Some("was already added to this project")
    } else {
        None
    };
    let summary = project
        .append_project(&source.project)
        .map_err(|e| e.to_string())?
        .value;
    // The source's dedup state, so re-importing the Ariane/DMP file it was
    // built from is still caught here, plus its own identity.
    extend_unique(
        &mut project.imported_file_hashes,
        &source.project.imported_file_hashes,
    );
    extend_unique(
        &mut project.imported_line_hashes,
        &source.project.imported_line_hashes,
    );
    extend_unique(
        &mut project.imported_file_hashes,
        std::slice::from_ref(&identity),
    );
    let shot_count = summary
        .new_lines()
        .iter()
        .filter_map(|id| project.line(*id))
        .map(|line| line.shots.len())
        .sum();
    Ok(AddReport {
        summary,
        duplicate,
        shot_count,
    })
}

/// The source's ignores the copy honours, keyed by the copy's shot uuids:
/// an ignore the source could never honour (error severity) and one whose
/// shot the source does not hold are dropped, one per (shot, code).
fn carried_ignores(source: &SourceProject, summary: &AppendSummary) -> Vec<IgnoredWarning> {
    let renamed: BTreeMap<&str, &str> = summary
        .shot_uuid_renames
        .iter()
        .map(|r| (r.source.as_str(), r.target.as_str()))
        .collect();
    let mut seen = BTreeSet::new();
    let mut carried = Vec::new();
    for ignore in &source.ignores {
        if !warning_is_ignorable(&ignore.code) || source.project.shot_by_uuid(&ignore.shot).is_none()
        {
            continue;
        }
        let shot = renamed
            .get(ignore.shot.as_str())
            .map_or_else(|| ignore.shot.clone(), |target| (*target).to_string());
        if seen.insert((shot.clone(), ignore.code.clone())) {
            carried.push(IgnoredWarning {
                shot,
                ..ignore.clone()
            });
        }
    }
    carried
}

/// 0.6.0's preview rows of an add: the file, the lines and the shots added.
fn add_preview(label: &str, report: &AddReport) -> RecordedPreview {
    let row = |field: &str, new: String| PreviewRow {
        field: field.to_string(),
        old: String::new(),
        new,
    };
    RecordedPreview {
        rows: vec![
            row("File", label.to_string()),
            row("Lines added", report.summary.line_map.len().to_string()),
            row("Shots added", report.shot_count.to_string()),
        ],
        more: 0,
    }
}

fn add_receipt(label: &str, source: &SourceProject, report: &AddReport) -> ImportReceipt {
    let mut warnings = Vec::new();
    if let Some(why) = report.duplicate {
        warnings.push(format!("'{label}' {why}; its lines were added again"));
    }
    if let Some(name) = &source.link_name {
        let name = sanitize_display_text(name, MAX_FILE_NAME_LEN);
        warnings.push(format!(
            "'{label}' is linked to SpeleoDB project '{name}'; its data was copied, \
             that project is unchanged"
        ));
    }
    if let Some((incoming, existing)) = report.summary.elevation_conflict {
        warnings.push(format!(
            "'{label}' has an entrance elevation of {incoming:.2} m, which differs from \
             this project's {existing:.2} m; kept {existing:.2} m"
        ));
    }
    if source.project.lines().is_empty() {
        warnings.push(format!("'{label}' holds no survey lines"));
    }
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let mut receipt = ImportReceipt {
        source_format: "Stalagmight".to_string(),
        file_name: label.to_string(),
        lines_found: count(source.project.lines().len()),
        lines_imported: count(report.summary.line_map.len()),
        shot_count: count(report.shot_count),
        closures_requested: count(source.project.closures.len()),
        closures_created: count(report.summary.closure_map.len()),
        unit_conversion: None,
        warnings,
        info: Vec::new(),
        zero_datum_created: None,
    };
    receipt.cap_warnings();
    receipt
}

impl AppState {
    /// `Command::AddProjectFile` (reached through `execute`, after its edit
    /// gates). Refuses with nothing changed when no project is open, when the
    /// open file is read-only, when `path` is the open project's own file,
    /// when the file cannot be read, when an id space would run out, and when
    /// another project was opened while the file was being read.
    pub(super) fn execute_add_project_file(&self, cmd: &Command) -> CommandResult {
        let Command::AddProjectFile { path, .. } = cmd else {
            return CommandResult::failure("Not an add of a project file".to_string());
        };
        let label = file_label(path);
        if let Err(refused) = self.guard_file_writable() {
            return refused;
        }
        // The project the add goes into, by its slot generation: a project
        // load does not take `command_lock`, so another project can be
        // committed while the source is read, and every commit bumps it.
        let (generation, open_path) = {
            let guard = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(project) = guard.as_ref() else {
                return CommandResult::failure("No project is open to add the file to".to_string());
            };
            (
                self.speleodb_session_generation(),
                project.file_path.clone(),
            )
        };
        if open_path.is_some_and(|open| same_file(&open, path)) {
            return CommandResult::failure(format!(
                "'{label}' is the open project itself, so it cannot be added to it"
            ));
        }

        let source = match self.read_source_project(path, &label, "added") {
            Ok(source) => source,
            Err(e) => return CommandResult::failure(e),
        };
        add_window(self, AddWindow::DuringRead);

        let mut carried: Vec<(String, String)> = Vec::new();
        let outcome = self.run_recorded(
            |project| Record::of(project, cmd),
            |project, record| {
                // The project the add was started against left the slot
                // during the read (a SpeleoDB sign-in or sign-out bumps the
                // generation too; refusing then is conservative).
                if self.speleodb_session_generation() != generation {
                    return Err(format!(
                        "The open project changed while '{label}' was being added, so it \
                         was not added. No changes were made."
                    ));
                }
                let report = append_into(project, &source)?;
                record.refs.lines = report.summary.new_lines();
                record.preview = add_preview(&label, &report);
                // Under the same project guard as the copy, so a save never
                // sees the lines without their ignores (lock order: project,
                // then file).
                let ignores = carried_ignores(&source, &report.summary);
                carried = self.hold_carried_ignores(ignores);
                Ok(report)
            },
        );
        let report = match outcome {
            Ok(report) => report,
            Err(failure) => {
                self.drop_carried_ignores(&carried);
                return failure;
            }
        };
        tracing::info!(
            lines = report.summary.line_map.len(),
            ignores = carried.len(),
            renamed_uuids = report.summary.shot_uuid_renames.len(),
            purpose = "add a project file into the open project",
            "Added a .stalx file's survey to the open project"
        );
        let receipt = add_receipt(&label, &source, &report);
        CommandResult::success()
            .with_line_ids(report.summary.new_lines())
            .with_import_receipt(receipt)
    }

    /// Add `ignores` to the open file's ignores, keeping one already held
    /// for the same shot and code. Returns the (shot, code) keys it added.
    fn hold_carried_ignores(&self, ignores: Vec<IgnoredWarning>) -> Vec<(String, String)> {
        self.with_file(|file| {
            let list = &mut file.extras.ignored_warnings;
            let mut held: BTreeSet<(String, String)> = list
                .iter()
                .map(|i| (i.shot.clone(), i.code.clone()))
                .collect();
            let mut added = Vec::new();
            for ignore in ignores {
                let key = (ignore.shot.clone(), ignore.code.clone());
                if held.insert(key.clone()) {
                    list.push(ignore);
                    added.push(key);
                }
            }
            list.sort();
            added
        })
    }

    /// Take back what [`Self::hold_carried_ignores`] added (the add was
    /// refused after the copy).
    fn drop_carried_ignores(&self, keys: &[(String, String)]) {
        if keys.is_empty() {
            return;
        }
        let keys: BTreeSet<&(String, String)> = keys.iter().collect();
        self.with_file(|file| {
            file.extras
                .ignored_warnings
                .retain(|i| !keys.contains(&(i.shot.clone(), i.code.clone())));
        });
    }
}

/// The window in which no `project` guard is held during an add, where a
/// project load (which does not take `command_lock`) can commit another
/// project into the slot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum AddWindow {
    /// Between the add's first `project` lock and its recorded step, where
    /// the source file is read.
    DuringRead,
}

/// A no-op outside tests.
#[cfg(not(test))]
fn add_window(_state: &AppState, _window: AddWindow) {}

/// Test-only: run the hook a test installed with [`set_add_window_hook`] for
/// `window`, once. Per thread, so it reaches no other test. Defined below every
/// production item on purpose: the slot-commit tripwire in `speleodb_session`
/// reads each file only up to its first test gate.
#[cfg(test)]
fn add_window(state: &AppState, window: AddWindow) {
    let hook = ADD_WINDOW_HOOK.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_some_and(|(at, _)| *at == window) {
            slot.take()
        } else {
            None
        }
    });
    if let Some((_, hook)) = hook {
        hook(state);
    }
}

#[cfg(test)]
type AddWindowHook = (AddWindow, Box<dyn FnOnce(&AppState)>);

#[cfg(test)]
thread_local! {
    static ADD_WINDOW_HOOK: std::cell::RefCell<Option<AddWindowHook>> =
        const { std::cell::RefCell::new(None) };
}

/// Test-only: run `hook` inside `window` of the next add on this thread.
#[cfg(test)]
pub(super) fn set_add_window_hook(window: AddWindow, hook: impl FnOnce(&AppState) + 'static) {
    ADD_WINDOW_HOOK.with(|slot| *slot.borrow_mut() = Some((window, Box::new(hook))));
}
