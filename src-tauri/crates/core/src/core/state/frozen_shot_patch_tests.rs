//! [#576 RV6b-R5] What an edit of a FROZEN shot may change.
//!
//! A shot in an adjusted survey network (a loop anchored by two datums) is
//! frozen: its geometry - azimuths, length, depths, pitches - is what the network
//! is solved against, so an update that carries any geometry field is refused.
//! Edit Shot and Edit Section send only the fields that changed (#576 F32-EDIT),
//! so a comment-only or LRUD-only edit of a frozen shot carries no geometry field
//! and goes through. Before that change both dialogs sent every geometry field on
//! every Save, and such an edit was refused with "Cannot edit geometry of a frozen
//! shot". Both halves are pinned here: the non-geometry patches succeed and land,
//! and any patch carrying a geometry field is still refused - even one that
//! re-sends the stored value - and changes nothing.
//!
//! All data is synthetic, built in memory.

use super::AppState;
use crate::core::Command;
use crate::models::{
    GpsCoordinates, Project, SectionData, SectionId, ShotData, ShotId, ShotUpdates, StationMerge,
    StationRef, ZeroDatum,
};
use crate::test_fixtures::state_holding;

/// A two-section loop (both ends of A merged onto both ends of B) anchored by a
/// datum at each section's head: every shot is frozen. Returns the state, the
/// first shot of A and its stored azimuth.
fn frozen_shot() -> (AppState, SectionId, ShotId, f64) {
    let shot = |length: f64| ShotData {
        azimuth_in: 45.0,
        azimuth_out: 225.0,
        length,
        ..Default::default()
    };
    let mut project = Project::new("CaveZ".to_owned(), None);
    let a = project
        .add_section(SectionData {
            color: "#ff0000".to_owned(),
            shots: vec![shot(5.0), shot(7.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            color: "#00ff00".to_owned(),
            shots: vec![shot(5.0), shot(7.0)],
            ..Default::default()
        })
        .id;
    for station in [0, 2] {
        project.station_merges.push(StationMerge {
            station_a: StationRef::new(a, station).into(),
            station_b: StationRef::new(b, station).into(),
            merged_at: chrono::Utc::now(),
        });
    }
    for (section, lat) in [(a, 45.0), (b, 46.0)] {
        project.zero_datums.push(ZeroDatum {
            station_ref: StationRef::new(section, 0).into(),
            gps: GpsCoordinates::new(lat, -73.0, 100.0).unwrap(),
        });
    }
    let target = &project.sections[0].shots[0];
    let (shot_id, azimuth) = (target.id, target.azimuth_in);
    assert!(
        project.is_shot_frozen_by_id(a, shot_id),
        "precondition: the loop and its two datums freeze the shot"
    );
    let state = state_holding(project);
    (state, a, shot_id, azimuth)
}

fn update(state: &AppState, section_id: SectionId, shot_id: ShotId, updates: ShotUpdates) -> bool {
    let result = state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates,
    });
    if let Some(error) = &result.error {
        println!("  refused: {error}");
    }
    result.success
}

fn stored(state: &AppState, section_id: SectionId, shot_id: ShotId) -> crate::models::Shot {
    state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .find(|s| s.id == section_id)
        .unwrap()
        .shots
        .iter()
        .find(|s| s.id == shot_id)
        .unwrap()
        .clone()
}

#[test]
fn a_comment_or_lrud_edit_of_a_frozen_shot_goes_through() {
    let (state, section_id, shot_id, _) = frozen_shot();
    let comment = update(
        &state,
        section_id,
        shot_id,
        ShotUpdates {
            comment: Some(Some("CommentA".to_owned())),
            ..Default::default()
        },
    );
    let lrud = update(
        &state,
        section_id,
        shot_id,
        ShotUpdates {
            left: Some(Some(1.0)),
            right: Some(Some(2.0)),
            up: Some(Some(0.5)),
            down: Some(Some(0.25)),
            ..Default::default()
        },
    );
    let shot = stored(&state, section_id, shot_id);
    println!(
        "frozen shot: comment-only edit accepted: {comment}; LRUD-only edit accepted: {lrud}; \
         stored comment {:?}, LRUD {:?}",
        shot.comment,
        (shot.left, shot.right, shot.up, shot.down)
    );
    assert!(comment, "a comment-only edit touches no geometry");
    assert!(lrud, "an LRUD-only edit touches no geometry");
    assert_eq!(shot.comment.as_deref(), Some("CommentA"));
    assert_eq!(
        (shot.left, shot.right, shot.up, shot.down),
        (Some(1.0), Some(2.0), Some(0.5), Some(0.25))
    );
}

#[test]
fn a_geometry_edit_of_a_frozen_shot_is_still_refused() {
    let (state, section_id, shot_id, azimuth) = frozen_shot();
    let before = stored(&state, section_id, shot_id);
    let cases = [
        (
            "a new length",
            ShotUpdates {
                length: Some(9.0),
                ..Default::default()
            },
        ),
        (
            "the stored azimuth re-sent",
            ShotUpdates {
                azimuth_in: Some(azimuth),
                ..Default::default()
            },
        ),
        (
            "a whole row with a comment (what the dialogs used to send)",
            ShotUpdates {
                azimuth_in: Some(before.azimuth_in),
                azimuth_out: Some(before.azimuth_out),
                length: Some(before.length),
                depth_in: Some(before.depth_in),
                depth_out: Some(before.depth_out),
                comment: Some(Some("CommentB".to_owned())),
                ..Default::default()
            },
        ),
    ];
    let mut accepted = Vec::new();
    for (what, updates) in cases {
        println!("frozen shot, {what}:");
        if update(&state, section_id, shot_id, updates) {
            accepted.push(what);
        }
    }
    let unchanged = serde_json::to_value(stored(&state, section_id, shot_id)).unwrap()
        == serde_json::to_value(&before).unwrap();
    println!("geometry patches accepted: {accepted:?}; shot unchanged: {unchanged}");
    assert!(
        accepted.is_empty(),
        "every geometry patch is refused: {accepted:?}"
    );
    assert!(
        unchanged,
        "a refused patch changes nothing, comment included"
    );
}
