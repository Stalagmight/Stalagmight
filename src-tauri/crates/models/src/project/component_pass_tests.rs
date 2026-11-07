//! The offset passes that walk the survey's components once (#597, #602): the
//! orphan-closure sweep, the load-time alignment remeasure and the offset
//! re-solve's seed choice. Each used to rescan every merge (and closure) per
//! visited section; the equivalence tests keep that old algorithm verbatim as
//! the oracle and require identical results on named and random projects.

use super::*;
use crate::test_fixtures::straight_shot;
use crate::ClosureStrategy;

/// Two zero-datum-anchored components of `n` one-shot sections each (chained by
/// merges), and `closures` cross-component loop closures between them. Every
/// closure is an orphan whose promotion is refused (it would double-anchor), so
/// the pre-#597 sweep ran a full walk per closure, rescanning every merge per
/// visited station, on every pass.
fn two_anchored_components_with_orphan_closures(n: usize, closures: usize) -> Project {
    let mut project = Project::new("Orphans".to_string(), None);
    let component = |project: &mut Project| -> Vec<SectionId> {
        let ids: Vec<SectionId> = (0..n)
            .map(|_| {
                project
                    .add_section(SectionData {
                        shots: vec![straight_shot(90.0, 1.0)],
                        ..Default::default()
                    })
                    .id
            })
            .collect();
        for pair in ids.windows(2) {
            project.station_merges.push(StationMerge {
                station_a: StationKey::new(
                    pair[0],
                    StationAnchor::AfterShot(
                        project.get_section(pair[0]).unwrap().shots[0]
                            .uuid
                            .clone()
                            .unwrap(),
                    ),
                ),
                station_b: StationKey::new(pair[1], StationAnchor::SectionStart),
                merged_at: Utc::now(),
            });
        }
        ids
    };
    let a = component(&mut project);
    let b = component(&mut project);
    for (ids, lat) in [(&a, 20.5), (&b, 20.6)] {
        project.zero_datums.push(ZeroDatum {
            station_ref: StationKey::new(ids[0], StationAnchor::SectionStart),
            gps: GpsCoordinates::new(lat, -87.4, 0.0).unwrap(),
        });
    }
    for i in 0..closures {
        let (sa, sb) = (a[i % n], b[(i * 7) % n]);
        project.loop_closures.push(LoopClosure {
            id: i as u64 + 1,
            station_a: StationKey::new(sa, StationAnchor::SectionStart),
            station_b: StationKey::new(sb, StationAnchor::SectionStart),
            path_length: 0.0,
            misclosure_distance: 0.0,
            error_percentage: 0.0,
            horizontal_misclosure: 0.0,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
    }
    project.next_closure_id = closures as u64 + 1;
    project
}

#[test]
fn the_orphan_sweep_is_fast_with_thousands_of_refused_promotions() {
    let mut project = two_anchored_components_with_orphan_closures(1_000, 2_000);

    let started = std::time::Instant::now();
    project.redrive_offsets_after_load();
    let elapsed = started.elapsed();

    assert_eq!(
        project.loop_closures.len(),
        2_000,
        "every promotion is refused (both sides anchored), so all stay closures"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "the whole post-load redrive took {elapsed:?}"
    );
}

#[test]
fn the_orphan_sweep_still_promotes_in_order_and_keeps_a_redundant_twin() {
    // One anchored component and one free one, bridged by two closures between
    // the same stations: the first becomes the merge, the second is then a
    // genuine (redundant) loop and stays a closure.
    let mut project = two_anchored_components_with_orphan_closures(3, 0);
    project.zero_datums.pop();
    let (a0, b0) = (project.sections[0].id, project.sections[3].id);
    for id in [11, 12] {
        project.loop_closures.push(LoopClosure {
            id,
            station_a: StationKey::new(a0, StationAnchor::SectionStart),
            station_b: StationKey::new(b0, StationAnchor::SectionStart),
            path_length: 0.0,
            misclosure_distance: 0.0,
            error_percentage: 0.0,
            horizontal_misclosure: 0.0,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
    }

    let converted = project.convert_orphan_closures_to_merges();

    assert_eq!(converted, 1);
    assert_eq!(
        project
            .loop_closures
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>(),
        [12]
    );
}

#[test]
fn the_orphan_sweep_sees_merges_through_a_deleted_section() {
    // Two anchored sections bridged only by merges through a section id that no
    // longer exists. The merges do not connect them (they do not resolve), so the
    // closure is an orphan; but `merge_would_double_anchor` walks stored ids, sees
    // one merge component, and allows the promotion. The sweep must agree.
    let mut project = two_anchored_components_with_orphan_closures(1, 1);
    let (a, b) = (project.sections[0].id, project.sections[1].id);
    let gone: SectionId = project.sections.iter().map(|s| s.id).max().unwrap_or(0) + 100;
    for (x, y) in [(a, gone), (gone, b)] {
        project.station_merges.push(StationMerge {
            station_a: StationKey::new(x, StationAnchor::SectionStart),
            station_b: StationKey::new(y, StationAnchor::SectionStart),
            merged_at: Utc::now(),
        });
    }

    assert_eq!(project.convert_orphan_closures_to_merges(), 1);
    assert!(project.loop_closures.is_empty());
}

#[test]
fn the_orphan_sweep_examines_the_closure_right_after_a_converted_one() {
    // Two different orphans side by side: converting the first must not skip
    // the second.
    let mut project = two_anchored_components_with_orphan_closures(3, 0);
    project.zero_datums.pop();
    let loose = project
        .add_section(SectionData {
            shots: vec![straight_shot(90.0, 1.0)],
            ..Default::default()
        })
        .id;
    let (a0, b0) = (project.sections[0].id, project.sections[3].id);
    for (id, other) in [(11, b0), (12, loose)] {
        project.loop_closures.push(LoopClosure {
            id,
            station_a: StationKey::new(a0, StationAnchor::SectionStart),
            station_b: StationKey::new(other, StationAnchor::SectionStart),
            path_length: 0.0,
            misclosure_distance: 0.0,
            error_percentage: 0.0,
            horizontal_misclosure: 0.0,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
    }

    assert_eq!(project.convert_orphan_closures_to_merges(), 2);
    assert!(project.loop_closures.is_empty());
}

#[test]
fn remeasuring_alignment_is_fast_with_thousands_of_aligned_closures_in_one_component() {
    // One 4000-section merge chain, with a snap-to-merge closure doubling every
    // merge: each closure's gap is zero, so each reaches the double-anchor
    // question, which a per-closure merge walk made quadratic.
    let mut project = two_anchored_components_with_orphan_closures(4_000, 0);
    let doubled: Vec<(StationKey, StationKey)> = project
        .station_merges
        .iter()
        .map(|m| (m.station_a.clone(), m.station_b.clone()))
        .collect();
    for (i, (a, b)) in doubled.into_iter().enumerate() {
        project.loop_closures.push(LoopClosure {
            id: i as u64 + 1,
            station_a: a,
            station_b: b,
            path_length: 0.0,
            misclosure_distance: 0.0,
            error_percentage: 0.0,
            horizontal_misclosure: 0.0,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::Move,
            is_aligned: false,
        });
    }

    // Place the sections (untimed: the offset cascade's own cost is tracked
    // separately), then re-measure from scratch.
    project.redrive_offsets_after_load();
    for closure in &mut project.loop_closures {
        closure.is_aligned = false;
    }

    let started = std::time::Instant::now();
    project.remeasure_closure_alignment();
    let elapsed = started.elapsed();

    assert!(project.loop_closures.iter().all(|c| c.is_aligned));
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "took {elapsed:?}"
    );
}

fn test_closure(
    id: u64,
    station_a: StationKey,
    station_b: StationKey,
    strategy: ClosureStrategy,
    is_aligned: bool,
) -> LoopClosure {
    LoopClosure {
        id,
        station_a,
        station_b,
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy,
        is_aligned,
    }
}

/// A snap-to-merge closure with the same endpoints as every merge: the shape
/// #602 measured the load-time offset cascade on.
fn double_every_merge_with_move_closures(project: &mut Project, is_aligned: bool) {
    let doubled: Vec<(StationKey, StationKey)> = project
        .station_merges
        .iter()
        .map(|m| (m.station_a.clone(), m.station_b.clone()))
        .collect();
    for (a, b) in doubled {
        let id = project.next_closure_id;
        project.next_closure_id += 1;
        project
            .loop_closures
            .push(test_closure(id, a, b, ClosureStrategy::Move, is_aligned));
    }
}

#[test]
fn resolving_offsets_is_fast_with_thousands_of_closures_doubling_merges() {
    // Two 8000-section merge chains with a Move closure doubling every merge.
    // Seeding the solve walked each component rescanning every merge and closure
    // per visited section, O(sections x edges) (#602).
    let mut project = two_anchored_components_with_orphan_closures(8_000, 0);
    double_every_merge_with_move_closures(&mut project, false);
    project.redrive_offsets_after_load();
    assert!(project.loop_closures.iter().all(|c| c.is_aligned));

    // Debug build: about 0.65 s of CPU, against about 5.2 s before #602.
    let started = std::time::Instant::now();
    project.resolve_offsets();
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "took {elapsed:?}"
    );
}

/// `resolve_offsets` exactly as it was before #602, kept as the equivalence
/// oracle: one `get_connected_section_ids` walk per component to pick seeds.
fn resolve_offsets_before_602(project: &mut Project) -> usize {
    let before = project.check_merge_alignment().len();
    let offsets_before = project.section_offsets.clone();
    let seeds = realign_seeds_before_602(project);
    let was_deferred = project.cascade_deferred;
    project.cascade_deferred = false;
    project.cascade_connection_offsets(&seeds);
    project.cascade_deferred = was_deferred;
    if project.section_offsets != offsets_before {
        project.touch();
    }
    before.saturating_sub(project.check_merge_alignment().len())
}

/// The pre-#602 seed choice, verbatim: one walk per unvisited section.
fn realign_seeds_before_602(project: &Project) -> Vec<SectionId> {
    let datum_sections: HashSet<SectionId> = project
        .zero_datums
        .iter()
        .map(|z| z.station_ref.section_id)
        .collect();
    let mut seeds = Vec::new();
    let mut visited: HashSet<SectionId> = HashSet::new();
    for section in &project.sections {
        if visited.contains(&section.id) {
            continue;
        }
        let component = project.get_connected_section_ids(section.id);
        let anchor = component
            .iter()
            .copied()
            .filter(|s| datum_sections.contains(s))
            .min()
            .or_else(|| component.iter().copied().min());
        if let Some(a) = anchor {
            seeds.push(a);
        }
        visited.extend(component);
    }
    seeds
}

/// Run the oracle and the live solve on two copies of `project` and require
/// bit-identical offsets, return values and `modified_at` behaviour.
fn assert_resolve_matches_oracle(project: &Project, label: &str) {
    // The seeds directly, not only the offsets the cascade derives from them: a
    // seed bug the cascade happens to converge past would otherwise hide.
    assert_eq!(
        project.realign_seeds(),
        realign_seeds_before_602(project),
        "{label}: seeds"
    );
    let stamp = DateTime::<Utc>::from_timestamp(1_000_000, 0).unwrap();
    let mut old = project.clone();
    let mut new = project.clone();
    old.modified_at = stamp;
    new.modified_at = stamp;
    let old_healed = resolve_offsets_before_602(&mut old);
    let new_healed = new.resolve_offsets();
    let bits = |p: &Project| {
        let mut v: Vec<(SectionId, u64, u64)> = p
            .section_offsets
            .iter()
            .map(|(&id, &(x, y))| (id, x.to_bits(), y.to_bits()))
            .collect();
        v.sort_unstable();
        v
    };
    assert_eq!(new_healed, old_healed, "{label}: healed count");
    assert_eq!(bits(&new), bits(&old), "{label}: section offsets");
    assert_eq!(
        new.modified_at == stamp,
        old.modified_at == stamp,
        "{label}: touch"
    );
    assert_eq!(
        new.cascade_deferred, old.cascade_deferred,
        "{label}: deferral"
    );
}

/// `n` one-shot sections on `azimuth`, each merged end-to-start onto the next.
fn merge_chain(project: &mut Project, n: usize, azimuth: f64) -> Vec<SectionId> {
    let ids: Vec<SectionId> = (0..n)
        .map(|_| {
            project
                .add_section(SectionData {
                    shots: vec![straight_shot(azimuth, 3.0)],
                    ..Default::default()
                })
                .id
        })
        .collect();
    for pair in ids.windows(2) {
        let end = end_key(project, pair[0]);
        project.station_merges.push(StationMerge {
            station_a: end,
            station_b: StationKey::new(pair[1], StationAnchor::SectionStart),
            merged_at: Utc::now(),
        });
    }
    ids
}

fn end_key(project: &Project, id: SectionId) -> StationKey {
    let section = project.get_section(id).unwrap();
    let uuid = section.shots.last().unwrap().uuid.clone().unwrap();
    StationKey::new(id, StationAnchor::AfterShot(uuid))
}

fn start_key(id: SectionId) -> StationKey {
    StationKey::new(id, StationAnchor::SectionStart)
}

fn datum_at(key: StationKey, lat: f64) -> ZeroDatum {
    ZeroDatum {
        station_ref: key,
        gps: GpsCoordinates::new(lat, -87.4, 0.0).unwrap(),
    }
}

#[test]
fn resolve_offsets_matches_the_pre_602_solve_on_named_shapes() {
    // Anchored merge chain, no closures, persisted (stale) offsets.
    let mut p = Project::new("chain".to_string(), None);
    let c = merge_chain(&mut p, 5, 30.0);
    p.zero_datums.push(datum_at(start_key(c[2]), 20.5));
    p.section_offsets.insert(c[0], (12.5, -3.0));
    p.section_offsets.insert(c[4], (0.0, 0.0));
    assert_resolve_matches_oracle(&p, "anchored chain");

    // The same chain unanchored, with every merge doubled by a Move closure.
    let mut p = Project::new("unanchored doubled".to_string(), None);
    let c = merge_chain(&mut p, 6, 75.0);
    p.section_offsets.insert(c[3], (4.0, 4.0));
    double_every_merge_with_move_closures(&mut p, true);
    assert_resolve_matches_oracle(&p, "unanchored doubled");

    // The #602 shape, before and after the closures are measured aligned.
    let mut p = two_anchored_components_with_orphan_closures(30, 0);
    double_every_merge_with_move_closures(&mut p, false);
    p.section_offsets.insert(p.sections[7].id, (-9.0, 2.0));
    assert_resolve_matches_oracle(&p, "602 shape, unaligned");
    for closure in &mut p.loop_closures {
        closure.is_aligned = true;
    }
    assert_resolve_matches_oracle(&p, "602 shape, aligned");

    // Two unanchored merge groups bridged only by a misaligned Move closure
    // still flagged aligned, plus one flagged unaligned; a ghost section id
    // below every live one sits in the bridged component.
    let mut p = Project::new("bridged".to_string(), None);
    p.next_section_id = 5;
    let a = merge_chain(&mut p, 3, 10.0);
    let b = merge_chain(&mut p, 3, 200.0);
    let d = merge_chain(&mut p, 2, 300.0);
    p.section_offsets.insert(b[1], (40.0, -7.0));
    p.section_offsets.insert(d[0], (-15.0, 1.0));
    let e = end_key(&p, a[2]);
    p.loop_closures.push(test_closure(
        1,
        e,
        start_key(b[0]),
        ClosureStrategy::Move,
        true,
    ));
    let e = end_key(&p, b[2]);
    p.loop_closures.push(test_closure(
        2,
        e,
        start_key(d[0]),
        ClosureStrategy::Move,
        false,
    ));
    p.station_merges.push(StationMerge {
        station_a: start_key(b[2]),
        station_b: start_key(1),
        merged_at: Utc::now(),
    });
    assert_resolve_matches_oracle(&p, "bridged + ghost");
    p.zero_datums.push(datum_at(start_key(b[1]), 20.5));
    assert_resolve_matches_oracle(&p, "bridged + ghost, datum on b");
    p.zero_datums.push(datum_at(start_key(a[0]), 20.6));
    assert_resolve_matches_oracle(&p, "bridged, two datums");
    p.cascade_deferred = true;
    assert_resolve_matches_oracle(&p, "bridged, deferred");
}

/// A deterministic generator for the randomized equivalence sweep.
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

fn random_project(seed: u64) -> Project {
    let mut rng = Lcg(seed);
    let mut p = Project::new(format!("random {seed}"), None);
    p.next_section_id = 3;
    let n = 2 + rng.below(11) as usize;
    let ids: Vec<SectionId> = (0..n)
        .map(|_| {
            let shots = (0..=rng.below(3))
                .map(|_| {
                    let az = rng.real(0.0, 360.0);
                    straight_shot(az, rng.real(0.5, 20.0))
                })
                .collect();
            p.add_section(SectionData {
                shots,
                ..Default::default()
            })
            .id
        })
        .collect();
    let key = |rng: &mut Lcg, p: &Project| -> StationKey {
        if rng.below(10) == 0 {
            return start_key(rng.below(3) as SectionId);
        }
        let id = ids[rng.below(ids.len() as u64) as usize];
        let section = p.get_section(id).unwrap();
        let pick = rng.below(section.shots.len() as u64 + 1) as usize;
        match pick {
            0 => start_key(id),
            k => StationKey::new(
                id,
                StationAnchor::AfterShot(section.shots[k - 1].uuid.clone().unwrap()),
            ),
        }
    };
    for _ in 0..rng.below(n as u64 + 3) {
        let (a, b) = (key(&mut rng, &p), key(&mut rng, &p));
        p.station_merges.push(StationMerge {
            station_a: a,
            station_b: b,
            merged_at: Utc::now(),
        });
    }
    for id in 1..=rng.below(n as u64 + 1) {
        let (a, b) = (key(&mut rng, &p), key(&mut rng, &p));
        let strategy = if rng.below(3) == 0 {
            ClosureStrategy::Batch
        } else {
            ClosureStrategy::Move
        };
        let aligned = rng.below(2) == 0;
        p.loop_closures
            .push(test_closure(id, a, b, strategy, aligned));
    }
    for _ in 0..rng.below(3) {
        let k = key(&mut rng, &p);
        let lat = rng.real(20.0, 21.0);
        p.zero_datums.push(datum_at(k, lat));
    }
    for &id in &ids {
        if rng.below(3) == 0 {
            let v = (rng.real(-50.0, 50.0), rng.real(-50.0, 50.0));
            p.section_offsets.insert(id, v);
        }
    }
    p.cascade_deferred = rng.below(10) == 0;
    p
}

#[test]
fn resolve_offsets_matches_the_pre_602_solve_on_random_projects() {
    for seed in 0..400 {
        assert_resolve_matches_oracle(&random_project(seed), &format!("seed {seed}"));
    }
}

/// `convert_closures_orphaned_since` as it was before #597, kept as the
/// equivalence oracle for the one-pass sweep. Ported from
/// `1178568c8^:src-tauri/crates/models/src/project/offsets.rs` (the parent of
/// 1178568c8, "perf(models #597): orphan-closure sweep in one pass over section
/// components"): restart the scan from the first closure after every
/// conversion, answer orphanhood with a station walk per closure, and ask
/// `plan_merge_promotion` about every candidate. Adapted only to run outside
/// `impl Project` (`self` is `project`); it also hands back the refused ids so
/// the sweep can count them.
fn convert_closures_orphaned_since_before_597(
    project: &mut Project,
    preexisting: &HashSet<u64>,
) -> (Vec<offsets::MintedMerge>, HashSet<u64>) {
    let mut minted = Vec::new();
    // Sole-bridge closures left UNCONVERTED because turning them into a rigid
    // merge would fuse two independently zero-datum-anchored groups. Tracked by
    // id so the orphan scan below does not reconsider them on every iteration
    // (which would loop forever, since they are never removed).
    let mut left_as_closure: HashSet<u64> = HashSet::new();
    loop {
        let orphan = project.loop_closures.iter().position(|c| {
            if left_as_closure.contains(&c.id) || preexisting.contains(&c.id) {
                return false;
            }
            is_orphan_closure_before_597(project, c)
        });
        let Some(idx) = orphan else { break };

        let (id, a, b) = {
            let closure = &project.loop_closures[idx];
            (
                closure.id,
                closure.station_a.clone(),
                closure.station_b.clone(),
            )
        };
        let Some(plan) = project.plan_merge_promotion(&a, &b) else {
            left_as_closure.insert(id);
            continue;
        };

        let closure = project.loop_closures.remove(idx);
        let already = match (
            project.resolve_key(&closure.station_a),
            project.resolve_key(&closure.station_b),
        ) {
            (Some(a), Some(b)) => project.has_direct_merge(a, b),
            _ => false,
        };
        if !already {
            project.record_station_merge(StationMerge {
                station_a: closure.station_a,
                station_b: closure.station_b,
                merged_at: Utc::now(),
            });
        }
        minted.push(plan);
    }
    (minted, left_as_closure)
}

/// The pre-#597 orphan predicate (same source as above): a station walk over
/// shots and merges, closures excluded.
fn is_orphan_closure_before_597(project: &Project, closure: &LoopClosure) -> bool {
    if closure.station_a.section_id == closure.station_b.section_id {
        return false;
    }
    let (Some(a), Some(b)) = (
        project.resolve_key(&closure.station_a),
        project.resolve_key(&closure.station_b),
    ) else {
        return false;
    };
    !project.are_stations_connected_excluding_shot(a, b, None, false)
}

/// What a sweep leaves behind, in order: the closures (every field), the merge
/// endpoints (not `merged_at`, a wall-clock stamp) and the offsets' bits.
type SweepOutcome = (
    String,
    Vec<(StationKey, StationKey)>,
    Vec<(SectionId, u64, u64)>,
);

fn sweep_outcome(project: &Project) -> SweepOutcome {
    let mut offsets: Vec<(SectionId, u64, u64)> = project
        .section_offsets
        .iter()
        .map(|(&id, &(x, y))| (id, x.to_bits(), y.to_bits()))
        .collect();
    offsets.sort_unstable();
    (
        format!("{:?}", project.loop_closures),
        project
            .station_merges
            .iter()
            .map(|m| (m.station_a.clone(), m.station_b.clone()))
            .collect(),
        offsets,
    )
}

/// Sweep two copies of `project`, one with the oracle and one live, and require
/// the same promotions with the same plans in the same order, then the same
/// closures, merges and settled offsets. Returns the oracle's (conversions,
/// refusals) so the caller can prove the run was not vacuous.
fn assert_sweep_matches_oracle(
    project: &Project,
    preexisting: &HashSet<u64>,
    label: &str,
) -> (usize, usize) {
    let mut old = project.clone();
    let mut new = project.clone();
    let (old_minted, refused) = convert_closures_orphaned_since_before_597(&mut old, preexisting);
    let new_minted = new.convert_closures_orphaned_since(preexisting);
    assert_eq!(
        format!("{new_minted:?}"),
        format!("{old_minted:?}"),
        "{label}: promotions"
    );
    old.settle_minted_merges(&old_minted);
    new.settle_minted_merges(&new_minted);
    let (old_closures, old_merges, old_offsets) = sweep_outcome(&old);
    let (new_closures, new_merges, new_offsets) = sweep_outcome(&new);
    assert_eq!(new_closures, old_closures, "{label}: closures");
    assert_eq!(new_merges, old_merges, "{label}: merges");
    assert_eq!(new_offsets, old_offsets, "{label}: settled offsets");
    (old_minted.len(), refused.len())
}

/// [`random_project`] with more of what the sweep decides on: extra closures
/// across sections, and extra zero datums so both kinds of orphan (promotable
/// and double-anchored) turn up.
fn random_orphan_project(seed: u64) -> Project {
    let mut p = random_project(seed);
    let mut rng = Lcg(seed ^ 0x5eed_0597);
    let ids: Vec<SectionId> = p.sections.iter().map(|s| s.id).collect();
    let station = |rng: &mut Lcg, p: &Project| -> StationKey {
        let id = ids[rng.below(ids.len() as u64) as usize];
        let section = p.get_section(id).unwrap();
        match rng.below(section.shots.len() as u64 + 1) as usize {
            0 => start_key(id),
            k => StationKey::new(
                id,
                StationAnchor::AfterShot(section.shots[k - 1].uuid.clone().unwrap()),
            ),
        }
    };
    for _ in 0..rng.below(3) {
        let k = station(&mut rng, &p);
        let lat = rng.real(20.0, 21.0);
        p.zero_datums.push(datum_at(k, lat));
    }
    let first = p.loop_closures.iter().map(|c| c.id).max().unwrap_or(0) + 1;
    for id in first..first + rng.below(6) {
        let (a, b) = (station(&mut rng, &p), station(&mut rng, &p));
        p.loop_closures
            .push(test_closure(id, a, b, ClosureStrategy::Batch, false));
    }
    p
}

#[test]
fn the_orphan_sweep_matches_the_pre_597_sweep_on_random_projects() {
    let (mut conversions, mut refusals) = (0, 0);
    let (mut seeds_converting, mut seeds_refusing) = (0, 0);
    for seed in 0..600 {
        let project = random_orphan_project(seed);
        let (c, r) =
            assert_sweep_matches_oracle(&project, &HashSet::new(), &format!("seed {seed}"));
        // A mutation path's sweep, which leaves the orphans it saw beforehand.
        let mut rng = Lcg(seed);
        let preexisting: HashSet<u64> = project
            .loop_closures
            .iter()
            .map(|c| c.id)
            .filter(|_| rng.below(3) == 0)
            .collect();
        let (pc, pr) =
            assert_sweep_matches_oracle(&project, &preexisting, &format!("seed {seed} scoped"));
        conversions += c + pc;
        refusals += r + pr;
        seeds_converting += usize::from(c > 0);
        seeds_refusing += usize::from(r > 0);
    }
    println!(
        "orphan sweep coverage: {conversions} conversions, {refusals} refused promotions, \
         {seeds_converting} seeds converting, {seeds_refusing} seeds refusing"
    );
    assert!(
        conversions >= 300 && seeds_converting >= 150,
        "too few conversions to mean anything"
    );
    assert!(
        refusals >= 100 && seeds_refusing >= 50,
        "too few refused promotions to mean anything"
    );
}
