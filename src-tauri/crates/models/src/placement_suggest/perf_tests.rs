//! [#576 A2] Placement suggestions: pinned output and bounded work.
//!
//! `suggest_placements` runs for every existing section x both import leaves x
//! every start index x both directions, so anything per call that scales with
//! the existing section's length makes the whole pass quadratic in it. These
//! tests pin two things: that the suggestions themselves do not change (a
//! golden list captured from the matrix implementation, plus randomized
//! comparisons against copies of the old routines kept here as oracles), and
//! that the work per call no longer scales with the section length.

use super::crossing::{
    crossing_prefilter_margin, placement_crossing_clearance_ok, straight_runs, CrossingIndex,
    CrossingLane,
};
use super::{
    best_merge_offset, section_trace_points, station_context_multiplier, suggest_all_placements,
    try_match_sequence, work, CrossSectionSuggestion, ExistingSection, ImportSectionForSuggestion,
    MatchDirection, MatchShot, PlacementSuggestion, SuggestionConfidence, SuggestionConfig,
    TracePoint,
};
use crate::config::PlacementSuggestionConfig;
use crate::test_fixtures::straight_shot;
use crate::{AzimuthReference, SectionId, Shot, ShotData};

/// xorshift64* - deterministic, dependency-free.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
}

/// A passage-like random walk: the heading wanders by up to 25 degrees a leg,
/// 1-10 m legs, the depth drifting.
fn random_run(rng: &mut Rng, len: usize) -> Vec<ShotData> {
    let mut depth = rng.range(5.0, 40.0);
    let mut heading = rng.range(0.0, 360.0);
    (0..len)
        .map(|_| {
            heading = (heading + rng.range(-25.0, 25.0)).rem_euclid(360.0);
            let next = (depth + rng.range(-2.0, 2.0)).max(0.0);
            let s = straight_shot(heading, rng.range(1.0, 10.0)).with_depths(depth, next);
            depth = next;
            s
        })
        .collect()
}

/// A dead-straight passage at one depth: every shot matches every other, so
/// every start index survives the matcher and reaches the crossing check.
fn straight_run(len: usize, azimuth: f64) -> Vec<ShotData> {
    (0..len)
        .map(|_| straight_shot(azimuth, 5.0).with_depths(10.0, 10.0))
        .collect()
}

fn existing(id: SectionId, shots: &[ShotData]) -> ExistingSection {
    ExistingSection {
        id,
        name: format!("CaveA{id}"),
        shots: shots
            .iter()
            .enumerate()
            .map(|(i, sd)| Shot::new(i as u32, sd.clone()))
            .collect(),
        azimuth_reference: AzimuthReference::default(),
    }
}

fn import(index: u32, shots: Vec<ShotData>) -> ImportSectionForSuggestion {
    ImportSectionForSuggestion {
        index,
        name: format!("Import{index}"),
        shots,
        azimuth_reference: None,
    }
}

/// A copy of `source[from..from + len]`, perturbed by survey-sized noise, so
/// the import genuinely overlaps an existing passage.
fn noisy_copy(rng: &mut Rng, source: &[ShotData], from: usize, len: usize) -> Vec<ShotData> {
    source[from..from + len]
        .iter()
        .map(|s| {
            let az = (s.azimuth_in + rng.range(-1.5, 1.5)).rem_euclid(360.0);
            let mut copy = straight_shot(az, s.length + rng.range(-0.05, 0.05)).with_depths(
                s.depth_in + rng.range(-0.05, 0.05),
                s.depth_out + rng.range(-0.05, 0.05),
            );
            copy.azimuth_out = az;
            copy
        })
        .collect()
}

