//! [#576 A10 / A13] The two lookup caches (`section_index_cache`,
//! `shot_uuid_index`) stay equal to a fresh rebuild across every mutation
//! that moves a section or a shot.
//!
//! Both caches are self-validating - a stale entry falls back to a linear
//! scan, so no answer is ever wrong - which is exactly why a forgotten
//! maintenance step went unnoticed: `remove_section` CLEARED the section cache
//! and nothing on the forward path rebuilt it, so every later `get_section`
//! was `O(sections)` for the rest of the session (measured: the coordinate
//! solve 109 -> 572 ms per edit at 14,299 sections), and a mid-section shot
//! insert or delete left that section's shot-uuid entries pointing at the
//! wrong indices.

use std::time::Instant;

use super::Project;
use crate::test_fixtures::straight_shot;
use crate::{SectionData, SectionId, ShotData, StationAnchor, StationKey, StationMerge};

/// xorshift64* - deterministic, dependency-free.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn shot(i: usize) -> ShotData {
    straight_shot((i * 37 % 360) as f64, 2.0 + (i % 7) as f64).with_depths(10.0, 10.0)
}

fn section_data(shots: usize) -> SectionData {
    SectionData {
        name: Some("CaveA".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots: (0..shots).map(shot).collect(),
    }
}

/// The caches as they are, against the caches a full rebuild produces.
fn assert_caches_fresh(project: &Project, step: &str) {
    let mut fresh = project.clone();
    fresh.rebuild_section_index();
    fresh.rebuild_shot_uuid_index();
    assert_eq!(
        project.cache_snapshot(),
        fresh.cache_snapshot(),
        "caches went stale after {step}"
    );
}

fn project_with(sections: usize, shots: usize) -> Project {
    let mut project = Project::new("CaveA".to_string(), None);
    for _ in 0..sections {
        project.add_section(section_data(shots));
    }
    project
}

#[test]
fn removing_a_section_keeps_the_section_index_warm() {
    let mut project = project_with(6, 3);
    assert_caches_fresh(&project, "add_section");
    let doomed = project.sections[2].id;
    assert!(project.remove_section(doomed).is_some());
    assert_caches_fresh(&project, "remove_section");
    let first = project.sections[0].id;
    assert!(project.remove_section(first).is_some());
    assert_caches_fresh(&project, "remove_section of the first section");
}

#[test]
fn inserting_a_shot_mid_section_reindexes_the_section() {
    let mut project = project_with(2, 5);
    let (section_id, after) = (project.sections[1].id, project.sections[1].shots[1].id);
    assert!(project
        .insert_shot_after(section_id, after, shot(99), None, None)
        .is_some());
    assert_caches_fresh(&project, "insert_shot_after");
    let first = project.sections[1].shots[0].id;
    assert!(project
        .insert_shot_before(section_id, first, shot(98), None, None)
        .is_some());
    assert_caches_fresh(&project, "insert_shot_before (prepend)");
}

#[test]
fn deleting_and_splitting_shots_reindex_the_section() {
    let mut project = project_with(2, 6);
    let section_id = project.sections[0].id;
    let middle = project.sections[0].shots[2].id;
    assert!(project.remove_shot(section_id, middle).is_some());
    assert_caches_fresh(&project, "remove_shot");
    let to_split = project.sections[0].shots[1].id;
    assert!(project
        .split_shot_at_distance(section_id, to_split, 1.0)
        .is_ok());
    assert_caches_fresh(&project, "split_shot_at_distance");
    assert!(project.add_shot(section_id, shot(7)).is_some());
    assert_caches_fresh(&project, "add_shot");
}

/// Random mutation sequences: after EVERY step both caches equal a rebuild,
/// and every station key resolves exactly as the cold scan resolves it.
#[test]
fn caches_stay_fresh_across_random_mutation_sequences() {
    let mut rng = Rng(0xCA_C4E5_0576_A10A);
    for run in 0..60 {
        let mut project = project_with(1 + rng.below(4), rng.below(6));
        for step in 0..40 {
            let op = rng.below(8);
            let label = format!("run {run} step {step} op {op}");
            match op {
                0 => {
                    project.add_section(section_data(rng.below(6)));
                }
                1 if project.sections.len() > 1 => {
                    let id = project.sections[rng.below(project.sections.len())].id;
                    project.remove_section(id);
                }
                2 | 3 | 4 | 5 | 6 | 7 => {
                    let Some(section) = project
                        .sections
                        .get(rng.below(project.sections.len()))
                        .cloned()
                    else {
                        continue;
                    };
                    let pick = |rng: &mut Rng| {
                        (!section.shots.is_empty())
                            .then(|| section.shots[rng.below(section.shots.len())].id)
                    };
                    match (op, pick(&mut rng)) {
                        (2, _) => {
                            project.add_shot(section.id, shot(step));
                        }
                        (3, Some(id)) => {
                            project.insert_shot_after(section.id, id, shot(step), None, None);
                        }
                        (4, Some(id)) => {
                            project.insert_shot_before(section.id, id, shot(step), None, None);
                        }
                        (5, Some(id)) => {
                            project.remove_shot(section.id, id);
                        }
                        (6, Some(id)) => {
                            let _ = project.split_shot_at_distance(section.id, id, 0.75);
                        }
                        (7, _) => {
                            project.reverse_section(section.id, false);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            assert_caches_fresh(&project, &label);
            for section in &project.sections {
                for (i, s) in section.shots.iter().enumerate() {
                    let Some(uuid) = s.uuid.clone() else { continue };
                    let key = StationKey::new(section.id, StationAnchor::AfterShot(uuid.clone()));
                    let cold = section
                        .shots
                        .iter()
                        .position(|x| x.uuid.as_deref() == Some(uuid.as_str()))
                        .map(|p| p as u32 + 1);
                    assert_eq!(
                        project.resolve_key(&key).map(|r| r.station_index),
                        cold,
                        "{label}: shot {i} of section {}",
                        section.id
                    );
                }
            }
        }
    }
}

/// A chain of `sections` short sections, each merged end-to-start onto the
/// next by shot uuid - the merge-endpoint key shape the solver resolves.
fn merged_chain(sections: usize, shots: usize) -> Project {
    let mut project = project_with(sections, shots);
    let ids: Vec<SectionId> = project.sections.iter().map(|s| s.id).collect();
    for pair in ids.windows(2) {
        let last_uuid = project
            .get_section(pair[0])
            .and_then(|s| s.shots.last()?.uuid.clone())
            .unwrap_or_default();
        project.record_station_merge(StationMerge {
            station_a: StationKey::new(pair[0], StationAnchor::AfterShot(last_uuid)),
            station_b: StationKey::new(pair[1], StationAnchor::SectionStart),
            merged_at: chrono::Utc::now(),
        });
    }
    project
}

fn time_ms<T>(f: impl FnOnce() -> T) -> (f64, T) {
    let start = Instant::now();
    let out = f();
    (start.elapsed().as_secs_f64() * 1000.0, out)
}

/// The A10 / A13 analysis scene: 14,299 sections, ~75k shots, a merge per
/// section boundary. Run in release:
/// `cargo test --release -p stalagmight-models --lib cache_probe -- --ignored --nocapture`
#[test]
#[ignore = "timing probe; run in release with --ignored --nocapture"]
fn cache_probe_after_a_section_delete() {
    let mut project = merged_chain(14_299, 5);
    // One section with no connections, so deleting it disturbs nothing else.
    let spare = project.add_section(section_data(5)).id;
    let keys: Vec<StationKey> = project
        .station_merges
        .iter()
        .flat_map(|m| [m.station_a.clone(), m.station_b.clone()])
        .collect();
    let shots: usize = project.sections.iter().map(|s| s.shots.len()).sum();
    println!(
        "scene: {} sections, {shots} shots, {} merge-endpoint keys",
        project.sections.len(),
        keys.len()
    );

    let measure = |project: &Project, label: &str| {
        let (resolve, resolved) =
            time_ms(|| keys.iter().filter_map(|k| project.resolve_key(k)).count());
        let (solve, stations) = time_ms(|| project.compute_station_coordinates().len());
        println!(
            "{label}: {} key resolves {resolve:.2} ms; compute_station_coordinates {solve:.1} ms \
             ({stations} stations)",
            resolved
        );
    };
    measure(&project, "warm, before any delete");
    assert!(project.remove_section(spare).is_some());
    measure(&project, "after one remove_section");
}
