//! Station groups -> station records (plan 7.2.1).
//!
//! Each group of the merge plan is one station. Its **owner** is the member
//! the map labelled (the label-representative rule, [`Slots::rank`]): the
//! station takes that member's Station ID, the name 0.6.0 showed for it and
//! its own comment; every other member becomes a record (an archived
//! station, RS 3) naming its line, with its id, the name 0.6.0 showed there
//! and its own comment. So far every line shows what 0.6.0 showed on it;
//! the assembly then keeps each junction's member on the longest line (rule
//! K, `crate::persistence::kept::keep_longest`), so the owner only decides
//! the ids. No comment is ever joined (plan 2026-11-23 RS 8.3).
//!
//! Ids are preserved wherever they are usable (below `MAX_STATION_ID` and not
//! claimed by an earlier group); a station left without one is minted a fresh
//! id above every preserved id and the legacy counter, and a record left
//! without one is minted one after every station's.

use std::collections::{BTreeMap, HashSet};

use stalagmight_legacy::models::StationId as LegacyStationId;
use stalagmight_legacy::LegacyProject;

use crate::models::survey::{ArchivedStation, Station};
use crate::models::{StationId, MAX_STATION_ID};

use super::merges::MergePlan;
use super::slots::{resolve_map, Slots};
use super::{MigrateError, MigrationReport};

/// The stations of the migrated project.
pub(super) struct StationPlan {
    /// Per dense slot: its station.
    pub(super) of_slot: Vec<StationId>,
    pub(super) stations: BTreeMap<StationId, Station>,
    /// Above every station and record id in use, and the legacy counter.
    pub(super) next_station_id: StationId,
}

/// One group's ids: the station's (`None`: minted), the member whose
/// identity the station takes, and the other members with their record id
/// (`None`: minted).
struct Claim {
    id: Option<StationId>,
    owner: usize,
    records: Vec<(Option<StationId>, usize)>,
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
    // members become records, with their usable id or (`None`) one minted
    // below. The owner is the member whose id the station takes (else the
    // first member): its identity is the station's.
    let usable = |id: LegacyStationId| id < MAX_STATION_ID;
    let mut claimed: HashSet<StationId> = HashSet::new();
    let mut chosen: Vec<Claim> = Vec::with_capacity(groups.len());
    for group in &groups {
        let mut owner: Option<(usize, StationId)> = None;
        let mut records: Vec<(Option<StationId>, usize)> = Vec::new();
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
                    records.push((Some(id), member));
                }
                (None, _) => records.push((None, member)),
            }
        }
        let owner_member = owner.map_or(group[0], |(member, _)| member);
        records.retain(|&(_, member)| member != owner_member);
        chosen.push(Claim {
            id: owner.map(|(_, id)| id),
            owner: owner_member,
            records,
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
    // A member's own comment; an empty one is none, as it always converted.
    let own_comment = |member: usize| {
        comments
            .by_slot
            .get(&member)
            .filter(|text| !text.is_empty())
            .cloned()
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
        for &member in group {
            of_slot[member] = id;
        }
        stations.insert(
            id,
            Station {
                id,
                name: shown(claim.owner),
                comment: own_comment(claim.owner),
                external: None,
                archived: Vec::new(),
            },
        );
    }
    // Every other member is archived in the station: a record naming its
    // line with its own name and comment, under the member's id, or one
    // minted after every station's (so stations keep the ids they had).
    for claim in &chosen {
        let mut records = Vec::with_capacity(claim.records.len());
        for &(record, member) in &claim.records {
            let record = if let Some(id) = record {
                id
            } else {
                if next >= MAX_STATION_ID {
                    return Err(MigrateError::StationIdsExhausted);
                }
                report.record_ids_minted += 1;
                next += 1;
                next - 1
            };
            records.push(ArchivedStation {
                id: record,
                name: shown(member),
                comment: own_comment(member),
                external: None,
                // The member's own line (UX-4, D-migration-upgrade-8).
                into: None,
                lines: vec![slots.slot(member).line_id],
            });
        }
        report.members_archived += records.len();
        if let Some(station) = stations.get_mut(&of_slot[claim.owner]) {
            station.archived = records;
        }
    }
    Ok(StationPlan {
        of_slot,
        stations,
        next_station_id: next,
    })
}
