//! Placement suggestions for an import on the station graph: the scoring is
//! `models::placement_suggest` (unchanged); this feeds it the open survey's
//! lines and finds the orphaned pieces an import left.
//!
//! - **Import placement** (`suggest_import_placement` /
//!   `suggest_all_import_placements`): every existing line, scored in its own
//!   north reference (#438).
//! - **Orphans** (#446, #674): a piece (stations joined by shots; a junction
//!   is one station, closures do not join) that holds no datum is an orphan.
//!   Its lines' leaf ends (a terminal station that is no junction, with at
//!   least three free shots before a junction or an aligned snap end) are
//!   matched against the lines of datum-held pieces: any of them when the
//!   import touched the orphan (an Add .stalx into an anchored project), only
//!   the newly imported ones otherwise (a later import never re-raises a
//!   pairing the user already had their chance to act on).

use std::collections::{BTreeSet, HashSet};

use super::AppState;
use crate::config::PlacementSuggestionConfig;
use crate::models::placement_suggest::{
    suggest_all_placements, suggest_placements, CrossLineSuggestion, ExistingLine,
    ImportLineForSuggestion, OrphanPlacementSuggestion, PlacementSuggestion,
};
use crate::models::survey::{Line, Project};
use crate::models::{AzimuthReference, LineId, ShotData, StationId};

/// The longest leaf run an orphan is matched with.
const MAX_LEAF_SHOTS: usize = 5;
/// The fewest free shots a leaf end needs to be matched.
const MIN_FREE_SHOTS: usize = 3;

/// `line` as the matcher reads it: its name (`Line N` when it has none, as
/// 0.6.0), shots and resolved north reference.
fn existing_line(project: &Project, line: &Line) -> ExistingLine {
    ExistingLine {
        id: line.id,
        name: line
            .name
            .clone()
            .unwrap_or_else(|| format!("Line {}", line.id)),
        shots: line
            .shots
            .iter()
            .map(|s| crate::models::Shot::with_uuid(s.id, s.to_shot_data(), Some(s.uuid.clone())))
            .collect(),
        azimuth_reference: project.azimuth_reference_for(line),
    }
}

/// Every line of the survey, for the matcher.
fn existing_lines(project: &Project) -> Vec<ExistingLine> {
    project
        .lines()
        .iter()
        .map(|line| existing_line(project, line))
        .collect()
}

/// Suggest where an import line of `shots` overlaps existing survey.
/// `reference` is what `shots` are measured against; `None` falls back to
/// the project default (the line does not exist yet).
pub(crate) fn suggest_import_placement(
    project: &Project,
    shots: &[ShotData],
    reference: Option<AzimuthReference>,
    config: &PlacementSuggestionConfig,
) -> Vec<PlacementSuggestion> {
    let reference = reference
        .map(AzimuthReference::sanitised)
        .or(project.settings.default_azimuth_reference)
        .unwrap_or_default();
    suggest_placements(shots, reference, &existing_lines(project), config)
}

/// Suggest placements for every import line against the survey and each
/// other. Each line is scored in its own declared reference (re-imposed:
/// it comes straight off the wire), the project default being the fallback.
pub(crate) fn suggest_all_import_placements(
    project: Option<&Project>,
    import_lines: Vec<ImportLineForSuggestion>,
    config: &PlacementSuggestionConfig,
) -> Vec<CrossLineSuggestion> {
    let existing = project.map(existing_lines).unwrap_or_default();
    let fallback = project
        .and_then(|p| p.settings.default_azimuth_reference)
        .unwrap_or_default();
    let import_lines: Vec<ImportLineForSuggestion> = import_lines
        .into_iter()
        .map(|mut line| {
            line.azimuth_reference = line.azimuth_reference.map(AzimuthReference::sanitised);
            line
        })
        .collect();
    suggest_all_placements(&import_lines, fallback, &existing, config)
}

/// The pieces of the survey: for each, its lines and whether it holds a
/// datum.
fn pieces(project: &Project) -> Vec<(Vec<LineId>, bool)> {
    let topology = project.topology();
    let placement = project.placement_datums();
    topology
        .components()
        .iter()
        .enumerate()
        .map(|(c, component)| {
            (
                component.lines.clone(),
                placement.get(c).copied().flatten().is_some(),
            )
        })
        .collect()
}

