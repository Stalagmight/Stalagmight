//! Offline tests of the oracle: each invariant fires on a synthetic snapshot
//! that breaks it, and stays quiet on its excused twin. No server: commits are
//! a map, files and the app are plain values.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::app::{Claim, Outcome, Refusal};
use super::oracle::{check, CommitFps, FileSnap, Ghost, Snap};
use super::world::{Fp, ServerView};
use crate::models::SpeleoLink;

/// Server commits as a map: `(project, sha) -> content`.
struct Commits(HashMap<(String, String), Fp>);

impl CommitFps for Commits {
    fn commit_fp(&mut self, id: &str, sha: &str) -> Result<Fp, String> {
        self.0
            .get(&(id.to_owned(), sha.to_owned()))
            .cloned()
            .ok_or_else(|| "unknown commit".to_owned())
    }
}

const OWNER: &str = "owner@fuzz.invalid";
const PROXY: &str = "http://127.0.0.1:9/";

fn fp(names: &[&str]) -> Fp {
    names.iter().map(|n| ((*n).to_owned(), 2)).collect()
}

fn link(base: &str, dirty: bool) -> SpeleoLink {
    SpeleoLink {
        endpoint: PROXY.to_owned(),
        project_id: "p1".to_owned(),
        project_name: "P".to_owned(),
        base_commit_sha: Some(base.to_owned()),
        has_unpushed_edits: dirty,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: None,
    }
}

fn root() -> PathBuf {
    PathBuf::from("/cache/speleodb")
}

fn cache_file() -> PathBuf {
    root().join("p1").join("project.stalx")
}

/// One linked cache copy of `p1` holding `content`, open in memory as `mem`.
fn snap(content: &[&str], mem: &[&str], l: SpeleoLink, holder: Option<&str>) -> Snap {
    let mut s = Snap {
        endpoint: Some(PROXY.to_owned()),
        mem: Some((Some(cache_file()), fp(mem))),
        cache_files: vec![cache_file()],
        ..Snap::default()
    };
    s.files.insert(
        cache_file(),
        FileSnap {
            fp: Ok(fp(content)),
            link: Ok(Some(l)),
        },
    );
    s.server.insert(
        "p1".to_owned(),
        ServerView {
            exists: true,
            lock_holder: holder.map(str::to_owned),
            commits: vec!["c1".to_owned(), "c2".to_owned()],
            tip: Some("c2".to_owned()),
            kind: "other".to_owned(),
            name: "P".to_owned(),
        },
    );
    s
}

fn commits() -> Commits {
    Commits(
        [
            (("p1".to_owned(), "c1".to_owned()), fp(&["A"])),
            (("p1".to_owned(), "c2".to_owned()), fp(&["A", "B"])),
        ]
        .into_iter()
        .collect(),
    )
}

fn ids(out: &Outcome, before: &Snap, after: &Snap, ghost: &mut Ghost) -> Vec<String> {
    check(
        ghost,
        out,
        false,
        before,
        after,
        &mut commits(),
        OWNER,
        &[PROXY],
        Path::new("/cache/speleodb"),
    )
    .into_iter()
    .map(|v| v.id)
    .collect()
}

fn claims(claims: Vec<Claim>) -> Outcome {
    Outcome {
        claims,
        ..Outcome::default()
    }
}

#[test]
fn a_silently_vanished_edit_is_i1_and_a_disclosed_one_is_not() {
    let before = snap(
        &["A", "B", "E1"],
        &["A", "B", "E1"],
        link("c2", true),
        Some(OWNER),
    );
    let after = snap(&["A", "B"], &["A", "B"], link("c2", true), None);
    let mut ghost = Ghost::default();
    ghost.pending.insert("E1".to_owned());
    let silent = ids(&Outcome::default(), &before, &after, &mut ghost);
    assert!(silent.contains(&"I1".to_owned()), "{silent:?}");

    let mut ghost = Ghost::default();
    ghost.pending.insert("E1".to_owned());
    let told = ids(
        &claims(vec![Claim::DisclosedLoss]),
        &before,
        &after,
        &mut ghost,
    );
    assert!(!told.contains(&"I1".to_owned()), "{told:?}");
    assert!(ghost.pending.is_empty(), "a disclosed loss is forgotten");
}

