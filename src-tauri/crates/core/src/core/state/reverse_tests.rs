//! `Command::ReverseSection` at the state layer (issue #24).
//!
//! The model-level transform is covered in `models::project::reverse`; these
//! tests pin the command's contract: geometry is invariant through the full
//! execute pipeline (including the reconcile epilogue), undo restores exactly,
//! frozen shots do not block it, and warnings are re-emitted.

use std::collections::HashMap;

use super::{AppState, VirtualShotMode};
use crate::core::{Command, ReversedSection, UndoData};
use crate::models::{
    Coordinates, DivePlanSnapshot, GpsCoordinates, ImportPlacement, ImportSectionConfig,
    MarkedPoint, Project, SectionId, SectionUpdates, ShotData, StationRef,
};

/// One expected [`ReversedSection`] entry, so the assertions below read as
/// "section X, mirrored against N shots" rather than a struct literal each time.
fn reversed_entry(section_id: SectionId, shot_count: u32) -> ReversedSection {
    ReversedSection {
        section_id,
        shot_count,
    }
}

fn shot(azimuth: f64, length: f64, depth_in: f64, depth_out: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in,
        depth_out,
        ..Default::default()
    }
}

fn abs_coord_map(project: &Project) -> HashMap<StationRef, Coordinates> {
    project
        .compute_station_coordinates()
        .into_iter()
        .map(|s| (s.station_ref, s.coordinates))
        .collect()
}

fn assert_coords_close(a: Coordinates, b: Coordinates) {
    let eps = 1e-6;
    assert!(
        (a.x - b.x).abs() < eps && (a.y - b.y).abs() < eps && (a.z - b.z).abs() < eps,
        "coordinate moved: {a:?} vs {b:?}"
    );
}

/// `shot`, plus per-shot LRUD and pitch that are distinct from each other and
/// from every other shot's.
///
/// Every field here is one a reverse must either move, mirror or negate, and all
/// of them default to `None` - so a fixture built on `shot()` alone makes any
/// assertion about them vacuously true (that is exactly how the import path's
/// "left/right is never swapped" claim went unguarded). Neither LRUD nor pitch
/// feeds the coordinate walk (`horizontal_length` is derived from `length` and
/// the depth delta), so populating them cannot perturb the geometry assertions.
fn shot_with_lrud(azimuth: f64, length: f64, depth_in: f64, depth_out: f64, n: f64) -> ShotData {
    ShotData {
        left: Some(n),
        right: Some(n + 0.5),
        up: Some(n + 1.0),
        down: Some(n + 1.5),
        pitch_in: Some(n + 2.0),
        pitch_out: Some(-(n + 3.0)),
        ..shot(azimuth, length, depth_in, depth_out)
    }
}

/// A project with one three-shot section that turns a corner and changes depth,
/// so a botched reverse shows up as a moved station rather than a coincidence.
fn state_with_section() -> (AppState, SectionId) {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Reverse".to_string(),
        location: None,
    });
    let created = state.execute(Command::CreateSection {
        name: Some("Main line".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots: vec![
            shot_with_lrud(0.0, 10.0, 0.0, 2.0, 1.0),
            shot_with_lrud(90.0, 5.0, 2.0, 2.0, 10.0),
            shot_with_lrud(210.0, 7.5, 2.0, 1.0, 20.0),
        ],
    });
    let section_id = created.section_id.unwrap();
    (state, section_id)
}

#[test]
fn reverse_keeps_every_station_where_it_was() {
    let (state, section_id) = state_with_section();
    let before = abs_coord_map(&state.get_project_state().unwrap());

    let result = state.execute(Command::ReverseSection {
        section_id,
        swap_left_right: false,
    });
    assert!(result.success, "reverse failed: {:?}", result.error);

    let after = abs_coord_map(&state.get_project_state().unwrap());
    assert_eq!(before.len(), after.len(), "station count changed");

    // Station i is now station N - i, at the same place.
    let n = 3;
    for i in 0..=n {
        assert_coords_close(
            before[&StationRef::new(section_id, i)],
            after[&StationRef::new(section_id, n - i)],
        );
    }
}

#[test]
fn reverse_reverses_the_shot_order() {
    let (state, section_id) = state_with_section();
    let before = state.get_project_state().unwrap();
    let lengths: Vec<_> = before
        .get_section(section_id)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.length)
        .collect();

    assert!(
        state
            .execute(Command::ReverseSection {
                section_id,
                swap_left_right: false,
            })
            .success
    );

    let after = state.get_project_state().unwrap();
    let reversed: Vec<_> = after
        .get_section(section_id)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.length)
        .collect();
    let mut expected = lengths;
    expected.reverse();
    assert_eq!(reversed, expected);
}

#[test]
fn undo_restores_the_original_section_exactly() {
    let (state, section_id) = state_with_section();
    let before = state.get_project_state().unwrap();
    let original = before.get_section(section_id).unwrap().clone();
    let coords_before = abs_coord_map(&before);

    assert!(
        state
            .execute(Command::ReverseSection {
                section_id,
                swap_left_right: true,
            })
            .success
    );
    assert!(state.undo().success, "reverse must be undoable");

    let restored_project = state.get_project_state().unwrap();
    let restored = restored_project.get_section(section_id).unwrap();
    assert_eq!(restored.shots.len(), original.shots.len());
    for (a, b) in restored.shots.iter().zip(&original.shots) {
        assert_eq!(a.id, b.id);
        assert_eq!(a.uuid, b.uuid);
        assert_eq!(a.azimuth_in, b.azimuth_in);
        assert_eq!(a.azimuth_out, b.azimuth_out);
        assert_eq!(a.depth_in, b.depth_in);
        assert_eq!(a.depth_out, b.depth_out);
        assert_eq!(a.left, b.left);
        assert_eq!(a.right, b.right);
        // The fields `swap_left_right: true` and the pitch negation actually
        // touch - without these the test's "exactly" only covered id/uuid/
        // azimuth/depth.
        assert_eq!(a.up, b.up);
        assert_eq!(a.down, b.down);
        assert_eq!(a.pitch_in, b.pitch_in);
        assert_eq!(a.pitch_out, b.pitch_out);
    }
    for (sr, coord) in coords_before {
        assert_coords_close(coord, abs_coord_map(&restored_project)[&sr]);
    }
}

#[test]
fn redo_reapplies_the_reverse() {
    let (state, section_id) = state_with_section();
    assert!(
        state
            .execute(Command::ReverseSection {
                section_id,
                swap_left_right: false,
            })
            .success
    );
    let reversed = state.get_project_state().unwrap();
    let expected: Vec<_> = reversed
        .get_section(section_id)
        .unwrap()
        .shots
        .iter()
        .map(|s| (s.id, s.length, s.azimuth_in))
        .collect();

    assert!(state.undo().success);
    assert!(state.redo().success, "reverse must be redoable");

    let after = state.get_project_state().unwrap();
    let actual: Vec<_> = after
        .get_section(section_id)
        .unwrap()
        .shots
        .iter()
        .map(|s| (s.id, s.length, s.azimuth_in))
        .collect();
    assert_eq!(actual, expected);
}

