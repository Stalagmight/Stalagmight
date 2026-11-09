//! The server side of a fuzz run: the run's accounts, the per-run projects, the
//! oracle's view of the truth, and the second client.
//!
//! Every call here goes DIRECTLY to the instance (never through the fault
//! proxy): the oracle must see the truth, and the second client is another
//! computer. All content is synthetic: project names come from the caller, the
//! survey from [`data::project_with`], the description is `fuzz project` and the
//! country `US`.
//!
//! Token robustness: SpeleoDB token auth may answer 401 for a token another
//! sign-in rotated (and 403 on `GET api/v2/user/`). Every [`Server`] method
//! therefore re-signs that account in once and retries the call once on such an
//! answer ([`retry_after_relogin`]); tokens sit behind a `Mutex` so `&self`
//! methods can refresh them.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Write as _;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use chrono::{DateTime, FixedOffset};
use tempfile::TempDir;

use super::data::{self, Fp};
use super::target::{check_target_url, Account, FuzzTarget};
use super::wire;
use crate::models::Project;
use crate::persistence::Database;
use crate::speleodb::models::{Project as RemoteProject, RevisionCommit};
use crate::speleodb::{client, Permission, SpeleoDbError, TransportCause};

/// Which account an actor step runs as.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub(super) enum Who {
    /// The superuser: the oracle's eyes once [`Server::observe`] granted it.
    Admin,
    /// The app's own account.
    Owner,
    /// The second client.
    Collab,
    /// The optional read-only account ([`Server::has`] says whether it exists).
    Ro,
}

/// What the oracle sees of one project on the server.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub(super) struct ServerView {
    /// The project still exists for the run's accounts: the superuser's GET
    /// succeeds AND the owner's or the collaborator's project list holds it (an
    /// unreadable list counts as holding it). SpeleoDB soft-deletes, so after a
    /// delete the superuser may keep a view while the run's accounts lose it.
    pub(super) exists: bool,
    /// The lock holder's email (`active_mutex.user`), as the superuser's GET shows it.
    pub(super) lock_holder: Option<String>,
    /// Commit ids, in the order the revisions endpoint lists them (empty when
    /// the superuser cannot read the revisions).
    pub(super) commits: Vec<String>,
    /// Commit id -> its parent ids.
    pub(super) parents: BTreeMap<String, Vec<String>>,
    /// The topological tip ([`topological_tip`]).
    pub(super) tip: Option<String>,
    /// The project type, lowercase (`stalagmight` for every run project).
    pub(super) kind: String,
    pub(super) name: String,
    /// Each configured run account's own permission, from a GET of the project
    /// with that account's token; `None` when that GET fails (403, 404, any
    /// other error) or names no permission.
    pub(super) perms: BTreeMap<Who, Option<Permission>>,
}

