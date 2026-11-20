//! Chains of random commands on surveys with several zero datums (#741),
//! through the real command path on the station graph: `AppState::execute`,
//! the split and combine recipes, undo, redo, and a save-and-reopen round
//! trip. Synthetic surveys only.
//!
//! A chain starts from a random project built by commands (lines, some
//! junctions, one to three datums), then runs random steps: set a datum
//! (placement or fix, its GPS 0-100 m from where the survey puts the
//! station; on a station that holds one, a GPS edit), set a datum with
//! another datum's GPS bit for bit, change only an accuracy, remove a datum
//! (the frame on purpose now and then), merge (two separately placed pieces
//! on purpose now and then), detach a line from a junction, add a loop
//! closure, split, Combine Lines through an offered candidate, delete a line
//! or a shot, add a shot, Move Line, undo, redo, reopen.
//!
//! After every state change, each failure message starting with its class:
//!
//! - `nonfinite`: a raw or computed coordinate, or a datum's GPS, is not
//!   finite.
//! - `integrity`: `validate` or `integrity_check` reports anything.
//! - `roles`: `datum_roles` disagrees with a re-derivation from the line
//!   chains (the first datum of each shot component in list order places it,
//!   every later one is a fix).
//! - `correction`: a GPS fix carries a correction (demotion clears it), or a
//!   stored correction is below `CORRECTION_EPSILON_M` or not finite (plan
//!   4.3: those are stored as none).
//! - `placement-off-anchor`: a placement datum's station is not exactly at
//!   `gps_to_local(its GPS, the frame's GPS) + correction` (the frame at its
//!   correction).
//! - `placement-off-gps` / `frame-off-origin`: a placement datum's correction
//!   (how far its piece sits from its own GPS) exceeds what re-framing can
//!   leave: a re-frame translates by the new frame's measured position, which
//!   the local tangent plane makes differ from the new frame's own projection
//!   by a small amount that grows with each re-frame. The tolerance is
//!   `2e-6 * 2^k + eps * (2^k - 1)`: `eps` the largest such discrepancy over
//!   every GPS the chain has seen, `k` the re-frames so far.
//! - `frame-change-moved`: a frame change moved one station relative to
//!   another (plan 4.3: the survey translates rigidly, the new frame station
//!   to the origin), or removing a placement datum that held its piece alone
//!   moved anything.
//! - `hard-fix-off-gps` / `hard-fix-no-unknowns`: in the Closures Computed
//!   View, a hard fix the solve holds is not on its GPS (rule derived from
//!   `adjust.rs`: constraint groups over the closures the solve ties,
//!   placement datums held first, then each hard fix's group in list order).
//!   `-no-unknowns`: its component has no unknown left.
//! - `placement-moved-computed`: a placement datum moved in the computed view.
//! - `depth-moved`: a datum command changed a depth (topology unchanged), or
//!   a fix changed a computed depth (compared with the same view with every
//!   fix removed).
//! - `fix-moved-raw`: storing or removing a GPS fix, or an accuracy-only
//!   edit, moved raw geometry.
//! - `undo-inexact` / `redo-inexact`: an undo or a redo did not land on the
//!   state it should, to the bit (the canonical v2 text; the id counters may
//!   stand higher), with every shot's session id back in the session. Every
//!   forward step is undone and redone at once; random undo and redo steps go
//!   deeper. `undo-across-reopen`: the same, for a state from before a reopen.
//! - `reopen-drift`: save and reopen changed the canonical text, the raw
//!   layout or the computed view (bit for bit).
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
//!
//! Classes the SQLite build had and this one does not: `datum-twin-station`
//! (two datums on one station: the station graph allows it, plan 5.7, S8,
//! E1); `promoted-twin-gps`, `datum-key-reused` and `frame-gps-edit` were
//! mechanism labels of `placement-off-gps` for datums re-keyed by
//! `StationKey` (gone: a datum is on a station id, D5), all fixed before the
//! switch.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use super::multi_datum_steps_fuzz::{
    apply, datum_op, every_station, next_op, parse_op, shots, station_at, Applied, Op, Rng,
};
use super::AppState;
use crate::config::ClosureAdjustmentConfig;
use crate::core::Command;
use crate::models::survey::{Project, RawPositions, CORRECTION_EPSILON_M};
use crate::models::{
    gps_to_local, DatumRole, GpsCoordinates, LineId, StationId, StationRef, SurveyDirection,
    DEFAULT_GPS_ACCURACY_M,
};
use crate::persistence::stalx_v2::{self, Extras, Mode};

/// `adjust.rs`'s `HARD_FIX_ACCURACY_M`.
const HARD_FIX_ACCURACY_M: f64 = 1e-3;

/// Raw coordinates a re-anchoring must keep (a placement laid out from
/// another station of its piece sums the same shots in another order).
const EXACT_M: f64 = 1e-9;

/// Failure classes that are open bugs, muted unless
/// `STALAGMIGHT_DATUM_FUZZ_MUTE` is set. None.
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

/// A datum as compared, bit for bit.
#[derive(Debug, Clone, PartialEq)]
struct DatumRow {
    station: StationId,
    at: Option<StationRef>,
    gps: [u64; 3],
    accuracy: Option<u64>,
    role: Option<DatumRole>,
    correction: Option<[u64; 2]>,
}

/// Everything a step must keep or restore.
#[derive(Debug, Clone)]
struct Snap {
    /// The canonical v2 text (`modified_at` and the path masked).
    text: String,
    /// Every shot's session id, line by line.
    ids: Vec<Vec<u32>>,
    lines: Vec<(LineId, usize)>,
    datums: Vec<DatumRow>,
    /// Every station's `[x, y, z]` (raw), ascending.
    xyz: BTreeMap<StationId, [f64; 3]>,
    /// Closures `(id, a, b)`.
    closures: Vec<(u64, StationId, StationId)>,
    /// Junctions: each station's lines, for the stations on two or more.
    junctions: BTreeMap<StationId, Vec<LineId>>,
}