#[test]
fn reverse_twice_is_the_identity() {
    let (state, section_id) = state_with_section();
    let original = state
        .get_project_state()
        .unwrap()
        .get_section(section_id)
        .unwrap()
        .clone();

    for _ in 0..2 {
        assert!(
            state
                .execute(Command::ReverseSection {
                    section_id,
                    swap_left_right: true,
                })
                .success
        );
    }

    let project = state.get_project_state().unwrap();
    let round_tripped = project.get_section(section_id).unwrap();
    for (a, b) in round_tripped.shots.iter().zip(&original.shots) {
        assert_eq!(a.id, b.id);
        assert_eq!(a.azimuth_in, b.azimuth_in);
        assert_eq!(a.azimuth_out, b.azimuth_out);
        assert_eq!(a.depth_in, b.depth_in);
        assert_eq!(a.depth_out, b.depth_out);
        // `swap_left_right: true` twice must land back on the recorded values,
        // and the pitch double-negation must cancel.
        assert_eq!(a.left, b.left);
        assert_eq!(a.right, b.right);
        assert_eq!(a.pitch_in, b.pitch_in);
        assert_eq!(a.pitch_out, b.pitch_out);
    }
}

/// A zero datum freezes the shots it anchors; `update_shot` rejects geometry
/// edits on them. Reverse must still work, because it leaves each physical line
/// untouched - and the datum must stay on the station the surveyor pinned.
#[test]
fn reverse_works_on_a_datum_anchored_section_and_keeps_the_datum_in_place() {
    let (state, section_id) = state_with_section();
    let datum_at = StationRef::new(section_id, 0);
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: datum_at,
                coordinates: GpsCoordinates {
                    latitude: 20.2,
                    longitude: -87.4,
                    depth_reference: 0.0,
                },
            })
            .success
    );

    let before = abs_coord_map(&state.get_project_state().unwrap());

    let result = state.execute(Command::ReverseSection {
        section_id,
        swap_left_right: false,
    });
    assert!(
        result.success,
        "frozen shots must not block reverse: {:?}",
        result.error
    );

    // Nothing moved, and the datum now sits on station 3 - the same physical
    // station it was pinned to before the reverse.
    let project = state.get_project_state().unwrap();
    let after = abs_coord_map(&project);
    for i in 0..=3u32 {
        assert_coords_close(
            before[&StationRef::new(section_id, i)],
            after[&StationRef::new(section_id, 3 - i)],
        );
    }
    assert_eq!(project.zero_datums.len(), 1);
    assert_eq!(
        project.resolve_key(&project.zero_datums[0].station_ref),
        Some(StationRef::new(section_id, 3)),
        "the zero datum must follow its physical station, not stay on index 0"
    );
}

/// q14: the import toggle reverses BEFORE the section is created, so the
/// wizard's placement indices address the final order. `station_comments`
/// indices are given in the FILE's order and mirrored backend-side.
#[test]
fn import_reverse_applies_before_placement_and_comments_resolve() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Import".to_string(),
        location: None,
    });

    // Three shots; a comment on station 0 of the FINAL (reversed) section.
    let result = state.execute_import_sections(
        vec![ImportSectionConfig {
            section_index: 0,
            name: Some("Imported".to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![
                shot_with_lrud(0.0, 10.0, 0.0, 2.0, 1.0),
                shot_with_lrud(90.0, 5.0, 2.0, 2.0, 10.0),
                shot_with_lrud(210.0, 7.5, 2.0, 1.0, 20.0),
            ],
            placement: ImportPlacement::Origin,
            // Station 0 of the FILE - the backend mirrors it to station 3.
            station_comments: vec![(0, "file station 0".to_string())],
            station_names: Vec::new(),
            reverse: true,
            azimuth_reference: None,
            explorers: None,
            override_date: None,
            surveyors: None,
        }],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "import failed: {:?}", result.error);

    let project = state.get_project_state().unwrap();
    let section = project.sections.first().unwrap();

    // The file's last shot is now first, and its readings are reciprocated.
    assert_eq!(section.shots.len(), 3);
    assert_eq!(section.shots[0].length, 7.5);
    assert_eq!(section.shots[2].length, 10.0);
    assert!((section.shots[2].average_azimuth_measured() - 180.0).abs() < 1e-9);

    // Left/right are untouched on the import path - there is no dialog to ask.
    // The file's last shot (left 20.0) is now first, and its LRUD travelled with
    // it UNSWAPPED. Asserting the values rather than `is_none()`: with a fixture
    // that leaves LRUD at its `None` default this assertion holds no matter what
    // the import does, and flipping `reverse_shots(.., false)` to `true` failed
    // no test at all.
    assert_eq!(section.shots[0].left, Some(20.0));
    assert_eq!(section.shots[0].right, Some(20.5));
    assert_eq!(section.shots[2].left, Some(1.0));
    assert_eq!(section.shots[2].right, Some(1.5));

    // The comment was given against the file's station 0, so it must land on the
    // physical station that is now numbered 3.
    assert_eq!(project.station_comments.len(), 1);
    let (key, text) = project.station_comments.iter().next().unwrap();
    assert_eq!(text, "file station 0");
    assert_eq!(
        project.resolve_key(key),
        Some(StationRef::new(section.id, 3))
    );
}

/// Ticking the import toggle changes the numbering, never where the passage
/// lands.  `Origin` and `Manual` position station 0 directly, so without the
/// offset compensation a reversed section - whose station 0 is now the far end -
/// would sit a full end-to-end displacement away from the un-reversed import.
#[test]
fn import_reverse_lands_the_section_in_the_same_place() {
    let shots = vec![
        shot(0.0, 10.0, 0.0, 2.0),
        shot(90.0, 5.0, 2.0, 2.0),
        shot(210.0, 7.5, 2.0, 1.0),
    ];

    let import = |reverse: bool, placement: ImportPlacement| {
        let state = AppState::new().unwrap();
        state.execute(Command::CreateProject {
            name: "Import".to_string(),
            location: None,
        });
        let result = state.execute_import_sections(
            vec![ImportSectionConfig {
                section_index: 0,
                name: Some("Imported".to_string()),
                surveyor: None,
                date: None,
                color: "#4ecdc4".to_string(),
                direction: None,
                quality: 1.0,
                shots: shots.clone(),
                placement,
                station_comments: Vec::new(),
                station_names: Vec::new(),
                reverse,
                azimuth_reference: None,
                explorers: None,
                override_date: None,
                surveyors: None,
            }],
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
            VirtualShotMode::Auto,
        );
        assert!(result.success, "import failed: {:?}", result.error);
        let project = state.get_project_state().unwrap();
        let mut coords: Vec<Coordinates> = project
            .compute_station_coordinates()
            .into_iter()
            .map(|s| s.coordinates)
            .collect();
        // Reversed numbering walks the same stations from the other end.
        if reverse {
            coords.reverse();
        }
        coords
    };

    for placement in [
        ImportPlacement::Origin,
        ImportPlacement::Manual {
            align_station_index: 0,
            offset: Some(Coordinates {
                x: 40.0,
                y: -15.0,
                z: 0.0,
            }),
        },
    ] {
        let plain = import(false, placement.clone());
        let reversed = import(true, placement.clone());
        assert_eq!(plain.len(), reversed.len(), "station count changed");
        for (i, (a, b)) in plain.iter().zip(&reversed).enumerate() {
            assert!(
                (a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6,
                "station {i} moved under {placement:?}: {a:?} vs {b:?}"
            );
        }
    }
}

/// The three-shot file every placement test below imports. It turns a corner and
/// changes depth, so a placement that lands on the wrong physical station shows
/// up as a moved station rather than a coincidence.
fn import_shots() -> Vec<ShotData> {
    vec![
        shot(0.0, 10.0, 0.0, 2.0),
        shot(90.0, 5.0, 2.0, 2.0),
        shot(210.0, 7.5, 2.0, 1.0),
    ]
}

/// Import [`import_shots`] under `placement`, optionally reversed, and return the
/// imported section's station coordinates in the FILE's order (station 0 of the
/// file first) so the two runs are directly comparable.
///
/// The project already contains one placed section, passed to `placement` as its
/// id: `FromExisting`/`ManualSnap` need a real station to attach to, and it fixes
/// the world frame for both runs.
fn import_one(reverse: bool, placement: fn(SectionId) -> ImportPlacement) -> Vec<Coordinates> {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Import".to_string(),
        location: None,
    });
    let anchor = state
        .execute(Command::CreateSection {
            name: Some("Anchor".to_string()),
            surveyor: None,
            date: None,
            color: "#ff6b6b".to_string(),
            direction: None,
            shots: vec![shot(0.0, 6.0, 0.0, 1.0), shot(90.0, 6.0, 1.0, 2.0)],
        })
        .section_id
        .unwrap();

    let result = state.execute_import_sections(
        vec![ImportSectionConfig {
            section_index: 0,
            name: Some("Imported".to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            quality: 1.0,
            shots: import_shots(),
            placement: placement(anchor),
            station_comments: Vec::new(),
            station_names: Vec::new(),
            reverse,
            azimuth_reference: None,
            explorers: None,
            override_date: None,
            surveyors: None,
        }],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "import failed: {:?}", result.error);

    let project = state.get_project_state().unwrap();
    let imported = project.sections.iter().find(|s| s.id != anchor).unwrap().id;
    let coords = abs_coord_map(&project);
    let mut out: Vec<Coordinates> = (0..=import_shots().len() as u32)
        .map(|i| coords[&StationRef::new(imported, i)])
        .collect();
    // Reversed numbering walks the same stations from the other end.
    if reverse {
        out.reverse();
    }
    out
}

