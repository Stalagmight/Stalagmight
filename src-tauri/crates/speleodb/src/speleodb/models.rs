//! Application projections of SDK responses and IPC-facing types.
//! The generated SDK owns the HTTP wire contract.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

// [Track A] #186/D-B2: the typed action carried on the IPC error DTO.
use super::contract::{EndpointRefusal, SpeleoDbAction, TransportCause};

/// Project permission level.
///
/// The server serializes the **label string** (e.g. `"READ_AND_WRITE"`), not the
/// numeric `PermissionLevel` enum - verified in `serializers/project.py`
/// (`get_permission` → `str(...level_label)`). Issues #188/#203 describe this as
/// an integer; that is incorrect for the wire format.
///
/// Variants are ordered ascending so `>=` expresses capability thresholds
/// (download ≥ `ReadOnly`, upload/lock ≥ `ReadAndWrite`, administration = `Admin`).
///
/// `export_to` without `export` (C3 rule): the file itself is still written
/// only by the gen_ipc_types example (which takes the file name), but the path
/// must use the member-crate base so `SpeleoLinkStatus`'s export test can
/// compute the sibling `./Permission` import - ts-rs diffs the two `EXPORT_TO`
/// strings, and mixing the default `bindings/` base with the member
/// `../../bindings/` base makes that diff unrepresentable (the test panics).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export_to = "../../bindings/Permission.ts")]
pub enum Permission {
    #[serde(rename = "WEB_VIEWER")]
    WebViewer,
    #[serde(rename = "READ_ONLY")]
    ReadOnly,
    #[serde(rename = "READ_AND_WRITE")]
    ReadAndWrite,
    #[serde(rename = "ADMIN")]
    Admin,
}

/// Project visibility (#206/#207). The server's `ProjectVisibility` is an
/// `IntegerChoices` (PRIVATE=0, PUBLIC=1), but the v2 API's `CustomChoiceField`
/// serializes/accepts the **uppercase label** on the wire (verified in
/// `serializer_fields.py` + `test_project_api.py`), so we key on the label, not
/// the integer. Default on create is PRIVATE.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum Visibility {
    #[serde(rename = "PRIVATE")]
    Private,
    #[serde(rename = "PUBLIC")]
    Public,
}

/// A git commit as carried in `latest_commit`. Only the SHA is needed here.
#[derive(Debug, Clone, Deserialize)]
pub struct Commit {
    /// 40-hex git commit SHA.
    pub id: String,
}

/// One commit in a project's **linear** revision history
/// (`GET /api/v2/projects/{id}/revisions/`). The ancestry classifier (#193) needs
/// only `id` / `parent_ids` / `author_email`; the display fields (`authored_date`,
/// `message`) are captured too for the #218 history view. `author_name` is ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct RevisionCommit {
    /// 40-hex git commit SHA.
    pub id: String,
    /// Parent SHAs. Empty for the root commit; exactly one for a linear commit;
    /// more than one only for a merge (which the classifier treats as
    /// non-linear → fail closed).
    #[serde(default)]
    pub parent_ids: Vec<String>,
    /// Commit author's email - compared against our own to tell "my push
    /// landed" from "someone else pushed".
    #[serde(default)]
    pub author_email: String,
    /// [#218] Commit timestamp, for the history view: a UTC instant with
    /// milliseconds (`2026-11-10T08:18:23.123Z`), or, from a SpeleoDB older than
    /// its ISO revision dates, that server's `YYYY/MM/DD HH:MM` wall time in its
    /// zone (America/New_York) with NO offset (sdk#7), which the frontend converts
    /// to an instant (`src/utils/speleoRevisionTime.ts`). See
    /// `client::commit_dates::display_date`. `None` if the server omits it or sends
    /// neither form.
    #[serde(default)]
    pub authored_date: Option<String>,
    /// [#218] Commit message, for the history view. `None` if the server omits it.
    #[serde(default)]
    pub message: Option<String>,
}

/// Response of `GET /api/v2/projects/{id}/revisions/` - the project's linear
/// commit list. The sibling `project` object the server also returns is ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct RevisionHistory {
    #[serde(default)]
    pub commits: Vec<RevisionCommit>,
}

/// [#218] One commit projected for the frontend revision-history view. A read-only
/// slice of [`RevisionCommit`] carrying only what the UI renders (no token, no
/// server-internal fields). The list is server-ordered (newest first).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct SpeleoRevision {
    /// 40-hex git commit SHA.
    pub id: String,
    /// Parent SHAs (empty for the root commit; >1 marks a merge).
    pub parent_ids: Vec<String>,
    /// Commit author's email (may be empty if the server omitted it).
    pub author_email: String,
    /// Commit timestamp: a UTC instant, or an older server's New York wall time
    /// with no zone (see [`RevisionCommit::authored_date`]); `None` when there is
    /// no time to show.
    pub authored_date: Option<String>,
    /// Commit message, or `None` if the server omitted it.
    pub message: Option<String>,
}

