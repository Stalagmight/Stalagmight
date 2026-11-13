//! The loaded history follows the load's shot UUID heal (`uuid_remap.rs`).
//!
//! Every project and entry here is synthetic: sections 0 and 1 hold the same
//! three ids, the live heal renames section 1's, and each test checks that an
//! entry recorded against the OLD ids now addresses the renamed shots - in every
//! place the variant keeps an id - while anything about section 0 is untouched.

use std::collections::HashMap;

use chrono::Utc;

use super::*;
use crate::core::undo::AddedContent;
use crate::core::Command;
use crate::models::{
    ClosureStrategy, Section, SectionIdentity, ShotData, StationMerge, StationRef, ZeroDatum,
};

const X: [&str; 3] = [
    "aaaaaaaa-0000-4000-8000-000000000001",
    "aaaaaaaa-0000-4000-8000-000000000002",
    "aaaaaaaa-0000-4000-8000-000000000003",
];

fn shot(uuid: &str) -> ShotData {
    ShotData {
        uuid: Some(uuid.to_string()),
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        length: 10.0,
        ..Default::default()
    }
}

fn copy_data() -> SectionData {
    SectionData {
        shots: X.iter().map(|u| shot(u)).collect(),
        ..Default::default()
    }
}

/// Sections 0 and 1 holding the same ids, as a pre-uniqueness file loads.
fn two_copies() -> Project {
    let mut project = Project::new("Copies".to_string(), None);
    project.add_section(copy_data());
    project.add_section(copy_data());
    project
}

fn healed() -> (Project, ShotUuidRemap) {
    let mut project = two_copies();
    let renamed = project.make_shot_uuids_project_unique();
    assert_eq!(renamed.len(), 3);
    (project, renamed)
}

fn new_id(old: &str) -> String {
    reminted_shot_uuid(1, old)
}

fn key(section: SectionId, uuid: &str) -> StationKey {
    StationKey::new(section, StationAnchor::AfterShot(uuid.to_string()))
}

fn closure(a: StationKey, b: StationKey) -> LoopClosure {
    LoopClosure {
        id: 1,
        station_a: a,
        station_b: b,
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    }
}

fn after_shot(key: &StationKey) -> &str {
    match &key.anchor {
        StationAnchor::AfterShot(uuid) => uuid,
        other => panic!("expected an AfterShot anchor, got {other:?}"),
    }
}

fn command() -> Command {
    Command::DeleteSection { section_id: 1 }
}

fn stack_of(entries: Vec<UndoData>) -> UndoStack {
    let mut stack = UndoStack::new();
    for data in entries {
        stack.push_with_data(command(), data);
    }
    stack
}

fn only_entry(stack: &UndoStack) -> &UndoData {
    let mut entries = stack.undo_entries();
    let data = &entries.next().unwrap().undo_data;
    assert!(entries.next().is_none());
    data
}

#[test]
fn shot_field_entries_follow_both_copies_of_a_renamed_id() {
    let (project, renamed) = healed();
    let mut stack = stack_of(vec![
        UndoData::PreviousValues {
            section_id: 1,
            shot_id: Some(100_000),
            shot_uuid: Some(X[0].to_string()),
            values_json: "{}".to_string(),
            previous_offsets: HashMap::new(),
        },
        UndoData::CreatedShotSnapshot {
            section_id: 1,
            shot_id: 100_001,
            shot_data: shot(X[1]),
            position: 1,
            station_shift_from: 0,
            after_shot_id: None,
            before_shot_id: None,
            after_shot_uuid: Some(X[0].to_string()),
            before_shot_uuid: Some(X[2].to_string()),
            uuid: Some(X[1].to_string()),
            previous_offsets: HashMap::new(),
            consumed_legacy: None,
        },
        UndoData::ShotSnapshot {
            section_id: 1,
            shot_id: 100_002,
            shot_data: shot(X[2]),
            position: 2,
            uuid: Some(X[2].to_string()),
        },
        // Section 0 kept its ids: an entry about it must not move.
        UndoData::PreviousValues {
            section_id: 0,
            shot_id: Some(0),
            shot_uuid: Some(X[0].to_string()),
            values_json: "{}".to_string(),
            previous_offsets: HashMap::new(),
        },
    ]);

    assert_eq!(stack.translate_shot_uuids(&project, &renamed), 3);

    let entries: Vec<&UndoData> = stack.undo_entries().map(|e| &e.undo_data).collect();
    // `undo_entries` is newest first.
    let UndoData::PreviousValues { shot_uuid, .. } = entries[0] else {
        panic!("section 0 entry");
    };
    assert_eq!(shot_uuid.as_deref(), Some(X[0]));
    let UndoData::ShotSnapshot {
        shot_data, uuid, ..
    } = entries[1]
    else {
        panic!("shot snapshot");
    };
    assert_eq!(shot_data.uuid.as_deref(), Some(new_id(X[2]).as_str()));
    assert_eq!(uuid.as_deref(), Some(new_id(X[2]).as_str()));
    let UndoData::CreatedShotSnapshot {
        shot_data,
        after_shot_uuid,
        before_shot_uuid,
        uuid,
        ..
    } = entries[2]
    else {
        panic!("created shot snapshot");
    };
    assert_eq!(shot_data.uuid.as_deref(), Some(new_id(X[1]).as_str()));
    assert_eq!(uuid.as_deref(), Some(new_id(X[1]).as_str()));
    assert_eq!(after_shot_uuid.as_deref(), Some(new_id(X[0]).as_str()));
    assert_eq!(before_shot_uuid.as_deref(), Some(new_id(X[2]).as_str()));
    let UndoData::PreviousValues { shot_uuid, .. } = entries[3] else {
        panic!("section 1 entry");
    };
    assert_eq!(shot_uuid.as_deref(), Some(new_id(X[0]).as_str()));
}

