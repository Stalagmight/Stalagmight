//! [#576 NF-W4U-3] Differential gate for the placement crossing check.
//!
//! [`reference`] keeps the crossing half of the suggestion pipeline exactly as
//! it stood at b83b12de5, before the collinear speedup: `suggest_all_placements`,
//! `suggest_placements_among`, `best_merge_offset`,
//! `placement_crossing_clearance_ok` and the `CrossingPrefilter` grid, copied
//! verbatim but for the work counter and the crossing log. What those call -
//! the matcher, the scoring, the segment test itself - is shared with
//! production and unchanged.
//!
//! Every scene is built from one seed. For each, the production pipeline must
//! return exactly what the reference returns: the same suggestions in the same
//! order with bit-identical scores, after the same crossing checks in the same
//! order with the same verdicts (so the same candidates are rejected); and the
//! crossing check alone must agree with the reference's on extra placements of
//! every section pair, in and out of range, with and without ignored pairs.
//!
//! Neither pipeline may panic. A NaN reading makes a score NaN, and the score
//! sorts used to treat NaN as equal to everything, which the standard sort
//! aborts on; they order by `score_cmp` now (#576 NF-W9PERF-1), so a scene
//! with non-finite readings is compared like any other.
//!
//! A failure names its seed. Replay it, or run more seeds, with
//! `PLACEMENT_DIFF_SEED=<seed>` or `PLACEMENT_DIFF_SEEDS=<count>` (plus
//! `PLACEMENT_DIFF_FIRST=<seed>`, `PLACEMENT_DIFF_MAX_LEN=<shots>`):
//! `PLACEMENT_DIFF_SEEDS=20000 cargo test --release -p stalagmight-models --lib differential -- --nocapture`.

use super::crossing::{
    placement_crossing_clearance_ok, straight_runs, CrossingIndex, CrossingLane,
};
use super::{
    crossing_log, ignored_crossing_pairs_for_suggestion, section_trace_points,
    suggest_all_placements, CrossSectionSuggestion, ExistingSection, ImportSectionForSuggestion,
    MatchDirection, MatchShot, PlacementSuggestion, SuggestionConfidence, SuggestionConfig,
    TracePoint,
};
use crate::config::PlacementSuggestionConfig;
use crate::test_fixtures::straight_shot;
use crate::{AzimuthReference, SectionId, Shot, ShotData};

/// SplitMix64: every seed, zero included, gives a full-period stream.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    /// Uniform in `0..n`, `n > 0`.
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }

    /// Uniform in `lo..=hi`.
    fn between(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi - lo + 1)
    }

    fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())]
    }

    /// `10^U(lo, hi)`, with a random sign.
    fn signed_log(&mut self, lo: f64, hi: f64) -> f64 {
        let magnitude = 10f64.powf(self.range(lo, hi));
        if self.chance(0.5) {
            magnitude
        } else {
            -magnitude
        }
    }
}

/// A passage-like random walk: the heading wanders up to `turn` degrees a
/// leg, 1-10 m legs, the depth drifting by up to `drift` a leg.
fn random_run(rng: &mut Rng, len: usize, turn: f64, drift: f64) -> Vec<ShotData> {
    let mut depth = rng.range(5.0, 40.0);
    let mut heading = rng.range(0.0, 360.0);
    (0..len)
        .map(|_| {
            heading = (heading + rng.range(-turn, turn)).rem_euclid(360.0);
            let next = (depth + rng.range(-drift, drift)).max(0.0);
            let s = straight_shot(heading, rng.range(1.0, 10.0)).with_depths(depth, next);
            depth = next;
            s
        })
        .collect()
}

/// A dead-straight passage at one depth.
fn straight(len: usize, azimuth: f64, step: f64, depth: f64) -> Vec<ShotData> {
    (0..len)
        .map(|_| straight_shot(azimuth, step).with_depths(depth, depth))
        .collect()
}

/// `source[from..from + len]` with survey noise of `scale` (0 = exact copy).
fn noisy_copy(
    rng: &mut Rng,
    source: &[ShotData],
    from: usize,
    len: usize,
    scale: f64,
) -> Vec<ShotData> {
    source[from..from + len]
        .iter()
        .map(|s| {
            let az = (s.azimuth_in + scale * rng.range(-1.5, 1.5)).rem_euclid(360.0);
            straight_shot(az, s.length + scale * rng.range(-0.05, 0.05)).with_depths(
                s.depth_in + scale * rng.range(-0.05, 0.05),
                s.depth_out + scale * rng.range(-0.05, 0.05),
            )
        })
        .collect()
}

/// The same passage surveyed the other way.
fn walked_backwards(shots: &[ShotData]) -> Vec<ShotData> {
    shots
        .iter()
        .rev()
        .map(|s| {
            straight_shot((s.azimuth_in + 180.0).rem_euclid(360.0), s.length)
                .with_depths(s.depth_out, s.depth_in)
        })
        .collect()
}

fn reference_with(correction: f64) -> AzimuthReference {
    AzimuthReference {
        correction,
        ..AzimuthReference::default()
    }
}

/// A north reference that is usually the default and sometimes rotated.
fn some_reference(rng: &mut Rng) -> AzimuthReference {
    if rng.chance(0.8) {
        AzimuthReference::default()
    } else {
        reference_with(rng.range(-20.0, 20.0))
    }
}

fn existing_section(
    id: SectionId,
    shots: &[ShotData],
    reference: AzimuthReference,
) -> ExistingSection {
    ExistingSection {
        id,
        name: format!("CaveA{id}"),
        shots: shots
            .iter()
            .enumerate()
            .map(|(i, sd)| Shot::new(i as u32, sd.clone()))
            .collect(),
        azimuth_reference: reference,
    }
}

struct Scene {
    imports: Vec<ImportSectionForSuggestion>,
    existing: Vec<ExistingSection>,
    fallback: AzimuthReference,
    config: PlacementSuggestionConfig,
}

/// Assembles a scene from bare shot lists, rotating north references now and
/// then and varying the config knobs that change which candidates exist.
struct SceneBuilder<'r> {
    rng: &'r mut Rng,
    imports: Vec<ImportSectionForSuggestion>,
    existing: Vec<ExistingSection>,
}

impl<'r> SceneBuilder<'r> {
    fn new(rng: &'r mut Rng) -> Self {
        Self {
            rng,
            imports: Vec::new(),
            existing: Vec::new(),
        }
    }

    fn existing(&mut self, shots: &[ShotData]) {
        let reference = some_reference(self.rng);
        let id = self.existing.len() as SectionId + 1;
        self.existing.push(existing_section(id, shots, reference));
    }

