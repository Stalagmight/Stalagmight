//! The app flows: each step's IPC sequence, in the frontend's order, with the
//! frontend's own guards (unsaved-work prompts answered "Discard changes", the
//! read-only refusal over pending work, the upload dialog's preconditions).
//! What the frontend does between verbs is reproduced only where it changes
//! backend state; a tree replacement only re-reads the backend, so it is a
//! status refresh here.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::app::{plant_hot_journal, short, App, Claim, Exit, Outcome, Proj};
use super::model::{Acct, Act};
use crate::core::Command;
use crate::speleodb::{
    DoneResult, PushResult, SpeleoDbAction, SpeleoPublishParams, StartEditingResult, UpdateResult,
    Visibility,
};

/// The exit hook's deadline (`EXIT_LOCK_RELEASE_TIMEOUT` in the root crate).
pub(super) const EXIT_RELEASE_DEADLINE: Duration = Duration::from_secs(5);

/// The commit message the server's 1024-character column cannot hold.
fn long_message() -> String {
    "fuzz long message ".repeat(64)
}

pub(super) fn dispatch(app: &App, act: &Act, projects: &[Proj], out: &mut Outcome) {
    let pick = |p: u8| {
        projects
            .get(usize::from(p) % projects.len().max(1))
            .cloned()
    };
    match act {
        Act::Login { acct } => login(app, *acct, out),
        Act::LoginOther => login_at(
            app,
            &app.other_url.clone(),
            &app.accounts.owner.clone(),
            out,
        ),
        Act::Logout => logout(app, out),
        Act::ListProjects => match app.st().speleodb_list_projects() {
            Ok(list) => out.note(format!("list -> {} projects", list.len())),
            Err(e) => out.refused("list_projects", &e),
        },
        Act::Open { p, take_server } => {
            if let Some(proj) = pick(*p) {
                open_remote(app, &proj, *take_server, out);
            }
        }
        Act::OpenFile { p } => {
            if let Some(proj) = pick(*p) {
                let path = app.cache_file(&proj.id);
                if path.exists() {
                    open_file(app, &path, out);
                } else {
                    out.note("open_file: no cached copy");
                }
            }
        }
        Act::NewLocal => new_local(app, out),
        Act::StartEditing => {
            let answer = app.st().speleodb_start_editing();
            started(app, answer, "start_editing", out);
        }
        Act::Edit => edit(app, out),
        Act::EditShot => edit_shot(app, out),
        Act::Autosave => {
            if app.front().unsaved {
                save(app, None, out);
            } else {
                out.note("autosave: nothing unsaved");
            }
        }
        Act::SaveChanges { long_message } => save_changes(app, *long_message, out),
        Act::SaveAndClose => save_and_close(app, out),
        Act::Close => close(app, out),
        Act::Quit => quit(app, out),
        Act::Crash { residue } => crash(app, *residue, out),
        Act::RestartUpdate => {
            out.claims.extend(app.leaving(Exit::RestartUpdate));
            out.claims.push(Claim::Crashed);
            out.note("restart-to-update: the process restarts without the exit hook");
            app.restart();
        }
        Act::ReadOnly => read_only(app, out),
        Act::Reload => reload(app, out),
        Act::Refresh => link_status(app, out),
        Act::TakeRemote => take_remote(app, out),
        Act::DiscardKeep => discard_keep(app, out),
        Act::View { back } => super::history::view(app, *back, out),
        Act::Revert { back } => super::history::revert(app, *back, out),
        Act::Unwind => super::history::unwind(app, false, out),
        Act::Rewind => super::history::unwind(app, true, out),
        Act::SaveCopy => save_copy(app, out),
        Act::Publish => super::history::publish(app, out),
        Act::ConfigTouch => {
            let st = app.st();
            match st.update_config(st.get_config()) {
                Ok(()) => out.note("update_config -> Ok"),
                Err(e) => out.note(format!("update_config -> Err({e:#})")),
            }
        }
        Act::StateRead => {
            let open = app.st().get_project_state().is_some();
            out.note(format!("get_project_state -> open={open}"));
        }
        _ => out.note("not an app step"),
    }
}

