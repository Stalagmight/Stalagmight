//! Work and memory stay proportional to the input: the cases an independent
//! review found quadratic or unbounded, pinned.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::models::survey::{ArchivedStation, CounterKind, LineMeta, Project, StartAt, Violation};
use crate::models::{Person, ShotData, SurveyCredit};

use super::test_helpers::{generate, text_of, REQUIRES, SMALL};
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
        REQUIRES,
        &format!("\"requires\":[\"archived-stations\",{}]", many(500)),
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

/// `text` (a file this build wrote) with every station row renamed by
/// `name` (its Station ID to its new name), and, with `earlier`, without the
/// kept-stations feature, as an earlier build of version 2 wrote it.
fn rewritten(text: &str, earlier: bool, name: impl Fn(u64) -> String) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_stations = false;
    for row in text.split_inclusive('\n') {
        if row.starts_with("\"stations\":[") {
            in_stations = true;
        } else if in_stations && row.starts_with(']') {
            in_stations = false;
        } else if in_stations {
            let id: u64 = row[1..row.find(',').unwrap()].parse().unwrap();
            let comma = if row.trim_end().ends_with("],") {
                ","
            } else {
                ""
            };
            let _ = writeln!(
                out,
                "[{id},{}]{comma}",
                serde_json::to_string(&name(id)).unwrap()
            );
            continue;
        }
        out.push_str(row);
    }
    if earlier {
        out = out.replacen(REQUIRES, "\"requires\":[]", 1);
    }
    out
}

fn repeats(violations: &[Violation]) -> usize {
    violations
        .iter()
        .filter(|v| matches!(v, Violation::DuplicateStationName { .. }))
        .count()
}

/// Data review finding data-1: N8 found each repeat of a line once by a
/// linear scan of the repeats found so far, so one long line of stations
/// that all share a name took about 5e9 comparisons before the file was
/// refused as damaged. Injection: the scan (`out.contains`) back in
/// `line_name_repeats` takes this test far past its bound.
#[test]
fn a_long_line_of_one_repeated_name_is_refused_in_linear_time() {
    let bytes = encode(
        &one_line(MAX_SHOTS_PER_LINE),
        &Extras::default(),
        Mode::Normal,
    )
    .unwrap();
    let text = rewritten(text_of(&bytes), false, |_| "x".to_string());
    let started = Instant::now();
    let refused = decode(text.as_bytes(), &DecodeOptions::default());
    let elapsed = started.elapsed();
    match refused {
        Err(DecodeError::Invalid { violations }) => {
            assert_eq!(repeats(&violations), MAX_SHOTS_PER_LINE);
        }
        other => panic!("{:?}", other.map(|_| ())),
    }
    assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
}

/// Data review finding data-2: an earlier version 2 file whose long line
/// shows one name at every station is settled by rule L at decode, which
/// built each lettered station's taken names from every station of its
/// lines and re-ranked every holder from the whole closure list: about
/// 1e10 string clones before decode returned. Injection: rule L as it was
/// (`names_around` per lettered station) runs this test past 3 minutes
/// here; it takes about 2 s.
#[test]
fn an_earlier_file_with_a_long_line_of_one_name_is_lettered_in_linear_time() {
    let bytes = encode(
        &one_line(MAX_SHOTS_PER_LINE),
        &Extras::default(),
        Mode::Normal,
    )
    .unwrap();
    let text = rewritten(text_of(&bytes), true, |_| "x".to_string());
    let started = Instant::now();
    let mut decoded = decode(text.as_bytes(), &DecodeOptions::default()).unwrap();
    let report = decoded.upgrade().unwrap();
    let elapsed = started.elapsed();
    assert_eq!(report.lettered, MAX_SHOTS_PER_LINE);
    assert!(decoded.project.validate().is_empty());
    assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
}

/// A version 2 file written by hand: line 0 runs `shots` shots from station
/// 0 through stations `1..=shots`; `sides[k]` starts line `k + 1`, one shot
/// from that station of line 0 to a station of its own. `name` names every
/// station, `record` gives a station of line 0 a record naming line 0 (with
/// its name), and `closures` ties stations of line 0, each optionally
/// archiving its end `"a"` or `"b"`. With `earlier`, the file does not list
/// the kept-stations feature, as an earlier build of version 2 wrote it.
struct Crafted<'a> {
    shots: u64,
    sides: &'a [u64],
    name: &'a dyn Fn(u64) -> String,
    record: &'a dyn Fn(u64) -> Option<String>,
    closures: &'a [(u64, u64, Option<&'a str>)],
    earlier: bool,
}

