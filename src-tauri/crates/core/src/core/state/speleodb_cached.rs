//! The SpeleoDB copies cached on this machine: which account each one belongs
//! to, the listing the home screen shows, and removing copies (one, or all).
//!
//! Every project opened from SpeleoDB lives at
//! `<cache>/speleodb/<project_id>/project.stalx` (see `speleodb_local_state`). The
//! `.stalx` records the SpeleoDB instance it came from, but not the ACCOUNT: the
//! file rides to the server on every upload, so an account email stamped into it
//! would leak to every collaborator who downloads the project. The account is
//! therefore recorded beside the copy, in `owner.json`, which never leaves this
//! machine and goes away with the copy's directory.
//!
//! Ownership rules (relative to the session at the time of the listing):
//!
//! - **Signed out**: every copy is listed. None of them can be edited, and opening
//!   one is a read-only, network-free open of the cached file.
//! - **Signed in, owner recorded**: listed only when the record names the signed-in
//!   account on the signed-in instance.
//! - **Signed in, no owner record** (copies downloaded before the record existed):
//!   listed when the copy's own link names the signed-in instance, hidden when it
//!   names another one. Nothing tells whose it is, and hiding a pre-upgrade copy
//!   from the account that downloaded it would hide the way back to any work it
//!   holds; the next successful online open records the owner.
//!
//! Removal never reaches un-uploaded work without the caller's explicit consent
//! (`discard_unpushed`), and never touches the project that is open (asked again
//! under the project lock at the delete itself, so a copy opened after the check is
//! refused rather than closed and trashed). Home's removal and the purge reach only
//! the copies the listing shows, and sign-out's Discard only the session's own
//! copies: another account's work is never removed through this account's
//! session. A `.stalx` in the cache that is no longer linked to SpeleoDB (left by
//! an unlink, and possibly edited and saved since) is the user's own file: no
//! listing shows it and no removal reaches it, whatever the consent. Nor does a
//! removal follow a project directory that is a symlink: it would reach whatever
//! the link points at.
//!
//! Nor is it ever taken over. The fast-forward open opens un-uploaded work as it is
//! only for the copy's recorded owner (see `speleodb_local_state`); the guarded
//! download's prompt and the offline open name another owner and open its work
//! read-only (`speleodb_pull`); and Start Editing refuses a copy recorded for
//! another account while it holds un-uploaded edits
//! (`AppState::speleodb_guard_open_copy_owner`).
//!
//! Removal does not strand a lock either. Once the copy is gone, when the session
//! is the copy's account on its instance (owner recorded as this account, or no
//! owner recorded and the copy is from this instance), the server is asked who
//! holds the project's lock, and if it says this account, the lock is handed back
//! through the normal release. Signed out, for another account's copy, or when the
//! server cannot be asked, nothing else happens: a lock held on the server stays
//! until that account's next sign-in (whose held-lock prompt offers to release it)
//! or the SpeleoDB website.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::speleodb_local_state::{
    cache_path_in, cache_root, is_the_cached_copy, read_cached_link, CachedLink,
};
use super::AppState;
use crate::core::{CommandResult, ValidationWarning, WarningSeverity};
use crate::speleodb::auth::normalize_endpoint;

/// The ownership record beside each cached copy.
const OWNER_FILE: &str = "owner.json";

/// Refusal text when a removal would discard work SpeleoDB does not have. The
/// frontend asks before it calls, so reaching this means the copy changed (or could
/// not be read) between the question and the removal.
const UNPUSHED_REFUSAL: &str = "This copy has changes that were never uploaded to SpeleoDB, \
or its state could not be read. Nothing was removed.";

/// Refusal text when Home's "Remove local copy" names another account's copy while
/// signed in. Home does not list such a copy, so reaching this means the listing was
/// stale (it was read signed out) or the call did not come from it.
const OTHER_ACCOUNTS_REMOVAL_REFUSAL: &str = "This copy belongs to another SpeleoDB account, \
so it cannot be removed while you are signed in. Nothing was removed.";

/// Refusal text when sign-out's Discard names a copy the session is not the account
/// of (another account's or instance's, or no session at all). The sign-out only
/// lists the signed-in account's projects, so reaching this means the call did not
/// come from that list.
const NOT_THIS_ACCOUNTS_REFUSAL: &str = "Only copies downloaded by the SpeleoDB account in \
use can be discarded here. Nothing was removed.";

/// Refusal text when Home's "Remove local copy" names the project open right now.
const REMOVE_OPEN_REFUSAL: &str = "This project is open. Close it first, then remove its copy.";

/// Refusal text when sign-out's Discard names the project open right now.
const DISCARD_OPEN_REFUSAL: &str =
    "This project is open. Close it first, then discard its changes.";

