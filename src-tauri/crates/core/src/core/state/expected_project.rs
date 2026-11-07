//! [#576 FZ-FZ-INT-4, FZ-FZ-INT-7] The project a save or a survey edit was MADE
//! for, checked under `command_lock` in the same span as the write.
//!
//! A save or an edit IPC used to name no project, and the backend applied it to
//! whichever project was open when it ran. One issued while a project switch was
//! under way therefore landed in the NEW project whenever the backend applied the
//! switch first: an edit of CaveA added its section to the project just opened (the
//! small per-project section and shot ids mostly resolve there), and a Save As of
//! CaveA wrote the NEW project to the path the user chose for CaveA.
//!
//! Every save and mutation IPC now carries the project it was made for - the same
//! identity a display-setting write carries ([`super::DisplaySettingTarget`],
//! `save_display_setting_for`), and the same rule decides it
//! ([`AppState::names_open_project`]): the same `created_at`, and the file the open
//! project was committed from, the one it is at now, or one a Save As moved it off
//! this session. A mismatch is refused with `PROJECT_CHANGED` and nothing is
//! written.
//!
//! The command wrapper runs its body inside [`for_project`]; the body's mutation
//! entry points check it once they hold `command_lock`
//! ([`AppState::refuse_other_project`], called from `guard_metadata_mutation` - the
//! chokepoint every survey mutation calls first under the lock - and from
//! `execute` for a save). Every project-slot commit takes that lock too, so no
//! open, create or close can land between the check and the write. The
//! expectation travels with the body on its pool thread (a thread-local scope), so
//! the many `AppState` mutation methods keep their signatures; a body run outside
//! any scope - the core's own callers and tests - is not checked.
//!
//! [#576 RVF-c-C1] Not only the survey mutations and the save: every IPC that
//! writes the OPEN project carries the identity and runs in [`for_project`] (the
//! app crate's contract scan lists them, `app_contract_tests.rs`). The dive-plan
//! save and delete write its `.plnx` under `command_lock`
//! (`save_dive_plan_for` / `delete_dive_plan_for`). The SpeleoDB verbs that act on
//! its link check at two kinds of point: where they first read the link
//! ([`AppState::speleodb_link_made_for`] - the check and the read in one
//! `command_lock` span, released before any network call, so the request goes to
//! the remote the right project is linked to), and in each `command_lock` span
//! that then writes the open file (the unlink's Save As, the push's local half and
//! its link writes after the upload, Publish's relocation, the metadata name fold,
//! the undo-mode write). A refusal reaches the frontend as `PROJECT_CHANGED`
//! (through `refused_by` for a SpeleoDB verb) with nothing written.
//!
//! [#576 RVF-c-C2] What the identity does NOT tell apart: two sessions of the SAME
//! file. After load A, load B, load A (or a close and reopen of A), a request made
//! in the first A session has the `created_at` and the path of the new one, and the
//! commit resets `opened_from` to that path, so it is ACCEPTED and runs against the
//! freshly loaded A. That is the display-setting rule on purpose (a reload of the
//! same file opens the project the write was made in, #576 RV8b-3); the backend
//! keeps no session counter. The frontend tells sessions apart itself
//! (`projectSessionEpoch`, `lostProjectOf`, and its switch gate), so this check is
//! the backstop for ANOTHER project or another copy, not for an older session.
use std::cell::RefCell;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use ts_rs::TS;

use super::speleodb::{local_dto, refused_by};
use super::AppState;
use crate::core::{CommandError, CommandErrorCode};
use crate::models::SpeleoLink;
use crate::speleodb::SpeleoErrorDto;

/// [#576 FZ-FZ-INT-4, FZ-FZ-INT-7] The open project a save or an edit was made
/// for, as the frontend names it when the user makes it: the shape of
/// [`super::DisplaySettingTarget`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ProjectIdentity {
    /// The project's `created_at`, in the RFC 3339 form the wire carries it in.
    pub created_at: String,
    /// The file the project is open from (`Project::file_path`); `None` for a
    /// project that has never been saved.
    pub file_path: Option<String>,
}

/// What a command body was made for: `None` inside means made with no project open.
#[derive(Debug, Clone)]
struct MadeFor(Option<ProjectIdentity>);

thread_local! {
    /// The expectation of the command body running on this thread; `None` outside
    /// any [`for_project`] scope (unchecked).
    static EXPECTED: RefCell<Option<MadeFor>> = const { RefCell::new(None) };
}

