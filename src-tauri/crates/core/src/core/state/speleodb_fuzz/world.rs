//! The server side of a fuzz run: accounts, the per-run projects, the oracle's
//! view of the truth, and the second client.
//!
//! Every call here goes DIRECTLY to the instance (never through the fault proxy):
//! the oracle must see the truth, and the second client is another computer.

use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::target::{Account, FuzzTarget};
use super::wire;
use crate::models::{Project, SectionData, ShotData};
use crate::persistence::Database;
use crate::speleodb::client;
use crate::speleodb::dump::Dump;
use crate::speleodb::models::RevisionCommit;
use crate::speleodb::Permission;

/// A survey's content as the invariants compare it: section name -> shot count.
pub(super) type Fp = BTreeMap<String, usize>;

/// The fingerprint of a loaded project.
pub(super) fn fingerprint(project: &Project) -> Fp {
    project
        .sections
        .iter()
        .map(|s| (s.name.clone().unwrap_or_default(), s.shots.len()))
        .collect()
}

/// Read a `.stalx` from disk without writing to it. `Err` carries why not (a
/// newer-format file, a half-written file).
pub(super) fn fingerprint_file(path: &Path) -> Result<Fp, String> {
    let db = Database::open_readonly(path).map_err(|e| format!("open: {e:#}"))?;
    let project = db.load_project().map_err(|e| format!("load: {e:#}"))?;
    Ok(fingerprint(&project))
}

/// The `speleo_link` row of a `.stalx` on disk, read without writing to it.
pub(super) fn link_of_file(path: &Path) -> Result<Option<crate::models::SpeleoLink>, String> {
    let db = Database::open_readonly(path).map_err(|e| format!("open: {e:#}"))?;
    db.load_speleo_link().map_err(|e| format!("link: {e:#}"))
}

/// One signed-in account.
#[derive(Clone)]
pub(super) struct Session {
    pub(super) email: String,
    pub(super) token: String,
}

/// What the oracle sees of one project on the server.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub(super) struct ServerView {
    /// The project answers a superuser GET and is still active (not soft-deleted).
    pub(super) exists: bool,
    pub(super) lock_holder: Option<String>,
    /// Commit ids, as the revisions endpoint lists them.
    pub(super) commits: Vec<String>,
    /// The topological tip (the commit no other commit names as a parent).
    pub(super) tip: Option<String>,
    pub(super) kind: String,
    pub(super) name: String,
}

/// A shot the fuzz edits and seeds use; `n` varies the geometry.
pub(super) fn shot(n: usize) -> ShotData {
    let az = f64::from(u32::try_from(n % 36).unwrap_or(0)) * 10.0;
    ShotData {
        length: 3.0 + f64::from(u32::try_from(n % 5).unwrap_or(0)),
        azimuth_in: az,
        azimuth_out: (az + 180.0) % 360.0,
        depth_in: 3.0,
        depth_out: 3.0,
        ..Default::default()
    }
}

/// A section with `shots` shots named `name`.
pub(super) fn section(name: &str, shots: usize) -> SectionData {
    SectionData {
        name: Some(name.to_owned()),
        color: "#4ecdc4".to_owned(),
        shots: (0..shots).map(shot).collect(),
        ..Default::default()
    }
}

/// A synthetic project holding `sections`.
pub(super) fn project_with(sections: &[(String, usize)]) -> Project {
    let mut project = Project::new("FuzzCave".to_owned(), None);
    for (name, shots) in sections {
        project.add_section(section(name, *shots));
    }
    project
}

