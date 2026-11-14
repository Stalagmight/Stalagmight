//! Timing of the Combine Sections candidate queries on a large synthetic
//! survey (#737 review, point 2): the station menu (`combine_candidates`,
//! `combine_candidates_at_point`) and the two-section query
//! (`combine_candidate_for_sections`) on 2000 sections of 20 shots, a merged
//! tree, and 300 Batch closures tying section ends (each one a closure join
//! that closes by a shift, so every query plans the anchor rule).
//!
//! `#[ignore]`: run it in release, by hand -
//! `cargo test --release -p stalagmight-models --lib -- --ignored --nocapture
//! combine_candidate_queries_on_a_large_survey`. It asserts nothing about
//! time; it prints it.

use std::time::Instant;

use crate::{ClosureStrategy, LoopClosure, Project, SectionData, ShotData, StationRef};

/// xorshift64*: deterministic, no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

const SECTIONS: u64 = 2_000;
const SHOTS: u32 = 20;
const CLOSURES: u64 = 300;

/// The survey, and the closure joins: (the end station the menu is opened on,
/// the two sections it joins).
fn large_survey() -> (Project, Vec<(StationRef, u32, u32)>) {
    let mut rng = Rng(0x0737_b3c4);
    let mut project = Project::new("Bench".to_string(), None);
    let mut ids = Vec::new();
    for i in 0..SECTIONS {
        let shots = (0..SHOTS)
            .map(|_| {
                let azimuth = rng.below(360) as f64;
                ShotData {
                    azimuth_in: azimuth,
                    azimuth_out: azimuth,
                    length: 2.0 + rng.below(30) as f64 / 10.0,
                    ..Default::default()
                }
            })
            .collect();
        let id = project
            .add_section(SectionData {
                name: Some(format!("S{i}")),
                shots,
                ..Default::default()
            })
            .id;
        if i > 0 {
            // A tree: this section's start merged to a station of an earlier one.
            let parent = ids[rng.below(i) as usize];
            let at = rng.below(u64::from(SHOTS) + 1) as u32;
            project
                .merge_stations(StationRef::new(id, 0), StationRef::new(parent, at))
                .unwrap();
        }
        ids.push(id);
    }
    // Closures from a section's end to another's start: each is a closure
    // join the station menu offers on that end.
    let mut joins = Vec::new();
    for _ in 0..CLOSURES {
        let a = ids[rng.below(SECTIONS) as usize];
        let b = ids[rng.below(SECTIONS) as usize];
        if a == b {
            continue;
        }
        let end = StationRef::new(a, SHOTS);
        let start = StationRef::new(b, 0);
        let id = project.next_closure_id;
        project.next_closure_id += 1;
        project.loop_closures.push(LoopClosure {
            id,
            station_a: project.key_for_ref(end).unwrap(),
            station_b: project.key_for_ref(start).unwrap(),
            path_length: 1.0,
            misclosure_distance: 1.0,
            error_percentage: 1.0,
            horizontal_misclosure: 1.0,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::Batch,
            is_aligned: false,
        });
        joins.push((end, a, b));
    }
    (project, joins)
}

fn time(what: &str, queries: usize, mut f: impl FnMut() -> usize) {
    let started = Instant::now();
    let found = f();
    let took = started.elapsed();
    let per = took / u32::try_from(queries.max(1)).unwrap_or(1);
    eprintln!("{what}: {queries} queries in {took:?} ({per:?} each, {found} candidates)");
}

#[test]
#[ignore = "timing: run in release with --ignored --nocapture"]
fn combine_candidate_queries_on_a_large_survey() {
    let built = Instant::now();
    let (project, joins) = large_survey();
    let stations: usize = project.sections.iter().map(|s| s.shots.len() + 1).sum();
    eprintln!(
        "survey: {} sections, {stations} stations, {} merges, {} closures (built in {:?})",
        project.sections.len(),
        project.station_merges.len(),
        project.loop_closures.len(),
        built.elapsed()
    );
    let sample: Vec<_> = joins.iter().take(100).copied().collect();
    // What one across-one-shot sweep costs here: before the review fix the
    // anchor rule ran one per shifting plan, two per closure-join candidate.
    time("closures_across_one_shot (one sweep)", 20, || {
        (0..20)
            .map(|_| project.closures_across_one_shot().len())
            .sum()
    });
    // For scale: the component walk every shifting plan already does (#735).
    time("get_connected_section_ids (one walk)", 20, || {
        (0..20)
            .map(|_| project.get_connected_section_ids(sample[0].1).len())
            .sum()
    });
    time("combine_candidates (station menu)", sample.len(), || {
        sample
            .iter()
            .map(|(at, ..)| project.combine_candidates(*at).len())
            .sum()
    });
    time("combine_candidates_at_point", sample.len(), || {
        sample
            .iter()
            .map(|(at, ..)| project.combine_candidates_at_point(*at).len())
            .sum()
    });
    time(
        "combine_candidate_for_sections (Combine Sections...)",
        sample.len(),
        || {
            sample
                .iter()
                .filter_map(|(_, a, b)| project.combine_candidate_for_sections(*a, *b))
                .count()
        },
    );
}