/// Equal to the bit, except that `0.0` and `-0.0` are the same place.
fn same(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a == 0.0 && b == 0.0)
}

fn gps_bits(g: &GpsCoordinates) -> [u64; 3] {
    [
        g.latitude.to_bits(),
        g.longitude.to_bits(),
        g.depth_reference.to_bits(),
    ]
}

fn canonical(p: &Project) -> String {
    let mut p = p.clone();
    p.modified_at = p.created_at;
    p.file_path = None;
    String::from_utf8(stalx_v2::encode(&p, &Extras::default(), Mode::Normal).unwrap()).unwrap()
}

fn snap(p: &Project) -> Snap {
    let raw = p.raw_positions();
    let roles = p.datum_roles();
    let topology = p.topology();
    Snap {
        text: canonical(p),
        ids: p
            .lines()
            .iter()
            .map(|l| l.shots.iter().map(|s| s.id).collect())
            .collect(),
        lines: p.lines().iter().map(|l| (l.id, l.shots.len())).collect(),
        datums: p
            .datums
            .iter()
            .zip(roles)
            .map(|(d, role)| DatumRow {
                station: d.station,
                at: p.datum_slot(d),
                gps: gps_bits(&d.gps),
                accuracy: d.accuracy_m.map(f64::to_bits),
                role,
                correction: d.correction.map(|c| [c[0].to_bits(), c[1].to_bits()]),
            })
            .collect(),
        xyz: raw.iter().collect(),
        closures: p.closures.iter().map(|c| (c.id, c.a, c.b)).collect(),
        junctions: p
            .stations
            .keys()
            .filter(|s| topology.is_junction(**s))
            .map(|s| (*s, topology.lines_of(*s)))
            .collect(),
    }
}

/// Whether two canonical texts are equal except that the id counters may
/// stand higher in `got` (plan 8.1: a step applies them as
/// `max(live, recorded)`).
fn same_up_to_counters(want: &str, got: &str) -> Option<String> {
    let (w, g): (Vec<&str>, Vec<&str>) = (want.lines().collect(), got.lines().collect());
    if w.len() != g.len() {
        return Some(format!("{} line(s) -> {}", w.len(), g.len()));
    }
    for (i, (w, g)) in w.iter().zip(&g).enumerate() {
        if w == g {
            continue;
        }
        if w.starts_with("\"counters\":") && g.starts_with("\"counters\":") {
            let parse = |line: &str| -> BTreeMap<String, u64> {
                serde_json::from_str(line.trim_start_matches("\"counters\":").trim_end_matches(','))
                    .unwrap_or_default()
            };
            let (wc, gc) = (parse(w), parse(g));
            if wc.keys().eq(gc.keys()) && wc.iter().all(|(k, v)| gc[k] >= *v) {
                continue;
            }
        }
        return Some(format!("line {}: {w} -> {g}", i + 1));
    }
    None
}

/// The first difference between `want` and `got` (to the bit), if any;
/// `ids` also compares the session shot ids.
fn diff(want: &Snap, got: &Snap, ids: bool) -> Option<String> {
    if let Some(d) = same_up_to_counters(&want.text, &got.text) {
        if want.datums != got.datums {
            return Some(format!("datums {:?} -> {:?} ({d})", want.datums, got.datums));
        }
        return Some(d);
    }
    if ids && want.ids != got.ids {
        return Some(format!("session shot ids {:?} -> {:?}", want.ids, got.ids));
    }
    None
}

// ---------------------------------------------------------------------------
// The computed view
// ---------------------------------------------------------------------------

/// A computed view: positions per station, and the lines of failed
/// components.
struct View {
    at: BTreeMap<StationId, [f64; 3]>,
    failed: BTreeSet<LineId>,
}

fn solve(p: &Project) -> View {
    let solved = p.adjusted_positions(&ClosureAdjustmentConfig::default());
    View {
        at: solved.iter().collect(),
        failed: solved
            .failures()
            .iter()
            .flat_map(|f| f.lines.iter().copied())
            .collect(),
    }
}

/// Union-find over station ids.
#[derive(Default)]
struct Groups(BTreeMap<StationId, StationId>);

impl Groups {
    fn find(&mut self, x: StationId) -> StationId {
        let up = *self.0.get(&x).unwrap_or(&x);
        if up == x {
            return x;
        }
        let root = self.find(up);
        self.0.insert(x, root);
        root
    }

    fn union(&mut self, a: StationId, b: StationId) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0.insert(ra, rb);
        }
    }
}

/// The solve's constraint groups (stations tied by the closures the solve
/// ties: eligible ones and aligned snaps, minus the across-one-shot skip
/// set), the groups it holds (placement datums first, then each hard fix's
/// group in list order unless held already), and the hard fixes that hold
/// theirs with their GPS in local metres. Derived from `adjust.rs`.
fn held_groups(
    p: &Project,
    raw: &RawPositions,
) -> (Groups, BTreeSet<StationId>, Vec<(StationId, [f64; 2])>) {
    let aligned = p.closure_alignment(raw);
    let skipped: BTreeSet<u64> = p
        .closures_across_one_shot_with(&aligned)
        .into_iter()
        .map(|c| c.closure_id)
        .collect();
    let mut groups = Groups::default();
    for (i, c) in p.closures.iter().enumerate() {
        if skipped.contains(&c.id)
            || !Project::closure_ties_in_solve(c, aligned.get(i).copied().unwrap_or(false))
        {
            continue;
        }
        groups.union(c.a, c.b);
    }
    let roles = p.datum_roles();
    let mut held: BTreeSet<StationId> = BTreeSet::new();
    for (d, role) in p.datums.iter().zip(&roles) {
        if *role == Some(DatumRole::Placement) {
            held.insert(groups.find(d.station));
        }
    }
    let mut fixes = Vec::new();
    let Some(frame) = p.frame_datum().and_then(|f| p.datums.get(f)).map(|d| d.gps) else {
        return (groups, held, fixes);
    };
    for (d, role) in p.datums.iter().zip(&roles) {
        if *role != Some(DatumRole::GpsFix) {
            continue;
        }
        let accuracy = match d.accuracy_m.unwrap_or(DEFAULT_GPS_ACCURACY_M) {
            a if a.is_finite() && a >= 0.0 => a,
            _ => DEFAULT_GPS_ACCURACY_M,
        };
        if accuracy < HARD_FIX_ACCURACY_M && held.insert(groups.find(d.station)) {
            let (x, y) = gps_to_local(&d.gps, &frame);
            fixes.push((d.station, [x, y]));
        }
    }
    (groups, held, fixes)
}

