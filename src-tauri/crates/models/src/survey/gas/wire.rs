//! The slot boundary of the station-graph gas engine: the old entry point's signature (slots in,
//! the per-slot [`GasReachField`] out) on top of [`Project::gas_reach`].
//!
//! Replaces the slot handling of `models/src/project/gas.rs`
//! (`compute_gas_reach_field_with_overrides`, `route_legs`). Wire rules:
//!
//! - `station_refs`: every slot in slot order (lines in list order, then chain index), which is
//!   the old `compute_station_coordinates` order; each slot carries its station's values.
//! - `turn_frontier`: the first slot of each frontier station, in slot order.
//! - route legs: a swum leg is named by the two slots on the leg shot's line, a closure tie by the
//!   two stations' first slots.
//! - a route node's slot (`stage_drop_refs`, `final_turn_ref`, the profile's `station_ref`) is its
//!   slot on [`GasReach::route_lines`]: the line of the arriving leg (the departing leg for the
//!   first node).
//! - `scooter_drop_ref` is the caller's own way-point slot.
//! - route indices index the station route, which has no merge hops.

use crate::config::GasConfig;
use crate::gas::{Cylinder, Diver, GasReachField, PlanSegment, RouteLeg, RouteProfilePoint};
use crate::{LineId, ShotKind, StationId, StationRef};

use super::super::{Project, Topology};
use super::GasReach;

/// `station`'s slot on `line`, else its first slot.
fn slot_on_or_first(
    topology: &Topology,
    station: StationId,
    line: Option<LineId>,
) -> Option<StationRef> {
    line.and_then(|l| topology.slot_on(station, l))
        .or_else(|| topology.first_slot(station))
}

/// A route index on the wire.
fn wire_index(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
}

impl GasReach {
    /// The wire slot of route node `i`.
    fn route_slot(&self, topology: &Topology, i: usize) -> Option<StationRef> {
        let station = *self.route.get(i)?;
        slot_on_or_first(
            topology,
            station,
            self.route_lines.get(i).copied().flatten(),
        )
    }