    fn import(&mut self, shots: Vec<ShotData>) {
        let reference = if self.rng.chance(0.7) {
            None
        } else {
            Some(some_reference(self.rng))
        };
        let index = self.imports.len() as u32;
        self.imports.push(ImportSectionForSuggestion {
            index,
            name: format!("Import{index}"),
            shots,
            azimuth_reference: reference,
        });
    }

    fn finish(self) -> Scene {
        let rng = self.rng;
        let mut config = PlacementSuggestionConfig::default();
        if rng.chance(0.35) {
            config.max_reference_shots = rng.between(1, 10) as u32;
            config.max_suggestions = rng.pick(&[1, 2, 5, 10, 50, 1000]);
            config.min_aggregate_score = rng.range(0.3, 0.6);
        }
        Scene {
            imports: self.imports,
            existing: self.existing,
            fallback: some_reference(rng),
            config,
        }
    }
}

/// An import that genuinely overlaps `source` somewhere (forward or walked
/// backwards), with random passage before and/or after the overlap.
fn tie_in(rng: &mut Rng, source: &[ShotData], noise: f64) -> Vec<ShotData> {
    if source.is_empty() {
        let len = rng.between(1, 12);
        return random_run(rng, len, 25.0, 2.0);
    }
    let len = rng.between(1, source.len().min(8));
    let from = rng.below(source.len() - len + 1);
    let mut overlap = noisy_copy(rng, source, from, len, noise);
    if rng.chance(0.4) {
        overlap = walked_backwards(&overlap);
    }
    let mut shots = Vec::new();
    if rng.chance(0.5) {
        let len = rng.between(0, 12);
        shots.extend(random_run(rng, len, 25.0, 2.0));
    }
    shots.extend(overlap);
    if rng.chance(0.7) {
        let len = rng.between(0, 12);
        shots.extend(random_run(rng, len, 25.0, 2.0));
    }
    shots
}

const HEADINGS: [f64; 9] = [0.0, 90.0, 180.0, 270.0, 45.0, 135.0, 225.0, 315.0, -1.0];

fn heading(rng: &mut Rng) -> f64 {
    match rng.pick(&HEADINGS) {
        h if h < 0.0 => rng.range(0.0, 360.0),
        h => h,
    }
}

fn step(rng: &mut Rng) -> f64 {
    if rng.chance(0.5) {
        5.0
    } else {
        rng.range(0.3, 25.0)
    }
}

const KINDS: [&str; 11] = [
    "random",
    "collinear",
    "skewed",
    "micro-noise",
    "bend",
    "crossing",
    "constant-depth",
    "zigzag",
    "long-legs",
    "degenerate",
    "import-to-import",
];

