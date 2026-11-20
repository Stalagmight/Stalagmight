//! Test support for the M3 cross-checks: convert a survey built with the
//! frozen 0.6.0 model (`stalagmight_legacy::models::Project`) into the
//! station-graph model, and read the legacy derived views per slot.
//!
//! The conversion (a test sketch; the real migration is stream G):
//! - each legacy merge group becomes one station (ids minted in slot order of
//!   the group root, names `s<id>`);
//! - lines keep their ids, list order, metadata and shots; station offsets
//!   become per-line hand moves;
//! - datums keep list order, accuracy and entrance, with `via` = the legacy
//!   slot's line; each placement datum gets the correction that holds its
//!   station where the legacy model shows it;
//! - closures keep id, list order and strategy, with `via` hints; a legacy
//!   aligned non-`Move` closure becomes `Tie` (plan 7.2.4);
//! - each datum-less component gets its `Placement` on the legacy adjustment
//!   gauge station at its legacy raw position (so the least-squares gauge is
//!   the same station, plan 3.7).
//!
//! Fully synthetic data only.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::Serialize;
use stalagmight_legacy::models as old;

use crate::config::ClosureAdjustmentConfig;
use crate::{LineId, ShotData, StationId, StationRef};

use super::{ClosureStrategy, LineMeta, LoopClosure, Project, Station, ZeroDatum};

pub(super) type Old = stalagmight_legacy::LegacyProject;

/// A frozen-model value as the live type of the same name: both serialize
/// identically (the frozen copy only dropped the TS derives).
pub(super) fn relive<T: Serialize, U: DeserializeOwned>(value: &T) -> U {
    match serde_json::to_value(value).and_then(serde_json::from_value) {
        Ok(live) => live,
        Err(error) => panic!("frozen value does not read as the live type: {error}"),
    }
}

/// A frozen-model slot as the live slot.
pub(super) fn live(slot: old::StationRef) -> StationRef {
    StationRef::new(slot.line_id, slot.station_index)
}

/// A live slot as the frozen-model slot.
pub(super) fn frozen(slot: StationRef) -> old::StationRef {
    old::StationRef::new(slot.line_id, slot.station_index)
}

/// A converted project and the slot -> station map.
pub(super) struct Converted {
    pub(super) project: Project,
    pub(super) station_of: BTreeMap<StationRef, StationId>,
}

impl Converted {
    /// The new station of a legacy slot.
    pub(super) fn station(&self, slot: StationRef) -> StationId {
        match self.station_of.get(&slot) {
            Some(&station) => station,
            None => panic!("legacy slot {slot} has no station"),
        }
    }
}

