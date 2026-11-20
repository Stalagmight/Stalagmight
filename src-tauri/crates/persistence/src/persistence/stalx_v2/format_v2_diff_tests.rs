//! Diff locality: one edit changes O(1) physical lines of the file. Measured
//! on a 5,000-shot survey as the multiset of lines removed plus added (what a
//! line-based diff reports for edits at separate places).

use std::collections::HashMap;

use crate::models::survey::{ClosureStrategy, Project, Shot, StartAt};
use crate::models::ShotData;

use super::test_helpers::Rng;
use super::{encode, Extras, Mode};

/// The bench shape: 200 lines of 25 shots, each line starting at a station
/// of an earlier one (a junction), one placement.
pub(super) fn survey(lines: usize, shots_per: usize, seed: u64) -> Project {
    let mut rng = Rng(seed);
    let mut p = Project::new_at(
        "Synthetic bench system",
        Some("Nowhere, synthetic".to_string()),
        chrono::DateTime::from_timestamp(1_763_640_000, 0).unwrap(),
    );
    let round = |v: f64, places: i32| {
        let m = 10f64.powi(places);
        (v * m).round() / m
    };
    let mut uuid = 0u64;
    for i in 0..lines {
        let mut depth = round(rng.range(3.0, 30.0), 1);
        let shots: Vec<ShotData> = (0..shots_per)
            .map(|j| {
                uuid += 1;
                let az = round(rng.range(0.0, 360.0), 1) % 360.0;
                let depth_in = depth;
                depth = round((depth + rng.range(-1.5, 1.5)).max(0.5), 1);
                let lrud = rng.chance(0.7);
                ShotData {
                    uuid: Some(format!("{seed:08x}-{uuid:04x}-4000-8000-{uuid:012x}")),
                    azimuth_in: az,
                    azimuth_out: az,
                    length: round(rng.range(0.5, 12.0), 2),
                    depth_in,
                    depth_out: depth,
                    left: lrud.then(|| round(rng.range(0.2, 6.0), 1)),
                    right: lrud.then(|| round(rng.range(0.2, 6.0), 1)),
                    up: lrud.then(|| round(rng.range(0.2, 4.0), 1)),
                    down: lrud.then(|| round(rng.range(0.2, 4.0), 1)),
                    comment: rng
                        .chance(0.08)
                        .then(|| format!("lead {j} on the left, low silt bedding")),
                    marker_index: rng.chance(0.05).then(|| rng.below(4) as u32),
                    temperature: rng.chance(0.5).then(|| round(rng.range(22.0, 26.0), 1)),
                    ..Default::default()
                }
            })
            .collect();
        let meta = crate::models::survey::LineMeta {
            name: Some(format!("Line {i}")),
            color: "#4ecdc4".to_string(),
            ..Default::default()
        };
        let start = if i == 0 {
            StartAt::New
        } else {
            let ids: Vec<_> = p.stations.keys().copied().collect();
            StartAt::Station(ids[rng.below(ids.len())])
        };
        p.add_line(meta, start, shots).unwrap();
    }
    // Names unique project-wide; a comment on 2% of stations.
    let ids: Vec<_> = p.stations.keys().copied().collect();
    for id in ids {
        let comment = rng
            .chance(0.02)
            .then(|| "tie-off on a rock, arrow pointing out".to_string());
        let station = p.station_mut(id).unwrap();
        station.name = format!("A{id}");
        station.comment = comment;
    }
    let first = p.lines()[0].start;
    p.set_placement(first, 0.0, 0.0).unwrap();
    for _ in 0..lines / 20 {
        let ids: Vec<_> = p.stations.keys().copied().collect();
        let (a, b) = (ids[rng.below(ids.len())], ids[rng.below(ids.len())]);
        if a != b {
            p.push_closure(a, None, b, None, ClosureStrategy::Batch);
        }
    }
    assert!(p.validate().is_empty());
    p
}

/// Lines removed plus lines added, as multisets.
fn changed_lines(a: &[u8], b: &[u8]) -> usize {
    let mut counts: HashMap<&[u8], i64> = HashMap::new();
    for line in a.split(|&c| c == b'\n') {
        *counts.entry(line).or_default() += 1;
    }
    for line in b.split(|&c| c == b'\n') {
        *counts.entry(line).or_default() -= 1;
    }
    counts.values().map(|v| v.unsigned_abs() as usize).sum()
}

type Edit = fn(&mut Project, &mut Rng);