/// `speleodb_link_status`, as every refresh site calls it.
pub(super) fn link_status(app: &App, out: &mut Outcome) {
    match app.st().speleodb_link_status() {
        Ok(s) => out.note(format!(
            "link_status -> remote={} edit={} dirty={} state={:?} holder={:?} perm={:?}",
            s.is_remote,
            s.edit_enabled,
            s.has_unpushed_edits,
            s.remote_state,
            s.lock_holder,
            s.permission
        )),
        Err(e) => out.refused("link_status", &e),
    }
}

/// `projectStore.loadProject(path, opts)`: the access preflight unless confirmed
/// (or the path is already open), the binary load, the post-load status reads,
/// then auto-acquire for a remote project unless `acquire` is false. A switch to
/// another project releases the old one's lock in the load's finalize step.
pub(super) fn load(app: &App, path: &Path, confirmed: bool, acquire: bool, out: &mut Outcome) {
    let st = app.st();
    let open = app.open_path();
    if !confirmed && open.as_deref() != Some(path) {
        if let Err(e) = st.speleodb_project_access_at(path.to_string_lossy().into_owned(), false) {
            out.refused("project_access_at", &e);
            return;
        }
    }
    let leaving = match open.as_deref() {
        Some(o) if o != path => app.leaving(Exit::Switch),
        _ => None,
    };
    st.cancel_loading();
    match st.do_load_project_full(path.to_path_buf()) {
        Ok(_) => {
            app.front().unsaved = false;
            out.claims.extend(leaving);
            out.note(format!("load {} -> Ok", short(path)));
            link_status(app, out);
            let _ = st.speleodb_session_status();
            if acquire && app.open_link().is_some() {
                auto_acquire(app, out);
            }
        }
        Err(e) if e.code == "ALREADY_OPEN" => {
            out.note("load -> already open (in-place swap)");
            link_status(app, out);
        }
        Err(e) => {
            out.note(format!(
                "load -> Err({}: {})",
                e.code,
                super::app::clip(&e.message)
            ));
            out.refusals.push(super::app::Refusal {
                verb: "load_project_full".to_owned(),
                action: e.code.clone(),
                status: None,
                message: e.message.clone(),
                newer_schema: e.code.to_ascii_uppercase().contains("NEWER"),
            });
        }
    }
}

/// `autoAcquireSpeleoLock`: Start Editing, the fast-forward tree swap, a status
/// refresh.
pub(super) fn auto_acquire(app: &App, out: &mut Outcome) {
    let answer = app.st().speleodb_start_editing();
    started(app, answer, "start_editing(auto)", out);
}

/// Route a Start Editing answer as the frontend does.
pub(super) fn started(
    app: &App,
    answer: Result<StartEditingResult, crate::speleodb::SpeleoErrorDto>,
    verb: &str,
    out: &mut Outcome,
) {
    match answer {
        Ok(StartEditingResult::FastForwarded { local_path }) => {
            out.note(format!("{verb} -> Ok(FastForwarded)"));
            load(app, Path::new(&local_path), true, false, out);
        }
        Ok(r) => {
            out.note(format!("{verb} -> Ok({r:?})"));
            link_status(app, out);
        }
        Err(e) => out.refused(verb, &e),
    }
}

/// A discard confirmation the frontend raises over unsaved work.
fn discard_unsaved(app: &App, out: &mut Outcome) {
    if app.front().unsaved {
        out.claims.push(Claim::DiscardedUnsaved);
        out.note("prompt: discard unsaved changes");
    }
}

/// `close_project(releaseLock: true)`, as the command runs it.
pub(super) fn close_project(app: &App, out: &mut Outcome) {
    let st = app.st();
    st.speleodb_release_held_lock_on_close();
    let r = st.execute(Command::CloseProject);
    app.front().unsaved = false;
    out.note(format!("close_project -> {}", r.success));
}

/// `protectAccountTransition`: unsaved work is discarded (confirmed) by closing
/// and reopening the same file read-only before the account changes.
fn protect_account_transition(app: &App, out: &mut Outcome) {
    if !app.front().unsaved {
        return;
    }
    let Some(path) = app.open_path() else {
        return;
    };
    discard_unsaved(app, out);
    close_project(app, out);
    load(app, &path, true, false, out);
}

