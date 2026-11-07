//! Byte-level pins of the Ariane export (#576 B4).
//!
//! Every other TML test asserts a property - a chain is rooted, a uuid
//! survives, a depth is re-hung. None of them notices an exporter that writes
//! the same survey with a field renamed, reordered, reformatted or dropped,
//! which is exactly what a refactor of the element-name tables or of the shot
//! constructors could do silently. These tests pin the WHOLE document for one
//! synthetic project that walks every constructor the exporter has:
//!
//! * a root section split at its zero datum (START, forward leg and the
//!   reversed "(pre-zero-datum)" leg),
//! * a child merged mid-section (forward block plus "(reversed)" block),
//! * a child merged at its END (the whole section reversed),
//! * a child merged at its station 0 (the plain block), with a virtual shot,
//! * a detached section (its own START entrance),
//! * a loop closure, station comments and multi-line shot comments,
//! * the SpeleoDB sidecar's lone-START padding shot.
//!
//! A change to any of these bytes must be deliberate: update the golden in
//! the same commit, so the diff shows what the file now says. On a mismatch
//! the actual document is written next to the system temp dir and the first
//! differing line is named.
//!
//! The `.tml` (long-name) document is not pinned separately: it must be the
//! `.tmlu` golden with every tag renamed through THIS file's own table of
//! Ariane spellings - an oracle written independently of the exporter's
//! tables, so a typo in either shows up as a mismatch here.

#![expect(clippy::unwrap_used, clippy::panic)]

use std::io::Read as _;

use crate::models::test_fixtures::straight_shot;
use crate::models::{
    ClosureStrategy, GpsCoordinates, LoopClosure, Project, SectionData, ShotKind, StationMerge,
    StationRef, ZeroDatum,
};

use super::tml::{export_tml, export_tml_for_speleodb};

/// Ariane's two spellings of every element the exporter writes, as the format
/// documents them (docs/ariane-export.md, Data Mapping), `(short, long)`.
/// Deliberately a second, independent statement of the schema.
const ARIANE_SPELLINGS: &[(&str, &str)] = &[
    ("SRVD", "SurveyData"),
    ("AZ", "Azimut"),
    ("CID", "ClosureToID"),
    ("CL", "Color"),
    ("CM", "Comment"),
    ("DT", "Date"),
    ("DP", "Depth"),
    ("DPI", "DepthIn"),
    ("D", "Down"),
    ("EXC", "Excluded"),
    ("EX", "Explorer"),
    ("FRID", "FromID"),
    ("INC", "Inclination"),
    ("LT", "Latitude"),
    ("L", "Left"),
    ("LG", "Length"),
    ("LCK", "Locked"),
    ("LGT", "Longitude"),
    ("NM", "Name"),
    ("PT", "Profiletype"),
    ("R", "Right"),
    ("SC", "Section"),
    ("TY", "Type"),
    ("U", "Up"),
    ("UU", "UUID"),
];

fn merge(a: (u32, u32), b: (u32, u32)) -> StationMerge {
    StationMerge {
        station_a: StationRef::new(a.0, a.1).into(),
        station_b: StationRef::new(b.0, b.1).into(),
        merged_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
    }
}