/// Refusal text when the copy's `.stalx` is no longer linked to SpeleoDB (an
/// unlink keeps the file, which may have been edited and saved since). No listing
/// shows such a file, so reaching this means the listing was stale.
const UNLINKED_REFUSAL: &str = "This file is no longer linked to SpeleoDB, so it is not \
removed here: it may hold work saved since it was unlinked. Nothing was removed.";

/// Refusal text when the copy's directory is not a plain directory of the cache (a
/// symlink to another folder, say): removing the copy would reach through it.
const NOT_A_CACHE_DIR_REFUSAL: &str = "This copy's folder links to another place, so \
Stalagmight does not remove anything through it. Nothing was removed.";

/// Which account a cached copy belongs to, relative to the signed-in session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum SpeleoCacheOwnership {
    /// No SpeleoDB session: every copy is listed, and none can be edited.
    SignedOut,
    /// Downloaded by the signed-in account on the signed-in instance.
    Mine,
    /// A copy from before owners were recorded, from the signed-in instance.
    Unknown,
    /// Another account, or another SpeleoDB instance. Not listed while signed in.
    OtherAccount,
}

impl SpeleoCacheOwnership {
    /// Whether the home screen lists a copy with this ownership.
    pub fn listed(self) -> bool {
        !matches!(self, Self::OtherAccount)
    }
}

/// One cached copy, as the home screen and the cache purge see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct SpeleoCachedProject {
    /// SpeleoDB project UUID (the cache directory name).
    pub project_id: String,
    /// The SpeleoDB name captured when the copy was downloaded.
    pub project_name: String,
    /// The cached `.stalx`.
    pub local_path: String,
    pub ownership: SpeleoCacheOwnership,
    /// The copy holds edits SpeleoDB does not have.
    pub has_unpushed_edits: bool,
    /// The copy is there but could not be read: NOT a claim that it is clean.
    pub state_unknown: bool,
    /// The copy is the project open right now.
    pub is_open: bool,
}

/// What "Purge all cached files" did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, TS)]
pub struct SpeleoPurgeReport {
    /// Copies removed.
    pub removed: u32,
    /// Names of copies kept because they hold un-uploaded (or unreadable) work.
    pub kept_unpushed: Vec<String>,
    /// Names of copies kept because they are open.
    pub kept_open: Vec<String>,
    /// `name: reason` for copies whose removal failed.
    pub failed: Vec<String>,
    /// Names of removed copies whose editing lock this account held and handed back.
    pub locks_released: Vec<String>,
    /// Names of removed copies of this account whose lock could not be checked or
    /// handed back (SpeleoDB unreachable, or the release failed).
    pub locks_unchecked: Vec<String>,
}

/// The `owner.json` record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CacheOwner {
    endpoint: String,
    email: String,
    /// Display fallback for a copy whose `.stalx` cannot be read.
    #[serde(default)]
    project_name: String,
}

/// The owner record beside the cached copy at `copy`.
fn owner_path_for(copy: &Path) -> PathBuf {
    copy.with_file_name(OWNER_FILE)
}

