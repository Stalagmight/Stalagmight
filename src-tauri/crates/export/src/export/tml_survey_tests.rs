//! The Ariane export on the station-graph model: the rows the graph walk
//! builds, ids and parent links, GPS on the STARTs, comments, determinism,
//! the round trip through the importer, and byte equality with the legacy
//! exporter where the two models agree.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use super::super::{AzimuthExport, TmlProfile, SHORT};
use super::{build_rows, build_xml};
use crate::export::survey::test_helpers::{cave, gps_at, meta, project, run, Cave};
use crate::models::survey::{Project, StartAt, ZeroDatum};
use crate::models::StationId;

fn rows(project: &Project) -> Vec<super::ExportShot> {
    let decision = AzimuthExport::decide_from(
        project
            .lines()
            .iter()
            .filter(|l| !l.shots.is_empty())
            .map(|l| project.azimuth_reference_for(l)),
    );
    build_rows(project, TmlProfile::User, decision).0
}

fn id(s: StationId) -> i64 {
    i64::from(s)
}

fn row(rows: &[super::ExportShot], id: i64) -> &super::ExportShot {
    rows.iter()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("no row {id}"))
}

/// Every row's `ID` is the station it ends at, its `FromID` the station it
/// starts from, and the blocks are the legacy ones: the entrance START, a
/// branch hung off its junction, a line ending at a junction reversed, a line
/// passing a junction split in two, an entrance with a mid-line datum split
/// with its pre-zero-datum block, a free piece as its own entrance.
#[test]
fn rows_walk_the_station_graph_away_from_each_entrance() {
    let Cave {
        project,
        t,
        n,
        b,
        m,
        e,
        f,
        ..
    } = cave();
    let rows = rows(&project);
    let shape: Vec<(i64, i64, &str, &str)> = rows
        .iter()
        .map(|r| (r.id, r.from_id, r.shot_type, r.line.as_str()))
        .collect();
    let max = id(*project.stations.keys().next_back().unwrap());
    let expected: Vec<(i64, i64, &str, &str)> = vec![
        (id(t[0]), -1, "START", "Trunk"),
        (id(t[1]), id(t[0]), "REAL", "Trunk"),
        (id(t[2]), id(t[1]), "REAL", "Trunk"),
        (id(t[3]), id(t[2]), "REAL", "Trunk"),
        (id(t[4]), id(t[3]), "REAL", "Trunk"),
        (id(n[1]), id(t[2]), "REAL", "North"),
        (id(n[2]), id(n[1]), "REAL", "North"),
        (id(n[3]), id(n[2]), "REAL", "North"),
        (id(b[1]), id(t[4]), "REAL", "Back (reversed)"),
        (id(b[0]), id(b[1]), "REAL", "Back (reversed)"),
        (id(m[2]), id(n[2]), "REAL", "Mid"),
        (id(m[0]), id(n[2]), "REAL", "Mid (reversed)"),
        (id(e[2]), -1, "START", "Island"),
        (id(e[3]), id(e[2]), "REAL", "Island"),
        (id(e[1]), id(e[2]), "REAL", "Island (pre-zero-datum)"),
        (id(e[0]), id(e[1]), "REAL", "Island (pre-zero-datum)"),
        (id(f[0]), -1, "START", "Free"),
        (id(f[1]), id(f[0]), "REAL", "Free"),
        (id(f[2]), id(f[1]), "REAL", "Free"),
        // The loop closure, numbered above every station.
        (max + 1, id(n[3]), "CLOSURE", "North"),
        // The GPS fix on Mid's end: a START of its own and its tie.
        (max + 2, -1, "START", "Mid"),
        (max + 3, id(m[2]), "CLOSURE", "Mid"),
    ];
    assert_eq!(shape, expected);
    assert_eq!(row(&rows, max + 1).closure_to_id, id(b[0]));
    assert_eq!(row(&rows, max + 3).closure_to_id, max + 2);
}

