//! Round trip, byte fixpoint, float bits, times and the pinned summary.

use chrono::{TimeZone, Utc};

use crate::models::survey::Project;
use crate::models::{ShotData, SurveyCredit};

use super::summary::Summary;
use super::test_helpers::{assert_round_trip, generate, text_of, Rng, Shape, MEDIUM, SMALL};
use super::write::{float, normalized_time, parse_time, time_text};
use super::{decode, encode, probe_bytes, DecodeOptions, Extras, Mode};

#[test]
fn generated_surveys_round_trip_and_reach_a_byte_fixpoint() {
    for seed in 0..120 {
        let shape = if seed % 4 == 0 { MEDIUM } else { SMALL };
        let (p, extras) = generate(seed, shape);
        assert_round_trip(&p, &extras);
    }
}

#[test]
fn plain_surveys_round_trip() {
    for seed in 0..20 {
        let (p, extras) = generate(
            seed,
            Shape {
                lines: 30,
                max_shots: 30,
                rich: false,
            },
        );
        assert_round_trip(&p, &extras);
    }
}

#[test]
fn an_empty_project_round_trips() {
    let p = Project::new_at("Empty", None, Utc.timestamp_opt(0, 0).single().unwrap());
    let bytes = assert_round_trip(&p, &Extras::default());
    let text = text_of(&bytes);
    assert!(text.starts_with("{\"format\":\"stalx\",\"version\":2,\"solver\":1,\"requires\":[],\n"));
    assert!(text.ends_with(",\n\"end\":true}\n"));
    assert!(!text.contains("\"lines\":["), "empty members are omitted");
    assert!(!text.contains("\"history\""), "empty members are omitted");
}

#[test]
fn every_record_is_on_its_own_line() {
    let (p, extras) = generate(7, MEDIUM);
    let bytes = encode(&p, &extras, Mode::Normal).unwrap();
    let text = text_of(&bytes);
    let rows = text.lines().filter(|l| l.starts_with("[\"")).count();
    let shots: usize = p.lines().iter().map(|l| l.shots.len()).sum();
    assert_eq!(rows, shots, "one physical line per shot row");
    let stations = text
        .lines()
        .filter(|l| l.starts_with('[') && !l.starts_with("[\""))
        .count();
    assert_eq!(stations, p.stations.len(), "one physical line per station");
    assert_eq!(
        text.lines().filter(|l| l.starts_with("{\"id\":") && l.contains("\"start\":")).count(),
        p.lines().len(),
        "one physical line per line header"
    );
}

/// Random bit patterns through the whole pipeline: a shot holding them is
/// written, read back and compared bit for bit.
#[test]
fn every_finite_double_survives_bit_for_bit() {
    let mut rng = Rng(0x5eed);
    let mut values: Vec<f64> = vec![
        0.0,
        -0.0,
        f64::MIN_POSITIVE,
        -f64::MIN_POSITIVE,
        f64::from_bits(1),
        -f64::from_bits(1),
        f64::MAX,
        f64::MIN,
        f64::EPSILON,
        0.1,
        1e16,
        1e-7,
        123_456_789_012_345_680.0,
        5e-324,
    ];
    while values.len() < 4000 {
        let v = f64::from_bits(rng.next());
        if v.is_finite() {
            values.push(v);
        }
        values.push(f64::from_bits(rng.next() % (1 << 52))); // subnormals
    }
    let mut p = Project::new_at("Floats", None, Utc.timestamp_opt(0, 0).single().unwrap());
    let shots: Vec<ShotData> = values
        .chunks(5)
        .enumerate()
        .map(|(i, c)| ShotData {
            uuid: Some(format!("f{i}")),
            length: c[0],
            azimuth_in: *c.get(1).unwrap_or(&0.0),
            azimuth_out: *c.get(2).unwrap_or(&0.0),
            depth_in: *c.get(3).unwrap_or(&0.0),
            depth_out: *c.get(4).unwrap_or(&0.0),
            left: c.get(2).copied(),
            temperature: c.get(1).copied(),
            ..ShotData::default()
        })
        .collect();
    let line = p
        .add_line(Default::default(), crate::models::survey::StartAt::New, shots)
        .unwrap();
    let start = p.line(line).unwrap().start;
    p.set_placement(start, -0.0, f64::from_bits(3)).unwrap();
    let bytes = encode(&p, &Extras::default(), Mode::Normal).unwrap();
    let q = decode(&bytes, &DecodeOptions::default()).unwrap().project;
    let bits = |p: &Project| -> Vec<u64> {
        p.lines()
            .iter()
            .flat_map(|l| &l.shots)
            .flat_map(|s| {
                [
                    s.length,
                    s.azimuth_in,
                    s.azimuth_out,
                    s.depth_in,
                    s.depth_out,
                    s.left.unwrap_or(1.5),
                    s.temperature.unwrap_or(1.5),
                ]
            })
            .chain(p.placements.iter().flat_map(|pl| [pl.x, pl.y]))
            .map(f64::to_bits)
            .collect()
    };
    assert_eq!(bits(&p), bits(&q));
}

