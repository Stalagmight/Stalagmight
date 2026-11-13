//! Keeping the least-squares gauge on one physical station across the edits
//! that move stations between sections (Split Section Here, Combine Sections).
//!
//! The gauge is named by a pair of shots ([`Section::gauge_shot_uuid`], read
//! by `Project::gauge_index`). An edit that moves the gauge station's shots
//! into another section, or cuts the neighbour shot off, would leave a pair
//! that names a different station, or none, and the adjusted component would
//! translate by the adjustment residual. [`gauge_pair_for_station`] is the
//! inverse of `gauge_index`: the pair that names a given station.
//!
//! A section's own gauge is not the whole story. A datum-less component is
//! held fixed at the gauge station of its LOWEST-ID section
//! (`default_anchor_for_component`), and these edits change which sections a
//! component holds: a combine retires a section id, a split or a
//! delete-and-split mints one. So every such edit brackets itself with
//! [`Project::hold_component_gauges`] and [`Project::restore_component_gauges`],
//! which record the physical station each touched component was held at and
//! put it back on the section that anchors the component afterwards. When
//! that section does not hold the station (a third section with an id
//! between the two combined ones, or a split whose anchor station went to the
//! new half while the halves stay connected), the station is recorded in
//! [`Project::held_gauges`] instead, which `default_anchor_for_component`
//! reads before the lowest-id rule.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{Section, SectionId, StationRef};

use super::Project;

/// A station named by the shots it touches in `section_id`: the
/// least-squares gauge an edit kept for a component whose anchoring section
/// does not hold it (see the module doc and [`Project::held_gauges`]).
///
/// The same naming as a section's gauge pair: with `neighbour` in the section
/// the station is the end of `shot` facing away from it, which a reverse
/// carries intact; without it, `end` says which end of `shot` (its start or
/// its end) as the record stood when `parity` (the shot's record parity,
/// [`Project::shot_record_parity`]) was read, and a reverse of the record since
/// flips it.
///
/// Scoped by section, like a station key: a file saved before shot uuids were
/// unique across the project can hold one uuid in two sections, and only the
/// section says which copy is meant (the load's rename follows it through
/// `apply_shot_uuid_remap`). It also makes a lookup one probe of the shot uuid
/// index. Every edit that moves shots to another section re-records the held
/// gauge of each component it touches.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HeldGauge {
    pub section_id: SectionId,
    pub shot: String,
    #[serde(default)]
    pub neighbour: Option<String>,
    pub end: bool,
    pub parity: bool,
}

/// The stations an edit's touched components were held at, recorded before
/// the edit by [`Project::hold_component_gauges`].
#[derive(Debug, Clone, Default)]
pub struct ComponentGauges {
    /// One per touched component, in the order of the component's lowest
    /// section id (so when an edit joins two, the one the lowest-id rule would
    /// have picked is restored first and keeps the joined component).
    held: Vec<HeldStation>,
}

#[derive(Debug, Clone)]
enum HeldStation {
    /// Named by its shots: the station may move to another section or index.
    Shots(HeldGauge),
    /// A shot-less section's single station, in a section the edit does not
    /// change: its reference survives as is.
    Untouched(StationRef),
}

/// A section's recorded gauge pair: `(gauge_shot_uuid, gauge_neighbour_uuid)`.
pub(super) type GaugePair = (Option<String>, Option<String>);

/// `section`'s gauge pair when a save captured one, else `None`.
pub(super) fn captured_gauge(section: &Section) -> Option<GaugePair> {
    section.gauge_shot_uuid.as_ref()?;
    Some((
        section.gauge_shot_uuid.clone(),
        section.gauge_neighbour_uuid.clone(),
    ))
}

