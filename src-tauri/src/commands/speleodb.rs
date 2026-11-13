//! IPC command surface for the SpeleoDB integration (#182 epic).
//!
//! Thin wrappers over the `AppState::speleodb_*` orchestration (which lives in
//! `core::state::speleodb`, over the `crate::speleodb` client primitives). The
//! surface covers authentication, native-file transfer, write locks, revision
//! history, metadata and publishing.
//!
//! Errors cross the boundary as the structured [`SpeleoErrorDto`] (#186), which
//! carries the typed [`SpeleoDbAction`] the frontend branches on - not a
//! flattened `String`. The auth token is never returned to the webview (login
//! yields only the account email and instance).
//!
//! Each command runs its blocking work (HTTP + file I/O) off the main
//! thread via `spawn_blocking`: Tauri executes synchronous commands on the main
//! thread, so doing the network/file operations inline would freeze the UI.

use std::collections::HashMap;

use tauri::{AppHandle, Manager};

use crate::core::AppState;
// [Track A] #186/D-B2: commands now return the structured SpeleoErrorDto (carrying
// the typed SpeleoDbAction) instead of a flattened String, so the FE branches on
// `err.action`. The orchestration `AppState::speleodb_*` methods build the DTO at
// the call site where the endpoint is known.
use crate::speleodb::{
    DoneResult, PushResult, SpeleoDbAction, SpeleoErrorDto, SpeleoLinkStatus,
    SpeleoLocalProjectState, SpeleoLoginResult, SpeleoOpenResult, SpeleoProjectSummary,
    SpeleoPublishParams, SpeleoPublishResult, SpeleoRevision, SpeleoRewindTarget,
    SpeleoSessionStatus, SpeleoUnwindTarget, StartEditingResult, UpdateResult,
};

/// Run a blocking SpeleoDB orchestration call off the main thread. The
/// orchestration already returns the typed [`SpeleoErrorDto`]; a task panic is
/// the only error this layer originates, surfaced as a `Fatal` DTO.
async fn run_blocking<T: Send + 'static>(
    app: AppHandle,
    f: impl FnOnce(&AppState) -> Result<T, SpeleoErrorDto> + Send + 'static,
) -> Result<T, SpeleoErrorDto> {
    tauri::async_runtime::spawn_blocking(move || f(app.state::<AppState>().inner()))
        .await
        .map_err(|e| SpeleoErrorDto {
            message: format!("SpeleoDB task panicked: {e}"),
            action: SpeleoDbAction::Fatal,
            status: None,
            transport: None,
            unpushed_local: None,
            newer_schema: None,
            endpoint_refusal: None,
        })?
}

/// [`run_blocking`] for a user-visible transfer - a lock, upload, download,
/// release or account change, i.e. every command the frontend's quit gate counts
/// as `exclusive` (`speleoNetworkCommands.ts`). The app's exit waits for it
/// instead of cutting it (lock#6, `exit_wait`); a read never holds the exit.
async fn run_transfer<T: Send + 'static>(
    app: AppHandle,
    f: impl FnOnce(&AppState) -> Result<T, SpeleoErrorDto> + Send + 'static,
) -> Result<T, SpeleoErrorDto> {
    let _transfer = super::exit_wait::user_transfer(&app);
    run_blocking(app, f).await
}

/// sdk#2: the reason an upload description cannot be sent, or `None` when it
/// can. Checked at the IPC boundary, BEFORE any work starts: a description the
/// server would commit and then fail to store breaks every later upload of the
/// project, and a publish would already have created the server project by the
/// time `client::upload_stalx` (which refuses it too) saw it. A blank
/// description is not refused here: the upload verbs substitute a default.
fn unstorable_commit_message(text: Option<&str>) -> Option<SpeleoErrorDto> {
    use crate::speleodb::commit_message::{check_commit_message, CommitMessageError};
    match check_commit_message(text?) {
        Err(e @ CommitMessageError::TooLong { .. }) => Some(SpeleoErrorDto {
            message: e.to_string(),
            action: SpeleoDbAction::Fatal,
            status: None,
            transport: None,
            unpushed_local: None,
            newer_schema: None,
            endpoint_refusal: None,
        }),
        Ok(()) | Err(CommitMessageError::Empty) => None,
    }
}

