//! Orphan placement suggestions and the unanchored-section count.
//!
//! Lines 14998-15307 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

/// A leaf shot sequence with enough distinct shots (>= 3 free) to qualify as
/// an orphan leaf candidate, repeated identically so a placement match against
/// it scores near-perfect.
fn orphan_leaf_shots() -> Vec<ShotData> {
    vec![
        shot(45.0, 45.0, 10.0, 1.0, 2.0),
        shot(90.0, 90.0, 8.0, 2.0, 1.0),
        shot(135.0, 135.0, 6.0, 0.0, 0.0),
        shot(180.0, 180.0, 4.0, 1.0, 1.0),
    ]
}

#[test]
fn suggest_orphan_placements_finds_intra_import_suggestion_on_first_import() {
    // #446: a first import into an EMPTY project used to suggest nothing even
    // when the imported file contained both an anchored component and a
    // floating (unanchored) one - `orphan_components` was computed only over
    // PRE-EXISTING sections, and a first import has none. It must now
    // consider the whole post-import project, so the floating section
    // becomes a placement candidate against the anchored one from the SAME
    // import.
    let mut project = Project::new("FirstImport".to_string(), None);
    let anchored = project
        .add_section(SectionData {
            shots: orphan_leaf_shots(),
            ..Default::default()
        })
        .id;
    let floating = project
        .add_section(SectionData {
            shots: orphan_leaf_shots(),
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(anchored, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });

    let suggestions = project.suggest_orphan_placements(
        &[anchored, floating],
        &crate::config::PlacementSuggestionConfig::default(),
    );

    assert!(
        !suggestions.is_empty(),
        "a floating component imported alongside an anchored one in the same \
         file must get at least one placement suggestion (#446)"
    );
    assert!(
        suggestions.iter().all(|s| s.orphan_section_id == floating),
        "the only orphan in this project is the floating section: {suggestions:?}"
    );
}

#[test]
fn suggest_orphan_placements_never_pairs_a_pre_existing_orphan_with_a_pre_existing_anchor() {
    // #446: a pre-existing floating component that had its chance against its
    // own import must not start getting matched against a pre-existing
    // anchored section just because SOME LATER, unrelated import happens.
    // `target_sections` is built from `newly_imported_section_ids` alone, so
    // a pre-existing anchor can never be a suggestion target. Both fixtures
    // are shot-IDENTICAL (`orphan_leaf_shots`), so a broken guard would match
    // them with a near-perfect score - the empty result is the guard, not an
    // accident of mismatched shot shapes.
    let mut project = Project::new("LaterImport".to_string(), None);
    let pre_anchor = project
        .add_section(SectionData {
            shots: orphan_leaf_shots(),
            ..Default::default()
        })
        .id;
    let pre_orphan = project
        .add_section(SectionData {
            shots: orphan_leaf_shots(),
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(pre_anchor, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });

    // A later, unrelated import that offers no anchored target of its own -
    // the ONLY conceivable match left is the forbidden pre/pre pairing.
    let new_section = project
        .add_section(SectionData {
            shots: vec![shot(200.0, 200.0, 20.0, 3.0, 3.0)],
            ..Default::default()
        })
        .id;

    let suggestions = project.suggest_orphan_placements(
        &[new_section],
        &crate::config::PlacementSuggestionConfig::default(),
    );

    assert!(
        suggestions.is_empty(),
        "the only possible pairing here (pre-existing orphan onto pre-existing \
         anchor) is forbidden, so nothing may be suggested (#446): {pre_orphan:?} {suggestions:?}"
    );
}

#[test]
fn suggest_orphan_placements_still_suggests_a_pre_existing_orphan_against_a_newly_imported_anchor()
{
    // The positive case for the guard above: a pre-existing orphan is NOT
    // suggestion-dead - it can still be matched against a target from a
    // LATER import, as long as that target is itself newly imported AND
    // datum-anchored. Same shot-identical fixture as the forbidden case, so
    // this proves the mechanism works at all (not just that it's always
    // empty).
    let mut project = Project::new("LaterImportAnchored".to_string(), None);
    let pre_orphan = project
        .add_section(SectionData {
            shots: orphan_leaf_shots(),
            ..Default::default()
        })
        .id;
    let new_target = project
        .add_section(SectionData {
            shots: orphan_leaf_shots(),
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(new_target, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });

    let suggestions = project.suggest_orphan_placements(
        &[new_target],
        &crate::config::PlacementSuggestionConfig::default(),
    );

    assert!(
        suggestions
            .iter()
            .any(|s| s.orphan_section_id == pre_orphan && s.target_section_id == new_target),
        "a pre-existing orphan must still be suggested against a newly \
         imported, datum-anchored target (#446): {suggestions:?}"
    );
}

#[test]
fn suggest_orphan_placements_never_suggests_two_floating_components_onto_each_other() {
    // #446: a suggestion TARGET must itself be datum-anchored - otherwise two
    // floating components can be suggested onto each other, a "placement"
    // that is circular and anchors nothing. A datum exists ELSEWHERE in the
    // project (on an unrelated section) so the project-wide
    // no-datum-at-all early-out does NOT short-circuit this case; only the
    // per-target anchor check does.
    let mut project = Project::new("Floating".to_string(), None);
    let elsewhere = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 3.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(elsewhere, 0).into(),
        gps: GpsCoordinates::new(1.0, 2.0, 0.0).unwrap(),
    });

    let a = project
        .add_section(SectionData {
            shots: orphan_leaf_shots(),
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: orphan_leaf_shots(),
            ..Default::default()
        })
        .id;

    let suggestions = project.suggest_orphan_placements(
        &[a, b],
        &crate::config::PlacementSuggestionConfig::default(),
    );

    assert!(
        suggestions.is_empty(),
        "two floating components must never be suggested onto each other - \
         neither is datum-anchored (#446): {suggestions:?}"
    );
}

#[test]
fn suggest_orphan_placements_short_circuits_when_the_project_has_no_datum_at_all() {
    // #446 perf: with no datum anywhere in the project, every component is
    // technically "orphaned" - without an early-out this pathological case
    // still walks every component's leaves against every other. The cheap
    // `zero_datums`-resolve check must return empty before that walk starts.
    let mut project = Project::new("NoDatumAtAll".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: orphan_leaf_shots(),
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: orphan_leaf_shots(),
            ..Default::default()
        })
        .id;

    let suggestions = project.suggest_orphan_placements(
        &[a, b],
        &crate::config::PlacementSuggestionConfig::default(),
    );

    assert!(suggestions.is_empty());
}

