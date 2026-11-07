//! [#576 C4] No failure message carries a routing code marker.
//!
//! The backend used to say WHAT kind of failure it was by prefixing a marker onto
//! the message ("EDIT_DISABLED: WORKSPACE_READ_ONLY: Undo is disabled: ...",
//! "FS_PATH_NOT_FOUND: Cannot save to ...") and the frontend matched the text. The
//! classification is now `CommandError::code` (`CommandResult::code` inside the
//! core, `SpeleoErrorDto::code` for a SpeleoDB verb) and the message is display
//! only, so a marker in it is a leak: it reaches the user verbatim.
//!
//! One case per producer family, each driven through the public entry point a
//! command calls, on synthetic data only (projects built in code, files written
//! into a temp dir).

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::AppState;
use crate::core::undo::UndoData;
use crate::core::{Command, CommandResult};
use crate::models::{CommandErrorCode, Project, SectionData, SpeleoLink};
use crate::persistence::{Database, ResolvedWarning};
use crate::test_fixtures::{bare_shot, state_holding};

/// The marker spellings the backend used to prefix (and the codes that replaced
/// them), as literals: the constants they were spelled with are gone.
const MARKERS: [&str; 19] = [
    "EDIT_DISABLED",
    "WORKSPACE_READ_ONLY",
    "NEWER_FILE_READ_ONLY",
    "NEWER_FILE_EDIT_DISABLED",
    "ADOPTION_IN_PROGRESS",
    "EDIT_REVERIFY_DIVERGED",
    "FS_PERMISSION_DENIED",
    "FS_PATH_NOT_FOUND",
    "FS_PATH_TOO_LONG",
    "IGNORES_UNREADABLE",
    "SPELEODB_LINK_UNREADABLE",
    "UNDO_MARKERS_UNREADABLE",
    "PROJECT_SETTINGS_UNREADABLE",
    "IMPORT_CANCELLED",
    "PRE_RELEASE_FILE",
    "REFUSED_AFTER_INTERNAL_ERROR",
    "NOTHING_TO_UNDO",
    "NOTHING_TO_REDO",
    "HISTORY_STEP_FAILED",
];

/// Every marker `message` carries: a literal above, any `CODE:` prefix of a wire
/// code, or a serialized error's `"code"` field.
fn markers_in(message: &str) -> Vec<String> {
    let mut found: Vec<String> = MARKERS
        .iter()
        .filter(|marker| message.contains(**marker))
        .map(|marker| (*marker).to_owned())
        .collect();
    for code in CommandErrorCode::ALL {
        let tag = format!("{}:", code.as_str());
        if message.contains(&tag) && !found.iter().any(|f| f.contains(code.as_str())) {
            found.push(tag);
        }
    }
    if message.contains("\"code\"") {
        found.push("a serialized \"code\" field".to_owned());
    }
    found
}

/// A refusal as a test reads it, whatever error type its entry point returns.
trait Refusal {
    fn message(&self) -> &str;
    fn code(&self) -> Option<CommandErrorCode>;
}

impl Refusal for CommandResult {
    fn message(&self) -> &str {
        self.error.as_deref().unwrap_or_default()
    }
    fn code(&self) -> Option<CommandErrorCode> {
        self.code
    }
}

impl Refusal for String {
    fn message(&self) -> &str {
        self
    }
    fn code(&self) -> Option<CommandErrorCode> {
        None
    }
}

impl Refusal for crate::core::CommandError {
    fn message(&self) -> &str {
        &self.message
    }
    fn code(&self) -> Option<CommandErrorCode> {
        Some(self.code)
    }
}

impl Refusal for crate::speleodb::SpeleoErrorDto {
    fn message(&self) -> &str {
        &self.message
    }
    fn code(&self) -> Option<CommandErrorCode> {
        Some(self.code)
    }
}

/// The refusals one test checked. Every one is printed (the evidence a run
/// captures) and checked for markers; [`Self::finish`] fails the test naming
/// every message that carried one, so a run shows them all, not just the first.
#[derive(Default)]
struct Checks {
    offenders: RefCell<Vec<String>>,
}

