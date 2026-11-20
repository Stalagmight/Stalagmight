//! Build the station-graph project from a settled legacy project (plan 7.2.1
//! to 7.2.6). Each step is described where it happens; the module root
//! (`super`) lists the rules.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use stalagmight_legacy::models::{self as old, ComponentHold};
use stalagmight_legacy::{LegacySurvey, Slot};

use crate::models::survey::{
    ClosureId, ClosureStrategy, Line, LoopClosure, Placement, Project, ZeroDatum,
    MAX_SHOT_UUID_CHARS,
};
use crate::models::{LineId, StationId, MAX_LINE_ID, MAX_SECTION_ID};

use super::convert::{self, Credits};
use super::merges::{self, Fate};
use super::slots::{resolve_map, Slots};
use super::stations;
use super::{MigrateError, MigrationReport};

/// What the verification needs to know about how the project was built.
pub(super) struct Built {
    pub(super) project: Project,
    pub(super) slots: Slots,
    /// Per component of the migrated project (topology order): the dense
    /// legacy slot whose legacy position its anchor holds.
    pub(super) anchor_slots: Vec<usize>,
    /// Legacy `is_aligned` of every migrated `Move` closure, by id.
    pub(super) legacy_snap_alignment: BTreeMap<ClosureId, bool>,
    /// Legacy shot `(line, uuid after the load)` -> migrated uuid.
    pub(super) final_uuid: HashMap<(LineId, String), String>,
}

/// The namespace migrated shot uuids are minted in (a shot the legacy load
/// left without a usable uuid). Fixed, so a migration is deterministic.
const MIGRATED_UUID_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x3c5e_9a71_2d48_4f06_b1e3_77c0_5d29_a814);

pub(super) fn build(
    legacy: &LegacySurvey,
    report: &mut MigrationReport,
) -> Result<Built, MigrateError> {
    let lp = &legacy.project;
    let slots = Slots::new(lp);

    // 1. Merges -> shared stations and demoted ties; one station per group.
    let merges = merges::plan(legacy, &slots);
    report.merges_collapsed = merges.count(Fate::Collapsed);
    report.merges_demoted_cycle = merges.count(Fate::DemotedCycle);
    report.merges_demoted_same_line = merges.count(Fate::DemotedSameLine);
    report.merges_dropped_self = merges.count(Fate::DroppedSelf);
    report.merges_dropped_redundant = merges.count(Fate::DroppedRedundant);
    report.merges_dropped_dangling = merges.count(Fate::DroppedDangling);
    let station_plan = stations::plan(lp, &slots, &merges, report)?;
    let of_slot = station_plan.of_slot;

    let mut project = Project::new_at(lp.name.clone(), lp.location.clone(), lp.created_at);
    project.modified_at = lp.modified_at;
    project.settings.validation = convert::validation(&lp.validation);
    project.settings.surface_elevation = lp.surface_elevation;
    project.settings.default_azimuth_reference = lp
        .default_azimuth_reference
        .as_ref()
        .map(convert::azimuth_reference);
    project
        .imported_file_hashes
        .clone_from(&lp.imported_file_hashes);
    project
        .imported_line_hashes
        .clone_from(&lp.imported_line_hashes);
    project.sections = lp.sections.iter().map(convert::section).collect();
    project.stations = station_plan.stations;
    project.counters.next_station_id = station_plan.next_station_id;
    project.counters.next_section_id = lp
        .sections
        .iter()
        .map(|s| s.id.saturating_add(1))
        .max()
        .unwrap_or(0)
        .max(lp.next_section_id)
        .min(MAX_SECTION_ID);

    // 2. Lines: shots in order with their uuids (minted where the load left
    //    none usable), chains mapped through the station plan.
    let (lines, final_uuid) = build_lines(legacy, &slots, &of_slot, report)?;
    project.lines_mut().extend(lines);

    // 3. Hand moves, per line (a merged member's move stays on its line).
    let hand = resolve_map(lp, &slots, &lp.station_offsets);
    report.station_keys_dangling += hand.dangling;
    report.station_keys_duplicate += hand.duplicates;
    for (dense, (dx, dy, dz)) in hand.by_slot {
        if !(dx.is_finite() && dy.is_finite() && dz.is_finite()) {
            report.numbers_sanitized += 1;
            continue;
        }
        let position = slots.line_position_of(dense);
        if let Some(line) = project.lines_mut().get_mut(position) {
            line.hand_moves.insert(of_slot[dense], [dx, dy, dz]);
            report.hand_moves_carried += 1;
        }
    }

    // 4. Datums, all kept in list order, each hinted with its member's line.
    let mut datum_slots = Vec::new();
    let mut datum_sources = Vec::new();
    for (index, datum) in lp.zero_datums.iter().enumerate() {
        let Some(dense) = slots
            .resolve(&datum.station_ref)
            .and_then(|slot| slots.dense(slot))
        else {
            report.datums_dropped_dangling += 1;
            continue;
        };
        project.datums.push(ZeroDatum {
            station: of_slot[dense],
            via: Some(slots.slot(dense).line_id),
            gps: convert::gps(&datum.gps),
            accuracy_m: datum.accuracy_m,
            entrance: datum.entrance,
            correction: None,
        });
        datum_slots.push(dense);
        datum_sources.push(index);
    }
    report.datums_kept = project.datums.len();

    // 5. Closures: demoted merges first (merges were united before any
    //    closure, so later closures keep their skip verdicts), then the legacy
    //    closures in list order with their ids.
    let legacy_snap_alignment =
        build_closures(legacy, &slots, &of_slot, &merges, &mut project, report);

    // 6. Anchors: corrections for placement datums, placements for the rest.
    let anchor_slots = build_anchors(legacy, &slots, &of_slot, &datum_slots, &mut project, report);
    drop_unanchored_closures(&mut project, report);

    // 7. Entrance overrides where the new default would differ from 0.6.0's.
    write_entrance_overrides(lp, &datum_sources, &mut project, report);

    // 8. Names unique on every line of each junction.
    uniquify_junction_names(&mut project, report);

    report.held_gauges_dropped = lp.held_gauges.len();
    report.gauge_pairs_dropped = lp
        .lines
        .iter()
        .filter(|l| l.gauge_shot_uuid.is_some())
        .count();
    Ok(Built {
        project,
        slots,
        anchor_slots,
        legacy_snap_alignment,
        final_uuid,
    })
}