/// Save `project` as a `.stalx` + its `.tml` sidecar in `dir`; returns both paths.
fn write_upload_pair(
    dir: &Path,
    project: &Project,
    newer: bool,
) -> Result<(PathBuf, PathBuf), String> {
    let stalx = dir.join("upload.stalx");
    let tml = dir.join("upload.tml");
    let _ = std::fs::remove_file(&stalx);
    {
        let mut db = Database::open(&stalx).map_err(|e| format!("create .stalx: {e:#}"))?;
        db.save_project(project)
            .map_err(|e| format!("save .stalx: {e:#}"))?;
    }
    if newer {
        // What a NEWER build's file looks like to this one: stamped one schema
        // version above (as `newer_download_refusal_tests` builds it).
        let conn = rusqlite::Connection::open(&stalx).map_err(|e| format!("open raw: {e}"))?;
        let current: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|e| format!("read stamp: {e}"))?;
        conn.execute_batch(&format!(
            "UPDATE project SET schema_version = {n}; PRAGMA user_version = {n};",
            n = current + 1
        ))
        .map_err(|e| format!("stamp newer: {e}"))?;
    }
    crate::export::tml::export_tml_for_speleodb(project, &tml)
        .map_err(|e| format!("export .tml: {e:#}"))?;
    Ok((stalx, tml))
}

/// The run's view of the instance.
pub(super) struct Server {
    pub(super) url: String,
    /// The oracle's eyes. A superuser has NO implicit API access on SpeleoDB, so
    /// every project of a run grants this account READ_ONLY ([`Self::observe`]).
    pub(super) admin: Session,
    pub(super) owner: Session,
    pub(super) collab: Session,
    /// Fingerprints of server commits, by `(project, sha)`; a commit never changes.
    fp_cache: HashMap<(String, String), Result<Fp, String>>,
    scratch: TempDir,
}

fn login(url: &str, account: &Account) -> Result<Session, String> {
    let token = client::login(url, false, &account.email, &account.password)
        .map_err(|e| format!("login {}: {e}", account.email))?;
    Ok(Session {
        email: account.email.clone(),
        token,
    })
}

impl Server {
    pub(super) fn connect(target: &FuzzTarget) -> Result<Self, String> {
        Ok(Self {
            url: target.url.clone(),
            admin: login(&target.url, &target.admin)?,
            owner: login(&target.url, &target.owner)?,
            collab: login(&target.url, &target.collab)?,
            fp_cache: HashMap::new(),
            scratch: TempDir::new().map_err(|e| format!("scratch: {e}"))?,
        })
    }

    fn pick(&self, who: Who) -> &Session {
        match who {
            Who::Admin => &self.admin,
            Who::Owner => &self.owner,
            Who::Collab => &self.collab,
        }
    }

    /// Create a `type: other` project as `who` and push `sections` as its first
    /// `.stalx` revision (lock taken and handed back). Returns the id.
    pub(super) fn create_stalx_project(
        &self,
        who: Who,
        name: &str,
        sections: &[(String, usize)],
    ) -> Result<String, String> {
        let s = self.pick(who);
        let id = client::create_project(&self.url, &s.token, false, name)
            .map_err(|e| format!("create {name}: {e}"))?
            .id;
        self.push_project(who, &id, &project_with(sections), false, "seed")?;
        Ok(id)
    }

    /// Create a `type: ariane` project as `who` whose only file is an
    /// `ariane.tml` exported from `sections`. Returns the id.
    pub(super) fn create_ariane_project(
        &self,
        who: Who,
        name: &str,
        sections: &[(String, usize)],
    ) -> Result<String, String> {
        let s = self.pick(who);
        let payload = serde_json::json!({
            "name": name,
            "description": "fuzz ariane project",
            "country": "US",
            "type": "ariane",
        })
        .to_string();
        let id = client::create_project_with(&self.url, &s.token, false, &payload)
            .map_err(|e| format!("create ariane {name}: {e}"))?
            .id;
        let dir = self.scratch.path();
        let (_, tml) = write_upload_pair(dir, &project_with(sections), false)?;
        let tml_bytes = std::fs::read(&tml).map_err(|e| format!("read tml: {e}"))?;
        client::acquire_lock(&self.url, &s.token, false, &id).map_err(|e| format!("lock: {e}"))?;
        let (status, body) = wire::multipart_put(
            &self.url,
            &format!("api/v2/projects/{id}/upload/auto/"),
            &s.token,
            &[(
                "artifact",
                "ariane.tml",
                "application/octet-stream",
                &tml_bytes,
            )],
            "seed ariane",
        )?;
        let _ = client::release_lock(&self.url, &s.token, false, &id);
        if !(200..300).contains(&status) {
            return Err(format!(
                "ariane upload: HTTP {status}: {}",
                String::from_utf8_lossy(&body)
                    .chars()
                    .take(200)
                    .collect::<String>()
            ));
        }
        Ok(id)
    }