/// `p` without its GPS fixes (no other role changes: a fix is never first).
fn without_fixes(p: &Project) -> Project {
    let mut stripped = p.clone();
    let roles = p.datum_roles();
    let mut role = roles.iter();
    stripped
        .datums
        .retain(|_| role.next().copied().flatten() != Some(DatumRole::GpsFix));
    stripped
}

/// The lines of `station`'s shot component.
fn component_lines(p: &Project, station: StationId) -> BTreeSet<LineId> {
    let topology = p.topology();
    topology
        .component_of(station)
        .and_then(|c| topology.components().get(c))
        .map(|c| c.lines.iter().copied().collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// A chain
// ---------------------------------------------------------------------------

struct Failure {
    class: &'static str,
    detail: String,
}

fn fail(class: &'static str, detail: String) -> Failure {
    Failure { class, detail }
}

#[derive(Default)]
struct Stats {
    steps: usize,
    applied: BTreeMap<String, usize>,
    refused: usize,
    skipped: usize,
    frame_changes: usize,
    frame_gps_edits: usize,
    rigid_reframes: usize,
    promotions: usize,
    held_fixes: usize,
    placed_merges: usize,
    reopens: usize,
    corrections: usize,
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
    /// Timeline entries below this index are out of reach: a reopen keeps
    /// the newest `window` entries of each stack (the history limit).
    floor: usize,
    window: usize,
    gps_seen: Vec<GpsCoordinates>,
    eps: f64,
    reframes: u32,
    verbose: bool,
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

impl Chain {
    /// The project of `seed`: lines, some joined, and one to three datums,
    /// all through commands. Mostly near the equator, where re-framing moves
    /// a placement by well under a micrometre.
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
        let window = state.undo_stack.lock().unwrap().history_limit();
        let chain = Self {
            state,
            _dir: dir,
            path,
            origin,
            timeline: Vec::new(),
            cursor: 0,
            reopened_at: 0,
            floor: 0,
            window,
            gps_seen: Vec::new(),
            eps: 0.0,
            reframes: 0,
            verbose,
        };
        let mut setup = Vec::new();
        let p = project(&chain.state);
        let ids: Vec<LineId> = p.lines().iter().map(|s| s.id).collect();
        for (i, &id) in ids.iter().enumerate().skip(1) {
            if rng.chance(60) {
                let other = ids[rng.below(i)];
                setup.push(Op::Merge {
                    a: Self::station_of(&p, other, &mut rng),
                    b: Self::station_of(&p, id, &mut rng),
                });
            }
        }
        for _ in 0..=rng.below(3) {
            let at = rng.pick(&every_station(&p)).unwrap();
            setup.push(datum_op(&mut rng, at));
        }
        (chain, setup)
    }

    fn station_of(p: &Project, id: LineId, rng: &mut Rng) -> StationRef {
        let n = p.line(id).map_or(0, |s| s.shots.len());
        let i = if rng.chance(35) { 0 } else { rng.below(n + 1) };
        StationRef::new(id, i as u32)
    }

    fn say(&self, text: &str) {
        if self.verbose {
            eprintln!("{text}");
        }
    }

    /// Placement tolerance after `reframes` re-frames (module docs).
    fn placement_tol(&self) -> f64 {
        let k = 2f64.powi(self.reframes.min(40) as i32);
        2e-6 * k + self.eps * (k - 1.0)
    }

    /// Record every GPS the project holds, widening `eps` by the largest
    /// re-frame discrepancy over each new triple.
    fn see_gps(&mut self, p: &Project) {
        for d in &p.datums {
            let g = d.gps;
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

/// The roles re-derived from the line chains: union-find over the stations
/// each shot joins.
fn expected_roles(p: &Project) -> Vec<Option<DatumRole>> {
    let mut groups = Groups::default();
    for line in p.lines() {
        let chain: Vec<StationId> = line.stations().collect();
        for pair in chain.windows(2) {
            groups.union(pair[0], pair[1]);
        }
    }
    let mut placed = BTreeSet::new();
    p.datums
        .iter()
        .map(|d| {
            Some(if placed.insert(groups.find(d.station)) {
                DatumRole::Placement
            } else {
                DatumRole::GpsFix
            })
        })
        .collect()
}

impl Chain {
    /// Every per-state property.
    fn check_state(&self, p: &Project, stats: &mut Stats) -> Result<(), Failure> {
        let raw = p.raw_positions();
        for (s, [x, y, z]) in raw.iter() {
            if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                return Err(fail("nonfinite", format!("raw station {s} at ({x}, {y}, {z})")));
            }
        }
        for (i, d) in p.datums.iter().enumerate() {
            if !(d.gps.latitude.is_finite() && d.gps.longitude.is_finite()) {
                return Err(fail("nonfinite", format!("datum {i} GPS {:?}", d.gps)));
            }
        }
        let roles = p.datum_roles();
        let want = expected_roles(p);
        if roles != want {
            return Err(fail(
                "roles",
                format!("datum_roles {roles:?}, the chains say {want:?}"),
            ));
        }
        let violations = p.validate();
        let messages = self.state.integrity_check().messages;
        if !violations.is_empty() || !messages.is_empty() {
            return Err(fail(
                "integrity",
                format!("validate {violations:?}, integrity_check {messages:?}"),
            ));
        }
        for (i, (d, role)) in p.datums.iter().zip(&roles).enumerate() {
            match (d.correction, role) {
                (Some(c), Some(DatumRole::GpsFix)) => {
                    return Err(fail(
                        "correction",
                        format!("GPS fix {i} carries the correction {c:?}"),
                    ))
                }
                (Some(c), _)
                    if !(c[0].is_finite() && c[1].is_finite())
                        || c[0].hypot(c[1]) < CORRECTION_EPSILON_M =>
                {
                    return Err(fail(
                        "correction",
                        format!("datum {i} stores the correction {c:?}"),
                    ))
                }
                _ => {}
            }
        }
        stats.corrections = stats
            .corrections
            .max(p.datums.iter().filter(|d| d.correction.is_some()).count());
        self.check_placements(p, &raw, &roles)?;
        self.check_computed(p, &raw, stats)
    }

    /// Every placement datum exactly at its anchor, and its correction (its
    /// piece's distance from its own GPS) within what re-framing leaves.
    fn check_placements(
        &self,
        p: &Project,
        raw: &RawPositions,
        roles: &[Option<DatumRole>],
    ) -> Result<(), Failure> {
        let Some(frame_index) = p.frame_datum() else {
            return Ok(());
        };
        let frame = p.datums[frame_index].gps;
        let tol = self.placement_tol();
        for (i, (d, role)) in p.datums.iter().zip(roles).enumerate() {
            if *role != Some(DatumRole::Placement) {
                continue;
            }
            let base = if i == frame_index {
                [0.0, 0.0]
            } else {
                let (x, y) = gps_to_local(&d.gps, &frame);
                [x, y]
            };
            let c = d.correction.unwrap_or([0.0, 0.0]);
            let anchor = [base[0] + c[0], base[1] + c[1]];
            let Some(at) = raw.xy(d.station) else {
                continue;
            };
            if !same(at[0], anchor[0]) || !same(at[1], anchor[1]) {
                return Err(fail(
                    "placement-off-anchor",
                    format!(
                        "datum {i} at station {} sits at {at:?}, its anchor is {anchor:?}",
                        d.station
                    ),
                ));
            }
            let off = c[0].hypot(c[1]);
            if off > tol {
                let class = if i == frame_index {
                    "frame-off-origin"
                } else {
                    "placement-off-gps"
                };
                return Err(fail(
                    class,
                    format!(
                        "datum {i} at station {}: its piece sits {off} m off its GPS (tolerance \
                         {tol} after {} re-frames)",
                        d.station, self.reframes
                    ),
                ));
            }
        }
        Ok(())
    }

    /// The Closures Computed View: finite; placement datums held; hard fixes
    /// held on their GPS; no depth moved by a fix.
    fn check_computed(
        &self,
        p: &Project,
        raw: &RawPositions,
        stats: &mut Stats,
    ) -> Result<(), Failure> {
        let view = solve(p);
        let failed_station = |view: &View, s: StationId| {
            p.topology()
                .lines_of(s)
                .iter()
                .any(|l| view.failed.contains(l))
        };
        for (s, [x, y, z]) in &view.at {
            if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                return Err(fail(
                    "nonfinite",
                    format!("computed station {s} at ({x}, {y}, {z})"),
                ));
            }
        }
        let roles = p.datum_roles();
        if roles.contains(&Some(DatumRole::GpsFix)) {
            let bare = solve(&without_fixes(p));
            for (s, [_, _, z]) in &view.at {
                if failed_station(&view, *s) || failed_station(&bare, *s) {
                    continue;
                }
                let bz = bare.at.get(s).map_or(f64::NAN, |v| v[2]);
                if !same(*z, bz) {
                    return Err(fail(
                        "depth-moved",
                        format!("computed station {s} z {z} with the fixes, {bz} without"),
                    ));
                }
            }
        }
        for (d, role) in p.datums.iter().zip(&roles) {
            if *role != Some(DatumRole::Placement) || failed_station(&view, d.station) {
                continue;
            }
            let (Some(r), Some(c)) = (raw.xy(d.station), view.at.get(&d.station)) else {
                continue;
            };
            if !same(r[0], c[0]) || !same(r[1], c[1]) {
                return Err(fail(
                    "placement-moved-computed",
                    format!("station {} raw {r:?}, computed {c:?}", d.station),
                ));
            }
        }
        let (mut groups, held, fixes) = held_groups(p, raw);
        stats.held_fixes += fixes.len();
        for (s, [gx, gy]) in fixes {
            if failed_station(&view, s) {
                continue;
            }
            let Some(c) = view.at.get(&s) else {
                continue;
            };
            if !same(c[0], gx) || !same(c[1], gy) {
                let lines = component_lines(p, s);
                let all_held = p.stations.keys().all(|t| {
                    !p.topology()
                        .lines_of(*t)
                        .iter()
                        .any(|l| lines.contains(l))
                        || held.contains(&groups.find(*t))
                });
                let class = if all_held {
                    "hard-fix-no-unknowns"
                } else {
                    "hard-fix-off-gps"
                };
                return Err(fail(
                    class,
                    format!("station {s} computed {c:?}, GPS ({gx}, {gy})"),
                ));
            }
        }
        Ok(())
    }

    /// What one forward step may change: a datum command never moves a depth
    /// (topology unchanged); storing or removing a fix, and an accuracy-only
    /// edit, never move raw geometry; a frame change translates the survey
    /// rigidly (plan 4.3); removing a datum that held its piece alone moves
    /// nothing.
    fn check_step(op: &Op, p0: &Project, s0: &Snap, s1: &Snap, stats: &mut Stats) -> Result<(), Failure> {
        let same_topology = s0.lines == s1.lines
            && s0.junctions == s1.junctions
            && s0.closures.iter().map(|c| (c.1, c.2)).eq(s1.closures.iter().map(|c| (c.1, c.2)));
        let moved_most = || -> f64 {
            s0.xyz
                .iter()
                .filter_map(|(s, [x, y, _])| {
                    let [nx, ny, _] = s1.xyz.get(s)?;
                    Some((nx - x).hypot(ny - y))
                })
                .fold(0.0, f64::max)
        };
        let target = match op {
            Op::Datum { at, .. } | Op::Twin { at, .. } => station_at(p0, *at),
            Op::Accuracy { k, .. } | Op::Remove { k } => s0.datums.get(*k).map(|d| d.station),
            _ => None,
        };
        let Some(target) = target else {
            return Ok(());
        };
        if same_topology {
            for (s, [_, _, z]) in &s0.xyz {
                if s1.xyz.get(s).is_some_and(|[_, _, nz]| !same(*nz, *z)) {
                    return Err(fail(
                        "depth-moved",
                        format!("{op} moved station {s} from z {z} to {:?}", s1.xyz.get(s)),
                    ));
                }
            }
        }
        let role_on = |s: &Snap| s.datums.iter().find(|d| d.station == target).map(|d| d.role);
        let (role_before, role_after) = (role_on(s0).flatten(), role_on(s1).flatten());
        let stores_fix = !matches!(op, Op::Remove { .. })
            && role_after == Some(DatumRole::GpsFix)
            && role_before != Some(DatumRole::Placement);
        let removes_fix = matches!(op, Op::Remove { k } if s0.datums.get(*k).and_then(|d| d.role) == Some(DatumRole::GpsFix))
            && same_topology;
        let accuracy_only = matches!(op, Op::Accuracy { .. });
        if stores_fix || removes_fix || accuracy_only {
            let most = moved_most();
            if most > 0.0 {
                return Err(fail(
                    "fix-moved-raw",
                    format!("{op} on station {target} moved raw geometry by {most} m"),
                ));
            }
        }
        if let Op::Remove { k } = op {
            let frame_before = s0.datums.iter().position(|d| d.role.is_some());
            let removed = s0.datums.get(*k);
            let alone = removed.is_some_and(|r| {
                let lines = component_lines(p0, r.station);
                s0.datums.iter().enumerate().all(|(j, d)| {
                    j == *k || !p0.topology().lines_of(d.station).iter().any(|l| lines.contains(l))
                })
            });
            if alone && removed.is_some_and(|r| r.role == Some(DatumRole::Placement)) {
                if frame_before == Some(*k) {
                    // A frame change: one translation for every station.
                    let Some((s, t)) = s0.xyz.iter().find_map(|(s, [x, y, _])| {
                        let [nx, ny, _] = s1.xyz.get(s)?;
                        Some((*s, [nx - x, ny - y]))
                    }) else {
                        return Ok(());
                    };
                    for (u, [x, y, _]) in &s0.xyz {
                        let Some([nx, ny, _]) = s1.xyz.get(u) else {
                            continue;
                        };
                        let off = (nx - x - t[0]).hypot(ny - y - t[1]);
                        if off > EXACT_M {
                            return Err(fail(
                                "frame-change-moved",
                                format!(
                                    "{op}: station {u} moved {off} m relative to station {s} \
                                     (translation {t:?})"
                                ),
                            ));
                        }
                    }
                    if let Some(new_frame) = s1.datums.iter().find(|d| d.role.is_some()) {
                        if let Some([x, y, _]) = s1.xyz.get(&new_frame.station) {
                            if x.hypot(*y) > EXACT_M && new_frame.correction.is_none() {
                                return Err(fail(
                                    "frame-change-moved",
                                    format!(
                                        "{op}: the new frame station {} is at ({x}, {y}), not \
                                         the origin",
                                        new_frame.station
                                    ),
                                ));
                            }
                        }
                    }
                    stats.rigid_reframes += 1;
                } else {
                    let most = moved_most();
                    if most > EXACT_M {
                        return Err(fail(
                            "frame-change-moved",
                            format!("{op}: removing a placement datum moved the survey {most} m"),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Note a state change: re-frames, promotions and the GPS seen.
    fn note(&mut self, prev: &Snap, now: &Snap, now_p: &Project, stats: &mut Stats) {
        let frame = |s: &Snap| s.datums.iter().find(|d| d.role.is_some()).map(|d| d.gps);
        if frame(prev).is_some() && frame(now).is_some() && frame(prev) != frame(now) {
            self.reframes += 1;
            stats.frame_changes += 1;
        }
        for d in &now.datums {
            let was_fix = prev.datums.iter().any(|e| {
                e.station == d.station && e.gps == d.gps && e.role == Some(DatumRole::GpsFix)
            });
            if was_fix && d.role == Some(DatumRole::Placement) {
                stats.promotions += 1;
            }
        }
        self.see_gps(now_p);
        stats.max_eps = stats.max_eps.max(self.eps);
    }

    /// Count a step that gave the frame datum another GPS on its own station
    /// while another placement datum placed a piece of its own (#769).
    fn note_frame_gps_edit(prev: &Snap, now: &Snap, stats: &mut Stats) {
        let Some(was) = prev.datums.iter().find(|d| d.role.is_some()) else {
            return;
        };
        let other_piece = prev
            .datums
            .iter()
            .filter(|d| d.role == Some(DatumRole::Placement))
            .count()
            > 1;
        let edited = now
            .datums
            .iter()
            .find(|d| d.role.is_some())
            .is_some_and(|d| d.station == was.station && d.gps[..2] != was.gps[..2]);
        if other_piece && edited {
            stats.frame_gps_edits += 1;
        }
    }

    fn undo_depth(&self) -> usize {
        self.state.get_undo_stack().len()
    }

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
        match apply(&self.state, &self.origin, op, &p0) {
            Applied::Skipped(why) => {
                stats.skipped += 1;
                self.say(&format!("  skipped: {why}"));
                return Ok(());
            }
            Applied::Refused(why) => {
                stats.refused += 1;
                self.say(&format!("  refused: {why}"));
                let s1 = snap(&project(&self.state));
                if let Some(d) = diff(&s0, &s1, true) {
                    return Err(fail("refusal-mutated", format!("{op} refused ({why}): {d}")));
                }
                if s0.text != s1.text || self.undo_depth() != depth0 {
                    return Err(fail(
                        "refusal-mutated",
                        format!("{op} refused ({why}) but changed the counters or the history"),
                    ));
                }
                return Ok(());
            }
            Applied::Done => {}
        }
        let name = op.to_string();
        *stats
            .applied
            .entry(name.split('(').next().unwrap_or("?").to_string())
            .or_default() += 1;
        if let Op::Merge { a, b } = op {
            let placed_component = |at: &StationRef| {
                station_at(&p0, *at).is_some_and(|s| {
                    let lines = component_lines(&p0, s);
                    p0.datums
                        .iter()
                        .any(|d| p0.topology().lines_of(d.station).iter().any(|l| lines.contains(l)))
                })
            };
            if placed_component(a) && placed_component(b) {
                stats.placed_merges += 1;
            }
        }
        let p1 = project(&self.state);
        let s1 = snap(&p1);
        Self::note_frame_gps_edit(&s0, &s1, stats);
        self.note(&s0, &s1, &p1, stats);
        self.show(&p1);
        Self::check_step(op, &p0, &s0, &s1, stats)?;
        self.check_state(&p1, stats)?;
        let depth1 = self.undo_depth();
        if depth1 != depth0 + 1 {
            if let Some(d) = diff(&s0, &s1, true) {
                return Err(fail(
                    "undo-inexact",
                    format!("{op} changed the project but the undo depth went {depth0} -> {depth1}: {d}"),
                ));
            }
            self.timeline.truncate(self.cursor + 1);
            return Ok(());
        }
        // Undo it at once, then redo it: both to the bit, session ids too.
        let r = self.state.undo();
        if !r.success {
            return Err(fail("undo-inexact", format!("undo of {op} refused: {:?}", r.error)));
        }
        if let Some(d) = diff(&s0, &snap(&project(&self.state)), true) {
            return Err(fail("undo-inexact", format!("undo of {op}: {d}")));
        }
        let r = self.state.redo();
        if !r.success {
            return Err(fail("redo-inexact", format!("redo of {op} refused: {:?}", r.error)));
        }
        if let Some(d) = diff(&s1, &snap(&project(&self.state)), true) {
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
        if undo && self.cursor <= self.floor {
            if self.floor == 0 {
                return Ok(());
            }
            let prev = snap(&project(&self.state));
            let r = self.state.undo();
            if r.success {
                return Err(fail(
                    "undo-across-reopen",
                    format!("an undo below the persisted window (floor {}) succeeded", self.floor),
                ));
            }
            if let Some(d) = diff(&prev, &snap(&project(&self.state)), true) {
                return Err(fail("refusal-mutated", format!("a refused undo: {d}")));
            }
            return Ok(());
        }
        let (target, class) = if undo {
            (self.cursor - 1, "undo-inexact")
        } else {
            (self.cursor + 1, "redo-inexact")
        };
        let class = if target < self.reopened_at && class == "undo-inexact" {
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
        // Session ids are exact only for entries recorded in this session.
        let in_session = target >= self.reopened_at && self.cursor >= self.reopened_at;
        if let Some(d) = diff(&self.timeline[target], &now, in_session) {
            return Err(fail(class, d));
        }
        self.cursor = target;
        self.note(&prev, &now, &p, stats);
        self.show(&p);
        self.check_state(&p, stats)
    }

    /// Save, close and reopen: nothing may change, to the bit.
    fn reopen(&mut self, stats: &mut Stats) -> Result<(), Failure> {
        let p0 = project(&self.state);
        let s0 = snap(&p0);
        let v0 = solve(&p0);
        let saved = self.state.execute(Command::SaveProject {
            path: Some(self.path.clone()),
        });
        if !saved.success {
            return Err(fail("reopen-drift", format!("save refused: {:?}", saved.error)));
        }
        let closed = self.state.execute(Command::CloseProject);
        if !closed.success {
            return Err(fail("reopen-drift", format!("close refused: {:?}", closed.error)));
        }
        let opened = self.state.execute(Command::OpenProject {
            path: self.path.clone(),
        });
        if !opened.success {
            return Err(fail("reopen-drift", format!("open refused: {:?}", opened.error)));
        }
        let p1 = project(&self.state);
        let s1 = snap(&p1);
        if s0.text != s1.text {
            return Err(fail(
                "reopen-drift",
                same_up_to_counters(&s0.text, &s1.text).unwrap_or_else(|| "counters".to_string()),
            ));
        }
        if s0.xyz.iter().ne(s1.xyz.iter()) {
            return Err(fail("reopen-drift", "the raw layout changed".to_string()));
        }
        let v1 = solve(&p1);
        for (s, a) in &v0.at {
            let b = v1.at.get(s);
            if b.is_none_or(|b| a.iter().zip(b).any(|(x, y)| x.to_bits() != y.to_bits())) {
                return Err(fail(
                    "reopen-drift",
                    format!("computed station {s} {a:?} -> {b:?}"),
                ));
            }
        }
        self.reopened_at = self.timeline.len();
        self.floor = self.floor.max(self.cursor.saturating_sub(self.window));
        let top = (self.cursor + self.window).min(self.timeline.len() - 1);
        self.timeline.truncate(top + 1);
        stats.reopens += 1;
        self.note(&s0, &s1, &p1, stats);
        self.check_state(&p1, stats)
    }

    /// In a replay, the datums after a step: role, station, correction, and
    /// where the raw survey puts the station.
    fn show(&self, p: &Project) {
        if !self.verbose {
            return;
        }
        let raw = p.raw_positions();
        for (d, role) in p.datums.iter().zip(p.datum_roles()) {
            eprintln!(
                "    {role:?} at station {} via {:?} accuracy {:?} correction {:?}: raw {:?}",
                d.station,
                d.via,
                d.accuracy_m,
                d.correction,
                raw.xy(d.station)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Running chains
// ---------------------------------------------------------------------------

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
        match apply(&chain.state, &chain.origin, op, &p) {
            Applied::Refused(why) | Applied::Skipped(why) => {
                chain.say(&format!("  refused: {why}"));
                continue;
            }
            Applied::Done => {}
        }
        let p1 = project(&chain.state);
        let (s0, s1) = (snap(&p), snap(&p1));
        Chain::note_frame_gps_edit(&s0, &s1, stats);
        chain.note(&s0, &s1, &p1, stats);
        chain.show(&p1);
        Chain::check_step(op, &p, &s0, &s1, stats)
            .and_then(|()| chain.check_state(&p1, stats))
            .map_err(|f| Failure {
                class: f.class,
                detail: format!("in the setup, after {op}: {}", f.detail),
            })?;
    }
    chain.timeline = vec![snap(&project(&chain.state))];
    let mut rng = Rng::new(seed ^ 0xD1B5_4A32_D192_ED03);
    let script: Vec<Op> = script.map(<[Op]>::to_vec).unwrap_or_default();
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
            let (can_undo, can_redo) = (
                chain.cursor > chain.floor,
                chain.cursor + 1 < chain.timeline.len(),
            );
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

/// Random chains of datum, merge, detach, split, combine, delete, add, move,
/// undo, redo and reopen steps keep every invariant in the module doc.
#[test]
fn datum_command_chains_keep_every_invariant() {
    let _config = crate::test_helpers::isolated_config();
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
         skipped, {} merges of two placed pieces, {} reopens, {} re-frames ({} rigid), {} \
         frame GPS edits with another placed piece, {} promotions, {} hard fixes held, at most \
         {} stored corrections, largest re-frame discrepancy {:.3e} m, muted {:?} (mute list \
         {mute:?})",
        first + seeds,
        stats.steps,
        stats.applied,
        stats.refused,
        stats.skipped,
        stats.placed_merges,
        stats.reopens,
        stats.frame_changes,
        stats.rigid_reframes,
        stats.frame_gps_edits,
        stats.promotions,
        stats.held_fixes,
        stats.corrections,
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
        assert!(
            stats.frame_changes > 0 && stats.rigid_reframes > 0,
            "no frame change, or none of the rigid kind"
        );
        assert!(stats.corrections > 0, "no datum ever stored a correction");
    }
    assert!(
        failed.is_empty(),
        "{} failing chains, first: {}",
        failed.len(),
        failed[0]
    );
}

/// Chains the farm or a local sweep once failed on (on the SQLite build),
/// replayed on every run: they are mechanism-rich trails (re-keyed twins,
/// frame GPS edits, promotions after combines), and on the station graph
/// they must keep every invariant too (format: `STALAGMIGHT_DATUM_FUZZ_REPLAY`).
const PINNED_CHAINS: &[&str] = &[
    // datum-key-reused: Delete Shot re-anchors a fix onto the removed datum's key.
    "54;delshot(1,0)",
    // promoted-twin-gps: a fix with the removed placement datum's exact GPS.
    "701;twin(0:3,1,0.0);rm(0)",
    // datum-key-reused with a twin GPS: only the station position tells them apart.
    "1138;merge(0:1,1:4);acc(0,-);unmerge(2:2,3:1);delsec(1);combine(3:start,4:start);delsec(2);acc(0,-);split(3,2,b);datum(3:1,20.953755677079684,-16.914921358435627,5.0);merge(5:0,0:1);datum(5:0,16.04357755301331,-1.7832103357017155,0.5);merge(5:0,0:3);addshot(0,-1,44.39334284186008,6.11431610947411,1.0519934908613826,0.0);merge(0:2,3:1);reopen;twin(5:1,2,1000.0);split(0,1,b);unmerge(5:0,6:1);unmerge(6:1,3:1);rm(0);delshot(5,0)",
    // promoted-twin-gps after a combine that re-keys and moves both twins.
    "1026755;closure(3:2,3:3);acc(0,1000.0);undo;closure(4:1,0:0);split(3,2,a);twin(5:0,1,-);acc(0,0.0);closure(2:2,1:2);addshot(5,0,83.26639828096893,9.85053026440123,28.642278304656077,28.880991248958054);unmerge(0:0,3:0);unmerge(1:0,2:1);addshot(6,-1,255.74044991586703,19.905929536886184,1.2756380852736882,0.0);datum(4:3,24.901653430245844,29.355911207344825,0.0);merge(0:0,2:1);rm(2);datum(1:2,-45.864378812639124,11.359789645079355,50.0);combine(5:start,4:start)",
    // datum-key-reused: a combine re-keys a placement datum onto its twin fix.
    "50028990;split(4,2,b);acc(0,0.0);datum(3:4,-10.355085626613327,-38.63552075781107,1000.0);datum(2:2,45.046864107381886,43.49016961757878,50.0);addshot(0,end,241.82506602311437,12.181044570237113,10.15784749770614,6.664029904480876);datum(3:3,-28.281957111543417,-33.37293276249461,0.5);reopen;unmerge(0:0,1:0);undo;split(0,1,b);acc(3,50.0);datum(5:0,0.0,0.0,50.0);reopen;undo;delshot(3,4);acc(2,-);undo;twin(4:0,1,5.0);closure(4:2,6:0);merge(4:2,3:0);reopen;combine(0:start,2:start);twin(5:1,4,0.5);datum(0:3,-18.54270846695945,37.349707765143265,0.5);split(1,1,b);closure(3:0,3:0);undo;undo;rm(0);datum(0:0,-70.10296390335284,-51.46255498275887,5.0);undo;datum(6:0,11.025626281126955,2.450822161617217,50.0);closure(4:1,0:4);datum(4:2,27.224552254542683,-43.394823892024185,5.0);closure(4:0,5:0);addshot(6,0,93.8038739660679,10.717179406412239,28.31985522332138,25.400500444955643);merge(3:3,0:4);twin(3:0,5,1000.0);rm(0);combine(4:start,5:start)",
    // datum-key-reused: a combine re-keys a fix onto its twin placement datum.
    "1294806;unmerge(0:1,2:1);reopen;datum(1:0,25.650794071477637,-73.60666169426631,50.0);merge(1:0,3:1);datum(3:3,0.0,0.0,5.0);datum(2:0,19.994246212841716,-36.50338157646604,-);rm(4);twin(2:0,0,-);merge(1:0,3:5);unmerge(1:0,3:1);addshot(3,1,169.26310033886833,19.806523673505293,9.60276030515832,6.003958612032227);rm(0);twin(3:0,3,0.0);split(3,5,a);combine(4:start,3:end);acc(0,-);split(4,5,b);datum(4:2,33.880412148064,5.378012870132235,0.5);undo;delshot(1,0);undo;datum(4:1,-73.71064559461756,42.86751369073772,5.0);datum(5:1,0.27217256649528937,5.768254987005169,-);split(4,2,b);combine(4:start,2:end)",
    // datum-key-reused (#778): a combine re-keys two twins with one GPS and accuracy.
    "50081590;unmerge(0:3,1:3);delshot(0,4);datum(1:0,13.24267146086881,-20.05368604646883,-);merge(1:4,1:0);acc(0,0.0);addshot(2,1,17.866115572604944,5.9032088746755855,19.265457069394493,22.74353247521857);split(1,3,b);merge(3:1,1:1);datum(1:2,55.9977495443496,-7.523486185277852,5.0);rm(1);undo;addshot(2,end,311.70366481862885,15.752435002361782,27.23354170456898,31.112966696965994);rm(0);twin(2:4,2,-);undo;datum(3:0,33.266388411108196,84.2336720198981,50.0);datum(2:4,-14.858293515230148,-16.521479181272248,0.5);twin(3:0,5,0.5);datum(2:0,-80.11672339036002,28.213635258484583,5.0);twin(0:0,5,0.5);delsec(2);datum(0:3,-91.83940601821898,-14.350035526455445,0.0);datum(0:0,-16.779784327958737,-51.68504678474291,0.0);undo;closure(3:0,0:4);addshot(3,0,30.355090231295016,13.560162195854868,27.178394127517052,30.28326879909831);reopen;undo;undo;redo;combine(3:start,0:end)",
    // datum-twin-station: Combine joining two datum stations (refused then).
    "350;combine(0:start,3:start)",
    // hard-fix-no-unknowns: every group held, the hard fix still applied.
    "112",
    // frame-gps-edit: the setup gives the frame datum a new GPS while another
    // datum places a piece of its own (#769).
    "11",
    // frame-gps-edit (#778): the frame takes the GPS and accuracy of a fix on
    // its spot, on a piece the frame correction moves.
    "1436150;twin(3:0,0,-);datum(2:1,-28.76717640316142,44.73562740619891,5.0);acc(0,0.5);acc(0,5.0);datum(2:0,73.16789539354458,-38.58700483932714,-);twin(4:3,1,-);merge(3:0,2:0);datum(4:4,25.196609454458898,59.09807127063376,5.0);delshot(1,0);unmerge(0:1,4:2);acc(5,0.5);twin(0:1,3,-)",
    "50124619;delsec(4);acc(0,50.0);delshot(3,1);reopen;undo;reopen;datum(1:0,-21.964371565441986,48.867914065325635,0.5);datum(2:3,-19.025474508692568,96.3596903811504,-);delshot(2,1);rm(1);split(1,1,b);datum(3:0,37.86557792113264,57.57345304260396,1000.0);merge(3:2,2:2);combine(4:start,1:end);unmerge(4:0,3:2);rm(0);rm(0);merge(3:0,2:1);combine(3:end,2:end);twin(3:3,0,0.5);acc(0,50.0);merge(3:2,4:2);split(4,1,b);datum(0:1,-0.10493873460938649,-0.2260835289914435,0.0);merge(0:0,3:2);unmerge(3:2,5:1);delshot(0,0);merge(3:1,0:0);closure(3:2,5:0);unmerge(3:1,0:0);combine(5:start,4:end);delsec(5);reopen;merge(0:0,3:3);split(3,2,b);acc(0,1000.0);closure(0:0,4:2);datum(4:0,-79.63158641198679,38.92858175150428,1000.0);merge(3:1,3:2);twin(3:0,2,0.0)",
];

#[test]
fn pinned_datum_chains_keep_every_invariant() {
    let _config = crate::test_helpers::isolated_config();
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

/// A reopen keeps the whole history (the history limit is the window): 24
/// accuracy edits, a reopen, then 24 undos, every one landing on its recorded
/// state. On the SQLite build only the newest 20 entries were persisted.
#[test]
fn a_reopen_keeps_the_history_and_bounds_the_undo_guard() {
    let _config = crate::test_helpers::isolated_config();
    let mut ops: Vec<Op> = (0..24)
        .map(|i| Op::Accuracy {
            k: 0,
            acc: Some(if i % 2 == 0 { 0.5 } else { 5.0 }),
        })
        .collect();
    ops.push(Op::Reopen);
    ops.extend((0..24).map(|_| Op::Undo));
    let mut stats = Stats::default();
    if let Err(e) = chain_outcome(1, 0, Some(&ops), false, &[], &mut stats) {
        panic!("{e}");
    }
    assert_eq!(stats.reopens, 1, "the chain reopened once");
    assert!(
        stats.applied.get("acc").copied().unwrap_or(0) >= 20,
        "the accuracy edits ran: {:?}",
        stats.applied
    );
}

/// Replay one chain from its trail, printing each step and the datums after
/// it: `STALAGMIGHT_DATUM_FUZZ_REPLAY="seed;op;op;..."`.
#[test]
#[ignore = "replays one chain: STALAGMIGHT_DATUM_FUZZ_REPLAY=\"seed;op;op;...\""]
fn replay_a_datum_chain() {
    let _config = crate::test_helpers::isolated_config();
    let spec = std::env::var("STALAGMIGHT_DATUM_FUZZ_REPLAY").unwrap();
    let mut parts = spec.split(';').filter(|s| !s.trim().is_empty());
    let seed: u64 = parts.next().unwrap().trim().parse().unwrap();
    let ops: Vec<Op> = parts.map(parse_op).collect();
    let mut stats = Stats::default();
    if let Err(e) = chain_outcome(seed, 0, Some(&ops), true, &[], &mut stats) {
        panic!("{e}");
    }
}