#[test]
fn a_crash_excuses_only_edits_that_never_reached_the_disk() {
    let before = snap(
        &["A", "B"],
        &["A", "B", "E1"],
        link("c2", true),
        Some(OWNER),
    );
    let after = snap(&["A", "B"], &["A", "B"], link("c2", true), Some(OWNER));
    let mut ghost = Ghost::default();
    ghost.pending.insert("E1".to_owned());
    let v = ids(&claims(vec![Claim::Crashed]), &before, &after, &mut ghost);
    assert!(
        !v.contains(&"I1".to_owned()),
        "memory-only edits die with the process: {v:?}"
    );

    let saved = snap(
        &["A", "B", "E1"],
        &["A", "B", "E1"],
        link("c2", true),
        Some(OWNER),
    );
    let mut ghost = Ghost::default();
    ghost.pending.insert("E1".to_owned());
    let v = ids(&claims(vec![Claim::Crashed]), &saved, &after, &mut ghost);
    assert!(
        v.contains(&"I1".to_owned()),
        "a crash never takes a saved edit: {v:?}"
    );
}

#[test]
fn an_edit_on_the_server_is_safe_wherever_the_copy_goes() {
    let before = snap(
        &["A", "B", "E1"],
        &["A", "B", "E1"],
        link("c2", true),
        Some(OWNER),
    );
    let mut after = snap(&["A"], &["A"], link("c1", false), None);
    after.server_tokens.insert("E1".to_owned());
    let mut ghost = Ghost::default();
    ghost.pending.insert("E1".to_owned());
    let v = ids(&Outcome::default(), &before, &after, &mut ghost);
    assert!(!v.contains(&"I1".to_owned()), "{v:?}");
    assert!(ghost.pending.is_empty());
}

#[test]
fn a_clean_mark_over_content_the_base_lacks_is_i4() {
    let before = snap(&["A", "B"], &["A", "B"], link("c2", false), Some(OWNER));
    let after = snap(
        &["A", "B", "E1"],
        &["A", "B", "E1"],
        link("c2", false),
        Some(OWNER),
    );
    let v = ids(&Outcome::default(), &before, &after, &mut Ghost::default());
    assert!(v.contains(&"I4".to_owned()), "{v:?}");
    let dirty = snap(
        &["A", "B", "E1"],
        &["A", "B", "E1"],
        link("c2", true),
        Some(OWNER),
    );
    let v = ids(&Outcome::default(), &before, &dirty, &mut Ghost::default());
    assert!(!v.contains(&"I4".to_owned()), "{v:?}");
    let memory_only = snap(
        &["A", "B"],
        &["A", "B", "E1"],
        link("c2", false),
        Some(OWNER),
    );
    let v = ids(
        &Outcome::default(),
        &before,
        &memory_only,
        &mut Ghost::default(),
    );
    assert!(
        v.contains(&"I4".to_owned()),
        "the open copy's memory counts too: {v:?}"
    );
}

#[test]
fn a_base_the_server_never_had_is_i3() {
    let s = snap(&["A"], &["A"], link("nope", true), Some(OWNER));
    let v = ids(&Outcome::default(), &s, &s, &mut Ghost::default());
    assert!(v.contains(&"I3".to_owned()), "{v:?}");
}