/// The placements that re-anchor a section through an INDEX. `Origin` and
/// `Manual` (covered above) position station 0 directly and carry no station
/// index the reverse can misplace; these three do, and the wizard hands the index
/// over ALREADY MIRRORED (`src/utils/reversePlacement.ts`) because the backend
/// reverses before the section exists. If the mirrored index does not land on the
/// physical station the user clicked, the passage attaches by the wrong end -
/// invisible in the shot list, obvious on the map.
#[test]
fn import_reverse_lands_indexed_placements_on_the_station_the_user_picked() {
    // (label, placement as the user built it, the same placement mirrored for a
    // reversed import). Three shots, so the mirror is `3 - i`.
    let cases: [(
        &str,
        fn(SectionId) -> ImportPlacement,
        fn(SectionId) -> ImportPlacement,
    ); 3] = [
        (
            "zero datum on the file's station 1",
            |_| ImportPlacement::ZeroDatum {
                coordinates: GpsCoordinates {
                    latitude: 20.2,
                    longitude: -87.4,
                    depth_reference: 0.0,
                },
                station_index: 1,
            },
            |_| ImportPlacement::ZeroDatum {
                coordinates: GpsCoordinates {
                    latitude: 20.2,
                    longitude: -87.4,
                    depth_reference: 0.0,
                },
                station_index: 2,
            },
        ),
        // Only the IMPORT side mirrors: the target was already in the project
        // with its final numbering, so its index must not move.
        (
            "the file's station 1 merged onto an existing station",
            |anchor| ImportPlacement::FromExisting {
                import_station: 1,
                existing_station: StationRef::new(anchor, 2),
            },
            |anchor| ImportPlacement::FromExisting {
                import_station: 2,
                existing_station: StationRef::new(anchor, 2),
            },
        ),
        (
            "the file's station 0 snapped onto an existing station",
            |anchor| ImportPlacement::ManualSnap {
                align_station_index: 0,
                snapped_to_station: StationRef::new(anchor, 0),
            },
            |anchor| ImportPlacement::ManualSnap {
                align_station_index: 3,
                snapped_to_station: StationRef::new(anchor, 0),
            },
        ),
    ];

    for (label, plain_placement, reversed_placement) in cases {
        let plain = import_one(false, plain_placement);
        let reversed = import_one(true, reversed_placement);
        assert_eq!(
            plain.len(),
            reversed.len(),
            "{label}: station count changed"
        );
        for (i, (a, b)) in plain.iter().zip(&reversed).enumerate() {
            assert!(
                (a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6 && (a.z - b.z).abs() < 1e-6,
                "{label}: the file's station {i} moved: {a:?} vs {b:?}"
            );
        }
    }
}

/// Import a two-section batch wired together by `FromImporting`, optionally with
/// BOTH sections reversed, and return each section's stations in FILE order.
///
/// The trunk has three shots and the branch two, so the two sides of the
/// connection mirror by different divisors - a mirror that used one section's
/// shot count for the other lands on a different physical station.
fn import_two(
    reverse: bool,
    import_station: u32,
    target_station_index: u32,
) -> (Vec<Coordinates>, Vec<Coordinates>) {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Import".to_string(),
        location: None,
    });

    let config =
        |section_index: u32, name: &str, shots: Vec<ShotData>, placement| ImportSectionConfig {
            section_index,
            name: Some(name.to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            quality: 1.0,
            shots,
            placement,
            station_comments: Vec::new(),
            station_names: Vec::new(),
            reverse,
            azimuth_reference: None,
            explorers: None,
            override_date: None,
            surveyors: None,
        };
    let result = state.execute_import_sections(
        vec![
            config(0, "Trunk", import_shots(), ImportPlacement::Origin),
            config(
                1,
                "Branch",
                // Its far end sits at the trunk's station-1 depth, so the
                // connection below is clean in all three axes.
                vec![shot(45.0, 8.0, 4.0, 3.0), shot(135.0, 4.0, 3.0, 2.0)],
                ImportPlacement::FromImporting {
                    import_station,
                    target_section_index: 0,
                    target_station_index,
                },
            ),
        ],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "import failed: {:?}", result.error);

    let project = state.get_project_state().unwrap();
    let coords = abs_coord_map(&project);
    let stations = |index: usize, shot_count: u32| {
        let section_id = project.sections[index].id;
        let mut out: Vec<Coordinates> = (0..=shot_count)
            .map(|i| coords[&StationRef::new(section_id, i)])
            .collect();
        if reverse {
            out.reverse();
        }
        out
    };
    (stations(0, 3), stations(1, 2))
}

