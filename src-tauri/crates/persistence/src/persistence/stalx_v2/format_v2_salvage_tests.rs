//! Salvage: a file that parses but breaks the invariants is refused by the
//! normal decode and repaired, with a report, by `decode_salvage`.

use super::test_helpers::{append_to_lines, edit_line_header, generate, text_of, Shape, SMALL};
use super::{decode, decode_salvage, encode, DecodeError, DecodeOptions, Mode, Repair};

fn base(seed: u64) -> String {
    let (p, extras) = generate(
        seed,
        Shape {
            lines: 5,
            max_shots: 4,
            rich: false,
        },
    );
    text_of(&encode(&p, &extras, Mode::Normal).unwrap()).to_string()
}

/// Salvage `text`, which the normal decode must refuse, and check the result
/// is valid and encodes.
fn salvaged(text: &str) -> (crate::models::survey::Project, Vec<Repair>) {
    assert!(decode(text.as_bytes(), &DecodeOptions::default()).is_err());
    let (project, report) = decode_salvage(text.as_bytes());
    let Some(project) = project else {
        panic!("salvage gave up: {report:?}");
    };
    assert!(project.validate().is_empty());
    let extras = report.extras.clone().unwrap();
    encode(&project, &extras, Mode::Normal).unwrap();
    (project, report.repairs)
}

fn first_row(text: &str) -> String {
    text.lines()
        .find(|l| l.starts_with("[\""))
        .unwrap()
        .to_string()
}

#[test]
fn a_dangling_chain_station_is_created() {
    let text = base(1);
    let row = first_row(&text);
    let fields: Vec<&str> = row.splitn(3, ',').collect();
    let edited = format!("{},77777,{}", fields[0], fields[2]);
    let (project, repairs) = salvaged(&text.replacen(&row, &edited, 1));
    assert!(project.stations.contains_key(&77_777));
    assert!(repairs.contains(&Repair::CreatedStation { id: 77_777 }));
}

#[test]
fn dangling_anchors_and_closures_are_dropped() {
    for seed in 0..40 {
        let text = base(seed);
        let Some(line) = text.lines().find(|l| l.starts_with("{\"station\":")) else {
            continue;
        };
        let station = &line[11..line.find(',').unwrap()];
        let edited = line.replacen(
            &format!("{{\"station\":{station},"),
            "{\"station\":55555,",
            1,
        );
        let (_, repairs) = salvaged(&text.replacen(line, &edited, 1));
        assert!(repairs.iter().any(|r| matches!(
            r,
            Repair::DroppedDatum { station: 55_555 } | Repair::DroppedPlacement { station: 55_555 }
        )));
        return;
    }
    panic!("no file with a datum or placement");
}

#[test]
fn an_unanchored_piece_gets_a_placement() {
    for seed in 0..40 {
        let text = base(seed);
        let Some(start) = text.find("\"placements\":[\n") else {
            continue;
        };
        let end = start + text[start..].find("\n],\n").unwrap() + 4;
        let edited = format!("{}{}", &text[..start], &text[end..]);
        let (project, repairs) = salvaged(&edited);
        assert!(repairs
            .iter()
            .any(|r| matches!(r, Repair::AddedPlacement { .. })));
        assert!(!project.placements.is_empty());
        return;
    }
    panic!("no file with placements");
}

#[test]
fn a_cycle_is_broken_into_a_batch_closure() {
    let (p, extras) = generate(
        3,
        Shape {
            lines: 1,
            max_shots: 4,
            rich: false,
        },
    );
    let text = text_of(&encode(&p, &extras, Mode::Normal).unwrap()).to_string();
    let chain = p.line_stations(p.lines()[0].id).unwrap().to_vec();
    assert!(chain.len() >= 3, "seed must give a line of 2+ shots");
    let header = format!(
        "{{\"id\":99,\"color\":\"#000\",\"direction\":\"in\",\"quality\":1.0,\"start\":{}}},\n[\"loop\",{},3.0,0.0,0.0,0.0,0.0]",
        chain[chain.len() - 1],
        chain[0]
    );
    let edited = append_to_lines(&text, &header);
    assert!(matches!(
        decode(edited.as_bytes(), &DecodeOptions::default()).unwrap_err(),
        DecodeError::Invalid { .. }
    ));
    let (project, repairs) = salvaged(&edited);
    let broke = repairs
        .iter()
        .find_map(|r| match r {
            Repair::BrokeCycle {
                line: 99, closure, ..
            } => Some(*closure),
            _ => None,
        })
        .unwrap();
    let closure = project.closures.iter().find(|c| c.id == broke).unwrap();
    assert_eq!(closure.b, chain[0]);
    assert_eq!(
        closure.strategy,
        crate::models::survey::ClosureStrategy::Batch
    );
}

#[test]
fn duplicates_names_and_counters_are_repaired() {
    let text = base(5);
    // A second station 0 and a station counter below the ids in use.
    let at = text.find("\"counters\":{\"station\":").unwrap() + "\"counters\":{\"station\":".len();
    let digits = text[at..].find(',').unwrap();
    let edited = format!("{}1{}", &text[..at], &text[at + digits..]).replacen(
        "\"stations\":[\n",
        "\"stations\":[\n[0,\"dup\"],\n",
        1,
    );
    let (project, repairs) = salvaged(&edited);
    assert!(repairs.contains(&Repair::DroppedDuplicateStation { id: 0 }));
    assert!(repairs
        .iter()
        .any(|r| matches!(r, Repair::RaisedCounter { .. })));
    assert!(project
        .stations
        .keys()
        .all(|&id| id < project.counters.next_station_id));

    let names = text.replacen("[1,\"S1\"]", "[1,\"S0\"]", 1);
    if names != text && decode(names.as_bytes(), &DecodeOptions::default()).is_err() {
        let (_, repairs) = salvaged(&names);
        assert!(repairs
            .iter()
            .any(|r| matches!(r, Repair::RenamedStation { .. })));
    }
    let lines = edit_line_header(&text, 1, |h| h.replacen("{\"id\":1,", "{\"id\":0,", 1));
    if lines != text {
        let (_, repairs) = salvaged(&lines);
        assert!(repairs
            .iter()
            .any(|r| matches!(r, Repair::RenumberedLine { from: 0, .. })));
    }
}

#[test]
fn unparseable_bytes_salvage_nothing() {
    let text = base(6);
    let (project, report) = decode_salvage(&text.as_bytes()[..text.len() / 2]);
    assert!(project.is_none());
    assert!(matches!(report.error, Some(DecodeError::Truncated { .. })));
}

#[test]
fn a_valid_file_salvages_unchanged() {
    for seed in 0..10 {
        let (p, extras) = generate(seed, SMALL);
        let bytes = encode(&p, &extras, Mode::Normal).unwrap();
        let (project, report) = decode_salvage(&bytes);
        assert!(report.repairs.is_empty());
        let mut expected = p.clone();
        expected.assign_session_shot_ids();
        assert!(project.unwrap() == expected);
    }
}
