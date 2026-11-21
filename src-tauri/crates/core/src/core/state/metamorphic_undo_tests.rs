//! METAMORPHIC oracles: two routes to the same place, driven side by side,
//! on the station graph and the changeset engine.
//!
//! # What this adds that the roundtrip fuzzer cannot see
//!
//! A roundtrip fuzzer compares a state against ITSELF at an earlier moment.
//! That catches everything a history move fails to restore, and nothing about
//! whether two DIFFERENT ways of asking for the same edit agree. Several entry
//! points promise such an equivalence in their own docs: a one-shot range
//! split IS the single delete-and-split, a batch IS its commands in order, a
//! Combine Lines is the inverse of a fresh Split Line Here, Combine reverses
//! the other line exactly as Reverse Line does. A generalization that does
//! not agree with the thing it generalizes is a bug in one of the two.
//!
//! # The twins
//!
//! Both routes start from the SAME state, byte-identical: the base survey is
//! built once (seeded commands through `AppState::execute`) and saved, then
//! opened into two `AppState`s from that one `.stalx` v2 file. That the twins
//! really start equal is ASSERTED (`Twins::build`), which is also the check
//! that the loader is a function of the file.
//!
//! # Five oracles
//!
//! Every comparison runs: the stored survey (the canonical v2 text, every
//! number at 12 significant digits as the 0.6.0 engine compared it, uuids
//! masked only where both routes mint shots), the model's own integrity
//! verdict (`validate`), the Computed View (every slot within 1e-6 m), and
//! the ID ALLOCATORS (the station, closure and Section counters, the derived
//! next line id, every line's session shot allocator): two routes that leave
//! different allocators hand out different ids afterwards.
//!
//! # The inverse is a pair too
//!
//! Every pair is checked TWICE: on the states the two routes produce, and on
//! the states they unwind to (every entry undone on both twins). 0.6.0 only
//! compared the two unwound twins with each other and MEASURED, without
//! asserting, how far each stayed from its start, because four open findings
//! (#413 findings 1, 2, 3 and 5) left an unwind short of where it began.
//! Those findings are gone with the engine (an undo lands on the recorded
//! state to the bit), so the measurement is an assertion now: each unwound
//! twin is its own starting state (the allocators may stand higher, plan
//! 8.1), and an undo that is refused is a problem rather than a skip.
//!
//! # The one documented divergence
//!
//! The atomic range delete settles the anchors once, after the whole range
//! is gone; the single deletes settle after each shot. On a range whose
//! removed stations include a junction, a partner line hanging off it is
//! dragged by the first single delete's gap-close while it is still
//! attached, and then left there; the range delete leaves it where the
//! surveyor put it (#413 finding 22, resolved as intended; the hand-built
//! case is `a_doomed_anchor_range_shift_keeps_the_partner_in_place` in
//! `open_undo_bugs_tests.rs`). So on that shape alone the partner lines'
//! placements and positions may differ, the ATOMIC route has to be the one
//! that left them alone, and a partner cut loose entirely has to be exactly
//! where it was. Three more consequences of the singles' intermediate states
//! are allowed. A loop closure from the edited line to a partner can become
//! the partners' only link after the first single delete; the orphan-closure
//! sweep then promotes it to a merge (plan 5.5), so on the singles' side the
//! edited line's end of it survives on the partner line under its own id and
//! name, with the partner's station as its alias, where the atomic route
//! deletes that closure with its station and the partner keeps its own
//! station. The comparison undoes exactly those promotions (the ones the
//! singles' own previews reported whose survivor is a station of the edited
//! line the atomic route deleted) before it compares the stored surveys
//! (seed 3063). A loop closure between two partner pieces the range cuts
//! apart is promoted by the range's own sweep, which moves one of the two
//! pieces onto the other by the closure's measured gap (plan 5.5; two free
//! pieces: the piece of the closure's first end moves, plan 4.2), so the
//! atomic route's direction assertion is exact for that piece: every slot of
//! it moved by the move that lands its retired end, as measured before the
//! delete, on the junction where the range left the survivor (which may
//! have moved with the gap-close: a line hanging off the edited line's
//! tail, seed 17514647), and by nothing else (seed 17519277). The singles
//! promote that closure at an intermediate state: onto a survivor the first
//! gap-close dragged (the same direction, seed 17519277), or the other way
//! round when the first delete freed only one piece and the other still
//! hangs off the datum's piece (a placed piece never moves for an unplaced
//! one, plan 4.2). Promoted in opposite directions, the junction keeps the
//! other end's id in each route (each survivor carries the other end as its
//! alias), so the comparison gives the singles' junction the atomic route's
//! station record, after asserting that every line through it shows it by
//! the same name in both routes, with the same comment and the same ids. A
//! promotion is matched by its closure id and its junction found at the
//! closure end's slot on a partner line, which neither route edits: a
//! preview names stations as they were, and the delete may give the
//! junction an alias identity back (U1; seed 17515360, where the range's
//! junction is the restored 9 the preview calls 6). Farm seeds 17514597 and
//! 17515360: the earlier filter, "the atomic route has no survivor and keeps
//! the retired end", took the opposite-direction shape for the first kind,
//! and renaming the junction to its alias's name on every line showed one
//! line two stations named alike, which the v2 encoder refuses: the farm's
//! `DuplicateStationName` panic, an oracle error (both routes' surveys
//! validate). And a piece cut loose is laid out from the anchor algorithm's
//! still point (plan 4.1), so it is that station, not the placement station,
//! that must stay where it was measured: a placement station past the closed
//! gap moves with the gap-close (seed 3019).
//! Everything else is compared strictly, with one consequence allowed: where a loop closure ties a partner to the rest of the survey,
//! the least squares carries the partner's different measured layout into
//! the Computed View of every station of that solve. Per axis that solve is
//! a network with positive weights whose sources are its tie gaps, unmet
//! shots and fix offsets; moving a partner moves only the gaps and fix
//! offsets it takes part in, by at most its own move, so (the maximum
//! principle plus superposition, as for `computed_view_within_its_misclosures`
//! in `migrate_fuzz_tests.rs`) no station's adjusted position changes by more
//! than the sum, over the partner lines, of how far the routes' measured
//! layouts of that line differ on that axis. A non-partner station may
//! differ by up to that bound and no more (farm-style seeds 3019, 3023 and
//! 3063 with closures: a partner tied by a closure carries the whole solve
//! 0.8 m).
//!
//! # Data
//!
//! Everything is generated in code. No fixture is read, copied or derived
//! from.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    reason = "a fuzzer's per-seed summary IS its output (`cargo test -- --nocapture`), and a \
              failed setup step is a failed test"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::survey::changeset::testkit::Rng;
use crate::models::survey::{ClosureId, LoopClosure, Project, Promotion, Station};
use crate::models::{
    ClosureStrategy, CombineLinesOptions, CombinePick, Coordinates, GpsCoordinates, LineId, Person,
    ShotData, ShotId, ShotUpdates, SplitAt, SplitLineOptions, StationId, StationRef, SurveyCredit,
};
use crate::persistence::stalx_v2::{self, Extras, Mode};

const COLORS: &[&str] = &["#ff0000", "#00aa00", "#0000ff", "#aa00aa", "#008888"];

/// The same bound the roundtrip fuzzer's adjusted-geometry oracle used.
const POSITION_TOLERANCE_M: f64 = 1e-6;

fn float(rng: &mut Rng, lo: f64, hi: f64) -> f64 {
    lo + rng.unit() * (hi - lo)
}

/// A value quantized to `1 / scale`, so the generator never asks for more
/// precision than a survey reading has.
fn quantized(rng: &mut Rng, lo: f64, hi: f64, scale: f64) -> f64 {
    (float(rng, lo, hi) * scale).round() / scale
}

fn random_shot(rng: &mut Rng) -> ShotData {
    let depth_in = quantized(rng, 0.0, 30.0, 10.0);
    ShotData {
        length: quantized(rng, 1.0, 15.0, 100.0),
        azimuth_in: quantized(rng, 0.0, 359.9, 10.0),
        azimuth_out: quantized(rng, 0.0, 359.9, 10.0),
        depth_in,
        depth_out: depth_in + quantized(rng, -2.0, 2.0, 10.0),
        left: rng.chance(0.5).then(|| quantized(rng, 0.0, 5.0, 10.0)),
        right: rng.chance(0.5).then(|| quantized(rng, 0.0, 5.0, 10.0)),
        ..ShotData::default()
    }
}

fn random_offset(rng: &mut Rng) -> Coordinates {
    Coordinates::new(
        quantized(rng, -20.0, 20.0, 1000.0),
        quantized(rng, -20.0, 20.0, 1000.0),
        0.0,
    )
}

fn pick<'a, T>(rng: &mut Rng, items: &'a [T]) -> Option<&'a T> {
    (!items.is_empty()).then(|| &items[rng.below(items.len())])
}

// ===========================================================================
// Observation
// ===========================================================================

/// What a pair may differ in before the comparison starts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mask {
    /// Nothing.
    Strict,
    /// Every uuid replaced by a placeholder: only for pairs where BOTH routes
    /// mint shots (a fresh uuid cannot agree across two routes). The count,
    /// the order and every other field are still compared.
    MintedUuids,
}

/// 12 significant digits (the 0.6.0 engine's comparison), and a value
/// within a picometre of zero is zero.
///
/// The floor is new, and numeric, not a classifier: 0.6.0 made float dust in
/// a stored offset unrepresentable (`Project::OFFSET_DUST_M`, #413 finding
/// 2), so a translation and its inverse always landed on the same stored
/// zero. The station graph stores a `Placement` as computed, so `MoveLine(+d)`
/// then `MoveLine(-d)` on a placement already sitting at 2.2e-16 m lands on
/// 0.0 (IEEE absorbs the dust), a difference no derived view can show
/// (positions compare within 1e-6 m) and relative rounding cannot remove.
fn round_significant(value: f64) -> f64 {
    if !value.is_finite() || value == 0.0 {
        return value;
    }
    if value.abs() < 1e-12 {
        return 0.0;
    }
    format!("{value:.11e}").parse::<f64>().unwrap_or(value)
}

fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.chars().enumerate().all(|(index, ch)| match index {
            8 | 13 | 18 | 23 => ch == '-',
            _ => ch.is_ascii_hexdigit(),
        })
}

/// Every number at 12 significant digits and, with `mask_uuids`, every uuid
/// a placeholder.
fn canonicalize(value: &mut serde_json::Value, mask_uuids: bool) {
    match value {
        serde_json::Value::Number(number) => {
            if let Some(raw) = number.as_f64() {
                if !number.is_i64() && !number.is_u64() {
                    if let Some(rounded) = serde_json::Number::from_f64(round_significant(raw)) {
                        *number = rounded;
                    }
                }
            }
        }
        serde_json::Value::String(text) => {
            if mask_uuids && is_uuid(text) {
                *text = "<uuid>".to_string();
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                canonicalize(item, mask_uuids);
            }
        }
        serde_json::Value::Object(map) => {
            for child in map.values_mut() {
                canonicalize(child, mask_uuids);
            }
        }
        serde_json::Value::Bool(_) | serde_json::Value::Null => {}
    }
}

/// The stored survey of `p`: its canonical v2 text with the edit time that
/// a step stamps afresh set to the creation time, as JSON.
fn stored_of(p: &Project, mask: Mask) -> serde_json::Value {
    let mut p = p.clone();
    p.modified_at = p.created_at;
    p.file_path = None;
    let bytes = stalx_v2::encode(&p, &Extras::default(), Mode::Normal).expect("a valid survey");
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).expect("v2 is JSON");
    canonicalize(&mut value, mask == Mask::MintedUuids);
    value
}

/// Every allocator, by name.
fn allocators_of(p: &Project) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    out.insert(
        "next_station_id".to_string(),
        u64::from(p.counters.next_station_id),
    );
    out.insert("next_closure_id".to_string(), p.counters.next_closure_id);
    out.insert(
        "next_section_id".to_string(),
        u64::from(p.counters.next_section_id),
    );
    out.insert("next_line_id".to_string(), u64::from(p.next_line_id()));
    for line in p.lines() {
        out.insert(
            format!("line {}.next_shot_id", line.id),
            u64::from(line.next_shot_id),
        );
    }
    out
}

/// The Computed View, per slot (`line:index`).
fn adjusted_of(state: &AppState) -> BTreeMap<String, [f64; 3]> {
    state
        .get_adjusted_stations()
        .unwrap_or_default()
        .into_iter()
        .map(|s| {
            (
                s.station_ref.to_string(),
                [s.coordinates.x, s.coordinates.y, s.coordinates.z],
            )
        })
        .collect()
}

