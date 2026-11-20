//! The legacy slot space: every `(line, station index)` of the settled legacy
//! project, numbered densely in slot order (lines in list order, then index),
//! and the resolution of the legacy `StationKey`-keyed side maps onto it.

use std::collections::{BTreeMap, HashMap};

use stalagmight_legacy::models::{self as old, LineId, StationAnchor, StationKey};
use stalagmight_legacy::{LegacyProject, Slot};

/// The slot space of one legacy project.
pub(super) struct Slots {
    /// Per line, in list order: its id and its first dense slot.
    lines: Vec<(LineId, usize)>,
    /// Total number of slots.
    count: usize,
    /// Line id -> list position (the first line with that id, as the legacy
    /// `get_line` finds it).
    position: HashMap<LineId, usize>,
    /// Per line position: shot uuid -> the first index holding it.
    uuids: Vec<HashMap<String, usize>>,
}

impl Slots {
    pub(super) fn new(project: &LegacyProject) -> Self {
        let mut lines = Vec::with_capacity(project.lines.len());
        let mut position = HashMap::with_capacity(project.lines.len());
        let mut uuids = Vec::with_capacity(project.lines.len());
        let mut count = 0;
        for (i, line) in project.lines.iter().enumerate() {
            lines.push((line.id, count));
            position.entry(line.id).or_insert(i);
            let mut by_uuid = HashMap::with_capacity(line.shots.len());
            for (index, shot) in line.shots.iter().enumerate() {
                if let Some(uuid) = &shot.uuid {
                    by_uuid.entry(uuid.clone()).or_insert(index);
                }
            }
            uuids.push(by_uuid);
            count += line.shots.len() + 1;
        }
        Self {
            lines,
            count,
            position,
            uuids,
        }
    }

    /// Number of slots.
    pub(super) fn len(&self) -> usize {
        self.count
    }

    /// Number of lines.
    pub(super) fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// The list position of line `id`.
    pub(super) fn line_position(&self, id: LineId) -> Option<usize> {
        self.position.get(&id).copied()
    }

    /// Number of stations on the line at list position `position`.
    pub(super) fn stations_on(&self, position: usize) -> usize {
        let start = self.lines[position].1;
        let end = self
            .lines
            .get(position + 1)
            .map_or(self.count, |&(_, next)| next);
        end - start
    }

    /// The dense index of a slot, `None` when it is not in the project.
    pub(super) fn dense(&self, slot: Slot) -> Option<usize> {
        let position = self.line_position(slot.line_id)?;
        let index = slot.station_index as usize;
        (index < self.stations_on(position)).then(|| self.lines[position].1 + index)
    }

    /// The slot at a dense index.
    pub(super) fn slot(&self, dense: usize) -> Slot {
        let position = self
            .lines
            .partition_point(|&(_, first)| first <= dense)
            .saturating_sub(1);
        let (line_id, first) = self.lines[position];
        Slot {
            line_id,
            station_index: (dense - first) as u32,
        }
    }

    /// The list position of the line holding a dense slot.
    pub(super) fn line_position_of(&self, dense: usize) -> usize {
        self.lines
            .partition_point(|&(_, first)| first <= dense)
            .saturating_sub(1)
    }

    /// Resolve a stored key exactly as the legacy `Project::resolve_key`:
    /// `LineStart` is index 0, `AfterShot(uuid)` the station after the first
    /// shot of the line holding that uuid, `Index(i)` any index up to the
    /// line's last station.
    pub(super) fn resolve(&self, key: &StationKey) -> Option<Slot> {
        let position = self.line_position(key.line_id)?;
        let index = match &key.anchor {
            StationAnchor::LineStart => 0,
            StationAnchor::AfterShot(uuid) => self.uuids[position].get(uuid)? + 1,
            StationAnchor::Index(i) => {
                let i = *i as usize;
                if i >= self.stations_on(position) {
                    return None;
                }
                i
            }
        };
        Some(Slot {
            line_id: key.line_id,
            station_index: index as u32,
        })
    }

    /// The label-representative rank of a slot (lower is better): not its
    /// line's first station, then not its last, then line list order, then
    /// index. The map's and the image export's rule for which member of a
    /// merge group names it (`labelMergeGroups.ts`, `point_representatives`).
    pub(super) fn rank(&self, dense: usize) -> (bool, bool, usize, usize) {
        let position = self.line_position_of(dense);
        let index = dense - self.lines[position].1;
        let last = self.stations_on(position) - 1;
        (index == 0, index == last, position, index)
    }
}

/// A legacy `StationKey`-keyed map resolved onto slots.
pub(super) struct Resolved<V> {
    pub(super) by_slot: BTreeMap<usize, V>,
    /// Keys that resolve to no slot.
    pub(super) dangling: usize,
    /// Keys dropped because another key of the map resolved to the same slot.
    pub(super) duplicates: usize,
}

/// Resolve every key of `map` to its dense slot. Where several keys land on
/// one slot (only a hand-edited or pre-anchor file can hold that), the
/// canonical key (`key_for_ref`) wins, then the lowest key by its text, so
/// the result never depends on hash order.
pub(super) fn resolve_map<V: Clone>(
    project: &LegacyProject,
    slots: &Slots,
    map: &HashMap<StationKey, V>,
) -> Resolved<V> {
    let mut found: BTreeMap<usize, Vec<(&StationKey, &V)>> = BTreeMap::new();
    let mut dangling = 0;
    for (key, value) in map {
        match slots.resolve(key).and_then(|slot| slots.dense(slot)) {
            Some(dense) => found.entry(dense).or_default().push((key, value)),
            None => dangling += 1,
        }
    }
    let mut duplicates = 0;
    let by_slot = found
        .into_iter()
        .filter_map(|(dense, mut candidates)| {
            if candidates.len() > 1 {
                duplicates += candidates.len() - 1;
                let canonical = project.key_for_ref(old::StationRef::from(slots.slot(dense)));
                candidates.sort_by_cached_key(|(key, _)| {
                    (Some(*key) != canonical.as_ref(), key.to_string())
                });
            }
            candidates
                .into_iter()
                .next()
                .map(|(_, value)| (dense, value.clone()))
        })
        .collect();
    Resolved {
        by_slot,
        dangling,
        duplicates,
    }
}