/// The scene for one seed: its kind is `seed % KINDS.len()`, everything else
/// comes from the seed's stream. `max_len` caps the long sections.
fn scene(seed: u64, max_len: usize) -> (&'static str, Scene) {
    let kind = KINDS[(seed % KINDS.len() as u64) as usize];
    let mut rng = Rng(seed ^ 0x0576_F4D1_FF00_0003);
    let rng = &mut rng;
    let mut b = SceneBuilder::new(rng);
    match kind {
        "random" => {
            let passages: Vec<Vec<ShotData>> = (0..b.rng.between(1, 5))
                .map(|_| {
                    let len = b.rng.between(1, max_len / 2);
                    random_run(b.rng, len, 25.0, 2.0)
                })
                .collect();
            for p in &passages {
                b.existing(p);
            }
            for _ in 0..b.rng.between(1, 3) {
                let source = &passages[b.rng.below(passages.len())];
                let shots = if b.rng.chance(0.6) {
                    let noise = b.rng.pick(&[0.0, 1.0]);
                    tie_in(b.rng, source, noise)
                } else {
                    let len = b.rng.between(1, max_len / 2);
                    random_run(b.rng, len, 25.0, 2.0)
                };
                b.import(shots);
            }
        }
        "collinear" => {
            let az = heading(b.rng);
            let st = step(b.rng);
            let shared_depth = b.rng.range(5.0, 40.0);
            for _ in 0..b.rng.between(1, 3) {
                let depth = if b.rng.chance(0.8) {
                    shared_depth
                } else {
                    b.rng.range(5.0, 40.0)
                };
                let len = b.rng.between(1, max_len);
                b.existing(&straight(len, az, st, depth));
            }
            for _ in 0..b.rng.between(1, 2) {
                let len = b.rng.between(1, max_len);
                let import_az = if b.rng.chance(0.3) { az + 180.0 } else { az };
                let import_step = if b.rng.chance(0.8) {
                    st
                } else {
                    st * b.rng.range(0.98, 1.02)
                };
                let mut shots =
                    straight(len, import_az.rem_euclid(360.0), import_step, shared_depth);
                if b.rng.chance(0.3) {
                    // Turn off the line part-way.
                    let turn = b.rng.range(-170.0, 170.0);
                    let tail = b.rng.between(1, 20);
                    shots.extend(straight(
                        tail,
                        (import_az + turn).rem_euclid(360.0),
                        import_step,
                        shared_depth,
                    ));
                }
                b.import(shots);
            }
        }
        "skewed" => {
            let az = heading(b.rng);
            let st = step(b.rng);
            let depth = b.rng.range(5.0, 40.0);
            for _ in 0..b.rng.between(1, 3) {
                let skew = if b.rng.chance(0.5) {
                    0.0
                } else {
                    b.rng.signed_log(-12.0, 0.0)
                };
                let len = b.rng.between(1, max_len);
                b.existing(&straight(len, (az + skew).rem_euclid(360.0), st, depth));
            }
            for _ in 0..b.rng.between(1, 2) {
                let skew = b.rng.signed_log(-12.0, 0.0);
                let len = b.rng.between(1, max_len);
                b.import(straight(len, (az + skew).rem_euclid(360.0), st, depth));
            }
        }
        "micro-noise" => {
            // Nearly straight, nearly parallel, at one depth: overlaid, the
            // traces cross each other again and again at near-zero angles,
            // around the parallel cutoff of the segment test. The scales
            // straddle what a straight run tolerates (~2e-10 degrees on 5 m
            // legs), what the parallel proof can certify (~1e-9) and where
            // the segment test stops calling a pair parallel (~2e-9).
            let az = heading(b.rng);
            let st = step(b.rng);
            let depth = b.rng.range(5.0, 40.0);
            let wobble = |rng: &mut Rng, len: usize, lo: f64, hi: f64| -> Vec<ShotData> {
                let scale = if rng.chance(0.1) {
                    0.0
                } else {
                    10f64.powf(rng.range(lo, hi))
                };
                (0..len)
                    .map(|_| {
                        straight_shot((az + scale * rng.range(-1.0, 1.0)).rem_euclid(360.0), st)
                            .with_depths(depth, depth)
                    })
                    .collect()
            };
            for _ in 0..b.rng.between(1, 3) {
                let len = b.rng.between(1, max_len);
                let (lo, hi) = if b.rng.chance(0.8) {
                    (-12.0, -6.0)
                } else {
                    (-6.0, -3.0)
                };
                let shots = wobble(b.rng, len, lo, hi);
                b.existing(&shots);
            }
            for _ in 0..b.rng.between(1, 2) {
                let len = b.rng.between(1, max_len);
                let (lo, hi) = if b.rng.chance(0.7) {
                    (-14.0, -9.0)
                } else {
                    (-9.0, -3.0)
                };
                let shots = wobble(b.rng, len, lo, hi);
                b.import(shots);
            }
        }
        "bend" => {
            let az = heading(b.rng);
            let st = step(b.rng);
            let depth = b.rng.range(5.0, 40.0);
            for _ in 0..b.rng.between(1, 2) {
                let first = b.rng.between(1, max_len / 2);
                let mut shots = straight(first, az, st, depth);
                let bend = b.rng.between(1, 6);
                let mut h = az;
                for _ in 0..bend {
                    h += b.rng.range(-60.0, 60.0);
                    let d = depth + b.rng.pick(&[0.0, 0.0, 2.0, 5.0]);
                    shots.push(straight_shot(h.rem_euclid(360.0), st).with_depths(d, d));
                }
                let second = b.rng.between(0, max_len / 2);
                let second_az = if b.rng.chance(0.5) { az } else { h };
                shots.extend(straight(second, second_az.rem_euclid(360.0), st, depth));
                b.existing(&shots);
            }
            for _ in 0..b.rng.between(1, 2) {
                let len = b.rng.between(1, max_len);
                let mut shots = straight(len, az, st, depth);
                if b.rng.chance(0.4) {
                    let tail = b.rng.between(1, 10);
                    let turn = b.rng.range(-120.0, 120.0);
                    shots.extend(straight(tail, (az + turn).rem_euclid(360.0), st, depth));
                }
                b.import(shots);
            }
        }
        "crossing" => {
            // An import that ties into an existing passage, then loops back
            // across it at a depth offset near the 4 m clearance.
            let depth = b.rng.range(10.0, 30.0);
            let passage = if b.rng.chance(0.5) {
                let len = b.rng.between(4, max_len.max(8) / 2);
                let mut p = random_run(b.rng, len, 25.0, 0.0);
                for s in &mut p {
                    s.depth_in = depth;
                    s.depth_out = depth;
                }
                p
            } else {
                let len = b.rng.between(4, max_len.max(8) / 2);
                let az = heading(b.rng);
                straight(len, az, step(b.rng), depth)
            };
            b.existing(&passage);
            for _ in 0..b.rng.between(1, 3) {
                let len = b.rng.between(1, passage.len().min(6));
                let from = b.rng.below(passage.len() - len + 1);
                let noise = b.rng.pick(&[0.0, 0.0, 1.0]);
                let mut shots = noisy_copy(b.rng, &passage, from, len, noise);
                let offset = b
                    .rng
                    .pick(&[4.0, 4.0 - 1e-9, 4.0 + 1e-9, 3.999, 4.001, 0.0, -4.0])
                    * if b.rng.chance(0.2) {
                        b.rng.range(0.0, 2.0)
                    } else {
                        1.0
                    };
                let last_out = shots.last().map_or(depth, |s| s.depth_out);
                let mut h = shots.last().map_or(0.0, |s| s.azimuth_in);
                // Step down to the offset, then wander back across.
                shots.push(
                    straight_shot(h, b.rng.range(1.0, 6.0)).with_depths(last_out, depth + offset),
                );
                for _ in 0..b.rng.between(2, 12) {
                    h += b.rng.range(90.0, 200.0) / 3.0;
                    shots.push(
                        straight_shot(h.rem_euclid(360.0), b.rng.range(1.0, 8.0))
                            .with_depths(depth + offset, depth + offset),
                    );
                }
                b.import(shots);
            }
        }
        "constant-depth" => {
            let depth = b.rng.range(5.0, 40.0);
            let flat = |rng: &mut Rng, len: usize| -> Vec<ShotData> {
                let mut p = random_run(rng, len, 40.0, 0.0);
                for s in &mut p {
                    s.depth_in = depth;
                    s.depth_out = depth;
                }
                p
            };
            let passages: Vec<Vec<ShotData>> = (0..b.rng.between(1, 4))
                .map(|_| {
                    let len = b.rng.between(1, max_len / 2);
                    flat(b.rng, len)
                })
                .collect();
            for p in &passages {
                b.existing(p);
            }
            for _ in 0..b.rng.between(1, 3) {
                let source = &passages[b.rng.below(passages.len())];
                let noise = b.rng.pick(&[0.0, 0.3]);
                let mut shots = tie_in(b.rng, source, noise);
                for s in &mut shots {
                    s.depth_in = depth;
                    s.depth_out = depth;
                }
                b.import(shots);
            }
        }
        "zigzag" => {
            // A periodic passage: every period matches every other, and an
            // exact copy overlays it segment on segment.
            let az = heading(b.rng);
            let swing = b.rng.range(5.0, 90.0);
            let st = step(b.rng);
            let depth = b.rng.range(5.0, 40.0);
            let period = b.rng.between(2, 4);
            let zig = |len: usize, phase: usize| -> Vec<ShotData> {
                (0..len)
                    .map(|i| {
                        let h = if (i + phase).is_multiple_of(period) {
                            az + swing
                        } else {
                            az
                        };
                        straight_shot(h.rem_euclid(360.0), st).with_depths(depth, depth)
                    })
                    .collect()
            };
            for _ in 0..b.rng.between(1, 2) {
                let len = b.rng.between(1, max_len);
                b.existing(&zig(len, 0));
            }
            for _ in 0..b.rng.between(1, 2) {
                let len = b.rng.between(1, max_len);
                let phase = b.rng.below(period);
                b.import(zig(len, phase));
            }
        }
        "long-legs" => {
            // Legs past the grid's bound (exhaustive fallback), and long
            // straight sections with coordinates large enough that rounding
            // could defeat any proof built on them.
            let az = heading(b.rng);
            let depth = b.rng.range(5.0, 40.0);
            let leg = b.rng.pick(&[60.0, 150.0, 230.0, 400.0, 1500.0]);
            for _ in 0..b.rng.between(1, 2) {
                let len = b.rng.between(1, max_len.min(60));
                if b.rng.chance(0.5) {
                    b.existing(&straight(len, az, leg, depth));
                } else {
                    let mut p = random_run(b.rng, len, 25.0, 0.0);
                    for s in &mut p {
                        s.length *= leg / 5.0;
                        s.depth_in = depth;
                        s.depth_out = depth;
                    }
                    b.existing(&p);
                }
            }
            for _ in 0..b.rng.between(1, 2) {
                let len = b.rng.between(1, max_len.min(60));
                let skew = if b.rng.chance(0.5) {
                    0.0
                } else {
                    b.rng.signed_log(-12.0, -2.0)
                };
                b.import(straight(len, (az + skew).rem_euclid(360.0), leg, depth));
            }
        }
        "degenerate" => {
            // Zero-length and vertical shots, one-shot and empty sections,
            // repeated stations, and now and then a non-finite reading.
            let depth = b.rng.range(5.0, 40.0);
            let odd = |rng: &mut Rng, len: usize| -> Vec<ShotData> {
                (0..len)
                    .map(|_| {
                        let az = rng.pick(&[0.0, 90.0, 45.0, 180.0]);
                        match rng.below(10) {
                            0 => straight_shot(az, 0.0).with_depths(depth, depth),
                            1 => straight_shot(az, 3.0).with_depths(depth, depth + 3.0),
                            2 => straight_shot(az, 3.0).with_depths(depth + 3.0, depth),
                            3 if rng.chance(0.1) => {
                                straight_shot(f64::NAN, 5.0).with_depths(depth, depth)
                            }
                            4 if rng.chance(0.1) => {
                                straight_shot(az, f64::INFINITY).with_depths(depth, depth)
                            }
                            _ => straight_shot(az, 5.0).with_depths(depth, depth),
                        }
                    })
                    .collect()
            };
            for _ in 0..b.rng.between(1, 3) {
                let len = b.rng.between(0, 12);
                let shots = odd(b.rng, len);
                b.existing(&shots);
            }
            for _ in 0..b.rng.between(1, 3) {
                let len = b.rng.between(0, 12);
                let shots = odd(b.rng, len);
                b.import(shots);
            }
        }
        _ => {
            // import-to-import: several imports overlapping each other, with
            // zero or one existing section.
            let shared = if b.rng.chance(0.5) {
                let len = b.rng.between(2, max_len / 2);
                random_run(b.rng, len, 25.0, 2.0)
            } else {
                let len = b.rng.between(2, max_len / 2);
                let az = heading(b.rng);
                straight(len, az, step(b.rng), 12.0)
            };
            if b.rng.chance(0.5) {
                b.existing(&shared);
            }
            for _ in 0..b.rng.between(2, 5) {
                let shots = if b.rng.chance(0.3) {
                    shared.clone()
                } else {
                    let noise = b.rng.pick(&[0.0, 1.0]);
                    tie_in(b.rng, &shared, noise)
                };
                b.import(shots);
            }
        }
    }
    (kind, b.finish())
}

