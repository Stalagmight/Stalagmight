//! The connectivity searches over edges resolved once ([`Project::station_edges`])
//! against the per-station rescan they replaced: on random surveys - merges,
//! loop closures, a dangling merge, placement datums - every search answers as
//! the rescan does, for every excluded shot and every range.
#![expect(
    clippy::unwrap_used,
    reason = "test module: a failed lookup is a failed test"
)]

use std::collections::{HashSet, VecDeque};

use crate::{
    ClosureStrategy, GpsCoordinates, LineData, LineId, Project, ShotData, StationAnchor,
    StationKey, StationMerge, StationRef,
};

/// A tiny deterministic generator (xorshift), so a failing seed replays.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            usize::try_from(self.next() % n as u64).unwrap()
        }
    }
}

fn survey(seed: u64) -> Project {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut project = Project::new("Graph".to_string(), None);
    let lines = 4 + rng.below(6);
    for line in 0..lines {
        let shots = (0..=rng.below(5))
            .map(|i| ShotData {
                length: 2.0 + i as f64,
                azimuth_in: (line * 40 + i * 7) as f64 % 360.0,
                azimuth_out: (line * 40 + i * 7) as f64 % 360.0,
                depth_in: 5.0,
                depth_out: 5.5,
                ..Default::default()
            })
            .collect();
        project.add_line(LineData {
            name: Some(format!("L{line}")),
            color: "#123456".to_string(),
            shots,
            ..Default::default()
        });
    }
    let stations = all_stations(&project);
    let pick = |rng: &mut Rng| stations[rng.below(stations.len())];
    for _ in 0..rng.below(lines + 2) {
        let (a, b) = (pick(&mut rng), pick(&mut rng));
        let _ = project.merge_stations(a, b);
    }
    for _ in 0..rng.below(3) {
        let (a, b) = (pick(&mut rng), pick(&mut rng));
        let strategy = if rng.below(2) == 0 {
            ClosureStrategy::LeastSquaresWeighted
        } else {
            ClosureStrategy::Move
        };
        let _ = project.create_loop_closure(a, b, strategy);
    }
    // A merge whose far end names a shot no line holds: neither search may
    // reach anything through it.
    if rng.below(2) == 0 {
        let a = pick(&mut rng);
        if let Some(key) = project.key_for_ref(a) {
            project.station_merges.push(StationMerge {
                station_a: key,
                station_b: StationKey {
                    line_id: a.line_id,
                    anchor: StationAnchor::AfterShot("no-such-shot".to_string()),
                },
                merged_at: chrono::Utc::now(),
            });
        }
    }
    for _ in 0..rng.below(3) {
        let at = pick(&mut rng);
        let _ = project.set_zero_datum(
            at,
            GpsCoordinates {
                latitude: 20.0,
                longitude: -87.0,
                depth_reference: 0.0,
            },
        );
    }
    project
}

fn all_stations(project: &Project) -> Vec<StationRef> {
    project
        .lines
        .iter()
        .flat_map(|line| {
            (0..=line.shots.len()).map(move |i| StationRef::new(line.id, u32::try_from(i).unwrap()))
        })
        .collect()
}

/// The neighbour scan the searches used before: every merge (and closure)
/// re-resolved at every station visited.
fn rescan_neighbors(
    project: &Project,
    station: StationRef,
    skipped: &dyn Fn(LineId, usize) -> bool,
    include_closures: bool,
) -> Vec<StationRef> {
    let mut out = Vec::new();
    let Some(line) = project.get_line(station.line_id) else {
        return out;
    };
    let idx = station.station_index as usize;
    if idx > line.shots.len() {
        return out;
    }
    if idx > 0 && !skipped(station.line_id, idx - 1) {
        out.push(StationRef::new(station.line_id, station.station_index - 1));
    }
    if idx < line.shots.len() && !skipped(station.line_id, idx) {
        out.push(StationRef::new(station.line_id, station.station_index + 1));
    }
    for merge in &project.station_merges {
        if project.key_is_ref(&merge.station_a, station) {
            out.extend(project.resolve_key(&merge.station_b));
        } else if project.key_is_ref(&merge.station_b, station) {
            out.extend(project.resolve_key(&merge.station_a));
        }
    }
    if include_closures {
        for closure in &project.loop_closures {
            if project.key_is_ref(&closure.station_a, station) {
                out.extend(project.resolve_key(&closure.station_b));
            } else if project.key_is_ref(&closure.station_b, station) {
                out.extend(project.resolve_key(&closure.station_a));
            }
        }
    }
    out
}

