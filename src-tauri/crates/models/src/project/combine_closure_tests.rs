//! Combine Sections across loops (`combine_plan.rs`): two ends tied by a loop
//! closure join with a shift, the ties in the way become loop closures, and
//! the side a zero datum holds stays put. Then a randomised sweep over small
//! cave networks with merged branches, snapped and recorded loops and zero
//! datums, checking every combine the station menu offers against the
//! invariants the app keeps.

use std::collections::{HashMap, HashSet};

use crate::{
    ClosureStrategy, CombineCandidate, CombineJoinEnds, CombineJoinKind, CombinePick,
    CombineSectionsOptions, GpsCoordinates, Project, SectionData, SectionEnd, SectionId, ShotData,
    StationRef, TornTieKind,
};

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..Default::default()
    }
}

fn add(project: &mut Project, name: &str, shots: Vec<ShotData>) -> SectionId {
    project
        .add_section(SectionData {
            name: Some(name.to_string()),
            color: "#123456".to_string(),
            shots,
            ..Default::default()
        })
        .id
}

fn options(join: Option<CombineJoinEnds>) -> CombineSectionsOptions {
    CombineSectionsOptions {
        name: "Joined".to_string(),
        color: "#abcdef".to_string(),
        swap_lr: true,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Kept,
        join,
    }
}

fn xy(project: &Project, at: StationRef) -> (f64, f64) {
    let c = project.compute_station_raw_coords(at).unwrap();
    (c.x, c.y)
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// Move `id` so its station `index` sits exactly on `target`.
fn place(project: &mut Project, id: SectionId, index: u32, target: (f64, f64)) {
    let now = xy(project, StationRef::new(id, index));
    project.adjust_section_offset(id, target.0 - now.0, target.1 - now.1);
}

fn stations(project: &Project, id: SectionId) -> Vec<(f64, f64)> {
    let n = project.get_section(id).unwrap().shots.len() as u32;
    (0..=n)
        .map(|i| xy(project, StationRef::new(id, i)))
        .collect()
}

fn gps() -> GpsCoordinates {
    GpsCoordinates {
        latitude: 20.0,
        longitude: -87.0,
        depth_reference: 0.0,
    }
}

fn resolved(project: &Project, a: &crate::StationKey, b: &crate::StationKey) -> [StationRef; 2] {
    let mut ends = [
        project.resolve_key(a).unwrap(),
        project.resolve_key(b).unwrap(),
    ];
    ends.sort_by_key(|r| (r.section_id, r.station_index));
    ends
}

/// K runs 10 m east; O runs 9.6 m west and its END is merged to K's start,
/// so O's start sits 0.4 m short of K's end, where a loop closure ties them.
fn two_section_loop() -> (Project, SectionId, SectionId) {
    let mut project = Project::new("Loop".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0)]);
    let o = add(&mut project, "O", vec![shot(270.0, 9.6)]);
    project.set_section_offset(k, (100.0, -40.0));
    project
        .merge_stations(StationRef::new(k, 0), StationRef::new(o, 1))
        .unwrap();
    project
        .create_loop_closure(
            StationRef::new(k, 1),
            StationRef::new(o, 0),
            ClosureStrategy::Batch,
        )
        .unwrap();
    (project, k, o)
}