impl From<RevisionCommit> for SpeleoRevision {
    fn from(c: RevisionCommit) -> Self {
        Self {
            id: c.id,
            parent_ids: c.parent_ids,
            author_email: c.author_email,
            authored_date: c.authored_date,
            message: c.message,
        }
    }
}

/// What this machine holds LOCALLY for one SpeleoDB project, joined onto the
/// server-side scans (held locks, the GlobalView project list) that know nothing
/// about the local cache. Purely local: no token, no network, no server fields.
///
/// Reported per project id by `speleodb_local_project_states`. Every field is a
/// statement about the local copy only, so one unreadable file costs its own row
/// rather than failing the whole listing - but it says so via [`Self::state_unknown`]
/// instead of impersonating a clean row.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct SpeleoLocalProjectState {
    /// A local `.stalx` for this project exists AND its SpeleoDB link row could be
    /// read. False when the file is missing, corrupt, or carries no link row. Also
    /// false when the file is present but unreadable - that row is distinguished by
    /// [`Self::state_unknown`], which the UI MUST check before treating the row as
    /// "nothing here".
    pub has_local_copy: bool,
    /// The local copy has edits that are not on the server yet
    /// (`speleo_link.has_unpushed_edits`). Always false without a local copy, and
    /// meaningless (not "clean") when [`Self::state_unknown`] is set.
    pub has_unpushed_edits: bool,
    /// When the local copy was last edited (RFC-3339), when known. `None` for a
    /// clean copy, an older file that never recorded one, or no local copy.
    pub last_edit_at: Option<String>,
    /// This is the project currently open in the app, so the reported state came
    /// from the live open database rather than a file on disk.
    pub is_open: bool,
    /// A local `.stalx` for this project IS present but could not be read, so
    /// `has_local_copy` / `has_unpushed_edits` state nothing about it - in
    /// particular NOT that it is free of un-pushed work.
    ///
    /// This exists because the listing fails OPEN (one bad file may cost only its
    /// own row) while the replace guard fails CLOSED on the very same input, and
    /// without this flag the two disagreed silently. The realistic cause is a hot
    /// journal left by a kill or power loss - which a read-only connection cannot
    /// roll back (`SQLITE_READONLY_ROLLBACK`) though a read-write open would - i.e.
    /// precisely the abandoned session the held-lock prompt is raised over. Offering
    /// that row the irreversible "Release lock" because it "looks clean" frees the
    /// project for someone else to push over recoverable work.
    ///
    /// The UI must render this as UNKNOWN (and withhold the destructive choices),
    /// never as clean.
    pub state_unknown: bool,
}

/// The active lock ("mutex") on a project, when one is held.
#[derive(Debug, Clone, Deserialize)]
pub struct ActiveMutex {
    /// Email of the user currently holding the lock.
    pub user: String,
}

/// A SpeleoDB project as returned by `GET /api/v2/projects/` (and `/{id}/`),
/// parsed down to the fields the spike consumes.
#[derive(Debug, Clone, Deserialize)]
pub struct Project {
    /// Server update time, independent of the downloaded cache file.
    #[serde(default)]
    pub modified_date: Option<String>,
    pub id: String,
    pub name: String,
    /// Absent (not null) when the requesting user has no permission.
    #[serde(default)]
    pub permission: Option<Permission>,
    /// Present only while the project is locked.
    #[serde(default)]
    pub active_mutex: Option<ActiveMutex>,
    /// Null on an empty (never-committed) repo.
    #[serde(default)]
    pub latest_commit: Option<Commit>,
    /// Raw project-type label (`ariane`/`compass`/…/`other`). Kept as the wire
    /// string: it is only a display heuristic, and staying a string is robust to
    /// new server types. The project browser includes only STALAGMIGHT projects.
    #[serde(default, rename = "type")]
    pub kind: String,
}

/// IPC-facing summary of a project for the spike dialog's list. Derived from
/// [`Project`]; carries only display fields - never a token or raw API detail.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "../../bindings/SpeleoProjectSummary.ts")]
pub struct SpeleoProjectSummary {
    /// Server update time; absent when server metadata is unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub modified_date: Option<String>,
    pub id: String,
    pub name: String,
    pub permission: Option<Permission>,
    /// Email of the lock holder, if the project is currently locked.
    pub lock_holder: Option<String>,
    /// Raw project-type label (display heuristic only).
    pub type_tag: String,
    /// SHA of the latest commit, if the project has any commits.
    pub latest_commit_sha: Option<String>,
}

