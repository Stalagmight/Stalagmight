use serde::{Deserialize, Serialize};

use crate::models::config::PlacementSuggestionConfig;

use super::{horizontal_length_from_slope, AzimuthReference, LineId, Shot, ShotData};

/// Direction of match between import and existing shots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MatchDirection {
    /// Import shots match existing shots in the same order.
    Forward,
    /// Import shots retrace existing shots backwards.
    Reverse,
}

/// Confidence tier for a placement suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SuggestionConfidence {
    High,
    Medium,
    Low,
}

/// A suggested placement for an import line based on reference shot overlap.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacementSuggestion {
    /// The existing line that the import overlaps with.
    pub existing_line_id: LineId,
    /// Station index in the existing line to merge with.
    pub existing_station_index: usize,
    /// Station index in the import line (0 = first, N = last).
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
    /// Human-readable name of the existing line.
    pub existing_line_name: String,
}

/// Whether the target of a cross-line suggestion is an existing project
/// line or another line being imported in the same batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TargetType {
    Existing,
    Importing,
}

/// A placement suggestion that includes source/target metadata for cross-line
/// matching (import-to-existing AND import-to-import).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrossLineSuggestion {
    /// Station index in the existing/target line to merge with.
    pub existing_station_index: usize,
    /// Station index in the import line (0 = first, N = last).
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
    /// Human-readable name of the target line.
    pub existing_line_name: String,
    /// Index of the import line this suggestion is for.
    pub import_line_index: u32,
    /// Name of the import line.
    pub import_line_name: String,
    /// Whether the target is an existing project line or another import.
    pub target_type: TargetType,
    /// For existing targets: the line ID.
    pub existing_line_id: Option<LineId>,
    /// For importing targets: the index of the target import line.
    pub target_import_index: Option<u32>,
}

/// A suggestion for connecting an orphaned line leaf to a newly imported line.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrphanPlacementSuggestion {
    /// The orphan line ID
    pub orphan_line_id: LineId,
    /// Human-readable name of the orphan line
    pub orphan_line_name: String,
    /// Station index on the orphan line's leaf end
    pub orphan_station_index: usize,
    /// The target (newly imported) line ID
    pub target_line_id: LineId,
    /// Human-readable name of the target line
    pub target_line_name: String,
    /// Station index on the target line to merge with
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

/// Minimal per-line data needed for the cross-line suggestion algorithm.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportLineForSuggestion {
    pub index: u32,
    pub name: String,
    pub shots: Vec<ShotData>,
    /// What THIS line's readings are measured against (#438), as the
    /// importer that produced them declared it on its `ImportLineConfig`.
    ///
    /// The matcher compares bearings across the import/existing boundary, so a
    /// line scored in the wrong frame mismatches a correct overlap by the
    /// difference of the two corrections - 5 degrees is already outside the
    /// default `max_azimuth_error_full`. `None` falls back to the project
    /// default, which is what a caller with nothing better to say sends.
    #[serde(default)]
    pub azimuth_reference: Option<AzimuthReference>,
}