/// One suggestion per line, every field, every float by its bits.
fn fingerprint(suggestions: &[CrossSectionSuggestion]) -> Vec<String> {
    suggestions
        .iter()
        .map(|s| {
            format!(
                "{}:{} -> {:?}/{:?}/{:?} '{}' e{} i{} m{} em{} p{} {:?} {} {:?} {:016x}",
                s.import_section_index,
                s.import_section_name,
                s.target_type,
                s.existing_section_id,
                s.target_import_index,
                s.existing_section_name,
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
        .collect()
}

/// The first index two lists differ at, for a readable failure.
fn first_difference<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    a.iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()))
}

#[derive(Default)]
struct Tally {
    scenes: usize,
    suggestions: usize,
    checks: usize,
    rejected: usize,
    replayed: usize,
    replay_rejected: usize,
    /// Section pairs indexed, and how much of them the speedup touched:
    /// pairs with import segments proven parallel to every existing segment
    /// (and how many such segments), and lanes over a subset of the grid.
    pairs: usize,
    pairs_cleared: usize,
    segments_cleared: usize,
    sparse_lanes: usize,
    /// Replayed checks in which at least one chunk missed its lane's grid.
    chunk_skips: usize,
}

/// The production crossing check for one section pair, as one closure over
/// (import station, existing station, ignored pairs) returning the verdict and
/// whether a chunk was skipped whole (coverage).
fn production_check<'a>(
    import_trace: &'a [TracePoint],
    existing_trace: &'a [TracePoint],
    config: &'a SuggestionConfig,
    tally: &mut Tally,
) -> impl Fn(usize, usize, &[(usize, usize)]) -> (bool, bool) + 'a {
    let index = CrossingIndex::build(import_trace, existing_trace, &straight_runs(import_trace));
    let walked: usize = index.chunks.iter().map(|c| c.end - c.start).sum();
    let cleared = import_trace.len().saturating_sub(1) - walked;
    tally.pairs += 1;
    tally.pairs_cleared += usize::from(cleared > 0);
    tally.segments_cleared += cleared;
    tally.sparse_lanes += index.lanes.len() - 1;
    move |import_station, existing_station, ignored| {
        // Coverage only: would any chunk be skipped whole at this placement?
        let skips = import_station < import_trace.len()
            && existing_station < existing_trace.len()
            && index.chunks.iter().any(|chunk| {
                let dx = existing_trace[existing_station].x - import_trace[import_station].x;
                let dy = existing_trace[existing_station].y - import_trace[import_station].y;
                let b = chunk.points_box;
                matches!(&index.lanes[chunk.lane], CrossingLane::Grid(grid)
                    if grid.misses(&grid.query([b[0] + dx, b[1] + dx, b[2] + dy, b[3] + dy])))
            });
        let clear = placement_crossing_clearance_ok(
            import_trace,
            existing_trace,
            &index,
            import_station,
            existing_station,
            ignored,
            config,
        );
        (clear, skips)
    }
}

fn match_shots(shots: &[ShotData], reference: AzimuthReference) -> Vec<MatchShot> {
    shots
        .iter()
        .map(|s| MatchShot::from_shot_data(s, reference))
        .collect()
}