#[test]
fn count_unanchored_sections_is_zero_for_an_empty_project() {
    let project = Project::new("Empty".to_string(), None);
    assert_eq!(project.count_unanchored_sections(&[]), 0);
}

#[test]
fn count_unanchored_sections_counts_every_member_of_a_merged_orphan_component() {
    let mut project = Project::new("Merged".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    // No datum anywhere - A and B form one orphan component of size 2.
    assert_eq!(project.count_unanchored_sections(&[a]), 2);
}

#[test]
fn count_unanchored_sections_treats_a_datum_reached_through_a_merge_as_anchored() {
    let mut project = Project::new("MergedAnchored".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 1).into(),
        gps: GpsCoordinates::new(1.0, 2.0, 0.0).unwrap(),
    });

    // The datum lives on A alone, but the merge means B's rigid group is
    // anchored too - the component has a datum, so it is not orphaned.
    assert_eq!(project.count_unanchored_sections(&[a, b]), 0);
}

#[test]
fn count_unanchored_sections_is_scoped_to_the_newly_imported_component() {
    // A clean import into a project with unrelated, pre-existing orphans
    // must not blame THIS import's receipt for orphans it didn't create
    // (#446).
    let mut project = Project::new("Scoped".to_string(), None);
    let _pre_existing_orphan = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let new_anchored = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(new_anchored, 0).into(),
        gps: GpsCoordinates::new(1.0, 2.0, 0.0).unwrap(),
    });

    assert_eq!(
        project.count_unanchored_sections(&[new_anchored]),
        0,
        "the pre-existing orphan sitting elsewhere in the project is not \
         this import's problem"
    );
}