/// Access inspection is conservative when the server cannot be verified.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct SpeleoFileAccess {
    pub project: SpeleoProjectSummary,
    pub endpoint: String,
    pub verification: SpeleoAccessVerification,
    pub release_context: Option<SpeleoReleaseContext>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../bindings/")]
pub enum SpeleoAccessVerification {
    Verified,
    Unavailable,
}

/// A transition belongs to this account, project session and local edit version.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct SpeleoReleaseContext {
    pub project_modified_at: String,
    pub session_generation: String,
    pub project_id: String,
    pub endpoint: String,
    pub user_email: String,
}

impl SpeleoProjectSummary {
    pub fn from_project(p: Project) -> Self {
        Self {
            modified_date: p.modified_date,
            id: p.id,
            name: p.name,
            permission: p.permission,
            lock_holder: p.active_mutex.map(|m| m.user),
            type_tag: p.kind,
            latest_commit_sha: p.latest_commit.map(|c| c.id),
        }
    }
}

/// Result of a successful login, returned to the webview. Deliberately does NOT
/// include the token - the token is stored backend-side only (see [`super::auth`]).
#[derive(Debug, Clone, Serialize, TS)]
pub struct SpeleoLoginResult {
    pub user_email: String,
    /// The server this login authenticated against (the resolved endpoint), so the
    /// UI can show "signed in as … at …" immediately without re-querying.
    pub endpoint: String,
}

/// One person with access to the linked SpeleoDB project, offered as a
/// survey-credit suggestion (`speleodb_project_collaborators`). `name` is the
/// account's display name when the server's name lookup found it; `level` is
/// the permission level verbatim, empty for the signed-in user when the
/// permission list did not name them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct SpeleoCollaborator {
    pub name: Option<String>,
    pub email: String,
    pub level: String,
}

/// [#207] Parameters for creating + publishing a local project on SpeleoDB. The
/// server injects `created_by`; `type` is fixed to `stalagmight`; `exclude_geojson` is
/// sent `false` explicitly. `latitude`/`longitude` are both-or-neither.
#[derive(Debug, Clone, Deserialize, TS)]
pub struct SpeleoPublishParams {
    /// Message for the first native-file commit.
    #[serde(default)]
    #[ts(optional)]
    pub message: Option<String>,
    pub name: String,
    pub description: String,
    /// ISO 3166-1 alpha-2 country code; required.
    pub country: String,
    pub visibility: Visibility,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

/// [#207] Result of a successful create-on-SDB publish: the new project's id +
/// the endpoint it was created on, so the webview can re-seed the link state.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SpeleoPublishResult {
    pub project_id: String,
    pub endpoint: String,
    /// The SpeleoDB cache path the freshly-published project now lives at; the
    /// webview reloads it through the normal `open_project` flow (read-only-remote).
    pub local_path: String,
    /// As on [`DoneResult::Released`]: `Some(true)` when the first upload left the
    /// undo history out to fit under the server's size limit (sdk#3). Absent
    /// otherwise, including when that upload did not land.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub undo_history_omitted: Option<bool>,
}

/// Result of pulling + extracting a project, returned to the webview. The
/// frontend then opens `local_path` through the normal `open_project` flow.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SpeleoOpenResult {
    pub local_path: String,
    /// Present only when a failed connection required opening a validated cache.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub offline: Option<bool>,
    /// Preserve unsynced work when seeding the frontend's offline link state.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub has_unpushed_edits: Option<bool>,
    /// Present only when the cached copy opened read-only without the server's
    /// say: no usable SpeleoDB session, or another account's un-uploaded work.
    /// The frontend must not try to take the lock.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub read_only: Option<SpeleoReadOnlyOpen>,
    /// With [`SpeleoReadOnlyOpen::OtherAccount`]: the account (email) the copy is
    /// recorded for, so the frontend can say whose work it holds.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub other_owner: Option<String>,
}

/// Why an open returned the cached copy read-only, without the server's say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum SpeleoReadOnlyOpen {
    /// Nobody is signed in: the copy opened with no network call at all.
    SignedOut,
    /// The server rejected the saved token during this open. The token was dropped,
    /// so the app is now signed out, and the copy opened as for a signed-out user.
    SessionExpired,
    /// The copy's owner record names another account and it holds un-uploaded
    /// edits: that account's work, which this account may look at (and save a
    /// copy of) but never take the lock over, edit or upload.
    OtherAccount,
}

