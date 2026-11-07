use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::config::PlacementSuggestionConfig;

use super::{horizontal_length_from_slope, AzimuthReference, SectionId, Shot, ShotData};

mod crossing;

use crossing::{placement_crossing_clearance_ok, straight_runs, CrossingIndex};

/// Direction of match between import and existing shots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum MatchDirection {
    /// Import shots match existing shots in the same order.
    Forward,
    /// Import shots retrace existing shots backwards.
    Reverse,
}

/// Confidence tier for a placement suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SuggestionConfidence {
    High,
    Medium,
    Low,
}

/// A suggested placement for an import section based on reference shot overlap.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlacementSuggestion {
    /// The existing section that the import overlaps with.
    pub existing_section_id: SectionId,
    /// Station index in the existing section to merge with.
    pub existing_station_index: usize,
    /// Station index in the import section (0 = first, N = last).
    pub import_station_index: usize,
    /// Aggregate match score (higher = better).
    pub score: f64,
    /// Number of shots that matched.
    pub matched_shots: usize,
    /// Number of existing/target shots consumed by the match.
    #[serde(default)]
    pub existing_matched_shots: usize,
    /// Preferred import-side merge offset within the overlap.
    #[serde(default)]
    pub preferred_merge_offset: usize,
    /// Direction of match.
    pub direction: MatchDirection,
    /// Whether the last matched shot was a partial (shorter) match.
    pub is_partial: bool,
    /// Confidence tier based on score.
    pub confidence: SuggestionConfidence,
    /// Human-readable name of the existing section.
    pub existing_section_name: String,
}

/// Whether the target of a cross-section suggestion is an existing project
/// section or another section being imported in the same batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TargetType {
    Existing,
    Importing,
}

/// A placement suggestion that includes source/target metadata for cross-section
/// matching (import-to-existing AND import-to-import).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CrossSectionSuggestion {
    /// Station index in the existing/target section to merge with.
    pub existing_station_index: usize,
    /// Station index in the import section (0 = first, N = last).
    pub import_station_index: usize,
    /// Aggregate match score (higher = better).
    pub score: f64,
    /// Number of shots that matched.
    pub matched_shots: usize,
    /// Number of existing/target shots consumed by the match.
    #[serde(default)]
    pub existing_matched_shots: usize,
    /// Preferred import-side merge offset within the overlap.
    #[serde(default)]
    pub preferred_merge_offset: usize,
    /// Direction of match.
    pub direction: MatchDirection,
    /// Whether the last matched shot was a partial (shorter) match.
    pub is_partial: bool,
    /// Confidence tier based on score.
    pub confidence: SuggestionConfidence,
    /// Human-readable name of the target section.
    pub existing_section_name: String,
    /// Index of the import section this suggestion is for.
    pub import_section_index: u32,
    /// Name of the import section.
    pub import_section_name: String,
    /// Whether the target is an existing project section or another import.
    pub target_type: TargetType,
    /// For existing targets: the section ID.
    pub existing_section_id: Option<SectionId>,
    /// For importing targets: the index of the target import section.
    pub target_import_index: Option<u32>,
}

/// A suggestion for connecting an orphaned section leaf to a newly imported section.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OrphanPlacementSuggestion {
    /// The orphan section ID
    pub orphan_section_id: SectionId,
    /// Human-readable name of the orphan section
    pub orphan_section_name: String,
    /// Station index on the orphan section's leaf end
    pub orphan_station_index: usize,
    /// The target (newly imported) section ID
    pub target_section_id: SectionId,
    /// Human-readable name of the target section
    pub target_section_name: String,
    /// Station index on the target section to merge with
    pub target_station_index: usize,
    /// Aggregate match score (higher = better)
    pub score: f64,
    /// Number of shots that matched
    pub matched_shots: usize,
    /// Number of target shots consumed by the match
    #[serde(default)]
    pub existing_matched_shots: usize,
    /// Preferred import-side merge offset within the overlap
    #[serde(default)]
    pub preferred_merge_offset: usize,
    /// Direction of match
    pub direction: MatchDirection,
    /// Whether the last matched shot was a partial (shorter) match
    pub is_partial: bool,
    /// Confidence tier based on score
    pub confidence: SuggestionConfidence,
}

/// Minimal per-section data needed for the cross-section suggestion algorithm.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ImportSectionForSuggestion {
    pub index: u32,
    pub name: String,
    pub shots: Vec<ShotData>,
    /// What THIS section's readings are measured against (#438), as the
    /// importer that produced them declared it on its `ImportSectionConfig`.
    ///
    /// The matcher compares bearings across the import/existing boundary, so a
    /// section scored in the wrong frame mismatches a correct overlap by the
    /// difference of the two corrections - 5 degrees is already outside the
    /// default `max_azimuth_error_full`. `None` falls back to the project
    /// default, which is what a caller with nothing better to say sends.
    #[serde(default)]
    #[ts(optional)]
    pub azimuth_reference: Option<AzimuthReference>,
}

/// Suggest placements for ALL import sections against BOTH existing project
/// sections AND each other. Returns a flat list sorted by score descending.
///
/// Each import section is scored in ITS OWN frame
/// ([`ImportSectionForSuggestion::azimuth_reference`]); `fallback_reference` is
/// what a section that declares none is read against, normally the project
/// default (#438). Every comparison therefore has both sides in true north:
/// import-to-existing uses each existing section's own reference, and
/// import-to-import uses each import section's.
pub fn suggest_all_placements(
    import_sections: &[ImportSectionForSuggestion],
    fallback_reference: AzimuthReference,
    existing_sections: &[ExistingSection],
    ps_config: &PlacementSuggestionConfig,
) -> Vec<CrossSectionSuggestion> {
    let mut all: Vec<CrossSectionSuggestion> = Vec::new();

    // Use high IDs for import sections so they don't collide with real section IDs.
    // Map: fake_id = FAKE_ID_BASE + import.index
    const FAKE_ID_BASE: SectionId = 0x8000_0000;

    let import_as_existing: Vec<ExistingSection> = import_sections
        .iter()
        .map(|s| ExistingSection {
            id: FAKE_ID_BASE + SectionId::from(s.index),
            name: s.name.clone(),
            shots: s
                .shots
                .iter()
                .enumerate()
                .map(|(i, sd)| Shot::new(i as u32, sd.clone()))
                .collect(),
            azimuth_reference: s.azimuth_reference.unwrap_or(fallback_reference),
        })
        .collect();

    for import in import_sections {
        if import.shots.is_empty() {
            continue;
        }

        let fake_self_id = FAKE_ID_BASE + SectionId::from(import.index);
        let import_reference = import.azimuth_reference.unwrap_or(fallback_reference);

        // Match against existing project sections
        let existing_suggestions = suggest_placements(
            &import.shots,
            import_reference,
            existing_sections,
            ps_config,
        );
        for s in existing_suggestions {
            all.push(CrossSectionSuggestion {
                existing_station_index: s.existing_station_index,
                import_station_index: s.import_station_index,
                score: s.score,
                matched_shots: s.matched_shots,
                existing_matched_shots: s.existing_matched_shots,
                preferred_merge_offset: s.preferred_merge_offset,
                direction: s.direction,
                is_partial: s.is_partial,
                confidence: s.confidence,
                existing_section_name: s.existing_section_name,
                import_section_index: import.index,
                import_section_name: import.name.clone(),
                target_type: TargetType::Existing,
                existing_section_id: Some(s.existing_section_id),
                target_import_index: None,
            });
        }

        // Match against other import sections (exclude self), borrowed
        let other_imports = || import_as_existing.iter().filter(|e| e.id != fake_self_id);

        if other_imports().next().is_some() {
            let import_suggestions = suggest_placements_among(
                &import.shots,
                import_reference,
                other_imports(),
                ps_config,
            );
            for s in import_suggestions {
                // Decode the fake ID back to an import index
                let target_idx = (s.existing_section_id - FAKE_ID_BASE) as u32;
                let target_name = import_sections
                    .iter()
                    .find(|is| is.index == target_idx)
                    .map(|is| is.name.clone())
                    .unwrap_or_default();

                all.push(CrossSectionSuggestion {
                    existing_station_index: s.existing_station_index,
                    import_station_index: s.import_station_index,
                    score: s.score,
                    matched_shots: s.matched_shots,
                    existing_matched_shots: s.existing_matched_shots,
                    preferred_merge_offset: s.preferred_merge_offset,
                    direction: s.direction,
                    is_partial: s.is_partial,
                    confidence: s.confidence,
                    existing_section_name: target_name,
                    import_section_index: import.index,
                    import_section_name: import.name.clone(),
                    target_type: TargetType::Importing,
                    existing_section_id: None,
                    target_import_index: Some(target_idx),
                });
            }
        }
    }

    // Sort by score descending
    all.sort_by(|a, b| score_cmp(b.score, a.score));

    all
}

/// Internal configuration for the suggestion algorithm.
struct SuggestionConfig {
    /// Sigma for azimuth error (degrees).
    azimuth_sigma: f64,
    /// Sigma for length error (meters).
    length_sigma: f64,
    /// Sigma for depth error (meters).
    depth_sigma: f64,
    /// Weight for azimuth score component.
    azimuth_weight: f64,
    /// Weight for length score component.
    length_weight: f64,
    /// Weight for depth_in score component.
    depth_in_weight: f64,
    /// Weight for depth_out score component.
    depth_out_weight: f64,
    /// Minimum per-shot score to continue matching.
    min_shot_score: f64,
    /// Minimum aggregate score to include a suggestion.
    min_aggregate_score: f64,
    /// Maximum number of reference shots to try from each leaf.
    max_reference_shots: usize,
    /// Maximum number of suggestions to return.
    max_suggestions: usize,
    /// Short shot threshold (meters) - shots below this get reduced weight.
    short_shot_threshold: f64,
    /// Penalty multiplier for single-shot matches (0.0-1.0).
    single_shot_penalty: f64,
    /// Hard cutoff: max azimuth error for full shots (degrees).
    max_azimuth_error_full: f64,
    /// Hard cutoff: max depth error per measurement point (meters).
    max_depth_error: f64,
    /// Length error fraction for dynamic cutoff.
    max_length_error_fraction: f64,
    /// Length error floor for dynamic cutoff (meters).
    max_length_error_floor: f64,
    /// Base length error fraction for one-shot overlaps.
    sequence_length_error_base_fraction: f64,
    /// Additional length error fraction allowed for each extra matched shot.
    sequence_length_error_step_fraction: f64,
    /// Maximum length error fraction allowed while exploring longer runs.
    sequence_length_error_max_fraction: f64,
    /// Minimum unpenalized score required for one-shot suggestions.
    single_shot_min_raw_score: f64,
    /// Stricter one-shot azimuth cutoff (degrees).
    single_shot_max_azimuth_error: f64,
    /// Stricter one-shot depth cutoff per measurement point (meters).
    single_shot_max_depth_error: f64,
    /// Stricter one-shot length error fraction.
    single_shot_max_length_error_fraction: f64,
    /// Stricter one-shot length error floor (meters).
    single_shot_max_length_error_floor: f64,
    /// Minimum vertical separation allowed where placed traces cross (meters).
    min_crossing_depth_clearance: f64,
}

impl Default for SuggestionConfig {
    fn default() -> Self {
        Self::from(&PlacementSuggestionConfig::default())
    }
}

impl From<&PlacementSuggestionConfig> for SuggestionConfig {
    fn from(c: &PlacementSuggestionConfig) -> Self {
        Self {
            azimuth_sigma: c.azimuth_sigma,
            length_sigma: c.length_sigma,
            depth_sigma: c.depth_sigma,
            azimuth_weight: c.azimuth_weight,
            length_weight: c.length_weight,
            depth_in_weight: c.depth_in_weight,
            depth_out_weight: c.depth_out_weight,
            min_shot_score: c.min_shot_score,
            min_aggregate_score: c.min_aggregate_score,
            max_reference_shots: c.max_reference_shots as usize,
            max_suggestions: c.max_suggestions as usize,
            short_shot_threshold: c.short_shot_threshold,
            single_shot_penalty: c.single_shot_penalty,
            max_azimuth_error_full: c.max_azimuth_error_full,
            max_depth_error: c.max_depth_error,
            max_length_error_fraction: c.max_length_error_fraction,
            max_length_error_floor: c.max_length_error_floor,
            sequence_length_error_base_fraction: 0.03,
            sequence_length_error_step_fraction: 0.02,
            sequence_length_error_max_fraction: 0.11,
            single_shot_min_raw_score: 0.85,
            single_shot_max_azimuth_error: 8.0,
            single_shot_max_depth_error: 0.30,
            single_shot_max_length_error_fraction: 0.075,
            single_shot_max_length_error_floor: 0.50,
            min_crossing_depth_clearance: 4.0,
        }
    }
}

/// Minimal shot data needed for matching.
#[derive(Clone, Copy)]
struct MatchShot {
    avg_azimuth: f64,
    length: f64,
    depth_in: f64,
    depth_out: f64,
}

#[derive(Clone, Copy)]
struct TracePoint {
    x: f64,
    y: f64,
    depth: f64,
}

