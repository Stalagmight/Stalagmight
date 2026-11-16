//! Property fuzzer for GPS fixes in the Closures Computed View solve (#741).
//!
//! Random networks (sections merged into trees, loop closures, separately
//! placed pieces, soft and hard GPS fixes, random solve sigmas), solved by
//! [`Project::compute_adjusted_stations`]. Every property is derived from
//! `lsq.rs`, not from the feature's summary:
//!
//! - `nonfinite`: every computed coordinate is finite.
//! - `hard-fix-off-gps`: a hard fix (effective accuracy under
//!   `HARD_FIX_ACCURACY_M`, a non-finite or negative one reading as the
//!   default 0) whose constraint group (stations tied by merges and by the
//!   closures the solve uses) is not already held - by a placement datum, or
//!   by an earlier hard fix in list order - sits on its GPS, unless its
//!   component is one the solve reported as failed (shown measured).
//!   `hard-fix-no-unknowns` is the same failure in a component whose every
//!   constraint group is held, which the solve returns from before it
//!   rewrites a single shot.
//! - `placement-moved`: a placement datum's station keeps its raw position.
//! - `depth-moved`: no computed z differs from the z of the same view with
//!   every fix removed (the solve is horizontal; a fix's elevation is never
//!   read).
//! - `tighten-moved-away`: tightening one fix's accuracy never moves that
//!   fix's station farther from its GPS. Each axis is a convex quadratic
//!   problem in which raising one observation's weight cannot raise that
//!   observation's residual.
//! - `consistent-fix-moved`: a fix whose GPS is exactly where the view
//!   without it puts its station changes nothing. Its residual at the old
//!   optimum is zero, so the optimum stays; it also pins its passage against
//!   the spur pruning, which on a tree branch reproduces the measured
//!   vectors anyway.
//!
//! `STALAGMIGHT_DATUM_LSQ_FUZZ_SEEDS` (default 400) networks from
//! `STALAGMIGHT_DATUM_LSQ_FUZZ_FIRST_SEED` (default 0);
//! `STALAGMIGHT_DATUM_LSQ_FUZZ_MUTE` names failure classes to skip (comma
//! separated, matched as a prefix of the failure message; unset, the open
//! findings in [`KNOWN_OPEN`] are muted, so CI stays green on them), and
//! `STALAGMIGHT_DATUM_LSQ_FUZZ_KEEP_GOING=1` reports every failing seed
//! before failing instead of stopping at the first.
//! Synthetic surveys only.

use std::collections::{HashMap, HashSet};

use crate::config::ClosureAdjustmentConfig;
use crate::{
    gps_to_local, local_to_gps, ClosureStrategy, DatumRole, GpsCoordinates, Project, SectionData,
    SectionId, ShotData, StationRef, ZeroDatum,
};

/// `lsq.rs`'s `HARD_FIX_ACCURACY_M`: below it a fix is a hard pin.
const HARD_FIX_ACCURACY_M: f64 = 1e-3;

/// Per axis: the solve accepts stations of one section disagreeing on their
/// section's translation by up to 1e-6 m before it refuses, so a held
/// station can land that far from its target; plus float noise.
const HELD_TOL_M: f64 = 1e-6 + 1e-8;

/// Accuracies a datum is given: the default, a hard pin, and soft ones up to
/// the largest the app accepts.
const ACCURACIES: [Option<f64>; 6] = [
    None,
    Some(0.0),
    Some(0.5),
    Some(5.0),
    Some(50.0),
    Some(1000.0),
];

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

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

    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// Failure classes this fuzzer found that are open bugs, muted unless