/// The recorded owner of the copy at `copy`, or `None` when there is no record or
/// it cannot be read (an unreadable record is treated exactly like a missing one:
/// unknown owner).
fn read_owner_for(copy: &Path) -> Option<CacheOwner> {
    let bytes = std::fs::read(owner_path_for(copy)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Write the owner record through a temporary file and a rename, so a crash never
/// leaves a half-written record (which would read as unknown anyway).
///
/// The temporary file is one this call creates: whatever already has its name (a
/// leftover from a crash, or a symlink planted there) is removed first, and
/// `create_new` then refuses anything that appears under the name in between. A
/// plain write followed a planted symlink and wrote the record over its target.
/// The rename replaces the `owner.json` entry itself, never what a link there
/// points at.
fn write_owner_for(copy: &Path, owner: &CacheOwner) -> std::io::Result<()> {
    use std::io::Write as _;
    let path = owner_path_for(copy);
    let tmp = path.with_extension("json.tmp");
    match std::fs::remove_file(&tmp) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)?;
    file.write_all(&serde_json::to_vec(owner)?)?;
    drop(file);
    std::fs::rename(&tmp, &path)
}

/// Classify one copy against the session (`(endpoint, email)`, `None` = signed out).
fn ownership_of(
    session: Option<(&str, &str)>,
    owner: Option<&CacheOwner>,
    link_endpoint: Option<&str>,
) -> SpeleoCacheOwnership {
    let Some((endpoint, email)) = session else {
        return SpeleoCacheOwnership::SignedOut;
    };
    let same_instance = |other: &str| normalize_endpoint(other) == normalize_endpoint(endpoint);
    match owner {
        Some(owner)
            if same_instance(&owner.endpoint) && owner.email.eq_ignore_ascii_case(email) =>
        {
            SpeleoCacheOwnership::Mine
        }
        Some(_) => SpeleoCacheOwnership::OtherAccount,
        // No record: only the copy's own instance can rule it out.
        None if link_endpoint.is_some_and(|linked| !same_instance(linked)) => {
            SpeleoCacheOwnership::OtherAccount
        }
        None => SpeleoCacheOwnership::Unknown,
    }
}

/// [`ownership_of`] for the copy at `copy`, reading its owner record. Shared with
/// the fast-forward open (`speleodb_local_state`), which must apply the same rule
/// as the listing before it opens a copy as the session's own work.
pub(super) fn copy_ownership(
    session: Option<(&str, &str)>,
    copy: &Path,
    link_endpoint: Option<&str>,
) -> SpeleoCacheOwnership {
    ownership_of(session, read_owner_for(copy).as_ref(), link_endpoint)
}

/// The account (email) the owner record of the copy at `copy` names when that is
/// NOT the session's `(endpoint, email)`: another account, or the same email
/// recorded on another instance. `None` when there is no record (an owner-less copy
/// keeps the unknown-owner rules) or it names the session's own account.
pub(super) fn copy_recorded_for_other_account(
    session: (&str, &str),
    copy: &Path,
) -> Option<String> {
    let owner = read_owner_for(copy)?;
    (ownership_of(Some(session), Some(&owner), None) == SpeleoCacheOwnership::OtherAccount)
        .then_some(owner.email)
}

/// Whether `path` IS the managed cache copy of `project_id` in the cache at `root`:
/// the only place an owner record is ever written, so the only file one may speak
/// for. Not just a path shaped like one (`<anything>/<project_id>/project.stalx`):
/// that let Start Editing write `owner.json` beside any such file the user opened,
/// anywhere on disk. The project's directory must be a real one of the cache, too.
fn is_managed_cache_copy(root: &Path, project_id: &str, path: &Path) -> bool {
    super::speleodb::validate_project_id(project_id).is_ok()
        && !cache_dir_is_foreign(root, project_id)
        && is_the_cached_copy(root, project_id, path)
}

/// Whether `root/<project_id>` is something other than a directory of the cache:
/// a symlink (to one of the user's own folders, say), a file, or an entry that
/// cannot be examined. Removing the copy "inside" it would reach whatever it points
/// at. An absent directory is not foreign: there is nothing there to remove.
fn cache_dir_is_foreign(root: &Path, project_id: &str) -> bool {
    match std::fs::symlink_metadata(root.join(project_id)) {
        Ok(meta) => !meta.file_type().is_dir(),
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    }
}

/// The endpoint the copy at `copy` links to, `None` when it cannot be read.
fn copy_link_endpoint(copy: &Path) -> Option<String> {
    match read_cached_link(copy) {
        CachedLink::Linked(link) => Some(link.endpoint),
        CachedLink::NotManaged | CachedLink::Unreadable => None,
    }
}

/// Every directory under the cache root named like a project id.
fn cached_ids(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|id| super::speleodb::validate_project_id(id).is_ok())
        .collect();
    ids.sort();
    ids
}

impl AppState {
    /// Record that the cached copy at `copy` was opened online by the account
    /// `email` on `endpoint`. Best effort: a failure only leaves the owner unknown,
    /// which lists the copy for every account of that instance.
    pub(super) fn speleodb_record_cache_owner(
        &self,
        copy: &Path,
        endpoint: &str,
        email: &str,
        project_name: &str,
    ) {
        let owner = CacheOwner {
            endpoint: endpoint.to_owned(),
            email: email.to_owned(),
            project_name: project_name.to_owned(),
        };
        if let Err(e) = write_owner_for(copy, &owner) {
            tracing::warn!(
                path = %copy.display(),
                error = %e,
                purpose = "record which account a cached SpeleoDB copy belongs to",
                "speleodb: could not record the cached copy's owner"
            );
        }
    }

    /// The cached SpeleoDB copies on this machine, every one of them, classified
    /// against the current session. Purely local: no token is sent anywhere.
    pub fn speleodb_cached_projects(&self) -> Vec<SpeleoCachedProject> {
        cache_root()
            .map(|root| self.speleodb_cached_projects_in(&root))
            .unwrap_or_default()
    }

