//! Serde DTOs for the SpeleoDB v2 JSON API (read-only spike, #183), plus the
//! IPC-facing summary types returned to the webview.
//!
//! Wire format: the `/api/v2/` endpoints return **bare** JSON (the serializer
//! output directly). Only the deprecated `/api/v1/` alias is wrapped in a
//! `{data, success, …}` envelope by `DRFWrapResponseMiddleware`, which
//! early-returns unless `/api/v1/` is in the path. So a project list is a bare
//! JSON array and a project detail is a bare JSON object - no envelope to peel.
//!
//! Only the fields the spike needs are modelled; serde ignores the rest. Fields
//! the server *omits* when empty (notably `permission` when the user has no
//! access) are `Option<_>` with `#[serde(default)]`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

// [Track A] #186/D-B2: the typed action carried on the IPC error DTO.
use super::contract::{SpeleoDbAction, TransportCause};
use crate::models::ImportReceipt;

/// Project permission level.
///
/// The server serializes the **label string** (e.g. `"READ_AND_WRITE"`), not the
/// numeric `PermissionLevel` enum - verified in `serializers/project.py`
/// (`get_permission` → `str(...level_label)`). Issues #188/#203 describe this as
/// an integer; that is incorrect for the wire format.
///
/// Ordered by capability so `>=` expresses thresholds (download >= `ReadOnly`,
/// upload/lock >= `ReadAndWrite`, delete/collab = `Admin`) - see [`Self::rank`].
///
/// `export_to` without `export` (C3 rule): the file itself is still written
/// only by the gen_ipc_types example (which takes the file name), but the path
/// must use the member-crate base so `SpeleoLinkStatus`'s export test can
/// compute the sibling `./Permission` import - ts-rs diffs the two `EXPORT_TO`
/// strings, and mixing the default `bindings/` base with the member
/// `../../bindings/` base makes that diff unrepresentable (the test panics).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
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
    /// [#576 B16] A tier the server added after this build shipped. Without it one
    /// project (or one collaborator grant) carrying such a label failed the WHOLE
    /// project list / roster, because each is parsed as one value. Ranked BELOW
    /// every known tier ([`Self::rank`]): every capability check (`>= ReadOnly`,
    /// `>= ReadAndWrite`, `== Admin`) denies it, the fail-safe direction. Never
    /// sent back to the server (the permission writes refuse it). Declared last
    /// only because serde requires the `other` catch-all to be.
    #[serde(rename = "UNKNOWN")]
    #[serde(other)]
    Unknown,
}

impl Permission {
    /// The capability rank `Ord` compares: an unknown tier grants less than the
    /// lowest known one.
    const fn rank(self) -> u8 {
        match self {
            Permission::Unknown => 0,
            Permission::WebViewer => 1,
            Permission::ReadOnly => 2,
            Permission::ReadAndWrite => 3,
            Permission::Admin => 4,
        }
    }
}

impl PartialOrd for Permission {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Permission {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.rank().cmp(&other.rank())
    }
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
    /// [#576 B16] A visibility the server added after this build shipped: kept as a
    /// value instead of failing the project list, shown as unknown, and never sent
    /// back (the metadata PATCH and the create body leave it out).
    #[serde(rename = "UNKNOWN")]
    #[serde(other)]
    Unknown,
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
    /// [#218] Commit timestamp (RFC-3339), for the history view. `None` if the
    /// server omits it.
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

/// [#303] Which revision the next "Unwind to previous SpeleoDB version" would load.
///
/// `is_current` distinguishes the two shapes of the same action, which the confirmation
/// dialog has to word differently:
///
/// - `true` - the target is the commit the working copy ALREADY equals (the first step
///   after a push). Nothing moves; what comes back is the undo history that produced this
///   state, so the user can keep stepping backwards through their own pushed operations.
/// - `false` - the target is an ancestor, so the working copy DOES move back, and any
///   later push lands that older content as a new forward commit.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct SpeleoUnwindTarget {
    /// 40-hex commit SHA to check out.
    pub hexsha: String,
    /// The target is the working copy's own commit rather than an ancestor.
    pub is_current: bool,
    /// [#303] Going ahead would throw away un-pushed work: the copy is dirty and no
    /// Unwind chain pointer accounts for it, so the swap replaces edits the user never
    /// pushed (and the redo steps that would have replayed them).
    ///
    /// The dialog raises its red warning on this, and only a caller that passes the
    /// matching acknowledgement gets past the backend guard - the frontend is told what
    /// will happen, it is not trusted to decide it.
    pub discards_unpushed: bool,
}

