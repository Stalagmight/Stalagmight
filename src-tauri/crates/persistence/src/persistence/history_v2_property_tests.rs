//! The undo engine against the `.stalx` v2 bytes: random walks through the
//! survey operations, a changeset captured after each step; undo all and
//! redo all land on every recorded state with identical v2 bytes (counters
//! may stand higher), in the session and through the persisted entries.
//! And end to end: the entries ride the file's `history` member, the file is
//! decoded, each entry decoded, and the decoded project undoes back to the
//! start through them.

use stalagmight_models::survey::changeset::testkit::{
    entry_text, record_walk, replay, same_state, session_ids_sound,
};
use stalagmight_models::survey::changeset::{decode_entry, undo, EntryLimits};
use stalagmight_models::survey::Project;

use super::stalx_v2::{decode, encode, DecodeOptions, Extras, History, Mode, RawJson};

fn v2(p: &Project) -> Vec<u8> {
    encode(p, &Extras::default(), Mode::Normal).expect("a valid model encodes")
}

/// `live` stepped onto `expected`: the same v2 bytes once the counters
/// (which may stand higher) are set to the recorded ones.
fn same_v2(live: &Project, expected: &Project, what: &str) {
    if let Err(e) = same_state(live, expected) {
        panic!("{what}: {e}");
    }
    if let Err(e) = session_ids_sound(live) {
        panic!("{what}: {e}");
    }
    let mut normalized = live.clone();
    normalized.counters = expected.counters;
    assert!(
        v2(&normalized) == v2(expected),
        "{what}: the v2 bytes differ"
    );
}

#[test]
fn undo_and_redo_land_on_every_state_with_identical_v2_bytes() {
    for seed in 0..32 {
        let recorded = record_walk(seed, 60);
        for persisted in [false, true] {
            replay(&recorded, persisted, |live, expected, what| {
                same_v2(
                    live,
                    expected,
                    &format!("seed {seed} persisted {persisted}: {what}"),
                );
            });
        }
    }
}

#[test]
fn a_history_rides_the_file_and_undoes_from_the_decoded_project() {
    for seed in 100..116 {
        let recorded = record_walk(seed, 50);
        let entries: Vec<RawJson> = recorded
            .changes
            .iter()
            .enumerate()
            .map(|(k, cs)| {
                let text = entry_text(cs, recorded.ops[k]);
                let raw = RawJson::parse(&text).expect("an entry is one JSON value");
                assert_eq!(
                    raw.as_str(),
                    text,
                    "an entry is already in the format's canonical form"
                );
                raw
            })
            .collect();
        let last = recorded.states.last().expect("a start state");
        let extras = Extras {
            history: History {
                undo: entries,
                redo: Vec::new(),
            },
            ..Extras::default()
        };
        let bytes = encode(last, &extras, Mode::Normal).expect("the file encodes");
        let decoded = decode(&bytes, &DecodeOptions::default()).expect("the file decodes");
        assert_eq!(
            encode(&decoded.project, &decoded.extras, Mode::Normal).expect("re-encode"),
            bytes,
            "seed {seed}: the file is a fixpoint with its history"
        );
        let mut live = decoded.project;
        let undo_stack = &decoded.extras.history.undo;
        assert_eq!(undo_stack.len(), recorded.changes.len());
        for k in (0..undo_stack.len()).rev() {
            let entry = decode_entry(undo_stack[k].as_str(), &EntryLimits::default())
                .unwrap_or_else(|e| panic!("seed {seed} entry {k}: {e}"));
            undo(&mut live, &entry.changes)
                .unwrap_or_else(|e| panic!("seed {seed} undo {k} ({}): {e}", recorded.ops[k]));
            same_v2(
                &live,
                &recorded.states[k],
                &format!("seed {seed} file undo {k}"),
            );
        }
    }
}
