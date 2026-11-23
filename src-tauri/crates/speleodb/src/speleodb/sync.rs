//! Pure SpeleoDB sync-engine logic - the data-loss-critical decisions, with no
//! I/O.
//!
//! These functions take the SHAs / commit lists already fetched by the client
//! and return a typed verdict that the orchestration in the app crate's
//! `core::state::speleodb` acts on. Keeping the policy pure makes every
//! branch exhaustively unit-testable offline, which is what the guardrail suite
//! (#211) relies on.
//!
//! - [`fence`] - the base-SHA divergence fence (#192): BASE vs server tip.
//! - [`classify_ancestry`] - guardrail-7 ancestry recovery (#193): after a push
//!   whose confirmation was lost, did my edit actually land on the server? Only
//!   a revision this session can show it uploaded ([`OwnUploads`]) counts as
//!   ours, never one that merely carries our email (R3).
//! - [`replaced_by_upload`] - after a landed upload, the revisions it took HEAD
//!   from: another session's, pushed between the push's HEAD check and the
//!   upload (R3, the lost race).

use std::collections::{HashMap, HashSet};

use super::commit_message::normalize_like_server;
use super::models::{RemoteState, RevisionCommit};

/// Verdict of the base-SHA fence: our recorded BASE commit compared against the
/// server's current tip, given whether we hold un-pushed local edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FenceOutcome {
    /// Server tip == our BASE (including both at the empty-repo BASE). There is
    /// no remote change to reconcile; any local dirty edits are simply ours to
    /// push.
    UpToDate,
    /// Server advanced beyond our BASE and we hold NO local edits - safe to
    /// fast-forward (pull the server state and re-stamp BASE).
    FastForward,
    /// Server advanced beyond our BASE AND we hold un-pushed local edits - a
    /// genuine divergence. NEVER auto-resolved or overwritten; the caller
    /// surfaces it for the user to reconcile.
    Diverged,
    /// Anomalous: we hold a BASE commit but the server reports no tip at all
    /// (e.g. a transient/empty response, or a vanished commit). Fail closed -
    /// the caller should re-fetch once before trusting it and must never assume
    /// it may overwrite the server.
    FailClosed,
}

/// The base-SHA divergence fence (#192).
///
/// Compares our recorded BASE against the server tip to decide whether starting
/// to edit (or refreshing) is a no-op, a safe fast-forward, a divergence, or an
/// anomaly to fail closed on. The server has **no** optimistic-concurrency
/// control (last-writer-wins), so this client-side fence is the *only* thing
/// standing between a stale local copy and silently clobbering someone else's
/// push.
///
/// - `base`  - our `speleo_link.base_commit_sha` (`None` = empty-repo BASE).
/// - `tip`   - the server's current latest commit (`None` = server reports none).
/// - `dirty` - `has_unpushed_edits`.
#[must_use]
pub(crate) fn fence(base: Option<&str>, tip: Option<&str>, dirty: bool) -> FenceOutcome {
    match (base, tip) {
        // base == tip (including the empty-repo None == None): nothing remote to
        // reconcile. The dirty bit is orthogonal - those edits are ours to push.
        (None, None) => FenceOutcome::UpToDate,
        (Some(b), Some(t)) if b == t => FenceOutcome::UpToDate,
        // We based on a real commit but the server now reports none - anomalous,
        // never an invitation to overwrite.
        (Some(_), None) => FenceOutcome::FailClosed,
        // Server tip moved beyond our BASE: either (None -> Some) (the repo
        // gained its first commit since we pulled the empty BASE) or two
        // differing Somes. Clean -> fast-forward; dirty -> divergence.
        _ => {
            if dirty {
                FenceOutcome::Diverged
            } else {
                FenceOutcome::FastForward
            }
        }
    }
}

/// Verdict of guardrail-7 ancestry recovery: after a push whose confirmation was
/// lost, whether our edit landed on the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AncestryOutcome {
    /// The server tip is still our BASE - our push is NOT on the server. The
    /// re-push is retry-safe.
    NotLanded,
    /// Our push landed: every commit between BASE (exclusive) and the tip is ours
    /// and forms a linear chain back to BASE (or to the root, for an empty-repo
    /// BASE). Advance BASE to the tip. `clear_dirty` is true ONLY when no local
    /// edit happened after the push began (`last_edit_at <= push_start_at`); a
    /// post-push edit is not in the landed commit, so the dirty bit must stay set.
    Landed { clear_dirty: bool },
    /// A commit authored by someone else sits between BASE and the tip - a real
    /// divergence. Never overwritten.
    Diverged,
    /// History we cannot reason about safely (a merge / multiple parents, a
    /// referenced commit missing from the fetched history, or a tip we can't
    /// resolve). Fail closed.
    FailClosed,
}

/// What THIS session knows it uploaded of the open project (R3): the revisions
/// its uploads' answers named, and the uploads whose answer never came.
///
/// The revision history names each revision's author by email, but SpeleoDB
/// locks are per ACCOUNT: a second session of the same account (another
/// machine, another window) holds the lock too, and its revisions carry our
/// email. So the email says nothing about which session pushed a revision, and
/// adopting one as "our own landed push" uploads this copy over that
/// session's work (the fuzzer's same-account-overwrite: its revision is then
/// only in the history). This record is what does say it. It is process-local
/// and never persisted: a restart opens through Start editing, whose reconcile
/// has no authorship recovery at all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OwnUploads {
    /// The revisions this session's uploads made, as the server's 200 answers
    /// named them (the envelope's `hexsha`).
    pub landed: Vec<String>,
    /// The uploads whose answer was lost (a dropped connection, a 5xx retried to
    /// exhaustion, a 200 that named no revision), oldest first, until an answer
    /// or an adoption settles them: each made at most one revision, directly on
    /// [`UnansweredUpload::base`], with its description. More than one when a
    /// retry was sent while an earlier upload's fate was still unknown: the
    /// server may yet land the earlier one, and the retry must not take it for
    /// another session's (F4R3-locks-push-5).
    pub unanswered: Vec<UnansweredUpload>,
}

/// An upload this session sent whose answer did not say what it made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnansweredUpload {
    /// The BASE the upload sat on (`None` = the empty-repo BASE).
    pub base: Option<String>,
    /// The description it carried, as sent.
    pub message: String,
}

impl UnansweredUpload {
    /// Could `commit` be the revision this upload made? It must sit directly on
    /// the upload's BASE (an upload is one revision on the tip it was sent to;
    /// uploads are never retried by the client), be authored by our account,
    /// and carry the upload's description as the server stores it
    /// ([`normalize_like_server`]).
    ///
    /// A revision with no description is ours only where the server keeps no
    /// descriptions at all (`described_history` false: an older server, whose
    /// history is matched on the BASE and the account alone). Our upload always
    /// carries one (the upload verbs put in a default for a blank one), so where
    /// the history shows descriptions, an undescribed revision on our BASE is
    /// another session's or another tool's (pre-PR review of the X3 round,
    /// should-fix 4: it used to count as ours and could clear the dirty mark).
    ///
    /// The description is what tells our own landed upload from another
    /// session's revision in the one place they can both be: on our BASE,
    /// authored by our account. It cannot tell two sessions that wrote the same
    /// words, nor anything on a server that keeps no descriptions: the residue.
    pub fn made(&self, commit: &RevisionCommit, my_email: &str, described_history: bool) -> bool {
        let on_base = match (&self.base, commit.parent_ids.as_slice()) {
            (None, []) => true,
            (Some(base), [parent]) => parent.eq_ignore_ascii_case(base),
            _ => false,
        };
        let ours =
            !my_email.trim().is_empty() && commit.author_email.eq_ignore_ascii_case(my_email);
        let described = match commit.message.as_deref() {
            Some(stored) => normalize_like_server(stored) == normalize_like_server(&self.message),
            None => !described_history,
        };
        on_base && ours && described
    }
}

/// Whether the server keeps revision descriptions on this history: any revision
/// in it carries one. [`UnansweredUpload::made`] matches an undescribed revision
/// only where it does not.
#[must_use]
pub fn history_records_descriptions(commits: &[RevisionCommit]) -> bool {
    commits.iter().any(|commit| commit.message.is_some())
}

/// Guardrail-7 ancestry recovery (#193).
///
/// Invoked when a push's confirmation was lost (an ambiguous result): the dirty
/// bit is still set and BASE still old, but the edit may or may not have reached
/// the server. We fetch the linear revision history and classify whether our
/// edit landed, so we never re-push over our own landed commit nor clobber a
/// stranger's.
///
/// "My" commits are the ones `own` can account for: a revision one of this
/// session's uploads was answered with, or the one revision its unanswered
/// upload may have made ([`UnansweredUpload::made`]). The author email alone is
/// NOT enough: the lock is the account's, so a second session of the same
/// account (another machine) holds it too and its revisions carry our email
/// (R3, the fuzzer's same-account-overwrite). A gap commit nothing in `own`
/// accounts for is someone else's push, whoever it names: the divergence
/// prompt, never an adopt and an upload over it.
///
/// - `base` - our recorded BASE (`None` = empty-repo BASE).
/// - `tip` - the server's current tip (`None` = server reports none).
/// - `commits` - the project's linear revision history.
/// - `my_email` - the authenticated user's email.
/// - `own` - what this session knows it uploaded.
/// - `last_edit_at` / `push_start_at` - RFC-3339 UTC timestamps gating whether
///   the landed push captured all our edits (see [`AncestryOutcome::Landed`]).
pub(crate) fn classify_ancestry(
    base: Option<&str>,
    tip: Option<&str>,
    commits: &[RevisionCommit],
    my_email: &str,
    own: &OwnUploads,
    last_edit_at: Option<&str>,
    push_start_at: Option<&str>,
) -> AncestryOutcome {
    // 1. Server tip unchanged from our BASE (incl. None == None): our push is not
    //    on the server, so it is safe to retry.
    if base == tip {
        return AncestryOutcome::NotLanded;
    }
    // 2. base != tip. A vanished server tip (base Some, tip None) is anomalous.
    let Some(tip) = tip else {
        return AncestryOutcome::FailClosed;
    };

    let by_id: HashMap<&str, &RevisionCommit> =
        commits.iter().map(|c| (c.id.as_str(), c)).collect();

    // 3. Walk the linear chain from the tip back toward BASE (or the root, for an
    //    empty-repo BASE), collecting the commits new relative to BASE.
    let mut cursor: &str = tip;
    let mut new_commits: Vec<&RevisionCommit> = Vec::new();
    loop {
        // Reached our BASE: the chain base..tip is fully collected.
        if base == Some(cursor) {
            break;
        }
        let Some(c) = by_id.get(cursor).copied() else {
            // A referenced commit is absent from the fetched history - we can't
            // verify ancestry.
            return AncestryOutcome::FailClosed;
        };
        // A merge (more than one parent) is non-linear - cannot reason safely.
        if c.parent_ids.len() > 1 {
            return AncestryOutcome::FailClosed;
        }
        new_commits.push(c);
        if let Some(parent) = c.parent_ids.first() {
            cursor = parent.as_str();
        } else {
            // Reached a root commit.
            if base.is_none() {
                break; // empty-repo BASE: the root is the valid terminus.
            }
            // We hold a BASE commit, but it is not in the tip's linear history -
            // the histories diverged.
            return AncestryOutcome::Diverged;
        }
        // Cycle / runaway guard: a linear chain can't be longer than the history.
        if new_commits.len() > commits.len() {
            return AncestryOutcome::FailClosed;
        }
    }

    // 4. `new_commits` is the set strictly between BASE (exclusive) and the tip
    //    (inclusive); `base != tip` guarantees it is non-empty.
    if new_commits.is_empty() {
        return AncestryOutcome::FailClosed; // defensive (unreachable for base != tip)
    }
    // We must be able to identify our OWN commits to claim the gap as our landed
    // push. An empty `my_email` (absent session email → unwrap_or_default) matches
    // nothing, and a server-omitted author deserializes to "" (#[serde(default)]):
    // without this guard `"" == ""` would mis-classify a stranger's commit as ours
    // and adopt+overwrite it. Fail closed to the divergence prompt instead.
    if my_email.trim().is_empty() {
        return AncestryOutcome::Diverged;
    }
    // Every new commit must be one this session can account for (R3): a revision
    // an upload's answer named, or the one revision an unanswered upload may have
    // made (each accounts for one at most). Anything else is somebody else's push
    // - a stranger's, or another session of our own account, which holds the same
    // per-account lock and signs with our email.
    let mut unanswered: Vec<&UnansweredUpload> = own.unanswered.iter().collect();
    let described_history = history_records_descriptions(commits);
    for c in &new_commits {
        if own.landed.iter().any(|id| id.eq_ignore_ascii_case(&c.id)) {
            continue;
        }
        match unanswered
            .iter()
            .position(|upload| upload.made(c, my_email, described_history))
        {
            Some(at) => {
                unanswered.remove(at);
            }
            None => return AncestryOutcome::Diverged,
        }
    }
    // All new commits are ours and linear back to BASE/root → our push landed.
    AncestryOutcome::Landed {
        clear_dirty: edits_captured_by_push(last_edit_at, push_start_at),
    }
}