type BuiltLines = (Vec<Line>, HashMap<(LineId, String), String>);

fn build_lines(
    legacy: &LegacySurvey,
    slots: &Slots,
    of_slot: &[StationId],
    report: &mut MigrationReport,
) -> Result<BuiltLines, MigrateError> {
    let lp = &legacy.project;
    let mut credits = Credits::default();
    let mut taken: HashSet<String> = HashSet::new();
    let mut final_uuid = HashMap::new();
    let mut seen_lines = HashSet::new();
    let mut lines = Vec::with_capacity(lp.lines.len());
    for line in &lp.lines {
        if line.id > MAX_LINE_ID {
            return Err(MigrateError::LineIdOutOfRange(line.id));
        }
        if !seen_lines.insert(line.id) {
            return Err(MigrateError::DuplicateLineId(line.id));
        }
        let station = |index: usize| {
            slots
                .dense(Slot {
                    line_id: line.id,
                    station_index: index as u32,
                })
                .map(|dense| of_slot[dense])
        };
        let start = station(0).ok_or(MigrateError::DuplicateLineId(line.id))?;
        let mut shots = Vec::with_capacity(line.shots.len());
        for (index, source) in line.shots.iter().enumerate() {
            let usable = source.uuid.as_deref().filter(|u| {
                !u.is_empty() && u.chars().count() <= MAX_SHOT_UUID_CHARS && !taken.contains(*u)
            });
            let uuid = if let Some(u) = usable {
                u.to_string()
            } else {
                report.shot_uuids_minted += 1;
                mint_uuid(line.id, index, source.uuid.as_deref(), &taken)
            };
            taken.insert(uuid.clone());
            if let Some(old) = &source.uuid {
                final_uuid
                    .entry((line.id, old.clone()))
                    .or_insert_with(|| uuid.clone());
            }
            let to = station(index + 1).ok_or(MigrateError::DuplicateLineId(line.id))?;
            let (shot, sanitized) = convert::shot(source, uuid, to, &mut credits);
            if sanitized {
                report.numbers_sanitized += 1;
            }
            shots.push(shot);
        }
        let quality = if line.quality.is_finite() {
            line.quality
        } else {
            report.numbers_sanitized += 1;
            1.0
        };
        let mut built = Line {
            id: line.id,
            name: line.name.clone(),
            color: line.color.clone(),
            direction: convert::direction(line.direction),
            quality,
            azimuth_reference: line
                .azimuth_reference
                .as_ref()
                .map(convert::azimuth_reference),
            section_id: line.section_id,
            legacy_surveyor: line.legacy_surveyor.clone(),
            legacy_date: line.legacy_date,
            start,
            shots,
            hand_moves: BTreeMap::new(),
            next_shot_id: 0,
        };
        built.assign_session_shot_ids();
        lines.push(built);
    }
    report.lines = lines.len();
    report.shots = lines.iter().map(|l| l.shots.len()).sum();
    report.slots = slots.len();
    Ok((lines, final_uuid))
}

