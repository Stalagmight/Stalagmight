//! Combine Sections across loops (`combine_plan.rs`): two ends tied by a loop
//! closure join with a shift, the ties in the way become loop closures, and
//! the side a zero datum holds stays put. Then a randomised sweep over small
//! cave networks with merged branches, snapped and recorded loops and zero
//! datums, checking every combine the station menu offers against the
//! invariants the app keeps.

use std::collections::{HashMap, HashSet};

use crate::{
    ClosureStrategy, CombineCandidate, CombineJoinEnds, CombineJoinKind, CombinePick,
    CombineSectionsOptions, GpsCoordinates, KeptMovesReason, Project, SectionData, SectionEnd,
    SectionId, ShotData, StationRef, TornTieKind, TornTieReason,
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

/// Keeping the higher id across a closure join: the lower id holds the
/// component's gauge (the station the closure-adjusted view is held at), so
/// moving it would shift the whole cave in Plan and Export by the misclosure.
/// The kept side moves instead, and says why; every closure-adjusted station
/// stays where it was (reported after #726: the anchor moved 0.400 m).
#[test]
fn a_closure_join_moves_the_side_without_the_adjustment_anchor() {
    let (mut project, k, o) = two_section_loop();
    let anchor = project.adjustment_anchor_for_tests(k).unwrap();
    assert_eq!(anchor.section_id, k, "setup: the lower id holds the gauge");
    let candidate = project.combine_candidates(StationRef::new(k, 1)).remove(0);
    // Keeping K (the lower id): O moves, as before.
    assert_eq!(candidate.plan_keeping_a.moving_section, Some(o));
    assert_eq!(candidate.plan_keeping_a.kept_moves_because, None);
    // Keeping O: K holds the anchor, so O (the kept one) moves.
    assert_eq!(candidate.plan_keeping_b.moving_section, Some(o));
    assert_eq!(
        candidate.plan_keeping_b.kept_moves_because,
        Some(KeptMovesReason::AdjustmentAnchor)
    );
    let was = adjusted(&project);
    let anchor_at = was[&anchor];
    project
        .combine_sections(
            o,
            k,
            &options(Some(CombineJoinEnds {
                kept: SectionEnd::Start,
                other: SectionEnd::End,
            })),
        )
        .unwrap();
    // K's end joins O's start: K is prepended, so K's station i is O's i and
    // O's station i is O's i + 1.
    let now = adjusted(&project);
    let anchor_now = now[&StationRef::new(o, 0)];
    assert!(
        dist(anchor_at, anchor_now) < 1e-6,
        "the anchor moved {:.3} m",
        dist(anchor_at, anchor_now)
    );
    for (at, p) in &was {
        let home = if at.section_id == k {
            StationRef::new(o, at.station_index)
        } else {
            StationRef::new(o, at.station_index + 1)
        };
        assert!(
            dist(*p, now[&home]) < 1e-6,
            "{at:?} moved in the adjusted view"
        );
    }
}

/// A section that ends at the point with both ends: its end snapped there
/// (side by side), its start across a loop closure 1.4 m away. The station
/// menu offers the pair once, joined side by side (the way the two-section
/// query prefers), never the closure join that would move it 1.4 m.
#[test]
fn a_pair_meeting_twice_at_a_station_is_offered_once_by_its_best_join() {
    let mut project = Project::new("Twice".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0)]);
    let far = add(&mut project, "Far", vec![shot(0.0, 1.4)]);
    let k_end = xy(&project, StationRef::new(k, 1));
    place(&mut project, far, 1, k_end);
    // L runs from Far's start to K's end, which it reaches exactly.
    let l = add(&mut project, "L", vec![shot(0.0, 1.4)]);
    let far_start = xy(&project, StationRef::new(far, 0));
    place(&mut project, l, 0, far_start);
    project
        .merge_stations(StationRef::new(far, 0), StationRef::new(l, 0))
        .unwrap();
    // Far's end is merged to K's end, and a closure ties K's end to Far's
    // start (and so to L's start, merged there).
    project
        .merge_stations(StationRef::new(k, 1), StationRef::new(far, 1))
        .unwrap();
    project
        .create_loop_closure(
            StationRef::new(k, 1),
            StationRef::new(far, 0),
            ClosureStrategy::Batch,
        )
        .unwrap();
    let both_ends: Vec<_> = project
        .combine_candidates(StationRef::new(k, 1))
        .into_iter()
        .filter(|c| c.section_a == l || c.section_b == l)
        .map(|c| c.join_kind)
        .collect();
    assert_eq!(
        both_ends,
        vec![CombineJoinKind::CoLocated],
        "setup: L meets K's end once"
    );
    let at_point = project.combine_candidates_at_point(StationRef::new(k, 1));
    let pairs: Vec<(SectionId, SectionId, CombineJoinKind)> = at_point
        .iter()
        .map(|c| (c.section_a, c.section_b, c.join_kind))
        .collect();
    // K with Far (merged here), K with L (side by side here, not across the
    // closure), Far with L (side by side here). Not Far with L joined at their
    // starts: that is across the closure, 1.4 m away.
    assert_eq!(
        pairs,
        vec![
            (k, far, CombineJoinKind::Merged),
            (k, l, CombineJoinKind::CoLocated),
            (far, l, CombineJoinKind::CoLocated),
        ]
    );
    assert!(at_point
        .iter()
        .all(|c| c.plan_keeping_a.moving_section.is_none()));
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
    assert_eq!(
        plan.torn_ties[0].reason,
        TornTieReason::ZeroDatum,
        "it tears because B's datum holds B, not to open a loop"
    );
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