fn login(app: &App, acct: Acct, out: &mut Outcome) {
    let account = match acct {
        Acct::Owner => app.accounts.owner.clone(),
        Acct::Collab => app.accounts.collab.clone(),
        Acct::Ro => {
            let Some(a) = app.accounts.ro.clone() else {
                out.note("login ro: no read-only account configured");
                return;
            };
            a
        }
        Acct::Admin => {
            out.note("the app never signs in as the superuser");
            return;
        }
    };
    login_at(app, &app.proxy_url.clone(), &account, out);
}

fn login_at(app: &App, endpoint: &str, account: &super::target::Account, out: &mut Outcome) {
    let session = app.session();
    let same = session
        .user_email
        .as_deref()
        .is_some_and(|e| e.eq_ignore_ascii_case(&account.email))
        && session
            .endpoint
            .as_deref()
            .is_some_and(|e| e.trim_end_matches('/') == endpoint.trim_end_matches('/'));
    if !same {
        protect_account_transition(app, out);
    }
    let leaving = if same {
        None
    } else {
        app.leaving(Exit::LoginSwitch)
    };
    match app.st().speleodb_login(
        Some(endpoint.to_owned()),
        Some(account.email.clone()),
        Some(account.password.clone()),
        None,
    ) {
        Ok(r) => {
            out.note(format!("login {} -> Ok", r.user_email));
            out.claims.extend(leaving);
            if let Err(e) = app.st().speleodb_list_projects() {
                out.refused("list_projects", &e);
            }
            link_status(app, out);
        }
        Err(e) => out.refused("login", &e),
    }
}

fn logout(app: &App, out: &mut Outcome) {
    protect_account_transition(app, out);
    let leaving = app.leaving(Exit::Logout);
    match app.st().speleodb_logout() {
        Ok(()) => out.note("logout -> Ok"),
        Err(e) => out.refused("logout", &e),
    }
    out.claims.extend(leaving);
}

/// `useOpenRemoteProject.openRemote`.
pub(super) fn open_remote(app: &App, proj: &Proj, take_server: bool, out: &mut Outcome) {
    discard_unsaved(app, out);
    let st = app.st();
    let answer = st.speleodb_open_project(proj.id.clone(), proj.name.clone(), None);
    let res = match answer {
        Ok(res) => res,
        Err(e) => {
            out.refused("open_project", &e);
            if e.action != SpeleoDbAction::UnpushedLocalEdits || e.unpushed_local.is_none() {
                return;
            }
            if !take_server {
                out.note("unpushed-local prompt: Cancel");
                return;
            }
            out.claims.push(Claim::DiscardedLocal);
            match st.speleodb_open_project_discarding_local(
                proj.id.clone(),
                proj.name.clone(),
                None,
            ) {
                Ok(res) => res,
                Err(e) => {
                    out.refused("open_project_discarding_local", &e);
                    return;
                }
            }
        }
    };
    if res.offline == Some(true) {
        out.claims.push(Claim::OfflineOpen {
            project_id: proj.id.clone(),
        });
    }
    out.note(format!("open_project -> Ok(offline={:?})", res.offline));
    load(app, Path::new(&res.local_path), true, true, out);
}

/// `routeOpenedFile`: the file-open path (Ctrl+O, recents of local files).
fn open_file(app: &App, path: &Path, out: &mut Outcome) {
    let unsaved = app.front().unsaved;
    if unsaved && app.open_path().as_deref() != Some(path) {
        discard_unsaved(app, out);
        if app.open_link().is_some() {
            // `speleodb-close {pendingOpenPath}`: access check, close, load.
            if let Err(e) = app
                .st()
                .speleodb_project_access_at(path.to_string_lossy().into_owned(), false)
            {
                out.refused("project_access_at", &e);
                return;
            }
            out.claims.extend(app.leaving(Exit::Close));
            close_project(app, out);
            load(app, path, true, true, out);
            return;
        }
    }
    load(app, path, false, true, out);
}

fn new_local(app: &App, out: &mut Outcome) {
    let n = {
        let mut front = app.front();
        front.locals += 1;
        front.locals
    };
    let _ = std::fs::create_dir_all(&app.copies_dir);
    let path = app.copies_dir.join(format!("local-{n}.stalx"));
    if let Err(e) = super::data::write_stalx(&path, &[(format!("L{n}"), 2)]) {
        out.note(format!("new_local: {e}"));
        return;
    }
    open_file(app, &path, out);
}