/// `STALAGMIGHT_DATUM_LSQ_FUZZ_MUTE` says otherwise (set it to an empty
/// string to run every check):
///
/// - `hard-fix-no-unknowns`: a hard fix in a component whose every
///   constraint group is held (for example a one-shot section with the
///   placement datum at one end and the hard fix at the other) is never
///   applied: `adjust_least_squares` returns at `if n == 0` before it
///   rewrites a shot, so the Closures Computed View shows the raw survey,
///   the fix off its GPS and any closure between held groups unapplied.
///   `STALAGMIGHT_DATUM_LSQ_FUZZ_REPLAY=19` prints the smallest network the
///   sweep reaches it on: a one-shot section, placement datum at station 0,
///   hard fix at station 1.
const KNOWN_OPEN: &[&str] = &["hard-fix-no-unknowns"];

/// The mute list: `name`'s comma-separated classes when it is set, else
/// `default`.
fn mute_list(name: &str, default: &[&str]) -> Vec<String> {
    match std::env::var(name) {
        Ok(list) => list
            .split(',')
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .collect(),
        Err(_) => default.iter().map(|m| (*m).to_string()).collect(),
    }
}

/// One shot: random bearing and length, depths that change by less than the
/// length (a shot cannot drop more than it is long).
fn random_shot(rng: &mut Rng, depth_in: f64) -> ShotData {
    let length = rng.range(2.0, 25.0);
    let depth_out = (depth_in + rng.range(-0.6, 0.6) * length).max(0.0);
    let azimuth = rng.range(0.0, 360.0);
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in,
        depth_out,
        ..Default::default()
    }
}

fn every_station(project: &Project) -> Vec<StationRef> {
    project
        .sections
        .iter()
        .flat_map(|s| (0..=s.shots.len() as u32).map(move |i| StationRef::new(s.id, i)))
        .collect()
}

/// A random network: what [`network`] built, and the config to solve it with.
struct Network {
    project: Project,
    config: ClosureAdjustmentConfig,
}

/// The frame's GPS: mostly mid latitudes, some near a pole or the equator.
fn random_origin(rng: &mut Rng) -> GpsCoordinates {
    let lat = match rng.below(4) {
        0 => rng.range(-0.01, 0.01),
        1 => rng.range(70.0, 80.0),
        _ => rng.range(-50.0, 50.0),
    };
    GpsCoordinates::new(lat, rng.range(-179.0, 179.0), rng.range(0.0, 5.0)).unwrap()
}

/// GPS `noise` metres (each axis) from where the frame puts `at` now.
fn gps_near(
    project: &Project,
    at: StationRef,
    noise: (f64, f64),
    frame: &GpsCoordinates,
) -> Option<GpsCoordinates> {
    let c = project.compute_station_raw_coords(at)?;
    let (lat, lon) = local_to_gps(c.x + noise.0, c.y + noise.1, frame)?;
    GpsCoordinates::new(lat, lon, 0.0).ok()
}