/// Resolve the server HEAD as the leaf of a linear commit history: the unique
/// commit that **no other commit lists as a parent**.
///
/// We CANNOT use the server's `latest_commit` field for this. It is computed as
/// `commits.order_by("-authored_date").first()` - the max git *author* timestamp
/// - and project-creation + the first upload land within the same second, so
/// their author timestamps tie and the tie breaks arbitrarily (verified live: it
/// returns the service-account ROOT, not the real HEAD; confirmed in the server
/// source). The parent-id topology is unambiguous, so the fence resolves the tip
/// from it instead.
///
/// Returns `None` for an empty history OR an ambiguous one (zero or multiple
/// leaves - e.g. a fork), so the caller fails closed rather than guessing.
pub fn tip_of(commits: &[RevisionCommit]) -> Option<&str> {
    let parents: HashSet<&str> = commits
        .iter()
        .flat_map(|c| c.parent_ids.iter().map(String::as_str))
        .collect();
    let mut leaves = commits
        .iter()
        .map(|c| c.id.as_str())
        .filter(|id| !parents.contains(id));
    let head = leaves.next()?;
    if leaves.next().is_some() {
        return None; // more than one head (fork) - ambiguous, fail closed
    }
    Some(head)
}

/// Pure structural check: is `base` reachable by walking the linear parent chain
/// back from `tip`? I.e. is the server `tip` a genuine **descendant** of our
/// `base` (a real fast-forward) rather than an **ancestor** or a fork.
///
/// This guards the clean `FastForward` path against the server's
/// eventually-consistent `latest_commit`: right after a confirmed push, `acquire`
/// / `get_project` were observed live to still report the PRIOR tip (the
/// service-account root) as `latest_commit`. Fast-forwarding to that stale
/// ancestor would re-pull an OLDER tree and reset the project BACKWARD, losing the
/// just-pushed state. Only fast-forward when this returns `true`.
///
/// - `base == None` → we hold no commit yet (fresh open); any tip is adoptable → `true`.
/// - `base == tip` → already there → `true`.
/// - a merge (non-linear) or a commit missing from `commits` → `false` (fail safe:
///   never fast-forward into something we cannot verify).
/// - reaching a root without meeting `base` → `base` is not an ancestor → `false`.
pub(crate) fn base_is_ancestor_of_tip(
    base: Option<&str>,
    tip: &str,
    commits: &[RevisionCommit],
) -> bool {
    let Some(base) = base else {
        return true;
    };
    if base == tip {
        return true;
    }
    let by_id: HashMap<&str, &RevisionCommit> =
        commits.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut cursor: &str = tip;
    let mut steps = 0usize;
    loop {
        if cursor == base {
            return true;
        }
        let Some(c) = by_id.get(cursor).copied() else {
            return false; // tip (or an ancestor) absent from history - can't verify
        };
        if c.parent_ids.len() > 1 {
            return false; // a merge is non-linear - refuse
        }
        let Some(parent) = c.parent_ids.first() else {
            return false; // reached a root without meeting base → not an ancestor
        };
        cursor = parent.as_str();
        steps += 1;
        if steps > commits.len() {
            return false; // cycle / runaway guard
        }
    }
}

/// After a server error answered an upload: is it CERTAIN that the upload did
/// NOT land (plan 12.1)? Only then may the push call the error a refusal of the
/// file (`contract::upload_refusal`); anything less stays the ambiguous
/// "could not be confirmed", recovered by ancestry on the next push.
///
/// - A recorded `base`: the server tip is still that commit (compared
///   case-insensitively, like every SHA here). A moved tip, a missing one or a
///   fork is not certain.
/// - No `base` (a just-published project whose create response named no commit):
///   no commit in the history is authored by `my_email` - the server's own
///   creation commit is not ours. An empty `my_email` cannot tell ours from
///   anyone's, so it is never certain.
///
/// Pure over an already-fetched history, like [`plan_push`].
#[must_use]
pub fn upload_not_landed(
    base: Option<&str>,
    tip: Option<&str>,
    commits: &[RevisionCommit],
    my_email: &str,
) -> bool {
    match base {
        Some(base) => tip.is_some_and(|tip| tip.eq_ignore_ascii_case(base)),
        None => {
            !my_email.trim().is_empty()
                && !commits
                    .iter()
                    .any(|c| c.author_email.eq_ignore_ascii_case(my_email))
        }
    }
}

/// After an upload landed as the revision `landed` on the BASE `upload_base`:
/// the revisions that landed between that BASE and it, newest first (R3, the
/// lost race).
///
/// The push checks that the server's HEAD is still its BASE before it uploads,
/// but SpeleoDB has no conditional upload: another session of the same account
/// (which holds the same per-account lock) can land a revision between that
/// check and the upload, and the server then puts ours on top of it. Ours does
/// not contain that session's work, so those revisions are no longer HEAD; the
/// history still has them, where they can be viewed or restored.
///
/// Empty in the ordinary case (`landed` sits directly on `upload_base`) and when
/// the history cannot tell (`landed` is not in it). SpeleoDB's own creation
/// commit is never reported. A chain that never reaches `upload_base` (a
/// history rewritten on the server) reports only the revision directly under
/// ours, the one ours took HEAD from, rather than everything down to the root.
///
/// Pure over a history fetched after the upload, like [`plan_push`].
#[must_use]
pub fn replaced_by_upload(
    upload_base: Option<&str>,
    landed: &str,
    commits: &[RevisionCommit],
) -> Vec<RevisionCommit> {
    let by_id: HashMap<String, &RevisionCommit> = commits
        .iter()
        .map(|c| (c.id.to_ascii_lowercase(), c))
        .collect();
    let Some(top) = by_id.get(&landed.to_ascii_lowercase()) else {
        return Vec::new();
    };
    let is_base = |id: &str| upload_base.is_some_and(|base| base.eq_ignore_ascii_case(id));
    let mut replaced = Vec::new();
    let mut cursor = top.parent_ids.first();
    let mut reached_base = upload_base.is_none();
    while let Some(id) = cursor {
        if is_base(id) {
            reached_base = true;
            break;
        }
        // A commit outside the fetched history, or a runaway chain: stop here.
        let Some(commit) = by_id.get(&id.to_ascii_lowercase()) else {
            break;
        };
        if replaced.len() >= commits.len() {
            break;
        }
        replaced.push((*commit).clone());
        cursor = commit.parent_ids.first();
    }
    if !reached_base {
        replaced.truncate(1);
    }
    replaced.retain(|c| !is_project_creation_commit(c));
    replaced
}

/// `replaced` ([`replaced_by_upload`]) without the revisions one of this
/// session's own unanswered uploads may have made ([`UnansweredUpload::made`]):
/// an upload whose answer was lost, retried before the server had landed it,
/// can land late, under the retry. That revision is this copy's own earlier
/// snapshot, not another session's work, and reporting it as replaced would
/// stop the exit over our own upload (F4R3-locks-push-5).
#[must_use]
pub fn not_our_own(
    replaced: Vec<RevisionCommit>,
    unanswered: &[UnansweredUpload],
    my_email: &str,
    described_history: bool,
) -> Vec<RevisionCommit> {
    replaced
        .into_iter()
        .filter(|commit| {
            !unanswered
                .iter()
                .any(|upload| upload.made(commit, my_email, described_history))
        })
        .collect()
}

/// Whether a confirmed-landed push captured all our local edits - i.e. no edit
/// happened after the push began. Biased to **false** (keep the dirty bit) on any
/// uncertainty: a wrongly-kept dirty bit costs only a redundant no-op re-push,
/// whereas a wrongly-cleared one drops un-pushed edits.
///
/// Used both by `classify_ancestry` (g7) and by the push 200/304 dirty-clear
/// (#197): a confirmed push clears the dirty bit ONLY when this returns true, so a
/// concurrent edit recorded after `push_start_at` (not in the uploaded snapshot)
/// keeps its dirty bit and is re-pushed on the next Done rather than silently lost.
pub fn edits_captured_by_push(last_edit_at: Option<&str>, push_start_at: Option<&str>) -> bool {
    match (last_edit_at, push_start_at) {
        (Some(le), Some(ps)) => match (
            chrono::DateTime::parse_from_rfc3339(le),
            chrono::DateTime::parse_from_rfc3339(ps),
        ) {
            (Ok(edited), Ok(started)) => edited <= started,
            _ => false, // unparseable → conservative
        },
        // Unknown edit/push timing (e.g. a legacy dirty bit) → keep dirty.
        _ => false,
    }
}

/// Verdict for the read-side reconcile paths (Start editing / Update): how our
/// local copy stands against the server tip, collapsing the fence + the
/// fast-forward ancestry guard into the three outcomes those callers act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileVerdict {
    /// BASE == tip: nothing remote to reconcile.
    UpToDate,
    /// Tip moved forward of a clean BASE AND BASE is a genuine ancestor of it -
    /// safe to pull + re-stamp.
    FastForward,
    /// Anything we must NOT auto-resolve: a real fork, a fast-forward target BASE
    /// is not an ancestor of (the eventually-consistent `latest_commit`), OR a
    /// server inconsistency (non-linear / multi-root / a recorded BASE with no
    /// tip). All route to the divergence PROMPT - never a hard error, never an
    /// overwrite (#193 b2: "fail closed to the divergence prompt").
    Diverged,
}

