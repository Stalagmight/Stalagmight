//! Lock invariants.
//!
//! - **LK-CLAIM** the app said it released P's lock (Done -> Released, Switch to
//!   Read Only -> Ok), yet the server shows the app's account holding it.
//! - **LK-EXIT-close / -quit / -switch / -logout / -unlink** an exit path left P,
//!   which the app was editing, and the server still shows the app's account
//!   holding P's lock. Close, Quit, a switch to another project and Unlink
//!   always release (docs/SpeleoDB.md "Save, switching and exit"); logout keeps
//!   the lock only over un-pushed edits. Excused when a fault fired on a request
//!   for P (or made the server look unreachable) during the step, or the second
//!   client changed the app account's access to P. The review-named variants:
//!   **P2-login-switch-strands-lock** (a sign-in as another account or deployment
//!   over an open, editing project), **R5-restart-update-leak** (restart-to-update
//!   skips the exit hook), **P2-quit-mid-op** (a quit while an upload or acquire
//!   of the app was in flight), **P2-ambiguous-acquire-leak** (the leaked lock was
//!   taken by an acquire whose answer was lost, so the app never recorded it).
//! - **R1-stranded-unpushed** a close, quit or switch released P's lock while
//!   P's copy held saved, un-uploaded edits: online, the next open refuses over
//!   them and offers only Cancel or Take the server copy, while another editor
//!   can lock and push.
//! - **LK-GATE** the edit gate is open on P while the server does not show the
//!   signed-in account holding P with write permission. Excused while P is
//!   unverified (a fault or the second client touched it since the app last
//!   confirmed P without a fault: the documented transient-offline case).
//! - **P2-take-remote-editable** Take remote (which releases the lock) returned
//!   with the edit gate open; only the status refresh that follows it in the
//!   frontend shuts the gate again.
//! - **LK-OPEN** the gate went from shut to open in this step without the server
//!   showing the signed-in account holding P with write permission (no excuse:
//!   only a verified acquisition may open it).
//! - **P2-exit-frees-foreign-lock** another account held P's lock before an app
//!   step and nobody holds it after: the app released a lock that was not its own.
//! - **LK-OWN** Start Editing reported a lock held by the signed-in account as
//!   another editor's (a lock the account holds is the user's own).

use super::app::{Claim, Exit};
use super::model::Act;
use super::oracle::{violation, Ghost, StepCtx, Violation};
use crate::speleodb::Permission;

fn holder_is(cx: &StepCtx<'_>, project_id: &str, email: &str) -> bool {
    cx.after.server.get(project_id).is_some_and(|s| {
        s.exists
            && s.lock_holder
                .as_deref()
                .is_some_and(|h| h.eq_ignore_ascii_case(email))
    })
}

/// The run account whose email is `email`.
fn who_of(cx: &StepCtx<'_>, email: &str) -> Option<super::world::Who> {
    use super::world::Who;
    let e = email.to_ascii_lowercase();
    if e == cx.emails.owner {
        Some(Who::Owner)
    } else if e == cx.emails.collab {
        Some(Who::Collab)
    } else if cx.emails.ro.as_deref() == Some(e.as_str()) {
        Some(Who::Ro)
    } else {
        None
    }
}

/// The signed-in account may write P and holds its lock, as the server says.
fn verified_editor(cx: &StepCtx<'_>, project_id: &str) -> Result<(), String> {
    let Some(email) = cx.after.app.email.as_deref() else {
        return Err("no account is signed in".to_owned());
    };
    let Some(view) = cx.after.server.get(project_id) else {
        return Ok(());
    };
    if !view.exists {
        return Err("the project no longer exists".to_owned());
    }
    let holds = view
        .lock_holder
        .as_deref()
        .is_some_and(|h| h.eq_ignore_ascii_case(email));
    let perm = who_of(cx, email).and_then(|w| view.perms.get(&w).copied().flatten());
    if !holds {
        return Err(format!("the server lock holder is {:?}", view.lock_holder));
    }
    if perm.is_none_or(|p| p < Permission::ReadAndWrite) {
        return Err(format!("the account's permission is {perm:?}"));
    }
    Ok(())
}

/// The sections of P's cache copy that no server commit of P holds: saved work
/// that exists only on this machine. (A dirty mark alone is not that: discarding
/// unsaved edits leaves the mark over content the server has.)
fn saved_unuploaded(cx: &StepCtx<'_>, project_id: &str) -> Vec<String> {
    let path = cx.cache_root.join(project_id).join("project.stalx");
    let Some(Ok(fp)) = cx.after.files.get(&path).map(|f| f.fp.as_ref()) else {
        return Vec::new();
    };
    let server = cx.after.server_tokens.get(project_id);
    fp.keys()
        .filter(|k| server.is_some_and(|s| !s.contains(*k)))
        .cloned()
        .collect()
}

