//! [#576 RV5-F4-7] Which shot an `InverseCommand` entry's undo and redo address,
//! for the load-time filter that drops entries whose shot no longer resolves.
//!
//! `shot_target` matched `InverseCommand(_)` as "no shot" apart from the legacy
//! `SetShotKind`: a wildcard over every command, so a shot-addressing command
//! recorded as an inverse compiled there and skipped the filter silently - the
//! defect class RV4-R-U3 named every `UndoData` arm to close. The inner match is
//! now exhaustive over `Command`; these cases pin its decisions. All data is
//! synthetic.

use super::UndoData;
use crate::core::Command;
use crate::models::{ShotData, ShotKind, ShotUpdates, StationRef};

type Target = Option<(u32, Option<String>, u32)>;

/// `(undo target, redo target)` of `inverse` recorded as an `InverseCommand`.
fn targets(inverse: Command) -> (Target, Target) {
    let data = UndoData::InverseCommand(inverse);
    let own = |t: Option<(u32, Option<&str>, u32)>| t.map(|(s, u, i)| (s, u.map(str::to_owned), i));
    (own(data.shot_target()), own(data.redo_shot_target()))
}

#[test]
fn an_inverse_that_acts_on_a_shot_by_id_is_checked_by_the_load_filter() {
    let cases = [
        (
            "SetShotKind",
            targets(Command::SetShotKind {
                section_id: 1,
                shot_id: 3,
                kind: ShotKind::Jump,
            }),
            (Some((1, None, 3)), Some((1, None, 3))),
        ),
        (
            "UpdateShot",
            targets(Command::UpdateShot {
                section_id: 1,
                shot_id: 3,
                updates: ShotUpdates {
                    length: Some(4.0),
                    ..ShotUpdates::default()
                },
            }),
            (Some((1, None, 3)), Some((1, None, 3))),
        ),
        // The undo deletes the shot (needs it); the redo re-creates it (absent by
        // construction on the redo stack), like `CreatedShotSnapshot`.
        (
            "DeleteShot",
            targets(Command::DeleteShot {
                section_id: 1,
                shot_id: 3,
            }),
            (Some((1, None, 3)), None),
        ),
        (
            "CreateShot",
            targets(Command::CreateShot {
                section_id: 1,
                shot: ShotData::default(),
                after_shot_id: Some(2),
                before_shot_id: None,
            }),
            (None, None),
        ),
        (
            "SetStationComment",
            targets(Command::SetStationComment {
                station_ref: StationRef::new(1, 3),
                comment: None,
            }),
            (None, None),
        ),
    ];
    let mut wrong = Vec::new();
    for (name, got, want) in &cases {
        println!("InverseCommand({name}): (undo, redo) target = {got:?}, expected {want:?}");
        if got != want {
            wrong.push(*name);
        }
    }
    assert!(wrong.is_empty(), "misclassified inverses: {wrong:?}");
}
