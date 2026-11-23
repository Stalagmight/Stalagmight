//! Kept and archived stations in the file (plan 2026-11-23 RS 8.1 and 8.2):
//! every file this build writes requires `archived-stations`, and a version
//! 2 file an earlier build wrote (U1 per-line aliases, no feature) is read
//! as it was and upgraded by an explicit step: rule K per junction (the
//! node takes the kept record's id), rule K per closure in list order with
//! C1, rule L, and a report of what each line shows differently. Every file
//! here is synthetic, written by hand in the earlier form.

use crate::models::survey::{ClosureEnd, Project};

use super::test_helpers::text_of;
use super::{
    decode, decode_salvage, encode, rename_project, rewrite_local, DecodeError, DecodeOptions,
    Decoded, Mode, ARCHIVED_STATIONS,
};

const CONVENTIONS: &str = "\"conventions\":{\"length\":\"m\",\"angles\":\"deg\",\
    \"azimuth\":\"cw-from-north\",\"inclination\":\"+down\",\"depth\":\"+down\",\
    \"local\":\"enu-m\",\"gps\":\"wgs84-deg\",\"temperature\":\"degC\"}";

/// A version 2 file as an earlier build wrote it: `requires` as given (an
/// earlier build wrote `[]`), the members in canonical order, each list's
/// rows as given.
fn earlier_file(requires: &str, stations: &[&str], lines: &[&str], closures: &[&str]) -> String {
    let mut text = format!(
        "{{\"format\":\"stalx\",\"version\":2,\"solver\":1,\"requires\":[{requires}],\n{CONVENTIONS},\n\
         \"project\":{{\"name\":\"Earlier\",\"created\":\"2026-11-01T00:00:00Z\",\
         \"modified\":\"2026-11-01T00:00:00Z\"}},\n\
         \"counters\":{{\"station\":20,\"closure\":5,\"section\":0}},\n"
    );
    let list = |key: &str, rows: &[&str]| {
        if rows.is_empty() {
            String::new()
        } else {
            format!("\"{key}\":[\n{}\n],\n", rows.join(",\n"))
        }
    };
    text.push_str(&list("stations", stations));
    text.push_str(&list("lines", lines));
    text.push_str("\"placements\":[\n{\"station\":0,\"x\":0.0,\"y\":0.0}\n],\n");
    text.push_str(&list("closures", closures));
    text.push_str("\"end\":true}\n");
    text
}

/// Main: M0 -> M1 -> M2 (10 m); Side: M2 -> S1 -> S2 (40 m). The junction
/// M2 is station 2; Side knew it as S0 (alias 9). U1 joined both comments
/// into the station.
const STATIONS: [&str; 5] = [
    "[0,\"M0\"]",
    "[1,\"M1\"]",
    "[2,\"M2\",{\"comment\":\"sump. low air\",\"external\":\"ext-M2\",\"aliases\":[[9,\"S0\",1]]}]",
    "[3,\"S1\"]",
    "[4,\"S2\"]",
];
const LINES: [&str; 6] = [
    "{\"id\":0,\"name\":\"Main\",\"color\":\"#f00\",\"start\":0}",
    "[\"u1\",1,5.0,0.0,0.0,0.0,0.0]",
    "[\"u2\",2,5.0,0.0,0.0,0.0,0.0]",
    "{\"id\":1,\"name\":\"Side\",\"color\":\"#0f0\",\"start\":2}",
    "[\"u3\",3,20.0,90.0,90.0,0.0,0.0]",
    "[\"u4\",4,20.0,90.0,90.0,0.0,0.0]",
];

fn open(text: &str) -> Decoded {
    decode(text.as_bytes(), &DecodeOptions::default()).unwrap()
}

/// What `name` the slot `index` of line `line` shows.
fn shown_at(p: &Project, line: u32, index: u32) -> String {
    let node = p
        .station_at(crate::models::StationRef::new(line, index))
        .unwrap();
    p.shown_name(node).unwrap().to_string()
}