/// The station menu on K's end offers O across the closure. Combining there
/// moves O 0.4 m east onto K's end, the merge at K's start becomes the loop
/// closure, and K does not move.
#[test]
fn two_sections_in_a_loop_combine_at_the_closure() {
    let (mut project, k, o) = two_section_loop();
    let at_end = project.combine_candidates(StationRef::new(k, 1));
    assert_eq!(at_end.len(), 1);
    let candidate = &at_end[0];
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(candidate.join_kind, CombineJoinKind::Closure);
    assert!(!candidate.merged);
    let plan = &candidate.plan_keeping_a;
    assert_eq!(plan.moving_section, Some(o));
    assert_eq!(plan.moved_sections, vec![o]);
    assert!((plan.shift_m - 0.4).abs() < 1e-9, "{}", plan.shift_m);
    assert_eq!(plan.torn_ties.len(), 1);
    assert_eq!(plan.torn_ties[0].kind, TornTieKind::Merge);

    // The two-section query prefers the merge at K's start instead.
    let pair = project.combine_candidate_for_sections(k, o).unwrap();
    assert_eq!(pair.join_kind, CombineJoinKind::Merged);
    assert!(pair.plan_keeping_a.torn_ties.is_empty());

    let k_before = stations(&project, k);
    let o_before = stations(&project, o);
    project
        .combine_sections(
            k,
            o,
            &options(Some(CombineJoinEnds {
                kept: SectionEnd::End,
                other: SectionEnd::Start,
            })),
        )
        .unwrap();
    let after = stations(&project, k);
    assert_eq!(after.len(), 3);
    assert!(dist(after[0], k_before[0]) < 1e-9 && dist(after[1], k_before[1]) < 1e-9);
    let moved = (o_before[1].0 + 0.4, o_before[1].1);
    assert!(dist(after[2], moved) < 1e-9, "{:?} vs {moved:?}", after[2]);
    assert!(project.station_merges.is_empty());
    assert_eq!(project.loop_closures.len(), 1);
    let closure = &project.loop_closures[0];
    assert_eq!(
        resolved(&project, &closure.station_a, &closure.station_b),
        [StationRef::new(k, 0), StationRef::new(k, 2)]
    );
    assert_eq!(closure.strategy, ClosureStrategy::Batch);
    assert!((closure.misclosure_distance - 0.4).abs() < 1e-9);
    assert!(
        (closure.path_length - 19.6).abs() < 1e-9,
        "{}",
        closure.path_length
    );
    assert!(project.check_structural_integrity().is_empty());
}

/// K east, X north from K's end, O from X's end back toward K's start,
/// falling 0.5 m short, with a closure there. Combining K and O at that
/// closure tears the merge between O and X (the one nearest O), moves only
/// O, and leaves K and X where they were.
#[test]
fn a_three_section_loop_tears_the_merge_nearest_the_moving_section() {
    let mut project = Project::new("Three".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0)]);
    let x = add(&mut project, "X", vec![shot(0.0, 10.0)]);
    let o = add(&mut project, "O", vec![shot(225.0, 200.0_f64.sqrt() - 0.5)]);
    let k_end = xy(&project, StationRef::new(k, 1));
    place(&mut project, x, 0, k_end);
    project
        .merge_stations(StationRef::new(k, 1), StationRef::new(x, 0))
        .unwrap();
    let x_end = xy(&project, StationRef::new(x, 1));
    place(&mut project, o, 0, x_end);
    project
        .merge_stations(StationRef::new(x, 1), StationRef::new(o, 0))
        .unwrap();
    project
        .create_loop_closure(
            StationRef::new(o, 1),
            StationRef::new(k, 0),
            ClosureStrategy::Batch,
        )
        .unwrap();

    let candidate = project.combine_candidate(StationRef::new(k, 0)).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(candidate.join_kind, CombineJoinKind::Closure);
    let plan = &candidate.plan_keeping_a;
    assert_eq!(plan.moved_sections, vec![o]);
    assert!((plan.shift_m - 0.5).abs() < 1e-9);
    let torn: Vec<_> = plan
        .torn_ties
        .iter()
        .map(|t| (t.station_a, t.station_b))
        .collect();
    assert_eq!(torn, vec![(StationRef::new(x, 1), StationRef::new(o, 0))]);

    let (k_before, x_before) = (stations(&project, k), stations(&project, x));
    project
        .combine_sections(
            k,
            o,
            &options(Some(CombineJoinEnds {
                kept: SectionEnd::Start,
                other: SectionEnd::End,
            })),
        )
        .unwrap();
    // O was prepended: K's stations are now 1 and 2.
    assert_eq!(stations(&project, k)[1..], k_before[..]);
    assert_eq!(stations(&project, x), x_before);
    assert_eq!(project.station_merges.len(), 1, "K-X stays");
    let closure = &project.loop_closures[0];
    assert_eq!(
        resolved(&project, &closure.station_a, &closure.station_b),
        [StationRef::new(k, 0), StationRef::new(x, 1)]
    );
    assert!((closure.misclosure_distance - 0.5).abs() < 1e-9);
    assert!(project.check_structural_integrity().is_empty());
}

