//! Cross-check against the legacy model: equivalent surveys built with
//! the frozen 0.6.0 `Project` (`stalagmight-legacy`: lines, merges, line offsets, hand
//! moves, datums) lay out every slot where `compute_station_coordinates` puts
//! it, within 1e-9 m. This proves the traversal reproduces today's layout.
//!
//! The conversion here is a test-only sketch (merge groups become one station,
//! station offsets become per-line hand moves, each component is anchored by
//! its first datum or a placement at its lowest station's old position); the
//! real migration is stream G.

use std::collections::BTreeMap;

use stalagmight_legacy::models as old;

use crate::{LineId, ShotData, StationId, StationRef};

use super::legacy_convert::{live, relive};
use super::test_helpers::{meta, project};
use super::{Project, Station, ZeroDatum};

type Old = stalagmight_legacy::LegacyProject;

/// A frozen-model GPS position (depth reference 0).
fn gps(latitude: f64, longitude: f64) -> old::GpsCoordinates {
    old::GpsCoordinates {
        latitude,
        longitude,
        depth_reference: 0.0,
    }
}

const TOL: f64 = 1e-9;

fn data(
    azimuth_in: f64,
    azimuth_out: f64,
    length: f64,
    depth_in: f64,
    depth_out: f64,
) -> old::ShotData {
    old::ShotData {
        azimuth_in,
        azimuth_out,
        length,
        depth_in,
        depth_out,
        ..old::ShotData::default()
    }
}

fn old_line(old: &mut Old, shots: Vec<old::ShotData>) -> LineId {
    old.add_line(old::LineData {
        shots,
        quality: 1.0,
        ..old::LineData::default()
    })
    .id
}

fn merge(old: &mut Old, a: (LineId, u32), b: (LineId, u32)) {
    let result = old.merge_stations(
        old::StationRef::new(a.0, a.1),
        old::StationRef::new(b.0, b.1),
    );
    assert_eq!(result, Ok(true), "legacy merge {a:?} onto {b:?}");
}

fn hand(old: &mut Old, slot: (LineId, u32), offset: (f64, f64, f64)) {
    old.station_offsets.insert(
        old::StationKey::from(old::StationRef::new(slot.0, slot.1)),
        offset,
    );
}

fn datum(old: &mut Old, slot: (LineId, u32), gps: old::GpsCoordinates) {
    let result = old.set_zero_datum(old::StationRef::new(slot.0, slot.1), gps);
    assert!(result.is_ok(), "legacy datum at {slot:?}: {result:?}");
}

/// Union-find over slots by the legacy merges.
fn merge_groups(old: &Old) -> BTreeMap<StationRef, StationRef> {
    let mut parent: BTreeMap<StationRef, StationRef> = BTreeMap::new();
    fn find(parent: &BTreeMap<StationRef, StationRef>, x: StationRef) -> StationRef {
        let mut root = x;
        while let Some(&p) = parent.get(&root) {
            if p == root {
                break;
            }
            root = p;
        }
        root
    }
    for line in &old.lines {
        for i in 0..=line.shots.len() {
            let slot = StationRef::new(line.id, i as u32);
            parent.insert(slot, slot);
        }
    }
    for m in &old.station_merges {
        let (Some(a), Some(b)) = (old.resolve_key(&m.station_a), old.resolve_key(&m.station_b))
        else {
            panic!("a legacy merge does not resolve");
        };
        let (ra, rb) = (find(&parent, live(a)), find(&parent, live(b)));
        if ra != rb {
            parent.insert(ra.max(rb), ra.min(rb));
        }
    }
    let slots: Vec<StationRef> = parent.keys().copied().collect();
    slots
        .into_iter()
        .map(|slot| (slot, find(&parent, slot)))
        .collect()
}

/// The legacy positions, by slot.
fn old_positions(old: &Old) -> BTreeMap<StationRef, [f64; 3]> {
    old.compute_station_coordinates()
        .into_iter()
        .map(|s| {
            (
                live(s.station_ref),
                [s.coordinates.x, s.coordinates.y, s.coordinates.z],
            )
        })
        .collect()
}