fn edits() -> Vec<(&'static str, Edit)> {
    vec![
        ("re-save, no edit", |_, _| {}),
        ("edit one shot's length", |p, rng| {
            let line = p.lines()[rng.below(p.lines().len())].id;
            let shots = &mut p.line_mut(line).unwrap().shots;
            let i = rng.below(shots.len());
            shots[i].length += 0.25;
        }),
        ("edit a shot near the entrance", |p, _| {
            let line = p.lines()[0].id;
            p.line_mut(line).unwrap().shots[0].azimuth_in += 1.0;
        }),
        ("comment a shot", |p, rng| {
            let line = p.lines()[rng.below(p.lines().len())].id;
            let shots = &mut p.line_mut(line).unwrap().shots;
            let i = rng.below(shots.len());
            shots[i].comment = Some("checked twice".to_string());
        }),
        ("rename a station", |p, rng| {
            let ids: Vec<_> = p.stations.keys().copied().collect();
            let id = ids[rng.below(ids.len())];
            p.station_mut(id).unwrap().name = format!("Renamed {id}");
        }),
        ("append a shot to a line", |p, rng| {
            let line = p.lines()[rng.below(p.lines().len())].id;
            let to = p.mint_station("appended").unwrap();
            let shot = Shot::from_data(
                0,
                ShotData {
                    uuid: Some("appended-shot".to_string()),
                    length: 2.0,
                    ..Default::default()
                },
                to,
            );
            p.line_mut(line).unwrap().shots.push(shot);
        }),
        ("insert a shot mid-line", |p, rng| {
            let line = p.lines()[rng.below(p.lines().len())].id;
            let to = p.mint_station("inserted").unwrap();
            let shot = Shot::from_data(
                0,
                ShotData {
                    uuid: Some("inserted-shot".to_string()),
                    length: 2.0,
                    ..Default::default()
                },
                to,
            );
            let shots = &mut p.line_mut(line).unwrap().shots;
            let at = shots.len() / 2;
            shots.insert(at, shot);
        }),
        ("delete a line's last shot", |p, _| {
            // The last line's end station is on no other line in this shape.
            let line = p.lines().last().unwrap().id;
            let shot = p.line_mut(line).unwrap().shots.pop().unwrap();
            if p.slots_of(shot.to).is_empty() {
                p.stations.remove(&shot.to);
                p.closures.retain(|c| c.a != shot.to && c.b != shot.to);
            }
        }),
        ("add a closure", |p, _| {
            let line = p.lines()[3].id;
            let chain = p.line_stations(line).unwrap().to_vec();
            p.push_closure(
                chain[1],
                Some(line),
                chain[chain.len() - 1],
                None,
                ClosureStrategy::Move,
            );
        }),
        ("add a line at a junction", |p, _| {
            let start = p.lines()[5].start;
            let shots = (0..25)
                .map(|i| ShotData {
                    uuid: Some(format!("new-line-{i}")),
                    length: 3.0,
                    ..Default::default()
                })
                .collect();
            let line = p
                .add_line(
                    crate::models::survey::LineMeta::default(),
                    StartAt::Station(start),
                    shots,
                )
                .unwrap();
            let chain = p.line_stations(line).unwrap().to_vec();
            for station in &chain[1..] {
                let name = format!("N{station}");
                p.station_mut(*station).unwrap().name = name;
            }
        }),
        ("move the placement", |p, _| {
            p.placements[0].x += 10.0;
        }),
        ("rename the project", |p, _| {
            p.name = "Renamed system".to_string()
        }),
    ]
}

#[test]
fn one_edit_touches_a_bounded_number_of_lines() {
    let base = survey(200, 25, 7);
    let extras = Extras::default();
    let before = encode(&base, &extras, Mode::Normal).unwrap();
    let mut rng = Rng(99);
    let mut report = Vec::new();
    for (name, edit) in edits() {
        let mut worst = 0;
        for _ in 0..5 {
            let mut p = base.clone();
            edit(&mut p, &mut rng);
            assert!(p.validate().is_empty(), "{name}: the edit broke the model");
            let after = encode(&p, &extras, Mode::Normal).unwrap();
            worst = worst.max(changed_lines(&before, &after));
        }
        report.push((name, worst));
    }
    for (name, worst) in &report {
        // Adding a 25-shot line writes its header, 25 rows and 25 stations.
        let bound = if *name == "add a line at a junction" {
            60
        } else {
            8
        };
        assert!(worst <= &bound, "{name}: {worst} changed lines");
    }
    for (name, worst) in &report {
        println!("{name}: {worst} changed lines");
    }
}