    /// Grant `email` `level` on `id`, as `who` (an admin of the project).
    pub(super) fn grant(
        &self,
        who: Who,
        id: &str,
        email: &str,
        level: Permission,
    ) -> Result<(), String> {
        let s = self.pick(who);
        match client::add_user_permission(&self.url, &s.token, false, id, email, level) {
            Ok(()) => Ok(()),
            Err(_) => client::set_user_permission(&self.url, &s.token, false, id, email, level)
                .map_err(|e| format!("grant {email} {level:?} on {id}: {e}")),
        }
    }

    /// Let the oracle's account read `id` (granted by `who`, an admin of it).
    pub(super) fn observe(&self, who: Who, id: &str) -> Result<(), String> {
        self.grant(who, id, &self.admin.email.clone(), Permission::ReadOnly)
    }

    pub(super) fn revoke(&self, who: Who, id: &str, email: &str) -> Result<(), String> {
        let s = self.pick(who);
        client::remove_user_permission(&self.url, &s.token, false, id, email)
            .map_err(|e| format!("revoke {email} on {id}: {e}"))
    }

    /// Push `project` as a new revision of `id` as `who`: acquire, upload, release.
    /// When `who` already holds the lock the acquire refreshes it and the release
    /// hands it back all the same.
    pub(super) fn push_project(
        &self,
        who: Who,
        id: &str,
        project: &Project,
        newer: bool,
        message: &str,
    ) -> Result<(), String> {
        let s = self.pick(who);
        let dir = TempDir::new().map_err(|e| format!("tmp: {e}"))?;
        let (stalx, tml) = write_upload_pair(dir.path(), project, newer)?;
        client::acquire_lock(&self.url, &s.token, false, id).map_err(|e| format!("lock: {e}"))?;
        let up = client::upload_auto(
            &client::TestWire,
            &self.url,
            &s.token,
            false,
            id,
            &stalx,
            &tml,
            message,
        );
        let rel = client::release_lock(&self.url, &s.token, false, id);
        up.map_err(|e| format!("upload: {e}"))?;
        rel.map_err(|e| format!("release: {e}"))?;
        Ok(())
    }

    pub(super) fn acquire(&self, who: Who, id: &str) -> Result<(), String> {
        let s = self.pick(who);
        client::acquire_lock(&self.url, &s.token, false, id)
            .map(|_| ())
            .map_err(|e| format!("{e}"))
    }

    pub(super) fn release(&self, who: Who, id: &str) -> Result<(), String> {
        let s = self.pick(who);
        client::release_lock(&self.url, &s.token, false, id)
            .map(|_| ())
            .map_err(|e| format!("{e}"))
    }

    pub(super) fn delete(&self, who: Who, id: &str) -> Result<(), String> {
        let s = self.pick(who);
        client::delete_project(&self.url, &s.token, false, id, &self.url)
            .map_err(|e| format!("{e}"))
    }

    pub(super) fn rename(&self, who: Who, id: &str, name: &str) -> Result<(), String> {
        let s = self.pick(who);
        let payload = serde_json::json!({ "name": name }).to_string();
        client::patch_project(&client::TestWire, &self.url, &s.token, false, id, &payload)
            .map(|_| ())
            .map_err(|e| format!("{e}"))
    }

    /// The server's tip content for `id` as a project `who` could build on.
    pub(super) fn tip_project(&self, who: Who, id: &str) -> Result<Project, String> {
        let token = &self.pick(who).token;
        let bytes = client::download_dump(&client::TestWire, &self.url, token, false, id)
            .map_err(|e| format!("download: {e}"))?;
        let dir = TempDir::new().map_err(|e| format!("tmp: {e}"))?;
        let path = extract_stalx(&bytes, dir.path())?.ok_or("the tip holds no .stalx")?;
        let db = Database::open_readonly(&path).map_err(|e| format!("open tip: {e:#}"))?;
        db.load_project().map_err(|e| format!("load tip: {e:#}"))
    }