/// Extra placements of every (import, target) pair in the scene: stations the
/// pipeline would merge at (near either import leaf) and anywhere else, in and
/// out of range, with the ignored pairs a suggestion there would carry, random
/// ones, or none.
fn replay_crossing_checks(seed: u64, kind: &str, scene: &Scene, rng: &mut Rng, tally: &mut Tally) {
    let config = SuggestionConfig::from(&scene.config);
    let max_ref = scene.config.max_reference_shots as usize;
    // Converted exactly as the pipeline converts them: existing sections and
    // import sections as targets through `Shot`, the import side from its
    // `ShotData`.
    let targets: Vec<(SectionId, Vec<MatchShot>)> = scene
        .existing
        .iter()
        .map(|e| {
            let shots = e
                .shots
                .iter()
                .map(|s| MatchShot::from_shot(s, e.azimuth_reference));
            (e.id, shots.collect())
        })
        .chain(scene.imports.iter().map(|i| {
            let reference = i.azimuth_reference.unwrap_or(scene.fallback);
            let shots =
                i.shots.iter().enumerate().map(|(k, sd)| {
                    MatchShot::from_shot(&Shot::new(k as u32, sd.clone()), reference)
                });
            (0x8000_0000 + i.index, shots.collect())
        }))
        .collect();
    for import in &scene.imports {
        if import.shots.is_empty() {
            continue;
        }
        let reference = import.azimuth_reference.unwrap_or(scene.fallback);
        let import_match = match_shots(&import.shots, reference);
        let import_trace = section_trace_points(&import_match);
        let n = import_match.len();
        for (target_id, target_match) in &targets {
            if *target_id == 0x8000_0000 + import.index || target_match.is_empty() {
                continue;
            }
            let existing_trace = section_trace_points(target_match);
            let m = target_match.len();
            let check = production_check(&import_trace, &existing_trace, &config, tally);
            let old_prefilter = reference::CrossingPrefilter::build(&import_trace, &existing_trace);
            for _ in 0..12 {
                let near_leaf = rng.chance(0.7);
                let import_station = if near_leaf {
                    let k = rng.below(max_ref.min(n) + 1);
                    if rng.chance(0.5) {
                        k
                    } else {
                        n - k
                    }
                } else {
                    rng.below(n + 2)
                };
                let existing_station = rng.below(m + 2);
                let ignored: Vec<(usize, usize)> = match rng.below(3) {
                    0 => Vec::new(),
                    1 => {
                        let matched = rng.between(1, max_ref.max(1));
                        let suggestion = PlacementSuggestion {
                            existing_section_id: *target_id,
                            existing_station_index: existing_station,
                            import_station_index: if rng.chance(0.5) { 0 } else { n },
                            score: 0.9,
                            matched_shots: matched,
                            existing_matched_shots: matched,
                            preferred_merge_offset: matched,
                            direction: if rng.chance(0.5) {
                                MatchDirection::Forward
                            } else {
                                MatchDirection::Reverse
                            },
                            is_partial: false,
                            confidence: SuggestionConfidence::High,
                            existing_section_name: String::new(),
                        };
                        ignored_crossing_pairs_for_suggestion(&suggestion, n, m)
                    }
                    _ => (0..rng.below(8))
                        .map(|_| (rng.below(n), rng.below(m)))
                        .collect(),
                };
                let (new, skipped) = check(import_station, existing_station, &ignored);
                tally.chunk_skips += usize::from(skipped);
                let old = reference::placement_crossing_clearance_ok(
                    &import_trace,
                    &existing_trace,
                    old_prefilter.as_ref(),
                    import_station,
                    existing_station,
                    &ignored,
                    &config,
                );
                assert_eq!(
                    new, old,
                    "seed {seed:#018x} ({kind}): crossing check of import {} on target \
                     {target_id} at stations ({import_station}, {existing_station}), ignored \
                     {ignored:?}: optimised says clear={new}, reference clear={old}. Replay with \
                     PLACEMENT_DIFF_SEED={seed:#x}",
                    import.index
                );
                tally.replayed += 1;
                tally.replay_rejected += usize::from(!old);
            }
        }
    }
}

/// One pipeline run: its suggestions (`None` if it panicked) and the crossing
/// checks it made, up to the panic if there was one.
fn run_logged(
    pipeline: impl FnOnce() -> Vec<CrossSectionSuggestion>,
) -> (
    Option<Vec<CrossSectionSuggestion>>,
    Vec<crossing_log::Entry>,
) {
    crossing_log::start();
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(pipeline)).ok();
    (out, crossing_log::take())
}

/// Runs one seed's scene through both pipelines and the replay; panics, naming
/// the seed, on the first difference or when either pipeline panicked.
fn compare_seed(seed: u64, max_len: usize, tally: &mut Tally) {
    let (kind, scene) = scene(seed, max_len);

    let (new, new_log) = run_logged(|| {
        suggest_all_placements(
            &scene.imports,
            scene.fallback,
            &scene.existing,
            &scene.config,
        )
    });
    let (old, old_log) = run_logged(|| {
        reference::suggest_all_placements(
            &scene.imports,
            scene.fallback,
            &scene.existing,
            &scene.config,
        )
    });

    let at = first_difference(&new_log, &old_log);
    assert!(
        new_log == old_log,
        "seed {seed:#018x} ({kind}): the crossing checks differ at check {at} of {} / {}: \
         optimised {:?}, reference {:?} (section, import station, existing station, clear). \
         Replay with PLACEMENT_DIFF_SEED={seed:#x}",
        new_log.len(),
        old_log.len(),
        new_log.get(at),
        old_log.get(at),
    );
    assert!(
        new.is_some() && old.is_some(),
        "seed {seed:#018x} ({kind}): a pipeline panicked (optimised: {}, reference: {}). \
         Replay with PLACEMENT_DIFF_SEED={seed:#x}",
        new.is_none(),
        old.is_none()
    );
    let (Some(new), Some(old)) = (new, old) else {
        return;
    };
    let (new_print, old_print) = (fingerprint(&new), fingerprint(&old));
    let at = first_difference(&new_print, &old_print);
    assert!(
        new_print == old_print,
        "seed {seed:#018x} ({kind}): suggestion {at} of {} / {} differs:\n  optimised {:?}\n  \
         reference {:?}\nReplay with PLACEMENT_DIFF_SEED={seed:#x}",
        new_print.len(),
        old_print.len(),
        new_print.get(at),
        old_print.get(at),
    );

    tally.scenes += 1;
    tally.suggestions += old.len();
    tally.checks += old_log.len();
    tally.rejected += old_log.iter().filter(|e| !e.3).count();
    let mut rng = Rng(seed ^ 0x5EED_0576_4E9E_A7ED);
    replay_crossing_checks(seed, kind, &scene, &mut rng, tally);
}

fn env_u64(name: &str) -> Option<u64> {
    let raw = std::env::var(name).ok()?;
    let raw = raw.trim();
    match raw.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => raw.parse().ok(),
    }
}