/// The measured (raw) layout, per slot (`line:index`): where the surveyor's
/// decisions put each station, before any closure adjustment.
fn raw_of(state: &AppState) -> BTreeMap<String, [f64; 3]> {
    let Some(p) = state.get_project_state() else {
        return BTreeMap::new();
    };
    p.raw_positions()
        .slots(&p)
        .into_iter()
        .map(|s| (s.slot.to_string(), [s.x, s.y, s.z]))
        .collect()
}

fn line_of_key(key: &str) -> Option<LineId> {
    key.split(':').next()?.parse().ok()
}

/// A snapshot of everything the twins are compared on, from one side.
struct Observed {
    stored: serde_json::Value,
    /// What `stored` was masked with.
    mask: Mask,
    verdict: Vec<String>,
    adjusted: BTreeMap<String, [f64; 3]>,
    allocators: BTreeMap<String, u64>,
    /// The project, for the documented allowance.
    project: Project,
}

fn observe(state: &AppState, mask: Mask) -> Observed {
    let project = state.get_project_state().expect("a project is open");
    let mut verdict: Vec<String> = project.validate().iter().map(ToString::to_string).collect();
    verdict.sort();
    Observed {
        stored: stored_of(&project, mask),
        mask,
        verdict,
        adjusted: adjusted_of(state),
        allocators: allocators_of(&project),
        project,
    }
}

/// Every path where two JSON values differ, as readable lines (at most 12).
fn json_differences(left: &serde_json::Value, right: &serde_json::Value) -> Vec<String> {
    fn walk(
        path: &str,
        left: &serde_json::Value,
        right: &serde_json::Value,
        out: &mut Vec<String>,
    ) {
        if left == right || out.len() >= 12 {
            return;
        }
        match (left, right) {
            (serde_json::Value::Object(l), serde_json::Value::Object(r)) => {
                let keys: BTreeSet<&String> = l.keys().chain(r.keys()).collect();
                for key in keys {
                    let child = format!("{path}.{key}");
                    match (l.get(key), r.get(key)) {
                        (Some(a), Some(b)) => walk(&child, a, b, out),
                        (a, b) => out.push(format!("{child}: {a:?} vs {b:?}")),
                    }
                }
            }
            (serde_json::Value::Array(l), serde_json::Value::Array(r)) if l.len() == r.len() => {
                for (index, (a, b)) in l.iter().zip(r).enumerate() {
                    walk(&format!("{path}[{index}]"), a, b, out);
                }
            }
            _ => out.push(format!("{path}: {left} vs {right}")),
        }
    }
    let mut out = Vec::new();
    walk("", left, right, &mut out);
    out
}

/// What a comparison found: the differences nothing explains, and the shapes
/// the run reached (a pair whose assertions depend on a generated topology
/// counts it, so an arm that was never exercised reads as such).
#[derive(Default)]
struct Comparison {
    problems: Vec<String>,
    shapes: BTreeMap<&'static str, usize>,
}

impl Comparison {
    fn problem(problem: String) -> Self {
        Self {
            problems: vec![problem],
            shapes: BTreeMap::new(),
        }
    }

    fn absorb(&mut self, other: Self) {
        self.problems.extend(other.problems);
        for (id, count) in other.shapes {
            *self.shapes.entry(id).or_insert(0) += count;
        }
    }

    fn prefixed(mut self, prefix: &str) -> Self {
        self.problems = self
            .problems
            .into_iter()
            .map(|line| format!("{prefix}{line}"))
            .collect();
        self
    }
}

/// The one place two routes are DOCUMENTED to disagree (module docs): the
/// partner lines of a range delete that removes a junction.
struct Documented {
    /// The partner lines: every other line of a junction the range removes,
    /// plus everything tied to those through junctions without passing
    /// through the edited line.
    partners: BTreeSet<LineId>,
    /// The lines the two routes may place differently: the partners, and
    /// the lines of every piece the atomic route cut loose and placed (the
    /// edited line's own head, when the junction it leaves was what tied it
    /// to a datum: the single deletes drag it by the first gap-close while
    /// it is still attached, the same divergence seen from the other side).
    allowed: BTreeSet<LineId>,
    /// The measured layout before either route ran (the twins are asserted
    /// equal there), so "the atomic route is the one that did not move it"
    /// is decidable. Measured, not the Computed View: a partner cut out of a
    /// solved loop leaves the solve, so its adjusted position changes
    /// whichever route ran (it is shown measured from then on).
    before: BTreeMap<String, [f64; 3]>,
    /// Per axis, the sum over the `allowed` lines of how far the two routes'
    /// measured layouts of that line differ at most: the largest change the
    /// solve can carry into any other station's adjusted position (module
    /// docs).
    propagation: [f64; 3],
    /// The closures the singles' sweep promoted on an intermediate state
    /// (module docs), as `(survivor, retired)`: the survivor is a station the
    /// atomic route deleted, the retired one its partner-side end, which the
    /// atomic route kept.
    singles_promotions: Vec<(StationId, StationId)>,
    /// The closures the range's own sweep promoted, each with the piece it
    /// moved (module docs).
    range_promotions: Vec<RangePromotion>,
    /// The closures both routes promoted, in opposite directions (module
    /// docs).
    mirrored: Vec<Mirrored>,
}

/// A closure the range's own sweep promoted between two partner pieces it
/// cut apart: the lines of the piece that moved (the retired end's side) and
/// its move, from the retired end as measured before the delete onto the
/// junction where the range left the survivor.
struct RangePromotion {
    closure: ClosureId,
    moved_lines: BTreeSet<LineId>,
    shift: [f64; 2],
}

/// A closure both routes promoted, in opposite directions (module docs),
/// found by its id; its junction is found by slot, on a partner line (one
/// neither route edits), since the delete can give a station back an alias
/// identity (U1) and a preview names it as it was.
struct Mirrored {
    closure: ClosureId,
    /// The singles' junction id.
    singles_id: StationId,
    /// The atomic route's junction, which the comparison gives the singles'.
    atomic_station: Station,
}

/// Every reference to station `from` in `p` made to `to` (lines, hand
/// moves, datums, placements, closures).
fn rekey(p: &mut Project, from: StationId, to: StationId) {
    let swap = |s: &mut StationId| {
        if *s == from {
            *s = to;
        }
    };
    for line in p.lines_mut() {
        swap(&mut line.start);
        for shot in &mut line.shots {
            swap(&mut shot.to);
        }
        if let Some(hand) = line.hand_moves.remove(&from) {
            line.hand_moves.insert(to, hand);
        }
    }
    for datum in &mut p.datums {
        swap(&mut datum.station);
    }
    for placement in &mut p.placements {
        swap(&mut placement.station);
    }
    for closure in &mut p.closures {
        swap(&mut closure.a);
        swap(&mut closure.b);
    }
}

/// `p` with each promotion `(survivor, retired)` the singles' sweep made
/// undone: the survivor, which carries `retired` as an alias, becomes
/// `retired` again under that alias's name, everywhere it is referenced.
/// Only a promotion whose alias is still there is undone; any other trace of
/// the merge (a joined comment) is left for the comparison to report.
fn undo_promotions(p: &Project, promotions: &[(StationId, StationId)]) -> Project {
    let mut p = p.clone();
    for &(survivor, retired) in promotions {
        if p.stations.contains_key(&retired) {
            continue;
        }
        let Some(mut station) = p.stations.remove(&survivor) else {
            continue;
        };
        let Some(at) = station.aliases.iter().position(|a| a.id == retired) else {
            p.stations.insert(survivor, station);
            continue;
        };
        let alias = station.aliases.remove(at);
        station.id = retired;
        if let Some(name) = alias.name {
            station.name = name;
        }
        p.stations.insert(retired, station);
        rekey(&mut p, survivor, retired);
    }
    p
}

/// `p` (the singles' survey) with each mirrored junction given the atomic
/// route's identity: the station record the atomic route kept, under its
/// id, everywhere it is referenced. What a line shows of the junction is
/// asserted equal first ([`mirrored_junction_problems`]).
fn adopt_atomic_junctions(p: &Project, mirrored: &[Mirrored]) -> Project {
    let mut p = p.clone();
    for m in mirrored {
        let atomic_id = m.atomic_station.id;
        if (atomic_id != m.singles_id && p.stations.contains_key(&atomic_id))
            || p.stations.remove(&m.singles_id).is_none()
        {
            continue;
        }
        p.stations.insert(atomic_id, m.atomic_station.clone());
        rekey(&mut p, m.singles_id, atomic_id);
    }
    p
}

/// Where a promoted closure's junction is in both routes' results: the slot
/// of a closure end (named as in `pre`) on a partner line, which neither
/// route edits.
fn junction_slot(pre: &Project, closure: &LoopClosure, edited: LineId) -> Option<(LineId, usize)> {
    [(closure.a, closure.a_via), (closure.b, closure.b_via)]
        .into_iter()
        .find_map(|(station, via)| {
            let line = via.filter(|l| *l != edited)?;
            let index = pre
                .line_stations(line)?
                .iter()
                .position(|s| *s == station)?;
            Some((line, index))
        })
}

/// The station at `slot` in `p`.
fn station_at_slot(p: &Project, (line, index): (LineId, usize)) -> Option<StationId> {
    p.line_stations(line).and_then(|s| s.get(index).copied())
}

/// The piece the range's own `promotion` (named as in `pre`) moved, read
/// with its result `atomic`: the lines through its retired end before the
/// delete, other than the edited line, closed over the result's junctions
/// except the new one, and the move (the survivor's piece may itself have
/// moved with the gap-close: a line hanging off the edited line's tail).
fn range_promotion(
    pre: &Project,
    atomic: &Project,
    edited: LineId,
    promotion: &Promotion,
) -> Option<RangePromotion> {
    let junction = station_at_slot(atomic, junction_slot(pre, &promotion.closure, edited)?)?;
    let from = pre.raw_positions().xy(promotion.retired)?;
    let to = atomic.raw_positions().xy(junction)?;
    let topology = atomic.topology();
    let mut moved_lines: BTreeSet<LineId> = pre
        .topology()
        .lines_of(promotion.retired)
        .into_iter()
        .filter(|l| *l != edited && atomic.line(*l).is_some())
        .collect();
    let mut queue: Vec<LineId> = moved_lines.iter().copied().collect();
    while let Some(line) = queue.pop() {
        for &station in atomic.line_stations(line).unwrap_or_default() {
            if station == junction {
                continue;
            }
            for other in topology.lines_of(station) {
                if moved_lines.insert(other) {
                    queue.push(other);
                }
            }
        }
    }
    Some(RangePromotion {
        closure: promotion.closure.id,
        moved_lines,
        shift: [to[0] - from[0], to[1] - from[1]],
    })
}

/// The mirrored promotion of the closure the range promoted as `promotion`
/// (named as in `pre`): both routes' junctions, found by slot.
fn mirrored_promotion(
    pre: &Project,
    atomic: &Project,
    singles: &Project,
    edited: LineId,
    promotion: &Promotion,
) -> Option<Mirrored> {
    let slot = junction_slot(pre, &promotion.closure, edited)?;
    Some(Mirrored {
        closure: promotion.closure.id,
        singles_id: station_at_slot(singles, slot)?,
        atomic_station: atomic
            .stations
            .get(&station_at_slot(atomic, slot)?)?
            .clone(),
    })
}

/// What the user sees of a mirrored junction must not depend on the route:
/// every line through it shows it by the same name, it holds the same
/// comment, and it keeps the same ids (one as its own, the rest as aliases).
fn mirrored_junction_problems(atomic: &Project, singles: &Project, m: &Mirrored) -> Vec<String> {
    let a = &m.atomic_station;
    let Some(s) = singles.stations.get(&m.singles_id) else {
        return vec![format!(
            "closure {} promoted both ways, but the singles have no junction {}",
            m.closure, m.singles_id
        )];
    };
    let mut problems = Vec::new();
    let lines: BTreeSet<LineId> = atomic
        .topology()
        .lines_of(a.id)
        .into_iter()
        .chain(singles.topology().lines_of(s.id))
        .collect();
    for line in lines {
        let (shown_a, shown_s) = (atomic.name_on(a.id, line), singles.name_on(s.id, line));
        if shown_a != shown_s {
            problems.push(format!(
                "line {line} shows the junction of closure {} (promoted both ways) as \
                 {shown_a:?} after the range and {shown_s:?} after the singles",
                m.closure
            ));
        }
    }
    if a.comment != s.comment {
        problems.push(format!(
            "the junction of closure {} (promoted both ways) holds the comment {:?} after the \
             range and {:?} after the singles",
            m.closure, a.comment, s.comment
        ));
    }
    let ids = |station: &Station| -> BTreeSet<StationId> {
        std::iter::once(station.id)
            .chain(station.aliases.iter().map(|alias| alias.id))
            .collect()
    };
    if ids(a) != ids(s) {
        problems.push(format!(
            "the junction of closure {} (promoted both ways) keeps the ids {:?} after the range \
             and {:?} after the singles",
            m.closure,
            ids(a),
            ids(s)
        ));
    }
    problems
}