    /// [`Self::speleodb_cached_projects`] against an explicit cache root.
    pub(super) fn speleodb_cached_projects_in(&self, root: &Path) -> Vec<SpeleoCachedProject> {
        let account = self
            .speleodb_active_account()
            .ok()
            .map(|(endpoint, _token, email)| (endpoint, email));
        let session = account
            .as_ref()
            .map(|(endpoint, email)| (endpoint.as_str(), email.as_str()));
        let open_path = self.open_project_path();
        let live = self.speleodb_open_link().ok();
        cached_ids(root)
            .into_iter()
            .filter_map(|id| {
                let path = cache_path_in(root, &id);
                let owner = read_owner_for(&path);
                let is_open = open_path
                    .as_deref()
                    .is_some_and(|open| super::project_io::same_file(open, &path));
                // The open project's dirty bit is stamped into its live database on
                // every edit, so it is at least as fresh as the file: fold it in when
                // (and only when) the open project IS this file.
                let live = live
                    .as_ref()
                    .filter(|link| is_open && link.project_id == id);
                let (name, endpoint, dirty, state_unknown) = match (read_cached_link(&path), live) {
                    (CachedLink::NotManaged, _) => return None,
                    (CachedLink::Linked(link), live) => (
                        link.project_name,
                        Some(link.endpoint),
                        link.has_unpushed_edits || live.is_some_and(|l| l.has_unpushed_edits),
                        false,
                    ),
                    (CachedLink::Unreadable, Some(live)) => (
                        live.project_name.clone(),
                        Some(live.endpoint.clone()),
                        live.has_unpushed_edits,
                        false,
                    ),
                    (CachedLink::Unreadable, None) => (
                        owner
                            .as_ref()
                            .map(|o| o.project_name.clone())
                            .unwrap_or_default(),
                        None,
                        false,
                        true,
                    ),
                };
                let ownership = ownership_of(session, owner.as_ref(), endpoint.as_deref());
                Some(SpeleoCachedProject {
                    project_name: if name.trim().is_empty() {
                        id.clone()
                    } else {
                        name
                    },
                    project_id: id,
                    local_path: path.to_string_lossy().into_owned(),
                    ownership,
                    has_unpushed_edits: dirty,
                    state_unknown,
                    is_open,
                })
            })
            .collect()
    }

    /// Remove the cached copy of `project_id`: the `.stalx` goes to the OS trash
    /// where there is one (through `delete_project`, which also drops its recents
    /// row), then the rest of its directory goes. Then, when the session is the
    /// copy's account, this account's lock on the project is handed back if the
    /// server says it holds it (see the module docs); a lock that could not be
    /// checked or handed back is reported as a warning.
    ///
    /// Refuses the open project, and - unless `discard_unpushed` carries the user's
    /// explicit consent - a copy with un-uploaded edits or an unreadable state.
    pub fn speleodb_remove_cached_copy(
        &self,
        project_id: &str,
        discard_unpushed: bool,
    ) -> CommandResult {
        match cache_root() {
            Some(root) => self.speleodb_remove_cached_copy_in(&root, project_id, discard_unpushed),
            None => CommandResult::failure(
                "Could not find Stalagmight's SpeleoDB storage. Nothing was changed.".to_owned(),
            ),
        }
    }

    /// [`Self::speleodb_remove_cached_copy`] against an explicit cache root.
    pub(super) fn speleodb_remove_cached_copy_in(
        &self,
        root: &Path,
        project_id: &str,
        discard_unpushed: bool,
    ) -> CommandResult {
        if let Err(e) = super::speleodb::validate_project_id(project_id) {
            return CommandResult::failure(e.to_string());
        }
        // Signed in, only a copy the home screen lists for this account (the same
        // rule as the purge); signed out, any copy, as before.
        let account = match self.removal_scope() {
            Ok(account) => account,
            Err(refusal) => return CommandResult::failure(refusal),
        };
        let path = cache_path_in(root, project_id);
        let session = account
            .as_ref()
            .map(|(endpoint, email)| (endpoint.as_str(), email.as_str()));
        if !copy_ownership(session, &path, copy_link_endpoint(&path).as_deref()).listed() {
            return CommandResult::failure(OTHER_ACCOUNTS_REMOVAL_REFUSAL.to_owned());
        }
        match self.cached_copy_removal(root, project_id, discard_unpushed) {
            Removal::Open => CommandResult::failure(REMOVE_OPEN_REFUSAL.to_owned()),
            Removal::AtRisk => CommandResult::failure(UNPUSHED_REFUSAL.to_owned()),
            Removal::Unlinked => CommandResult::failure(UNLINKED_REFUSAL.to_owned()),
            Removal::NotACacheDir => CommandResult::failure(NOT_A_CACHE_DIR_REFUSAL.to_owned()),
            Removal::Allowed => {
                tracing::info!(
                    project_id = %project_id,
                    discard_unpushed,
                    purpose = "remove a cached SpeleoDB copy (user confirmed)",
                    "speleodb: removing a cached copy"
                );
                let Some((result, lock)) =
                    self.remove_cached_copy_and_lock(root, project_id, &mut ServerReach::default())
                else {
                    // Opened since the check above: nothing was done.
                    return CommandResult::failure(REMOVE_OPEN_REFUSAL.to_owned());
                };
                match lock {
                    LockHandBack::Unknown(reason) => {
                        let mut warnings = result.warnings.clone();
                        warnings.push(ValidationWarning {
                            code: "speleodb_lock_not_handed_back".to_owned(),
                            message: format!(
                                "The copy was removed, but its editing lock could not be handed \
                                 back ({reason}). If you hold it, it stays held until your next \
                                 sign-in offers to release it, or until you release it on the \
                                 SpeleoDB website."
                            ),
                            severity: WarningSeverity::Warning,
                        });
                        result.with_warnings(warnings)
                    }
                    _ => result,
                }
            }
        }
    }