/// Convert a legacy project into the station-graph model (see module doc).
fn convert(old: &Old) -> Project {
    let groups = merge_groups(old);
    let before = old_positions(old);
    let mut p = project();
    // One station per merge group, ids in slot order of the group root.
    let mut station_of_root: BTreeMap<StationRef, StationId> = BTreeMap::new();
    for line in &old.lines {
        for i in 0..=line.shots.len() {
            let root = groups[&StationRef::new(line.id, i as u32)];
            station_of_root.entry(root).or_insert_with(|| {
                let id = p.mint_station_id().unwrap();
                p.insert_station(Station::new(id, format!("s{id}")))
                    .unwrap();
                id
            });
        }
    }
    let station = |slot: StationRef| station_of_root[&groups[&slot]];
    for line in &old.lines {
        let chain: Vec<StationId> = (0..=line.shots.len())
            .map(|i| station(StationRef::new(line.id, i as u32)))
            .collect();
        let shots: Vec<ShotData> = line
            .shots
            .iter()
            .map(|shot| relive(&shot.to_shot_data()))
            .collect();
        let id = p
            .add_line_on_stations(meta(&format!("L{}", line.id)), &chain, shots)
            .unwrap();
        assert_eq!(id, line.id, "line ids line up");
    }
    for (key, &(dx, dy, dz)) in &old.station_offsets {
        let slot = live(old.resolve_key(key).unwrap());
        p.line_mut(slot.line_id)
            .unwrap()
            .hand_moves
            .insert(station(slot), [dx, dy, dz]);
    }
    for old_datum in &old.zero_datums {
        let slot = live(old.resolve_key(&old_datum.station_ref).unwrap());
        let mut datum = ZeroDatum::new(station(slot), relive(&old_datum.gps));
        datum.via = Some(slot.line_id);
        p.push_datum(datum);
    }
    // Placement datums hold their station where the legacy model shows it.
    let placement_datums = p.placement_datums();
    for &d in placement_datums.iter().flatten() {
        let slot = live(old.resolve_key(&old.zero_datums[d].station_ref).unwrap());
        let [x, y, _] = before[&slot];
        p.datums[d].correction = p.correction_to_hold(d, [x, y]);
    }
    // Datum-less components: a placement at the lowest station.
    let topology = p.topology_arc();
    for (c, component) in topology.components().iter().enumerate() {
        if placement_datums[c].is_some() {
            continue;
        }
        let lowest = component.stations[0];
        let slot = topology.first_slot(lowest).unwrap();
        let [x, y, _] = before[&slot];
        p.set_placement(lowest, x, y).unwrap();
    }
    assert_eq!(p.validate(), Vec::new());
    p
}

/// Every slot of the converted model sits where the legacy model shows it.
fn assert_same_layout(old: &Old) -> Project {
    let p = convert(old);
    let expected = old_positions(old);
    let positions = p.raw_positions();
    let slots = positions.slots(&p);
    assert_eq!(slots.len(), expected.len());
    for slot in slots {
        let [x, y, z] = expected[&slot.slot];
        let gap = ((slot.x - x).powi(2) + (slot.y - y).powi(2)).sqrt();
        assert!(gap <= TOL, "slot {}: xy off by {gap}", slot.slot);
        assert!(
            (slot.z - z).abs() <= TOL,
            "slot {}: z {} vs legacy {z}",
            slot.slot,
            slot.z
        );
    }
    p
}

