//! Command contracts the Playwright IPC mock used to re-implement (#576 E3).
//!
//! The browser tier's mock (`e2e-playwright/mocks/tauri-ipc.ts`) once answered
//! these commands with its own copy of the backend's rules, and specs asserted
//! that copy. The mock now answers canned replies, so the two rules nothing
//! else pinned are pinned here, through `AppState::execute`: a reverse of a
//! shot-less section is left out of `reversed_sections` (nothing was
//! renumbered, and the count is the frontend's mirror divisor), and
//! `UnmergeStationPair` refuses a pair that is not directly merged, leaving the
//! project untouched.

#![expect(
    clippy::expect_used,
    reason = "test module: a failed expect is the failure report"
)]

use super::AppState;
use crate::core::Command;
use crate::models::{ShotData, StationRef};
use crate::test_fixtures::{state_with_project, straight_shot};

fn create_section(state: &AppState, name: &str, shots: Vec<ShotData>) -> u32 {
    let result = state.execute(Command::CreateSection {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots,
    });
    assert!(result.success, "{:?}", result.error);
    result.section_id.expect("a created section id")
}

/// A reverse of a section with no shots succeeds but renumbers nothing, so it
/// must not appear in `reversed_sections` - on its own, or beside a real one in
/// a batch. A `{section, shot_count: 0}` entry would hand the frontend's mirror
/// a zero divisor.
#[test]
fn a_shot_less_section_is_left_out_of_reversed_sections() {
    let state = state_with_project("CaveA");
    let real = create_section(
        &state,
        "StationA1",
        vec![straight_shot(90.0, 10.0), straight_shot(90.0, 5.0)],
    );
    let empty = create_section(&state, "Waypoint", Vec::new());

    let alone = state.execute(Command::ReverseSection {
        section_id: empty,
        swap_left_right: false,
    });
    assert!(alone.success, "{:?}", alone.error);
    assert!(
        alone.reversed_sections.is_none(),
        "a shot-less reverse renumbered nothing: {:?}",
        alone.reversed_sections
    );

    let batch = state.execute(Command::Batch {
        commands: vec![
            Command::ReverseSection {
                section_id: real,
                swap_left_right: false,
            },
            Command::ReverseSection {
                section_id: empty,
                swap_left_right: false,
            },
        ],
    });
    assert!(batch.success, "{:?}", batch.error);
    let reported: Vec<(u32, u32)> = batch
        .reversed_sections
        .unwrap_or_default()
        .iter()
        .map(|r| (r.section_id, r.shot_count))
        .collect();
    assert_eq!(
        reported,
        vec![(real, 2)],
        "only the section with shots is reported"
    );
}

/// `UnmergeStationPair` on two stations that are not directly merged is a
/// refusal, not a silent success, and it changes nothing.
#[test]
fn unmerging_a_pair_that_is_not_merged_is_refused() {
    let state = state_with_project("CaveA");
    let a = create_section(&state, "A", vec![straight_shot(90.0, 10.0)]);
    let b = create_section(&state, "B", vec![straight_shot(90.0, 8.0)]);
    let c = create_section(&state, "C", vec![straight_shot(90.0, 6.0)]);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });
    assert!(merged.success, "{:?}", merged.error);
    let before =
        serde_json::to_string(&state.get_project_state().expect("project")).expect("serialize");

    let refused = state.execute(Command::UnmergeStationPair {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(c, 0),
    });
    assert!(
        !refused.success,
        "a pair that is not directly merged must be refused"
    );
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|e| e.contains("not directly merged")),
        "{:?}",
        refused.error
    );
    let after =
        serde_json::to_string(&state.get_project_state().expect("project")).expect("serialize");
    assert_eq!(
        before, after,
        "a refused unmerge leaves the project untouched"
    );
}