/// The fixed scene the golden list is captured on: existing passages, imports
/// that tie into them at either end (forward and reversed), imports that tie
/// into each other, and pure noise.
fn golden_scene() -> (Vec<ImportSectionForSuggestion>, Vec<ExistingSection>) {
    let mut rng = Rng(0x5EED_A2A2_0576_0001);
    let passages: Vec<Vec<ShotData>> = (0..6).map(|_| random_run(&mut rng, 30)).collect();
    let existing_sections: Vec<ExistingSection> = passages
        .iter()
        .enumerate()
        .map(|(i, p)| existing(i as SectionId + 1, p))
        .collect();

    let mut imports = Vec::new();
    // Tie-ins that start ON an existing passage and run off it.
    for (k, p) in passages.iter().enumerate().take(4) {
        let mut shots = noisy_copy(&mut rng, p, 3 + 4 * k, 4);
        shots.extend(random_run(&mut rng, 12));
        imports.push(import(imports.len() as u32, shots));
    }
    // Tie-ins whose END lies on a passage (the station-N leaf).
    for p in passages.iter().skip(4) {
        let mut shots = random_run(&mut rng, 10);
        shots.extend(noisy_copy(&mut rng, p, 20, 5));
        imports.push(import(imports.len() as u32, shots));
    }
    // A reversed overlap: walk an existing stretch backwards.
    let mut reversed: Vec<ShotData> = noisy_copy(&mut rng, &passages[1], 10, 5)
        .into_iter()
        .rev()
        .map(|s| {
            let az = (s.azimuth_in + 180.0).rem_euclid(360.0);
            let mut r = straight_shot(az, s.length).with_depths(s.depth_out, s.depth_in);
            r.azimuth_out = az;
            r
        })
        .collect();
    reversed.extend(random_run(&mut rng, 8));
    imports.push(import(imports.len() as u32, reversed));
    // Two imports that share a stretch with each other (import-to-import).
    let shared = random_run(&mut rng, 6);
    let mut first = shared.clone();
    first.extend(random_run(&mut rng, 10));
    let mut second = noisy_copy(&mut rng, &shared, 0, 6);
    second.extend(random_run(&mut rng, 10));
    imports.push(import(imports.len() as u32, first));
    imports.push(import(imports.len() as u32, second));
    // Noise, and an empty section (skipped by the matcher).
    imports.push(import(imports.len() as u32, random_run(&mut rng, 25)));
    imports.push(import(imports.len() as u32, Vec::new()));
    (imports, existing_sections)
}

