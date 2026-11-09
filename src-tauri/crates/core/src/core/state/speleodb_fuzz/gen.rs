//! The seeded step generator: which verb next, with which fault, given what the
//! run knows about the app (a [`Hint`]).

use super::model::{Acct, Act, Fault, FaultSpec, HoldSpec, Rng, Step};

/// What the generator may know about the app between steps.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Hint {
    pub(super) logged_in: bool,
    pub(super) open: bool,
    pub(super) linked: bool,
    pub(super) editing: bool,
    /// The open copy holds un-pushed edits (the link's dirty bit).
    pub(super) dirty: bool,
    /// The frontend holds unsaved changes.
    pub(super) unsaved: bool,
    pub(super) projects: u8,
    pub(super) has_ro: bool,
}

/// One route faults aim at, with its draw weight.
struct FaultPath {
    weight: u32,
    method: &'static str,
    path: &'static str,
}

/// Every SpeleoDB route the app uses.
const FAULT_PATHS: &[FaultPath] = &[
    FaultPath {
        weight: 14,
        method: "PUT",
        path: "/upload/stalagmight/",
    },
    FaultPath {
        weight: 9,
        method: "POST",
        path: "/acquire/",
    },
    FaultPath {
        weight: 9,
        method: "POST",
        path: "/release/",
    },
    FaultPath {
        weight: 10,
        method: "GET",
        path: "/download/stalagmight/",
    },
    FaultPath {
        weight: 8,
        method: "GET",
        path: "/revisions/",
    },
    FaultPath {
        weight: 8,
        method: "GET",
        path: "api/v2/projects/",
    },
    FaultPath {
        weight: 3,
        method: "POST",
        path: "/user/auth-token/",
    },
    FaultPath {
        weight: 2,
        method: "GET",
        path: "api/v2/user/",
    },
    FaultPath {
        weight: 2,
        method: "PATCH",
        path: "api/v2/projects/",
    },
    FaultPath {
        weight: 2,
        method: "POST",
        path: "api/v2/projects/",
    },
];

pub(super) fn pick_fault(rng: &mut Rng) -> FaultSpec {
    let latency = 200 + rng.below(1800);
    let fault = rng
        .weighted(&[
            (6, Fault::Status(500)),
            (4, Fault::Status(502)),
            (3, Fault::Status(503)),
            (2, Fault::Status(429)),
            (3, Fault::Status(404)),
            (2, Fault::Status(409)),
            (2, Fault::Status(403)),
            (2, Fault::Status(401)),
            (6, Fault::StatusAfter(500)),
            (3, Fault::StatusAfter(502)),
            (3, Fault::StatusAfter(504)),
            (4, Fault::LatencyMs(latency)),
            (5, Fault::DropBefore),
            (6, Fault::DropAfter),
            (5, Fault::Truncate),
            (3, Fault::Corrupt),
            (3, Fault::Scramble),
            // Past the 5 s lock-probe budget, well under every other one.
            (2, Fault::HangMs(6_000)),
            (4, Fault::Refuse),
        ])
        .unwrap_or(Fault::Status(500));
    let weighted: Vec<(u32, usize)> = FAULT_PATHS
        .iter()
        .enumerate()
        .map(|(i, f)| (f.weight, i))
        .collect();
    let route = rng
        .weighted(&weighted)
        .and_then(|i| FAULT_PATHS.get(i))
        .map_or(("GET", "api/v2/"), |f| (f.method, f.path));
    let times = rng
        .weighted(&[(10, 1u32), (3, 2), (3, 3), (1, 50)])
        .unwrap_or(1);
    FaultSpec {
        fault,
        method: Some(route.0.to_owned()),
        path: route.1.to_owned(),
        times,
    }
}

fn hold(method: &str, path: &str, reply: bool) -> HoldSpec {
    HoldSpec {
        method: method.to_owned(),
        path: path.to_owned(),
        reply,
    }
}