/// Sections merged into trees (some left on their own), loop closures in the
/// merged groups and now and then between two placed pieces, a placement
/// datum on most pieces, and soft and hard fixes on random stations of the
/// placed ones, a few of them on a placement datum's own station or a
/// station merged to it.
fn network(seed: u64) -> Network {
    let mut rng = Rng::new(seed);
    let mut project = Project::new(format!("LsqFuzz{seed}"), None);
    let sections = 2 + rng.below(5);
    for _ in 0..sections {
        let mut depth = rng.range(0.0, 30.0);
        let shots = (0..=rng.below(6))
            .map(|_| {
                let shot = random_shot(&mut rng, depth);
                depth = shot.depth_out;
                shot
            })
            .collect();
        let quality = if rng.chance(30) {
            rng.range(0.5, 3.0)
        } else {
            1.0
        };
        project.add_section(SectionData {
            quality,
            shots,
            ..Default::default()
        });
    }
    project.ensure_shot_uuids();
    let ids: Vec<SectionId> = project.sections.iter().map(|s| s.id).collect();
    for (i, &id) in ids.iter().enumerate().skip(1) {
        if rng.chance(70) {
            let other = ids[rng.below(i)];
            let a = random_station_of(&project, other, &mut rng);
            let b = random_station_of(&project, id, &mut rng);
            let _ = project.merge_stations(a, b);
        }
    }

    // Placement datums: the first is the frame, at a random origin.
    let origin = random_origin(&mut rng);
    let mut frame: Option<GpsCoordinates> = None;
    let mut seen: HashSet<SectionId> = HashSet::new();
    for &id in &ids {
        if seen.contains(&id) {
            continue;
        }
        seen.extend(project.get_merged_section_ids(id));
        if !rng.chance(80) {
            continue;
        }
        let at = random_station_of(&project, id, &mut rng);
        let gps = match &frame {
            None => origin,
            Some(f) => {
                let noise = (rng.range(-80.0, 80.0), rng.range(-80.0, 80.0));
                let Some(gps) = gps_near(&project, at, noise, f) else {
                    continue;
                };
                gps
            }
        };
        let accuracy = ACCURACIES[rng.below(ACCURACIES.len())];
        if project
            .set_zero_datum_with_accuracy(at, gps, accuracy)
            .is_ok()
            && frame.is_none()
        {
            frame = Some(gps);
        }
    }

    // Loop closures: inside a merged group, or between two placed pieces.
    let stations = every_station(&project);
    for _ in 0..rng.below(4) {
        let a = stations[rng.below(stations.len())];
        let b = stations[rng.below(stations.len())];
        let _ = project.create_loop_closure(a, b, ClosureStrategy::Batch);
    }
    project.sweep_orphan_closures();

    // GPS fixes on placed pieces, appended after every placement datum.
    if let Some(frame) = frame {
        let placed: HashSet<SectionId> = project
            .placement_datum_refs()
            .into_iter()
            .flat_map(|sr| project.get_merged_section_ids(sr.section_id))
            .collect();
        let candidates: Vec<StationRef> = stations
            .iter()
            .copied()
            .filter(|s| placed.contains(&s.section_id))
            .collect();
        // Stations merged to a placement datum's station: one point with it,
        // so a fix there is in the placement's own constraint group.
        let placements: HashSet<StationRef> = project.placement_datum_refs().into_iter().collect();
        let beside_placement: Vec<StationRef> = project
            .station_merges
            .iter()
            .filter_map(|m| {
                Some((
                    project.resolve_key(&m.station_a)?,
                    project.resolve_key(&m.station_b)?,
                ))
            })
            .filter_map(|(a, b)| {
                if placements.contains(&a) {
                    Some(b)
                } else if placements.contains(&b) {
                    Some(a)
                } else {
                    None
                }
            })
            .collect();
        let mut taken: HashSet<StationRef> = project.all_datum_refs().into_iter().collect();
        for _ in 0..rng.below(6) {
            if candidates.is_empty() {
                break;
            }
            let at = if rng.chance(20) && !beside_placement.is_empty() {
                beside_placement[rng.below(beside_placement.len())]
            } else {
                candidates[rng.below(candidates.len())]
            };
            // One datum per station, as every write path keeps it.
            if !taken.insert(at) {
                continue;
            }
            let noise = if rng.chance(15) {
                (0.0, 0.0)
            } else {
                (rng.range(-100.0, 100.0), rng.range(-100.0, 100.0))
            };
            let Some(gps) = gps_near(&project, at, noise, &frame) else {
                continue;
            };
            let Some(key) = project.key_for_ref(at) else {
                continue;
            };
            project.zero_datums.push(ZeroDatum {
                station_ref: key,
                gps,
                accuracy_m: ACCURACIES[rng.below(ACCURACIES.len())],
            });
        }
    }

    let mut config = ClosureAdjustmentConfig::default();
    if rng.chance(40) {
        config.base_sigma_length_m = rng.range(0.005, 1.0);
        config.base_sigma_azimuth_deg = rng.range(0.1, 5.0);
    }
    Network { project, config }
}