#[test]
fn merged_junctions_a_reversed_line_hand_moves_and_a_free_piece() {
    let mut old = Old::new("legacy".to_string(), None);
    let l0 = old_line(
        &mut old,
        vec![
            data(80.0, 82.0, 10.0, 1.0, 2.5),
            data(95.0, 93.5, 12.5, 2.5, 2.0),
            data(120.0, 121.0, 8.0, 2.0, 4.0),
            data(359.0, 1.5, 6.0, 4.0, 4.5),
            data(45.0, 47.0, 9.0, 4.5, 3.0),
        ],
    );
    let l1 = old_line(
        &mut old,
        vec![
            data(10.0, 12.0, 7.0, 2.1, 3.0),
            data(20.0, 18.0, 5.5, 3.0, 3.2),
            data(300.0, 302.0, 4.0, 3.2, 2.2),
        ],
    );
    let l2 = old_line(
        &mut old,
        vec![
            data(200.0, 201.0, 6.0, 0.5, 1.0),
            data(210.0, 209.0, 6.5, 1.0, 1.5),
            data(190.0, 192.0, 3.0, 1.5, 2.0),
            data(170.0, 171.0, 4.5, 2.0, 2.4),
        ],
    );
    let l3 = old_line(
        &mut old,
        vec![
            data(270.0, 271.0, 11.0, 3.0, 5.0),
            data(260.0, 262.0, 3.0, 5.0, 5.5),
        ],
    );
    let l4 = old_line(
        &mut old,
        vec![
            data(0.0, 0.5, 20.0, 10.0, 11.0),
            data(90.0, 91.0, 5.0, 11.0, 12.0),
            data(180.0, 181.0, 5.0, 12.0, 11.0),
        ],
    );
    let l5 = old_line(
        &mut old,
        vec![
            data(135.0, 136.0, 3.0, 11.5, 11.0),
            data(225.0, 226.0, 3.0, 11.0, 12.5),
        ],
    );
    // Hand moves first (a merge aligns shown positions), one on a junction.
    hand(&mut old, (l1, 0), (0.3, -0.2, 0.1));
    hand(&mut old, (l2, 2), (0.5, 0.5, -0.4));
    // L1 starts on L0:2; L2 ENDS on L1:3 (a line whose junction is its last
    // station, as a reversed line has it); L3 hangs off L0's end.
    merge(&mut old, (l1, 0), (l0, 2));
    merge(&mut old, (l2, 4), (l1, 3));
    merge(&mut old, (l3, 0), (l0, 5));
    // A free piece: L4 placed by hand, L5 merged at its middle onto L4:2.
    old.line_offsets.insert(l4, (37.5, -12.25));
    merge(&mut old, (l5, 1), (l4, 2));
    let p = assert_same_layout(&old);
    assert_eq!(p.topology().components().len(), 2);
    assert!(p
        .topology()
        .is_junction(p.station_at(StationRef::new(l1, 3)).unwrap()));
}

#[test]
fn datum_pieces_place_on_their_gps_with_no_correction() {
    let mut old = Old::new("legacy".to_string(), None);
    let a = old_line(
        &mut old,
        vec![
            data(90.0, 90.0, 10.0, 0.0, 1.0),
            data(80.0, 81.0, 15.0, 1.0, 1.5),
            data(70.0, 72.0, 12.0, 1.5, 2.0),
        ],
    );
    let a2 = old_line(
        &mut old,
        vec![
            data(0.0, 1.0, 8.0, 1.5, 2.5),
            data(10.0, 11.0, 8.0, 2.5, 2.0),
        ],
    );
    let b = old_line(
        &mut old,
        vec![
            data(180.0, 181.0, 9.0, 3.0, 3.5),
            data(170.0, 171.0, 9.0, 3.5, 4.0),
        ],
    );
    let b2 = old_line(&mut old, vec![data(250.0, 251.0, 7.0, 4.0, 4.5)]);
    let free = old_line(&mut old, vec![data(45.0, 44.0, 6.0, 0.0, 0.0)]);
    merge(&mut old, (a2, 0), (a, 3));
    datum(&mut old, (a, 0), gps(20.5, -87.4));
    datum(&mut old, (b, 1), gps(20.5012, -87.3985));
    merge(&mut old, (b2, 0), (b, 2));
    // A GPS fix in A's piece, far off where its station sits: moves nothing.
    datum(&mut old, (a2, 2), gps(20.5003, -87.3999));
    old.line_offsets.insert(free, (-40.0, 25.0));
    let p = assert_same_layout(&old);
    // The frame and B's datum need no correction: the legacy model put them
    // exactly on their GPS, as the new solver derives.
    assert_eq!(p.datums.len(), 3);
    assert!(
        p.datums.iter().all(|d| d.correction.is_none()),
        "{:?}",
        p.datums
    );
    assert_eq!(p.frame_datum(), Some(0));
}

#[test]
fn different_hand_moves_on_two_merged_members_become_per_line_hand_moves() {
    let mut old = Old::new("legacy".to_string(), None);
    let l0 = old_line(
        &mut old,
        vec![
            data(90.0, 90.0, 10.0, 4.0, 4.0),
            data(90.0, 90.0, 10.0, 4.0, 4.0),
        ],
    );
    let l1 = old_line(&mut old, vec![data(0.0, 0.0, 5.0, 7.0, 7.0)]);
    hand(&mut old, (l0, 1), (1.0, 0.0, 0.5));
    hand(&mut old, (l1, 0), (0.0, 2.0, -1.0));
    merge(&mut old, (l1, 0), (l0, 1));
    let p = assert_same_layout(&old);
    let j = p.station_at(StationRef::new(l0, 1)).unwrap();
    assert_eq!(p.line(l0).unwrap().hand(j), [1.0, 0.0, 0.5]);
    assert_eq!(p.line(l1).unwrap().hand(j), [0.0, 2.0, -1.0]);
}