fn edit(app: &App, out: &mut Outcome) {
    let n = {
        let mut front = app.front();
        front.edits += 1;
        front.edits
    };
    let token = format!("E{n}");
    let r = app.st().execute(Command::CreateSection {
        name: Some(token.clone()),
        surveyor: None,
        date: None,
        color: "#abcdef".to_owned(),
        direction: None,
        shots: (0..=n % 3).map(super::data::shot).collect(),
    });
    if r.success {
        app.front().unsaved = true;
        out.note(format!("edit {token} -> Ok"));
        out.claims.push(Claim::Edited(token));
    } else {
        out.note(format!("edit {token} -> refused {:?}", r.error));
    }
}

fn edit_shot(app: &App, out: &mut Outcome) {
    let st = app.st();
    let target = st.project.lock().ok().and_then(|p| {
        p.as_ref()?
            .sections
            .iter()
            .rev()
            .find(|s| s.name.as_deref().is_some_and(|n| n.starts_with('E')))
            .map(|s| s.id)
    });
    let Some(section_id) = target else {
        out.note("edit_shot: no fuzz section to extend");
        return;
    };
    let n = app.front().edits + 7;
    let r = st.execute(Command::CreateShot {
        section_id,
        shot: super::data::shot(n),
        after_shot_id: None,
        before_shot_id: None,
    });
    if r.success {
        app.front().unsaved = true;
    }
    out.note(format!("edit_shot -> {}", r.success));
}

/// `save_project`: `Some(path)` is a Save As.
pub(super) fn save(app: &App, path: Option<PathBuf>, out: &mut Outcome) -> bool {
    let r = app.st().execute(Command::SaveProject { path });
    if r.success {
        app.front().unsaved = false;
    }
    out.note(format!("save -> {} {:?}", r.success, r.error));
    r.success
}

fn message(app: &App) -> String {
    let mut front = app.front();
    front.messages += 1;
    format!("fuzz upload {}", front.messages)
}

/// Save Changes (`dispatchProjectSave` then `SpeleoDoneDialog` in push mode).
fn save_changes(app: &App, long: bool, out: &mut Outcome) {
    let Some(link) = app.open_link() else {
        if app.open_path().is_some() {
            save(app, None, out);
        }
        return;
    };
    let unsaved = app.front().unsaved;
    if !link.has_unpushed_edits && !unsaved && app.editing() {
        out.note("save changes: No changes to upload");
        return;
    }
    if !app.editing() || !app.session().logged_in {
        out.note("save changes: blocked (no lock or signed out)");
        return;
    }
    if !save(app, None, out) {
        return;
    }
    let text = if long { long_message() } else { message(app) };
    let unwound = app.st().speleodb_unwound_from().is_some();
    let pending = app.open_link().is_some_and(|l| l.has_unpushed_edits);
    if unwound && pending {
        // The unwound-push dialog: "Push anyway" uploads the older revision.
        out.claims.push(Claim::Reverted {
            project_id: link.project_id.clone(),
        });
    }
    match app
        .st()
        .speleodb_push_only_acknowledged(Some(text), unwound)
    {
        Ok(r) => {
            out.note(format!("push -> Ok({r:?})"));
            if long {
                out.claims.push(Claim::LongMessage {
                    project_id: link.project_id.clone(),
                });
            }
            if matches!(r, PushResult::Pushed) {
                if pending {
                    out.claims.push(Claim::Uploaded {
                        project_id: link.project_id,
                    });
                }
                save(app, None, out);
            }
        }
        Err(e) => {
            if long {
                out.claims.push(Claim::LongMessage {
                    project_id: link.project_id,
                });
            }
            out.refused("push", &e);
        }
    }
}

