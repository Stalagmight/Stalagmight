//! The per-slot station list of the wire (plan 3.5, 9.2): one
//! [`crate::Station`] per slot, lines in list order then chain index, as
//! `get_project_state` and `get_adjusted_stations` send it today.
//!
//! Per slot `(L, i)` with station `S`:
//! - coordinates: `S`'s position (raw or Computed View), depth plus `L`'s
//!   hand-move z at `S`;
//! - `name`: the name `L` shows `S` by ([`Project::name_on`]: the alias `L`
//!   knows a junction by, else `S`'s own name), so a junction's slots may
//!   differ;
//! - `id`, `comment`, `aliases`: `S`'s, shared by every slot of a junction
//!   (`aliases` absent when `S` has none; each with the line it names);
//! - `merged_with`: `S`'s other slots (slot order), so a junction's slots
//!   list each other, transitively, as today's merge groups did;
//! - `is_zero_datum` / `gps_coordinates`: the datum the slot's datum dialog
//!   edits ([`Project::datum_index_for`]: the first datum on `S` attached
//!   via `L`, else the first on `S`), so every slot of a datum station shows
//!   it;
//! - `closures_with`: today's rule, with a station's slots as its merge
//!   group: a closure lists its far end directly on the end's own slot and
//!   transitively on the end station's other slots, plus the far station's
//!   other slots; then a slot with two or more direct closures lets each of
//!   its partners see the others ("closure-chain" entries).

use std::collections::BTreeMap;

use crate::{ClosureRef, Coordinates, StationId, StationRef};

use super::super::{AdjustedPositions, Project, RawPositions, StationAlias, StationPositions};

/// Everything a wire station holds but its coordinates (one state).
struct SlotRelations {
    /// Every slot with its station, in wire order.
    slots: Vec<(StationRef, StationId)>,
    /// Index of each line's first slot in `slots`, by line position.
    line_start: Vec<usize>,
    /// Per slot: the datum it shows.
    datum: Vec<Option<usize>>,
    /// Per slot: its closure references.
    closures: Vec<Vec<ClosureRef>>,
}

impl SlotRelations {
    fn of(project: &Project) -> Self {
        let mut slots = Vec::new();
        let mut line_start = Vec::with_capacity(project.lines().len());
        for line in project.lines() {
            line_start.push(slots.len());
            slots.extend(
                line.stations()
                    .enumerate()
                    .map(|(index, station)| (StationRef::new(line.id, index as u32), station)),
            );
        }
        let mut relations = Self {
            datum: vec![None; slots.len()],
            closures: vec![Vec::new(); slots.len()],
            slots,
            line_start,
        };
        relations.attach_datums(project);
        relations.attach_closures(project);
        relations
    }

    /// The position of `slot` in the wire list.
    fn index(&self, project: &Project, slot: StationRef) -> Option<usize> {
        let start = *self.line_start.get(project.line_position(slot.line_id)?)?;
        let index = start + slot.station_index as usize;
        (self.slots.get(index)?.0 == slot).then_some(index)
    }

    fn attach_datums(&mut self, project: &Project) {
        let mut by_station: BTreeMap<StationId, Vec<usize>> = BTreeMap::new();
        for (index, datum) in project.datums.iter().enumerate() {
            by_station.entry(datum.station).or_default().push(index);
        }
        if by_station.is_empty() {
            return;
        }
        for (shown, (slot, station)) in self.datum.iter_mut().zip(&self.slots) {
            let Some(held) = by_station.get(station) else {
                continue;
            };
            *shown = held
                .iter()
                .copied()
                .find(|&d| project.datums[d].via == Some(slot.line_id))
                .or_else(|| held.first().copied());
        }
    }

    fn attach_closures(&mut self, project: &Project) {
        let topology = project.topology();
        for closure in &project.closures {
            if closure.a == closure.b {
                continue;
            }
            let (Some(a_ref), Some(b_ref)) = (
                project.closure_end_slot(closure.a, closure.a_via),
                project.closure_end_slot(closure.b, closure.b_via),
            ) else {
                continue;
            };
            for (near, near_ref, far, far_ref) in [
                (closure.a, a_ref, closure.b, b_ref),
                (closure.b, b_ref, closure.a, a_ref),
            ] {
                for &slot in topology.slots_of(near) {
                    let Some(at) = self.index(project, slot) else {
                        continue;
                    };
                    let via = (slot != near_ref).then_some(near_ref);
                    let list = &mut self.closures[at];
                    list.push(ClosureRef {
                        closure_id: closure.id,
                        station_ref: far_ref,
                        via_station: via,
                        is_transitive: via.is_some(),
                    });
                    list.extend(
                        topology
                            .slots_of(far)
                            .iter()
                            .filter(|&&member| member != far_ref)
                            .map(|&member| ClosureRef {
                                closure_id: closure.id,
                                station_ref: member,
                                via_station: Some(far_ref),
                                is_transitive: true,
                            }),
                    );
                }
            }
        }
        self.attach_closure_chains(project);
    }

