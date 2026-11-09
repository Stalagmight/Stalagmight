//! What the request log and the step's answers must show, and the ghost's
//! bookkeeping of what the app can have verified.
//!
//! - **ENDPOINT** the app session holds an endpoint other than the fault proxy or
//!   the other-deployment stub (the run is aborted).
//! - **ERR-PANIC** a verb panicked. **ERR-EMPTY** a refusal carries no message.
//! - **RL-NOLOCK** Reload sent an acquire (Reload never takes a lock).
//! - **RL-NODL** Reload of a clean copy whose BASE is the tip downloaded anything.
//! - **RO-NOUP** Switch to Read Only uploaded or downloaded.
//! - **EX-NOUP** Close or Quit uploaded (exits release without uploading).
//! - **SE-PIN** Start Editing downloaded an unpinned HEAD (only verified
//!   descendants, by exact revision).
//! - **ST-REFRESH-OPEN** a status refresh opened a shut edit gate.
//! - **ST-CRED** after a sign-out the app is still signed in, or a token is still
//!   saved.
//! - **OFF-1** an open answered with the offline cache although the download's
//!   last failure was an HTTP status or a delivered-but-invalid body (only
//!   connection-level failures may fall back).
//! - **P2-publish-duplicate** a publish left more than one server project under
//!   its name.
//! - **DB-NEWER-TYPED** a step downloaded a newer-schema revision and was refused
//!   without the typed newer-schema error.

use super::app::Claim;
use super::model::Act;
use super::oracle::{is_unreachable, project_of, violation, Ghost, StepCtx, Violation};

/// Keep the ghost's verification bookkeeping current (before the checks read it).
fn update_ghost(ghost: &mut Ghost, cx: &StepCtx<'_>) {
    let open = cx.after.app.link.as_ref().map(|l| l.project_id.clone());
    let mut any_fault = false;
    for (e, label) in cx.faulted() {
        any_fault = true;
        if let Some(pid) = project_of(&e.path) {
            ghost.unverified.insert(pid.to_owned());
            let lost = [
                "StatusAfter",
                "DropAfterForward",
                "HangThenForward",
                "TruncateBody",
            ]
            .iter()
            .any(|k| label.starts_with(k));
            if lost && e.method == "POST" && e.path.contains("/acquire/") {
                ghost.lost_acquire.insert(pid.to_owned());
            }
        }
        if is_unreachable(label) {
            ghost.unverified.extend(open.clone());
        }
    }
    let verifying = matches!(
        cx.act,
        Act::Refresh | Act::StartEditing | Act::Open { .. } | Act::OpenFile { .. }
    );
    if verifying && !any_fault {
        if let Some(pid) = &open {
            ghost.unverified.remove(pid);
            if cx.after.app.gate {
                ghost.access_changed.remove(pid);
                ghost.lost_acquire.remove(pid);
            }
        }
    }
    for c in cx.claims() {
        match c {
            Claim::Residue { project_id } => {
                ghost.residue.insert(project_id.clone());
            }
            Claim::LongMessage { project_id } => {
                ghost.long_message.insert(project_id.clone());
            }
            _ => {}
        }
    }
    ghost.residue.retain(|pid| {
        cx.after
            .files
            .get(&cx.cache_root.join(pid).join("project.stalx"))
            .is_some_and(|f| f.residue)
    });
}

fn requests<'a>(
    cx: &'a StepCtx<'_>,
    method: &'a str,
    path: &'a str,
) -> impl Iterator<Item = &'a super::proxy::ProxyEvent> {
    cx.events
        .iter()
        .filter(move |e| e.method == method && e.path.contains(path))
}

