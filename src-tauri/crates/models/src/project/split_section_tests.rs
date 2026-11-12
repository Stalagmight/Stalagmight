//! Split Section Here (#335) on the model: positions, references, the
//! duplicated split station, credit and the least-squares gauge.

use chrono::NaiveDate;

use crate::{
    GpsCoordinates, Person, Project, SectionData, SectionId, ShotData, SplitSectionOptions,
    SplitSide, StationRef, SurveyCredit,
};

fn options(comment_side: SplitSide, datum_side: SplitSide) -> SplitSectionOptions {
    SplitSectionOptions {
        name_a: "Main-A".to_string(),
        name_b: "Main-B".to_string(),
        color_a: "#111111".to_string(),
        color_b: "#222222".to_string(),
        comment_side,
        datum_side,
    }
}

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..Default::default()
    }
}

/// `shot` reading `depth` at its start and one metre deeper at its end, so
/// a station's two readings differ.
fn sloped(azimuth: f64, length: f64, depth: f64) -> ShotData {
    ShotData {
        depth_in: depth,
        depth_out: depth + 1.0,
        ..shot(azimuth, length)
    }
}

/// "Main": four sloping shots bending east then north, plus a side section
/// whose start is merged to Main's station 2.
fn project() -> (Project, SectionId, SectionId) {
    let mut project = Project::new("Split".to_string(), None);
    let main = project
        .add_section(SectionData {
            name: Some("Main".to_string()),
            shots: vec![
                sloped(90.0, 10.0, 10.0),
                sloped(90.0, 5.0, 12.0),
                sloped(0.0, 7.0, 14.0),
                sloped(45.0, 3.0, 16.0),
            ],
            ..Default::default()
        })
        .id;
    let side = project
        .add_section(SectionData {
            name: Some("Side".to_string()),
            shots: vec![shot(180.0, 4.0)],
            ..Default::default()
        })
        .id;
    project
        .merge_stations(StationRef::new(main, 2), StationRef::new(side, 0))
        .unwrap();
    (project, main, side)
}

fn xy(project: &Project, section: SectionId, index: u32) -> (f64, f64) {
    let c = project
        .compute_station_raw_coords(StationRef::new(section, index))
        .unwrap();
    ((c.x * 1e6).round() / 1e6, (c.y * 1e6).round() / 1e6)
}

fn z(project: &Project, section: SectionId, index: u32) -> f64 {
    project
        .compute_station_raw_coords(StationRef::new(section, index))
        .unwrap()
        .z
}

/// Main is offset first, so a second half placed without the section's
/// own offset lands in the wrong place. The split station's two readings
/// differ (15 m arriving, 16 m leaving): the unsplit station averaged them,
/// and each copy now reads its own side.
#[test]
fn every_station_keeps_its_position_across_the_split() {
    let (mut project, main, _) = project();
    project.set_section_offset(main, (100.0, -40.0));
    let before: Vec<_> = (0..=4).map(|i| xy(&project, main, i)).collect();
    let before_z: Vec<_> = (0..=4).map(|i| z(&project, main, i)).collect();
    assert_eq!(before_z[3], 15.5, "setup: the unsplit station averages");

    let second = project
        .split_section_at_station(main, 3, &options(SplitSide::A, SplitSide::A))
        .unwrap();

    assert_eq!(project.get_section(main).unwrap().shots.len(), 3);
    assert_eq!(project.get_section(second).unwrap().shots.len(), 1);
    let after: Vec<_> = (0..=3)
        .map(|i| xy(&project, main, i))
        .chain(std::iter::once(xy(&project, second, 1)))
        .collect();
    assert_eq!(after, before);
    assert_eq!(xy(&project, second, 0), before[3], "the copy is co-located");

    for i in 0..3 {
        assert_eq!(z(&project, main, i), before_z[i as usize], "station {i}");
    }
    assert_eq!(z(&project, second, 1), before_z[4]);
    assert_eq!(
        z(&project, main, 3),
        15.0,
        "the first half's end reads depth_out"
    );
    assert_eq!(
        z(&project, second, 0),
        16.0,
        "the second half's start reads depth_in"
    );
}

fn credit(surveyor: &str, explorer: Option<&str>, day: u32) -> SurveyCredit {
    SurveyCredit {
        surveyors: Person::named(surveyor).into_iter().collect(),
        explorers: explorer.and_then(Person::named).into_iter().collect(),
        date: NaiveDate::from_ymd_opt(2026, 3, day),
    }
}

