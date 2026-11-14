//! Section topology for the Ariane TML export: which section roots the
//! document, how child sections hang off their parents via station merges,
//! and which zero datum an entrance section stands on. Extracted from `tml.rs`
//! (#157 size policy); the writer itself stays there.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::models::{Project, Section, SectionId, StationRef, ZeroDatum};

#[derive(Debug, Clone, Copy)]
pub(super) struct ParentLink {
    pub(super) parent_station: StationRef,
    pub(super) child_station_index: u32,
}

/// Pick a deterministic root using the legacy "lowest section ID wins parent"
/// heuristic so exports without zero datums remain stable.
pub(super) fn choose_default_root_section_id(project: &Project) -> SectionId {
    let mut child_sections: HashSet<SectionId> = HashSet::new();
    for merge in &project.station_merges {
        let child = if merge.station_a.section_id <= merge.station_b.section_id {
            merge.station_b.section_id
        } else {
            merge.station_a.section_id
        };
        child_sections.insert(child);
    }

    project
        .sections
        .iter()
        .map(|s| s.id)
        .find(|id| !child_sections.contains(id))
        .unwrap_or(project.sections[0].id)
}

pub(super) fn station_depth(section: &Section, station_index: usize) -> f64 {
    if section.shots.is_empty() {
        return 0.0;
    }
    if station_index == 0 {
        section.shots[0].depth_in
    } else if station_index >= section.shots.len() {
        section.shots[section.shots.len() - 1].depth_out
    } else {
        section.shots[station_index].depth_in
    }
}

/// The datum each section's entrance START stands on when that section is an
/// entrance: the section's first PLACEMENT datum (mirrors `Project::frame_datum`'s
/// first-that-resolves rule, scoped to one section). A GPS fix never roots an
/// entrance (#741): its merge component already holds a placement datum, which
/// does, and the fix gets a START of its own (`tml_datums`).
pub(super) fn entrance_datums(project: &Project) -> HashMap<SectionId, &ZeroDatum> {
    let mut by_section: HashMap<SectionId, &ZeroDatum> = HashMap::new();
    for datum in project.placement_datums() {
        if let Some(sr) = project.resolve_key(&datum.station_ref) {
            by_section.entry(sr.section_id).or_insert(datum);
        }
    }
    by_section
}

/// Build a parent map from station_merges.
///
/// Returns: child_section_id → (parent_station_ref, child_station_index)
///
/// Parent links are chosen by traversing the merge graph from `root_section_id`.
pub(super) fn build_parent_map(
    project: &Project,
    root_section_id: SectionId,
    entrance_datums: &HashMap<SectionId, &ZeroDatum>,
) -> HashMap<SectionId, ParentLink> {
    #[derive(Debug, Clone, Copy)]
    struct NeighborLink {
        neighbor_section_id: SectionId,
        parent_station: StationRef,
        child_station_index: u32,
    }

    let mut adjacency: HashMap<SectionId, Vec<NeighborLink>> = HashMap::new();
    for merge in &project.station_merges {
        let (Some(a), Some(b)) = (
            project.resolve_key(&merge.station_a),
            project.resolve_key(&merge.station_b),
        ) else {
            continue;
        };
        adjacency
            .entry(a.section_id)
            .or_default()
            .push(NeighborLink {
                neighbor_section_id: b.section_id,
                parent_station: a,
                child_station_index: b.station_index,
            });
        adjacency
            .entry(b.section_id)
            .or_default()
            .push(NeighborLink {
                neighbor_section_id: a.section_id,
                parent_station: b,
                child_station_index: a.station_index,
            });
    }

    for links in adjacency.values_mut() {
        links.sort_by_key(|l| {
            (
                l.neighbor_section_id,
                l.child_station_index,
                l.parent_station.station_index,
            )
        });
    }

    let mut map: HashMap<SectionId, ParentLink> = HashMap::new();
    let mut visited: HashSet<SectionId> = HashSet::new();
    let mut queue = VecDeque::new();
    visited.insert(root_section_id);
    queue.push_back(root_section_id);

    // Traverse from the export root, then from one section per REMAINING merge-connected
    // component. Seeding only from `root_section_id` leaves every section outside that
    // component absent from `map`, and the caller reads absence as "this is an entrance"
    // - so a gallery of sections merged to each other exported as that many disconnected
    // STARTs stacked on one anchor, with the merges between them never applied. One root
    // per component instead: that component's first section is the entrance, and the rest
    // resolve their parent links normally.
    loop {
        while let Some(parent_section_id) = queue.pop_front() {
            let Some(links) = adjacency.get(&parent_section_id) else {
                continue;
            };
            for link in links {
                if visited.insert(link.neighbor_section_id) {
                    map.insert(
                        link.neighbor_section_id,
                        ParentLink {
                            parent_station: link.parent_station,
                            child_station_index: link.child_station_index,
                        },
                    );
                    queue.push_back(link.neighbor_section_id);
                }
            }
        }

        // A component's entrance prefers a section holding a placement datum -
        // the same rule the MAIN root follows via `frame_datum` (#378 review):
        // GPS lands only on entrance STARTs, so rooting a datum-carrying
        // component at some other section would export it anchored to the frame
        // datum's coordinates while its own datum went unwritten. Within each
        // preference tier, `project.sections` order keeps the choice
        // deterministic and the export byte-stable. A section with no merges at
        // all is not in `adjacency` and is left out of the map deliberately: it
        // is an entrance in its own right.
        let candidate = |id: &SectionId| !visited.contains(id) && adjacency.contains_key(id);
        let Some(next_root) = project
            .sections
            .iter()
            .map(|section| section.id)
            .filter(candidate)
            .find(|id| entrance_datums.contains_key(id))
            .or_else(|| {
                project
                    .sections
                    .iter()
                    .map(|section| section.id)
                    .find(candidate)
            })
        else {
            break;
        };

        tracing::debug!(
            section_id = next_root,
            root_section_id,
            "Rooting a merge component that is disconnected from the export root"
        );
        visited.insert(next_root);
        queue.push_back(next_root);
    }

    map
}