/// The gauge pair `(gauge_shot_uuid, gauge_neighbour_uuid)` that names station
/// `index` of `section`, or `None` when no pair can.
///
/// Prefers a two-shot pair, which a reverse carries intact and an extension at
/// either end does not disturb:
/// - the start of `shots[index]`, facing away from `shots[index + 1]`;
/// - else the end of `shots[index - 1]`, facing away from `shots[index - 2]`.
///
/// Only a section of one or two shots can lack both. There a lone shot names
/// the station, and its side comes from [`Section::reverse_parity`] (the rule
/// `gauge_index` applies to a pair with no neighbour): the start of the shot
/// while the parity is false, its end while it is true. A one-shot section
/// whose parity points at the other end cannot name `index` at all.
///
/// A shot without a uuid (a pre-v6 file before its first save) cannot be named
/// either.
pub(crate) fn gauge_pair_for_station(
    section: &Section,
    index: usize,
) -> Option<(String, Option<String>)> {
    let n = section.shots.len();
    if n == 0 || index > n {
        return None;
    }
    let uuid = |i: usize| section.shots.get(i).and_then(|s| s.uuid.clone());
    if index + 1 < n {
        return Some((uuid(index)?, Some(uuid(index + 1)?)));
    }
    if index >= 2 {
        return Some((uuid(index - 1)?, Some(uuid(index - 2)?)));
    }
    if section.reverse_parity {
        let shot = index.checked_sub(1)?;
        Some((uuid(shot)?, None))
    } else if index < n {
        Some((uuid(index)?, None))
    } else {
        None
    }
}

/// What a cut needs to keep a section's orientation and least-squares gauge,
/// read BEFORE its shots move: its reverse parity and the gauge station it
/// uses, with the pair a save captured for it, if any.
#[derive(Debug, Clone)]
pub struct CutOrientation {
    reverse_parity: bool,
    gauge_station: usize,
    gauge_capture: Option<GaugePair>,
}

impl Project {
    /// Name station `at` by its shots ([`HeldGauge`]); `None` when it has no
    /// shot, or a shot without a uuid.
    pub fn held_gauge_at(&self, at: StationRef) -> Option<HeldGauge> {
        let section = self.get_section(at.section_id)?;
        let (n, i) = (section.shots.len(), at.station_index as usize);
        if n == 0 || i > n {
            return None;
        }
        let uuid = |i: usize| section.shots.get(i).and_then(|s| s.uuid.clone());
        // The station is the start of `shots[i]`, or the end of `shots[i - 1]`.
        let (shot, end, neighbour) = if i + 1 < n {
            (uuid(i)?, false, uuid(i + 1))
        } else if i >= 2 {
            (uuid(i - 1)?, true, uuid(i - 2))
        } else if i < n {
            (uuid(i)?, false, None)
        } else {
            (uuid(i - 1)?, true, None)
        };
        let parity = self.shot_record_parity(section, Some(&shot));
        Some(HeldGauge {
            section_id: at.section_id,
            shot,
            neighbour,
            end,
            parity,
        })
    }

    /// Where shot `uuid` sits in `section`: one probe of the shot uuid index,
    /// checked against the section (a stale or missing entry falls back to a
    /// scan of that section only, as `resolve_key` does).
    fn shot_position(&self, section: &Section, uuid: &str) -> Option<usize> {
        if let Some(&i) = self.shot_uuid_index.get(&(section.id, uuid.to_string())) {
            if section
                .shots
                .get(i as usize)
                .and_then(|s| s.uuid.as_deref())
                == Some(uuid)
            {
                return Some(i as usize);
            }
        }
        section
            .shots
            .iter()
            .position(|s| s.uuid.as_deref() == Some(uuid))
    }

    /// Where `held` names a station now, in its own section; `None` when that
    /// section no longer holds its shot.
    pub(crate) fn resolve_held_gauge(&self, held: &HeldGauge) -> Option<StationRef> {
        let section = self.get_section(held.section_id)?;
        self.place_held_gauge(held, section)
    }

    /// Where `held` names a station now, in whichever section holds its shot:
    /// its own section first, else the first that does. For the stations an
    /// edit records before it moves shots between sections
    /// ([`Self::hold_component_gauges`]); at most one per touched component,
    /// once per edit.
    fn resolve_held_gauge_anywhere(&self, held: &HeldGauge) -> Option<StationRef> {
        self.resolve_held_gauge(held).or_else(|| {
            self.sections.iter().find_map(|section| {
                section
                    .shots
                    .iter()
                    .any(|s| s.uuid.as_deref() == Some(held.shot.as_str()))
                    .then(|| self.place_held_gauge(held, section))
                    .flatten()
            })
        })
    }

