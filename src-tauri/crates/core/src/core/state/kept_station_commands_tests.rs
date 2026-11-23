//! Kept and archived stations through `AppState::execute` (plan 2026-11-23,
//! RS 4.1, 4.2, 5.2, 5.3, 6 and 7): Merge Stations with the dialog's keep
//! and renames, Detach with renames, Create Loop Closure keeping an end and
//! Remove Closure bringing it back, the history rows naming the kept
//! station, the identity notice of the paths without a dialog, the
//! read-only previews, and the commands' serde forms (a batch written
//! before the fields existed still reads). All data is synthetic.

use super::topology_kit::{
    history, line_at, line_from, open, project, refused, run, shot, slot, st,
};
use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::survey::Project;
use crate::models::{ClosureStrategy, LineId, StationId, StationRef, StationRename};

fn rename(station_id: StationId, name: &str) -> StationRename {
    StationRename {
        station_id,
        name: name.to_string(),
    }
}

fn merge_keeping(
    a: StationRef,
    b: StationRef,
    keep: StationId,
    renames: Option<Vec<StationRename>>,
) -> Command {
    Command::MergeStations {
        station_a: a,
        station_b: b,
        keep: Some(keep),
        renames,
    }
}

fn shown_name(state: &AppState, at: StationRef) -> String {
    let p = project(state);
    p.slot_name(at).unwrap().to_string()
}

/// A (0, 1, 2; east 2 x 5 m) at the origin and B (0, 1; north 4 m) apart.
fn two_lines() -> (Project, LineId, LineId) {
    let mut p = Project::new("Kept", None);
    let a = line_at(
        &mut p,
        "A",
        [0.0, 0.0],
        vec![shot(90.0, 5.0), shot(90.0, 5.0)],
    );
    let b = line_at(&mut p, "B", [50.0, 50.0], vec![shot(0.0, 4.0)]);
    (p, a, b)
}

/// A loop: A runs east 4 x 10 m (0..4); B leaves A's 0 north 10 m, east
/// 40 m and south 10 m (1..3), its 3 on A's 4's spot, unmerged.
fn loop_lines() -> (Project, LineId, LineId) {
    let mut p = Project::new("Loop", None);
    let a = line_at(
        &mut p,
        "A",
        [0.0, 0.0],
        (0..4).map(|_| shot(90.0, 10.0)).collect(),
    );
    let a0 = st(&p, a, 0);
    let b = line_from(
        &mut p,
        "B",
        a0,
        vec![shot(0.0, 10.0), shot(90.0, 40.0), shot(180.0, 10.0)],
    );
    (p, a, b)
}