/// [#303] Which revision a FORWARD chain step would check out - the mirror of
/// [`SpeleoUnwindTarget`], walking back up toward BASE.
///
/// Deliberately carries no `discards_unpushed`. A forward step is only defined while a
/// chain pointer exists, and `unwind_discards_unpushed` is false whenever one does, so the
/// field could only ever be `false` - a permanently dead warning that would read as though
/// the case had been considered and could fire. An edit drops the pointer, which is what
/// removes this direction entirely rather than warning about it.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct SpeleoRewindTarget {
    /// 40-hex commit SHA to check out.
    pub hexsha: String,
    /// This step lands back ON BASE, i.e. returns the copy to the latest version, so the
    /// confirmation can say that rather than naming a revision.
    pub is_latest: bool,
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
    /// Commit timestamp (RFC-3339), or `None` if the server omitted it.
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
    /// new server types. The dump classifier is the authoritative openability check.
    #[serde(default, rename = "type")]
    pub kind: String,
    // [Track A] #185/D-A-methods: the server's GeoJSON-export toggle. Parsed (not
    // ignored) so the full v2 payload round-trips; defaults false when omitted.
    // Consumed by #200 (surfaced via SpeleoLinkStatus to drive the rendering-disabled
    // banner + Enable-rendering PATCH).
    #[serde(default)]
    pub exclude_geojson: bool,
    // [#206] editable metadata, parsed so the Project Settings tab can prefill the
    // current server truth. `description`/`country` are required server-side (always
    // present); `visibility` is PRIVATE/PUBLIC; `latitude`/`longitude` are JSON
    // numbers (the serializer's `format_coordinate` rounds to a float) and are
    // OMITTED when null (both-or-neither).
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) country: Option<String>,
    #[serde(default)]
    pub(crate) visibility: Option<Visibility>,
    #[serde(default)]
    pub(crate) latitude: Option<f64>,
    #[serde(default)]
    pub(crate) longitude: Option<f64>,
}

/// One active per-user permission grant on a project, from the `permissions[]`
/// array of `GET /api/v2/projects/{id}/permissions/user/` (#215). Only the fields
/// the collaborator UI needs are modelled; serde ignores `creation_date` etc.
#[derive(Debug, Clone, Deserialize)]
pub struct UserPermission {
    /// Collaborator email.
    pub user: String,
    pub level: Permission,
}

/// Wrapper for `GET .../permissions/user/` -> `{project, permissions: [...]}`.
/// Only the permissions array is consumed (the sibling `project` is ignored).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct UserPermissionList {
    #[serde(default)]
    pub(crate) permissions: Vec<UserPermission>,
}

/// One active per-team permission grant, from `GET .../permissions/team/` (#215).
/// `team` is the team UUID (resolved to a display name against the caller's team
/// list, falling back to the raw UUID).
#[derive(Debug, Clone, Deserialize)]
pub struct TeamPermission {
    pub team: String,
    pub level: Permission,
}

/// Wrapper for `GET .../permissions/team/` -> `{project, permissions: [...]}`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TeamPermissionList {
    #[serde(default)]
    pub(crate) permissions: Vec<TeamPermission>,
}

/// A survey team the authenticated user belongs to, from the bare-array
/// `GET /api/v2/teams/` (#215). Only id + name are consumed (the name map for the
/// team roster + the add-team picker).
#[derive(Debug, Clone, Deserialize)]
pub struct Team {
    pub id: String,
    pub name: String,
}

/// One autocomplete suggestion from the bare-array `GET /api/v2/user/autocomplete/`
/// (#215): a user matched by email/name substring. The server pre-formats `label`
/// as "Name <email>"; `name` is ignored (folded into `label`).
#[derive(Debug, Clone, Deserialize)]
pub struct UserAutocomplete {
    pub email: String,
    pub label: String,
}