    /// The station `held` names in `section`, if `section` holds its shot.
    fn place_held_gauge(&self, held: &HeldGauge, section: &Section) -> Option<StationRef> {
        let px = self.shot_position(section, &held.shot)?;
        let neighbour = held
            .neighbour
            .as_deref()
            .and_then(|n| self.shot_position(section, n));
        let index = match neighbour {
            Some(py) if py < px => px + 1,
            Some(_) => px,
            None => {
                // A reverse of the record since flips which end `end` names.
                let flipped = held.parity != self.shot_record_parity(section, Some(&held.shot));
                if held.end ^ flipped {
                    px + 1
                } else {
                    px
                }
            }
        };
        Some(StationRef::new(section.id, index as u32))
    }

    /// Whether `held` still names a station (its shot is in the project).
    pub fn held_gauge_resolves(&self, held: &HeldGauge) -> bool {
        self.resolve_held_gauge(held).is_some()
    }

    /// Drop every held gauge that no longer names a station (its section no
    /// longer holds its shot). The save writes only the ones that resolve: a
    /// dangling entry anchors nothing. One index probe per entry.
    pub fn prune_held_gauges(&mut self) {
        let keep: Vec<bool> = self
            .held_gauges
            .iter()
            .map(|held| self.resolve_held_gauge(held).is_some())
            .collect();
        let mut keep = keep.into_iter();
        self.held_gauges.retain(|_| keep.next().unwrap_or(false));
    }

    /// Bring `held_gauges` to what the edits keep it at: entries that name a
    /// station, at most ONE per connected component (the first; the anchor
    /// lookup reads only the first anyway), so never more than there are
    /// sections. Run by the load after its uuid heals, and by Add and Merge
    /// projects after they append a project's entries: a file is untrusted, and
    /// a crafted list would otherwise cost every adjustment and every edit.
    ///
    /// Linear: one index probe per entry, and the components labelled once
    /// with a union-find over the same edges `get_connected_section_ids` walks
    /// (every merge and every loop closure).
    pub fn normalize_held_gauges(&mut self) {
        if self.held_gauges.is_empty() {
            return;
        }
        let mut parent: std::collections::HashMap<SectionId, SectionId> =
            self.sections.iter().map(|s| (s.id, s.id)).collect();
        fn root(
            parent: &mut std::collections::HashMap<SectionId, SectionId>,
            id: SectionId,
        ) -> SectionId {
            let mut at = id;
            while let Some(&up) = parent.get(&at) {
                if up == at {
                    break;
                }
                at = up;
            }
            let top = at;
            let mut at = id;
            while let Some(&up) = parent.get(&at) {
                if up == top {
                    break;
                }
                parent.insert(at, top);
                at = up;
            }
            top
        }
        let edges: Vec<(SectionId, SectionId)> = self
            .station_merges
            .iter()
            .map(|m| (m.station_a.section_id, m.station_b.section_id))
            .chain(
                self.loop_closures
                    .iter()
                    .map(|c| (c.station_a.section_id, c.station_b.section_id)),
            )
            .collect();
        for (a, b) in edges {
            if !(parent.contains_key(&a) && parent.contains_key(&b)) {
                continue;
            }
            let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
            if ra != rb {
                parent.insert(ra.max(rb), ra.min(rb));
            }
        }
        let mut seen: HashSet<SectionId> = HashSet::new();
        let keep: Vec<bool> = self
            .held_gauges
            .iter()
            .map(|held| {
                self.resolve_held_gauge(held).is_some()
                    && seen.insert(root(&mut parent, held.section_id))
            })
            .collect();
        let mut keep = keep.into_iter();
        self.held_gauges.retain(|_| keep.next().unwrap_or(false));
    }