pub(super) fn check(ghost: &mut Ghost, cx: &StepCtx<'_>, v: &mut Vec<Violation>) {
    update_ghost(ghost, cx);
    if let Some(endpoint) = &cx.after.app.endpoint {
        if !super::target::endpoint_allowed(
            endpoint,
            &cx.allowed.iter().map(String::as_str).collect::<Vec<_>>(),
        ) {
            v.push(violation(
                "ENDPOINT",
                format!("the app session holds {endpoint}"),
            ));
        }
    }
    if let Some(p) = &cx.out.panicked {
        v.push(violation("ERR-PANIC", format!("a verb panicked: {p}")));
    }
    for r in &cx.out.refusals {
        if r.message.trim().is_empty() {
            v.push(violation(
                "ERR-EMPTY",
                format!("{} refused with {} and no message", r.verb, r.action),
            ));
        }
    }
    let downloads = || requests(cx, "GET", "/download/stalagmight/");
    match cx.act {
        Act::Reload => {
            if requests(cx, "POST", "/acquire/").next().is_some() {
                v.push(violation("RL-NOLOCK", "Reload sent an acquire".to_owned()));
            }
            let clean_at_tip = cx.before.app.link.as_ref().is_some_and(|l| {
                !l.has_unpushed_edits
                    && !cx.before.app.unsaved
                    && !cx.before.app.unwound
                    && cx
                        .before
                        .server
                        .get(&l.project_id)
                        .is_some_and(|s| s.tip.is_some() && s.tip == l.base_commit_sha)
            });
            let unchanged = cx.before.app.link.as_ref().is_some_and(|l| {
                cx.after.server.get(&l.project_id).map(|s| &s.tip)
                    == cx.before.server.get(&l.project_id).map(|s| &s.tip)
            });
            if clean_at_tip && unchanged && downloads().next().is_some() {
                v.push(violation(
                    "RL-NODL",
                    "Reload of a clean copy at the tip downloaded the project".to_owned(),
                ));
            }
        }
        Act::ReadOnly => {
            if requests(cx, "PUT", "/upload/").next().is_some() || downloads().next().is_some() {
                v.push(violation(
                    "RO-NOUP",
                    "Switch to Read Only transferred the project".to_owned(),
                ));
            }
        }
        Act::Close | Act::Quit => {
            if requests(cx, "PUT", "/upload/").next().is_some() {
                v.push(violation("EX-NOUP", format!("{} uploaded", cx.act.name())));
            }
        }
        Act::StartEditing => {
            for e in downloads() {
                let tail = e.path.split("/download/stalagmight/").nth(1).unwrap_or("");
                if tail.trim_matches('/').is_empty() {
                    v.push(violation(
                        "SE-PIN",
                        format!("Start Editing downloaded HEAD: {}", e.path),
                    ));
                }
            }
        }
        Act::Refresh if !cx.before.app.gate && cx.after.app.gate => {
            v.push(violation(
                "ST-REFRESH-OPEN",
                "a status refresh opened the edit gate".to_owned(),
            ));
        }
        Act::Logout if cx.after.app.logged_in || cx.after.app.saved_token => {
            v.push(violation(
                "ST-CRED",
                format!(
                    "after sign-out: signed in {}, token saved {}",
                    cx.after.app.logged_in, cx.after.app.saved_token
                ),
            ));
        }
        _ => {}
    }
    for c in cx.claims() {
        match c {
            Claim::OfflineOpen { project_id } => {
                let last = downloads()
                    .filter(|e| e.path.contains(project_id.as_str()))
                    .last();
                let connection_level = last
                    .and_then(|e| e.fault.as_deref())
                    .is_some_and(is_unreachable);
                if !connection_level {
                    v.push(violation(
                        "OFF-1",
                        format!(
                            "{project_id}: the open fell back to the cache after {:?}",
                            last.map(|e| (&e.upstream_status, &e.fault))
                        ),
                    ));
                }
            }
            Claim::Published(p) => {
                let n = cx
                    .after
                    .owner_list
                    .iter()
                    .filter(|(name, _)| name == &p.name)
                    .count();
                if n > 1 {
                    v.push(violation(
                        "P2-publish-duplicate",
                        format!("{n} server projects are named {}", p.name),
                    ));
                }
            }
            _ => {}
        }
    }
    // DB-NEWER-TYPED: a clean download of a newer revision must refuse typed.
    let newer_download = cx.before.server.iter().any(|(pid, view)| {
        view.tip.as_ref().is_some_and(|tip| {
            cx.newer(pid, tip)
                && downloads()
                    .any(|e| e.path.contains(pid.as_str()) && e.fault.is_none() && e.delivered)
        })
    });
    let refused = cx.out.refusals.iter().any(|r| {
        !matches!(
            r.verb.as_str(),
            "link_status" | "project_access_at" | "list_projects"
        )
    });
    if newer_download && refused && !cx.out.refusals.iter().any(|r| r.newer_schema) {
        v.push(violation(
            "DB-NEWER-TYPED",
            format!(
                "a newer-schema revision was downloaded and refused without the typed error: {:?}",
                cx.out
                    .refusals
                    .iter()
                    .map(|r| (&r.verb, &r.action))
                    .collect::<Vec<_>>()
            ),
        ));
    }
}