/// [`Documented::propagation`]: per axis, the sum over `lines` of the
/// largest difference between `left` and `right` at one of that line's
/// slots.
fn propagation_bound(
    lines: &BTreeSet<LineId>,
    left: &BTreeMap<String, [f64; 3]>,
    right: &BTreeMap<String, [f64; 3]>,
) -> [f64; 3] {
    let mut per_line: BTreeMap<LineId, [f64; 3]> = BTreeMap::new();
    for (key, a) in left {
        let Some(line) = line_of_key(key).filter(|line| lines.contains(line)) else {
            continue;
        };
        let Some(b) = right.get(key) else {
            continue;
        };
        let worst = per_line.entry(line).or_insert([0.0; 3]);
        for axis in 0..3 {
            worst[axis] = worst[axis].max((a[axis] - b[axis]).abs());
        }
    }
    per_line.values().fold([0.0; 3], |sum, worst| {
        [sum[0] + worst[0], sum[1] + worst[1], sum[2] + worst[2]]
    })
}

impl Documented {
    /// The stored survey without the placements of pieces on partner lines
    /// (a placement names its piece by a station).
    fn without_partner_placements(&self, observed: &Observed) -> serde_json::Value {
        self.strip_partner_placements(&observed.project, observed.stored.clone())
    }

    /// The singles' stored survey with their intermediate promotions undone
    /// ([`undo_promotions`]), the mirrored junctions under the atomic route's
    /// identity ([`adopt_atomic_junctions`]) and the partner placements left
    /// out.
    fn singles_without_promotions(&self, observed: &Observed) -> serde_json::Value {
        if self.singles_promotions.is_empty() && self.mirrored.is_empty() {
            return self.without_partner_placements(observed);
        }
        let project = adopt_atomic_junctions(
            &undo_promotions(&observed.project, &self.singles_promotions),
            &self.mirrored,
        );
        let stored = stored_of(&project, observed.mask);
        self.strip_partner_placements(&project, stored)
    }

    fn strip_partner_placements(
        &self,
        project: &Project,
        mut stored: serde_json::Value,
    ) -> serde_json::Value {
        let partner_stations: BTreeSet<StationId> = self
            .allowed
            .iter()
            .filter_map(|line| project.line_stations(*line))
            .flatten()
            .copied()
            .collect();
        if let Some(serde_json::Value::Array(placements)) = stored.get_mut("placements") {
            placements.retain(|placement| {
                placement
                    .get("station")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|s| u32::try_from(s).ok())
                    .is_none_or(|s| !partner_stations.contains(&s))
            });
        }
        stored
    }
}

fn differ_in_plane(left: [f64; 3], right: [f64; 3]) -> bool {
    (left[0] - right[0]).abs().max((left[1] - right[1]).abs()) > POSITION_TOLERANCE_M
}

/// How two routes' id allocators must compare.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Allocators {
    /// Two forward routes: the same allocators, or they hand out different
    /// ids from here on.
    Equal,
    /// The right route undid something: an undo never hands an id back
    /// (plan 8.1, a step applies the counters as `max(live, recorded)`), so
    /// its allocators may stand higher, never lower.
    RightMayStandHigher,
    /// Both twins unwound different histories: each is checked against its
    /// own start instead ([`baseline_differences`]).
    Unchecked,
}

/// Every way two observed states differ, as readable lines.
fn differences(
    left: &Observed,
    right: &Observed,
    documented: Option<&Documented>,
    allocators: Allocators,
) -> Comparison {
    let mut out = Comparison::default();
    let (mut l, mut r) = match documented {
        Some(doc) => (
            doc.without_partner_placements(left),
            doc.singles_without_promotions(right),
        ),
        None => (left.stored.clone(), right.stored.clone()),
    };
    if allocators != Allocators::Equal {
        // The stored counters are allocators: compared by the rule below.
        for stored in [&mut l, &mut r] {
            if let Some(map) = stored.as_object_mut() {
                map.remove("counters");
            }
        }
    }
    let stored = json_differences(&l, &r);
    if !stored.is_empty() {
        out.problems.push(format!(
            "the stored surveys differ:\n    {}",
            stored.join("\n    ")
        ));
    }
    if left.verdict != right.verdict {
        out.problems.push(format!(
            "the integrity verdicts differ:\n    left:  {:?}\n    right: {:?}",
            left.verdict, right.verdict
        ));
    }
    let keys: BTreeSet<&String> = left.adjusted.keys().chain(right.adjusted.keys()).collect();
    let mut geometry = Vec::new();
    for key in keys {
        let allowed = documented
            .is_some_and(|doc| line_of_key(key).is_some_and(|line| doc.allowed.contains(&line)));
        match (left.adjusted.get(key), right.adjusted.get(key)) {
            (Some(a), Some(b)) => {
                let worst = (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0_f64, f64::max);
                // The partners' different layout, carried by the solve
                // (module docs): within its bound on every axis.
                let carried = documented.is_some_and(|doc| {
                    (0..3).all(|i| (a[i] - b[i]).abs() <= doc.propagation[i] + POSITION_TOLERANCE_M)
                });
                if worst > POSITION_TOLERANCE_M && !allowed && !carried {
                    let bound = documented.map(|doc| doc.propagation);
                    geometry.push(format!(
                        "slot {key} {worst:.6} m apart: {a:?} vs {b:?} (partner bound {bound:?})"
                    ));
                }
            }
            (a, b) => geometry.push(format!("slot {key} on one side only: {a:?} vs {b:?}")),
        }
    }
    if !geometry.is_empty() {
        out.problems.push(format!(
            "the Computed View differs:\n    {}",
            geometry.join("\n    ")
        ));
    }
    let allocators_agree = match allocators {
        Allocators::Equal => left.allocators == right.allocators,
        Allocators::RightMayStandHigher => {
            left.allocators.keys().eq(right.allocators.keys())
                && left
                    .allocators
                    .iter()
                    .all(|(name, was)| right.allocators.get(name).is_some_and(|is| is >= was))
        }
        Allocators::Unchecked => true,
    };
    if !allocators_agree {
        out.problems.push(format!(
            "the id ALLOCATORS differ, so the two routes hand out different ids from here on: \
             {:?} vs {:?}",
            left.allocators, right.allocators
        ));
    }
    out
}

/// How an unwound twin differs from its own start: every oracle but the
/// allocators, which may stand higher (an id once handed out is not handed
/// out again).
fn baseline_differences(baseline: &Observed, now: &Observed) -> Vec<String> {
    let mut problems = Vec::new();
    let strip = |v: &serde_json::Value| {
        let mut v = v.clone();
        if let Some(map) = v.as_object_mut() {
            map.remove("counters");
        }
        v
    };
    let stored = json_differences(&strip(&baseline.stored), &strip(&now.stored));
    if !stored.is_empty() {
        problems.push(format!(
            "the unwound survey is not the one it started from:\n    {}",
            stored.join("\n    ")
        ));
    }
    for (name, was) in &baseline.allocators {
        if now.allocators.get(name).is_some_and(|is| is < was) {
            problems.push(format!("allocator {name} went back below {was}"));
        }
    }
    if baseline.verdict != now.verdict {
        problems.push("the unwound integrity verdict differs from the start".to_string());
    }
    for (key, was) in &baseline.adjusted {
        match now.adjusted.get(key) {
            Some(is)
                if !differ_in_plane(*was, *is)
                    && (was[2] - is[2]).abs() <= POSITION_TOLERANCE_M => {}
            other => problems.push(format!("unwound slot {key}: {was:?} vs {other:?}")),
        }
    }
    problems
}

// ===========================================================================
// The twins
// ===========================================================================

/// Two `AppState`s holding the same project, one per route.
struct Twins {
    left: AppState,
    right: AppState,
    /// Holds the seed `.stalx` alive for the run.
    _temp: TempDir,
    /// The state both sides started from.
    baseline: std::cell::RefCell<Observed>,
    /// The topology the pair was built on, for coverage reporting.
    signature: String,
}

#[derive(Clone, Copy)]
enum Side {
    Left,
    Right,
}

/// A topology signature: lines, junctions, closures, datums, placements.
fn signature_of(p: &Project) -> String {
    let topology = p.topology();
    let junctions = topology
        .stations()
        .iter()
        .filter(|s| topology.is_junction(**s))
        .count();
    format!(
        "lines={} junctions={junctions} closures={} datums={} placements={}",
        p.lines().len(),
        p.closures.len(),
        p.datums.len(),
        p.placements.len()
    )
}

impl Twins {
    /// Build a survey from `seed`, run `prelude` random commands over it,
    /// then open it into both twins from one file. `None` when the base
    /// could not be saved (a skip, not a failure).
    fn build(seed: u32, closures: bool, prelude: usize) -> Option<Self> {
        let source = AppState::new().ok()?;
        let created = source.execute(Command::CreateProject {
            name: format!("Metamorphic {seed}"),
            location: None,
        });
        if !created.success {
            return None;
        }
        let mut rng = Rng::new(u64::from(seed));
        build_project(&source, &mut rng, closures);
        for _ in 0..prelude {
            prelude_command(&source, &mut rng);
        }
        let temp = TempDir::new().ok()?;
        let path = temp.path().join("twins.stalx");
        if !source
            .execute(Command::SaveProject {
                path: Some(path.clone()),
            })
            .success
        {
            return None;
        }
        drop(source);
        let left = AppState::new().ok()?;
        let right = AppState::new().ok()?;
        for side in [&left, &right] {
            if !side
                .execute(Command::OpenProject { path: path.clone() })
                .success
            {
                return None;
            }
            // Both routes start from an empty history, so "unwind
            // everything" unwinds the route and not the prelude.
            side.undo_stack.lock().unwrap().clear();
        }
        let baseline = observe(&left, Mask::Strict);
        let mirror = observe(&right, Mask::Strict);
        assert_eq!(
            baseline.stored, mirror.stored,
            "seed {seed}: two opens of ONE .stalx produced different surveys"
        );
        assert_eq!(baseline.allocators, mirror.allocators, "seed {seed}");
        let signature = signature_of(&baseline.project);
        Some(Self {
            left,
            right,
            _temp: temp,
            baseline: std::cell::RefCell::new(baseline),
            signature,
        })
    }

    const fn state(&self, side: Side) -> &AppState {
        match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
        }
    }

    /// How the two twins differ now (two forward routes).
    fn compare(&self, mask: Mask) -> Comparison {
        self.compare_allowing(mask, None)
    }

    /// [`Self::compare`] after the right twin undid its route.
    fn compare_after_undo(&self, mask: Mask) -> Comparison {
        differences(
            &observe(&self.left, mask),
            &observe(&self.right, mask),
            None,
            Allocators::RightMayStandHigher,
        )
    }

    fn compare_allowing(&self, mask: Mask, documented: Option<&Documented>) -> Comparison {
        differences(
            &observe(&self.left, mask),
            &observe(&self.right, mask),
            documented,
            Allocators::Equal,
        )
    }

    /// The twins' common start changed by hand (a pair's setup writes the
    /// same thing onto both): the baseline is read again.
    fn rebase(&self) {
        *self.baseline.borrow_mut() = observe(&self.left, Mask::Strict);
    }

    /// Undo every entry on both sides, then compare the two unwound twins
    /// with each other AND each with its own start (module docs).
    fn unwind_and_compare(&self, mask: Mask) -> Comparison {
        let mut out = Comparison::default();
        for (name, side) in [("left", Side::Left), ("right", Side::Right)] {
            let state = self.state(side);
            let mut steps = 0;
            while state.can_undo() {
                steps += 1;
                if steps > 512 {
                    out.problems
                        .push(format!("{name} never emptied its undo stack"));
                    return out;
                }
                let result = state.undo();
                if !result.success {
                    out.problems.push(format!(
                        "{name} could not unwind (the history is wedged): {}",
                        result.error.unwrap_or_default()
                    ));
                    return out;
                }
            }
            let now = observe(state, Mask::Strict);
            for problem in baseline_differences(&self.baseline.borrow(), &now) {
                out.problems.push(format!("{name}: {problem}"));
            }
        }
        out.absorb(differences(
            &observe(&self.left, mask),
            &observe(&self.right, mask),
            None,
            Allocators::Unchecked,
        ));
        out
    }
}