/// Every shot keeps its own credit exactly, on both halves: survey credit
/// lives on the shot, and the second half must not be given a section-level
/// template that would credit its uncredited shots.
#[test]
fn every_shot_keeps_its_own_credit_across_the_split() {
    let (mut project, main, _) = project();
    let credits = [
        Some(credit("Bob", None, 2)),
        Some(credit("Ann", Some("Cid"), 3)),
        None,
        Some(credit("Ann", None, 4)),
    ];
    for (shot, credit) in project
        .get_section_mut(main)
        .unwrap()
        .shots
        .iter_mut()
        .zip(&credits)
    {
        shot.set_credit(credit.clone().unwrap_or_default());
    }
    let before: Vec<(Option<String>, Option<SurveyCredit>)> = project
        .get_section(main)
        .unwrap()
        .shots
        .iter()
        .map(|s| (s.uuid.clone(), s.credit.as_deref().cloned()))
        .collect();

    let second = project
        .split_section_at_station(main, 2, &options(SplitSide::A, SplitSide::A))
        .unwrap();

    let after: Vec<(Option<String>, Option<SurveyCredit>)> = [main, second]
        .iter()
        .flat_map(|id| project.get_section(*id).unwrap().shots.iter())
        .map(|s| (s.uuid.clone(), s.credit.as_deref().cloned()))
        .collect();
    assert_eq!(
        after, before,
        "every shot keeps its own credit, the empty one included"
    );
}

/// A reversed four-shot section whose gauge pair names `shots[shot]` and
/// `shots[neighbour]` (positions after the reverse).
fn reversed_with_gauge(shot: usize, neighbour: usize) -> (Project, SectionId) {
    let mut project = Project::new("Gauge".to_string(), None);
    let main = project
        .add_section(SectionData {
            shots: (0..4_u8)
                .map(|i| self::shot(90.0, 5.0 + f64::from(i)))
                .collect(),
            ..Default::default()
        })
        .id;
    assert!(project.reverse_section(main, false));
    let section = project.get_section_mut(main).unwrap();
    section.gauge_shot_uuid = section.shots[shot].uuid.clone();
    section.gauge_neighbour_uuid = section.shots[neighbour].uuid.clone();
    (project, main)
}

fn gauge_pair(project: &Project, section: SectionId) -> (Option<String>, Option<String>) {
    let s = project.get_section(section).unwrap();
    (s.gauge_shot_uuid.clone(), s.gauge_neighbour_uuid.clone())
}

/// The gauge station of `section` as a world xy, read the way the solver
/// reads it.
fn gauge_xy(project: &Project, section: SectionId) -> (f64, f64) {
    let index = Project::gauge_index(project.get_section(section).unwrap());
    xy(project, section, index as u32)
}

#[test]
fn a_gauge_pair_past_the_cut_moves_to_the_second_half_with_the_parity() {
    let (mut project, main) = reversed_with_gauge(3, 2);
    let pair = gauge_pair(&project, main);
    assert!(pair.0.is_some() && pair.1.is_some());
    let station = gauge_xy(&project, main);

    let second = project
        .split_section_at_station(main, 2, &options(SplitSide::A, SplitSide::A))
        .unwrap();

    assert!(project.get_section(main).unwrap().reverse_parity);
    assert!(project.get_section(second).unwrap().reverse_parity);
    assert_eq!(
        gauge_pair(&project, main),
        (None, None),
        "the first half no longer holds the gauge shots"
    );
    assert_eq!(gauge_pair(&project, second), pair);
    assert_eq!(gauge_xy(&project, second), station);
}

/// The cut takes the pair's neighbour shot into the other half. The half that
/// holds the gauge STATION re-captures a pair naming it, instead of clearing
/// the gauge and moving it to the index fallback.
#[test]
fn a_gauge_pair_the_cut_separates_is_recaptured_on_the_same_station() {
    // shots[2] facing away from shots[1]: station 3, past the cut at 2.
    let (mut project, main) = reversed_with_gauge(2, 1);
    let station = gauge_xy(&project, main);

    let second = project
        .split_section_at_station(main, 2, &options(SplitSide::A, SplitSide::A))
        .unwrap();

    assert!(project.get_section(second).unwrap().reverse_parity);
    assert_eq!(gauge_pair(&project, main), (None, None));
    let (shot, _) = gauge_pair(&project, second);
    assert!(shot.is_some(), "the second half re-captured the gauge");
    assert_eq!(gauge_xy(&project, second), station);

    // shots[1] facing away from shots[2]: station 1, before the cut at 2.
    let (mut project, main) = reversed_with_gauge(1, 2);
    let station = gauge_xy(&project, main);
    project
        .split_section_at_station(main, 2, &options(SplitSide::A, SplitSide::A))
        .unwrap();
    assert!(gauge_pair(&project, main).0.is_some());
    assert_eq!(gauge_xy(&project, main), station);
}