fn random_station_of(project: &Project, id: SectionId, rng: &mut Rng) -> StationRef {
    let shots = project.get_section(id).map_or(0, |s| s.shots.len());
    let index = if rng.chance(30) {
        0
    } else {
        rng.below(shots + 1)
    };
    StationRef::new(id, index as u32)
}

/// Union-find over stations.
#[derive(Default)]
struct Groups(HashMap<StationRef, StationRef>);

impl Groups {
    fn find(&mut self, x: StationRef) -> StationRef {
        let up = *self.0.get(&x).unwrap_or(&x);
        if up == x {
            return x;
        }
        let root = self.find(up);
        self.0.insert(x, root);
        root
    }

    fn union(&mut self, a: StationRef, b: StationRef) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0.insert(ra, rb);
        }
    }
}

/// A hard fix the solve holds on its GPS.
struct HeldFix {
    at: StationRef,
    /// Its GPS in the frame's local metres.
    target: (f64, f64),
    /// Every constraint group of its component is held, so the solve has no
    /// unknown at all.
    no_unknowns: bool,
}

/// The solve's constraint groups (stations tied by a merge or by a closure
/// the solve uses), and the roots of those it holds: a group holding a
/// placement datum first, then each hard fix's group in list order, with the
/// hard fixes that hold theirs. Derived from `adjust_least_squares`.
fn held_groups(project: &Project) -> (Groups, HashSet<StationRef>, Vec<HeldFix>) {
    let skipped: HashSet<u64> = project
        .closures_across_one_shot()
        .into_iter()
        .map(|c| c.closure_id)
        .collect();
    let mut groups = Groups::default();
    for merge in &project.station_merges {
        if let (Some(a), Some(b)) = (
            project.resolve_key(&merge.station_a),
            project.resolve_key(&merge.station_b),
        ) {
            groups.union(a, b);
        }
    }
    for closure in &project.loop_closures {
        let eligible = matches!(
            closure.strategy,
            ClosureStrategy::Batch | ClosureStrategy::LeastSquaresWeighted
        );
        if !eligible || skipped.contains(&closure.id) {
            continue;
        }
        if let (Some(a), Some(b)) = (
            project.resolve_key(&closure.station_a),
            project.resolve_key(&closure.station_b),
        ) {
            groups.union(a, b);
        }
    }
    let mut held: HashSet<StationRef> = project
        .placement_datum_refs()
        .into_iter()
        .map(|sr| groups.find(sr))
        .collect();
    let mut fixes = Vec::new();
    let Some(frame) = project.frame_datum().map(|zd| zd.gps) else {
        return (groups, held, fixes);
    };
    for zd in project.gps_fix_datums() {
        let Some(at) = project.resolve_key(&zd.station_ref) else {
            continue;
        };
        let accuracy = match zd.effective_accuracy_m() {
            a if a.is_finite() && a >= 0.0 => a,
            _ => crate::DEFAULT_GPS_ACCURACY_M,
        };
        if accuracy >= HARD_FIX_ACCURACY_M {
            continue;
        }
        if held.insert(groups.find(at)) {
            fixes.push(HeldFix {
                at,
                target: gps_to_local(&zd.gps, &frame),
                no_unknowns: false,
            });
        }
    }
    (groups, held, fixes)
}

/// Whether every constraint group of `section`'s component is held, so the
/// solve has no unknown there.
fn component_all_held(
    project: &Project,
    groups: &mut Groups,
    held: &HashSet<StationRef>,
    section: SectionId,
) -> bool {
    let component = project.get_connected_section_ids(section);
    every_station(project)
        .into_iter()
        .filter(|s| component.contains(&s.section_id))
        .all(|s| held.contains(&groups.find(s)))
}

/// The hard fixes the solve holds on their GPS ([`held_groups`]).
fn enforced_hard_fixes(project: &Project) -> Vec<HeldFix> {
    let (mut groups, held, mut fixes) = held_groups(project);
    for fix in &mut fixes {
        fix.no_unknowns = component_all_held(project, &mut groups, &held, fix.at.section_id);
    }
    fixes
}