/// Read-side reconcile decision for Start editing / Update (#192, #193 b2). Pure
/// over the already-fetched tip + history so every branch is unit-tested offline.
/// FailClosed folds into `Diverged` here precisely so non-linear / multi-root /
/// empty-with-base histories surface the divergence prompt instead of erroring.
#[must_use]
pub fn reconcile_verdict(
    base: Option<&str>,
    tip: Option<&str>,
    dirty: bool,
    commits: &[RevisionCommit],
) -> ReconcileVerdict {
    match fence(base, tip, dirty) {
        FenceOutcome::UpToDate => ReconcileVerdict::UpToDate,
        FenceOutcome::FastForward
            if base_is_ancestor_of_tip(base, tip.unwrap_or_default(), commits) =>
        {
            ReconcileVerdict::FastForward
        }
        // FastForward-but-not-ancestor, Diverged, and FailClosed all fail closed to
        // the divergence prompt rather than overwriting or surfacing a hard error.
        FenceOutcome::FastForward | FenceOutcome::Diverged | FenceOutcome::FailClosed => {
            ReconcileVerdict::Diverged
        }
    }
}

/// What a Done push should do, after fencing BASE against the server tip and (on a
/// moved tip we hold the lock over) classifying the BASE..tip gap by authorship.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushPlan {
    /// BASE == tip: upload our edits directly.
    Upload,
    /// Our prior (ambiguous) push landed but an edit followed it: adopt the landed
    /// tip as BASE, then re-push the current state on top (a no-op 304 if nothing
    /// changed since the landed commit).
    AdoptThenUpload,
    /// Our prior push landed and nothing was edited after it: adopt the tip as BASE
    /// and clear the dirty bit WITHOUT a redundant re-push (the push is confirmed).
    AdoptConfirmed,
    /// A stranger's commit, OR a server inconsistency (non-linear / multi-root /
    /// recorded BASE with no tip): route to the divergence prompt. Never overwrite,
    /// never guess (#193 b2 + b5).
    Diverged,
}

/// Pure Done-push decision (#192/#193): fence BASE against the tip, and on a moved
/// tip classify the gap by what this session uploaded (`own`). The upload goes
/// ahead only on a HEAD that is still BASE, or one that moved by nothing but this
/// session's own landed upload. A HEAD moved by anybody else - including another
/// session of our own account, which holds the same per-account lock (R3) - is
/// the divergence prompt. Every fail-closed branch routes there too, rather than
/// to a hard error or a blind re-push.
#[must_use]
pub fn plan_push(
    base: Option<&str>,
    tip: Option<&str>,
    commits: &[RevisionCommit],
    my_email: &str,
    own: &OwnUploads,
    last_edit_at: Option<&str>,
    push_start_at: Option<&str>,
) -> PushPlan {
    match fence(base, tip, true) {
        FenceOutcome::UpToDate => PushPlan::Upload,
        FenceOutcome::Diverged => {
            match classify_ancestry(
                base,
                tip,
                commits,
                my_email,
                own,
                last_edit_at,
                push_start_at,
            ) {
                AncestryOutcome::Landed { clear_dirty: true } => PushPlan::AdoptConfirmed,
                AncestryOutcome::Landed { clear_dirty: false } => PushPlan::AdoptThenUpload,
                // Stranger commit, a non-linear/multi-root inconsistency, or (not
                // reachable on a moved tip) a not-landed push - never overwrite or
                // guess, route to the divergence prompt.
                AncestryOutcome::Diverged
                | AncestryOutcome::NotLanded
                | AncestryOutcome::FailClosed => PushPlan::Diverged,
            }
        }
        // A dirty copy never fast-forwards (the fence returns Diverged for a dirty
        // moved tip); a recorded BASE with no tip is a server inconsistency. Both
        // fail closed to the divergence prompt.
        FenceOutcome::FastForward | FenceOutcome::FailClosed => PushPlan::Diverged,
    }
}

/// The passive status-button verdict: how the open project stands against the
/// server tip, used by on-demand status refreshes. Pure over an
/// already-fetched tip + history so every branch is unit-tested offline.
///
/// This is deliberately the SAME reasoning the write path uses, not a cheaper
/// approximation. A bare [`fence`] is not a verdict: a dirty copy whose tip has
/// moved fences to `Diverged` even when the only commit in the gap is **our own
/// just-landed push**, which [`plan_push`] recovers from via `AdoptThenUpload`.
/// Two ways into that state:
///
/// - a status refresh that read the link row BEFORE a push and resolved the tip
///   AFTER it (nothing holds a command-level lock, so a push lands inside that
///   window), and
/// - the #193 ambiguous-push case, where a lost confirmation leaves BASE behind.
///
/// Reporting `Conflict` there advertises a conflict against the user's own
/// commit and routes their next click to the divergence dialog - a self-conflict
/// nobody else was involved in. Classifying the gap by authorship keeps the
/// passive button honest about what Done would actually do on click.
///
/// The authorship test is `own` (what this session uploaded, see
/// [`classify_ancestry`]), NOT `author_email == my_email`: a second session
/// signed in as the SAME account breaks that proxy exactly. Its commit reads as
/// ours, so the gap would classify as `Landed` and the mark keep its resting
/// state over a genuine conflict, ending in Done superseding the other
/// session's revision with no prompt at any point. Holding the lock does not
/// tell the two apart either (R3): SpeleoDB's lock is the account's, so both
/// sessions hold it.
///
/// `we_hold_the_lock` stays as a second gate: our own landed-but-unrecorded
/// push can only have been made while we held the single-writer lock, so
/// requiring it costs the recovery nothing.
///
/// It also keeps this verdict AGREEING with [`reconcile_verdict`], which has no
/// authorship recovery. That function drives Start editing - reachable only while
/// the lock is NOT ours - so the two now answer the same thing in every state a
/// user can click through. Without the gate the refresh could paint the resting,
/// unbadged mark while the click behind it raised the divergence prompt: the same
/// passive/active disagreement this function exists to remove, moved from the
/// badge to the click.
///
/// `FailClosed` stays separate from [`reconcile_verdict`]'s handling: a recorded
/// BASE with no server tip is a transient empty response, and nothing local is at
/// risk, so it degrades to `Unknown` rather than shouting `Conflict`.
#[must_use]
pub fn status_verdict(
    base: Option<&str>,
    tip: Option<&str>,
    dirty: bool,
    commits: &[RevisionCommit],
    my_email: &str,
    own: &OwnUploads,
    we_hold_the_lock: bool,
) -> RemoteState {
    match fence(base, tip, dirty) {
        FenceOutcome::UpToDate => RemoteState::UpToDate,
        // Only advertise a clean pull when BASE genuinely descends to the tip. A
        // FastForward whose BASE is NOT an ancestor is a server-side history
        // rewrite that Update's stricter reconcile calls Diverged - paint it the
        // same here, so the button never offers a pull that would immediately fail
        // closed to the divergence prompt on click.
        FenceOutcome::FastForward => {
            if base_is_ancestor_of_tip(base, tip.unwrap_or_default(), commits) {
                RemoteState::RemoteAhead
            } else {
                RemoteState::Conflict
            }
        }
        // Not our lock, so a commit in the gap is not ours to explain away - even
        // when it carries our email. Fail closed to the divergence prompt, which is
        // also exactly what `reconcile_verdict` will say when the mark is clicked.
        FenceOutcome::Diverged if !we_hold_the_lock => RemoteState::Conflict,
        FenceOutcome::Diverged => match classify_ancestry(
            base, tip, commits, my_email, own,
            // Both timing inputs are `None` on purpose: only the AUTHORSHIP half of
            // the answer is read below (`clear_dirty` is destructured away), so
            // threading a `last_edit_at` that cannot change the verdict would be a
            // live wire - a later change making it load-bearing in
            // `classify_ancestry` would silently move this verdict with no test
            // noticing. `plan_push` is where the freshness fold is actually used.
            None, None,
        ) {
            // Every commit between BASE and the tip is ours - our push landed.
            // Nothing remote to reconcile, so the button keeps showing the
            // ordinary editing state and Done still works.
            AncestryOutcome::Landed { .. } => RemoteState::UpToDate,
            // A stranger's commit, or history we cannot reason about safely (a
            // merge, a commit missing from the fetched range). `NotLanded` means
            // base == tip, which the fence already routed to UpToDate, so it is
            // unreachable here.
            AncestryOutcome::Diverged
            | AncestryOutcome::NotLanded
            | AncestryOutcome::FailClosed => RemoteState::Conflict,
        },
        FenceOutcome::FailClosed => RemoteState::Unknown,
    }
}

/// [#303] Which revision the next Unwind should check out, and whether that is the
/// commit the working copy already equals.
///
/// The chain is deliberately "BASE first, then first-parents". After a push the working
/// copy IS the new commit and its history has just been cleared locally, so the FIRST
/// Unwind loads that same commit to get its history back - the content does not move and
/// the undo list refills with the operations the user just pushed. Once those are undone
/// the copy sits at the parent's state, so the NEXT Unwind loads the parent, and so on.
/// (The alternative - always jumping straight to the parent - would make a user's own
/// just-pushed operations un-steppable, revertable only in one lump.)
///
/// - `unwound_from` is the chain pointer: `None` = not mid-unwind, so target BASE.
/// - `base` is `speleo_link.base_commit_sha`; `None` (an empty repo) has nothing to reach.
/// - Merges take `parent_ids[0]`, the git-mainline convention.
/// - The root commit has no parent, so the chain ENDS there and this returns `None` - the
///   frontend hides the affordance rather than offering a step that cannot be taken.
///
/// Returns `(hexsha, is_current)` where `is_current` marks the BASE case, so the caller
/// can say "reload this revision's history" rather than "go back one".
#[must_use]
pub fn unwind_target(
    unwound_from: Option<&str>,
    base: Option<&str>,
    commits: &[RevisionCommit],
) -> Option<(String, bool)> {
    match unwound_from {
        // Mid-unwind: walk to the first parent of the revision we are sitting on.
        Some(current) => {
            let commit = commits
                .iter()
                .find(|c| c.id.eq_ignore_ascii_case(current))?;
            // A commit outside the fetched range, or a genuine root: the chain ends.
            let parent = commit.parent_ids.first()?;
            // Never offer SpeleoDB's own creation commit - the same reason
            // `display_history` drops it: it predates any survey data, so "unwind to
            // nothing" is not a step anyone means to take.
            let parent_commit = commits.iter().find(|c| c.id.eq_ignore_ascii_case(parent))?;
            if is_project_creation_commit(parent_commit) {
                return None;
            }
            Some((parent_commit.id.clone(), false))
        }
        // Not mid-unwind: the working copy equals BASE, and BASE's own file carries the
        // history that produced it.
        None => {
            let base = base?;
            let commit = commits.iter().find(|c| c.id.eq_ignore_ascii_case(base))?;
            if is_project_creation_commit(commit) {
                return None;
            }
            Some((commit.id.clone(), true))
        }
    }
}