#[test]
fn section_entries_follow_in_their_data_and_their_identity() {
    let (project, renamed) = healed();
    let mut stack = stack_of(vec![
        UndoData::SectionSnapshot {
            section_id: 1,
            section_data: copy_data(),
        },
        UndoData::CreatedSectionSnapshot {
            section_id: 1,
            section_data: copy_data(),
            identity: SectionIdentity {
                shot_uuids: X.iter().map(|u| Some((*u).to_string())).collect(),
                gauge_shot_uuid: Some(X[0].to_string()),
                gauge_neighbour_uuid: Some(X[1].to_string()),
                ..Default::default()
            },
        },
    ]);
    assert_eq!(stack.translate_shot_uuids(&project, &renamed), 2);

    let expected: Vec<Option<String>> = X.iter().map(|u| Some(new_id(u))).collect();
    for entry in stack.undo_entries() {
        match &entry.undo_data {
            UndoData::SectionSnapshot { section_data, .. } => {
                let ids: Vec<_> = section_data.shots.iter().map(|s| s.uuid.clone()).collect();
                assert_eq!(ids, expected);
            }
            UndoData::CreatedSectionSnapshot {
                section_data,
                identity,
                ..
            } => {
                let ids: Vec<_> = section_data.shots.iter().map(|s| s.uuid.clone()).collect();
                assert_eq!(ids, expected);
                assert_eq!(identity.shot_uuids, expected);
                assert_eq!(identity.gauge_shot_uuid, expected[0]);
                assert_eq!(identity.gauge_neighbour_uuid, expected[1]);
            }
            other => panic!("unexpected entry {other:?}"),
        }
    }
}

#[test]
fn closure_and_merge_entries_follow_only_their_renamed_endpoint() {
    let (project, renamed) = healed();
    let pair = || closure(key(1, X[1]), key(0, X[1]));
    let mut stack = stack_of(vec![
        UndoData::ClosureInfo {
            closure: pair(),
            position: None,
        },
        UndoData::MergeInfo {
            station_a: StationRef::new(1, 0),
            station_b: StationRef::new(0, 2),
            removed_closures: vec![pair()],
            previous_offsets: HashMap::new(),
        },
        UndoData::UnmergeInfo {
            station_a: StationRef::new(1, 0),
            station_b: StationRef::new(0, 2),
            removed_closures: vec![pair()],
        },
    ]);
    assert_eq!(stack.translate_shot_uuids(&project, &renamed), 3);

    let check = |c: &LoopClosure| {
        assert_eq!(after_shot(&c.station_a), new_id(X[1]));
        assert_eq!(after_shot(&c.station_b), X[1]);
    };
    for entry in stack.undo_entries() {
        match &entry.undo_data {
            UndoData::ClosureInfo { closure, .. } => check(closure),
            UndoData::MergeInfo {
                removed_closures, ..
            }
            | UndoData::UnmergeInfo {
                removed_closures, ..
            } => removed_closures.iter().for_each(check),
            other => panic!("unexpected entry {other:?}"),
        }
    }
}

