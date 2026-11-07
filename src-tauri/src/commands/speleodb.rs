//! IPC command surface for the SpeleoDB integration (#182 epic).
//!
//! Thin wrappers over the `AppState::speleodb_*` orchestration (which lives in
//! `core::state::speleodb`, over the `crate::speleodb` client primitives). The
//! full surface is here: session (login / logout / session), pull (list / open /
//! adopt), the lock + edit gate (start-editing / force-edit / done / push /
//! update / take-remote / the release verbs), link status + unlink, revision
//! history and restore, and project admin (metadata / publish / collaborators /
//! teams / delete).
//!
//! Errors cross the boundary as the [`CommandError`] built from the structured
//! [`SpeleoErrorDto`] (#186, #576 C4): its `SPELEODB_*` code (one per
//! `SpeleoDbAction`) is what the frontend branches on - not a flattened
//! `String`. The auth token is never returned to the webview (login
//! yields only the email + keychain flag).
//!
//! Each command runs its blocking work (HTTP + OS-keychain I/O) off the main
//! thread via `spawn_blocking`: Tauri executes synchronous commands on the main
//! thread, so doing the network/keychain round-trips inline would freeze the UI.
//!
//! [#576 RVF-c-C1] A verb that writes the OPEN project - its `.stalx`, its link
//! row, or its server side through that link - takes `expected_project` (the
//! project the frontend made the request for) and runs inside `for_project`, so
//! one made for another project than the open one is refused with
//! `PROJECT_CHANGED` (`core::state::expected_project`). The app crate's contract
//! scan lists which verbs those are.

use std::collections::HashMap;

use tauri::{AppHandle, Manager};

use crate::config::TokenFallback;
use crate::core::state::{for_project, ProjectIdentity};
use crate::core::{
    AppState, CommandError, SpeleoDoneReport, SpeleoLogoutReport, SpeleoPushReport,
    SpeleoSignOutLocks,
};
// [Track A] #186/D-B2: the orchestration `AppState::speleodb_*` methods build the
// structured SpeleoErrorDto at the call site where the endpoint is known; the
// commands hand it on as a CommandError carrying its code (#576 C4), so the FE
// branches on `err.code`.
use crate::models::UndoMode;
use crate::speleodb::{
    Permission, SpeleoAdoptResult, SpeleoCollaborators, SpeleoDumpCandidate, SpeleoErrorDto,
    SpeleoLinkStatus, SpeleoLocalProjectState, SpeleoLoginResult, SpeleoOpenResult,
    SpeleoProjectMetadata, SpeleoProjectMetadataPatch, SpeleoProjectSummary, SpeleoPublishParams,
    SpeleoRevision, SpeleoRewindTarget, SpeleoSessionStatus, SpeleoTeamOption, SpeleoUnwindTarget,
    SpeleoUserSuggestion, StartEditingResult, UpdateResult,
};

/// Run a blocking SpeleoDB orchestration call off the main thread. The
/// orchestration already returns the typed [`SpeleoErrorDto`], which crosses the
/// boundary as the [`CommandError`] carrying its code (#576 C4); a task panic is
/// the only error this layer originates, surfaced as `INTERNAL_ERROR`.
async fn run_blocking<T: Send + 'static>(
    app: AppHandle,
    f: impl FnOnce(&AppState) -> Result<T, SpeleoErrorDto> + Send + 'static,
) -> Result<T, CommandError> {
    match tauri::async_runtime::spawn_blocking(move || f(app.state::<AppState>().inner())).await {
        Ok(answer) => answer.map_err(CommandError::from),
        Err(e) => Err(CommandError::internal(format!(
            "SpeleoDB task panicked: {e}"
        ))),
    }
}