/// [#303] Which revision a forward chain step should check out - the mirror of
/// [`unwind_target`], walking back UP toward BASE instead of down toward the root.
///
/// The pointer is the whole input. A forward step is only defined relative to a walk in
/// progress: with no pointer the copy is already at BASE and there is nothing ahead of it,
/// which is also what makes an edit disable this direction (a forward command drops the
/// pointer - see `speleodb_forget_unwind_position`).
///
/// The child is found by walking the first-parent chain DOWN from BASE and taking the
/// commit whose parent is where we stand - never by scanning for "any commit whose parent
/// is `current`". Two commits can name the same parent (a branchy history, or a server that
/// retains an abandoned line), and `authored_date` cannot break the tie - the same
/// unreliable ordering [`display_history`] refuses to trust. Walking from BASE yields the
/// child on OUR path or nothing.
///
/// BASE is the end of the line. Anything beyond it is work this copy never had - another
/// session's push - and adopting that is an Update, not a step through our own history.
///
/// Returns `(hexsha, is_latest)` where `is_latest` marks the step that lands back ON BASE,
/// so the caller can say "return to the latest version" instead of naming a revision.
#[must_use]
pub fn rewind_target(
    unwound_from: Option<&str>,
    base: Option<&str>,
    commits: &[RevisionCommit],
) -> Option<(String, bool)> {
    let current = unwound_from?;
    let base = base?;
    // Already home: the walk has nothing further forward to offer.
    if base.eq_ignore_ascii_case(current) {
        return None;
    }
    // Walk BASE -> root by first parents, remembering the commit we came from, and stop
    // when a parent is where we stand: that child is the next step on our own path.
    let mut child = commits.iter().find(|c| c.id.eq_ignore_ascii_case(base))?;
    // Bounded by the commit count, the same runaway guard `display_history` walks with,
    // and for the same reason: `parent_ids` is whatever the server said, and a first-parent
    // CYCLE (`A -> B -> A`, or a self-parent) makes every lookup succeed while the exit
    // condition never fires. Unbounded, that is not a wrong answer but a HANG - the Unwind
    // wedges with no panic to report and nothing in the log. Git cannot author a cycle, so
    // this is robustness against a malformed or hostile response rather than a shape any
    // real history takes. A walk longer than the fetched range cannot reach a step we could
    // offer anyway, so exhausting the budget hides the affordance exactly like walking off
    // the end does.
    for _ in 0..commits.len() {
        let parent = child.parent_ids.first()?;
        if parent.eq_ignore_ascii_case(current) {
            let is_latest = child.id.eq_ignore_ascii_case(base);
            return Some((child.id.clone(), is_latest));
        }
        // A pointer naming a commit outside the fetched range walks off the end here and
        // yields `None`, so the affordance hides rather than offering an unreachable step.
        child = commits.iter().find(|c| c.id.eq_ignore_ascii_case(parent))?;
    }
    None
}

/// SpeleoDB's own project-creation commit: the one it writes when a project is
/// created, before any survey data exists.
///
/// Identified by its message, and only at the root of the history - both halves
/// matter. The message alone would hide a real revision someone happened to name
/// the same thing; parentless alone hides a genuine first revision on any project
/// whose history has no separate creation commit. Matching is case-insensitive and
/// ignores surrounding whitespace, because the marker is server-generated copy
/// rather than a stable identifier.
fn is_project_creation_commit(commit: &RevisionCommit) -> bool {
    const CREATION_MESSAGE: &str = "[automated] project creation";
    commit.parent_ids.is_empty()
        && commit
            .message
            .as_deref()
            .is_some_and(|m| m.trim().eq_ignore_ascii_case(CREATION_MESSAGE))
}

/// The revision history as a person should SEE it: newest first, ordered by the
/// parent chain rather than by timestamp, and without SpeleoDB's own
/// project-creation commit.
///
/// Two complaints, one cause. The server orders by `authored_date`, and project
/// creation plus the first upload land inside the same second - so the tie breaks
/// arbitrarily and the creation commit can sort ABOVE the publish that came after
/// it. That is the same unreliable ordering every fence in this module already
/// refuses to trust, so the fix is the same: walk the parent chain, which has no
/// ties to break.
///
/// The root is dropped because it is not a revision of the survey. SpeleoDB writes
/// it when the project is created, before any data exists, authored by the service
/// account rather than by anyone who has touched the cave. In a list whose two
/// actions are View and Revert, offering it invites someone to roll their project
/// back to nothing.
///
/// It is dropped on its MESSAGE, though, not merely on being parentless. Being
/// parentless is a property of every history's first commit, and not every project
/// gets a separate creation commit ahead of its first upload - an import path that
/// does not write one, or a project made before that behaviour existed, has a real
/// survey revision at its root. Hiding it structurally meant that revision could
/// not be viewed and could not be reverted to, with nothing on screen saying
/// anything had been withheld. Recognising SpeleoDB's own commit instead keeps the
/// filter aimed at the thing it was written for.
///
/// Falls back to the server's order when the chain cannot be walked - a fork, a
/// merge, or a commit outside the fetched range. The completeness check is the
/// point of that: a walk that does not account for every commit would silently
/// HIDE real revisions, which is far worse than showing them in a debatable order.
/// The root is dropped either way.
#[must_use]
pub fn display_history(commits: Vec<RevisionCommit>) -> Vec<RevisionCommit> {
    let by_id: HashMap<&str, usize> = commits
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id.as_str(), i))
        .collect();

    let walked = tip_of(&commits)
        .and_then(|tip| {
            let mut order = Vec::with_capacity(commits.len());
            let mut cursor = tip;
            loop {
                let &i = by_id.get(cursor)?;
                order.push(i);
                if order.len() > commits.len() {
                    return None; // cycle / runaway guard
                }
                match commits[i].parent_ids.first() {
                    Some(parent) => cursor = parent.as_str(),
                    None => break, // reached the root
                }
            }
            Some(order)
        })
        // Every commit must be accounted for, or we would be hiding revisions.
        .filter(|order| order.len() == commits.len());

    let order = walked.unwrap_or_else(|| (0..commits.len()).collect());
    let mut slots: Vec<Option<RevisionCommit>> = commits.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(order.len());
    for i in order {
        if let Some(commit) = slots[i].take() {
            if is_project_creation_commit(&commit) {
                continue;
            }
            out.push(commit);
        }
    }
    out
}

#[cfg(test)]
// [#303] `expect` arrived with the `unwind_target` tests, which assert on an
// `Option<(String, bool)>`: unwrapping it names the expectation at the assertion rather
// than pushing every case through a `match` whose failure arm says nothing.
#[expect(clippy::expect_used, reason = "unit test fixtures fail loudly")]
mod tests {
    use super::{
        base_is_ancestor_of_tip, classify_ancestry, display_history, edits_captured_by_push, fence,
        history_records_descriptions, not_our_own, plan_push, reconcile_verdict,
        replaced_by_upload, rewind_target, status_verdict, tip_of, unwind_target, AncestryOutcome,
        FenceOutcome, OwnUploads, PushPlan, ReconcileVerdict, RemoteState, RevisionCommit,
        UnansweredUpload,
    };

    /// Build a revision-history commit with the given parents and author.
    fn commit(id: &str, parents: &[&str], email: &str) -> RevisionCommit {
        RevisionCommit {
            id: id.to_string(),
            parent_ids: parents.iter().map(|s| (*s).to_string()).collect(),
            author_email: email.to_string(),
            authored_date: None,
            message: None,
        }
    }

    /// SpeleoDB's own project-creation commit: parentless, service-account
    /// authored, and carrying the generated marker message that identifies it.
    fn creation_commit(id: &str) -> RevisionCommit {
        RevisionCommit {
            message: Some("[Automated] Project Creation".to_owned()),
            ..commit(id, &[], "contact@speleodb.org")
        }
    }

    #[test]
    fn upload_not_landed_only_when_certain() {
        use super::upload_not_landed;
        let me = "me@example.com";
        let history = [commit("b1", &[], me)];
        // A recorded BASE: the tip is still it (either case), or it is not.
        assert!(upload_not_landed(Some("b1"), Some("b1"), &history, me));
        assert!(upload_not_landed(Some("B1"), Some("b1"), &history, me));
        let landed = [commit("b1", &[], me), commit("p2", &["b1"], me)];
        assert!(!upload_not_landed(Some("b1"), Some("p2"), &landed, me));
        // A vanished tip or a fork is not certain.
        assert!(!upload_not_landed(Some("b1"), None, &history, me));
        // No BASE (a just-published project): the server's creation commit is
        // not ours, so nothing of ours landed...
        let created = [creation_commit("root")];
        assert!(upload_not_landed(None, Some("root"), &created, me));
        assert!(upload_not_landed(None, None, &[], me));
        // ...one of ours on top of it did, whatever the case of the email...
        let ours = [
            creation_commit("root"),
            commit("p2", &["root"], "ME@example.com"),
        ];
        assert!(!upload_not_landed(None, Some("p2"), &ours, me));
        // ...and without our email nothing can be told apart.
        assert!(!upload_not_landed(None, Some("root"), &created, " "));
    }

    const ME: &str = "me@example.com";
    const OTHER: &str = "other@example.com";
    // T1 < T2 < T3, all RFC-3339 UTC.
    const T1: &str = "2026-06-27T10:00:00+00:00";
    const T2: &str = "2026-06-27T11:00:00+00:00";
    const T3: &str = "2026-06-27T12:00:00+00:00";

    #[test]
    fn up_to_date_when_both_at_empty_base() {
        // None == None: empty repo on both sides.
        assert_eq!(fence(None, None, false), FenceOutcome::UpToDate);
    }

    // --- tip_of (topological HEAD resolution) ---

    #[test]
    fn tip_of_finds_the_leaf_regardless_of_order() {
        // root <- a <- b. b is the leaf (HEAD). Order in the slice is irrelevant
        // (the server returns commits newest-first by authored_date, which ties).
        let commits = [
            commit("root", &[], OTHER),
            commit("b", &["a"], ME),
            commit("a", &["root"], ME),
        ];
        assert_eq!(tip_of(&commits), Some("b"));
    }

    #[test]
    fn tip_of_single_commit_is_the_root() {
        let commits = [commit("root", &[], OTHER)];
        assert_eq!(tip_of(&commits), Some("root"));
    }

    #[test]
    fn tip_of_none_for_empty_or_forked() {
        assert_eq!(tip_of(&[]), None);
        // Two leaves (b and c both children of a) → ambiguous fork → None.
        let forked = [
            commit("a", &[], ME),
            commit("b", &["a"], ME),
            commit("c", &["a"], ME),
        ];
        assert_eq!(tip_of(&forked), None);
    }

    // --- base_is_ancestor_of_tip (clean fast-forward guard) ---

    #[test]
    fn ancestor_guard_true_for_genuine_fast_forward() {
        // root <- a(base) <- b(tip): base IS an ancestor of tip → real FF.
        let commits = [
            commit("b", &["a"], OTHER),
            commit("a", &["root"], ME),
            commit("root", &[], OTHER),
        ];
        assert!(base_is_ancestor_of_tip(Some("a"), "b", &commits));
    }

    #[test]
    fn ancestor_guard_false_for_stale_ancestor_tip() {
        // The server reports a STALE tip "a" that is an ANCESTOR of our base "b"
        // (the eventual-consistency lag after our own push). Must NOT fast-forward
        // backward.
        let commits = [
            commit("b", &["a"], ME),
            commit("a", &["root"], OTHER),
            commit("root", &[], OTHER),
        ];
        assert!(!base_is_ancestor_of_tip(Some("b"), "a", &commits));
    }

    #[test]
    fn ancestor_guard_true_for_none_base_and_equal() {
        // No base yet → adopt any tip; and base == tip is trivially an ancestor.
        let commits = [commit("root", &[], OTHER)];
        assert!(base_is_ancestor_of_tip(None, "root", &commits));
        assert!(base_is_ancestor_of_tip(Some("root"), "root", &commits));
    }

    #[test]
    fn ancestor_guard_false_for_missing_or_merge() {
        // Tip absent from history → can't verify → false.
        assert!(!base_is_ancestor_of_tip(Some("a"), "ghost", &[]));
        // A merge commit on the path is non-linear → false.
        let merged = [commit("m", &["a", "x"], ME), commit("a", &[], ME)];
        assert!(!base_is_ancestor_of_tip(Some("a"), "m", &merged));
    }