/// IPC-facing summary of a project for the spike dialog's list. Derived from
/// [`Project`]; carries only display fields - never a token or raw API detail.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SpeleoProjectSummary {
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

impl SpeleoProjectSummary {
    pub fn from_project(p: Project) -> Self {
        Self {
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
    /// True when the token was stored in the OS keychain; false means a
    /// session-only in-memory fallback (e.g. headless CI with no secret service).
    ///
    /// Kept alongside `storage` rather than replaced by it: this bool answers
    /// "durable?" for existing callers, while `storage` answers "where?".
    pub keychain_backed: bool,
    /// Where the API key actually ended up. `keychain_backed` cannot distinguish
    /// the plaintext file from session-only memory, and the two have very
    /// different consequences for the user.
    pub storage: TokenStorage,
    /// What this login taught us about the OS keychain.
    pub keychain: KeychainReport,
    /// The keychain failed during THIS login and the user has not already told us
    /// where to put the key instead - so the frontend should ask, now, while the
    /// failure is in context. False when the keychain worked, and false when the
    /// user has already declined it (we are not re-litigating a settled choice).
    pub needs_storage_choice: bool,
    /// [#576 FZ-SDB-3-D1] This login replaced a live session for another account or
    /// server, and ran logout's hand-back for it first: the old session's held lock
    /// was released best-effort with its token, the edit gate was shut and the
    /// claim forgotten. The frontend mirrors that locally, as it does a logout.
    pub replaced_session: bool,
}

/// Where the API key is being kept. Mirrors `auth::TokenTier` across the IPC
/// boundary.
#[derive(Debug, Clone, Copy, Serialize, TS, PartialEq, Eq)]
#[ts(export, export_to = "../../bindings/")]
#[serde(rename_all = "snake_case")]
pub enum TokenStorage {
    /// The OS keychain: survives a restart, encrypted by the platform.
    Keychain,
    /// A `0o600` file under the app config dir: survives a restart, but readable
    /// by anything running as this user.
    Plaintext,
    /// This session only: gone when the app closes.
    Memory,
}

/// What is known about the OS keychain - strictly what earlier access taught us,
/// never the result of a probe. Probing a locked keyring is itself what raises a
/// desktop unlock prompt, so `NotAttempted` is a real answer we show rather than
/// go looking.
#[derive(Debug, Clone, Copy, Serialize, TS, PartialEq, Eq)]
#[ts(export, export_to = "../../bindings/")]
#[serde(rename_all = "snake_case")]
pub enum KeychainReport {
    /// Nothing has touched it yet (no login this session, no rehydrate).
    NotAttempted,
    /// A read or write succeeded.
    Available,
    /// A read or write failed - absent, locked, or refused.
    Unavailable,
    /// The user declined it; we are deliberately leaving it alone until they sign
    /// in again.
    Declined,
    /// Switched off in Settings. Reported apart from [`Self::Declined`] because the
    /// retry verb revokes a decline but cannot bypass this, so a frontend that
    /// cannot tell them apart offers a control that does nothing.
    Disabled,
}

// The storage layer and the IPC surface keep separate enums on purpose, so the
// tiers can be reworked without breaking the webview contract. These are the two
// bridges, kept next to the IPC types they produce.

impl From<super::auth::TokenTier> for TokenStorage {
    fn from(tier: super::auth::TokenTier) -> Self {
        match tier {
            super::auth::TokenTier::Keychain => Self::Keychain,
            super::auth::TokenTier::Plaintext => Self::Plaintext,
            super::auth::TokenTier::Memory => Self::Memory,
        }
    }
}

impl From<super::auth::KeychainState> for KeychainReport {
    fn from(state: super::auth::KeychainState) -> Self {
        match state {
            super::auth::KeychainState::NotAttempted => Self::NotAttempted,
            super::auth::KeychainState::Available => Self::Available,
            super::auth::KeychainState::Unavailable => Self::Unavailable,
            super::auth::KeychainState::Declined => Self::Declined,
            super::auth::KeychainState::Disabled => Self::Disabled,
        }
    }
}

/// [#206] The editable SpeleoDB project metadata, returned to the webview by
/// `speleodb_get_project_metadata` (to prefill the Project Settings tab) and by
/// `speleodb_patch_project` (the post-edit server truth). `description`/`country`
/// are required server-side; `latitude`/`longitude` are the optional both-or-neither
/// location pin.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SpeleoProjectMetadata {
    pub name: String,
    pub description: String,
    /// ISO 3166-1 alpha-2 country code (e.g. "US").
    pub country: String,
    pub visibility: Visibility,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub exclude_geojson: bool,
}

impl SpeleoProjectMetadata {
    /// Project the SDB [`Project`] DTO down to the editable metadata surface,
    /// defaulting the required-but-Option-parsed fields (they are always present
    /// on a real GET/PATCH response).
    pub fn from_project(p: &Project) -> Self {
        Self {
            name: p.name.clone(),
            description: p.description.clone().unwrap_or_default(),
            country: p.country.clone().unwrap_or_default(),
            visibility: p.visibility.unwrap_or(Visibility::Private),
            latitude: p.latitude,
            longitude: p.longitude,
            exclude_geojson: p.exclude_geojson,
        }
    }
}

/// [#215] One individual collaborator on the linked project, returned to the
/// Project Settings tab. `is_self` marks the signed-in user so the FE disables
/// its own row's actions (the server 401s a self grant/change/remove regardless).
#[derive(Debug, Clone, Serialize, TS)]
pub struct SpeleoCollaborator {
    pub email: String,
    pub permission: Permission,
    pub is_self: bool,
}

/// [#215] One team grant on the linked project. `team_name` is resolved from the
/// caller's team list; it falls back to the raw UUID when the team is not among
/// the caller's teams (still removable by id).
#[derive(Debug, Clone, Serialize, TS)]
pub struct SpeleoTeamGrant {
    pub team_id: String,
    pub team_name: String,
    pub permission: Permission,
}

/// [#215] A team the signed-in user can grant access to (the add-team picker).
#[derive(Debug, Clone, Serialize, TS)]
pub struct SpeleoTeamOption {
    pub id: String,
    pub name: String,
}

/// [#215] One suggestion for the add-collaborator email typeahead: the exact
/// `email` to grant, plus the server's "Name <email>" `label` to display.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SpeleoUserSuggestion {
    pub email: String,
    pub label: String,
}