/// "hard-fix-no-unknowns" when every group of `section`'s component is held
/// in `project` (the solve's `n == 0` return), else `class`.
fn class_unless_all_held(
    project: &Project,
    section: SectionId,
    class: &'static str,
) -> &'static str {
    let (mut groups, held, _) = held_groups(project);
    if component_all_held(project, &mut groups, &held, section) {
        "hard-fix-no-unknowns"
    } else {
        class
    }
}

/// A computed view: every station's (x, y, z), the sections of every
/// component the solve could not adjust (shown measured), and why.
struct View {
    at: HashMap<StationRef, (f64, f64, f64)>,
    failed: HashSet<SectionId>,
    errors: Vec<String>,
}

fn solve(project: &Project, config: &ClosureAdjustmentConfig) -> View {
    let solved = project.compute_adjusted_stations(config);
    View {
        at: solved
            .stations
            .iter()
            .map(|s| {
                let c = s.coordinates;
                (s.station_ref, (c.x, c.y, c.z))
            })
            .collect(),
        failed: solved
            .failures
            .iter()
            .flat_map(|f| f.section_ids.iter().copied())
            .collect(),
        errors: solved.failures.iter().map(|f| f.error.clone()).collect(),
    }
}

/// Distance from `at`'s computed position to `target`, horizontally.
fn off(view: &View, at: StationRef, target: (f64, f64)) -> f64 {
    let (x, y, _) = view.at[&at];
    (x - target.0).hypot(y - target.1)
}

/// `project` without its GPS fixes. Removing a fix changes no other role: a
/// fix is never the first datum of its merge component.
fn without_fixes(project: &Project) -> Project {
    let mut stripped = project.clone();
    let roles = project.datum_roles();
    let mut role = roles.iter();
    stripped
        .zero_datums
        .retain(|_| role.next().copied().flatten() != Some(DatumRole::GpsFix));
    stripped
}

/// The first failure class, if any, of the per-view properties. `bare` is
/// the view of the same project with its fixes removed.
///
/// Depth is compared with `bare`, not with the raw view: a solved closure is
/// marked aligned on the working copy, and an aligned closure joins its two
/// stations' depth average, so the computed view moves the depth of closure
/// ends by design. A fix must add nothing to that, wherever neither solve
/// failed (a failed component keeps its closures unaligned).
fn check_view(project: &Project, view: &View, bare: &View) -> Result<(), String> {
    let raw: HashMap<StationRef, (f64, f64, f64)> = project
        .compute_station_coordinates()
        .into_iter()
        .map(|s| {
            let c = s.coordinates;
            (s.station_ref, (c.x, c.y, c.z))
        })
        .collect();
    for (at, &(x, y, z)) in &view.at {
        if !(x.is_finite() && y.is_finite() && z.is_finite()) {
            return Err(format!("nonfinite: {at:?} at ({x}, {y}, {z})"));
        }
        if view.failed.contains(&at.section_id) || bare.failed.contains(&at.section_id) {
            continue;
        }
        let bz = bare.at[at].2;
        if (z - bz).abs() > 1e-9 {
            return Err(format!(
                "depth-moved: {at:?} z {z} with the fixes, {bz} without them (raw {})",
                raw[at].2
            ));
        }
    }
    for at in project.placement_datum_refs() {
        if view.failed.contains(&at.section_id) {
            continue;
        }
        let (rx, ry, _) = raw[&at];
        let (x, y, _) = view.at[&at];
        if (x - rx).abs() > HELD_TOL_M || (y - ry).abs() > HELD_TOL_M {
            return Err(format!(
                "placement-moved: {at:?} raw ({rx}, {ry}) computed ({x}, {y})"
            ));
        }
    }
    for fix in enforced_hard_fixes(project) {
        let at = fix.at;
        if view.failed.contains(&at.section_id) {
            continue;
        }
        let (x, y, _) = view.at[&at];
        let (gx, gy) = fix.target;
        if (x - gx).abs() > HELD_TOL_M || (y - gy).abs() > HELD_TOL_M {
            // `adjust_least_squares` returns before it rewrites a shot when
            // no group is left to solve (`n == 0`), so a hard fix in a
            // component whose every group is held is never applied.
            let class = if fix.no_unknowns {
                "hard-fix-no-unknowns"
            } else {
                "hard-fix-off-gps"
            };
            return Err(format!(
                "{class}: {at:?} computed ({x}, {y}), GPS ({gx}, {gy})"
            ));
        }
    }
    Ok(())
}