#[test]
fn an_add_entry_follows_in_its_sections_refs_and_ignores() {
    let unhealed = two_copies();
    let (project, renamed) = healed();
    let mut copied: Section = unhealed.get_section(1).unwrap().clone();
    copied.gauge_shot_uuid = Some(X[0].to_string());
    let content = AddedContent {
        sections: vec![copied],
        station_comments: vec![(key(1, X[2]), "comment".to_string())],
        station_names: vec![(key(1, X[1]), "7".to_string())],
        station_offsets: vec![(key(1, X[0]), (1.0, 0.0, 0.0))],
        zero_datums: vec![ZeroDatum {
            station_ref: key(1, X[0]),
            gps: crate::models::GpsCoordinates::new(20.0, -87.0, 0.0).unwrap(),
        }],
        station_merges: vec![StationMerge {
            station_a: key(1, X[0]),
            station_b: key(0, X[2]),
            merged_at: Utc::now(),
        }],
        loop_closures: vec![closure(key(1, X[2]), key(0, X[0]))],
        ignores: vec![crate::core::undo::CarriedIgnore {
            section_id: 1,
            shot_uuid: X[1].to_string(),
            warning_code: "W1".to_string(),
            resolved_at: Utc::now(),
            resolved_comment: None,
        }],
        ..Default::default()
    };
    let mut stack = stack_of(vec![UndoData::AddedProjectContent(Box::new(content))]);
    assert_eq!(stack.translate_shot_uuids(&project, &renamed), 1);

    let UndoData::AddedProjectContent(content) = only_entry(&stack) else {
        panic!("add entry");
    };
    let ids: Vec<_> = content.sections[0]
        .shots
        .iter()
        .map(|s| s.uuid.clone().unwrap())
        .collect();
    assert_eq!(ids, X.iter().map(|u| new_id(u)).collect::<Vec<_>>());
    assert_eq!(content.sections[0].gauge_shot_uuid, Some(new_id(X[0])));
    assert_eq!(after_shot(&content.station_comments[0].0), new_id(X[2]));
    assert_eq!(after_shot(&content.station_names[0].0), new_id(X[1]));
    assert_eq!(after_shot(&content.station_offsets[0].0), new_id(X[0]));
    assert_eq!(
        after_shot(&content.zero_datums[0].station_ref),
        new_id(X[0])
    );
    assert_eq!(
        after_shot(&content.station_merges[0].station_a),
        new_id(X[0])
    );
    assert_eq!(after_shot(&content.station_merges[0].station_b), X[2]);
    assert_eq!(
        after_shot(&content.loop_closures[0].station_a),
        new_id(X[2])
    );
    assert_eq!(after_shot(&content.loop_closures[0].station_b), X[0]);
    assert_eq!(content.ignores[0].shot_uuid, new_id(X[1]));
}

#[test]
fn a_station_move_entry_follows_too() {
    let (project, renamed) = healed();
    let mut stack = stack_of(vec![UndoData::StationMoveInfo {
        station_ref: StationRef::new(1, 1),
        prev_shot_before: Some((100_000, shot(X[0]))),
        prev_shot_after: Some((100_001, shot(X[1]))),
        prev_offset: None,
        prev_section_offset: None,
    }]);
    assert_eq!(stack.translate_shot_uuids(&project, &renamed), 1);
    let UndoData::StationMoveInfo {
        prev_shot_before,
        prev_shot_after,
        ..
    } = only_entry(&stack)
    else {
        panic!("station move");
    };
    assert_eq!(
        prev_shot_before.as_ref().unwrap().1.uuid,
        Some(new_id(X[0]))
    );
    assert_eq!(prev_shot_after.as_ref().unwrap().1.uuid, Some(new_id(X[1])));
}

/// The pre-heal project as an undo snapshot, with a comment on section 1's
/// station 3 and WITHOUT the `validation` key, as a legacy snapshot is.
fn legacy_snapshot() -> String {
    let mut project = two_copies();
    project
        .station_comments
        .insert(key(1, X[2]), "on the copy".to_string());
    project
        .station_comments
        .insert(key(0, X[2]), "on the first".to_string());
    let mut value = serde_json::to_value(&project).unwrap();
    value.as_object_mut().unwrap().remove("validation");
    serde_json::to_string(&value).unwrap()
}