/// One row per station (the START included), no id twice, and every
/// `FromID` names a row: nothing is left parentless but the STARTs.
#[test]
fn every_station_is_one_node_and_every_parent_resolves() {
    let cave = cave();
    let rows = rows(&cave.project);
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let unique: BTreeSet<i64> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len(), "an id is used twice");
    let stations: BTreeSet<i64> = cave.project.stations.keys().map(|&s| id(s)).collect();
    let station_rows: BTreeSet<i64> = unique
        .iter()
        .copied()
        .filter(|i| stations.contains(i))
        .collect();
    assert_eq!(station_rows, stations, "every station has exactly one row");
    for r in &rows {
        if r.shot_type == "START" {
            assert_eq!(r.from_id, -1);
        } else {
            assert!(
                unique.contains(&r.from_id),
                "row {} hangs off nothing",
                r.id
            );
        }
    }
}

/// Each shot uuid is on exactly one row, and the synthesized rows carry none.
#[test]
fn each_shot_uuid_is_on_one_row() {
    let cave = cave();
    let rows = rows(&cave.project);
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for r in &rows {
        match r.shot_type {
            "START" | "CLOSURE" => assert!(r.uuid.is_none()),
            _ => *seen.entry(r.uuid.clone().unwrap()).or_default() += 1,
        }
    }
    let project_uuids: BTreeSet<String> = cave
        .project
        .lines()
        .iter()
        .flat_map(|l| l.shots.iter().map(|s| s.uuid.clone()))
        .collect();
    assert_eq!(seen.keys().cloned().collect::<BTreeSet<_>>(), project_uuids);
    assert!(seen.values().all(|&count| count == 1));
}

/// A reversed row is the shot walked backwards: reciprocal heading, depths
/// swapped, pitch negated, left and right exchanged.
#[test]
fn a_reversed_row_mirrors_its_shot() {
    let cave = cave();
    let rows = rows(&cave.project);
    let back = cave.project.line(cave.back).unwrap();
    let r = row(&rows, id(cave.b[0]));
    let shot = &back.shots[0];
    assert!((r.azimuth - 315.0).abs() < 1e-9, "{}", r.azimuth);
    assert!((r.depth - shot.depth_in).abs() < 1e-12);
    assert!((r.depth_in - shot.depth_out).abs() < 1e-12);
    assert!(
        (r.inclination - 4.0).abs() < 1e-12,
        "a 4 deg descent reversed reads +4 up"
    );
}

/// GPS lands on the STARTs: the frame's own, the second piece's datum, the
/// free piece at where the raw survey puts it, and the mid-survey fix on a
/// START of its own. Nothing is left unwritten.
#[test]
fn every_datum_is_on_a_start() {
    let cave = cave();
    let decision = AzimuthExport::Magnetic;
    let (rows, unwritten) = build_rows(&cave.project, TmlProfile::User, decision);
    assert_eq!(unwritten, 0);
    let frame = row(&rows, id(cave.t[0]));
    assert!((frame.latitude - cave.project.datums[0].gps.latitude).abs() < 1e-12);
    let island = row(&rows, id(cave.e[2]));
    assert!((island.latitude - cave.project.datums[1].gps.latitude).abs() < 1e-12);
    let free = row(&rows, id(cave.f[0]));
    let expected = crate::models::local_to_gps(10.0, 20.0, &cave.project.datums[0].gps).unwrap();
    assert!((free.latitude - expected.0).abs() < 1e-9);
    assert!((free.longitude - expected.1).abs() < 1e-9);
    let max = id(*cave.project.stations.keys().next_back().unwrap());
    let fix = row(&rows, max + 2);
    assert!((fix.latitude - cave.project.datums[2].gps.latitude).abs() < 1e-12);
}

/// A station comment folds into the one row ending at the station, once,
/// junction or not; the comment of a station written as a START is dropped,
/// as before; shot comments are ZWSP-marked.
#[test]
fn station_comments_fold_once_per_station() {
    let cave = cave();
    let rows = rows(&cave.project);
    let hub = row(&rows, id(cave.t[2])).comment.clone().unwrap();
    let (shot, station) = crate::models::comment_split::split(Some(hub.as_str()));
    assert_eq!(station.as_deref(), Some("junction comment"));
    assert_eq!(shot.as_deref(), Some("shot comment"));
    let mid = row(&rows, id(cave.m[0])).comment.clone().unwrap();
    assert!(mid.contains("mid comment"));
    let all: String = rows.iter().filter_map(|r| r.comment.clone()).collect();
    assert_eq!(all.matches("junction comment").count(), 1);
    assert!(!all.contains("entrance comment"));
}

