//! Offline tests of the oracle over synthetic snapshots: each invariant fires
//! on the shape it names and stays quiet when its excuse holds.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use super::app::{Claim, Exit, Outcome};
use super::data::Fp;
use super::model::{Acct, Act};
use super::oracle::{check, CommitFps, Emails, Ghost, StepCtx, Violation};
use super::proxy::ProxyEvent;
use super::snap::{AppView, FileSnap, Snap};
use super::world::{ServerView, Who};
use crate::models::SpeleoLink;
use crate::speleodb::Permission;

const P: &str = "0b5c2d1e-0000-4000-8000-123456789abc";
const OWNER: &str = "owner@example.invalid";
const COLLAB: &str = "collab@example.invalid";

#[derive(Default)]
struct Commits(HashMap<String, Fp>);

impl CommitFps for Commits {
    fn commit_fp(&mut self, _id: &str, sha: &str) -> Result<Fp, String> {
        self.0.get(sha).cloned().ok_or_else(|| "unknown".to_owned())
    }

    fn is_newer(&self, _id: &str, _sha: &str) -> bool {
        false
    }
}

fn fp(names: &[&str]) -> Fp {
    names.iter().map(|n| ((*n).to_owned(), 1)).collect()
}

fn view(holder: Option<&str>, commits: &[(&str, Option<&str>)]) -> ServerView {
    ServerView {
        exists: true,
        lock_holder: holder.map(str::to_owned),
        commits: commits.iter().map(|(c, _)| (*c).to_owned()).collect(),
        parents: commits
            .iter()
            .map(|(c, p)| {
                (
                    (*c).to_owned(),
                    p.map(|p| vec![p.to_owned()]).unwrap_or_default(),
                )
            })
            .collect(),
        tip: commits.last().map(|(c, _)| (*c).to_owned()),
        kind: "stalagmight".to_owned(),
        name: "p".to_owned(),
        perms: BTreeMap::from([(Who::Owner, Some(Permission::Admin))]),
    }
}

fn link(base: &str, dirty: bool) -> SpeleoLink {
    SpeleoLink {
        endpoint: "http://127.0.0.1:1/".to_owned(),
        project_id: P.to_owned(),
        project_name: "p".to_owned(),
        base_commit_sha: Some(base.to_owned()),
        has_unpushed_edits: dirty,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: None,
    }
}

fn snap(holder: Option<&str>, gate: bool, link: Option<SpeleoLink>, mem: Fp) -> Snap {
    let mut s = Snap {
        app: AppView {
            logged_in: true,
            email: Some(OWNER.to_owned()),
            gate,
            link,
            mem: Some(mem),
            ..AppView::default()
        },
        ..Snap::default()
    };
    s.server.insert(P.to_owned(), view(holder, &[("c1", None)]));
    s.server_tokens
        .insert(P.to_owned(), ["A".to_owned()].into_iter().collect());
    s
}

fn emails() -> Emails {
    Emails {
        owner: OWNER.to_owned(),
        collab: COLLAB.to_owned(),
        ro: None,
    }
}

fn run_check(
    ghost: &mut Ghost,
    act: &Act,
    out: &Outcome,
    events: &[ProxyEvent],
    before: &Snap,
    after: &Snap,
    commits: &mut Commits,
) -> Vec<Violation> {
    let emails = emails();
    let newer = BTreeSet::new();
    let allowed = Vec::new();
    let cx = StepCtx {
        act,
        out,
        events,
        before,
        after,
        emails: &emails,
        allowed: &allowed,
        cache_root: Path::new("/nonexistent-cache"),
        newer_shas: &newer,
    };
    check(ghost, &cx, commits)
}

fn ids(v: &[Violation]) -> Vec<&str> {
    v.iter().map(|x| x.id.as_str()).collect()
}

fn left(exit: Exit, dirty: bool) -> Outcome {
    Outcome {
        claims: vec![Claim::Left {
            project_id: P.to_owned(),
            exit,
            account: OWNER.to_owned(),
            dirty,
        }],
        ..Outcome::default()
    }
}

fn release_fault() -> ProxyEvent {
    ProxyEvent {
        method: "POST".to_owned(),
        path: format!("/api/v2/projects/{P}/release/"),
        upstream_status: None,
        fault: Some("Status(500)".to_owned()),
        held_ms: None,
        delivered: true,
    }
}

