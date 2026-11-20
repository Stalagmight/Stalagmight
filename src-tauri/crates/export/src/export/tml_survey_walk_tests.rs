//! The Ariane walk's line adjacency (I-import-export-2): the walk is linear
//! in the slots, so a hub of thousands of lines exports in moments, and it
//! enters every line exactly where the pairwise walk it replaced did. The
//! pairwise walk is kept here as the oracle. Synthetic surveys only.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use super::super::{AzimuthExport, TmlProfile};
use super::{build_rows, default_root_line, entrance_datums, entry_stations};
use crate::export::survey::test_helpers::{flat, gps_at, meta, project, Survey};
use crate::models::survey::{Project, Shot, StartAt, Topology};
use crate::models::{LineId, StationRef};

/// The walk before I-import-export-2, verbatim: every ordered pair of slots
/// at each station is an adjacency entry (`k * (k - 1)` at a hub of `k`
/// lines), sorted, then a breadth-first walk over it.
fn entry_stations_by_pairs(
    project: &Project,
    topology: &Topology,
    root: LineId,
    entrance_datums: &HashMap<LineId, usize>,
) -> HashMap<LineId, usize> {
    let mut adjacency: HashMap<LineId, Vec<(LineId, u32, u32)>> = HashMap::new();
    for &station in topology.stations() {
        let slots = topology.slots_of(station);
        for here in slots {
            for there in slots.iter().filter(|there| there.line_id != here.line_id) {
                adjacency.entry(here.line_id).or_default().push((
                    there.line_id,
                    there.station_index,
                    here.station_index,
                ));
            }
        }
    }
    for links in adjacency.values_mut() {
        links.sort_unstable();
        links.dedup();
    }
    let mut entries: HashMap<LineId, usize> = HashMap::new();
    let mut visited: HashSet<LineId> = HashSet::from([root]);
    let mut queue: VecDeque<LineId> = VecDeque::from([root]);
    loop {
        while let Some(parent) = queue.pop_front() {
            for &(child, index, _) in adjacency.get(&parent).into_iter().flatten() {
                if visited.insert(child) {
                    entries.insert(child, index as usize);
                    queue.push_back(child);
                }
            }
        }
        let candidate = |id: &LineId| !visited.contains(id) && adjacency.contains_key(id);
        let lines = || project.lines().iter().map(|line| line.id);
        let Some(next) = lines()
            .filter(candidate)
            .find(|id| entrance_datums.contains_key(id))
            .or_else(|| lines().find(candidate))
        else {
            break;
        };
        visited.insert(next);
        queue.push_back(next);
    }
    entries
}

/// A small deterministic generator (no dependency, same draws everywhere).
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % n.max(1)
    }
}

/// A random survey: one to three pieces, each a tree of lines branching off
/// stations of earlier lines (hubs come from repeated draws of one station;
/// zero-shot lines too), some stations of two pieces merged, some datums, and
/// the line list shuffled in some seeds (list order and id order differ).
fn random_survey(seed: u64) -> Project {
    let mut rng = Lcg(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x51ed);
    let mut survey = Survey::new("Walk");
    let mut pieces: Vec<Vec<LineId>> = Vec::new();
    for piece in 0..=rng.below(3) {
        let trunk = survey.line(
            meta(&format!("P{piece}"), "#ff0000"),
            (0..=rng.below(4))
                .map(|i| flat(2.0, (i * 37 % 360) as f64))
                .collect(),
        );
        let mut lines = vec![trunk];
        for i in 0..rng.below(9) {
            let from = lines[rng.below(lines.len())];
            let count = survey.project.line(from).map_or(1, |l| l.station_count());
            // Every third branch leaves the trunk's first station: a hub.
            let index = if i % 3 == 0 { 0 } else { rng.below(count) } as u32;
            let shots = (0..rng.below(4))
                .map(|j| flat(1.5, ((i * 53 + j * 17) % 360) as f64))
                .collect();
            let start = survey.station(from, index);
            let line = survey
                .project
                .add_line(
                    meta(&format!("P{piece}L{i}"), "#00ff00"),
                    StartAt::Station(start),
                    shots,
                )
                .unwrap_or_else(|e| panic!("seed {seed}: branch refused: {e}"));
            lines.push(line);
        }
        pieces.push(lines);
    }
    for _ in 0..rng.below(3) {
        if pieces.len() < 2 {
            break;
        }
        let (a, b) = (rng.below(pieces.len()), rng.below(pieces.len()));
        if a == b {
            continue;
        }
        let pick = |rng: &mut Lcg, piece: &[LineId], survey: &Survey| {
            let line = piece[rng.below(piece.len())];
            let count = survey.project.line(line).map_or(1, |l| l.station_count());
            survey.station(line, rng.below(count) as u32)
        };
        let (sa, sb) = (
            pick(&mut rng, &pieces[a], &survey),
            pick(&mut rng, &pieces[b], &survey),
        );
        // A refused merge (it would close a loop, or tear a placed piece)
        // leaves the survey as it was.
        let _ = survey.project.merge_stations(sa, sb);
    }
    for _ in 0..rng.below(3) {
        let piece = &pieces[rng.below(pieces.len())];
        let line = piece[rng.below(piece.len())];
        let count = survey.project.line(line).map_or(1, |l| l.station_count());
        let slot = StationRef::new(line, rng.below(count) as u32);
        let _ = survey
            .project
            .set_datum_at(slot, gps_at(rng.below(50) as f64, 3.0), None);
    }
    if rng.below(2) == 0 {
        let lines = survey.project.lines_mut();
        for i in (1..lines.len()).rev() {
            let j = (seed as usize + i * 7) % (i + 1);
            lines.swap(i, j);
        }
    }
    survey.project
}