/// The run's direct view of the instance.
pub(super) struct Server {
    /// The instance base URL (a checked loopback `http://` URL ending in `/`).
    pub(super) url: String,
    accounts: BTreeMap<Who, Account>,
    tokens: Mutex<BTreeMap<Who, String>>,
    /// `(project, sha)` of every commit [`Server::push`] stamped newer-schema.
    newer: Mutex<BTreeSet<(String, String)>>,
    /// Fingerprints of server commits by `(project, sha)`; a commit never changes.
    fp_cache: HashMap<(String, String), Result<Fp, String>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Server {
    /// Sign in every configured account (admin, owner, collab, and ro when the
    /// target has one) and confirm each token names its own account
    /// (`GET api/v2/user/`, emails compared case-insensitively).
    pub(super) fn connect(target: &FuzzTarget) -> Result<Self, String> {
        let (url, _) = check_target_url(&target.url)?;
        let mut accounts = BTreeMap::from([
            (Who::Admin, target.admin.clone()),
            (Who::Owner, target.owner.clone()),
            (Who::Collab, target.collab.clone()),
        ]);
        if let Some(ro) = &target.ro {
            accounts.insert(Who::Ro, ro.clone());
        }
        let mut tokens = BTreeMap::new();
        for (who, account) in &accounts {
            let token = client::login(&url, false, &account.email, &account.password)
                .map_err(|e| format!("sign in {who:?} ({}): {e}", account.email))?;
            tokens.insert(*who, token);
        }
        let server = Self {
            url,
            accounts,
            tokens: Mutex::new(tokens),
            newer: Mutex::new(BTreeSet::new()),
            fp_cache: HashMap::new(),
        };
        for (who, account) in &server.accounts {
            let named = server
                .as_account(*who, true, |t| {
                    client::fetch_user_email(&server.url, t, false)
                })
                .map_err(|e| format!("whoami {who:?}: {e}"))?;
            if !named.trim().eq_ignore_ascii_case(account.email.trim()) {
                return Err(format!(
                    "the {who:?} token signs in as {named}, not {}",
                    account.email
                ));
            }
        }
        Ok(server)
    }

    /// `who`'s email; empty for an account the target does not configure.
    pub(super) fn email(&self, who: Who) -> &str {
        self.accounts.get(&who).map_or("", |a| a.email.as_str())
    }

    /// Is `who` configured? Always true but for [`Who::Ro`], which is optional.
    pub(super) fn has(&self, who: Who) -> bool {
        self.accounts.contains_key(&who)
    }

    /// `who`'s current token (empty when not configured). For tests only.
    pub(super) fn token(&self, who: Who) -> String {
        lock(&self.tokens).get(&who).cloned().unwrap_or_default()
    }

    /// Run `op` with `who`'s token; on a stale-token answer sign `who` in again
    /// and run it once more. `whoami` marks `GET api/v2/user/`, where a stale
    /// token answers 403.
    fn as_account<T, F>(&self, who: Who, whoami: bool, op: F) -> Result<T, SpeleoDbError>
    where
        F: Fn(&str) -> Result<T, SpeleoDbError>,
    {
        if !self.has(who) {
            return Err(SpeleoDbError::NotAuthenticated);
        }
        retry_after_relogin(&self.token(who), whoami, op, || self.relogin(who))
    }

    fn relogin(&self, who: Who) -> Result<String, SpeleoDbError> {
        let account = self
            .accounts
            .get(&who)
            .ok_or(SpeleoDbError::NotAuthenticated)?;
        let token = client::login(&self.url, false, &account.email, &account.password)?;
        lock(&self.tokens).insert(who, token.clone());
        Ok(token)
    }

    /// A raw request as `who` ([`wire::request`]); a non-2xx answer is an
    /// `Err(Status)` carrying the first 200 characters of the body.
    fn wire_call(
        &self,
        who: Who,
        method: &str,
        path: &str,
        json: Option<&serde_json::Value>,
    ) -> Result<wire::Reply, SpeleoDbError> {
        let whoami = method == "GET" && path.trim_start_matches('/') == "api/v2/user/";
        self.as_account(who, whoami, |t| {
            ok_reply(wire::request(&self.url, method, path, Some(t), json))
        })
    }

    /// `who`'s own GET of project `id`.
    fn remote(&self, who: Who, id: &str) -> Result<RemoteProject, SpeleoDbError> {
        self.as_account(who, false, |t| client::get_project(&self.url, t, false, id))
    }

    /// Create an empty private STALAGMIGHT project named `name` as `who`.
    fn create_empty(&self, who: Who, name: &str) -> Result<String, String> {
        let payload = serde_json::json!({
            "name": name,
            "description": "fuzz project",
            "country": "US",
            "type": "STALAGMIGHT",
            "visibility": "PRIVATE",
        })
        .to_string();
        self.as_account(who, false, |t| {
            client::create_project_with(&self.url, t, false, &payload)
        })
        .map(|p| p.id)
        .map_err(|e| format!("create {name:?} as {who:?}: {e}"))
    }

    /// Create a private STALAGMIGHT project named `name` as `who`, then push
    /// `data::project_with(sections)` as its first revision (acquire, native
    /// upload, release). Returns the project id. The superuser cannot see it
    /// until [`Self::observe`].
    pub(super) fn create_project(
        &self,
        who: Who,
        name: &str,
        sections: &[(String, usize)],
    ) -> Result<String, String> {
        let id = self.create_empty(who, name)?;
        self.push(
            who,
            &id,
            &data::project_with(sections),
            false,
            false,
            "fuzz seed",
        )?;
        Ok(id)
    }

    /// Create a project as [`Self::create_project`] does but publish its first
    /// revision the way v0.5.x builds did: the `.stalx` (as `project.stalx`,
    /// `application/octet-stream`, multipart field `artifact`, plus a `message`
    /// field) through the GENERIC `PUT api/v2/projects/{id}/upload/auto/` route,
    /// between an acquire and a release. Older servers stored such an upload as
    /// `misc/project.stalx`. Returns the id; when the server refuses the upload
    /// the `Err` carries the HTTP status and the first 200 characters of the body,
    /// the lock is released and the empty project is deleted (best effort), so no
    /// project the caller never heard of stays in the accounts' lists.
    pub(super) fn create_legacy_project(
        &self,
        who: Who,
        name: &str,
        sections: &[(String, usize)],
    ) -> Result<String, String> {
        let id = self.create_empty(who, name)?;
        let uploaded = self.legacy_upload(who, &id, sections);
        if uploaded.is_err() {
            let _ = self.delete(who, &id);
        }
        uploaded.map(|()| id)
    }

    fn legacy_upload(
        &self,
        who: Who,
        id: &str,
        sections: &[(String, usize)],
    ) -> Result<(), String> {
        let dir = scratch()?;
        let path = dir.path().join("project.stalx");
        write_upload(&path, &data::project_with(sections), false)?;
        let bytes = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        self.hold(who, id)?;
        let route = format!("api/v2/projects/{id}/upload/auto/");
        let sent = self.as_account(who, false, |t| {
            ok_reply(wire::multipart_put(
                &self.url,
                &route,
                t,
                &[(
                    "artifact",
                    "project.stalx",
                    "application/octet-stream",
                    bytes.as_slice(),
                )],
                &[("message", "fuzz seed (legacy upload)")],
            ))
        });
        let released = self.release(who, id);
        sent.map_err(|e| format!("legacy upload of {id} as {who:?}: {e}"))?;
        released
    }

    /// Give `email` the permission `level` on `id`, as `who` (an ADMIN of it):
    /// `POST api/v2/projects/{id}/permission/user/detail/` `{user, level}`, and
    /// when that is refused (the grant may already exist) the same body as a
    /// `PUT`, which changes an existing grant.
    pub(super) fn grant(
        &self,
        who: Who,
        id: &str,
        email: &str,
        level: Permission,
    ) -> Result<(), String> {
        let path = permission_path(id);
        let body = serde_json::json!({ "user": email, "level": level_label(level) });
        let Err(added) = self.wire_call(who, "POST", &path, Some(&body)) else {
            return Ok(());
        };
        self.wire_call(who, "PUT", &path, Some(&body))
            .map(|_| ())
            .map_err(|updated| {
                format!(
                    "grant {email} {level:?} on {id} as {who:?}: add: {added}; update: {updated}"
                )
            })
    }

    /// Remove `email`'s permission on `id`, as `who`:
    /// `DELETE api/v2/projects/{id}/permission/user/detail/` `{user}`.
    pub(super) fn revoke(&self, who: Who, id: &str, email: &str) -> Result<(), String> {
        let body = serde_json::json!({ "user": email });
        self.wire_call(who, "DELETE", &permission_path(id), Some(&body))
            .map(|_| ())
            .map_err(|e| format!("revoke {email} on {id} as {who:?}: {e}"))
    }

    /// Let the superuser read `id` (READ_ONLY, granted by `who`, an ADMIN of it):
    /// a superuser has no implicit API access on SpeleoDB.
    pub(super) fn observe(&self, who: Who, id: &str) -> Result<(), String> {
        self.grant(who, id, self.email(Who::Admin), Permission::ReadOnly)
    }

    /// Take `id`'s write lock as `who` (`POST .../acquire/`), the server's answer
    /// as it is.
    pub(super) fn acquire(&self, who: Who, id: &str) -> Result<(), String> {
        self.as_account(who, false, |t| {
            client::acquire_lock(&self.url, t, false, id)
        })
        .map(|_| ())
        .map_err(|e| format!("acquire {id} as {who:?}: {e}"))
    }

    /// Hand `id`'s write lock back as `who` (`POST .../release/`).
    pub(super) fn release(&self, who: Who, id: &str) -> Result<(), String> {
        self.as_account(who, false, |t| {
            client::release_lock(&self.url, t, false, id)
        })
        .map(|_| ())
        .map_err(|e| format!("release {id} as {who:?}: {e}"))
    }

    /// [`Self::acquire`], skipped (no request) when `who`'s own GET shows `who`
    /// already holds the lock.
    fn hold(&self, who: Who, id: &str) -> Result<(), String> {
        let mine = self
            .remote(who, id)
            .ok()
            .and_then(|p| p.active_mutex)
            .is_some_and(|m| m.user.trim().eq_ignore_ascii_case(self.email(who).trim()));
        if mine {
            Ok(())
        } else {
            self.acquire(who, id)
        }
    }

    /// Push `project` as a new native revision of `id` as `who`, and return the
    /// commit sha the server answered. Saves a temp `.stalx`; when
    /// `newer_schema` it is stamped one schema version above this build
    /// ([`write_upload`]) and the sha is recorded for [`Self::is_newer`]. Then
    /// takes the lock (no request when `who` holds it already), uploads, and
    /// releases unless `keep_lock`; the release also runs after a failed
    /// upload. A push that landed but whose release failed is an `Err` that
    /// names the sha.
    pub(super) fn push(
        &self,
        who: Who,
        id: &str,
        project: &Project,
        newer_schema: bool,
        keep_lock: bool,
        message: &str,
    ) -> Result<String, String> {
        let dir = scratch()?;
        let path = dir.path().join("project.stalx");
        write_upload(&path, project, newer_schema)?;
        self.hold(who, id)?;
        let uploaded = self.as_account(who, false, |t| {
            client::upload_stalx(&self.url, t, false, id, &path, message)
        });
        let released = if keep_lock {
            Ok(())
        } else {
            self.release(who, id)
        };
        let sha = uploaded
            .map_err(|e| format!("upload {id} as {who:?}: {e}"))?
            .latest_commit
            .map(|c| c.id)
            .filter(|sha| !sha.is_empty())
            .ok_or_else(|| format!("upload {id} as {who:?}: the server named no commit"))?;
        if newer_schema {
            lock(&self.newer).insert((id.to_owned(), sha.clone()));
        }
        released.map_err(|e| format!("pushed {sha}, but: {e}"))?;
        Ok(sha)
    }

    /// Was `sha` of `id` pushed by [`Self::push`] with `newer_schema`?
    pub(super) fn is_newer(&self, id: &str, sha: &str) -> bool {
        lock(&self.newer).contains(&(id.to_owned(), sha.to_owned()))
    }

    /// Delete `id` as `who`: `DELETE api/v2/projects/{id}/` (a soft delete).
    pub(super) fn delete(&self, who: Who, id: &str) -> Result<(), String> {
        self.wire_call(who, "DELETE", &format!("api/v2/projects/{id}/"), None)
            .map(|_| ())
            .map_err(|e| format!("delete {id} as {who:?}: {e}"))
    }

    /// Rename `id` as `who` (`PATCH` `{"name": name}`).
    pub(super) fn rename(&self, who: Who, id: &str, name: &str) -> Result<(), String> {
        let payload = serde_json::json!({ "name": name }).to_string();
        self.as_account(who, false, |t| {
            client::patch_project(&self.url, t, false, id, &payload)
        })
        .map(|_| ())
        .map_err(|e| format!("rename {id} as {who:?}: {e}"))
    }

    /// The server's HEAD content of `id` as `who` downloads it, as a project
    /// `who` could build on (opened read-only, so a newer-schema tip is an `Err`).
    pub(super) fn tip_project(&self, who: Who, id: &str) -> Result<Project, String> {
        let file = self
            .as_account(who, false, |t| {
                client::download_stalx(&self.url, t, false, id)
            })
            .map_err(|e| format!("download {id} as {who:?}: {e}"))?;
        let db = Database::open_readonly(file.path())
            .map_err(|e| format!("open the tip of {id}: {e:#}"))?;
        db.load_project()
            .map_err(|e| format!("load the tip of {id}: {e:#}"))
    }

    /// The oracle's view of every project in `ids` (see [`ServerView`]). The
    /// owner's and the collaborator's project lists are fetched once for all ids.
    pub(super) fn views(&self, ids: &[String]) -> BTreeMap<String, ServerView> {
        let lists = [self.listed(Who::Owner), self.listed(Who::Collab)];
        ids.iter()
            .map(|id| (id.clone(), self.view(id, &lists)))
            .collect()
    }

    /// The ids in `who`'s project list (lowercase); `None` when it cannot be read.
    fn listed(&self, who: Who) -> Option<BTreeSet<String>> {
        self.as_account(who, false, |t| client::list_projects(&self.url, t, false))
            .ok()
            .map(|list| {
                list.into_iter()
                    .map(|p| p.id.to_ascii_lowercase())
                    .collect()
            })
    }

    fn view(&self, id: &str, lists: &[Option<BTreeSet<String>>]) -> ServerView {
        let admin = self.remote(Who::Admin, id);
        let perms = self
            .accounts
            .keys()
            .map(|&who| {
                let level = if who == Who::Admin {
                    admin.as_ref().ok().and_then(|p| p.permission)
                } else {
                    self.remote(who, id).ok().and_then(|p| p.permission)
                };
                (who, level)
            })
            .collect();
        let Ok(project) = admin else {
            return ServerView {
                perms,
                ..ServerView::default()
            };
        };
        let commits = self
            .as_account(Who::Admin, false, |t| {
                client::get_revisions(&self.url, t, false, id)
            })
            .map(|h| h.commits)
            .unwrap_or_default();
        ServerView {
            exists: exists_rule(true, lists, id),
            lock_holder: project.active_mutex.map(|m| m.user),
            tip: topological_tip(&commits),
            parents: commits
                .iter()
                .map(|c| (c.id.clone(), c.parent_ids.clone()))
                .collect(),
            commits: commits.into_iter().map(|c| c.id).collect(),
            kind: project.kind,
            name: project.name,
            perms,
        }
    }

    /// The fingerprint of commit `sha` of `id`, downloaded as the superuser.
    /// Answers are cached (a commit never changes), an unreadable file included;
    /// a failed download (transport or status) is never cached.
    pub(super) fn fingerprint_at(&mut self, id: &str, sha: &str) -> Result<Fp, String> {
        let key = (id.to_owned(), sha.to_owned());
        if let Some(hit) = self.fp_cache.get(&key) {
            return hit.clone();
        }
        let file = self
            .as_account(Who::Admin, false, |t| {
                client::download_stalx_at_hash(&self.url, t, false, id, sha)
            })
            .map_err(|e| format!("download {id}@{sha}: {e}"))?;
        let got = data::fingerprint_file(file.path());
        self.fp_cache.insert(key, got.clone());
        got
    }
}

/// Does `err` say the token is stale? A 401 anywhere, or a 403 from
/// `GET api/v2/user/` (`whoami`), which is how SpeleoDB answers a rotated
/// token there.
fn stale_token(err: &SpeleoDbError, whoami: bool) -> bool {
    match err {
        SpeleoDbError::Status { status: 401, .. } => true,
        SpeleoDbError::Status { status: 403, .. } => whoami,
        _ => false,
    }
}

/// Run `op` with `token`; when it fails with a stale token ([`stale_token`]),
/// get a fresh one from `relogin` and run `op` exactly once more. A failed
/// re-login returns the re-login's error; a second stale answer is returned as
/// it is (no loop). A 401 means the server did nothing, so the retry is safe
/// even for a non-idempotent call.
fn retry_after_relogin<T, F, L>(
    token: &str,
    whoami: bool,
    op: F,
    relogin: L,
) -> Result<T, SpeleoDbError>
where
    F: Fn(&str) -> Result<T, SpeleoDbError>,
    L: FnOnce() -> Result<String, SpeleoDbError>,
{
    match op(token) {
        Err(err) if stale_token(&err, whoami) => op(&relogin()?),
        done => done,
    }
}

/// A wire result as the client crate's error type: a send or parse failure is
/// `Transport`, a non-2xx reply is `Status` with the first 200 characters of
/// its body as the message.
fn ok_reply(sent: Result<wire::Reply, String>) -> Result<wire::Reply, SpeleoDbError> {
    let reply = sent.map_err(|detail| SpeleoDbError::Transport {
        cause: TransportCause::Unknown,
        detail,
    })?;
    if (200..300).contains(&reply.status) {
        Ok(reply)
    } else {
        Err(SpeleoDbError::Status {
            status: reply.status,
            message: String::from_utf8_lossy(&reply.body)
                .chars()
                .take(200)
                .collect(),
        })
    }
}

fn permission_path(id: &str) -> String {
    format!("api/v2/projects/{id}/permission/user/detail/")
}

/// The wire label of a permission level, as the grant endpoints take it.
fn level_label(level: Permission) -> &'static str {
    match level {
        Permission::WebViewer => "WEB_VIEWER",
        Permission::ReadOnly => "READ_ONLY",
        Permission::ReadAndWrite => "READ_AND_WRITE",
        Permission::Admin => "ADMIN",
    }
}

