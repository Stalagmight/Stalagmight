//! #576 B3-EXTRACT: Compass placements and closures pinned for looped
//! synthetic surveys. The values were captured from the converter before the
//! station-graph back end was shared with Walls, so a change to Compass's
//! merge-point choice (which shared station becomes the merge, and in which
//! order sections are created) fails here. Synthetic surveys only.

use compass_data::Datum;

use super::tests::{parse_dat_text, synthetic_survey_text};
use super::{convert_compass_to_import_configs, CompassImportOptions, FixedStation};

/// What the converter decided, in a form a test can compare: one line per
/// config in emitted order, the closures in emitted order, and the warnings.
fn convert(texts: &[String], anchor: Option<&str>) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut parsed = parse_dat_text(&texts.concat());
    if let Some(name) = anchor {
        parsed.fixed_stations.push(FixedStation {
            name: name.to_string(),
            easting: 500_000.0,
            northing: 2_200_000.0,
            elevation: 0.0,
            datum: Some(Datum::WGS1984),
            utm_zone: Some(16),
        });
        parsed.utm_zone = Some(16);
        parsed.datum = Some(Datum::WGS1984);
    }
    let (configs, closures, warnings) =
        convert_compass_to_import_configs(&parsed, &CompassImportOptions::default()).unwrap();
    let placements: Vec<String> = configs
        .iter()
        .map(|c| {
            let placement = match &c.placement {
                crate::models::ImportPlacement::ZeroDatum { station_index, .. } => {
                    format!("ZeroDatum@{station_index}")
                }
                crate::models::ImportPlacement::FromImporting {
                    import_station,
                    target_section_index,
                    target_station_index,
                } => format!("{import_station}->{target_section_index}:{target_station_index}"),
                other => format!("{other:?}"),
            };
            format!(
                "{} {}: {placement}",
                c.section_index,
                c.name.as_deref().unwrap_or("")
            )
        })
        .collect();
    let ties: Vec<String> = closures
        .iter()
        .map(|c| {
            format!(
                "{}:{}->{}:{}",
                c.from_section_index, c.from_station_index, c.to_section_index, c.to_station_index
            )
        })
        .collect();
    println!("placements: {placements:?}");
    println!("closures: {ties:?}");
    println!("warnings: {warnings:?}");
    (placements, ties, warnings)
}

const FORMAT: &str = "DDDDLRUDLADN";

/// Main A1-A2-J-A3; Side J-B1-B2-A1 (a loop through two surveys); Spur
/// C1-J-C2-C1 (a loop inside one survey, fixed at C2); Far D1-D2-D3-D1 (a
/// loop in a separate component). J is shared by three surveys, so which
/// section each one merges onto depends on how chains are connected:
/// Compass connects every pair sharing a station, so Main and Side both
/// merge onto the anchored Spur.
fn looped_surveys() -> Vec<String> {
    vec![
        synthetic_survey_text(
            "Main",
            FORMAT,
            &[
                "A1 A2 10.00 90.00 0.00 1 1 1 1",
                "A2 J 10.00 90.00 0.00 1 1 1 1",
                "J A3 10.00 90.00 0.00 1 1 1 1",
            ],
        ),
        synthetic_survey_text(
            "Side",
            FORMAT,
            &[
                "J B1 10.00 0.00 0.00 1 1 1 1",
                "B1 B2 10.00 270.00 0.00 1 1 1 1",
                "B2 A1 22.00 225.00 0.00 1 1 1 1",
            ],
        ),
        synthetic_survey_text(
            "Spur",
            FORMAT,
            &[
                "C1 J 10.00 180.00 0.00 1 1 1 1",
                "J C2 10.00 90.00 0.00 1 1 1 1",
                "C2 C1 14.00 315.00 0.00 1 1 1 1",
            ],
        ),
        synthetic_survey_text(
            "Far",
            FORMAT,
            &[
                "D1 D2 10.00 90.00 0.00 1 1 1 1",
                "D2 D3 10.00 0.00 0.00 1 1 1 1",
                "D3 D1 14.00 225.00 0.00 1 1 1 1",
            ],
        ),
    ]
}

#[test]
fn anchored_looped_surveys_keep_their_merges_and_closures() {
    let (placements, ties, warnings) = convert(&looped_surveys(), Some("C2"));
    assert_eq!(
        placements,
        [
            "2 Spur: ZeroDatum@2",
            "3 Far: Origin",
            "0 Main: 2->2:1",
            "1 Side: 0->2:1"
        ]
    );
    assert_eq!(ties, ["1:3->0:0", "2:3->2:0", "3:3->3:0"]);
    assert_eq!(
        warnings,
        [
            "Section 'Far' starts a separate component not connected to the georeferenced \
             survey; placed at origin"
        ]
    );
}

#[test]
fn unanchored_looped_surveys_keep_their_merges_and_closures() {
    let (placements, ties, warnings) = convert(&looped_surveys(), None);
    assert_eq!(
        placements,
        [
            "0 Main: Origin",
            "3 Far: Origin",
            "1 Side: 0->0:2",
            "2 Spur: 1->0:2"
        ]
    );
    assert_eq!(ties, ["1:3->0:0", "2:3->2:0", "3:3->3:0"]);
    // [#576 NF-W4M-3] No fixed station: there is no georeferenced survey to name.
    assert_eq!(
        warnings,
        [
            "Section 'Far' starts a separate component not connected to section 'Main'; \
             no fixed station georeferences either, so both are placed at origin"
        ]
    );
}

#[test]
fn an_anchor_in_a_later_component_keeps_its_merges_and_closures() {
    let (placements, ties, warnings) = convert(&looped_surveys(), Some("D2"));
    assert_eq!(
        placements,
        [
            "3 Far: ZeroDatum@1",
            "0 Main: Origin",
            "1 Side: 0->0:2",
            "2 Spur: 1->0:2"
        ]
    );
    assert_eq!(ties, ["1:3->0:0", "2:3->2:0", "3:3->3:0"]);
    assert_eq!(
        warnings,
        [
            "Section 'Main' starts a separate component not connected to the georeferenced \
             survey; placed at origin"
        ]
    );
}