/// A zero datum on the other section holds it: the kept section's side
/// moves instead, and the datum station stays exactly where it was.
#[test]
fn a_zero_datum_on_the_other_section_moves_the_kept_side() {
    let (mut project, k, o) = two_section_loop();
    project
        .set_zero_datum(StationRef::new(o, 0), gps())
        .unwrap();
    let datum_at = xy(&project, StationRef::new(o, 0));
    let candidate = project.combine_candidates(StationRef::new(k, 1)).remove(0);
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(candidate.plan_keeping_a.moving_section, Some(k));
    assert_eq!(candidate.plan_keeping_b.moving_section, Some(k));

    project
        .combine_sections(
            k,
            o,
            &options(Some(CombineJoinEnds {
                kept: SectionEnd::End,
                other: SectionEnd::Start,
            })),
        )
        .unwrap();
    let datum = &project.zero_datums[0];
    let now = project.resolve_key(&datum.station_ref).unwrap();
    assert_eq!(
        now,
        StationRef::new(k, 1),
        "the datum is on the joined station"
    );
    assert!(dist(xy(&project, now), datum_at) < 1e-9);
}

/// Two anchored groups, each merged to one of the two sections, tied only
/// by a closure between the sections' ends: the other section tears off
/// its datum side and moves alone; the torn merge stays a closure (it ties
/// two anchored groups), and a reload changes nothing.
#[test]
fn both_sides_held_tear_the_merge_to_the_moving_sides_datum() {
    let mut project = Project::new("Anchored".to_string(), None);
    let a = add(&mut project, "A", vec![shot(0.0, 5.0)]);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0)]);
    let a_end = xy(&project, StationRef::new(a, 1));
    place(&mut project, k, 0, a_end);
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(k, 0))
        .unwrap();
    project
        .set_zero_datum(StationRef::new(a, 0), gps())
        .unwrap();
    let b = add(&mut project, "B", vec![shot(180.0, 5.0)]);
    let o = add(&mut project, "O", vec![shot(90.0, 8.0)]);
    let k_end = xy(&project, StationRef::new(k, 1));
    place(&mut project, o, 0, (k_end.0 + 0.3, k_end.1 + 0.2));
    let o_end = xy(&project, StationRef::new(o, 1));
    place(&mut project, b, 0, o_end);
    project
        .merge_stations(StationRef::new(o, 1), StationRef::new(b, 0))
        .unwrap();
    project.zero_datums.push(crate::ZeroDatum {
        station_ref: project.key_for_ref(StationRef::new(b, 1)).unwrap(),
        gps: gps(),
    });
    project
        .create_loop_closure(
            StationRef::new(k, 1),
            StationRef::new(o, 0),
            ClosureStrategy::Batch,
        )
        .unwrap();

    let candidate = project.combine_candidates(StationRef::new(k, 1)).remove(0);
    assert_eq!(candidate.blocked_reason, None);
    let plan = &candidate.plan_keeping_a;
    assert_eq!(plan.moved_sections, vec![o]);
    assert_eq!(plan.torn_ties.len(), 1);
    let (a_before, b_before) = (stations(&project, a), stations(&project, b));
    project
        .combine_sections(
            k,
            o,
            &options(Some(CombineJoinEnds {
                kept: SectionEnd::End,
                other: SectionEnd::Start,
            })),
        )
        .unwrap();
    assert_eq!(stations(&project, a), a_before);
    assert_eq!(stations(&project, b), b_before);
    let mut reloaded = project.clone();
    reloaded.redrive_offsets_after_load();
    assert_eq!(reloaded.loop_closures.len(), project.loop_closures.len());
    for (a, b) in stations(&reloaded, k)
        .into_iter()
        .zip(stations(&project, k))
    {
        assert!(dist(a, b) < 1e-9);
    }
}

