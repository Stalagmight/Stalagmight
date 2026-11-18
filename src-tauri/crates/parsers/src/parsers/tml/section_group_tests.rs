//! Auto-grouping of an Ariane import into Sections (#327): the lines one
//! Ariane section fanned out into join a Section of that name.

use super::test_helpers::*;
use super::*;

#[test]
fn the_base_name_strips_branch_and_trip_suffixes() {
    assert_eq!(ariane_section_of("Main"), "Main");
    assert_eq!(ariane_section_of("Main (branch 2)"), "Main");
    assert_eq!(ariane_section_of("Main (trip 3)"), "Main");
    assert_eq!(ariane_section_of("Main (branch 2) (trip 3)"), "Main");
    assert_eq!(
        ariane_section_of("A (branch 2) + B"),
        "A (branch 2) + B",
        "a joined name is its own base"
    );
    assert_eq!(
        ariane_section_of("Hall (trip x)"),
        "Hall (trip x)",
        "only a numbered suffix is the linearizer's"
    );
}

#[test]
fn a_section_that_fanned_out_groups_its_lines() {
    let shots = [
        tml_shot_xml_az_len(1, -1, "Sump", 90.0, 2.0),
        tml_shot_xml_az_len(2, -1, "Sump", 90.0, 30.0),
        tml_shot_xml_az_len(3, 2, "Sump", 90.0, 30.0),
        tml_shot_xml_az_len(4, -1, "Side", 0.0, 10.0),
        tml_shot_xml_az_len(5, 4, "Side", 0.0, 10.0),
    ]
    .join("\n");
    let tml = parse_synthetic_tmlu(&wrap_xml(&shots));
    let (configs, _, _) = convert_tml_to_import_configs(&tml, false, false).unwrap();

    let grouped: Vec<(&str, Option<&str>)> = configs
        .iter()
        .map(|c| (c.name.as_deref().unwrap(), c.cave_section.as_deref()))
        .collect();
    assert!(
        grouped.contains(&("Sump", Some("Sump"))),
        "the mainline joins its section's group: {grouped:?}"
    );
    assert!(
        grouped.contains(&("Sump (branch 2)", Some("Sump"))),
        "the branch joins it too: {grouped:?}"
    );
    assert!(
        grouped.contains(&("Side", None)),
        "a section that stayed one line is not grouped: {grouped:?}"
    );
}

#[test]
fn a_lone_surviving_branch_is_not_grouped() {
    // "Gallery" fans out, but every shot of its branch is excluded: one line
    // is left, which `simplify_lone_branch_names` renames back to "Gallery".
    let mut shots = vec![
        tml_shot_xml_az_len(1, -1, "Gallery", 90.0, 30.0),
        tml_shot_xml_az_len(2, 1, "Gallery", 90.0, 30.0),
    ];
    let excluded = tml_shot_xml_az_len(3, -1, "Gallery", 90.0, 2.0)
        .replace("<Excluded>false</Excluded>", "<Excluded>true</Excluded>");
    shots.push(excluded);
    let tml = parse_synthetic_tmlu(&wrap_xml(&shots.join("\n")));
    let (configs, _, _) = convert_tml_to_import_configs(&tml, false, false).unwrap();
    assert_eq!(configs.len(), 1);
    assert_eq!(configs[0].name.as_deref(), Some("Gallery"));
    assert_eq!(configs[0].cave_section, None);
}