impl TracePoint {
    fn translated(self, dx: f64, dy: f64) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            depth: self.depth,
        }
    }
}

impl MatchShot {
    /// `avg_azimuth` is a TRUE-north bearing (#438). The matcher compares an
    /// incoming section against sections already in the project, and those two
    /// can carry different declinations - comparing measured readings across
    /// that boundary would score a correct overlap as a mismatch by the
    /// difference of the corrections.
    fn from_shot(shot: &Shot, reference: AzimuthReference) -> Self {
        Self {
            avg_azimuth: shot.average_azimuth_true(reference),
            length: shot.length,
            depth_in: shot.depth_in,
            depth_out: shot.depth_out,
        }
    }

    fn from_shot_data(shot: &ShotData, reference: AzimuthReference) -> Self {
        Self {
            avg_azimuth: shot.average_azimuth_true(reference),
            length: shot.length,
            depth_in: shot.depth_in,
            depth_out: shot.depth_out,
        }
    }

    /// Reverse this shot (flip azimuth 180°, swap depth_in/depth_out).
    fn reversed(&self) -> Self {
        Self {
            avg_azimuth: (self.avg_azimuth + 180.0) % 360.0,
            length: self.length,
            depth_in: self.depth_out,
            depth_out: self.depth_in,
        }
    }
}

/// [#576 NF-W9PERF-1] The order of the score sorts: `partial_cmp`, made total.
///
/// A NaN reading (an azimuth or length the import carries as NaN) gives a NaN
/// score, and every sort here used `partial_cmp(..).unwrap_or(Equal)`: NaN
/// "equal" to two scores that are not equal to each other is no order at all,
/// and the standard sort panics when it notices ("does not correctly implement
/// a total order"). Two numbers compare exactly as `partial_cmp` compares them
/// (so `-0.0 == 0.0` still, and no finite order moves); a NaN is below every
/// number and equal to another NaN, so the descending sorts put it last.
///
/// Not `f64::total_cmp`, which orders `-0.0` before `0.0` and so could reorder
/// two finite candidates the old sort kept as tied. The rest of the pipeline
/// tolerates a non-finite reading the same way - it proves nothing with it
/// (`crossing::trace_is_finite`) rather than refusing the import.
fn score_cmp(a: f64, b: f64) -> std::cmp::Ordering {
    a.partial_cmp(&b)
        .unwrap_or_else(|| b.is_nan().cmp(&a.is_nan()))
}

/// [#576 NF-W10BE-1] Whether a candidate's score clears the bar to be offered.
///
/// A NaN score never does. Both admission checks used to read
/// `score < min_aggregate_score`, which is false for NaN, so a NaN-scored
/// candidate was offered (sorted last by [`score_cmp`], serialised as JSON
/// null) - and, kept first at a dedupe or endpoint-pair key, it would keep a
/// later finite candidate with that key out, since `finite > NaN` is false too.
/// Filtered here, at admission, it takes no part in the dedupe, the
/// suppressions, the sort or the cap.
fn admitted(score: f64, config: &SuggestionConfig) -> bool {
    score >= config.min_aggregate_score
}

/// Compute the angular difference between two azimuths in [0, 180].
fn angular_difference(a: f64, b: f64) -> f64 {
    let diff = (a - b).rem_euclid(360.0);
    if diff > 180.0 {
        360.0 - diff
    } else {
        diff
    }
}

/// Gaussian falloff: exp(-(error / sigma)^2).
fn gaussian(error: f64, sigma: f64) -> f64 {
    (-((error / sigma).powi(2))).exp()
}

/// Dynamic length cutoff: 3% of existing length, floored at 0.4m.
fn max_length_error(existing_length: f64, config: &SuggestionConfig) -> f64 {
    (config.max_length_error_fraction * existing_length).max(config.max_length_error_floor)
}

fn sequence_length_error_fraction(matched_count: usize, config: &SuggestionConfig) -> f64 {
    let extra = matched_count.saturating_sub(1) as f64;
    (config.sequence_length_error_base_fraction
        + config.sequence_length_error_step_fraction * extra)
        .min(config.sequence_length_error_max_fraction)
}

fn max_sequence_length_error(
    existing_length: f64,
    matched_count: usize,
    config: &SuggestionConfig,
) -> f64 {
    (sequence_length_error_fraction(matched_count, config) * existing_length)
        .max(config.max_length_error_floor)
}

fn max_single_shot_length_error(existing_length: f64, config: &SuggestionConfig) -> f64 {
    (config.single_shot_max_length_error_fraction * existing_length)
        .max(config.single_shot_max_length_error_floor)
}

fn station_depths_from_match_shots(shots: &[MatchShot]) -> Vec<f64> {
    let mut depths = Vec::with_capacity(shots.len() + 1);
    for station_idx in 0..=shots.len() {
        let mut sum = 0.0;
        let mut count = 0.0;
        if station_idx > 0 {
            sum += shots[station_idx - 1].depth_out;
            count += 1.0;
        }
        if station_idx < shots.len() {
            sum += shots[station_idx].depth_in;
            count += 1.0;
        }
        depths.push(if count > 0.0 { sum / count } else { 0.0 });
    }
    depths
}

fn section_trace_points(shots: &[MatchShot]) -> Vec<TracePoint> {
    let depths = station_depths_from_match_shots(shots);
    let mut points = Vec::with_capacity(shots.len() + 1);
    let (mut x, mut y) = (0.0_f64, 0.0_f64);
    points.push(TracePoint {
        x,
        y,
        depth: depths[0],
    });
    for (idx, shot) in shots.iter().enumerate() {
        let azimuth_rad = shot.avg_azimuth.to_radians();
        let horizontal = horizontal_length_from_slope(shot.length, shot.depth_out - shot.depth_in);
        x += horizontal * azimuth_rad.sin();
        y += horizontal * azimuth_rad.cos();
        points.push(TracePoint {
            x,
            y,
            depth: depths[idx + 1],
        });
    }
    points
}

fn ignored_crossing_pairs_for_suggestion(
    suggestion: &PlacementSuggestion,
    import_shot_count: usize,
    existing_shot_count: usize,
) -> Vec<(usize, usize)> {
    let mut pairs = Vec::with_capacity(suggestion.matched_shots + 1);
    for offset in 0..=suggestion.matched_shots {
        let Some(import_idx) = (if suggestion.import_station_index == 0 {
            Some(offset)
        } else {
            suggestion.import_station_index.checked_sub(offset + 1)
        }) else {
            continue;
        };
        let Some(existing_idx) = (match suggestion.direction {
            MatchDirection::Forward => suggestion.existing_station_index.checked_add(offset),
            MatchDirection::Reverse => suggestion.existing_station_index.checked_sub(offset + 1),
        }) else {
            continue;
        };
        if import_idx >= import_shot_count || existing_idx >= existing_shot_count {
            continue;
        }
        pairs.push((import_idx, existing_idx));
    }
    pairs
}

/// Compute score comparing two shots in a given direction.
fn compute_shot_score(
    import: &MatchShot,
    existing: &MatchShot,
    direction: MatchDirection,
    config: &SuggestionConfig,
) -> f64 {
    let (existing_az, existing_depth_in, existing_depth_out) = match direction {
        MatchDirection::Forward => (existing.avg_azimuth, existing.depth_in, existing.depth_out),
        MatchDirection::Reverse => (
            (existing.avg_azimuth + 180.0) % 360.0,
            existing.depth_out,
            existing.depth_in,
        ),
    };

    let az_error = angular_difference(import.avg_azimuth, existing_az);
    let len_error = (import.length - existing.length).abs();
    let depth_in_error = (import.depth_in - existing_depth_in).abs();
    let depth_out_error = (import.depth_out - existing_depth_out).abs();

    // Hard cutoffs - reject physically implausible matches outright
    if az_error > config.max_azimuth_error_full
        || depth_in_error > config.max_depth_error
        || depth_out_error > config.max_depth_error
        || len_error > max_length_error(existing.length, config)
    {
        return 0.0;
    }

    let az_score = gaussian(az_error, config.azimuth_sigma);
    let len_score = gaussian(len_error, config.length_sigma);
    let depth_in_score = gaussian(depth_in_error, config.depth_sigma);
    let depth_out_score = gaussian(depth_out_error, config.depth_sigma);

    config.azimuth_weight * az_score
        + config.length_weight * len_score
        + config.depth_in_weight * depth_in_score
        + config.depth_out_weight * depth_out_score
}

fn compute_sequence_shot_score(
    import: &MatchShot,
    existing: &MatchShot,
    direction: MatchDirection,
    matched_count: usize,
    config: &SuggestionConfig,
) -> f64 {
    let (existing_az, existing_depth_in, existing_depth_out) = match direction {
        MatchDirection::Forward => (existing.avg_azimuth, existing.depth_in, existing.depth_out),
        MatchDirection::Reverse => (
            (existing.avg_azimuth + 180.0) % 360.0,
            existing.depth_out,
            existing.depth_in,
        ),
    };

    let az_error = angular_difference(import.avg_azimuth, existing_az);
    let len_error = (import.length - existing.length).abs();
    let depth_in_error = (import.depth_in - existing_depth_in).abs();
    let depth_out_error = (import.depth_out - existing_depth_out).abs();

    if az_error > config.max_azimuth_error_full
        || depth_in_error > config.max_depth_error
        || depth_out_error > config.max_depth_error
        || len_error > max_sequence_length_error(existing.length, matched_count, config)
    {
        return 0.0;
    }

    let az_score = gaussian(az_error, config.azimuth_sigma);
    let len_score = gaussian(len_error, config.length_sigma);
    let depth_in_score = gaussian(depth_in_error, config.depth_sigma);
    let depth_out_score = gaussian(depth_out_error, config.depth_sigma);

    config.azimuth_weight * az_score
        + config.length_weight * len_score
        + config.depth_in_weight * depth_in_score
        + config.depth_out_weight * depth_out_score
}

fn compute_exploratory_shot_score(
    import: &MatchShot,
    existing: &MatchShot,
    direction: MatchDirection,
    config: &SuggestionConfig,
) -> f64 {
    compute_sequence_shot_score(
        import,
        existing,
        direction,
        config.max_reference_shots.max(1),
        config,
    )
}

fn compute_single_shot_score(
    import: &MatchShot,
    existing: &MatchShot,
    direction: MatchDirection,
    config: &SuggestionConfig,
) -> f64 {
    let (existing_az, existing_depth_in, existing_depth_out) = match direction {
        MatchDirection::Forward => (existing.avg_azimuth, existing.depth_in, existing.depth_out),
        MatchDirection::Reverse => (
            (existing.avg_azimuth + 180.0) % 360.0,
            existing.depth_out,
            existing.depth_in,
        ),
    };

    let az_error = angular_difference(import.avg_azimuth, existing_az);
    let len_error = (import.length - existing.length).abs();
    let depth_in_error = (import.depth_in - existing_depth_in).abs();
    let depth_out_error = (import.depth_out - existing_depth_out).abs();

    if az_error > config.single_shot_max_azimuth_error
        || depth_in_error > config.single_shot_max_depth_error
        || depth_out_error > config.single_shot_max_depth_error
        || len_error > max_single_shot_length_error(existing.length, config)
    {
        return 0.0;
    }

    let score = compute_shot_score(import, existing, direction, config);
    if score >= config.single_shot_min_raw_score {
        score
    } else {
        0.0
    }
}

/// [#576 A2] Test-only tally of the matcher's inner-loop work, so a test can
/// pin how that work grows with section length without timing anything.
#[cfg(test)]
mod work {
    use std::cell::Cell;

    thread_local! {
        static UNITS: Cell<u64> = const { Cell::new(0) };
    }

    pub(super) fn charge(units: usize) {
        UNITS.with(|u| u.set(u.get().saturating_add(units as u64)));
    }

    /// The units charged on this thread since the last call.
    pub(super) fn take() -> u64 {
        UNITS.with(|u| u.replace(0))
    }
}

#[cfg(test)]
fn charge_work(units: usize) {
    work::charge(units);
}

#[cfg(not(test))]
#[inline(always)]
fn charge_work(_: usize) {}

/// [#576 NF-W4U-3] Test-only log of every crossing check `best_merge_offset`
/// makes, in call order, so the differential test can show an optimised
/// pipeline rejects exactly the candidates its reference copy does. Off until
/// a test starts it.
#[cfg(test)]
mod crossing_log {
    use std::cell::RefCell;

    use super::SectionId;

    /// (existing section, import station, existing station, clear).
    pub(super) type Entry = (SectionId, usize, usize, bool);

    thread_local! {
        static LOG: RefCell<Option<Vec<Entry>>> = const { RefCell::new(None) };
    }

    pub(super) fn start() {
        LOG.with(|log| *log.borrow_mut() = Some(Vec::new()));
    }

    pub(super) fn record(entry: Entry) {
        LOG.with(|log| {
            if let Some(entries) = log.borrow_mut().as_mut() {
                entries.push(entry);
            }
        });
    }

    /// The checks logged since `start`, and logging off again.
    pub(super) fn take() -> Vec<Entry> {
        LOG.with(|log| log.borrow_mut().take().unwrap_or_default())
    }
}