/// A snapped closure between the two ends (aligned, a few mm apart) is a
/// closure join: it goes, and the other section moves the few mm.
#[test]
fn a_snapped_closure_at_the_join_joins_and_goes() {
    let mut project = Project::new("Snapped".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0)]);
    // O loops back to K's start, 4 mm longer than K: its start lands 4 mm
    // from K's end, and a Move closure snaps the two.
    let o = add(
        &mut project,
        "O",
        vec![shot(0.0, 6.0), shot(270.0, 10.004), shot(180.0, 6.0)],
    );
    project
        .merge_stations(StationRef::new(k, 0), StationRef::new(o, 3))
        .unwrap();
    let gap = dist(
        xy(&project, StationRef::new(k, 1)),
        xy(&project, StationRef::new(o, 0)),
    );
    assert!((gap - 0.004).abs() < 1e-9, "{gap}");
    let closure = project
        .create_loop_closure(
            StationRef::new(k, 1),
            StationRef::new(o, 0),
            ClosureStrategy::Move,
        )
        .unwrap();
    assert!(closure.is_aligned, "within the 1 cm merge invariant");

    let candidate = project.combine_candidates(StationRef::new(k, 1)).remove(0);
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(candidate.join_kind, CombineJoinKind::Closure);
    project
        .combine_sections(
            k,
            o,
            &options(Some(CombineJoinEnds {
                kept: SectionEnd::End,
                other: SectionEnd::Start,
            })),
        )
        .unwrap();
    assert!(project.station_merges.is_empty());
    assert_eq!(
        project.loop_closures.len(),
        1,
        "the merge became the loop's closure"
    );
    assert!(!project.loop_closures[0].is_aligned);
}

// ---- The sweep ---------------------------------------------------------------

/// xorshift64*: deterministic, no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn random_shots(rng: &mut Rng) -> Vec<ShotData> {
    (0..=rng.below(3))
        .map(|_| shot(rng.range(0.0, 360.0), rng.range(2.0, 15.0)))
        .collect()
}

fn every_station(project: &Project) -> Vec<StationRef> {
    project
        .sections
        .iter()
        .flat_map(|s| (0..=s.shots.len() as u32).map(move |i| StationRef::new(s.id, i)))
        .collect()
}

