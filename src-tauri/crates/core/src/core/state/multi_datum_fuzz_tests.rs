//! Chains of random commands on surveys with several zero datums (#741),
//! through the real command path: `AppState::execute`, the split and
//! combine entry points, undo, redo, and a save-and-reopen round trip.
//! Synthetic surveys only.
//!
//! A chain starts from a random project built by commands (lines, some
//! merges, one to three datums), then runs random steps: set a datum
//! (placement or fix, its GPS 0-100 m from where the survey puts the
//! station), set a datum with another datum's GPS bit for bit, change only
//! an accuracy, remove a datum (the frame on purpose now and then), merge
//! (two separately placed pieces on purpose now and then), unmerge, add a
//! loop closure, split, Combine Lines through an offered candidate,
//! delete a line or a shot, add a shot, undo, redo, reopen. Datums land
//! on station 0, on lines that already hold one and in the frame's own
//! merge component more often than chance.
//!
//! After every state change, each failure message starting with its class:
//!
//! - `nonfinite`: a raw or computed coordinate, or a datum's GPS, is not
//!   finite.
//! - `integrity`: `AppState::integrity_check` reports anything (the setup is
//!   clean).
//! - `roles`: `datum_roles` disagrees with a re-derivation from the merges
//!   (the first resolvable datum of a merge component places it, every later
//!   one is a fix).
//! - `placement-off-gps` / `frame-off-origin`: a placement datum's station
//!   is not at `gps_to_local(its GPS, the frame's GPS)`. Every placement is
//!   exact against the frame in effect when it was placed (`set_zero_datum`,
//!   `settle_datum_layout`); a re-frame translates by the new frame's
//!   measured position, which the local tangent plane makes differ from the
//!   new frame's own projection by a small amount that grows with each
//!   re-frame. The tolerance is `2e-6 * 2^k + eps * (2^k - 1)`: `eps` the
//!   largest such discrepancy over every GPS the chain has seen, `k` the
//!   re-frames so far. Most chains sit near the equator, where `eps` stays
//!   under a micrometre; the others widen with it.
//! - `promoted-twin-gps`, `datum-key-reused`, `frame-gps-edit`: the same
//!   failure in a step with a recognised mechanism: a promoted fix with a GPS
//!   bit-identical to a datum that was a placement before; a datum
//!   re-anchored onto the key of a datum the step removed; a new GPS on the
//!   frame datum (all fixed, see [`KNOWN_OPEN`]).
//! - `datum-twin-station`: two datums resolve to one station (Combine
//!   Lines joining two datum stations; now refused).
//! - `hard-fix-off-gps` / `hard-fix-no-unknowns`: in the Closures Computed
//!   View, a hard fix the solve holds is not on its GPS (rule derived from
//!   `lsq.rs`; see `held_groups`). `-no-unknowns`: its component has no
//!   unknown left (fixed: the solve once returned at `n == 0` before it
//!   rewrote a shot).
//! - `placement-moved-computed`: a placement datum moved in the computed
//!   view.
//! - `depth-moved`: a datum command changed a station's depth (topology
//!   unchanged), or a fix changed a computed depth (compared with the same
//!   view with every fix removed).
//! - `fix-moved-raw`: storing or removing a GPS fix moved raw geometry.
//! - `undo-inexact` / `redo-inexact`: an undo or redo did not restore the
//!   state it should (datums with GPS bits, accuracies and roles, merges,
//!   closures, lines, raw coordinates to 1e-9). Every forward step is
//!   undone and redone at once; random undo and redo steps go deeper.
//!   `undo-across-reopen`: the same, for a state from before a reopen.
//! - `reopen-drift`: save and reopen changed the datums, roles, merges,
//!   closures or raw coordinates (1e-9), or the computed view (1e-6).
//! - `refusal-mutated`: a refused command changed the project.
//! - `panic`: a command or a check panicked.
//!
//! Environment: `STALAGMIGHT_DATUM_FUZZ_SEEDS` (default 40) chains from
//! `STALAGMIGHT_DATUM_FUZZ_FIRST_SEED` (default 0), each
//! `STALAGMIGHT_DATUM_FUZZ_STEPS` (default 12) steps long;
//! `STALAGMIGHT_DATUM_FUZZ_MUTE`, comma separated classes to mute (unset:
//! [`KNOWN_OPEN`]; empty: none), a muted failure ending its chain;
//! `STALAGMIGHT_DATUM_FUZZ_KEEP_GOING=1` reports every failing chain before
//! failing. A failure prints its trail as a replay:
//! `STALAGMIGHT_DATUM_FUZZ_REPLAY="seed;op;op;..."` runs
//! `replay_a_datum_chain` (ignored by default), which prints each step.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use super::multi_datum_steps_fuzz::{
    apply, datum_op, every_station, next_op, parse_op, shots, Applied, Op, Rng,
};
use super::AppState;
use crate::config::ClosureAdjustmentConfig;
use crate::core::Command;
use crate::models::{
    gps_to_local, ClosureStrategy, DatumRole, GpsCoordinates, LineId, Project, StationKey,
    StationRef, SurveyDirection,
};

/// `lsq.rs`'s `HARD_FIX_ACCURACY_M`.
const HARD_FIX_ACCURACY_M: f64 = 1e-3;

/// Per axis, for a station the solve holds: stations of one line may
/// disagree on its translation by 1e-6 m before the solve refuses.
const HELD_TOL_M: f64 = 1e-6 + 1e-8;

/// Raw coordinates an undo, a redo or a reopen must restore.
const EXACT_M: f64 = 1e-9;

/// Failure classes that are open bugs, muted unless
/// `STALAGMIGHT_DATUM_FUZZ_MUTE` is set. The replay after each reaches it
/// with the generators as they are (a generator change can turn it into a
/// different chain). None today.
///
/// No longer muted, and pinned in `multi_datum_tests.rs`: `promoted-twin-gps`
/// (a fix with the bit-identical GPS of the removed placement datum) and
/// `datum-key-reused` (Delete Shot re-anchoring a fix onto the removed
/// datum's key, or a combine re-keying one of two twins, a placement and a
/// fix with one GPS, onto the key of the other), both fixed in
/// `DatumLayout::was_unplaced`; and
/// `datum-twin-station` (Combine Lines joining two stations that each
/// hold a datum), now refused; and `hard-fix-no-unknowns` (a hard fix in a
/// component with no unknown left), now applied; and `frame-gps-edit` (a new
/// GPS on the frame datum left every other placed piece off its own GPS,
/// #769), now moved by the frame correction in
/// `Project::place_pieces_on_frame`.
const KNOWN_OPEN: &[&str] = &[];

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn mute_list() -> Vec<String> {
    match std::env::var("STALAGMIGHT_DATUM_FUZZ_MUTE") {
        Ok(list) => list
            .split(',')
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .collect(),
        Err(_) => KNOWN_OPEN.iter().map(|m| (*m).to_string()).collect(),
    }
}