/// Every file this build writes lists the feature, whatever it holds, and
/// reads back as not earlier; the feature never shows in `Extras::requires`.
#[test]
fn every_file_written_requires_archived_stations() {
    let p = Project::new("Empty", None);
    let bytes = encode(&p, &super::Extras::default(), Mode::Normal).unwrap();
    assert!(text_of(&bytes).contains("\"requires\":[\"archived-stations\"]"));
    let decoded = open(text_of(&bytes));
    assert!(decoded.earlier.is_none());
    assert!(decoded.extras.requires.is_empty());
    assert!(decoded.read_only().is_none());
    assert!(super::KNOWN_FEATURES.contains(&ARCHIVED_STATIONS));
}

/// RS 8.2: decode reads the earlier file as written (U1 aliases as records
/// naming their lines) and leaves the upgrade for `Decoded::upgrade`. The
/// junction keeps the identity on its longest line: Side (40 m) beats Main
/// (10 m), so the node takes alias 9's id and name; the U1 joined comment
/// stays with the shown identity; the old identity is a record naming Main
/// with its external id and no comment. Main's slot is reported shown as
/// another station.
#[test]
fn an_earlier_junction_keeps_the_identity_on_its_longest_line() {
    let text = earlier_file("", &STATIONS, &LINES, &[]);
    let mut decoded = open(&text);
    let written = decoded.project.clone();
    let record = &written.stations[&2].archived[0];
    assert_eq!((record.id, record.name.as_str()), (9, "S0"));
    assert_eq!(record.lines, vec![1]);
    let report = decoded.upgrade().expect("an earlier file");
    assert!(decoded.upgrade().is_none(), "applied once");
    let p = &decoded.project;
    assert_eq!(p.validate(), Vec::new());
    assert!(!p.stations.contains_key(&2));
    let j = &p.stations[&9];
    assert_eq!(j.name, "S0");
    assert_eq!(j.comment.as_deref(), Some("sump. low air"));
    assert_eq!(j.external, None);
    assert_eq!(j.archived.len(), 1);
    let old = &j.archived[0];
    assert_eq!(
        (old.id, old.name.as_str(), old.lines.as_slice()),
        (2, "M2", [0].as_slice())
    );
    assert_eq!(
        old.comment, None,
        "U1 kept one comment, now the shown one's"
    );
    assert_eq!(old.external.as_deref(), Some("ext-M2"));
    assert_eq!(report.swapped, vec![(2, 9)]);
    assert_eq!(report.records, 1);
    assert_eq!(shown_at(p, 0, 2), "S0");
    assert_eq!(shown_at(p, 1, 0), "S0");
    let names = &report.changes.names;
    assert_eq!(names.len(), 1);
    assert_eq!(
        (
            names[0].line.as_str(),
            names[0].from.as_str(),
            names[0].to.as_str()
        ),
        ("Main", "M2", "S0")
    );
    assert_eq!(names[0].kept_line.as_deref(), Some("Side"));
    assert!(report.changes.comments_hidden.is_empty());
    // Nothing moves: the upgrade changes ids and names only.
    assert_eq!(p.placements[0].station, 0);
    assert_eq!(slot_positions(p), slot_positions(&written));
}

/// Every slot's raw position, bit for bit, in slot order.
fn slot_positions(p: &Project) -> Vec<[u64; 3]> {
    p.raw_positions()
        .slots(p)
        .iter()
        .map(|s| [s.x.to_bits(), s.y.to_bits(), s.z.to_bits()])
        .collect()
}

/// When the line the station knew itself by is the longest, nothing is
/// swapped: Side's slot shows the junction's own name and is reported.
#[test]
fn an_earlier_junction_on_its_longest_line_keeps_its_own_identity() {
    let lines = [
        LINES[0],
        "[\"u1\",1,50.0,0.0,0.0,0.0,0.0]",
        LINES[2],
        LINES[3],
        LINES[4],
        LINES[5],
    ];
    let mut decoded = open(&earlier_file("", &STATIONS, &lines, &[]));
    let report = decoded.upgrade().unwrap();
    let p = &decoded.project;
    assert!(report.swapped.is_empty());
    assert_eq!(p.stations[&2].name, "M2");
    assert_eq!(p.stations[&2].archived[0].id, 9);
    assert_eq!(shown_at(p, 1, 0), "M2");
    assert_eq!(report.changes.names[0].from, "S0");
    assert_eq!(report.changes.names[0].kept_line.as_deref(), Some("Main"));
}