    #[test]
    fn up_to_date_when_base_equals_tip() {
        assert_eq!(
            fence(Some("abc"), Some("abc"), false),
            FenceOutcome::UpToDate
        );
    }

    #[test]
    fn up_to_date_ignores_dirty_when_tip_unmoved() {
        // We hold local edits but no one else pushed (tip == base): still
        // UpToDate - the divergence fence is about REMOTE movement only.
        assert_eq!(
            fence(Some("abc"), Some("abc"), true),
            FenceOutcome::UpToDate
        );
        assert_eq!(fence(None, None, true), FenceOutcome::UpToDate);
    }

    #[test]
    fn fast_forward_when_tip_moved_and_clean() {
        assert_eq!(
            fence(Some("abc"), Some("def"), false),
            FenceOutcome::FastForward
        );
    }

    #[test]
    fn fast_forward_from_empty_base_when_clean() {
        // We pulled the empty BASE (None); the server has since gained a commit.
        assert_eq!(fence(None, Some("def"), false), FenceOutcome::FastForward);
    }

    #[test]
    fn diverged_when_tip_moved_and_dirty() {
        assert_eq!(
            fence(Some("abc"), Some("def"), true),
            FenceOutcome::Diverged
        );
    }

    #[test]
    fn diverged_from_empty_base_when_dirty() {
        assert_eq!(fence(None, Some("def"), true), FenceOutcome::Diverged);
    }

    #[test]
    fn fail_closed_when_base_some_but_tip_none() {
        // Clean or dirty, a vanished/absent server tip is never overwrite-safe.
        assert_eq!(fence(Some("abc"), None, false), FenceOutcome::FailClosed);
        assert_eq!(fence(Some("abc"), None, true), FenceOutcome::FailClosed);
    }

    // --- #193 ancestry recovery ---

    /// Nothing uploaded by this session.
    fn nothing() -> OwnUploads {
        OwnUploads::default()
    }

    /// The upload this session sent on `base` whose answer never came.
    fn unanswered(base: Option<&str>, message: &str) -> OwnUploads {
        OwnUploads {
            landed: Vec::new(),
            unanswered: vec![UnansweredUpload {
                base: base.map(str::to_owned),
                message: message.to_owned(),
            }],
        }
    }

    /// The revisions this session's uploads were answered with.
    fn landed(ids: &[&str]) -> OwnUploads {
        OwnUploads {
            landed: ids.iter().map(|id| (*id).to_owned()).collect(),
            unanswered: Vec::new(),
        }
    }

    /// [`commit`] carrying a description, as SpeleoDB stores it.
    fn described(id: &str, parents: &[&str], email: &str, message: &str) -> RevisionCommit {
        RevisionCommit {
            message: Some(message.to_owned()),
            ..commit(id, parents, email)
        }
    }

    #[test]
    fn ancestry_not_landed_when_tip_unchanged() {
        // Tip still at BASE (and the empty-repo None == None): nothing landed.
        assert_eq!(
            classify_ancestry(
                Some("b"),
                Some("b"),
                &[],
                ME,
                &nothing(),
                Some(T1),
                Some(T2)
            ),
            AncestryOutcome::NotLanded
        );
        assert_eq!(
            classify_ancestry(None, None, &[], ME, &nothing(), None, None),
            AncestryOutcome::NotLanded
        );
    }

    #[test]
    fn ancestry_landed_when_my_linear_commit_on_top() {
        // c (mine) sits directly on BASE b, where my unanswered upload went.
        // Edited before the push began → clear.
        let commits = [commit("c", &["b"], ME)];
        assert_eq!(
            classify_ancestry(
                Some("b"),
                Some("c"),
                &commits,
                ME,
                &unanswered(Some("b"), "m"),
                Some(T1),
                Some(T2)
            ),
            AncestryOutcome::Landed { clear_dirty: true }
        );
    }

    #[test]
    fn ancestry_landed_walks_multi_commit_chain() {
        // c -> b -> a(BASE): b is a revision one of my uploads was answered with,
        // c the one my unanswered upload made on top of it.
        let commits = [commit("c", &["b"], ME), commit("b", &["a"], ME)];
        let mut own = unanswered(Some("b"), "m");
        own.landed.push("b".to_owned());
        assert_eq!(
            classify_ancestry(Some("a"), Some("c"), &commits, ME, &own, Some(T1), Some(T1)),
            AncestryOutcome::Landed { clear_dirty: true }
        );
        // One unanswered upload makes one revision, not two.
        assert_eq!(
            classify_ancestry(
                Some("a"),
                Some("c"),
                &commits,
                ME,
                &unanswered(Some("a"), "m"),
                Some(T1),
                Some(T1)
            ),
            AncestryOutcome::Diverged
        );
    }

    #[test]
    fn ancestry_landed_from_empty_base_to_root() {
        // Empty-repo BASE (None): the chain ends at the root commit.
        let commits = [commit("c", &[], ME)];
        assert_eq!(
            classify_ancestry(
                None,
                Some("c"),
                &commits,
                ME,
                &unanswered(None, "m"),
                Some(T1),
                Some(T2)
            ),
            AncestryOutcome::Landed { clear_dirty: true }
        );
    }

    #[test]
    fn ancestry_landed_keeps_dirty_when_edited_after_push_start() {
        // edit -> push starts -> push lands -> confirmation lost -> edit AGAIN.
        // The landed commit predates the second edit, so the dirty bit must stay
        // set (post-push edits survive). last_edit_at (T3) > push_start_at (T2).
        let commits = [commit("c", &["b"], ME)];
        let own = unanswered(Some("b"), "m");
        assert_eq!(
            classify_ancestry(Some("b"), Some("c"), &commits, ME, &own, Some(T3), Some(T2)),
            AncestryOutcome::Landed { clear_dirty: false }
        );
    }

    #[test]
    fn ancestry_landed_keeps_dirty_on_unknown_timing() {
        // Legacy dirty bit (no last_edit_at) or missing push_start → conservative.
        let commits = [commit("c", &["b"], ME)];
        let own = unanswered(Some("b"), "m");
        assert_eq!(
            classify_ancestry(Some("b"), Some("c"), &commits, ME, &own, None, Some(T2)),
            AncestryOutcome::Landed { clear_dirty: false }
        );
        assert_eq!(
            classify_ancestry(Some("b"), Some("c"), &commits, ME, &own, Some(T1), None),
            AncestryOutcome::Landed { clear_dirty: false }
        );
    }

    #[test]
    fn ancestry_diverged_on_stranger_commit() {
        let commits = [commit("c", &["b"], OTHER)];
        assert_eq!(
            classify_ancestry(
                Some("b"),
                Some("c"),
                &commits,
                ME,
                &unanswered(Some("b"), "m"),
                Some(T1),
                Some(T2)
            ),
            AncestryOutcome::Diverged
        );
    }

    #[test]
    fn ancestry_diverged_on_the_accounts_own_email_without_an_upload_of_ours() {
        // R3, the fuzzer's same-account-overwrite: another session of OUR account
        // pushed c on our BASE. It carries our email, but this session uploaded
        // nothing that can be it - so it is somebody else's push, never adopted.
        // Injection: test authorship by `author_email` alone and this is Landed.
        let commits = [commit("c", &["b"], ME)];
        assert_eq!(
            classify_ancestry(
                Some("b"),
                Some("c"),
                &commits,
                ME,
                &nothing(),
                Some(T1),
                Some(T2)
            ),
            AncestryOutcome::Diverged
        );
        // A revision of ours elsewhere in the history is no licence either.
        assert_eq!(
            classify_ancestry(
                Some("b"),
                Some("c"),
                &commits,
                ME,
                &landed(&["b", "z"]),
                Some(T1),
                Some(T2)
            ),
            AncestryOutcome::Diverged
        );
    }

    #[test]
    fn ancestry_landed_for_a_revision_an_upload_of_ours_was_answered_with() {
        // The status race: the link was read before our push's answer advanced
        // BASE, the tip after it. The answer named the revision, whatever case
        // the SHA comes back in.
        let commits = [commit("c1", &["b"], ME)];
        assert_eq!(
            classify_ancestry(
                Some("b"),
                Some("c1"),
                &commits,
                ME,
                &landed(&["C1"]),
                None,
                None
            ),
            AncestryOutcome::Landed { clear_dirty: false }
        );
    }

    #[test]
    fn ancestry_an_unanswered_upload_matches_only_the_revision_it_could_have_made() {
        // Our upload went to BASE b with a description; its answer was lost. The
        // one revision on b, by our account, with that description is ours.
        let own = unanswered(Some("b"), "Surveyed the sump\n\nand the side passage");
        let ours = [described(
            "c",
            &["b"],
            ME,
            // As SpeleoDB stores it: line breaks folded into ". ".
            "Surveyed the sump. and the side passage",
        )];
        assert_eq!(
            classify_ancestry(Some("b"), Some("c"), &ours, ME, &own, Some(T1), Some(T2)),
            AncestryOutcome::Landed { clear_dirty: true }
        );
        // Another session's revision in the same place: its description is its
        // own. Injection: drop the description test and this is Landed.
        let theirs = [described("c", &["b"], ME, "Fixed the entrance")];
        assert_eq!(
            classify_ancestry(Some("b"), Some("c"), &theirs, ME, &own, Some(T1), Some(T2)),
            AncestryOutcome::Diverged
        );
        // Not on the upload's BASE: not what that upload made.
        let elsewhere = [
            described("c", &["x"], ME, "Surveyed the sump. and the side passage"),
            commit("x", &["b"], ME),
        ];
        assert_eq!(
            classify_ancestry(
                Some("b"),
                Some("c"),
                &elsewhere,
                ME,
                &own,
                Some(T1),
                Some(T2)
            ),
            AncestryOutcome::Diverged
        );
        // A history without descriptions (an older server) matches on the BASE
        // and the account alone.
        let bare = [commit("c", &["b"], ME)];
        assert_eq!(
            classify_ancestry(Some("b"), Some("c"), &bare, ME, &own, Some(T1), Some(T2)),
            AncestryOutcome::Landed { clear_dirty: true }
        );
    }

    #[test]
    fn an_undescribed_revision_is_ours_only_where_the_server_keeps_no_descriptions() {
        // Pre-PR review of the X3 round, should-fix 4. Our upload always carries a
        // description (the upload verbs put in a default for a blank one), and a
        // server that stores descriptions shows it. So on such a server a revision
        // with no description on our BASE, by our account, is not what our
        // unanswered upload made: another session's or another tool's revision.
        // Taking it for ours used to adopt it as our landed push and clear the
        // dirty mark over work that never reached SpeleoDB.
        // Injection: let an undescribed revision match whatever the history keeps
        // (the rule before) and the first case is Landed.
        let own = unanswered(Some("b"), "Surveyed the sump");
        let mixed = [
            commit("c", &["b"], ME),
            described("b", &["a"], ME, "First survey"),
            described("a", &[], ME, "Created"),
        ];
        assert_eq!(
            classify_ancestry(Some("b"), Some("c"), &mixed, ME, &own, Some(T1), Some(T2)),
            AncestryOutcome::Diverged,
            "an undescribed revision where the server keeps descriptions is not ours"
        );
        // And a late landing of it is reported as replaced, not hidden as ours.
        let after = [
            commit("u", &["c"], ME),
            commit("c", &["b"], ME),
            described("b", &["a"], ME, "First survey"),
            described("a", &[], ME, "Created"),
        ];
        let replaced = replaced_by_upload(Some("b"), "u", &after);
        assert_eq!(ids(&replaced), ["c"]);
        assert_eq!(
            ids(&not_our_own(
                replaced,
                &own.unanswered,
                ME,
                history_records_descriptions(&after)
            )),
            ["c"]
        );

        // The residue, pinned: a server that keeps no descriptions at all (an
        // older one) cannot tell our upload from another session's on our BASE,
        // so the BASE and the account decide, as documented.
        let bare = [commit("c", &["b"], ME), commit("b", &[], ME)];
        assert!(!history_records_descriptions(&bare));
        assert_eq!(
            classify_ancestry(Some("b"), Some("c"), &bare, ME, &own, Some(T1), Some(T2)),
            AncestryOutcome::Landed { clear_dirty: true }
        );
    }