/// Suggest placements for ALL import lines against BOTH existing project
/// lines AND each other. Returns a flat list sorted by score descending.
///
/// Each import line is scored in ITS OWN frame
/// ([`ImportLineForSuggestion::azimuth_reference`]); `fallback_reference` is
/// what a line that declares none is read against, normally the project
/// default (#438). Every comparison therefore has both sides in true north:
/// import-to-existing uses each existing line's own reference, and
/// import-to-import uses each import line's.
pub fn suggest_all_placements(
    import_lines: &[ImportLineForSuggestion],
    fallback_reference: AzimuthReference,
    existing_lines: &[ExistingLine],
    ps_config: &PlacementSuggestionConfig,
) -> Vec<CrossLineSuggestion> {
    let mut all: Vec<CrossLineSuggestion> = Vec::new();

    // Use high IDs for import lines so they don't collide with real line IDs.
    // Map: fake_id = FAKE_ID_BASE + import.index
    const FAKE_ID_BASE: LineId = 0x8000_0000;

    let import_as_existing: Vec<ExistingLine> = import_lines
        .iter()
        .map(|s| ExistingLine {
            id: FAKE_ID_BASE + LineId::from(s.index),
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

    for import in import_lines {
        if import.shots.is_empty() {
            continue;
        }

        let fake_self_id = FAKE_ID_BASE + LineId::from(import.index);
        let import_reference = import.azimuth_reference.unwrap_or(fallback_reference);

        // Match against existing project lines
        let existing_suggestions =
            suggest_placements(&import.shots, import_reference, existing_lines, ps_config);
        for s in existing_suggestions {
            all.push(CrossLineSuggestion {
                existing_station_index: s.existing_station_index,
                import_station_index: s.import_station_index,
                score: s.score,
                matched_shots: s.matched_shots,
                existing_matched_shots: s.existing_matched_shots,
                preferred_merge_offset: s.preferred_merge_offset,
                direction: s.direction,
                is_partial: s.is_partial,
                confidence: s.confidence,
                existing_line_name: s.existing_line_name,
                import_line_index: import.index,
                import_line_name: import.name.clone(),
                target_type: TargetType::Existing,
                existing_line_id: Some(s.existing_line_id),
                target_import_index: None,
            });
        }

        // Match against other import lines (exclude self)
        let other_imports: Vec<ExistingLine> = import_as_existing
            .iter()
            .filter(|e| e.id != fake_self_id)
            .cloned()
            .collect();

        if !other_imports.is_empty() {
            let import_suggestions =
                suggest_placements(&import.shots, import_reference, &other_imports, ps_config);
            for s in import_suggestions {
                // Decode the fake ID back to an import index
                let target_idx = (s.existing_line_id - FAKE_ID_BASE) as u32;
                let target_name = import_lines
                    .iter()
                    .find(|is| is.index == target_idx)
                    .map(|is| is.name.clone())
                    .unwrap_or_default();

                all.push(CrossLineSuggestion {
                    existing_station_index: s.existing_station_index,
                    import_station_index: s.import_station_index,
                    score: s.score,
                    matched_shots: s.matched_shots,
                    existing_matched_shots: s.existing_matched_shots,
                    preferred_merge_offset: s.preferred_merge_offset,
                    direction: s.direction,
                    is_partial: s.is_partial,
                    confidence: s.confidence,
                    existing_line_name: target_name,
                    import_line_index: import.index,
                    import_line_name: import.name.clone(),
                    target_type: TargetType::Importing,
                    existing_line_id: None,
                    target_import_index: Some(target_idx),
                });
            }
        }
    }

    // Sort by score descending
    all.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

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
    /// incoming line against lines already in the project, and those two
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

fn line_trace_points(shots: &[MatchShot]) -> Vec<TracePoint> {
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

fn segment_intersection_params(
    a0: TracePoint,
    a1: TracePoint,
    b0: TracePoint,
    b1: TracePoint,
) -> Option<(f64, f64)> {
    const EPS: f64 = 1e-9;
    const ENDPOINT_EPS: f64 = 1e-6;

    let ax = a1.x - a0.x;
    let ay = a1.y - a0.y;
    let bx = b1.x - b0.x;
    let by = b1.y - b0.y;
    let denom = ax * by - ay * bx;
    if denom.abs() < EPS {
        return None;
    }

    let cx = b0.x - a0.x;
    let cy = b0.y - a0.y;
    let t = (cx * by - cy * bx) / denom;
    let u = (cx * ay - cy * ax) / denom;

    if t > ENDPOINT_EPS && t < 1.0 - ENDPOINT_EPS && u > ENDPOINT_EPS && u < 1.0 - ENDPOINT_EPS {
        Some((t, u))
    } else {
        None
    }
}

fn interpolate_depth(a: TracePoint, b: TracePoint, t: f64) -> f64 {
    a.depth + (b.depth - a.depth) * t
}

fn placement_crossing_clearance_ok(
    import_trace: &[TracePoint],
    existing_trace: &[TracePoint],
    import_station_index: usize,
    existing_station_index: usize,
    ignored_pairs: &[(usize, usize)],
    config: &SuggestionConfig,
) -> bool {
    if import_station_index >= import_trace.len() || existing_station_index >= existing_trace.len()
    {
        return false;
    }

    let dx = existing_trace[existing_station_index].x - import_trace[import_station_index].x;
    let dy = existing_trace[existing_station_index].y - import_trace[import_station_index].y;
    let placed_import: Vec<TracePoint> = import_trace
        .iter()
        .map(|point| point.translated(dx, dy))
        .collect();

    for import_idx in 0..placed_import.len().saturating_sub(1) {
        let import_a = placed_import[import_idx];
        let import_b = placed_import[import_idx + 1];
        for existing_idx in 0..existing_trace.len().saturating_sub(1) {
            if ignored_pairs.contains(&(import_idx, existing_idx)) {
                continue;
            }
            let existing_a = existing_trace[existing_idx];
            let existing_b = existing_trace[existing_idx + 1];
            let Some((import_t, existing_t)) =
                segment_intersection_params(import_a, import_b, existing_a, existing_b)
            else {
                continue;
            };

            let import_depth = interpolate_depth(import_a, import_b, import_t);
            let existing_depth = interpolate_depth(existing_a, existing_b, existing_t);
            if (import_depth - existing_depth).abs() < config.min_crossing_depth_clearance {
                return false;
            }
        }
    }

    true
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

/// The existing shots read away from `start_idx` in `direction`, at most
/// `limit` of them.
fn oriented_existing_shots(
    existing_shots: &[MatchShot],
    start_idx: usize,
    direction: MatchDirection,
    limit: usize,
) -> Vec<MatchShot> {
    match direction {
        MatchDirection::Forward => existing_shots[start_idx..]
            .iter()
            .take(limit)
            .cloned()
            .collect(),
        MatchDirection::Reverse => existing_shots[..=start_idx]
            .iter()
            .rev()
            .take(limit)
            .map(MatchShot::reversed)
            .collect(),
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
) -> Option<(LineId, usize, usize, usize, usize)> {
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
        suggestion.existing_line_id,
        import_lo,
        import_hi,
        existing_lo,
        existing_hi,
    ))
}

fn suppress_equivalent_single_front_reverses(suggestions: &mut Vec<PlacementSuggestion>) {
    const SCORE_EPSILON: f64 = 0.02;

    let forward_keys: Vec<(LineId, (usize, usize), f64)> = suggestions
        .iter()
        .filter(|suggestion| suggestion.direction == MatchDirection::Forward)
        .filter_map(|suggestion| {
            single_front_overlap_key(suggestion)
                .map(|key| (suggestion.existing_line_id, key, suggestion.score))
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
            .any(|(line_id, other_key, other_score)| {
                *line_id == candidate.existing_line_id
                    && *other_key == key
                    && *other_score + SCORE_EPSILON >= candidate.score
            })
    });
}

fn suppress_duplicate_endpoint_pairs(suggestions: &mut Vec<PlacementSuggestion>) {
    let mut kept: Vec<PlacementSuggestion> = Vec::with_capacity(suggestions.len());

    for suggestion in suggestions.drain(..) {
        let Some(key) = suggestion_endpoint_key(&suggestion) else {
            kept.push(suggestion);
            continue;
        };

        if let Some(existing) = kept
            .iter_mut()
            .find(|candidate| suggestion_endpoint_key(candidate) == Some(key))
        {
            if suggestion.score > existing.score {
                *existing = suggestion;
            }
        } else {
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

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
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

    // The only pairing is import shot `k` with oriented existing shot `k`, so
    // the matched prefixes are the whole search space: walk them in order and
    // stop at the first pair under the per-shot floor. Only the first
    // `import_shots.len()` oriented shots can ever be paired, so no more are
    // built - copying the rest of a long line at every start made each
    // line's scan quadratic in its length (#674).
    let existing_oriented =
        oriented_existing_shots(existing_shots, start_idx, direction, import_shots.len());
    let mut candidates: Vec<DpCell> = Vec::new();
    let mut cell = DpCell {
        score_sum: 0.0,
        weight_sum: 0.0,
        import_count: 0,
        existing_count: 0,
    };
    for (import_shot, existing_shot) in import_shots.iter().zip(&existing_oriented) {
        let score = compute_exploratory_shot_score(
            import_shot,
            existing_shot,
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
        candidates.push(cell);
    }

    candidates.sort_by(|a, b| {
        cell_final_score(*b, config)
            .partial_cmp(&cell_final_score(*a, config))
            .unwrap_or(std::cmp::Ordering::Equal)
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

/// Existing line data needed for matching.
#[derive(Clone)]
pub struct ExistingLine {
    pub id: LineId,
    pub name: String,
    pub shots: Vec<Shot>,
    /// The line's resolved north reference (#438), so its bearings can be
    /// compared with an import's in one frame. Callers get it from
    /// [`Project::azimuth_reference_for`](crate::models::models::Project::azimuth_reference_for).
    pub azimuth_reference: AzimuthReference,
}

/// Suggest placements for an import line against existing lines.
pub fn suggest_placements(
    import_shots: &[ShotData],
    import_reference: AzimuthReference,
    existing_lines: &[ExistingLine],
    ps_config: &PlacementSuggestionConfig,
) -> Vec<PlacementSuggestion> {
    let config = &SuggestionConfig::from(ps_config);

    if import_shots.is_empty() || existing_lines.is_empty() {
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
    let import_trace = line_trace_points(&import_match_shots);

    // Build import match shots for station N (last N shots, reversed)
    let import_from_n: Vec<MatchShot> = import_shots[import_shots.len().saturating_sub(max_ref)..]
        .iter()
        .rev()
        .map(|s| MatchShot::from_shot_data(s, import_reference).reversed())
        .collect();

    let import_station_n = import_shots.len(); // station index for last station

    let mut suggestions: Vec<PlacementSuggestion> = Vec::new();

    for existing in existing_lines {
        if existing.shots.is_empty() {
            continue;
        }

        let existing_match_shots: Vec<MatchShot> = existing
            .shots
            .iter()
            .map(|s| MatchShot::from_shot(s, existing.azimuth_reference))
            .collect();
        let existing_trace = line_trace_points(&existing_match_shots);

        // Try from both leaves of the import line
        let leaf_configs: &[(&[MatchShot], usize)] =
            &[(&import_from_0, 0), (&import_from_n, import_station_n)];

        for &(ref_shots, import_station) in leaf_configs {
            if ref_shots.is_empty() {
                continue;
            }

            // Try all starting positions in the existing line
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

                    if result.matched_count == 0 || result.score < config.min_aggregate_score {
                        continue;
                    }

                    // Compute the existing station index for the merge point
                    let existing_station_index = match direction {
                        MatchDirection::Forward => start_idx,
                        MatchDirection::Reverse => start_idx + 1,
                    };

                    let mut suggestion = PlacementSuggestion {
                        existing_line_id: existing.id,
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
                        existing_line_name: existing.name.clone(),
                    };

                    let mut best_context: Option<(f64, usize)> = None;
                    let ignored_crossing_pairs = ignored_crossing_pairs_for_suggestion(
                        &suggestion,
                        import_match_shots.len(),
                        existing_match_shots.len(),
                    );
                    for (import_offset, existing_offset) in merge_offset_pairs(&suggestion) {
                        let Some((merge_import_station, merge_existing_station)) =
                            merge_station_pair_for_offsets(
                                import_match_shots.len(),
                                existing_match_shots.len(),
                                &suggestion,
                                import_offset,
                                existing_offset,
                            )
                        else {
                            continue;
                        };
                        if !placement_crossing_clearance_ok(
                            &import_trace,
                            &existing_trace,
                            merge_import_station,
                            merge_existing_station,
                            &ignored_crossing_pairs,
                            config,
                        ) {
                            continue;
                        }
                        let context_multiplier = station_context_multiplier(
                            &import_match_shots,
                            &existing_match_shots,
                            merge_import_station,
                            merge_existing_station,
                            config,
                        );
                        let context_multiplier = if suggestion.matched_shots == 1 {
                            context_multiplier.max(1.0)
                        } else {
                            context_multiplier
                        };
                        if best_context
                            .map(|(best, best_offset)| {
                                context_multiplier > best
                                    || ((context_multiplier - best).abs() < 1e-9
                                        && import_offset > best_offset)
                            })
                            .unwrap_or(true)
                        {
                            best_context = Some((context_multiplier, import_offset));
                        }
                    }
                    let Some((context_multiplier, preferred_offset)) = best_context else {
                        continue;
                    };
                    suggestion.preferred_merge_offset = preferred_offset;
                    suggestion.score *= context_multiplier;

                    if suggestion.score < config.min_aggregate_score {
                        continue;
                    }

                    suggestion.confidence = if suggestion.score >= 0.90 {
                        SuggestionConfidence::High
                    } else if suggestion.score >= 0.70 {
                        SuggestionConfidence::Medium
                    } else {
                        SuggestionConfidence::Low
                    };

                    // Check for duplicates
                    let is_dup = suggestions.iter().any(|s| {
                        s.existing_line_id == suggestion.existing_line_id
                            && s.existing_station_index == suggestion.existing_station_index
                            && s.import_station_index == suggestion.import_station_index
                            && s.direction == suggestion.direction
                    });
                    if is_dup {
                        // Keep the better score
                        if let Some(existing_suggestion) = suggestions.iter_mut().find(|s| {
                            s.existing_line_id == suggestion.existing_line_id
                                && s.existing_station_index == suggestion.existing_station_index
                                && s.import_station_index == suggestion.import_station_index
                                && s.direction == suggestion.direction
                        }) {
                            if suggestion.score > existing_suggestion.score {
                                *existing_suggestion = suggestion;
                            }
                        }
                        continue;
                    }

                    suggestions.push(suggestion);
                }
            }
        }
    }

    suppress_equivalent_single_front_reverses(&mut suggestions);
    suppress_duplicate_endpoint_pairs(&mut suggestions);

    // Sort by score descending
    suggestions.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Keep top N
    suggestions.truncate(config.max_suggestions);

    suggestions
}