// ===========================================================================
// The generator
// ===========================================================================

/// A line's id and its shot ids, and every slot, on one twin.
struct Targets {
    lines: Vec<(LineId, Vec<ShotId>)>,
    slots: Vec<StationRef>,
}

fn read_targets(state: &AppState) -> Targets {
    let p = state.get_project_state().expect("a project is open");
    let lines = p
        .lines()
        .iter()
        .map(|l| (l.id, l.shots.iter().map(|s| s.id).collect()))
        .collect();
    let slots = p
        .lines()
        .iter()
        .flat_map(|l| (0..=l.shots.len() as u32).map(move |i| StationRef::new(l.id, i)))
        .collect();
    Targets { lines, slots }
}

fn create_line_command(name: String, color: &str, shots: Vec<ShotData>) -> Command {
    Command::CreateLine {
        name: Some(name),
        surveyor: None,
        date: None,
        color: color.to_string(),
        direction: None,
        shots,
        cave_section_id: None,
    }
}

/// The base survey: 3 to 5 lines of 2 to 6 shots, most of them starting on
/// a station of an earlier line (a junction, often mid-line), a datum half
/// the time, and with `closures` a closure or two.
fn build_project(state: &AppState, rng: &mut Rng, closures: bool) {
    let count = 3 + rng.below(3);
    let mut lines: Vec<(LineId, usize)> = Vec::new();
    for i in 0..count {
        let shots: Vec<ShotData> = (0..2 + rng.below(5)).map(|_| random_shot(rng)).collect();
        let n = shots.len();
        let color = *pick(rng, COLORS).unwrap_or(&"#000000");
        let created = state.execute(create_line_command(format!("L{i}"), color, shots));
        let Some(id) = created.line_id else {
            continue;
        };
        if let Some(&(parent, parent_shots)) = pick(rng, &lines) {
            if rng.chance(0.7) {
                let at = rng.below(parent_shots + 1) as u32;
                state.execute(Command::MergeStations {
                    station_a: StationRef::new(id, 0),
                    station_b: StationRef::new(parent, at),
                });
            }
        }
        lines.push((id, n));
    }
    if rng.chance(0.5) {
        if let Some(&(line, _)) = lines.first() {
            state.execute(Command::SetZeroDatum {
                station_ref: StationRef::new(line, 0),
                coordinates: GpsCoordinates::new(20.4, -87.3, 0.0).expect("a valid GPS"),
                accuracy_m: None,
                entrance: None,
            });
        }
    }
    if closures {
        let targets = read_targets(state);
        for _ in 0..=rng.below(2) {
            let (Some(&a), Some(&b)) = (pick(rng, &targets.slots), pick(rng, &targets.slots))
            else {
                break;
            };
            let strategy = *pick(
                rng,
                &[
                    ClosureStrategy::Batch,
                    ClosureStrategy::LeastSquaresWeighted,
                    ClosureStrategy::Move,
                ],
            )
            .unwrap_or(&ClosureStrategy::Batch);
            state.execute(Command::CreateLoopClosure {
                station_a: a,
                station_b: b,
                closure_strategy: strategy,
            });
        }
    }
}

/// A shot from a line that has one.
fn pick_shot(targets: &Targets, rng: &mut Rng) -> Option<(LineId, ShotId)> {
    let populated: Vec<&(LineId, Vec<ShotId>)> = targets
        .lines
        .iter()
        .filter(|(_, shots)| !shots.is_empty())
        .collect();
    let (line_id, shots) = pick(rng, &populated)?;
    Some((*line_id, *pick(rng, shots)?))
}

/// One random command against the base survey, before the twins split: a
/// narrower generator than a bug hunter's (no deletes, splits or closures),
/// to give the pairs varied topology to work on.
fn prelude_command(state: &AppState, rng: &mut Rng) {
    let targets = read_targets(state);
    if targets.lines.is_empty() {
        return;
    }
    match rng.below(6) {
        0 => {
            let shots: Vec<ShotData> = (0..=rng.below(3)).map(|_| random_shot(rng)).collect();
            let color = *pick(rng, COLORS).unwrap_or(&"#000000");
            let name = format!("pre-{}", 1000 + rng.below(9000));
            state.execute(create_line_command(name, color, shots));
        }
        1 => {
            let Some((line_id, shots)) = pick(rng, &targets.lines).cloned() else {
                return;
            };
            let after = if !shots.is_empty() && rng.chance(0.5) {
                pick(rng, &shots).copied()
            } else {
                None
            };
            state.execute(Command::CreateShot {
                line_id,
                shot: random_shot(rng),
                after_shot_id: after,
                before_shot_id: None,
            });
        }
        2 => {
            let Some((line_id, shot_id)) = pick_shot(&targets, rng) else {
                return;
            };
            state.execute(Command::UpdateShot {
                line_id,
                shot_id,
                updates: ShotUpdates {
                    length: Some(quantized(rng, 1.0, 15.0, 100.0)),
                    ..ShotUpdates::default()
                },
            });
        }
        3 => {
            let Some(&(line_id, _)) = pick(rng, &targets.lines) else {
                return;
            };
            state.execute(Command::MoveLine {
                line_id,
                offset: random_offset(rng),
            });
        }
        4 => {
            let Some(&slot) = pick(rng, &targets.slots) else {
                return;
            };
            state.execute(Command::SetStationComment {
                station_ref: slot,
                comment: Some(format!("pre-note-{}", rng.below(10_000))),
            });
        }
        _ => {
            let Some(&a) = pick(rng, &targets.slots) else {
                return;
            };
            let others: Vec<StationRef> = targets
                .slots
                .iter()
                .copied()
                .filter(|s| s.line_id != a.line_id)
                .collect();
            let Some(&b) = pick(rng, &others) else {
                return;
            };
            state.execute(Command::MergeStations {
                station_a: a,
                station_b: b,
            });
        }
    }
}

fn shot_ids(state: &AppState, line_id: LineId) -> Vec<ShotId> {
    state
        .get_project_state()
        .and_then(|p| Some(p.line(line_id)?.shots.iter().map(|s| s.id).collect()))
        .unwrap_or_default()
}

/// A line with at least `count` shots, picked deterministically.
fn line_with_shots(state: &AppState, rng: &mut Rng, count: usize) -> Option<LineId> {
    let candidates: Vec<LineId> = read_targets(state)
        .lines
        .into_iter()
        .filter(|(_, shots)| shots.len() >= count)
        .map(|(id, _)| id)
        .collect();
    pick(rng, &candidates).copied()
}

// ===========================================================================
// The pairs
// ===========================================================================

/// One equivalence, and where it is promised.
struct Pair {
    id: &'static str,
    /// Where the promise comes from. A pair with no promise anywhere is not
    /// in this table.
    promise: &'static str,
    /// `None` when the shape is not buildable in this state: a SKIP, counted
    /// apart from a pass so a run that built nothing cannot read as green.
    run: fn(&Twins, &mut Rng) -> Option<Comparison>,
}

const DOOMED_ANCHOR_SHAPE: &str = "doomed-anchor-range";
/// The doomed-anchor shape where the piece cut loose is the edited line's
/// own head.
const CUT_LOOSE_HEAD_SHAPE: &str = "cut-loose-head";
/// The doomed-anchor shape where the range's own sweep promoted a closure
/// between two partner pieces (module docs).
const RANGE_PROMOTION_SHAPE: &str = "range-promotion";
/// The doomed-anchor shape where both routes promoted one closure, in
/// opposite directions (module docs).
const MIRRORED_PROMOTION_SHAPE: &str = "mirrored-promotion";

const PAIRS: &[Pair] = &[
    Pair {
        id: "combine-lines-then-undo-is-identity",
        promise: "command_combine_lines.rs execute_combine_lines - Combine Lines is ONE undo \
                  entry (the reverse of the other line included): undoing it restores every \
                  line, shot, station and allocator",
        run: pair_combine_lines_then_undo_is_identity,
    },
    Pair {
        id: "split-then-combine-at-the-join-is-identity",
        promise: "combine_lines.rs module doc - Combine Lines is the inverse of Split Line \
                  Here: combining a fresh split at its join, with the original name and colour, \
                  gives back the line, every shot's survey credit included (the allocators \
                  aside)",
        run: pair_split_then_combine_at_the_join_is_identity,
    },
    Pair {
        id: "combine-end-to-end-is-reverse-then-combine",
        promise: "combine_lines.rs module doc - when two lines meet end to end the other line \
                  is reversed first with reverse_line, so combining them equals reversing it \
                  yourself and combining end to start",
        run: pair_combine_end_to_end_is_reverse_then_combine,
    },
    Pair {
        id: "split-line-then-undo-is-identity",
        promise: "command_split_line.rs execute_split_line - Split Line Here is ONE undo entry: \
                  undoing it restores every line, shot, junction, closure, datum, comment and \
                  per-shot survey credit exactly (#335)",
        run: pair_split_line_then_undo_is_identity,
    },
    Pair {
        id: "range-shift-is-n-single-deletes",
        promise: "command_delete_split.rs execute_delete_shot_range_shift - an ATOMIC range \
                  gap-close: identical to the n single deletes on every range that removes no \
                  junction, and on a range that removes one it keeps the partner line in place \
                  where the singles drag it (#413 finding 22)",
        run: pair_range_shift_vs_singles,
    },
    Pair {
        id: "one-shot-range-split-is-delete-and-split",
        promise: "command_delete_split.rs - the range split generalizes the single-shot \
                  delete-and-split from one shot to [start..=end]",
        run: pair_range_split_of_one_vs_delete_and_split,
    },
    Pair {
        id: "batch-is-its-commands-in-order",
        promise: "commands.rs - a batch is all-or-nothing over the same commands, committed as \
                  ONE undo entry",
        run: pair_batch_vs_sequential,
    },
    Pair {
        id: "a-failed-batch-is-a-no-op",
        promise: "commands.rs - on any sub-command failure the batch rolls back to its \
                  before-state and pushes nothing, where the same commands run singly apply up \
                  to the failure",
        run: pair_poisoned_batch_is_a_no_op,
    },
    Pair {
        id: "reverse-twice-is-identity",
        promise: "ReverseLine reverses the chain and flips every shot; applying it twice \
                  returns each to itself",
        run: pair_reverse_twice_is_identity,
    },
    Pair {
        id: "move-out-and-back-is-identity",
        promise: "MoveLine translates a rigid group by an offset; the negated offset translates \
                  it back",
        run: pair_move_out_and_back_is_identity,
    },
    Pair {
        id: "create-line-with-shots-is-create-then-append",
        promise: "CreateLine { shots } and CreateLine { shots: [] } + one CreateShot append per \
                  shot build the same line",
        run: pair_create_with_shots_vs_appends,
    },
];