#[test]
fn a_close_that_leaves_the_lock_held_is_flagged_unless_its_release_was_faulted() {
    let before = snap(Some(OWNER), true, Some(link("c1", false)), fp(&["A"]));
    let after = snap(Some(OWNER), false, None, Fp::new());
    let out = left(Exit::Close, false);
    let mut commits = Commits::default();
    let v = run_check(
        &mut Ghost::default(),
        &Act::Close,
        &out,
        &[],
        &before,
        &after,
        &mut commits,
    );
    assert_eq!(ids(&v), ["LK-EXIT-close"]);
    let v = run_check(
        &mut Ghost::default(),
        &Act::Close,
        &out,
        &[release_fault()],
        &before,
        &after,
        &mut commits,
    );
    assert!(v.is_empty(), "{v:?}");
}

#[test]
fn a_lifecycle_release_over_saved_unuploaded_work_is_r1() {
    let before = snap(Some(OWNER), true, Some(link("c1", true)), fp(&["A"]));
    let mut after = snap(None, false, None, Fp::new());
    let cache = PathBuf::from("/nonexistent-cache")
        .join(P)
        .join("project.stalx");
    let saved = FileSnap {
        fp: Ok(fp(&["A", "E1"])),
        link: Ok(Some(link("c1", true))),
        hash: 1,
        integrity: Ok(()),
        residue: false,
        newer: false,
    };
    after.files.insert(cache.clone(), saved);
    let v = run_check(
        &mut Ghost::default(),
        &Act::Quit,
        &left(Exit::Quit, true),
        &[],
        &before,
        &after,
        &mut Commits::default(),
    );
    assert_eq!(ids(&v), ["R1-stranded-unpushed"]);
    // A dirty mark over content the server has (unsaved edits discarded) strands
    // nothing.
    let mut discarded = after.clone();
    if let Some(f) = discarded.files.get_mut(&cache) {
        f.fp = Ok(fp(&["A"]));
    }
    let v = run_check(
        &mut Ghost::default(),
        &Act::Close,
        &left(Exit::Close, true),
        &[],
        &before,
        &discarded,
        &mut Commits::default(),
    );
    assert!(v.iter().all(|x| x.id != "R1-stranded-unpushed"), "{v:?}");
    // Logout keeps such a lock on purpose: neither id fires.
    let held = snap(Some(OWNER), false, None, Fp::new());
    let v = run_check(
        &mut Ghost::default(),
        &Act::Logout,
        &left(Exit::Logout, true),
        &[],
        &before,
        &held,
        &mut Commits::default(),
    );
    assert!(
        v.iter()
            .all(|x| !x.id.starts_with("LK-EXIT") && !x.id.starts_with("R1")),
        "{v:?}"
    );
}

#[test]
fn an_open_gate_without_the_lock_is_flagged_only_once_verified() {
    let before = snap(None, false, Some(link("c1", false)), fp(&["A"]));
    let after = snap(Some(COLLAB), true, Some(link("c1", false)), fp(&["A"]));
    let out = Outcome::default();
    let mut commits = Commits(HashMap::from([("c1".to_owned(), fp(&["A"]))]));
    let v = run_check(
        &mut Ghost::default(),
        &Act::StartEditing,
        &out,
        &[],
        &before,
        &after,
        &mut commits,
    );
    assert_eq!(ids(&v), ["LK-OPEN"]);
    let open_before = snap(Some(OWNER), true, Some(link("c1", false)), fp(&["A"]));
    let mut ghost = Ghost::default();
    ghost.unverified.insert(P.to_owned());
    let v = run_check(
        &mut ghost,
        &Act::Edit,
        &out,
        &[],
        &open_before,
        &after,
        &mut commits,
    );
    assert!(v.is_empty(), "the transient case is excused: {v:?}");
    let v = run_check(
        &mut Ghost::default(),
        &Act::Edit,
        &out,
        &[],
        &open_before,
        &after,
        &mut commits,
    );
    assert_eq!(ids(&v), ["LK-GATE"]);
}

#[test]
fn freeing_another_accounts_lock_in_an_app_step_is_flagged() {
    let before = snap(Some(COLLAB), true, Some(link("c1", false)), fp(&["A"]));
    let after = snap(None, false, None, Fp::new());
    let v = run_check(
        &mut Ghost::default(),
        &Act::Close,
        &Outcome::default(),
        &[],
        &before,
        &after,
        &mut Commits::default(),
    );
    assert_eq!(ids(&v), ["P2-exit-frees-foreign-lock"]);
    // The second client releasing it is not the app's doing.
    let v = run_check(
        &mut Ghost::default(),
        &Act::ActorRelease {
            who: Acct::Collab,
            p: 0,
        },
        &Outcome::default(),
        &[],
        &before,
        &after,
        &mut Commits::default(),
    );
    assert!(v.is_empty(), "{v:?}");
}