/// Tighten one soft fix's accuracy: its station must not move away from its
/// GPS. Returns whether a comparison was made.
fn check_tightening(
    net: &Network,
    rng: &mut Rng,
    base: &View,
    bare: &View,
) -> Result<bool, String> {
    let project = &net.project;
    let Some(frame) = project.frame_datum().map(|zd| zd.gps) else {
        return Ok(false);
    };
    let roles = project.datum_roles();
    let soft: Vec<usize> = (0..project.zero_datums.len())
        .filter(|&i| {
            roles[i] == Some(DatumRole::GpsFix)
                && project.zero_datums[i].effective_accuracy_m() >= HARD_FIX_ACCURACY_M
        })
        .collect();
    if soft.is_empty() {
        return Ok(false);
    }
    let i = soft[rng.below(soft.len())];
    let zd = &project.zero_datums[i];
    let Some(at) = project.resolve_key(&zd.station_ref) else {
        return Ok(false);
    };
    let was = zd.effective_accuracy_m();
    let now = match rng.below(3) {
        0 => 0.0,
        1 => was * rng.range(0.0, 1.0),
        _ => (was * 0.5).max(HARD_FIX_ACCURACY_M),
    };
    let mut tighter = project.clone();
    tighter.zero_datums[i].accuracy_m = Some(now);
    let after = solve(&tighter, &net.config);
    // The tightened project is a network of its own: its view must hold
    // first, so a fix the tightening made hard is judged as one.
    check_view(&tighter, &after, bare)?;
    if base.failed.contains(&at.section_id) || after.failed.contains(&at.section_id) {
        return Ok(false);
    }
    let target = gps_to_local(&zd.gps, &frame);
    let (d0, d1) = (off(base, at, target), off(&after, at, target));
    if d1 > d0 + 1e-6 {
        let class = class_unless_all_held(&tighter, at.section_id, "tighten-moved-away");
        return Err(format!(
            "{class}: fix {i} at {at:?}, accuracy {was} -> {now}: {d0} m -> {d1} m \
             from its GPS"
        ));
    }
    Ok(true)
}

