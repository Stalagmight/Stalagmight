//! What a fuzz run is made of: the steps (an app verb or a second-client action,
//! optionally with a fault armed on the proxy), the seeded generator that picks
//! them, and the deterministic RNG it draws from.
//!
//! A trace is the list of steps as EXECUTED, so a replay or a shrink re-runs the
//! same list against a fresh world. Parameters that depend on the world (project
//! ids, commit shas) are recorded as indices into the world, never as values.

use serde::{Deserialize, Serialize};

use super::world::Who;

/// SplitMix64: small, fast, and identical on every platform, so a seed names the
/// same run everywhere.
#[derive(Clone)]
pub(super) struct Rng(u64);

impl Rng {
    pub(super) fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e37_79b9_7f4a_7c15)
    }

    pub(super) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (`0` when `n == 0`).
    pub(super) fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }

    pub(super) fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    /// Pick from `(weight, item)` pairs; `None` when every weight is zero.
    pub(super) fn weighted<T: Clone>(&mut self, items: &[(u32, T)]) -> Option<T> {
        let total: u64 = items.iter().map(|(w, _)| u64::from(*w)).sum();
        let mut roll = self.below(total);
        for (w, item) in items {
            let w = u64::from(*w);
            if roll < w {
                return Some(item.clone());
            }
            roll -= w;
        }
        None
    }
}

/// What the user picks when an open is refused over un-pushed local edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum OnUnpushed {
    KeepLocal,
    TakeServer,
}

/// How a close (or quit) with a lock / un-pushed edits is resolved in its dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum LeaveMode {
    /// "Push, release & close" (`speleodb_done`, then close releasing).
    PushRelease,
    /// "Keep lock & close" (`close_project(releaseLock=false)`).
    KeepLock,
    /// Quit only: "Discard & release" (`speleodb_release_current_lock`).
    DiscardRelease,
}

/// One step's verb. `p` indexes the run's projects (modulo their count).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) enum Act {
    // ---- the app (signed in as the owner, through the fault proxy) ----
    Login,
    /// Sign the app in to ANOTHER SpeleoDB deployment (the in-process stub).
    LoginOther,
    Logout,
    ListProjects,
    Open {
        p: u8,
        on_unpushed: OnUnpushed,
    },
    StartEditing,
    ResumeHeldLock,
    ForceEdit,
    /// Add a section named by a fresh edit token.
    Edit,
    /// Add a shot to the newest fuzz section (a content change with no new token).
    EditShot,
    /// The frontend's autosave.
    Save,
    Push,
    Done,
    Update,
    TakeRemote,
    Discard {
        keep_lock: bool,
    },
    /// Revision View: `back` commits behind the tip (modulo the history).
    View {
        back: u8,
    },
    /// Revision Restore; `discard` answers the un-pushed-edits re-ask.
    Restore {
        back: u8,
        discard: bool,
    },
    Unwind,
    Rewind,
    Poll,
    ReleaseCurrentLock,
    ReleaseAllLocks,
    /// Close the project; `mode` answers the close dialog when it is raised.
    Close {
        mode: LeaveMode,
    },
    /// Quit the app (the exit hook's bounded release) and start it again.
    Quit {
        mode: LeaveMode,
    },
    /// Kill the app: no save, no release; then start it again.
    Crash,
    Unlink,
    /// The divergence prompt's "Save my version as a local copy".
    SaveLocalCopy,
    DeleteProject,
    Publish,
    // ---- the second client (direct to the server, never through the proxy) ----
    ActorLock {
        who: Who,
        p: u8,
    },
    ActorRelease {
        who: Who,
        p: u8,
    },
    /// Push a revision on top of the tip (acquire, upload, release).
    ActorPush {
        who: Who,
        p: u8,
    },
    /// Push a revision a NEWER Stalagmight saved (one schema version above).
    ActorPushNewer {
        who: Who,
        p: u8,
    },
    ActorDelete {
        p: u8,
    },
    ActorRename {
        p: u8,
    },
    /// Take the owner's permission on a project the collaborator administers
    /// down to `level` (0 = remove, 1 = READ_ONLY, 2 = READ_AND_WRITE).
    ActorSetOwnerAccess {
        p: u8,
        level: u8,
    },
}

impl Act {
    /// The second client's steps (they never go through the proxy).
    pub(super) fn is_actor(&self) -> bool {
        matches!(
            self,
            Act::ActorLock { .. }
                | Act::ActorRelease { .. }
                | Act::ActorPush { .. }
                | Act::ActorPushNewer { .. }
                | Act::ActorDelete { .. }
                | Act::ActorRename { .. }
                | Act::ActorSetOwnerAccess { .. }
        )
    }
}

