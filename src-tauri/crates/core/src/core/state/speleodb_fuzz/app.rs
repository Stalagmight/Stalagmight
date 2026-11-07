//! The app under test: one `AppState`, driven through the SAME verbs the
//! frontend invokes, in the order it invokes them (the IPC sequences of
//! `src/hooks/useOpenRemoteProject.ts`, `useSpeleoActions.ts`,
//! `useRestoreRevision.ts`, the close / quit dialogs), minus the webview.
//!
//! What the frontend does between verbs is reproduced only where it changes
//! backend state: the full load after an open (`do_load_project_full`), the
//! autosave after a push, the close guards. The in-place tree swap
//! (`applySpeleoTreeReplacement`) only re-reads the backend, so it has no
//! counterpart here.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::model::{Act, LeaveMode, OnUnpushed};
use super::target::Account;
use crate::core::state::AppState;
use crate::core::Command;
use crate::models::{CommandErrorCode, SectionId};
use crate::speleodb::auth::{InMemoryStore, TokenStore};
use crate::speleodb::{
    DoneResult, PushResult, SpeleoErrorDto, SpeleoPublishParams, StartEditingResult, UpdateResult,
    Visibility,
};
use crate::test_fixtures::state_with_null_sink;

/// A project of the run: its server id and the name the list shows.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub(super) struct Proj {
    pub(super) id: String,
    pub(super) name: String,
}

/// What a step's result asserts about the world, for the oracle to verify.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(super) enum Claim {
    /// A new edit token is in the working copy.
    Edited(String),
    /// The local copy and the server tip are the same content, BASE is the tip,
    /// and the copy is marked clean (a confirmed push / Done / take-remote).
    InSync { project_id: String },
    /// BASE names the server tip (Update said up to date or fast-forwarded).
    BaseIsTip { project_id: String },
    /// The app handed the lock back on a deliberate verb.
    LockReleased { project_id: String },
    /// Every lock of the account was handed back.
    AllLocksReleased,
    /// A silent best-effort release ran (logout / close / quit) over a clean copy.
    LockReleaseBestEffort { project_id: String },
    /// The project was deleted on the server.
    Deleted { project_id: String },
    /// The user confirmed losing local edits (take remote, discard, a discarding
    /// open / restore / unwind); edits gone after this step are not silent.
    DisclosedLoss,
    /// The app crashed: edits that were only in memory are gone (not silent).
    Crashed,
    /// The step drops the copy's link on purpose (unlink, delete, local copy).
    LinkDropped,
    /// A project was published: the world grows.
    Published(Proj),
}

/// One refusal a verb returned: what the UI is handed.
#[derive(Clone, Debug, serde::Serialize)]
pub(super) struct Refusal {
    pub(super) verb: String,
    pub(super) code: String,
    pub(super) message: String,
}

/// The result of one step, for the trace and the oracle.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub(super) struct Outcome {
    pub(super) log: Vec<String>,
    pub(super) claims: Vec<Claim>,
    pub(super) refusals: Vec<Refusal>,
    pub(super) panicked: Option<String>,
}

impl Outcome {
    fn note(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
    }

    fn refused(&mut self, verb: &str, e: &SpeleoErrorDto) {
        self.refusals.push(Refusal {
            verb: verb.to_owned(),
            code: code_name(e.code),
            message: e.message.clone(),
        });
        self.note(format!(
            "{verb} -> Err({}: {})",
            code_name(e.code),
            clip(&e.message)
        ));
    }
}

pub(super) fn code_name(code: CommandErrorCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{code:?}"))
}

fn clip(s: &str) -> String {
    s.chars().take(160).collect()
}

/// The app process. `restart` replaces the `AppState` as a relaunch would; the
/// cache and config folders (process-wide) survive it, as they do on disk.
pub(super) struct App {
    pub(super) state: AppState,
    pub(super) proxy_url: String,
    pub(super) other_url: String,
    pub(super) owner: Account,
    copies_dir: PathBuf,
    edits: usize,
    copies: usize,
    publishes: usize,
    /// The frontend's `isDirty`: an edit not yet saved to the file.
    pub(super) unsaved: bool,
    /// The run tag, so names are unique on the shared instance.
    tag: String,
}