/// The seeded scenes: by default 660 (60 per kind) of up to 160 shots, which
/// runs in seconds in a debug build.
#[test]
fn optimised_pipeline_matches_the_reference_on_seeded_scenes() {
    let max_len = env_u64("PLACEMENT_DIFF_MAX_LEN").map_or(160, |n| n.max(8) as usize);
    let seeds: Vec<u64> = if let Some(seed) = env_u64("PLACEMENT_DIFF_SEED") {
        vec![seed]
    } else {
        let first = env_u64("PLACEMENT_DIFF_FIRST").unwrap_or(0);
        let count = env_u64("PLACEMENT_DIFF_SEEDS").unwrap_or(660);
        (first..first + count).collect()
    };
    let mut tally = Tally::default();
    let mut by_kind: Vec<Tally> = KINDS.iter().map(|_| Tally::default()).collect();
    for &seed in &seeds {
        let kind = (seed % KINDS.len() as u64) as usize;
        // A panic inside either pipeline must still name its seed.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            compare_seed(seed, max_len, &mut by_kind[kind]);
        }));
        assert!(
            outcome.is_ok(),
            "seed {seed:#018x} ({}) panicked (message above). Replay with \
             PLACEMENT_DIFF_SEED={seed:#x}",
            KINDS[kind]
        );
    }
    for (kind, t) in KINDS.iter().zip(&by_kind) {
        println!(
            "  {kind:>16}: {:>5} scenes, {:>6} suggestions, {:>7} crossing checks ({:>6} \
             rejected), {:>6} replayed ({:>5} rejected); {:>5} pairs, {:>4} with cleared \
             segments ({:>6}), {:>4} sparse lanes, {:>5} replays skipping a chunk",
            t.scenes,
            t.suggestions,
            t.checks,
            t.rejected,
            t.replayed,
            t.replay_rejected,
            t.pairs,
            t.pairs_cleared,
            t.segments_cleared,
            t.sparse_lanes,
            t.chunk_skips
        );
        tally.scenes += t.scenes;
        tally.suggestions += t.suggestions;
        tally.checks += t.checks;
        tally.rejected += t.rejected;
        tally.replayed += t.replayed;
        tally.replay_rejected += t.replay_rejected;
        tally.pairs += t.pairs;
        tally.pairs_cleared += t.pairs_cleared;
        tally.segments_cleared += t.segments_cleared;
        tally.sparse_lanes += t.sparse_lanes;
        tally.chunk_skips += t.chunk_skips;
    }
    println!(
        "placement differential: {} scenes (seeds {:#x}..), {} suggestions, {} pipeline crossing \
         checks ({} rejected), {} replayed checks ({} rejected); {} section pairs, {} with \
         segments proven parallel to all ({} segments), {} sparse lanes, {} replays skipping a \
         chunk; 0 panics, 0 mismatches",
        tally.scenes,
        seeds.first().copied().unwrap_or(0),
        tally.suggestions,
        tally.checks,
        tally.rejected,
        tally.replayed,
        tally.replay_rejected,
        tally.pairs,
        tally.pairs_cleared,
        tally.segments_cleared,
        tally.sparse_lanes,
        tally.chunk_skips,
    );
    // The comparison must exercise both verdicts and every shortcut, not just
    // agree on easy passes (skipped for a single replayed seed).
    if seeds.len() > 100 {
        assert!(
            tally.suggestions > 1000
                && tally.rejected > 1000
                && tally.checks > 2 * tally.rejected
                && tally.replay_rejected > 1000
                && tally.pairs_cleared > 100
                && tally.sparse_lanes > 100
                && tally.chunk_skips > 1000,
            "too little coverage: {} suggestions, {} checks, {} rejected, {} replay rejections, \
             {} pairs with cleared segments, {} sparse lanes, {} chunk skips",
            tally.suggestions,
            tally.checks,
            tally.rejected,
            tally.replay_rejected,
            tally.pairs_cleared,
            tally.sparse_lanes,
            tally.chunk_skips
        );
    }
}

fn straight_section(id: SectionId, len: usize, azimuth: f64) -> ExistingSection {
    existing_section(
        id,
        &straight(len, azimuth, 5.0, 10.0),
        AzimuthReference::default(),
    )
}

fn straight_import(index: u32, len: usize, azimuth: f64) -> ImportSectionForSuggestion {
    ImportSectionForSuggestion {
        index,
        name: format!("Import{index}"),
        shots: straight(len, azimuth, 5.0, 10.0),
        azimuth_reference: None,
    }
}