/// Log in to SpeleoDB. Either email+`password` (exchanged for a token) or a
/// pasted `token` must be supplied; with a pasted token the `email` is ignored
/// and derived from the API instead. `endpoint` overrides the configured/env
/// endpoint for this session, and `insecure_http` the saved plaintext-http
/// choice (Settings passes the values on screen); both are saved with the
/// session on success. Returns the (authoritative) user email + the
/// authenticated instance - never the token itself.
#[tauri::command]
pub async fn speleodb_login(
    app: AppHandle,
    endpoint: Option<String>,
    insecure_http: Option<bool>,
    email: Option<String>,
    password: Option<String>,
    token: Option<String>,
) -> Result<SpeleoLoginResult, SpeleoErrorDto> {
    run_transfer(app, move |s| {
        s.speleodb_login(endpoint, insecure_http, email, password, token)
    })
    .await
}

/// List the projects visible to the logged-in user (display summaries).
#[tauri::command]
pub async fn speleodb_list_projects(
    app: AppHandle,
) -> Result<Vec<SpeleoProjectSummary>, SpeleoErrorDto> {
    run_blocking(app, AppState::speleodb_list_projects).await
}

/// Pull a project from SpeleoDB and extract its `.stalx` into the app cache,
/// returning the local path. The frontend then opens it via `open_project`.
///
/// [Track B] (#189, D-B9) `project_name` + `latest_commit_sha` are extra args
/// (NOT a new command name → EVENT_NAMES stays 418) used to stamp the
/// `speleo_link` row so the pulled project is `is_remote == true` end-to-end.
#[tauri::command]
pub async fn speleodb_open_project(
    app: AppHandle,
    project_id: String,
    project_name: String,
    latest_commit_sha: Option<String>,
) -> Result<SpeleoOpenResult, SpeleoErrorDto> {
    run_transfer(app, move |s| {
        s.speleodb_open_project(project_id, project_name, latest_commit_sha)
    })
    .await
}

/// Pull a project from SpeleoDB over a local copy that still holds UN-PUSHED
/// EDITS, destroying them. The "take remote and discard local" branch of the
/// prompt raised when [`speleodb_open_project`] refuses with
/// `SpeleoDbAction::UnpushedLocalEdits`; call it with the same arguments the
/// refused open was given, and only after the user has confirmed the loss.
///
/// The other branch, "keep local", calls NO SpeleoDB command: it opens the
/// `local_path` carried on the refusal through the normal `load_project_full`
/// flow, so nothing is downloaded and nothing is overwritten.
///
/// A separate command rather than a flag on [`speleodb_open_project`], for the
/// same reason `speleodb_take_remote` is its own verb: the irreversible pull must
/// be spelled out at the call site.
#[tauri::command]
pub async fn speleodb_open_project_discarding_local(
    app: AppHandle,
    project_id: String,
    project_name: String,
    latest_commit_sha: Option<String>,
) -> Result<SpeleoOpenResult, SpeleoErrorDto> {
    run_transfer(app, move |s| {
        s.speleodb_open_project_discarding_local(project_id, project_name, latest_commit_sha)
    })
    .await
}

/// Forget the locally stored token, clear the session, and clear the persisted
/// account email (no server call). #270: an `Err` here does NOT mean the logout
/// failed - the session is always dropped; it means the credential could not be
/// removed from speleodb.conf, so the frontend warns (and can
/// offer a retry) while treating the user as logged out.
#[tauri::command]
pub async fn speleodb_logout(app: AppHandle) -> Result<(), SpeleoErrorDto> {
    run_transfer(app, AppState::speleodb_logout).await
}

/// Acquire the write lock on the open remote project and reconcile it against the
/// server (#194). Enables editing on a clean reconcile; reports divergence,
/// contention (locked-by), or an error otherwise.
#[tauri::command]
pub async fn speleodb_start_editing(app: AppHandle) -> Result<StartEditingResult, SpeleoErrorDto> {
    run_transfer(app, AppState::speleodb_start_editing).await
}