/// [#215] The full collaborator roster for the linked project (individual users +
/// teams), returned by `speleodb_list_collaborators`.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SpeleoCollaborators {
    pub users: Vec<SpeleoCollaborator>,
    pub teams: Vec<SpeleoTeamGrant>,
}

/// [#206] A partial metadata edit from the webview. The frontend sends only the
/// fields the user changed; the orchestration folds the `Some` fields into the
/// `PATCH /projects/{id}/` body. `coordinates_edited` gates the location pin
/// (both-or-neither): when true, `latitude`+`longitude` are sent (both `Some` =
/// set the pin, both `None` = clear it); when false the pin is untouched.
#[derive(Debug, Clone, Deserialize, TS)]
pub struct SpeleoProjectMetadataPatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub country: Option<String>,
    pub visibility: Option<Visibility>,
    pub coordinates_edited: bool,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

/// [#207] Parameters for creating + publishing a local project on SpeleoDB. The
/// server injects `created_by`; `type` is fixed to `stalagmight`; `exclude_geojson` is
/// sent `false` explicitly. `latitude`/`longitude` are both-or-neither.
#[derive(Debug, Clone, Deserialize, TS)]
pub struct SpeleoPublishParams {
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
}

/// Result of pulling + extracting a project, returned to the webview. The
/// frontend then opens `local_path` through the normal `open_project` flow.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SpeleoOpenResult {
    pub local_path: String,
    /// #193 b8: the server reports a write lock held by THIS account at open time,
    /// i.e. a second same-account session is editing the project. The frontend
    /// surfaces a notice (the ancestry self-heal assumes a single same-account
    /// session - a concurrent one would misattribute its commits as ours).
    pub concurrent_session: bool,
}