/// U-P2-13a/b: of several aliases naming one line the latest names it and
/// the earlier ones are history; a nameless alias showed the station's own
/// name. Here the line keeps showing that name, so nothing is reported for
/// it, and the history record keeps its id.
#[test]
fn the_latest_alias_names_its_line_and_a_nameless_one_shows_the_station() {
    let stations = [
        STATIONS[0],
        STATIONS[1],
        "[2,\"M2\",{\"aliases\":[[7,\"old\",1],[9,null,1]]}]",
        STATIONS[3],
        STATIONS[4],
    ];
    let mut decoded = open(&earlier_file("", &stations, &LINES, &[]));
    let records = decoded.project.stations[&2].archived.clone();
    assert_eq!(records.len(), 2);
    assert!(records[0].is_history(), "alias 7 no longer names Side");
    assert_eq!(
        records[1].name, "M2",
        "a nameless alias takes the station's own"
    );
    let report = decoded.upgrade().unwrap();
    let p = &decoded.project;
    assert_eq!(p.validate(), Vec::new());
    assert_eq!(report.swapped, vec![(2, 9)], "Side is longer");
    assert!(report.changes.names.is_empty(), "every line still reads M2");
    let ids: Vec<u32> = p.stations[&9].archived.iter().map(|r| r.id).collect();
    assert!(ids.contains(&7) && ids.contains(&2));
}

/// Steps 3 and 4: each closure archives the end rule K does not keep (M1 on
/// the 10 m Main, kept S2 on the 40 m Side), and a name the line then shows
/// twice is lettered by rule L on the station shown on fewer lines: Main's
/// own "S0" yields to the junction now shown as S0 on Main.
#[test]
fn an_earlier_closure_archives_its_shorter_end_and_rule_l_letters_the_single_line_station() {
    let stations = [
        "[0,\"S0\"]",
        STATIONS[1],
        STATIONS[2],
        STATIONS[3],
        STATIONS[4],
    ];
    let closure = "{\"id\":1,\"a\":1,\"b\":4,\"strategy\":\"batch\"}";
    let mut decoded = open(&earlier_file("", &stations, &LINES, &[closure]));
    let report = decoded.upgrade().unwrap();
    let p = &decoded.project;
    assert_eq!(p.validate(), Vec::new());
    assert_eq!(p.closures[0].archived, Some(ClosureEnd::A));
    assert_eq!(report.closures_archived, 1);
    assert_eq!(shown_at(p, 0, 1), "S2");
    assert_eq!(p.stations[&1].name, "M1", "its own name is kept, not shown");
    assert_eq!(p.stations[&0].name, "S0a");
    assert_eq!(p.stations[&9].name, "S0", "the junction is never lettered");
    assert_eq!(report.lettered, 1);
    let lettered: Vec<_> = report
        .changes
        .names
        .iter()
        .filter(|n| n.lettered)
        .map(|n| (n.line.as_str(), n.from.as_str(), n.to.as_str()))
        .collect();
    assert_eq!(lettered, [("Main", "S0", "S0a")]);
}

/// Step 3 never archives the other end (U-P2-11): a second closure whose
/// rule-K end the first already archives (C1) archives nothing, and the
/// report lists it. A hidden comment of a closure-archived end is listed.
#[test]
fn a_closure_c1_refuses_keeps_both_names_and_is_reported() {
    let stations = [
        STATIONS[0],
        "[1,\"M1\",{\"comment\":\"the M1 tie-off\"}]",
        STATIONS[2],
        STATIONS[3],
        STATIONS[4],
    ];
    let first = "{\"id\":1,\"a\":1,\"b\":4,\"strategy\":\"batch\"}";
    let second = "{\"id\":2,\"a\":1,\"b\":3,\"strategy\":\"batch\"}";
    let mut decoded = open(&earlier_file("", &stations, &LINES, &[first, second]));
    let report = decoded.upgrade().unwrap();
    let p = &decoded.project;
    assert_eq!(p.validate(), Vec::new());
    let archived: Vec<_> = p.closures.iter().map(|c| c.archived).collect();
    assert_eq!(archived, [Some(ClosureEnd::A), None]);
    assert_eq!(report.changes.closures_keep_both.len(), 1);
    assert_eq!(report.changes.comments_hidden.len(), 1);
    assert_eq!(report.changes.comments_hidden[0].station, "M1");
}