/// Outcome of `speleodb_start_editing` (#194): the result of acquiring the write
/// lock and reconciling the open project against the server. A discriminated
/// union (`kind`) the webview branches on. Errors (no permission, anomalies,
/// network) surface through the `Err(SpeleoErrorDto)` channel instead.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../bindings/")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StartEditingResult {
    /// Lock acquired; the local copy already matched the server tip. Editing is
    /// now enabled.
    Editing,
    /// Lock acquired; the server had advanced with no local edits, so we
    /// fast-forwarded (re-pulled + reloaded the project) and enabled editing. The
    /// webview should refresh its view from the reloaded `local_path`.
    FastForwarded { local_path: String },
    /// The server diverged from our base AND we hold un-pushed local edits:
    /// editing is NOT enabled. A lock this call FRESHLY acquired was released;
    /// one it inherited (a resumed session - the held-lock prompt's "Continue
    /// editing") is deliberately kept, since handing it back would free the
    /// project while the un-pushed work is still only local. Either way the
    /// user must reconcile (the divergence prompt is #198, next batch).
    DivergenceNeeded,
    /// Someone else holds the write lock; `email` is the holder when known.
    LockedBy { email: Option<String> },
}

/// Outcome of `speleodb_done` (#197): push the open project's edits, then release
/// the lock and return to read-only.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../bindings/")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DoneResult {
    /// Edits pushed (or there was nothing to push); the lock was released and the
    /// project is read-only again.
    ///
    /// This is the ONLY arm that means "the local work is on the server", so it is
    /// the only one the frontend may clear its `hasUnpushedEdits` on. See
    /// [`Self::NotEditing`] for the case that used to be folded in here.
    Released {
        /// `Some(true)` when the uploaded copy left out the undo history to fit
        /// under the server's size limit (sdk#3; see
        /// [`super::client::SERVER_UPLOAD_LIMIT_BYTES`]). The survey data was
        /// uploaded in full; the new revision just cannot be unwound step by step.
        /// Absent otherwise.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        undo_history_omitted: Option<bool>,
    },
    /// There was no edit session to finish: the backend's edit gate was already
    /// closed, so no lock was held, nothing was pushed and nothing was released.
    ///
    /// Distinct from [`Self::Released`] because the two are NOT interchangeable
    /// when the project is dirty. The gate can be closed while
    /// `has_unpushed_edits` is still set - a 401 auto-drop runs `speleodb_forget`,
    /// which closes the gate but deliberately leaves the dirty bit alone - and
    /// reporting that as `Released` told the frontend the work had landed, which
    /// cleared its `hasUnpushedEdits` and disarmed the quit guard over edits that
    /// were never uploaded. The frontend must leave its dirty state untouched on
    /// this arm and route the user back through Start editing (re-login first if
    /// the token is gone).
    NotEditing,
    /// The server diverged (a stranger commit between our base and the tip):
    /// nothing was pushed, the lock + dirty bit are retained for the user to
    /// reconcile (#198).
    DivergenceNeeded,
    /// The push outcome was ambiguous (no confirmed server commit): the dirty bit
    /// AND the lock are retained so a later retry recovers via ancestry (#193) -
    /// no edits are lost.
    PushUnconfirmed,
}

/// Outcome of `speleodb_update` (#197): a read-only refresh of the open project
/// against the server (no lock taken).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../bindings/")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UpdateResult {
    /// Already at the server tip - nothing to do.
    UpToDate,
    /// Fast-forwarded to the server tip (re-pulled + reloaded the project).
    FastForwarded { local_path: String },
    /// The server diverged from our base AND we hold un-pushed local edits - the
    /// refresh cannot proceed without reconciliation (#198/#199, next batch).
    DivergenceNeeded,
}

/// Current backend session status, so the dialog can restore its view when
/// reopened instead of forcing a re-login: the cached token lives in
/// `AppState` for the whole app run, but the dialog's React state resets on each
/// remount. Carries no token - only whether one is held and for whom.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SpeleoSessionStatus {
    pub logged_in: bool,
    pub user_email: Option<String>,
    pub endpoint: Option<String>,
}