/// A fault the proxy applies to the matching request(s) of one app step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Fault {
    /// Answer this status; the server never sees the request.
    Status(u16),
    /// The server applies the request; the client gets this status instead.
    StatusAfter(u16),
    LatencyMs(u64),
    /// Drop the connection mid-upload, before the server sees the request.
    DropBefore,
    /// The server applies the request; the reply is lost.
    DropAfter,
    /// Cut the response body short (mid-download drop).
    Truncate,
    /// Answer half the body with a consistent length (a malformed payload).
    Corrupt,
    /// Hold the reply past the client's timeout, then let the server apply it.
    HangMs(u64),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct FaultSpec {
    pub(super) fault: Fault,
    pub(super) method: Option<String>,
    /// A substring of the request path (never a project id: the world differs per run).
    pub(super) path: String,
    pub(super) times: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Step {
    pub(super) act: Act,
    pub(super) fault: Option<FaultSpec>,
    /// [#576 SDB-POLL-D1] Further faults armed after `fault`, in this order (the
    /// first armed rule that matches a request fires), for a directed step whose act
    /// needs two of its requests to fail. The generator never draws one; step lists
    /// saved without it read as empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) then: Vec<FaultSpec>,
}

/// What the generator may know about the app between steps.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Hint {
    pub(super) logged_in: bool,
    pub(super) open: bool,
    pub(super) linked: bool,
    pub(super) editing: bool,
    pub(super) dirty: bool,
    pub(super) force_editing: bool,
    pub(super) projects: u8,
}

/// The paths faults aim at: every SpeleoDB route the app uses.
const FAULT_PATHS: &[(u32, Option<&str>, &str)] = &[
    (14, Some("PUT"), "/upload/auto/"),
    (8, Some("POST"), "/acquire/"),
    (8, Some("POST"), "/release/"),
    (10, Some("GET"), "/download/dump/"),
    (8, Some("GET"), "/revisions/"),
    (8, Some("GET"), "api/v2/projects/"),
    (3, Some("DELETE"), "api/v2/projects/"),
    (3, Some("POST"), "/user/auth-token/"),
    (3, Some("GET"), "api/v2/user/"),
    (2, Some("DELETE"), "release_all_locks"),
    (2, Some("PATCH"), "api/v2/projects/"),
    (2, Some("POST"), "api/v2/projects/"),
];

fn pick_fault(rng: &mut Rng) -> FaultSpec {
    let latency = 200 + rng.below(1800);
    let fault = rng
        .weighted(&[
            (6, Fault::Status(500)),
            (4, Fault::Status(502)),
            (3, Fault::Status(503)),
            (3, Fault::Status(429)),
            (3, Fault::Status(404)),
            (2, Fault::Status(409)),
            (2, Fault::Status(412)),
            (2, Fault::Status(403)),
            (2, Fault::Status(401)),
            (6, Fault::StatusAfter(500)),
            (3, Fault::StatusAfter(502)),
            (3, Fault::StatusAfter(504)),
            (4, Fault::LatencyMs(latency)),
            (5, Fault::DropBefore),
            (6, Fault::DropAfter),
            (5, Fault::Truncate),
            (4, Fault::Corrupt),
            (1, Fault::HangMs(32_000)),
        ])
        .unwrap_or(Fault::Status(500));
    let (method, path) = rng
        .weighted(
            &FAULT_PATHS
                .iter()
                .map(|(w, m, p)| (*w, (*m, *p)))
                .collect::<Vec<_>>(),
        )
        .unwrap_or((None, "api/v2/"));
    let times = rng
        .weighted(&[(10, 1u32), (3, 2), (2, 3), (1, 50)])
        .unwrap_or(1);
    FaultSpec {
        fault,
        method: method.map(str::to_owned),
        path: path.to_owned(),
        times,
    }
}