/// Push the open project's edits (if any) and release the lock, returning to
/// read-only (#197). `message` is the commit message (a default is used when
/// blank). On divergence / an ambiguous push the lock + dirty bit are retained.
#[tauri::command]
pub async fn speleodb_done(
    app: AppHandle,
    message: Option<String>,
    accept_older_version: Option<bool>,
) -> Result<DoneResult, SpeleoErrorDto> {
    if let Some(refused) = unstorable_commit_message(message.as_deref()) {
        return Err(refused);
    }
    run_transfer(app, move |s| {
        s.speleodb_done_acknowledged(message, accept_older_version.unwrap_or(false))
    })
    .await
}

/// Read-only refresh of the open project against the server (#197): no lock is
/// taken (UpToDate / FastForward / DivergenceNeeded).
#[tauri::command]
#[expect(
    clippy::result_large_err,
    reason = "IPC returns the shared structured SpeleoDB error"
)]
pub async fn speleodb_update(
    app: AppHandle,
    discard_changes: Option<bool>,
    expected: Option<stalagmight_core::speleodb::SpeleoReleaseContext>,
) -> Result<UpdateResult, SpeleoErrorDto> {
    run_transfer(app, move |s| {
        s.speleodb_reload(discard_changes.unwrap_or(false), expected)
    })
    .await
}

/// Push the open project's edits to SpeleoDB WITHOUT releasing the lock (T2d):
/// like `speleodb_done` but the lock is retained so editing continues. `message`
/// is the commit message (a default is used when blank).
#[tauri::command]
pub async fn speleodb_push(
    app: AppHandle,
    message: Option<String>,
    accept_older_version: Option<bool>,
) -> Result<PushResult, SpeleoErrorDto> {
    if let Some(refused) = unstorable_commit_message(message.as_deref()) {
        return Err(refused);
    }
    run_transfer(app, move |s| {
        s.speleodb_push_only_acknowledged(message, accept_older_version.unwrap_or(false))
    })
    .await
}

/// Take the SpeleoDB version, DISCARDING local unpushed edits (#198 "Take
/// remote"): re-pull the server tip and reset the local project to it, returning
/// the reloaded `.stalx` path so the frontend reloads the view. The frontend
/// gates this behind a second destroy-confirmation - it is the only irreversible
/// choice in the divergence prompt, so there is no single-click path to it.
#[tauri::command]
pub async fn speleodb_take_remote(app: AppHandle) -> Result<String, SpeleoErrorDto> {
    run_transfer(app, AppState::speleodb_take_remote).await
}

/// Hand back a write lock only if THIS process took it, and only the one it took.
///
/// For an unwind whose context went stale mid-flight. Deliberately takes no project
/// id: the caller's captured id is not necessarily what got locked, and an inherited
/// same-account lock must never be handed back. See
/// [`AppState::speleodb_release_lock_acquired_here`].
#[tauri::command]
pub async fn speleodb_release_lock_acquired_here(app: AppHandle) -> Result<(), SpeleoErrorDto> {
    run_transfer(app, AppState::speleodb_release_lock_acquired_here).await
}

/// Settle the OPEN project's write lock before a project switch or a quit leaves
/// it, without closing anything: hand it back, holder-checked (`release_lock`
/// unset or `true`), or keep it (`false`, the "Couldn't hand back the lock"
/// dialog's Close and keep the lock). The reply carries what the release came to
/// (`CommandResult::lock_release`); a failed or uncertain one fails the call, and
/// the switch or quit waits for the user's answer. A user-visible transfer: the
/// app's exit waits for it (lock#6).
#[tauri::command]
pub async fn speleodb_settle_open_lock(
    app: AppHandle,
    release_lock: Option<bool>,
) -> crate::core::CommandResult {
    let _transfer = super::exit_wait::user_transfer(&app);
    let release = release_lock != Some(false);
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<AppState>()
            .inner()
            .speleodb_settle_open_lock(release)
    })
    .await
    .unwrap_or_else(|e| {
        crate::core::CommandResult::failure(format!("Handing back the lock failed: {e}"))
            .with_lock_release(Some(crate::core::LockReleaseOutcome::Failed))
    })
}

/// Release the write lock on ONE project by id (#201 per-project "Release" in the
/// held-lock resume prompt). Targets an arbitrary held project, not necessarily
/// the open one; on success the caller re-scans to refresh the recents badges.
#[tauri::command]
pub async fn speleodb_release_lock(
    app: AppHandle,
    project_id: String,
) -> Result<(), SpeleoErrorDto> {
    run_transfer(app, move |s| s.speleodb_release_lock(project_id)).await
}

