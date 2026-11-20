//! The derived topology of a survey (plan 3.1): line chains, slots, and
//! connected components.
//!
//! Built in one pass over the lines (`O(n log n)`, `n` = stations + shots) and
//! cached on the [`super::Project`] until the lines are next borrowed mutably.
//! Total on invalid models: a dangling `to`, a station twice on one chain or a
//! cycle are indexed as they are, and [`super::Project::validate`] reports them.

use std::collections::BTreeMap;

use crate::{LineId, StationId, StationRef};

use super::Line;

/// A connected component: stations joined by shots, junctions joined because
/// they are one station.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    /// Its stations, ascending.
    pub stations: Vec<StationId>,
    /// The lines whose chains lie in it, in list order.
    pub lines: Vec<LineId>,
}

impl Component {
    /// The lowest station id (components are ordered by it).
    pub fn lowest_station(&self) -> Option<StationId> {
        self.stations.first().copied()
    }

    /// Whether `station` is in this component.
    pub fn contains(&self, station: StationId) -> bool {
        self.stations.binary_search(&station).is_ok()
    }
}

/// Slots, chains and components of one state of the lines.
#[derive(Debug, Clone, Default)]
pub struct Topology {
    /// Line id -> list position (the first line with that id).
    line_positions: BTreeMap<LineId, usize>,
    /// Per line, in list order: the chain `[start, shots[0].to, ...]`.
    chains: Vec<Vec<StationId>>,
    /// Every station on some chain, ascending (the dense index space).
    stations: Vec<StationId>,
    /// CSR offsets into `slots`: station `stations[d]` owns
    /// `slots[slot_offsets[d]..slot_offsets[d + 1]]`.
    slot_offsets: Vec<usize>,
    /// Every slot, grouped by station; within a station sorted by (line list
    /// order, index).
    slots: Vec<StationRef>,
    /// Component of each dense station.
    component_of: Vec<usize>,
    /// Components, ordered by their lowest station id.
    components: Vec<Component>,
}

impl Topology {
    /// Index `lines` (in list order).
    pub fn build(lines: &[Line]) -> Self {
        let mut line_positions = BTreeMap::new();
        let mut chains = Vec::with_capacity(lines.len());
        for (position, line) in lines.iter().enumerate() {
            line_positions.entry(line.id).or_insert(position);
            chains.push(line.stations().collect::<Vec<_>>());
        }

        let mut stations: Vec<StationId> = chains.iter().flatten().copied().collect();
        stations.sort_unstable();
        stations.dedup();
        let dense = |station: StationId| stations.binary_search(&station).unwrap_or(0);

        // Slots: (dense station, line position, index), stably grouped by
        // station so each group keeps (line order, index).
        let mut keyed: Vec<(usize, StationRef)> = Vec::with_capacity(stations.len());
        for (line, chain) in lines.iter().zip(&chains) {
            for (index, &station) in chain.iter().enumerate() {
                keyed.push((dense(station), StationRef::new(line.id, index as u32)));
            }
        }
        keyed.sort_by_key(|(d, _)| *d);
        let mut slot_offsets = vec![0; stations.len() + 1];
        for (d, _) in &keyed {
            slot_offsets[d + 1] += 1;
        }
        for d in 0..stations.len() {
            slot_offsets[d + 1] += slot_offsets[d];
        }
        let slots = keyed.into_iter().map(|(_, slot)| slot).collect();

        // Components: union-find over consecutive chain stations.
        let mut parent: Vec<usize> = (0..stations.len()).collect();
        for chain in &chains {
            for pair in chain.windows(2) {
                union(&mut parent, dense(pair[0]), dense(pair[1]));
            }
        }
        let mut root_component: BTreeMap<usize, usize> = BTreeMap::new();
        let mut component_of = Vec::with_capacity(stations.len());
        let mut components: Vec<Component> = Vec::new();
        for (d, &station) in stations.iter().enumerate() {
            let root = find(&mut parent, d);
            let index = *root_component.entry(root).or_insert_with(|| {
                components.push(Component {
                    stations: Vec::new(),
                    lines: Vec::new(),
                });
                components.len() - 1
            });
            components[index].stations.push(station);
            component_of.push(index);
        }
        for line in lines {
            let index = component_of[dense(line.start)];
            components[index].lines.push(line.id);
        }

        Self {
            line_positions,
            chains,
            stations,
            slot_offsets,
            slots,
            component_of,
            components,
        }
    }

    /// The list position of line `id`.
    pub fn line_position(&self, id: LineId) -> Option<usize> {
        self.line_positions.get(&id).copied()
    }

    /// Line `id`'s chain.
    pub fn chain(&self, id: LineId) -> Option<&[StationId]> {
        self.line_position(id)
            .and_then(|position| self.chain_at(position))
    }