/// A junction whose every line knew it by an alias (A4 as written) shows the
/// longest one's identity; its own becomes history.
#[test]
fn a_junction_every_line_knew_by_an_alias_takes_the_longest_one() {
    let stations = [
        STATIONS[0],
        STATIONS[1],
        "[2,\"X\",{\"aliases\":[[8,\"M2\",0],[9,\"S0\",1]]}]",
        STATIONS[3],
        STATIONS[4],
    ];
    let mut decoded = open(&earlier_file("", &stations, &LINES, &[]));
    let report = decoded.upgrade().unwrap();
    let p = &decoded.project;
    assert_eq!(p.validate(), Vec::new());
    assert_eq!(report.swapped, vec![(2, 9)]);
    let history = p.stations[&9].archived.iter().find(|r| r.id == 2).unwrap();
    assert!(history.is_history());
    assert_eq!(shown_at(p, 0, 2), "S0");
}

/// The file's own damage is still damage: the upgrade settles only what an
/// earlier file's rules allowed.
#[test]
fn an_earlier_file_with_its_own_damage_is_refused() {
    let lines = [LINES[0], "[\"u1\",15,5.0,0.0,0.0,0.0,0.0]"];
    let text = earlier_file("", &STATIONS[..1], &lines, &[]);
    assert!(matches!(
        decode(text.as_bytes(), &DecodeOptions::default()),
        Err(DecodeError::Invalid { .. })
    ));
}

/// Salvage of an earlier file repairs its own damage, then upgrades it.
#[test]
fn salvage_upgrades_an_earlier_file() {
    let mut stations = STATIONS.to_vec();
    stations.push("[11,\"orphan\"]");
    let text = earlier_file("", &stations, &LINES, &[]);
    assert!(decode(text.as_bytes(), &DecodeOptions::default()).is_err());
    let (repaired, report) = decode_salvage(text.as_bytes());
    let repaired = repaired.expect("salvaged");
    assert_eq!(repaired.validate(), Vec::new());
    let upgrade = report.upgrade.expect("upgraded");
    assert_eq!(upgrade.swapped, vec![(2, 9)]);
}

/// Until its first save the file stays as an earlier build wrote it: a
/// local-only rewrite and a rename copy `requires` and the body, so the next
/// open upgrades it again. The upgraded survey saves with the feature and
/// reads back as it was upgraded.
#[test]
fn an_earlier_file_stays_earlier_until_a_save_writes_the_survey() {
    let text = earlier_file("", &STATIONS, &LINES, &[]);
    let mut decoded = open(&text);
    let mut extras = decoded.extras.clone();
    extras
        .local
        .display
        .insert("grid".to_string(), "true".to_string());
    let rewritten = rewrite_local(text.as_bytes(), &extras).unwrap();
    assert!(text_of(&rewritten).contains("\"requires\":[]"));
    assert!(open(text_of(&rewritten)).earlier.is_some());
    let renamed = rename_project(text.as_bytes(), "Renamed").unwrap();
    assert!(open(text_of(&renamed)).earlier.is_some());

    decoded.upgrade().unwrap();
    let saved = encode(&decoded.project, &decoded.extras, Mode::Normal).unwrap();
    let again = open(text_of(&saved));
    assert!(again.earlier.is_none());
    assert!(again.project == decoded.project);
}

/// An earlier file that nothing in this build reads differently (no
/// junction alias, no closure) upgrades to an empty report: no notice.
#[test]
fn an_earlier_file_without_junctions_or_closures_upgrades_to_nothing() {
    let stations = [STATIONS[0], STATIONS[1], "[2,\"M2\"]"];
    let mut decoded = open(&earlier_file("", &stations, &LINES[..3], &[]));
    assert!(decoded.upgrade().unwrap().is_empty());
}

/// An earlier file that also requires an unknown feature is read-only, and
/// still upgraded in memory (its model must hold this build's rules).
#[test]
fn an_earlier_file_with_an_unknown_feature_is_read_only_and_upgraded() {
    let text = earlier_file("\"zz-future\"", &STATIONS, &LINES, &[]);
    let mut decoded = open(&text);
    assert_eq!(
        decoded.read_only().unwrap().unknown_features,
        vec!["zz-future".to_string()]
    );
    assert!(decoded.upgrade().is_some());
    assert_eq!(decoded.project.validate(), Vec::new());
}