impl Checks {
    fn plain(&self, label: &str, refusal: &dyn Refusal) -> (String, Option<CommandErrorCode>) {
        let message = refusal.message().to_owned();
        let code = refusal.code();
        println!("{label}: code={code:?} message={message:?}");
        assert!(!message.is_empty(), "{label}: the failure has a message");
        let found = markers_in(&message);
        if !found.is_empty() {
            self.offenders
                .borrow_mut()
                .push(format!("{label}: carries {found:?}: {message:?}"));
        }
        (message, code)
    }

    /// `result` must be a failure.
    fn refused(&self, label: &str, result: &CommandResult) -> (String, Option<CommandErrorCode>) {
        assert!(!result.success, "{label}: must be refused");
        self.plain(label, result)
    }

    /// `result` must be an `Err`.
    fn refused_err<T: std::fmt::Debug, E: Refusal>(
        &self,
        label: &str,
        result: &Result<T, E>,
    ) -> (String, Option<CommandErrorCode>) {
        match result {
            Ok(value) => panic!("{label}: must be refused, got Ok({value:?})"),
            Err(e) => self.plain(label, e),
        }
    }

    /// [`Self::refused`], and the code says what kind of failure it is.
    fn coded(&self, label: &str, result: &CommandResult, code: CommandErrorCode) {
        let (_, got) = self.refused(label, result);
        assert_eq!(got, Some(code), "{label}: the failure's code");
    }

    /// [`Self::refused_err`], and the code says what kind of failure it is.
    fn coded_err<T: std::fmt::Debug, E: Refusal>(
        &self,
        label: &str,
        result: &Result<T, E>,
        code: CommandErrorCode,
    ) {
        let (_, got) = self.refused_err(label, result);
        assert_eq!(got, Some(code), "{label}: the failure's code");
    }

    fn finish(self) {
        let offenders = self.offenders.into_inner();
        assert!(
            offenders.is_empty(),
            "{} message(s) carry a code marker:\n{}",
            offenders.len(),
            offenders.join("\n")
        );
    }
}

fn create_section() -> Command {
    Command::CreateSection {
        name: Some("SectionA".to_owned()),
        surveyor: None,
        date: None,
        color: "#123456".to_owned(),
        direction: None,
        shots: vec![bare_shot(5.0)],
    }
}

fn one_section_project() -> Project {
    let mut project = Project::new("CaveA".to_owned(), None);
    project.add_section(SectionData {
        name: Some("SectionA".to_owned()),
        color: "#123456".to_owned(),
        shots: vec![bare_shot(5.0), bare_shot(7.0)],
        ..Default::default()
    });
    project
}

/// A local project open in memory (no file).
fn local_state() -> AppState {
    state_holding(one_section_project())
}

fn link() -> SpeleoLink {
    SpeleoLink {
        endpoint: "http://speleodb.invalid/".to_owned(),
        project_id: "11111111-1111-1111-1111-111111111111".to_owned(),
        project_name: "CaveA".to_owned(),
        base_commit_sha: Some("abc123".to_owned()),
        has_unpushed_edits: false,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: None,
    }
}

/// A saved project at `dir/CaveA.stalx` (linked, with both undo-chain markers,
/// when `linked`) carrying one ignored warning, opened, with the edit gate open
/// and one edit landed.
fn opened_file(dir: &Path, linked: bool) -> (AppState, PathBuf) {
    let path = dir.join("CaveA.stalx");
    let mut db = Database::open(&path).unwrap();
    db.save_project(&one_section_project()).unwrap();
    let stored = db.load_project().unwrap();
    let section = &stored.sections[0];
    db.save_resolved_warning(&ResolvedWarning {
        section_id: section.id,
        shot_id: section.shots[0].id,
        warning_code: "SHORT_SHOT".to_owned(),
        resolved_at: chrono::Utc::now(),
        resolved_comment: None,
        shot_uuid: section.shots[0].uuid.clone(),
    })
    .unwrap();
    if linked {
        db.save_speleo_link(Some(&link())).unwrap();
        db.save_speleo_undo_chain(Some("abc123")).unwrap();
        db.save_speleo_undo_watermark(Some("abc123")).unwrap();
    }
    drop(db);
    let state = AppState::new().unwrap();
    let opened = state.open_project(path.clone());
    assert!(opened.success, "open: {:?}", opened.error);
    state.set_edit_enabled(true);
    assert!(state.execute(create_section()).success, "the edit lands");
    (state, path)
}

/// Break a table of the open file behind the session's back.
fn break_table(path: &Path, sql: &str) {
    rusqlite::Connection::open(path)
        .unwrap()
        .execute_batch(sql)
        .unwrap();
}

