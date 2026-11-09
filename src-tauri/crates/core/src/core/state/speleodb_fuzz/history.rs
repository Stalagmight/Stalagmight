//! The History flows (View, Revert, Unwind / Rewind) and Publish, in the
//! frontend's order (`useRestoreRevision`, `useRevertRevision`,
//! `SpeleoUnwindDialog`, `publishProjectAndReopen`).

use std::path::Path;

use super::app::{App, Claim, Outcome, Proj};
use super::flows::{link_status, load, publish_params, started};
use crate::speleodb::DoneResult;

/// The commit `back` steps behind the newest one in the history list.
fn pick_revision(app: &App, back: u8, out: &mut Outcome) -> Option<(String, String, String)> {
    let link = app.open_link()?;
    match app.st().speleodb_get_revisions(link.project_id.clone()) {
        Ok(revisions) => {
            let rev = revisions.get(usize::from(back) % revisions.len().max(1))?;
            Some((link.project_id, link.project_name, rev.id.clone()))
        }
        Err(e) => {
            out.refused("get_revisions", &e);
            None
        }
    }
}

/// History View: a read-only checkout of a revision, then the tree swap.
pub(super) fn view(app: &App, back: u8, out: &mut Outcome) {
    if app.front().unsaved {
        out.note("view: skipped over unsaved work");
        return;
    }
    let Some((id, name, sha)) = pick_revision(app, back, out) else {
        out.note("view: no revision");
        return;
    };
    match app
        .st()
        .speleodb_restore_revision(id, name, sha, false, false, false, false, false)
    {
        Ok(res) => {
            out.note("restore_revision(view) -> Ok");
            load(app, Path::new(&res.local_path), true, false, out);
        }
        Err(e) => out.refused("restore_revision(view)", &e),
    }
}

/// History Revert: Start Editing, restore the revision as an edit, upload it.
pub(super) fn revert(app: &App, back: u8, out: &mut Outcome) {
    let Some(link) = app.open_link() else {
        out.note("revert: no linked project");
        return;
    };
    if app.front().unsaved || link.has_unpushed_edits {
        out.note("revert: stopped by the frontend (pending work)");
        return;
    }
    let Some((id, name, sha)) = pick_revision(app, back, out) else {
        out.note("revert: no revision");
        return;
    };
    let answer = app.st().speleodb_start_editing();
    let ok = answer.is_ok();
    started(app, answer, "start_editing(revert)", out);
    if !ok || !app.st().edit_enabled() {
        return;
    }
    match app.st().speleodb_restore_revision(
        id.clone(),
        name,
        sha,
        true,
        false,
        false,
        false,
        false,
    ) {
        Ok(res) => {
            out.note("restore_revision(revert) -> Ok");
            load(app, Path::new(&res.local_path), true, false, out);
        }
        Err(e) => {
            out.refused("restore_revision(revert)", &e);
            link_status(app, out);
            if !app.open_link().is_some_and(|l| l.has_unpushed_edits) {
                let _ = app.st().speleodb_done_acknowledged(None, false);
            }
            return;
        }
    }
    out.claims.push(Claim::Reverted {
        project_id: id.clone(),
    });
    match app
        .st()
        .speleodb_done_acknowledged(Some("fuzz revert".to_owned()), false)
    {
        Ok(DoneResult::Released) => {
            out.note("done(revert) -> Ok(Released)");
            out.claims.push(Claim::Released { project_id: id });
        }
        Ok(r) => out.note(format!("done(revert) -> Ok({r:?})")),
        Err(e) => out.refused("done(revert)", &e),
    }
}

/// Unwind (or, `forward`, Rewind): enabled only while editing with an empty
/// local undo (redo) stack.
pub(super) fn unwind(app: &App, forward: bool, out: &mut Outcome) {
    let st = app.st();
    if !app.editing() || app.front().unsaved {
        out.note("unwind: not available");
        return;
    }
    if (!forward && st.can_undo()) || (forward && st.can_redo()) {
        out.note("unwind: the local stack is not empty");
        return;
    }
    let target = if forward {
        st.speleodb_rewind_target()
            .map(|t| t.map(|t| (t.hexsha, false)))
    } else {
        st.speleodb_unwind_target()
            .map(|t| t.map(|t| (t.hexsha, t.discards_unpushed)))
    };
    let (sha, discards) = match target {
        Ok(Some(t)) => t,
        Ok(None) => {
            out.note("unwind target -> None");
            return;
        }
        Err(e) => {
            out.refused("unwind_target", &e);
            return;
        }
    };
    let Some(link) = app.open_link() else {
        return;
    };
    if discards {
        out.claims.push(Claim::DiscardedLocal);
    }
    match st.speleodb_restore_revision(
        link.project_id,
        link.project_name,
        sha,
        true,
        true,
        true,
        discards,
        forward,
    ) {
        Ok(res) => {
            out.note("restore_revision(unwind) -> Ok");
            load(app, Path::new(&res.local_path), true, false, out);
        }
        Err(e) => out.refused("restore_revision(unwind)", &e),
    }
}

/// Publish the open local project and reopen it managed. A failure leaves the
/// form up, and the user submits it once more with the same name.
pub(super) fn publish(app: &App, out: &mut Outcome) {
    if app.open_path().is_none() || app.open_link().is_some() || !app.session().logged_in {
        out.note("publish: needs an open local project and a session");
        return;
    }
    let n = app.front().publishes + 1;
    let params = publish_params(app, n);
    let mut answer = app.st().speleodb_publish(params.clone());
    if let Err(e) = &answer {
        out.refused("publish", e);
        if app.open_link().is_some() {
            return;
        }
        out.note("publish: the user submits the form again");
        answer = app.st().speleodb_publish(params.clone());
    }
    let result = match answer {
        Ok(r) => r,
        Err(e) => {
            out.refused("publish(retry)", &e);
            return;
        }
    };
    app.front().publishes = n;
    out.note("publish -> Ok");
    out.claims.push(Claim::Published(Proj {
        id: result.project_id.clone(),
        name: params.name.clone(),
    }));
    link_status(app, out);
    let pending = app.open_link().is_some_and(|l| l.has_unpushed_edits);
    if pending || app.front().unsaved {
        out.note("publish: upload pending (Save Changes recovers it)");
        return;
    }
    match app
        .st()
        .speleodb_open_project(result.project_id, params.name, None)
    {
        Ok(res) if res.offline == Some(true) => out.note("publish: reopen offline (reload failed)"),
        Ok(res) => {
            load(app, Path::new(&res.local_path), true, false, out);
            super::flows::auto_acquire(app, out);
        }
        Err(e) => out.refused("open_project(publish)", &e),
    }
}
