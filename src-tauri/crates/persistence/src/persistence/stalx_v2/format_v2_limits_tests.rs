//! Work and memory stay proportional to the input: the cases an independent
//! review found quadratic or unbounded, pinned.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::models::survey::{LineMeta, Project, StartAt, StationAlias};
use crate::models::{Person, ShotData, SurveyCredit};

use super::test_helpers::{generate, text_of, SMALL};
use super::types::Limit;
use super::{
    decode, decode_salvage, encode, DecodeError, DecodeOptions, EncodeError, Extras, Mode,
    MAX_SHOTS_PER_LINE,
};

fn one_line(shots: usize) -> Project {
    let mut p = Project::new_at(
        "Long",
        None,
        chrono::DateTime::from_timestamp(0, 0).unwrap(),
    );
    let data = (0..shots)
        .map(|i| ShotData {
            uuid: Some(format!("u{i}")),
            length: 1.0,
            ..ShotData::default()
        })
        .collect();
    let line = p.add_line(LineMeta::default(), StartAt::New, data).unwrap();
    let start = p.line(line).unwrap().start;
    p.set_placement(start, 0.0, 0.0).unwrap();
    p
}

/// A header credit naming thousands of people, shared by thousands of shots:
/// each credit is looked at once, not once per shot.
#[test]
fn a_huge_shared_credit_encodes_in_linear_time() {
    let mut p = one_line(10_000);
    let crowd = Arc::new(SurveyCredit {
        surveyors: (0..10_000)
            .map(|i| Person {
                name: format!("Diver {i}"),
                email: None,
            })
            .collect(),
        explorers: Vec::new(),
        date: None,
    });
    let line = p.lines()[0].id;
    for shot in &mut p.line_mut(line).unwrap().shots {
        shot.credit = Some(Arc::clone(&crowd));
    }
    let started = Instant::now();
    let bytes = encode(&p, &Extras::default(), Mode::Normal).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(text_of(&bytes).matches("Diver 9999").count(), 1);
    let started = Instant::now();
    let decoded = decode(&bytes, &DecodeOptions::default()).unwrap();
    let again = encode(&decoded.project, &decoded.extras, Mode::Normal).unwrap();
    assert_eq!(again, bytes);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
}

/// Thousands of repeated shot uuids: one salvage pass re-mints them all.
#[test]
fn salvaging_many_repeated_uuids_is_linear() {
    let p = one_line(8_000);
    let text = text_of(&encode(&p, &Extras::default(), Mode::Normal).unwrap()).to_string();
    let mut broken = String::with_capacity(text.len());
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("[\"u") {
            let tail = &rest[rest.find('"').unwrap()..];
            let _ = writeln!(broken, "[\"x{tail}");
        } else {
            let _ = writeln!(broken, "{line}");
        }
    }
    let started = Instant::now();
    let (project, report) = decode_salvage(broken.as_bytes());
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    let project = project.unwrap();
    assert!(project.validate().is_empty());
    assert_eq!(report.repairs.len(), 7_999);
}

#[test]
fn string_lists_are_counted_as_they_arrive() {
    let base =
        text_of(&encode(&one_line(3), &Extras::default(), Mode::Normal).unwrap()).to_string();
    let many = |n: usize| {
        let items: Vec<String> = (0..n).map(|i| format!("\"h{i}\"")).collect();
        items.join(",")
    };
    let imports = base.replacen(
        "\n\"end\":true",
        &format!("\n\"imports\":{{\"files\":[{}]}},\n\"end\":true", many(50)),
        1,
    );
    let mut options = DecodeOptions::default();
    options.limits.import_hashes = 10;
    assert!(matches!(
        decode(imports.as_bytes(), &options),
        Err(DecodeError::TooComplex {
            limit: Limit::ImportHashes,
            ..
        })
    ));
    let requires = base.replacen(
        "\"requires\":[]",
        &format!("\"requires\":[{}]", many(500)),
        1,
    );
    let mut options = DecodeOptions::default();
    options.limits.preserved = 100;
    assert!(matches!(
        decode(requires.as_bytes(), &options),
        Err(DecodeError::TooComplex {
            limit: Limit::Preserved,
            ..
        })
    ));
}

/// Session shot ids are `line * 100000 + index`: a line holds at most 99,999
/// shots, on both sides.
#[test]
fn a_line_holds_at_most_99999_shots() {
    let p = one_line(MAX_SHOTS_PER_LINE + 1);
    assert!(matches!(
        encode(&p, &Extras::default(), Mode::Normal),
        Err(EncodeError::OutOfRange { .. })
    ));
    // Written by hand: the decoder refuses it too.
    let mut text = String::from(
        "{\"format\":\"stalx\",\"version\":2,\"solver\":1,\"requires\":[],\n\
         \"conventions\":{\"length\":\"m\",\"angles\":\"deg\",\"azimuth\":\"cw-from-north\",\
         \"inclination\":\"+down\",\"depth\":\"+down\",\"local\":\"enu-m\",\"gps\":\"wgs84-deg\",\
         \"temperature\":\"degC\"},\n\
         \"project\":{\"name\":\"x\",\"created\":\"2025-01-01T00:00:00Z\",\
         \"modified\":\"2025-01-01T00:00:00Z\"},\n\"lines\":[\n\
         {\"id\":0,\"color\":\"#000\",\"start\":0}",
    );
    for i in 0..=MAX_SHOTS_PER_LINE {
        let _ = write!(text, ",\n[\"u{i}\",{},1.0,0.0,0.0,0.0,0.0]", i + 1);
    }
    text.push_str("\n],\n\"end\":true}\n");
    assert!(matches!(
        decode(text.as_bytes(), &DecodeOptions::default()),
        Err(DecodeError::TooComplex {
            limit: Limit::ShotsPerLine,
            ..
        })
    ));
}

#[test]
fn alias_ids_and_counters_stay_in_range() {
    let (mut p, extras) = generate(2, SMALL);
    let host = *p.stations.keys().next().unwrap();
    p.station_mut(host).unwrap().aliases.push(StationAlias {
        id: 4_000_000_000,
        name: None,
    });
    p.counters.next_station_id = 4_000_000_001;
    // `validate` does not check the range; the encoder does.
    assert!(matches!(
        encode(&p, &extras, Mode::Normal),
        Err(EncodeError::OutOfRange { .. })
    ));
    let text =
        text_of(&encode(&one_line(3), &Extras::default(), Mode::Normal).unwrap()).to_string();
    let first = text.lines().find(|l| l.starts_with("[0,")).unwrap();
    let row = first.trim_end_matches(',').trim_end_matches(']');
    let with_alias = format!("{row},{{\"aliases\":[[4000000000]]}}],");
    let bad = text.replacen(first, &with_alias, 1);
    assert!(matches!(
        decode(bad.as_bytes(), &DecodeOptions::default()),
        Err(DecodeError::BadValue { .. })
    ));
    let at = text.find("\"counters\":{\"station\":").unwrap() + "\"counters\":{\"station\":".len();
    let digits = text[at..].find(',').unwrap();
    let counter = format!("{}3000000000{}", &text[..at], &text[at + digits..]);
    assert!(matches!(
        decode(counter.as_bytes(), &DecodeOptions::default()),
        Err(DecodeError::BadValue { .. })
    ));
}