impl Crafted<'_> {
    fn text(&self) -> String {
        let requires = if self.earlier {
            "\"requires\":[]"
        } else {
            REQUIRES
        };
        let sides = self.sides.len() as u64;
        let side_station = |k: u64| self.shots + 1 + k;
        let mut next = self.shots + 1 + sides;
        let mut t = format!(
            "{{\"format\":\"stalx\",\"version\":2,\"solver\":1,{requires},\n\
             \"conventions\":{{\"length\":\"m\",\"angles\":\"deg\",\"azimuth\":\"cw-from-north\",\
             \"inclination\":\"+down\",\"depth\":\"+down\",\"local\":\"enu-m\",\"gps\":\"wgs84-deg\",\
             \"temperature\":\"degC\"}},\n\
             \"project\":{{\"name\":\"x\",\"created\":\"2025-01-01T00:00:00Z\",\
             \"modified\":\"2025-01-01T00:00:00Z\"}},\n\"stations\":["
        );
        let mut rows = Vec::new();
        let stations = next;
        for id in 0..stations {
            let name = serde_json::to_string(&(self.name)(id)).unwrap();
            match (id <= self.shots).then(|| (self.record)(id)).flatten() {
                Some(record) => {
                    let record = serde_json::to_string(&record).unwrap();
                    rows.push(format!(
                        "[{id},{name},{{\"archived\":[{{\"id\":{next},\"name\":{record},\"lines\":[0]}}]}}]"
                    ));
                    next += 1;
                }
                None => rows.push(format!("[{id},{name}]")),
            }
        }
        t.push_str(&rows.join(",\n"));
        let _ = write!(
            t,
            "\n],\n\"counters\":{{\"station\":{next},\"closure\":{},\"section\":0}},\n\"lines\":[\n\
             {{\"id\":0,\"color\":\"#000\",\"start\":0}}",
            self.closures.len() + 1
        );
        for i in 0..self.shots {
            let _ = write!(t, ",\n[\"u{i}\",{},1.0,0.0,0.0,0.0,0.0]", i + 1);
        }
        for (k, &at) in (0..).zip(self.sides) {
            let _ = write!(
                t,
                ",\n{{\"id\":{},\"color\":\"#000\",\"start\":{at}}},\n[\"s{k}\",{},1.0,90.0,0.0,0.0,0.0]",
                k + 1,
                side_station(k)
            );
        }
        t.push_str("\n],\n\"placements\":[{\"station\":0,\"x\":0.0,\"y\":0.0}]");
        if !self.closures.is_empty() {
            t.push_str(",\n\"closures\":[");
            let rows: Vec<String> = (1..)
                .zip(self.closures)
                .map(|(id, &(a, b, archived))| {
                    let archived = archived.map_or(String::new(), |end| format!(",\"archived\":\"{end}\""));
                    format!("{{\"id\":{id},\"a\":{a},\"a_via\":0,\"b\":{b},\"b_via\":0,\"strategy\":\"batch\"{archived}}}")
                })
                .collect();
            t.push_str(&rows.join(",\n"));
            t.push(']');
        }
        t.push_str(",\n\"end\":true}\n");
        t
    }
}

fn own_name(id: u64) -> String {
    id.to_string()
}

fn no_record(_: u64) -> Option<String> {
    None
}

/// 50,000 closures (the cap) chaining consecutive stations of a 99,999-shot
/// line, each archiving its later end: every shown identity resolves
/// through up to 50,000 kept ends, and C2 follows each chain. Data review
/// finding data-2 (the upgrade at decode must stay linear) in the closure
/// shape: rule K measured the whole line twice per closure, the
/// normalisation and validate walked each chain from its kept end, and
/// every slot's shown identity walked the chain again. Injection: those
/// four as they were run this test past 4 minutes here; it takes about 3 s.
#[test]
fn a_chain_of_closure_archives_decodes_in_linear_time() {
    let closures: Vec<(u64, u64, Option<&str>)> = (0..50_000).map(|i| (i, i + 1, None)).collect();
    let archived: Vec<(u64, u64, Option<&str>)> =
        (0..50_000).map(|i| (i, i + 1, Some("b"))).collect();
    for (earlier, closures) in [(true, &closures), (false, &archived)] {
        let text = Crafted {
            shots: MAX_SHOTS_PER_LINE as u64,
            sides: &[],
            name: &own_name,
            record: &no_record,
            closures,
            earlier,
        }
        .text();
        let started = Instant::now();
        let mut decoded = decode(text.as_bytes(), &DecodeOptions::default()).unwrap();
        let report = decoded.upgrade();
        let elapsed = started.elapsed();
        assert_eq!(
            report.map(|r| r.closures_archived),
            earlier.then_some(50_000)
        );
        assert_eq!(decoded.project.shown(50_000), 0);
        assert!(
            elapsed < Duration::from_secs(20),
            "earlier {earlier}: {elapsed:?}"
        );
    }
}