/// The cross-section connection, with BOTH sections reversed and DIFFERENT shot
/// counts - the case a single-section test cannot reach.
///
/// The user, looking at the file's order, ties the branch's station 2 to the
/// trunk's station 1. Reversed, those become the branch's station 0 (`2 - 2`, two
/// shots) and the trunk's station 2 (`3 - 1`, three shots). Every index in play
/// moves, and the two sides land on different numbers, so swapping the divisors
/// picks a station the user never chose rather than coincidentally the right one.
/// The batch creates the trunk before the branch merges onto it, so each index
/// has to address the post-reverse numbering of its OWN section; this asserts the
/// resulting world geometry rather than the stored indices, because that is what
/// the surveyor sees.
#[test]
fn import_reverse_connects_two_reversed_sections_by_the_stations_the_user_picked() {
    // As the user drew it: branch station 2 onto trunk station 1.
    let (trunk_plain, branch_plain) = import_two(false, 2, 1);
    // The same intent through the wizard's mirror: 2 - 2 on the branch, 3 - 1 on
    // the trunk.
    let (trunk_reversed, branch_reversed) = import_two(true, 0, 2);

    for (label, plain, reversed) in [
        ("trunk", &trunk_plain, &trunk_reversed),
        ("branch", &branch_plain, &branch_reversed),
    ] {
        assert_eq!(
            plain.len(),
            reversed.len(),
            "{label}: station count changed"
        );
        for (i, (a, b)) in plain.iter().zip(reversed).enumerate() {
            assert!(
                (a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6 && (a.z - b.z).abs() < 1e-6,
                "{label}: the file's station {i} moved under a reversed import: {a:?} vs {b:?}"
            );
        }
    }

    // And the connection itself is where the user put it: the branch's station 2
    // sits exactly on the trunk's station 1, both in the file's numbering.
    assert_coords_close(branch_reversed[2], trunk_reversed[1]);
    // Not vacuous - the trunk's stations are genuinely distinct places, so
    // landing on the wrong one is detectable.
    for i in [0usize, 2, 3] {
        assert!(
            (trunk_reversed[i].x - trunk_reversed[1].x).abs() > 1e-3
                || (trunk_reversed[i].y - trunk_reversed[1].y).abs() > 1e-3,
            "precondition: trunk station {i} coincides with station 1"
        );
    }
}

/// Reversing a multi-selection goes through a batch so it is one user action:
/// every section or none, and a single Undo to get back.
#[test]
fn batched_multi_section_reverse_is_atomic_and_undoes_in_one_step() {
    let (state, first_id) = state_with_section();
    let second = state.execute(Command::CreateSection {
        name: Some("Side passage".to_string()),
        surveyor: None,
        date: None,
        color: "#ff6b6b".to_string(),
        direction: None,
        shots: vec![shot(45.0, 8.0, 0.0, 1.0), shot(315.0, 6.0, 1.0, 3.0)],
    });
    let second_id = second.section_id.unwrap();

    let before = abs_coord_map(&state.get_project_state().unwrap());
    let first_lengths = |project: &Project| {
        project
            .sections
            .iter()
            .find(|s| s.id == first_id)
            .map(|s| s.shots.iter().map(|shot| shot.length).collect::<Vec<_>>())
            .unwrap()
    };
    let lengths_before = first_lengths(&state.get_project_state().unwrap());

    // A batch whose LAST command names a section that does not exist: the two
    // good reverses must not survive it.
    let failed = state.execute(Command::Batch {
        commands: vec![
            Command::ReverseSection {
                section_id: first_id,
                swap_left_right: false,
            },
            Command::ReverseSection {
                section_id: second_id,
                swap_left_right: false,
            },
            Command::ReverseSection {
                section_id: 9999,
                swap_left_right: false,
            },
        ],
    });
    assert!(!failed.success, "batch with a missing section should fail");
    assert_eq!(
        first_lengths(&state.get_project_state().unwrap()),
        lengths_before,
        "a failed batch left a section reversed"
    );

    // The all-good batch applies to both sections and costs exactly one undo.
    let ok = state.execute(Command::Batch {
        commands: vec![
            Command::ReverseSection {
                section_id: first_id,
                swap_left_right: false,
            },
            Command::ReverseSection {
                section_id: second_id,
                swap_left_right: false,
            },
        ],
    });
    assert!(ok.success, "batch failed: {:?}", ok.error);

    let reversed = state.get_project_state().unwrap();
    let mut expected = lengths_before.clone();
    expected.reverse();
    assert_eq!(first_lengths(&reversed), expected);

    assert!(state.undo().success);
    let restored = state.get_project_state().unwrap();
    assert_eq!(
        first_lengths(&restored),
        lengths_before,
        "one undo should restore both sections"
    );
    let restored_coords = abs_coord_map(&restored);
    assert_eq!(restored_coords.len(), before.len());
    for (station_ref, coords) in &before {
        assert_coords_close(*coords, *restored_coords.get(station_ref).unwrap());
    }
}

/// A shot-less section (GPX imports one per waypoint) reverses to itself, so
/// `generate_undo_data` has nothing to capture - but the command still SUCCEEDS.
/// Pushing that `UndoData::None` entry wedges the whole stack: `undo()` pops it,
/// maps `None` to a failure, and its epilogue pushes it straight back, so every
/// older entry underneath is unreachable forever (and the stack is persisted, so
/// it survives close/reopen). The entry must simply not be pushed.
#[test]
fn reversing_a_shotless_section_leaves_the_undo_stack_usable() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Waypoints".to_string(),
        location: None,
    });

    // An ordinary, undoable edit first - this is what must stay reachable.
    let created = state.execute(Command::CreateSection {
        name: Some("Real line".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots: vec![shot(0.0, 10.0, 0.0, 2.0)],
    });
    assert!(created.success);

    // Then a waypoint-style section with no shots, and a reverse of it.
    let empty = state.execute(Command::CreateSection {
        name: Some("Waypoint".to_string()),
        surveyor: None,
        date: None,
        color: "#ff6b6b".to_string(),
        direction: None,
        shots: Vec::new(),
    });
    let empty_id = empty.section_id.unwrap();
    let reversed = state.execute(Command::ReverseSection {
        section_id: empty_id,
        swap_left_right: false,
    });
    assert!(reversed.success, "reverse failed: {:?}", reversed.error);

    // The next undo must do real work, not pop-and-repush the no-op forever.
    let first = state.undo();
    assert!(
        first.success,
        "undo blocked by an unundoable entry: {:?}",
        first.error
    );
    // ... and the section creations underneath it are still reachable.
    assert!(state.undo().success);
    let project = state.get_project_state().unwrap();
    assert!(
        project.sections.is_empty(),
        "older history was left unreachable: {:?}",
        project.sections.iter().map(|s| s.id).collect::<Vec<_>>()
    );
}

/// The mirror is `shot_count - station_index`, so an index past the last station
/// has no mirror at all. `saturating_sub` would silently park such a comment on
/// station 0 - a station it was never about - where the un-reversed import just
/// fails `key_for_ref` and drops it.
#[test]
fn import_reverse_drops_an_out_of_range_station_comment() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Import".to_string(),
        location: None,
    });

    let result = state.execute_import_sections(
        vec![ImportSectionConfig {
            section_index: 0,
            name: Some("Imported".to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![shot(0.0, 10.0, 0.0, 2.0), shot(90.0, 5.0, 2.0, 2.0)],
            placement: ImportPlacement::Origin,
            // Two shots means stations 0..2; station 7 does not exist.
            station_comments: vec![(7, "nowhere".to_string()), (1, "the bend".to_string())],
            station_names: Vec::new(),
            reverse: true,
            azimuth_reference: None,
            explorers: None,
            override_date: None,
            surveyors: None,
        }],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "import failed: {:?}", result.error);

    let project = state.get_project_state().unwrap();
    let section = project.sections.first().unwrap();
    assert_eq!(
        project.station_comments.len(),
        1,
        "the out-of-range comment must be dropped, not relocated: {:?}",
        project.station_comments.values().collect::<Vec<_>>()
    );
    let (key, text) = project.station_comments.iter().next().unwrap();
    assert_eq!(text, "the bend");
    assert_eq!(
        project.resolve_key(key),
        Some(StationRef::new(section.id, 1)),
        "the in-range comment must still mirror to station 1"
    );
}