    /// Record, before an edit that changes `sections`, the station each of
    /// their components is held at by the least-squares adjustment when it
    /// has no zero datum (`default_anchor_for_component`). Pair it with
    /// [`Self::restore_component_gauges`] once the edit is done.
    ///
    /// Recorded whether or not the component has a datum or a closure today:
    /// the edit should not change which station a later adjustment holds
    /// either. A station that cannot be named by its shots (a shot without a
    /// uuid) in a section the edit changes is not recorded, and its component
    /// falls back to the lowest-id rule.
    pub fn hold_component_gauges(&self, sections: &[SectionId]) -> ComponentGauges {
        let mut components: Vec<(SectionId, HashSet<SectionId>)> = Vec::new();
        for &id in sections {
            if self.get_section(id).is_none() || components.iter().any(|(_, c)| c.contains(&id)) {
                continue;
            }
            let component = self.get_connected_section_ids(id);
            let lowest = component.iter().copied().min().unwrap_or(id);
            components.push((lowest, component));
        }
        components.sort_by_key(|(lowest, _)| *lowest);
        let held = components
            .iter()
            .filter_map(|(_, component)| {
                let at = self.default_anchor_for_component(component)?;
                match self.held_gauge_at(at) {
                    Some(held) => Some(HeldStation::Shots(held)),
                    None if !sections.contains(&at.section_id) => Some(HeldStation::Untouched(at)),
                    None => {
                        tracing::warn!(
                            ?at,
                            "the gauge station cannot be named by its shots; its component \
                             falls back to the lowest-id rule after this edit"
                        );
                        None
                    }
                }
            })
            .collect();
        ComponentGauges { held }
    }

    /// After the edit [`Self::hold_component_gauges`] recorded `before` for:
    /// hold every component that still contains one of the recorded stations
    /// at that station. The section that now anchors the component (its
    /// lowest id) gets its gauge put on the station when it holds it;
    /// otherwise, or when no gauge pair can name the station there, the
    /// station is recorded in [`Self::held_gauges`]. Any held gauge the
    /// component had before is replaced.
    ///
    /// A recorded station the edit deleted (a delete-and-split of the shots
    /// around it) holds nothing: its component falls back to the lowest-id
    /// rule. When an edit joined two recorded components into one, the first
    /// (the one with the lower lowest id, which the lowest-id rule would have
    /// picked had they been joined all along) keeps the joined component.
    pub fn restore_component_gauges(&mut self, before: &ComponentGauges) {
        let mut claimed: Vec<HashSet<SectionId>> = Vec::new();
        for held in &before.held {
            let at = match held {
                HeldStation::Shots(held) => self.resolve_held_gauge_anywhere(held),
                HeldStation::Untouched(at) => Some(*at),
            };
            let Some(at) = at else {
                tracing::warn!(
                    "the edit removed its component's gauge station; the component falls back \
                     to the lowest-id rule"
                );
                continue;
            };
            if claimed.iter().any(|c| c.contains(&at.section_id)) {
                continue;
            }
            let component = self.get_connected_section_ids(at.section_id);
            self.held_gauges
                .retain(|g| !component.contains(&g.section_id));
            let lowest = component.iter().copied().min().unwrap_or(at.section_id);
            if at.section_id == lowest {
                self.keep_gauge_on_station(lowest, at.station_index as usize, None);
            }
            if self.default_anchor_for_component(&component) != Some(at) {
                if let Some(held) = self.held_gauge_at(at) {
                    self.held_gauges.push(held);
                } else {
                    tracing::warn!(
                        ?at,
                        "the gauge station cannot be named by its shots; its component falls \
                         back to the lowest-id rule"
                    );
                }
            }
            claimed.push(component);
        }
        // An entry whose shots this edit moved to another section names
        // nothing now (its component was re-recorded above).
        self.prune_held_gauges();
    }

    /// See [`CutOrientation`]; `None` when the section does not exist.
    pub fn cut_orientation(&self, section_id: SectionId) -> Option<CutOrientation> {
        let section = self.get_section(section_id)?;
        Some(CutOrientation {
            reverse_parity: section.reverse_parity,
            gauge_station: Self::gauge_index(section),
            gauge_capture: captured_gauge(section),
        })
    }