    #[test]
    fn ancestry_diverged_on_empty_my_email() {
        // #197 review: an absent session email (unwrap_or_default → "") must NOT let
        // a server-omitted author (serde default → "") match as "ours" and get
        // adopted+overwritten. With my_email empty we cannot identify our own commit,
        // so the gap is foreign → divergence prompt, never a silent adopt.
        let commits = [commit("c", &["b"], "")];
        let own = unanswered(Some("b"), "m");
        assert_eq!(
            classify_ancestry(Some("b"), Some("c"), &commits, "", &own, Some(T1), Some(T2)),
            AncestryOutcome::Diverged,
            "empty my_email must fail closed, not classify a stranger's commit as ours"
        );
        // Even when our email IS known, a "" author is still a stranger.
        let anon = [commit("c", &["b"], "")];
        assert_eq!(
            classify_ancestry(Some("b"), Some("c"), &anon, ME, &own, Some(T1), Some(T2)),
            AncestryOutcome::Diverged
        );
    }

    #[test]
    fn ancestry_landed_matches_author_case_insensitively() {
        // #197 review: the server may normalise email case. Our own landed commit
        // authored as "Me@Example.COM" must still classify as ours (Landed), not a
        // stranger.
        let commits = [commit("c", &["b"], "Me@Example.COM")];
        assert_eq!(
            classify_ancestry(
                Some("b"),
                Some("c"),
                &commits,
                ME,
                &unanswered(Some("B"), "m"),
                Some(T1),
                Some(T2)
            ),
            AncestryOutcome::Landed { clear_dirty: true },
            "a case-variant of our own email must not read as a stranger"
        );
    }

    #[test]
    fn ancestry_diverged_when_base_not_an_ancestor() {
        // We hold BASE z, but the tip's full history (c -> root) never contains z.
        let commits = [commit("c", &[], ME)];
        assert_eq!(
            classify_ancestry(
                Some("z"),
                Some("c"),
                &commits,
                ME,
                &unanswered(Some("z"), "m"),
                Some(T1),
                Some(T2)
            ),
            AncestryOutcome::Diverged
        );
    }

    #[test]
    fn ancestry_fail_closed_on_merge() {
        // A commit with two parents is non-linear.
        let commits = [commit("c", &["b", "x"], ME)];
        assert_eq!(
            classify_ancestry(
                Some("b"),
                Some("c"),
                &commits,
                ME,
                &nothing(),
                Some(T1),
                Some(T2)
            ),
            AncestryOutcome::FailClosed
        );
    }

    #[test]
    fn ancestry_fail_closed_on_missing_parent() {
        // c's parent d is absent from the fetched history (and isn't BASE).
        let commits = [commit("c", &["d"], ME)];
        assert_eq!(
            classify_ancestry(
                Some("b"),
                Some("c"),
                &commits,
                ME,
                &nothing(),
                Some(T1),
                Some(T2)
            ),
            AncestryOutcome::FailClosed
        );
    }

    #[test]
    fn ancestry_fail_closed_when_tip_absent_from_history() {
        // Tip x isn't in the commit list at all.
        assert_eq!(
            classify_ancestry(
                Some("b"),
                Some("x"),
                &[],
                ME,
                &nothing(),
                Some(T1),
                Some(T2)
            ),
            AncestryOutcome::FailClosed
        );
    }

    #[test]
    fn ancestry_fail_closed_when_base_some_tip_none() {
        assert_eq!(
            classify_ancestry(Some("b"), None, &[], ME, &nothing(), Some(T1), Some(T2)),
            AncestryOutcome::FailClosed
        );
    }

    // ---- reconcile_verdict (Start editing / Update routing, #193 b2) ----
    // A linear chain root -> a -> b authored by ME, used by the routing tests.
    fn linear_history() -> Vec<RevisionCommit> {
        vec![
            commit("root", &[], ME),
            commit("a", &["root"], ME),
            commit("b", &["a"], ME),
        ]
    }

    #[test]
    fn reconcile_up_to_date_when_base_equals_tip() {
        assert_eq!(
            reconcile_verdict(Some("b"), Some("b"), false, &linear_history()),
            ReconcileVerdict::UpToDate
        );
        // None == None (empty repo) is up to date too.
        assert_eq!(
            reconcile_verdict(None, None, false, &[]),
            ReconcileVerdict::UpToDate
        );
    }

    #[test]
    fn reconcile_fast_forward_only_for_clean_genuine_descendant() {
        // Clean, tip is a real descendant of BASE -> fast-forward.
        assert_eq!(
            reconcile_verdict(Some("a"), Some("b"), false, &linear_history()),
            ReconcileVerdict::FastForward
        );
        // Dirty over a moved tip is a divergence, never a fast-forward.
        assert_eq!(
            reconcile_verdict(Some("a"), Some("b"), true, &linear_history()),
            ReconcileVerdict::Diverged
        );
    }

    #[test]
    fn reconcile_diverges_when_tip_not_a_descendant_of_base() {
        // Tip moved but BASE is NOT an ancestor (stale latest_commit / fork): the
        // ancestor guard refuses the fast-forward and we route to the prompt.
        assert_eq!(
            reconcile_verdict(Some("b"), Some("a"), false, &linear_history()),
            ReconcileVerdict::Diverged
        );
    }

    #[test]
    fn reconcile_non_linear_history_routes_to_divergence_not_error() {
        // A merge commit (two parents) is non-linear -> the ancestor guard fails
        // -> divergence prompt (#193 b2), NOT a hard error.
        let merge = vec![
            commit("root", &[], ME),
            commit("a", &["root"], ME),
            commit("m", &["a", "root"], ME), // merge: two parents
        ];
        assert_eq!(
            reconcile_verdict(Some("a"), Some("m"), false, &merge),
            ReconcileVerdict::Diverged
        );
    }

    #[test]
    fn reconcile_empty_tip_with_recorded_base_routes_to_divergence() {
        // FailClosed (recorded BASE, server reports no tip - incl. the multi-root
        // case where tip_of() yields None) folds into the divergence prompt.
        assert_eq!(
            reconcile_verdict(Some("b"), None, false, &[]),
            ReconcileVerdict::Diverged
        );
    }

    // ---- plan_push (Done push routing, #193 b2 + b5) ----

    #[test]
    fn plan_push_uploads_directly_when_up_to_date() {
        assert_eq!(
            plan_push(
                Some("b"),
                Some("b"),
                &linear_history(),
                ME,
                &nothing(),
                Some(T1),
                Some(T2)
            ),
            PushPlan::Upload
        );
    }

    #[test]
    fn plan_push_adopts_and_confirms_when_my_push_landed_clean() {
        // My commit b landed on top of BASE a (my unanswered upload went there),
        // no edit after push-start -> adopt + confirm without a redundant re-push.
        assert_eq!(
            plan_push(
                Some("a"),
                Some("b"),
                &linear_history(),
                ME,
                &unanswered(Some("a"), "m"),
                Some(T1),
                Some(T2)
            ),
            PushPlan::AdoptConfirmed
        );
    }

    #[test]
    fn plan_push_adopts_then_reuploads_when_edited_after_push_start() {
        // My push landed (b) but I edited again after push-start (T3 > T2) -> adopt
        // the landed tip, then re-push the later edit on top.
        assert_eq!(
            plan_push(
                Some("a"),
                Some("b"),
                &linear_history(),
                ME,
                &unanswered(Some("a"), "m"),
                Some(T3),
                Some(T2)
            ),
            PushPlan::AdoptThenUpload
        );
    }

    #[test]
    fn plan_push_diverges_on_a_same_account_revision_it_did_not_upload() {
        // R3: b sits on our BASE a, authored by our account, but this session
        // uploaded nothing that can be it (another machine of ours pushed it, under
        // the same per-account lock). Never adopt it and upload over it.
        // Injection: test authorship by `author_email` alone and this is
        // AdoptThenUpload, the overwrite.
        assert_eq!(
            plan_push(
                Some("a"),
                Some("b"),
                &linear_history(),
                ME,
                &nothing(),
                Some(T3),
                Some(T2)
            ),
            PushPlan::Diverged
        );
        // An unanswered upload of ours on another BASE does not make it ours.
        assert_eq!(
            plan_push(
                Some("a"),
                Some("b"),
                &linear_history(),
                ME,
                &unanswered(Some("root"), "m"),
                Some(T3),
                Some(T2)
            ),
            PushPlan::Diverged
        );
    }

    #[test]
    fn plan_push_diverges_on_a_strangers_commit() {
        let stolen = vec![
            commit("root", &[], ME),
            commit("a", &["root"], ME),
            commit("b", &["a"], OTHER), // a stranger pushed into the gap (lock theft)
        ];
        assert_eq!(
            plan_push(
                Some("a"),
                Some("b"),
                &stolen,
                ME,
                &unanswered(Some("a"), "m"),
                Some(T1),
                Some(T2)
            ),
            PushPlan::Diverged
        );
    }

    #[test]
    fn plan_push_diverges_on_non_linear_gap_instead_of_error() {
        // A merge in the BASE..tip gap is an inconsistency -> divergence prompt
        // (#193 b2), not a hard error and not a blind re-push.
        let merge = vec![
            commit("root", &[], ME),
            commit("a", &["root"], ME),
            commit("m", &["a", "root"], ME),
        ];
        assert_eq!(
            plan_push(
                Some("a"),
                Some("m"),
                &merge,
                ME,
                &nothing(),
                Some(T1),
                Some(T2)
            ),
            PushPlan::Diverged
        );
    }

    #[test]
    fn plan_push_diverges_when_base_some_but_tip_none() {
        assert_eq!(
            plan_push(Some("a"), None, &[], ME, &nothing(), Some(T1), Some(T2)),
            PushPlan::Diverged
        );
    }

    // ---- status_verdict: the passive status-button verdict ----

    #[test]
    fn status_verdict_reports_our_own_landed_push_as_up_to_date_not_a_conflict() {
        // The self-conflict. A status refresh read the link row before a push and
        // resolved the tip after it, so it fences a BASE one commit behind - and
        // that commit is OUR OWN. A bare fence calls this Diverged; reporting it as
        // a conflict paints the button red against the user's own work and sends
        // their next click to the divergence dialog. Ours: the push's answer named
        // it, or (its answer not in yet) it is what the upload in flight made.
        let commits = vec![commit("c1", &["c0"], ME), commit("c0", &[], ME)];
        for own in [landed(&["c1"]), unanswered(Some("c0"), "m")] {
            assert_eq!(
                status_verdict(Some("c0"), Some("c1"), true, &commits, ME, &own, true),
                RemoteState::UpToDate
            );
        }
    }