// ---------------------------------------------------------------------------
// The state a step must keep or restore
// ---------------------------------------------------------------------------

/// A datum as compared: its station, GPS bits, accuracy bits and role, and
/// its key (for the promotion rule, never compared).
#[derive(Debug, Clone)]
struct DatumRow {
    at: Option<StationRef>,
    gps: [u64; 3],
    accuracy: Option<u64>,
    role: Option<DatumRole>,
    key: StationKey,
}

impl PartialEq for DatumRow {
    fn eq(&self, other: &Self) -> bool {
        (self.at, self.gps, self.accuracy, self.role)
            == (other.at, other.gps, other.accuracy, other.role)
    }
}

/// Everything a step must keep or restore, in comparable form.
#[derive(Debug, Clone)]
struct Snap {
    lines: Vec<(LineId, usize)>,
    datums: Vec<DatumRow>,
    /// Resolved merge pairs, each ordered, sorted.
    merges: Vec<(StationRef, StationRef)>,
    /// `(id, ends, aligned)`, sorted by id.
    closures: Vec<(u64, StationRef, StationRef, bool)>,
    raw: BTreeMap<StationRef, (f64, f64, f64)>,
}

fn gps_bits(g: &GpsCoordinates) -> [u64; 3] {
    [
        g.latitude.to_bits(),
        g.longitude.to_bits(),
        g.depth_reference.to_bits(),
    ]
}

fn ordered(a: StationRef, b: StationRef) -> (StationRef, StationRef) {
    if (a.line_id, a.station_index) <= (b.line_id, b.station_index) {
        (a, b)
    } else {
        (b, a)
    }
}

fn snap(p: &Project) -> Snap {
    let datums = p
        .zero_datums
        .iter()
        .zip(p.datum_roles())
        .map(|(zd, role)| DatumRow {
            at: p.resolve_key(&zd.station_ref),
            gps: gps_bits(&zd.gps),
            accuracy: zd.accuracy_m.map(f64::to_bits),
            role,
            key: zd.station_ref.clone(),
        })
        .collect();
    let mut merges: Vec<_> = p
        .station_merges
        .iter()
        .filter_map(|m| {
            Some(ordered(
                p.resolve_key(&m.station_a)?,
                p.resolve_key(&m.station_b)?,
            ))
        })
        .collect();
    merges.sort_by_key(|(a, b)| (a.line_id, a.station_index, b.line_id, b.station_index));
    let mut closures: Vec<_> = p
        .loop_closures
        .iter()
        .filter_map(|c| {
            let (a, b) = ordered(p.resolve_key(&c.station_a)?, p.resolve_key(&c.station_b)?);
            Some((c.id, a, b, c.is_aligned))
        })
        .collect();
    closures.sort_by_key(|c| c.0);
    Snap {
        lines: p.lines.iter().map(|s| (s.id, s.shots.len())).collect(),
        datums,
        merges,
        closures,
        raw: p
            .compute_station_coordinates()
            .into_iter()
            .map(|s| {
                let c = s.coordinates;
                (s.station_ref, (c.x, c.y, c.z))
            })
            .collect(),
    }
}