/// One suggestion per line, every float by its exact bits.
fn fingerprint(suggestions: &[CrossSectionSuggestion]) -> String {
    suggestions
        .iter()
        .map(|s| {
            format!(
                "{}:{}->{:?}/{:?}/{:?} e{} i{} m{} em{} p{} {:?} {} {:?} {:016x}",
                s.import_section_index,
                s.import_section_name,
                s.target_type,
                s.existing_section_id,
                s.target_import_index,
                s.existing_station_index,
                s.import_station_index,
                s.matched_shots,
                s.existing_matched_shots,
                s.preferred_merge_offset,
                s.direction,
                s.is_partial,
                s.confidence,
                s.score.to_bits(),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
#[ignore = "prints the golden fingerprint; run with --ignored --nocapture"]
fn print_golden_fingerprint() {
    let (imports, existing_sections) = golden_scene();
    let all = suggest_all_placements(
        &imports,
        AzimuthReference::default(),
        &existing_sections,
        &PlacementSuggestionConfig::default(),
    );
    println!("GOLDEN-BEGIN\n{}\nGOLDEN-END", fingerprint(&all));
}

/// Wall-clock probe for the scenarios the #576 A2 analysis measured.
/// `cargo test --release -p stalagmight-models --lib placement_suggest_timing -- --ignored --nocapture`
#[test]
#[ignore = "timing probe; run in release with --ignored --nocapture"]
fn placement_suggest_timing_probe() {
    let config = PlacementSuggestionConfig::default();
    let mut rng = Rng(0xA2_0576);
    let scenario = |label: &str,
                    existing_sections: Vec<ExistingSection>,
                    imports: Vec<ImportSectionForSuggestion>| {
        let start = std::time::Instant::now();
        let all = suggest_all_placements(
            &imports,
            AzimuthReference::default(),
            &existing_sections,
            &config,
        );
        println!(
            "{label}: {:.1} ms, {} suggestions",
            start.elapsed().as_secs_f64() * 1000.0,
            all.len()
        );
    };

    let existing_random: Vec<ExistingSection> = (0..200)
        .map(|i| existing(i, &random_run(&mut rng, 50)))
        .collect();
    let imports_random: Vec<ImportSectionForSuggestion> = (0..20)
        .map(|i| import(i, random_run(&mut rng, 50)))
        .collect();
    scenario(
        "spec-scale random: existing 200x50, import 20x50",
        existing_random,
        imports_random,
    );

    let existing_straight: Vec<ExistingSection> = (0..200)
        .map(|i| existing(i, &straight_run(50, f64::from(i) * 1.7)))
        .collect();
    let imports_straight: Vec<ImportSectionForSuggestion> = (0..20)
        .map(|i| import(i, straight_run(50, f64::from(i) * 17.0)))
        .collect();
    scenario(
        "spec-scale straight: existing 200x50, import 20x50",
        existing_straight,
        imports_straight,
    );

    scenario(
        "long random: existing 1x2000, import 1x2000",
        vec![existing(0, &random_run(&mut rng, 2000))],
        vec![import(0, random_run(&mut rng, 2000))],
    );
    // The matrix implementation took 370 s and 2284 s on these two (#576 A2);
    // set A2_PROBE_SKIP_LONG_STRAIGHT to leave them out.
    if std::env::var_os("A2_PROBE_SKIP_LONG_STRAIGHT").is_some() {
        return;
    }
    scenario(
        "long straight: existing 1x2000, import 1x2000",
        vec![existing(0, &straight_run(2000, 90.0))],
        vec![import(0, straight_run(2000, 90.0))],
    );
    scenario(
        "long straight: existing 5x2000, import 2x2000",
        (0..5)
            .map(|i| existing(i, &straight_run(2000, 90.0)))
            .collect(),
        (0..2)
            .map(|i| import(i, straight_run(2000, 90.0)))
            .collect(),
    );
}

/// The base (3e450812) matrix matcher, verbatim but for the work counter:
/// the oracle the diagonal walk is compared against.
mod oracle {
    use super::super::crossing::{interpolate_depth, segment_intersection_params};
    use super::super::{
        cell_final_score, compute_exploratory_shot_score, ignored_crossing_pairs_for_suggestion,
        merge_offset_pairs, merge_station_pair_for_offsets, sequence_match_passes_final_cutoffs,
        shot_weight, station_context_multiplier, DpCell, MatchDirection, MatchShot,
        PlacementSuggestion, SequenceMatch, SuggestionConfig, TracePoint,
    };

    fn oriented_existing_shots(
        existing_shots: &[MatchShot],
        start_idx: usize,
        direction: MatchDirection,
    ) -> Vec<MatchShot> {
        match direction {
            MatchDirection::Forward => existing_shots[start_idx..].to_vec(),
            MatchDirection::Reverse => existing_shots[..=start_idx]
                .iter()
                .rev()
                .map(MatchShot::reversed)
                .collect(),
        }
    }

    pub(super) fn try_match_sequence(
        import_shots: &[MatchShot],
        existing_shots: &[MatchShot],
        start_idx: usize,
        direction: MatchDirection,
        config: &SuggestionConfig,
    ) -> SequenceMatch {
        if import_shots.is_empty() || existing_shots.is_empty() || start_idx >= existing_shots.len()
        {
            return SequenceMatch {
                score: 0.0,
                matched_count: 0,
                existing_matched_count: 0,
                is_partial: false,
            };
        }

        let existing_oriented = oriented_existing_shots(existing_shots, start_idx, direction);
        let n = import_shots.len();
        let m = existing_oriented.len();
        let mut dp: Vec<Vec<Option<DpCell>>> = vec![vec![None; m + 1]; n + 1];
        dp[0][0] = Some(DpCell {
            score_sum: 0.0,
            weight_sum: 0.0,
            import_count: 0,
            existing_count: 0,
        });

        for i in 0..=n {
            for j in 0..=m {
                let Some(cell) = dp[i][j] else { continue };
                if i + 1 > n || j + 1 > m {
                    continue;
                }
                let import_shot = &import_shots[i];
                let existing_shot = &existing_oriented[j];
                let score = compute_exploratory_shot_score(
                    import_shot,
                    existing_shot,
                    MatchDirection::Forward,
                    config,
                );
                if score < config.min_shot_score {
                    continue;
                }
                let weight = shot_weight(import_shot, config);
                let next = DpCell {
                    score_sum: cell.score_sum + score * weight,
                    weight_sum: cell.weight_sum + weight,
                    import_count: cell.import_count + 1,
                    existing_count: cell.existing_count + 1,
                };
                let target = &mut dp[i + 1][j + 1];
                let replace = target.is_none_or(|old| {
                    cell_final_score(next, config) > cell_final_score(old, config)
                });
                if replace {
                    *target = Some(next);
                }
            }
        }

        let mut candidates: Vec<DpCell> = Vec::new();
        for row in dp.iter().skip(1) {
            for cell in row.iter().flatten() {
                candidates.push(*cell);
            }
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
                && sequence_match_passes_final_cutoffs(
                    import_shots,
                    &existing_oriented,
                    *cell,
                    config,
                )
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

    /// The base merge-offset fold: crossing-check every offset, keep the
    /// highest multiplier, a near-tie going to the larger offset.
    pub(super) fn best_merge_offset(
        import_match_shots: &[MatchShot],
        existing_match_shots: &[MatchShot],
        import_trace: &[TracePoint],
        existing_trace: &[TracePoint],
        suggestion: &PlacementSuggestion,
        config: &SuggestionConfig,
    ) -> Option<(f64, usize)> {
        let mut best_context: Option<(f64, usize)> = None;
        let ignored_crossing_pairs = ignored_crossing_pairs_for_suggestion(
            suggestion,
            import_match_shots.len(),
            existing_match_shots.len(),
        );
        for (import_offset, existing_offset) in merge_offset_pairs(suggestion) {
            let Some((merge_import_station, merge_existing_station)) =
                merge_station_pair_for_offsets(
                    import_match_shots.len(),
                    existing_match_shots.len(),
                    suggestion,
                    import_offset,
                    existing_offset,
                )
            else {
                continue;
            };
            if !placement_crossing_clearance_ok(
                import_trace,
                existing_trace,
                merge_import_station,
                merge_existing_station,
                &ignored_crossing_pairs,
                config,
            ) {
                continue;
            }
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
            if best_context.is_none_or(|(best, best_offset)| {
                context_multiplier > best
                    || ((context_multiplier - best).abs() < 1e-9 && import_offset > best_offset)
            }) {
                best_context = Some((context_multiplier, import_offset));
            }
        }
        best_context
    }

    /// The base exhaustive crossing check: every import segment against every
    /// existing one.
    pub(super) fn placement_crossing_clearance_ok(
        import_trace: &[TracePoint],
        existing_trace: &[TracePoint],
        import_station_index: usize,
        existing_station_index: usize,
        ignored_pairs: &[(usize, usize)],
        config: &SuggestionConfig,
    ) -> bool {
        if import_station_index >= import_trace.len()
            || existing_station_index >= existing_trace.len()
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
}

/// Captured from the matrix implementation (base 3e450812) with
/// `print_golden_fingerprint`. Any change to the matcher that moves one bit
/// of one score, or adds, drops or reorders one suggestion, fails here.
const GOLDEN: &str = "\
7:Import7->Importing/None/Some(8) e0 i0 m5 em5 p5 Forward false High 3ff75c4fea7cca47
8:Import8->Importing/None/Some(7) e0 i0 m5 em5 p5 Forward false High 3ff75c4fea7cca47
5:Import5->Existing/Some(6)/None e25 i15 m5 em5 p4 Reverse false High 3ff758ff9e3cd4b8
4:Import4->Existing/Some(5)/None e25 i15 m5 em5 p4 Reverse false High 3ff7543303c0d583
1:Import1->Existing/Some(2)/None e7 i0 m4 em4 p3 Forward false High 3ff6bdbc00fdb116
0:Import0->Existing/Some(1)/None e3 i0 m4 em4 p3 Forward false High 3ff6b3cae50ff2dc
2:Import2->Existing/Some(3)/None e11 i0 m4 em4 p3 Forward false High 3ff6b30393332af8
6:Import6->Existing/Some(2)/None e15 i0 m5 em5 p5 Reverse false High 3ff109acdbc288e7";

#[test]
fn suggestions_match_the_golden_list_bit_for_bit() {
    let (imports, existing_sections) = golden_scene();
    let all = suggest_all_placements(
        &imports,
        AzimuthReference::default(),
        &existing_sections,
        &PlacementSuggestionConfig::default(),
    );
    assert_eq!(fingerprint(&all), GOLDEN);
}

fn match_shots(shots: &[ShotData]) -> Vec<MatchShot> {
    shots
        .iter()
        .map(|s| MatchShot::from_shot_data(s, AzimuthReference::default()))
        .collect()
}

fn same_match(a: &super::SequenceMatch, b: &super::SequenceMatch) -> bool {
    a.score.to_bits() == b.score.to_bits()
        && a.matched_count == b.matched_count
        && a.existing_matched_count == b.existing_matched_count
        && a.is_partial == b.is_partial
}

/// Every start index and both directions of 1,500 random existing/import
/// pairs, across the whole `max_reference_shots` range the config allows:
/// the diagonal walk returns exactly what the matrix returned, bit for bit.
#[test]
fn diagonal_walk_matches_the_matrix_matcher() {
    let mut rng = Rng(0xD1A6_0576_A2A2);
    let mut compared = 0usize;
    let mut matched = 0usize;
    let mut multi = 0usize;
    for case in 0..1500 {
        let max_ref = 1 + (case % 50);
        let config = SuggestionConfig::from(&PlacementSuggestionConfig {
            max_reference_shots: max_ref as u32,
            ..PlacementSuggestionConfig::default()
        });
        let existing_len = 1 + (rng.next_u64() % 60) as usize;
        let existing_shots = random_run(&mut rng, existing_len);
        let existing = match_shots(&existing_shots);
        // Half the imports overlap the existing run (so long matches occur),
        // half are unrelated (so early breaks occur).
        let import_shots = if rng.unit() < 0.5 && existing_len > 1 {
            let from = (rng.next_u64() % (existing_len as u64 - 1)) as usize;
            let len = 1 + (rng.next_u64() % (existing_len - from) as u64) as usize;
            noisy_copy(&mut rng, &existing_shots, from, len)
        } else {
            let len = 1 + (rng.next_u64() % 12) as usize;
            random_run(&mut rng, len)
        };
        let import = match_shots(&import_shots[..import_shots.len().min(max_ref)]);
        for start_idx in 0..=existing.len() {
            for direction in [MatchDirection::Forward, MatchDirection::Reverse] {
                let new = try_match_sequence(&import, &existing, start_idx, direction, &config);
                let old =
                    oracle::try_match_sequence(&import, &existing, start_idx, direction, &config);
                assert!(
                    same_match(&new, &old),
                    "case {case} start {start_idx} {direction:?}: walk ({}, {}, {}) vs matrix \
                     ({}, {}, {})",
                    new.score,
                    new.matched_count,
                    new.existing_matched_count,
                    old.score,
                    old.matched_count,
                    old.existing_matched_count
                );
                compared += 1;
                matched += usize::from(old.matched_count > 0);
                multi += usize::from(old.matched_count > 2);
            }
        }
    }
    // The comparison must exercise real (and multi-shot) matches, not just
    // agree on zeros.
    assert!(
        matched > 500 && multi > 100,
        "{matched} of {compared} calls matched, {multi} over two shots"
    );
}

/// A trace that runs straight at `azimuth`, starting from `(x0, y0)`.
fn straight_trace(len: usize, azimuth: f64, step: f64, depth: f64) -> Vec<TracePoint> {
    let shots: Vec<ShotData> = (0..len)
        .map(|_| straight_shot(azimuth, step).with_depths(depth, depth))
        .collect();
    section_trace_points(&match_shots(&shots))
}

fn random_ignored(rng: &mut Rng, import_len: usize, existing_len: usize) -> Vec<(usize, usize)> {
    (0..(rng.next_u64() % 7))
        .map(|_| {
            (
                (rng.next_u64() % import_len.max(1) as u64) as usize,
                (rng.next_u64() % existing_len.max(1) as u64) as usize,
            )
        })
        .collect()
}

/// The grid-prefiltered crossing check against the exhaustive one, on random
/// passages, straight and nearly parallel passages at one depth (the shape
/// where rounding could make the exact test report a crossing between boxes
/// that do not touch), very long legs (past the bound, so the exhaustive
/// fallback runs) and out-of-range stations.
#[test]
fn prefiltered_crossing_check_matches_the_exhaustive_one() {
    let mut rng = Rng(0xC055_0576_A2A2);
    let config = SuggestionConfig::default();
    let mut verdicts = [0usize; 2];
    let mut prefiltered = 0usize;
    for case in 0..3000 {
        let (import_trace, existing_trace) = match case % 4 {
            0 => {
                let (import_len, existing_len) = (
                    1 + (rng.next_u64() % 30) as usize,
                    1 + (rng.next_u64() % 40) as usize,
                );
                (
                    section_trace_points(&match_shots(&random_run(&mut rng, import_len))),
                    section_trace_points(&match_shots(&random_run(&mut rng, existing_len))),
                )
            }
            1 => {
                let azimuth = rng.range(0.0, 360.0);
                // Nearly parallel: a nano- to milli-degree apart.
                let skew = rng.range(-1.0, 1.0) * 10f64.powf(rng.range(-9.0, -3.0));
                (
                    straight_trace(
                        1 + (rng.next_u64() % 20) as usize,
                        azimuth + skew,
                        5.0,
                        10.0,
                    ),
                    straight_trace(1 + (rng.next_u64() % 40) as usize, azimuth, 5.0, 10.0),
                )
            }
            2 => {
                // Crossing passages at a random depth difference.
                let azimuth = rng.range(0.0, 360.0);
                (
                    straight_trace(
                        1 + (rng.next_u64() % 20) as usize,
                        azimuth + rng.range(20.0, 160.0),
                        rng.range(0.5, 20.0),
                        rng.range(0.0, 12.0),
                    ),
                    straight_trace(1 + (rng.next_u64() % 20) as usize, azimuth, 5.0, 6.0),
                )
            }
            _ => {
                // Legs longer than the prefilter's bound: exhaustive fallback.
                let azimuth = rng.range(0.0, 360.0);
                (
                    straight_trace(3, azimuth + rng.range(10.0, 170.0), 300.0, 10.0),
                    straight_trace(3, azimuth, 300.0, 10.0),
                )
            }
        };
        let index = CrossingIndex::build(
            &import_trace,
            &existing_trace,
            &straight_runs(&import_trace),
        );
        prefiltered += usize::from(matches!(index.lanes[0], CrossingLane::Grid(_)));
        let import_station = (rng.next_u64() % (import_trace.len() as u64 + 1)) as usize;
        let existing_station = (rng.next_u64() % (existing_trace.len() as u64 + 1)) as usize;
        let ignored = random_ignored(&mut rng, import_trace.len(), existing_trace.len());
        let new = placement_crossing_clearance_ok(
            &import_trace,
            &existing_trace,
            &index,
            import_station,
            existing_station,
            &ignored,
            &config,
        );
        let old = oracle::placement_crossing_clearance_ok(
            &import_trace,
            &existing_trace,
            import_station,
            existing_station,
            &ignored,
            &config,
        );
        assert_eq!(new, old, "case {case}: prefiltered {new}, exhaustive {old}");
        verdicts[usize::from(old)] += 1;
    }
    // Both verdicts occur, and most cases ran through the grid.
    assert!(
        verdicts[0] > 100 && verdicts[1] > 100,
        "verdicts {verdicts:?}"
    );
    assert!(
        prefiltered > 2000,
        "only {prefiltered} cases were prefiltered"
    );
}

/// `best_merge_offset` ranks the offsets and crossing-checks them best first;
/// the base folded over all of them. Random suggestions on overlapping
/// passages (so the crossing check both passes and fails) agree bit for bit.
#[test]
fn ranked_merge_offset_matches_the_fold() {
    let mut rng = Rng(0x0FF5_0576_A2A2);
    let config = SuggestionConfig::default();
    let mut placed = 0usize;
    for case in 0..2000 {
        let existing_len = 2 + (rng.next_u64() % 30) as usize;
        let existing_shots = random_run(&mut rng, existing_len);
        let from = (rng.next_u64() % (existing_len as u64 - 1)) as usize;
        let len = 1 + (rng.next_u64() % (existing_len - from).min(6) as u64) as usize;
        let mut import_shots = noisy_copy(&mut rng, &existing_shots, from, len);
        let tail = (rng.next_u64() % 10) as usize;
        import_shots.extend(random_run(&mut rng, tail));
        let (import_match, existing_match) =
            (match_shots(&import_shots), match_shots(&existing_shots));
        let (import_trace, existing_trace) = (
            section_trace_points(&import_match),
            section_trace_points(&existing_match),
        );
        let matched = 1 + (rng.next_u64() % 5) as usize;
        let suggestion = PlacementSuggestion {
            existing_section_id: 1,
            existing_station_index: (rng.next_u64() % (existing_len as u64 + 1)) as usize,
            import_station_index: if rng.unit() < 0.5 {
                0
            } else {
                import_shots.len()
            },
            score: 0.9,
            matched_shots: matched,
            existing_matched_shots: matched,
            preferred_merge_offset: matched,
            direction: if rng.unit() < 0.5 {
                MatchDirection::Forward
            } else {
                MatchDirection::Reverse
            },
            is_partial: rng.unit() < 0.2,
            confidence: SuggestionConfidence::High,
            existing_section_name: String::new(),
        };
        let new = best_merge_offset(
            &import_match,
            &existing_match,
            &import_trace,
            &existing_trace,
            &CrossingIndex::build(
                &import_trace,
                &existing_trace,
                &straight_runs(&import_trace),
            ),
            &suggestion,
            &config,
        );
        let old = oracle::best_merge_offset(
            &import_match,
            &existing_match,
            &import_trace,
            &existing_trace,
            &suggestion,
            &config,
        );
        assert_eq!(
            new.map(|(c, o)| (c.to_bits(), o)),
            old.map(|(c, o)| (c.to_bits(), o)),
            "case {case}: ranked {new:?}, fold {old:?}"
        );
        placed += usize::from(old.is_some());
    }
    assert!(placed > 200, "only {placed} suggestions found a placement");
}

/// What `best_merge_offset`'s equivalence rests on: the station-context
/// multiplier only takes a fixed set of constants (so the fold's 1e-9 tie is
/// equality). A continuous multiplier would need the fold back.
#[test]
fn station_context_multiplier_takes_only_its_constants() {
    const CONSTANTS: [f64; 6] = [0.65, 0.72, 0.75, 0.86, 1.0, 1.18];
    let mut rng = Rng(0xC0_0576_A2A2);
    let config = SuggestionConfig::default();
    for _ in 0..3000 {
        let import_len = (rng.next_u64() % 8) as usize;
        let existing_len = (rng.next_u64() % 8) as usize;
        let import = match_shots(&random_run(&mut rng, import_len));
        let existing = match_shots(&random_run(&mut rng, existing_len));
        let value = station_context_multiplier(
            &import,
            &existing,
            (rng.next_u64() % (import_len as u64 + 2)) as usize,
            (rng.next_u64() % (existing_len as u64 + 2)) as usize,
            &config,
        );
        assert!(
            CONSTANTS.iter().any(|c| c.to_bits() == value.to_bits()),
            "multiplier {value} is not one of {CONSTANTS:?}"
        );
    }
}

/// The margin grows with the legs and stops where the bound does.
#[test]
fn prefilter_margin_is_bounded_and_refuses_long_legs() {
    let short = crossing_prefilter_margin(5.0).unwrap_or(f64::NAN);
    let long = crossing_prefilter_margin(100.0).unwrap_or(f64::NAN);
    assert!(short > 0.0 && short < 0.01, "5 m legs: {short}");
    assert!(long > short && long < 100.0, "100 m legs: {long}");
    assert!(crossing_prefilter_margin(300.0).is_none());
    assert!(crossing_prefilter_margin(f64::INFINITY).is_none());
}

/// The total matcher work of one `suggest_all_placements` pass of a 10-shot
/// straight import against one straight existing section of `len` shots.
fn straight_work(len: usize) -> u64 {
    let existing_sections = vec![existing(1, &straight_run(len, 90.0))];
    let imports = vec![import(0, straight_run(10, 90.0))];
    work::take();
    let all = suggest_all_placements(
        &imports,
        AzimuthReference::default(),
        &existing_sections,
        &PlacementSuggestionConfig::default(),
    );
    assert!(
        !all.is_empty(),
        "the straight scene must produce suggestions"
    );
    work::take()
}

/// [#576 NF-W4U-3] Collinear sections are the crossing check's worst case:
/// every start index survives the matcher, and each surviving candidate used
/// to walk the whole placed import through the grid, so the work grew with
/// candidates x import length (10x the import, ~9x the work). Import segments
/// provably parallel to every existing segment are now skipped, so a longer
/// straight import costs no more crossing work than a short one.
#[test]
fn collinear_crossing_work_does_not_grow_with_the_import() {
    let work_for = |import_len: usize| {
        let existing_sections = vec![existing(1, &straight_run(200, 90.0))];
        let imports = vec![import(0, straight_run(import_len, 90.0))];
        work::take();
        let all = suggest_all_placements(
            &imports,
            AzimuthReference::default(),
            &existing_sections,
            &PlacementSuggestionConfig::default(),
        );
        assert!(
            !all.is_empty(),
            "the collinear scene must produce suggestions"
        );
        work::take()
    };
    let short = work_for(20);
    let long = work_for(200);
    let ratio = long as f64 / short as f64;
    println!("collinear work: 20-shot import {short}, 200-shot import {long}, ratio {ratio:.2}");
    assert!(
        ratio < 2.0,
        "10x the straight import cost {ratio:.2}x the work ({short} -> {long} units); the \
         crossing check is walking provably parallel segments again"
    );
}

/// The pass makes a constant number of calls per start index, so total work
/// linear in the section is work per call that does NOT grow with it. The
/// matrix matcher allocated and walked `(n + 1) x (m + 1)` cells and copied
/// the section tail per call, and the crossing check tested every import
/// segment against every existing one: 10x the section was ~100x the work.
#[test]
fn matcher_work_grows_linearly_with_the_existing_section() {
    let short = straight_work(40);
    let long = straight_work(400);
    let ratio = long as f64 / short as f64;
    println!("matcher work: 40 shots {short}, 400 shots {long}, ratio {ratio:.1}");
    assert!(
        ratio < 20.0,
        "10x the existing section cost {ratio:.1}x the work ({short} -> {long} units); \
         per-call work is scaling with the section length"
    );
}