    #[test]
    fn status_verdict_reports_a_same_account_revision_as_a_conflict_under_our_lock() {
        // R3: both sessions of the account hold the lock (it is the account's), so
        // holding it does not make the other session's revision ours. This
        // session uploaded nothing that can be c1: the mark shows the conflict
        // that Save Changes will report, instead of the resting state over it.
        let commits = vec![commit("c1", &["c0"], ME), commit("c0", &[], ME)];
        assert_eq!(
            status_verdict(Some("c0"), Some("c1"), true, &commits, ME, &nothing(), true),
            RemoteState::Conflict
        );
    }

    #[test]
    fn status_verdict_reports_a_same_account_second_session_as_a_conflict() {
        // A second session under the same account can author the gap commit. It
        // carries OUR email but was pushed by our other session, so the authorship
        // proxy - "same email means this session pushed it" - is exactly wrong here.
        // We are not the lock holder, which is what says so: our own unrecorded push
        // could only have been made while we held the single writer lock.
        //
        // Getting this wrong is silent data loss, not a bad badge: the mark keeps
        // its resting state, the user edits on and presses Done, and `plan_push`
        // classifies the same gap as `Landed` and supersedes the other session's
        // revision with no prompt at any point.
        let commits = vec![commit("c1", &["c0"], ME), commit("c0", &[], ME)];
        assert_eq!(
            status_verdict(
                Some("c0"),
                Some("c1"),
                true,
                &commits,
                ME,
                &landed(&["c1"]),
                false
            ),
            RemoteState::Conflict
        );
    }

    #[test]
    fn status_verdict_agrees_with_reconcile_verdict_wherever_the_mark_is_clickable() {
        // The passive mark and the verb behind it must not disagree. `status_verdict`
        // has authorship recovery and `reconcile_verdict` (which drives Start
        // editing) has none, so the recovery is gated on holding the lock -
        // reachable only when Start editing is NOT the click action. Pin the
        // agreement: with the lock not ours, both say the same thing.
        let commits = vec![commit("c1", &["c0"], ME), commit("c0", &[], ME)];
        assert_eq!(
            status_verdict(
                Some("c0"),
                Some("c1"),
                true,
                &commits,
                ME,
                &nothing(),
                false
            ),
            RemoteState::Conflict
        );
        assert_eq!(
            reconcile_verdict(Some("c0"), Some("c1"), true, &commits),
            ReconcileVerdict::Diverged,
            "a calm, unbadged mark whose click raises the divergence prompt is the \
             same passive/active split the recovery was added to remove"
        );
    }

    #[test]
    fn status_verdict_still_reports_a_strangers_commit_as_a_conflict() {
        // The recovery above must not swallow a real divergence: someone else's
        // commit sits between BASE and the tip.
        let commits = vec![commit("c1", &["c0"], OTHER), commit("c0", &[], ME)];
        let own = unanswered(Some("c0"), "m");
        assert_eq!(
            status_verdict(Some("c0"), Some("c1"), true, &commits, ME, &own, true),
            RemoteState::Conflict
        );
    }

    #[test]
    fn status_verdict_reports_a_conflict_when_we_cannot_identify_our_own_commits() {
        // No session email -> `classify_ancestry` can match nothing as ours, so it
        // fails closed. The passive button says the same thing Done would.
        let commits = vec![commit("c1", &["c0"], ME), commit("c0", &[], ME)];
        assert_eq!(
            status_verdict(
                Some("c0"),
                Some("c1"),
                true,
                &commits,
                "",
                &unanswered(Some("c0"), "m"),
                true
            ),
            RemoteState::Conflict
        );
    }

    #[test]
    fn status_verdict_maps_the_clean_and_anomalous_arms_unchanged() {
        let commits = vec![commit("c1", &["c0"], ME), commit("c0", &[], ME)];
        // base == tip: nothing to reconcile.
        assert_eq!(
            status_verdict(Some("c1"), Some("c1"), true, &commits, ME, &nothing(), true),
            RemoteState::UpToDate
        );
        // Clean copy, tip genuinely descends from BASE: a real pull is available.
        assert_eq!(
            status_verdict(
                Some("c0"),
                Some("c1"),
                false,
                &commits,
                ME,
                &nothing(),
                false
            ),
            RemoteState::RemoteAhead
        );
        // Clean copy but BASE is NOT an ancestor (a server-side history rewrite):
        // never advertise a pull the Update verb would immediately fail closed on.
        let forked = vec![commit("x1", &["x0"], ME), commit("x0", &[], ME)];
        assert_eq!(
            status_verdict(
                Some("c0"),
                Some("x1"),
                false,
                &forked,
                ME,
                &nothing(),
                false
            ),
            RemoteState::Conflict
        );
        // A recorded BASE with no server tip is a transient empty response, not a
        // conflict - nothing local is at risk of being clobbered.
        assert_eq!(
            status_verdict(Some("c0"), None, true, &[], ME, &nothing(), true),
            RemoteState::Unknown
        );
    }

    // ---- replaced_by_upload: what a landed upload took HEAD from (R3) ----

    fn ids(commits: &[RevisionCommit]) -> Vec<&str> {
        commits.iter().map(|c| c.id.as_str()).collect()
    }

    #[test]
    fn ancestry_every_unanswered_upload_is_kept_until_settled() {
        // F4R3-locks-push-5: U1's answer was lost before the server landed it, so
        // the retry U2 was sent on the same BASE with the same description, and
        // both are unanswered. The server then landed U1 late: it is ours.
        let mut own = unanswered(Some("b"), "From B");
        own.unanswered.push(UnansweredUpload {
            base: Some("b".to_owned()),
            message: "From B".to_owned(),
        });
        let late = [described("u1", &["b"], ME, "From B"), commit("b", &[], ME)];
        assert_eq!(
            classify_ancestry(Some("b"), Some("u1"), &late, ME, &own, Some(T1), Some(T2)),
            AncestryOutcome::Landed { clear_dirty: true }
        );
        // Each unanswered upload accounts for one revision at most.
        let two = [
            described("u2", &["u1"], ME, "From B"),
            described("u1", &["b"], ME, "From B"),
            commit("b", &[], ME),
        ];
        assert_eq!(
            classify_ancestry(Some("b"), Some("u2"), &two, ME, &own, Some(T1), Some(T2)),
            AncestryOutcome::Diverged
        );
    }

    #[test]
    fn our_own_late_upload_is_not_reported_as_replaced() {
        // F4R3-locks-push-5: U1 (unanswered) landed late, under the retry U2. The
        // history after U2's 200 shows U1 between U2's BASE and U2, which is our
        // own earlier snapshot, not another session's work. Another session's
        // revision in that place is still reported.
        // Injection: report `replaced_by_upload` as it is and U1 is named.
        let own = unanswered(Some("b0"), "From B");
        let history = [
            commit("u2", &["u1"], ME),
            described("u1", &["b0"], ME, "From B"),
            commit("b0", &[], ME),
        ];
        let replaced = replaced_by_upload(Some("b0"), "u2", &history);
        assert_eq!(ids(&replaced), ["u1"]);
        assert!(not_our_own(
            replaced,
            &own.unanswered,
            ME,
            history_records_descriptions(&history)
        )
        .is_empty());
        let theirs = [
            commit("u2", &["m1"], ME),
            described("m1", &["b0"], ME, "From A"),
            commit("b0", &[], ME),
        ];
        let replaced = replaced_by_upload(Some("b0"), "u2", &theirs);
        assert_eq!(
            ids(&not_our_own(
                replaced,
                &own.unanswered,
                ME,
                history_records_descriptions(&theirs)
            )),
            ["m1"]
        );
    }

    #[test]
    fn replaced_by_upload_is_empty_when_ours_sits_on_its_base() {
        let history = [commit("ours", &["b0"], ME), commit("b0", &[], ME)];
        assert!(replaced_by_upload(Some("b0"), "ours", &history).is_empty());
        // Whatever case the SHAs come back in.
        assert!(replaced_by_upload(Some("B0"), "OURS", &history).is_empty());
    }

    #[test]
    fn replaced_by_upload_names_what_landed_between_the_check_and_the_upload() {
        // Another session of the account landed m1 then m2 on our BASE b0 while
        // our upload was on its way; the server put ours on top of m2.
        // Injection: answer only the direct parent and m1 is missing.
        let history = [
            commit("ours", &["m2"], ME),
            described("m2", &["m1"], ME, "Second from the other machine"),
            described("m1", &["b0"], ME, "First from the other machine"),
            commit("b0", &[], ME),
        ];
        let replaced = replaced_by_upload(Some("b0"), "ours", &history);
        assert_eq!(ids(&replaced), ["m2", "m1"], "newest first");
        assert_eq!(
            replaced[0].message.as_deref(),
            Some("Second from the other machine")
        );
    }

    #[test]
    fn replaced_by_upload_cannot_tell_without_our_revision_in_the_history() {
        let history = [commit("m1", &["b0"], ME), commit("b0", &[], ME)];
        assert!(replaced_by_upload(Some("b0"), "ours", &history).is_empty());
    }

    #[test]
    fn replaced_by_upload_from_an_empty_base_never_names_the_creation_commit() {
        // A just-created project: BASE None, the server's own creation commit at
        // the root. Only a real revision under ours is reported.
        let alone = [commit("ours", &["root"], ME), creation_commit("root")];
        assert!(replaced_by_upload(None, "ours", &alone).is_empty());
        let raced = [
            commit("ours", &["m1"], ME),
            commit("m1", &["root"], ME),
            creation_commit("root"),
        ];
        assert_eq!(ids(&replaced_by_upload(None, "ours", &raced)), ["m1"]);
    }

    #[test]
    fn replaced_by_upload_off_the_bases_chain_names_only_the_revision_under_ours() {
        // Our BASE b0 is not below ours at all (a history rewritten on the
        // server): name the one ours took HEAD from, not everything to the root.
        let history = [
            commit("ours", &["x2"], ME),
            commit("x2", &["x1"], OTHER),
            commit("x1", &[], OTHER),
        ];
        assert_eq!(
            ids(&replaced_by_upload(Some("b0"), "ours", &history)),
            ["x2"]
        );
    }

    // ---- display_history: what the revision list shows ----

    /// The shape of a real project: SpeleoDB's creation commit, the publish that
    /// followed it inside the same second, then ordinary edits. The server returns
    /// these newest-first by `authored_date`, and the tie between the first two
    /// breaks arbitrarily - here, the wrong way round, exactly as observed live.
    fn creation_then_publish() -> Vec<RevisionCommit> {
        vec![
            commit("8899f253", &["09f326b7"], ME),
            commit("09f326b7", &["a14d46cf"], ME),
            // The tie: sorted above the publish it actually precedes.
            creation_commit("e2ad104b"),
            commit("a14d46cf", &["e2ad104b"], ME),
        ]
    }

    #[test]
    fn display_history_hides_the_project_creation_commit() {
        // It is not a revision of the survey - it predates any data, and it is
        // authored by the service account, not by anyone who has been in the cave.
        // Offering it in a list whose actions are View and Revert invites someone to
        // roll their project back to an empty one.
        let shown = display_history(creation_then_publish());
        assert!(
            !shown.iter().any(|c| c.id == "e2ad104b"),
            "the automated creation commit must not be listed"
        );
        assert_eq!(shown.len(), 3);
    }