/// The scenes of the timing probe at full size, and bent / skewed / wobbling
/// variants of the worst one. Minutes in a debug build; run in release:
/// `cargo test --release -p stalagmight-models --lib differential -- --ignored --nocapture`.
#[test]
#[ignore = "worst-case sizes; run in release with --ignored --nocapture"]
fn optimised_pipeline_matches_the_reference_at_worst_case_sizes() {
    let mut rng = Rng(0x0576_F4D1_0B16_0001);
    let mut scenes: Vec<(String, Scene)> = Vec::new();
    let mut add =
        |label: &str, existing: Vec<ExistingSection>, imports: Vec<ImportSectionForSuggestion>| {
            scenes.push((
                label.to_owned(),
                Scene {
                    imports,
                    existing,
                    fallback: AzimuthReference::default(),
                    config: PlacementSuggestionConfig::default(),
                },
            ));
        };
    let random_existing = |rng: &mut Rng, id: SectionId, len: usize| {
        existing_section(
            id,
            &random_run(rng, len, 25.0, 2.0),
            AzimuthReference::default(),
        )
    };
    let random_import = |rng: &mut Rng, index: u32, len: usize| ImportSectionForSuggestion {
        index,
        name: format!("Import{index}"),
        shots: random_run(rng, len, 25.0, 2.0),
        azimuth_reference: None,
    };
    add(
        "spec-scale random 200x50 / 20x50",
        (0..200).map(|i| random_existing(&mut rng, i, 50)).collect(),
        (0..20).map(|i| random_import(&mut rng, i, 50)).collect(),
    );
    add(
        "spec-scale straight 200x50 / 20x50",
        (0..200)
            .map(|i| straight_section(i, 50, f64::from(i) * 1.7))
            .collect(),
        (0..20)
            .map(|i| straight_import(i, 50, f64::from(i) * 17.0))
            .collect(),
    );
    add(
        "long random 1x2000 / 1x2000",
        vec![random_existing(&mut rng, 0, 2000)],
        vec![random_import(&mut rng, 0, 2000)],
    );
    add(
        "long straight 1x2000 / 1x2000",
        vec![straight_section(0, 2000, 90.0)],
        vec![straight_import(0, 2000, 90.0)],
    );
    add(
        "long straight 5x2000 / 2x2000",
        (0..5).map(|i| straight_section(i, 2000, 90.0)).collect(),
        (0..2).map(|i| straight_import(i, 2000, 90.0)).collect(),
    );
    add(
        "long straight at 37 degrees 2x2000 / 2x2000",
        (0..2).map(|i| straight_section(i, 2000, 37.0)).collect(),
        (0..2).map(|i| straight_import(i, 2000, 37.0)).collect(),
    );
    let mut bent = straight(1000, 90.0, 5.0, 10.0);
    bent.extend(
        (0..8).map(|k| straight_shot(90.0 - 11.0 * f64::from(k), 5.0).with_depths(10.0, 10.0)),
    );
    bent.extend(straight(992, 0.0, 5.0, 10.0));
    add(
        "bent 2000 / straight 2000",
        vec![existing_section(0, &bent, AzimuthReference::default())],
        vec![straight_import(0, 2000, 90.0)],
    );
    add(
        "straight 2000 / skewed 2000 (1e-7 deg)",
        vec![straight_section(0, 2000, 90.0)],
        vec![straight_import(0, 2000, 90.0 + 1e-7)],
    );
    add(
        "long straight 1 degree apart 2x2000 / 2x2000 (90 / 91 deg)",
        (0..2).map(|i| straight_section(i, 2000, 90.0)).collect(),
        (0..2).map(|i| straight_import(i, 2000, 91.0)).collect(),
    );
    add(
        "long straight 1 degree apart 2x2000 / 2x2000 (37 / 38 deg)",
        (0..2).map(|i| straight_section(i, 2000, 37.0)).collect(),
        (0..2).map(|i| straight_import(i, 2000, 38.0)).collect(),
    );
    let wobble = |rng: &mut Rng, len: usize, scale: f64| -> Vec<ShotData> {
        (0..len)
            .map(|_| {
                straight_shot(90.0 + scale * rng.range(-1.0, 1.0), 5.0).with_depths(10.0, 10.0)
            })
            .collect()
    };
    let wobbled = wobble(&mut rng, 2000, 1e-9);
    add(
        "wobbling 2000 / wobbling 2000 (1e-9 deg)",
        vec![existing_section(0, &wobbled, AzimuthReference::default())],
        vec![ImportSectionForSuggestion {
            index: 0,
            name: "Import0".to_owned(),
            shots: wobble(&mut rng, 2000, 1e-9),
            azimuth_reference: None,
        }],
    );

    let mut tally = Tally::default();
    for (label, scene) in &scenes {
        crossing_log::start();
        let start = std::time::Instant::now();
        let new = suggest_all_placements(
            &scene.imports,
            scene.fallback,
            &scene.existing,
            &scene.config,
        );
        let new_ms = start.elapsed().as_secs_f64() * 1000.0;
        let new_log = crossing_log::take();
        crossing_log::start();
        let start = std::time::Instant::now();
        let old = reference::suggest_all_placements(
            &scene.imports,
            scene.fallback,
            &scene.existing,
            &scene.config,
        );
        let old_ms = start.elapsed().as_secs_f64() * 1000.0;
        let old_log = crossing_log::take();
        assert!(
            new_log == old_log,
            "{label}: crossing checks differ at {}",
            first_difference(&new_log, &old_log)
        );
        assert_eq!(fingerprint(&new), fingerprint(&old), "{label}");
        println!(
            "{label}: {} suggestions, {} crossing checks ({} rejected); optimised {new_ms:.1} ms, \
             reference {old_ms:.1} ms",
            old.len(),
            old_log.len(),
            old_log.iter().filter(|e| !e.3).count(),
        );
        tally.scenes += 1;
    }
    let mut replay_rng = Rng(0x0576_F4D1_0B16_0002);
    for (label, scene) in &scenes {
        replay_crossing_checks(0, label, scene, &mut replay_rng, &mut tally);
    }
    println!(
        "{} worst-case scenes, {} replayed checks, 0 mismatches",
        tally.scenes, tally.replayed
    );
}

/// The base (b83b12de5) crossing pipeline, verbatim but for the work counter,
/// the crossing log, the `pub(super)` the harness needs, the score order
/// (`score_cmp`, #576 NF-W9PERF-1: the old `partial_cmp`, made total, as in
/// production - it orders two numbers exactly as before) and the admission
/// (`admitted`, #576 NF-W10BE-1: the old `score < min_aggregate_score` test,
/// which also turns a NaN score away, as in production - it admits every
/// number exactly as before).
mod reference {
    use super::super::crossing::{
        crossing_prefilter_margin, max_segment_extent, pair_crosses_too_close, segment_box,
        trace_is_finite,
    };
    use super::super::{
        admitted, ignored_crossing_pairs_for_suggestion, log_crossing_check, merge_offset_pairs,
        merge_station_pair_for_offsets, score_cmp, section_trace_points,
        station_context_multiplier, suppress_duplicate_endpoint_pairs,
        suppress_equivalent_single_front_reverses, try_match_sequence, CrossSectionSuggestion,
        ExistingSection, ImportSectionForSuggestion, MatchDirection, MatchShot,
        PlacementSuggestion, SuggestionConfidence, SuggestionConfig, TargetType, TracePoint,
    };
    use crate::config::PlacementSuggestionConfig;
    use crate::{AzimuthReference, SectionId, Shot, ShotData};