/// Same project, same bytes, for both containers and both profiles.
#[test]
fn exporting_twice_gives_the_same_bytes() {
    let cave = cave();
    let dir = tempfile::TempDir::new().unwrap();
    for name in ["a.tmlu", "a.tml"] {
        let one = dir.path().join(format!("one-{name}"));
        let two = dir.path().join(format!("two-{name}"));
        super::export_tml(&cave.project, &one).unwrap();
        super::export_tml(&cave.project, &two).unwrap();
        assert_eq!(std::fs::read(&one).unwrap(), std::fs::read(&two).unwrap());
    }
    let one = dir.path().join("one-sidecar.tml");
    let two = dir.path().join("two-sidecar.tml");
    super::export_tml_for_speleodb(&cave.project, &one).unwrap();
    super::export_tml_for_speleodb(&cave.project, &two).unwrap();
    assert_eq!(std::fs::read(&one).unwrap(), std::fs::read(&two).unwrap());
}

/// Export, re-import through the Ariane parser: every shot comes back once,
/// with its uuid, length, LRUD and (forward-equivalent) azimuth.
#[test]
fn a_round_trip_through_the_importer_keeps_every_shot_and_uuid() {
    let cave = cave();
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("cave.tmlu");
    super::export_tml(&cave.project, &path).unwrap();
    let parsed = crate::parsers::tml::parse_tml(&path).unwrap();
    let (configs, closures, _warnings) =
        crate::parsers::tml::convert_tml_to_import_configs(&parsed, false, true).unwrap();
    let back: HashMap<String, (f64, f64)> = configs
        .iter()
        .flat_map(|c| c.shots.iter())
        .map(|s| (s.uuid.clone().unwrap(), (s.length, s.up.unwrap_or(0.0))))
        .collect();
    let mut want: HashMap<String, (f64, f64)> = HashMap::new();
    for line in cave.project.lines() {
        for s in &line.shots {
            want.insert(s.uuid.clone(), (s.length, s.up.unwrap_or(0.0)));
        }
    }
    assert_eq!(back.len(), want.len(), "every shot comes back once");
    for (uuid, (length, up)) in &want {
        let (l, u) = back.get(uuid).unwrap_or_else(|| panic!("{uuid} lost"));
        assert!((l - length).abs() < 1e-6 && (u - up).abs() < 1e-6, "{uuid}");
    }
    assert!(!closures.is_empty(), "the loop closure comes back");
}

/// A shot reversed by the model (Reverse Line) exports as the same
/// physical leg: a project and its reversed copy re-import to the same
/// uuids.
#[test]
fn a_reversed_line_exports_every_shot_once() {
    let mut cave = cave();
    cave.project.reverse_line(cave.north, false).unwrap();
    let rows = rows(&cave.project);
    let north_rows: Vec<&super::ExportShot> = rows
        .iter()
        .filter(|r| r.shot_type == "REAL" && r.line.starts_with("North"))
        .collect();
    // North now ends at the hub, so it hangs off it reversed.
    assert!(north_rows.iter().all(|r| r.line == "North (reversed)"));
    assert_eq!(north_rows.len(), 3);
    let uuids: HashSet<String> = rows.iter().filter_map(|r| r.uuid.clone()).collect();
    assert_eq!(uuids.len(), 4 + 3 + 2 + 2 + 3 + 2);
}

/// A plain project (one line from its datum) writes the same document as the
/// 0.6.0 exporter: the Station IDs are the legacy positional ids there. The
/// pinned bytes are the legacy exporter's `.tmlu`, captured before it was
/// deleted.
#[test]
fn a_simple_survey_matches_the_legacy_document_byte_for_byte() {
    let mut p = project("Simple");
    let shots = run(0, 3, 90.0, 10.0, 2.0, 1.0);
    let main = p
        .add_line(meta("Main", "#336699"), StartAt::New, shots)
        .unwrap();
    let start = p.line(main).unwrap().start;
    p.datums.push(ZeroDatum::new(start, gps_at(0.0, 0.0)));
    assert!(p.validate().is_empty());

    let (ours, notes) = build_xml(&p, &SHORT, TmlProfile::User).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(String::from_utf8(ours).unwrap(), LEGACY_SIMPLE_TMLU);
}