/// Per-project LOCAL state for a set of SpeleoDB projects: whether this machine
/// has a readable local copy and whether that copy has un-pushed edits. Joins the
/// local half onto the server-only scans (the held-lock resume prompt, the
/// GlobalView remote list), so the UI can offer "push and release" rather than a
/// bare "release" - and can say which listed projects are even present here.
///
/// `project_ids` limits the reply to those ids (each still gets a row, reporting
/// "nothing here" when absent); omitting it reports every project in the local
/// pull cache plus the open one. Keyed by project id for direct indexing. Purely
/// local: no token, no network, and an unreadable file costs only its own row.
#[tauri::command]
pub async fn speleodb_local_project_states(
    app: AppHandle,
    project_ids: Option<Vec<String>>,
) -> Result<HashMap<String, SpeleoLocalProjectState>, SpeleoErrorDto> {
    run_blocking(app, move |s| {
        Ok(s.speleodb_local_project_states(project_ids))
    })
    .await
}

/// [#218] Read a project's revision history for the Project Settings history view:
/// `GET /projects/{id}/revisions/`. Returns the commit chain projected to
/// [`SpeleoRevision`] DTOs. Permission-free read; no lock required.
#[tauri::command]
pub async fn speleodb_get_revisions(
    app: AppHandle,
    project_id: String,
) -> Result<Vec<SpeleoRevision>, SpeleoErrorDto> {
    run_blocking(app, move |s| s.speleodb_get_revisions(project_id)).await
}

/// [#218] Check the open remote project's working copy out to a past revision:
/// download the native SQLite file AT `hexsha`, replace the working copy, and reopen it. With
/// `restore == true` (the user holds the lock) it is stamped as an unpushed edit on
/// top of the current server HEAD (a git-forward restore; the user pushes to
/// persist); with `restore == false` it is a read-only View that a later Update
/// fast-forwards away. Returns the local path for the FE to refresh from.
///
/// `discard_local` waives the un-pushed-edits guard, and the FE may pass `true`
/// ONLY after a second, explicit confirmation naming what is being discarded. It
/// exists because a restore itself stamps the working copy as un-pushed, so without
/// it Restore was single-use: restoring a second revision refused over edits the
/// first restore had just created.
///
/// `keep_undo_history` (#303) is the "Unwind to previous SpeleoDB version" flavour: the
/// downloaded copy's undo history is KEPT rather than dropped, because that history is
/// what the action came for. Its extra preconditions (an empty undo stack, and any dirt
/// being a previous Unwind's own doing) are enforced in the backend, never trusted from
/// here.
///
/// `discard_unpushed` (#303) says the Unwind dialog raised its red warning and the user
/// went ahead anyway - the copy holds un-pushed work that this swap replaces (typically
/// edits the user undid, which sit on the redo stack). It is an acknowledgement, not a
/// decision: `speleodb_unwind_target.discards_unpushed` is what tells the FE to warn, and
/// the backend re-derives the same predicate before honouring this. Omitted or `false`,
/// the guard refuses exactly as it did before, so nothing else that passes
/// `keep_undo_history` can discard un-pushed work by accident.
#[tauri::command]
pub async fn speleodb_restore_revision(
    app: AppHandle,
    project_id: String,
    project_name: String,
    hexsha: String,
    restore: bool,
    discard_local: Option<bool>,
    keep_undo_history: Option<bool>,
    discard_unpushed: Option<bool>,
    chain_forward: Option<bool>,
) -> Result<SpeleoOpenResult, SpeleoErrorDto> {
    let discard_local = discard_local.unwrap_or(false);
    let keep_undo_history = keep_undo_history.unwrap_or(false);
    let discard_unpushed = discard_unpushed.unwrap_or(false);
    let chain_forward = chain_forward.unwrap_or(false);
    run_transfer(app, move |s| {
        s.speleodb_restore_revision(
            project_id,
            project_name,
            hexsha,
            restore,
            discard_local,
            keep_undo_history,
            discard_unpushed,
            chain_forward,
        )
    })
    .await
}