/// Poison the undo stack's lock, as a command that panicked holding it does.
fn poison_undo_stack(state: &AppState) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = state.undo_stack.lock();
        std::panic::resume_unwind(Box::new("poison the undo stack for the test"));
    }));
}

// --- The edit gate ---------------------------------------------------------------

/// A remote SpeleoDB project open without the write lock refuses every edit.
#[test]
fn an_edit_of_a_read_only_remote_project() {
    let _cfg = crate::test_helpers::isolated_config();
    let checks = Checks::default();
    let (_temp, state) = super::speleodb::remote_readonly_fixture();
    checks.coded(
        "execute",
        &state.execute(create_section()),
        CommandErrorCode::EditDisabled,
    );
    checks.coded("undo", &state.undo(), CommandErrorCode::EditDisabled);
    checks.coded_err(
        "repair",
        &state.repair_open_project_integrity(),
        CommandErrorCode::EditDisabled,
    );
    let warning = ResolvedWarning {
        section_id: 0,
        shot_id: 0,
        warning_code: "SHORT_SHOT".to_owned(),
        resolved_at: chrono::Utc::now(),
        resolved_comment: None,
        shot_uuid: None,
    };
    checks.coded_err(
        "resolve warning",
        &state.save_resolved_warning_gated(&warning),
        CommandErrorCode::EditDisabled,
    );
    checks.coded_err(
        "unresolve warning",
        &state.delete_resolved_warning_gated(0, 0, "SHORT_SHOT", None),
        CommandErrorCode::EditDisabled,
    );
    checks.finish();
}

/// An Ariane adoption handing the project over to SpeleoDB holds the gate shut.
#[test]
fn an_edit_during_an_adoption_hand_over() {
    let _cfg = crate::test_helpers::isolated_config();
    let checks = Checks::default();
    let state = local_state();
    state.speleodb_shut_gate_for_adoption();
    checks.coded(
        "execute",
        &state.execute(create_section()),
        CommandErrorCode::AdoptionInProgress,
    );
    checks.finish();
}

/// The Plan workspace never edits the survey, nor replaces it through SpeleoDB.
#[test]
fn an_edit_in_the_plan_workspace() {
    let _cfg = crate::test_helpers::isolated_config();
    let checks = Checks::default();
    let state = local_state();
    state.set_workspace_mode(super::workspace_mode::WorkspaceMode::Plan);
    checks.coded(
        "execute",
        &state.execute(create_section()),
        CommandErrorCode::WorkspaceReadOnly,
    );
    checks.coded("redo", &state.redo(), CommandErrorCode::WorkspaceReadOnly);
    checks.coded_err(
        "speleodb_discard_changes",
        &state.speleodb_discard_changes(false, true),
        CommandErrorCode::WorkspaceReadOnly,
    );
    checks.finish();
}

/// A project a newer Stalagmight saved: an edit, a save and a SpeleoDB verb.
#[test]
fn an_edit_and_a_save_of_a_newer_file() {
    let _cfg = crate::test_helpers::isolated_config();
    let checks = Checks::default();
    let temp = TempDir::new().unwrap();
    let (state, _path) = opened_file(temp.path(), false);
    state.set_newer_file_schema(Some(99));
    // The EDIT of a newer file is the edit gate's code; a refused SAVE (and the
    // SpeleoDB verb that would write it) is the save's.
    checks.coded(
        "execute",
        &state.execute(create_section()),
        CommandErrorCode::NewerFileEditDisabled,
    );
    checks.coded(
        "save",
        &state.save_project(None),
        CommandErrorCode::NewerFileReadOnly,
    );
    checks.coded_err(
        "speleodb_force_edit",
        &state.speleodb_force_edit(),
        CommandErrorCode::NewerFileReadOnly,
    );
    checks.finish();
}