    /// The oracle's view of every project in `ids`, as the superuser sees it.
    /// "Exists" means an account of the run can still see it: a soft delete drops
    /// every permission, so the run's accounts lose it while the superuser keeps it.
    /// Each account's list is fetched once for all of `ids`.
    pub(super) fn views(&self, ids: &[String]) -> BTreeMap<String, ServerView> {
        let visible = |token: &str| -> Option<std::collections::HashSet<String>> {
            client::list_projects(&self.url, token, false)
                .ok()
                .map(|list| list.into_iter().map(|p| p.id).collect())
        };
        let lists = [visible(&self.owner.token), visible(&self.collab.token)];
        ids.iter()
            .map(|id| {
                // An unreadable list says nothing: count the project as present.
                let active = lists
                    .iter()
                    .any(|l| l.as_ref().is_none_or(|set| set.contains(id)));
                (id.clone(), self.view(id, active))
            })
            .collect()
    }

    fn view(&self, id: &str, active: bool) -> ServerView {
        let Ok(project) = client::get_project(&self.url, &self.admin.token, false, id) else {
            return ServerView::default();
        };
        let commits = client::get_revisions(&self.url, &self.admin.token, false, id)
            .map(|h| h.commits)
            .unwrap_or_default();
        ServerView {
            exists: active,
            lock_holder: project.active_mutex.map(|m| m.user),
            tip: topological_tip(&commits),
            commits: commits.into_iter().map(|c| c.id).collect(),
            kind: project.kind,
            name: project.name,
        }
    }

    /// The fingerprint of commit `sha` of `id` (cached; commits are immutable).
    pub(super) fn fingerprint_at(&mut self, id: &str, sha: &str) -> Result<Fp, String> {
        let key = (id.to_owned(), sha.to_owned());
        if let Some(hit) = self.fp_cache.get(&key) {
            return hit.clone();
        }
        let got = self.fetch_fingerprint(id, sha);
        // A transport hiccup is not a fact about the commit: only cache answers.
        if got.is_ok() || got.as_ref().is_err_and(|e| !e.starts_with("download")) {
            self.fp_cache.insert(key, got.clone());
        }
        got
    }

    fn fetch_fingerprint(&self, id: &str, sha: &str) -> Result<Fp, String> {
        let bytes = client::download_dump_at_hash(
            &client::TestWire,
            &self.url,
            &self.admin.token,
            false,
            id,
            sha,
        )
        .map_err(|e| format!("download {sha}: {e}"))?;
        let dir = TempDir::new().map_err(|e| format!("tmp: {e}"))?;
        match extract_stalx(&bytes, dir.path())? {
            Some(path) => fingerprint_file(&path),
            None => Err("no .stalx in this commit".to_owned()),
        }
    }
}

/// Which account an actor step runs as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) enum Who {
    Admin,
    Owner,
    Collab,
}

/// The commit no other listed commit names as a parent (the newest by author
/// date when there are several heads, which a linear history never has).
pub(super) fn topological_tip(commits: &[RevisionCommit]) -> Option<String> {
    let parents: std::collections::HashSet<&str> = commits
        .iter()
        .flat_map(|c| c.parent_ids.iter().map(String::as_str))
        .collect();
    commits
        .iter()
        .filter(|c| !parents.contains(c.id.as_str()))
        .max_by(|a, b| a.authored_date.cmp(&b.authored_date))
        .map(|c| c.id.clone())
}

/// Extract the one `.stalx` of a dump into `dir`; `None` when the dump has none.
fn extract_stalx(zip: &[u8], dir: &Path) -> Result<Option<PathBuf>, String> {
    let mut dump = Dump::open(zip).map_err(|e| format!("zip: {e}"))?;
    let entries = dump
        .entry_paths()
        .map_err(|e| format!("zip entries: {e}"))?;
    let Some(entry) = entries.iter().find(|p| {
        Path::new(p.as_str())
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("stalx"))
    }) else {
        return Ok(None);
    };
    let dest = dir.join("commit.stalx");
    dump.extract_entry_to(entry, &dest)
        .map_err(|e| format!("extract: {e}"))?;
    Ok(Some(dest))
}

/// Append one JSON line to `path` (the run trace).
pub(super) fn append_line(path: &Path, value: &serde_json::Value) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{value}");
    }
}