/// 40,000 junctions of a 99,999-shot line (line ids stop at 42,948), each a
/// one-shot side line's own station that an earlier build showed on the long
/// line by a record (a U1 alias): the upgrade keeps each record (its line is the longest) and
/// re-keys the node to its id. Data review finding data-2 in the junction
/// shape: rule K measured the long line once per junction, and each re-key
/// rebuilt the topology and walked every datum, closure and placement.
/// Injection: one `rekey_node` and one measure per junction, as before,
/// run this test past 4 minutes here; it takes about 2 s.
#[test]
fn an_earlier_file_with_many_swapped_junctions_upgrades_in_linear_time() {
    let sides: Vec<u64> = (1..=40_000).collect();
    let record = |id: u64| (1..=40_000).contains(&id).then(|| format!("r{id}"));
    let text = Crafted {
        shots: MAX_SHOTS_PER_LINE as u64,
        sides: &sides,
        name: &own_name,
        record: &record,
        closures: &[],
        earlier: true,
    }
    .text();
    let started = Instant::now();
    let mut decoded = decode(text.as_bytes(), &DecodeOptions::default()).unwrap();
    let report = decoded.upgrade().unwrap();
    let elapsed = started.elapsed();
    assert_eq!(report.swapped.len(), 40_000);
    let p = &decoded.project;
    let junction = p.station_at(crate::models::StationRef::new(0, 7)).unwrap();
    assert_eq!(p.stations[&junction].name, "r7");
    assert_eq!(
        p.record_naming(junction, 7).map(|r| r.name.as_str()),
        Some("7")
    );
    assert!(elapsed < Duration::from_secs(20), "{elapsed:?}");
}

/// Data review finding data-2, last part: stalx-reader's deadline stopped
/// nothing inside `decode`, which runs an earlier file's upgrade, so it was
/// checked only before and after. A deadline in the options is now checked
/// between decode's phases; one already passed stops a decode of either
/// kind of file, after its parse, and none or a later one changes nothing.
/// Injection: on 333e24ac5 `DecodeOptions` has no deadline.
#[test]
fn a_decode_deadline_stops_between_phases() {
    let bytes = encode(&one_line(100), &Extras::default(), Mode::Normal).unwrap();
    let earlier = rewritten(text_of(&bytes), true, |_| "x".to_string());
    for text in [text_of(&bytes), earlier.as_str()] {
        let passed = DecodeOptions {
            deadline: Some(Instant::now()),
            ..DecodeOptions::default()
        };
        assert_eq!(
            decode(text.as_bytes(), &passed).map(|_| ()),
            Err(DecodeError::DeadlinePassed)
        );
        let later = DecodeOptions {
            deadline: Some(Instant::now() + Duration::from_hours(1)),
            ..DecodeOptions::default()
        };
        assert!(decode(text.as_bytes(), &later).is_ok());
    }
}

/// Data review finding data-5: the archived-station cap counts a record once
/// plus once per line it names, and an earlier build's U1 alias row the
/// same (its one line, if any), so the two forms of the same stations meet
/// the cap alike: here two stations of line 0 each keep one naming line 0
/// (4 against a cap of 3). Injection: on 333e24ac5 an alias row counted 1
/// whatever its line, and the alias form passed.
#[test]
fn alias_rows_count_like_records_against_the_archived_cap() {
    let record = |id: u64| (1..=2).contains(&id).then(|| format!("r{id}"));
    let records = Crafted {
        shots: 3,
        sides: &[],
        name: &own_name,
        record: &record,
        closures: &[],
        earlier: true,
    }
    .text();
    let aliases = records
        .replace(
            "{\"archived\":[{\"id\":4,\"name\":\"r1\",\"lines\":[0]}]}",
            "{\"aliases\":[[4,\"r1\",0]]}",
        )
        .replace(
            "{\"archived\":[{\"id\":5,\"name\":\"r2\",\"lines\":[0]}]}",
            "{\"aliases\":[[5,\"r2\",0]]}",
        );
    assert_eq!(aliases.matches("\"aliases\"").count(), 2, "{aliases}");
    let mut options = DecodeOptions::default();
    for text in [&records, &aliases] {
        options.limits.archived = 4;
        let decoded = decode(text.as_bytes(), &options);
        assert!(decoded.is_ok(), "{:?} {text}", decoded.err());
        options.limits.archived = 3;
        assert!(
            matches!(
                decode(text.as_bytes(), &options),
                Err(DecodeError::TooComplex {
                    limit: Limit::Archived,
                    max: 3
                })
            ),
            "{text}"
        );
    }
}

/// Data review finding data-6: a record's `lines` array counts against the
/// archived-station cap as it streams, like every other array, so a crafted
/// record whose array fills the file is refused before it is held: here the
/// cap is passed long before the array's bad last element. Injection: on
/// 333e24ac5 the array was read whole first, and the bad element refused it
/// as malformed instead.
#[test]
fn record_lines_are_counted_as_they_arrive() {
    let record = |id: u64| (id == 1).then(|| "r1".to_string());
    let text = Crafted {
        shots: 3,
        sides: &[],
        name: &own_name,
        record: &record,
        closures: &[],
        earlier: false,
    }
    .text();
    let flood = format!("\"lines\":[{}\"x\"]", "0,".repeat(50));
    let text = text.replacen("\"lines\":[0]", &flood, 1);
    assert!(text.contains(&flood));
    let mut options = DecodeOptions::default();
    options.limits.archived = 10;
    assert!(matches!(
        decode(text.as_bytes(), &options),
        Err(DecodeError::TooComplex {
            limit: Limit::Archived,
            max: 10
        })
    ));
}
