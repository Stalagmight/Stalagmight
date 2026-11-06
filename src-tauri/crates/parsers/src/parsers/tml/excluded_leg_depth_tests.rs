//! [#576 W27] A dropped excluded leg in the middle of a section keeps the depth
//! the file records at the station after it. Found by the tml metres-vs-feet
//! fuzzer (seed 363281): the feet file's rounded lengths cut the section at a
//! junction where the metric one did not, so an excluded shot led a section in
//! one import (its end depth carried, #576 NF-TML-2) and sat mid-section in
//! the other, where the next shot's "same as previous" `DepthIn` resolved to
//! the kept shot BEFORE the dropped one - a depth 3 cm off its own station.

use super::{convert_shots_to_shot_data, schema, TmlShot, TmlShotType};

fn shot(id: i64, depth: f64, excluded: bool, shot_type: TmlShotType) -> TmlShot {
    TmlShot {
        uuid: None,
        id,
        from_id: id - 1,
        shot_type,
        azimuth: 90.0,
        length: 20.0,
        depth,
        depth_in: schema::DEPTH_IN_SAME_AS_PREVIOUS,
        inclination: 0.0,
        left: 0.0,
        right: 0.0,
        up: 0.0,
        down: 0.0,
        comment: None,
        explorer: None,
        date: None,
        color: "#000000".to_string(),
        excluded,
        latitude: 0.0,
        longitude: 0.0,
        closure_to_id: -1,
        section: "Test".to_string(),
    }
}

fn depths(shots: &[TmlShot], include_excluded: bool) -> Vec<(f64, f64)> {
    convert_shots_to_shot_data(shots, 1.5, include_excluded)
        .iter()
        .map(|d| (d.depth_in, d.depth_out))
        .collect()
}

#[test]
fn a_dropped_leg_mid_section_keeps_the_depth_the_next_shot_starts_at() {
    // 10.0 -> excluded leg to 10.5 -> a "same as previous" shot to 12.0.
    let chain = [
        shot(1, 10.0, false, TmlShotType::Real),
        shot(2, 10.5, true, TmlShotType::Real),
        shot(3, 12.0, false, TmlShotType::Real),
    ];
    assert_eq!(depths(&chain, false), [(1.5, 10.0), (10.5, 12.0)]);
    // The same station depth the include-excluded import reads ...
    assert_eq!(depths(&chain, true)[2], (10.5, 12.0));
    // ... and the one the dropped leg gives when it leads the section.
    assert_eq!(depths(&chain[1..], false), [(10.5, 12.0)]);
}

#[test]
fn a_closure_row_after_the_dropped_leg_is_no_leg_and_a_recorded_depth_in_stands() {
    let mut recorded = shot(4, 12.0, false, TmlShotType::Real);
    recorded.depth_in = 11.0;
    let chain = [
        shot(1, 10.0, false, TmlShotType::Real),
        shot(2, 10.5, true, TmlShotType::Real),
        shot(3, 30.0, false, TmlShotType::Closure),
        recorded,
    ];
    assert_eq!(depths(&chain, false), [(1.5, 10.0), (11.0, 12.0)]);
    let mut inherits = chain;
    inherits[3].depth_in = schema::DEPTH_IN_SAME_AS_PREVIOUS;
    assert_eq!(depths(&inherits, false), [(1.5, 10.0), (10.5, 12.0)]);
}
