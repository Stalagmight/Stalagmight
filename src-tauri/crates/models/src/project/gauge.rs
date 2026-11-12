//! Keeping the least-squares gauge on one physical station across the edits
//! that move stations between sections (Split Section Here, Combine Sections).
//!
//! The gauge is named by a pair of shots ([`Section::gauge_shot_uuid`], read
//! by `Project::gauge_index`). An edit that moves the gauge station's shots
//! into another section, or cuts the neighbour shot off, would leave a pair
//! that names a different station, or none, and the adjusted component would
//! translate by the adjustment residual. [`gauge_pair_for_station`] is the
//! inverse of `gauge_index`: the pair that names a given station.

use crate::{Section, SectionId};

use super::Project;

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

impl Project {
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