/// Log in to SpeleoDB. Either email+`password` (exchanged for a token) or a
/// pasted `token` must be supplied; with a pasted token the `email` is ignored
/// and derived from the API instead. `endpoint` overrides the configured/env
/// endpoint for this session. Returns the (authoritative) user email + whether
/// the token was stored in the OS keychain - never the token itself.
///
/// [#576 SIGNOUT-PROMPT-D1] `release_locks` is the sign-out prompt's answer for the
/// session this sign-in replaces: `false` keeps that session's write lock held on
/// SpeleoDB. Ask [`speleodb_sign_in_hand_back_locks`] first whether there is
/// anything to ask about.
#[tauri::command]
pub async fn speleodb_login(
    app: AppHandle,
    endpoint: Option<String>,
    email: Option<String>,
    password: Option<String>,
    token: Option<String>,
    release_locks: bool,
) -> Result<SpeleoLoginResult, CommandError> {
    run_blocking(app, move |s| {
        s.speleodb_login(endpoint, email, password, token, release_locks)
    })
    .await
}

/// [#576 SIGNOUT-PROMPT-D1/D2] The write locks this session holds at a sign-out:
/// what the sign-out prompt names before a logout - those it would hand back, and
/// those it keeps for un-pushed edits. Both empty when there is nothing to ask.
#[tauri::command]
pub async fn speleodb_sign_out_locks(app: AppHandle) -> Result<SpeleoSignOutLocks, CommandError> {
    run_blocking(app, |s| Ok(s.speleodb_sign_out_locks())).await
}

/// [#576 SIGNOUT-PROMPT-D1/D2] The write locks of the live session a sign-in with
/// these credentials replaces (another account or server), split as
/// [`speleodb_sign_out_locks`] splits them: what the sign-out prompt names before
/// such a sign-in. Both empty when it replaces nothing or that session holds no lock. A pasted `token` is resolved to its
/// account as the login resolves it.
#[tauri::command]
pub async fn speleodb_sign_in_hand_back_locks(
    app: AppHandle,
    endpoint: Option<String>,
    email: Option<String>,
    token: Option<String>,
) -> Result<SpeleoSignOutLocks, CommandError> {
    run_blocking(app, move |s| {
        s.speleodb_sign_in_hand_back_locks(endpoint, email, token)
    })
    .await
}