    /// "Purge all cached files": remove every cached copy the session reaches except
    /// the open one, and except those with un-uploaded (or unreadable) work unless
    /// `discard_unpushed`.
    ///
    /// The purge reaches exactly the copies the home screen lists. Signed in: the
    /// account's own on its instance, and those with no owner record from that
    /// instance. Another account's copy (or another instance's) is not touched, not
    /// even with `discard_unpushed`: its un-uploaded work is not this user's to
    /// throw away, and the confirmation never named it. Signed out, every copy.
    /// Either way, never a `.stalx` that is no longer linked to SpeleoDB (the
    /// listing does not show one, and it may hold saved work): it is skipped, and
    /// not reported, since the confirmation never named it either.
    pub fn speleodb_purge_cached_copies(&self, discard_unpushed: bool) -> SpeleoPurgeReport {
        cache_root()
            .map(|root| self.speleodb_purge_cached_copies_in(&root, discard_unpushed))
            .unwrap_or_default()
    }

    /// [`Self::speleodb_purge_cached_copies`] against an explicit cache root.
    pub(super) fn speleodb_purge_cached_copies_in(
        &self,
        root: &Path,
        discard_unpushed: bool,
    ) -> SpeleoPurgeReport {
        let mut report = SpeleoPurgeReport::default();
        // Read once, so every copy is judged against the same account.
        let account = match self.removal_scope() {
            Ok(account) => account,
            Err(refusal) => {
                report.failed.push(refusal);
                return report;
            }
        };
        let session = account
            .as_ref()
            .map(|(endpoint, email)| (endpoint.as_str(), email.as_str()));
        let names: std::collections::HashMap<String, String> = self
            .speleodb_cached_projects_in(root)
            .into_iter()
            .map(|copy| (copy.project_id, copy.project_name))
            .collect();
        let mut other_accounts = 0_usize;
        let mut not_listed = 0_usize;
        // One server for the whole run: once it has not answered, the remaining
        // copies' locks are reported unchecked without another probe each.
        let mut server = ServerReach::default();
        for id in cached_ids(root) {
            let path = cache_path_in(root, &id);
            let link_endpoint = copy_link_endpoint(&path);
            if !copy_ownership(session, &path, link_endpoint.as_deref()).listed() {
                other_accounts += 1;
                continue;
            }
            let name = names.get(&id).cloned().unwrap_or_else(|| id.clone());
            match self.cached_copy_removal(root, &id, discard_unpushed) {
                Removal::Open => report.kept_open.push(name),
                Removal::AtRisk => report.kept_unpushed.push(name),
                // Not on the listing the confirmation was built from (see the doc).
                Removal::Unlinked | Removal::NotACacheDir => not_listed += 1,
                Removal::Allowed => {
                    let Some((result, lock)) =
                        self.remove_cached_copy_and_lock(root, &id, &mut server)
                    else {
                        // Opened since the check: kept, with nothing done.
                        report.kept_open.push(name);
                        continue;
                    };
                    if result.success {
                        report.removed += 1;
                    } else {
                        report
                            .failed
                            .push(format!("{name}: {}", result.error.unwrap_or_default()));
                    }
                    match lock {
                        LockHandBack::Released => report.locks_released.push(name),
                        LockHandBack::Unknown(_) => report.locks_unchecked.push(name),
                        LockHandBack::NotOurs | LockHandBack::NotHeld => {}
                    }
                }
            }
        }
        tracing::info!(
            removed = report.removed,
            kept_unpushed = report.kept_unpushed.len(),
            kept_open = report.kept_open.len(),
            failed = report.failed.len(),
            locks_released = report.locks_released.len(),
            locks_unchecked = report.locks_unchecked.len(),
            other_accounts,
            not_listed,
            discard_unpushed,
            purpose = "purge the SpeleoDB cache (user confirmed)",
            "speleodb: purged cached copies"
        );
        report
    }