    /// Suggest placements for ALL import sections against BOTH existing project
    /// sections AND each other. Returns a flat list sorted by score descending.
    ///
    /// Each import section is scored in ITS OWN frame
    /// ([`ImportSectionForSuggestion::azimuth_reference`]); `fallback_reference` is
    /// what a section that declares none is read against, normally the project
    /// default (#438). Every comparison therefore has both sides in true north:
    /// import-to-existing uses each existing section's own reference, and
    /// import-to-import uses each import section's.
    pub(super) fn suggest_all_placements(
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
            let existing_suggestions = suggest_placements_among(
                &import.shots,
                import_reference,
                existing_sections.iter(),
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

    /// [#576 A2] An existing section's trace segments bucketed into a uniform
    /// grid, built once per (import, existing) section pair, so the crossing
    /// check visits the existing segments NEAR each placed import segment instead
    /// of all of them. The exhaustive loop tested every import segment against
    /// every existing one for every surviving candidate and merge offset; on two
    /// straight 2,000-shot sections that was 370 s.
    pub(super) struct CrossingPrefilter {
        /// See [`crossing_prefilter_margin`].
        margin: f64,
        origin_x: f64,
        origin_y: f64,
        cell: f64,
        nx: usize,
        ny: usize,
        /// CSR buckets: the segments of cell `(cx, cy)` are
        /// `entries[starts[cy * nx + cx]..starts[cy * nx + cx + 1]]`.
        starts: Vec<usize>,
        entries: Vec<usize>,
        boxes: Vec<[f64; 4]>,
    }

    impl CrossingPrefilter {
        /// `None` when the exact bound does not apply (non-finite coordinates, or
        /// segments too long for [`crossing_prefilter_margin`]): the caller then
        /// tests every pair.
        pub(super) fn build(
            import_trace: &[TracePoint],
            existing_trace: &[TracePoint],
        ) -> Option<Self> {
            if !trace_is_finite(import_trace) || !trace_is_finite(existing_trace) {
                return None;
            }
            let extent = max_segment_extent(import_trace).max(max_segment_extent(existing_trace));
            let margin = crossing_prefilter_margin(extent)?;
            let boxes: Vec<[f64; 4]> = existing_trace
                .windows(2)
                .map(|w| segment_box(w[0], w[1]))
                .collect();
            let (origin_x, max_x, origin_y, max_y) = boxes.iter().fold(
                (
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                ),
                |(lx, hx, ly, hy), b| (lx.min(b[0]), hx.max(b[1]), ly.min(b[2]), hy.max(b[3])),
            );
            if boxes.is_empty() {
                return None;
            }
            let (width, height) = (max_x - origin_x, max_y - origin_y);
            // At least one segment's extent and both margins wide, so a query
            // spans a handful of cells; and no finer than ~4 cells per segment
            // over the trace's box, so the grid stays linear in the section.
            let cell = extent
                .max(2.0 * margin)
                .max(1.0)
                .max((width * height / (4.0 * boxes.len() as f64)).sqrt());
            let nx = (width / cell).floor() as usize + 1;
            let ny = (height / cell).floor() as usize + 1;
            let mut grid = Self {
                margin,
                origin_x,
                origin_y,
                cell,
                nx,
                ny,
                starts: vec![0; nx * ny + 1],
                entries: Vec::new(),
                boxes,
            };
            // Counting pass, then fill: two walks of the same cell ranges.
            for b in &grid.boxes {
                let (x0, x1, y0, y1) = grid.cell_range(b);
                for cy in y0..=y1 {
                    for cx in x0..=x1 {
                        grid.starts[cy * nx + cx + 1] += 1;
                    }
                }
            }
            for i in 1..grid.starts.len() {
                grid.starts[i] += grid.starts[i - 1];
            }
            let mut fill = grid.starts.clone();
            grid.entries = vec![0; grid.starts[nx * ny]];
            for (segment, b) in grid.boxes.iter().enumerate() {
                let (x0, x1, y0, y1) = grid.cell_range(b);
                for cy in y0..=y1 {
                    for cx in x0..=x1 {
                        let slot = &mut fill[cy * nx + cx];
                        grid.entries[*slot] = segment;
                        *slot += 1;
                    }
                }
            }
            Some(grid)
        }

        /// The cell column of `x`, clamped into the grid.
        fn column(&self, x: f64) -> usize {
            (((x - self.origin_x) / self.cell).floor().max(0.0) as usize).min(self.nx - 1)
        }

        fn row(&self, y: f64) -> usize {
            (((y - self.origin_y) / self.cell).floor().max(0.0) as usize).min(self.ny - 1)
        }

        /// Inclusive cell ranges `(x0, x1, y0, y1)` a box covers, clamped.
        fn cell_range(&self, b: &[f64; 4]) -> (usize, usize, usize, usize) {
            (
                self.column(b[0]),
                self.column(b[1]),
                self.row(b[2]),
                self.row(b[3]),
            )
        }

        /// Does any existing segment the prefilter cannot rule out cross the
        /// placed import segment `(a, b)` too closely? Every existing segment
        /// whose box comes within `margin` of the import segment's box is tested
        /// exactly once; every other one is provably one the exact test would
        /// pass (see [`crossing_prefilter_margin`]).
        fn segment_violates(
            &self,
            existing_trace: &[TracePoint],
            import_idx: usize,
            a: TracePoint,
            b: TracePoint,
            ignored_pairs: &[(usize, usize)],
            config: &SuggestionConfig,
        ) -> bool {
            let seg = segment_box(a, b);
            let query = [
                seg[0] - self.margin,
                seg[1] + self.margin,
                seg[2] - self.margin,
                seg[3] + self.margin,
            ];
            let grid_hi_x = self.origin_x + self.cell * self.nx as f64;
            let grid_hi_y = self.origin_y + self.cell * self.ny as f64;
            if query[1] < self.origin_x
                || query[0] > grid_hi_x
                || query[3] < self.origin_y
                || query[2] > grid_hi_y
            {
                return false;
            }
            let (x0, x1, y0, y1) = self.cell_range(&query);
            for cy in y0..=y1 {
                for cx in x0..=x1 {
                    let cell = cy * self.nx + cx;
                    for &existing_idx in &self.entries[self.starts[cell]..self.starts[cell + 1]] {
                        let e = &self.boxes[existing_idx];
                        if e[0] > query[1] || e[1] < query[0] || e[2] > query[3] || e[3] < query[2]
                        {
                            continue;
                        }
                        // A segment spanning several cells is met in each; test it
                        // only in the cell holding the low corner of the overlap,
                        // which lies inside both boxes and so in exactly one
                        // visited cell.
                        if self.column(query[0].max(e[0])) != cx
                            || self.row(query[2].max(e[2])) != cy
                        {
                            continue;
                        }
                        if ignored_pairs.contains(&(import_idx, existing_idx)) {
                            continue;
                        }
                        if pair_crosses_too_close(
                            a,
                            b,
                            existing_trace[existing_idx],
                            existing_trace[existing_idx + 1],
                            config,
                        ) {
                            return true;
                        }
                    }
                }
            }
            false
        }
    }

    pub(super) fn placement_crossing_clearance_ok(
        import_trace: &[TracePoint],
        existing_trace: &[TracePoint],
        prefilter: Option<&CrossingPrefilter>,
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

        // The placed import is translated on the fly - the same `translated`
        // arithmetic, so the same coordinates - rather than copied per call.
        for (import_idx, w) in import_trace.windows(2).enumerate() {
            let import_a = w[0].translated(dx, dy);
            let import_b = w[1].translated(dx, dy);
            let violates = match prefilter {
                Some(grid) => grid.segment_violates(
                    existing_trace,
                    import_idx,
                    import_a,
                    import_b,
                    ignored_pairs,
                    config,
                ),
                None => existing_trace
                    .windows(2)
                    .enumerate()
                    .any(|(existing_idx, e)| {
                        !ignored_pairs.contains(&(import_idx, existing_idx))
                            && pair_crosses_too_close(import_a, import_b, e[0], e[1], config)
                    }),
            };
            if violates {
                return false;
            }
        }

        true
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
        prefilter: Option<&CrossingPrefilter>,
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
                let (merge_import_station, merge_existing_station) =
                    merge_station_pair_for_offsets(
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
                    prefilter,
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

        // Build import match shots for station N (last N shots, reversed)
        let import_from_n: Vec<MatchShot> = import_shots
            [import_shots.len().saturating_sub(max_ref)..]
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
            let existing_trace = section_trace_points(&existing_match_shots);
            let prefilter = CrossingPrefilter::build(&import_trace, &existing_trace);

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

                        let best_context = best_merge_offset(
                            &import_match_shots,
                            &existing_match_shots,
                            &import_trace,
                            &existing_trace,
                            prefilter.as_ref(),
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
}