/// [#202] The lazy write-lock re-verify finds the lock is someone else's.
#[test]
fn an_edit_whose_lock_re_verify_diverged() {
    let _cfg = crate::test_helpers::isolated_config();
    let checks = Checks::default();
    let (base, _listener, _seen) =
        super::speleodb::recording_stub(super::speleodb::HELD_BY_ANOTHER_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = super::speleodb::editing_fixture(&base);
    checks.coded(
        "execute",
        &state.execute(create_section()),
        CommandErrorCode::EditReverifyDiverged,
    );
    checks.finish();
}

/// [#576 A11] After a crash poisoned the history, undo and redo are refused.
#[test]
fn a_history_step_after_an_internal_error() {
    let _cfg = crate::test_helpers::isolated_config();
    let checks = Checks::default();
    let state = local_state();
    assert!(state.execute(create_section()).success);
    poison_undo_stack(&state);
    checks.coded(
        "undo",
        &state.undo(),
        CommandErrorCode::RefusedAfterInternalError,
    );
    checks.coded(
        "undo_to",
        &state.undo_to(0),
        CommandErrorCode::RefusedAfterInternalError,
    );
    checks.finish();
}

// --- Saves -----------------------------------------------------------------------

/// [#429] A save into a folder that is not there.
#[test]
fn a_save_into_a_missing_folder() {
    let _cfg = crate::test_helpers::isolated_config();
    let checks = Checks::default();
    let temp = TempDir::new().unwrap();
    let state = local_state();
    let missing = temp.path().join("gone").join("CaveA.stalx");
    checks.coded(
        "save",
        &state.save_project(Some(missing)),
        CommandErrorCode::FsPathNotFound,
    );
    checks.finish();
}

/// [#429] A save into a folder the OS refuses.
#[cfg(unix)]
#[test]
fn a_save_into_a_folder_the_os_refuses() {
    use std::os::unix::fs::PermissionsExt;
    let _cfg = crate::test_helpers::isolated_config();
    let checks = Checks::default();
    let temp = TempDir::new().unwrap();
    let locked = temp.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    crate::models::test_support::assert_dir_refuses_new_files(&locked);
    let state = local_state();
    let result = state.save_project(Some(locked.join("CaveA.stalx")));
    let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));
    checks.coded("save", &result, CommandErrorCode::FsPermissionDenied);
    checks.finish();
}

/// [#169] The legacy open of a file in a folder the OS will not traverse.
#[cfg(unix)]
#[test]
fn an_open_in_a_folder_the_os_refuses() {
    use std::os::unix::fs::PermissionsExt;
    let _cfg = crate::test_helpers::isolated_config();
    let checks = Checks::default();
    let temp = TempDir::new().unwrap();
    let locked = temp.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    crate::models::test_support::assert_lookup_refused(&locked.join("CaveA.stalx"));
    let state = AppState::new().unwrap();
    let result = state.open_project(locked.join("CaveA.stalx"));
    let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));
    checks.coded(
        "open_project",
        &result,
        CommandErrorCode::FsPermissionDenied,
    );
    checks.finish();
}

/// [#517] A save that cannot read the ignore list it would rewrite.
#[test]
fn a_save_over_an_unreadable_ignore_list() {
    let _cfg = crate::test_helpers::isolated_config();
    let checks = Checks::default();
    let temp = TempDir::new().unwrap();
    let (state, path) = opened_file(temp.path(), false);
    let table = super::save_sidecars::Sidecar::ResolvedWarnings.tables()[0];
    break_table(
        &path,
        &format!("ALTER TABLE {table} RENAME COLUMN warning_code TO broken_code;"),
    );
    checks.coded(
        "save",
        &state.save_project(None),
        CommandErrorCode::IgnoresUnreadable,
    );
    checks.finish();
}

/// [#576 B35(b)] A save that cannot read the SpeleoDB link it would rewrite.
#[test]
fn a_save_over_an_unreadable_link() {
    let _cfg = crate::test_helpers::isolated_config();
    let checks = Checks::default();
    let temp = TempDir::new().unwrap();
    let (state, path) = opened_file(temp.path(), true);
    break_table(
        &path,
        "ALTER TABLE speleo_link RENAME COLUMN project_name TO project_title;",
    );
    checks.coded(
        "save",
        &state.save_project(None),
        CommandErrorCode::SpeleodbLinkUnreadable,
    );
    checks.finish();
}

/// [#576 A3] A save that cannot read the undo-chain markers it would rewrite.
#[test]
fn a_save_over_unreadable_undo_markers() {
    let _cfg = crate::test_helpers::isolated_config();
    let checks = Checks::default();
    let temp = TempDir::new().unwrap();
    let (state, path) = opened_file(temp.path(), true);
    break_table(
        &path,
        "ALTER TABLE speleo_undo_watermark RENAME COLUMN inherited_through TO broken_value;",
    );
    checks.coded(
        "save",
        &state.save_project(None),
        CommandErrorCode::UndoMarkersUnreadable,
    );
    checks.finish();
}

