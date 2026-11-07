//! #576 NF-FIX3R-3: one closure per loop, none onto a point the merges
//! already joined. Synthetic surveys only.

use crate::models::section::{ImportClosure, ImportPlacement};

use super::test_support::parse_srv;
use super::{convert_walls_to_import_configs, ParsedWalls, WallsImportOptions};

fn ties(closures: &[ImportClosure]) -> Vec<String> {
    let mut ties: Vec<String> = closures
        .iter()
        .map(|c| {
            format!(
                "{}:{}->{}:{}",
                c.from_section_index, c.from_station_index, c.to_section_index, c.to_station_index
            )
        })
        .collect();
    ties.sort();
    ties
}

/// Main E1-E2-J-E3, Side J-S1-S2-E3 (fixed at S1) and Loop E3-L1-J-X1: J and
/// E3 are each shared by all three sections. The spanning tree roots Side at
/// its fix, merges Main's J onto Side's J and Loop's E3 onto Main's E3, so
/// Side's J IS the canonical J (Main's) already: a closure tying the two is a
/// point tied to itself (zero misclosure). The two loops that remain are
/// Side's E3 against Main's and Loop's J against Main's.
#[test]
fn a_station_three_sections_share_gets_no_closure_onto_its_merged_point() {
    let main = parse_srv(
        "#UNITS meters order=DAV\n\
         E1 E2 10 90 0\n\
         E2 J 10 90 0\n\
         J E3 10 90 0\n",
    );
    let side = parse_srv(
        "#UNITS meters order=DAV\n\
         #FIX S1 W87:18:44 N20:25:42 0\n\
         J S1 10 0 0\n\
         S1 S2 10 0 0\n\
         S2 E3 14.5 135 0\n",
    );
    let lp = parse_srv(
        "#UNITS meters order=DAV\n\
         E3 L1 10 180 0\n\
         L1 J 14 315 0\n\
         J X1 10 200 0\n",
    );
    let parsed = ParsedWalls {
        project_name: "Synthetic".to_string(),
        book_count: 0,
        surveys: vec![main, side, lp],
        georef: None,
        missing_files: Vec::new(),
        skipped_attachments: 0,
        warnings: Vec::new(),
    };
    let (configs, closures, _) =
        convert_walls_to_import_configs(&parsed, &WallsImportOptions::default()).unwrap();
    let placements: Vec<String> = configs
        .iter()
        .map(|c| format!("{}:{:?}", c.section_index, c.placement))
        .collect();
    let ties = ties(&closures);
    println!("placements: {placements:?}");
    println!("closures: {} {ties:?} (expected 2)", closures.len());
    assert!(
        configs.iter().any(|c| c.section_index == 1
            && matches!(
                c.placement,
                ImportPlacement::ZeroDatum {
                    station_index: 1,
                    ..
                }
            )),
        "premise: Side is rooted at its fix S1: {placements:?}"
    );
    assert_eq!(ties, ["1:3->0:3", "2:2->0:2"]);
}

/// Main Z-A-X; Fixed F-Z-X-Y (fixed at F); Spur X-W-Y. Rooted at its fix,
/// Fixed takes Main by Z and Spur by X, so Fixed's X and Spur's X are ONE
/// point, distinct from Main's (canonical) X; and Fixed's Z is Main's Z. One
/// closure ties the X point to Main's X - not one per occurrence, which
/// counted that loop twice in the least-squares solve - none ties the Z
/// point to itself, and Spur's Y closes the second loop.
#[test]
fn a_merged_point_holding_two_occurrences_gets_one_closure() {
    let main = parse_srv(
        "#UNITS meters order=DAV\n\
         Z A 10 90 0\n\
         A X 10 90 0\n",
    );
    let fixed = parse_srv(
        "#UNITS meters order=DAV\n\
         #FIX F W87:18:44 N20:25:42 0\n\
         F Z 10 0 0\n\
         Z X 20 90 0\n\
         X Y 10 0 0\n",
    );
    let spur = parse_srv(
        "#UNITS meters order=DAV\n\
         X W 10 45 0\n\
         W Y 7 315 0\n",
    );
    let parsed = ParsedWalls {
        project_name: "Synthetic".to_string(),
        book_count: 0,
        surveys: vec![main, fixed, spur],
        georef: None,
        missing_files: Vec::new(),
        skipped_attachments: 0,
        warnings: Vec::new(),
    };
    let (configs, closures, _) =
        convert_walls_to_import_configs(&parsed, &WallsImportOptions::default()).unwrap();
    let placements: Vec<String> = configs
        .iter()
        .map(|c| format!("{}:{:?}", c.section_index, c.placement))
        .collect();
    let ties = ties(&closures);
    println!("placements: {placements:?}");
    println!("closures: {} {ties:?} (expected 2)", closures.len());
    assert!(
        configs.iter().any(|c| c.section_index == 2
            && matches!(
                c.placement,
                ImportPlacement::FromImporting {
                    import_station: 0,
                    target_section_index: 1,
                    target_station_index: 2,
                }
            )),
        "premise: Spur's X merges onto Fixed's X: {placements:?}"
    );
    assert_eq!(ties, ["1:2->0:2", "2:2->1:3"]);
}