/// The 0.6.0 exporter's `.tmlu` of
/// [`a_simple_survey_matches_the_legacy_document_byte_for_byte`]'s survey.
const LEGACY_SIMPLE_TMLU: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<CaveFile>
    <caveName>Simple</caveName>
    <firstStartAbsoluteElevation>0.0</firstStartAbsoluteElevation>
    <Data>
        <SRVD>
            <AZ>0.0</AZ>
            <CID>-1</CID>
            <CL>0x336699ff</CL>
            <CM>START</CM>
            <DT>2000-01-01</DT>
            <DP>2.0</DP>
            <DPI>2.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>unknown</EX>
            <FRID>-1</FRID>
            <ID>0</ID>
            <INC>0.0</INC>
            <LT>20.5</LT>
            <L>0.0</L>
            <LG>0.0</LG>
            <LCK>true</LCK>
            <LGT>-87.25</LGT>
            <NM>START</NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Main</SC>
            <TY>START</TY>
            <U>0.0</U>
        </SRVD>
        <SRVD>
            <AZ>90.0</AZ>
            <CID>-1</CID>
            <CL>0x336699ff</CL>
            <CM></CM>
            <DT>2000-01-01</DT>
            <DP>3.0</DP>
            <DPI>-1.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>unknown</EX>
            <FRID>0</FRID>
            <ID>1</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>10.0</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Main</SC>
            <TY>REAL</TY>
            <U>0.0</U>
            <UU>00000000-5a00-4000-8000-000000000000</UU>
        </SRVD>
        <SRVD>
            <AZ>90.0</AZ>
            <CID>-1</CID>
            <CL>0x336699ff</CL>
            <CM></CM>
            <DT>2000-01-01</DT>
            <DP>4.0</DP>
            <DPI>-1.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>unknown</EX>
            <FRID>1</FRID>
            <ID>2</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>10.0</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Main</SC>
            <TY>REAL</TY>
            <U>0.0</U>
            <UU>01000000-5a00-4000-8000-000000000000</UU>
        </SRVD>
        <SRVD>
            <AZ>90.0</AZ>
            <CID>-1</CID>
            <CL>0x336699ff</CL>
            <CM></CM>
            <DT>2000-01-01</DT>
            <DP>5.0</DP>
            <DPI>-1.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>unknown</EX>
            <FRID>2</FRID>
            <ID>3</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>10.0</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Main</SC>
            <TY>REAL</TY>
            <U>0.0</U>
            <UU>02000000-5a00-4000-8000-000000000000</UU>
        </SRVD>
    </Data>
    <unit>m</unit>
    <useMagneticAzimuth>true</useMagneticAzimuth>
</CaveFile>"#;

/// A migrated rigid tie (`Tie`, e.g. a cycle-closing merge) is a loop like
/// any closure: it is written as a CLOSURE row (the legacy exporter dropped
/// a cycle-closing merge silently).
#[test]
fn a_tie_closure_is_written_as_a_closure_row() {
    let mut cave = cave();
    cave.project.push_closure(
        cave.t[1],
        None,
        cave.n[1],
        None,
        crate::models::survey::ClosureStrategy::Tie,
    );
    assert!(cave.project.validate().is_empty());
    let rows = rows(&cave.project);
    let ties: Vec<&super::ExportShot> = rows
        .iter()
        .filter(|r| r.shot_type == "CLOSURE" && r.from_id == id(cave.t[1]))
        .collect();
    assert_eq!(ties.len(), 1);
    assert_eq!(ties[0].closure_to_id, id(cave.n[1]));
}

/// Two datums on one station (a merge of two placed pieces): the first holds
/// the node, the second cannot be carried and is reported unwritten.
#[test]
fn a_second_datum_on_one_station_is_reported_unwritten() {
    let mut cave = cave();
    let station = cave.t[0];
    cave.project
        .datums
        .push(ZeroDatum::new(station, gps_at(1.0, 1.0)));
    assert!(cave.project.validate().is_empty());
    let (xml, notes) = build_xml(&cave.project, &SHORT, TmlProfile::User).unwrap();
    assert_eq!(notes, vec!["gps-anchors-not-written".to_string()]);
    assert!(!xml.is_empty());
}