    /// The account Home's removals are scoped to: `Some((endpoint, email))` signed
    /// in, `None` signed out (every copy is in reach then, as Home lists them all).
    /// A session that cannot be read at all is neither: `Err` with the refusal, and
    /// nothing is removed.
    fn removal_scope(&self) -> Result<Option<(String, String)>, String> {
        match self.speleodb_active_account() {
            Ok((endpoint, _token, email)) => Ok(Some((endpoint, email))),
            Err(crate::speleodb::SpeleoDbError::NotAuthenticated) => Ok(None),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    purpose = "scope a cached-copy removal to the signed-in account",
                    "speleodb: could not read the session; nothing removed"
                );
                Err(format!(
                    "Could not tell which account is connected ({e}). Nothing was removed."
                ))
            }
        }
    }

    /// Start Editing's owner guard (must-fix 1: one account never takes over another
    /// account's un-uploaded work). When the open project is a managed cache copy
    /// whose owner record names another account (or another instance):
    ///
    /// - with un-uploaded edits, refuse: acquiring the lock would make that
    ///   account's work this account's to edit and upload. The copy stays open
    ///   read-only, where it can still be saved as a copy.
    /// - clean, it holds only the server's content, nobody's work: it is recorded
    ///   as this account's from now on, so Home lists it and the next open's
    ///   fast-forward knows whose edits it carries.
    ///
    /// A copy with no owner record keeps the unknown-owner rules, and a file that is
    /// not the managed cache copy (in the app's own cache) has no record to read or
    /// write.
    pub(super) fn speleodb_guard_open_copy_owner(
        &self,
        endpoint: &str,
        link: &crate::models::SpeleoLink,
    ) -> Result<(), crate::speleodb::SpeleoDbError> {
        let (Some(email), Some(path), Some(root)) = (
            self.speleodb_my_email(),
            self.open_project_path(),
            cache_root(),
        ) else {
            return Ok(());
        };
        if !is_managed_cache_copy(&root, &link.project_id, &path) {
            return Ok(());
        }
        let Some(owner) = copy_recorded_for_other_account((endpoint, &email), &path) else {
            return Ok(());
        };
        if link.has_unpushed_edits {
            tracing::info!(
                project_id = %link.project_id,
                purpose = "never adopt another account's un-uploaded work",
                "speleodb: the open copy holds another account's un-uploaded edits; not editing"
            );
            return Err(crate::speleodb::SpeleoDbError::OtherAccountsCopy { owner });
        }
        self.speleodb_record_cache_owner(&path, endpoint, &email, &link.project_name);
        Ok(())
    }

    /// The backend guard of sign-out's per-project Discard
    /// (`AppState::speleodb_discard_cached_copy`), which must not rely on the
    /// dialog's consent: `Some(refusal)` unless the copy of `project_id` may be
    /// thrown away now. Refused: a copy the session is not the account of (the same
    /// rule as the lock hand-back after a removal: owner recorded as this account,
    /// or no owner recorded and the copy is from this instance; signed out reaches
    /// nothing), the open project, and - unless `discard_unpushed` carries the
    /// user's explicit Discard - a copy with un-uploaded or unreadable work.
    pub(super) fn cached_copy_discard_refusal(
        &self,
        root: &Path,
        project_id: &str,
        discard_unpushed: bool,
    ) -> Option<String> {
        if self
            .copy_account_session(&cache_path_in(root, project_id))
            .is_none()
        {
            return Some(NOT_THIS_ACCOUNTS_REFUSAL.to_owned());
        }
        match self.cached_copy_removal(root, project_id, discard_unpushed) {
            Removal::Allowed => None,
            Removal::Open => Some(DISCARD_OPEN_REFUSAL.to_owned()),
            Removal::AtRisk => Some(UNPUSHED_REFUSAL.to_owned()),
            Removal::Unlinked => Some(UNLINKED_REFUSAL.to_owned()),
            Removal::NotACacheDir => Some(NOT_A_CACHE_DIR_REFUSAL.to_owned()),
        }
    }

    /// Sign-out's Discard, past [`Self::cached_copy_discard_refusal`]: trash the
    /// copy's `.stalx` (see [`Self::trash_cached_copy`]), refusing a copy that was
    /// opened since the check.
    pub(super) fn discard_cached_copy_files(&self, root: &Path, project_id: &str) -> CommandResult {
        self.trash_cached_copy(root, project_id)
            .unwrap_or_else(|| CommandResult::failure(DISCARD_OPEN_REFUSAL.to_owned()))
    }

    /// May the copy of `project_id` be removed now?
    fn cached_copy_removal(
        &self,
        root: &Path,
        project_id: &str,
        discard_unpushed: bool,
    ) -> Removal {
        if cache_dir_is_foreign(root, project_id) {
            return Removal::NotACacheDir;
        }
        let path = cache_path_in(root, project_id);
        if self
            .open_project_path()
            .is_some_and(|open| super::project_io::same_file(&open, &path))
        {
            return Removal::Open;
        }
        // Unlinked, the file is the user's own project now (see the module doc).
        // An absent file reads as `NotManaged` too, and there nothing is at risk.
        if matches!(read_cached_link(&path), CachedLink::NotManaged)
            && path.try_exists().unwrap_or(true)
        {
            return Removal::Unlinked;
        }
        if discard_unpushed {
            return Removal::Allowed;
        }
        // The same reader the download guard uses: unreadable fails closed.
        match self.local_copy_state(project_id, &path) {
            Some(state) if !state.has_unpushed_edits && !state.state_unknown => Removal::Allowed,
            _ => Removal::AtRisk,
        }
    }

    /// Remove the copy's files, then hand back this account's lock on the project
    /// when that is ours to do. The account is resolved BEFORE the removal, because
    /// the owner record goes with the copy's directory. The lock is handled only
    /// after a successful removal: until the files are gone, the lock still guards
    /// whatever they hold.
    ///
    /// `None`, with nothing done, when the copy turned out to be open (see
    /// [`Self::trash_cached_copy`]).
    fn remove_cached_copy_and_lock(
        &self,
        root: &Path,
        project_id: &str,
        server: &mut ServerReach,
    ) -> Option<(CommandResult, LockHandBack)> {
        let account = self.copy_account_session(&cache_path_in(root, project_id));
        let result = self.remove_cached_copy_files(root, project_id)?;
        if !result.success {
            return Some((result, LockHandBack::NotOurs));
        }
        let lock = self.hand_back_removed_copy_lock(project_id, account, server);
        Some((result, lock))
    }

    /// The session's `(endpoint, token, email)` when it is the account of the copy at
    /// `copy`: its recorded owner on its instance, or (no owner recorded) any account
    /// of the copy's own instance. `None` signed out or for another account's copy.
    fn copy_account_session(&self, copy: &Path) -> Option<(String, String, String)> {
        let (endpoint, token, email) = self.speleodb_active_account().ok()?;
        let ownership = copy_ownership(
            Some((endpoint.as_str(), email.as_str())),
            copy,
            copy_link_endpoint(copy).as_deref(),
        );
        matches!(
            ownership,
            SpeleoCacheOwnership::Mine | SpeleoCacheOwnership::Unknown
        )
        .then_some((endpoint, token, email))
    }

    /// Ask the server who holds the project's lock (the short, unretried probe) and,
    /// when it says this account, hand it back through the normal release. Never
    /// drops the session on a failed probe: this is a best-effort courtesy after a
    /// removal, not an authenticated operation the user asked for.
    ///
    /// When an earlier probe of the same run (`server`) could not reach the server,
    /// nothing is asked: the lock is reported unchecked straight away instead of
    /// waiting out the probe's timeout once more for every copy.
    fn hand_back_removed_copy_lock(
        &self,
        project_id: &str,
        account: Option<(String, String, String)>,
        server: &mut ServerReach,
    ) -> LockHandBack {
        let Some((endpoint, token, email)) = account else {
            return LockHandBack::NotOurs;
        };
        if let Some(reason) = &server.unreachable {
            return LockHandBack::Unknown(reason.clone());
        }
        let insecure_http = self.speleodb_insecure_http();
        match crate::speleodb::client::get_project_probe(
            &endpoint,
            &token,
            insecure_http,
            project_id,
        ) {
            Ok(remote) => {
                let holder = remote.active_mutex.as_ref().map(|m| m.user.as_str());
                if !super::speleodb::lock_held_by_me(holder, Some(&email)) {
                    return LockHandBack::NotHeld;
                }
            }
            Err(e) => {
                let reason = e.to_string();
                if matches!(e, crate::speleodb::SpeleoDbError::Transport { .. }) {
                    server.unreachable = Some(reason.clone());
                }
                return LockHandBack::Unknown(reason);
            }
        }
        match self.speleodb_release_lock(project_id.to_owned()) {
            Ok(()) => {
                tracing::info!(
                    project_id = %project_id,
                    purpose = "hand back the lock of a removed cached copy",
                    "speleodb: released this account's lock after removing its copy"
                );
                LockHandBack::Released
            }
            Err(e) => LockHandBack::Unknown(e.message),
        }
    }

    /// Trash the `.stalx` (dropping its recents row), then remove what is left of
    /// its directory: the owner record and any SQLite sidecars, which must not
    /// outlive the file they belong to (a later download into the same directory
    /// would otherwise replay them).
    ///
    /// The trash gets the file under the project's name (see [`trash_file_name`]),
    /// so a user restoring a removed copy can tell which one it is.
    ///
    /// `None`, with nothing done, when the copy turned out to be open.
    pub(super) fn remove_cached_copy_files(
        &self,
        root: &Path,
        project_id: &str,
    ) -> Option<CommandResult> {
        let result = self.trash_cached_copy(root, project_id)?;
        if !result.success {
            return Some(result);
        }
        Some(match std::fs::remove_dir_all(root.join(project_id)) {
            Ok(()) => result,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => result,
            Err(e) => {
                tracing::warn!(
                    project_id = %project_id,
                    error = %e,
                    purpose = "remove a cached SpeleoDB copy",
                    "speleodb: the copy is gone but its directory could not be removed"
                );
                let mut warnings = result.warnings.clone();
                warnings.push(ValidationWarning {
                    code: "speleodb_cache_leftovers".to_owned(),
                    message: format!(
                        "The copy was removed, but some of its files could not be ({e})."
                    ),
                    severity: WarningSeverity::Warning,
                });
                result.with_warnings(warnings)
            }
        })
    }

    /// Trash the cached copy of `project_id`'s `.stalx` (under the project's name,
    /// dropping its recents row) unless it is the open project: `None`, with
    /// nothing done, when it is. Every removal of a copy that is not open goes
    /// through here (Home's removal, the purge, sign-out's Discard).
    ///
    /// The open-project question is asked again by the delete itself, under the
    /// same `project` lock acquisition its teardown would use
    /// (`delete_project_unless_open`): the callers' own check
    /// ([`Self::cached_copy_removal`]) runs earlier, and a copy opened in between
    /// was closed and trashed. Its directory is checked again here too, right
    /// before the delete, so a symlink put in its place since is never followed.
    fn trash_cached_copy(&self, root: &Path, project_id: &str) -> Option<CommandResult> {
        if cache_dir_is_foreign(root, project_id) {
            return Some(CommandResult::failure(NOT_A_CACHE_DIR_REFUSAL.to_owned()));
        }
        let path = cache_path_in(root, project_id);
        let trash_name = cached_copy_trash_name(&path);
        self.delete_project_unless_open(path, trash_name.as_deref())
    }
}

