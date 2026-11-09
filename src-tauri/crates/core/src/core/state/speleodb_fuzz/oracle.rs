//! The oracle: the invariants the fuzzer holds the app to after every step.
//! Each violation carries a stable id; ids that reproduce a finding of the
//! master review name it (`R1-..`, `R3-..`, `P2-..`). The groups:
//!
//! - [`super::oracle_lock`]: lock ownership on every exit path, the edit gate,
//!   other accounts' locks (`LK-*`, `R1-*`, `R5-*`, `P2-*` lock items).
//! - [`super::oracle_data`]: BASE, the dirty bit, edits, uploads, overwrites,
//!   newer-schema files and the cache (`DB-*`, `CA-*`, `R3-*`, `R6-*`).
//! - [`super::oracle_proto`]: what the request log and the answers show
//!   (`RL-*`, `RO-*`, `EX-*`, `SE-*`, `OFF-*`, `ST-*`, `ERR-*`, `ENDPOINT`, `P2-*`).
//! - [`super::interleave`]: late replies after a session move (`ST-LATE`,
//!   `R7-*`), credentials after a sign-out (`ST-CRED`), main-thread commands
//!   waiting on a held network span (`R4-*`, `P2-project-mutex-block`).

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use super::app::{Claim, Outcome};
use super::data::Fp;
use super::model::Act;
use super::proxy::ProxyEvent;
use super::snap::Snap;
use super::world::Server;

/// One broken invariant.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub(super) struct Violation {
    pub(super) id: String,
    pub(super) severity: String,
    pub(super) detail: String,
}

pub(super) fn violation(id: &str, detail: String) -> Violation {
    let severity = match id {
        "ENDPOINT" => "critical",
        _ if id.starts_with("DB-LOSS")
            || id.starts_with("R3")
            || id.starts_with("R6")
            || id.starts_with("R7")
            || id.starts_with("ERR-PANIC")
            || id.starts_with("ST-") =>
        {
            "high"
        }
        _ if id.starts_with("LK") || id.starts_with("R1") || id.starts_with("DB") => "medium",
        _ => "low",
    };
    Violation {
        id: id.to_owned(),
        severity: severity.to_owned(),
        detail,
    }
}

/// The content of a server commit, by project and sha (a map in offline tests).
pub(super) trait CommitFps {
    fn commit_fp(&mut self, id: &str, sha: &str) -> Result<Fp, String>;
    fn is_newer(&self, id: &str, sha: &str) -> bool;
}

impl CommitFps for Server {
    fn commit_fp(&mut self, id: &str, sha: &str) -> Result<Fp, String> {
        self.fingerprint_at(id, sha)
    }

    fn is_newer(&self, id: &str, sha: &str) -> bool {
        Server::is_newer(self, id, sha)
    }
}

/// The fuzzer's memory across steps.
#[derive(Default, Debug)]
pub(super) struct Ghost {
    /// Edit tokens the app accepted that no server commit holds yet.
    pub(super) pending: BTreeSet<String>,
    /// Projects whose lock state the app cannot have verified: a fault hit one of
    /// their requests, or the second client touched their lock or the app
    /// account's access, since the app last confirmed it without a fault.
    pub(super) unverified: HashSet<String>,
    /// Projects whose app-account access the second client changed, or that it
    /// deleted, since the app last acquired them: a refused release is excused.
    pub(super) access_changed: HashSet<String>,
    /// Projects whose acquire answered ambiguously (the server may hold it).
    pub(super) lost_acquire: HashSet<String>,
    /// Projects a crash left a hot journal beside.
    pub(super) residue: HashSet<String>,
    /// Projects that received an upload with an over-long commit message.
    pub(super) long_message: HashSet<String>,
}

/// The run's accounts, lowercased.
#[derive(Clone, Debug, Default)]
pub(super) struct Emails {
    pub(super) owner: String,
    pub(super) collab: String,
    pub(super) ro: Option<String>,
}

