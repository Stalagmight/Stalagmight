//! [#576 RV18-GEO-1] A loop closure's metrics are a function of the project
//! alone: the same project gives bit-identical path length, error percentage
//! and misclosures on every call, in every process.
//!
//! Every `HashMap`/`HashSet` in one process is keyed afresh, so computing the
//! same metrics many times in one test run walks any hash-ordered float sum in
//! many orders. Float addition is not associative, so a dependence on hash
//! order shows up as two calls a last bit apart.
//!
//! The one dependence these found was `path_length_dijkstra`'s no-route
//! fallback, which summed shot lengths in a `HashSet`'s order (see
//! `pathfinding::path_length_tests`). The misclosures, the stored offsets and
//! the least-squares adjustment never differed.

use super::*;
use crate::{ClosurePreview, ClosureStrategy};

/// A deterministic generator (the one `component_pass_tests` uses).
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % n
    }

    fn real(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * (self.below(1_000_000) as f64 / 1_000_000.0)
    }
}

const FIELDS: [&str; 5] = [
    "path_length",
    "misclosure_distance",
    "error_percentage",
    "horizontal_misclosure",
    "vertical_misclosure",
];

fn preview_values(m: &ClosurePreview) -> [f64; 5] {
    [
        m.path_length,
        m.misclosure_distance,
        m.error_percentage,
        m.horizontal_misclosure,
        m.vertical_misclosure,
    ]
}

fn closure_values(c: &LoopClosure) -> [f64; 5] {
    [
        c.path_length,
        c.misclosure_distance,
        c.error_percentage,
        c.horizontal_misclosure,
        c.vertical_misclosure,
    ]
}

/// Panic naming the first field whose bits differ.
fn assert_same_bits(first: [f64; 5], again: [f64; 5], label: &str) {
    for (field, (x, y)) in FIELDS.iter().zip(first.iter().zip(again.iter())) {
        assert_eq!(x.to_bits(), y.to_bits(), "{label} {field}: {x} vs {y}");
    }
}

/// Half the shot lengths are the awkward decimals whose sums depend on order
/// (0.1 + 0.2 + 0.3 is not 0.3 + 0.2 + 0.1).
fn length(rng: &mut Lcg) -> f64 {
    const AWKWARD: [f64; 8] = [0.1, 0.2, 0.3, 0.6, 0.7, 1.1, 2.3, 3.3];
    if rng.below(2) == 0 {
        AWKWARD[rng.below(AWKWARD.len() as u64) as usize]
    } else {
        rng.real(0.5, 30.0)
    }
}

fn station_key(p: &Project, id: SectionId, index: usize) -> StationKey {
    match index {
        0 => StationKey::new(id, StationAnchor::SectionStart),
        k => {
            let uuid = p.get_section(id).unwrap().shots[k - 1]
                .uuid
                .clone()
                .unwrap();
            StationKey::new(id, StationAnchor::AfterShot(uuid))
        }
    }
}

fn random_key(rng: &mut Lcg, p: &Project, ids: &[SectionId]) -> StationKey {
    let id = ids[rng.below(ids.len() as u64) as usize];
    let count = p.get_section(id).unwrap().shots.len();
    station_key(p, id, rng.below(count as u64 + 1) as usize)
}

/// One to three components, each a merge tree of one to five sections (so
/// loops come only from closures), most of them zero-datum anchored, with
/// closures inside and across components. Returns every station too.
fn random_survey(seed: u64) -> (Project, Vec<StationRef>) {
    let mut rng = Lcg(seed);
    let mut p = Project::new(format!("CaveA {seed}"), None);
    let mut stations = Vec::new();
    let mut every_id = Vec::new();
    for _ in 0..=rng.below(3) {
        let mut ids: Vec<SectionId> = Vec::new();
        for _ in 0..=rng.below(5) {
            let shots: Vec<ShotData> = (0..=rng.below(4))
                .map(|_| {
                    let azimuth = rng.real(0.0, 360.0);
                    ShotData {
                        azimuth_in: azimuth,
                        azimuth_out: azimuth,
                        length: length(&mut rng),
                        depth_in: rng.real(0.0, 30.0),
                        depth_out: rng.real(0.0, 30.0),
                        ..Default::default()
                    }
                })
                .collect();
            let count = shots.len();
            let id = p
                .add_section(SectionData {
                    shots,
                    ..Default::default()
                })
                .id;
            stations.extend((0..=count).map(|i| StationRef::new(id, i as u32)));
            if !ids.is_empty() {
                let parent = random_key(&mut rng, &p, &ids);
                p.station_merges.push(StationMerge {
                    station_a: parent,
                    station_b: station_key(&p, id, 0),
                    merged_at: Utc::now(),
                });
            }
            ids.push(id);
        }
        if rng.below(3) != 0 {
            let key = random_key(&mut rng, &p, &ids);
            let lat = rng.real(20.0, 21.0);
            p.zero_datums.push(ZeroDatum {
                station_ref: key,
                gps: GpsCoordinates::new(lat, -87.4, 0.0).unwrap(),
            });
        }
        every_id.extend(ids);
    }
    for id in 1..=rng.below(6) {
        let (a, b) = (
            random_key(&mut rng, &p, &every_id),
            random_key(&mut rng, &p, &every_id),
        );
        p.loop_closures.push(LoopClosure {
            id,
            station_a: a,
            station_b: b,
            path_length: 0.0,
            misclosure_distance: 0.0,
            error_percentage: 0.0,
            horizontal_misclosure: 0.0,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::Batch,
            is_aligned: rng.below(3) == 0,
        });
    }
    p.next_closure_id = 100;
    for &id in &every_id {
        if rng.below(3) == 0 {
            let offset = (rng.real(-50.0, 50.0), rng.real(-50.0, 50.0));
            p.section_offsets.insert(id, offset);
        }
    }
    (p, stations)
}