/// [#303] Which revision the next "Unwind to previous SpeleoDB version" would load, or
/// `None` when there is nothing left to reach (an empty repo, the root of the history, or
/// a project whose undo history is not pushed at all).
///
/// Read-only and permission-free, like the revision list it reads. Costs one
/// `GET /revisions/`, so the frontend asks when the undo dropdown opens rather than on
/// every status refresh.
#[tauri::command]
pub async fn speleodb_unwind_target(
    app: AppHandle,
) -> Result<Option<SpeleoUnwindTarget>, SpeleoErrorDto> {
    run_blocking(app, AppState::speleodb_unwind_target).await
}

/// [#303] Discard the local copy and return to the server tip.
///
/// The way out of a copy the user does not want to keep - most often one a chain step
/// walked back through history, where the dirty bit means "this is an older version",
/// not "you have work to save". Both callers confirm first: the push dialog raised when
/// a copy is dirty only because it was unwound, and the explicit Discard menu action,
/// which offers it over ordinary edits too and says so.
///
/// DESTRUCTIVE and deliberately not second-guessed here: the backend does not re-check
/// that the copy was merely unwound, because the menu action reaches it with real edits
/// pending on purpose.
///
/// What it does insist on is `acknowledged` - the caller reporting that it put the loss in
/// front of the user, the same shape as the Unwind guard's `discard_unpushed`. Without it
/// the backend has no way to tell a confirmed discard from a stray invocation of the one
/// verb here that destroys work irrecoverably. It is NOT a read-only or lock gate: a 401
/// auto-drop leaves the session read-only while still holding un-pushed work, and this is
/// the way out of that state.
///
/// `keep_lock` decides the only thing left once the copy is gone - carry on editing from
/// the latest, or hand the project back. Keeping returns the Start-editing outcome, so a
/// lock someone else took meanwhile surfaces as it would anywhere else; releasing returns
/// `null`, which is not the same as an editing result and must not be read as one.
#[tauri::command]
pub async fn speleodb_discard_changes(
    app: AppHandle,
    keep_lock: bool,
    acknowledged: bool,
) -> Result<Option<StartEditingResult>, SpeleoErrorDto> {
    run_transfer(app, move |s| {
        s.speleodb_discard_changes(keep_lock, acknowledged)
    })
    .await
}

/// [#303] Which revision a "Redo into next SpeleoDB version" step would load, or `None`
/// when there is nothing ahead: not mid-walk (the copy is already at BASE), already back
/// at BASE, or a chain pointer that cannot be walked from.
///
/// That first case is also how an edit removes the direction - a forward command drops the
/// chain pointer, so this stops resolving and the frontend hides the entry rather than
/// offering a step that would discard the edit.
///
/// Same cost and same timing as its backward twin: one `GET /revisions/`, asked when the
/// redo dropdown opens rather than on every status refresh.
#[tauri::command]
pub async fn speleodb_rewind_target(
    app: AppHandle,
) -> Result<Option<SpeleoRewindTarget>, SpeleoErrorDto> {
    run_blocking(app, AppState::speleodb_rewind_target).await
}

/// Report whether the backend already holds a usable token (and for whom). The
/// dialog calls this on open to restore the project list instead of re-prompting
/// for credentials - the in-memory token outlives the dialog's React state.
#[tauri::command]
pub async fn speleodb_session(app: AppHandle) -> Result<SpeleoSessionStatus, SpeleoErrorDto> {
    run_blocking(app, AppState::speleodb_session_status).await
}

/// Report the open project's SpeleoDB link/edit status (#204) so the action shell
/// can render the read-only/editing indicator and gate its controls. The local
/// fields are cheap; `permission`/`lock_holder` are best-effort (`None` when
/// logged out / offline). Called after every project load (and after edit-verb
/// results) to seed the frontend edit-state.
#[tauri::command]
pub async fn speleodb_link_status(app: AppHandle) -> Result<SpeleoLinkStatus, SpeleoErrorDto> {
    run_blocking(app, AppState::speleodb_link_status).await
}

/// Detach the open project from SpeleoDB, turning it into a plain unmanaged
/// `.stalx` (T2/#204): drop the `speleo_link` row and open the edit gate. Used to
/// keep a copy the server refuses us access to, or locally-edited work after a
/// divergence. Nothing is pushed or released - the managed copy is abandoned (a
/// later Open-from-SpeleoDB re-pulls the server tip). The caller typically saves
/// the project to a new path first so the user's edits land somewhere permanent.
#[tauri::command]
pub async fn speleodb_unlink(app: AppHandle) -> Result<(), SpeleoErrorDto> {
    run_transfer(app, AppState::speleodb_unlink).await
}