/// Reversing swaps each shot's depths, negates its pitch and normalises its
/// azimuths, so the per-project discrepancy checks can read differently
/// afterwards - and the shot ids stay with POSITIONS, so a warning that survives
/// now names a different row. `reverse_section` re-runs `Validator::validate_shot`
/// over the reversed section for exactly that reason; without it the panel keeps
/// showing the pre-reverse verdict against the post-reverse rows.
///
/// The fixture makes both directions visible at once: the file's first shot has
/// out-of-range azimuths (a warning the reverse's normalisation REMOVES) and its
/// last shot records an 80-degree descent on a level leg (a mismatch the reverse
/// carries to position 1 with the sign flipped).
#[test]
fn reverse_re_emits_warnings_computed_from_the_reversed_geometry() {
    let state = AppState::new().unwrap();
    // Built directly rather than through `CreateProject` so the thresholds are the
    // library defaults, not whatever this machine's config seeds a new project
    // with - the assertion below counts warnings exactly.
    let mut project = Project::new("Warnings".to_string(), None);

    // Validation only WARNS on an out-of-range azimuth, so a project can carry
    // one; `reverse_shots` rem_euclid's both readings, so it cannot survive.
    let mut out_of_range = shot(0.0, 10.0, 0.0, 0.0);
    out_of_range.azimuth_in = 400.0;
    out_of_range.azimuth_out = 400.0;
    // A level shot recording an 80-degree descent: the implied inclination is 0,
    // so the two disagree by 80 - well past the 15-degree default threshold.
    let mut mismatch = shot(90.0, 10.0, 1.0, 1.0);
    mismatch.pitch_in = Some(80.0);
    mismatch.pitch_out = Some(80.0);

    let section_id = project
        .add_section(crate::models::SectionData {
            name: Some("Main".to_string()),
            shots: vec![out_of_range, shot(45.0, 8.0, 0.0, 1.0), mismatch],
            ..Default::default()
        })
        .id;
    state.set_project_for_tests(project);

    let result = state.execute(Command::ReverseSection {
        section_id,
        swap_left_right: false,
    });
    assert!(result.success, "reverse failed: {:?}", result.error);

    // Skipping the re-emission reports none; validating the PRE-reverse shots
    // reports three (an INVALID_AZIMUTH per out-of-range leg, plus the mismatch).
    assert_eq!(
        result.warnings.len(),
        1,
        "warnings must be recomputed from the reversed section: {:?}",
        result.warnings
    );
    let warning = &result.warnings[0];
    assert_eq!(warning.code, "INCLINATION_DEPTH_MISMATCH");
    assert!(
        warning
            .message
            .starts_with(&format!("Shot {section_id}:1 ")),
        "the mismatched shot is now FIRST, so the warning must name row 1: {}",
        warning.message
    );
    assert!(
        warning.message.contains("-80.0"),
        "the reverse negates pitch, so the recorded inclination is now -80: {}",
        warning.message
    );
}

/// `Command::ReverseSection` is graded `GEOMETRY_ONLY` in `commands.rs`, whose
/// comment claims the reconcile epilogue is a no-op for it. Only a MERGED section
/// can test that claim: the epilogue's `realign_merges` is tolerance-gated and
/// does nothing unless a merge is actually drifted - and a reverse that failed to
/// carry the merge to the mirrored station leaves exactly that drift, at which
/// point the epilogue re-closes it by MOVING a section. `Project::reverse_section`
/// on its own never runs that pass, so the model-level tests cannot see it.
#[test]
fn reversing_a_merge_anchored_section_moves_nothing_through_the_full_execute() {
    let (state, main_id) = state_with_section();
    let side_id = state
        .execute(Command::CreateSection {
            name: Some("Side passage".to_string()),
            surveyor: None,
            date: None,
            color: "#ff6b6b".to_string(),
            direction: None,
            // Starts at the main line's station-1 depth so the merge is clean.
            shots: vec![shot(45.0, 8.0, 2.0, 2.0), shot(135.0, 4.0, 2.0, 3.0)],
        })
        .section_id
        .unwrap();

    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(side_id, 0),
        station_b: StationRef::new(main_id, 1),
    });
    assert!(merged.success, "merge failed: {:?}", merged.error);

    let before = abs_coord_map(&state.get_project_state().unwrap());

    let result = state.execute(Command::ReverseSection {
        section_id: main_id,
        swap_left_right: false,
    });
    assert!(result.success, "reverse failed: {:?}", result.error);

    let project = state.get_project_state().unwrap();
    let after = abs_coord_map(&project);
    assert_eq!(before.len(), after.len(), "station count changed");

    // The main line's station i is now station 3 - i, in the same place; the side
    // passage was not renumbered and must not have been dragged either.
    for i in 0..=3u32 {
        assert_coords_close(
            before[&StationRef::new(main_id, i)],
            after[&StationRef::new(main_id, 3 - i)],
        );
    }
    for i in 0..=2u32 {
        assert_coords_close(
            before[&StationRef::new(side_id, i)],
            after[&StationRef::new(side_id, i)],
        );
    }

    // Nothing the reverse re-keyed was left pointing at a station that no longer
    // exists. Checked BEFORE the specific merge assertions below, because a stranded
    // anchor makes every one of them report an uninformative `None`, and because
    // nothing in the pipeline catches it: a reverse is GEOMETRY_ONLY in
    // `command_integrity`, whose comment justifies skipping the (expensive) rollback
    // snapshot with "reverse rewrites its own external refs to the mirrored stations,
    // so nothing is left dangling". No snapshot means `execute` never runs
    // `guard_introduced_corruption` here, so a reverse that DID strand an anchor
    // commits and still reports success - the claim has to be asserted directly. (A
    // randomized fuzz property used to cover it. The targeted tests that replaced it
    // check that named anchors resolve, which says nothing about a ref in some OTHER
    // collection - `map_station_refs` walks merges, closures, datums, offsets and
    // comments separately, so one can be missed on its own.)
    let dangling: Vec<_> = project
        .check_structural_integrity()
        .into_iter()
        .filter(|v| v.category.starts_with("dangling_"))
        .collect();
    assert!(
        dangling.is_empty(),
        "the reverse left a dangling cross-reference: {dangling:#?}"
    );

    // The merge followed the physical station rather than the index, and is still
    // closed - "the epilogue is a no-op" means `realign_merges` found nothing.
    assert_eq!(project.station_merges.len(), 1);
    let merge = &project.station_merges[0];
    let resolved = [
        project.resolve_key(&merge.station_a),
        project.resolve_key(&merge.station_b),
    ];
    assert!(
        resolved.contains(&Some(StationRef::new(main_id, 2))),
        "the merge must follow the main line's middle station to its mirrored \
         index 2: {resolved:?}"
    );
    assert!(
        resolved.contains(&Some(StationRef::new(side_id, 0))),
        "the side passage's end of the merge must not move: {resolved:?}"
    );
    assert!(
        project.check_merge_alignment().is_empty(),
        "the merge drifted through the reverse: {:?}",
        project.check_merge_alignment()
    );
}

#[test]
fn reverse_of_a_missing_section_fails() {
    let (state, _) = state_with_section();
    let result = state.execute(Command::ReverseSection {
        section_id: 9999,
        swap_left_right: false,
    });
    assert!(!result.success);
    assert!(result.error.unwrap().contains("not found"));
}