/// A fresh `AppState` as the app starts: the OS keychain swapped for an
/// in-memory store (a headless host has none, and the fuzzer must not touch a
/// real one), force-edit allowed so that verb is reachable.
fn fresh_state() -> AppState {
    let state = state_with_null_sink();
    {
        let mut config = state.config.lock();
        config.speleodb.allow_force_edit = true;
        config.speleodb.keychain_disabled = true;
        config.speleodb.endpoint = None;
    }
    state.speleodb.lock().token_store =
        TokenStore::from_stores(Box::new(InMemoryStore::new()), None);
    state
}

impl App {
    pub(super) fn new(
        proxy_url: String,
        other_url: String,
        owner: Account,
        copies_dir: PathBuf,
        tag: String,
    ) -> Self {
        Self {
            state: fresh_state(),
            proxy_url,
            other_url,
            owner,
            copies_dir,
            edits: 0,
            copies: 0,
            publishes: 0,
            unsaved: false,
            tag,
        }
    }

    /// The open project's file and link, as the backend holds them.
    pub(super) fn open_path(&self) -> Option<PathBuf> {
        self.state
            .project
            .lock()
            .as_ref()
            .and_then(|p| p.file_path.clone())
    }

    fn open_link(&self) -> Option<crate::models::SpeleoLink> {
        self.state
            .database
            .lock()
            .as_ref()
            .and_then(|db| db.load_speleo_link().ok().flatten())
    }

    /// The open linked project's id while the app edits it under the lock (gate
    /// open, not force-editing): the lock a silent release promises to hand back.
    /// A read-only copy holds no lock of the app's, whoever holds the account's.
    fn held_here(&self) -> Option<String> {
        if !self.state.edit_enabled() || self.state.force_editing() {
            return None;
        }
        self.open_link().map(|l| l.project_id)
    }

    pub(super) fn logged_in(&self) -> bool {
        self.state
            .speleodb_session_status()
            .is_ok_and(|s| s.logged_in)
    }

    pub(super) fn editing(&self) -> bool {
        self.state.edit_enabled() && !self.state.force_editing()
    }

    pub(super) fn hint_bits(&self) -> (bool, bool, bool, bool) {
        let link = self.open_link();
        (
            self.open_path().is_some(),
            link.is_some(),
            link.as_ref().is_some_and(|l| l.has_unpushed_edits),
            self.state.force_editing(),
        )
    }

    /// Run one app verb, catching a panic as the outcome it is.
    pub(super) fn run(&mut self, act: &Act, projects: &[Proj]) -> Outcome {
        let mut out = Outcome::default();
        let answer = catch_unwind(AssertUnwindSafe(|| self.dispatch(act, projects, &mut out)));
        if let Err(panic) = answer {
            let text = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_else(|| "non-string panic".to_owned());
            out.panicked = Some(text);
        }
        out
    }