fn identities(r: &CommandResult) -> Vec<(StationId, String, String)> {
    r.identities
        .as_ref()
        .map(|n| {
            n.lettered
                .iter()
                .map(|l| (l.station_id, l.from.clone(), l.to.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// RS 4.1: the dialog's keep is the station shown on every line, the other
/// is archived, and the history row says which ("Merge A / 2 into B / 0
/// (keeps 0)"). A then shows B's 0 at its end with B's dot beside its own
/// 0: a foreign slot counts for no uniqueness (RS-FX5), so the merge needs
/// no answer with or without the dialog and letters nothing. A rename the
/// merge's collisions do not ask for (here of the kept station, with none)
/// refuses and changes nothing (RS-FX9 item 1: a dialog answers only its
/// collisions); Set Station Name renames the kept station on every line.
#[test]
fn a_merge_keeps_the_dialogs_station_and_answers_its_collisions() {
    let (p, a, b) = two_lines();
    let b0 = st(&p, b, 0);
    let opened = open(&p);
    let state = &opened.state;

    let r = run(state, merge_keeping(slot(a, 2), slot(b, 0), b0, None));
    assert_eq!(
        history(state)[0].1,
        "Merge A / 2 into B / 0 (keeps 0)",
        "the row names the kept station"
    );
    assert!(identities(&r).is_empty(), "{:?}", r.identities);
    assert_eq!(shown_name(state, slot(a, 2)), "0");
    assert_eq!(shown_name(state, slot(a, 0)), "0", "A's own 0 stays");
    assert!(state.undo().success);

    let r = run(
        state,
        merge_keeping(slot(a, 2), slot(b, 0), b0, Some(Vec::new())),
    );
    assert!(r.identities.is_none(), "{:?}", r.identities);
    assert!(state.undo().success);

    let reason = refused(
        state,
        merge_keeping(slot(a, 2), slot(b, 0), b0, Some(vec![rename(b0, "J")])),
    );
    assert!(
        reason.contains("is in none of the name collisions"),
        "{reason}"
    );
    assert_eq!(shown_name(state, slot(b, 0)), "0");
    let r = run(
        state,
        merge_keeping(slot(a, 2), slot(b, 0), b0, Some(Vec::new())),
    );
    assert!(r.identities.is_none(), "{:?}", r.identities);
    run(
        state,
        Command::SetStationName {
            station_ref: slot(b, 0),
            name: "J".to_string(),
        },
    );
    for at in [slot(a, 2), slot(b, 0)] {
        assert_eq!(shown_name(state, at), "J");
    }
    assert_eq!(shown_name(state, slot(a, 0)), "0");
    // The wire: A's slot is foreign, shows B's 0's Station ID, and the node
    // keeps A's own 2 as a record.
    let wire = state.project_state_wire().unwrap();
    let at = wire
        .stations
        .iter()
        .find(|s| s.station_ref == slot(a, 2))
        .unwrap();
    assert_eq!(at.shown_id(), b0);
    assert!(at.is_foreign());
    assert_eq!(at.archived.as_ref().map(Vec::len), Some(1));
}

/// RS 4.2: Detach brings A's own station back; its preview says so first,
/// and the pair form carries renames.
#[test]
fn detach_brings_the_archived_station_back_with_the_dialogs_answers() {
    let (p, a, b) = two_lines();
    let (a1, a2, b0) = (st(&p, a, 1), st(&p, a, 2), st(&p, b, 0));
    let opened = open(&p);
    let state = &opened.state;
    run(
        state,
        merge_keeping(slot(a, 2), slot(b, 0), b0, Some(Vec::new())),
    );
    // A's own 2 is archived: the name is free on A.
    run(
        state,
        Command::SetStationName {
            station_ref: slot(a, 1),
            name: "2".to_string(),
        },
    );
    let preview = state.preview_detach_line(slot(a, 2), a, None).unwrap();
    assert!(preview.restored);
    assert_eq!(preview.station_id, a2);
    assert_eq!(preview.collisions.len(), 1, "{preview:?}");
    let owner = state.preview_station_identity(slot(a, 2)).unwrap();
    assert!(owner.foreign);
    assert_eq!(owner.own_station.map(|s| s.id), Some(a2));

    let pair = |renames| Command::UnmergeStationPair {
        station_a: slot(b, 0),
        station_b: slot(a, 2),
        renames,
        rest_keep: None,
    };
    refused(state, pair(Some(Vec::new())));
    let r = run(state, pair(Some(vec![rename(a1, "1")])));
    assert!(r.identities.is_none());
    assert_eq!(shown_name(state, slot(a, 2)), "2");
    assert_eq!(shown_name(state, slot(a, 1)), "1");
    assert_eq!(st(&project(state), a, 2), a2, "its own Station ID");
}

/// RS 5.2, 5.3, 6: a closure keeping A's 4 shows it on B's 3 (the history
/// row says so); Remove Closure's preview names what comes back and, once
/// the user reused the name, the collision; with renames an unanswered one
/// refuses, without them rule L letters it and the notice says so.
#[test]
fn a_closure_keeps_an_end_and_remove_brings_it_back() {
    let (p, a, b) = loop_lines();
    let (a4, b1, b3) = (st(&p, a, 4), st(&p, b, 1), st(&p, b, 3));
    let opened = open(&p);
    let state = &opened.state;

    let naming = state
        .preview_loop_closure(slot(b, 3), slot(a, 4), ClosureStrategy::Batch)
        .unwrap();
    assert_eq!(naming.allowed, [b3, a4]);
    assert_eq!(naming.default_keep, Some(b3), "B is the longer line");
    assert_eq!(naming.options.len(), 3);

    run(
        state,
        Command::CreateLoopClosure {
            station_a: slot(b, 3),
            station_b: slot(a, 4),
            closure_strategy: ClosureStrategy::Batch,
            keep: Some(a4),
            renames: Some(Vec::new()),
        },
    );
    assert_eq!(
        history(state)[0].1,
        "Create loop closure: B / 3 shown as A / 4"
    );
    assert_eq!(shown_name(state, slot(b, 3)), "4");
    let wire = state.project_state_wire().unwrap();
    let closure = &wire.project.loop_closures[0];
    let archive = closure.archived.as_ref().unwrap();
    assert_eq!((archive.station.id, archive.shown_as.id), (b3, a4));
    let id = closure.id;

    let preview = state.preview_remove_loop_closure(id).unwrap();
    assert_eq!(
        preview.comes_back.as_ref().map(|s| s.name.as_str()),
        Some("3")
    );
    assert!(preview.collisions.is_empty());

    run(
        state,
        Command::SetStationName {
            station_ref: slot(b, 1),
            name: "3".to_string(),
        },
    );
    let preview = state.preview_remove_loop_closure(id).unwrap();
    assert_eq!(preview.collisions.len(), 1);
    let remove = |renames| Command::RemoveLoopClosure {
        closure_id: id,
        renames,
    };
    refused(state, remove(Some(Vec::new())));
    let r = run(state, remove(None));
    assert_eq!(identities(&r), [(b3, "3".to_string(), "3a".to_string())]);
    assert!(state.undo().success);
    let r = run(state, remove(Some(vec![rename(b1, "1")])));
    assert!(r.identities.is_none());
    assert_eq!(shown_name(state, slot(b, 3)), "3");
}

/// The commands' serde forms: a batch written before the RS fields existed
/// still reads (every field defaults), and the fields round-trip.
#[test]
fn station_commands_read_with_and_without_the_rs_fields() {
    let old: Command = serde_json::from_str(
        r#"{"type":"merge_stations","station_a":{"line_id":1,"station_index":0},
            "station_b":{"line_id":2,"station_index":1}}"#,
    )
    .unwrap();
    assert!(matches!(
        old,
        Command::MergeStations {
            keep: None,
            renames: None,
            ..
        }
    ));
    let new = merge_keeping(slot(1, 0), slot(2, 1), 7, Some(vec![rename(3, "3a")]));
    let json = serde_json::to_value(&new).unwrap();
    assert_eq!(json["keep"], serde_json::json!(7));
    assert_eq!(
        json["renames"],
        serde_json::json!([{ "station_id": 3, "name": "3a" }])
    );
    let back: Command = serde_json::from_value(json).unwrap();
    assert!(matches!(
        back,
        Command::MergeStations {
            keep: Some(7),
            renames: Some(ref r),
            ..
        } if r.len() == 1
    ));
    let closure: Command =
        serde_json::from_str(r#"{"type":"remove_loop_closure","closure_id":4}"#).unwrap();
    assert!(matches!(
        closure,
        Command::RemoveLoopClosure {
            closure_id: 4,
            renames: None
        }
    ));
}