/// The oracle's existence rule: the superuser's GET succeeded (`admin_sees`)
/// AND one of the run accounts' project lists holds `id`, an unreadable list
/// (`None`) counting as holding it. Ids compare case-insensitively.
fn exists_rule(admin_sees: bool, lists: &[Option<BTreeSet<String>>], id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    admin_sees
        && lists
            .iter()
            .any(|list| list.as_ref().is_none_or(|set| set.contains(&id)))
}

/// The oracle's own definition of a history's tip: the commit no other listed
/// commit names as a parent. With several such heads (which a linear history
/// never has), the newest by `authored_date` (RFC 3339, compared as instants; an
/// unparseable date ranks oldest), and on a tie the one listed first. `None`
/// for an empty list or one where every commit is someone's parent.
pub(super) fn topological_tip(commits: &[RevisionCommit]) -> Option<String> {
    let named: HashSet<&str> = commits
        .iter()
        .flat_map(|c| c.parent_ids.iter().map(String::as_str))
        .collect();
    let mut best: Option<(&RevisionCommit, Option<DateTime<FixedOffset>>)> = None;
    for commit in commits.iter().filter(|c| !named.contains(c.id.as_str())) {
        let at = commit
            .authored_date
            .as_deref()
            .and_then(|d| DateTime::parse_from_rfc3339(d.trim()).ok());
        if best.as_ref().is_none_or(|(_, newest)| at > *newest) {
            best = Some((commit, at));
        }
    }
    best.map(|(commit, _)| commit.id.clone())
}