/// Every station's closure-adjusted position (the Plan and Export geometry).
fn adjusted(project: &Project) -> HashMap<StationRef, (f64, f64)> {
    project
        .compute_adjusted_station_coordinates(&crate::config::ClosureAdjustmentConfig::default())
        .into_iter()
        .map(|s| (s.station_ref, (s.coordinates.x, s.coordinates.y)))
        .collect()
}

/// `project` with every snapped (Move) closure counted as an ordinary one,
/// so the solve holds a snapped pair together as the survey does.
fn with_snaps(project: &Project) -> Project {
    let mut copy = project.clone();
    for closure in &mut copy.loop_closures {
        if closure.strategy == ClosureStrategy::Move {
            closure.strategy = ClosureStrategy::Batch;
        }
    }
    copy
}

/// Add an ordinary loop closure between `a` and `b`, so the solve holds them
/// on one point.
fn tie(project: &mut Project, a: StationRef, b: StationRef) {
    let (Some(station_a), Some(station_b)) = (project.key_for_ref(a), project.key_for_ref(b))
    else {
        return;
    };
    let id = project
        .loop_closures
        .iter()
        .map(|c| c.id)
        .max()
        .unwrap_or(0)
        + 1;
    project.loop_closures.push(crate::LoopClosure {
        id,
        station_a,
        station_b,
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });
}

