//! What is true after a step: the server (as the superuser and each run account
//! see it), the app (session, gate, open project, memory), and every `.stalx`
//! the user can reach. Files are read through a private COPY with their SQLite
//! sidecars, so reading never replays a hot journal into the real file (the
//! oracle must not change what the app finds next) while still showing what the
//! app's next read-write open will see.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash as _, Hasher as _};
use std::path::{Path, PathBuf};

use super::app::App;
use super::data::{fingerprint, fingerprint_file, Fp};
use super::world::{Server, ServerView, Who};
use crate::models::SpeleoLink;

/// One `.stalx` as the oracle read it.
#[derive(Clone, Debug)]
pub(super) struct FileSnap {
    pub(super) fp: Result<Fp, String>,
    pub(super) link: Result<Option<SpeleoLink>, String>,
    /// A hash of the file's bytes (sidecars excluded).
    pub(super) hash: u64,
    /// `PRAGMA integrity_check` after the next open's journal replay.
    pub(super) integrity: Result<(), String>,
    /// A rollback journal sits next to the file.
    pub(super) residue: bool,
    /// A newer Stalagmight schema wrote the file.
    pub(super) newer: bool,
}

/// The app as the oracle reads it.
#[derive(Clone, Debug, Default)]
pub(super) struct AppView {
    pub(super) logged_in: bool,
    /// The signed-in account, lowercased.
    pub(super) email: Option<String>,
    pub(super) endpoint: Option<String>,
    /// The backend edit gate.
    pub(super) gate: bool,
    pub(super) open_path: Option<PathBuf>,
    pub(super) mem: Option<Fp>,
    pub(super) link: Option<SpeleoLink>,
    pub(super) unsaved: bool,
    pub(super) unwound: bool,
    /// A token is saved in the app's SpeleoDB configuration.
    pub(super) saved_token: bool,
}

/// Everything true after a step.
#[derive(Clone, Debug, Default)]
pub(super) struct Snap {
    pub(super) server: BTreeMap<String, ServerView>,
    /// Every section name in any commit of each project of the run.
    pub(super) server_tokens: BTreeMap<String, BTreeSet<String>>,
    pub(super) app: AppView,
    pub(super) files: BTreeMap<PathBuf, FileSnap>,
    /// Every file under the cache root.
    pub(super) cache_files: Vec<PathBuf>,
    /// `(name, id)` of every project the owner's list shows.
    pub(super) owner_list: Vec<(String, String)>,
}

/// Every file under `dir`, recursively.
pub(super) fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

pub(super) fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// The sidecar `suffix` of `path` (`-journal`, `-wal`, `-shm`).
fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", path.display()))
}

/// Read one `.stalx` through a private copy (see the module doc).
pub(super) fn read_file(path: &Path) -> FileSnap {
    let hash = std::fs::read(path).map_or(0, |b| hash_bytes(&b));
    let residue = sidecar(path, "-journal").exists();
    let fail = |e: String| FileSnap {
        fp: Err(e.clone()),
        link: Err(e.clone()),
        hash,
        integrity: Err(e),
        residue,
        newer: false,
    };
    let Ok(dir) = tempfile::tempdir() else {
        return fail("no temp dir".to_owned());
    };
    let copy = dir.path().join("copy.stalx");
    if let Err(e) = std::fs::copy(path, &copy) {
        return fail(format!("copy: {e}"));
    }
    for suffix in ["-journal", "-wal", "-shm"] {
        let side = sidecar(path, suffix);
        if side.exists() {
            let _ = std::fs::copy(&side, sidecar(&copy, suffix));
        }
    }
    let newer = crate::persistence::Database::check_not_newer_at(&copy).is_err();
    let integrity = rusqlite::Connection::open(&copy)
        .and_then(|conn| conn.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0)))
        .map_err(|e| format!("integrity: {e}"))
        .and_then(|answer| if answer == "ok" { Ok(()) } else { Err(answer) });
    let fp = fingerprint_file(&copy);
    let link = crate::persistence::Database::open_readonly(&copy)
        .map_err(|e| format!("open: {e:#}"))
        .and_then(|db| db.load_speleo_link().map_err(|e| format!("link: {e:#}")));
    FileSnap {
        fp,
        link,
        hash,
        integrity,
        residue,
        newer,
    }
}

/// The app's side of a snapshot.
pub(super) fn app_view(app: &App) -> AppView {
    let st = app.st();
    let session = app.session();
    let (open_path, mem) = st
        .project
        .lock()
        .ok()
        .and_then(|p| p.as_ref().map(|p| (p.file_path.clone(), fingerprint(p))))
        .map_or((None, None), |(path, fp)| (path, Some(fp)));
    let saved_token = st.config.lock().is_ok_and(|c| c.speleodb.token.0.is_some());
    AppView {
        logged_in: session.logged_in,
        email: session.user_email.map(|e| e.to_ascii_lowercase()),
        endpoint: session.endpoint,
        gate: st.edit_enabled(),
        open_path,
        mem,
        link: st.speleodb_open_link().ok(),
        unsaved: app.front().unsaved,
        unwound: st.speleodb_unwound_from().is_some(),
        saved_token,
    }
}

/// Read the world. `ids` are the run's projects; `roots` the folders holding the
/// user's files (the cache root and the local-copies folder).
pub(super) fn snapshot(
    app: &App,
    server: &mut Server,
    ids: &[String],
    cache_root: &Path,
    copies: &Path,
) -> Snap {
    let mut snap = Snap {
        app: app_view(app),
        ..Snap::default()
    };
    for (id, view) in server.views(ids) {
        let mut tokens = BTreeSet::new();
        for sha in &view.commits {
            if let Ok(fp) = server.fingerprint_at(&id, sha) {
                tokens.extend(fp.keys().cloned());
            }
        }
        snap.server_tokens.insert(id.clone(), tokens);
        snap.server.insert(id, view);
    }
    if let Ok(list) =
        crate::speleodb::client::list_projects(&server.url, &server.token(Who::Owner), false)
    {
        snap.owner_list = list.into_iter().map(|p| (p.name, p.id)).collect();
    }
    walk(cache_root, &mut snap.cache_files);
    let mut stalx: Vec<PathBuf> = snap
        .cache_files
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n == "project.stalx"))
        .cloned()
        .collect();
    let mut local = Vec::new();
    walk(copies, &mut local);
    stalx.extend(
        local
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e == "stalx")),
    );
    for path in stalx {
        let file = read_file(&path);
        snap.files.insert(path, file);
    }
    snap
}