    /// The chain of the line at list position `position`.
    pub fn chain_at(&self, position: usize) -> Option<&[StationId]> {
        self.chains.get(position).map(Vec::as_slice)
    }

    /// The station at a slot, if the slot exists.
    pub fn station_at(&self, slot: StationRef) -> Option<StationId> {
        self.chain(slot.line_id)?
            .get(slot.station_index as usize)
            .copied()
    }

    /// Every station on some chain, ascending.
    pub fn stations(&self) -> &[StationId] {
        &self.stations
    }

    /// Whether `station` is on some chain.
    pub fn contains(&self, station: StationId) -> bool {
        self.dense(station).is_some()
    }

    /// The dense index of `station` (its position in [`Self::stations`]).
    pub fn dense(&self, station: StationId) -> Option<usize> {
        self.stations.binary_search(&station).ok()
    }

    /// The slots of `station`, sorted by (line list order, index). Empty for a
    /// station on no chain.
    pub fn slots_of(&self, station: StationId) -> &[StationRef] {
        self.dense(station).map_or(&[], |d| {
            &self.slots[self.slot_offsets[d]..self.slot_offsets[d + 1]]
        })
    }

    /// The lines `station` is on, in list order (once each, even if an invalid
    /// chain repeats it).
    pub fn lines_of(&self, station: StationId) -> Vec<LineId> {
        let mut lines: Vec<LineId> = Vec::new();
        for slot in self.slots_of(station) {
            if lines.last() != Some(&slot.line_id) && !lines.contains(&slot.line_id) {
                lines.push(slot.line_id);
            }
        }
        lines
    }

    /// Whether `station` is on two or more lines.
    pub fn is_junction(&self, station: StationId) -> bool {
        self.lines_of(station).len() > 1
    }

    /// The first slot of `station` (its address on the wire).
    pub fn first_slot(&self, station: StationId) -> Option<StationRef> {
        self.slots_of(station).first().copied()
    }

    /// The slot of `station` on line `line` (the first, if an invalid chain
    /// repeats it).
    pub fn slot_on(&self, station: StationId, line: LineId) -> Option<StationRef> {
        self.slots_of(station)
            .iter()
            .find(|slot| slot.line_id == line)
            .copied()
    }

    /// The component index of `station`.
    pub fn component_of(&self, station: StationId) -> Option<usize> {
        self.dense(station).map(|d| self.component_of[d])
    }

    /// Every component, ordered by lowest station id.
    pub fn components(&self) -> &[Component] {
        &self.components
    }

    /// Component `index`.
    pub fn component(&self, index: usize) -> Option<&Component> {
        self.components.get(index)
    }

    /// Whether two stations are in one component (connected by shots and
    /// junctions; closures do not connect).
    pub fn connected(&self, a: StationId, b: StationId) -> bool {
        match (self.component_of(a), self.component_of(b)) {
            (Some(ca), Some(cb)) => ca == cb,
            _ => false,
        }
    }
}

fn find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let (ra, rb) = (find(parent, a), find(parent, b));
    if ra != rb {
        // Lower index wins, so roots do not depend on the edge order.
        let (keep, drop) = if ra < rb { (ra, rb) } else { (rb, ra) };
        parent[drop] = keep;
    }
}

/// Where a shot is: its line and index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShotLocation {
    pub line: LineId,
    pub index: usize,
}

/// Shot uuid -> every place it occurs (one, in a valid model), in list order.
#[derive(Debug, Clone, Default)]
pub struct ShotUuidIndex {
    by_uuid: BTreeMap<String, Vec<ShotLocation>>,
}

impl ShotUuidIndex {
    /// Index every shot of `lines`.
    pub fn build(lines: &[Line]) -> Self {
        let mut by_uuid: BTreeMap<String, Vec<ShotLocation>> = BTreeMap::new();
        for line in lines {
            for (index, shot) in line.shots.iter().enumerate() {
                by_uuid
                    .entry(shot.uuid.clone())
                    .or_default()
                    .push(ShotLocation {
                        line: line.id,
                        index,
                    });
            }
        }
        Self { by_uuid }
    }

    /// The first occurrence of `uuid`.
    pub fn get(&self, uuid: &str) -> Option<ShotLocation> {
        self.by_uuid
            .get(uuid)
            .and_then(|locations| locations.first())
            .copied()
    }

    /// Whether some shot already has `uuid`.
    pub fn contains(&self, uuid: &str) -> bool {
        self.by_uuid.contains_key(uuid)
    }

    /// Every uuid held by more than one shot, with all its locations, in uuid
    /// order.
    pub fn duplicates(&self) -> impl Iterator<Item = (&str, &[ShotLocation])> {
        self.by_uuid
            .iter()
            .filter(|(_, locations)| locations.len() > 1)
            .map(|(uuid, locations)| (uuid.as_str(), locations.as_slice()))
    }
}