#[test]
fn reverse_reports_the_section_it_touched() {
    let (state, section_id) = state_with_section();
    let result = state.execute(Command::ReverseSection {
        section_id,
        swap_left_right: false,
    });
    assert_eq!(result.section_id, Some(section_id));
}

/// A resolved-warning row with a NULL `shot_uuid` is keyed by `(section_id, shot_id)`,
/// and the reverse deliberately keeps shot IDs attached to POSITIONS - so after it that
/// key names a DIFFERENT physical shot. The save-time carry-over then upgrades the row
/// through `live_by_id.get(&(section_id, shot_id))` and pins it, by UUID, to the mirrored
/// shot - permanently suppressing an unrelated shot's warning.
///
/// Not a legacy-only shape: `commands::validation` deliberately stores `None` whenever the
/// shot's UUID is not yet on disk, which includes any shot created in the current session.
#[test]
fn reverse_keeps_a_position_keyed_ignore_on_its_own_shot() {
    let temp = tempfile::TempDir::new().unwrap();
    let (state, section_id) = state_with_section();
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(temp.path().join("cave.stalx")),
            })
            .success
    );

    // Shot ids and UUIDs as they stand before the reverse.
    let (ids, uuids): (Vec<u32>, Vec<String>) = {
        let guard = state.project.lock().unwrap();
        let section = guard.as_ref().unwrap().get_section(section_id).unwrap();
        (
            section.shots.iter().map(|s| s.id).collect(),
            section
                .shots
                .iter()
                .map(|s| s.uuid.clone().unwrap())
                .collect(),
        )
    };
    assert!(ids.len() >= 3);

    // A position-keyed ignore on the FIRST shot.
    state
        .save_resolved_warning(&crate::persistence::ResolvedWarning {
            section_id,
            shot_id: ids[0],
            warning_code: "TEST_IGNORE".to_string(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: None,
            shot_uuid: None,
        })
        .unwrap();

    assert!(
        state
            .execute(Command::ReverseSection {
                section_id,
                swap_left_right: false,
            })
            .success
    );
    // The save is where the row's position key would be re-read as an identity.
    assert!(state.execute(Command::SaveProject { path: None }).success);

    let rows = state.load_resolved_warnings().unwrap();
    assert_eq!(rows.len(), 1, "the ignore must survive the reverse");
    assert_eq!(
        rows[0].shot_uuid.as_deref(),
        Some(uuids[0].as_str()),
        "the ignore must stay on the shot it was taken on, not migrate to the \
         shot the reverse moved onto that position (which is {:?})",
        uuids.last()
    );
}

/// The renumbering signal has to survive the shapes a reverse actually arrives in:
/// batched (reversing a multi-selection is one atomic user action) and replayed through
/// undo/redo, where the snapshot restore leaves NOTHING in the geometry to diff - every
/// station keeps its world position either way.
#[test]
fn reverse_reports_every_renumbered_section_through_batch_undo_and_redo() {
    let (state, first) = state_with_section();
    let second = state
        .execute(Command::CreateSection {
            name: Some("Side".to_string()),
            surveyor: None,
            date: None,
            color: "#ff6b6b".to_string(),
            direction: None,
            shots: vec![shot(45.0, 8.0, 0.0, 1.0), shot(135.0, 4.0, 1.0, 1.0)],
        })
        .section_id
        .unwrap();

    let batch = state.execute(Command::Batch {
        commands: vec![
            Command::ReverseSection {
                section_id: first,
                swap_left_right: false,
            },
            Command::ReverseSection {
                section_id: second,
                swap_left_right: false,
            },
        ],
    });
    assert!(batch.success, "{:?}", batch.error);
    assert_eq!(
        batch.reversed_sections,
        Some(vec![reversed_entry(first, 3), reversed_entry(second, 2)])
    );

    let undone = state.undo();
    assert!(undone.success, "{:?}", undone.error);
    assert_eq!(
        undone.reversed_sections,
        Some(vec![reversed_entry(first, 3), reversed_entry(second, 2)]),
        "undoing a reverse re-reverses the numbering, so it reports the same sections"
    );

    let redone = state.redo();
    assert!(redone.success, "{:?}", redone.error);
    assert_eq!(
        redone.reversed_sections,
        Some(vec![reversed_entry(first, 3), reversed_entry(second, 2)])
    );

    // A command that renumbers nothing must leave the field off the wire entirely.
    let moved = state.execute(Command::MoveSection {
        section_id: first,
        offset: Coordinates::new(1.0, 2.0, 0.0),
    });
    assert!(moved.success, "{:?}", moved.error);
    assert!(moved.reversed_sections.is_none());
}

/// The history dropdown undoes several entries at once. Each inner step knows what
/// it renumbered, but only the aggregate reaches the frontend, which mirrors its
/// positional refs from it - so a reverse buried in the middle of the range must
/// still be reported, or the gas plan and the distance origin are left on the
/// mirrored stations with nothing to signal it.
#[test]
fn undo_to_and_redo_to_report_every_reverse_in_the_range() {
    let (state, section) = state_with_section();

    let reversed = state.execute(Command::ReverseSection {
        section_id: section,
        swap_left_right: false,
    });
    assert!(reversed.success, "{:?}", reversed.error);

    // Bury the reverse behind a later, non-renumbering edit so undo_to has to walk
    // past it rather than land on it.
    let moved = state.execute(Command::MoveSection {
        section_id: section,
        offset: Coordinates::new(1.0, 2.0, 0.0),
    });
    assert!(moved.success, "{:?}", moved.error);

    // Index 1 = two undos: the move, then the reverse.
    let undone = state.undo_to(1);
    assert!(undone.success, "{:?}", undone.error);
    assert_eq!(
        undone.reversed_sections,
        Some(vec![reversed_entry(section, 3)]),
        "a reverse inside the undone range must reach the frontend"
    );

    let redone = state.redo_to(1);
    assert!(redone.success, "{:?}", redone.error);
    assert_eq!(
        redone.reversed_sections,
        Some(vec![reversed_entry(section, 3)])
    );
}

/// `undo_to`'s aggregate is an ORDERED PER-STEP LOG, not a parity-reduced set:
/// two reverses of one section are listed TWICE, each with the shot count in
/// force at its own step. That is deliberate, and it does NOT contradict the
/// parity reduction a single `Batch` gets - a batch cannot change the shot count
/// mid-flight, whereas the steps of an undo range can (a `CreateShot` undone
/// between two reverses), so only the aggregate has to keep the per-step counts.
/// The consumer composes the entries: equal counts an even number of times give
/// the exact identity. Reducing here instead would throw the counts away.
#[test]
fn undo_to_repeats_a_section_reversed_twice() {
    let (state, section) = state_with_section();

    for _ in 0..2 {
        let reversed = state.execute(Command::ReverseSection {
            section_id: section,
            swap_left_right: false,
        });
        assert!(reversed.success, "{:?}", reversed.error);
    }

    let undone = state.undo_to(1);
    assert!(undone.success, "{:?}", undone.error);
    assert_eq!(
        undone.reversed_sections,
        Some(vec![reversed_entry(section, 3), reversed_entry(section, 3)])
    );
}