/// The project name of the copy at `copy`: its link, else its owner record.
pub(super) fn cached_copy_name(copy: &Path) -> Option<String> {
    match read_cached_link(copy) {
        CachedLink::Linked(link) => Some(link.project_name),
        CachedLink::NotManaged | CachedLink::Unreadable => {
            read_owner_for(copy).map(|owner| owner.project_name)
        }
    }
}

/// The name the cached copy at `copy` goes to the trash under ([`trash_file_name`]
/// of [`cached_copy_name`]). Every removal of a managed copy uses it: Home's
/// "Remove local copy", the purge, the exit prompt's and sign-out's Discard.
pub(super) fn cached_copy_trash_name(copy: &Path) -> Option<String> {
    cached_copy_name(copy).as_deref().and_then(trash_file_name)
}

/// The file name a removed copy is trashed under: the project's name made safe on
/// every desktop OS, plus `.stalx`. `None` when nothing usable is left (the copy is
/// then trashed as `project.stalx`).
///
/// Path separators, the characters Windows forbids, control characters and
/// Unicode format characters become `_`; leading and trailing dots and spaces go
/// (Windows strips the trailing ones, a leading dot hides the file); a Windows
/// device name (`CON`, `COM1`, ...) gets a `_` suffix; and the name is cut to 150
/// bytes, well inside every file system's 255-byte limit.
pub(super) fn trash_file_name(project_name: &str) -> Option<String> {
    const MAX_BYTES: usize = 150;
    let mut stem = String::new();
    for c in project_name.chars() {
        let c = if c.is_control()
            || is_format_char(c)
            || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        {
            '_'
        } else {
            c
        };
        if stem.len() + c.len_utf8() > MAX_BYTES {
            break;
        }
        stem.push(c);
    }
    let stem = stem.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if stem.is_empty() {
        return None;
    }
    let device = stem
        .split('.')
        .next()
        .unwrap_or(stem)
        .trim_end()
        .to_ascii_uppercase();
    let reserved = matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (device.len() == 4
            && (device.starts_with("COM") || device.starts_with("LPT"))
            && device.ends_with(|c: char| c.is_ascii_digit()));
    Some(if reserved {
        format!("{stem}_.stalx")
    } else {
        format!("{stem}.stalx")
    })
}