    /// After a cut turned `first` into two sections - `first` keeping the
    /// original stations `0..=first_end`, and the new `second` holding the
    /// original stations from `second_start` on - give `second` the section's
    /// reverse parity and keep the gauge on its physical station.
    ///
    /// The parity is what a single-neighbour dive-plan pin and the gauge
    /// fallback read to know which end of a shot they sit on, so a second half
    /// built fresh (parity false) out of a reversed section would put both one
    /// station off. Split Section Here cuts AT a station (`first_end ==
    /// second_start`, the station duplicated); a delete-and-split removes the
    /// shots in between, and a gauge station the delete removed (strictly
    /// between the two) is left to the halves' index fallback.
    pub fn settle_cut_halves(
        &mut self,
        first: SectionId,
        second: SectionId,
        before: &CutOrientation,
        first_end: usize,
        second_start: usize,
    ) {
        if let Some(section) = self.get_section_mut(second) {
            section.reverse_parity = before.reverse_parity;
        }
        let gauge = before.gauge_station;
        let capture = before.gauge_capture.as_ref();
        if gauge <= first_end {
            self.keep_gauge_on_station(first, gauge, capture);
        } else {
            self.clear_unresolvable_gauge(first);
        }
        if gauge >= second_start {
            self.keep_gauge_on_station(second, gauge - second_start, capture);
        }
    }

    /// Keep `section_id`'s gauge on its station `index`.
    ///
    /// `inherited` is the pair of the section the station came from, when that
    /// section was CAPTURED (a save recorded its pair). A captured gauge stays
    /// captured: the inherited pair is kept when it still names `index` here,
    /// else a pair naming it is re-captured with [`gauge_pair_for_station`].
    /// Without a capture to inherit, a section whose index fallback already
    /// names `index` is left uncaptured: an edit captures nothing a save did
    /// not, and the next save captures it as usual.
    ///
    /// When no pair can name the station the section is left uncaptured, so it
    /// falls back to the index rule.
    pub(super) fn keep_gauge_on_station(
        &mut self,
        section_id: SectionId,
        index: usize,
        inherited: Option<&GaugePair>,
    ) {
        let Some(section) = self.get_section_mut(section_id) else {
            return;
        };
        let names_index = |section: &Section| {
            // A pair naming a shot the section no longer holds does not count
            // even when it lands on `index` through the fallback: see
            // `clear_unresolvable_gauge` for why a dangling pair is worse than
            // none.
            pair_resolves(section) && Self::gauge_index(section) == index
        };
        let captured = section.gauge_shot_uuid.is_some();
        if names_index(section) && (captured || inherited.is_none()) {
            return;
        }
        if let Some((shot, neighbour)) = inherited {
            section.gauge_shot_uuid.clone_from(shot);
            section.gauge_neighbour_uuid.clone_from(neighbour);
            if section.gauge_shot_uuid.is_some() && names_index(section) {
                return;
            }
        }
        if let Some((shot, neighbour)) = gauge_pair_for_station(section, index) {
            section.gauge_shot_uuid = Some(shot);
            section.gauge_neighbour_uuid = neighbour;
        } else {
            tracing::warn!(
                section_id,
                index,
                "no gauge pair can name this station; the section falls back to the index \
                 rule"
            );
            section.gauge_shot_uuid = None;
            section.gauge_neighbour_uuid = None;
        }
    }

    /// Clear a gauge pair that names a shot `section_id` no longer holds. A
    /// pair that does not resolve is no pin at all, and it is worse than none:
    /// `capture_missing_gauge_anchors` skips every section that has one, so
    /// the section would never be captured again (see
    /// `reapply_heals_after_restore`).
    pub(super) fn clear_unresolvable_gauge(&mut self, section_id: SectionId) {
        let Some(section) = self.get_section_mut(section_id) else {
            return;
        };
        if !pair_resolves(section) {
            section.gauge_shot_uuid = None;
            section.gauge_neighbour_uuid = None;
        }
    }
}