    fn dispatch(&mut self, act: &Act, projects: &[Proj], out: &mut Outcome) {
        let pick = |p: u8| {
            projects
                .get(usize::from(p) % projects.len().max(1))
                .cloned()
        };
        match act {
            Act::Login => {
                self.login(&self.proxy_url.clone(), out);
            }
            Act::LoginOther => {
                // A sign-in that replaces the session (Settings shows the form while
                // signed in) leaves this server: a lock this process holds on it is
                // stranded unless handed back, as a logout hands it back.
                let clean = !self.hint_bits().2;
                let held = self.held_here();
                let logged_in = self.login(&self.other_url.clone(), out);
                if let (true, true, Some(project_id)) = (logged_in, clean, held) {
                    out.claims.push(Claim::LockReleaseBestEffort { project_id });
                }
            }
            Act::Logout => {
                let clean = !self.hint_bits().2;
                let held = self.held_here();
                match self.state.speleodb_logout() {
                    Ok(()) => {
                        out.note("logout -> Ok");
                        if let (true, Some(project_id)) = (clean, held) {
                            out.claims.push(Claim::LockReleaseBestEffort { project_id });
                        }
                    }
                    Err(e) => out.refused("logout", &e),
                }
            }
            Act::ListProjects => match self.state.speleodb_list_projects() {
                Ok(list) => out.note(format!("list -> {} projects", list.len())),
                Err(e) => out.refused("list_projects", &e),
            },
            Act::Open { p, on_unpushed } => {
                if let Some(proj) = pick(*p) {
                    self.open(&proj, *on_unpushed, out);
                }
            }
            Act::StartEditing => {
                let answer = self.state.speleodb_start_editing();
                self.started(answer, "start_editing", out);
            }
            Act::ResumeHeldLock => {
                let answer = self.state.speleodb_resume_held_lock_editing();
                self.started(answer, "resume_held_lock", out);
            }
            Act::ForceEdit => match self.state.speleodb_force_edit() {
                Ok(()) => out.note("force_edit -> Ok"),
                Err(e) => out.refused("force_edit", &e),
            },
            Act::Edit => self.edit(out),
            Act::EditShot => self.edit_shot(out),
            Act::Save => self.save(out),
            Act::Push => self.push(out),
            Act::Done => {
                let _ = self.done(out);
            }
            Act::Update => self.update(out),
            Act::TakeRemote => self.take_remote(out),
            Act::Discard { keep_lock } => self.discard(*keep_lock, out),
            Act::View { back } => self.revision(*back, false, false, out),
            Act::Restore { back, discard } => self.revision(*back, true, *discard, out),
            Act::Unwind => self.unwind(out),
            Act::Rewind => self.rewind(out),
            Act::Poll => self.poll(out),
            Act::ReleaseCurrentLock => {
                let pid = self.open_link().map(|l| l.project_id);
                match self.state.speleodb_release_current_lock() {
                    Ok(()) => {
                        out.note("release_current_lock -> Ok");
                        if let Some(project_id) = pid {
                            out.claims.push(Claim::LockReleased { project_id });
                        }
                    }
                    Err(e) => out.refused("release_current_lock", &e),
                }
            }
            Act::ReleaseAllLocks => match self.state.speleodb_release_all_locks() {
                Ok(()) => {
                    out.note("release_all_locks -> Ok");
                    out.claims.push(Claim::AllLocksReleased);
                }
                Err(e) => out.refused("release_all_locks", &e),
            },
            Act::Close { mode } => self.close(*mode, out),
            Act::Quit { mode } => self.quit(*mode, out),
            Act::Crash => {
                out.note("crash: the process dies (no save, no release)");
                out.claims.push(Claim::Crashed);
                self.restart();
            }
            Act::Unlink => {
                let held = self.held_here();
                // [#576 FZ-SDB-5] Unlink moves the copy out of the cache, to the path
                // the Save As answered; the frontend then loads it.
                if self.unlink_to_a_copy("unlink", out) {
                    // Unlink hands a held lock back best-effort (it abandons the link,
                    // so nothing could release it from this copy again).
                    if let Some(project_id) = held {
                        out.claims.push(Claim::LockReleaseBestEffort { project_id });
                    }
                }
            }
            Act::SaveLocalCopy => self.save_local_copy(out),
            Act::DeleteProject => {
                let pid = self.open_link().map(|l| l.project_id);
                match self.state.speleodb_delete_project() {
                    Ok(()) => {
                        out.note("delete_project -> Ok");
                        out.claims.push(Claim::LinkDropped);
                        if let Some(project_id) = pid {
                            out.claims.push(Claim::Deleted { project_id });
                        }
                    }
                    Err(e) => out.refused("delete_project", &e),
                }
            }
            Act::Publish => self.publish(out),
            _ => out.note("not an app step"),
        }
    }

    /// Sign in as the owner at `endpoint`; whether it succeeded.
    fn login(&self, endpoint: &str, out: &mut Outcome) -> bool {
        match self.state.speleodb_login(
            Some(endpoint.to_owned()),
            Some(self.owner.email.clone()),
            Some(self.owner.password.clone()),
            None,
        ) {
            Ok(r) => {
                out.note(format!("login {} -> Ok", r.endpoint));
                true
            }
            Err(e) => {
                out.refused("login", &e);
                false
            }
        }
    }