/// Pick the next step. `fault_percent` is the chance an app step gets a fault.
pub(super) fn next_step(rng: &mut Rng, hint: Hint, fault_percent: u64) -> Step {
    let projects = u64::from(hint.projects.max(1));
    let p = u8::try_from(rng.below(projects)).unwrap_or(0);
    let who = if rng.chance(80) {
        Who::Collab
    } else {
        Who::Owner
    };
    let back = u8::try_from(rng.below(4)).unwrap_or(0);
    let on_unpushed = if rng.chance(60) {
        OnUnpushed::KeepLocal
    } else {
        OnUnpushed::TakeServer
    };
    let mode = rng
        .weighted(&[
            (5, LeaveMode::PushRelease),
            (3, LeaveMode::KeepLock),
            (2, LeaveMode::DiscardRelease),
        ])
        .unwrap_or(LeaveMode::PushRelease);
    let mut table: Vec<(u32, Act)> = vec![
        (6, Act::ActorPush { who, p }),
        (3, Act::ActorLock { who, p }),
        (3, Act::ActorRelease { who, p }),
        (1, Act::ActorPushNewer { who, p }),
        (1, Act::ActorRename { p }),
        (1, Act::ActorDelete { p }),
        (
            2,
            Act::ActorSetOwnerAccess {
                p,
                level: u8::try_from(rng.below(3)).unwrap_or(0),
            },
        ),
        (1, Act::LoginOther),
        (1, Act::Crash),
        (2, Act::Quit { mode }),
        (2, Act::ListProjects),
    ];
    if hint.logged_in {
        table.extend([
            (2, Act::Logout),
            (if hint.open { 3 } else { 14 }, Act::Open { p, on_unpushed }),
            (4, Act::Poll),
            (1, Act::ReleaseAllLocks),
        ]);
    } else {
        table.push((30, Act::Login));
    }
    if hint.open {
        table.extend([(2, Act::Close { mode }), (2, Act::Save), (1, Act::Unlink)]);
        if hint.linked {
            table.extend([
                (if hint.editing { 2 } else { 12 }, Act::StartEditing),
                (2, Act::ResumeHeldLock),
                (3, Act::Update),
                (if hint.editing { 1 } else { 4 }, Act::View { back }),
                (2, Act::TakeRemote),
                (2, Act::SaveLocalCopy),
                (1, Act::ReleaseCurrentLock),
                (1, Act::DeleteProject),
                (1, Act::ForceEdit),
            ]);
        } else {
            table.push((4, Act::Publish));
        }
        if hint.editing || hint.force_editing {
            table.extend([
                (16, Act::Edit),
                (4, Act::EditShot),
                (6, Act::Save),
                (7, Act::Push),
                (7, Act::Done),
                (
                    3,
                    Act::Discard {
                        keep_lock: rng.chance(40),
                    },
                ),
                (
                    2,
                    Act::Restore {
                        back,
                        discard: rng.chance(50),
                    },
                ),
                (2, Act::Unwind),
                (1, Act::Rewind),
            ]);
        } else {
            table.push((2, Act::Edit));
        }
        if hint.dirty {
            table.extend([(3, Act::Done), (2, Act::StartEditing)]);
        }
    }
    let act = rng.weighted(&table).unwrap_or(Act::Poll);
    let fault = (!act.is_actor() && rng.chance(fault_percent)).then(|| pick_fault(rng));
    Step {
        act,
        fault,
        then: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{next_step, Hint, Rng, Step};

    fn run(seed: u64) -> Vec<Step> {
        let mut rng = Rng::new(seed);
        let hint = Hint {
            logged_in: true,
            open: true,
            linked: true,
            editing: true,
            projects: 3,
            ..Hint::default()
        };
        (0..200).map(|_| next_step(&mut rng, hint, 30)).collect()
    }

    #[test]
    fn a_seed_names_one_sequence_and_traces_round_trip() {
        assert_eq!(run(7), run(7));
        assert_ne!(run(7), run(8));
        let steps = run(11);
        let json = serde_json::to_string(&steps).expect("serialize");
        let back: Vec<Step> = serde_json::from_str(&json).expect("parse");
        assert_eq!(back, steps);
        assert!(steps.iter().any(|s| s.fault.is_some()), "faults are drawn");
        assert!(
            steps.iter().any(|s| s.act.is_actor()),
            "the second client acts"
        );
        assert!(
            steps
                .iter()
                .all(|s| !(s.act.is_actor() && s.fault.is_some())),
            "no fault is armed on a second-client step (it bypasses the proxy)"
        );
        assert!(
            steps.iter().all(|s| s.then.is_empty()),
            "the generator arms one fault per step; further ones are for directed steps"
        );
    }

    /// [#576 SDB-POLL-D1] A step list saved before `then` existed still reads, and a
    /// directed step's further faults survive the round trip in their order.
    #[test]
    fn further_faults_are_optional_and_keep_their_order() {
        let old: Step = serde_json::from_str(r#"{"act":"StartEditing","fault":null}"#)
            .expect("an old step reads");
        assert_eq!(
            old.then,
            Vec::<super::FaultSpec>::new(),
            "no further faults"
        );
        assert!(
            !serde_json::to_string(&old)
                .expect("serialize")
                .contains("then"),
            "and is written back the same way"
        );
        let directed: Step = serde_json::from_str(
            r#"{"act":"StartEditing","fault":null,"then":[
                {"fault":{"StatusAfter":502},"method":"POST","path":"/acquire/","times":1},
                {"fault":{"Status":503},"method":"GET","path":"/projects/","times":3}]}"#,
        )
        .expect("a directed step reads");
        let json = serde_json::to_string(&directed).expect("serialize");
        let back: Step = serde_json::from_str(&json).expect("parse");
        assert_eq!(back, directed);
        assert_eq!(
            back.then.iter().map(|f| f.times).collect::<Vec<_>>(),
            [1, 3]
        );
    }

    #[test]
    fn the_rng_is_the_splitmix_reference() {
        // SplitMix64's published first outputs for state 0 after one increment,
        // shifted by our seed whitening: pinned so a seed means the same run on
        // every build.
        let mut rng = Rng::new(0x9e37_79b9_7f4a_7c15);
        assert_eq!(rng.next_u64(), 0xe220_a839_7b1d_cdaf);
    }
}