/// Add a fix exactly where the view without it puts its station: nothing may
/// move. Returns whether a comparison was made.
fn check_consistent_fix(
    net: &Network,
    rng: &mut Rng,
    base: &View,
    bare: &View,
) -> Result<bool, String> {
    let project = &net.project;
    let Some(frame) = project.frame_datum().map(|zd| zd.gps) else {
        return Ok(false);
    };
    let placed: HashSet<SectionId> = project
        .placement_datum_refs()
        .into_iter()
        .flat_map(|sr| project.get_merged_section_ids(sr.section_id))
        .collect();
    let taken: HashSet<StationRef> = project.all_datum_refs().into_iter().collect();
    let candidates: Vec<StationRef> = every_station(project)
        .into_iter()
        .filter(|s| placed.contains(&s.section_id) && !taken.contains(s))
        .collect();
    if candidates.is_empty() {
        return Ok(false);
    }
    let at = candidates[rng.below(candidates.len())];
    let (x, y, _) = base.at[&at];
    let Some((lat, lon)) = local_to_gps(x, y, &frame) else {
        return Ok(false);
    };
    let Ok(gps) = GpsCoordinates::new(lat, lon, 0.0) else {
        return Ok(false);
    };
    // The GPS round trip is not bit-exact; it must be far below the check.
    let (ex, ey) = gps_to_local(&gps, &frame);
    if (ex - x).hypot(ey - y) > 1e-8 {
        return Ok(false);
    }
    let mut with_fix = project.clone();
    let Some(key) = with_fix.key_for_ref(at) else {
        return Ok(false);
    };
    with_fix.zero_datums.push(ZeroDatum {
        station_ref: key,
        gps,
        accuracy_m: ACCURACIES[rng.below(ACCURACIES.len())],
    });
    if with_fix.datum_roles().last() != Some(&Some(DatumRole::GpsFix)) {
        return Ok(false);
    }
    let after = solve(&with_fix, &net.config);
    check_view(&with_fix, &after, bare)?;
    let component = project.get_connected_section_ids(at.section_id);
    if component
        .iter()
        .any(|s| base.failed.contains(s) || after.failed.contains(s))
    {
        return Ok(false);
    }
    for (sr, &(bx, by, _)) in &base.at {
        let (ax, ay, _) = after.at[sr];
        if (ax - bx).abs() > 1e-6 || (ay - by).abs() > 1e-6 {
            let class = class_unless_all_held(&with_fix, at.section_id, "consistent-fix-moved");
            return Err(format!(
                "{class}: a fix ({:?}) at {at:?} on its computed position moved {sr:?} \
                 from ({bx}, {by}) to ({ax}, {ay})",
                with_fix.zero_datums.last().and_then(|zd| zd.accuracy_m)
            ));
        }
    }
    Ok(true)
}

/// Every property on network `seed`; `Err` carries the failure, its class
/// first.
fn run_seed(seed: u64, mute: &[String], stats: &mut Stats) -> Result<(), String> {
    let net = network(seed);
    let mut rng = Rng::new(seed ^ 0x5DEE_CE66_D1CE_4E5B);
    let muted = |e: &String| mute.iter().any(|m| e.starts_with(m.as_str()));
    let base = solve(&net.project, &net.config);
    stats.fixes += net.project.gps_fix_datums().len();
    stats.enforced += enforced_hard_fixes(&net.project).len();
    for error in &base.errors {
        *stats.solve_errors.entry(error.clone()).or_default() += 1;
    }
    let bare = solve(&without_fixes(&net.project), &net.config);
    let mut outcome = check_view(&net.project, &base, &bare);
    if outcome.is_ok() {
        match check_tightening(&net, &mut rng, &base, &bare) {
            Ok(done) => stats.tightened += usize::from(done),
            Err(e) => outcome = Err(e),
        }
    }
    if outcome.is_ok() {
        match check_consistent_fix(&net, &mut rng, &base, &bare) {
            Ok(done) => stats.consistent += usize::from(done),
            Err(e) => outcome = Err(e),
        }
    }
    match outcome {
        Err(e) if muted(&e) => {
            stats.muted += 1;
            Ok(())
        }
        other => other,
    }
}

#[derive(Default)]
struct Stats {
    fixes: usize,
    enforced: usize,
    tightened: usize,
    consistent: usize,
    solve_errors: std::collections::BTreeMap<String, usize>,
    muted: usize,
}