/// Remove the `speleo_link` row from a CLOSED foreign `.stalx` so it can be
/// opened as a plain local project - the strip half of the managed-file prompt
/// raised when a non-canonical copy of a managed project is opened (the other
/// half re-opens the project from SpeleoDB). Refuses the currently open
/// project: that one detaches via `speleodb_unlink`, which also releases the
/// held write lock.
///
/// `expected` is the refusal payload the prompt was raised with, re-checked
/// against the file before the row is deleted: a prompt can sit on screen while
/// the `.stalx` at that path is replaced, and stripping whatever is there NOW
/// would unmanage a project the user was never asked about.
#[tauri::command]
pub async fn speleodb_remove_link_at(
    app: AppHandle,
    path: std::path::PathBuf,
    expected: crate::core::loading::SpeleoManagedFile,
) -> Result<(), SpeleoErrorDto> {
    run_blocking(app, move |s| s.speleodb_remove_link_at(path, expected)).await
}

/// Everyone with access to SpeleoDB project `project_id` (the signed-in user
/// included), with display names where the server knows them: the survey-credit
/// people inputs' suggestions for a linked project. Two bounded reads, served
/// from an in-memory per-project cache for ten minutes:
/// `GET /api/v2/projects/{id}/permissions/user/`, then
/// `GET /api/v2/user/autocomplete/?query=<email>` per user.
#[tauri::command]
pub async fn speleodb_project_collaborators(
    app: AppHandle,
    project_id: String,
) -> Result<Vec<stalagmight_core::speleodb::SpeleoCollaborator>, SpeleoErrorDto> {
    run_blocking(app, move |s| s.speleodb_project_collaborators(project_id)).await
}

/// Create the open local project on SpeleoDB and push its first revision (#207):
/// `POST /projects/` → relocate into the cache → acquire → upload → release. Returns
/// the new project id, endpoint, and cache `local_path` the frontend reloads.
#[tauri::command]
pub async fn speleodb_publish(
    app: AppHandle,
    params: SpeleoPublishParams,
) -> Result<SpeleoPublishResult, SpeleoErrorDto> {
    if let Some(refused) = unstorable_commit_message(params.message.as_deref()) {
        return Err(refused);
    }
    run_transfer(app, move |s| s.speleodb_publish(params)).await
}

// Release the open project's lock.

/// Switch a clean linked project to read-only, releasing only our own lock.
#[tauri::command]
#[expect(
    clippy::result_large_err,
    reason = "IPC returns the shared structured SpeleoDB error"
)]
pub async fn speleodb_release_current_lock(
    app: AppHandle,
    expected: Option<stalagmight_core::speleodb::SpeleoReleaseContext>,
) -> Result<(), SpeleoErrorDto> {
    run_transfer(app, move |s| s.speleodb_release_current_lock(expected)).await
}

/// Read access restrictions before opening a linked SQLite project from disk.
#[tauri::command]
pub async fn speleodb_project_access_at(
    app: AppHandle,
    path: String,
    local_only: Option<bool>,
) -> Result<Option<stalagmight_core::speleodb::SpeleoFileAccess>, SpeleoErrorDto> {
    run_blocking(app, move |s| {
        s.speleodb_project_access_at(path, local_only.unwrap_or(false))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_upload_description_the_server_cannot_store_is_refused_up_front() {
        // sdk#2: 1001 characters once each line break becomes ". ".
        let long = format!("{}\n{}", "a".repeat(500), "b".repeat(499));
        let refused = unstorable_commit_message(Some(&long));
        assert!(
            matches!(&refused, Some(dto) if dto.action == SpeleoDbAction::Fatal && dto.message.contains("1001")),
            "{refused:?}"
        );
        assert!(unstorable_commit_message(Some(&"a".repeat(1000))).is_none());
        // Blank and absent fall through to the default description.
        assert!(unstorable_commit_message(Some("  ")).is_none());
        assert!(unstorable_commit_message(None).is_none());
    }
}