/// A uuid for the shot at `index` of line `line` derived from its place and
/// its old uuid, stepping past any already taken.
fn mint_uuid(line: LineId, index: usize, old: Option<&str>, taken: &HashSet<String>) -> String {
    let name = format!("{line}/{index}/{}", old.unwrap_or(""));
    (0..u32::MAX)
        .map(|n| {
            let seed = if n == 0 {
                name.clone()
            } else {
                format!("{name}#{n}")
            };
            uuid::Uuid::new_v5(&MIGRATED_UUID_NAMESPACE, seed.as_bytes()).to_string()
        })
        .find(|candidate| !taken.contains(candidate))
        .unwrap_or_default()
}

fn build_closures(
    legacy: &LegacySurvey,
    slots: &Slots,
    of_slot: &[StationId],
    merges: &merges::MergePlan,
    project: &mut Project,
    report: &mut MigrationReport,
) -> BTreeMap<ClosureId, bool> {
    let lp = &legacy.project;
    let end = |dense: usize| (of_slot[dense], Some(slots.slot(dense).line_id));
    // (closure, legacy is_aligned of a Move, whether its id must be minted)
    let mut built: Vec<(LoopClosure, Option<bool>, bool)> = Vec::new();
    for (a, b) in merges.ties() {
        let ((sa, va), (sb, vb)) = (end(a), end(b));
        let tie = LoopClosure {
            id: 0,
            a: sa,
            a_via: va,
            b: sb,
            b_via: vb,
            strategy: ClosureStrategy::Tie,
        };
        built.push((tie, None, true));
    }
    let mut used: HashSet<ClosureId> = HashSet::new();
    for closure in &lp.loop_closures {
        let resolve = |key| slots.resolve(key).and_then(|slot| slots.dense(slot));
        let (Some(a), Some(b)) = (resolve(&closure.station_a), resolve(&closure.station_b)) else {
            report.closures_dropped_dangling += 1;
            continue;
        };
        let ((sa, va), (sb, vb)) = (end(a), end(b));
        if sa == sb {
            report.closures_dropped_one_station += 1;
            continue;
        }
        let strategy = if closure.strategy != old::ClosureStrategy::Move && closure.is_aligned {
            report.aligned_closures_to_tie += 1;
            ClosureStrategy::Tie
        } else {
            convert::closure_strategy(closure.strategy)
        };
        let keep_id = closure.id != 0 && used.insert(closure.id);
        if !keep_id {
            report.closure_ids_reminted += 1;
        }
        let snap = (strategy == ClosureStrategy::Move).then_some(closure.is_aligned);
        let migrated = LoopClosure {
            id: closure.id,
            a: sa,
            a_via: va,
            b: sb,
            b_via: vb,
            strategy,
        };
        built.push((migrated, snap, !keep_id));
    }
    let mut next = used
        .iter()
        .map(|id| id.saturating_add(1))
        .max()
        .unwrap_or(1)
        .max(lp.next_closure_id)
        .max(1);
    let mut snaps = BTreeMap::new();
    for (mut closure, snap, mint) in built {
        if mint {
            closure.id = next;
            next = next.saturating_add(1);
        }
        if let Some(aligned) = snap {
            snaps.insert(closure.id, aligned);
        }
        project.closures.push(closure);
    }
    project.counters.next_closure_id = next;
    snaps
}