#[cfg(test)]
fn log_crossing_check(
    section: SectionId,
    import_station: usize,
    existing_station: usize,
    clear: bool,
) {
    crossing_log::record((section, import_station, existing_station, clear));
}

#[cfg(not(test))]
#[inline(always)]
fn log_crossing_check(_: SectionId, _: usize, _: usize, _: bool) {}

/// Result of trying to match a sequence of import shots against existing shots.
struct SequenceMatch {
    score: f64,
    matched_count: usize,
    existing_matched_count: usize,
    is_partial: bool,
}

#[derive(Clone, Copy)]
struct DpCell {
    score_sum: f64,
    weight_sum: f64,
    import_count: usize,
    existing_count: usize,
}

/// How many existing shots a run starting at `start_idx` can walk in
/// `direction` (`start_idx < existing_len`).
fn oriented_len(existing_len: usize, start_idx: usize, direction: MatchDirection) -> usize {
    match direction {
        MatchDirection::Forward => existing_len - start_idx,
        MatchDirection::Reverse => start_idx + 1,
    }
}

/// The `k`-th existing shot of a run starting at `start_idx`, oriented so the
/// run reads forward: walking backwards from `start_idx` for a reverse run,
/// each shot reversed. Read by index rather than by copying the oriented tail,
/// which cost `O(section)` per call - and the matcher makes four calls per
/// start index, so the copy alone made a section's pass quadratic (#576 A2).
fn oriented_existing_shot(
    existing_shots: &[MatchShot],
    start_idx: usize,
    direction: MatchDirection,
    k: usize,
) -> MatchShot {
    match direction {
        MatchDirection::Forward => existing_shots[start_idx + k],
        MatchDirection::Reverse => existing_shots[start_idx - k].reversed(),
    }
}

fn shot_weight(shot: &MatchShot, config: &SuggestionConfig) -> f64 {
    if shot.length < config.short_shot_threshold {
        shot.length / config.short_shot_threshold
    } else {
        1.0
    }
}

fn cell_final_score(cell: DpCell, config: &SuggestionConfig) -> f64 {
    if cell.import_count == 0 || cell.weight_sum <= 0.0 {
        return 0.0;
    }
    let raw_score = cell.score_sum / cell.weight_sum;
    let match_bonus = 1.0 + 0.15 * cell.weight_sum.ln();
    let single_penalty = if cell.import_count == 1 {
        config.single_shot_penalty
    } else {
        1.0
    };
    raw_score * match_bonus.max(1.0) * single_penalty
}

fn single_front_overlap_key(suggestion: &PlacementSuggestion) -> Option<(usize, usize)> {
    if suggestion.import_station_index != 0 || suggestion.matched_shots != 1 {
        return None;
    }

    let existing_shot_index = match suggestion.direction {
        MatchDirection::Forward => suggestion.existing_station_index,
        MatchDirection::Reverse => suggestion.existing_station_index.checked_sub(1)?,
    };

    Some((0, existing_shot_index))
}

fn suggestion_endpoint_key(
    suggestion: &PlacementSuggestion,
) -> Option<(SectionId, usize, usize, usize, usize)> {
    if suggestion.is_partial || suggestion.matched_shots != suggestion.existing_matched_shots {
        return None;
    }

    let import_a = suggestion.import_station_index;
    let import_b = if suggestion.import_station_index == 0 {
        suggestion.matched_shots
    } else {
        suggestion
            .import_station_index
            .checked_sub(suggestion.matched_shots)?
    };

    let existing_b = match suggestion.direction {
        MatchDirection::Forward => suggestion
            .existing_station_index
            .checked_add(suggestion.matched_shots)?,
        MatchDirection::Reverse => suggestion
            .existing_station_index
            .checked_sub(suggestion.matched_shots)?,
    };
    let existing_a = suggestion.existing_station_index;

    let (import_lo, import_hi) = if import_a <= import_b {
        (import_a, import_b)
    } else {
        (import_b, import_a)
    };
    let (existing_lo, existing_hi) = if existing_a <= existing_b {
        (existing_a, existing_b)
    } else {
        (existing_b, existing_a)
    };

    Some((
        suggestion.existing_section_id,
        import_lo,
        import_hi,
        existing_lo,
        existing_hi,
    ))
}

fn suppress_equivalent_single_front_reverses(suggestions: &mut Vec<PlacementSuggestion>) {
    const SCORE_EPSILON: f64 = 0.02;

    let forward_keys: Vec<(SectionId, (usize, usize), f64)> = suggestions
        .iter()
        .filter(|suggestion| suggestion.direction == MatchDirection::Forward)
        .filter_map(|suggestion| {
            single_front_overlap_key(suggestion)
                .map(|key| (suggestion.existing_section_id, key, suggestion.score))
        })
        .collect();

    suggestions.retain(|candidate| {
        if candidate.direction != MatchDirection::Reverse {
            return true;
        }

        let Some(key) = single_front_overlap_key(candidate) else {
            return true;
        };

        !forward_keys
            .iter()
            .any(|(section_id, other_key, other_score)| {
                *section_id == candidate.existing_section_id
                    && *other_key == key
                    && *other_score + SCORE_EPSILON >= candidate.score
            })
    });
}

fn suppress_duplicate_endpoint_pairs(suggestions: &mut Vec<PlacementSuggestion>) {
    let mut kept: Vec<PlacementSuggestion> = Vec::with_capacity(suggestions.len());
    // Where each endpoint key sits in `kept`. A key enters `kept` once and a
    // later suggestion with it replaces that entry in place (keeping both the
    // key and the position), so keys in `kept` are unique and this finds the
    // entry a front-to-back scan for the first match found - without the
    // O(suggestions^2) scan (#576 A2).
    let mut kept_at: std::collections::HashMap<(SectionId, usize, usize, usize, usize), usize> =
        std::collections::HashMap::new();

    for suggestion in suggestions.drain(..) {
        let Some(key) = suggestion_endpoint_key(&suggestion) else {
            kept.push(suggestion);
            continue;
        };

        if let Some(&at) = kept_at.get(&key) {
            if suggestion.score > kept[at].score {
                kept[at] = suggestion;
            }
        } else {
            kept_at.insert(key, kept.len());
            kept.push(suggestion);
        }
    }

    *suggestions = kept;
}

fn outward_runs(shots: &[MatchShot], station_index: usize) -> Vec<(bool, Vec<MatchShot>)> {
    let mut runs = Vec::new();

    if station_index >= 1 {
        runs.push((false, vec![shots[station_index - 1].reversed()]));
    }

    if station_index < shots.len() {
        runs.push((true, vec![shots[station_index]]));
    }

    runs
}

fn station_context_multiplier(
    import_shots: &[MatchShot],
    existing_shots: &[MatchShot],
    import_station_index: usize,
    existing_station_index: usize,
    config: &SuggestionConfig,
) -> f64 {
    if import_station_index > import_shots.len() || existing_station_index > existing_shots.len() {
        return 0.75;
    }

    let import_runs = outward_runs(import_shots, import_station_index);
    let existing_runs = outward_runs(existing_shots, existing_station_index);
    if import_runs.is_empty() || existing_runs.is_empty() {
        return 1.0;
    }

    let mut scored: Vec<(f64, bool, bool)> = Vec::new();
    for (import_forward, import_run) in &import_runs {
        for (existing_forward, existing_run) in &existing_runs {
            let (Some(import_shot), Some(existing_shot)) =
                (import_run.first(), existing_run.first())
            else {
                continue;
            };
            let score =
                compute_shot_score(import_shot, existing_shot, MatchDirection::Forward, config);
            if score >= config.min_shot_score {
                scored.push((score, *import_forward, *existing_forward));
            }
        }
    }

    if scored.is_empty() {
        return 0.65;
    }

    scored.sort_by(|a, b| score_cmp(b.0, a.0));
    let best = scored[0];
    let has_second_side = scored
        .iter()
        .skip(1)
        .any(|(_, import_forward, existing_forward)| {
            *import_forward != best.1 && *existing_forward != best.2
        });

    let import_leaf = import_station_index == 0 || import_station_index == import_shots.len();
    let existing_leaf =
        existing_station_index == 0 || existing_station_index == existing_shots.len();

    if has_second_side {
        1.18
    } else if import_leaf || existing_leaf {
        1.0
    } else if best.0 >= 0.85 {
        0.86
    } else {
        0.72
    }
}

fn merge_offset_pairs(suggestion: &PlacementSuggestion) -> Vec<(usize, usize)> {
    let import_max_offset = if suggestion.is_partial {
        suggestion.matched_shots.saturating_sub(1)
    } else {
        suggestion.matched_shots
    };
    (0..=import_max_offset).map(|k| (k, k)).collect()
}

fn merge_station_pair_for_offsets(
    import_shot_count: usize,
    existing_shot_count: usize,
    suggestion: &PlacementSuggestion,
    import_offset: usize,
    existing_offset: usize,
) -> Option<(usize, usize)> {
    let import_station = if suggestion.import_station_index == 0 {
        import_offset
    } else {
        suggestion.import_station_index.checked_sub(import_offset)?
    };
    let existing_station = match suggestion.direction {
        MatchDirection::Forward => suggestion
            .existing_station_index
            .checked_add(existing_offset)?,
        MatchDirection::Reverse => suggestion
            .existing_station_index
            .checked_sub(existing_offset)?,
    };

    if import_station <= import_shot_count && existing_station <= existing_shot_count {
        Some((import_station, existing_station))
    } else {
        None
    }
}

fn sequence_match_passes_final_cutoffs(
    import_shots: &[MatchShot],
    existing_oriented: &[MatchShot],
    cell: DpCell,
    config: &SuggestionConfig,
) -> bool {
    if cell.import_count == 0
        || cell.existing_count == 0
        || cell.import_count != cell.existing_count
        || cell.import_count > import_shots.len()
        || cell.existing_count > existing_oriented.len()
    {
        return false;
    }

    if cell.import_count == 1 {
        return compute_single_shot_score(
            &import_shots[0],
            &existing_oriented[0],
            MatchDirection::Forward,
            config,
        ) >= config.min_shot_score;
    }

    for idx in 0..cell.import_count {
        let score = compute_sequence_shot_score(
            &import_shots[idx],
            &existing_oriented[idx],
            MatchDirection::Forward,
            cell.import_count,
            config,
        );
        if score < config.min_shot_score {
            return false;
        }
    }

    true
}

/// Try matching a sequence of import shots against existing shots starting at `start_idx`.
fn try_match_sequence(
    import_shots: &[MatchShot],
    existing_shots: &[MatchShot],
    start_idx: usize,
    direction: MatchDirection,
    config: &SuggestionConfig,
) -> SequenceMatch {
    if import_shots.is_empty() || existing_shots.is_empty() || start_idx >= existing_shots.len() {
        return SequenceMatch {
            score: 0.0,
            matched_count: 0,
            existing_matched_count: 0,
            is_partial: false,
        };
    }

    // The run pairs import shot k with oriented existing shot k and only ever
    // extends by one of each, so its states are the DIAGONAL prefixes
    // (k shots, k shots), and the first shot that scores below
    // `min_shot_score` ends every longer run. This used to be phrased as an
    // `(n + 1) x (m + 1)` matrix filled from `dp[0][0]` - but a cell was only
    // ever written as `dp[i + 1][j + 1]` from `dp[i][j]`, so nothing off the
    // diagonal was ever reached, while the whole matrix was allocated and
    // walked on every call (#576 A2). Walking the diagonal visits the same
    // cells in the same order and accumulates the same sums in the same
    // order, so every candidate is bit-identical; the stable sort below then
    // orders them exactly as it ordered the matrix rows.
    let reach = import_shots
        .len()
        .min(oriented_len(existing_shots.len(), start_idx, direction));
    let mut existing_oriented: Vec<MatchShot> = Vec::with_capacity(reach);
    let mut candidates: Vec<DpCell> = Vec::with_capacity(reach);
    let mut cell = DpCell {
        score_sum: 0.0,
        weight_sum: 0.0,
        import_count: 0,
        existing_count: 0,
    };
    for (k, import_shot) in import_shots.iter().enumerate().take(reach) {
        charge_work(1);
        let existing_shot = oriented_existing_shot(existing_shots, start_idx, direction, k);
        let score = compute_exploratory_shot_score(
            import_shot,
            &existing_shot,
            MatchDirection::Forward,
            config,
        );
        if score < config.min_shot_score {
            break;
        }
        let weight = shot_weight(import_shot, config);
        cell = DpCell {
            score_sum: cell.score_sum + score * weight,
            weight_sum: cell.weight_sum + weight,
            import_count: cell.import_count + 1,
            existing_count: cell.existing_count + 1,
        };
        existing_oriented.push(existing_shot);
        candidates.push(cell);
    }

    candidates.sort_by(|a, b| {
        score_cmp(cell_final_score(*b, config), cell_final_score(*a, config))
            .then_with(|| b.import_count.cmp(&a.import_count))
    });

    let Some(best) = candidates.into_iter().find(|cell| {
        cell.import_count > 0
            && cell.weight_sum > 0.0
            && sequence_match_passes_final_cutoffs(import_shots, &existing_oriented, *cell, config)
    }) else {
        return SequenceMatch {
            score: 0.0,
            matched_count: 0,
            existing_matched_count: 0,
            is_partial: false,
        };
    };

    SequenceMatch {
        score: cell_final_score(best, config),
        matched_count: best.import_count,
        existing_matched_count: best.existing_count,
        is_partial: false,
    }
}