/// Save and Close (`SpeleoDoneDialog` with the close intent).
fn save_and_close(app: &App, out: &mut Outcome) {
    let Some(link) = app.open_link() else {
        if app.open_path().is_some() && save(app, None, out) {
            close_project(app, out);
        }
        return;
    };
    if !app.editing() || !app.session().logged_in {
        out.note("save and close: blocked (no lock or signed out)");
        return;
    }
    if !save(app, None, out) {
        return;
    }
    let pending = app.open_link().is_some_and(|l| l.has_unpushed_edits);
    let leaving = app.leaving(Exit::Close);
    let text = message(app);
    match app.st().speleodb_done_acknowledged(Some(text), false) {
        Ok(DoneResult::Released) => {
            out.note("done -> Ok(Released)");
            out.claims.push(Claim::Released {
                project_id: link.project_id.clone(),
            });
            if pending {
                out.claims.push(Claim::Uploaded {
                    project_id: link.project_id,
                });
            }
            save(app, None, out);
            let clean =
                !app.front().unsaved && !app.open_link().is_some_and(|l| l.has_unpushed_edits);
            if clean {
                out.claims.extend(leaving);
                close_project(app, out);
            } else {
                out.note("save and close: newer changes, the project stays open");
            }
        }
        Ok(r) => out.note(format!("done -> Ok({r:?})")),
        Err(e) => out.refused("done", &e),
    }
}

fn close(app: &App, out: &mut Outcome) {
    if app.open_path().is_none() {
        out.note("close: nothing open");
        return;
    }
    discard_unsaved(app, out);
    out.claims.extend(app.leaving(Exit::Close));
    close_project(app, out);
}

/// Quit: `guardQuit` (unsaved work on a linked project closes it first), then the
/// exit hook's bounded release, then the process is gone.
fn quit(app: &App, out: &mut Outcome) {
    let unsaved = app.front().unsaved;
    let leaving = app.leaving(Exit::Quit);
    if unsaved {
        discard_unsaved(app, out);
        if app.open_link().is_some() {
            close_project(app, out);
        }
    }
    app.st()
        .speleodb_release_held_lock_bounded(EXIT_RELEASE_DEADLINE);
    out.claims.extend(leaving);
    out.note("quit -> relaunch");
    app.restart();
}

fn crash(app: &App, residue: bool, out: &mut Outcome) {
    let target = app.open_link().zip(app.open_path());
    out.claims.push(Claim::Crashed);
    if app.front().unsaved {
        out.claims.push(Claim::DiscardedUnsaved);
    }
    out.note("crash: the process dies (no save, no release)");
    app.restart();
    if let (true, Some((link, path))) = (residue, target) {
        match plant_hot_journal(&path) {
            Ok(()) => {
                out.note(format!(
                    "crash residue: hot journal beside {}",
                    short(&path)
                ));
                out.claims.push(Claim::Residue {
                    project_id: link.project_id,
                });
            }
            Err(e) => out.note(format!("crash residue: not planted: {e}")),
        }
    }
}

/// The release context the frontend reads from `project_access_at(localOnly)`.
pub(super) fn release_context(
    app: &App,
    out: &mut Outcome,
) -> Option<crate::speleodb::SpeleoReleaseContext> {
    let path = app.open_path()?;
    match app
        .st()
        .speleodb_project_access_at(path.to_string_lossy().into_owned(), true)
    {
        Ok(Some(access)) => access.release_context,
        Ok(None) => None,
        Err(e) => {
            out.refused("project_access_at", &e);
            None
        }
    }
}

/// Switch to Read Only Mode (`releaseOpenProjectLock`).
pub(super) fn read_only(app: &App, out: &mut Outcome) {
    let Some(link) = app.open_link() else {
        out.note("read only: no linked project");
        return;
    };
    if app.front().unsaved || link.has_unpushed_edits {
        out.note("read only: refused by the frontend (pending work)");
        return;
    }
    let Some(ctx) = release_context(app, out) else {
        out.note("read only: no release context");
        return;
    };
    match app.st().speleodb_release_current_lock(Some(ctx)) {
        Ok(()) => {
            out.note("release_current_lock -> Ok");
            out.claims.push(Claim::Released {
                project_id: link.project_id,
            });
            link_status(app, out);
        }
        Err(e) => out.refused("release_current_lock", &e),
    }
}