/// Run `body` as a command made for `expected` (`None`: made with no project
/// open). Every save and survey mutation `body` reaches is refused with
/// `PROJECT_CHANGED` unless `expected` names the project open when it holds
/// `command_lock`. The previous scope is restored when `body` returns or unwinds.
pub fn for_project<T>(expected: Option<ProjectIdentity>, body: impl FnOnce() -> T) -> T {
    struct Restore(Option<MadeFor>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            EXPECTED.with(|scope| *scope.borrow_mut() = previous);
        }
    }
    let previous = EXPECTED.with(|scope| scope.replace(Some(MadeFor(expected))));
    let _restore = Restore(previous);
    body()
}

impl AppState {
    /// [#576 RV6b-2, NF-W8FE-3, RV10b-4] Whether `made_for` names the open
    /// project: both none, or the same `created_at` AND the file the open project
    /// was committed from, the one it has now, or one a Save As moved it off since
    /// that commit (`opened_from`). Another copy of the file - a file the project
    /// was never at this session - is another project. The rule a display-setting
    /// write and every save and edit are decided by.
    ///
    /// Takes `project`, then `opened_from`, each briefly: call it under
    /// `command_lock` (so no commit can change the answer before the write it
    /// decides) and never while holding `project`.
    pub(super) fn names_open_project(&self, made_for: Option<(&str, Option<&str>)>) -> bool {
        let open = self
            .project
            .lock()
            .as_ref()
            .map(|p| (p.created_at.utc(), p.file_path.clone()));
        match (made_for, open) {
            (None, None) => true,
            (Some((created_at, file_path)), Some((open_created_at, open_path))) => {
                let same_created_at = DateTime::parse_from_rfc3339(created_at)
                    .is_ok_and(|at| at.with_timezone(&Utc) == open_created_at);
                let made_for_path = file_path.map(PathBuf::from);
                same_created_at
                    && (made_for_path == open_path
                        || self.opened_from.lock().contains(&made_for_path))
            }
            _ => false,
        }
    }

    /// [#576 FZ-FZ-INT-4, FZ-FZ-INT-7] Refuse `what` with `PROJECT_CHANGED` when
    /// the command body running on this thread was made for another project than
    /// the open one ([`for_project`]); `Ok` outside any scope. Call it holding
    /// `command_lock`, before anything is written, and never while holding
    /// `project`.
    pub(super) fn refuse_other_project(&self, what: &str) -> Result<(), CommandError> {
        let Some(MadeFor(expected)) = EXPECTED.with(|scope| scope.borrow().clone()) else {
            return Ok(());
        };
        let made_for = expected
            .as_ref()
            .map(|identity| (identity.created_at.as_str(), identity.file_path.as_deref()));
        if self.names_open_project(made_for) {
            return Ok(());
        }
        tracing::info!(
            what,
            purpose = "keep a save or an edit in the project it was made for",
            "Refused: another project is open now"
        );
        Err(CommandError::new(
            CommandErrorCode::ProjectChanged,
            format!("{what} was not applied: another project is open now."),
        ))
    }

    /// [#576 RVF-c-C1] [`Self::refuse_other_project`] for a SpeleoDB verb: the same
    /// refusal as its error DTO, its `PROJECT_CHANGED` code kept. Same rules: hold
    /// `command_lock`, nothing written yet, never while holding `project`.
    #[expect(
        clippy::result_large_err,
        reason = "the SpeleoErrorDto every SpeleoDB verb returns; boxing the error type is a \
                  repo-wide change outside #576 RVF-c-C1"
    )]
    pub(super) fn speleodb_refuse_other_project(&self, what: &str) -> Result<(), SpeleoErrorDto> {
        self.refuse_other_project(what)
            .map_err(|refusal| local_dto(refused_by(refusal)))
    }

    /// [#576 RVF-c-C1] The open project's validated SpeleoDB link, read in ONE
    /// `command_lock` span with the identity check: a verb made for another project
    /// than the open one is refused with `PROJECT_CHANGED` before it reads the link,
    /// so no request goes to the remote of the project a switch opened. The span ends
    /// here - before the caller's network call - and every later write the verb makes
    /// to the open file checks again in its own span. Takes `command_lock`: call it
    /// holding no lock but `speleodb_lock_ops` (which sits above it).
    #[expect(
        clippy::result_large_err,
        reason = "the SpeleoErrorDto every SpeleoDB verb returns; boxing the error type is a \
                  repo-wide change outside #576 RVF-c-C1"
    )]
    pub(super) fn speleodb_link_made_for(&self, what: &str) -> Result<SpeleoLink, SpeleoErrorDto> {
        let _commands = self.hold_commands();
        self.speleodb_refuse_other_project(what)?;
        self.speleodb_validated_link()
    }
}