/// The merge offset a suggestion is placed at, and its station-context
/// multiplier: the highest multiplier among the offsets whose placement passes
/// the crossing check, a tie going to the larger offset. `None` when no offset
/// passes.
///
/// [#576 A2] Ranked FIRST and crossing-checked in that order, stopping at the
/// first that passes, instead of crossing-checking every offset and folding:
/// the crossing check is the expensive half (`O(import length)` per offset)
/// and the multiplier is a handful of comparisons. The two agree because the
/// multiplier only takes the fixed constants `station_context_multiplier`
/// returns, so the fold's "within 1e-9" tie is plain equality and the fold
/// computes the lexicographic maximum of (multiplier, offset) over the passing
/// offsets - which is exactly the first passing entry of this ranking.
fn best_merge_offset(
    import_match_shots: &[MatchShot],
    existing_match_shots: &[MatchShot],
    import_trace: &[TracePoint],
    existing_trace: &[TracePoint],
    crossing: &CrossingIndex,
    suggestion: &PlacementSuggestion,
    config: &SuggestionConfig,
) -> Option<(f64, usize)> {
    let ignored_crossing_pairs = ignored_crossing_pairs_for_suggestion(
        suggestion,
        import_match_shots.len(),
        existing_match_shots.len(),
    );
    let mut ranked: Vec<(f64, usize, usize, usize)> = merge_offset_pairs(suggestion)
        .into_iter()
        .filter_map(|(import_offset, existing_offset)| {
            let (merge_import_station, merge_existing_station) = merge_station_pair_for_offsets(
                import_match_shots.len(),
                existing_match_shots.len(),
                suggestion,
                import_offset,
                existing_offset,
            )?;
            let context_multiplier = station_context_multiplier(
                import_match_shots,
                existing_match_shots,
                merge_import_station,
                merge_existing_station,
                config,
            );
            let context_multiplier = if suggestion.matched_shots == 1 {
                context_multiplier.max(1.0)
            } else {
                context_multiplier
            };
            Some((
                context_multiplier,
                import_offset,
                merge_import_station,
                merge_existing_station,
            ))
        })
        .collect();
    ranked.sort_by(|a, b| score_cmp(b.0, a.0).then_with(|| b.1.cmp(&a.1)));
    ranked
        .into_iter()
        .find(|&(_, _, merge_import_station, merge_existing_station)| {
            let clear = placement_crossing_clearance_ok(
                import_trace,
                existing_trace,
                crossing,
                merge_import_station,
                merge_existing_station,
                &ignored_crossing_pairs,
                config,
            );
            log_crossing_check(
                suggestion.existing_section_id,
                merge_import_station,
                merge_existing_station,
                clear,
            );
            clear
        })
        .map(|(context_multiplier, import_offset, _, _)| (context_multiplier, import_offset))
}

/// Existing section data needed for matching.
#[derive(Clone)]
pub struct ExistingSection {
    pub id: SectionId,
    pub name: String,
    pub shots: Vec<Shot>,
    /// The section's resolved north reference (#438), so its bearings can be
    /// compared with an import's in one frame. Callers get it from
    /// [`Project::azimuth_reference_for`](crate::models::Project::azimuth_reference_for).
    pub azimuth_reference: AzimuthReference,
}

/// Suggest placements for an import section against existing sections.
pub fn suggest_placements(
    import_shots: &[ShotData],
    import_reference: AzimuthReference,
    existing_sections: &[ExistingSection],
    ps_config: &PlacementSuggestionConfig,
) -> Vec<PlacementSuggestion> {
    suggest_placements_among(
        import_shots,
        import_reference,
        existing_sections.iter(),
        ps_config,
    )
}

/// [`suggest_placements`] over BORROWED sections, so
/// [`suggest_all_placements`] can match one import section against all the
/// others without cloning them (#576 A2: it used to copy every other import
/// section once per import section, O(K^2) section copies).
fn suggest_placements_among<'a>(
    import_shots: &[ShotData],
    import_reference: AzimuthReference,
    existing_sections: impl Iterator<Item = &'a ExistingSection>,
    ps_config: &PlacementSuggestionConfig,
) -> Vec<PlacementSuggestion> {
    let config = &SuggestionConfig::from(ps_config);

    // An empty `existing_sections` falls through to an empty result.
    if import_shots.is_empty() {
        return Vec::new();
    }

    let max_ref = config.max_reference_shots.min(import_shots.len());

    // Build import match shots for station 0 (first N shots, natural order)
    let import_from_0: Vec<MatchShot> = import_shots[..max_ref]
        .iter()
        .map(|s| MatchShot::from_shot_data(s, import_reference))
        .collect();

    let import_match_shots: Vec<MatchShot> = import_shots
        .iter()
        .map(|s| MatchShot::from_shot_data(s, import_reference))
        .collect();
    let import_trace = section_trace_points(&import_match_shots);
    let import_runs = straight_runs(&import_trace);

    // Build import match shots for station N (last N shots, reversed)
    let import_from_n: Vec<MatchShot> = import_shots[import_shots.len().saturating_sub(max_ref)..]
        .iter()
        .rev()
        .map(|s| MatchShot::from_shot_data(s, import_reference).reversed())
        .collect();

    let import_station_n = import_shots.len(); // station index for last station

    let mut suggestions: Vec<PlacementSuggestion> = Vec::new();
    // Where each (section, existing station, import station, direction) key
    // sits in `suggestions`: the duplicate check used to scan the whole list
    // for every candidate. Keys are unique in the list (a duplicate is merged
    // into the entry already there), so this finds the same entry the scan's
    // first match did.
    let mut suggestion_at: std::collections::HashMap<(SectionId, usize, usize, bool), usize> =
        std::collections::HashMap::new();

    for existing in existing_sections {
        if existing.shots.is_empty() {
            continue;
        }

        let existing_match_shots: Vec<MatchShot> = existing
            .shots
            .iter()
            .map(|s| MatchShot::from_shot(s, existing.azimuth_reference))
            .collect();
        // [#576 NF-W4U-3] The trace and its crossing index are pure functions
        // of the two sections, needed only once a candidate reaches the
        // crossing check - which most section pairs never produce - so they
        // are built on that first candidate rather than for every pair.
        let crossing_for_pair: std::cell::OnceCell<(Vec<TracePoint>, CrossingIndex)> =
            std::cell::OnceCell::new();

        // Try from both leaves of the import section
        let leaf_configs: &[(&[MatchShot], usize)] =
            &[(&import_from_0, 0), (&import_from_n, import_station_n)];

        for &(ref_shots, import_station) in leaf_configs {
            if ref_shots.is_empty() {
                continue;
            }

            // Try all starting positions in the existing section
            for start_idx in 0..existing_match_shots.len() {
                // Try both forward and reverse directions
                for &direction in &[MatchDirection::Forward, MatchDirection::Reverse] {
                    let result = try_match_sequence(
                        ref_shots,
                        &existing_match_shots,
                        start_idx,
                        direction,
                        config,
                    );

                    if result.matched_count == 0 || !admitted(result.score, config) {
                        continue;
                    }

                    // Compute the existing station index for the merge point
                    let existing_station_index = match direction {
                        MatchDirection::Forward => start_idx,
                        MatchDirection::Reverse => start_idx + 1,
                    };

                    let mut suggestion = PlacementSuggestion {
                        existing_section_id: existing.id,
                        existing_station_index,
                        import_station_index: import_station,
                        score: result.score,
                        matched_shots: result.matched_count,
                        existing_matched_shots: result.existing_matched_count,
                        preferred_merge_offset: if result.is_partial {
                            result.matched_count.saturating_sub(1)
                        } else {
                            result.matched_count
                        },
                        direction,
                        is_partial: result.is_partial,
                        confidence: SuggestionConfidence::Low,
                        existing_section_name: existing.name.clone(),
                    };

                    let (existing_trace, crossing) = crossing_for_pair.get_or_init(|| {
                        let trace = section_trace_points(&existing_match_shots);
                        let index = CrossingIndex::build(&import_trace, &trace, &import_runs);
                        (trace, index)
                    });
                    let best_context = best_merge_offset(
                        &import_match_shots,
                        &existing_match_shots,
                        &import_trace,
                        existing_trace,
                        crossing,
                        &suggestion,
                        config,
                    );
                    let Some((context_multiplier, preferred_offset)) = best_context else {
                        continue;
                    };
                    suggestion.preferred_merge_offset = preferred_offset;
                    suggestion.score *= context_multiplier;

                    if !admitted(suggestion.score, config) {
                        continue;
                    }

                    suggestion.confidence = if suggestion.score >= 0.90 {
                        SuggestionConfidence::High
                    } else if suggestion.score >= 0.70 {
                        SuggestionConfidence::Medium
                    } else {
                        SuggestionConfidence::Low
                    };

                    // Check for duplicates, keeping the better score
                    let key = (
                        suggestion.existing_section_id,
                        suggestion.existing_station_index,
                        suggestion.import_station_index,
                        suggestion.direction == MatchDirection::Reverse,
                    );
                    if let Some(&at) = suggestion_at.get(&key) {
                        if suggestion.score > suggestions[at].score {
                            suggestions[at] = suggestion;
                        }
                        continue;
                    }

                    suggestion_at.insert(key, suggestions.len());
                    suggestions.push(suggestion);
                }
            }
        }
    }

    suppress_equivalent_single_front_reverses(&mut suggestions);
    suppress_duplicate_endpoint_pairs(&mut suggestions);

    // Sort by score descending
    suggestions.sort_by(|a, b| score_cmp(b.score, a.score));

    // Keep top N
    suggestions.truncate(config.max_suggestions);

    suggestions
}

#[cfg(test)]
#[expect(clippy::print_stdout)]
mod perf_tests;

#[cfg(test)]
#[expect(clippy::print_stdout)]
mod differential_tests;

#[cfg(test)]
#[expect(
    clippy::print_stdout,
    reason = "the NaN-order test prints the order it observed, so a failure shows it"
)]
mod tests {
    use super::*;
    use crate::test_fixtures::bare_shot;

    fn make_shot_data(
        az_in: f64,
        az_out: f64,
        length: f64,
        depth_in: f64,
        depth_out: f64,
    ) -> ShotData {
        bare_shot(length)
            .with_azimuths(az_in, az_out)
            .with_depths(depth_in, depth_out)
    }

    fn make_shot(
        id: u32,
        az_in: f64,
        az_out: f64,
        length: f64,
        depth_in: f64,
        depth_out: f64,
    ) -> Shot {
        Shot::new(
            id,
            make_shot_data(az_in, az_out, length, depth_in, depth_out),
        )
    }

    fn make_section(id: SectionId, name: &str, shots: Vec<Shot>) -> ExistingSection {
        ExistingSection {
            id,
            name: name.to_string(),
            shots,
            azimuth_reference: AzimuthReference::default(),
        }
    }

    /// #438: the matcher compares an INCOMING section's bearings against
    /// sections already in the project, and the two can carry different
    /// declinations. Both sides have to be read in one frame or a correct
    /// overlap scores as a mismatch by the difference of the corrections - 5
    /// degrees is already past `max_azimuth_error_full`.
    ///
    /// The fixture is the smallest thing that can tell the two apart: the same
    /// physical passage, recorded once as magnetic readings with a declared 5
    /// degree declination and once as the corrected bearings, so it matches
    /// only when the import side is read through its own reference.
    #[test]
    fn an_import_section_is_scored_in_its_own_frame_not_the_projects() {
        let corrected: Vec<Shot> = (0..4)
            .map(|i| {
                make_shot(
                    i,
                    15.0 + f64::from(i) * 20.0,
                    15.0 + f64::from(i) * 20.0,
                    10.0,
                    0.0,
                    0.0,
                )
            })
            .collect();
        let existing = vec![make_section(0, "Existing", corrected)];

        // The same line as MEASURED, 5 degrees off, with the declination on the
        // section rather than in the readings.
        let measured: Vec<ShotData> = (0..4)
            .map(|i| {
                make_shot_data(
                    10.0 + f64::from(i) * 20.0,
                    10.0 + f64::from(i) * 20.0,
                    10.0,
                    0.0,
                    0.0,
                )
            })
            .collect();
        let config = PlacementSuggestionConfig::default();

        let declared = AzimuthReference::magnetic(5.0, crate::CorrectionSource::FileDeclared);
        let with_reference = suggest_placements(&measured, declared, &existing, &config);
        assert!(
            !with_reference.is_empty(),
            "read through its own reference the two are the same passage and must match"
        );

        // The bug: scoring the same shots as if they were already corrected.
        let without =
            suggest_placements(&measured, AzimuthReference::default(), &existing, &config);
        let best_with = with_reference
            .iter()
            .map(|s| s.score)
            .fold(f64::NEG_INFINITY, f64::max);
        let best_without = without
            .iter()
            .map(|s| s.score)
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(
            best_with > best_without,
            "the correct frame must score higher: {best_with} vs {best_without}"
        );
    }