/// [`adjusted`], and whether the solve held: a solved component puts both
/// ends of every closure it adjusts on one point; a failed one falls back to
/// the measured geometry, where they stay apart.
fn adjusted_checked(project: &Project) -> (HashMap<StationRef, (f64, f64)>, bool) {
    let view = adjusted(project);
    let ok = project
        .loop_closures
        .iter()
        .filter(|c| {
            matches!(
                c.strategy,
                ClosureStrategy::Batch | ClosureStrategy::LeastSquaresWeighted
            )
        })
        .all(|c| {
            match (
                project.resolve_key(&c.station_a),
                project.resolve_key(&c.station_b),
            ) {
                (Some(a), Some(b)) => match (view.get(&a), view.get(&b)) {
                    (Some(p), Some(q)) => dist(*p, *q) < 1e-6,
                    _ => true,
                },
                _ => true,
            }
        });
    (view, ok)
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
    let other_end = if kept == candidate.section_a {
        candidate.end_b
    } else {
        candidate.end_a
    };
    // Where the other section's station i lands: after the kept stations on
    // an append, before them on a prepend, counted backwards when reversed.
    let reversed = kept_end == other_end;
    for i in 0..=no {
        let was = old[&StationRef::new(other, i)];
        let want = (was.0 + o_shift.0, was.1 + o_shift.1);
        let along = if reversed { no - i } else { i };
        let index = if kept_end == SectionEnd::End {
            nk + along
        } else {
            along
        };
        assert!(
            dist(combined[index as usize], want) < tolerance,
            "{what}: other station {i} is not at combined station {index}"
        );
    }
    // No moved section carries a zero datum.
    for id in &plan.moved_sections {
        assert!(!before.carries_zero_datum(*id), "{what}: moved a datum");
    }
    // The closure-adjusted view (Plan, Export) stays put: what it holds the
    // survey at (a zero datum, else the component's gauge station) is on the
    // side that stays. Checked when the two were one connected survey before
    // (two surveys joined are held at one point from then on), and only where
    // both views solved (a failed solve shows the measured geometry).
    //
    // The solve leaves some coincident pairs untied: a snapped (Move) closure
    // and two side-by-side ends. Another loop's adjustment can then pull such
    // a pair apart in the view, and a combine that joins or demotes one holds
    // it together again: that drift is the view's own (reported separately),
    // not the combine's. So the "before" view ties them too: every snapped
    // closure counts (`with_snaps`, both sides), and a co-located join gets a
    // closure. With those tied, a combine never turns a solving survey into a
    // failing one either (a demoted snap can expose the solver's own limit, a
    // closure group holding both ends of one shot; the same survey hits it
    // before the combine once its snaps count).
    let mut tied = with_snaps(before);
    if candidate.join_kind == CombineJoinKind::CoLocated {
        tie(&mut tied, candidate.station_a, candidate.station_b);
    }
    let (was, was_ok) = adjusted_checked(&tied);
    let (now, now_ok) = adjusted_checked(&with_snaps(after));
    assert!(
        !was_ok || now_ok,
        "{what}: the closure-adjusted view solved before the combine and fails after it"
    );
    if before.get_connected_section_ids(kept).contains(&other) && was_ok && now_ok {
        let home = |at: StationRef| -> StationRef {
            if at.section_id == kept {
                StationRef::new(kept, at.station_index + offset)
            } else if at.section_id == other {
                let along = if reversed {
                    no - at.station_index
                } else {
                    at.station_index
                };
                StationRef::new(
                    kept,
                    if kept_end == SectionEnd::End {
                        nk + along
                    } else {
                        along
                    },
                )
            } else {
                at
            }
        };
        let tolerance = 1e-4;
        for (at, p) in &was {
            let Some(q) = now.get(&home(*at)) else {
                continue;
            };
            assert!(
                dist(*p, *q) < tolerance,
                "{what}: the closure-adjusted {at:?} moved {:.4} m",
                dist(*p, *q)
            );
        }
    }

    check_structure(after, what);
}

/// What every edit leaves, whatever it was: a clean integrity report, merges
/// on one point and forming a forest (a rigid loop is a closure's job), no
/// closure between merged stations and no snapped closure inside one
/// section, and a project a reload neither re-ties nor moves.
fn check_structure(after: &Project, what: &str) {
    check_structure_muting(after, what, &[]);
}

