//! Record owners in the file (RS-FX4): an archived station's `into` names
//! the archived station it was archived under. A merged junction archived
//! whole survives a save and a load with its records inside it; a crafted
//! file whose owners dangle, sit under history or cycle is refused (and
//! repaired by salvage) in linear time; an earlier version 2 file's records
//! are all put under their station's own identity. Fully synthetic data.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use chrono::{TimeZone, Utc};

use crate::models::survey::{LineMeta, Project, StartAt, Violation};
use crate::models::{LineId, ShotData, StationId};

use super::test_helpers::{assert_round_trip, text_of, REQUIRES};
use super::{decode, decode_salvage, encode, DecodeError, DecodeOptions, Extras, Mode, Repair};

/// A free line of two shots east from `(0, north)`, its start placed.
fn free_line(p: &mut Project, north: f64, prefix: &str) -> LineId {
    let shots = (0..2)
        .map(|i| ShotData {
            uuid: Some(format!("{prefix}-{i}")),
            length: 10.0,
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            ..ShotData::default()
        })
        .collect();
    let line = p
        .add_line(
            LineMeta {
                name: Some(prefix.to_string()),
                color: "#000".to_string(),
                ..LineMeta::default()
            },
            StartAt::New,
            shots,
        )
        .unwrap();
    let chain = p.line_stations(line).unwrap().to_vec();
    p.set_placement(chain[0], 0.0, north).unwrap();
    for (i, s) in chain.into_iter().enumerate() {
        p.station_mut(s).unwrap().name = format!("{prefix}{i}");
    }
    line
}

/// Junction X (LX native, LX2 named by X's record R) merged into Y keeping
/// Y: `(project, lx2, x, r, y)`. Each identity has a comment and an
/// external id.
fn nested() -> (Project, LineId, StationId, StationId, StationId) {
    let at = |s: i64| Utc.timestamp_opt(s, 0).single().unwrap();
    let mut p = Project::new_at("Owners", None, at(1_763_640_000));
    let lx = free_line(&mut p, 0.0, "X");
    let lx2 = free_line(&mut p, 50.0, "R");
    let ly = free_line(&mut p, 100.0, "Y");
    let station = |p: &Project, line: LineId, i: usize| p.line_stations(line).unwrap()[i];
    let (x, r, y) = (station(&p, lx, 1), station(&p, lx2, 0), station(&p, ly, 1));
    for (s, text) in [(x, "X"), (r, "R"), (y, "Y")] {
        let st = p.station_mut(s).unwrap();
        st.comment = Some(format!("{text} note"));
        st.external = Some(format!("{text}-ext"));
    }
    p.merge_stations(r, x, x, &[]).unwrap();
    p.merge_stations(x, y, y, &[]).unwrap();
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    let records = &p.stations[&y].archived;
    assert_eq!(
        records
            .iter()
            .map(|rec| (rec.id, rec.into))
            .collect::<Vec<_>>(),
        [(r, Some(x)), (x, None)],
        "R is archived under X, X under Y"
    );
    (p, lx2, x, r, y)
}

/// A save then a load keeps every record's owner (`into`), byte for byte,
/// and the loaded survey's Detach still gives X back whole.
#[test]
fn record_owners_survive_a_save_and_a_load() {
    let (p, lx2, x, r, y) = nested();
    let extras = Extras::default();
    let bytes = assert_round_trip(&p, &extras);
    let text = text_of(&bytes);
    assert!(
        text.contains(&format!(
            "{{\"id\":{r},\"name\":\"R0\",\"comment\":\"R note\",\"external\":\"R-ext\",\
             \"into\":{x},\"lines\":[{lx2}]}}"
        )),
        "{text}"
    );
    let mut loaded = decode(&bytes, &DecodeOptions::default()).unwrap().project;
    loaded.detach_line(y, lx2, &[], None).unwrap();
    let back = &loaded.stations[&x];
    assert_eq!(
        (back.name.as_str(), back.comment.as_deref()),
        ("X1", Some("X note"))
    );
    assert_eq!(back.archived.len(), 1);
    assert_eq!((back.archived[0].id, back.archived[0].into), (r, None));
}

/// The text of `nested()` with R's owner replaced by `into`.
fn with_owner(into: &str) -> String {
    let (p, _, x, _, _) = nested();
    let text = text_of(&encode(&p, &Extras::default(), Mode::Normal).unwrap()).to_string();
    let from = format!("\"into\":{x}");
    assert!(text.contains(&from));
    text.replacen(&from, &format!("\"into\":{into}"), 1)
}