/// What adoption managed to do with the converted `.stalx` before returning.
///
/// Adoption hands the project straight back to SpeleoDB - take the write lock,
/// upload the `.stalx` with its Ariane sidecar, hand the lock back - so the
/// conversion is durable on the server rather than living only in this machine's
/// cache. Every arm but [`Self::Pushed`] means the copy on disk is the only one
/// that exists, so it is stamped `has_unpushed_edits` and the webview must say so
/// instead of reporting a finished hand-over.
///
/// A discriminated union rather than a `bool` + reason pair, because the two can
/// disagree: the frontend clears its dirty seed on exactly one arm, and a shape
/// that lets "not pushed" travel without a reason is how that arm gets widened by
/// accident.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../bindings/")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AdoptUpload {
    /// The `.stalx` is on the server, the lock was handed back, and the local copy
    /// is clean. The ONLY arm the webview may seed `hasUnpushedEdits: false` on.
    Pushed,
    /// Somebody holds the write lock, so the upload was not attempted: yanking a
    /// mutex out from under another writer - or another window of our own account -
    /// to finish an adopt is not a trade this flow makes. `email` is the holder when
    /// the server named one.
    LockHeld { email: Option<String> },
    /// The lock was taken but the server had moved on from the revision this
    /// adoption converted, so the push would have overwritten a stranger commit.
    /// Nothing was uploaded and the user must reconcile.
    Diverged,
    /// The `.stalx` IS on the server, but handing the write lock back failed, so the
    /// project is still locked by us.
    ///
    /// Kept apart from [`Self::Failed`] because the two differ on the only question
    /// the user actually cares about - where their survey is. Folding this into
    /// `Failed` told someone whose work had just landed that it was "on this machine
    /// only", which is both wrong and the more alarming of the two directions to be
    /// wrong in. Nothing is at risk here: the copy is safe on the server and the lock
    /// is released by the next Done, a close, or the held-lock prompt at next login.
    ///
    /// Means "the mutex is still ours, OR we could not establish that it is not" - not
    /// "the release was refused". The release is a non-idempotent POST whose client also
    /// parses the reply, so a mid-response transport failure returns `Err` over a lock
    /// the server has already handed back. Adoption re-reads the holder from the GET it
    /// makes for the type flip and demotes this to [`Self::Pushed`] when the answer is
    /// positively "not us"; an unreadable holder leaves this arm standing, because "we
    /// could not ask" must not become "the lock is free".
    PushedLockHeld,
    /// The lock or the upload failed outright (no write access, offline, a server
    /// error). `message` is the humanised reason.
    ///
    /// `message` here and `email` above are DIAGNOSTIC, not display: the webview
    /// branches on `kind` alone and its three toasts are fixed strings, so neither
    /// value reaches a user today. They are carried because the alternative is a
    /// backend that knows why it stopped and throws it away before the boundary - the
    /// log line in `adopt_hand_over` is what reads them. Rendering the holder's email
    /// in the local-only toast is the obvious next use and needs no backend change.
    Failed { message: String },
}