/// Every station the rescan reaches from `from`, never passing a `removed` one.
fn rescan_reach(
    project: &Project,
    from: StationRef,
    skipped: &dyn Fn(LineId, usize) -> bool,
    include_closures: bool,
    removed: &HashSet<StationRef>,
) -> HashSet<StationRef> {
    let mut seen = HashSet::from([from]);
    let mut queue = VecDeque::from([from]);
    while let Some(current) = queue.pop_front() {
        for next in rescan_neighbors(project, current, skipped, include_closures) {
            if !removed.contains(&next) && seen.insert(next) {
                queue.push_back(next);
            }
        }
    }
    seen
}

/// What the random surveys held, so a test can show it exercised each kind of
/// edge and both answers.
#[derive(Default)]
struct Seen {
    merges: usize,
    closures: usize,
    datums: usize,
    connected: usize,
    apart: usize,
}

impl Seen {
    fn note(&mut self, project: &Project) {
        self.merges += project
            .station_merges
            .iter()
            .filter(|m| {
                project.resolve_key(&m.station_a).is_some()
                    && project.resolve_key(&m.station_b).is_some()
            })
            .count();
        self.closures += project.loop_closures.len();
        self.datums += project.placement_datum_refs().len();
    }
    fn answer(&mut self, connected: bool) {
        if connected {
            self.connected += 1;
        } else {
            self.apart += 1;
        }
    }
    fn assert_exercised(&self) {
        assert!(self.merges > 0, "premise: some survey holds a merge");
        assert!(
            self.closures > 0,
            "premise: some survey holds a loop closure"
        );
        assert!(
            self.datums > 0,
            "premise: some survey holds a placement datum"
        );
        assert!(
            self.connected > 0 && self.apart > 0,
            "premise: both answers came up ({} connected, {} apart)",
            self.connected,
            self.apart
        );
    }
}

#[test]
fn a_search_over_edges_resolved_once_answers_as_the_rescan_did() {
    let mut seen = Seen::default();
    for seed in 1..=12u64 {
        let project = survey(seed);
        seen.note(&project);
        let stations = all_stations(&project);
        let datums: HashSet<StationRef> = project.placement_datum_refs().into_iter().collect();
        let shots: Vec<Option<(LineId, usize)>> = std::iter::once(None)
            .chain(
                project
                    .lines
                    .iter()
                    .flat_map(|line| (0..line.shots.len()).map(move |i| Some((line.id, i)))),
            )
            .collect();
        let none = HashSet::new();
        for excluded in &shots {
            let skipped = |line: LineId, shot: usize| *excluded == Some((line, shot));
            for &a in &stations {
                for include_closures in [false, true] {
                    let reach = rescan_reach(&project, a, &skipped, include_closures, &none);
                    for &b in &stations {
                        let expected = a == b || reach.contains(&b);
                        seen.answer(expected);
                        assert_eq!(
                            project.are_stations_connected_excluding_shot(
                                a,
                                b,
                                *excluded,
                                include_closures
                            ),
                            expected,
                            "seed {seed}: {a:?} -> {b:?} without {excluded:?}, closures {include_closures}"
                        );
                    }
                }
                let reach = rescan_reach(&project, a, &skipped, true, &none);
                assert_eq!(
                    project.station_reaches_zero_datum_excluding_shot(a, *excluded),
                    reach.iter().any(|s| datums.contains(s)),
                    "seed {seed}: {a:?} reaching a datum without {excluded:?}"
                );
            }
        }
    }
    seen.assert_exercised();
}