/// On random surveys, from the walk's own root and from every line as a
/// root, the linear walk enters each line at the chain index the pairwise
/// walk did (so the rows, which follow from the entries, are unchanged).
#[test]
fn the_linear_walk_enters_every_line_where_the_pairwise_walk_did() {
    let mut compared = 0;
    let mut with_hub = 0;
    for seed in 0..300 {
        let project = random_survey(seed);
        if !project.validate().is_empty() {
            continue;
        }
        let topology = project.topology();
        let entrances = entrance_datums(&project, topology);
        let root = project
            .frame_datum()
            .and_then(|datum| super::datum_line(&project, topology, datum))
            .unwrap_or_else(|| default_root_line(&project, topology));
        let roots = std::iter::once(root).chain(project.lines().iter().map(|line| line.id));
        for root in roots {
            assert_eq!(
                entry_stations(&project, topology, root, &entrances),
                entry_stations_by_pairs(&project, topology, root, &entrances),
                "seed {seed}, root {root}"
            );
            compared += 1;
        }
        with_hub += usize::from(
            topology
                .stations()
                .iter()
                .any(|&s| topology.lines_of(s).len() > 2),
        );
    }
    assert!(compared > 1000, "only {compared} walks compared");
    assert!(with_hub > 100, "only {with_hub} surveys hold a hub");
}

/// A crafted file can hang tens of thousands of lines off one station
/// (MAX_LINE_ID 42,948 bounds them). The pairwise walk built one entry per
/// ordered pair of them, about 12 bytes each: 1.7 GB and tens of seconds at
/// 12,000 lines. The walk is linear: the whole Ariane row build of such a hub
/// takes moments, and every spoke hangs off the hub.
#[test]
fn a_hub_of_twelve_thousand_lines_exports_in_linear_time() {
    const SPOKES: u32 = 12_000;
    let mut project = project("Hub");
    let trunk = project
        .add_line(meta("Trunk", "#ff0000"), StartAt::New, vec![flat(1.0, 0.0)])
        .unwrap();
    let hub = project.station_at(StationRef::new(trunk, 0)).unwrap();
    project.set_placement(hub, 0.0, 0.0).unwrap();
    let first = project
        .add_line(
            meta("Spoke", "#00ff00"),
            StartAt::Station(hub),
            vec![flat(1.0, 1.0)],
        )
        .unwrap();
    let template = project.line(first).unwrap().clone();
    for i in 1..SPOKES {
        let end = project.mint_station(format!("S{i}")).unwrap();
        let mut line = template.clone();
        line.id = first + i;
        line.shots = vec![Shot::from_data(0, flat(1.0, f64::from(i % 360)), end)];
        project.lines_mut().push(line);
    }
    project.assign_session_shot_ids();
    assert!(project.validate().is_empty(), "setup: the hub is valid");

    let started = Instant::now();
    let decision = AzimuthExport::decide_from(
        project
            .lines()
            .iter()
            .map(|line| project.azimuth_reference_for(line)),
    );
    let (rows, _) = build_rows(&project, TmlProfile::User, decision);
    let took = started.elapsed();
    assert!(
        took < Duration::from_secs(10),
        "the rows of a {SPOKES}-line hub took {took:?}"
    );
    let off_hub = rows
        .iter()
        .filter(|row| row.shot_type == "REAL" && row.from_id == i64::from(hub))
        .count();
    // The trunk's shot and every spoke's.
    assert_eq!(off_hub, SPOKES as usize + 1);
}
