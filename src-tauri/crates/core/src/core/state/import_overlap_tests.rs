//! The doubly-anchored guard's line graph (I-import-export-2): one linear
//! pass per updated line gives the lines the pairwise walk it replaced
//! blocked, and a hub of thousands of lines takes moments. The pairwise
//! guard is kept here as the oracle. Synthetic surveys only.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

use super::{apply_overlap_updates, doubly_anchored_lines};
use crate::models::survey::{ClosureStrategy, LineMeta, Project, Shot, StartAt, ZeroDatum};
use crate::models::{
    ExistingShotUpdate, GpsCoordinates, LineId, ShotData, ShotUpdates, StationId, StationRef,
};

/// The guard before I-import-export-2, verbatim: line adjacency as a set of
/// every pair of lines at each junction and across each aligned snap, and a
/// breadth-first walk per partner.
fn doubly_anchored_lines_by_pairs(
    project: &Project,
    by_line: &BTreeMap<LineId, Vec<&ExistingShotUpdate>>,
) -> BTreeSet<LineId> {
    let positions = project.raw_positions();
    let aligned = project.closure_alignment(&positions);
    let topology = project.topology();
    let placement_stations: BTreeSet<StationId> = project
        .placement_datums()
        .into_iter()
        .flatten()
        .filter_map(|i| project.datums.get(i).map(|d| d.station))
        .collect();
    let datum_lines: BTreeSet<LineId> = placement_stations
        .iter()
        .flat_map(|&s| topology.lines_of(s))
        .collect();
    let mut adjacent: BTreeMap<LineId, BTreeSet<LineId>> = BTreeMap::new();
    let mut link = |a: &[LineId], b: &[LineId]| {
        for &x in a {
            for &y in b {
                if x != y {
                    adjacent.entry(x).or_default().insert(y);
                    adjacent.entry(y).or_default().insert(x);
                }
            }
        }
    };
    for &station in topology.stations() {
        if topology.is_junction(station) {
            let lines = topology.lines_of(station);
            link(&lines, &lines);
        }
    }
    for (closure, &is_aligned) in project.closures.iter().zip(&aligned) {
        if is_aligned {
            link(&topology.lines_of(closure.a), &topology.lines_of(closure.b));
        }
    }
    let reaches_datum = |start: LineId, excluded: LineId| {
        let mut seen = BTreeSet::new();
        let mut queue = VecDeque::from([start]);
        while let Some(line) = queue.pop_front() {
            if line == excluded || !seen.insert(line) {
                continue;
            }
            if datum_lines.contains(&line) {
                return true;
            }
            if let Some(next) = adjacent.get(&line) {
                queue.extend(next.iter().copied().filter(|l| !seen.contains(l)));
            }
        }
        false
    };
    let mut blocked = BTreeSet::new();
    for (&line_id, line_updates) in by_line {
        let (Some(line), Some(chain)) = (project.line(line_id), topology.chain(line_id)) else {
            continue;
        };
        let modified: Vec<usize> = line_updates
            .iter()
            .filter_map(|u| line.shots.iter().position(|s| s.id == u.shot_id))
            .collect();
        let (Some(&min_shot), Some(&max_shot)) = (modified.iter().min(), modified.iter().max())
        else {
            continue;
        };
        let partner_reaches = |station: StationId| {
            topology
                .lines_of(station)
                .into_iter()
                .any(|partner| partner != line_id && reaches_datum(partner, line_id))
        };
        let mut anchors: Vec<usize> = Vec::new();
        for (index, &station) in chain.iter().enumerate() {
            if placement_stations.contains(&station)
                || (topology.is_junction(station) && partner_reaches(station))
            {
                anchors.push(index);
            }
        }
        for (closure, &is_aligned) in project.closures.iter().zip(&aligned) {
            if !is_aligned {
                continue;
            }
            for (end, other) in [(closure.a, closure.b), (closure.b, closure.a)] {
                if let Some(index) = chain.iter().position(|&s| s == end) {
                    if partner_reaches(other) {
                        anchors.push(index);
                    }
                }
            }
        }
        let front = anchors.iter().any(|&i| i <= min_shot);
        let back = anchors.iter().any(|&i| i > max_shot);
        if front && back {
            blocked.insert(line_id);
        }
    }
    blocked
}

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