/// A `Batch` that reverses ONE section TWICE leaves the numbering exactly as it
/// found it, so there is nothing to report and - crucially - nothing to fix up.
///
/// `settle_reversed_sections` applies its two fix-ups ONCE PER LIST ENTRY against
/// one FINAL project state, and the `.plnx` re-key is the cyclic shift
/// `j -> (j - 1) mod (N + 1)`, not an involution. So an unreduced `[A, A]` walks
/// every saved plan key TWO stations along a numbering that never moved, and the
/// panel shows no warning because the keys still resolve.
#[test]
fn batch_reversing_one_section_twice_reports_nothing_and_leaves_the_plnx_alone() {
    let temp = tempfile::TempDir::new().unwrap();
    let (state, section_id) = state_with_section();
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(temp.path().join("cave.stalx")),
            })
            .success
    );

    // Every kind of key a plan holds, so a two-station walk cannot hide in a slot
    // the fixture happens not to use.
    let mut snapshot: DivePlanSnapshot = serde_json::from_str("{}").unwrap();
    snapshot.origin = Some(StationRef::new(section_id, 0));
    snapshot.endpoint = Some(StationRef::new(section_id, 3));
    snapshot.waypoints = vec![StationRef::new(section_id, 2)];
    snapshot.marked_points = vec![MarkedPoint {
        station: StationRef::new(section_id, 1),
        label: Some("photo".to_string()),
    }];
    let saved = state
        .save_dive_plan(None, "Plan".to_string(), snapshot)
        .unwrap();

    let batch = state.execute(Command::Batch {
        commands: vec![
            Command::ReverseSection {
                section_id,
                swap_left_right: false,
            },
            Command::ReverseSection {
                section_id,
                swap_left_right: false,
            },
        ],
    });
    assert!(batch.success, "{:?}", batch.error);
    assert!(
        batch.reversed_sections.is_none(),
        "a net no-op reverse renumbered nothing, so the field must be off the wire \
         entirely: {:?}",
        batch.reversed_sections
    );

    let plan = state.load_dive_plan(saved.id).unwrap();
    assert_eq!(plan.stale_ref_count, 0);
    assert_eq!(
        plan.snapshot.origin,
        Some(StationRef::new(section_id, 0)),
        "the numbering never moved, so the saved plan must be untouched"
    );
    assert_eq!(plan.snapshot.endpoint, Some(StationRef::new(section_id, 3)));
    assert_eq!(
        plan.snapshot.waypoints,
        vec![StationRef::new(section_id, 2)]
    );
    assert_eq!(
        plan.snapshot.marked_points[0].station,
        StationRef::new(section_id, 1)
    );
}

/// Parity is resolved PER SECTION, not per batch: `[A, B, A]` nets to `B` alone.
/// A's plan keys must sit exactly where they started while B's move once.
#[test]
fn batch_a_b_a_reports_only_b() {
    let temp = tempfile::TempDir::new().unwrap();
    let (state, first) = state_with_section();
    let second = state
        .execute(Command::CreateSection {
            name: Some("Side".to_string()),
            surveyor: None,
            date: None,
            color: "#ff6b6b".to_string(),
            direction: None,
            shots: vec![shot(45.0, 8.0, 0.0, 1.0), shot(135.0, 4.0, 1.0, 1.0)],
        })
        .section_id
        .unwrap();
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(temp.path().join("cave.stalx")),
            })
            .success
    );

    let mut snapshot: DivePlanSnapshot = serde_json::from_str("{}").unwrap();
    snapshot.origin = Some(StationRef::new(first, 0));
    snapshot.endpoint = Some(StationRef::new(second, 0));
    snapshot.waypoints = vec![StationRef::new(first, 2)];
    let saved = state
        .save_dive_plan(None, "Plan".to_string(), snapshot)
        .unwrap();

    let reverse_of = |section_id| Command::ReverseSection {
        section_id,
        swap_left_right: false,
    };
    let batch = state.execute(Command::Batch {
        commands: vec![reverse_of(first), reverse_of(second), reverse_of(first)],
    });
    assert!(batch.success, "{:?}", batch.error);
    assert_eq!(
        batch.reversed_sections,
        Some(vec![reversed_entry(second, 2)]),
        "only the section reversed an ODD number of times was renumbered"
    );

    let plan = state.load_dive_plan(saved.id).unwrap();
    assert_eq!(plan.stale_ref_count, 0);
    assert_eq!(
        plan.snapshot.origin,
        Some(StationRef::new(first, 0)),
        "the twice-reversed section's keys must not move"
    );
    assert_eq!(plan.snapshot.waypoints, vec![StationRef::new(first, 2)]);
    assert_eq!(
        plan.snapshot.endpoint,
        Some(StationRef::new(second, 2)),
        "the once-reversed section's station 0 is now its station 2"
    );
}

/// The warning re-pin has the same shape as the `.plnx` re-key: it rebuilds its
/// `(section_id, shot_id) -> mirrored uuid` map from the CURRENT project, so
/// running it for a section whose numbering never moved pins a position-keyed
/// ignore onto the shot at the mirrored position - permanently suppressing an
/// unrelated warning. An unreduced `[A, A]` collapses in the map to exactly one
/// such spurious application.
#[test]
fn a_null_uuid_warning_row_survives_a_double_reverse_in_one_batch() {
    let temp = tempfile::TempDir::new().unwrap();
    let (state, section_id) = state_with_section();
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(temp.path().join("cave.stalx")),
            })
            .success
    );

    let first_shot_id = {
        let guard = state.project.lock().unwrap();
        guard
            .as_ref()
            .unwrap()
            .get_section(section_id)
            .unwrap()
            .shots[0]
            .id
    };
    state
        .save_resolved_warning(&crate::persistence::ResolvedWarning {
            section_id,
            shot_id: first_shot_id,
            warning_code: "TEST_IGNORE".to_string(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: None,
            shot_uuid: None,
        })
        .unwrap();

    let batch = state.execute(Command::Batch {
        commands: vec![
            Command::ReverseSection {
                section_id,
                swap_left_right: false,
            },
            Command::ReverseSection {
                section_id,
                swap_left_right: false,
            },
        ],
    });
    assert!(batch.success, "{:?}", batch.error);

    let rows = state.load_resolved_warnings().unwrap();
    assert_eq!(rows.len(), 1, "the ignore must survive the batch");
    assert_eq!(
        rows[0].shot_uuid, None,
        "the numbering never moved, so the row must stay position-keyed rather than \
         being pinned to whatever shot sits at the mirrored position"
    );
}

/// `undo_to` bails at the first failing step, but the steps BEFORE it have already
/// applied and cannot be rolled back. If one of them re-reversed a section, the
/// frontend's positional refs are now mirrored - so the failure result has to carry
/// what was applied, or the pins are silently left on the wrong stations.
#[test]
fn undo_to_reports_the_reverses_it_already_applied_when_a_later_step_fails() {
    let (state, section_id) = state_with_section();

    // A poisoned entry UNDERNEATH the reverse: `undo_inner` cannot parse its
    // `values_json`, so step 2 of the range fails after step 1 already ran.
    {
        let mut stack = state.undo_stack.lock().unwrap();
        stack.push_with_data(
            Command::UpdateSectionMetadata {
                section_id,
                updates: SectionUpdates::default(),
            },
            UndoData::PreviousValues {
                section_id,
                shot_id: None,
                shot_uuid: None,
                values_json: "not-valid-json".to_string(),
                previous_offsets: HashMap::new(),
            },
        );
    }
    assert!(
        state
            .execute(Command::ReverseSection {
                section_id,
                swap_left_right: false,
            })
            .success
    );

    let result = state.undo_to(1);
    assert!(!result.success, "step 2 must fail: {:?}", result.error);
    assert!(
        result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("step 2 of 2"),
        "{:?}",
        result.error
    );
    assert_eq!(
        result.reversed_sections,
        Some(vec![reversed_entry(section_id, 3)]),
        "step 1 re-reversed the section and is not rolled back, so the failure must \
         still report it"
    );
}