/// Reload Project (`useSpeleoActions.reload`).
pub(super) fn reload(app: &App, out: &mut Outcome) {
    let Some(link) = app.open_link() else {
        out.note("reload: no linked project");
        return;
    };
    if !app.session().logged_in {
        out.note("reload: disabled while signed out");
        return;
    }
    let discard = app.front().unsaved || link.has_unpushed_edits;
    if discard {
        out.note("prompt: discard pending work and reload");
        out.claims.push(Claim::DiscardedUnsaved);
        out.claims.push(Claim::DiscardedLocal);
    }
    let Some(ctx) = release_context(app, out) else {
        out.note("reload: no release context");
        return;
    };
    match app.st().speleodb_reload(discard, Some(ctx)) {
        Ok(UpdateResult::FastForwarded { local_path }) => {
            out.note("update -> Ok(FastForwarded)");
            app.front().unsaved = false;
            out.claims.push(Claim::BaseIsTip {
                project_id: link.project_id,
            });
            load(app, Path::new(&local_path), true, false, out);
        }
        Ok(UpdateResult::UpToDate) => {
            out.note("update -> Ok(UpToDate)");
            out.claims.push(Claim::BaseIsTip {
                project_id: link.project_id,
            });
            link_status(app, out);
        }
        Ok(r) => out.note(format!("update -> Ok({r:?})")),
        Err(e) => out.refused("update", &e),
    }
}

/// Take remote (the divergence dialog, confirmed twice).
pub(super) fn take_remote(app: &App, out: &mut Outcome) {
    if app.open_link().is_none() {
        out.note("take remote: no linked project");
        return;
    }
    out.claims.push(Claim::DiscardedUnsaved);
    out.claims.push(Claim::DiscardedLocal);
    let project_id = app.open_link().map(|l| l.project_id).unwrap_or_default();
    match app.st().speleodb_take_remote() {
        Ok(path) => {
            let gate = app.st().edit_enabled();
            out.note(format!("take_remote -> Ok (edit gate {gate})"));
            if gate {
                out.claims
                    .push(Claim::EditableAfterTakeRemote { project_id });
            }
            app.front().unsaved = false;
            load(app, Path::new(&path), false, true, out);
        }
        Err(e) => out.refused("take_remote", &e),
    }
}

/// Discard changes and keep editing (`SpeleoUnwoundPushDialog`).
fn discard_keep(app: &App, out: &mut Outcome) {
    if app.open_link().is_none() {
        out.note("discard: no linked project");
        return;
    }
    out.claims.push(Claim::DiscardedUnsaved);
    out.claims.push(Claim::DiscardedLocal);
    match app.st().speleodb_discard_changes(true, true) {
        Ok(Some(r)) => {
            app.front().unsaved = false;
            started(app, Ok(r), "discard_changes", out);
            link_status(app, out);
        }
        Ok(None) => {
            app.front().unsaved = false;
            out.note("discard_changes -> Ok(None)");
            link_status(app, out);
        }
        Err(e) => out.refused("discard_changes", &e),
    }
}

/// Save copy: Save As outside the cache, unlink, open the copy.
fn save_copy(app: &App, out: &mut Outcome) {
    if app.open_link().is_none() {
        out.note("save copy: no linked project");
        return;
    }
    let n = {
        let mut front = app.front();
        front.copies += 1;
        front.copies
    };
    let _ = std::fs::create_dir_all(&app.copies_dir);
    let path = app.copies_dir.join(format!("copy-{n}.stalx"));
    let leaving = app.leaving(Exit::Unlink);
    if !save(app, Some(path.clone()), out) {
        return;
    }
    match app.st().speleodb_unlink() {
        Ok(()) => {
            out.note("unlink -> Ok");
            out.claims.push(Claim::LinkDropped);
            out.claims.extend(leaving);
        }
        Err(e) => out.refused("unlink", &e),
    }
    link_status(app, out);
    load(app, &path, false, true, out);
}

/// The publish parameters for this run's `n`th publish.
pub(super) fn publish_params(app: &App, n: usize) -> SpeleoPublishParams {
    SpeleoPublishParams {
        message: Some("Project Creation".to_owned()),
        name: format!("{}-pub{n}", app.tag),
        description: "fuzz publish".to_owned(),
        country: "US".to_owned(),
        visibility: Visibility::Private,
        latitude: None,
        longitude: None,
    }
}