/// IPC-facing status of the currently open project's SpeleoDB link, so the
/// frontend action shell (#204) can render the read-only/editing indicator and
/// gate its controls on ANY load - including a reopen from Recent Projects,
/// where the open flow's [`SpeleoProjectSummary`] isn't in hand.
///
/// The cheap local fields (`is_remote`, `edit_enabled`, `has_unpushed_edits`)
/// come from the persisted `speleo_link` row + the transient edit gate. The
/// network fields (`permission`, `lock_holder`) are best-effort from a single
/// `get_project`; they degrade to `None` when logged out / offline so the local
/// truth (which the read-only gate hangs on) always returns. Carries no token.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct SpeleoLinkStatus {
    /// True when the open project is linked to a SpeleoDB project (a `speleo_link`
    /// row exists). False for a local project or when no project is open.
    pub is_remote: bool,
    /// The backend edit gate: true when mutations are allowed (a local project,
    /// or a remote one with the write lock held). False for a remote project
    /// opened read-only.
    pub edit_enabled: bool,
    /// The command-driven dirty bit: un-pushed remote edits are pending.
    pub has_unpushed_edits: bool,
    /// [#303] A chain step has moved this copy off the server tip: an Unwind (or a forward
    /// step) is mid-walk.
    ///
    /// The two are not the same thing, and conflating them is how a look at history turned
    /// into a published revert: the close prompt read the dirty bit, said "you have
    /// unpushed changes", and offered to push as its primary action. Pushing here does not
    /// save anyone's work; it records the OLD survey over the newer one for every
    /// collaborator.
    ///
    /// True exactly while the Unwind chain pointer stands, since any forward command drops
    /// it (`speleodb_forget_unwind_position`) - so the moment the user really does edit,
    /// this goes false and the ordinary "unpushed changes" wording is correct again.
    ///
    /// RAW rather than combined with the dirty bit, which it was at first and could not
    /// stay: a step onto the tip leaves the copy CLEAN, so the composite was false at the
    /// post-step status refresh, and the Undo that follows marks dirty optimistically
    /// without asking the backend anything. The frontend therefore has to be able to
    /// compose the two itself, which means holding this half unmixed.
    pub unwound: bool,
    /// This user's permission tier on the project. Best-effort - `None` when
    /// logged out / offline / the lookup failed.
    pub permission: Option<Permission>,
    /// Email of the current lock holder, if any. Best-effort - `None` when
    /// unlocked / logged out / the lookup failed.
    pub lock_holder: Option<String>,
    /// How the local copy relates to the server tip - drives the action button's
    /// "remote has an update" / "conflict" states (T2c). Best-effort over the
    /// revision topology; `Unknown` when logged out / offline / the lookup failed.
    pub remote_state: RemoteState,
    /// The project is linked but the server definitively refused this user access
    /// to it (a 403 on `get_project`) - e.g. a managed `.stalx` shared as a bare
    /// file to someone who is not a collaborator. Distinct from a transient
    /// offline miss (which leaves `permission: None` but this `false`), so the UI
    /// can offer converting it to an unmanaged local file rather than nagging to
    /// retry. Never set on a plain 401 (that is a dead token → re-login).
    pub access_denied: bool,
    /// An UNEXPECTED server error hit while probing the link (a status that is
    /// neither the handled 401 dead-token / 403 access-denied nor a transient
    /// offline miss - e.g. a 404 for a deleted project, a 422, or a 5xx). Carries
    /// the humanized detail so the action button can show a "?" state and open a
    /// dialog explaining what SpeleoDB said, instead of silently degrading to
    /// `Unknown`. `None` on success / offline / the handled auth cases.
    pub server_error: Option<SpeleoErrorDto>,
    /// [#303] The project's undo-history policy - what its next push will carry.
    pub undo_mode: crate::models::UndoMode,
    /// [#218] The open remote project's SpeleoDB project id, so the in-project menu
    /// can open the revision-history view for it (the FE otherwise has no way to name
    /// which remote project is open). `None` for a local project / none open.
    pub project_id: Option<String>,
}

/// The local copy's relationship to the server tip (the reconcile verdict,
/// projected for the UI). Drives the action button (T2c): `RemoteAhead` → pull,
/// `Conflict` → resolve. Derived from [`super::sync::reconcile_verdict`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "../../bindings/")]
#[serde(rename_all = "snake_case")]
pub enum RemoteState {
    /// Local BASE == server tip (nothing to pull).
    UpToDate,
    /// The server advanced and we can fast-forward cleanly (no local edits block it).
    RemoteAhead,
    /// The server diverged from BASE and we hold un-pushed edits - needs reconcile.
    Conflict,
    /// Not determined (logged out / offline / lookup failed).
    Unknown,
}

/// Outcome of `speleodb_push` (T2d): push the open project's edits **without**
/// releasing the lock (unlike [`DoneResult`], which releases). The lock is
/// retained in every arm so editing continues.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../bindings/")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PushResult {
    /// Edits pushed (or nothing to push); the lock is retained.
    ///
    /// The ONLY arm that means "the local work is on the server" - see
    /// [`DoneResult::Released`] for the same distinction on the release verb.
    Pushed {
        /// As on [`DoneResult::Released`]: `Some(true)` when the upload left out
        /// the undo history to fit under the server's size limit (sdk#3).
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        undo_history_omitted: Option<bool>,
    },
    /// There was no edit session to push from: the backend's edit gate was already
    /// closed, so no lock was held and nothing was uploaded. See
    /// [`DoneResult::NotEditing`] - same hazard, same frontend obligation (leave
    /// the dirty state alone; do NOT treat this as a successful push).
    NotEditing,
    /// The server diverged: nothing pushed, lock + dirty bit retained to reconcile.
    DivergenceNeeded,
    /// Ambiguous push (no confirmed server commit): dirty bit + lock retained so a
    /// retry recovers via ancestry (#193).
    PushUnconfirmed,
}