/// The import's virtual-shot override is unconditional: `AllReferences` lands
/// every virtual shot of the file as a Reference and `AllJumps` as a Jump,
/// whatever the chain rule would have said, and real shots stay Normal. Only the
/// `AllJumps` side had an assertion (`importing_a_second_file_leaves_existing_shot_kinds_alone`).
#[test]
fn the_import_override_lands_virtual_shots_as_the_chosen_kind() {
    use super::VirtualShotMode;
    use crate::models::{ImportPlacement, ImportSectionConfig, ShotKind};

    let config = |shots: Vec<ShotData>| ImportSectionConfig {
        section_index: 0,
        name: Some("CaveA".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots,
        station_comments: Vec::new(),
        placement: ImportPlacement::Origin,
        reverse: false,
        azimuth_reference: None,
    };
    // A virtual shot bounded by real survey: the chain rule (Auto) calls it a Jump.
    let virtual_run = || {
        vec![
            straight_shot(90.0, 5.0),
            ShotData {
                kind: ShotKind::Jump,
                ..straight_shot(90.0, 4.0)
            },
            straight_shot(90.0, 6.0),
        ]
    };
    let kinds_after = |mode: VirtualShotMode| {
        let state = state_with_project("CaveA");
        let imported = state.execute_import_sections(
            vec![config(virtual_run())],
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
            mode,
        );
        assert!(imported.success, "{:?}", imported.error);
        state.get_project_state().expect("project").sections[0]
            .shots
            .iter()
            .map(|s| s.kind)
            .collect::<Vec<_>>()
    };

    assert_eq!(
        kinds_after(VirtualShotMode::AllReferences),
        vec![ShotKind::Normal, ShotKind::Reference, ShotKind::Normal]
    );
    assert_eq!(
        kinds_after(VirtualShotMode::AllJumps),
        vec![ShotKind::Normal, ShotKind::Jump, ShotKind::Normal]
    );
}

/// An import lands each config as a section, joins a dependent section's
/// `import_station` to its target with a merge (`ImportPlacement::FromImporting`),
/// creates the loop closures it was handed, and counts all of it on the receipt.
/// The mocked tier used to derive these numbers itself for its Ariane fixture
/// (and asserted "2 sections - 5 shots - 1 loop" off its own arithmetic).
#[test]
fn an_import_merges_a_dependent_section_and_counts_what_landed() {
    use super::VirtualShotMode;
    use crate::models::{ImportClosure, ImportPlacement, ImportSectionConfig};

    let config =
        |index: u32, shots: Vec<ShotData>, placement: ImportPlacement| ImportSectionConfig {
            section_index: index,
            name: Some(format!("Section{index}")),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            quality: 1.0,
            shots,
            station_comments: Vec::new(),
            placement,
            reverse: false,
            azimuth_reference: None,
        };
    let toward = |azimuth: f64| ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length: 10.0,
        ..Default::default()
    };
    let state = state_with_project("CaveA");
    let result = state.execute_import_sections(
        vec![
            config(
                0,
                vec![toward(90.0), toward(90.0), toward(90.0)],
                ImportPlacement::Origin,
            ),
            config(
                1,
                vec![toward(45.0), toward(135.0)],
                ImportPlacement::FromImporting {
                    import_station: 0,
                    target_section_index: 0,
                    target_station_index: 1,
                },
            ),
        ],
        vec![ImportClosure {
            from_section_index: 1,
            from_station_index: 2,
            to_section_index: 0,
            to_station_index: 3,
        }],
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "{:?}", result.error);
    let receipt = result.import_receipt.expect("an import receipt");
    assert_eq!(
        (
            receipt.sections_imported,
            receipt.shot_count,
            receipt.closures_requested,
            receipt.closures_created
        ),
        (2, 5, 1, 1),
        "sections, shots, closures requested and created"
    );

    let project = state.get_project_state().expect("project");
    let ids: Vec<u32> = project.sections.iter().map(|s| s.id).collect();
    assert_eq!(ids.len(), 2);
    let merged: Vec<(Option<StationRef>, Option<StationRef>)> = project
        .station_merges
        .iter()
        .map(|m| {
            (
                project.resolve_key(&m.station_a),
                project.resolve_key(&m.station_b),
            )
        })
        .collect();
    assert_eq!(
        merged,
        vec![(
            Some(StationRef::new(ids[1], 0)),
            Some(StationRef::new(ids[0], 1))
        )],
        "the dependent section's station 0 merges with its target station"
    );
    assert_eq!(project.loop_closures.len(), 1);
}