    /// The cross-section entry point reads each import section's OWN reference,
    /// and only falls back for one that declares none.
    #[test]
    fn suggest_all_reads_each_import_sections_own_reference() {
        let corrected: Vec<Shot> = (0..4)
            .map(|i| {
                make_shot(
                    i,
                    15.0 + f64::from(i) * 20.0,
                    15.0 + f64::from(i) * 20.0,
                    10.0,
                    0.0,
                    0.0,
                )
            })
            .collect();
        let existing = vec![make_section(0, "Existing", corrected)];
        let measured: Vec<ShotData> = (0..4)
            .map(|i| {
                make_shot_data(
                    10.0 + f64::from(i) * 20.0,
                    10.0 + f64::from(i) * 20.0,
                    10.0,
                    0.0,
                    0.0,
                )
            })
            .collect();
        let config = PlacementSuggestionConfig::default();

        let declaring = vec![ImportSectionForSuggestion {
            index: 0,
            name: "Incoming".to_string(),
            shots: measured.clone(),
            azimuth_reference: Some(AzimuthReference::magnetic(
                5.0,
                crate::CorrectionSource::FileDeclared,
            )),
        }];
        // The FALLBACK is deliberately wrong here, so a suggestion can only
        // come from the section's own reference being read.
        let found =
            suggest_all_placements(&declaring, AzimuthReference::default(), &existing, &config);
        assert!(
            !found.is_empty(),
            "the section's own reference must win over the fallback"
        );

        let silent = vec![ImportSectionForSuggestion {
            index: 0,
            name: "Incoming".to_string(),
            shots: measured,
            azimuth_reference: None,
        }];
        let via_fallback = suggest_all_placements(
            &silent,
            AzimuthReference::magnetic(5.0, crate::CorrectionSource::FileDeclared),
            &existing,
            &config,
        );
        assert!(
            !via_fallback.is_empty(),
            "a section that declares nothing must still be read through the fallback"
        );
    }

    /// A synthetic scene shaped like the differential gate's seed 0x736: short
    /// straight sections of 5 m and 3 m legs with one NaN azimuth each, so the
    /// matcher yields multi-shot, single-shot and NaN-scored candidates together.
    fn nan_scene() -> (Vec<ExistingSection>, Vec<ImportSectionForSuggestion>) {
        // Level legs, and 3 m legs that climb (`up`) or drop (`down`) 3 m.
        let depth = 29.3;
        let leg = |az: f64, length: f64| make_shot_data(az, az, length, depth, depth);
        let up = |az: f64| make_shot_data(az, az, 3.0, depth + 3.0, depth);
        let down = |az: f64| make_shot_data(az, az, 3.0, depth, depth + 3.0);
        let existing_shots = [
            leg(0.0, 5.0),
            up(90.0),
            leg(0.0, 5.0),
            leg(f64::NAN, 5.0),
            leg(45.0, 0.0),
            leg(180.0, 5.0),
            down(0.0),
            leg(0.0, 5.0),
            leg(180.0, 5.0),
            up(180.0),
        ];
        let existing = vec![make_section(
            1,
            "CaveA",
            existing_shots
                .iter()
                .enumerate()
                .map(|(i, s)| Shot::new(i as u32, s.clone()))
                .collect(),
        )];
        let imports: Vec<ImportSectionForSuggestion> = [
            vec![
                leg(180.0, 5.0),
                leg(0.0, 5.0),
                down(90.0),
                leg(45.0, 5.0),
                leg(f64::NAN, 5.0),
                leg(180.0, 5.0),
                leg(180.0, 5.0),
                leg(180.0, 5.0),
            ],
            vec![
                leg(0.0, 0.0),
                leg(0.0, 5.0),
                up(0.0),
                leg(90.0, 5.0),
                leg(0.0, 5.0),
                leg(0.0, 5.0),
            ],
            vec![
                leg(0.0, 5.0),
                down(45.0),
                leg(f64::NAN, 5.0),
                down(45.0),
                leg(45.0, 5.0),
                leg(90.0, 5.0),
                down(0.0),
                leg(90.0, 5.0),
                down(90.0),
                leg(180.0, 5.0),
            ],
        ]
        .into_iter()
        .enumerate()
        .map(|(index, shots)| ImportSectionForSuggestion {
            index: index as u32,
            name: format!("Import{index}"),
            shots,
            azimuth_reference: None,
        })
        .collect();
        (existing, imports)
    }

    /// [#576 NF-W9PERF-1] A NaN reading made a candidate's score NaN, and the
    /// score sorts took NaN as "equal" to every score: equal to 1.3 and to 0.6,
    /// which are not equal to each other. That is no order, and the standard
    /// sort panics when it sees it ("does not correctly implement a total
    /// order"), so one bad azimuth panicked the whole suggestion pass.
    ///
    /// The scene is [`nan_scene`]. Its NaN-scored candidate is no longer offered
    /// (#576 NF-W10BE-1, [`a_nan_scored_candidate_is_not_offered`]); the matcher's
    /// own sort still sees it.
    #[test]
    fn a_nan_reading_neither_panics_the_score_sorts_nor_moves_the_finite_candidates() {
        let (existing, imports) = nan_scene();

        let found = suggest_all_placements(
            &imports,
            AzimuthReference::default(),
            &existing,
            &PlacementSuggestionConfig::default(),
        );

        let scores: Vec<f64> = found.iter().map(|s| s.score).collect();
        println!("suggestion scores, in order: {scores:?}");
        let distinct = {
            let mut bits: Vec<u64> = scores.iter().map(|s| s.to_bits()).collect();
            bits.dedup();
            bits.len()
        };
        assert!(
            distinct >= 2 && scores.iter().all(|s| s.is_finite()),
            "the scene must yield distinct finite scores to test the order: {scores:?}"
        );
        assert!(
            scores.windows(2).all(|w| w[0] >= w[1]),
            "the finite candidates stay best first: {scores:?}"
        );
    }

    /// [#576 NF-W10BE-1] A candidate whose score is NaN is not offered. A NaN
    /// reading gives the matcher a NaN score, and `NaN < min_aggregate_score` is
    /// false, so both admission checks let it through and it was offered (sorted
    /// last, serialised as JSON null).
    #[test]
    fn a_nan_scored_candidate_is_not_offered() {
        let (existing, imports) = nan_scene();
        let config = PlacementSuggestionConfig::default();
        let inner = SuggestionConfig::from(&config);

        // Precondition: the matcher itself scores some candidate of this scene NaN,
        // so what keeps it out below is the admission, not the scene.
        let existing_shots: Vec<MatchShot> = existing[0]
            .shots
            .iter()
            .map(|s| MatchShot::from_shot(s, existing[0].azimuth_reference))
            .collect();
        let mut nan_candidates = 0;
        for import in &imports {
            let shots: Vec<MatchShot> = import
                .shots
                .iter()
                .map(|s| MatchShot::from_shot_data(s, AzimuthReference::default()))
                .collect();
            let max_ref = inner.max_reference_shots.min(shots.len());
            let from_0 = shots[..max_ref].to_vec();
            let from_n: Vec<MatchShot> = shots[shots.len() - max_ref..]
                .iter()
                .rev()
                .map(MatchShot::reversed)
                .collect();
            for leaf in [&from_0, &from_n] {
                for start in 0..existing_shots.len() {
                    for direction in [MatchDirection::Forward, MatchDirection::Reverse] {
                        let m = try_match_sequence(leaf, &existing_shots, start, direction, &inner);
                        nan_candidates += usize::from(m.matched_count > 0 && m.score.is_nan());
                    }
                }
            }
        }

        let all = suggest_all_placements(&imports, AzimuthReference::default(), &existing, &config);
        let per_import: Vec<Vec<f64>> = imports
            .iter()
            .map(|import| {
                suggest_placements(
                    &import.shots,
                    AzimuthReference::default(),
                    &existing,
                    &config,
                )
                .iter()
                .map(|s| s.score)
                .collect()
            })
            .collect();
        let all_scores: Vec<f64> = all.iter().map(|s| s.score).collect();
        println!(
            "NaN-scored matcher candidates: {nan_candidates}; suggest_placements scores per \
             import: {per_import:?}; suggest_all_placements scores: {all_scores:?}"
        );
        assert!(
            nan_candidates > 0,
            "precondition: the matcher scores a candidate of this scene NaN"
        );
        assert!(
            per_import.iter().flatten().all(|s| !s.is_nan()),
            "suggest_placements offers no NaN-scored suggestion: {per_import:?}"
        );
        assert!(
            all_scores.iter().all(|s| !s.is_nan()),
            "suggest_all_placements offers no NaN-scored suggestion: {all_scores:?}"
        );
        assert!(
            all_scores.iter().any(|s| *s >= inner.min_aggregate_score),
            "the finite suggestions are still offered: {all_scores:?}"
        );
    }

    /// [#576 NF-W9PERF-1] The total order agrees with `partial_cmp` on every
    /// pair of numbers - signed zeros and infinities included - so no finite
    /// result can move; only a NaN gets a place (below every number).
    #[test]
    fn the_score_order_is_partial_cmp_on_numbers_and_puts_nan_lowest() {
        use std::cmp::Ordering;
        let numbers = [
            f64::NEG_INFINITY,
            -1.5,
            -0.0,
            0.0,
            f64::MIN_POSITIVE,
            0.6,
            1.3,
            f64::INFINITY,
        ];
        for a in numbers {
            for b in numbers {
                assert_eq!(Some(score_cmp(a, b)), a.partial_cmp(&b), "{a} vs {b}");
            }
            assert_eq!(score_cmp(f64::NAN, a), Ordering::Less, "NaN vs {a}");
            assert_eq!(score_cmp(a, f64::NAN), Ordering::Greater, "{a} vs NaN");
        }
        assert_eq!(score_cmp(f64::NAN, f64::NAN), Ordering::Equal);
        assert_eq!(score_cmp(-0.0, 0.0), Ordering::Equal, "unlike total_cmp");
    }

    // ========================================================================
    // angular_difference tests
    // ========================================================================

    #[test]
    fn angular_difference_basic() {
        assert!((angular_difference(10.0, 20.0) - 10.0).abs() < 1e-6);
        assert!((angular_difference(20.0, 10.0) - 10.0).abs() < 1e-6);
    }

    #[test]
    fn angular_difference_wraparound() {
        assert!((angular_difference(350.0, 10.0) - 20.0).abs() < 1e-6);
        assert!((angular_difference(10.0, 350.0) - 20.0).abs() < 1e-6);
    }

    #[test]
    fn angular_difference_opposite() {
        assert!((angular_difference(0.0, 180.0) - 180.0).abs() < 1e-6);
        assert!((angular_difference(90.0, 270.0) - 180.0).abs() < 1e-6);
    }

    #[test]
    fn angular_difference_same() {
        assert!((angular_difference(45.0, 45.0)).abs() < 1e-6);
    }

    // ========================================================================
    // Shot scoring tests
    // ========================================================================

    #[test]
    fn identical_shots_score_perfectly() {
        let config = SuggestionConfig::default();
        let shot = MatchShot {
            avg_azimuth: 90.0,
            length: 5.0,
            depth_in: 3.0,
            depth_out: 4.0,
        };
        let score = compute_shot_score(&shot, &shot, MatchDirection::Forward, &config);
        assert!(
            (score - 1.0).abs() < 1e-6,
            "identical shots should score 1.0, got {score}"
        );
    }

    #[test]
    fn reversed_shots_score_perfectly() {
        let config = SuggestionConfig::default();
        let import = MatchShot {
            avg_azimuth: 270.0, // 90 + 180
            length: 5.0,
            depth_in: 4.0, // swapped
            depth_out: 3.0,
        };
        let existing = MatchShot {
            avg_azimuth: 90.0,
            length: 5.0,
            depth_in: 3.0,
            depth_out: 4.0,
        };
        let score = compute_shot_score(&import, &existing, MatchDirection::Reverse, &config);
        assert!(
            (score - 1.0).abs() < 1e-6,
            "reversed identical shots should score 1.0, got {score}"
        );
    }

    #[test]
    fn very_different_shots_score_low() {
        let config = SuggestionConfig::default();
        let import = MatchShot {
            avg_azimuth: 0.0,
            length: 1.0,
            depth_in: 0.0,
            depth_out: 0.5,
        };
        let existing = MatchShot {
            avg_azimuth: 180.0,
            length: 20.0,
            depth_in: 50.0,
            depth_out: 55.0,
        };
        let score = compute_shot_score(&import, &existing, MatchDirection::Forward, &config);
        assert!(
            score == 0.0,
            "very different shots should be hard-rejected, got {score}"
        );
    }

    #[test]
    fn small_azimuth_error_high_score() {
        let config = SuggestionConfig::default();
        let import = MatchShot {
            avg_azimuth: 92.0, // 2° off
            length: 5.0,
            depth_in: 3.0,
            depth_out: 4.0,
        };
        let existing = MatchShot {
            avg_azimuth: 90.0,
            length: 5.0,
            depth_in: 3.0,
            depth_out: 4.0,
        };
        let score = compute_shot_score(&import, &existing, MatchDirection::Forward, &config);
        assert!(
            score > 0.9,
            "small azimuth error should still score high, got {score}"
        );
    }