fn exit_id(exit: Exit, cx: &StepCtx<'_>, ghost: &Ghost, project_id: &str) -> &'static str {
    if ghost.lost_acquire.contains(project_id) {
        return "P2-ambiguous-acquire-leak";
    }
    if matches!(cx.act, Act::Interleave { .. }) && exit == Exit::Quit {
        return "P2-quit-mid-op";
    }
    match exit {
        Exit::Close => "LK-EXIT-close",
        Exit::Quit => "LK-EXIT-quit",
        Exit::Switch => "LK-EXIT-switch",
        Exit::Logout => "LK-EXIT-logout",
        Exit::Unlink => "LK-EXIT-unlink",
        Exit::LoginSwitch => "P2-login-switch-strands-lock",
        Exit::RestartUpdate => "R5-restart-update-leak",
    }
}

pub(super) fn check(ghost: &Ghost, cx: &StepCtx<'_>, v: &mut Vec<Violation>) {
    if cx.act.is_actor() {
        // The second client moves locks on purpose; its effect on the app is
        // judged at the app's next step.
        return;
    }
    for claim in cx.claims() {
        match claim {
            Claim::Released { project_id } => {
                if let Some(email) = cx.before.app.email.as_deref() {
                    if holder_is(cx, project_id, email) {
                        v.push(violation(
                            "LK-CLAIM",
                            format!("{project_id}: the app said it released the lock; the server still shows {email}"),
                        ));
                    }
                }
            }
            Claim::Left {
                project_id,
                exit,
                account,
                dirty,
            } => {
                let held = holder_is(cx, project_id, account);
                let excused =
                    cx.fault_touched(project_id) || ghost.access_changed.contains(project_id);
                let keeps = *exit == Exit::Logout && *dirty;
                if held && !excused && !keeps {
                    v.push(violation(
                        exit_id(*exit, cx, ghost, project_id),
                        format!("{project_id}: {exit:?} left the project with {account}'s lock still held"),
                    ));
                }
                let lifecycle = matches!(exit, Exit::Close | Exit::Quit | Exit::Switch);
                let exists = cx.after.server.get(project_id).is_some_and(|s| s.exists);
                let stranded = saved_unuploaded(cx, project_id);
                if lifecycle && *dirty && !held && exists && !stranded.is_empty() {
                    v.push(violation(
                        "R1-stranded-unpushed",
                        format!("{project_id}: {exit:?} released the lock over saved, un-uploaded edits {stranded:?}; online the next open offers only Cancel or discard"),
                    ));
                }
            }
            Claim::EditableAfterTakeRemote { project_id } => {
                let email = cx.before.app.email.as_deref().unwrap_or_default();
                if !holder_is(cx, project_id, email) {
                    v.push(violation(
                        "P2-take-remote-editable",
                        format!("{project_id}: Take remote released the lock and returned with the edit gate open"),
                    ));
                }
            }
            _ => {}
        }
    }

    // LK-OPEN / LK-GATE.
    if let (true, Some(link)) = (cx.after.app.gate, cx.after.app.link.as_ref()) {
        let pid = &link.project_id;
        let was_open = cx.before.app.gate
            && cx
                .before
                .app
                .link
                .as_ref()
                .is_some_and(|l| &l.project_id == pid);
        let interleaved = matches!(cx.act, Act::Interleave { .. });
        if let Err(why) = verified_editor(cx, pid) {
            if !was_open && !interleaved {
                v.push(violation(
                    "LK-OPEN",
                    format!("{pid}: the edit gate opened in this step but {why}"),
                ));
            } else if !ghost.unverified.contains(pid) && !cx.fault_touched(pid) {
                let id = "LK-GATE";
                v.push(violation(
                    id,
                    format!("{pid}: the app is editing but {why}"),
                ));
            }
        }
    }

    // P2-exit-frees-foreign-lock: only the app acted in this step.
    let mine = cx.app_accounts();
    for (pid, was) in &cx.before.server {
        let Some(holder) = was.lock_holder.as_deref() else {
            continue;
        };
        if mine.iter().any(|m| m.eq_ignore_ascii_case(holder)) || !was.exists {
            continue;
        }
        let now = cx.after.server.get(pid);
        if now.is_some_and(|s| s.exists && s.lock_holder.is_none()) {
            v.push(violation(
                "P2-exit-frees-foreign-lock",
                format!(
                    "{pid}: {holder} held the lock before this app step and nobody holds it after"
                ),
            ));
        }
    }

    // LK-OWN: parsed from the Start Editing answers the flows logged.
    if let Some(me) = cx
        .before
        .app
        .email
        .as_deref()
        .or(cx.after.app.email.as_deref())
    {
        for line in &cx.out.log {
            if line.contains("LockedBy") && line.to_ascii_lowercase().contains(&format!("\"{me}\""))
            {
                v.push(violation(
                    "LK-OWN",
                    format!("Start Editing reported the signed-in account's own lock as another editor's: {line}"),
                ));
            }
        }
    }
}