    /// The per-slot wire field for `project` (the project this result was computed on).
    /// `waypoints` are the caller's way-point slots, in the order passed to the engine: the
    /// scooter park point is reported as one of them.
    pub fn to_wire(&self, project: &Project, waypoints: &[StationRef]) -> GasReachField {
        let topology = project.topology();

        let mut station_refs = Vec::new();
        let mut gas_to_reach = Vec::new();
        let mut reachable = Vec::new();
        let mut turn_frontier = Vec::new();
        for line in project.lines() {
            for (index, station) in line.stations().enumerate() {
                let slot = StationRef::new(line.id, index as u32);
                station_refs.push(slot);
                gas_to_reach.push(self.gas_at(station));
                reachable.push(self.is_reachable(station));
                if self.turn_frontier.binary_search(&station).is_ok()
                    && topology.first_slot(station) == Some(slot)
                {
                    turn_frontier.push(slot);
                }
            }
        }

        let route: Vec<RouteLeg> = self
            .route
            .windows(2)
            .enumerate()
            .filter_map(|(i, w)| {
                let line = self
                    .route_shots
                    .get(i)
                    .copied()
                    .flatten()
                    .map(|shot| shot.line);
                Some(RouteLeg {
                    from: slot_on_or_first(topology, w[0], line)?,
                    to: slot_on_or_first(topology, w[1], line)?,
                    is_jump: self.route_leg_kinds.get(i).copied().flatten() == Some(ShotKind::Jump),
                })
            })
            .collect();

        let stage_drop_refs: Vec<StationRef> = self
            .stage_drop_route_indices
            .iter()
            .filter_map(|&j| self.route_slot(topology, j))
            .collect();
        // The drop indices always name route nodes, so nothing above was dropped. Assert it
        // loudly rather than let a future change silently shorten `stage_drop_refs` and misalign
        // it from the per-stage gauges and distances, which are indexed like the drops.
        debug_assert_eq!(
            stage_drop_refs.len(),
            self.stage_drop_route_indices.len(),
            "a stage drop fell off the route - refs would misalign with the gauges"
        );
        let final_turn_ref = self.final_turn.and_then(|station| {
            let last = self.route.len().checked_sub(1)?;
            if self.route.get(last) == Some(&station) {
                self.route_slot(topology, last)
            } else {
                topology.first_slot(station)
            }
        });
        let route_profile: Vec<RouteProfilePoint> = self
            .route_profile
            .iter()
            .filter_map(|p| {
                Some(RouteProfilePoint {
                    route_index: wire_index(p.route_index),
                    station_ref: self.route_slot(topology, p.route_index)?,
                    distance_m: p.distance_m,
                    depth_m: p.depth_m,
                    gas_l: p.gas_l,
                    elapsed_min: p.elapsed_min,
                    outbound: p.outbound,
                })
            })
            .collect();

        GasReachField {
            station_refs,
            gas_to_reach,
            reachable,
            turn_frontier,
            max_reachable_gas: self.max_reachable_gas,
            budget_litres: self.budget_litres,
            route,
            stage_drop_refs,
            final_turn_ref,
            turn_toward_endpoint: self.turn_toward_endpoint,
            backgas_turn_bar: self.backgas_turn_bar,
            stage_drop_bars: self.stage_drop_bars.clone(),
            stage_drop_over_reserve: self.stage_drop_over_reserve.clone(),
            stage_drop_distances_m: self.stage_drop_distances_m.clone(),
            cylinder_usable_bars: self.cylinder_usable_bars.clone(),
            route_line_length_m: self.route_line_length_m,
            route_max_depth_m: self.route_max_depth_m,
            route_avg_depth_m: self.route_avg_depth_m,
            waypoint_gas_litres: self.waypoint_gas_litres.clone(),
            route_profile,
            stage_drop_route_indices: self
                .stage_drop_route_indices
                .iter()
                .map(|&j| wire_index(j))
                .collect(),
            waypoint_route_indices: self
                .waypoint_route_indices
                .iter()
                .map(|idx| idx.map(wire_index))
                .collect(),
            scooter_distance_m: self.scooter_distance_m,
            scooter_range_limit_m: self.scooter_range_limit_m,
            scooter_drop_ref: self
                .scooter_drop_waypoint
                .and_then(|k| waypoints.get(k).copied()),
            route_runtime_min: self.route_runtime_min,
            turn_on_scooter_range: self.turn_on_scooter_range,
        }
    }
}

impl Project {
    /// The gas reach field for the wire, with the SAME signature as the old
    /// `compute_gas_reach_field_with_overrides`: slots are resolved to stations at this boundary,
    /// and an unresolvable slot behaves like the old unresolvable key (an origin falls back to the
    /// entrance, an endpoint is ignored, a way point stalls the walk, a drop override keeps the
    /// strategy default). See [`Project::gas_reach`] for the engine.
    pub fn gas_reach_field_wire(
        &self,
        origin: Option<&StationRef>,
        endpoint: Option<&StationRef>,
        waypoints: &[StationRef],
        segments: &[PlanSegment],
        diver: &Diver,
        cylinders: &[Cylinder],
        gas_config: &GasConfig,
        stage_drop_overrides: &[Option<StationRef>],
    ) -> GasReachField {
        let resolve = |slot: &StationRef| self.station_at(*slot);
        let resolved_waypoints: Vec<Option<StationId>> = waypoints.iter().map(resolve).collect();
        let resolved_overrides: Vec<Option<StationId>> = stage_drop_overrides
            .iter()
            .map(|o| o.as_ref().and_then(resolve))
            .collect();
        self.gas_reach(
            origin.and_then(resolve),
            endpoint.and_then(resolve),
            &resolved_waypoints,
            segments,
            diver,
            cylinders,
            gas_config,
            &resolved_overrides,
        )
        .to_wire(self, waypoints)
    }
}