/// A small network: a tree of merged sections, one or two loops back onto
/// it (snapped exactly, or recorded with a misclosure), sometimes a zero
/// datum, and sometimes a second anchored network tied on by a closure.
fn network(seed: u64) -> Project {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut project = Project::new(format!("Net {seed}"), None);
    let first = add(&mut project, "S0", random_shots(&mut rng));
    project.set_section_offset(first, (rng.range(-50.0, 50.0), rng.range(-50.0, 50.0)));
    for i in 1..3 + rng.below(4) {
        let all = every_station(&project);
        let host = all[rng.below(all.len())];
        let s = add(&mut project, &format!("S{i}"), random_shots(&mut rng));
        let index = if rng.below(2) == 0 {
            0
        } else {
            project.get_section(s).unwrap().shots.len() as u32
        };
        let at = xy(&project, host);
        place(&mut project, s, index, at);
        let _ = project.merge_stations(host, StationRef::new(s, index));
    }
    for j in 0..=rng.below(2) {
        let all = every_station(&project);
        let (p, q) = (all[rng.below(all.len())], all[rng.below(all.len())]);
        let (pp, qq) = (xy(&project, p), xy(&project, q));
        let length = dist(pp, qq);
        if length < 1.0 || p.section_id == q.section_id {
            continue;
        }
        let azimuth = (qq.0 - pp.0)
            .atan2(qq.1 - pp.1)
            .to_degrees()
            .rem_euclid(360.0);
        let snapped = rng.below(2) == 0;
        let length = if snapped {
            length
        } else {
            length + rng.range(0.2, 1.5)
        };
        let l = add(&mut project, &format!("L{j}"), vec![shot(azimuth, length)]);
        place(&mut project, l, 0, pp);
        if project.merge_stations(p, StationRef::new(l, 0)).is_err() {
            continue;
        }
        let strategy = if snapped {
            ClosureStrategy::Move
        } else {
            ClosureStrategy::Batch
        };
        let _ = project.create_loop_closure(StationRef::new(l, 1), q, strategy);
    }
    if rng.below(3) == 0 {
        let all = every_station(&project);
        let _ = project.set_zero_datum(all[rng.below(all.len())], gps());
    }
    if rng.below(3) == 0 {
        // A second network, anchored on its own, tied on by a closure between
        // a section end of each.
        let t = add(&mut project, "T", random_shots(&mut rng));
        project.set_section_offset(t, (rng.range(-50.0, 50.0), rng.range(-50.0, 50.0)));
        project.zero_datums.push(crate::ZeroDatum {
            station_ref: project.key_for_ref(StationRef::new(t, 0)).unwrap(),
            gps: gps(),
        });
        let ends: Vec<StationRef> = project
            .sections
            .iter()
            .filter(|s| s.id != t)
            .flat_map(|s| {
                [
                    StationRef::new(s.id, 0),
                    StationRef::new(s.id, s.shots.len() as u32),
                ]
            })
            .collect();
        let main = ends[rng.below(ends.len())];
        if !project.section_has_zero_datum(main.section_id) {
            let _ = project.set_zero_datum(main, gps());
        }
        let t_end = StationRef::new(t, project.get_section(t).unwrap().shots.len() as u32);
        let _ = project.create_loop_closure(main, t_end, ClosureStrategy::Batch);
    }
    project.redrive_offsets_after_load();
    project
}

fn positions(project: &Project) -> HashMap<StationRef, (f64, f64)> {
    every_station(project)
        .into_iter()
        .map(|at| (at, xy(project, at)))
        .collect()
}

/// Union-find root of `x` in `forest`.
fn root(forest: &mut HashMap<SectionId, SectionId>, x: SectionId) -> SectionId {
    let parent = *forest.get(&x).unwrap_or(&x);
    if parent == x {
        x
    } else {
        let r = root(forest, parent);
        forest.insert(x, r);
        r
    }
}

