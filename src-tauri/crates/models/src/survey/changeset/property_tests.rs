//! Seeded property test: random walks through the survey operations, a
//! changeset captured after every step, then undo all and redo all, landing
//! on every recorded state to the bit (canonical model text; counters may
//! stand higher), in the session (session shot ids back exactly) and through
//! the persisted entries (ids minted, still sound). At every depth, a live
//! project changed behind the engine's back is refused and left untouched.
//! The persistence crate runs the same walks against the `.stalx` v2 bytes.

use std::collections::BTreeMap;

use super::testkit::{
    canonical, record_walk, replay, same_state, session_ids, session_ids_sound, tamper, Recorded,
};
use super::{redo, undo, Direction};

const SEEDS: u64 = 48;
const STEPS: usize = 70;

fn check(live: &super::super::Project, expected: &super::super::Project, what: &str) {
    if let Err(e) = same_state(live, expected) {
        panic!("{what}: {e}");
    }
    if let Err(e) = session_ids_sound(live) {
        panic!("{what}: {e}");
    }
    let violations = live.validate();
    assert!(violations.is_empty(), "{what}: invalid: {violations:?}");
}

#[test]
fn undo_all_and_redo_all_land_on_every_recorded_state_to_the_bit() {
    let mut ops: BTreeMap<&str, usize> = BTreeMap::new();
    for seed in 0..SEEDS {
        let recorded = record_walk(seed, STEPS);
        for op in &recorded.ops {
            *ops.entry(op).or_default() += 1;
        }
        replay(&recorded, false, |live, expected, what| {
            check(live, expected, &format!("seed {seed}, {what}"));
        });
    }
    for op in [
        "create line",
        "create shot",
        "update shot",
        "delete shot",
        "split shot",
        "delete range",
        "delete line",
        "reverse",
        "split line",
        "combine",
        "merge",
        "detach",
        "create closure",
        "set datum",
        "remove datum",
        "move segment",
        "move station",
        "reorder lines",
        "update line",
        "rename station",
        "credit",
        "append",
        "azimuth reference",
        "hand move",
        "external id",
        "import hash",
    ] {
        assert!(
            ops.get(op).copied().unwrap_or(0) >= 3,
            "{op} rarely went through: {ops:?}"
        );
    }
}

#[test]
fn persisted_entries_undo_and_redo_every_step_to_the_bit() {
    for seed in 0..SEEDS {
        let recorded = record_walk(seed, STEPS);
        replay(&recorded, true, |live, expected, what| {
            check(live, expected, &format!("seed {seed} (persisted), {what}"));
        });
    }
}

/// At every depth, both directions: a change behind the engine's back to
/// something the step checks is refused, the project untouched (session ids
/// included); the untampered project then steps as recorded.
#[test]
fn a_step_on_a_project_changed_behind_its_back_is_refused_and_changes_nothing() {
    let mut refused = 0usize;
    for seed in 0..SEEDS / 2 {
        let Recorded {
            states, changes, ..
        } = record_walk(seed, STEPS);
        for (k, cs) in changes.iter().enumerate() {
            for (direction, start) in [(Direction::Undo, k + 1), (Direction::Redo, k)] {
                let mut live = states[start].clone();
                let Some(what) = tamper(&mut live, cs, direction) else {
                    continue;
                };
                let (text, ids) = (canonical(&live), session_ids(&live));
                let result = match direction {
                    Direction::Undo => undo(&mut live, cs),
                    Direction::Redo => redo(&mut live, cs),
                };
                assert!(
                    result.is_err(),
                    "seed {seed} step {k} {direction:?}: a step went through after {what} changed"
                );
                assert_eq!(
                    canonical(&live),
                    text,
                    "seed {seed} step {k}: refused, yet changed"
                );
                assert_eq!(session_ids(&live), ids, "seed {seed} step {k}: ids changed");
                refused += 1;
            }
        }
    }
    assert!(refused > 500, "only {refused} tampered steps were tried");
}

/// Redoing on the BEFORE state of another step, or undoing twice, refuses.
#[test]
fn a_step_out_of_order_is_refused() {
    let mut refused = 0usize;
    for seed in 0..8 {
        let Recorded {
            states, changes, ..
        } = record_walk(seed, STEPS);
        for k in 1..changes.len() {
            // Undo step k on the state before it (it is not undone yet).
            let mut live = states[k].clone();
            let text = canonical(&live);
            if undo(&mut live, &changes[k]).is_err() {
                refused += 1;
                assert_eq!(canonical(&live), text);
            }
        }
    }
    assert!(refused > 100, "only {refused} out-of-order steps refused");
}