#[test]
fn a_released_lock_still_held_is_i2_and_a_false_delete_is_i9() {
    let s = snap(&["A", "B"], &["A", "B"], link("c2", false), Some(OWNER));
    let out = claims(vec![
        Claim::LockReleased {
            project_id: "p1".to_owned(),
        },
        Claim::Deleted {
            project_id: "p1".to_owned(),
        },
        Claim::LinkDropped,
    ]);
    let v = ids(&out, &s, &s, &mut Ghost::default());
    assert!(
        v.contains(&"I2".to_owned()) && v.contains(&"I9".to_owned()),
        "{v:?}"
    );
    let silent = claims(vec![Claim::LockReleaseBestEffort {
        project_id: "p1".to_owned(),
    }]);
    let v = ids(&silent, &s, &s, &mut Ghost::default());
    assert!(v.contains(&"I2b".to_owned()), "{v:?}");
    // [#576 SDB-ACCT-D1, SDB-QUIT-D1] The owner account took the lock in another
    // session (the second client): the app edits under it, so a silent release that
    // left it held is still I2b.
    let mut other_session = Ghost::default();
    other_session.interference.insert("p1".to_owned());
    let v = ids(&silent, &s, &s, &mut other_session);
    assert!(v.contains(&"I2b".to_owned()), "{v:?}");
    // The second client changed the owner's access (or deleted the project): excused.
    let mut access_changed = Ghost::default();
    access_changed.interference.insert("p1".to_owned());
    access_changed.access_interference.insert("p1".to_owned());
    let v = ids(&silent, &s, &s, &mut access_changed);
    assert!(!v.contains(&"I2b".to_owned()), "{v:?}");
}

#[test]
fn a_push_that_disagrees_with_the_tip_is_i6() {
    let before = snap(&["A", "B"], &["A", "B"], link("c2", false), Some(OWNER));
    let stale = snap(&["A", "B", "E1"], &["A", "B", "E1"], link("c1", true), None);
    let out = claims(vec![Claim::InSync {
        project_id: "p1".to_owned(),
    }]);
    let v = ids(&out, &before, &stale, &mut Ghost::default());
    assert!(v.contains(&"I6".to_owned()), "{v:?}");
    let good = snap(&["A", "B"], &["A", "B"], link("c2", false), None);
    let v = ids(&out, &before, &good, &mut Ghost::default());
    assert!(v.is_empty(), "{v:?}");
}

#[test]
fn an_unasked_unlink_is_i10_and_editing_without_the_lock_is_i11() {
    let before = snap(&["A", "B"], &["A", "B"], link("c2", false), Some(OWNER));
    let mut after = before.clone();
    if let Some(f) = after.files.get_mut(&cache_file()) {
        f.link = Ok(None);
    }
    let v = ids(&Outcome::default(), &before, &after, &mut Ghost::default());
    assert!(v.contains(&"I10".to_owned()), "{v:?}");
    let v = ids(
        &claims(vec![Claim::LinkDropped]),
        &before,
        &after,
        &mut Ghost::default(),
    );
    assert!(!v.contains(&"I10".to_owned()), "{v:?}");

    let mut editing = snap(
        &["A", "B"],
        &["A", "B"],
        link("c2", false),
        Some("collab@fuzz.invalid"),
    );
    editing.edit_enabled = true;
    let v = ids(
        &Outcome::default(),
        &before,
        &editing,
        &mut Ghost::default(),
    );
    assert!(v.contains(&"I11".to_owned()), "{v:?}");
    let mut excused = Ghost::default();
    excused.interference.insert("p1".to_owned());
    let v = ids(&Outcome::default(), &before, &editing, &mut excused);
    assert!(!v.contains(&"I11".to_owned()), "{v:?}");
}

#[test]
fn orphans_uncoded_refusals_panics_and_a_foreign_endpoint_are_reported() {
    let before = snap(&["A", "B"], &["A", "B"], link("c2", false), Some(OWNER));
    let mut after = before.clone();
    after
        .cache_files
        .push(root().join("p1").join("project.stalx.sanitize.tmp"));
    after.endpoint = Some("https://www.speleodb.org/".to_owned());
    let out = Outcome {
        refusals: vec![Refusal {
            verb: "done".to_owned(),
            code: "INTERNAL_ERROR".to_owned(),
            message: "boom".to_owned(),
        }],
        panicked: Some("index out of bounds".to_owned()),
        ..Outcome::default()
    };
    let v = ids(&out, &before, &after, &mut Ghost::default());
    for id in ["I5", "I7", "I7-panic", "I8"] {
        assert!(v.contains(&id.to_owned()), "{id} in {v:?}");
    }
}