fn scratch() -> Result<TempDir, String> {
    tempfile::Builder::new()
        .prefix("fz-sdb-")
        .tempdir()
        .map_err(|e| format!("scratch dir: {e}"))
}

/// Save `project` as a fresh `.stalx` at `path` (replacing any file there).
/// With `newer_schema`, stamp it as a NEWER build's file looks to this one:
/// `PRAGMA user_version` and `project.schema_version` both set to this build's
/// `user_version` + 1, which every project opener of this build refuses with
/// `NewerSchema`.
fn write_upload(path: &Path, project: &Project, newer_schema: bool) -> Result<(), String> {
    let _ = std::fs::remove_file(path);
    {
        let mut db =
            Database::open(path).map_err(|e| format!("create {}: {e:#}", path.display()))?;
        db.save_project(project)
            .map_err(|e| format!("save {}: {e:#}", path.display()))?;
    }
    if !newer_schema {
        return Ok(());
    }
    let conn = rusqlite::Connection::open(path)
        .map_err(|e| format!("open {} raw: {e}", path.display()))?;
    let current: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| format!("read the schema stamp: {e}"))?;
    conn.execute_batch(&format!(
        "UPDATE project SET schema_version = {n}; PRAGMA user_version = {n};",
        n = current + 1
    ))
    .map_err(|e| format!("stamp newer: {e}"))
}

/// Append one JSON line to `path` (the run trace). Best effort: a trace that
/// cannot be written never fails the run.
pub(super) fn append_line(path: &Path, value: &serde_json::Value) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{value}");
    }
}

#[cfg(test)]
#[path = "world_tests.rs"]
mod tests;