#[test]
fn an_edit_that_vanishes_without_a_word_is_a_loss() {
    let before = snap(Some(OWNER), true, Some(link("c1", true)), fp(&["A", "E1"]));
    let after = snap(Some(OWNER), true, Some(link("c1", false)), fp(&["A"]));
    let mut ghost = Ghost::default();
    ghost.pending.insert("E1".to_owned());
    let mut commits = Commits(HashMap::from([("c1".to_owned(), fp(&["A"]))]));
    let v = run_check(
        &mut ghost,
        &Act::Refresh,
        &Outcome::default(),
        &[],
        &before,
        &after,
        &mut commits,
    );
    assert_eq!(ids(&v), ["DB-LOSS"]);
    let mut ghost = Ghost::default();
    ghost.pending.insert("E1".to_owned());
    let disclosed = Outcome {
        claims: vec![Claim::DiscardedUnsaved],
        ..Outcome::default()
    };
    let v = run_check(
        &mut ghost,
        &Act::Reload,
        &disclosed,
        &[],
        &before,
        &after,
        &mut commits,
    );
    assert!(v.iter().all(|x| x.id != "DB-LOSS"), "{v:?}");
}

#[test]
fn an_upload_that_drops_its_parents_sections_is_an_overwrite() {
    let before = snap(Some(OWNER), true, Some(link("c1", true)), fp(&["A", "E1"]));
    let mut after = before.clone();
    after.server.insert(
        P.to_owned(),
        view(
            Some(OWNER),
            &[("c1", None), ("c2", Some("c1")), ("c3", Some("c2"))],
        ),
    );
    let mut before = before;
    before.server.insert(
        P.to_owned(),
        view(Some(OWNER), &[("c1", None), ("c2", Some("c1"))]),
    );
    let mut commits = Commits(HashMap::from([
        ("c1".to_owned(), fp(&["A"])),
        ("c2".to_owned(), fp(&["A", "X5"])),
        ("c3".to_owned(), fp(&["A", "E1"])),
    ]));
    let act = Act::SaveChanges {
        long_message: false,
    };
    let v = run_check(
        &mut Ghost::default(),
        &act,
        &Outcome::default(),
        &[],
        &before,
        &after,
        &mut commits,
    );
    assert!(ids(&v).contains(&"R3-same-account-overwrite"), "{v:?}");
    let reverted = Outcome {
        claims: vec![Claim::Reverted {
            project_id: P.to_owned(),
        }],
        ..Outcome::default()
    };
    let v = run_check(
        &mut Ghost::default(),
        &act,
        &reverted,
        &[],
        &before,
        &after,
        &mut commits,
    );
    assert!(!ids(&v).contains(&"R3-same-account-overwrite"), "{v:?}");
}

#[test]
fn an_offline_open_after_an_http_status_is_flagged() {
    let before = snap(None, false, None, Fp::new());
    let after = before.clone();
    let out = Outcome {
        claims: vec![Claim::OfflineOpen {
            project_id: P.to_owned(),
        }],
        ..Outcome::default()
    };
    let download = |fault: &str| ProxyEvent {
        method: "GET".to_owned(),
        path: format!("/api/v2/projects/{P}/download/stalagmight/c1/"),
        upstream_status: None,
        fault: Some(fault.to_owned()),
        held_ms: None,
        delivered: false,
    };
    let act = Act::Open {
        p: 0,
        take_server: false,
    };
    let mut commits = Commits::default();
    let v = run_check(
        &mut Ghost::default(),
        &act,
        &out,
        &[download("Status(404)")],
        &before,
        &after,
        &mut commits,
    );
    assert_eq!(ids(&v), ["OFF-1"]);
    let v = run_check(
        &mut Ghost::default(),
        &act,
        &out,
        &[download("Refuse")],
        &before,
        &after,
        &mut commits,
    );
    assert!(v.is_empty(), "{v:?}");
}

#[test]
fn a_clean_copy_must_hold_its_base() {
    let path = PathBuf::from("/nonexistent-cache")
        .join(P)
        .join("project.stalx");
    let mut after = snap(Some(OWNER), true, Some(link("c1", false)), fp(&["A"]));
    after.files.insert(
        path,
        FileSnap {
            fp: Ok(fp(&["A", "Z"])),
            link: Ok(Some(link("c1", false))),
            hash: 1,
            integrity: Ok(()),
            residue: false,
            newer: false,
        },
    );
    let before = after.clone();
    let mut commits = Commits(HashMap::from([("c1".to_owned(), fp(&["A"]))]));
    let v = run_check(
        &mut Ghost::default(),
        &Act::Refresh,
        &Outcome::default(),
        &[],
        &before,
        &after,
        &mut commits,
    );
    assert_eq!(ids(&v), ["DB-CLEAN"]);
}