#[test]
fn a_gauge_pair_before_the_cut_stays_on_the_first_half() {
    let (mut project, main) = reversed_with_gauge(0, 1);
    let pair = gauge_pair(&project, main);

    let second = project
        .split_section_at_station(main, 2, &options(SplitSide::A, SplitSide::A))
        .unwrap();

    assert_eq!(gauge_pair(&project, main), pair);
    assert_eq!(gauge_pair(&project, second), (None, None));
}

/// A split ON the gauge station pins both co-located copies.
#[test]
fn a_split_on_the_gauge_station_pins_both_copies() {
    // shots[2] facing away from shots[3]: station 2.
    let (mut project, main) = reversed_with_gauge(2, 3);
    let station = gauge_xy(&project, main);
    let second = project
        .split_section_at_station(main, 2, &options(SplitSide::A, SplitSide::A))
        .unwrap();
    assert_eq!(gauge_xy(&project, main), station);
    assert_eq!(gauge_xy(&project, second), station);
}

#[test]
fn the_first_and_last_station_cannot_be_split_and_nothing_changes() {
    let (mut project, main, _) = project();
    let sections = project.sections.len();
    for index in [0, 4, 5] {
        assert!(project
            .split_section_at_station(main, index, &options(SplitSide::A, SplitSide::A))
            .is_err());
    }
    assert!(project
        .split_section_at_station(main, 2, &options(SplitSide::A, SplitSide::Both))
        .is_err());
    assert_eq!(project.sections.len(), sections);
    assert_eq!(project.get_section(main).unwrap().shots.len(), 4);
}

#[test]
fn a_reference_past_the_split_follows_its_station_into_the_second_half() {
    let (mut project, main, _) = project();
    let key = project.key_for_ref(StationRef::new(main, 3)).unwrap();
    project
        .station_comments
        .insert(key.clone(), "past the split".to_string());
    project.station_offsets.insert(key.clone(), (0.5, 0.0, 0.0));
    project.station_names.insert(key, "S7".to_string());
    let before_name = project.key_for_ref(StationRef::new(main, 1)).unwrap();
    project.station_names.insert(before_name, "S5".to_string());

    let second = project
        .split_section_at_station(main, 2, &options(SplitSide::A, SplitSide::A))
        .unwrap();

    let moved = StationRef::new(second, 1);
    assert_eq!(
        project.station_comment_for_ref(moved).map(String::as_str),
        Some("past the split")
    );
    assert_eq!(project.station_offset_for_ref(moved), Some((0.5, 0.0, 0.0)));
    assert_eq!(
        project.station_name_for_ref(moved).map(String::as_str),
        Some("S7")
    );
    assert_eq!(
        project
            .station_name_for_ref(StationRef::new(main, 1))
            .map(String::as_str),
        Some("S5"),
        "a reference before the cut stays"
    );
    assert!(project.check_structural_integrity().is_empty());
}

#[test]
fn a_merge_on_the_split_station_ties_both_halves() {
    let (mut project, main, side) = project();
    let second = project
        .split_section_at_station(main, 2, &options(SplitSide::A, SplitSide::A))
        .unwrap();

    let tied: Vec<(StationRef, StationRef)> = project
        .station_merges
        .iter()
        .map(|m| {
            (
                project.resolve_key(&m.station_a).unwrap(),
                project.resolve_key(&m.station_b).unwrap(),
            )
        })
        .collect();
    let side_start = StationRef::new(side, 0);
    assert!(tied.contains(&(StationRef::new(main, 2), side_start)));
    assert!(tied.contains(&(StationRef::new(second, 0), side_start)));
}

#[test]
fn comment_and_datum_go_to_the_chosen_half() {
    for (comment_side, on_a, on_b) in [
        (SplitSide::A, true, false),
        (SplitSide::B, false, true),
        (SplitSide::Both, true, true),
    ] {
        let (mut project, main, _) = project();
        let key = project.key_for_ref(StationRef::new(main, 2)).unwrap();
        project
            .station_comments
            .insert(key, "split here".to_string());
        project
            .set_zero_datum(
                StationRef::new(main, 2),
                GpsCoordinates {
                    latitude: 20.0,
                    longitude: -87.0,
                    depth_reference: 0.0,
                },
            )
            .unwrap();

        let second = project
            .split_section_at_station(main, 2, &options(comment_side, SplitSide::B))
            .unwrap();

        let comment_on = |section: SectionId, index: u32| {
            project
                .station_comment_for_ref(StationRef::new(section, index))
                .is_some()
        };
        assert_eq!(comment_on(main, 2), on_a, "{comment_side:?}");
        assert_eq!(comment_on(second, 0), on_b, "{comment_side:?}");
        assert_eq!(project.zero_datums.len(), 1);
        assert_eq!(
            project.resolve_key(&project.zero_datums[0].station_ref),
            Some(StationRef::new(second, 0))
        );
    }
}