/// A small deterministic generator (an LCG).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn unit(&mut self) -> f64 {
        self.next() as f64 / f64::from(u32::MAX)
    }
}

/// A random legacy survey: lines merged at random slots onto lines of other
/// merge components (the legacy model refuses a connected pair), hand moves
/// set before each merge, datums (placements and fixes), free pieces moved.
fn random_legacy(seed: u64) -> Old {
    let mut rng = Lcg(seed);
    let mut old = Old::new("legacy".to_string(), None);
    for _ in 0..14 {
        let count = 1 + rng.below(5);
        let shots = (0..count)
            .map(|_| {
                let depth_in = rng.unit() * 30.0;
                let azimuth = rng.unit() * 360.0;
                data(
                    azimuth,
                    azimuth + (rng.unit() - 0.5) * 3.0,
                    1.0 + rng.unit() * 15.0,
                    depth_in,
                    depth_in + (rng.unit() - 0.5) * 3.0,
                )
            })
            .collect();
        let line = old_line(&mut old, shots);
        let len = old.lines.last().map_or(0, |l| l.shots.len());
        if rng.below(3) == 0 {
            let slot = (line, rng.below(len + 1) as u32);
            hand(
                &mut old,
                slot,
                (rng.unit() - 0.5, rng.unit() - 0.5, rng.unit() - 0.5),
            );
        }
        if old.lines.len() > 1 && rng.below(4) != 0 {
            let target = old.lines[rng.below(old.lines.len() - 1)].id;
            let target_len = old.get_line(target).map_or(0, |l| l.shots.len());
            let a = old::StationRef::new(line, rng.below(len + 1) as u32);
            let b = old::StationRef::new(target, rng.below(target_len + 1) as u32);
            // Refused when already connected: then the line stays free.
            let _ = old.merge_stations(a, b);
        }
    }
    let lines: Vec<(LineId, usize)> = old.lines.iter().map(|l| (l.id, l.shots.len())).collect();
    for _ in 0..4 {
        let (line, len) = lines[rng.below(lines.len())];
        let slot = (line, rng.below(len + 1) as u32);
        let lat = 20.0 + rng.unit() * 0.02;
        let lon = -87.0 + rng.unit() * 0.02;
        let _ = old.set_zero_datum(old::StationRef::new(slot.0, slot.1), gps(lat, lon));
    }
    // Move unmerged lines by hand (a merged line's offset would tear its
    // junctions, which no legacy command does).
    let merged: Vec<LineId> = merged_lines(&old);
    for (line, _) in lines {
        if rng.below(3) == 0 && !merged.contains(&line) {
            let (x, y) = old.line_offsets.get(&line).copied().unwrap_or((0.0, 0.0));
            old.line_offsets.insert(
                line,
                (
                    x + rng.unit() * 200.0 - 100.0,
                    y + rng.unit() * 200.0 - 100.0,
                ),
            );
        }
    }
    old
}

fn merged_lines(old: &Old) -> Vec<LineId> {
    let mut lines = Vec::new();
    for m in &old.station_merges {
        for key in [&m.station_a, &m.station_b] {
            if let Some(slot) = old.resolve_key(key) {
                lines.push(slot.line_id);
            }
        }
    }
    lines
}

#[test]
fn random_legacy_surveys_lay_out_identically() {
    let mut junctions = 0;
    let mut datums = 0;
    for seed in 0..40 {
        let old = random_legacy(seed);
        assert!(merges_hold(&old), "seed {seed}: the legacy junctions hold");
        let p = assert_same_layout(&old);
        junctions += p
            .topology()
            .stations()
            .iter()
            .filter(|&&s| p.topology().is_junction(s))
            .count();
        datums += p.datums.len();
    }
    assert!(
        junctions > 20,
        "the generator must exercise junctions ({junctions})"
    );
    assert!(datums > 20, "the generator must exercise datums ({datums})");
}

/// Whether every legacy merge joins two slots shown within 1e-9 m.
fn merges_hold(old: &Old) -> bool {
    let positions = old_positions(old);
    for m in &old.station_merges {
        let (Some(a), Some(b)) = (old.resolve_key(&m.station_a), old.resolve_key(&m.station_b))
        else {
            return false;
        };
        let ([ax, ay, _], [bx, by, _]) = (positions[&live(a)], positions[&live(b)]);
        if ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt() > TOL {
            return false;
        }
    }
    true
}