/// The synthetic survey every golden below is rendered from. Every uuid is
/// spelled out (a minted one would differ per run) and v4-shaped, so the
/// sidecar keeps them too.
fn golden_project() -> Project {
    let mut project = Project::new("CaveA".to_string(), None);
    project.surface_elevation = Some(12.5);

    // Section 0: the root, split at its zero datum on station 2.
    let mut first = straight_shot(10.0, 5.0)
        .with_depths(1.0, 2.5)
        .with_uuid("00000000-0000-4000-8000-000000000001");
    // Two different forward readings: the file carries a single azimuth.
    first.azimuth_out = 20.0;
    first.pitch_in = Some(-4.0);
    first.left = Some(1.5);
    first.down = Some(0.25);
    first.comment = Some("first line\nsecond line".to_string());
    let mut second = straight_shot(95.5, 4.0)
        .with_depths(2.5, 3.0)
        .with_uuid("00000000-0000-4000-8000-000000000002");
    second.right = Some(2.0);
    second.up = Some(0.0);
    let third = straight_shot(350.0, 6.25)
        .with_depths(3.0, 2.0)
        .with_uuid("00000000-0000-4000-8000-000000000003");
    project.add_section(SectionData {
        name: Some("Main".to_string()),
        surveyor: Some("Diver A / Diver B".to_string()),
        date: chrono::NaiveDate::from_ymd_opt(2024, 3, 5),
        color: "#336699".to_string(),
        shots: vec![first, second, third],
        ..Default::default()
    });

    // Section 1: merged MID-section (its station 1 onto root station 3), and
    // undated with no surveyor.
    project.add_section(SectionData {
        name: Some("Side".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        shots: vec![
            straight_shot(180.0, 3.0)
                .with_depths(2.0, 2.5)
                .with_uuid("00000000-0000-4000-8000-000000000011"),
            straight_shot(200.0, 2.0)
                .with_depths(2.0, 1.5)
                .with_uuid("00000000-0000-4000-8000-000000000012"),
        ],
        ..Default::default()
    });
    project.station_merges.push(merge((0, 3), (1, 1)));

    // Section 2: merged at its END (station 2 onto root station 1).
    project.add_section(SectionData {
        name: Some("Tail".to_string()),
        surveyor: Some("Diver C".to_string()),
        date: chrono::NaiveDate::from_ymd_opt(2023, 11, 30),
        color: "not-a-colour".to_string(),
        shots: vec![
            // Not a v4: the sidecar drops it, the user file keeps it verbatim.
            straight_shot(45.0, 2.5)
                .with_depths(0.5, 1.0)
                .with_uuid("legacy-id-21"),
            straight_shot(60.0, 1.5)
                .with_depths(1.0, 2.5)
                .with_uuid("00000000-0000-4000-8000-000000000022"),
        ],
        ..Default::default()
    });
    project.station_merges.push(merge((0, 1), (2, 2)));

    // Section 3: merged at its station 0 onto side station 2; a jump.
    let mut jump = straight_shot(270.0, 7.0)
        .with_depths(1.5, 1.5)
        .with_uuid("00000000-0000-4000-8000-000000000031");
    jump.kind = ShotKind::Jump;
    project.add_section(SectionData {
        name: Some("Branch".to_string()),
        color: "#aa00aa".to_string(),
        shots: vec![jump],
        ..Default::default()
    });
    project.station_merges.push(merge((1, 2), (3, 0)));

    // Section 4: detached - its own entrance. Its date predates the IGRF span
    // and its surveyor carries SpeleoDB's section-field separator, so the
    // sidecar rewrites both.
    project.add_section(SectionData {
        name: Some("Island".to_string()),
        surveyor: Some("Diver D \u{220e} Team".to_string()),
        date: chrono::NaiveDate::from_ymd_opt(1890, 6, 1),
        color: "#123456".to_string(),
        shots: vec![straight_shot(0.0, 9.0)
            .with_depths(4.0, 4.0)
            .with_uuid("00000000-0000-4000-8000-000000000041")],
        ..Default::default()
    });

    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(0, 2).into(),
        gps: GpsCoordinates::new(20.5, -87.25, 3.0).unwrap(),
    });

    project.loop_closures.push(LoopClosure {
        id: 9,
        station_a: StationRef::new(1, 2).into(),
        station_b: StationRef::new(0, 0).into(),
        path_length: 12.0,
        misclosure_distance: 0.3,
        error_percentage: 2.5,
        horizontal_misclosure: 0.3,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });

    for (section_id, station_index, text) in [
        (0, 3, "station note"),
        (1, 1, "junction note"),
        (2, 1, "tail note"),
    ] {
        let key = project
            .key_for_ref(StationRef::new(section_id, station_index))
            .unwrap();
        project.station_comments.insert(key, text.to_string());
    }

    project
}