/// [Track A] #186/D-B2: the structured error the SpeleoDB IPC commands return.
///
/// Replaces the spike's `String`-flattened error at the IPC boundary so the
/// frontend branches on the typed [`SpeleoDbAction`] (`action`) instead of
/// scraping message text. `message` is DISPLAY-only (from
/// `client::parse_error_message` / the error's `Display`); `status` is the HTTP
/// code for a `Status` error and `None` for transport / local errors. Carries no
/// token (the no-token-leak guard pins this).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct SpeleoErrorDto {
    pub message: String,
    pub action: SpeleoDbAction,
    pub status: Option<u16>,
    /// [#270] Set ONLY for a transport failure, classifying why the server was
    /// never reached. It is the discriminant the frontend branches on to render a
    /// translated "could not reach SpeleoDB at <endpoint>" line: `message` still
    /// carries the raw cause (an OS errno such as "Invalid argument"), which is
    /// for the log, not for the user. `None` on every non-transport error.
    pub transport: Option<TransportCause>,
    /// Set ONLY when a re-pull was REFUSED because the local copy still holds
    /// edits the server has never seen (`action == UnpushedLocalEdits`). It is
    /// what the frontend needs to offer the binary choice; `None` on every other
    /// error, which is what makes it a sound discriminant.
    pub unpushed_local: Option<UnpushedLocalEdits>,
    /// Present when a downloaded native file requires a newer schema.
    pub newer_schema: Option<crate::models::NewerSchema>,
    /// Set ONLY when the endpoint policy refused the server address before any
    /// request (a public `http://` server, or an unusable address), so the
    /// frontend can say which setting to change. Absent on every other error.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub endpoint_refusal: Option<EndpointRefusal>,
}

/// The local work a refused re-pull would have destroyed, carried on
/// [`SpeleoErrorDto::unpushed_local`] so the frontend can name it and act on it
/// without a second round-trip.
///
/// The two branches the frontend offers, and exactly what each calls:
///
/// - **Cancel** - leave the cached copy and its un-pushed edits intact.
/// - **Take remote and discard local** - call
///   `speleodb_open_project_discarding_local` with the same arguments the refused
///   `speleodb_open_project` was given. That is the abandoning verb: it performs
///   the very replace this error refused.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct UnpushedLocalEdits {
    /// The SpeleoDB project whose local copy holds the un-pushed edits.
    pub project_id: String,
    /// The `.stalx` that holds the protected edits.
    pub local_path: String,
    /// When it was last edited (RFC-3339), when known. `None` for an older copy
    /// that never recorded one.
    pub last_edit_at: Option<String>,
    /// The account (email) the copy's owner record names, when that is NOT the
    /// signed-in account: the edits are that account's work. The prompt then says
    /// so, and its "open the local copy" answer opens it read-only.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub other_owner: Option<String>,
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
mod tests {
    use super::{
        DoneResult, Permission, Project, PushResult, RemoteState, RevisionCommit, RevisionHistory,
        SpeleoDbAction, SpeleoErrorDto, SpeleoLinkStatus, SpeleoLoginResult, SpeleoOpenResult,
        SpeleoProjectSummary, SpeleoPublishResult, SpeleoReadOnlyOpen, SpeleoRevision,
        SpeleoSessionStatus, StartEditingResult, UpdateResult,
    };