/// List the projects visible to the logged-in user (display summaries).
#[tauri::command]
pub async fn speleodb_list_projects(
    app: AppHandle,
) -> Result<Vec<SpeleoProjectSummary>, CommandError> {
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
    entry_path: Option<String>,
) -> Result<SpeleoOpenResult, CommandError> {
    run_blocking(app, move |s| {
        s.speleodb_open_project(project_id, project_name, latest_commit_sha, entry_path)
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
    entry_path: Option<String>,
) -> Result<SpeleoOpenResult, CommandError> {
    run_blocking(app, move |s| {
        s.speleodb_open_project_discarding_local(
            project_id,
            project_name,
            latest_commit_sha,
            entry_path,
        )
    })
    .await
}

/// [#258] What a project dump holds, WITHOUT taking the write lock or touching the
/// cache. `hexsha` targets a specific revision; omit it for the current tip.
///
/// Lets a flow that needs the lock (the revert composite) put its "which file is the
/// survey?" question BEFORE acquiring, instead of taking the lock, failing, handing
/// it back to ask, and re-taking it.
///
/// Returns the candidates when there is a real choice to make, and `null` otherwise.
/// It ALWAYS downloads: it used to skip that when the session held an answer, but the
/// answer it held was about a different tree than the one being asked about, so the
/// short-circuit was answering the wrong question. The recall is consulted inside the
/// resolver instead, which honours it only for the revision it belongs to.
///
/// Advisory: it never reports its own failures, so the call that follows stays the one
/// authoritative place a problem is surfaced.
#[tauri::command]
pub async fn speleodb_dump_candidates(
    app: AppHandle,
    project_id: String,
    hexsha: Option<String>,
) -> Result<Option<Vec<SpeleoDumpCandidate>>, CommandError> {
    run_blocking(app, move |s| s.speleodb_dump_candidates(project_id, hexsha)).await
}

/// [#214] Adopt an Ariane (`.tml`-only) project: import its `ariane.tml`, save a
/// new `.stalx` into the cache, link it, and best-effort flip the server `type`
/// ariane→other. The frontend calls this (after a user confirm) when
/// `speleodb_open_project` reported `SpeleoDbAction::Adopt`, then reloads
/// `local_path` through the normal `open_project` flow (read-only-remote).
///
/// `discard_local` waives the un-pushed-edits guard, and the FE may pass `true`
/// ONLY on the path where the user has already confirmed that loss: the "take the
/// server copy and discard my local changes" branch of the refused-open prompt,
/// which lands back here when the project turns out to still be Ariane. That
/// happens for real - an adopt stamps the cache copy un-pushed, and the server
/// `type` flip is best-effort, so an adopt without write access leaves a project
/// that is both dirty locally AND still classified Ariane. Without the waiver the
/// user confirmed a discard, was handed the adopt confirm, and then hit a guard
/// refusal they had already answered.
#[tauri::command]
pub async fn speleodb_adopt_ariane(
    app: AppHandle,
    project_id: String,
    project_name: String,
    latest_commit_sha: Option<String>,
    discard_local: Option<bool>,
    entry_path: Option<String>,
) -> Result<SpeleoAdoptResult, CommandError> {
    let discard_local = discard_local.unwrap_or(false);
    run_blocking(app, move |s| {
        s.speleodb_adopt_ariane(
            project_id,
            project_name,
            latest_commit_sha,
            discard_local,
            entry_path,
        )
    })
    .await
}

/// Forget the locally stored token, clear the session, and clear the persisted
/// account email (no server call). #270: an `Err` here does NOT mean the logout
/// failed - the session is always dropped; it means the credential could not be
/// removed from the OS keychain / plaintext store, so the frontend warns (and can
/// offer a retry) while treating the user as logged out. [#576 Q-W61-3-D1] The `Ok`
/// answer names each project whose write lock could not be released, for the
/// frontend's warning (the sign-out happened regardless). [#576 SIGNOUT-PROMPT-D1]
/// `release_locks` is the sign-out prompt's answer: `false` sends no release and
/// leaves the write lock held on SpeleoDB.
#[tauri::command]
pub async fn speleodb_logout(
    app: AppHandle,
    release_locks: bool,
) -> Result<SpeleoLogoutReport, CommandError> {
    run_blocking(app, move |s| s.speleodb_logout(release_locks)).await
}

/// [#213] Enable optimistic force-edit: edit the open remote project WITHOUT the
/// server write lock (offline / contended), behind the FE's big-red warning. Opt-in
/// (gated on the `allow_force_edit` config). No acquire, no network, BASE unchanged;
/// the user later reconnects via `speleodb_start_editing`.
#[tauri::command]
pub async fn speleodb_force_edit(
    app: AppHandle,
    expected_project: Option<ProjectIdentity>,
) -> Result<(), CommandError> {
    run_blocking(app, move |s| {
        for_project(expected_project, || s.speleodb_force_edit())
    })
    .await
}

/// Acquire the write lock on the open remote project and reconcile it against the
/// server (#194). Enables editing on a clean reconcile; reports divergence,
/// contention (locked-by), or an error otherwise. [#213] Also serves as the
/// force-edit "reconnect".
///
/// [#576 RV2-R-13-RESTART] `resume_held_lock`: set by the held-lock prompt's
/// "Continue editing", which resumes a lock the account already holds and claims it
/// for this session, so Done releases it. Absent / false everywhere else.
#[tauri::command]
pub async fn speleodb_start_editing(
    app: AppHandle,
    expected_project: Option<ProjectIdentity>,
    resume_held_lock: Option<bool>,
) -> Result<StartEditingResult, CommandError> {
    run_blocking(app, move |s| {
        for_project(expected_project, || {
            if resume_held_lock == Some(true) {
                s.speleodb_resume_held_lock_editing()
            } else {
                s.speleodb_start_editing()
            }
        })
    })
    .await
}

/// Push the open project's edits (if any) and release the lock, returning to
/// read-only (#197). `message` is the commit message (a default is used when
/// blank). On divergence / an ambiguous push the lock + dirty bit are retained.
/// [#576 NF-W2SAVE-1] Answers the outcome with the pre-push save's warnings.
#[tauri::command]
pub async fn speleodb_done(
    app: AppHandle,
    expected_project: Option<ProjectIdentity>,
    message: Option<String>,
    accept_older_version: Option<bool>,
) -> Result<SpeleoDoneReport, CommandError> {
    run_blocking(app, move |s| {
        for_project(expected_project, || {
            s.speleodb_done_reported(message, accept_older_version.unwrap_or(false))
        })
    })
    .await
}

/// Read-only refresh of the open project against the server (#197): no lock is
/// taken (UpToDate / FastForward / DivergenceNeeded).
#[tauri::command]
pub async fn speleodb_update(
    app: AppHandle,
    expected_project: Option<ProjectIdentity>,
) -> Result<UpdateResult, CommandError> {
    run_blocking(app, move |s| {
        for_project(expected_project, || s.speleodb_update())
    })
    .await
}

/// Push the open project's edits to SpeleoDB WITHOUT releasing the lock (T2d):
/// like `speleodb_done` but the lock is retained so editing continues. `message`
/// is the commit message (a default is used when blank).
/// [#576 NF-W2SAVE-1] Answers the outcome with the pre-push save's warnings.
#[tauri::command]
pub async fn speleodb_push(
    app: AppHandle,
    expected_project: Option<ProjectIdentity>,
    message: Option<String>,
    accept_older_version: Option<bool>,
) -> Result<SpeleoPushReport, CommandError> {
    run_blocking(app, move |s| {
        for_project(expected_project, || {
            s.speleodb_push_only_reported(message, accept_older_version.unwrap_or(false))
        })
    })
    .await
}

/// Take the SpeleoDB version, DISCARDING local unpushed edits (#198 "Take
/// remote"): re-pull the server tip and reset the local project to it, returning
/// the reloaded `.stalx` path so the frontend reloads the view. The frontend
/// gates this behind a second destroy-confirmation - it is the only irreversible
/// choice in the divergence prompt, so there is no single-click path to it.
#[tauri::command]
pub async fn speleodb_take_remote(
    app: AppHandle,
    expected_project: Option<ProjectIdentity>,
) -> Result<String, CommandError> {
    run_blocking(app, move |s| {
        for_project(expected_project, || s.speleodb_take_remote())
    })
    .await
}

/// Release EVERY write lock this user currently holds (#201 "Release all"): the
/// held-lock resume prompt's resolution for locks a crashed/abandoned session
/// left behind. No-op server-side when none are held.
#[tauri::command]
pub async fn speleodb_release_all_locks(app: AppHandle) -> Result<(), CommandError> {
    run_blocking(app, AppState::speleodb_release_all_locks).await
}

/// Hand back a write lock only if THIS process took it, and only the one it took.
///
/// For an unwind whose context went stale mid-flight. Deliberately takes no project
/// id: the caller's captured id is not necessarily what got locked, and an inherited
/// same-account lock must never be handed back. See
/// [`AppState::speleodb_release_lock_acquired_here`].
#[tauri::command]
pub async fn speleodb_release_lock_acquired_here(app: AppHandle) -> Result<(), CommandError> {
    run_blocking(app, AppState::speleodb_release_lock_acquired_here).await
}

/// Release the write lock on ONE project by id (#201 per-project "Release" in the
/// held-lock resume prompt). Targets an arbitrary held project, not necessarily
/// the open one; on success the caller re-scans to refresh the recents badges.
#[tauri::command]
pub async fn speleodb_release_lock(app: AppHandle, project_id: String) -> Result<(), CommandError> {
    run_blocking(app, move |s| s.speleodb_release_lock(project_id)).await
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
) -> Result<HashMap<String, SpeleoLocalProjectState>, CommandError> {
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
) -> Result<Vec<SpeleoRevision>, CommandError> {
    run_blocking(app, move |s| s.speleodb_get_revisions(project_id)).await
}

/// [#218] Check the open remote project's working copy out to a past revision:
/// download the dump AT `hexsha`, replace the working copy, and reopen it. With
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
    expected_project: Option<ProjectIdentity>,
    project_id: String,
    project_name: String,
    hexsha: String,
    restore: bool,
    discard_local: Option<bool>,
    entry_path: Option<String>,
    keep_undo_history: Option<bool>,
    discard_unpushed: Option<bool>,
    chain_forward: Option<bool>,
) -> Result<SpeleoOpenResult, CommandError> {
    let discard_local = discard_local.unwrap_or(false);
    let keep_undo_history = keep_undo_history.unwrap_or(false);
    let discard_unpushed = discard_unpushed.unwrap_or(false);
    let chain_forward = chain_forward.unwrap_or(false);
    run_blocking(app, move |s| {
        for_project(expected_project, || {
            s.speleodb_restore_revision(
                project_id,
                project_name,
                hexsha,
                restore,
                discard_local,
                entry_path,
                keep_undo_history,
                discard_unpushed,
                chain_forward,
            )
        })
    })
    .await
}

/// [#303] Which revision the next "Unwind to previous SpeleoDB version" would load, or
/// `None` when there is nothing left to reach (an empty repo, the root of the history, or
/// a project whose undo history is not pushed at all).
///
/// Read-only and permission-free, like the revision list it reads. Costs one
/// `GET /revisions/`, so the frontend asks when the undo dropdown opens rather than on
/// every status poll.
#[tauri::command]
pub async fn speleodb_unwind_target(
    app: AppHandle,
) -> Result<Option<SpeleoUnwindTarget>, CommandError> {
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
    expected_project: Option<ProjectIdentity>,
    keep_lock: bool,
    acknowledged: bool,
) -> Result<Option<StartEditingResult>, CommandError> {
    run_blocking(app, move |s| {
        for_project(expected_project, || {
            s.speleodb_discard_changes(keep_lock, acknowledged)
        })
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
/// redo dropdown opens rather than on every status poll.
#[tauri::command]
pub async fn speleodb_rewind_target(
    app: AppHandle,
) -> Result<Option<SpeleoRewindTarget>, CommandError> {
    run_blocking(app, AppState::speleodb_rewind_target).await
}

/// [#303] Set the open project's per-project undo-history policy
/// (`speleo_link.undo_mode`).
///
/// EDIT-SESSION-REQUIRED: this writes the `.stalx`, which is project state that rides to
/// the server in the next push, so every collaborator inherits it. Force editing uses
/// the same gate deliberately - see `AppState::speleodb_set_undo_mode`.
#[tauri::command]
pub async fn speleodb_set_undo_mode(
    app: AppHandle,
    expected_project: Option<ProjectIdentity>,
    mode: UndoMode,
) -> Result<(), CommandError> {
    run_blocking(app, move |s| {
        for_project(expected_project, || s.speleodb_set_undo_mode(mode))
    })
    .await
}

/// Set the open project's server-side `exclude_geojson` flag (#200
/// Enable-rendering): `PATCH /projects/{id}/`, permission-gated, no lock. Passing
/// `false` re-enables web-viewer GeoJSON, effective on the next push.
#[tauri::command]
pub async fn speleodb_set_exclude_geojson(
    app: AppHandle,
    expected_project: Option<ProjectIdentity>,
    exclude: bool,
) -> Result<(), CommandError> {
    run_blocking(app, move |s| {
        for_project(expected_project, || s.speleodb_set_exclude_geojson(exclude))
    })
    .await
}

/// Report whether the backend already holds a usable token (and for whom). The
/// dialog calls this on open to restore the project list instead of re-prompting
/// for credentials - the in-memory token outlives the dialog's React state.
#[tauri::command]
pub async fn speleodb_session(app: AppHandle) -> Result<SpeleoSessionStatus, CommandError> {
    run_blocking(app, AppState::speleodb_session_status).await
}

/// Record where the API key should live after the keychain failed at login, and
/// move the held token there. `mode` is the new `token_fallback` preference -
/// `session_only` or `plaintext` - and answering it also marks the keychain
/// declined, so nothing touches it again until the next explicit sign-in.
///
/// No re-login: the token is already in memory, which is why the question is
/// worth asking at the moment of failure rather than in Settings later. Returns
/// the refreshed status, which reports where the key actually landed (a plaintext
/// write can fail, and then the truthful answer is session-only).
#[tauri::command]
pub async fn speleodb_set_token_storage(
    app: AppHandle,
    mode: TokenFallback,
) -> Result<SpeleoSessionStatus, CommandError> {
    run_blocking(app, move |s| s.speleodb_set_token_storage(mode)).await
}

/// Give the OS keychain another go (Settings' "Try again"): clear the persisted
/// refusal and re-store the held token, which now reaches the keychain again.
/// Safe to press twice - a keychain that is still broken just falls back the way
/// login does, and the returned status says so.
#[tauri::command]
pub async fn speleodb_retry_keychain(app: AppHandle) -> Result<SpeleoSessionStatus, CommandError> {
    run_blocking(app, AppState::speleodb_retry_keychain).await
}

/// Report the open project's SpeleoDB link/edit status (#204) so the action shell
/// can render the read-only/editing indicator and gate its controls. The local
/// fields are cheap; `permission`/`lock_holder` are best-effort (`None` when
/// logged out / offline). Called after every project load (and after edit-verb
/// results) to seed the frontend edit-state.
#[tauri::command]
pub async fn speleodb_link_status(app: AppHandle) -> Result<SpeleoLinkStatus, CommandError> {
    run_blocking(app, AppState::speleodb_link_status).await
}

/// [#576 NF-W2F-1] The open file's flags as they stand now (newer-format stamp,
/// unreadable link row). The frontend re-asks after every SpeleoDB in-place tree
/// swap - revision View/Restore, fast-forward, take remote - which reload the file
/// in the backend without going through the frontend's `loadProject`, so the
/// read-only banner and gate otherwise described the previous bytes until reopen.
/// A peek, like `history_discarded`: cheap, local, no network.
///
/// On the blocking pool like every command (#576 C2): it reads the open
/// database's link row. [#576 W81-1] Between commands (`settled_read`), since it also
/// compares the open project's stamp with its file's (`has_unsaved_changes`),
/// which a save in flight is about to rename over.
#[tauri::command]
pub async fn speleodb_open_file_flags<R: tauri::Runtime>(
    app: AppHandle<R>,
) -> Result<crate::core::loading::OpenFileFlags, CommandError> {
    crate::commands::pool::run(app, |state| {
        Ok(state.settled_read(|| state.open_file_flags()))
    })
    .await
}

/// Detach the open project from SpeleoDB by MOVING its copy out of the SpeleoDB
/// cache to `destination` (T2/#204, #576 FZ-SDB-5): the copy is written there
/// unlinked, the session on the cache file is closed, the cache file removed and a
/// held lock handed back. Used to keep a copy the server refuses us access to,
/// locally-edited work after a divergence, and an unlinked copy an earlier Unlink
/// left in the cache. Answers where the copy landed (`saved_path`), which the
/// frontend then loads, and what the save could not keep. Nothing is pushed.
///
/// `destination` passes the write-path policy first, like every save target.
#[tauri::command]
pub async fn speleodb_unlink(
    app: AppHandle,
    expected_project: Option<ProjectIdentity>,
    destination: std::path::PathBuf,
) -> Result<crate::core::outcome::SaveOutcome, CommandError> {
    crate::commands::write_path::check_write_path(
        &destination,
        crate::commands::write_path::WriteTarget::Project,
    )
    .map_err(|why| {
        crate::speleodb::contract::dto_for_error(
            crate::speleodb::contract::Endpoint::User,
            &crate::speleodb::SpeleoDbError::refused(why),
        )
    })?;
    run_blocking(app, move |s| {
        for_project(expected_project, || s.speleodb_unlink(destination))
    })
    .await
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
) -> Result<(), CommandError> {
    // [#576 C11-D1] It rewrites that project file.
    crate::commands::write_path::check_write_path(
        &path,
        crate::commands::write_path::WriteTarget::Project,
    )
    .map_err(|why| {
        crate::speleodb::contract::dto_for_error(
            crate::speleodb::contract::Endpoint::User,
            &crate::speleodb::SpeleoDbError::refused(why),
        )
    })?;
    run_blocking(app, move |s| s.speleodb_remove_link_at(path, expected)).await
}

/// GET the open remote project's editable metadata so the Project Settings tab can
/// prefill the current server truth (#206). No lock; permission-free read.
#[tauri::command]
pub async fn speleodb_get_project_metadata(
    app: AppHandle,
) -> Result<SpeleoProjectMetadata, CommandError> {
    run_blocking(app, AppState::speleodb_get_project_metadata).await
}

/// PATCH the open remote project's metadata with only the changed fields (#206):
/// `PATCH /projects/{id}/`, permission-gated (READ_AND_WRITE), no lock. On a name
/// change the SDB-authoritative name is folded into the local `.stalx` without
/// dirtying. Returns the post-edit server truth.
#[tauri::command]
pub async fn speleodb_patch_project(
    app: AppHandle,
    expected_project: Option<ProjectIdentity>,
    patch: SpeleoProjectMetadataPatch,
) -> Result<SpeleoProjectMetadata, CommandError> {
    run_blocking(app, move |s| {
        for_project(expected_project, || s.speleodb_patch_project(patch))
    })
    .await
}

/// Create the open local project on SpeleoDB and push its first revision (#207):
/// `POST /projects/` → relocate into the cache → acquire → upload → release. Returns
/// the new project id, endpoint, and cache `local_path` the frontend reloads.
#[tauri::command]
pub async fn speleodb_publish(
    app: AppHandle,
    expected_project: Option<ProjectIdentity>,
    params: SpeleoPublishParams,
) -> Result<crate::core::command::SpeleoPublishReport, CommandError> {
    run_blocking(app, move |s| {
        for_project(expected_project, || s.speleodb_publish(params))
    })
    .await
}

// [#215] Full Project Settings: collaborator management (per-user + per-team), in-app
// lock release, and project delete. All operate on the OPEN linked project and are
// permission-gated only in the UI; the server 401/403 is the real enforcement.

/// List the linked project's collaborator roster (individual users + teams). No
/// lock; requires READ_ONLY+ server-side.
#[tauri::command]
pub async fn speleodb_list_collaborators(
    app: AppHandle,
) -> Result<SpeleoCollaborators, CommandError> {
    run_blocking(app, AppState::speleodb_list_collaborators).await
}

/// Grant a collaborator (by email) a permission level on the linked project
/// (ADMIN). Returns the refreshed roster (server truth).
#[tauri::command]
pub async fn speleodb_add_collaborator(
    app: AppHandle,
    expected_project: Option<ProjectIdentity>,
    email: String,
    permission: Permission,
) -> Result<SpeleoCollaborators, CommandError> {
    run_blocking(app, move |s| {
        for_project(expected_project, || {
            s.speleodb_add_collaborator(email, permission)
        })
    })
    .await
}

/// Change an existing collaborator's permission level (ADMIN).
#[tauri::command]
pub async fn speleodb_set_collaborator(
    app: AppHandle,
    expected_project: Option<ProjectIdentity>,
    email: String,
    permission: Permission,
) -> Result<SpeleoCollaborators, CommandError> {
    run_blocking(app, move |s| {
        for_project(expected_project, || {
            s.speleodb_set_collaborator(email, permission)
        })
    })
    .await
}

/// Remove a collaborator from the linked project (ADMIN; soft-delete server-side).
#[tauri::command]
pub async fn speleodb_remove_collaborator(
    app: AppHandle,
    expected_project: Option<ProjectIdentity>,
    email: String,
) -> Result<SpeleoCollaborators, CommandError> {
    run_blocking(app, move |s| {
        for_project(expected_project, || s.speleodb_remove_collaborator(email))
    })
    .await
}

/// Grant a team a permission level on the linked project (ADMIN; teams cannot hold
/// ADMIN). Returns the refreshed roster.
#[tauri::command]
pub async fn speleodb_add_team(
    app: AppHandle,
    expected_project: Option<ProjectIdentity>,
    team_id: String,
    permission: Permission,
) -> Result<SpeleoCollaborators, CommandError> {
    run_blocking(app, move |s| {
        for_project(expected_project, || {
            s.speleodb_add_team(team_id, permission)
        })
    })
    .await
}

/// Change a team's permission level on the linked project (ADMIN).
#[tauri::command]
pub async fn speleodb_set_team(
    app: AppHandle,
    expected_project: Option<ProjectIdentity>,
    team_id: String,
    permission: Permission,
) -> Result<SpeleoCollaborators, CommandError> {
    run_blocking(app, move |s| {
        for_project(expected_project, || {
            s.speleodb_set_team(team_id, permission)
        })
    })
    .await
}

/// Revoke a team's access to the linked project (ADMIN; soft-delete server-side).
#[tauri::command]
pub async fn speleodb_remove_team(
    app: AppHandle,
    expected_project: Option<ProjectIdentity>,
    team_id: String,
) -> Result<SpeleoCollaborators, CommandError> {
    run_blocking(app, move |s| {
        for_project(expected_project, || s.speleodb_remove_team(team_id))
    })
    .await
}

/// List the teams the signed-in user belongs to, for the add-team picker.
#[tauri::command]
pub async fn speleodb_list_teams(app: AppHandle) -> Result<Vec<SpeleoTeamOption>, CommandError> {
    run_blocking(app, AppState::speleodb_list_teams).await
}

/// Autocomplete users by email/name substring for the add-collaborator typeahead.
/// Returns [] below the server's 3-char minimum.
#[tauri::command]
pub async fn speleodb_search_users(
    app: AppHandle,
    query: String,
) -> Result<Vec<SpeleoUserSuggestion>, CommandError> {
    run_blocking(app, move |s| s.speleodb_search_users(query)).await
}

/// Release the lock on the linked project (own lock, or an ADMIN force-release of
/// another user's lock). Does not push; a recovery action for a stuck lock.
#[tauri::command]
pub async fn speleodb_release_current_lock(
    app: AppHandle,
    expected_project: Option<ProjectIdentity>,
) -> Result<(), CommandError> {
    run_blocking(app, move |s| {
        for_project(expected_project, || s.speleodb_release_current_lock())
    })
    .await
}

/// Delete the linked project on SpeleoDB (ADMIN; soft-delete), then unlink locally
/// so the app stops treating it as remote.
#[tauri::command]
pub async fn speleodb_delete_project(
    app: AppHandle,
    expected_project: Option<ProjectIdentity>,
) -> Result<(), CommandError> {
    run_blocking(app, move |s| {
        for_project(expected_project, || s.speleodb_delete_project())
    })
    .await
}