/// Give every component its anchor (plan 7.2.5) and return, per component,
/// the dense legacy slot whose legacy position the anchor holds.
fn build_anchors(
    legacy: &LegacySurvey,
    slots: &Slots,
    of_slot: &[StationId],
    datum_slots: &[usize],
    project: &mut Project,
    report: &mut MigrationReport,
) -> Vec<usize> {
    let lp = &legacy.project;
    let topology = project.topology_arc();
    let placement_datums = project.placement_datums();
    // A legacy position that is not finite (a corrupt reading the old build
    // drew as NaN) cannot anchor anything: the origin stands in.
    let legacy_xy = |dense: usize| {
        legacy
            .raw_positions
            .get(&slots.slot(dense))
            .map(|p| [p[0], p[1]])
            .filter(|p| p[0].is_finite() && p[1].is_finite())
            .unwrap_or([0.0, 0.0])
    };
    let dense_of = |sr: old::StationRef| slots.dense(Slot::from(sr));

    // The legacy gauge of each datum-less least-squares component, by the
    // migrated component its station landed in.
    let mut gauges: BTreeMap<usize, (usize, ComponentHold)> = BTreeMap::new();
    for anchor in legacy.datumless_gauges() {
        if let Some(dense) = dense_of(anchor.station) {
            if let Some(c) = topology.component_of(of_slot[dense]) {
                gauges.entry(c).or_insert((dense, anchor.hold));
            }
        }
    }

    let mut anchor_slots = Vec::with_capacity(topology.components().len());
    for (c, component) in topology.components().iter().enumerate() {
        if let Some(d) = placement_datums[c] {
            let dense = datum_slots[d];
            let correction = project
                .correction_to_hold(d, legacy_xy(dense))
                .filter(|c| c[0].is_finite() && c[1].is_finite());
            if correction.is_some() {
                report.corrections_stored += 1;
            }
            project.datums[d].correction = correction;
            anchor_slots.push(dense);
            continue;
        }
        let in_component = |dense: usize| topology.component_of(of_slot[dense]) == Some(c);
        let held = gauges.get(&c).copied().or_else(|| {
            let lines: HashSet<LineId> = component.lines.iter().copied().collect();
            let (sr, hold) = lp.anchor_for_lines(&lines)?;
            dense_of(sr)
                .filter(|&dense| in_component(dense))
                .map(|dense| (dense, hold))
        });
        let dense = match held {
            Some((dense, ComponentHold::HeldGauge)) => {
                report.placements_held_gauge += 1;
                dense
            }
            Some((dense, _)) => {
                report.placements_default_gauge += 1;
                dense
            }
            None => {
                report.placements_fallback += 1;
                let lowest = component.lowest_station().unwrap_or_default();
                topology
                    .first_slot(lowest)
                    .and_then(|slot| {
                        slots.dense(Slot {
                            line_id: slot.line_id,
                            station_index: slot.station_index,
                        })
                    })
                    .unwrap_or_default()
            }
        };
        let [x, y] = legacy_xy(dense);
        project.placements.push(Placement {
            station: of_slot[dense],
            x,
            y,
        });
        anchor_slots.push(dense);
    }
    anchor_slots
}

/// Invariant 7: a closure may join two pieces only when both hold a datum.
/// The 0.6.0 load promotes every other such closure to a merge, so this only
/// fires on a file the legacy load could not settle; the closure is dropped.
fn drop_unanchored_closures(project: &mut Project, report: &mut MigrationReport) {
    let topology = project.topology_arc();
    let held: Vec<bool> = project
        .placement_datums()
        .iter()
        .map(Option::is_some)
        .collect();
    let before = project.closures.len();
    project.closures.retain(|closure| {
        match (
            topology.component_of(closure.a),
            topology.component_of(closure.b),
        ) {
            (Some(ca), Some(cb)) => ca == cb || (held[ca] && held[cb]),
            _ => false,
        }
    });
    report.closures_dropped_unanchored += before - project.closures.len();
    report.closures_kept = project.closures.len();
}

/// Plan 7.2.6 / 3.8: a placement datum is an entrance; a GPS fix follows its
/// override, else is an entrance when its station is the first or last of
/// its `via` line (of its station's first line without one). Where that
/// differs from what 0.6.0 derived for the datum, write 0.6.0's answer as
/// an explicit override, so no entrance changes.
fn write_entrance_overrides(
    lp: &old::Project,
    datum_sources: &[usize],
    project: &mut Project,
    report: &mut MigrationReport,
) {
    let legacy = lp.datum_entrances();
    let topology = project.topology_arc();
    let placement: BTreeSet<usize> = project.placement_datums().into_iter().flatten().collect();
    for (index, &source) in datum_sources.iter().enumerate() {
        let Some(old_value) = legacy.get(source).copied().flatten() else {
            continue;
        };
        let datum = project.datums[index];
        let new_value = if placement.contains(&index) {
            true
        } else if let Some(answer) = datum.entrance {
            answer
        } else {
            let line = datum
                .via
                .or_else(|| topology.lines_of(datum.station).first().copied());
            line.and_then(|line| {
                let slot = topology.slot_on(datum.station, line)?;
                let chain = topology.chain(line)?;
                Some(slot.station_index == 0 || slot.station_index as usize + 1 == chain.len())
            })
            .unwrap_or(false)
        };
        if new_value != old_value {
            project.datums[index].entrance = Some(old_value);
            report.entrance_overrides_written += 1;
        }
    }
}

/// Plan 5.3: a station's name is unique on every line it is on. Legacy names
/// were unique per line, so only a junction (a merged group named after its
/// survivor) can collide; it takes the first free letter suffix.
fn uniquify_junction_names(project: &mut Project, report: &mut MigrationReport) {
    let topology = project.topology_arc();
    let junctions: Vec<StationId> = topology
        .stations()
        .iter()
        .copied()
        .filter(|&s| topology.is_junction(s))
        .collect();
    for station in junctions {
        if project.uniquify_station_name(station).is_some() {
            report.stations_renamed += 1;
        }
    }
}