/// Everything one check needs about the step.
pub(super) struct StepCtx<'a> {
    pub(super) act: &'a Act,
    pub(super) out: &'a Outcome,
    pub(super) events: &'a [ProxyEvent],
    pub(super) before: &'a Snap,
    pub(super) after: &'a Snap,
    pub(super) emails: &'a Emails,
    pub(super) allowed: &'a [String],
    pub(super) cache_root: &'a Path,
    /// `(project, sha)` of every revision the second client stamped newer-schema.
    pub(super) newer_shas: &'a BTreeSet<(String, String)>,
}

impl StepCtx<'_> {
    /// Whether a newer schema wrote revision `sha` of `project_id`.
    pub(super) fn newer(&self, project_id: &str, sha: &str) -> bool {
        self.newer_shas
            .contains(&(project_id.to_owned(), sha.to_owned()))
    }

    pub(super) fn claims(&self) -> impl Iterator<Item = &Claim> {
        self.out.claims.iter()
    }

    pub(super) fn has(&self, f: impl Fn(&Claim) -> bool) -> bool {
        self.out.claims.iter().any(f)
    }

    /// Every account the app was signed in as before or after the step.
    pub(super) fn app_accounts(&self) -> Vec<String> {
        [&self.before.app.email, &self.after.app.email]
            .into_iter()
            .flatten()
            .cloned()
            .collect()
    }

    /// The faults that fired in the step, with the request they fired on.
    pub(super) fn faulted(&self) -> impl Iterator<Item = (&ProxyEvent, &str)> {
        self.events
            .iter()
            .filter_map(|e| e.fault.as_deref().map(|f| (e, f)))
    }

    /// Whether a fault fired on any request naming `project_id`, or one that makes
    /// the server look unreachable fired on any request at all.
    pub(super) fn fault_touched(&self, project_id: &str) -> bool {
        self.faulted().any(|(e, f)| {
            e.path.contains(project_id) || is_unreachable(f) || e.path.ends_with("api/v2/projects/")
        })
    }
}

/// A fault the client sees as an unreachable or silent server.
pub(super) fn is_unreachable(label: &str) -> bool {
    [
        "Refuse",
        "HangThenForward",
        "DropBeforeForward",
        "DropAfterForward",
        "TruncateBody",
    ]
    .iter()
    .any(|k| label.starts_with(k))
}

/// The project id a request path names (`api/v2/projects/<id>/...`).
pub(super) fn project_of(path: &str) -> Option<&str> {
    let rest = path.split("api/v2/projects/").nth(1)?;
    let id = rest.split('/').next()?;
    (id.len() == 36 && id.chars().filter(|c| *c == '-').count() == 4).then_some(id)
}

/// Update the ghost from the step, then check every invariant.
pub(super) fn check(
    ghost: &mut Ghost,
    cx: &StepCtx<'_>,
    server: &mut dyn CommitFps,
) -> Vec<Violation> {
    let mut v = Vec::new();
    super::oracle_proto::check(ghost, cx, &mut v);
    super::oracle_lock::check(ghost, cx, &mut v);
    super::oracle_data::check(ghost, cx, server, &mut v);
    v
}

/// What a set of section names lacks against another, for details.
pub(super) fn diff(a: &Fp, b: &Fp) -> String {
    let only_a: Vec<_> = a.iter().filter(|(k, n)| b.get(*k) != Some(*n)).collect();
    let only_b: Vec<_> = b.iter().filter(|(k, n)| a.get(*k) != Some(*n)).collect();
    format!("local-only {only_a:?} / server-only {only_b:?}")
}

pub(super) fn short(path: &Path) -> String {
    let parent = path
        .parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!(
        "{parent}/{}",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    )
}

#[test]
fn a_project_path_names_its_id() {
    let id = "0b5c2d1e-0000-4000-8000-123456789abc";
    assert_eq!(
        project_of(&format!("/api/v2/projects/{id}/acquire/")),
        Some(id)
    );
    assert_eq!(project_of("/api/v2/projects/"), None);
    assert_eq!(project_of("/api/v2/user/"), None);
}