    #[test]
    fn ipc_dtos_never_serialize_a_token() {
        // Load-bearing invariant: no IPC-facing DTO carries the auth token (it
        // lives only in the backend TokenStore). Pin it so a future field that
        // names or holds a token can't silently break "token never in IPC".
        let login = serde_json::to_string(&SpeleoLoginResult {
            user_email: "u@x".to_owned(),
            endpoint: "https://x/".to_owned(),
        })
        .expect("serialize login result");
        let status = serde_json::to_string(&SpeleoSessionStatus {
            logged_in: true,
            user_email: Some("u@x".to_owned()),
            endpoint: Some("https://x/".to_owned()),
        })
        .expect("serialize session status");
        let summary = serde_json::to_string(&SpeleoProjectSummary {
            modified_date: None,
            id: "11111111-1111-1111-1111-111111111111".to_owned(),
            name: "Cave".to_owned(),
            permission: Some(Permission::ReadOnly),
            lock_holder: None,
            type_tag: "ariane".to_owned(),
            latest_commit_sha: None,
        })
        .expect("serialize project summary");
        let open = serde_json::to_string(&SpeleoOpenResult {
            local_path: "/cache/project.stalx".to_owned(),
            offline: None,
            has_unpushed_edits: None,
            read_only: Some(SpeleoReadOnlyOpen::SessionExpired),
            other_owner: None,
        })
        .expect("serialize open result");
        // [Track A] #186/D-B2: the structured error DTO must also never carry a
        // token. (The message deliberately avoids the substring "token" so the
        // guard checks the field SHAPE, not an incidental word in the display
        // text - the real invariant is that no field name/value is the token.)
        let error = serde_json::to_string(&SpeleoErrorDto {
            message: "SpeleoDB returned HTTP 401: not authenticated".to_owned(),
            action: SpeleoDbAction::ReLogin,
            status: Some(401),
            transport: None,
            unpushed_local: None,
            newer_schema: None,
            endpoint_refusal: None,
        })
        .expect("serialize error dto");
        // The single-writer hand-off DTOs (#194/#197) must also never carry a token
        // - serialize the field-bearing variants so a future field is caught.
        let start = serde_json::to_string(&StartEditingResult::FastForwarded {
            local_path: "/cache/project.stalx".to_owned(),
        })
        .expect("serialize start-editing result");
        let locked = serde_json::to_string(&StartEditingResult::LockedBy {
            email: Some("holder@example.com".to_owned()),
        })
        .expect("serialize locked-by result");
        let done = serde_json::to_string(&DoneResult::Released {
            undo_history_omitted: Some(true),
        })
        .expect("serialize done result");
        let update = serde_json::to_string(&UpdateResult::FastForwarded {
            local_path: "/cache/project.stalx".to_owned(),
        })
        .expect("serialize update result");
        // #204: the link-status DTO is authed-derived (permission + lock holder)
        // but must likewise never carry the token.
        let link_status = serde_json::to_string(&SpeleoLinkStatus {
            is_remote: true,
            edit_enabled: false,
            has_unpushed_edits: false,
            unwound: false,
            permission: Some(Permission::ReadAndWrite),
            lock_holder: Some("holder@example.com".to_owned()),
            remote_state: RemoteState::RemoteAhead,
            access_denied: false,
            server_error: None,
            undo_mode: crate::models::UndoMode::Full,
            project_id: Some("11111111-1111-1111-1111-111111111111".to_owned()),
        })
        .expect("serialize link status");
        let push = serde_json::to_string(&PushResult::PushUnconfirmed).expect("serialize push");

        for json in [
            &login,
            &status,
            &summary,
            &open,
            &error,
            &start,
            &locked,
            &done,
            &update,
            &link_status,
            &push,
        ] {
            assert!(
                !json.to_ascii_lowercase().contains("token"),
                "IPC DTO must not expose a token field: {json}"
            );
        }
    }