/// A reverse of a shot-less section must be a TRUE no-op in the backend, not merely harmless in
/// the UI (#24).
///
/// `Project::reverse_section` returns `true` on `shot_count == 0` without touching a field, and
/// the dialog filters such sections out - but the dialog is not a guard. The batch path, the raw
/// IPC command and the agent harness all reach `execute` directly, and a GPX waypoint imports as
/// exactly this kind of section. Falling through cost the whole redo branch and stamped
/// `has_unpushed_edits`, proliferating a phantom edit to SpeleoDB for an operation that changed
/// nothing.
#[test]
fn reversing_a_shotless_section_leaves_redo_and_the_dirty_bit_alone() {
    let (state, real) = state_with_section();
    let empty = state
        .execute(Command::CreateSection {
            name: Some("Waypoint".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: Vec::new(),
        })
        .section_id
        .unwrap();

    // Give the redo stack something to lose: a real edit, then undo it.
    assert!(
        state
            .execute(Command::ReverseSection {
                section_id: real,
                swap_left_right: false,
            })
            .success
    );
    assert!(state.undo().success);
    let redo_before = state.undo_stack.lock().unwrap().redo_len();
    assert_eq!(redo_before, 1, "fixture: there is a redo branch to protect");

    // The no-op, by both routes that skip the dialog's filter.
    assert!(
        state
            .execute(Command::ReverseSection {
                section_id: empty,
                swap_left_right: false,
            })
            .success
    );
    assert!(
        state
            .execute(Command::Batch {
                commands: vec![Command::ReverseSection {
                    section_id: empty,
                    swap_left_right: false,
                }],
            })
            .success
    );

    assert_eq!(
        state.undo_stack.lock().unwrap().redo_len(),
        redo_before,
        "a provable no-op must not consume the redo branch"
    );
}

/// R3: the gauge must survive a save and reload, not just a reverse in memory.
///
/// `reverse_parity` and the captured gauge pair are the sole inputs to
/// `default_anchor_for_component`, which pins a station at its MEASURED coordinates for a
/// datum-less component. Losing either on the way to disk moves that pin on the next open,
/// translating the whole component - and mutation testing showed the read and write paths were
/// entirely unpinned: hardcoding either to a default passed the whole suite.
#[test]
fn the_gauge_survives_a_save_and_reload() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("gauge.stalx");
    let (state, section_id) = state_with_section();

    assert!(
        state
            .execute(Command::ReverseSection {
                section_id,
                swap_left_right: false,
            })
            .success
    );
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone()),
            })
            .success
    );

    let in_memory = {
        let guard = state.project.lock().unwrap();
        let project = guard.as_ref().unwrap();
        let section = project.get_section(section_id).unwrap();
        (
            section.reverse_parity,
            section.gauge_shot_uuid.clone(),
            project.adjustment_anchor_for_tests(section_id),
        )
    };
    assert!(in_memory.0, "the reverse recorded its parity");
    assert!(in_memory.1.is_some(), "the save captured the gauge pair");

    // Reopen from disk and compare.
    let reopened = AppState::new().unwrap();
    assert!(reopened.execute(Command::OpenProject { path }).success);
    let after = {
        let guard = reopened.project.lock().unwrap();
        let project = guard.as_ref().unwrap();
        let section = project.get_section(section_id).unwrap();
        (
            section.reverse_parity,
            section.gauge_shot_uuid.clone(),
            project.adjustment_anchor_for_tests(section_id),
        )
    };
    assert_eq!(after.0, in_memory.0, "reverse_parity must round-trip");
    assert_eq!(after.1, in_memory.1, "the gauge pair must round-trip");
    assert_eq!(
        after.2, in_memory.2,
        "and the gauge must therefore land on the same station after a reopen"
    );
}

/// The import toggle and an in-app reverse must agree about the section's orientation (#24).
///
/// `reverse_parity` is documented as flipping whenever a section's shot order is reversed, and
/// the import path reverses the shots with the same shared transform - so leaving the flag false
/// there made two geometrically identical projects disagree about which side of a shot a
/// terminal plan anchor sits on, and about which end the least-squares gauge pins.
#[test]
fn importing_with_the_reverse_toggle_records_the_orientation() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Import reverse".to_string(),
        location: None,
    });

    let result = state.execute_import_sections(
        vec![ImportSectionConfig {
            section_index: 0,
            name: Some("Imported".to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![shot(0.0, 10.0, 0.0, 2.0), shot(90.0, 5.0, 2.0, 2.0)],
            placement: ImportPlacement::Origin,
            station_comments: Vec::new(),
            station_names: Vec::new(),
            reverse: true,
            azimuth_reference: None,
            explorers: None,
            override_date: None,
            surveyors: None,
        }],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "import failed: {:?}", result.error);

    let project = state.get_project_state().unwrap();
    let section = project.sections.first().unwrap();
    assert!(
        section.reverse_parity,
        "an import that reversed the shots must record it, like Project::reverse_section does"
    );
}

/// `redo_to`'s partial-failure branch is the twin of the `undo_to` case above, and had no test:
/// a half-applied redo would leave the frontend's pins mirrored with nothing on the wire to
/// mirror them by.
#[test]
fn redo_to_reports_the_reverses_it_already_applied_when_a_later_step_fails() {
    let (state, section_id) = state_with_section();

    // Two undoable steps, both undone, so there is a redo range to walk.
    assert!(
        state
            .execute(Command::ReverseSection {
                section_id,
                swap_left_right: false,
            })
            .success
    );
    assert!(
        state
            .execute(Command::UpdateSectionMetadata {
                section_id,
                updates: SectionUpdates {
                    name: Some(Some("Renamed".to_string())),
                    ..Default::default()
                },
            })
            .success
    );
    assert!(state.undo().success);
    assert!(state.undo().success);

    // Poison the SECOND redo step so step 1 (the reverse) applies and step 2 fails. The redo
    // stack hands back the most recently undone entry first, so the reverse is on top and the
    // metadata edit is underneath.
    {
        let mut stack = state.undo_stack.lock().unwrap();
        let first = stack.pop_redo().unwrap();
        let second = stack.pop_redo().unwrap();
        stack.push_redo(crate::core::UndoEntry {
            command_bytes: None,
            command: second.command,
            // Redo REPLAYS forward state rather than restoring previous values, so a poisoned
            // `values_json` would not touch it - the snapshot it re-applies is what has to be
            // unparseable.
            undo_data: UndoData::ProjectSnapshotPair {
                before: "{}".to_string(),
                after: "not-valid-json".to_string(),
            },
            timestamp: second.timestamp,
        });
        stack.push_redo(first);
    }

    let result = state.redo_to(1);
    assert!(!result.success, "step 2 must fail: {:?}", result.error);
    assert_eq!(
        result.reversed_sections,
        Some(vec![reversed_entry(section_id, 3)]),
        "step 1 re-applied the reverse and is not rolled back, so the failure must still \
         report it - otherwise the frontend mirrors nothing while the backend has renumbered"
    );
}