#[test]
fn a_snapshot_pair_is_rewritten_without_gaining_the_keys_the_restore_probes() {
    let (project, renamed) = healed();
    let mut stack = stack_of(vec![UndoData::ProjectSnapshotPair {
        before: legacy_snapshot(),
        after: legacy_snapshot(),
    }]);
    assert_eq!(stack.translate_shot_uuids(&project, &renamed), 1);

    let UndoData::ProjectSnapshotPair { before, after } = only_entry(&stack) else {
        panic!("pair");
    };
    for half in [before, after] {
        assert!(
            !half.contains("\"validation\":"),
            "a legacy blob stays legacy"
        );
        let restored: Project = serde_json::from_str(half).unwrap();
        assert_eq!(
            restored.sections[1]
                .shots
                .iter()
                .map(|s| s.uuid.clone().unwrap())
                .collect::<Vec<_>>(),
            X.iter().map(|u| new_id(u)).collect::<Vec<_>>()
        );
        assert_eq!(
            restored.sections[0]
                .shots
                .iter()
                .map(|s| s.uuid.clone().unwrap())
                .collect::<Vec<_>>(),
            X
        );
        assert_eq!(
            restored
                .station_comments
                .get(&key(1, &new_id(X[2])))
                .map(String::as_str),
            Some("on the copy")
        );
        assert_eq!(
            restored
                .station_comments
                .get(&key(0, X[2]))
                .map(String::as_str),
            Some("on the first")
        );
        assert!(restored.check_structural_integrity().is_empty());
    }
    assert_eq!(
        stack.estimated_undo_bytes(),
        stack
            .undo_entries()
            .map(UndoStack::estimated_size)
            .sum::<usize>(),
        "the byte total follows the rewritten snapshot"
    );
}

#[test]
fn a_copy_only_the_history_still_holds_is_renamed_so_undo_cannot_repeat_an_id() {
    // Live: only section 0 holds the ids (the copy was deleted). The history
    // holds the delete's before-snapshot with BOTH copies, and the live heal
    // renamed nothing.
    let mut project = two_copies();
    project.sections.retain(|s| s.id == 0);
    project.rebuild_section_index();
    project.rebuild_shot_uuid_index();
    let renamed = project.make_shot_uuids_project_unique();
    assert!(renamed.is_empty());

    let mut stack = stack_of(vec![UndoData::ProjectSnapshotPair {
        before: legacy_snapshot(),
        after: serde_json::to_string(&project).unwrap(),
    }]);
    assert_eq!(stack.translate_shot_uuids(&project, &renamed), 1);

    let UndoData::ProjectSnapshotPair { before, after } = only_entry(&stack) else {
        panic!("pair");
    };
    let restored: Project = serde_json::from_str(before).unwrap();
    assert!(
        restored.check_structural_integrity().is_empty(),
        "undoing the delete must not bring a repeated id back"
    );
    assert_eq!(
        restored.sections[1].shots[0].uuid.as_deref(),
        Some(new_id(X[0]).as_str())
    );
    let kept: Project = serde_json::from_str(after).unwrap();
    assert_eq!(kept.sections[0].shots[0].uuid.as_deref(), Some(X[0]));
}

#[test]
fn a_history_without_repeats_is_left_byte_for_byte() {
    let mut project = Project::new("Plain".to_string(), None);
    project.add_section(copy_data());
    let renamed = project.make_shot_uuids_project_unique();
    let snapshot = serde_json::to_string(&project).unwrap();
    let mut stack = stack_of(vec![
        UndoData::ProjectSnapshotPair {
            before: snapshot.clone(),
            after: snapshot,
        },
        UndoData::PreviousValues {
            section_id: 0,
            shot_id: Some(0),
            shot_uuid: Some(X[0].to_string()),
            values_json: "{}".to_string(),
            previous_offsets: HashMap::new(),
        },
    ]);
    let before = serde_json::to_string(&stack.undo_entries().collect::<Vec<_>>()).unwrap();
    assert_eq!(stack.translate_shot_uuids(&project, &renamed), 0);
    let after = serde_json::to_string(&stack.undo_entries().collect::<Vec<_>>()).unwrap();
    assert_eq!(before, after);
}