/// The invariants after one combine (`what` names it in a failure).
fn check_combined(
    before: &Project,
    after: &Project,
    candidate: &CombineCandidate,
    kept: SectionId,
    what: &str,
) {
    let other = if kept == candidate.section_a {
        candidate.section_b
    } else {
        candidate.section_a
    };
    let plan = if kept == candidate.section_a {
        &candidate.plan_keeping_a
    } else {
        &candidate.plan_keeping_b
    };
    let kept_end = if kept == candidate.section_a {
        candidate.end_a
    } else {
        candidate.end_b
    };
    let old = positions(before);
    let moved: HashSet<SectionId> = plan.moved_sections.iter().copied().collect();
    // The shift, read off the moving section's own station 0.
    let shift = plan.moving_section.map_or((0.0, 0.0), |m| {
        if m == kept {
            let index = if kept_end == SectionEnd::End {
                0
            } else {
                before.get_section(other).unwrap().shots.len() as u32
            };
            let now = xy(after, StationRef::new(kept, index));
            let was = old[&StationRef::new(kept, 0)];
            (now.0 - was.0, now.1 - was.1)
        } else {
            let any = plan.moved_sections.iter().find(|s| **s != other);
            any.map_or_else(
                || {
                    // Only the other section moved: its join station lands on
                    // the kept one's.
                    let o_join = if candidate.section_a == other {
                        candidate.station_a
                    } else {
                        candidate.station_b
                    };
                    let k_join = if candidate.section_a == kept {
                        candidate.station_a
                    } else {
                        candidate.station_b
                    };
                    let (k, o) = (old[&k_join], old[&o_join]);
                    (k.0 - o.0, k.1 - o.1)
                },
                |s| {
                    let now = xy(after, StationRef::new(*s, 0));
                    let was = old[&StationRef::new(*s, 0)];
                    (now.0 - was.0, now.1 - was.1)
                },
            )
        }
    });
    assert!(
        (shift.0.hypot(shift.1) - plan.shift_m).abs() < 1e-6,
        "{what}: shift {shift:?} vs {}",
        plan.shift_m
    );
    // Third sections: moved by the shift, or not at all.
    for section in &after.sections {
        if section.id == kept {
            continue;
        }
        let d = if moved.contains(&section.id) {
            shift
        } else {
            (0.0, 0.0)
        };
        for i in 0..=section.shots.len() as u32 {
            let at = StationRef::new(section.id, i);
            let want = old[&at];
            assert!(
                dist(xy(after, at), (want.0 + d.0, want.1 + d.1)) < 1e-6,
                "{what}: {at:?} moved"
            );
        }
    }
    // The combined section: the kept stations shifted only if the kept side
    // moved, the other's onto the kept chain.
    let nk = before.get_section(kept).unwrap().shots.len() as u32;
    let no = before.get_section(other).unwrap().shots.len() as u32;
    let combined = stations(after, kept);
    assert_eq!(combined.len() as u32, nk + no + 1, "{what}");
    let k_shift = if moved.contains(&kept) {
        shift
    } else {
        (0.0, 0.0)
    };
    let offset = if kept_end == SectionEnd::End { 0 } else { no };
    for i in 0..=nk {
        let was = old[&StationRef::new(kept, i)];
        let now = combined[(i + offset) as usize];
        assert!(
            dist(now, (was.0 + k_shift.0, was.1 + k_shift.1)) < 1e-6,
            "{what}: kept station {i}"
        );
    }
    let o_shift = if moved.contains(&other) {
        shift
    } else {
        (0.0, 0.0)
    };
    let tolerance = if plan.moving_section.is_none() {
        1.1e-3
    } else {
        1e-6
    };
    for i in 0..=no {
        let was = old[&StationRef::new(other, i)];
        let want = (was.0 + o_shift.0, was.1 + o_shift.1);
        assert!(
            combined.iter().any(|now| dist(*now, want) < tolerance),
            "{what}: other station {i} lost"
        );
    }
    // No moved section carries a zero datum.
    for id in &plan.moved_sections {
        assert!(!before.carries_zero_datum(*id), "{what}: moved a datum");
    }

    // Structure.
    let violations = after.check_structural_integrity();
    assert!(violations.is_empty(), "{what}: {violations:?}");
    after.debug_assert_merge_alignment(what);
    let mut forest: HashMap<SectionId, SectionId> = HashMap::new();
    for merge in &after.station_merges {
        let (a, b) = (
            after.resolve_key(&merge.station_a).unwrap(),
            after.resolve_key(&merge.station_b).unwrap(),
        );
        assert_ne!(
            a.section_id, b.section_id,
            "{what}: a merge inside one section"
        );
        let (ra, rb) = (
            root(&mut forest, a.section_id),
            root(&mut forest, b.section_id),
        );
        assert_ne!(ra, rb, "{what}: merges close a loop");
        forest.insert(ra, rb);
    }
    for closure in &after.loop_closures {
        let a = after.resolve_key(&closure.station_a).unwrap();
        let b = after.resolve_key(&closure.station_b).unwrap();
        assert!(
            !after.are_stations_merged(a, b) && a != b,
            "{what}: a closure between merged stations"
        );
        if closure.is_aligned {
            assert_ne!(
                a.section_id, b.section_id,
                "{what}: a snapped closure inside the combined section"
            );
        }
    }
    // Opening the saved file changes nothing: no closure is promoted back.
    let mut reloaded = after.clone();
    reloaded.redrive_offsets_after_load();
    assert_eq!(
        reloaded.station_merges.len(),
        after.station_merges.len(),
        "{what}: a reload promoted a closure"
    );
    let now = positions(after);
    for (at, p) in positions(&reloaded) {
        assert!(dist(p, now[&at]) < 1e-6, "{what}: a reload moved {at:?}");
    }
}