    #[test]
    fn display_history_orders_by_ancestry_not_by_a_tied_timestamp() {
        // The bug this fixes: creation and the first publish share a timestamp, so
        // the server's `-authored_date` order put the root ABOVE the publish that
        // descends from it. The parent chain has no tie to break.
        let shown = display_history(creation_then_publish());
        let ids: Vec<&str> = shown.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["8899f253", "09f326b7", "a14d46cf"]);
    }

    #[test]
    fn display_history_falls_back_rather_than_hiding_revisions() {
        // A history we cannot walk (here a fork: two leaves, so no single tip) must
        // still list every real revision. Showing them in a debatable order is a
        // small cost; silently dropping someone's work from their own history is not.
        let forked = vec![
            commit("b1", &["root"], ME),
            commit("b2", &["root"], ME),
            creation_commit("root"),
        ];
        let shown = display_history(forked);
        let ids: Vec<&str> = shown.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["b1", "b2"],
            "both branches survive; only the root goes"
        );
    }

    #[test]
    fn display_history_keeps_a_genuine_first_revision_at_the_root() {
        // Not every project has a separate creation commit ahead of its first
        // upload. Dropping the root STRUCTURALLY hid a real survey revision on any
        // that does not: it could not be viewed and could not be reverted to, with
        // nothing on screen saying anything had been withheld.
        let history = vec![
            commit("r2", &["r1"], ME),
            RevisionCommit {
                message: Some("first survey upload".to_owned()),
                ..commit("r1", &[], ME)
            },
        ];
        let ids: Vec<String> = display_history(history).into_iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            vec!["r2", "r1"],
            "a parentless commit that is not SpeleoDB's creation marker is a real revision"
        );
    }

    #[test]
    fn display_history_leaves_nothing_for_a_project_that_was_only_created() {
        // A project created but never pushed to has no survey revisions at all, and
        // saying so is more honest than listing a commit that holds nothing.
        let shown = display_history(vec![creation_commit("root")]);
        assert!(shown.is_empty());
        assert!(display_history(vec![]).is_empty());
    }

    // ---- edits_captured_by_push (direct, #197 dirty-clear timing) ----

    #[test]
    fn edits_captured_only_when_last_edit_predates_push_start() {
        assert!(
            edits_captured_by_push(Some(T1), Some(T2)),
            "edited before push"
        );
        assert!(
            edits_captured_by_push(Some(T2), Some(T2)),
            "edited at push start"
        );
        assert!(
            !edits_captured_by_push(Some(T3), Some(T2)),
            "edited after push -> keep dirty"
        );
        assert!(
            !edits_captured_by_push(None, Some(T2)),
            "unknown edit time -> keep dirty"
        );
        assert!(
            !edits_captured_by_push(Some("not-a-date"), Some(T2)),
            "unparseable -> keep dirty"
        );
    }
    // --- [#303] unwind_target (spec items 19-22) ---

    /// Spec item 19: the FIRST unwind (no chain pointer) targets BASE itself, flagged
    /// `is_current`. The working copy already equals that commit, so nothing moves - what
    /// comes back is the history that produced it, which clear-on-push had removed
    /// locally. Loading the PARENT here instead would make a user's own just-pushed
    /// operations un-steppable.
    #[test]
    fn unwind_first_step_targets_base_itself() {
        let commits = vec![
            creation_commit("root"),
            commit("c1", &["root"], ME),
            commit("c2", &["c1"], ME),
        ];
        let (target, is_current) =
            unwind_target(None, Some("c2"), &commits).expect("BASE is reachable");
        assert_eq!(target, "c2");
        assert!(
            is_current,
            "the first step reloads the commit the copy already equals"
        );
    }

    /// Spec item 20: mid-unwind, the next step is the first parent of the revision we are
    /// sitting on - not of BASE, which a restore has stamped at the server tip.
    #[test]
    fn unwind_walks_first_parents_once_mid_unwind() {
        let commits = vec![
            creation_commit("root"),
            commit("c1", &["root"], ME),
            commit("c2", &["c1"], ME),
            commit("c3", &["c2"], ME),
        ];
        // Sitting on c3 (BASE is the tip after a restore, and is deliberately ignored).
        let (target, is_current) =
            unwind_target(Some("c3"), Some("c3"), &commits).expect("c2 is reachable");
        assert_eq!(target, "c2");
        assert!(!is_current, "this one moves the working copy back");

        // ...and again from c2.
        let (target, _) = unwind_target(Some("c2"), Some("c3"), &commits).expect("c1 reachable");
        assert_eq!(target, "c1");
    }

    /// A merge takes `parent_ids[0]` - the git-mainline convention, and the only choice
    /// that keeps "keep pressing undo" deterministic.
    #[test]
    fn unwind_takes_the_first_parent_of_a_merge() {
        let commits = vec![
            creation_commit("root"),
            commit("mainline", &["root"], ME),
            commit("side", &["root"], OTHER),
            commit("merge", &["mainline", "side"], ME),
        ];
        let (target, _) = unwind_target(Some("merge"), Some("merge"), &commits).expect("mainline");
        assert_eq!(target, "mainline");
    }

    /// Spec item 21: the chain ENDS rather than offering a step that cannot be taken -
    /// at the root, on an empty repo, and at SpeleoDB's own creation commit (which
    /// predates any survey data, so "unwind to nothing" is not a step anyone means).
    #[test]
    fn unwind_has_no_target_at_the_root_or_on_an_empty_repo() {
        let commits = vec![creation_commit("root"), commit("c1", &["root"], ME)];

        assert!(
            unwind_target(None, None, &commits).is_none(),
            "an empty-repo BASE has nothing to reach"
        );
        assert!(
            unwind_target(Some("c1"), Some("c1"), &commits).is_none(),
            "c1's only parent is the creation commit, so the chain ends at c1"
        );
        assert!(
            unwind_target(None, Some("root"), &commits).is_none(),
            "and the creation commit is never a target itself"
        );

        // A genuine parentless survey revision (no creation commit ahead of it) is a
        // valid target, but the chain still ends there.
        let real_root = vec![commit("first", &[], ME), commit("second", &["first"], ME)];
        let (target, _) =
            unwind_target(Some("second"), Some("second"), &real_root).expect("first is real");
        assert_eq!(target, "first");
        assert!(
            unwind_target(Some("first"), Some("second"), &real_root).is_none(),
            "a real root has no parent, so the chain ends"
        );
    }

    /// A pointer naming a commit outside the fetched history cannot be walked from, and
    /// must fail closed (no target) rather than guessing.
    #[test]
    fn unwind_fails_closed_on_an_unknown_pointer() {
        let commits = vec![creation_commit("root"), commit("c1", &["root"], ME)];
        assert!(unwind_target(Some("deadbeef"), Some("c1"), &commits).is_none());
        assert!(unwind_target(None, Some("deadbeef"), &commits).is_none());
    }

    // --- [#303] rewind_target: the same walk, forward ---

    /// Mid-walk, the forward step is the child of where we stand ON THE PATH BACK TO BASE,
    /// and it retraces exactly the commits the backward walk visited.
    #[test]
    fn rewind_steps_back_up_the_chain_it_came_down() {
        let commits = vec![
            creation_commit("root"),
            commit("c1", &["root"], ME),
            commit("c2", &["c1"], ME),
            commit("c3", &["c2"], ME),
        ];
        // Standing on c1 (two unwinds down from BASE c3), forward goes c1 -> c2 -> c3.
        let (target, is_latest) =
            rewind_target(Some("c1"), Some("c3"), &commits).expect("c2 is the next step up");
        assert_eq!(target, "c2");
        assert!(!is_latest, "c2 is not BASE, so this is not the last step");

        let (target, is_latest) =
            rewind_target(Some("c2"), Some("c3"), &commits).expect("c3 is reachable");
        assert_eq!(target, "c3");
        assert!(
            is_latest,
            "landing on BASE is the step that returns the copy to the latest version"
        );
    }

    /// The two ends of the walk, where there is nothing ahead.
    #[test]
    fn rewind_has_no_target_at_base_or_off_the_chain() {
        let commits = vec![
            creation_commit("root"),
            commit("c1", &["root"], ME),
            commit("c2", &["c1"], ME),
        ];
        assert!(
            rewind_target(None, Some("c2"), &commits).is_none(),
            "not mid-walk: the copy is already at BASE, so there is nothing ahead of it - \
             this is also what makes an edit (which drops the pointer) disable the direction"
        );
        assert!(
            rewind_target(Some("c2"), Some("c2"), &commits).is_none(),
            "already home"
        );
        assert!(
            rewind_target(Some("deadbeef"), Some("c2"), &commits).is_none(),
            "a pointer outside the fetched range walks off the end and fails closed"
        );
        assert!(
            rewind_target(Some("c1"), None, &commits).is_none(),
            "no BASE means no path to walk back along"
        );
    }

    /// The child is taken from BASE's own first-parent chain, never by scanning for any
    /// commit that names `current` as a parent. An abandoned line can share our parent, and
    /// `authored_date` cannot break that tie - so a scan could hand back a commit that is
    /// not on our path and walk the copy onto someone else's history.
    #[test]
    fn rewind_follows_our_own_path_when_a_sibling_shares_the_parent() {
        let commits = vec![
            creation_commit("root"),
            commit("c1", &["root"], ME),
            // Both descend from c1; only `mine` is on the path from BASE.
            commit("abandoned", &["c1"], ME),
            commit("mine", &["c1"], ME),
            commit("base", &["mine"], ME),
        ];
        let (target, _) = rewind_target(Some("c1"), Some("base"), &commits).expect("our own child");
        assert_eq!(
            target, "mine",
            "the step must follow BASE's ancestry, not whichever sibling was listed first"
        );
    }

    /// Merges take the mainline parent, matching `unwind_target`'s descent.
    #[test]
    fn rewind_takes_the_mainline_parent_across_a_merge() {
        let commits = vec![
            creation_commit("root"),
            commit("c1", &["root"], ME),
            commit("side", &["root"], ME),
            commit("merge", &["c1", "side"], ME),
        ];
        let (target, is_latest) =
            rewind_target(Some("c1"), Some("merge"), &commits).expect("mainline child");
        assert_eq!(target, "merge");
        assert!(is_latest);
        assert!(
            rewind_target(Some("side"), Some("merge"), &commits).is_none(),
            "the second parent is off the mainline, so it is not a step on our path"
        );
    }

    /// A first-parent CYCLE must not wedge the forward walk.
    ///
    /// Git cannot author `A -> B -> A`, so this is robustness against a malformed or
    /// hostile response rather than a shape any real history takes. It matters because
    /// of HOW an unbounded walk fails: every lookup succeeds and the exit condition
    /// never fires, so it does not return a wrong answer, it never returns at all -
    /// a wedged Unwind with no panic and nothing in the log.
    ///
    /// Run on a worker with a deadline for that exact reason. Asserted in-line, a
    /// regression here would hang the test run instead of failing it, which is the
    /// difference between a red build and a mysteriously stuck one.
    #[test]
    fn rewind_terminates_on_a_cyclic_first_parent_chain() {
        let commits = vec![commit("aaa", &["bbb"], ME), commit("bbb", &["aaa"], ME)];
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // A pointer on neither commit: the walk can never match and must run out
            // of budget rather than loop forever.
            let _ = tx.send(rewind_target(Some("zzz"), Some("aaa"), &commits));
        });
        // Asserted rather than matched with a `panic!`: the module denies `panic` in
        // favour of assertions, and a failed assert reports the same thing.
        let answer = rx.recv_timeout(std::time::Duration::from_secs(10));
        assert!(
            answer.is_ok(),
            "rewind_target did not terminate on a cyclic first-parent chain - the step \
             budget is gone"
        );
        assert!(
            answer.ok().flatten().is_none(),
            "a cyclic history offers no reachable forward step"
        );
    }
}