    #[test]
    fn length_deviation_is_hard_rejected() {
        let config = SuggestionConfig::default();
        let import = MatchShot {
            avg_azimuth: 90.0,
            length: 10.0,
            depth_in: 3.0,
            depth_out: 5.0,
        };
        let existing = MatchShot {
            avg_azimuth: 90.0,
            length: 5.0,
            depth_in: 3.0,
            depth_out: 4.0,
        };
        let score = compute_shot_score(&import, &existing, MatchDirection::Forward, &config);
        assert_eq!(score, 0.0, "large length deviation should be hard-rejected");
    }

    // ========================================================================
    // Sequence matching tests
    // ========================================================================

    #[test]
    fn multi_shot_forward_match() {
        let config = SuggestionConfig::default();
        let import = vec![
            MatchShot {
                avg_azimuth: 90.0,
                length: 5.0,
                depth_in: 3.0,
                depth_out: 4.0,
            },
            MatchShot {
                avg_azimuth: 45.0,
                length: 3.0,
                depth_in: 4.0,
                depth_out: 4.5,
            },
        ];
        let existing = vec![
            MatchShot {
                avg_azimuth: 90.0,
                length: 5.0,
                depth_in: 3.0,
                depth_out: 4.0,
            },
            MatchShot {
                avg_azimuth: 45.0,
                length: 3.0,
                depth_in: 4.0,
                depth_out: 4.5,
            },
            MatchShot {
                avg_azimuth: 180.0,
                length: 7.0,
                depth_in: 4.5,
                depth_out: 6.0,
            },
        ];
        let result = try_match_sequence(&import, &existing, 0, MatchDirection::Forward, &config);
        assert_eq!(result.matched_count, 2);
        assert!(
            result.score > 0.9,
            "perfect forward match should score high, got {}",
            result.score
        );
        assert!(!result.is_partial);
    }

    #[test]
    fn multi_shot_reverse_match() {
        let config = SuggestionConfig::default();
        // Import shots retrace existing shots backwards
        let import = vec![
            // Reversed version of existing[1]: az 45+180=225, depth swapped
            MatchShot {
                avg_azimuth: 225.0,
                length: 3.0,
                depth_in: 4.5,
                depth_out: 4.0,
            },
            // Reversed version of existing[0]: az 90+180=270, depth swapped
            MatchShot {
                avg_azimuth: 270.0,
                length: 5.0,
                depth_in: 4.0,
                depth_out: 3.0,
            },
        ];
        let existing = vec![
            MatchShot {
                avg_azimuth: 90.0,
                length: 5.0,
                depth_in: 3.0,
                depth_out: 4.0,
            },
            MatchShot {
                avg_azimuth: 45.0,
                length: 3.0,
                depth_in: 4.0,
                depth_out: 4.5,
            },
            MatchShot {
                avg_azimuth: 180.0,
                length: 7.0,
                depth_in: 4.5,
                depth_out: 6.0,
            },
        ];
        // Start at shot 1, matching backwards
        let result = try_match_sequence(&import, &existing, 1, MatchDirection::Reverse, &config);
        assert_eq!(result.matched_count, 2);
        assert!(
            result.score > 0.9,
            "perfect reverse match should score high, got {}",
            result.score
        );
    }

    #[test]
    fn sequence_stops_before_length_deviation() {
        let config = SuggestionConfig::default();
        let import = vec![
            MatchShot {
                avg_azimuth: 90.0,
                length: 5.0,
                depth_in: 3.0,
                depth_out: 4.0,
            },
            MatchShot {
                avg_azimuth: 45.0,
                length: 0.6,
                depth_in: 4.0,
                depth_out: 4.1,
            },
        ];
        let existing = vec![
            MatchShot {
                avg_azimuth: 90.0,
                length: 5.0,
                depth_in: 3.0,
                depth_out: 4.0,
            },
            MatchShot {
                avg_azimuth: 45.0,
                length: 3.0,
                depth_in: 4.0,
                depth_out: 4.5,
            },
        ];
        let result = try_match_sequence(&import, &existing, 0, MatchDirection::Forward, &config);
        assert_eq!(result.matched_count, 1);
        assert!(
            !result.is_partial,
            "length-deviating shot should not become a partial match"
        );
        assert!(
            result.score > config.min_aggregate_score,
            "score should be above minimum"
        );
    }

    #[test]
    fn no_match_when_shots_differ() {
        let config = SuggestionConfig::default();
        let import = vec![MatchShot {
            avg_azimuth: 0.0,
            length: 1.0,
            depth_in: 0.0,
            depth_out: 0.5,
        }];
        let existing = vec![MatchShot {
            avg_azimuth: 180.0,
            length: 20.0,
            depth_in: 50.0,
            depth_out: 55.0,
        }];
        let result = try_match_sequence(&import, &existing, 0, MatchDirection::Forward, &config);
        assert_eq!(result.matched_count, 0);
    }

    #[test]
    fn single_shot_match_penalized() {
        let config = SuggestionConfig::default();
        let import = vec![MatchShot {
            avg_azimuth: 90.0,
            length: 5.0,
            depth_in: 3.0,
            depth_out: 4.0,
        }];
        let existing = vec![
            MatchShot {
                avg_azimuth: 90.0,
                length: 5.0,
                depth_in: 3.0,
                depth_out: 4.0,
            },
            MatchShot {
                avg_azimuth: 45.0,
                length: 3.0,
                depth_in: 4.0,
                depth_out: 4.5,
            },
        ];
        let result = try_match_sequence(&import, &existing, 0, MatchDirection::Forward, &config);
        assert_eq!(result.matched_count, 1);
        // Perfect single shot: raw_score=1.0, match_bonus=1.0, penalty=0.6 → 0.6
        assert!(
            (result.score - config.single_shot_penalty).abs() < 1e-6,
            "single shot should be penalized to {}, got {}",
            config.single_shot_penalty,
            result.score,
        );
    }

    #[test]
    fn single_shot_match_requires_strict_fit() {
        let config = SuggestionConfig::default();
        let import = vec![MatchShot {
            avg_azimuth: 99.0,
            length: 5.0,
            depth_in: 3.0,
            depth_out: 4.0,
        }];
        let existing = vec![
            MatchShot {
                avg_azimuth: 90.0,
                length: 5.0,
                depth_in: 3.0,
                depth_out: 4.0,
            },
            MatchShot {
                avg_azimuth: 45.0,
                length: 3.0,
                depth_in: 4.0,
                depth_out: 4.5,
            },
        ];

        assert!(
            compute_shot_score(&import[0], &existing[0], MatchDirection::Forward, &config)
                >= config.min_shot_score,
            "fixture should remain inside normal shot tolerances",
        );

        let result = try_match_sequence(&import, &existing, 0, MatchDirection::Forward, &config);

        assert_eq!(
            result.matched_count, 0,
            "loose one-shot matches should be rejected"
        );
    }

    #[test]
    fn multi_shot_match_uses_normal_first_shot_tolerance() {
        let config = SuggestionConfig::default();
        let import = vec![
            MatchShot {
                avg_azimuth: 99.0,
                length: 5.0,
                depth_in: 3.0,
                depth_out: 4.0,
            },
            MatchShot {
                avg_azimuth: 45.0,
                length: 3.0,
                depth_in: 4.0,
                depth_out: 4.5,
            },
        ];
        let existing = vec![
            MatchShot {
                avg_azimuth: 90.0,
                length: 5.0,
                depth_in: 3.0,
                depth_out: 4.0,
            },
            MatchShot {
                avg_azimuth: 45.0,
                length: 3.0,
                depth_in: 4.0,
                depth_out: 4.5,
            },
        ];

        let result = try_match_sequence(&import, &existing, 0, MatchDirection::Forward, &config);

        assert_eq!(
            result.matched_count, 2,
            "multi-shot matches should keep normal shot tolerances"
        );
    }

    #[test]
    fn marker_station_split_does_not_match_single_target_shot() {
        let config = SuggestionConfig::default();
        let import = vec![
            MatchShot {
                avg_azimuth: 90.0,
                length: 4.0,
                depth_in: 3.0,
                depth_out: 3.4,
            },
            MatchShot {
                avg_azimuth: 90.5,
                length: 6.0,
                depth_in: 3.4,
                depth_out: 4.0,
            },
            MatchShot {
                avg_azimuth: 45.0,
                length: 3.0,
                depth_in: 4.0,
                depth_out: 4.5,
            },
        ];
        let existing = vec![
            MatchShot {
                avg_azimuth: 90.2,
                length: 10.0,
                depth_in: 3.0,
                depth_out: 4.0,
            },
            MatchShot {
                avg_azimuth: 45.0,
                length: 3.0,
                depth_in: 4.0,
                depth_out: 4.5,
            },
        ];

        let result = try_match_sequence(&import, &existing, 0, MatchDirection::Forward, &config);

        assert_eq!(
            result.matched_count, 0,
            "split import shots must not collapse onto one target shot"
        );
    }

    #[test]
    fn slight_kissoff_split_does_not_match_single_target_shot() {
        let config = SuggestionConfig::default();
        let import = vec![
            MatchShot {
                avg_azimuth: 82.0,
                length: 5.0,
                depth_in: 3.0,
                depth_out: 3.45,
            },
            MatchShot {
                avg_azimuth: 98.0,
                length: 5.0,
                depth_in: 3.45,
                depth_out: 4.0,
            },
            MatchShot {
                avg_azimuth: 45.0,
                length: 3.0,
                depth_in: 4.0,
                depth_out: 4.5,
            },
        ];
        let existing = vec![
            MatchShot {
                avg_azimuth: 90.0,
                length: 10.0,
                depth_in: 3.0,
                depth_out: 4.0,
            },
            MatchShot {
                avg_azimuth: 45.0,
                length: 3.0,
                depth_in: 4.0,
                depth_out: 4.5,
            },
        ];

        let result = try_match_sequence(&import, &existing, 0, MatchDirection::Forward, &config);

        assert_eq!(
            result.matched_count, 0,
            "kiss-off splits must not collapse onto one target shot"
        );
    }

    #[test]
    fn station_context_does_not_boost_station_density_mismatch() {
        let config = SuggestionConfig::default();
        let import = vec![MatchShot {
            avg_azimuth: 90.0,
            length: 10.0,
            depth_in: 3.0,
            depth_out: 4.0,
        }];
        let existing = vec![
            MatchShot {
                avg_azimuth: 89.5,
                length: 4.0,
                depth_in: 3.0,
                depth_out: 3.4,
            },
            MatchShot {
                avg_azimuth: 90.5,
                length: 6.0,
                depth_in: 3.4,
                depth_out: 4.0,
            },
        ];

        let multiplier = station_context_multiplier(&import, &existing, 0, 0, &config);

        assert!(
            multiplier <= 1.0,
            "station-density mismatch should not boost 1:1 matches, got {multiplier}"
        );
    }

    #[test]
    fn station_context_penalizes_one_sided_interior_match() {
        let config = SuggestionConfig::default();
        let import = vec![
            MatchShot {
                avg_azimuth: 90.0,
                length: 5.0,
                depth_in: 3.0,
                depth_out: 4.0,
            },
            MatchShot {
                avg_azimuth: 180.0,
                length: 5.0,
                depth_in: 4.0,
                depth_out: 5.0,
            },
        ];
        let existing = vec![
            MatchShot {
                avg_azimuth: 90.0,
                length: 5.0,
                depth_in: 3.0,
                depth_out: 4.0,
            },
            MatchShot {
                avg_azimuth: 270.0,
                length: 5.0,
                depth_in: 4.0,
                depth_out: 5.0,
            },
        ];

        let multiplier = station_context_multiplier(&import, &existing, 1, 1, &config);

        assert!(
            multiplier < 1.0,
            "unsupported interior match should be penalized, got {multiplier}"
        );
    }

    #[test]
    fn crossing_clearance_rejects_shallow_crossings() {
        let config = SuggestionConfig::default();
        let import_trace = vec![
            TracePoint {
                x: 0.0,
                y: 0.0,
                depth: 10.0,
            },
            TracePoint {
                x: 5.0,
                y: -5.0,
                depth: 10.0,
            },
            TracePoint {
                x: 5.0,
                y: 5.0,
                depth: 10.0,
            },
        ];
        let existing_trace = vec![
            TracePoint {
                x: 0.0,
                y: 0.0,
                depth: 11.0,
            },
            TracePoint {
                x: 10.0,
                y: 0.0,
                depth: 11.0,
            },
            TracePoint {
                x: 10.0,
                y: 10.0,
                depth: 11.0,
            },
        ];

        assert!(
            !placement_crossing_clearance_ok(
                &import_trace,
                &existing_trace,
                &CrossingIndex::build(
                    &import_trace,
                    &existing_trace,
                    &straight_runs(&import_trace)
                ),
                0,
                0,
                &[],
                &config
            ),
            "interior line crossing with 1m vertical clearance should be rejected",
        );
    }