/// A6 in a crafted file: an owner that is no record of the station, a
/// record that is its own owner, an owner cycle and an owner that is
/// history are each refused by name, and salvage puts the record under its
/// station's own identity.
#[test]
fn a_crafted_owner_is_refused_and_salvaged() {
    let (p, _, x, r, y) = nested();
    let cycle = {
        // X archived under R and R under X.
        let text = with_owner(&x.to_string());
        let own = format!("{{\"id\":{x},\"name\":\"X1\"");
        assert!(text.contains(&own));
        text.replacen(
            &own,
            &format!("{{\"id\":{x},\"into\":{r},\"name\":\"X1\""),
            1,
        )
    };
    let history = {
        // R archived under a history record H (no lines) of Y.
        let (mut q, _, _, _, _) = nested();
        let h = q.mint_station_id().unwrap();
        let mut record = q.stations[&y].archived[1].clone();
        record.id = h;
        record.lines.clear();
        q.station_mut(y).unwrap().archived.push(record);
        let text = text_of(&encode(&q, &Extras::default(), Mode::Normal).unwrap()).to_string();
        text.replacen(&format!("\"into\":{x}"), &format!("\"into\":{h}"), 1)
    };
    let h = p.counters.next_station_id;
    let cases = [
        (
            with_owner("999"),
            Violation::DanglingArchivedOwner {
                station: y,
                archived: r,
                into: 999,
            },
        ),
        (
            with_owner(&r.to_string()),
            Violation::DanglingArchivedOwner {
                station: y,
                archived: r,
                into: r,
            },
        ),
        (
            cycle,
            Violation::ArchivedOwnerCycle {
                station: y,
                archived: r,
            },
        ),
        (
            history,
            Violation::ArchivedUnderHistory {
                station: y,
                archived: r,
                into: h,
            },
        ),
    ];
    for (text, expected) in cases {
        let violations = match decode(text.as_bytes(), &DecodeOptions::default()) {
            Err(DecodeError::Invalid { violations }) => violations,
            other => panic!("expected an invariant refusal, got {other:?}"),
        };
        assert!(violations.contains(&expected), "{violations:?}");
        let (project, report) = decode_salvage(text.as_bytes());
        let project = project.unwrap_or_else(|| panic!("salvage gave up: {report:?}"));
        assert!(project.validate().is_empty());
        assert!(
            report.repairs.iter().any(
                |rep| matches!(rep, Repair::ArchivedOwnerCleared { station, .. } if *station == y)
            ),
            "{:?}",
            report.repairs
        );
        assert!(project.stations[&y]
            .archived
            .iter()
            .all(|rec| rec.into != Some(999)));
    }
}

/// A crafted station holding 200,000 records chained by their owners, the
/// last one's owner the first (a cycle through all of them), is refused,
/// and salvaged, in about the time a read takes (about a second here):
/// every owner is resolved once, and salvage clears them in one walk.
#[test]
fn a_long_owner_cycle_is_refused_without_hanging() {
    const N: u32 = 200_000;
    let mut records = String::new();
    for i in 0..N {
        let id = 10 + i;
        let into = 10 + (i + 1) % N;
        if i > 0 {
            records.push(',');
        }
        write!(records, "{{\"id\":{id},\"name\":\"r{i}\",\"into\":{into}}}").unwrap();
    }
    let text = format!(
        "{{\"format\":\"stalx\",\"version\":2,\"solver\":1,{REQUIRES},\n\
         \"conventions\":{{\"length\":\"m\",\"angles\":\"deg\",\"azimuth\":\"cw-from-north\",\
         \"inclination\":\"+down\",\"depth\":\"+down\",\"local\":\"enu-m\",\"gps\":\"wgs84-deg\",\
         \"temperature\":\"degC\"}},\n\
         \"project\":{{\"name\":\"x\",\"created\":\"2025-01-01T00:00:00Z\",\
         \"modified\":\"2025-01-01T00:00:00Z\"}},\n\
         \"counters\":{{\"station\":{},\"closure\":0,\"section\":0}},\n\
         \"stations\":[\n[0,\"0\",{{\"archived\":[{records}]}}],\n[1,\"1\"]\n],\n\
         \"lines\":[\n{{\"id\":0,\"color\":\"#000\",\"start\":0}},\n\
         [\"u0\",1,1.0,0.0,0.0,0.0,0.0]\n],\n\
         \"placements\":[{{\"station\":0,\"x\":0.0,\"y\":0.0}}],\n\"end\":true}}\n",
        10 + N
    );
    let started = Instant::now();
    let violations = match decode(text.as_bytes(), &DecodeOptions::default()) {
        Err(DecodeError::Invalid { violations }) => violations,
        other => panic!("expected an invariant refusal, got {other:?}"),
    };
    assert!(violations
        .iter()
        .any(|v| matches!(v, Violation::ArchivedOwnerCycle { station: 0, .. })));
    let (project, _) = decode_salvage(text.as_bytes());
    assert!(project.is_some_and(|p| p.validate().is_empty()));
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(20), "took {elapsed:?}");
}

/// An earlier version 2 file (no `archived-stations`) never had owners: its
/// load-time upgrade (RS 8.2) puts every record under its station's own
/// identity, even one whose object says otherwise (step 2 may then give the
/// junction another of its identities, by rule K).
#[test]
fn an_earlier_files_records_are_put_under_their_station() {
    let (p, _, _, _, _) = nested();
    let text = text_of(&encode(&p, &Extras::default(), Mode::Normal).unwrap()).to_string();
    let earlier = text.replacen(REQUIRES, "\"requires\":[]", 1);
    assert_ne!(earlier, text);
    let mut decoded = decode(earlier.as_bytes(), &DecodeOptions::default()).unwrap();
    decoded.upgrade().unwrap();
    let project = &decoded.project;
    assert!(project.validate().is_empty(), "{:?}", project.validate());
    let records: Vec<_> = project
        .stations
        .values()
        .flat_map(|s| &s.archived)
        .collect();
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|rec| rec.into.is_none()), "{records:?}");
}