/// Every combine the station menu offers in random networks, keeping either
/// section: refused only for two zero datums, and every invariant holds.
#[test]
fn every_offered_combine_in_random_networks_keeps_the_invariants() {
    let seeds: u64 = std::env::var("STALAGMIGHT_COMBINE_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(120);
    let mut combined = 0usize;
    let mut kinds: HashSet<String> = HashSet::new();
    let mut torn = 0usize;
    let (mut shifted, mut kept_moved, mut with_branches) = (0usize, 0usize, 0usize);
    for seed in 0..seeds {
        let project = network(seed);
        let ends: Vec<StationRef> = project
            .sections
            .iter()
            .flat_map(|s| {
                [
                    StationRef::new(s.id, 0),
                    StationRef::new(s.id, s.shots.len() as u32),
                ]
            })
            .collect();
        for end in ends {
            for candidate in project.combine_candidates(end) {
                if let Some(reason) = &candidate.blocked_reason {
                    assert!(
                        reason.contains("zero datum") || reason.contains("middle"),
                        "seed {seed} {end:?}: refused for {reason}"
                    );
                    continue;
                }
                kinds.insert(format!("{:?}", candidate.join_kind));
                for kept in [candidate.section_a, candidate.section_b] {
                    let (kept_end, other_end, other) = if kept == candidate.section_a {
                        (candidate.end_a, candidate.end_b, candidate.section_b)
                    } else {
                        (candidate.end_b, candidate.end_a, candidate.section_a)
                    };
                    let what = format!("seed {seed} {end:?} keep {kept} other {other}");
                    let mut after = project.clone();
                    after
                        .combine_sections(
                            kept,
                            other,
                            &options(Some(CombineJoinEnds {
                                kept: kept_end,
                                other: other_end,
                            })),
                        )
                        .unwrap_or_else(|e| panic!("{what}: {e}"));
                    let plan = if kept == candidate.section_a {
                        &candidate.plan_keeping_a
                    } else {
                        &candidate.plan_keeping_b
                    };
                    torn += plan.torn_ties.len();
                    shifted += usize::from(plan.shift_m > 0.1);
                    kept_moved += usize::from(plan.moving_section == Some(kept));
                    with_branches += usize::from(plan.moved_sections.len() > 1);
                    check_combined(&project, &after, &candidate, kept, &what);
                    combined += 1;
                }
            }
        }
    }
    // The sweep reaches what it is for (120 seeds: ~3500 combines, ~1400
    // torn ties, ~550 shifts, ~110 with the kept side moving).
    assert!(combined > seeds as usize, "only {combined} combines");
    assert!(torn > 0, "no tie was ever torn");
    assert!(shifted > 0, "no side ever moved");
    assert!(kept_moved > 0, "a zero datum never held the other side");
    assert!(
        with_branches > 0,
        "nothing ever moved with the moving section"
    );
    for kind in ["Merged", "CoLocated", "Closure"] {
        assert!(kinds.contains(kind), "no {kind} join in {kinds:?}");
    }
}