/// [#214] Result of adopting an Ariane (`.tml`-only) project: the imported
/// `.stalx` now lives at `local_path` (the webview reloads it through the normal
/// `open_project` flow). `type_flipped` reports whether the server project `type`
/// ended up PATCHed ariane→stalagmight. `false` covers TWO cases now, and the webview must
/// not read it alone: the PATCH was attempted and refused (no write access), or it was
/// never attempted because the upload did not land. `upload` is what tells them apart,
/// which is why the toast keys on `pushed && type_flipped` rather than on this field.
///
/// `upload` is the hand-over half: adoption uploads the converted project before
/// returning, so a successful adopt leaves nothing stranded in the local cache.
/// The type flip is ordered AFTER that upload deliberately - a project typed
/// `stalagmight` with no `.stalx` behind it is openable by nobody, since the Ariane file
/// it still holds is the one the new type says to disregard.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SpeleoAdoptResult {
    pub local_path: String,
    pub type_flipped: bool,
    /// Same meaning as [`SpeleoOpenResult::concurrent_session`]: a write lock held
    /// by THIS account at adopt time (a second same-account session). Read BEFORE
    /// the hand-over takes a lock of its own, or the probe would be answering about
    /// this very call.
    pub concurrent_session: bool,
    pub upload: AdoptUpload,
    /// [#411] What the `.tml` -> project conversion actually did: the same receipt
    /// every other foreign-format import produces, carrying the converter's anomaly
    /// warnings plus a line naming the excluded shots this path drops on purpose.
    ///
    /// Adoption IS an Ariane import, and import-quality warnings ride an import
    /// receipt (docs/notifications.md). This flow used to discard both at the call
    /// site - it has no import wizard to have shown them - so a pulled Ariane
    /// project lost its excluded shots and every parse anomaly in silence.
    ///
    /// `None` when the import produced no receipt at all. That is reachable today,
    /// not hypothetical: `execute_import_sections` attaches none when it is handed no
    /// sections, which is what a `.tml` whose every shot is excluded converts to - so
    /// the adoption synthesizes one there rather than letting the emptiest possible
    /// result be the quietest (see `adopt_receipt`). The arm stays because a receipt
    /// is not something this type can promise on its own.
    ///
    /// `#[ts(optional)]`: an older backend answers without the field at all, so the
    /// webview's type has to admit `undefined` as well as `null` - the alternative is
    /// a required field that is genuinely absent at runtime.
    #[ts(optional)]
    pub import_receipt: Option<ImportReceipt>,
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
    Released,
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
/// reopened instead of forcing a re-login: the in-memory/keychain token lives in
/// `AppState` for the whole app run, but the dialog's React state resets on each
/// remount. Carries no token - only whether one is held and for whom.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SpeleoSessionStatus {
    pub logged_in: bool,
    /// Email of the logged-in user, when `logged_in`.
    pub user_email: Option<String>,
    /// The server this session is authenticated against, when `logged_in`. Shown
    /// in Settings ("signed in as … at …") so the user can see the session is tied
    /// to a specific server - a later endpoint change only affects FUTURE logins.
    pub endpoint: Option<String>,
    /// False when the token is held in memory only (no OS keychain) - surfaced so
    /// the UI keeps showing the "won't survive a restart" note after a reopen.
    pub keychain_backed: bool,
    /// Where the API key actually lives, for the Settings credential-storage line.
    /// `None` when logged out (there is no key to place).
    pub storage: Option<TokenStorage>,
    /// What is known about the OS keychain, WITHOUT probing it - see
    /// [`KeychainReport`].
    pub keychain: KeychainReport,
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
    /// only moment the frontend polls, and the Undo that follows marks dirty optimistically
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
    /// The server's `exclude_geojson` flag for this project (#200): when true, the
    /// server generates NO new web-viewer GeoJSON on push, so the map may be stale.
    /// Drives the rendering-disabled banner + its Enable-rendering action. Best-effort
    /// over the same `get_project` probe; `false` when logged out / offline / the
    /// lookup failed (never nag on a transient miss).
    pub exclude_geojson: bool,
    /// [#303] The project's undo-history policy - what its next push will carry.
    pub undo_mode: crate::models::UndoMode,
    /// [#213] The open remote project is being force-edited WITHOUT the write lock
    /// (`edit_enabled` is also true, but no lock is held). Drives the persistent
    /// big-red "unmergeable `.stalx`" banner + the "Reconnect to sync" action, and
    /// tells the shell to offer Reconnect rather than Done. Transient; `false` for a
    /// normal read-only or lock-held session.
    pub force_editing: bool,
    /// [#213] The user's `SpeleoDbConfig.allow_force_edit` opt-in, surfaced here so
    /// the read-only shell can gate the "Edit without lock" affordance without a
    /// separate config fetch. A copy of a global preference (not per-link), refreshed
    /// on every poll so a Settings change takes effect on the next status read. When
    /// false, the force-edit menu item stays hidden and the backend also refuses.
    pub allow_force_edit: bool,
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
    Pushed,
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
    /// [#576 C4] The routing code the IPC boundary sends for this failure, one per
    /// [`SpeleoDbAction`] (`contract::code_for_action`). A command's rejection is
    /// the [`crate::models::CommandErrorCode`]-carrying `CommandError` built from
    /// this DTO; `action` stays for the backend's own branches.
    pub code: crate::models::CommandErrorCode,
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
    /// [#258] Set ONLY when the dump held more than one openable file
    /// (`action == Ambiguous`), listing them so the frontend can offer a picker
    /// instead of dead-ending. `None` on every other error.
    pub dump_candidates: Option<Vec<SpeleoDumpCandidate>>,
    /// [#258] Which ambiguity [`Self::dump_candidates`] describes, set on exactly
    /// the same errors and `None` on the rest.
    ///
    /// Carried rather than inferred from the candidates' extensions: which kind a
    /// set is, is the classifier's decision, and re-deriving it on the frontend
    /// would put a second copy of that rule where nothing keeps the two agreeing.
    /// It changes the verb the picker offers - extracting a `.stalx` and adopting
    /// an `ariane.tml` are different operations - so guessing it wrong shows the
    /// wrong dialog rather than merely the wrong wording.
    pub dump_candidate_kind: Option<DumpCandidateKind>,
}