fn random_pairs(rng: &mut Lcg, stations: &[StationRef], n: usize) -> Vec<(StationRef, StationRef)> {
    (0..n)
        .map(|_| {
            (
                stations[rng.below(stations.len() as u64) as usize],
                stations[rng.below(stations.len() as u64) as usize],
            )
        })
        .collect()
}

#[test]
fn closure_metrics_are_bit_identical_on_every_call_on_random_projects() {
    for seed in 0..1000 {
        let (mut p, stations) = random_survey(seed);
        p.redrive_offsets_after_load();
        // Every stored closure's endpoints, then pairs a preview could ask about
        // (many in two components no route joins).
        let mut pairs: Vec<(StationRef, StationRef)> = p
            .loop_closures
            .iter()
            .filter_map(|c| Some((p.resolve_key(&c.station_a)?, p.resolve_key(&c.station_b)?)))
            .collect();
        pairs.extend(random_pairs(&mut Lcg(!seed), &stations, 10));
        for (k, &(a, b)) in pairs.iter().enumerate() {
            let Ok(first) = p.compute_closure_metrics(a, b) else {
                continue;
            };
            for _ in 0..8 {
                let again = p.compute_closure_metrics(a, b).unwrap();
                assert_same_bits(
                    preview_values(&first),
                    preview_values(&again),
                    &format!("seed {seed} pair {k}"),
                );
            }
        }
    }
}

/// The metrics a project STORES: the importer's batch creation, single
/// creation and a least-squares adjustment, run three times from one start.
#[test]
fn stored_closure_metrics_are_bit_identical_across_runs_on_random_projects() {
    for seed in 0..500 {
        let (start, stations) = random_survey(seed);
        let pairs = random_pairs(&mut Lcg(seed ^ 0x1234_5678), &stations, 6);
        let run = |mut p: Project| -> Vec<[f64; 5]> {
            p.redrive_offsets_after_load();
            let _ = p.create_loop_closures_batch(&pairs[..3]);
            for &(a, b) in &pairs[3..] {
                let _ = p.create_loop_closure(a, b, ClosureStrategy::Batch);
            }
            let _ = p.adjust_loop_closures(
                pairs[0].0,
                ClosureStrategy::LeastSquaresWeighted,
                &ClosureAdjustmentConfig::default(),
            );
            p.loop_closures.iter().map(closure_values).collect()
        };
        let first = run(start.clone());
        for _ in 0..2 {
            let again = run(start.clone());
            assert_eq!(first.len(), again.len(), "seed {seed}: closure count");
            for (i, (x, y)) in first.iter().zip(again.iter()).enumerate() {
                assert_same_bits(*x, *y, &format!("seed {seed} closure {i}"));
            }
        }
    }
}

/// Five routes from a hub to one station, of lengths equal up to the last bit
/// (the same three shots in different orders): four joined on by merges, one
/// by closures. Dijkstra settles on one of them; which one may not depend on
/// hash order. (It never did - the heap and every adjacency list are filled in
/// `sections`/`station_merges`/`loop_closures` order - and this pins it.)
#[test]
fn near_equal_routes_give_one_path_length_on_every_call() {
    let mut p = Project::new("CaveA".to_string(), None);
    let level = |length: f64| ShotData {
        length,
        ..Default::default()
    };
    let hub = p
        .add_section(SectionData {
            shots: vec![level(1.0)],
            ..Default::default()
        })
        .id;
    let goal = p
        .add_section(SectionData {
            shots: vec![level(1.0)],
            ..Default::default()
        })
        .id;
    for order in [
        [0.1, 0.2, 0.3],
        [0.3, 0.2, 0.1],
        [0.2, 0.3, 0.1],
        [0.1, 0.3, 0.2],
    ] {
        let route = p
            .add_section(SectionData {
                shots: order.iter().map(|&l| level(l)).collect(),
                ..Default::default()
            })
            .id;
        p.station_merges.push(StationMerge {
            station_a: station_key(&p, hub, 1),
            station_b: station_key(&p, route, 0),
            merged_at: Utc::now(),
        });
        p.station_merges.push(StationMerge {
            station_a: station_key(&p, route, 3),
            station_b: station_key(&p, goal, 0),
            merged_at: Utc::now(),
        });
    }
    let route = p
        .add_section(SectionData {
            shots: [0.3, 0.1, 0.2].iter().map(|&l| level(l)).collect(),
            ..Default::default()
        })
        .id;
    for (id, (a, b)) in [((hub, 1), (route, 0)), ((route, 3), (goal, 0))]
        .into_iter()
        .enumerate()
    {
        p.loop_closures.push(LoopClosure {
            id: id as u64 + 1,
            station_a: station_key(&p, a.0, a.1),
            station_b: station_key(&p, b.0, b.1),
            path_length: 0.0,
            misclosure_distance: 0.0,
            error_percentage: 0.0,
            horizontal_misclosure: 0.0,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::Batch,
            is_aligned: false,
        });
    }
    let (from, to) = (StationRef::new(hub, 1), StationRef::new(goal, 1));
    let first = p.compute_closure_metrics(from, to).unwrap();
    assert!(
        (first.path_length - 1.6).abs() < 1e-9,
        "{}",
        first.path_length
    );
    for _ in 0..64 {
        let again = p.compute_closure_metrics(from, to).unwrap();
        assert_same_bits(
            preview_values(&first),
            preview_values(&again),
            "near-equal routes",
        );
    }
}