/// The first difference between `want` and `got`, if any.
fn diff(want: &Snap, got: &Snap) -> Option<String> {
    if want.lines != got.lines {
        return Some(format!("lines {:?} -> {:?}", want.lines, got.lines));
    }
    if want.datums != got.datums {
        return Some(format!("datums {:?} -> {:?}", want.datums, got.datums));
    }
    if want.merges != got.merges {
        return Some(format!("merges {:?} -> {:?}", want.merges, got.merges));
    }
    if want.closures != got.closures {
        return Some(format!(
            "closures {:?} -> {:?}",
            want.closures, got.closures
        ));
    }
    for (at, &(x, y, z)) in &want.raw {
        let Some(&(gx, gy, gz)) = got.raw.get(at) else {
            return Some(format!("{at:?} is gone"));
        };
        if (gx - x).abs() > EXACT_M || (gy - y).abs() > EXACT_M || (gz - z).abs() > EXACT_M {
            return Some(format!("{at:?} ({x}, {y}, {z}) -> ({gx}, {gy}, {gz})"));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// The computed view
// ---------------------------------------------------------------------------

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

/// The solve's constraint groups (stations tied by a merge or a closure the
/// solve uses), the roots it holds (placement datums first, then each hard
/// fix's group in list order unless held already), and the hard fixes that
/// hold theirs with their GPS in local metres. Derived from
/// `adjust_least_squares`.
fn held_groups(p: &Project) -> (Groups, HashSet<StationRef>, Vec<(StationRef, (f64, f64))>) {
    let skipped: HashSet<u64> = p
        .closures_across_one_shot()
        .into_iter()
        .map(|c| c.closure_id)
        .collect();
    let mut groups = Groups::default();
    for m in &p.station_merges {
        if let (Some(a), Some(b)) = (p.resolve_key(&m.station_a), p.resolve_key(&m.station_b)) {
            groups.union(a, b);
        }
    }
    for c in &p.loop_closures {
        let eligible = matches!(
            c.strategy,
            ClosureStrategy::Batch | ClosureStrategy::LeastSquaresWeighted
        );
        if !eligible || skipped.contains(&c.id) {
            continue;
        }
        if let (Some(a), Some(b)) = (p.resolve_key(&c.station_a), p.resolve_key(&c.station_b)) {
            groups.union(a, b);
        }
    }
    let mut held: HashSet<StationRef> = p
        .placement_datum_refs()
        .into_iter()
        .map(|sr| groups.find(sr))
        .collect();
    let mut fixes = Vec::new();
    let Some(frame) = p.frame_datum().map(|zd| zd.gps) else {
        return (groups, held, fixes);
    };
    for zd in p.gps_fix_datums() {
        let Some(at) = p.resolve_key(&zd.station_ref) else {
            continue;
        };
        let accuracy = match zd.effective_accuracy_m() {
            a if a.is_finite() && a >= 0.0 => a,
            _ => crate::models::DEFAULT_GPS_ACCURACY_M,
        };
        if accuracy < HARD_FIX_ACCURACY_M && held.insert(groups.find(at)) {
            fixes.push((at, gps_to_local(&zd.gps, &frame)));
        }
    }
    (groups, held, fixes)
}

/// Whether every constraint group of `line`'s component is held.
fn component_all_held(
    p: &Project,
    groups: &mut Groups,
    held: &HashSet<StationRef>,
    line: LineId,
) -> bool {
    let component = p.get_connected_line_ids(line);
    every_station(p)
        .into_iter()
        .filter(|s| component.contains(&s.line_id))
        .all(|s| held.contains(&groups.find(s)))
}

/// A computed view: positions, and the lines of failed components.
struct View {
    at: HashMap<StationRef, (f64, f64, f64)>,
    failed: HashSet<LineId>,
}

fn solve(p: &Project) -> View {
    let solved = p.compute_adjusted_stations(&ClosureAdjustmentConfig::default());
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
            .flat_map(|f| f.line_ids.iter().copied())
            .collect(),
    }
}

/// `p` without its GPS fixes (no other role changes: a fix is never first).
fn without_fixes(p: &Project) -> Project {
    let mut stripped = p.clone();
    let roles = p.datum_roles();
    let mut role = roles.iter();
    stripped
        .zero_datums
        .retain(|_| role.next().copied().flatten() != Some(DatumRole::GpsFix));
    stripped
}

// ---------------------------------------------------------------------------
// A chain
// ---------------------------------------------------------------------------

/// A failure: its class, then what was seen.
struct Failure {
    class: &'static str,
    detail: String,
}

fn fail(class: &'static str, detail: String) -> Failure {
    Failure { class, detail }
}

/// What the sweep counts.
#[derive(Default)]
struct Stats {
    steps: usize,
    applied: BTreeMap<&'static str, usize>,
    refused: usize,
    frame_changes: usize,
    frame_gps_edits: usize,
    promotions: usize,
    held_fixes: usize,
    placed_merges: usize,
    reopens: usize,
    max_eps: f64,
    muted: BTreeMap<&'static str, usize>,
}

struct Chain {
    state: AppState,
    _dir: tempfile::TempDir,
    path: PathBuf,
    origin: GpsCoordinates,
    /// The state at each undo depth since the setup, and where we are.
    timeline: Vec<Snap>,
    cursor: usize,
    /// Timeline entries below this index predate the last reopen.
    reopened_at: usize,
    gps_seen: Vec<GpsCoordinates>,
    eps: f64,
    reframes: u32,
    verbose: bool,
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

impl Chain {
    /// The project of `seed`: lines, some merged, and one to three datums,
    /// all through commands. Mostly near the equator, where re-framing moves a
    /// placement by well under a micrometre (see the module doc).
    fn new(seed: u64, verbose: bool) -> (Self, Vec<Op>) {
        let mut rng = Rng::new(seed);
        let state = AppState::new().unwrap();
        state.execute(Command::CreateProject {
            name: format!("DatumFuzz{seed}"),
            location: None,
        });
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("chain.stalx");
        let lat = if rng.chance(80) {
            rng.range(-0.01, 0.01)
        } else {
            rng.range(-60.0, 60.0)
        };
        let origin = GpsCoordinates::new(lat, rng.range(-179.0, 179.0), 0.0).unwrap();
        let count = 3 + rng.below(4);
        for i in 0..count {
            let n = 1 + rng.below(5);
            let r = state.execute(Command::CreateLine {
                name: Some(format!("S{i}")),
                surveyor: None,
                date: None,
                color: "#123456".to_string(),
                direction: Some(SurveyDirection::In),
                shots: shots(&mut rng, n),
                cave_section_id: None,
            });
            assert!(r.success, "{:?}", r.error);
        }
        let chain = Self {
            state,
            _dir: dir,
            path,
            origin,
            timeline: Vec::new(),
            cursor: 0,
            reopened_at: 0,
            gps_seen: Vec::new(),
            eps: 0.0,
            reframes: 0,
            verbose,
        };
        // The setup's own steps: merges, then datums.
        let mut setup = Vec::new();
        let p = project(&chain.state);
        let ids: Vec<LineId> = p.lines.iter().map(|s| s.id).collect();
        for (i, &id) in ids.iter().enumerate().skip(1) {
            if rng.chance(60) {
                let other = ids[rng.below(i)];
                setup.push(Op::Merge {
                    a: chain.station_of(&p, other, &mut rng),
                    b: chain.station_of(&p, id, &mut rng),
                });
            }
        }
        for _ in 0..=rng.below(3) {
            let at = rng.pick(&every_station(&p)).unwrap();
            setup.push(datum_op(&mut rng, at));
        }
        (chain, setup)
    }

    fn station_of(&self, p: &Project, id: LineId, rng: &mut Rng) -> StationRef {
        let n = p.get_line(id).map_or(0, |s| s.shots.len());
        let i = if rng.chance(35) { 0 } else { rng.below(n + 1) };
        StationRef::new(id, i as u32)
    }

    fn say(&self, text: &str) {
        if self.verbose {
            eprintln!("{text}");
        }
    }

    /// Placement tolerance after `reframes` re-frames: a re-frame onto a
    /// station off its GPS by `e` leaves every other placement off by up to
    /// its own error plus `e` plus the projection's `eps`, so the bound
    /// doubles per re-frame and gains `eps`, from the offset dust (1e-6 m per
    /// axis) a placement may start with.
    fn placement_tol(&self) -> f64 {
        let k = 2f64.powi(self.reframes.min(40) as i32);
        2e-6 * k + self.eps * (k - 1.0)
    }

    /// Record every GPS the project holds, widening `eps` by the largest
    /// re-frame discrepancy over each new triple.
    fn see_gps(&mut self, p: &Project) {
        for zd in &p.zero_datums {
            let g = zd.gps;
            if !(g.latitude.is_finite() && g.longitude.is_finite())
                || self.gps_seen.iter().any(|s| gps_bits(s) == gps_bits(&g))
            {
                continue;
            }
            self.gps_seen.push(g);
            let all = self.gps_seen.clone();
            for a in &all {
                for b in &all {
                    for c in &all {
                        if gps_bits(a) != gps_bits(&g)
                            && gps_bits(b) != gps_bits(&g)
                            && gps_bits(c) != gps_bits(&g)
                        {
                            continue;
                        }
                        let (cb, ca, ba) =
                            (gps_to_local(c, b), gps_to_local(c, a), gps_to_local(b, a));
                        let e = (cb.0 - (ca.0 - ba.0)).hypot(cb.1 - (ca.1 - ba.1));
                        if e.is_finite() {
                            self.eps = self.eps.max(e);
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The checks
// ---------------------------------------------------------------------------

/// A datum that was not a placement before (a fix, or under a key no datum
/// had) is one now, and shares its GPS (lat/lon bits) with a datum that was
/// a placement before under another key, without being that datum re-keyed
/// (the other one is still there, or this one was a fix):
/// `DatumLayout::was_unplaced` matches datums by GPS as well as by key, so
/// it takes this one for the placement it twins and never places it.
fn promoted_twin(before: &Snap, p: &Project) -> bool {
    p.zero_datums
        .iter()
        .zip(p.datum_roles())
        .filter(|(_, role)| *role == Some(DatumRole::Placement))
        .any(|(zd, _)| {
            let bits = gps_bits(&zd.gps);
            let mine = || before.datums.iter().filter(|d| d.key == zd.station_ref);
            if mine().any(|d| d.role == Some(DatumRole::Placement)) {
                return false;
            }
            let was_fix = mine().any(|d| d.role == Some(DatumRole::GpsFix));
            before.datums.iter().any(|d| {
                d.key != zd.station_ref
                    && d.gps[..2] == bits[..2]
                    && d.role == Some(DatumRole::Placement)
                    && (was_fix || p.zero_datums.iter().any(|now| now.station_ref == d.key))
            })
        })
}

/// Why every placement may legitimately-or-knownly be off in a step: the
/// mechanisms the placement check attributes a failure to, first match wins.
fn placement_mechanism(op: Option<&Op>, before: &Snap, p: &Project) -> Option<&'static str> {
    if promoted_twin(before, p) {
        return Some("promoted-twin-gps");
    }
    let datum_op = matches!(
        op,
        Some(Op::Datum { .. } | Op::Twin { .. } | Op::Accuracy { .. })
    );
    // A command that is not a datum edit left a datum under a key another
    // datum had before: a delete re-anchored a datum onto the key of one it
    // removed, and the settle step, which recognises datums by key, sees
    // nothing change. Told by the GPS or accuracy under that key changing, or
    // by a datum whose key is gone having exactly this one's GPS and accuracy.
    let keys_now: Vec<&StationKey> = p.zero_datums.iter().map(|zd| &zd.station_ref).collect();
    let reused = !datum_op
        && p.zero_datums.iter().any(|zd| {
            let (gps, accuracy) = (gps_bits(&zd.gps), zd.accuracy_m.map(f64::to_bits));
            let same = |d: &DatumRow| d.gps[..2] == gps[..2] && d.accuracy == accuracy;
            before.datums.iter().any(|d| d.key == zd.station_ref)
                && (before
                    .datums
                    .iter()
                    .any(|d| d.key == zd.station_ref && !same(d))
                    || before
                        .datums
                        .iter()
                        .any(|d| !keys_now.contains(&&d.key) && same(d)))
        });
    if reused {
        return Some("datum-key-reused");
    }
    // A datum edit gave the frame datum another GPS: the frame stays on the
    // origin, so every other placed piece is off its own GPS by the edit.
    let frame_before = before.datums.iter().find(|d| d.at.is_some());
    let frame_now = p.frame_datum();
    if let (true, Some(was), Some(now)) = (datum_op, frame_before, frame_now) {
        if was.key == now.station_ref && was.gps[..2] != gps_bits(&now.gps)[..2] {
            return Some("frame-gps-edit");
        }
    }
    None
}

/// The roles re-derived from the merges: union-find over lines.
fn expected_roles(p: &Project) -> Vec<Option<DatumRole>> {
    let mut up: HashMap<LineId, LineId> = HashMap::new();
    fn find(up: &mut HashMap<LineId, LineId>, x: LineId) -> LineId {
        let next = *up.get(&x).unwrap_or(&x);
        if next == x {
            return x;
        }
        let root = find(up, next);
        up.insert(x, root);
        root
    }
    for m in &p.station_merges {
        if let (Some(a), Some(b)) = (p.resolve_key(&m.station_a), p.resolve_key(&m.station_b)) {
            let (ra, rb) = (find(&mut up, a.line_id), find(&mut up, b.line_id));
            if ra != rb {
                up.insert(ra, rb);
            }
        }
    }
    let mut placed = HashSet::new();
    p.zero_datums
        .iter()
        .map(|zd| {
            let at = p.resolve_key(&zd.station_ref)?;
            Some(if placed.insert(find(&mut up, at.line_id)) {
                DatumRole::Placement
            } else {
                DatumRole::GpsFix
            })
        })
        .collect()
}

impl Chain {
    /// Every per-state property. `step` is the forward step that led here and
    /// the state it started from, for attributing a misplaced datum.
    fn check_state(
        &self,
        p: &Project,
        step: Option<(Option<&Op>, &Snap)>,
        stats: &mut Stats,
    ) -> Result<(), Failure> {
        let raw = snap(p).raw;
        for (at, &(x, y, z)) in &raw {
            if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                return Err(fail("nonfinite", format!("raw {at:?} at ({x}, {y}, {z})")));
            }
        }
        let refs: Vec<Option<StationRef>> = p
            .zero_datums
            .iter()
            .map(|zd| p.resolve_key(&zd.station_ref))
            .collect();
        for (i, zd) in p.zero_datums.iter().enumerate() {
            if !(zd.gps.latitude.is_finite() && zd.gps.longitude.is_finite()) {
                return Err(fail("nonfinite", format!("datum {i} GPS {:?}", zd.gps)));
            }
            if refs[i].is_some() && refs[..i].contains(&refs[i]) {
                return Err(fail(
                    "datum-twin-station",
                    format!("datums at {:?}: {:?}", refs[i], p.zero_datums),
                ));
            }
        }
        let roles = p.datum_roles();
        let want = expected_roles(p);
        if roles != want {
            return Err(fail(
                "roles",
                format!("datum_roles {roles:?}, merges say {want:?}"),
            ));
        }
        let messages = self.state.integrity_check().messages;
        if !messages.is_empty() {
            return Err(fail("integrity", format!("{messages:?}")));
        }
        self.check_placements(p, step)?;
        self.check_computed(p, &raw, stats)
    }

    /// Every placement datum on its GPS against the frame (module doc).
    fn check_placements(
        &self,
        p: &Project,
        step: Option<(Option<&Op>, &Snap)>,
    ) -> Result<(), Failure> {
        let Some(frame) = p.frame_datum() else {
            return Ok(());
        };
        let tol = self.placement_tol();
        for (zd, role) in p.zero_datums.iter().zip(p.datum_roles()) {
            if role != Some(DatumRole::Placement) {
                continue;
            }
            let Some(c) = p
                .resolve_key(&zd.station_ref)
                .and_then(|at| p.compute_station_raw_coords(at))
            else {
                continue;
            };
            let (gx, gy) = gps_to_local(&zd.gps, &frame.gps);
            let off = (c.x - gx).hypot(c.y - gy);
            if off > tol {
                let mechanism = step.and_then(|(op, before)| placement_mechanism(op, before, p));
                let class = if let Some(mechanism) = mechanism {
                    mechanism
                } else if zd.station_ref == frame.station_ref {
                    "frame-off-origin"
                } else {
                    "placement-off-gps"
                };
                return Err(fail(
                    class,
                    format!(
                        "{:?} at ({}, {}), its GPS at ({gx}, {gy}): {off} m off (tolerance \
                         {tol} after {} re-frames)",
                        p.resolve_key(&zd.station_ref),
                        c.x,
                        c.y,
                        self.reframes
                    ),
                ));
            }
        }
        Ok(())
    }

    /// The Closures Computed View: finite; placement datums held; hard fixes
    /// held on their GPS (`held_groups`); no depth moved by a fix.
    fn check_computed(
        &self,
        p: &Project,
        raw: &BTreeMap<StationRef, (f64, f64, f64)>,
        stats: &mut Stats,
    ) -> Result<(), Failure> {
        let view = solve(p);
        for (at, &(x, y, z)) in &view.at {
            if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                return Err(fail(
                    "nonfinite",
                    format!("computed {at:?} at ({x}, {y}, {z})"),
                ));
            }
        }
        if !p.gps_fix_datums().is_empty() {
            let bare = solve(&without_fixes(p));
            for (at, &(_, _, z)) in &view.at {
                if view.failed.contains(&at.line_id) || bare.failed.contains(&at.line_id) {
                    continue;
                }
                let bz = bare.at[at].2;
                if (z - bz).abs() > 1e-9 {
                    return Err(fail(
                        "depth-moved",
                        format!("computed {at:?} z {z} with the fixes, {bz} without"),
                    ));
                }
            }
        }
        for at in p.placement_datum_refs() {
            if view.failed.contains(&at.line_id) {
                continue;
            }
            let (rx, ry, _) = raw[&at];
            let (x, y, _) = view.at[&at];
            if (x - rx).abs() > HELD_TOL_M || (y - ry).abs() > HELD_TOL_M {
                return Err(fail(
                    "placement-moved-computed",
                    format!("{at:?} raw ({rx}, {ry}), computed ({x}, {y})"),
                ));
            }
        }
        let (mut groups, held, fixes) = held_groups(p);
        stats.held_fixes += fixes.len();
        for (at, (gx, gy)) in fixes {
            if view.failed.contains(&at.line_id) {
                continue;
            }
            let (x, y, _) = view.at[&at];
            if (x - gx).abs() > HELD_TOL_M || (y - gy).abs() > HELD_TOL_M {
                let class = if component_all_held(p, &mut groups, &held, at.line_id) {
                    "hard-fix-no-unknowns"
                } else {
                    "hard-fix-off-gps"
                };
                return Err(fail(
                    class,
                    format!("{at:?} computed ({x}, {y}), GPS ({gx}, {gy})"),
                ));
            }
        }
        Ok(())
    }

    /// What one forward step may change: a datum command never moves a depth
    /// (topology unchanged), storing or removing a fix never moves raw
    /// geometry, and re-setting a placement datum moves it by its error at
    /// most.
    fn check_step(&self, op: &Op, p0: &Project, s0: &Snap, s1: &Snap) -> Result<(), Failure> {
        let pairs = |s: &Snap| -> Vec<(StationRef, StationRef)> {
            s.closures.iter().map(|c| (c.1, c.2)).collect()
        };
        let same_topology =
            s0.lines == s1.lines && s0.merges == s1.merges && pairs(s0) == pairs(s1);
        let moved_most = || -> f64 {
            s0.raw
                .iter()
                .filter_map(|(at, &(x, y, _))| {
                    let &(nx, ny, _) = s1.raw.get(at)?;
                    Some((nx - x).hypot(ny - y))
                })
                .fold(0.0, f64::max)
        };
        // The station the datum step targets, and the role it had before.
        let target = match op {
            Op::Datum { at, .. } | Op::Twin { at, .. } => Some(*at),
            Op::Accuracy { k, .. } | Op::Remove { k } => s0.datums.get(*k).and_then(|d| d.at),
            _ => None,
        };
        let Some(target) = target else {
            return Ok(());
        };
        if same_topology {
            for (at, &(_, _, z)) in &s0.raw {
                if s1
                    .raw
                    .get(at)
                    .is_some_and(|&(_, _, nz)| nz.to_bits() != z.to_bits())
                {
                    return Err(fail(
                        "depth-moved",
                        format!("{op} moved {at:?} from z {z} to {:?}", s1.raw.get(at)),
                    ));
                }
            }
        }
        let role_before = s0
            .datums
            .iter()
            .find(|d| d.at == Some(target))
            .and_then(|d| d.role);
        let role_after = s1
            .datums
            .iter()
            .find(|d| d.at == Some(target))
            .and_then(|d| d.role);
        let stores_fix = !matches!(op, Op::Remove { .. }) && role_after == Some(DatumRole::GpsFix);
        let removes_fix = matches!(op, Op::Remove { .. })
            && role_before == Some(DatumRole::GpsFix)
            && same_topology;
        if stores_fix || removes_fix {
            let most = moved_most();
            if most > EXACT_M {
                return Err(fail(
                    "fix-moved-raw",
                    format!("{op} on a GPS fix at {target:?} moved raw geometry by {most} m"),
                ));
            }
        }
        if matches!(op, Op::Accuracy { .. }) && role_before == Some(DatumRole::Placement) {
            let most = moved_most();
            if most > self.placement_tol() + EXACT_M {
                return Err(fail(
                    "placement-off-gps",
                    format!(
                        "{op}: re-setting the placement datum at {target:?} with its own GPS \
                         moved the survey by {most} m"
                    ),
                ));
            }
        }
        let _ = p0;
        Ok(())
    }

    /// Note a state change: re-frames and the GPS seen.
    fn note(&mut self, prev: &Snap, now: &Project, stats: &mut Stats) {
        let was = prev.datums.iter().find(|d| d.at.is_some()).map(|d| d.gps);
        let is = now.frame_datum().map(|zd| gps_bits(&zd.gps));
        if was.is_some() && is.is_some() && was != is {
            self.reframes += 1;
            stats.frame_changes += 1;
        }
        let roles = now.datum_roles();
        for (zd, role) in now.zero_datums.iter().zip(&roles) {
            let was_fix = prev
                .datums
                .iter()
                .any(|d| d.key == zd.station_ref && d.role == Some(DatumRole::GpsFix));
            if was_fix && *role == Some(DatumRole::Placement) {
                stats.promotions += 1;
            }
        }
        self.see_gps(now);
        stats.max_eps = stats.max_eps.max(self.eps);
    }

    /// Count a forward step that gave the frame datum another GPS on its own
    /// station while another placement datum placed a piece of its own (#769).
    fn note_frame_gps_edit(prev: &Snap, now: &Project, stats: &mut Stats) {
        let Some(was) = prev.datums.iter().find(|d| d.at.is_some()) else {
            return;
        };
        let other_piece = prev
            .datums
            .iter()
            .filter(|d| d.role == Some(DatumRole::Placement))
            .count()
            > 1;
        let edited = now
            .frame_datum()
            .is_some_and(|zd| zd.station_ref == was.key && gps_bits(&zd.gps)[..2] != was.gps[..2]);
        if other_piece && edited {
            stats.frame_gps_edits += 1;
        }
    }

    fn undo_depth(&self) -> usize {
        self.state.get_undo_stack().len()
    }

    /// One step of the chain, checked.
    fn step(&mut self, op: &Op, stats: &mut Stats) -> Result<(), Failure> {
        stats.steps += 1;
        match op {
            Op::Undo | Op::Redo => self.history(matches!(op, Op::Undo), stats),
            Op::Reopen => self.reopen(stats),
            _ => self.forward(op, stats),
        }
    }

    fn forward(&mut self, op: &Op, stats: &mut Stats) -> Result<(), Failure> {
        let p0 = project(&self.state);
        let s0 = snap(&p0);
        let depth0 = self.undo_depth();
        if let Applied::Refused(why) = apply(&self.state, &self.origin, op, &p0) {
            stats.refused += 1;
            self.say(&format!("  refused: {why}"));
            let s1 = snap(&project(&self.state));
            if let Some(d) = diff(&s0, &s1) {
                return Err(fail(
                    "refusal-mutated",
                    format!("{op} refused ({why}): {d}"),
                ));
            }
            return Ok(());
        }
        *stats
            .applied
            .entry(op.to_string().split('(').next().map_or("?", |n| match n {
                "datum" => "datum",
                "twin" => "twin",
                "acc" => "acc",
                "rm" => "rm",
                "merge" => "merge",
                "unmerge" => "unmerge",
                "closure" => "closure",
                "split" => "split",
                "combine" => "combine",
                "delsec" => "delsec",
                "delshot" => "delshot",
                _ => "addshot",
            }))
            .or_default() += 1;
        if let Op::Merge { a, b } = op {
            let placed: HashSet<LineId> = p0
                .placement_datum_refs()
                .into_iter()
                .flat_map(|sr| p0.get_merged_line_ids(sr.line_id))
                .collect();
            if placed.contains(&a.line_id) && placed.contains(&b.line_id) {
                stats.placed_merges += 1;
            }
        }
        let p1 = project(&self.state);
        let s1 = snap(&p1);
        Self::note_frame_gps_edit(&s0, &p1, stats);
        self.note(&s0, &p1, stats);
        self.show(&p1);
        self.check_step(op, &p0, &s0, &s1)?;
        self.check_state(&p1, Some((Some(op), &s0)), stats)?;
        let depth1 = self.undo_depth();
        if depth1 != depth0 + 1 {
            if let Some(d) = diff(&s0, &s1) {
                return Err(fail(
                    "undo-inexact",
                    format!("{op} changed the project but the undo depth went {depth0} -> {depth1}: {d}"),
                ));
            }
            self.timeline.truncate(self.cursor + 1);
            return Ok(());
        }
        // Undo it at once, then redo it.
        let r = self.state.undo();
        if !r.success {
            return Err(fail(
                "undo-inexact",
                format!("undo of {op} refused: {:?}", r.error),
            ));
        }
        if let Some(d) = diff(&s0, &snap(&project(&self.state))) {
            return Err(fail("undo-inexact", format!("undo of {op}: {d}")));
        }
        let r = self.state.redo();
        if !r.success {
            return Err(fail(
                "redo-inexact",
                format!("redo of {op} refused: {:?}", r.error),
            ));
        }
        if let Some(d) = diff(&s1, &snap(&project(&self.state))) {
            return Err(fail("redo-inexact", format!("redo of {op}: {d}")));
        }
        self.reopened_at = self.reopened_at.min(self.cursor + 1);
        self.timeline.truncate(self.cursor + 1);
        self.timeline.push(s1);
        self.cursor += 1;
        Ok(())
    }

    /// A random undo or redo, against the state recorded at that depth.
    fn history(&mut self, undo: bool, stats: &mut Stats) -> Result<(), Failure> {
        let (target, class) = if undo {
            (self.cursor - 1, "undo-inexact")
        } else {
            (self.cursor + 1, "redo-inexact")
        };
        let class = if target < self.reopened_at && self.reopened_at > 0 && class == "undo-inexact"
        {
            "undo-across-reopen"
        } else {
            class
        };
        let prev = snap(&project(&self.state));
        let r = if undo {
            self.state.undo()
        } else {
            self.state.redo()
        };
        if !r.success {
            return Err(fail(class, format!("refused: {:?}", r.error)));
        }
        let p = project(&self.state);
        let now = snap(&p);
        if let Some(d) = diff(&self.timeline[target], &now) {
            return Err(fail(class, d));
        }
        self.cursor = target;
        self.note(&prev, &p, stats);
        self.show(&p);
        self.check_state(&p, None, stats)
    }

    /// Save, close and reopen: nothing may change.
    fn reopen(&mut self, stats: &mut Stats) -> Result<(), Failure> {
        let p0 = project(&self.state);
        let s0 = snap(&p0);
        let v0 = solve(&p0);
        let saved = self.state.execute(Command::SaveProject {
            path: Some(self.path.clone()),
        });
        if !saved.success {
            return Err(fail(
                "reopen-drift",
                format!("save refused: {:?}", saved.error),
            ));
        }
        let closed = self.state.execute(Command::CloseProject);
        if !closed.success {
            return Err(fail(
                "reopen-drift",
                format!("close refused: {:?}", closed.error),
            ));
        }
        let opened = self.state.open_project(self.path.clone());
        if !opened.success {
            return Err(fail(
                "reopen-drift",
                format!("open refused: {:?}", opened.error),
            ));
        }
        let p1 = project(&self.state);
        let s1 = snap(&p1);
        if let Some(d) = diff(&s0, &s1) {
            let class = if s0
                .datums
                .iter()
                .enumerate()
                .any(|(i, d)| d.at.is_some() && s0.datums[..i].iter().any(|e| e.at == d.at))
            {
                "datum-twin-station"
            } else {
                "reopen-drift"
            };
            return Err(fail(class, d));
        }
        let v1 = solve(&p1);
        for (at, &(x, y, z)) in &v0.at {
            let Some(&(nx, ny, nz)) = v1.at.get(at) else {
                return Err(fail("reopen-drift", format!("computed {at:?} is gone")));
            };
            if (nx - x).abs() > 1e-6 || (ny - y).abs() > 1e-6 || (nz - z).abs() > 1e-6 {
                return Err(fail(
                    "reopen-drift",
                    format!("computed {at:?} ({x}, {y}, {z}) -> ({nx}, {ny}, {nz})"),
                ));
            }
        }
        self.reopened_at = self.timeline.len();
        stats.reopens += 1;
        self.note(&s0, &p1, stats);
        self.check_state(&p1, None, stats)
    }

    /// In a replay, the datums after a step: role, station, and how far from
    /// its GPS the raw survey puts it.
    fn show(&self, p: &Project) {
        if !self.verbose {
            return;
        }
        let frame = p.frame_datum().map(|zd| zd.gps);
        for (zd, role) in p.zero_datums.iter().zip(p.datum_roles()) {
            let at = p.resolve_key(&zd.station_ref);
            let raw = at.and_then(|a| p.compute_station_raw_coords(a));
            let gps = frame.map(|f| gps_to_local(&zd.gps, &f));
            eprintln!(
                "    {role:?} at {at:?} accuracy {:?}: raw {:?}, GPS {gps:?}",
                zd.accuracy_m,
                raw.map(|c| (c.x, c.y, c.z))
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Running chains
// ---------------------------------------------------------------------------

/// Run chain `seed`: its setup, then `script` or `steps` random steps.
/// `trail` holds the steps run so far, for the failure message.
fn run_chain(
    seed: u64,
    steps: u64,
    script: Option<&[Op]>,
    verbose: bool,
    stats: &mut Stats,
    trail: &mut Vec<String>,
) -> Result<(), Failure> {
    let (mut chain, setup) = Chain::new(seed, verbose);
    for op in &setup {
        let p = project(&chain.state);
        chain.say(&format!("setup {op}"));
        if let Applied::Refused(why) = apply(&chain.state, &chain.origin, op, &p) {
            chain.say(&format!("  refused: {why}"));
            continue;
        }
        let p1 = project(&chain.state);
        Chain::note_frame_gps_edit(&snap(&p), &p1, stats);
        chain.note(&snap(&p), &p1, stats);
        chain.show(&p1);
        chain
            .check_state(&p1, Some((Some(op), &snap(&p))), stats)
            .map_err(|f| Failure {
                class: f.class,
                detail: format!("in the setup, after {op}: {}", f.detail),
            })?;
    }
    chain.timeline = vec![snap(&project(&chain.state))];
    let mut rng = Rng::new(seed ^ 0xD1B5_4A32_D192_ED03);
    let script: Vec<Op> = match script {
        Some(ops) => ops.to_vec(),
        None => Vec::new(),
    };
    let total = if script.is_empty() {
        steps as usize
    } else {
        script.len()
    };
    for i in 0..total {
        let op = if let Some(op) = script.get(i) {
            op.clone()
        } else {
            let p = project(&chain.state);
            let (can_undo, can_redo) = (chain.cursor > 0, chain.cursor + 1 < chain.timeline.len());
            match (0..8).find_map(|_| next_op(&mut rng, &p, &chain.state, can_undo, can_redo)) {
                Some(op) => op,
                None => continue,
            }
        };
        trail.push(op.to_string());
        chain.say(&format!("step {i}: {op}"));
        chain.step(&op, stats)?;
    }
    Ok(())
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .unwrap_or_default()
}

/// Run one chain, catching a panic. `Err` is the failure message, its class
/// first, with the replay.
fn chain_outcome(
    seed: u64,
    steps: u64,
    script: Option<&[Op]>,
    verbose: bool,
    mute: &[String],
    stats: &mut Stats,
) -> Result<(), String> {
    let mut trail: Vec<String> = Vec::new();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_chain(seed, steps, script, verbose, stats, &mut trail)
    }));
    let failure = match outcome {
        Ok(Ok(())) => return Ok(()),
        Ok(Err(f)) => f,
        Err(payload) => fail("panic", panic_text(payload.as_ref())),
    };
    if mute.iter().any(|m| m == failure.class) {
        *stats.muted.entry(failure.class).or_default() += 1;
        return Ok(());
    }
    let replay = std::iter::once(seed.to_string())
        .chain(trail.iter().cloned())
        .collect::<Vec<_>>()
        .join(";");
    Err(format!(
        "{}: {} [datum fuzz seed {seed}, step {}: {}] replay: \
         STALAGMIGHT_DATUM_FUZZ_REPLAY=\"{replay}\"",
        failure.class,
        failure.detail,
        trail.len().saturating_sub(1),
        trail.last().map_or("setup", String::as_str)
    ))
}

/// Random chains of datum, merge, split, combine, delete, add, undo, redo
/// and reopen steps keep every invariant in the module doc.
#[test]
fn datum_command_chains_keep_every_invariant() {
    let seeds = env_u64("STALAGMIGHT_DATUM_FUZZ_SEEDS", 40);
    let first = env_u64("STALAGMIGHT_DATUM_FUZZ_FIRST_SEED", 0);
    let steps = env_u64("STALAGMIGHT_DATUM_FUZZ_STEPS", 12);
    let keep_going = std::env::var("STALAGMIGHT_DATUM_FUZZ_KEEP_GOING").is_ok_and(|v| v == "1");
    let mute = mute_list();
    let mut stats = Stats::default();
    let mut failed: Vec<String> = Vec::new();
    for seed in first..first + seeds {
        if let Err(e) = chain_outcome(seed, steps, None, false, &mute, &mut stats) {
            assert!(keep_going, "{e}");
            eprintln!("FAIL {e}");
            failed.push(e);
        }
    }
    eprintln!(
        "datum fuzz seeds {first}..{} x {steps} steps: {} steps, applied {:?}, {} refused, {} \
         merges of two placed pieces, {} reopens, {} re-frames, {} frame GPS edits with \
         another placed piece, {} promotions, {} hard fixes held, largest re-frame \
         discrepancy {:.3e} m, muted {:?} (mute list {mute:?})",
        first + seeds,
        stats.steps,
        stats.applied,
        stats.refused,
        stats.placed_merges,
        stats.reopens,
        stats.frame_changes,
        stats.frame_gps_edits,
        stats.promotions,
        stats.held_fixes,
        stats.max_eps,
        stats.muted
    );
    if seeds >= 20 {
        assert!(stats.placed_merges > 0, "no merge of two placed pieces");
        assert!(
            stats.promotions > 0 && stats.reopens > 0,
            "no promotion or no reopen"
        );
        assert!(
            stats.frame_gps_edits > 0,
            "no frame GPS edit with another placed piece"
        );
    }
    assert!(
        failed.is_empty(),
        "{} failing chains, first: {}",
        failed.len(),
        failed[0]
    );
}

/// Chains the farm or a local sweep once failed on, replayed on every run so
/// the fixed mechanism stays fixed (format: `STALAGMIGHT_DATUM_FUZZ_REPLAY`).
const PINNED_CHAINS: &[&str] = &[
    // datum-key-reused: Delete Shot re-anchors a fix onto the removed datum's key.
    "54;delshot(1,0)",
    // promoted-twin-gps: a fix with the removed placement datum's exact GPS.
    "701;twin(0:3,1,0.0);rm(0)",
    // datum-key-reused with a twin GPS: only the station position tells them apart.
    "1138;merge(0:1,1:4);acc(0,-);unmerge(2:2,3:1);delsec(1);combine(3:start,4:start);delsec(2);acc(0,-);split(3,2,b);datum(3:1,20.953755677079684,-16.914921358435627,5.0);merge(5:0,0:1);datum(5:0,16.04357755301331,-1.7832103357017155,0.5);merge(5:0,0:3);addshot(0,-1,44.39334284186008,6.11431610947411,1.0519934908613826,0.0);merge(0:2,3:1);reopen;twin(5:1,2,1000.0);split(0,1,b);unmerge(5:0,6:1);unmerge(6:1,3:1);rm(0);delshot(5,0)",
    // promoted-twin-gps after a combine that re-keys and moves both twins: only
    // the stored accuracy tells them apart.
    "1026755;closure(3:2,3:3);acc(0,1000.0);undo;closure(4:1,0:0);split(3,2,a);twin(5:0,1,-);acc(0,0.0);closure(2:2,1:2);addshot(5,0,83.26639828096893,9.85053026440123,28.642278304656077,28.880991248958054);unmerge(0:0,3:0);unmerge(1:0,2:1);addshot(6,-1,255.74044991586703,19.905929536886184,1.2756380852736882,0.0);datum(4:3,24.901653430245844,29.355911207344825,0.0);merge(0:0,2:1);rm(2);datum(1:2,-45.864378812639124,11.359789645079355,50.0);combine(5:start,4:start)",
    // datum-key-reused: a combine re-keys a placement datum onto the key of
    // its twin fix (one GPS, another accuracy), which becomes the placement:
    // only the stored accuracy shows the fix's old key is gone.
    "50028990;split(4,2,b);acc(0,0.0);datum(3:4,-10.355085626613327,-38.63552075781107,1000.0);datum(2:2,45.046864107381886,43.49016961757878,50.0);addshot(0,end,241.82506602311437,12.181044570237113,10.15784749770614,6.664029904480876);datum(3:3,-28.281957111543417,-33.37293276249461,0.5);reopen;unmerge(0:0,1:0);undo;split(0,1,b);acc(3,50.0);datum(5:0,0.0,0.0,50.0);reopen;undo;delshot(3,4);acc(2,-);undo;twin(4:0,1,5.0);closure(4:2,6:0);merge(4:2,3:0);reopen;combine(0:start,2:start);twin(5:1,4,0.5);datum(0:3,-18.54270846695945,37.349707765143265,0.5);split(1,1,b);closure(3:0,3:0);undo;undo;rm(0);datum(0:0,-70.10296390335284,-51.46255498275887,5.0);undo;datum(6:0,11.025626281126955,2.450822161617217,50.0);closure(4:1,0:4);datum(4:2,27.224552254542683,-43.394823892024185,5.0);closure(4:0,5:0);addshot(6,0,93.8038739660679,10.717179406412239,28.31985522332138,25.400500444955643);merge(3:3,0:4);twin(3:0,5,1000.0);rm(0);combine(4:start,5:start)",
    // datum-key-reused: a combine re-keys a fix onto the key of its twin
    // placement datum (one GPS, another accuracy) and the fix becomes the
    // placement: only the stored accuracy says the twin is not the fix.
    "1294806;unmerge(0:1,2:1);reopen;datum(1:0,25.650794071477637,-73.60666169426631,50.0);merge(1:0,3:1);datum(3:3,0.0,0.0,5.0);datum(2:0,19.994246212841716,-36.50338157646604,-);rm(4);twin(2:0,0,-);merge(1:0,3:5);unmerge(1:0,3:1);addshot(3,1,169.26310033886833,19.806523673505293,9.60276030515832,6.003958612032227);rm(0);twin(3:0,3,0.0);split(3,5,a);combine(4:start,3:end);acc(0,-);split(4,5,b);datum(4:2,33.880412148064,5.378012870132235,0.5);undo;delshot(1,0);undo;datum(4:1,-73.71064559461756,42.86751369073772,5.0);datum(5:1,0.27217256649528937,5.768254987005169,-);split(4,2,b);combine(4:start,2:end)",
    // datum-twin-station: Combine joining two datum stations is refused.
    "350;combine(0:start,3:start)",
    // hard-fix-no-unknowns: every group held, the hard fix still applied.
    "112",
    // frame-gps-edit: the setup gives the frame datum a new GPS while another
    // datum places a piece of its own (#769).
    "11",
];

#[test]
fn pinned_datum_chains_keep_every_invariant() {
    for spec in PINNED_CHAINS {
        let mut parts = spec.split(';').filter(|s| !s.trim().is_empty());
        let seed: u64 = parts.next().unwrap().trim().parse().unwrap();
        let ops: Vec<Op> = parts.map(parse_op).collect();
        let mut stats = Stats::default();
        if let Err(e) = chain_outcome(seed, 0, Some(&ops), false, &[], &mut stats) {
            panic!("pinned chain {seed}: {e}");
        }
    }
}

/// Replay one chain from its trail, printing each step and the datums after
/// it: `STALAGMIGHT_DATUM_FUZZ_REPLAY="seed;op;op;..."`.
#[test]
#[ignore = "replays one chain: STALAGMIGHT_DATUM_FUZZ_REPLAY=\"seed;op;op;...\""]
fn replay_a_datum_chain() {
    let spec = std::env::var("STALAGMIGHT_DATUM_FUZZ_REPLAY").unwrap();
    let mut parts = spec.split(';').filter(|s| !s.trim().is_empty());
    let seed: u64 = parts.next().unwrap().trim().parse().unwrap();
    let ops: Vec<Op> = parts.map(parse_op).collect();
    let mut stats = Stats::default();
    if let Err(e) = chain_outcome(seed, 0, Some(&ops), true, &[], &mut stats) {
        panic!("{e}");
    }
}