    #[test]
    fn crossing_clearance_allows_deep_crossings() {
        let config = SuggestionConfig::default();
        let import_trace = vec![
            TracePoint {
                x: 0.0,
                y: 0.0,
                depth: 16.0,
            },
            TracePoint {
                x: 5.0,
                y: -5.0,
                depth: 16.0,
            },
            TracePoint {
                x: 5.0,
                y: 5.0,
                depth: 16.0,
            },
        ];
        let existing_trace = vec![
            TracePoint {
                x: 0.0,
                y: 0.0,
                depth: 11.0,
            },
            TracePoint {
                x: 10.0,
                y: 0.0,
                depth: 11.0,
            },
            TracePoint {
                x: 10.0,
                y: 10.0,
                depth: 11.0,
            },
        ];

        assert!(
            placement_crossing_clearance_ok(
                &import_trace,
                &existing_trace,
                &CrossingIndex::build(
                    &import_trace,
                    &existing_trace,
                    &straight_runs(&import_trace)
                ),
                0,
                0,
                &[],
                &config
            ),
            "interior line crossing with 5m vertical clearance should be allowed",
        );
    }

    #[test]
    fn crossing_ignore_pairs_include_aligned_shots_and_one_continuation() {
        let forward = PlacementSuggestion {
            existing_section_id: 1,
            existing_station_index: 10,
            import_station_index: 0,
            score: 1.0,
            matched_shots: 2,
            existing_matched_shots: 2,
            preferred_merge_offset: 2,
            direction: MatchDirection::Forward,
            is_partial: false,
            confidence: SuggestionConfidence::High,
            existing_section_name: "target".to_string(),
        };
        assert_eq!(
            ignored_crossing_pairs_for_suggestion(&forward, 5, 20),
            vec![(0, 10), (1, 11), (2, 12)],
        );

        let reverse = PlacementSuggestion {
            existing_station_index: 10,
            direction: MatchDirection::Reverse,
            ..forward
        };
        assert_eq!(
            ignored_crossing_pairs_for_suggestion(&reverse, 5, 20),
            vec![(0, 9), (1, 8), (2, 7)],
        );
    }

    // ========================================================================
    // suggest_placements integration tests
    // ========================================================================

    #[test]
    fn suggest_forward_match_from_station_0() {
        let import_shots = vec![
            make_shot_data(88.0, 92.0, 5.0, 3.0, 4.0),
            make_shot_data(43.0, 47.0, 3.0, 4.0, 4.5),
            make_shot_data(130.0, 135.0, 8.0, 4.5, 7.0),
        ];

        let existing = vec![make_section(
            1,
            "Main Passage",
            vec![
                make_shot(0, 88.0, 92.0, 5.0, 3.0, 4.0),
                make_shot(1, 43.0, 47.0, 3.0, 4.0, 4.5),
                make_shot(2, 200.0, 205.0, 10.0, 4.5, 10.0),
                make_shot(3, 270.0, 275.0, 4.0, 10.0, 11.0),
            ],
        )];

        let suggestions = suggest_placements(
            &import_shots,
            AzimuthReference::default(),
            &existing,
            &PlacementSuggestionConfig::default(),
        );
        assert!(
            !suggestions.is_empty(),
            "should find at least one suggestion"
        );

        let best = &suggestions[0];
        assert_eq!(best.existing_section_id, 1);
        assert_eq!(best.existing_station_index, 0);
        assert_eq!(best.import_station_index, 0);
        assert_eq!(best.direction, MatchDirection::Forward);
        assert!(best.score >= 0.45);
        assert!(best.matched_shots >= 2);
    }

    #[test]
    fn suggest_reverse_match() {
        // Import shots retrace existing shots backwards
        let import_shots = vec![
            // Reversed version of existing shot 1: az 45+180=225, depth swapped
            make_shot_data(223.0, 227.0, 3.0, 4.5, 4.0),
            // Reversed version of existing shot 0: az 90+180=270, depth swapped
            make_shot_data(268.0, 272.0, 5.0, 4.0, 3.0),
            // New shots continuing from here
            make_shot_data(130.0, 135.0, 8.0, 3.0, 5.0),
        ];

        let existing = vec![make_section(
            1,
            "Main Passage",
            vec![
                make_shot(0, 88.0, 92.0, 5.0, 3.0, 4.0),
                make_shot(1, 43.0, 47.0, 3.0, 4.0, 4.5),
                make_shot(2, 200.0, 205.0, 10.0, 4.5, 10.0),
            ],
        )];

        let suggestions = suggest_placements(
            &import_shots,
            AzimuthReference::default(),
            &existing,
            &PlacementSuggestionConfig::default(),
        );
        assert!(!suggestions.is_empty(), "should find reverse match");

        let best = &suggestions[0];
        assert_eq!(best.existing_section_id, 1);
        assert_eq!(best.direction, MatchDirection::Reverse);
        assert_eq!(best.import_station_index, 0);
        // Reverse from shot 1: existing_station_index = 1 + 1 = 2
        assert_eq!(best.existing_station_index, 2);
    }