#[test]
fn a_range_search_over_edges_resolved_once_answers_as_the_rescan_did() {
    let mut seen = Seen::default();
    for seed in 1..=6u64 {
        let project = survey(seed);
        seen.note(&project);
        let stations = all_stations(&project);
        for line in &project.lines {
            let n = line.shots.len();
            for start in 0..n {
                for end in start..n {
                    let skipped =
                        |l: LineId, shot: usize| l == line.id && (start..=end).contains(&shot);
                    // The interior stations of the range, as a range delete removes them.
                    let removed: HashSet<StationRef> = (start + 1..=end)
                        .map(|i| StationRef::new(line.id, u32::try_from(i).unwrap()))
                        .collect();
                    let datums: HashSet<StationRef> = project
                        .placement_datum_refs()
                        .into_iter()
                        .filter(|d| !removed.contains(d))
                        .collect();
                    for &a in &stations {
                        for include_closures in [false, true] {
                            let reach =
                                rescan_reach(&project, a, &skipped, include_closures, &removed);
                            for &b in &stations {
                                let expected = a == b || reach.contains(&b);
                                seen.answer(expected);
                                assert_eq!(
                                    project.stations_connected_excluding_range(
                                        a,
                                        b,
                                        line.id,
                                        start,
                                        end,
                                        &removed,
                                        include_closures
                                    ),
                                    expected,
                                    "seed {seed}: {a:?} -> {b:?} around {start}..={end} of {}",
                                    line.id
                                );
                            }
                        }
                        let reach = rescan_reach(&project, a, &skipped, true, &removed);
                        assert_eq!(
                            project.station_reaches_datum_excluding_range(
                                a, line.id, start, end, &removed
                            ),
                            datums.contains(&a) || reach.iter().any(|s| datums.contains(s)),
                            "seed {seed}: {a:?} reaching a datum around {start}..={end}"
                        );
                    }
                }
            }
        }
    }
    seen.assert_exercised();
}

/// The realign after a shot edit reads a station's raw x and y through
/// `compute_station_raw_xy`: the same numbers, to the bit, as the full raw
/// coordinate it replaced, which also walks the merge group for a depth.
#[test]
fn raw_xy_is_the_raw_coordinate_without_its_depth_to_the_bit() {
    for seed in 1..=60u64 {
        let mut project = survey(seed);
        // Offsets on lines and stations, so both terms are in play.
        let lines: Vec<LineId> = project.lines.iter().map(|l| l.id).collect();
        for (k, id) in lines.iter().enumerate() {
            project.adjust_line_offset(*id, 0.1 + k as f64 * 3.7, -2.3 + k as f64);
        }
        let stations = all_stations(&project);
        let mut held = 0;
        for (k, station) in stations.iter().enumerate().filter(|(k, _)| k % 3 == 0) {
            if let Some(key) = project.key_for_ref(*station) {
                let hand = (0.3 + k as f64 * 0.11, -0.7 + k as f64 * 0.05, 0.4);
                project.station_offsets.insert(key, hand);
                held += 1;
            }
        }
        assert!(held > 0, "premise: seed {seed} holds a station offset");
        // One past the end of a line: no station, on both paths.
        let last = project.get_line(lines[0]).unwrap().shots.len();
        let past = StationRef::new(lines[0], u32::try_from(last + 1).unwrap());
        assert!(project.compute_station_raw_xy(past).is_none());
        assert!(project.compute_station_raw_coords(past).is_none());
        for station in stations {
            let full = project.compute_station_raw_coords(station);
            let xy = project.compute_station_raw_xy(station);
            assert_eq!(
                full.map(|c| (c.x.to_bits(), c.y.to_bits())),
                xy.map(|(x, y)| (x.to_bits(), y.to_bits())),
                "seed {seed}: {station:?}"
            );
        }
    }
}
