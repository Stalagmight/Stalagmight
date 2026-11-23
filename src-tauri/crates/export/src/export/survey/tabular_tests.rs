//! CSV and JSON on the station-graph model: names and Station IDs read
//! through the chain (one per junction), the legacy layout and escaping,
//! determinism.

use chrono::{TimeZone, Utc};

use super::super::test_helpers::{
    cave, dotted_twelve, flat, meta, project, run, Cave, DottedTwelve, Survey,
};
use super::{json_export, write_csv, write_json};
use crate::export::CSV_HEADER;
use crate::models::survey::{ClosureStrategy, Project, StartAt};
use crate::models::{Person, SurveyCredit};

fn csv(project: &Project) -> String {
    let mut out = Vec::new();
    write_csv(project, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

/// A junction is one station: every line through it prints its one name and
/// one Station ID, whichever end of the shot it is.
#[test]
fn csv_rows_name_a_junction_once_on_every_line() {
    let Cave {
        project, t, n, b, ..
    } = cave();
    let text = csv(&project);
    let rows: Vec<&str> = text.lines().collect();
    assert_eq!(rows[0], CSV_HEADER);
    // Trunk shot 1 ends at the hub; North starts there; Back ends at T4.
    assert!(
        rows[2].starts_with(&format!("Trunk,1,Hub,{},{},", t[1], t[2])),
        "{}",
        rows[2]
    );
    assert!(
        rows[5].starts_with(&format!("North,Hub,1,{},{},", t[2], n[1])),
        "{}",
        rows[5]
    );
    assert!(
        rows[9].starts_with(&format!("Back,1,4,{},{},", b[1], t[4])),
        "{}",
        rows[9]
    );
    let shots: usize = project.lines().iter().map(|l| l.shots.len()).sum();
    assert_eq!(rows.len(), 1 + shots);
}

/// A line merged onto another line's station shows the kept station (RS 4.1):
/// both lines' rows print the kept name and the junction's one Station ID
/// (Side's own station 2 took the letter). Injection: print a line's archived
/// record name and Side's first row reads "0".
#[test]
fn csv_rows_name_a_junction_by_its_kept_name() {
    let mut s = Survey::new("Merged");
    let main = s.named("Main", "#ff0000", vec![flat(10.0, 90.0); 2]);
    let side = s.named("Side", "#00ff00", vec![flat(5.0, 0.0); 2]);
    s.merge((side, 0), (main, 2));
    let project = s.build();
    let junction = project.line(main).unwrap().station(2).unwrap();
    assert_eq!(project.line(side).unwrap().start, junction);
    let text = csv(&project);
    let rows: Vec<&str> = text.lines().collect();
    let side1 = project.line(side).unwrap().station(1).unwrap();
    assert!(
        rows[2].starts_with(&format!(
            "Main,1,2,{},{junction},",
            project.line(main).unwrap().station(1).unwrap()
        )),
        "{}",
        rows[2]
    );
    assert!(
        rows[3].starts_with(&format!("Side,2,1,{junction},{side1},")),
        "{}",
        rows[3]
    );
    let json =
        serde_json::to_value(json_export(&project, Utc.timestamp_opt(0, 0).unwrap())).unwrap();
    let side_shots = &json["lines"][1]["shots"];
    assert_eq!(side_shots[0]["from"], "2", "{side_shots}");
    assert_eq!(side_shots[0]["from_id"], junction, "{side_shots}");
}

/// A loop end a closure archives is written under its kept end's name and
/// Station ID (RS 9): Side's station 2 shows Main's "M3", so Side's rows name
/// "M3" with Main's id there, which is how a name- or id-tying reader ties
/// the loop; its own name "2" and Station ID are written nowhere. Injection:
/// write node ids, and Side's second row ends at Side's own id.
#[test]
fn a_closure_archived_end_is_written_as_its_kept_end() {
    let mut s = Survey::new("Loop");
    let main = s.named("Main", "#ff0000", vec![flat(10.0, 90.0); 3]);
    let side = s.branch(meta("Side", "#00ff00"), (main, 0), vec![flat(5.0, 0.0); 3]);
    s.rename((main, 3), "M3");
    s.closure_keeping((main, 3), (side, 2), ClosureStrategy::Batch);
    let project = s.build();
    let at = |line, i| project.line(line).unwrap().station(i).unwrap();
    let (kept, archived) = (at(main, 3), at(side, 2));
    assert_eq!(project.shown(archived), kept);
    let text = csv(&project);
    let rows: Vec<&str> = text.lines().collect();
    assert!(
        rows[5].starts_with(&format!("Side,1,M3,{},{kept},", at(side, 1))),
        "{}",
        rows[5]
    );
    assert!(
        rows[6].starts_with(&format!("Side,M3,3,{kept},{},", at(side, 3))),
        "{}",
        rows[6]
    );
    assert!(
        !text.contains(&format!(",{archived},")),
        "the archived end's own Station ID is written nowhere:\n{text}"
    );
    let json =
        serde_json::to_value(json_export(&project, Utc.timestamp_opt(0, 0).unwrap())).unwrap();
    let side_shots = &json["lines"][1]["shots"];
    assert_eq!(side_shots[1]["to"], "M3", "{side_shots}");
    assert_eq!(side_shots[1]["to_id"], kept, "{side_shots}");
    assert_eq!(side_shots[2]["from_id"], kept, "{side_shots}");
}

/// A closure whose two ends are on one line lists the kept end's name and
/// Station ID twice on that line (RS 9): Main's station 4 shows station 1.
#[test]
fn a_same_line_closure_lists_one_id_twice_on_its_line() {
    let mut s = Survey::new("Loop");
    let main = s.named("Main", "#ff0000", vec![flat(10.0, 90.0); 4]);
    s.closure_keeping((main, 1), (main, 4), ClosureStrategy::Batch);
    let project = s.build();
    let at = |i| project.line(main).unwrap().station(i).unwrap();
    let text = csv(&project);
    let rows: Vec<&str> = text.lines().collect();
    assert!(
        rows[1].starts_with(&format!("Main,0,1,{},{},", at(0), at(1))),
        "{}",
        rows[1]
    );
    assert!(
        rows[4].starts_with(&format!("Main,3,1,{},{},", at(3), at(1))),
        "{}",
        rows[4]
    );
}

/// The legacy layout: fixed decimals, LRUD, the kind label, an escaped
/// comment, a neutralised hostile station name.
#[test]
fn csv_keeps_the_legacy_format_and_escaping() {
    let mut p = project("Escape");
    let mut shots = run(0, 2, 45.0, 10.0, 5.0, 1.0);
    shots[0].left = Some(1.234);
    shots[0].comment = Some("a,b".to_string());
    let line = p
        .add_line(meta("Main", "#ff0000"), StartAt::New, shots)
        .unwrap();
    let start = p.line(line).unwrap().start;
    p.set_placement(start, 0.0, 0.0).unwrap();
    let to = p.line(line).unwrap().shots[0].to;
    p.station_mut(to).unwrap().name = "=cmd".to_string();
    let text = csv(&p);
    let rows: Vec<&str> = text.lines().collect();
    assert_eq!(
        rows[1],
        format!("Main,0,'=cmd,{start},{to},10.00,45.0,45.0,5.00,6.00,1.23,,,,Permanent,\"a,b\"")
    );
    assert!(rows[2].starts_with(&format!("Main,'=cmd,2,{to},")));
}

#[test]
fn csv_of_an_empty_project_is_the_header() {
    assert_eq!(csv(&project("Empty")), format!("{CSV_HEADER}\n"));
}

/// JSON carries the same station names and ids, the line credit aggregate,
/// the Section name, and is byte-stable for a given export time.
#[test]
fn json_records_name_stations_through_the_chain_and_are_deterministic() {
    let Cave {
        mut project,
        trunk,
        north,
        t,
        ..
    } = cave();
    let section = project.mint_section_id().unwrap();
    project.sections.push(crate::models::Section {
        id: section,
        name: "East".to_string(),
        color: "#00ff00".to_string(),
    });
    project.line_mut(trunk).unwrap().section_id = Some(section);
    let credit = SurveyCredit {
        surveyors: vec![Person::new("Ana", None).unwrap()],
        explorers: vec![Person::new("Ben", None).unwrap()],
        date: chrono::NaiveDate::from_ymd_opt(2024, 5, 6),
    };
    project.line_mut(north).unwrap().shots[1].credit = Some(std::sync::Arc::new(credit));
    let at = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();

    let export = json_export(&project, at);
    let main = &export.lines[0];
    assert_eq!(main.section.as_deref(), Some("East"));
    assert_eq!(main.shots[1].to, "Hub");
    assert_eq!(main.shots[1].to_id, Some(t[2]));
    let branch = &export.lines[1];
    assert_eq!(branch.shots[0].from, "Hub");
    assert_eq!(branch.shots[0].from_id, Some(t[2]));
    assert_eq!(branch.surveyor.as_deref(), Some("Ana"));
    assert_eq!(branch.surveyors, vec!["Ana".to_string()]);
    assert_eq!(branch.explorers, vec!["Ben".to_string()]);
    assert_eq!(branch.date.as_deref(), Some("2024-05-06"));
    assert_eq!(branch.shots[1].surveyors, vec!["Ana".to_string()]);
    assert!(main.shots[0].lrud.is_some());

    let mut one = Vec::new();
    let mut two = Vec::new();
    write_json(&project, at, &mut one).unwrap();
    write_json(&project, at, &mut two).unwrap();
    assert_eq!(one, two);
    let text = String::from_utf8(one).unwrap();
    assert!(text.contains("\"exported_at\": \"2026-11-20T12:00:00+00:00\""));
}

/// Exporting twice through the file writers gives the same CSV bytes.
#[test]
fn csv_export_is_byte_stable() {
    let cave = cave();
    let dir = tempfile::TempDir::new().unwrap();
    let (a, b) = (dir.path().join("a.csv"), dir.path().join("b.csv"));
    super::export_csv(&cave.project, &a).unwrap();
    super::export_csv(&cave.project, &b).unwrap();
    assert_eq!(std::fs::read(a).unwrap(), std::fs::read(b).unwrap());
}

/// RS-FX5, the user's example ([`dotted_twelve`]): Main / 12 is kept at
/// Branch's start, which shows "12" with Main's dot, and Branch's own 13th
/// station keeps its surveyed name "12". CSV and JSON write both by their
/// shown name "12" (RS 9), and the shown Station ID keeps them apart: Branch's
/// first row starts at the junction's id, the id Main's row into its 12 ends
/// at (the junction stays tied), while Branch's row into its own 12 ends at
/// that station's own id. A reader tying stations by name alone would join
/// them; the module docs say to tie by Station ID. Injection: write the
/// junction's archived record id on Branch and the first row no longer ties.
#[test]
fn csv_and_json_keep_a_lines_own_12_apart_from_its_dotted_junction() {
    let DottedTwelve {
        project,
        main,
        branch,
        junction,
        own_12,
    } = dotted_twelve();
    let at = |line, i| project.line(line).unwrap().station(i).unwrap();
    let text = csv(&project);
    let rows: Vec<&str> = text.lines().collect();
    // Header, Main's 14 rows, then Branch's 12.
    assert!(
        rows[12].starts_with(&format!("Main,11,12,{},{junction},", at(main, 11))),
        "{}",
        rows[12]
    );
    assert!(
        rows[15].starts_with(&format!("Branch,12,1,{junction},{},", at(branch, 1))),
        "{}",
        rows[15]
    );
    assert!(
        rows[26].starts_with(&format!("Branch,11,12,{},{own_12},", at(branch, 11))),
        "{}",
        rows[26]
    );
    assert_eq!(rows.len(), 1 + 14 + 12);

    let json =
        serde_json::to_value(json_export(&project, Utc.timestamp_opt(0, 0).unwrap())).unwrap();
    let main_shots = &json["lines"][0]["shots"];
    let branch_shots = &json["lines"][1]["shots"];
    assert_eq!(main_shots[11]["to"], "12");
    assert_eq!(main_shots[11]["to_id"], junction);
    assert_eq!(branch_shots[0]["from"], "12", "{branch_shots}");
    assert_eq!(branch_shots[0]["from_id"], junction, "tied to Main's 12");
    assert_eq!(branch_shots[11]["to"], "12", "{branch_shots}");
    assert_eq!(branch_shots[11]["to_id"], own_12, "Branch's own 12");
    assert_ne!(junction, own_12);
}
