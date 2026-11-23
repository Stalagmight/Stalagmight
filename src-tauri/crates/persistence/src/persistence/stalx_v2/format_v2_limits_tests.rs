//! Work and memory stay proportional to the input: the cases an independent
//! review found quadratic or unbounded, pinned.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::models::survey::{CounterKind, LineMeta, Project, StartAt, ArchivedStation, Violation};
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

/// Whether `violations` holds a [`Violation::TooManyShots`].
fn too_many_shots(violations: &[Violation]) -> bool {
    violations
        .iter()
        .any(|v| matches!(v, Violation::TooManyShots { .. }))
}

/// Session shot ids are `line * 100000 + index`: a line holds at most 99,999
/// shots, on both sides. The bound is the model's (`validate`), which the
/// encoder and the decoder both run.
#[test]
fn a_line_holds_at_most_99999_shots() {
    let p = one_line(MAX_SHOTS_PER_LINE + 1);
    assert!(matches!(
        encode(&p, &Extras::default(), Mode::Normal),
        Err(EncodeError::Invalid { violations }) if too_many_shots(&violations)
    ));
    // Written by hand: the decoder refuses it too, and salvage cannot repair it.
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
        Err(DecodeError::Invalid { violations }) if too_many_shots(&violations)
    ));
    let (salvaged, report) = decode_salvage(text.as_bytes());
    assert!(salvaged.is_none());
    assert!(too_many_shots(&report.remaining), "{:?}", report.remaining);
}

#[test]
fn archived_ids_and_counters_stay_in_range() {
    let (mut p, extras) = generate(2, SMALL);
    let host = *p.stations.keys().next().unwrap();
    p.station_mut(host).unwrap().archived.push(ArchivedStation {
        id: 4_000_000_000,
        name: "old".to_string(),
        comment: None,
        external: None,
        lines: Vec::new(),
    });
    p.counters.next_station_id = 4_000_000_001;
    // `validate` checks both ranges; the encoder refuses through it.
    let alias = Violation::ArchivedIdOutOfRange {
        station: host,
        archived: 4_000_000_000,
    };
    let counter = Violation::CounterOutOfRange {
        counter: CounterKind::Station,
        next: 4_000_000_001,
    };
    assert!(matches!(
        encode(&p, &extras, Mode::Normal),
        Err(EncodeError::Invalid { violations })
            if violations.contains(&alias) && violations.contains(&counter)
    ));
    let text =
        text_of(&encode(&one_line(3), &Extras::default(), Mode::Normal).unwrap()).to_string();
    let first = text.lines().find(|l| l.starts_with("[0,")).unwrap();
    let row = first.trim_end_matches(',').trim_end_matches(']');
    let with_alias = format!("{row},{{\"archived\":[{{\"id\":4000000000,\"name\":\"x\"}}]}}],");
    let bad = text.replacen(first, &with_alias, 1);
    assert!(matches!(
        decode(bad.as_bytes(), &DecodeOptions::default()),
        Err(DecodeError::Invalid { violations })
            if violations.iter().any(|v| matches!(v, Violation::ArchivedIdOutOfRange { .. }))
    ));
    let (salvaged, report) = decode_salvage(bad.as_bytes());
    assert!(salvaged.is_some_and(|q| q.is_valid()), "{report:?}");
    let at = text.find("\"counters\":{\"station\":").unwrap() + "\"counters\":{\"station\":".len();
    let digits = text[at..].find(',').unwrap();
    let counter = format!("{}3000000000{}", &text[..at], &text[at + digits..]);
    assert!(matches!(
        decode(counter.as_bytes(), &DecodeOptions::default()),
        Err(DecodeError::Invalid { violations }) if violations == vec![Violation::CounterOutOfRange {
            counter: CounterKind::Station,
            next: 3_000_000_000,
        }]
    ));
    let (salvaged, report) = decode_salvage(counter.as_bytes());
    assert_eq!(
        salvaged.map(|q| q.counters.next_station_id),
        Some(crate::models::MAX_STATION_ID),
        "{report:?}"
    );
}