fn meta(name: &str) -> LineMeta {
    LineMeta {
        name: Some(name.to_string()),
        color: "#ff0000".to_string(),
        ..LineMeta::default()
    }
}

/// A level 10 m shot on a grid azimuth, so stations of different lines
/// often coincide and a snap between them is aligned.
fn grid(quarter: usize) -> ShotData {
    let azimuth = (quarter % 4) as f64 * 90.0;
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length: 10.0,
        ..ShotData::default()
    }
}

fn gps(east_m: f64) -> GpsCoordinates {
    GpsCoordinates {
        latitude: 20.5,
        longitude: -87.25 + east_m / 104_000.0,
        depth_reference: 0.0,
    }
}

/// A random survey: one to three pieces, each a tree of grid lines branching
/// off earlier stations (hubs from repeated draws of one station), held by a
/// datum (somewhere on the piece, every datum on one GPS position) or a
/// placement, then `Move` snaps between stations that coincide (aligned),
/// within a piece or across two datum pieces, and a few random ones.
fn random_survey(seed: u64) -> Project {
    let mut rng = Lcg(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x0a1d);
    let mut p = Project::new("Guard", None);
    // Per station: its piece, and whether that piece holds a datum.
    let mut piece_of: BTreeMap<StationId, (usize, bool)> = BTreeMap::new();
    for piece in 0..=rng.below(3) {
        let shots = (0..=rng.below(4)).map(|_| grid(rng.below(4))).collect();
        let Ok(trunk) = p.add_line(meta(&format!("P{piece}")), StartAt::New, shots) else {
            continue;
        };
        let mut stations: Vec<StationId> = p
            .line(trunk)
            .map_or_else(Vec::new, |l| l.stations().collect());
        for i in 0..rng.below(8) {
            let start = if i % 3 == 0 {
                stations[0]
            } else {
                stations[rng.below(stations.len())]
            };
            let shots = (0..=rng.below(3)).map(|_| grid(rng.below(4))).collect();
            if let Ok(line) = p.add_line(
                meta(&format!("P{piece}L{i}")),
                StartAt::Station(start),
                shots,
            ) {
                stations.extend(
                    p.line(line)
                        .map_or_else(Vec::new, |l| l.stations().skip(1).collect::<Vec<_>>()),
                );
            }
        }
        let datum = rng.below(4) != 0;
        if datum {
            let at = stations[rng.below(stations.len())];
            p.push_datum(ZeroDatum::new(at, gps(0.0)));
        } else {
            let _ = p.set_placement(stations[0], 0.0, 0.0);
        }
        piece_of.extend(stations.iter().map(|&s| (s, (piece, datum))));
    }
    let positions = p.raw_positions();
    let mut at: BTreeMap<(i64, i64), Vec<StationId>> = BTreeMap::new();
    for &station in piece_of.keys() {
        if let Some([x, y]) = positions.xy(station) {
            at.entry(((x * 100.0).round() as i64, (y * 100.0).round() as i64))
                .or_default()
                .push(station);
        }
    }
    let joinable = |a: StationId, b: StationId| match (piece_of.get(&a), piece_of.get(&b)) {
        (Some(&(pa, da)), Some(&(pb, db))) => a != b && (pa == pb || (da && db)),
        _ => false,
    };
    for group in at.values() {
        for pair in group.windows(2) {
            if joinable(pair[0], pair[1]) && rng.below(2) == 0 {
                p.push_closure(pair[0], None, pair[1], None, ClosureStrategy::Move);
            }
        }
    }
    let all: Vec<StationId> = piece_of.keys().copied().collect();
    for _ in 0..rng.below(3) {
        let (a, b) = (all[rng.below(all.len())], all[rng.below(all.len())]);
        if joinable(a, b) {
            p.push_closure(a, None, b, None, ClosureStrategy::Move);
        }
    }
    p
}