/// [#258] Which flavour of ambiguity a picker is being offered for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "../../bindings/")]
#[serde(rename_all = "snake_case")]
pub enum DumpCandidateKind {
    /// Several `.stalx` - the chosen one is extracted.
    Stalx,
    /// No `.stalx` and several `ariane.tml` - the chosen one is adopted (imported).
    Ariane,
}

/// [#258] One openable file inside an ambiguous project dump, carried on
/// [`SpeleoErrorDto::dump_candidates`].
///
/// `size_bytes` and `modified` are free: the classifier already walks the central
/// directory, which carries both. `modified` is `None` when the entry declares no
/// timestamp - and note that a dump produced by `git archive` stamps every entry
/// with the same commit time, so the frontend suppresses a metadata column whose
/// values all agree rather than showing a uselessly uniform one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct SpeleoDumpCandidate {
    /// The entry's path inside the dump ZIP, and the value the frontend sends back
    /// as `entryPath` to resolve. It must be byte-identical to the archive's own
    /// name: it is looked up as an exact key and is never normalised, so a
    /// helpfully-cleaned path resolves to "not found" rather than the wrong file.
    pub entry_path: String,
    /// Uncompressed size, from the central directory.
    ///
    /// Typed as a TS `number`, not ts-rs's default `bigint` for `u64`: serde emits
    /// a JSON number and the webview parses it as one, so `bigint` would describe a
    /// value that never arrives. Bounded by the 512 MiB per-entry cap and so well
    /// inside the exact-integer range - but only because `Dump::entry_details` drops
    /// any entry above it. That is load-bearing, not incidental: the raw value is the
    /// central directory's unvalidated ZIP64 `u64`, and the cap used to be applied at
    /// extraction alone, which left this claim justifying the `number` type by a bound
    /// nothing on this path enforced.
    #[ts(type = "number")]
    pub size_bytes: u64,
    /// Last-modified (RFC-3339) when the entry carries one.
    pub modified: Option<String>,
}

/// The local work a refused re-pull would have destroyed, carried on
/// [`SpeleoErrorDto::unpushed_local`] so the frontend can name it and act on it
/// without a second round-trip.
///
/// The two branches the frontend offers, and exactly what each calls:
///
/// - **Keep local** - open [`Self::local_path`] through the normal
///   `load_project_full` open flow. Nothing is downloaded and nothing is
///   overwritten; the cached copy opens with its un-pushed edits intact.
/// - **Take remote and discard local** - call
///   `speleodb_open_project_discarding_local` with the same arguments the refused
///   `speleodb_open_project` was given. That is the abandoning verb: it performs
///   the very replace this error refused.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct UnpushedLocalEdits {
    /// The SpeleoDB project whose local copy holds the un-pushed edits.
    pub project_id: String,
    /// The `.stalx` that holds them - the file "keep local" opens.
    pub local_path: String,
    /// When it was last edited (RFC-3339), when known. `None` for an older copy
    /// that never recorded one.
    pub last_edit_at: Option<String>,
    /// [#576 FZ-SDB-5-D3] The file is a `.stalx` with NO SpeleoDB link sitting at
    /// the project's cache path - a copy an Unlink made before Unlink moved its copy
    /// out of the cache. Nothing can say whether it holds work, so the replace is
    /// refused like un-pushed edits; "keep local" then moves it out of the cache
    /// (Save As, then `speleodb_unlink`) instead of opening it in place.
    pub unlinked: bool,
    /// [#576 W53] The copy is there but could not be read (a hot journal, a lock
    /// another writer holds, damage), so it may or may not hold un-pushed edits:
    /// the frontend says that instead of claiming edits, and offers the same two
    /// branches. `false` for a copy that was read and found dirty.
    pub state_unknown: bool,
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
mod tests {
    use super::{
        DoneResult, KeychainReport, Permission, Project, PushResult, RemoteState, RevisionCommit,
        RevisionHistory, SpeleoDbAction, SpeleoErrorDto, SpeleoLinkStatus, SpeleoLoginResult,
        SpeleoOpenResult, SpeleoProjectSummary, SpeleoRevision, SpeleoSessionStatus,
        StartEditingResult, TokenStorage, UpdateResult, Visibility,
    };