/// Suggest placements for the leaf ends of orphaned pieces (module docs),
/// best first.
pub(crate) fn suggest_orphan_placements(
    project: &Project,
    newly_imported: &[LineId],
    config: &PlacementSuggestionConfig,
) -> Vec<OrphanPlacementSuggestion> {
    if newly_imported.is_empty() || project.datums.is_empty() {
        return Vec::new();
    }
    let pieces = pieces(project);
    let anchored: HashSet<LineId> = pieces
        .iter()
        .filter(|(_, held)| *held)
        .flat_map(|(lines, _)| lines.iter().copied())
        .collect();
    let orphans: Vec<&Vec<LineId>> = pieces
        .iter()
        .filter(|(lines, held)| !held && !lines.is_empty())
        .map(|(lines, _)| lines)
        .collect();
    if orphans.is_empty() || anchored.is_empty() {
        return Vec::new();
    }
    let new: HashSet<LineId> = newly_imported.iter().copied().collect();
    let target = |id: &LineId| project.line(*id).map(|line| existing_line(project, line));
    let new_targets: Vec<ExistingLine> = newly_imported
        .iter()
        .filter(|id| anchored.contains(id))
        .filter_map(target)
        .collect();
    let touched = |lines: &Vec<LineId>| lines.iter().any(|id| new.contains(id));
    let all_targets: Vec<ExistingLine> = if orphans.iter().any(|lines| touched(lines)) {
        project
            .lines()
            .iter()
            .filter(|line| anchored.contains(&line.id))
            .map(|line| existing_line(project, line))
            .collect()
    } else {
        Vec::new()
    };

    let topology = project.topology();
    let raw = project.raw_positions();
    let aligned = project.closure_alignment(&raw);
    let snapped: BTreeSet<StationId> = project
        .closures
        .iter()
        .zip(&aligned)
        .filter(|(_, &is_aligned)| is_aligned)
        .flat_map(|(c, _)| [c.a, c.b])
        .collect();
    let constrained = |s: StationId| topology.is_junction(s) || snapped.contains(&s);

    let mut out = Vec::new();
    for lines in orphans {
        let targets = if touched(lines) {
            &all_targets
        } else {
            &new_targets
        };
        if targets.is_empty() {
            continue;
        }
        for &line_id in lines {
            let (Some(line), Some(chain)) = (project.line(line_id), topology.chain(line_id)) else {
                continue;
            };
            let n = line.shots.len();
            if n == 0 {
                continue;
            }
            let name = line
                .name
                .clone()
                .unwrap_or_else(|| format!("Line {line_id}"));
            for (terminal, from_start) in [(0usize, true), (n, false)] {
                if chain
                    .get(terminal)
                    .is_some_and(|&s| topology.is_junction(s))
                {
                    continue;
                }
                // Free shots inward until the far station of a shot is
                // constrained.
                let free = if from_start {
                    (1..=n)
                        .take_while(|&i| chain.get(i).is_some_and(|&s| !constrained(s)))
                        .count()
                } else {
                    (0..n)
                        .rev()
                        .take_while(|&i| chain.get(i).is_some_and(|&s| !constrained(s)))
                        .count()
                };
                if free < MIN_FREE_SHOTS {
                    continue;
                }
                let take = (free - 1).min(MAX_LEAF_SHOTS);
                let leaf: Vec<ShotData> = if from_start {
                    line.shots
                        .iter()
                        .take(take)
                        .map(crate::models::survey::Shot::to_shot_data)
                        .collect()
                } else {
                    line.shots
                        .iter()
                        .rev()
                        .take(take)
                        .map(|s| {
                            let mut data = s.to_shot_data();
                            data.azimuth_in = (data.azimuth_in + 180.0) % 360.0;
                            data.azimuth_out = (data.azimuth_out + 180.0) % 360.0;
                            std::mem::swap(&mut data.depth_in, &mut data.depth_out);
                            data
                        })
                        .collect()
                };
                let reference = project.azimuth_reference_for(line);
                for ps in suggest_placements(&leaf, reference, targets, config) {
                    // Only the terminal end of the leaf is a meaningful match.
                    if ps.import_station_index != 0 {
                        continue;
                    }
                    out.push(OrphanPlacementSuggestion {
                        orphan_line_id: line_id,
                        orphan_line_name: name.clone(),
                        orphan_station_index: terminal,
                        target_line_id: ps.existing_line_id,
                        target_line_name: ps.existing_line_name,
                        target_station_index: ps.existing_station_index,
                        score: ps.score,
                        matched_shots: ps.matched_shots,
                        existing_matched_shots: ps.existing_matched_shots,
                        preferred_merge_offset: ps.preferred_merge_offset,
                        direction: ps.direction,
                        is_partial: ps.is_partial,
                        confidence: ps.confidence,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out
}

/// How many lines sit in pieces that hold no datum, counting only the
/// pieces `newly_imported` touches (so a clean import is not blamed for
/// orphans that were already there, #446).
pub(crate) fn count_unanchored_lines(project: &Project, newly_imported: &[LineId]) -> u32 {
    let new: HashSet<LineId> = newly_imported.iter().copied().collect();
    pieces(project)
        .into_iter()
        .filter(|(lines, held)| !held && lines.iter().any(|id| new.contains(id)))
        .map(|(lines, _)| lines.len() as u32)
        .sum()
}

impl AppState {
    /// [`suggest_import_placement`] on the open project.
    pub fn suggest_import_placement(
        &self,
        shots: &[ShotData],
        reference: Option<AzimuthReference>,
    ) -> Result<Vec<PlacementSuggestion>, String> {
        let config = self.get_config().placement_suggestion;
        self.with_project(|project| {
            Ok(suggest_import_placement(project, shots, reference, &config))
        })
    }

    /// [`suggest_all_import_placements`] on the open project (none open:
    /// import-to-import only, as 0.6.0).
    pub fn suggest_all_import_placements(
        &self,
        import_lines: Vec<ImportLineForSuggestion>,
    ) -> Vec<CrossLineSuggestion> {
        let config = self.get_config().placement_suggestion;
        let project = self.get_project_state();
        suggest_all_import_placements(project.as_ref(), import_lines, &config)
    }

    /// [`suggest_orphan_placements`] on a copy of the open project (the
    /// matching runs outside the lock).
    pub fn suggest_orphan_placements(
        &self,
        newly_imported: &[LineId],
    ) -> Result<Vec<OrphanPlacementSuggestion>, String> {
        let config = self.get_config().placement_suggestion;
        let project = self.get_project_state().ok_or("No project open")?;
        Ok(suggest_orphan_placements(&project, newly_imported, &config))
    }

    /// [`count_unanchored_lines`] on the open project.
    pub fn count_unanchored_lines(&self, newly_imported: &[LineId]) -> Result<u32, String> {
        self.with_project(|project| Ok(count_unanchored_lines(project, newly_imported)))
    }
}
