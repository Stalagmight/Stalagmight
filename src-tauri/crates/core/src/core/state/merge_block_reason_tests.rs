//! [#576 NF-W7E-1] A refused merge says WHY as a code, not only as English.
//!
//! The merge tool and the move-onto-station snap route a refused merge to the
//! loop-closure dialog when the pair is connected or both sides are fixed by a
//! zero datum, and to a toast otherwise. They used to decide that by comparing
//! `ConnectivityResult::reason` with the English line "Both sections have fixed
//! coordinates", so rewording it would have silently sent a fixed pair to the
//! toast. The answer now carries `reason_code`; `reason` stays as display text.
//!
//! All data is synthetic.

use super::AppState;
use crate::core::Command;
use ts_rs::TS;

use crate::models::{ConnectivityResult, GpsCoordinates, MergeBlockReason, StationRef};
use crate::test_fixtures::{shot, state_with_project};

/// A project with two unconnected one-shot sections, A and B.
fn two_sections(name: &str) -> (AppState, u32, u32) {
    let state = state_with_project(name);
    let mut ids = Vec::new();
    for (color, length) in [("#ff0000", 10.0), ("#00ff00", 5.0)] {
        let created = state.execute(Command::CreateSection {
            name: None,
            surveyor: None,
            date: None,
            color: color.to_string(),
            direction: None,
            shots: vec![shot(length)],
        });
        ids.push(created.section_id.expect("section id"));
    }
    (state, ids[0], ids[1])
}

#[test]
fn a_pair_fixed_by_two_zero_datums_is_refused_with_its_code() {
    let (state, a, b) = two_sections("BothFixed");
    for (section, lat) in [(a, 45.0), (b, 46.0)] {
        state.execute(Command::SetZeroDatum {
            station_ref: StationRef::new(section, 0),
            coordinates: GpsCoordinates::new(lat, -90.0, 0.0).expect("gps"),
        });
    }

    let result = state
        .check_merge_connectivity(StationRef::new(a, 1), StationRef::new(b, 0))
        .expect("check");
    println!(
        "[NF-W7E-1] fixed pair: can_merge={} reason={:?} reason_code={:?}",
        result.can_merge, result.reason, result.reason_code
    );

    assert!(!result.can_merge);
    assert_eq!(
        result.reason_code,
        Some(MergeBlockReason::BothFixedCoordinates)
    );
    assert_eq!(
        result.reason.as_deref(),
        Some(MergeBlockReason::BothFixedCoordinates.message())
    );
}

#[test]
fn a_connected_pair_is_refused_with_its_code() {
    let (state, a, b) = two_sections("Loop");
    state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });

    let result = state
        .check_merge_connectivity(StationRef::new(a, 0), StationRef::new(b, 1))
        .expect("check");
    println!(
        "[NF-W7E-1] connected pair: can_merge={} reason={:?} reason_code={:?}",
        result.can_merge, result.reason, result.reason_code
    );

    assert!(!result.can_merge);
    assert_eq!(
        result.reason_code,
        Some(MergeBlockReason::CreatesSurveyLoop)
    );
    assert_eq!(result.reason.as_deref(), Some("Creates survey loop"));
}

#[test]
fn a_mergeable_pair_carries_no_code() {
    let (state, a, b) = two_sections("Free");
    let result = state
        .check_merge_connectivity(StationRef::new(a, 1), StationRef::new(b, 0))
        .expect("check");
    assert!(result.can_merge);
    assert_eq!(result.reason_code, None);
    assert_eq!(result.reason, None);
}

#[test]
fn the_code_has_a_stable_wire_name() {
    let wire = serde_json::to_value(MergeBlockReason::BothFixedCoordinates).expect("serialize");
    assert_eq!(wire, serde_json::json!("BOTH_FIXED_COORDINATES"));
    let wire = serde_json::to_value(MergeBlockReason::CreatesSurveyLoop).expect("serialize");
    assert_eq!(wire, serde_json::json!("CREATES_SURVEY_LOOP"));
}

/// `reason_code`'s TS type is spelled out (the enum has no TS export of its own,
/// so an inlined dependency would import a file that is never written): it must
/// name exactly the codes the enum serializes, or the frontend's routing switch
/// would miss one without tsc noticing.
#[test]
fn the_ts_union_names_every_code() {
    let decl = <ConnectivityResult as TS>::decl();
    let field = decl
        .split("reason_code: ")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .expect("reason_code in the TS declaration")
        .to_string();
    let mut expected: Vec<String> = MergeBlockReason::ALL
        .iter()
        .map(|code| serde_json::to_value(code).expect("serialize").to_string())
        .collect();
    expected.push("null".to_string());
    let mut declared: Vec<String> = field.split(" | ").map(|s| s.trim().to_string()).collect();
    expected.sort();
    declared.sort();
    assert_eq!(declared, expected, "TS declaration: {decl}");
}