#[test]
fn reconciling_keeps_an_entry_the_drop_pass_would_otherwise_remove() {
    let (project, renamed) = healed();
    let entry = || UndoData::CreatedShotSnapshot {
        section_id: 1,
        shot_id: 100_000,
        shot_data: shot(X[0]),
        position: 0,
        station_shift_from: 0,
        after_shot_id: None,
        before_shot_id: None,
        after_shot_uuid: None,
        before_shot_uuid: None,
        uuid: Some(X[0].to_string()),
        previous_offsets: HashMap::new(),
        consumed_legacy: None,
    };

    // Without the translation the entry names (1, old id), which the healed
    // section no longer holds, so the load would drop it.
    let mut untranslated = stack_of(vec![entry()]);
    assert_eq!(
        untranslated.drop_entries_addressing_a_missing_shot(&project),
        1
    );

    let mut stack = stack_of(vec![entry()]);
    stack.reconcile_with_loaded_project(&project, &renamed);
    assert_eq!(stack.len(), 1);
}

/// When the live heal had to step past an id already in use, only the live
/// renames know where the shot went: the first derived candidate is a DIFFERENT
/// shot. A history entry about the shot must follow the live rename.
#[test]
fn the_map_reuses_the_live_renames_verbatim() {
    let mut project = two_copies();
    // Section 1 also holds, on its last shot, the id the heal would derive for
    // its first one - so the heal steps past it.
    let planted = new_id(X[0]);
    project.get_section_mut(1).unwrap().shots[2].uuid = Some(planted.clone());
    project.rebuild_shot_uuid_index();
    let renamed = project.make_shot_uuids_project_unique();
    let stepped = renamed.get(1, X[0]).unwrap().to_string();
    assert_ne!(stepped, planted, "the heal stepped past the planted id");

    let mut stack = stack_of(vec![UndoData::PreviousValues {
        section_id: 1,
        shot_id: Some(100_000),
        shot_uuid: Some(X[0].to_string()),
        values_json: "{}".to_string(),
        previous_offsets: HashMap::new(),
    }]);
    let mut ids = HistoryIds::default();
    for entry in &mut stack.undo_stack {
        ids.collect(&mut entry.undo_data);
    }
    let map = history_remap(&project, &renamed, &ids);
    assert_eq!(map.get(1, X[0]), Some(stepped.as_str()));
    assert_eq!(map.get(0, X[0]), None);
}

/// A split moves shots - and their ids - from one section to a new one. The
/// history then holds each moved id in BOTH sections, but never at the same
/// time, so nothing is renamed: an undo of the split after a reload must give
/// the shots back the ids they had.
#[test]
fn an_id_that_moved_between_sections_is_not_renamed() {
    let mut before = Project::new("Split".to_string(), None);
    before.add_section(copy_data());
    let mut after = before.clone();
    let mut tail = after.get_section(0).unwrap().clone();
    tail.id = 1;
    tail.shots.drain(..1);
    after.get_section_mut(0).unwrap().shots.truncate(1);
    after.sections.push(tail);
    after.next_section_id = 2;
    after.rebuild_section_index();
    after.rebuild_shot_uuid_index();

    let mut live = after.clone();
    let renamed = live.make_shot_uuids_project_unique();
    assert!(renamed.is_empty());

    let mut stack = stack_of(vec![UndoData::ProjectSnapshotPair {
        before: serde_json::to_string(&before).unwrap(),
        after: serde_json::to_string(&after).unwrap(),
    }]);
    let untouched = serde_json::to_string(&stack.undo_entries().collect::<Vec<_>>()).unwrap();
    assert_eq!(stack.translate_shot_uuids(&live, &renamed), 0);
    assert_eq!(
        serde_json::to_string(&stack.undo_entries().collect::<Vec<_>>()).unwrap(),
        untouched
    );
}

/// History saved after an earlier heal: section 1 is gone live, and the
/// history holds its shot under BOTH the old id (an entry from before that
/// heal) and the derived one (an entry from after). Both must name one shot.
#[test]
fn an_old_id_follows_the_derived_one_the_history_already_holds() {
    let mut live = Project::new("Gone".to_string(), None);
    live.add_section(copy_data());
    let renamed = live.make_shot_uuids_project_unique();
    let derived = new_id(X[0]);
    let entry = |uuid: &str| UndoData::PreviousValues {
        section_id: 1,
        shot_id: Some(100_000),
        shot_uuid: Some(uuid.to_string()),
        values_json: "{}".to_string(),
        previous_offsets: HashMap::new(),
    };
    let mut stack = stack_of(vec![entry(X[0]), entry(&derived)]);

    stack.translate_shot_uuids(&live, &renamed);

    for entry in stack.undo_entries() {
        let UndoData::PreviousValues { shot_uuid, .. } = &entry.undo_data else {
            panic!("previous values");
        };
        assert_eq!(shot_uuid.as_deref(), Some(derived.as_str()));
    }
}