/// An interleaving: the first verb's held request and the verb that races it.
/// Each template aims at one documented fence (late replies, session moves, the
/// main-thread commands that wait on the lock-operation guard).
fn pick_interleave(rng: &mut Rng, hint: Hint, p: u8, q: u8) -> Act {
    let reply = rng.chance(70);
    let switch = Act::Open {
        p: q,
        take_server: false,
    };
    let account_moves = [
        Act::Logout,
        Act::Login { acct: Acct::Collab },
        Act::LoginOther,
    ];
    let pick = |rng: &mut Rng, items: &[Act]| -> Act {
        let i = rng.below(items.len() as u64) as usize;
        items.get(i).cloned().unwrap_or(Act::Refresh)
    };
    // Only the templates whose first verb can reach its request in this state.
    let usable: Vec<u64> = (0..10)
        .filter(|t| match t {
            0 | 6 | 9 => hint.editing,
            1 | 3 | 4 | 8 => hint.linked,
            7 => hint.open && !hint.linked,
            _ => true,
        })
        .collect();
    let template = usable
        .get(rng.below(usable.len() as u64) as usize)
        .copied()
        .unwrap_or(5);
    let (first, held, second) = match template {
        // A held upload against an account move, a switch, a close, a quit.
        0 => (
            Act::SaveChanges {
                long_message: false,
            },
            hold("PUT", "/upload/stalagmight/", true),
            pick(
                rng,
                &[
                    account_moves[0].clone(),
                    account_moves[1].clone(),
                    switch,
                    Act::Close,
                    Act::ConfigTouch,
                    Act::Quit,
                ],
            ),
        ),
        // A held acquire / revision read of Start Editing.
        1 => (
            Act::StartEditing,
            if rng.chance(50) {
                hold("POST", "/acquire/", reply)
            } else {
                hold("GET", "/revisions/", reply)
            },
            pick(
                rng,
                &[
                    account_moves[0].clone(),
                    account_moves[1].clone(),
                    Act::Close,
                    switch,
                    Act::Quit,
                    Act::ConfigTouch,
                ],
            ),
        ),
        // A held download of an open against a newer open (A-B and A-B-A), an
        // account move, a close.
        2 => (
            Act::Open {
                p,
                take_server: false,
            },
            hold("GET", "/download/stalagmight/", reply),
            pick(
                rng,
                &[
                    switch,
                    Act::Open {
                        p,
                        take_server: false,
                    },
                    account_moves[0].clone(),
                    account_moves[1].clone(),
                    Act::Close,
                    Act::NewLocal,
                ],
            ),
        ),
        // A held revision download (View / Revert) against a switch or an edit.
        3 => (
            if rng.chance(60) {
                Act::View {
                    back: (rng.below(3)) as u8,
                }
            } else {
                Act::Revert {
                    back: (rng.below(3)) as u8,
                }
            },
            hold("GET", "/download/stalagmight/", true),
            pick(rng, &[switch, Act::NewLocal, Act::Edit, Act::Close]),
        ),
        // A held reload against an edit, a close, a switch, a logout.
        4 => (
            Act::Reload,
            if rng.chance(50) {
                hold("GET", "/revisions/", true)
            } else {
                hold("GET", "/download/stalagmight/", true)
            },
            pick(
                rng,
                &[Act::Edit, Act::Close, switch, account_moves[0].clone()],
            ),
        ),
        // A held sign-in against a sign-out (credential resurrection).
        5 => (
            Act::Login { acct: Acct::Owner },
            hold("POST", "/user/auth-token/", true),
            Act::Logout,
        ),
        // A held read-only release against reads of the project mutex.
        6 => (
            Act::ReadOnly,
            hold("POST", "/release/", reply),
            pick(rng, &[Act::StateRead, Act::Edit, Act::Close]),
        ),
        // A held publish against an account move or a switch.
        7 => (
            Act::Publish,
            if rng.chance(50) {
                hold("PUT", "/upload/stalagmight/", true)
            } else {
                hold("POST", "api/v2/projects/", true)
            },
            pick(rng, &[account_moves[0].clone(), switch, Act::Close]),
        ),
        // A held status probe against Start Editing, a close, a switch.
        8 => (
            Act::Refresh,
            hold("GET", "api/v2/projects/", true),
            pick(rng, &[Act::StartEditing, Act::Close, switch]),
        ),
        // A held close-time release against a reopen of the same project.
        _ => (
            Act::Close,
            hold("POST", "/release/", false),
            Act::Open {
                p,
                take_server: false,
            },
        ),
    };
    Act::Interleave {
        first: Box::new(first),
        hold: held,
        second: Box::new(second),
    }
}