/// A shotless section: the user file is a lone START, the sidecar pads it.
fn lone_start_project() -> Project {
    let mut project = Project::new("CaveB".to_string(), None);
    project.add_section(SectionData {
        name: Some("Empty".to_string()),
        date: chrono::NaiveDate::from_ymd_opt(2024, 7, 8),
        color: "#ff0000".to_string(),
        shots: vec![],
        ..Default::default()
    });
    project
}

fn export_user_tmlu(project: &Project) -> String {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("golden.tmlu");
    export_tml(project, &path).unwrap();
    std::fs::read_to_string(&path).unwrap()
}

fn export_user_tml_data_xml(project: &Project) -> String {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("golden.tml");
    export_tml(project, &path).unwrap();
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
    let mut xml = String::new();
    archive
        .by_name("Data.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    xml
}

fn export_sidecar_tmlu(project: &Project) -> String {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("golden.tmlu");
    export_tml_for_speleodb(project, &path).unwrap();
    std::fs::read_to_string(&path).unwrap()
}

/// `xml` with every short tag spelled long, via [`ARIANE_SPELLINGS`].
fn respell_long(xml: &str) -> String {
    let mut out = xml.to_string();
    for (short, long) in ARIANE_SPELLINGS {
        out = out
            .replace(&format!("<{short}>"), &format!("<{long}>"))
            .replace(&format!("</{short}>"), &format!("</{long}>"))
            .replace(&format!("<{short}/>"), &format!("<{long}/>"));
    }
    out
}

/// How the goldens spell the invisible U+200B shot-comment marker, so a
/// reviewer can see where it sits.
const ZWSP_TOKEN: &str = "[ZWSP]";

/// How the goldens spell U+220E, SpeleoDB's section-field separator.
const SEPARATOR_TOKEN: &str = "[U+220E]";

fn assert_golden(name: &str, actual: &str, expected: &str) {
    let expected = expected
        .replace(ZWSP_TOKEN, "\u{200b}")
        .replace(SEPARATOR_TOKEN, "\u{220e}");
    if actual == expected {
        return;
    }
    let dump = std::env::temp_dir().join(format!("stalagmight-golden-{name}.xml"));
    std::fs::write(&dump, actual).unwrap();
    let first_diff = actual
        .lines()
        .zip(expected.lines())
        .enumerate()
        .find(|(_, (a, e))| a != e)
        .map_or_else(
            || {
                format!(
                    "documents agree on their common lines; actual has {} lines, golden {}",
                    actual.lines().count(),
                    expected.lines().count()
                )
            },
            |(i, (a, e))| format!("line {}: actual {a:?}, golden {e:?}", i + 1),
        );
    panic!(
        "{name}: the exported document changed ({first_diff}). The full actual \
         document is at {}; if the change is intended, update the golden.",
        dump.display()
    );
}

#[test]
fn the_user_tmlu_export_is_pinned_byte_for_byte() {
    assert_golden(
        "user-tmlu",
        &export_user_tmlu(&golden_project()),
        GOLDEN_USER_TMLU,
    );
}

/// The `.tml` ZIP's `Data.xml` is the `.tmlu` document with long names.
#[test]
fn the_user_tml_export_is_the_tmlu_golden_spelled_long() {
    assert_golden(
        "user-tml",
        &export_user_tml_data_xml(&golden_project()),
        &respell_long(GOLDEN_USER_TMLU),
    );
}

/// What the exporter writes, our own reader reads back as the same fields -
/// in BOTH spellings, and for the fields a typo would have silently zeroed
/// (LRUD, inclination, colour, explorer, date). The two spellings must parse
/// to the identical survey.
#[test]
fn every_written_field_reads_back_identically_in_both_spellings() {
    use crate::parsers::tml::{parse_tml, TmlFile, TmlShot};

    let project = golden_project();
    let dir = tempfile::TempDir::new().unwrap();
    let long = dir.path().join("fields.tml");
    let short = dir.path().join("fields.tmlu");
    export_tml(&project, &long).unwrap();
    export_tml(&project, &short).unwrap();
    let from_long = parse_tml(&long).unwrap();
    let from_short = parse_tml(&short).unwrap();
    assert_eq!(
        serde_json::to_value(&from_long).unwrap(),
        serde_json::to_value(&from_short).unwrap(),
        "the .tml and .tmlu of one project must parse to the same survey"
    );

    let find = |tml: &TmlFile, uuid: &str| -> (String, TmlShot) {
        tml.sections
            .iter()
            .find_map(|s| {
                s.shots
                    .iter()
                    .find(|shot| shot.uuid.as_deref() == Some(uuid))
                    .map(|shot| (s.name.clone(), shot.clone()))
            })
            .unwrap()
    };
    // The first root shot went out reversed (it sits before the zero datum),
    // so its walls and pitch are mirrored.
    let (section, first) = find(&from_short, "00000000-0000-4000-8000-000000000001");
    assert_eq!(section, "Main (pre-zero-datum)");
    assert!((first.right - 1.5).abs() < f64::EPSILON, "{first:?}");
    assert!(first.left.abs() < f64::EPSILON, "{first:?}");
    assert!((first.down - 0.25).abs() < f64::EPSILON, "{first:?}");
    assert!((first.inclination - -4.0).abs() < f64::EPSILON, "{first:?}");
    assert!((first.length - 5.0).abs() < f64::EPSILON, "{first:?}");
    assert_eq!(first.color, "#336699");
    assert_eq!(first.explorer.as_deref(), Some("Diver A / Diver B"));
    assert_eq!(first.date, chrono::NaiveDate::from_ymd_opt(2024, 3, 5));
    assert!(first.comment.as_deref().unwrap().contains("second line"));

    let (_, second) = find(&from_short, "00000000-0000-4000-8000-000000000002");
    assert!((second.left - 2.0).abs() < f64::EPSILON, "{second:?}");
    let (_, jump) = find(&from_short, "00000000-0000-4000-8000-000000000031");
    assert_eq!(
        jump.shot_type,
        crate::parsers::tml::TmlShotType::Virtual,
        "{jump:?}"
    );
    assert!((jump.length - 7.0).abs() < f64::EPSILON);
    assert!((from_short.first_start_absolute_elevation - 12.5).abs() < f64::EPSILON);
    assert_eq!(from_short.cave_name, "CaveA");
}

#[test]
fn the_speleodb_sidecar_export_is_pinned_byte_for_byte() {
    assert_golden(
        "sidecar-tmlu",
        &export_sidecar_tmlu(&golden_project()),
        GOLDEN_SIDECAR_TMLU,
    );
}

#[test]
fn a_lone_start_user_export_is_pinned_byte_for_byte() {
    assert_golden(
        "lone-start-user",
        &export_user_tmlu(&lone_start_project()),
        GOLDEN_LONE_START_USER,
    );
}

#[test]
fn the_sidecar_padding_shot_is_pinned_byte_for_byte() {
    assert_golden(
        "lone-start-sidecar",
        &export_sidecar_tmlu(&lone_start_project()),
        GOLDEN_LONE_START_SIDECAR,
    );
}

const GOLDEN_USER_TMLU: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<CaveFile>
    <caveName>CaveA</caveName>
    <firstStartAbsoluteElevation>12.5</firstStartAbsoluteElevation>
    <Data>
        <SRVD>
            <AZ>0.0</AZ>
            <CID>-1</CID>
            <CL>0x336699ff</CL>
            <CM>START</CM>
            <DT>2024-03-05</DT>
            <DP>3.0</DP>
            <DPI>3.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>Diver A / Diver B</EX>
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
            <AZ>350.0</AZ>
            <CID>-1</CID>
            <CL>0x336699ff</CL>
            <CM>station note
junction note</CM>
            <DT>2024-03-05</DT>
            <DP>2.0</DP>
            <DPI>-1.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>Diver A / Diver B</EX>
            <FRID>0</FRID>
            <ID>1</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>6.25</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Main</SC>
            <TY>REAL</TY>
            <U>0.0</U>
            <UU>00000000-0000-4000-8000-000000000003</UU>
        </SRVD>
        <SRVD>
            <AZ>275.5</AZ>
            <CID>-1</CID>
            <CL>0x336699ff</CL>
            <CM></CM>
            <DT>2024-03-05</DT>
            <DP>2.5</DP>
            <DPI>-1.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>Diver A / Diver B</EX>
            <FRID>0</FRID>
            <ID>2</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>2.0</L>
            <LG>4.0</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Main (pre-zero-datum)</SC>
            <TY>REAL</TY>
            <U>0.0</U>
            <UU>00000000-0000-4000-8000-000000000002</UU>
        </SRVD>
        <SRVD>
            <AZ>195.0</AZ>
            <CID>-1</CID>
            <CL>0x336699ff</CL>
            <CM>[ZWSP]first line
[ZWSP]second line</CM>
            <DT>2024-03-05</DT>
            <DP>1.0</DP>
            <DPI>-1.0</DPI>
            <D>0.25</D>
            <EXC>false</EXC>
            <EX>Diver A / Diver B</EX>
            <FRID>2</FRID>
            <ID>3</ID>
            <INC>-4.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>5.0</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>1.5</R>
            <SC>Main (pre-zero-datum)</SC>
            <TY>REAL</TY>
            <U>0.0</U>
            <UU>00000000-0000-4000-8000-000000000001</UU>
        </SRVD>
        <SRVD>
            <AZ>200.0</AZ>
            <CID>-1</CID>
            <CL>0x00ff00ff</CL>
            <CM></CM>
            <DT>2000-01-01</DT>
            <!--stalagmight:no-survey-date-->
            <DP>1.5</DP>
            <DPI>-1.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>unknown</EX>
            <FRID>1</FRID>
            <ID>4</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>2.0</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Side</SC>
            <TY>REAL</TY>
            <U>0.0</U>
            <UU>00000000-0000-4000-8000-000000000012</UU>
        </SRVD>
        <SRVD>
            <AZ>0.0</AZ>
            <CID>-1</CID>
            <CL>0x00ff00ff</CL>
            <CM></CM>
            <DT>2000-01-01</DT>
            <!--stalagmight:no-survey-date-->
            <DP>2.0</DP>
            <DPI>2.5</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>unknown</EX>
            <FRID>1</FRID>
            <ID>5</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>3.0</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Side (reversed)</SC>
            <TY>REAL</TY>
            <U>0.0</U>
            <UU>00000000-0000-4000-8000-000000000011</UU>
        </SRVD>
        <SRVD>
            <AZ>240.0</AZ>
            <CID>-1</CID>
            <CL>0x4ecdc4ff</CL>
            <CM>tail note</CM>
            <DT>2023-11-30</DT>
            <DP>1.0</DP>
            <DPI>-1.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>Diver C</EX>
            <FRID>2</FRID>
            <ID>6</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>1.5</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Tail (reversed)</SC>
            <TY>REAL</TY>
            <U>0.0</U>
            <UU>00000000-0000-4000-8000-000000000022</UU>
        </SRVD>
        <SRVD>
            <AZ>225.0</AZ>
            <CID>-1</CID>
            <CL>0x4ecdc4ff</CL>
            <CM></CM>
            <DT>2023-11-30</DT>
            <DP>0.5</DP>
            <DPI>-1.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>Diver C</EX>
            <FRID>6</FRID>
            <ID>7</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>2.5</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Tail (reversed)</SC>
            <TY>REAL</TY>
            <U>0.0</U>
            <UU>legacy-id-21</UU>
        </SRVD>
        <SRVD>
            <AZ>270.0</AZ>
            <CID>-1</CID>
            <CL>0xaa00aaff</CL>
            <CM></CM>
            <DT>2000-01-01</DT>
            <!--stalagmight:no-survey-date-->
            <DP>1.5</DP>
            <DPI>-1.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>unknown</EX>
            <FRID>4</FRID>
            <ID>8</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>7.0</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Branch</SC>
            <TY>VIRTUAL</TY>
            <U>0.0</U>
            <UU>00000000-0000-4000-8000-000000000031</UU>
        </SRVD>
        <SRVD>
            <AZ>0.0</AZ>
            <CID>-1</CID>
            <CL>0x123456ff</CL>
            <CM>START</CM>
            <DT>1890-06-01</DT>
            <DP>4.0</DP>
            <DPI>4.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>Diver D [U+220E] Team</EX>
            <FRID>-1</FRID>
            <ID>9</ID>
            <INC>0.0</INC>
            <LT>20.5</LT>
            <L>0.0</L>
            <LG>0.0</LG>
            <LCK>true</LCK>
            <LGT>-87.25</LGT>
            <NM>START</NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Island</SC>
            <TY>START</TY>
            <U>0.0</U>
        </SRVD>
        <SRVD>
            <AZ>0.0</AZ>
            <CID>-1</CID>
            <CL>0x123456ff</CL>
            <CM></CM>
            <DT>1890-06-01</DT>
            <DP>4.0</DP>
            <DPI>-1.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>Diver D [U+220E] Team</EX>
            <FRID>9</FRID>
            <ID>10</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>9.0</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Island</SC>
            <TY>REAL</TY>
            <U>0.0</U>
            <UU>00000000-0000-4000-8000-000000000041</UU>
        </SRVD>
        <SRVD>
            <AZ>0.0</AZ>
            <CID>3</CID>
            <CL>0x00ff00ff</CL>
            <CM></CM>
            <DT>2000-01-01</DT>
            <!--stalagmight:no-survey-date-->
            <DP>1.0</DP>
            <DPI>1.5</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>unknown</EX>
            <FRID>4</FRID>
            <ID>11</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>0.5</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM>CLOSURE:CLOSURE</NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Side</SC>
            <TY>CLOSURE</TY>
            <U>0.0</U>
        </SRVD>
    </Data>
    <unit>m</unit>
    <useMagneticAzimuth>true</useMagneticAzimuth>
</CaveFile>"#;

const GOLDEN_SIDECAR_TMLU: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<CaveFile>
    <caveName>CaveA</caveName>
    <firstStartAbsoluteElevation>12.5</firstStartAbsoluteElevation>
    <Data>
        <SRVD>
            <AZ>0.0</AZ>
            <CID>-1</CID>
            <CL>0x336699ff</CL>
            <CM>START</CM>
            <DT>2024-03-05</DT>
            <DP>3.0</DP>
            <DPI>3.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>Diver A / Diver B</EX>
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
            <AZ>350.0</AZ>
            <CID>-1</CID>
            <CL>0x336699ff</CL>
            <CM>station note
junction note</CM>
            <DT>2024-03-05</DT>
            <DP>2.0</DP>
            <DPI>-1.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>Diver A / Diver B</EX>
            <FRID>0</FRID>
            <ID>1</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>6.25</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Main</SC>
            <TY>REAL</TY>
            <U>0.0</U>
            <UU>00000000-0000-4000-8000-000000000003</UU>
        </SRVD>
        <SRVD>
            <AZ>275.5</AZ>
            <CID>-1</CID>
            <CL>0x336699ff</CL>
            <CM></CM>
            <DT>2024-03-05</DT>
            <DP>2.5</DP>
            <DPI>-1.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>Diver A / Diver B</EX>
            <FRID>0</FRID>
            <ID>2</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>2.0</L>
            <LG>4.0</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Main (pre-zero-datum)</SC>
            <TY>REAL</TY>
            <U>0.0</U>
            <UU>00000000-0000-4000-8000-000000000002</UU>
        </SRVD>
        <SRVD>
            <AZ>195.0</AZ>
            <CID>-1</CID>
            <CL>0x336699ff</CL>
            <CM>[ZWSP]first line
[ZWSP]second line</CM>
            <DT>2024-03-05</DT>
            <DP>1.0</DP>
            <DPI>-1.0</DPI>
            <D>0.25</D>
            <EXC>false</EXC>
            <EX>Diver A / Diver B</EX>
            <FRID>2</FRID>
            <ID>3</ID>
            <INC>-4.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>5.0</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>1.5</R>
            <SC>Main (pre-zero-datum)</SC>
            <TY>REAL</TY>
            <U>0.0</U>
            <UU>00000000-0000-4000-8000-000000000001</UU>
        </SRVD>
        <SRVD>
            <AZ>200.0</AZ>
            <CID>-1</CID>
            <CL>0x00ff00ff</CL>
            <CM></CM>
            <DT>2000-01-01</DT>
            <DP>1.5</DP>
            <DPI>-1.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>unknown</EX>
            <FRID>1</FRID>
            <ID>4</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>2.0</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Side</SC>
            <TY>REAL</TY>
            <U>0.0</U>
            <UU>00000000-0000-4000-8000-000000000012</UU>
        </SRVD>
        <SRVD>
            <AZ>0.0</AZ>
            <CID>-1</CID>
            <CL>0x00ff00ff</CL>
            <CM></CM>
            <DT>2000-01-01</DT>
            <DP>2.0</DP>
            <DPI>2.5</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>unknown</EX>
            <FRID>1</FRID>
            <ID>5</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>3.0</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Side (reversed)</SC>
            <TY>REAL</TY>
            <U>0.0</U>
            <UU>00000000-0000-4000-8000-000000000011</UU>
        </SRVD>
        <SRVD>
            <AZ>240.0</AZ>
            <CID>-1</CID>
            <CL>0x4ecdc4ff</CL>
            <CM>tail note</CM>
            <DT>2023-11-30</DT>
            <DP>1.0</DP>
            <DPI>-1.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>Diver C</EX>
            <FRID>2</FRID>
            <ID>6</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>1.5</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Tail (reversed)</SC>
            <TY>REAL</TY>
            <U>0.0</U>
            <UU>00000000-0000-4000-8000-000000000022</UU>
        </SRVD>
        <SRVD>
            <AZ>225.0</AZ>
            <CID>-1</CID>
            <CL>0x4ecdc4ff</CL>
            <CM></CM>
            <DT>2023-11-30</DT>
            <DP>0.5</DP>
            <DPI>-1.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>Diver C</EX>
            <FRID>6</FRID>
            <ID>7</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>2.5</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Tail (reversed)</SC>
            <TY>REAL</TY>
            <U>0.0</U>
        </SRVD>
        <SRVD>
            <AZ>270.0</AZ>
            <CID>-1</CID>
            <CL>0xaa00aaff</CL>
            <CM></CM>
            <DT>2000-01-01</DT>
            <DP>1.5</DP>
            <DPI>-1.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>unknown</EX>
            <FRID>4</FRID>
            <ID>8</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>7.0</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Branch</SC>
            <TY>VIRTUAL</TY>
            <U>0.0</U>
            <UU>00000000-0000-4000-8000-000000000031</UU>
        </SRVD>
        <SRVD>
            <AZ>0.0</AZ>
            <CID>-1</CID>
            <CL>0x123456ff</CL>
            <CM>START</CM>
            <DT>1900-01-01</DT>
            <DP>4.0</DP>
            <DPI>4.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>Diver D   Team</EX>
            <FRID>-1</FRID>
            <ID>9</ID>
            <INC>0.0</INC>
            <LT>20.5</LT>
            <L>0.0</L>
            <LG>0.0</LG>
            <LCK>true</LCK>
            <LGT>-87.25</LGT>
            <NM>START</NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Island</SC>
            <TY>START</TY>
            <U>0.0</U>
        </SRVD>
        <SRVD>
            <AZ>0.0</AZ>
            <CID>-1</CID>
            <CL>0x123456ff</CL>
            <CM></CM>
            <DT>1900-01-01</DT>
            <DP>4.0</DP>
            <DPI>-1.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>Diver D   Team</EX>
            <FRID>9</FRID>
            <ID>10</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>9.0</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Island</SC>
            <TY>REAL</TY>
            <U>0.0</U>
            <UU>00000000-0000-4000-8000-000000000041</UU>
        </SRVD>
        <SRVD>
            <AZ>0.0</AZ>
            <CID>3</CID>
            <CL>0x00ff00ff</CL>
            <CM></CM>
            <DT>2000-01-01</DT>
            <DP>1.0</DP>
            <DPI>1.5</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>unknown</EX>
            <FRID>4</FRID>
            <ID>11</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>0.5</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM>CLOSURE:CLOSURE</NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Side</SC>
            <TY>CLOSURE</TY>
            <U>0.0</U>
        </SRVD>
    </Data>
    <unit>m</unit>
    <useMagneticAzimuth>true</useMagneticAzimuth>
</CaveFile>"#;

const GOLDEN_LONE_START_USER: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<CaveFile>
    <caveName>CaveB</caveName>
    <firstStartAbsoluteElevation>0.0</firstStartAbsoluteElevation>
    <Data>
        <SRVD>
            <AZ>0.0</AZ>
            <CID>-1</CID>
            <CL>0xff0000ff</CL>
            <CM>START</CM>
            <DT>2024-07-08</DT>
            <DP>0.0</DP>
            <DPI>0.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>unknown</EX>
            <FRID>-1</FRID>
            <ID>0</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>0.0</LG>
            <LCK>true</LCK>
            <LGT>0.0</LGT>
            <NM>START</NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Empty</SC>
            <TY>START</TY>
            <U>0.0</U>
        </SRVD>
    </Data>
    <unit>m</unit>
    <useMagneticAzimuth>true</useMagneticAzimuth>
</CaveFile>"#;

const GOLDEN_LONE_START_SIDECAR: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<CaveFile>
    <caveName>CaveB</caveName>
    <firstStartAbsoluteElevation>0.0</firstStartAbsoluteElevation>
    <Data>
        <SRVD>
            <AZ>0.0</AZ>
            <CID>-1</CID>
            <CL>0xff0000ff</CL>
            <CM>START</CM>
            <DT>2024-07-08</DT>
            <DP>0.0</DP>
            <DPI>0.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>unknown</EX>
            <FRID>-1</FRID>
            <ID>0</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>0.0</LG>
            <LCK>true</LCK>
            <LGT>0.0</LGT>
            <NM>START</NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Empty</SC>
            <TY>START</TY>
            <U>0.0</U>
        </SRVD>
        <SRVD>
            <AZ>0.0</AZ>
            <CID>-1</CID>
            <CL>0xff0000ff</CL>
            <CM></CM>
            <DT>2000-01-01</DT>
            <DP>0.0</DP>
            <DPI>0.0</DPI>
            <D>0.0</D>
            <EXC>false</EXC>
            <EX>unknown</EX>
            <FRID>-1</FRID>
            <ID>1</ID>
            <INC>0.0</INC>
            <LT>0.0</LT>
            <L>0.0</L>
            <LG>0.0</LG>
            <LCK>false</LCK>
            <LGT>0.0</LGT>
            <NM></NM>
            <PT>VERTICAL</PT>
            <R>0.0</R>
            <SC>Empty</SC>
            <TY>VIRTUAL</TY>
            <U>0.0</U>
        </SRVD>
    </Data>
    <unit>m</unit>
    <useMagneticAzimuth>true</useMagneticAzimuth>
</CaveFile>"#;