/// `execute_delete_shot_range_shift(a..=b)` against the same shots deleted
/// one at a time through `Command::DeleteShot` (the shot AT the start
/// position, `n` times: the ids do not shift but the positions do). An
/// INTERIOR range, so the gap-close has a head and a tail.
fn pair_range_shift_vs_singles(twins: &Twins, rng: &mut Rng) -> Option<Comparison> {
    let line_id = line_with_shots(&twins.left, rng, 3)?;
    let ids = shot_ids(&twins.left, line_id);
    let start = 1 + rng.below(ids.len().saturating_sub(2).max(1));
    let count = 1 + rng.below(2);
    let end = (start + count - 1).min(ids.len().saturating_sub(2));
    if start > end {
        return None;
    }
    let partners = doomed_anchor_partners(&twins.left, line_id, ids[start], ids[end]);
    let before = raw_of(&twins.left);
    let pre = twins.left.get_project_state()?;
    // What the range's own sweep promotes, named as in `pre`.
    let atomic_promotions: Vec<Promotion> = pre
        .preview_delete_shot_range(line_id, ids[start], ids[end])
        .map(|preview| preview.promoted_closures)
        .unwrap_or_default();
    let ranged = twins
        .left
        .execute_delete_shot_range_shift(line_id, ids[start], ids[end]);
    if !ranged.success {
        return None;
    }
    let mut promoted = Vec::new();
    for _ in start..=end {
        let live = shot_ids(&twins.right, line_id);
        let &target = live.get(start)?;
        if let Some(preview) = twins
            .right
            .get_project_state()
            .and_then(|p| p.preview_delete_shot(line_id, target).ok())
        {
            promoted.extend(preview.promoted_closures);
        }
        let single = twins.right.execute(Command::DeleteShot {
            line_id,
            shot_id: target,
        });
        if !single.success {
            // A single delete is guarded where the range is not (the frozen
            // check on each shot, the loop guard): no state to compare.
            return None;
        }
    }
    let mut cut_loose = Vec::new();
    let documented = partners.map(|partners| {
        let (lines, problems) = pieces_cut_loose(&pre, &twins.left, line_id);
        cut_loose = problems;
        let edited_head = lines.contains(&line_id);
        let mut allowed = partners.clone();
        allowed.extend(lines);
        let propagation = propagation_bound(&allowed, &raw_of(&twins.left), &raw_of(&twins.right));
        // A closure the range promoted too is none of the intermediate kind:
        // the mirrored kind when the range moved its other end. Of the rest,
        // only a promotion whose survivor is a station of the edited line
        // that the atomic route deleted, and whose retired end it kept, is
        // the intermediate-state kind (module docs).
        let atomic = twins.left.get_project_state();
        let singles = twins.right.get_project_state();
        let edited: BTreeSet<StationId> = pre
            .line_stations(line_id)
            .unwrap_or_default()
            .iter()
            .copied()
            .collect();
        let moves_a = |p: &Promotion| p.retired == p.closure.a;
        let range_promotions = atomic.as_ref().map_or_else(Vec::new, |atomic| {
            atomic_promotions
                .iter()
                .filter_map(|range| range_promotion(&pre, atomic, line_id, range))
                .collect()
        });
        let mut mirrored = Vec::new();
        let mut singles_promotions = Vec::new();
        for single in &promoted {
            match atomic_promotions
                .iter()
                .find(|a| a.closure.id == single.closure.id)
            {
                Some(range) if moves_a(range) != moves_a(single) => {
                    if let (Some(atomic), Some(singles)) = (&atomic, &singles) {
                        mirrored.extend(mirrored_promotion(&pre, atomic, singles, line_id, range));
                    }
                }
                Some(_) => {}
                None => {
                    let (survivor, retired) = (single.survivor, single.retired);
                    if edited.contains(&survivor)
                        && atomic.as_ref().is_some_and(|p| {
                            !p.stations.contains_key(&survivor) && p.stations.contains_key(&retired)
                        })
                    {
                        singles_promotions.push((survivor, retired));
                    }
                }
            }
        }
        (
            Documented {
                partners,
                allowed,
                before,
                propagation,
                singles_promotions,
                range_promotions,
                mirrored,
            },
            edited_head,
        )
    });
    let mut found = twins.compare_allowing(Mask::Strict, documented.as_ref().map(|(doc, _)| doc));
    if let Some((documented, edited_head)) = documented.as_ref() {
        *found.shapes.entry(DOOMED_ANCHOR_SHAPE).or_insert(0) += 1;
        if *edited_head {
            *found.shapes.entry(CUT_LOOSE_HEAD_SHAPE).or_insert(0) += 1;
        }
        if !documented.range_promotions.is_empty() {
            *found.shapes.entry(RANGE_PROMOTION_SHAPE).or_insert(0) += 1;
        }
        if !documented.mirrored.is_empty() {
            *found.shapes.entry(MIRRORED_PROMOTION_SHAPE).or_insert(0) += 1;
        }
        found
            .problems
            .extend(documented_divergence_problems(twins, documented));
        found.problems.extend(cut_loose);
    }
    found.absorb(
        twins
            .unwind_and_compare(Mask::Strict)
            .prefixed("AFTER UNWIND: "),
    );
    Some(found)
}

/// The lines a gap-close of `[start..=end]` may place differently by taking
/// the edited line off a junction, or `None` when it removes none: the shape
/// where the two routes must agree exactly. The junctions come from the
/// model's own preview (`junctions_kept`), so a change to which stations a
/// gap-close removes cannot leave this allowing the wrong lines.
fn doomed_anchor_partners(
    state: &AppState,
    line_id: LineId,
    start: ShotId,
    end: ShotId,
) -> Option<BTreeSet<LineId>> {
    let p = state.get_project_state()?;
    let preview = p.preview_delete_shot_range(line_id, start, end).ok()?;
    let topology = p.topology();
    let mut partners: BTreeSet<LineId> = preview
        .junctions_kept
        .iter()
        .flat_map(|s| topology.lines_of(*s))
        .filter(|l| *l != line_id)
        .collect();
    if partners.is_empty() {
        return None;
    }
    // Close over the junctions, never through the edited line: a partner
    // the delete cuts loose carries whatever hangs off it.
    loop {
        let mut grew = false;
        for partner in partners.clone() {
            for station in p.line_stations(partner).unwrap_or_default() {
                for other in topology.lines_of(*station) {
                    if other != line_id && partners.insert(other) {
                        grew = true;
                    }
                }
            }
        }
        if !grew {
            break;
        }
    }
    Some(partners)
}

/// The pieces the atomic route cut loose and placed: every placement of
/// the survey after the route (`now`) on a station that held none before
/// (`pre`), with the lines of its piece. The positive assertion: the piece
/// sits where it was measured before the delete (the anchors settled once,
/// after the whole range was gone): its placement station, or its still
/// point, is exactly at its pre-delete position, so a piece moved as a whole
/// fails. The still point is the anchor algorithm's for a piece with no
/// datum (plan 4.1 rules (a) to (d); the delete is a geometry edit of
/// `edited`): the edited line's pre-edit first station, else a pre-edit
/// placement station, else the edited line's first station with a pre-edit
/// position, else the piece's lowest such station. Either may be the one
/// that stays: the placement station (the piece's lowest station) may lie
/// past the closed gap and move with the gap-close while the still point
/// stays (seed 3019); and a piece the sweep then promoted a closure onto is
/// one piece with the moving piece it took in, whose stations, the edited
/// line's first among them, moved onto it while the placement station
/// stayed (seeds 168, 219, 3136).
fn pieces_cut_loose(
    pre: &Project,
    now: &AppState,
    edited: LineId,
) -> (BTreeSet<LineId>, Vec<String>) {
    let Some(p) = now.get_project_state() else {
        return (BTreeSet::new(), Vec::new());
    };
    let had: BTreeSet<StationId> = pre.placements.iter().map(|pl| pl.station).collect();
    let raw_before = pre.raw_positions();
    let raw_now = p.raw_positions();
    let topology = p.topology();
    let mut lines = BTreeSet::new();
    let mut problems = Vec::new();
    for placement in p.placements.iter().filter(|pl| !had.contains(&pl.station)) {
        let Some(component) = topology.component_of(placement.station) else {
            continue;
        };
        let mut members: BTreeSet<StationId> = BTreeSet::new();
        for line in p.lines() {
            if line
                .stations()
                .any(|s| topology.component_of(s) == Some(component))
            {
                lines.insert(line.id);
                members.extend(line.stations());
            }
        }
        let measured = |s: &StationId| members.contains(s) && raw_before.xy(*s).is_some();
        let still = pre
            .line(edited)
            .map(|l| l.start)
            .filter(|s| measured(s))
            .or_else(|| {
                pre.placements
                    .iter()
                    .map(|pl| pl.station)
                    .find(|s| measured(s))
            })
            .or_else(|| {
                p.line(edited)
                    .and_then(|l| l.stations().find(|s| measured(s)))
            })
            .or_else(|| members.iter().copied().find(|s| measured(s)));
        let Some(still) = still else {
            continue;
        };
        let moved = |station: StationId| -> Option<f64> {
            let (was, is) = (raw_before.xy(station)?, raw_now.xy(station)?);
            Some((is[0] - was[0]).hypot(is[1] - was[1]))
        };
        let (by_placement, by_still) = (moved(placement.station), moved(still));
        let stayed = |m: Option<f64>| m.is_none_or(|m| m <= POSITION_TOLERANCE_M);
        if !stayed(by_placement) && !stayed(by_still) {
            problems.push(format!(
                "the range delete cut a piece loose and moved it: its placement station {} \
                 moved {:.6} m and its still point, station {still}, {:.6} m from where they \
                 were measured",
                placement.station,
                by_placement.unwrap_or(f64::NAN),
                by_still.unwrap_or(f64::NAN),
            ));
        }
    }
    (lines, problems)
}

/// The positive assertions the documented divergence carries.
///
/// 1. DIRECTION: where the two routes disagree about a partner slot (in the
///    measured layout), the ATOMIC route (left) holds it at its pre-delete
///    position; on a piece the atomic route's own sweep moved by promoting a
///    closure, exactly that move away from it, whether or not the routes
///    disagree.
/// 2. THE CUT-LOOSE PARTNER: a partner line left with no junction and no
///    datum has nothing to move it; the atomic route left it exactly where
///    it was.
/// 3. THE MIRRORED JUNCTION: what a line shows of it is the same in both
///    routes ([`mirrored_junction_problems`]).
fn documented_divergence_problems(twins: &Twins, documented: &Documented) -> Vec<String> {
    let atomic = raw_of(&twins.left);
    let singles = raw_of(&twins.right);
    let p = twins.left.get_project_state().expect("a project is open");
    let topology = p.topology();
    let mut problems = Vec::new();
    if let Some(singles_project) = twins.right.get_project_state() {
        for m in &documented.mirrored {
            problems.extend(mirrored_junction_problems(&p, &singles_project, m));
        }
    }
    for (key, was) in &documented.before {
        let Some(line_id) = line_of_key(key) else {
            continue;
        };
        if !documented.partners.contains(&line_id) {
            continue;
        }
        let (Some(&now), Some(&other)) = (atomic.get(key), singles.get(key)) else {
            problems.push(format!(
                "partner slot {key} is missing from one route's view: atomic {:?}, singles {:?}",
                atomic.get(key),
                singles.get(key)
            ));
            continue;
        };
        if let Some(m) = documented
            .range_promotions
            .iter()
            .find(|m| m.moved_lines.contains(&line_id))
        {
            let moved = [was[0] + m.shift[0], was[1] + m.shift[1], now[2]];
            if differ_in_plane(now, moved) {
                problems.push(format!(
                    "the ATOMIC route moved partner slot {key} other than by its promotion of \
                     closure {}, {:?}: was {was:?}, atomic {now:?}, singles {other:?}",
                    m.closure, m.shift
                ));
            }
        } else if differ_in_plane(now, other) && differ_in_plane(now, *was) {
            problems.push(format!(
                "the routes disagree about partner slot {key} AND the ATOMIC route moved it: was \
                 {was:?}, atomic {now:?}, singles {other:?}"
            ));
        }
        let stations = p.line_stations(line_id).unwrap_or_default();
        let held = stations.iter().any(|s| topology.is_junction(*s))
            || p.datums.iter().any(|d| stations.contains(&d.station));
        if !held && differ_in_plane(now, *was) {
            problems.push(format!(
                "the range delete cut line {line_id} loose and MOVED it: slot {key} was {was:?}, \
                 atomic {now:?}"
            ));
        }
    }
    problems
}

/// A ONE-SHOT range split against the single delete-and-split it
/// generalizes. Strictly interior: both refuse a first or last shot.
fn pair_range_split_of_one_vs_delete_and_split(twins: &Twins, rng: &mut Rng) -> Option<Comparison> {
    let line_id = line_with_shots(&twins.left, rng, 3)?;
    let ids = shot_ids(&twins.left, line_id);
    let target = ids[1 + rng.below(ids.len() - 2)];
    let ranged =
        twins
            .left
            .execute_delete_shot_range_split(line_id, target, target, "A".into(), "B".into());
    if !ranged.success {
        return None;
    }
    let single = twins
        .right
        .execute_delete_shot_and_split(line_id, target, "A".into(), "B".into());
    if !single.success {
        return Some(Comparison::problem(format!(
            "the range path accepted a one-shot range that the single-shot path refused: {}",
            single.error.unwrap_or_default()
        )));
    }
    let mut found = twins.compare(Mask::MintedUuids);
    found.absorb(
        twins
            .unwind_and_compare(Mask::MintedUuids)
            .prefixed("AFTER UNWIND: "),
    );
    Some(found)
}