/// On random surveys, every updated line is blocked by the linear pass
/// exactly when the pairwise guard blocked it.
#[test]
fn the_linear_guard_blocks_what_the_pairwise_guard_did() {
    let (mut compared, mut blocked, mut aligned_snaps) = (0, 0, 0);
    for seed in 0..400 {
        let p = random_survey(seed);
        if !p.validate().is_empty() {
            continue;
        }
        let mut rng = Lcg(seed ^ 0xdead);
        let mut updates: Vec<ExistingShotUpdate> = Vec::new();
        for line in p.lines() {
            if line.shots.is_empty() || rng.below(2) == 0 {
                continue;
            }
            for _ in 0..=rng.below(2) {
                updates.push(ExistingShotUpdate {
                    line_id: line.id,
                    shot_id: line.shots[rng.below(line.shots.len())].id,
                    updates: ShotUpdates {
                        length: Some(11.0),
                        ..ShotUpdates::default()
                    },
                });
            }
        }
        let mut by_line: BTreeMap<LineId, Vec<&ExistingShotUpdate>> = BTreeMap::new();
        for update in &updates {
            by_line.entry(update.line_id).or_default().push(update);
        }
        let expected = doubly_anchored_lines_by_pairs(&p, &by_line);
        assert_eq!(doubly_anchored_lines(&p, &by_line), expected, "seed {seed}");
        compared += by_line.len();
        blocked += expected.len();
        aligned_snaps += p
            .closure_alignment(&p.raw_positions())
            .into_iter()
            .filter(|&a| a)
            .count();
    }
    eprintln!("{compared} updated lines, {blocked} blocked, {aligned_snaps} aligned snaps");
    assert!(compared > 500, "only {compared} updated lines compared");
    assert!(
        blocked > 50,
        "only {blocked} blocked lines: the guard is barely exercised"
    );
    assert!(aligned_snaps > 50, "only {aligned_snaps} aligned snaps");
}

/// A hub of 12,000 lines off one station (a crafted or degenerate survey;
/// MAX_LINE_ID bounds them at 42,948), the datum at the trunk's far end: an
/// update of one spoke's shot is guarded in moments. The pairwise guard
/// inserted every ordered pair of the hub's lines into a set first (144
/// million inserts here). The spoke is anchored at the hub only, so its
/// update lands.
#[test]
fn an_overlap_update_on_a_hub_of_twelve_thousand_lines_is_guarded_in_linear_time() {
    const SPOKES: u32 = 12_000;
    let mut p = Project::new("Hub", None);
    let trunk = p
        .add_line(meta("Trunk"), StartAt::New, vec![grid(0)])
        .unwrap_or_else(|e| panic!("{e}"));
    let hub = p
        .station_at(StationRef::new(trunk, 0))
        .unwrap_or_else(|| panic!("no hub"));
    let far = p
        .station_at(StationRef::new(trunk, 1))
        .unwrap_or_else(|| panic!("no far end"));
    p.push_datum(ZeroDatum::new(far, gps(0.0)));
    let first = p
        .add_line(meta("Spoke"), StartAt::Station(hub), vec![grid(1)])
        .unwrap_or_else(|e| panic!("{e}"));
    let template = p.line(first).cloned().unwrap_or_else(|| panic!("no spoke"));
    for i in 1..SPOKES {
        let end = p
            .mint_station(format!("S{i}"))
            .unwrap_or_else(|e| panic!("{e}"));
        let mut line = template.clone();
        line.id = first + i;
        line.shots = vec![Shot::from_data(0, grid(i as usize), end)];
        p.lines_mut().push(line);
    }
    p.assign_session_shot_ids();
    assert!(p.validate().is_empty(), "setup: the hub is valid");
    let spoke = first + SPOKES / 2;
    let shot_id = p.line(spoke).map(|l| l.shots[0].id).unwrap_or_default();
    let update = ExistingShotUpdate {
        line_id: spoke,
        shot_id,
        updates: ShotUpdates {
            length: Some(12.0),
            ..ShotUpdates::default()
        },
    };

    let started = Instant::now();
    let skipped = apply_overlap_updates(&mut p, &[update]);
    let took = started.elapsed();
    assert!(
        took < Duration::from_secs(10),
        "the guard on a {SPOKES}-line hub took {took:?}"
    );
    assert_eq!(skipped, 0, "anchored at the hub only");
    assert_eq!(p.line(spoke).map(|l| l.shots[0].length), Some(12.0));
}
