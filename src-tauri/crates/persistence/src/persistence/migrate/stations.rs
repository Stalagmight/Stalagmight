//! Station groups -> station records (plan 7.2.1).
//!
//! Each group of the merge plan is one station. Its **survivor** is the
//! member the map labelled (the label-representative rule, [`Slots::rank`]):
//! the station keeps that member's Station ID and the name 0.6.0 showed for
//! it; every other member becomes an alias naming its line, with its id and
//! the name 0.6.0 showed there, so every line keeps showing the names it
//! showed (plan 5.3 per line, I-3). Members' distinct non-empty comments are
//! joined in member order (survivor first, then by rank) with a blank line.
//!
//! Ids are preserved wherever they are usable (below `MAX_STATION_ID` and not
//! claimed by an earlier group); a station left without one is minted a fresh
//! id above every preserved id and the legacy counter, and an alias left
//! without one is minted one after every station's.

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

/// One group's ids: the station's (`None`: minted), the member whose line
/// shows the station's own name, and the other members with their alias id
/// (`None`: minted).
struct Claim {
    id: Option<StationId>,
    owner: usize,
    aliases: Vec<(Option<StationId>, usize)>,
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
    // members become aliases, with their usable id or (`None`) one minted
    // below. The owner is the member whose id the station takes (else the
    // first member): its line shows the station's own name.
    let usable = |id: LegacyStationId| id < MAX_STATION_ID;
    let mut claimed: HashSet<StationId> = HashSet::new();
    let mut chosen: Vec<Claim> = Vec::with_capacity(groups.len());
    for group in &groups {
        let mut owner: Option<(usize, StationId)> = None;
        let mut aliases: Vec<(Option<StationId>, usize)> = Vec::new();
        for &member in group {
            let id = ids.by_slot.get(&member).copied();
            let id = match id {
                Some(id) if usable(id) && !claimed.contains(&id) => Some(id),
                Some(_) => {
                    report.station_ids_dropped += 1;
                    None
                }
                None => None,
            };
            match (id, owner) {
                (Some(id), None) => {
                    claimed.insert(id);
                    owner = Some((member, id));
                }
                (Some(id), Some(_)) => {
                    claimed.insert(id);
                    aliases.push((Some(id), member));
                }
                (None, _) => aliases.push((None, member)),
            }
        }
        let owner_member = owner.map_or(group[0], |(member, _)| member);
        aliases.retain(|&(_, member)| member != owner_member);
        chosen.push(Claim {
            id: owner.map(|(_, id)| id),
            owner: owner_member,
            aliases,
        });
    }

    let mut next = claimed
        .iter()
        .map(|id| id + 1)
        .max()
        .unwrap_or(0)
        .max(project.next_station_id);
    // The name 0.6.0 showed at a slot: its stored name, else its index.
    let shown = |member: usize| {
        names
            .by_slot
            .get(&member)
            .cloned()
            .unwrap_or_else(|| slots.slot(member).station_index.to_string())
    };
    let mut of_slot = vec![0; slots.len()];
    let mut stations = BTreeMap::new();
    for (group, claim) in groups.iter().zip(&chosen) {
        let id = if let Some(id) = claim.id {
            id
        } else {
            if next >= MAX_STATION_ID {
                return Err(MigrateError::StationIdsExhausted);
            }
            report.station_ids_minted += 1;
            next += 1;
            next - 1
        };
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
        for &member in group {
            of_slot[member] = id;
        }
        stations.insert(
            id,
            Station {
                id,
                name: shown(claim.owner),
                comment,
                aliases: Vec::new(),
                external: None,
            },
        );
    }
    // Every other member's line keeps showing the name 0.6.0 showed there:
    // an alias naming the line, under the member's id, or one minted after
    // every station's (so stations keep the ids they had).
    for claim in &chosen {
        let mut aliases = Vec::with_capacity(claim.aliases.len());
        for &(alias, member) in &claim.aliases {
            let alias = if let Some(id) = alias {
                id
            } else {
                if next >= MAX_STATION_ID {
                    return Err(MigrateError::StationIdsExhausted);
                }
                report.alias_ids_minted += 1;
                next += 1;
                next - 1
            };
            aliases.push(StationAlias {
                id: alias,
                name: Some(shown(member)),
                // The member's line knew the junction by this alias (UX-4,
                // D-migration-upgrade-8).
                line: Some(slots.slot(member).line_id),
            });
        }
        report.aliases_created += aliases.len();
        if let Some(station) = stations.get_mut(&of_slot[claim.owner]) {
            station.aliases = aliases;
        }
    }
    Ok(StationPlan {
        of_slot,
        stations,
        next_station_id: next,
    })
}