/// The sub-commands both batch pairs use: geometry edits, so the epilogue
/// they are about runs.
fn batch_commands(state: &AppState, rng: &mut Rng) -> Option<Vec<Command>> {
    let targets = read_targets(state);
    let mut commands = Vec::new();
    for _ in 0..2 + rng.below(2) {
        let built = match rng.below(3) {
            0 => pick_shot(&targets, rng).map(|(line_id, shot_id)| Command::UpdateShot {
                line_id,
                shot_id,
                updates: ShotUpdates {
                    length: Some(quantized(rng, 1.0, 15.0, 100.0)),
                    ..ShotUpdates::default()
                },
            }),
            1 => pick(rng, &targets.lines).map(|(line_id, _)| Command::MoveLine {
                line_id: *line_id,
                offset: random_offset(rng),
            }),
            _ => pick(rng, &targets.lines).map(|(line_id, _)| Command::ReverseLine {
                line_id: *line_id,
                swap_left_right: rng.chance(0.5),
            }),
        };
        commands.extend(built);
    }
    (!commands.is_empty()).then_some(commands)
}

/// `Command::Batch` against the same commands one at a time: the STATE is
/// compared (the stacks are not: a batch is one entry, the sequence `n`).
fn pair_batch_vs_sequential(twins: &Twins, rng: &mut Rng) -> Option<Comparison> {
    let commands = batch_commands(&twins.left, rng)?;
    let batched = twins.left.execute(Command::Batch {
        commands: commands.clone(),
        label: None,
    });
    if !batched.success {
        return None;
    }
    for command in commands {
        if !twins.right.execute(command).success {
            return Some(Comparison::problem(
                "a command the batch accepted was refused when run on its own".to_string(),
            ));
        }
    }
    let mut found = twins.compare(Mask::Strict);
    found.absorb(
        twins
            .unwind_and_compare(Mask::Strict)
            .prefixed("AFTER UNWIND: "),
    );
    Some(found)
}

/// The FAILURE contract: a batch whose last sub-command cannot succeed
/// leaves the left twin EXACTLY as it started with an empty stack; the same
/// commands run singly move the right twin.
fn pair_poisoned_batch_is_a_no_op(twins: &Twins, rng: &mut Rng) -> Option<Comparison> {
    let mut commands = batch_commands(&twins.left, rng)?;
    let before = observe(&twins.left, Mask::Strict);
    commands.push(Command::MoveLine {
        line_id: 999_999,
        offset: Coordinates::new(1.0, 1.0, 1.0),
    });
    let batched = twins.left.execute(Command::Batch {
        commands: commands.clone(),
        label: None,
    });
    if batched.success {
        return Some(Comparison::problem(
            "a batch containing an impossible command reported success".to_string(),
        ));
    }
    let mut problems = Vec::new();
    let after = observe(&twins.left, Mask::Strict);
    let changed = json_differences(&before.stored, &after.stored);
    if !changed.is_empty() || before.allocators != after.allocators {
        problems.push(format!(
            "a FAILED batch left the survey changed, so its rollback is not one: {changed:?}"
        ));
    }
    if twins.left.can_undo() {
        problems.push("a FAILED batch pushed an undo entry".to_string());
    }
    let mut applied = 0;
    for command in commands {
        if twins.right.execute(command).success {
            applied += 1;
        }
    }
    if applied == 0 {
        return None;
    }
    if problems.is_empty() && observe(&twins.right, Mask::Strict).stored == before.stored {
        // The commands were no-ops (a line reversed twice, a shot-less
        // line reversed): the rollback held trivially, so the instance
        // proves nothing either way. A skip, counted as one.
        return None;
    }
    let mut found = Comparison {
        problems,
        shapes: BTreeMap::new(),
    };
    // The singles unwind to the start, and so does the untouched left twin.
    found.absorb(
        twins
            .unwind_and_compare(Mask::Strict)
            .prefixed("AFTER UNWIND: "),
    );
    Some(found)
}

/// `ReverseLine` twice against doing nothing, with either L/R flag.
fn pair_reverse_twice_is_identity(twins: &Twins, rng: &mut Rng) -> Option<Comparison> {
    let line_id = line_with_shots(&twins.left, rng, 2)?;
    let swap = rng.chance(0.5);
    for _ in 0..2 {
        let reversed = twins.right.execute(Command::ReverseLine {
            line_id,
            swap_left_right: swap,
        });
        if !reversed.success {
            return None;
        }
    }
    let mut found = twins.compare(Mask::Strict);
    found.absorb(
        twins
            .unwind_and_compare(Mask::Strict)
            .prefixed("AFTER UNWIND: "),
    );
    Some(found)
}

fn split_options(name: &str, second: &str, color: &str) -> SplitLineOptions {
    SplitLineOptions {
        name_a: name.to_string(),
        name_b: second.to_string(),
        color_a: color.to_string(),
        color_b: color.to_string(),
    }
}

fn combine_options(name: &str, color: &str, swap_lr: bool) -> CombineLinesOptions {
    CombineLinesOptions {
        name: name.to_string(),
        color: color.to_string(),
        swap_lr,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Kept,
        join: None,
    }
}

/// Split Line Here at a random mid-line station, then undo it, against
/// doing nothing.
fn pair_split_line_then_undo_is_identity(twins: &Twins, rng: &mut Rng) -> Option<Comparison> {
    let line_id = line_with_shots(&twins.left, rng, 2)?;
    let shot_count = shot_ids(&twins.left, line_id).len();
    let station_index = 1 + rng.below(shot_count - 1) as u32;
    // The comment and datum sides the dialog sent were drawn here; they have
    // left the wire (S9), and the draws stay so every seed draws what it drew.
    rng.below(3);
    rng.below(2);
    let split = twins.right.execute_split_line(
        line_id,
        SplitAt::Station { station_index },
        SplitLineOptions {
            name_a: "split-a".to_string(),
            name_b: "split-b".to_string(),
            color_a: "#111111".to_string(),
            color_b: "#222222".to_string(),
        },
    );
    if !split.success {
        return None;
    }
    let undo = twins.right.undo();
    if !undo.success {
        return Some(Comparison::problem(format!(
            "the split of line {line_id} at station {station_index} succeeded but its undo \
             failed: {:?}",
            undo.error
        )));
    }
    let mut found = twins.compare_after_undo(Mask::Strict);
    found.absorb(
        twins
            .unwind_and_compare(Mask::Strict)
            .prefixed("AFTER UNWIND: "),
    );
    Some(found)
}

/// Give every shot of `line_id` its own survey credit, the same on both
/// twins (one shot in three uncredited), so a pair that splits and combines
/// it compares PER-SHOT credit. Written straight onto both projects: the
/// twins' common starting state, not a step of either route.
fn credit_shots_distinctly(twins: &Twins, rng: &mut Rng, line_id: LineId) {
    let count = shot_ids(&twins.left, line_id).len();
    let credits: Vec<Option<Arc<SurveyCredit>>> = (0..count)
        .map(|i| {
            (i % 3 != 2).then(|| {
                Arc::new(SurveyCredit {
                    surveyors: Person::named(&format!("Surveyor {}", rng.below(4)))
                        .into_iter()
                        .collect(),
                    explorers: Person::named(&format!("Explorer {}", rng.below(3)))
                        .into_iter()
                        .collect(),
                    date: chrono::NaiveDate::from_ymd_opt(2026, 1 + rng.below(12) as u32, 1),
                })
            })
        })
        .collect();
    for side in [&twins.left, &twins.right] {
        let mut guard = side
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(line) = guard.as_mut().and_then(|p| p.line_mut(line_id)) {
            for (shot, credit) in line.shots.iter_mut().zip(&credits) {
                shot.credit.clone_from(credit);
            }
        }
        drop(guard);
        side.bump_revision();
    }
    twins.rebase();
}

/// Split a random line at a mid station that is no junction and no closure
/// end (Combine refuses a branch at the join), on each of `sides`. Returns
/// the two halves and the line's name and colour; `None` when no such
/// station exists or a split is refused or changed the closures.
fn split_at_a_free_station(
    twins: &Twins,
    rng: &mut Rng,
    sides: &[Side],
) -> Option<(LineId, LineId, String, String)> {
    let line_id = line_with_shots(&twins.left, rng, 2)?;
    credit_shots_distinctly(twins, rng, line_id);
    let p = twins.left.get_project_state()?;
    let line = p.line(line_id)?;
    let (name, color) = (line.name.clone().unwrap_or_default(), line.color.clone());
    let topology = p.topology();
    let free: Vec<u32> = (1..line.shots.len())
        .filter(|&i| {
            line.station(i).is_some_and(|s| {
                !topology.is_junction(s) && !p.closures.iter().any(|c| c.a == s || c.b == s)
            })
        })
        .map(|i| i as u32)
        .collect();
    let station_index = *pick(rng, &free)?;
    let closures = p.closures.len();
    let mut second = None;
    for &side in sides {
        let state = twins.state(side);
        let split = state.execute_split_line(
            line_id,
            SplitAt::Station { station_index },
            split_options(&name, &format!("{name}-B"), &color),
        );
        if !split.success || state.get_project_state()?.closures.len() != closures {
            return None;
        }
        second = split.line_id;
    }
    Some((line_id, second?, name, color))
}

/// Two lines both twins can combine: a pair the survey already holds, or
/// one made by splitting a line on both twins.
fn combinable_pair(twins: &Twins, rng: &mut Rng) -> Option<(LineId, LineId)> {
    let ids: Vec<LineId> = read_targets(&twins.left)
        .lines
        .iter()
        .map(|(id, _)| *id)
        .collect();
    let mut natural = Vec::new();
    for (i, &a) in ids.iter().enumerate() {
        for &b in &ids[i + 1..] {
            if twins
                .left
                .combine_candidate_for_lines(a, b)
                .ok()
                .flatten()
                .is_some_and(|c| c.blocked_reason.is_none())
            {
                natural.push((a, b));
            }
        }
    }
    if let Some(&pair) = pick(rng, &natural) {
        return Some(pair);
    }
    let (first, second, _, _) = split_at_a_free_station(twins, rng, &[Side::Left, Side::Right])?;
    twins
        .left
        .combine_candidate_for_lines(first, second)
        .ok()
        .flatten()?
        .blocked_reason
        .is_none()
        .then_some((first, second))
}

/// Combine Lines, then undo it, against doing nothing.
fn pair_combine_lines_then_undo_is_identity(twins: &Twins, rng: &mut Rng) -> Option<Comparison> {
    let (a, b) = combinable_pair(twins, rng)?;
    let (kept, other) = if rng.chance(0.5) { (a, b) } else { (b, a) };
    let combined = twins.right.execute_combine_lines(
        kept,
        other,
        combine_options("c", "#333333", rng.chance(0.5)),
    );
    if !combined.success {
        // The candidate can be blocked for one keep direction only
        // (`blocked_keeping_*`): the other order is the one to try.
        return None;
    }
    let undo = twins.right.undo();
    if !undo.success {
        return Some(Comparison::problem(format!(
            "the combine's undo failed: {:?}",
            undo.error
        )));
    }
    let mut found = twins.compare_after_undo(Mask::Strict);
    found.absorb(
        twins
            .unwind_and_compare(Mask::Strict)
            .prefixed("AFTER UNWIND: "),
    );
    Some(found)
}

/// Split Line Here at a free mid station, then Combine Lines at the join
/// with the original name and colour, against doing nothing. The
/// allocators may stand higher on the route that split (no route hands an
/// id back); everything else, per-shot credit included, is compared.
fn pair_split_then_combine_at_the_join_is_identity(
    twins: &Twins,
    rng: &mut Rng,
) -> Option<Comparison> {
    let (first, second, name, color) = split_at_a_free_station(twins, rng, &[Side::Right])?;
    let candidate = twins
        .right
        .combine_candidate_for_lines(first, second)
        .ok()
        .flatten()?;
    if candidate.blocked_reason.is_some() {
        return None;
    }
    let combined =
        twins
            .right
            .execute_combine_lines(first, second, combine_options(&name, &color, false));
    if !combined.success {
        return Some(Comparison::problem(format!(
            "the candidate said the halves combine, but the combine failed: {:?}",
            combined.error
        )));
    }
    let (left, right) = (
        observe(&twins.left, Mask::Strict),
        observe(&twins.right, Mask::Strict),
    );
    let mut found = Comparison::default();
    found.problems.extend(
        baseline_differences(&left, &right)
            .into_iter()
            .map(|p| format!("split then combine: {p}")),
    );
    found.absorb(
        twins
            .unwind_and_compare(Mask::Strict)
            .prefixed("AFTER UNWIND: "),
    );
    Some(found)
}