/// [`check_structure`], skipping the invariants named in `mute` (a long
/// campaign's known findings, `STALAGMIGHT_COMBINE_CHAIN_MUTE`):
/// `merged-closure` (a closure between merged stations).
fn check_structure_muting(after: &Project, what: &str, mute: &[String]) {
    let violations = after.check_structural_integrity();
    assert!(violations.is_empty(), "{what}: {violations:?}");
    after.debug_assert_merge_alignment(what);
    // Merges are rigid: sections and the points merges make (a merge group)
    // must form a forest. Redundant merges inside one group (a split copies a
    // junction's merges to both halves) tie one point to itself and are
    // fine; a cycle through two points, or a section at one point twice, is
    // a rigid loop, which is a closure's job.
    let mut points: HashMap<StationRef, StationRef> = HashMap::new();
    fn point(points: &mut HashMap<StationRef, StationRef>, x: StationRef) -> StationRef {
        let up = *points.get(&x).unwrap_or(&x);
        if up == x {
            x
        } else {
            let r = point(points, up);
            points.insert(x, r);
            r
        }
    }
    let mut merged: HashSet<StationRef> = HashSet::new();
    for merge in &after.station_merges {
        let (a, b) = (
            after.resolve_key(&merge.station_a).unwrap(),
            after.resolve_key(&merge.station_b).unwrap(),
        );
        assert_ne!(
            a.section_id, b.section_id,
            "{what}: a merge inside one section"
        );
        let (ra, rb) = (point(&mut points, a), point(&mut points, b));
        if ra != rb {
            points.insert(ra, rb);
        }
        merged.extend([a, b]);
    }
    // Node ids: a section as itself, a point as section id + 1_000_000 + n.
    let mut point_ids: HashMap<StationRef, SectionId> = HashMap::new();
    let mut forest: HashMap<SectionId, SectionId> = HashMap::new();
    let mut stations: Vec<StationRef> = merged.into_iter().collect();
    stations.sort_by_key(|at| (at.section_id, at.station_index));
    for at in stations {
        let p = point(&mut points, at);
        let next = 1_000_000 + point_ids.len() as SectionId;
        let node = *point_ids.entry(p).or_insert(next);
        let (ra, rb) = (root(&mut forest, at.section_id), root(&mut forest, node));
        assert_ne!(ra, rb, "{what}: merges close a loop");
        forest.insert(ra, rb);
    }
    for closure in &after.loop_closures {
        let a = after.resolve_key(&closure.station_a).unwrap();
        let b = after.resolve_key(&closure.station_b).unwrap();
        assert!(
            a != b
                && (mute.iter().any(|m| m == "merged-closure") || !after.are_stations_merged(a, b)),
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
///
/// `STALAGMIGHT_COMBINE_SEEDS` (default 120) seeds from
/// `STALAGMIGHT_COMBINE_FIRST_SEED` (default 0): a long campaign runs
/// rolling windows, and a failing seed reruns alone with `SEEDS=1`.
#[test]
fn every_offered_combine_in_random_networks_keeps_the_invariants() {
    let env = |name: &str, default: u64| -> u64 {
        std::env::var(name)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(default)
    };
    let seeds = env("STALAGMIGHT_COMBINE_SEEDS", 120);
    let first = env("STALAGMIGHT_COMBINE_FIRST_SEED", 0);
    let mut combined = 0usize;
    let mut kinds: HashSet<String> = HashSet::new();
    let mut torn = 0usize;
    let (mut shifted, mut kept_moved, mut with_branches) = (0usize, 0usize, 0usize);
    for seed in first..first + seeds {
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
            for candidate in project.combine_candidates_at_point(end) {
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
    eprintln!(
        "combine sweep seeds {first}..{}: {combined} combines, {torn} torn ties, {shifted} \
         shifted, {kept_moved} kept side moved, {with_branches} moved with attached sections",
        first + seeds
    );
    // The sweep reaches what it is for (120 seeds: ~3500 combines, ~1400
    // torn ties, ~550 shifts, ~110 with the kept side moving). Not asked of a
    // handful of seeds (a rerun of one).
    if seeds < 50 {
        return;
    }
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

/// Random chains of edits on random networks: combines (any pair the station
/// menu offers, either section kept), Split Section Here at a random station,
/// and reverses, each step checked like the single-combine sweep (a combine
/// against every combine invariant, the closure-adjusted view included; a
/// split or reverse against the structure), so an edit is also tested on
/// the shapes earlier edits leave.
///
/// `STALAGMIGHT_COMBINE_CHAIN_SEEDS` (default 40) from
/// `STALAGMIGHT_COMBINE_CHAIN_FIRST_SEED` (default 0), up to
/// `STALAGMIGHT_COMBINE_CHAIN_STEPS` (default 8) edits each.
#[test]
fn chains_of_combines_splits_and_reverses_keep_the_invariants() {
    let env = |name: &str, default: u64| -> u64 {
        std::env::var(name)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(default)
    };
    let seeds = env("STALAGMIGHT_COMBINE_CHAIN_SEEDS", 40);
    let first = env("STALAGMIGHT_COMBINE_CHAIN_FIRST_SEED", 0);
    let steps = env("STALAGMIGHT_COMBINE_CHAIN_STEPS", 8);
    // Known findings a long campaign mutes after they are filed, comma
    // separated (see `check_structure_muting`).
    let mute: Vec<String> = std::env::var("STALAGMIGHT_COMBINE_CHAIN_MUTE")
        .unwrap_or_default()
        .split(',')
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .collect();
    let (mut combines, mut splits, mut reverses) = (0usize, 0usize, 0usize);
    for seed in first..first + seeds {
        let mut rng = Rng(seed.wrapping_mul(0xA24B_AED4_963E_E407) | 1);
        let mut project = network(seed);
        let mut trail: Vec<String> = Vec::new();
        for step in 0..steps {
            let what = |op: &str, trail: &[String]| {
                format!(
                    "chain seed {seed} step {step} {op} after [{}]",
                    trail.join(", ")
                )
            };
            match rng.below(5) {
                0..=2 => {
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
                    let offered: Vec<CombineCandidate> = ends
                        .iter()
                        .flat_map(|end| project.combine_candidates_at_point(*end))
                        .filter(|c| c.blocked_reason.is_none())
                        .collect();
                    if offered.is_empty() {
                        continue;
                    }
                    let candidate = offered[rng.below(offered.len())].clone();
                    let keep_a = rng.below(2) == 0;
                    let (kept, kept_end, other, other_end) = if keep_a {
                        (
                            candidate.section_a,
                            candidate.end_a,
                            candidate.section_b,
                            candidate.end_b,
                        )
                    } else {
                        (
                            candidate.section_b,
                            candidate.end_b,
                            candidate.section_a,
                            candidate.end_a,
                        )
                    };
                    let op = format!("combine({kept}:{kept_end:?}, {other}:{other_end:?})");
                    let what = what(&op, &trail);
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
                    check_combined(&project, &after, &candidate, kept, &what);
                    project = after;
                    trail.push(op);
                    combines += 1;
                }
                3 => {
                    let long: Vec<SectionId> = project
                        .sections
                        .iter()
                        .filter(|s| s.shots.len() >= 2)
                        .map(|s| s.id)
                        .collect();
                    if long.is_empty() {
                        continue;
                    }
                    let id = long[rng.below(long.len())];
                    let shots = project.get_section(id).unwrap().shots.len();
                    let at = 1 + rng.below(shots - 1) as u32;
                    let op = format!("split({id} at {at})");
                    let what = what(&op, &trail);
                    let options = crate::SplitSectionOptions {
                        name_a: format!("S{id}a"),
                        name_b: format!("S{id}b"),
                        color_a: "#123456".to_string(),
                        color_b: "#654321".to_string(),
                        comment_side: crate::SplitSide::A,
                        datum_side: crate::SplitSide::A,
                    };
                    let mut after = project.clone();
                    if after.split_section_at_station(id, at, &options).is_err() {
                        continue;
                    }
                    // What the split command runs next: a closure across the
                    // cut that was the halves' only tie is a merge now.
                    after.convert_orphan_closures_to_merges();
                    check_structure_muting(&after, &what, &mute);
                    project = after;
                    trail.push(op);
                    splits += 1;
                }
                _ => {
                    let ids: Vec<SectionId> = project.sections.iter().map(|s| s.id).collect();
                    let id = ids[rng.below(ids.len())];
                    let op = format!("reverse({id})");
                    let what = what(&op, &trail);
                    let mut after = project.clone();
                    if !after.reverse_section(id, rng.below(2) == 0) {
                        continue;
                    }
                    // A reverse moves no station.
                    let n = after.get_section(id).unwrap().shots.len() as u32;
                    let was = positions(&project);
                    for (at, p) in positions(&after) {
                        let before_at = if at.section_id == id {
                            StationRef::new(id, n - at.station_index)
                        } else {
                            at
                        };
                        assert!(dist(p, was[&before_at]) < 1e-6, "{what}: {at:?} moved");
                    }
                    check_structure(&after, &what);
                    project = after;
                    trail.push(op);
                    reverses += 1;
                }
            }
        }
    }
    eprintln!(
        "combine chains seeds {first}..{}: {combines} combines, {splits} splits, {reverses} \
         reverses",
        first + seeds
    );
    if seeds >= 20 {
        assert!(combines > seeds as usize, "only {combines} combines");
        assert!(
            splits > 0 && reverses > 0,
            "splits {splits}, reverses {reverses}"
        );
    }
}

#[test]
#[ignore = "replays one chain: STALAGMIGHT_COMBINE_CHAIN_REPLAY=\"seed;op;op;...\""]
fn replay_a_combine_chain() {
    // The trail a chain failure prints, e.g.
    // "29;combine(0:Start, 1:End);split(0 at 1);reverse(4)", replayed with the
    // plan, ties and gauge printed before each combine.
    let spec = std::env::var("STALAGMIGHT_COMBINE_CHAIN_REPLAY").unwrap();
    let mut parts = spec.split(';');
    let seed: u64 = parts.next().unwrap().trim().parse().unwrap();
    let mut project = network(seed);
    let end = |s: &str| {
        if s.trim() == "End" {
            SectionEnd::End
        } else {
            SectionEnd::Start
        }
    };
    for op in parts {
        let op = op.trim();
        eprintln!("== {op}");
        if let Some(rest) = op.strip_prefix("combine(") {
            let rest = rest.trim_end_matches(')');
            let (k, o) = rest.split_once(", ").unwrap();
            let (k, ke) = k.split_once(':').unwrap();
            let (o, oe) = o.split_once(':').unwrap();
            let (k, o): (SectionId, SectionId) = (k.parse().unwrap(), o.parse().unwrap());
            let join = CombineJoinEnds {
                kept: end(ke),
                other: end(oe),
            };
            let c = project.combine_candidate_joining(k, o, Some(join)).unwrap();
            let plan = if c.section_a == k {
                &c.plan_keeping_a
            } else {
                &c.plan_keeping_b
            };
            let comp = project.get_connected_section_ids(k);
            eprintln!(
                "  kind {:?} plan {:?} anchor {:?} datums {:?}",
                c.join_kind,
                plan,
                project.adjustment_anchor_of(&comp),
                project
                    .zero_datums
                    .iter()
                    .map(|d| project.resolve_key(&d.station_ref))
                    .collect::<Vec<_>>()
            );
            for m in project.station_merges.iter() {
                eprintln!(
                    "  merge {:?} - {:?}",
                    project.resolve_key(&m.station_a),
                    project.resolve_key(&m.station_b)
                );
            }
            for cl in &project.loop_closures {
                eprintln!(
                    "  closure {:?} - {:?} aligned {} {:?}",
                    project.resolve_key(&cl.station_a),
                    project.resolve_key(&cl.station_b),
                    cl.is_aligned,
                    cl.strategy
                );
            }
            eprintln!("  held {:?}", project.held_gauges);
            let mut after = project.clone();
            after.combine_sections(k, o, &options(Some(join))).unwrap();
            check_combined(&project, &after, &c, k, op);
            project = after;
        } else if let Some(rest) = op.strip_prefix("split(") {
            let (id, at) = rest.trim_end_matches(')').split_once(" at ").unwrap();
            let id: SectionId = id.parse().unwrap();
            let opts = crate::SplitSectionOptions {
                name_a: format!("S{id}a"),
                name_b: format!("S{id}b"),
                color_a: "#123456".into(),
                color_b: "#654321".into(),
                comment_side: crate::SplitSide::A,
                datum_side: crate::SplitSide::A,
            };
            project
                .split_section_at_station(id, at.parse().unwrap(), &opts)
                .unwrap();
            project.convert_orphan_closures_to_merges();
        } else if let Some(rest) = op.strip_prefix("reverse(") {
            project.reverse_section(rest.trim_end_matches(')').parse().unwrap(), true);
        }
    }
}