/// A Unicode format character (general category Cf, as of Unicode 15.1). They are
/// invisible, and some reorder the text around them: with U+202E RIGHT-TO-LEFT
/// OVERRIDE a server-chosen name can show a different extension in a file
/// manager than the file has, so none is kept in a trash name.
fn is_format_char(c: char) -> bool {
    matches!(
        u32::from(c),
        0x00AD
            | 0x0600..=0x0605
            | 0x061C
            | 0x06DD
            | 0x070F
            | 0x0890..=0x0891
            | 0x08E2
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

/// The removal verdict for one copy.
enum Removal {
    Allowed,
    Open,
    /// Un-uploaded or unreadable work, and no consent to discard it.
    AtRisk,
    /// The `.stalx` is no longer linked to SpeleoDB: never removed.
    Unlinked,
    /// The project's directory is a symlink (or not a directory): never followed.
    NotACacheDir,
}

/// What happened to the server lock of a copy that was just removed.
enum LockHandBack {
    /// Signed out, or another account's copy: nothing was asked, and a lock held on
    /// the server stays until that account's next sign-in or the website.
    NotOurs,
    /// The server says this account does not hold the lock.
    NotHeld,
    /// This account's lock was handed back.
    Released,
    /// The server could not be asked, or the release failed.
    Unknown(String),
}

/// Whether the server has answered the lock probes of one removal run (a single
/// removal, or a whole purge: every probe goes to the signed-in instance).
#[derive(Default)]
struct ServerReach {
    /// Why the server could not be reached (a transport failure), once one probe
    /// has failed that way. Later copies are not probed: each would only wait out
    /// the probe's timeout again.
    unreachable: Option<String>,
}