/// True when every recorded member of `section`'s gauge pair is a shot it
/// holds (an unset member passes).
fn pair_resolves(section: &Section) -> bool {
    let holds = |uuid: &Option<String>| {
        uuid.as_deref().is_none_or(|uuid| {
            section
                .shots
                .iter()
                .any(|shot| shot.uuid.as_deref() == Some(uuid))
        })
    };
    holds(&section.gauge_shot_uuid) && holds(&section.gauge_neighbour_uuid)
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod tests {
    use super::gauge_pair_for_station;
    use crate::{Project, SectionData, SectionId, ShotData};

    fn project(shots: usize, reversed: bool) -> (Project, SectionId) {
        let mut project = Project::new("Gauge".to_string(), None);
        let id = project
            .add_section(SectionData {
                shots: (0..shots)
                    .map(|i| ShotData {
                        azimuth_in: 90.0,
                        azimuth_out: 90.0,
                        length: 5.0 + i as f64,
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            })
            .id;
        if reversed {
            assert!(project.reverse_section(id, false));
        }
        (project, id)
    }

    fn set_pair(project: &mut Project, id: SectionId, pair: (String, Option<String>)) {
        let section = project.get_section_mut(id).unwrap();
        section.gauge_shot_uuid = Some(pair.0);
        section.gauge_neighbour_uuid = pair.1;
    }

    /// Every station the helper names, `gauge_index` reads back, in either
    /// orientation; and the pair keeps naming the same physical station
    /// through a reverse.
    #[test]
    fn the_pair_names_the_station_and_survives_a_reverse() {
        for shots in 1..=5 {
            for reversed in [false, true] {
                for index in 0..=shots {
                    let (mut project, id) = project(shots, reversed);
                    let section = project.get_section(id).unwrap();
                    let Some(pair) = gauge_pair_for_station(section, index) else {
                        assert!(
                            shots == 1,
                            "only a one-shot section can lack a pair ({shots} shots, \
                             station {index}, reversed {reversed})"
                        );
                        continue;
                    };
                    set_pair(&mut project, id, pair);
                    let section = project.get_section(id).unwrap();
                    assert_eq!(
                        Project::gauge_index(section),
                        index,
                        "{shots} shots, station {index}, reversed {reversed}"
                    );
                    assert!(project.reverse_section(id, false));
                    let section = project.get_section(id).unwrap();
                    assert_eq!(
                        Project::gauge_index(section),
                        shots - index,
                        "after a reverse: {shots} shots, station {index}"
                    );
                }
            }
        }
    }

    #[test]
    fn keeping_a_gauge_that_already_resolves_changes_nothing() {
        let (mut project, id) = project(3, false);
        project.keep_gauge_on_station(id, 0, None);
        let section = project.get_section(id).unwrap();
        assert_eq!(
            section.gauge_shot_uuid, None,
            "the fallback already names 0"
        );

        project.keep_gauge_on_station(id, 2, None);
        let section = project.get_section(id).unwrap();
        assert_eq!(Project::gauge_index(section), 2);
        let captured = section.gauge_shot_uuid.clone();
        project.keep_gauge_on_station(id, 2, None);
        assert_eq!(project.get_section(id).unwrap().gauge_shot_uuid, captured);
    }

    /// A captured source stays captured, keeping its own pair when it still
    /// names the station.
    #[test]
    fn an_inherited_capture_is_kept_or_recaptured() {
        let (mut project, id) = project(3, false);
        let section = project.get_section(id).unwrap();
        let pair = gauge_pair_for_station(section, 0).unwrap();
        let inherited = (Some(pair.0.clone()), pair.1.clone());
        project.keep_gauge_on_station(id, 0, Some(&inherited));
        let section = project.get_section(id).unwrap();
        assert_eq!(section.gauge_shot_uuid.as_ref(), Some(&pair.0));
        assert_eq!(section.gauge_neighbour_uuid, pair.1);

        // The same capture cannot name station 3: a pair that can is taken.
        project.keep_gauge_on_station(id, 3, Some(&inherited));
        let section = project.get_section(id).unwrap();
        assert_eq!(Project::gauge_index(section), 3);
        assert_ne!(section.gauge_shot_uuid.as_ref(), Some(&pair.0));
    }
}