/// Two lines that meet END to END: combine them directly (the combine
/// reverses the other one), against reversing it first and combining end to
/// start.
fn pair_combine_end_to_end_is_reverse_then_combine(
    twins: &Twins,
    rng: &mut Rng,
) -> Option<Comparison> {
    let (first, second, _, _) = split_at_a_free_station(twins, rng, &[Side::Left, Side::Right])?;
    for side in [&twins.left, &twins.right] {
        let reversed = side.execute(Command::ReverseLine {
            line_id: second,
            swap_left_right: false,
        });
        if !reversed.success {
            return None;
        }
    }
    let candidate = twins
        .left
        .combine_candidate_for_lines(first, second)
        .ok()
        .flatten()?;
    if candidate.blocked_reason.is_some() || !candidate.reverses {
        return None;
    }
    let swap_lr = rng.chance(0.5);
    let options = combine_options("c", "#333333", swap_lr);
    let direct = twins
        .left
        .execute_combine_lines(first, second, options.clone());
    if !direct.success {
        return Some(Comparison::problem(format!(
            "the direct combine failed: {:?}",
            direct.error
        )));
    }
    let reversed = twins.right.execute(Command::ReverseLine {
        line_id: second,
        swap_left_right: swap_lr,
    });
    let combined = twins.right.execute_combine_lines(first, second, options);
    if !reversed.success || !combined.success {
        return Some(Comparison::problem(format!(
            "reverse then combine failed: {:?} {:?}",
            reversed.error, combined.error
        )));
    }
    let mut found = twins.compare(Mask::Strict);
    found.absorb(
        twins
            .unwind_and_compare(Mask::Strict)
            .prefixed("AFTER UNWIND: "),
    );
    Some(found)
}

/// How far a placement coordinate `start` may end from itself after
/// `fl(fl(start + d) - d)`, the round trip `MoveLine(+d)` then `MoveLine(-d)`
/// makes of it (`ops_move.rs` adds the offset to every placement of the rigid
/// group). Each IEEE sum is correctly rounded, so the result is within
/// `(ulp(sum) + ulp(result)) / 2` of `start`; an ulp of `v` is at most
/// `EPSILON * |v|`, and both values are at most `|start| + |d|` (to first
/// order), so twice `EPSILON * (|start| + |d|)` bounds it with room to spare.
/// The smallest normal is the floor for a subnormal ulp.
fn round_trip_bound(start: f64, offset: f64) -> f64 {
    2.0 * f64::EPSILON * (start.abs() + offset.abs()) + f64::MIN_POSITIVE
}

/// Every placement of `now` that is not `start`'s own back from a round trip
/// by `offset` (same stations in the same order, each coordinate within
/// [`round_trip_bound`]).
fn placements_not_back(start: &Project, now: &Project, offset: Coordinates) -> Vec<String> {
    if start.placements.len() != now.placements.len()
        || start
            .placements
            .iter()
            .zip(&now.placements)
            .any(|(was, is)| was.station != is.station)
    {
        return vec![format!(
            "the placements are not the ones it started with: {:?} vs {:?}",
            start.placements, now.placements
        )];
    }
    let mut out = Vec::new();
    for (was, is) in start.placements.iter().zip(&now.placements) {
        for (axis, from, to, d) in [("x", was.x, is.x, offset.x), ("y", was.y, is.y, offset.y)] {
            if (to - from).abs() > round_trip_bound(from, d) {
                out.push(format!(
                    "placement of station {} came back {:e} m off on {axis} ({from} vs {to}), \
                     past the round trip's rounding {:e} m",
                    was.station,
                    to - from,
                    round_trip_bound(from, d)
                ));
            }
        }
    }
    out
}

/// `MoveLine(+d)` then `MoveLine(-d)` against doing nothing.
///
/// The promise holds to the rounding of the translation and no better: a
/// placement `p` comes back as `fl(fl(p + d) - d)`, which IEEE arithmetic
/// does not make `p` ([`round_trip_bound`]). Twelve significant digits hide
/// that rounding only while `p` is not small against `d`: on farm seeds
/// 17017904 (no closures) and 17505032, 17506797, 17516558 (closures) a
/// placement straddled the twelfth digit, `0.001` back as
/// `0.000999999999999` after a move of about 15 m. So the placements are
/// asserted against that bound, each against its own start, and everything
/// else is compared strictly as before: the rest of the stored survey, the
/// integrity verdict, the Computed View (within 1e-6 m) and the allocators.
/// Undone, the twin is its start to the bit (the unwind below).
fn pair_move_out_and_back_is_identity(twins: &Twins, rng: &mut Rng) -> Option<Comparison> {
    let targets = read_targets(&twins.left);
    let &(line_id, _) = pick(rng, &targets.lines)?;
    let offset = random_offset(rng);
    if !twins
        .right
        .execute(Command::MoveLine { line_id, offset })
        .success
    {
        return None;
    }
    let back = twins.right.execute(Command::MoveLine {
        line_id,
        offset: Coordinates::new(-offset.x, -offset.y, -offset.z),
    });
    if !back.success {
        return Some(Comparison::problem(format!(
            "MoveLine({offset:?}) succeeded and its negation did not: {:?}",
            back.error
        )));
    }
    let start = observe(&twins.left, Mask::Strict);
    let mut now = observe(&twins.right, Mask::Strict);
    let not_back = placements_not_back(&start.project, &now.project, offset);
    if not_back.is_empty() {
        // Back within the round trip's rounding: the rest strictly.
        if let (Some(was), Some(is)) = (
            start.stored.get("placements"),
            now.stored.get_mut("placements"),
        ) {
            *is = was.clone();
        }
    }
    let mut found = Comparison {
        problems: not_back,
        shapes: BTreeMap::new(),
    };
    found.absorb(differences(&start, &now, None, Allocators::Equal));
    found.absorb(
        twins
            .unwind_and_compare(Mask::Strict)
            .prefixed("AFTER UNWIND: "),
    );
    Some(found)
}

/// `CreateLine { shots }` against `CreateLine { shots: [] }` followed by
/// one `CreateShot` append per shot: same shot count, values, order, station
/// names, allocators, placement (uuids masked: both routes mint them).
fn pair_create_with_shots_vs_appends(twins: &Twins, rng: &mut Rng) -> Option<Comparison> {
    let shots: Vec<ShotData> = (0..=rng.below(3)).map(|_| random_shot(rng)).collect();
    let name = format!("pair-{}", 1000 + rng.below(9000));
    let color = *pick(rng, COLORS).unwrap_or(&"#000000");
    let bulk = twins
        .left
        .execute(create_line_command(name.clone(), color, shots.clone()));
    if !bulk.success {
        return None;
    }
    let empty = twins
        .right
        .execute(create_line_command(name, color, Vec::new()));
    let line_id = empty.line_id?;
    for shot in shots {
        let appended = twins.right.execute(Command::CreateShot {
            line_id,
            shot,
            after_shot_id: None,
            before_shot_id: None,
        });
        if !appended.success {
            return None;
        }
    }
    let mut found = twins.compare(Mask::MintedUuids);
    found.absorb(
        twins
            .unwind_and_compare(Mask::MintedUuids)
            .prefixed("AFTER UNWIND: "),
    );
    Some(found)
}

// ===========================================================================
// The driver
// ===========================================================================

#[derive(Default)]
struct PairOutcome {
    built: usize,
    skipped: usize,
    problems: Vec<String>,
    shapes: BTreeMap<&'static str, usize>,
    signatures: BTreeSet<String>,
}

/// Run every pair once per `(seed, repeat)`, each on FRESH twins (a pair
/// that diverges would otherwise re-report itself in every later pair).
fn run_pairs(
    seed: u32,
    repeats: usize,
    closures: bool,
    prelude: usize,
) -> BTreeMap<&'static str, PairOutcome> {
    let mut out: BTreeMap<&'static str, PairOutcome> = BTreeMap::new();
    for pair in PAIRS {
        let entry = out.entry(pair.id).or_default();
        for repeat in 0..repeats {
            // Seeded per (seed, repeat): one pair's draws cannot move another
            // pair's stream.
            let mut rng = Rng::new(
                u64::from(seed)
                    .wrapping_mul(1_000_003)
                    .wrapping_add((repeat as u64).wrapping_mul(7919)),
            );
            let Some(twins) = Twins::build(seed, closures, prelude) else {
                entry.skipped += 1;
                continue;
            };
            entry.signatures.insert(twins.signature.clone());
            match (pair.run)(&twins, &mut rng) {
                None => entry.skipped += 1,
                Some(found) => {
                    entry.built += 1;
                    for (id, count) in found.shapes {
                        *entry.shapes.entry(id).or_insert(0) += count;
                    }
                    for problem in found.problems {
                        entry.problems.push(format!(
                            "seed {seed} repeat {repeat} [{}]\n  promise: {}\n  {problem}",
                            pair.id, pair.promise
                        ));
                    }
                }
            }
        }
    }
    out
}

fn summarize(seed: u32, outcomes: &BTreeMap<&'static str, PairOutcome>) -> String {
    let parts: Vec<String> = outcomes
        .iter()
        .map(|(id, o)| {
            let shapes: Vec<String> = o.shapes.iter().map(|(s, n)| format!("{s}={n}")).collect();
            format!(
                "{id}: {} built, {} skipped, {} problem(s){}",
                o.built,
                o.skipped,
                o.problems.len(),
                if shapes.is_empty() {
                    String::new()
                } else {
                    format!("; shapes: {}", shapes.join(" "))
                }
            )
        })
        .collect();
    format!("metamorphic seed={seed}\n  {}", parts.join("\n  "))
}

// ===========================================================================
// The gate
// ===========================================================================

/// Seeds the gate runs, pinned (see `metamorphic_fuzz` for re-choosing).
const GATE_SEEDS: &[u32] = &[1, 2, 3, 4, 5, 6, 7, 8];
/// Prelude commands before the twins split.
const GATE_PRELUDE: usize = 12;
/// Times each pair is built per seed, each on fresh twins.
const GATE_REPEATS: usize = 3;