    /// Today's closure-chain transitivity: a slot X with direct closures to
    /// Y and Z lets Y see Z (and Z see Y) via X, under the id of the closure
    /// that reaches the far one. Shared slots in wire order; an entry with
    /// the same target and via is not added twice.
    fn attach_closure_chains(&mut self, project: &Project) {
        let mut chained: Vec<(StationRef, ClosureRef)> = Vec::new();
        for (at, list) in self.closures.iter().enumerate() {
            let direct: Vec<(u64, StationRef)> = list
                .iter()
                .filter(|r| !r.is_transitive)
                .map(|r| (r.closure_id, r.station_ref))
                .collect();
            if direct.len() < 2 {
                continue;
            }
            let shared = self.slots[at].0;
            for (i, &(_, target_i)) in direct.iter().enumerate() {
                for (j, &(closure_j, target_j)) in direct.iter().enumerate() {
                    if i != j && target_i != target_j {
                        chained.push((
                            target_i,
                            ClosureRef {
                                closure_id: closure_j,
                                station_ref: target_j,
                                via_station: Some(shared),
                                is_transitive: true,
                            },
                        ));
                    }
                }
            }
        }
        for (slot, entry) in chained {
            let Some(at) = self.index(project, slot) else {
                continue;
            };
            let list = &mut self.closures[at];
            let known = list
                .iter()
                .any(|r| r.station_ref == entry.station_ref && r.via_station == entry.via_station);
            if !known {
                list.push(entry);
            }
        }
    }

    /// The wire stations with coordinates from `positions`.
    fn stations(self, project: &Project, positions: &impl StationPositions) -> Vec<crate::Station> {
        let topology = project.topology();
        let Self {
            slots,
            datum,
            closures,
            ..
        } = self;
        slots
            .into_iter()
            .zip(datum)
            .zip(closures)
            .map(|(((slot, station), datum), closures)| {
                let identity = project.stations.get(&station);
                let gps = datum.and_then(|d| project.datums.get(d)).map(|d| d.gps);
                crate::Station {
                    station_ref: slot,
                    coordinates: slot_coordinates(project, positions, slot, station),
                    is_zero_datum: gps.is_some(),
                    gps_coordinates: gps,
                    merged_with: topology
                        .slots_of(station)
                        .iter()
                        .copied()
                        .filter(|&other| other != slot)
                        .collect(),
                    closures_with: (!closures.is_empty()).then_some(closures),
                    comment: identity.and_then(|s| s.comment.clone()),
                    id: station,
                    name: project
                        .name_on(station, slot.line_id)
                        .map(str::to_string)
                        .unwrap_or_default(),
                    aliases: identity
                        .filter(|s| !s.aliases.is_empty())
                        .map(|s| s.aliases.iter().map(alias_wire).collect()),
                }
            })
            .collect()
    }
}

/// The wire form of one alias.
fn alias_wire(alias: &StationAlias) -> crate::StationAliasWire {
    crate::StationAliasWire {
        id: alias.id,
        name: alias.name.clone(),
        line: alias.line,
    }
}

/// A slot's coordinates: its station's XY, its station's depth plus the
/// slot line's hand-move z (exactly [`RawPositions::slots`]). A station
/// with no position (only in an invalid model) is at the origin.
fn slot_coordinates(
    project: &Project,
    positions: &impl StationPositions,
    slot: StationRef,
    station: StationId,
) -> Coordinates {
    let (Some([x, y]), Some(z)) = (positions.station_xy(station), positions.station_z(station))
    else {
        return Coordinates::default();
    };
    let z = project
        .line(slot.line_id)
        .and_then(|line| line.hand_moves.get(&station))
        .map_or(z, |hand| z + hand[2]);
    Coordinates::new(x, y, z)
}

impl Project {
    /// The per-slot stations of the raw (measured) layout `raw` of this
    /// state: `get_project_state`'s `stations`.
    pub fn stations_wire(&self, raw: &RawPositions) -> Vec<crate::Station> {
        SlotRelations::of(self).stations(self, raw)
    }

    /// The per-slot stations of the Closures Computed View `adjusted` of
    /// this state: `get_adjusted_stations` (same slots, same relations).
    pub fn adjusted_stations_wire(&self, adjusted: &AdjustedPositions) -> Vec<crate::Station> {
        SlotRelations::of(self).stations(self, adjusted)
    }
}