    #[test]
    fn project_parses_full_v2_payload() {
        // Extra server fields (creation_date, message, the editable metadata) are
        // ignored; the modelled fields parse.
        let json = r#"{
            "id": "11111111-1111-1111-1111-111111111111",
            "name": "Test Cave",
            "permission": "READ_AND_WRITE",
            "active_mutex": {"user": "holder@example.com", "creation_date": "2026-01-01T00:00:00Z"},
            "latest_commit": {"id": "0123456789abcdef0123456789abcdef01234567", "message": "init"},
            "type": "ariane",
            "exclude_geojson": true,
            "description": "A cave",
            "country": "US",
            "visibility": "PRIVATE",
            "latitude": -45.367573,
            "longitude": 100.423897
        }"#;
        let p: Project = serde_json::from_str(json).expect("valid project JSON");
        assert_eq!(p.id, "11111111-1111-1111-1111-111111111111");
        assert_eq!(p.name, "Test Cave");
        assert_eq!(p.permission, Some(Permission::ReadAndWrite));
        assert_eq!(
            p.active_mutex.as_ref().map(|m| m.user.as_str()),
            Some("holder@example.com")
        );
        assert_eq!(
            p.latest_commit.as_ref().map(|c| c.id.as_str()),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(p.kind, "ariane"); // #[serde(rename = "type")]

        // The display projection carries the lock holder + commit SHA across.
        let summary = SpeleoProjectSummary::from_project(p);
        assert_eq!(summary.lock_holder.as_deref(), Some("holder@example.com"));
        assert_eq!(
            summary.latest_commit_sha.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(summary.type_tag, "ariane");
    }

    #[test]
    fn project_tolerates_omitted_and_null_optionals() {
        // No permission (the server omits it when the user has none), no
        // active_mutex, no latest_commit, no type.
        let omitted: Project =
            serde_json::from_str(r#"{"id": "a", "name": "Bare"}"#).expect("minimal project JSON");
        assert_eq!(omitted.permission, None);
        assert!(omitted.active_mutex.is_none());
        assert!(omitted.latest_commit.is_none());
        assert_eq!(omitted.kind, "");

        // Explicit nulls deserialize to None just like absence.
        let nulled: Project = serde_json::from_str(
            r#"{"id": "b", "name": "N", "permission": null, "latest_commit": null, "type": "other"}"#,
        )
        .expect("nulled project JSON");
        assert_eq!(nulled.permission, None);
        assert!(nulled.latest_commit.is_none());
        assert_eq!(nulled.kind, "other");
    }

    #[test]
    fn permission_labels_round_trip() {
        // The wire format is the uppercase label string (not the integer the
        // issues describe); pin every variant both directions.
        for (variant, label) in [
            (Permission::WebViewer, "\"WEB_VIEWER\""),
            (Permission::ReadOnly, "\"READ_ONLY\""),
            (Permission::ReadAndWrite, "\"READ_AND_WRITE\""),
            (Permission::Admin, "\"ADMIN\""),
        ] {
            assert_eq!(serde_json::to_string(&variant).expect("serialize"), label);
            assert_eq!(
                serde_json::from_str::<Permission>(label).expect("deserialize"),
                variant
            );
        }
    }

    #[test]
    fn permission_orders_by_capability() {
        // `>=` is used as a capability threshold (download ≥ ReadOnly, upload/lock
        // ≥ ReadAndWrite, …), so the ascending ordering must hold.
        assert!(Permission::WebViewer < Permission::ReadOnly);
        assert!(Permission::ReadOnly < Permission::ReadAndWrite);
        assert!(Permission::ReadAndWrite < Permission::Admin);
    }

    #[test]
    fn revision_commit_deserializes_without_display_fields() {
        // #218: the ancestry classifier's payloads omit authored_date/message; they
        // must still parse (serde default -> None), never break the history read.
        let c: RevisionCommit =
            serde_json::from_str(r#"{"id": "abc", "parent_ids": ["p"], "author_email": "me@x"}"#)
                .expect("minimal revision JSON");
        assert_eq!(c.id, "abc");
        assert_eq!(c.parent_ids, vec!["p".to_owned()]);
        assert_eq!(c.author_email, "me@x");
        assert!(c.authored_date.is_none());
        assert!(c.message.is_none());
    }

    #[test]
    fn revision_history_projects_to_dtos_in_order() {
        // #218: the full server-ordered commit list projects to SpeleoRevision DTOs,
        // capturing the display fields (date/message) when present and preserving order.
        let history: RevisionHistory = serde_json::from_str(
            r#"{"commits": [
                {"id": "tip", "parent_ids": ["root"], "author_email": "me@x",
                 "authored_date": "2026-07-03T10:00:00+00:00", "message": "second"},
                {"id": "root", "parent_ids": [], "author_email": "you@x", "message": "first"}
            ]}"#,
        )
        .expect("revision history JSON");
        let dtos: Vec<SpeleoRevision> = history
            .commits
            .into_iter()
            .map(SpeleoRevision::from)
            .collect();
        assert_eq!(dtos.len(), 2);
        assert_eq!(dtos[0].id, "tip");
        assert_eq!(
            dtos[0].authored_date.as_deref(),
            Some("2026-07-03T10:00:00+00:00")
        );
        assert_eq!(dtos[0].message.as_deref(), Some("second"));
        assert_eq!(dtos[1].id, "root");
        assert!(dtos[1].parent_ids.is_empty());
        assert!(dtos[1].authored_date.is_none());
    }

    #[test]
    fn speleo_revision_never_serializes_a_token() {
        // Mirror the IPC token invariant for the new DTO.
        let json = serde_json::to_string(&SpeleoRevision {
            id: "abc".to_owned(),
            parent_ids: vec![],
            author_email: "me@x".to_owned(),
            authored_date: None,
            message: None,
        })
        .expect("serialize revision");
        assert!(!json.to_lowercase().contains("token"));
    }

    #[test]
    fn a_landed_push_names_an_omitted_undo_history_only_when_it_happened() {
        // sdk#3: the frontend keys on `kind` alone, so the ordinary success keeps
        // its exact wire shape; the notice field appears only when set.
        let plain = |v: &serde_json::Value| v.to_string();
        let released = serde_json::to_value(DoneResult::Released {
            undo_history_omitted: None,
        })
        .expect("serialize");
        assert_eq!(plain(&released), r#"{"kind":"released"}"#);
        let pushed = serde_json::to_value(PushResult::Pushed {
            undo_history_omitted: Some(true),
        })
        .expect("serialize");
        assert_eq!(
            plain(&pushed),
            r#"{"kind":"pushed","undo_history_omitted":true}"#
        );
        let publish = |undo_history_omitted| SpeleoPublishResult {
            project_id: "11111111-1111-1111-1111-111111111111".to_owned(),
            endpoint: "https://speleodb.example".to_owned(),
            local_path: "/cache/project.stalx".to_owned(),
            undo_history_omitted,
        };
        let quiet = serde_json::to_value(publish(None)).expect("serialize");
        assert!(quiet.get("undo_history_omitted").is_none());
        let noted = serde_json::to_value(publish(Some(true))).expect("serialize");
        assert_eq!(noted["undo_history_omitted"], serde_json::json!(true));
    }
}