    /// The status poll (`speleodb_link_status`, the frontend's `refreshLinkStatus`).
    fn poll(&self, out: &mut Outcome) {
        match self.state.speleodb_link_status() {
            Ok(s) => out.note(format!(
                "link_status -> remote={} edit={} dirty={} state={:?} holder={:?}",
                s.is_remote, s.edit_enabled, s.has_unpushed_edits, s.remote_state, s.lock_holder
            )),
            Err(e) => out.refused("link_status", &e),
        }
    }

    /// The frontend's full load after an open (`projectStore.ts` `loadProject`): no
    /// tree is fetched when that file is already the open project (the backend
    /// answers `ALREADY_OPEN`, which the frontend treats as success and swaps the
    /// tree in place). Both branches end in `refreshLinkStatus`, the status poll.
    fn load(&mut self, path: &Path, out: &mut Outcome) {
        if self.open_path().as_deref() == Some(path) {
            out.note("load -> already open (in-place swap)");
            self.poll(out);
            return;
        }
        match self.state.do_load_project_full(path.to_path_buf()) {
            Ok(_) => {
                self.unsaved = false;
                out.note(format!("load {} -> Ok", short(path)));
                self.poll(out);
            }
            Err(e) => {
                out.refusals.push(Refusal {
                    verb: "load_project_full".to_owned(),
                    code: code_name(e.code),
                    message: e.message.clone(),
                });
                out.note(format!(
                    "load -> Err({}: {})",
                    code_name(e.code),
                    clip(&e.message)
                ));
            }
        }
    }