    #[test]
    fn ipc_dtos_never_serialize_a_token() {
        // Load-bearing invariant: no IPC-facing DTO carries the auth token (it
        // lives only in the backend TokenStore). Pin it so a future field that
        // names or holds a token can't silently break "token never in IPC".
        let login = serde_json::to_string(&SpeleoLoginResult {
            user_email: "u@x".to_owned(),
            endpoint: "https://x/".to_owned(),
            keychain_backed: true,
            storage: TokenStorage::Keychain,
            keychain: KeychainReport::Available,
            needs_storage_choice: false,
            replaced_session: false,
        })
        .expect("serialize login result");
        let status = serde_json::to_string(&SpeleoSessionStatus {
            logged_in: true,
            user_email: Some("u@x".to_owned()),
            endpoint: Some("https://x/".to_owned()),
            keychain_backed: true,
            storage: Some(TokenStorage::Keychain),
            keychain: KeychainReport::Available,
        })
        .expect("serialize session status");
        let summary = serde_json::to_string(&SpeleoProjectSummary {
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
            concurrent_session: false,
        })
        .expect("serialize open result");
        // [Track A] #186/D-B2: the structured error DTO must also never carry a
        // token. (The message deliberately avoids the substring "token" so the
        // guard checks the field SHAPE, not an incidental word in the display
        // text - the real invariant is that no field name/value is the token.)
        let error = serde_json::to_string(&SpeleoErrorDto {
            code: crate::models::CommandErrorCode::SpeleodbRelogin,
            message: "SpeleoDB returned HTTP 401: not authenticated".to_owned(),
            action: SpeleoDbAction::ReLogin,
            status: Some(401),
            transport: None,
            unpushed_local: None,
            dump_candidates: None,
            dump_candidate_kind: None,
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
        let done = serde_json::to_string(&DoneResult::Released).expect("serialize done result");
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
            exclude_geojson: false,
            undo_mode: crate::models::UndoMode::Full,
            force_editing: false,
            allow_force_edit: false,
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
        // Extra server fields (creation_date, message) are ignored; the modelled
        // fields - including exclude_geojson (#185/D-A-methods) - parse.
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
        // [Track A] #185: exclude_geojson is now PARSED (not ignored).
        assert!(p.exclude_geojson);
        // [#206] editable metadata parses (visibility as the uppercase label,
        // lat/long as JSON numbers).
        assert_eq!(p.description.as_deref(), Some("A cave"));
        assert_eq!(p.country.as_deref(), Some("US"));
        assert_eq!(p.visibility, Some(Visibility::Private));
        assert_eq!(p.latitude, Some(-45.367573));
        assert_eq!(p.longitude, Some(100.423897));
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
        // [Track A] #185: exclude_geojson defaults to false when the server omits it.
        assert!(!omitted.exclude_geojson);
        // [#206] editable metadata defaults to None when omitted (an empty repo or
        // a coordinate-less project omits lat/long).
        assert!(omitted.description.is_none());
        assert!(omitted.country.is_none());
        assert!(omitted.visibility.is_none());
        assert!(omitted.latitude.is_none());
        assert!(omitted.longitude.is_none());

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
}