/// Union-find over slots by the legacy merges: slot -> group root (the lowest
/// slot of the group).
pub(super) fn merge_groups(old: &Old) -> BTreeMap<StationRef, StationRef> {
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
    let mut parent: BTreeMap<StationRef, StationRef> = BTreeMap::new();
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

/// The legacy raw positions, by slot.
pub(super) fn old_raw(old: &Old) -> BTreeMap<StationRef, [f64; 3]> {
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

/// The legacy Closures Computed View, by slot, and the failing components'
/// anchors.
pub(super) fn old_adjusted(
    old: &Old,
    config: &ClosureAdjustmentConfig,
) -> (BTreeMap<StationRef, [f64; 3]>, Vec<StationRef>) {
    let adjusted = old.compute_adjusted_stations(&relive(config));
    let positions = adjusted
        .stations
        .into_iter()
        .map(|s| {
            (
                live(s.station_ref),
                [s.coordinates.x, s.coordinates.y, s.coordinates.z],
            )
        })
        .collect();
    let failures = adjusted.failures.iter().map(|f| live(f.anchor)).collect();
    (positions, failures)
}

/// Convert a legacy project (see the module doc).
pub(super) fn convert(old: &Old) -> Converted {
    let groups = merge_groups(old);
    let before = old_raw(old);
    let mut p = super::test_helpers::project();
    p.settings.default_azimuth_reference = old.default_azimuth_reference.as_ref().map(relive);
    let mut station_of_root: BTreeMap<StationRef, StationId> = BTreeMap::new();
    for line in &old.lines {
        for i in 0..=line.shots.len() {
            let root = groups[&StationRef::new(line.id, i as u32)];
            station_of_root.entry(root).or_insert_with(|| {
                let id = p.mint_station_id().unwrap_or_default();
                if let Err(error) = p.insert_station(Station::new(id, format!("s{id}"))) {
                    panic!("station {id}: {error}");
                }
                id
            });
        }
    }
    let station_of: BTreeMap<StationRef, StationId> = groups
        .iter()
        .map(|(&slot, root)| (slot, station_of_root[root]))
        .collect();
    let station = |slot: StationRef| station_of[&slot];

    for line in &old.lines {
        let chain: Vec<StationId> = (0..=line.shots.len())
            .map(|i| station(StationRef::new(line.id, i as u32)))
            .collect();
        let shots: Vec<ShotData> = line
            .shots
            .iter()
            .map(|shot| relive(&shot.to_shot_data()))
            .collect();
        let meta = LineMeta {
            name: line.name.clone(),
            color: line.color.clone(),
            direction: relive(&line.direction),
            quality: line.quality,
            azimuth_reference: line.azimuth_reference.as_ref().map(relive),
            ..LineMeta::default()
        };
        // Legacy test surveys number lines 0, 1, 2, ... in list order, as the
        // new model derives them.
        match p.add_line_on_stations(meta, &chain, shots) {
            Ok(id) => assert_eq!(id, line.id, "line ids line up"),
            Err(error) => panic!("line {}: {error}", line.id),
        }
    }
    for (key, &(dx, dy, dz)) in &old.station_offsets {
        let Some(slot) = old.resolve_key(key).map(live) else {
            continue;
        };
        if let Some(line) = p.line_mut(slot.line_id) {
            line.hand_moves.insert(station(slot), [dx, dy, dz]);
        }
    }
    for old_datum in &old.zero_datums {
        let Some(slot) = old.resolve_key(&old_datum.station_ref).map(live) else {
            continue;
        };
        let mut datum = ZeroDatum::new(station(slot), relive(&old_datum.gps));
        datum.via = Some(slot.line_id);
        datum.accuracy_m = old_datum.accuracy_m;
        datum.entrance = old_datum.entrance;
        p.push_datum(datum);
    }
    let placement_datums = p.placement_datums();
    for &d in placement_datums.iter().flatten() {
        let Some(slot) = p.datum_slot(&p.datums[d]) else {
            continue;
        };
        let [x, y, _] = before[&slot];
        p.datums[d].correction = p.correction_to_hold(d, [x, y]);
    }
    let mut max_closure_id = 0;
    for c in &old.loop_closures {
        let (Some(a), Some(b)) = (
            old.resolve_key(&c.station_a).map(live),
            old.resolve_key(&c.station_b).map(live),
        ) else {
            continue;
        };
        let mut strategy = ClosureStrategy::from(relive::<_, crate::ClosureStrategy>(&c.strategy));
        if c.is_aligned && strategy != ClosureStrategy::Move {
            strategy = ClosureStrategy::Tie;
        }
        p.closures.push(LoopClosure {
            id: c.id,
            a: station(a),
            a_via: Some(a.line_id),
            b: station(b),
            b_via: Some(b.line_id),
            strategy,
        });
        max_closure_id = max_closure_id.max(c.id);
    }
    p.counters.next_closure_id = p.counters.next_closure_id.max(max_closure_id + 1);
    // Datum-less components: a placement on the legacy gauge station.
    let topology = p.topology_arc();
    for (c, component) in topology.components().iter().enumerate() {
        if placement_datums[c].is_some() {
            continue;
        }
        let Some(&line) = component.lines.first() else {
            continue;
        };
        let gauge = old
            .anchor_for_lines(&old.get_connected_line_ids(line))
            .map(|(gauge, _)| live(gauge))
            .unwrap_or_else(|| panic!("line {line}: no legacy gauge"));
        let [x, y, _] = before[&gauge];
        if let Err(error) = p.set_placement(station(gauge), x, y) {
            panic!("placement: {error}");
        }
    }
    let violations = p.validate();
    assert!(
        violations.is_empty(),
        "converted model invalid: {violations:?}"
    );
    Converted {
        project: p,
        station_of,
    }
}

/// A small deterministic generator (an LCG), as in M0's cross-check.
pub(super) struct Lcg(pub(super) u64);

impl Lcg {
    pub(super) fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    pub(super) fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    pub(super) fn unit(&mut self) -> f64 {
        self.next() as f64 / f64::from(u32::MAX)
    }
}

/// A legacy shot.
pub(super) fn data(
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

/// Add a legacy line, returning its id.
pub(super) fn old_line(old: &mut Old, shots: Vec<old::ShotData>) -> LineId {
    old.add_line(old::LineData {
        shots,
        quality: 1.0,
        ..old::LineData::default()
    })
    .id
}

/// A random legacy shot list (depths continuous along the line).
pub(super) fn random_shots(rng: &mut Lcg, count: usize) -> Vec<old::ShotData> {
    let mut depth = rng.unit() * 20.0;
    (0..count)
        .map(|_| {
            let azimuth = rng.unit() * 360.0;
            let next = (depth + (rng.unit() - 0.5) * 4.0).max(0.0);
            let shot = data(
                azimuth,
                azimuth + (rng.unit() - 0.5) * 2.0,
                2.0 + rng.unit() * 12.0,
                depth,
                next,
            );
            depth = next;
            shot
        })
        .collect()
}

/// What [`random_legacy_survey`] puts in.
#[derive(Debug, Clone, Copy)]
pub(super) struct Mix {
    pub(super) lines: usize,
    /// Batch / LeastSquaresWeighted loop closures attempted.
    pub(super) closures: usize,
    /// Datums attempted (the first of a piece places it, later ones are fixes).
    pub(super) datums: usize,
    /// Twin lines whose ends coincide, snapped with an aligned `Move` closure.
    pub(super) snaps: usize,
    /// Closures across exactly one shot, written as an old file would hold them.
    pub(super) across_one_shot: usize,
    /// Jump / reference shots.
    pub(super) virtual_shots: bool,
}

impl Default for Mix {
    fn default() -> Self {
        Self {
            lines: 12,
            closures: 4,
            datums: 4,
            snaps: 1,
            across_one_shot: 0,
            virtual_shots: true,
        }
    }
}

fn gps(latitude: f64, longitude: f64) -> old::GpsCoordinates {
    old::GpsCoordinates {
        latitude,
        longitude,
        depth_reference: 0.0,
    }
}

fn slot_count(old: &Old, line: LineId) -> usize {
    old.get_line(line).map_or(0, |l| l.shots.len() + 1)
}

/// A random legacy survey built through the legacy API only: lines merged at
/// random slots (refused merges leave free pieces), hand moves set before the
/// merges, datums (placements and hard / soft GPS fixes, entrance overrides),
/// free pieces moved by hand, loop closures (Batch, LeastSquaresWeighted, torn
/// and aligned `Move` snaps, legacy aligned Batch, legacy across-one-shot).
pub(super) fn random_legacy_survey(seed: u64, mix: Mix) -> Old {
    let mut rng = Lcg(seed);
    let mut old = Old::new("legacy".to_string(), None);
    for _ in 0..mix.lines {
        let count = 1 + rng.below(6);
        let mut shots = random_shots(&mut rng, count);
        if mix.virtual_shots {
            for shot in &mut shots {
                match rng.below(14) {
                    0 => shot.kind = old::ShotKind::Jump,
                    1 => shot.kind = old::ShotKind::Reference,
                    _ => {}
                }
            }
        }
        let line = old_line(&mut old, shots);
        if rng.below(4) == 0 {
            if let Some(l) = old.lines.iter_mut().find(|l| l.id == line) {
                l.quality = 0.5 + rng.unit() * 2.0;
            }
        }
        let len = slot_count(&old, line);
        if rng.below(4) == 0 {
            let slot = old::StationRef::new(line, rng.below(len) as u32);
            old.station_offsets.insert(
                old::StationKey::from(slot),
                (rng.unit() - 0.5, rng.unit() - 0.5, rng.unit() - 0.5),
            );
        }
        if old.lines.len() > 1 && rng.below(5) != 0 {
            let target = old.lines[rng.below(old.lines.len() - 1)].id;
            let a = old::StationRef::new(line, rng.below(len) as u32);
            let b = old::StationRef::new(target, rng.below(slot_count(&old, target)) as u32);
            let _ = old.merge_stations(a, b);
        }
    }
    // Twin lines from one station whose ends coincide: a snap.
    for _ in 0..mix.snaps {
        let host = old.lines[rng.below(old.lines.len())].id;
        let j = old::StationRef::new(host, rng.below(slot_count(&old, host)) as u32);
        let v = random_shots(&mut rng, 2);
        let mut swapped = vec![v[1].clone(), v[0].clone()];
        for s in &mut swapped {
            s.uuid = None;
        }
        let a = old_line(&mut old, v);
        let b = old_line(&mut old, swapped);
        if old.merge_stations(old::StationRef::new(a, 0), j).is_err()
            || old.merge_stations(old::StationRef::new(b, 0), j).is_err()
        {
            continue;
        }
        let _ = old.create_loop_closure(
            old::StationRef::new(a, 2),
            old::StationRef::new(b, 2),
            old::ClosureStrategy::Move,
        );
    }
    let lines: Vec<LineId> = old.lines.iter().map(|l| l.id).collect();
    for k in 0..mix.datums {
        let line = lines[rng.below(lines.len())];
        let slot = old::StationRef::new(line, rng.below(slot_count(&old, line)) as u32);
        let positions = old_raw(&old);
        let frame = old.zero_datums.first().map(|d| d.gps);
        let g = match frame {
            None => gps(20.5 + rng.unit() * 0.01, -87.4 + rng.unit() * 0.01),
            Some(frame) => {
                let [x, y, _] = positions
                    .get(&live(slot))
                    .copied()
                    .unwrap_or([0.0, 0.0, 0.0]);
                let (dx, dy) = ((rng.unit() - 0.5) * 30.0, (rng.unit() - 0.5) * 30.0);
                match crate::local_to_gps(x + dx, y + dy, &relive(&frame)) {
                    Some((lat, lon)) => gps(lat, lon),
                    None => continue,
                }
            }
        };
        if old.set_zero_datum(slot, g).is_err() {
            continue;
        }
        if let Some(datum) = old.zero_datums.last_mut() {
            match rng.below(4) {
                0 => datum.accuracy_m = Some(1.0 + rng.unit() * 8.0),
                1 => datum.accuracy_m = Some(0.0),
                _ => {}
            }
            if k % 3 == 2 {
                datum.entrance = Some(rng.below(2) == 0);
            }
        }
    }
    // Move unmerged lines by hand.
    let merged: Vec<LineId> = old
        .station_merges
        .iter()
        .flat_map(|m| [&m.station_a, &m.station_b])
        .filter_map(|key| old.resolve_key(key))
        .map(|slot| slot.line_id)
        .collect();
    for &line in &lines {
        if rng.below(3) == 0 && !merged.contains(&line) {
            let (x, y) = old.line_offsets.get(&line).copied().unwrap_or((0.0, 0.0));
            old.line_offsets.insert(
                line,
                (x + rng.unit() * 120.0 - 60.0, y + rng.unit() * 120.0 - 60.0),
            );
        }
    }
    for _ in 0..mix.closures * 4 {
        if old.loop_closures.len() >= mix.closures + mix.snaps {
            break;
        }
        let (la, lb) = (lines[rng.below(lines.len())], lines[rng.below(lines.len())]);
        let a = old::StationRef::new(la, rng.below(slot_count(&old, la)) as u32);
        let b = old::StationRef::new(lb, rng.below(slot_count(&old, lb)) as u32);
        let strategy = match rng.below(6) {
            0 => old::ClosureStrategy::LeastSquaresWeighted,
            1 => old::ClosureStrategy::Move,
            _ => old::ClosureStrategy::Batch,
        };
        if let Ok(closure) = old.create_loop_closure(a, b, strategy) {
            if strategy == old::ClosureStrategy::Batch && rng.below(6) == 0 {
                if let Some(c) = old.loop_closures.iter_mut().find(|c| c.id == closure.id) {
                    c.is_aligned = true;
                }
            }
        }
    }
    for _ in 0..mix.across_one_shot {
        let line = lines[rng.below(lines.len())];
        let len = slot_count(&old, line);
        if len < 2 {
            continue;
        }
        let i = rng.below(len - 1) as u32;
        let id = old.loop_closures.iter().map(|c| c.id).max().unwrap_or(0) + 1;
        old.loop_closures.push(old::LoopClosure {
            id,
            station_a: old::StationKey::from(old::StationRef::new(line, i)),
            station_b: old::StationKey::from(old::StationRef::new(line, i + 1)),
            path_length: 0.0,
            misclosure_distance: 0.0,
            error_percentage: 0.0,
            horizontal_misclosure: 0.0,
            vertical_misclosure: 0.0,
            strategy: old::ClosureStrategy::Batch,
            is_aligned: false,
        });
    }
    old
}