    /// The open guard (`useOpenRemoteProject.ts`): no open over a copy with
    /// un-pushed edits, a held lock or unsaved changes.
    fn leave_guard(&self) -> Option<&'static str> {
        let (open, linked, dirty, force_editing) = self.hint_bits();
        if !open {
            None
        } else if dirty {
            Some("un-pushed edits")
        } else if linked && self.state.edit_enabled() && !force_editing {
            // An unlinked (local) project's edit gate is simply open: no lock.
            Some("editing (lock held)")
        } else if self.unsaved {
            Some("unsaved changes")
        } else {
            None
        }
    }

    fn open(&mut self, proj: &Proj, on_unpushed: OnUnpushed, out: &mut Outcome) {
        if let Some(why) = self.leave_guard() {
            out.note(format!("open: the frontend guard refuses ({why})"));
            return;
        }
        let mut entry: Option<String> = None;
        let mut discarding = false;
        for _ in 0..3 {
            let answer = if discarding {
                self.state.speleodb_open_project_discarding_local(
                    proj.id.clone(),
                    proj.name.clone(),
                    None,
                    entry.clone(),
                )
            } else {
                self.state.speleodb_open_project(
                    proj.id.clone(),
                    proj.name.clone(),
                    None,
                    entry.clone(),
                )
            };
            match answer {
                Ok(res) => {
                    out.note(format!(
                        "open -> Ok (concurrent={})",
                        res.concurrent_session
                    ));
                    self.load(Path::new(&res.local_path), out);
                    return;
                }
                Err(e) => {
                    out.refused(
                        if discarding {
                            "open_discarding_local"
                        } else {
                            "open"
                        },
                        &e,
                    );
                    match e.code {
                        CommandErrorCode::SpeleodbUnpushedLocalEdits => match on_unpushed {
                            OnUnpushed::KeepLocal => {
                                if let Some(local) = e.unpushed_local {
                                    self.load(Path::new(&local.local_path), out);
                                }
                                return;
                            }
                            OnUnpushed::TakeServer if !discarding => {
                                out.claims.push(Claim::DisclosedLoss);
                                discarding = true;
                            }
                            OnUnpushed::TakeServer => return,
                        },
                        CommandErrorCode::SpeleodbAdopt => {
                            self.adopt(proj, discarding, entry, out);
                            return;
                        }
                        CommandErrorCode::SpeleodbAmbiguous => {
                            let first = e
                                .dump_candidates
                                .and_then(|c| c.into_iter().next())
                                .map(|c| c.entry_path);
                            if first.is_none() || entry.is_some() {
                                return;
                            }
                            entry = first;
                        }
                        _ => return,
                    }
                }
            }
        }
    }

    fn adopt(
        &mut self,
        proj: &Proj,
        discard_local: bool,
        entry: Option<String>,
        out: &mut Outcome,
    ) {
        match self.state.speleodb_adopt_ariane(
            proj.id.clone(),
            proj.name.clone(),
            None,
            discard_local,
            entry,
        ) {
            Ok(res) => {
                out.note(format!(
                    "adopt -> Ok(upload={:?}, flipped={})",
                    res.upload, res.type_flipped
                ));
                if matches!(res.upload, crate::speleodb::AdoptUpload::Pushed) {
                    out.claims.push(Claim::InSync {
                        project_id: proj.id.clone(),
                    });
                }
                self.load(Path::new(&res.local_path), out);
            }
            Err(e) => out.refused("adopt_ariane", &e),
        }
    }

    fn started(
        &mut self,
        answer: Result<StartEditingResult, SpeleoErrorDto>,
        verb: &str,
        out: &mut Outcome,
    ) {
        match answer {
            Ok(StartEditingResult::FastForwarded { local_path }) => {
                out.note(format!("{verb} -> Ok(FastForwarded)"));
                self.load(Path::new(&local_path), out);
            }
            Ok(r) => out.note(format!("{verb} -> Ok({r:?})")),
            Err(e) => out.refused(verb, &e),
        }
    }

    fn edit(&mut self, out: &mut Outcome) {
        self.edits += 1;
        let token = format!("E{}", self.edits);
        let r = self.state.execute(Command::CreateSection {
            name: Some(token.clone()),
            surveyor: None,
            date: None,
            color: "#abcdef".to_owned(),
            direction: None,
            shots: (0..=self.edits % 3).map(super::world::shot).collect(),
        });
        if r.success {
            self.unsaved = true;
            out.note(format!("edit {token} -> Ok"));
            out.claims.push(Claim::Edited(token));
        } else {
            out.note(format!("edit {token} -> refused {:?}", r.code));
        }
    }

    fn edit_shot(&mut self, out: &mut Outcome) {
        let target: Option<SectionId> = self.state.project.lock().as_ref().and_then(|p| {
            p.sections
                .iter()
                .rev()
                .find(|s| s.name.as_deref().is_some_and(|n| n.starts_with('E')))
                .map(|s| s.id)
        });
        let Some(section_id) = target else {
            out.note("edit_shot: no fuzz section to extend");
            return;
        };
        let r = self.state.execute(Command::CreateShot {
            section_id,
            shot: super::world::shot(self.edits + 7),
            after_shot_id: None,
            before_shot_id: None,
        });
        if r.success {
            self.unsaved = true;
        }
        out.note(format!(
            "edit_shot -> {}",
            if r.success { "Ok" } else { "refused" }
        ));
    }

    fn save(&mut self, out: &mut Outcome) {
        let r = self.state.execute(Command::SaveProject { path: None });
        if r.success {
            self.unsaved = false;
        }
        out.note(format!(
            "save -> {}",
            if r.success {
                "Ok".to_owned()
            } else {
                format!("refused {:?}", r.code)
            }
        ));
    }

    fn push(&mut self, out: &mut Outcome) {
        // Only an upload makes the copy and the tip one: a push over a clean copy
        // sends nothing and claims nothing about the tip.
        let pid = self
            .open_link()
            .filter(|l| l.has_unpushed_edits)
            .map(|l| l.project_id);
        match self
            .state
            .speleodb_push_only_reported(Some("fuzz push".to_owned()), false)
        {
            Ok(report) => {
                out.note(format!("push -> Ok({:?})", report.result));
                if matches!(report.result, PushResult::Pushed) {
                    self.save(out);
                    if let Some(project_id) = pid {
                        out.claims.push(Claim::InSync { project_id });
                    }
                }
            }
            Err(e) => out.refused("push", &e),
        }
    }

    /// Done; `true` when it released.
    fn done(&mut self, out: &mut Outcome) -> bool {
        let link = self.open_link();
        let uploads = link.as_ref().is_some_and(|l| l.has_unpushed_edits);
        let pid = link.map(|l| l.project_id);
        match self
            .state
            .speleodb_done_reported(Some("fuzz done".to_owned()), false)
        {
            Ok(report) => {
                out.note(format!("done -> Ok({:?})", report.result));
                let released = matches!(report.result, DoneResult::Released);
                if released {
                    self.save(out);
                }
                if let (true, true, Some(project_id)) = (released, uploads, pid.clone()) {
                    out.claims.push(Claim::InSync { project_id });
                }
                if let (DoneResult::Released, Some(project_id)) = (report.result, pid) {
                    out.claims.push(Claim::LockReleased { project_id });
                }
                released
            }
            Err(e) => {
                out.refused("done", &e);
                false
            }
        }
    }

    fn update(&mut self, out: &mut Outcome) {
        let pid = self.open_link().map(|l| l.project_id);
        match self.state.speleodb_update() {
            Ok(r) => {
                out.note(format!("update -> Ok({})", short_debug(&r)));
                if let UpdateResult::FastForwarded { local_path } = &r {
                    self.load(Path::new(local_path), out);
                }
                if matches!(
                    r,
                    UpdateResult::UpToDate | UpdateResult::FastForwarded { .. }
                ) {
                    if let Some(project_id) = pid {
                        out.claims.push(Claim::BaseIsTip { project_id });
                    }
                }
            }
            Err(e) => out.refused("update", &e),
        }
    }

    fn take_remote(&mut self, out: &mut Outcome) {
        let pid = self.open_link().map(|l| l.project_id);
        let held = self.held_here();
        out.claims.push(Claim::DisclosedLoss);
        match self.state.speleodb_take_remote() {
            Ok(path) => {
                out.note("take_remote -> Ok");
                self.load(Path::new(&path), out);
                if let Some(project_id) = pid {
                    out.claims.push(Claim::InSync { project_id });
                }
                if let Some(project_id) = held {
                    out.claims.push(Claim::LockReleaseBestEffort { project_id });
                }
            }
            Err(e) => out.refused("take_remote", &e),
        }
    }

    fn discard(&mut self, keep_lock: bool, out: &mut Outcome) {
        let pid = self.open_link().map(|l| l.project_id);
        out.claims.push(Claim::DisclosedLoss);
        match self.state.speleodb_discard_changes(keep_lock, true) {
            Ok(r) => {
                self.unsaved = false;
                out.note(format!("discard(keep_lock={keep_lock}) -> Ok({r:?})"));
                // "Throw the local copy away and come back at the server tip."
                if let Some(project_id) = pid {
                    out.claims.push(Claim::InSync {
                        project_id: project_id.clone(),
                    });
                    if r.is_none() {
                        out.claims.push(Claim::LockReleased { project_id });
                    }
                }
            }
            Err(e) => out.refused("discard_changes", &e),
        }
    }

    /// The open copy's project id and name (from its link), for the history verbs.
    fn linked_project(&self) -> Option<(String, String)> {
        self.open_link().map(|l| (l.project_id, l.project_name))
    }

    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "the verb's own flags, passed through"
    )]
    fn restore_call(
        &mut self,
        hexsha: &str,
        restore: bool,
        discard_local: bool,
        keep_undo: bool,
        discard_unpushed: bool,
        chain_forward: bool,
        out: &mut Outcome,
    ) -> Result<(), Box<SpeleoErrorDto>> {
        let Some((id, name)) = self.linked_project() else {
            out.note("restore: no linked project open");
            return Ok(());
        };
        let mut entry: Option<String> = None;
        loop {
            match self.state.speleodb_restore_revision(
                id.clone(),
                name.clone(),
                hexsha.to_owned(),
                restore,
                discard_local,
                entry.clone(),
                keep_undo,
                discard_unpushed,
                chain_forward,
            ) {
                Ok(res) => {
                    out.note(format!("restore_revision(restore={restore}) -> Ok"));
                    self.load(Path::new(&res.local_path), out);
                    return Ok(());
                }
                Err(e) if e.code == CommandErrorCode::SpeleodbAmbiguous && entry.is_none() => {
                    out.refused("restore_revision", &e);
                    entry = e
                        .dump_candidates
                        .and_then(|c| c.into_iter().next())
                        .map(|c| c.entry_path);
                    if entry.is_none() {
                        return Ok(());
                    }
                }
                Err(e) => return Err(Box::new(e)),
            }
        }
    }

    fn revision(&mut self, back: u8, restore: bool, discard: bool, out: &mut Outcome) {
        let Some((id, _)) = self.linked_project() else {
            out.note("revision: no linked project open");
            return;
        };
        let revisions = match self.state.speleodb_get_revisions(id) {
            Ok(r) => r,
            Err(e) => {
                out.refused("get_revisions", &e);
                return;
            }
        };
        let Some(rev) = revisions.get(usize::from(back) % revisions.len().max(1)) else {
            out.note("revision: empty history");
            return;
        };
        let sha = rev.id.clone();
        match self.restore_call(&sha, restore, false, false, false, false, out) {
            Ok(()) => {}
            Err(e)
                if e.code == CommandErrorCode::SpeleodbUnpushedLocalEdits && restore && discard =>
            {
                out.refused("restore_revision", &e);
                out.claims.push(Claim::DisclosedLoss);
                if let Err(e) = self.restore_call(&sha, true, true, false, false, false, out) {
                    out.refused("restore_revision(discard_local)", &e);
                }
            }
            Err(e) => out.refused("restore_revision", &e),
        }
    }

    fn unwind(&mut self, out: &mut Outcome) {
        match self.state.speleodb_unwind_target() {
            Ok(Some(t)) => {
                if t.discards_unpushed {
                    out.claims.push(Claim::DisclosedLoss);
                }
                if let Err(e) =
                    self.restore_call(&t.hexsha, true, true, true, t.discards_unpushed, false, out)
                {
                    out.refused("restore_revision(unwind)", &e);
                }
            }
            Ok(None) => out.note("unwind_target -> None"),
            Err(e) => out.refused("unwind_target", &e),
        }
    }

    fn rewind(&mut self, out: &mut Outcome) {
        match self.state.speleodb_rewind_target() {
            Ok(Some(t)) => {
                if let Err(e) = self.restore_call(&t.hexsha, true, true, true, false, true, out) {
                    out.refused("restore_revision(rewind)", &e);
                }
            }
            Ok(None) => out.note("rewind_target -> None"),
            Err(e) => out.refused("rewind_target", &e),
        }
    }

    /// `close_project(releaseLock)` as the command runs it.
    fn close_project(&mut self, release: bool, out: &mut Outcome) {
        let (_, _, dirty, _) = self.hint_bits();
        let held = self.held_here();
        if release {
            self.state.speleodb_release_held_lock_best_effort();
            if let (false, Some(project_id)) = (dirty, held) {
                out.claims.push(Claim::LockReleaseBestEffort { project_id });
            }
        } else {
            self.state.speleodb_keep_open_lock_deliberately();
        }
        let r = self.state.execute(Command::CloseProject);
        self.unsaved = false;
        out.note(format!("close(release={release}) -> {}", r.success));
    }

    fn close(&mut self, mode: LeaveMode, out: &mut Outcome) {
        if self.open_path().is_none() {
            out.note("close: nothing open");
            return;
        }
        if self.unsaved {
            self.save(out);
        }
        let (_, linked, dirty, _) = self.hint_bits();
        // `useProject.ts`: the close dialog is raised for un-pushed edits or a held lock.
        if dirty || (linked && self.editing()) {
            match mode {
                LeaveMode::PushRelease => {
                    if self.done(out) {
                        self.close_project(true, out);
                    } else {
                        out.note("close dialog stays open (Done did not release)");
                    }
                }
                LeaveMode::KeepLock | LeaveMode::DiscardRelease => self.close_project(false, out),
            }
        } else {
            self.close_project(true, out);
        }
    }

    fn quit(&mut self, mode: LeaveMode, out: &mut Outcome) {
        if self.unsaved {
            self.save(out);
        }
        let (_, _, dirty, _) = self.hint_bits();
        if dirty {
            match mode {
                LeaveMode::PushRelease => {
                    if !self.done(out) {
                        out.note("quit dialog: Done did not release; the user quits anyway keeping the lock");
                    }
                }
                LeaveMode::KeepLock => out.note("quit: keep lock"),
                LeaveMode::DiscardRelease => {
                    out.claims.push(Claim::DisclosedLoss);
                    match self.state.speleodb_release_current_lock() {
                        Ok(()) => out.note("release_current_lock -> Ok"),
                        Err(e) => out.refused("release_current_lock", &e),
                    }
                }
            }
        }
        // The exit hook (`lib.rs`, RunEvent::ExitRequested).
        let still_dirty = self.hint_bits().2;
        let held = self.held_here();
        self.state
            .speleodb_release_held_lock_bounded(Duration::from_secs(5));
        if let (false, Some(project_id)) = (still_dirty, held) {
            out.claims.push(Claim::LockReleaseBestEffort { project_id });
        }
        out.note("quit -> restart");
        self.restart();
    }

    /// [#576 FZ-SDB-5] The keep-my-copy flows (no access, divergence): the same
    /// move out of the cache as Unlink, since the copy the user picks IS the move.
    fn save_local_copy(&mut self, out: &mut Outcome) {
        self.unlink_to_a_copy("save_local_copy", out);
    }

    /// Unlink to the next copy path and load it, as the frontend does after its Save
    /// As. Returns whether the unlink answered Ok.
    fn unlink_to_a_copy(&mut self, what: &str, out: &mut Outcome) -> bool {
        if self.open_path().is_none() {
            out.note(format!("{what}: nothing open"));
            return false;
        }
        self.copies += 1;
        let path = self.copies_dir.join(format!("copy-{}.stalx", self.copies));
        match self.state.speleodb_unlink(path.clone()) {
            Ok(_) => {
                out.note(format!("{what}: unlink to a copy -> Ok"));
                out.claims.push(Claim::LinkDropped);
                // The copy holds everything the session held; the session is closed.
                self.unsaved = false;
                self.load(&path, out);
                true
            }
            Err(e) => {
                out.refused("unlink", &e);
                false
            }
        }
    }

    fn publish(&mut self, out: &mut Outcome) {
        if self.open_path().is_none() || self.open_link().is_some() {
            out.note("publish: needs an open local (unlinked) project");
            return;
        }
        self.publishes += 1;
        let name = format!("{}-pub{}", self.tag, self.publishes);
        match self.state.speleodb_publish(SpeleoPublishParams {
            name: name.clone(),
            description: "fuzz publish".to_owned(),
            country: "US".to_owned(),
            visibility: Visibility::Private,
            latitude: None,
            longitude: None,
        }) {
            Ok(report) => {
                out.note("publish -> Ok");
                let proj = Proj {
                    id: report.result.project_id.clone(),
                    name,
                };
                out.claims.push(Claim::Published(proj));
                self.load(Path::new(&report.result.local_path), out);
                // The initial push is best-effort: one that did not confirm leaves the
                // copy marked un-pushed for a later Done, which claims nothing.
                if !self.hint_bits().2 {
                    out.claims.push(Claim::InSync {
                        project_id: report.result.project_id,
                    });
                }
            }
            Err(e) => out.refused("publish", &e),
        }
    }

    /// Relaunch: a fresh `AppState` over the same folders, signed out (the token
    /// lived in memory only).
    pub(super) fn restart(&mut self) {
        let old = std::mem::replace(&mut self.state, fresh_state());
        drop(old);
        self.unsaved = false;
    }

    /// The endpoint the app session is signed in to, if any.
    pub(super) fn session_endpoint(&self) -> Option<String> {
        self.state.speleodb.lock().endpoint.clone()
    }
}

fn short(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn short_debug(r: &UpdateResult) -> &'static str {
    match r {
        UpdateResult::UpToDate => "UpToDate",
        UpdateResult::FastForwarded { .. } => "FastForwarded",
        UpdateResult::DivergenceNeeded => "DivergenceNeeded",
    }
}