/// The writer's spelling, parsed by serde_json (the reader's parser), gives
/// the same bits; and the spelling is the shortest ryu gives.
#[test]
fn the_float_spelling_is_shortest_and_exact() {
    let mut rng = Rng(42);
    for _ in 0..200_000 {
        let v = f64::from_bits(rng.next());
        if !v.is_finite() {
            continue;
        }
        let mut out = Vec::new();
        float(&mut out, v, String::new).unwrap();
        let text = std::str::from_utf8(&out).unwrap();
        let back: f64 = serde_json::from_str(text).unwrap();
        assert_eq!(back.to_bits(), v.to_bits(), "{text}");
    }
    let spell = |v: f64| {
        let mut out = Vec::new();
        float(&mut out, v, String::new).unwrap();
        String::from_utf8(out).unwrap()
    };
    assert_eq!(spell(-0.0), "-0.0");
    assert_eq!(spell(0.0), "0.0");
    assert_eq!(spell(1.0), "1.0");
    assert_eq!(spell(12.5), "12.5");
    assert_eq!(spell(0.1 + 0.2), "0.30000000000000004");
    assert_eq!(spell(1e16), "1e16");
    assert_eq!(spell(1.5e-7), "1.5e-7");
    assert_eq!(spell(5e-324), "5e-324");
    assert!(float(&mut Vec::new(), f64::NAN, || "x".to_string()).is_err());
    assert!(float(&mut Vec::new(), f64::INFINITY, || "x".to_string()).is_err());
}

#[test]
fn times_keep_their_nanoseconds_and_are_clamped_like_810() {
    let at = Utc.timestamp_opt(1_763_640_000, 123_456_789).single().unwrap();
    assert_eq!(time_text(at), "2025-11-20T12:00:00.123456789Z");
    assert_eq!(parse_time(&time_text(at)), Some(at));
    // Any offset reads; the instant is kept.
    assert_eq!(parse_time("2025-11-20T13:00:00.123456789+01:00"), Some(at));
    // Outside 0001-9999 is brought in, on the way in and out.
    let early = parse_time("0000-06-01T00:00:00Z").unwrap();
    assert_eq!(time_text(early), "0001-01-01T00:00:00.000000000Z");
    assert_eq!(normalized_time(early), early);

    let mut p = Project::new_at("Times", None, at);
    p.modified_at = Utc.timestamp_opt(1, 1).single().unwrap();
    let q = decode(&encode(&p, &Extras::default(), Mode::Normal).unwrap(), &DecodeOptions::default())
        .unwrap()
        .project;
    assert_eq!(q.created_at, p.created_at);
    assert_eq!(q.modified_at, p.modified_at);
    // The `.plnx` ownership string is formatted from memory, so it survives.
    assert_eq!(q.created_at.to_rfc3339(), p.created_at.to_rfc3339());
}

/// `summary` is a cache: the writer recomputes it, and what it wrote is the
/// recomputed value for the decoded project.
#[test]
fn the_summary_equals_the_recomputed_one() {
    for seed in 0..40 {
        let (p, extras) = generate(seed, SMALL);
        let bytes = encode(&p, &extras, Mode::Normal).unwrap();
        let header = probe_bytes(&bytes);
        let decoded = decode(&bytes, &DecodeOptions::default()).unwrap();
        let recomputed = Summary::of(&decoded.project, decoded.extras.speleodb.as_ref());
        assert_eq!(header.summary.as_ref(), Some(&recomputed), "seed {seed}");
    }
}

#[test]
fn a_summary_length_that_overflows_is_left_out() {
    let mut p = Project::new_at("Huge", None, Utc.timestamp_opt(0, 0).single().unwrap());
    let shots = (0..3)
        .map(|i| ShotData {
            uuid: Some(format!("h{i}")),
            length: f64::MAX,
            ..ShotData::default()
        })
        .collect();
    let line = p
        .add_line(Default::default(), crate::models::survey::StartAt::New, shots)
        .unwrap();
    let start = p.line(line).unwrap().start;
    p.set_placement(start, 0.0, 0.0).unwrap();
    let bytes = assert_round_trip(&p, &Extras::default());
    let summary = probe_bytes(&bytes).summary.unwrap();
    assert_eq!(summary.length, None);
    assert_eq!(summary.shots, 3);
}

#[test]
fn credits_shared_by_a_line_are_written_once_in_its_header() {
    let mut p = Project::new_at("Credit", None, Utc.timestamp_opt(0, 0).single().unwrap());
    let team = std::sync::Arc::new(SurveyCredit {
        surveyors: vec![crate::models::Person {
            name: "A. Diver".to_string(),
            email: None,
        }],
        explorers: Vec::new(),
        date: None,
    });
    let shots = (0..10)
        .map(|i| ShotData {
            uuid: Some(format!("c{i}")),
            length: 1.0,
            credit: (i != 3).then(|| std::sync::Arc::clone(&team)),
            ..ShotData::default()
        })
        .collect();
    let line = p
        .add_line(Default::default(), crate::models::survey::StartAt::New, shots)
        .unwrap();
    let start = p.line(line).unwrap().start;
    p.set_placement(start, 0.0, 0.0).unwrap();
    let bytes = assert_round_trip(&p, &Extras::default());
    let text = text_of(&bytes);
    assert_eq!(text.matches("A. Diver").count(), 1, "{text}");
    assert_eq!(text.matches("\"credit\":null").count(), 1, "{text}");
}

#[test]
#[ignore = "prints a sample file"]
fn print_a_sample() {
    let (p, extras) = generate(3, SMALL);
    let bytes = encode(&p, &extras, Mode::Normal).unwrap();
    println!("{}", text_of(&bytes));
}