/// [#576 B15] A save over stored settings the open could not read - and the
/// SpeleoDB pre-push save, which refuses with the save's own refusal.
#[test]
fn a_save_over_unread_project_settings() {
    let _cfg = crate::test_helpers::isolated_config();
    let checks = Checks::default();
    let temp = TempDir::new().unwrap();
    let (state, _path) = opened_file(temp.path(), true);
    state.set_unreadable_project_settings(vec!["validation".to_owned()]);
    checks.coded(
        "save",
        &state.save_project(None),
        CommandErrorCode::ProjectSettingsUnreadable,
    );
    // The pre-push save refuses the SpeleoDB verb with the save's own code.
    checks.coded_err(
        "pre-push save",
        &state.speleodb_flush_before_push(),
        CommandErrorCode::ProjectSettingsUnreadable,
    );
    checks.finish();
}

/// [#576 B1-D1] The legacy open of a file a pre-release build saved.
#[test]
fn an_open_of_a_pre_release_file() {
    let _cfg = crate::test_helpers::isolated_config();
    let checks = Checks::default();
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("CaveA.stalx");
    let mut db = Database::open(&path).unwrap();
    db.save_project(&one_section_project()).unwrap();
    drop(db);
    break_table(
        &path,
        "UPDATE project SET schema_version = 1; PRAGMA user_version = 0;",
    );
    let state = AppState::new().unwrap();
    checks.refused("open_project", &state.open_project(path));
    checks.finish();
}

// --- Undo / redo -----------------------------------------------------------------

/// An empty history: nothing ran.
#[test]
fn an_undo_or_redo_with_nothing_to_step() {
    let _cfg = crate::test_helpers::isolated_config();
    let checks = Checks::default();
    let state = local_state();
    checks.coded("undo", &state.undo(), CommandErrorCode::NothingToUndo);
    checks.coded("redo", &state.redo(), CommandErrorCode::NothingToRedo);
    checks.finish();
}

/// A single undo whose step ran and failed.
#[test]
fn a_single_undo_whose_step_failed() {
    let _cfg = crate::test_helpers::isolated_config();
    let checks = Checks::default();
    let state = local_state();
    state
        .undo_stack
        .lock()
        .push_with_data(Command::CloseProject, UndoData::None);
    let result = state.undo();
    checks.coded("undo", &result, CommandErrorCode::HistoryStepFailed);
    // The step ran: one of one. Its message is the step's own reason, as before.
    assert_eq!(
        result.history_step,
        Some(crate::core::history_step_failure(1, 1)),
        "a single step that ran and failed is step 1 of 1"
    );
    assert_eq!(
        result.error.as_deref(),
        Some("This command cannot be undone")
    );
    let wire = result.into_error();
    assert_eq!(wire.code, CommandErrorCode::HistoryStepFailed);
    assert!(
        wire.payload.history_step.is_some(),
        "the IPC error carries the step"
    );
    checks.finish();
}

/// A multi-step undo that failed part-way: the steps before it landed.
#[test]
fn a_multi_step_undo_that_failed_part_way() {
    let _cfg = crate::test_helpers::isolated_config();
    let checks = Checks::default();
    let state = local_state();
    state
        .undo_stack
        .lock()
        .push_with_data(Command::CloseProject, UndoData::None);
    assert!(state.execute(create_section()).success);
    assert!(state.execute(create_section()).success);
    let result = state.undo_to(2);
    checks.coded("undo_to", &result, CommandErrorCode::HistoryStepFailed);
    assert_eq!(
        result.history_step,
        Some(crate::core::history_step_failure(3, 3)),
        "the jump stopped at its third step of three"
    );
    assert!(
        result
            .error
            .as_deref()
            .is_some_and(|e| e.starts_with("Undo failed at step 3 of 3: ")),
        "the display text says where the jump stopped: {:?}",
        result.error
    );
    let wire = result.into_error();
    assert_eq!(wire.code, CommandErrorCode::HistoryStepFailed);
    assert_eq!(
        wire.payload.history_step,
        Some(crate::core::history_step_failure(3, 3))
    );
    checks.finish();
}