/// THE METAMORPHIC GATE, on every `cargo test`. It asserts the eleven
/// promised equivalences, both forward and unwound, and that every unwind
/// lands on its start. It does NOT assert that either route is correct:
/// two routes wrong the same way agree (the roundtrip fuzzer's job).
#[test]
fn metamorphic_gate() {
    let _config = crate::test_helpers::isolated_config();
    let mut problems = Vec::new();
    let mut built = 0;
    let mut built_per_pair: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut shapes: BTreeMap<&'static str, usize> = BTreeMap::new();
    for &seed in GATE_SEEDS {
        let outcomes = run_pairs(seed, GATE_REPEATS, false, GATE_PRELUDE);
        println!("{}", summarize(seed, &outcomes));
        for (id, outcome) in &outcomes {
            built += outcome.built;
            *built_per_pair.entry(id).or_insert(0) += outcome.built;
            problems.extend(outcome.problems.iter().cloned());
            for (shape, count) in &outcome.shapes {
                *shapes.entry(shape).or_insert(0) += count;
            }
        }
    }
    println!("metamorphic gate shape totals: {shapes:?}");
    assert!(
        built >= GATE_SEEDS.len() * PAIRS.len(),
        "the gate built only {built} pair instance(s) over {} seed(s) x {} pair(s): the \
         generators are not reaching the shapes the pairs need",
        GATE_SEEDS.len(),
        PAIRS.len()
    );
    let never: Vec<&&str> = built_per_pair
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(id, _)| id)
        .collect();
    assert!(never.is_empty(), "pairs no gate seed built: {never:?}");
    assert!(
        shapes.get(DOOMED_ANCHOR_SHAPE).copied().unwrap_or(0) > 0,
        "no gate seed produced a range whose removed stations hold a junction, so the \
         doomed-anchor arm asserted nothing: re-choose GATE_SEEDS"
    );
    assert!(
        problems.is_empty(),
        "{} metamorphic divergence(s):\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

/// Seeds 3019, 3023 and 3063 with closures, found by the integration sweep
/// (3001-3200), and 168 (1-300): a range delete that removes a junction whose partner line a
/// loop closure ties to the rest of the survey. The two routes place the
/// partner differently (the documented divergence) and the least squares
/// carries that into the Computed View of every station it solves, 0.8 m on
/// seed 3023, which the oracle now allows within the partners' bound and no
/// further (module docs). Seeds 3063 and 3019 pin the other two allowances
/// (the singles' intermediate promotions, the cut-loose piece's still point),
/// and 168 a piece whose placement station stayed while the sweep moved its
/// still point onto it. Injections: a zero `propagation` bound fails 3023,
/// no promotion undo fails 3063, the placement station alone fails 3019, the
/// still point alone fails 168. Farm seeds 17514597 and 17515360 (the
/// sweep's 4 repeats, with closures) pin the promotion both routes make in
/// opposite directions: the old promotion filter panics on the first (the v2
/// encoder refuses the renamed junction), the range's own promotion moves
/// three partner lines 38 m there, and on the second the range's junction is
/// a restored alias identity the preview names by its old id. Seeds 17519277
/// and 17514647 pin a range promotion the singles make the same way onto a
/// dragged end, and (17514647) one onto a survivor the range's own
/// gap-close moved.
#[test]
fn a_partner_tied_by_a_closure_moves_the_solve_only_within_its_bound() {
    let _config = crate::test_helpers::isolated_config();
    let mut problems = Vec::new();
    let mut doomed = 0;
    let mut mirrored = 0;
    let mut promoted = 0;
    for seed in [
        168, 3019, 3023, 3063, 17_514_597, 17_514_647, 17_515_360, 17_519_277,
    ] {
        let outcomes = run_pairs(seed, 4, true, GATE_PRELUDE);
        let outcome = &outcomes["range-shift-is-n-single-deletes"];
        doomed += outcome
            .shapes
            .get(DOOMED_ANCHOR_SHAPE)
            .copied()
            .unwrap_or(0);
        mirrored += outcome
            .shapes
            .get(MIRRORED_PROMOTION_SHAPE)
            .copied()
            .unwrap_or(0);
        promoted += outcome
            .shapes
            .get(RANGE_PROMOTION_SHAPE)
            .copied()
            .unwrap_or(0);
        problems.extend(outcome.problems.iter().cloned());
    }
    assert!(
        doomed >= 3,
        "the pinned seeds no longer reach the doomed-anchor shape ({doomed}): re-pin them"
    );
    assert!(
        promoted >= 4,
        "seeds 17514597, 17514647, 17515360 and 17519277 no longer reach a range promotion \
         ({promoted}): re-pin them"
    );
    assert!(
        mirrored >= 2,
        "seeds 17514597 and 17515360 no longer reach the mirrored promotion ({mirrored}): re-pin \
         them"
    );
    assert!(
        problems.is_empty(),
        "{} divergence(s):\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

/// The oracles are not vacuous: a one-ulp edit on one twin, an allocator
/// raised on one twin, and a twin whose unwind cannot reach its start (the
/// project changed behind the engine's back) are each reported.
#[test]
fn the_metamorphic_oracles_see_what_they_compare() {
    let _config = crate::test_helpers::isolated_config();
    let twins = Twins::build(1, false, GATE_PRELUDE).expect("twins");
    assert!(
        twins.compare(Mask::Strict).problems.is_empty(),
        "equal twins"
    );
    let (line, shot) = {
        let p = twins.right.get_project_state().unwrap();
        let l = p.lines().iter().find(|l| !l.shots.is_empty()).unwrap();
        (l.id, l.shots[0].id)
    };
    let length = twins
        .right
        .get_project_state()
        .unwrap()
        .line(line)
        .unwrap()
        .shots[0]
        .length;
    let r = twins.right.execute(Command::UpdateShot {
        line_id: line,
        shot_id: shot,
        updates: ShotUpdates {
            length: Some(length + 0.001),
            ..ShotUpdates::default()
        },
    });
    assert!(r.success, "{:?}", r.error);
    let found = twins.compare(Mask::Strict);
    assert!(
        found
            .problems
            .iter()
            .any(|p| p.contains("stored surveys differ")),
        "a different length is a stored difference: {:?}",
        found.problems
    );
    assert!(
        found.problems.iter().any(|p| p.contains("Computed View")),
        "and a geometry difference: {:?}",
        found.problems
    );
    // Undone, the twins agree again (an UpdateShot mints no id).
    assert!(twins.right.undo().success);
    assert!(twins.compare_after_undo(Mask::Strict).problems.is_empty());
    // An allocator one twin raised is an allocator difference.
    {
        let mut guard = twins.right.project.lock().unwrap();
        guard.as_mut().unwrap().counters.next_station_id += 1;
    }
    assert!(twins
        .compare(Mask::Strict)
        .problems
        .iter()
        .any(|p| p.contains("ALLOCATORS")));
    assert!(
        twins.compare_after_undo(Mask::Strict).problems.is_empty(),
        "an undo route may stand higher"
    );
    // A change behind the engine's back is no start an unwind can reach.
    {
        let mut guard = twins.right.project.lock().unwrap();
        let p = guard.as_mut().unwrap();
        p.name = "Changed behind the engine's back".to_string();
    }
    let unwound = twins.unwind_and_compare(Mask::Strict);
    assert!(
        unwound
            .problems
            .iter()
            .any(|p| p.starts_with("right: the unwound survey is not the one it started from")),
        "{:?}",
        unwound.problems
    );
}

/// Determinism: without it a failing seed is not a repro.
#[test]
fn the_metamorphic_engine_is_deterministic() {
    let _config = crate::test_helpers::isolated_config();
    type Shape = Vec<(String, usize, usize, usize, BTreeMap<&'static str, usize>)>;
    let shape = |outcomes: &BTreeMap<&'static str, PairOutcome>| -> Shape {
        outcomes
            .iter()
            .map(|(id, o)| {
                (
                    (*id).to_string(),
                    o.built,
                    o.skipped,
                    o.problems.len(),
                    o.shapes.clone(),
                )
            })
            .collect()
    };
    for &seed in &GATE_SEEDS[..3] {
        let first = run_pairs(seed, 2, false, GATE_PRELUDE);
        let second = run_pairs(seed, 2, false, GATE_PRELUDE);
        assert_eq!(
            shape(&first),
            shape(&second),
            "seed {seed} did not replay identically"
        );
    }
}

/// Every problem `pair` reports on the given farm seeds (each run as the
/// sweep runs it, four repeats), with how many instances it built.
fn pinned_seed_problems(pair: &str, seeds: &[(u32, bool)]) -> (usize, Vec<String>) {
    let mut built = 0;
    let mut problems = Vec::new();
    for &(seed, closures) in seeds {
        let outcomes = run_pairs(seed, 4, closures, GATE_PRELUDE);
        let outcome = &outcomes[pair];
        built += outcome.built;
        problems.extend(outcome.problems.iter().cloned());
    }
    (built, problems)
}

/// Farm seeds of reverse-twice-is-identity (sg-metamorphic 17003130 and
/// 17003411; with closures 17500744 and 17519186): a shot whose two readings
/// are typed exactly opposite but one ulp short of it in f64 (`158.1` /
/// `338.1`). The reverse took it for an ordinary pair, so its bearing did not
/// turn, and the second reverse turned it the other way: the line came back
/// metres off with that shot's readings exchanged (module docs of
/// `models::shot_reverse`; declared fix, plan Deviations T4-R1). Injection:
/// the old bit-exact antipodal test fails all four.
#[test]
fn a_shot_typed_exactly_opposite_reverses_twice_to_itself() {
    let _config = crate::test_helpers::isolated_config();
    let (built, problems) = pinned_seed_problems(
        "reverse-twice-is-identity",
        &[
            (17_003_130, false),
            (17_003_411, false),
            (17_500_744, true),
            (17_519_186, true),
        ],
    );
    assert!(
        built >= 4,
        "the pinned seeds built only {built} reverse pair(s)"
    );
    assert!(
        problems.is_empty(),
        "{} divergence(s):\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

/// Farm seeds of move-out-and-back-is-identity (sg-metamorphic 17017904;
/// with closures 17505032, 17506797 and 17516558): a placement came back
/// from `MoveLine(+d)` then `MoveLine(-d)` within the rounding of the
/// translation, and twelve significant digits straddled it (the pair's
/// docs). Injection: comparing the placements strictly fails all four.
#[test]
fn a_move_out_and_back_lands_within_its_rounding() {
    let _config = crate::test_helpers::isolated_config();
    let (built, problems) = pinned_seed_problems(
        "move-out-and-back-is-identity",
        &[
            (17_017_904, false),
            (17_505_032, true),
            (17_506_797, true),
            (17_516_558, true),
        ],
    );
    assert!(
        built >= 4,
        "the pinned seeds built only {built} move pair(s)"
    );
    assert!(
        problems.is_empty(),
        "{} divergence(s):\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

/// The bound is not vacuous: a placement one micrometre off after the round
/// trip is reported, one rounding error off is not.
#[test]
fn a_placement_off_by_more_than_the_rounding_is_reported() {
    let _config = crate::test_helpers::isolated_config();
    let twins = Twins::build(17_017_904, false, GATE_PRELUDE).expect("twins");
    let start = twins.left.get_project_state().expect("a project");
    assert!(!start.placements.is_empty(), "the seed has a placement");
    let offset = Coordinates::new(15.123, -7.5, 0.0);
    let mut now = start.clone();
    let p = &mut now.placements[0];
    p.x = (p.x + offset.x) - offset.x;
    p.y = (p.y + offset.y) - offset.y;
    assert!(
        placements_not_back(&start, &now, offset).is_empty(),
        "one round trip's rounding"
    );
    now.placements[0].x += 1e-6;
    assert_eq!(placements_not_back(&start, &now, offset).len(), 1);
}

/// The SWEEP: the same pairs at a scale a gate cannot afford, and the tool
/// for re-choosing `GATE_SEEDS`.
///
/// ```text
/// STALAGMIGHT_RUST_META_SEEDS=60 STALAGMIGHT_RUST_META_REPEATS=6 \
///   STALAGMIGHT_RUST_META_CLOSURES=1 \
///   TELEMETRY_ENDPOINT=http://localhost/dummy cargo test \
///   --manifest-path src-tauri/Cargo.toml -p stalagmight-core \
///   --lib metamorphic_fuzz -- --ignored --nocapture
/// ```
///
/// `STALAGMIGHT_RUST_META_FIRST_SEED=n` starts the seed window at `n` (default 1).
#[test]
#[ignore = "sweep: prints per-seed status, used to choose the metamorphic GATE_SEEDS"]
fn metamorphic_fuzz() {
    let _config = crate::test_helpers::isolated_config();
    let env = |name: &str, default: usize| -> usize {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(default)
    };
    let seeds = env("STALAGMIGHT_RUST_META_SEEDS", 40) as u32;
    let repeats = env("STALAGMIGHT_RUST_META_REPEATS", 4);
    let prelude = env("STALAGMIGHT_RUST_META_PRELUDE", GATE_PRELUDE);
    let closures = std::env::var("STALAGMIGHT_RUST_META_CLOSURES").is_ok();
    let verbose = std::env::var("STALAGMIGHT_RUST_META_VERBOSE").is_ok();
    let mut totals: BTreeMap<&'static str, (usize, usize, usize)> = BTreeMap::new();
    let mut shapes: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut signatures: BTreeSet<String> = BTreeSet::new();
    let mut dirty = Vec::new();
    for seed in crate::test_helpers::sweep_seeds("STALAGMIGHT_RUST_META_FIRST_SEED", seeds) {
        let outcomes = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_pairs(seed, repeats, closures, prelude)
        }));
        let Ok(outcomes) = outcomes else {
            dirty.push(seed);
            println!("metamorphic seed={seed} PANICKED");
            continue;
        };
        let mut seed_problems = 0;
        for (id, outcome) in &outcomes {
            let entry = totals.entry(id).or_insert((0, 0, 0));
            entry.0 += outcome.built;
            entry.1 += outcome.skipped;
            entry.2 += outcome.problems.len();
            seed_problems += outcome.problems.len();
            for (shape, count) in &outcome.shapes {
                *shapes.entry(shape).or_insert(0) += count;
            }
            signatures.extend(outcome.signatures.iter().cloned());
            if verbose {
                for problem in &outcome.problems {
                    println!("--- {problem}");
                }
            }
        }
        if seed_problems > 0 {
            dirty.push(seed);
            println!("{}", summarize(seed, &outcomes));
        }
    }
    println!("metamorphic sweep: {seeds} seed(s) x {repeats} repeat(s), closures={closures}");
    for (id, (built, skipped, problems)) in &totals {
        println!("  {id}: {built} built, {skipped} skipped, {problems} problem(s)");
    }
    println!(
        "topology coverage: {} distinct signature(s)",
        signatures.len()
    );
    println!("generated shape totals: {shapes:?}");
    println!(
        "seeds with at least one divergence ({}): {dirty:?}",
        dirty.len()
    );
}