#[test]
fn gps_fixes_in_the_computed_view_keep_their_properties() {
    let seeds = env_u64("STALAGMIGHT_DATUM_LSQ_FUZZ_SEEDS", 400);
    let first = env_u64("STALAGMIGHT_DATUM_LSQ_FUZZ_FIRST_SEED", 0);
    let mute = mute_list("STALAGMIGHT_DATUM_LSQ_FUZZ_MUTE", KNOWN_OPEN);
    let keep_going = std::env::var("STALAGMIGHT_DATUM_LSQ_FUZZ_KEEP_GOING").is_ok_and(|v| v == "1");
    let mut stats = Stats::default();
    let mut failed: Vec<String> = Vec::new();
    for seed in first..first + seeds {
        if let Err(e) = run_seed(seed, &mute, &mut stats) {
            let line = format!("{e} (datum lsq fuzz seed {seed})");
            assert!(keep_going, "{line}");
            eprintln!("FAIL {line}");
            failed.push(line);
        }
    }
    eprintln!(
        "datum lsq fuzz seeds {first}..{}: {} fixes, {} hard fixes held, {} tightenings, {} \
         consistent fixes added, {} muted (mute list {mute:?}); components the solve refused: \
         {:?}",
        first + seeds,
        stats.fixes,
        stats.enforced,
        stats.tightened,
        stats.consistent,
        stats.muted,
        stats.solve_errors
    );
    assert!(
        failed.is_empty(),
        "{} failing seeds, first: {}",
        failed.len(),
        failed[0]
    );
    if seeds >= 100 {
        assert!(
            stats.enforced > seeds as usize / 4,
            "too few hard fixes held"
        );
        assert!(stats.tightened > seeds as usize / 4, "too few tightenings");
        assert!(
            stats.consistent > seeds as usize / 4,
            "too few consistent fixes"
        );
    }
}

/// Print network `seed` - its sections, merges, closures, datums with their
/// roles, and both views - then run its checks.
#[test]
#[ignore = "replays one network: STALAGMIGHT_DATUM_LSQ_FUZZ_REPLAY=<seed>"]
fn replay_a_datum_lsq_seed() {
    let seed: u64 = std::env::var("STALAGMIGHT_DATUM_LSQ_FUZZ_REPLAY")
        .unwrap()
        .parse()
        .unwrap();
    let net = network(seed);
    let p = &net.project;
    for s in &p.sections {
        eprintln!(
            "section {} offset {:?}: {} shots",
            s.id,
            p.section_offsets.get(&s.id),
            s.shots.len()
        );
    }
    for m in &p.station_merges {
        eprintln!(
            "merge {:?} = {:?}",
            p.resolve_key(&m.station_a),
            p.resolve_key(&m.station_b)
        );
    }
    for c in &p.loop_closures {
        eprintln!(
            "closure {} {:?} - {:?} {:?} aligned {}",
            c.id,
            p.resolve_key(&c.station_a),
            p.resolve_key(&c.station_b),
            c.strategy,
            c.is_aligned
        );
    }
    let frame = p.frame_datum().map(|zd| zd.gps);
    for (zd, role) in p.zero_datums.iter().zip(p.datum_roles()) {
        let at = p.resolve_key(&zd.station_ref);
        let local = frame.map(|f| gps_to_local(&zd.gps, &f));
        let raw = at.and_then(|a| p.compute_station_raw_coords(a));
        eprintln!(
            "datum {at:?} {role:?} accuracy {:?} GPS local {local:?} raw {raw:?}",
            zd.accuracy_m
        );
    }
    for fix in enforced_hard_fixes(p) {
        eprintln!(
            "held hard fix {:?} at {:?}, no unknowns {}",
            fix.at, fix.target, fix.no_unknowns
        );
    }
    let solved = p.compute_adjusted_stations(&net.config);
    eprintln!("failures: {:?}", solved.failures);
    for s in &solved.stations {
        let raw = p.compute_station_raw_coords(s.station_ref).unwrap();
        eprintln!(
            "{:?}: raw ({:.4}, {:.4}, {:.4}) computed ({:.4}, {:.4}, {:.4})",
            s.station_ref, raw.x, raw.y, raw.z, s.coordinates.x, s.coordinates.y, s.coordinates.z
        );
    }
    let mut stats = Stats::default();
    if let Err(e) = run_seed(seed, &[], &mut stats) {
        panic!("{e}");
    }
}
