//! Station groups -> station records (plan 7.2.1).
//!
//! Each group of the merge plan is one station. Its **survivor** is the
//! member the map labelled (the label-representative rule, [`Slots::rank`]):
//! the station keeps that member's Station ID and name; every other member's
//! id (and name) becomes an alias. Members' distinct non-empty comments are
//! joined in member order (survivor first, then by rank) with a blank line.
//!
//! Ids are preserved wherever they are usable (below `MAX_STATION_ID` and not
//! claimed by an earlier group); a station left without one is minted a fresh
//! id above every preserved id and the legacy counter.

use std::collections::{BTreeMap, HashSet};

use stalagmight_legacy::models::StationId as LegacyStationId;
use stalagmight_legacy::LegacyProject;

use crate::models::survey::{Station, StationAlias};
use crate::models::{StationId, MAX_STATION_ID};

use super::merges::MergePlan;
use super::slots::{resolve_map, Slots};
use super::{MigrateError, MigrationReport};

/// The stations of the migrated project.
pub(super) struct StationPlan {
    /// Per dense slot: its station.
    pub(super) of_slot: Vec<StationId>,
    pub(super) stations: BTreeMap<StationId, Station>,
    /// Above every id and alias id in use, and the legacy counter.
    pub(super) next_station_id: StationId,
}

/// Build one station per group of `merges`.
pub(super) fn plan(
    project: &LegacyProject,
    slots: &Slots,
    merges: &MergePlan,
    report: &mut MigrationReport,
) -> Result<StationPlan, MigrateError> {
    let ids = resolve_map(project, slots, &project.station_ids);
    let names = resolve_map(project, slots, &project.station_names);
    let comments = resolve_map(project, slots, &project.station_comments);
    for resolved in [ids.dangling, names.dangling, comments.dangling] {
        report.station_keys_dangling += resolved;
    }
    for resolved in [ids.duplicates, names.duplicates, comments.duplicates] {
        report.station_keys_duplicate += resolved;
    }

    // Members per group, survivor first, the rest by rank. Groups are
    // ordered by their survivor's slot.
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); slots.len()];
    for dense in 0..slots.len() {
        members[merges.group[dense]].push(dense);
    }
    let mut groups: Vec<Vec<usize>> = members.into_iter().filter(|g| !g.is_empty()).collect();
    for group in &mut groups {
        group.sort_by_key(|&d| slots.rank(d));
    }
    groups.sort_by_key(|group| group[0]);

    // Claim ids: the survivor's, else the first usable member id; the other
    // usable member ids become aliases.
    let usable = |id: LegacyStationId| id < MAX_STATION_ID;
    let mut claimed: HashSet<StationId> = HashSet::new();
    let mut chosen: Vec<(Option<StationId>, Vec<(StationId, usize)>)> =
        Vec::with_capacity(groups.len());
    for group in &groups {
        let mut survivor = None;
        let mut aliases = Vec::new();
        for &member in group {
            let Some(&id) = ids.by_slot.get(&member) else {
                continue;
            };
            if !usable(id) || claimed.contains(&id) {
                report.station_ids_dropped += 1;
                continue;
            }
            claimed.insert(id);
            if survivor.is_none() {
                survivor = Some(id);
            } else {
                aliases.push((id, member));
            }
        }
        chosen.push((survivor, aliases));
    }

    let mut next = claimed
        .iter()
        .map(|id| id + 1)
        .max()
        .unwrap_or(0)
        .max(project.next_station_id);
    let mut of_slot = vec![0; slots.len()];
    let mut stations = BTreeMap::new();
    for (group, (survivor, aliases)) in groups.iter().zip(chosen) {
        let id = if let Some(id) = survivor {
            id
        } else {
            if next >= MAX_STATION_ID {
                return Err(MigrateError::StationIdsExhausted);
            }
            report.station_ids_minted += 1;
            next += 1;
            next - 1
        };
        let name = group
            .iter()
            .find_map(|member| names.by_slot.get(member).cloned())
            .unwrap_or_else(|| slots.slot(group[0]).station_index.to_string());
        let mut joined: Vec<&str> = Vec::new();
        for member in group {
            if let Some(text) = comments.by_slot.get(member) {
                if !text.is_empty() && !joined.contains(&text.as_str()) {
                    joined.push(text);
                }
            }
        }
        if joined.len() > 1 {
            report.comments_joined += 1;
        }
        let comment = (!joined.is_empty()).then(|| joined.join("\n\n"));
        report.aliases_created += aliases.len();
        let aliases = aliases
            .into_iter()
            .map(|(alias, member)| StationAlias {
                id: alias,
                name: names.by_slot.get(&member).cloned(),
            })
            .collect();
        for &member in group {
            of_slot[member] = id;
        }
        stations.insert(
            id,
            Station {
                id,
                name,
                comment,
                aliases,
                external: None,
            },
        );
    }
    Ok(StationPlan {
        of_slot,
        stations,
        next_station_id: next,
    })
}