    #[test]
    fn suggest_match_from_station_n() {
        // Reference shots at the END of the import section
        let import_shots = vec![
            // New shots first
            make_shot_data(130.0, 135.0, 8.0, 20.0, 22.0),
            make_shot_data(300.0, 305.0, 6.0, 22.0, 23.0),
            // Then reference shots matching existing section (forward)
            make_shot_data(88.0, 92.0, 5.0, 3.0, 4.0),
            make_shot_data(43.0, 47.0, 3.0, 4.0, 4.5),
        ];

        let existing = vec![make_section(
            1,
            "Main Passage",
            vec![
                make_shot(0, 88.0, 92.0, 5.0, 3.0, 4.0),
                make_shot(1, 43.0, 47.0, 3.0, 4.0, 4.5),
                make_shot(2, 200.0, 205.0, 10.0, 4.5, 10.0),
            ],
        )];

        let suggestions = suggest_placements(
            &import_shots,
            AzimuthReference::default(),
            &existing,
            &PlacementSuggestionConfig::default(),
        );

        // Should find a match from station N (= 4, last station)
        let station_n_suggestion = suggestions.iter().find(|s| s.import_station_index == 4);
        assert!(
            station_n_suggestion.is_some(),
            "should find match from station N; got suggestions: {:?}",
            suggestions
                .iter()
                .map(|s| (
                    s.import_station_index,
                    s.existing_station_index,
                    s.direction,
                    s.score
                ))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn no_suggestions_for_empty_project() {
        let import_shots = vec![make_shot_data(90.0, 90.0, 5.0, 3.0, 4.0)];
        let suggestions = suggest_placements(
            &import_shots,
            AzimuthReference::default(),
            &[],
            &PlacementSuggestionConfig::default(),
        );
        assert!(suggestions.is_empty());
    }

    #[test]
    fn no_suggestions_for_empty_import() {
        let existing = vec![make_section(
            1,
            "Test",
            vec![make_shot(0, 90.0, 90.0, 5.0, 3.0, 4.0)],
        )];
        let suggestions = suggest_placements(
            &[],
            AzimuthReference::default(),
            &existing,
            &PlacementSuggestionConfig::default(),
        );
        assert!(suggestions.is_empty());
    }

    #[test]
    fn no_match_for_unrelated_sections() {
        let import_shots = vec![
            make_shot_data(0.0, 0.0, 1.0, 0.0, 0.5),
            make_shot_data(0.0, 0.0, 1.0, 0.5, 1.0),
        ];
        let existing = vec![make_section(
            1,
            "Different",
            vec![
                make_shot(0, 180.0, 180.0, 20.0, 50.0, 55.0),
                make_shot(1, 90.0, 90.0, 15.0, 55.0, 60.0),
            ],
        )];
        let suggestions = suggest_placements(
            &import_shots,
            AzimuthReference::default(),
            &existing,
            &PlacementSuggestionConfig::default(),
        );
        // Should have no good suggestions
        assert!(
            suggestions.is_empty() || suggestions[0].score < 0.45,
            "unrelated sections should not match well"
        );
    }

    #[test]
    fn multiple_existing_sections_finds_best() {
        // Reference shots match section 2, not section 1
        let import_shots = vec![
            make_shot_data(175.0, 185.0, 7.0, 10.0, 12.0),
            make_shot_data(260.0, 265.0, 4.0, 12.0, 13.0),
        ];

        let existing = vec![
            make_section(
                1,
                "Wrong Section",
                vec![
                    make_shot(0, 90.0, 90.0, 5.0, 3.0, 4.0),
                    make_shot(1, 45.0, 45.0, 3.0, 4.0, 4.5),
                ],
            ),
            make_section(
                2,
                "Right Section",
                vec![
                    make_shot(0, 175.0, 185.0, 7.0, 10.0, 12.0),
                    make_shot(1, 260.0, 265.0, 4.0, 12.0, 13.0),
                    make_shot(2, 300.0, 305.0, 9.0, 13.0, 16.0),
                ],
            ),
        ];

        let suggestions = suggest_placements(
            &import_shots,
            AzimuthReference::default(),
            &existing,
            &PlacementSuggestionConfig::default(),
        );
        assert!(!suggestions.is_empty());
        assert_eq!(
            suggestions[0].existing_section_id, 2,
            "should match section 2"
        );
    }

    #[test]
    fn confidence_tiers() {
        // Perfect match should be High confidence
        let import_shots = vec![
            make_shot_data(90.0, 90.0, 5.0, 3.0, 4.0),
            make_shot_data(45.0, 45.0, 3.0, 4.0, 4.5),
        ];
        let existing = vec![make_section(
            1,
            "Test",
            vec![
                make_shot(0, 90.0, 90.0, 5.0, 3.0, 4.0),
                make_shot(1, 45.0, 45.0, 3.0, 4.0, 4.5),
                make_shot(2, 180.0, 180.0, 10.0, 4.5, 10.0),
            ],
        )];

        let suggestions = suggest_placements(
            &import_shots,
            AzimuthReference::default(),
            &existing,
            &PlacementSuggestionConfig::default(),
        );
        assert!(!suggestions.is_empty());
        assert_eq!(suggestions[0].confidence, SuggestionConfidence::High);
    }

    #[test]
    fn single_shot_section_can_match() {
        let import_shots = vec![make_shot_data(90.0, 90.0, 5.0, 3.0, 4.0)];
        let existing = vec![make_section(
            1,
            "Test",
            vec![
                make_shot(0, 90.0, 90.0, 5.0, 3.0, 4.0),
                make_shot(1, 45.0, 45.0, 3.0, 4.0, 4.5),
            ],
        )];

        let suggestions = suggest_placements(
            &import_shots,
            AzimuthReference::default(),
            &existing,
            &PlacementSuggestionConfig::default(),
        );
        // A single matching shot should still produce a suggestion, but penalized
        assert!(!suggestions.is_empty(), "single shot should still match");
        assert!(
            suggestions[0].score < 0.7,
            "single shot should be penalized, got {}",
            suggestions[0].score
        );
    }

    #[test]
    fn single_adjacent_shot_can_support_station_alignment() {
        let import_shots = vec![
            // The shot behind station 1 matches the target shot behind station 16.
            make_shot_data(41.8, 45.8, 3.77, 18.30, 17.91),
        ];
        let mut existing_shots = Vec::new();
        for idx in 0..15 {
            existing_shots.push(make_shot(idx, 200.0, 205.0, 0.0, 50.0, 50.0));
        }
        existing_shots.push(make_shot(15, 41.0, 45.0, 3.74, 18.02, 18.04));

        let config = SuggestionConfig::default();
        let reference = AzimuthReference::default();
        let import_match = MatchShot::from_shot_data(&import_shots[0], reference);
        let existing_match = MatchShot::from_shot(&existing_shots[15], reference);
        let single_score = compute_single_shot_score(
            &import_match,
            &existing_match,
            MatchDirection::Forward,
            &config,
        );
        assert!(
            single_score > 0.0,
            "fixture shot should pass the one-shot scorer"
        );
        let import_trace = section_trace_points(&[import_match]);
        let existing_match_shots: Vec<MatchShot> = existing_shots
            .iter()
            .map(|s| MatchShot::from_shot(s, reference))
            .collect();
        let existing_trace = section_trace_points(&existing_match_shots);
        assert!(
            placement_crossing_clearance_ok(
                &import_trace,
                &existing_trace,
                &CrossingIndex::build(
                    &import_trace,
                    &existing_trace,
                    &straight_runs(&import_trace)
                ),
                1,
                16,
                &[(0, 15)],
                &config
            ),
            "fixture placement should pass crossing clearance",
        );

        let suggestions = suggest_placements(
            &import_shots,
            AzimuthReference::default(),
            &[make_section(1, "AC3", existing_shots)],
            &PlacementSuggestionConfig::default(),
        );

        let station_alignment = suggestions.iter().find(|suggestion| {
            suggestion.import_station_index == 0
                && suggestion.existing_station_index == 15
                && suggestion.preferred_merge_offset == 1
        });
        assert!(
            station_alignment.is_some(),
            "one adjacent matched shot should be enough to offer the supported station offset; got {:?}",
            suggestions.iter().map(|s| (s.import_station_index, s.existing_station_index, s.preferred_merge_offset, s.matched_shots, s.score)).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn length_deviation_does_not_become_partial_in_integration() {
        let import_shots = vec![
            make_shot_data(90.0, 90.0, 5.0, 3.0, 4.0),
            make_shot_data(45.0, 45.0, 0.6, 4.0, 4.1),
        ];
        let existing = vec![make_section(
            1,
            "Test",
            vec![
                make_shot(0, 90.0, 90.0, 5.0, 3.0, 4.0),
                make_shot(1, 45.0, 45.0, 3.0, 4.0, 4.5),
            ],
        )];

        let suggestions = suggest_placements(
            &import_shots,
            AzimuthReference::default(),
            &existing,
            &PlacementSuggestionConfig::default(),
        );
        assert!(!suggestions.is_empty());
        let best = &suggestions[0];
        assert!(!best.is_partial, "partial matches should not be emitted");
        assert_eq!(best.matched_shots, 1);
    }

    #[test]
    fn multi_shot_length_mismatch_is_rejected_even_when_azimuths_match() {
        let import_shots = vec![
            make_shot_data(275.3, 285.2, 0.95, 21.40, 21.20),
            make_shot_data(263.6, 262.7, 3.02, 21.17, 21.63),
        ];
        let existing = vec![make_section(
            1,
            "AA1",
            vec![
                make_shot(0, 300.0, 291.1, 2.23, 21.43, 21.70),
                make_shot(1, 269.9, 271.3, 4.47, 21.67, 21.49),
            ],
        )];

        let suggestions = suggest_placements(
            &import_shots,
            AzimuthReference::default(),
            &existing,
            &PlacementSuggestionConfig::default(),
        );

        assert!(
            suggestions.is_empty(),
            "AA2-style length mismatch should not be a valid candidate: {suggestions:?}",
        );
    }

    #[test]
    fn length_tolerance_scales_with_matched_shot_count() {
        let config = SuggestionConfig::default();
        let existing = vec![
            MatchShot {
                avg_azimuth: 90.0,
                length: 20.0,
                depth_in: 3.0,
                depth_out: 4.0,
            },
            MatchShot {
                avg_azimuth: 45.0,
                length: 20.0,
                depth_in: 4.0,
                depth_out: 5.0,
            },
            MatchShot {
                avg_azimuth: 120.0,
                length: 20.0,
                depth_in: 5.0,
                depth_out: 6.0,
            },
        ];

        let single_import = vec![MatchShot {
            length: 20.9,
            ..existing[0]
        }];
        let single = try_match_sequence(
            &single_import,
            &existing,
            0,
            MatchDirection::Forward,
            &config,
        );
        assert_eq!(
            single.matched_count, 0,
            "single-shot match should keep the strict 3% length gate"
        );

        let two_import = vec![
            MatchShot {
                length: 20.9,
                ..existing[0]
            },
            MatchShot {
                length: 20.9,
                ..existing[1]
            },
        ];
        let two = try_match_sequence(&two_import, &existing, 0, MatchDirection::Forward, &config);
        assert_eq!(
            two.matched_count, 2,
            "two-shot match should allow a 5%-class length error"
        );

        let too_loose_for_two = vec![
            MatchShot {
                length: 21.2,
                ..existing[0]
            },
            MatchShot {
                length: 21.2,
                ..existing[1]
            },
        ];
        let rejected_two = try_match_sequence(
            &too_loose_for_two,
            &existing,
            0,
            MatchDirection::Forward,
            &config,
        );
        assert_eq!(
            rejected_two.matched_count, 0,
            "two-shot match should still reject a 7%-class length error"
        );

        let three_import = vec![
            MatchShot {
                length: 21.2,
                ..existing[0]
            },
            MatchShot {
                length: 21.2,
                ..existing[1]
            },
            MatchShot {
                length: 21.2,
                ..existing[2]
            },
        ];
        let three = try_match_sequence(
            &three_import,
            &existing,
            0,
            MatchDirection::Forward,
            &config,
        );
        assert_eq!(
            three.matched_count, 3,
            "three-shot match should allow a 7%-class length error"
        );
    }

    // ========================================================================
    // Hard cutoff boundary tests
    // ========================================================================

    #[test]
    fn full_shot_azimuth_cutoff_boundary() {
        let config = SuggestionConfig::default();
        let base = MatchShot {
            avg_azimuth: 90.0,
            length: 5.0,
            depth_in: 3.0,
            depth_out: 4.0,
        };

        // 24.9° - just under cutoff (max_azimuth_error_full=25.0), should pass
        let import_pass = MatchShot {
            avg_azimuth: 90.0 + 24.9,
            ..base
        };
        let score = compute_shot_score(&import_pass, &base, MatchDirection::Forward, &config);
        assert!(
            score > 0.0,
            "24.9° azimuth error should pass cutoff, got {score}"
        );

        // 25.1° - just over cutoff, should be rejected
        let import_fail = MatchShot {
            avg_azimuth: 90.0 + 25.1,
            ..base
        };
        let score = compute_shot_score(&import_fail, &base, MatchDirection::Forward, &config);
        assert!(
            score == 0.0,
            "25.1° azimuth error should be hard-rejected, got {score}"
        );
    }

    #[test]
    fn full_shot_length_cutoff_boundary() {
        let config = SuggestionConfig::default();
        let existing = MatchShot {
            avg_azimuth: 90.0,
            length: 5.0,
            depth_in: 3.0,
            depth_out: 4.0,
        };

        let import_pass = MatchShot {
            length: 5.39,
            ..existing
        };
        let score = compute_shot_score(&import_pass, &existing, MatchDirection::Forward, &config);
        assert!(
            score > 0.0,
            "0.39m length error should pass cutoff, got {score}"
        );

        let import_fail = MatchShot {
            length: 5.41,
            ..existing
        };
        let score = compute_shot_score(&import_fail, &existing, MatchDirection::Forward, &config);
        assert!(
            score == 0.0,
            "0.41m length error should be hard-rejected, got {score}"
        );
    }

    #[test]
    fn depth_cutoff_boundary() {
        let config = SuggestionConfig::default();
        let base = MatchShot {
            avg_azimuth: 90.0,
            length: 5.0,
            depth_in: 3.0,
            depth_out: 4.0,
        };

        // 0.29m depth_in error - just under cutoff (max_depth_error=0.30)
        let import_pass = MatchShot {
            depth_in: 3.29,
            ..base
        };
        let score = compute_shot_score(&import_pass, &base, MatchDirection::Forward, &config);
        assert!(
            score > 0.0,
            "0.29m depth error should pass cutoff, got {score}"
        );

        // 0.31m depth_in error - just over cutoff
        let import_fail = MatchShot {
            depth_in: 3.31,
            ..base
        };
        let score = compute_shot_score(&import_fail, &base, MatchDirection::Forward, &config);
        assert!(
            score == 0.0,
            "0.31m depth error should be hard-rejected, got {score}"
        );
    }

    #[test]
    fn explicit_placement_tolerances_are_honored() {
        let mut ps_config = PlacementSuggestionConfig::default();
        ps_config.max_depth_error = 1.0;
        ps_config.max_length_error_fraction = 0.15;
        ps_config.max_length_error_floor = 2.0;
        let config = SuggestionConfig::from(&ps_config);

        let base = MatchShot {
            avg_azimuth: 90.0,
            length: 10.0,
            depth_in: 3.0,
            depth_out: 4.0,
        };

        let depth_loose = MatchShot {
            depth_in: 3.8,
            ..base
        };
        let score = compute_shot_score(&depth_loose, &base, MatchDirection::Forward, &config);
        assert!(
            score > 0.0,
            "explicit 1.0m depth tolerance should be honored, got {score}"
        );

        let length_loose = MatchShot {
            length: 11.4,
            ..base
        };
        let score = compute_shot_score(&length_loose, &base, MatchDirection::Forward, &config);
        assert!(
            score > 0.0,
            "explicit 15% length tolerance should be honored, got {score}"
        );
    }

    #[test]
    fn length_cutoff_uses_floor_for_short_shots() {
        let config = SuggestionConfig::default();
        // Short existing shot (2.0m): 3% = 0.06m, but floor is 0.4m
        let existing = MatchShot {
            avg_azimuth: 90.0,
            length: 2.0,
            depth_in: 3.0,
            depth_out: 4.0,
        };

        // 0.39m error - under the 0.4m floor
        let import_pass = MatchShot {
            length: 2.39,
            ..existing
        };
        let score = compute_shot_score(&import_pass, &existing, MatchDirection::Forward, &config);
        assert!(
            score > 0.0,
            "0.39m length error should pass floor cutoff, got {score}"
        );

        // 0.41m error - over the 0.4m floor
        let import_fail = MatchShot {
            length: 2.41,
            ..existing
        };
        let score = compute_shot_score(&import_fail, &existing, MatchDirection::Forward, &config);
        assert!(
            score == 0.0,
            "0.41m length error should be hard-rejected by floor, got {score}"
        );
    }

    #[test]
    fn length_cutoff_scales_for_long_shots() {
        let config = SuggestionConfig::default();
        // Long existing shot (20.0m): 3% = 0.6m > 0.4m floor
        let existing = MatchShot {
            avg_azimuth: 90.0,
            length: 20.0,
            depth_in: 3.0,
            depth_out: 4.0,
        };

        // 0.59m error - under the 0.6m (3%) cutoff
        let import_pass = MatchShot {
            length: 20.59,
            ..existing
        };
        let score = compute_shot_score(&import_pass, &existing, MatchDirection::Forward, &config);
        assert!(
            score > 0.0,
            "0.59m length error should pass 3% cutoff for 20m shot, got {score}"
        );

        // 0.61m error - over the 0.6m (3%) cutoff
        let import_fail = MatchShot {
            length: 20.61,
            ..existing
        };
        let score = compute_shot_score(&import_fail, &existing, MatchDirection::Forward, &config);
        assert!(
            score == 0.0,
            "0.61m length error should be hard-rejected by 3% cutoff, got {score}"
        );
    }

    #[test]
    fn deduplication_keeps_best_score() {
        // Two sequences might match the same position - should keep only the best
        let import_shots = vec![make_shot_data(90.0, 90.0, 5.0, 3.0, 4.0)];
        let existing = vec![make_section(
            1,
            "Test",
            vec![make_shot(0, 90.0, 90.0, 5.0, 3.0, 4.0)],
        )];

        let suggestions = suggest_placements(
            &import_shots,
            AzimuthReference::default(),
            &existing,
            &PlacementSuggestionConfig::default(),
        );
        // Should not have duplicate entries for the same position
        let count = suggestions
            .iter()
            .filter(|s| {
                s.existing_section_id == 1
                    && s.existing_station_index == 0
                    && s.import_station_index == 0
                    && s.direction == MatchDirection::Forward
            })
            .count();
        assert!(count <= 1, "should not have duplicate suggestions");
    }

    #[test]
    fn equivalent_single_front_reverse_is_suppressed() {
        let forward = PlacementSuggestion {
            existing_section_id: 1,
            existing_station_index: 10,
            import_station_index: 0,
            score: 0.580,
            matched_shots: 1,
            existing_matched_shots: 1,
            preferred_merge_offset: 1,
            direction: MatchDirection::Forward,
            is_partial: false,
            confidence: SuggestionConfidence::Low,
            existing_section_name: "AA1".to_string(),
        };
        let reverse = PlacementSuggestion {
            existing_section_id: 1,
            existing_station_index: 11,
            import_station_index: 0,
            score: 0.581,
            matched_shots: 1,
            existing_matched_shots: 1,
            preferred_merge_offset: 1,
            direction: MatchDirection::Reverse,
            is_partial: true,
            confidence: SuggestionConfidence::Low,
            existing_section_name: "AA1".to_string(),
        };
        let mut suggestions = vec![reverse, forward.clone()];

        suppress_equivalent_single_front_reverses(&mut suggestions);

        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].direction, MatchDirection::Forward);
        assert_eq!(
            suggestions[0].existing_station_index,
            forward.existing_station_index
        );
    }

    #[test]
    fn duplicate_endpoint_pairs_keep_best_score() {
        let forward = PlacementSuggestion {
            existing_section_id: 1,
            existing_station_index: 10,
            import_station_index: 0,
            score: 0.8,
            matched_shots: 2,
            existing_matched_shots: 2,
            preferred_merge_offset: 2,
            direction: MatchDirection::Forward,
            is_partial: false,
            confidence: SuggestionConfidence::Medium,
            existing_section_name: "AA1".to_string(),
        };
        let reverse = PlacementSuggestion {
            existing_section_id: 1,
            existing_station_index: 12,
            import_station_index: 0,
            score: 0.9,
            matched_shots: 2,
            existing_matched_shots: 2,
            preferred_merge_offset: 2,
            direction: MatchDirection::Reverse,
            is_partial: false,
            confidence: SuggestionConfidence::High,
            existing_section_name: "AA1".to_string(),
        };
        let mut suggestions = vec![forward, reverse.clone()];

        suppress_duplicate_endpoint_pairs(&mut suggestions);

        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].direction, MatchDirection::Reverse);
        assert_eq!(suggestions[0].score, reverse.score);
    }
}