/// Pick the next step. `fault_percent` is the chance an app step gets a fault;
/// `skip` names acts never to draw (`Act::name`).
pub(super) fn next_step(rng: &mut Rng, hint: Hint, fault_percent: u64, skip: &[String]) -> Step {
    let projects = u64::from(hint.projects.max(1));
    let p = u8::try_from(rng.below(projects)).unwrap_or(0);
    let q = u8::try_from(rng.below(projects)).unwrap_or(0);
    let who = if rng.chance(55) {
        Acct::Collab
    } else {
        Acct::Owner
    };
    let back = u8::try_from(rng.below(4)).unwrap_or(0);
    let mut table: Vec<(u32, Act)> = vec![
        (
            6,
            Act::ActorPush {
                who,
                p,
                keep_lock: rng.chance(30),
            },
        ),
        (3, Act::ActorLock { who, p }),
        (3, Act::ActorRelease { who, p }),
        (
            1,
            Act::ActorRelease {
                who: Acct::Admin,
                p,
            },
        ),
        (1, Act::ActorPushNewer { who, p }),
        (1, Act::ActorRename { p }),
        (1, Act::ActorDelete { p }),
        (
            2,
            Act::ActorSetAccess {
                p,
                level: u8::try_from(rng.below(3)).unwrap_or(0),
            },
        ),
        (1, Act::LoginOther),
        (
            1,
            Act::Crash {
                residue: rng.chance(60),
            },
        ),
        (2, Act::Quit),
        (1, Act::RestartUpdate),
        (2, Act::ListProjects),
        (2, Act::NewLocal),
        (1, Act::ConfigTouch),
    ];
    if hint.logged_in {
        table.extend([
            (2, Act::Logout),
            (
                if hint.open { 4 } else { 14 },
                Act::Open {
                    p,
                    take_server: rng.chance(40),
                },
            ),
            (1, Act::OpenFile { p }),
            (4, Act::Refresh),
            (1, Act::Login { acct: Acct::Collab }),
            (1, Act::Login { acct: Acct::Owner }),
        ]);
        if hint.has_ro {
            table.push((1, Act::Login { acct: Acct::Ro }));
        }
        table.push((5, pick_interleave(rng, hint, p, q)));
    } else {
        table.push((30, Act::Login { acct: Acct::Owner }));
    }
    if hint.open {
        table.extend([(3, Act::Close), (3, Act::Autosave), (1, Act::StateRead)]);
        if hint.linked {
            table.extend([
                (if hint.editing { 1 } else { 8 }, Act::StartEditing),
                (3, Act::Reload),
                (2, Act::View { back }),
                (2, Act::TakeRemote),
                (1, Act::SaveCopy),
                (2, Act::ReadOnly),
            ]);
        } else {
            table.push((5, Act::Publish));
        }
        if hint.editing {
            table.extend([
                (16, Act::Edit),
                (4, Act::EditShot),
                (
                    8,
                    Act::SaveChanges {
                        long_message: rng.chance(3),
                    },
                ),
                (5, Act::SaveAndClose),
                (2, Act::DiscardKeep),
                (2, Act::Revert { back }),
                (2, Act::Unwind),
                (1, Act::Rewind),
            ]);
        } else {
            table.push((2, Act::Edit));
        }
        if hint.dirty || hint.unsaved {
            table.extend([(3, Act::SaveAndClose), (2, Act::StartEditing)]);
        }
    }
    table.retain(|(_, act)| !skip.iter().any(|s| s == &act.name()));
    let act = rng.weighted(&table).unwrap_or(Act::Refresh);
    let faultable = !act.is_actor() && !matches!(act, Act::Interleave { .. });
    let fault = (faultable && rng.chance(fault_percent)).then(|| pick_fault(rng));
    Step {
        act,
        fault,
        then: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{next_step, Hint};
    use crate::core::state::speleodb_fuzz::model::{Act, Rng, Step};

    fn run(seed: u64, skip: &[String]) -> Vec<Step> {
        let mut rng = Rng::new(seed);
        let hint = Hint {
            logged_in: true,
            open: true,
            linked: true,
            editing: true,
            projects: 3,
            ..Hint::default()
        };
        (0..300)
            .map(|_| next_step(&mut rng, hint, 30, skip))
            .collect()
    }

    #[test]
    fn a_seed_names_one_sequence_and_traces_round_trip() {
        assert_eq!(run(7, &[]), run(7, &[]));
        assert_ne!(run(7, &[]), run(8, &[]));
        let steps = run(11, &[]);
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
                .any(|s| matches!(s.act, Act::Interleave { .. })),
            "interleavings are drawn"
        );
        assert!(
            steps.iter().all(|s| s.fault.is_none()
                || !(s.act.is_actor() || matches!(s.act, Act::Interleave { .. }))),
            "no fault is armed on a second-client step or an interleaving"
        );
    }

    #[test]
    fn skipped_acts_are_never_drawn() {
        let skip = vec!["Edit".to_owned(), "Interleave".to_owned()];
        let steps = run(5, &skip);
        assert!(steps
            .iter()
            .all(|s| !matches!(s.act, Act::Edit | Act::Interleave { .. })));
        assert_eq!(Act::Edit.name(), "Edit");
        assert_eq!(
            Act::Open {
                p: 0,
                take_server: false
            }
            .name(),
            "Open"
        );
    }

    #[test]
    fn the_rng_is_the_splitmix_reference() {
        // SplitMix64's published first output for state 0 after one increment,
        // shifted by the seed whitening: pinned so a seed means the same run on
        // every build.
        let mut rng = Rng::new(0x9e37_79b9_7f4a_7c15);
        assert_eq!(rng.next_u64(), 0xe220_a839_7b1d_cdaf);
    }
}
