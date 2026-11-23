//! The kept-and-archived-stations properties (plan 2026-11-23, RS section
//! 12) on random synthetic surveys, as a seeded fuzzer the farm can run.
//!
//! A seed builds a small survey (free lines, branches, comments, a loop) and
//! runs a random sequence of the operations that move identities: Merge
//! Stations with either keep and every way of answering its collisions
//! (rule L, the preview's suggestions, its alternatives, nothing), Detach
//! Line from Station with a `rest_keep`, Create Loop Closure with an allowed
//! keep, "Keep both names" or a keep C1 and C2 refuse, Remove Closure, Delete
//! Shot, range deletes, Delete Line, Split Line Here, Insert Station, Combine
//! Lines, Reverse Line, comments, datums and Add .stalx. After every step:
//!
//! | id | property |
//! |---|---|
//! | `rs-valid` | the model validates (A1 to A5, C1, C2, N8 and the rest) |
//! | `rs-refusal` | a refused operation changed nothing |
//! | `rs-stale-record` | no record is history (or names another line) while a shot next to the slot it named is still on a line: the node at that slot, wherever the shot went, is the record or holds it for that line (RS 4.3, U-P1-4, M-P1-4c) |
//! | `rs-comment` | every comment shown before the step is shown after it, or its station is gone, or the step's preview (or, for a step without one, its report) names it as hidden or moved |
//! | `rs-rule-l` | no letter the step reports went to an identity shown on more lines than the one it collided with (RS 2) |
//! | `rs-answers` | a merge, detach or closure given its preview's suggested renames is accepted, and one given no answers is refused only for a collision its preview listed |
//! | `rs-preview` | a merge, closure or detach the preview refused went through (or the reverse) |
//!
//! and, per merge and per closure:
//!
//! | id | property |
//! |---|---|
//! | `rs-keep-layout` | the merge keeping either station gives the same raw layout, slot by slot, bit for bit (M-P1-1) |
//! | `rs-keep-computed` | and the same Computed View within [`COMPUTED_TOL_M`] (M-P2-9) |
//! | `rs-keep-refusal` | and either keep is refused exactly when the other is |
//! | `rs-merge-detach` | detaching the archived station's line restores every shown identity and record (RS 12; see [`merge_then_detach`] for the exceptions) |
//! | `rs-closure-remove` | removing a closure just created restores every shown identity and record |
//!
//! A failure names its seed, step and property id; a panic is `rs-panic`.
//!
//! # Entry points
//!
//! - `rs_properties_hold_on_the_default_seeds`: the CI gate, seeds
//!   `1..=DEFAULT_SEEDS`, [`DEFAULT_STEPS`] steps each.
//! - `rs_property_sweep` (ignored): the farm's window.
//!   `STALAGMIGHT_RUST_RS_SEEDS` (default 200) seeds from
//!   `STALAGMIGHT_RUST_RS_FIRST_SEED` (default 1), each
//!   `STALAGMIGHT_RUST_RS_STEPS` steps (default [`DEFAULT_STEPS`]). Each
//!   failing seed prints `seed N step M: <id>: <detail>`, a panic `seed N:
//!   PANICKED <message>`, and the sweep ends with `rs sweep: seeds with a
//!   failure (K) of N`. `STALAGMIGHT_RUST_RS_TRACE=1` prints every step (a
//!   single-seed replay: `SEEDS=1 FIRST_SEED=n TRACE=1`).
//!
//! # Data
//!
//! Everything is generated in code; no fixture is read, copied or derived
//! from.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::config::ClosureAdjustmentConfig;
use crate::{
    CombineJoinEnds, CombineLinesOptions, CombinePick, LineId, ShotData, SplitAt, StationId,
    StationRef,
};

use super::ops_test_helpers::gps_at;
use super::test_helpers::{meta, project, shot};
use super::{
    ClosureId, ClosureStrategy, EditError, HiddenComment, IdentityReport, LetteredStation, OpError,
    OpOutcome, Project, RangeDeleteMode, SplitOptions, StartAt, SweepReport,
};

/// Seeds the CI gate runs.
const DEFAULT_SEEDS: u32 = 24;
/// Steps per seed.
const DEFAULT_STEPS: usize = 50;
/// The Computed View of the two keeps of one merge agree within this (M-P2-9:
/// the solve orders groups by station id, so its last bits may differ).
pub(super) const COMPUTED_TOL_M: f64 = 1e-6;

// ----- generator --------------------------------------------------------------

/// splitmix64: small, seedable, the same everywhere.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    /// Uniform in `[lo, hi)`, quantised to centimetres so layouts stay
    /// readable in a failure.
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        let unit = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        ((lo + (hi - lo) * unit) * 100.0).round() / 100.0
    }

    fn pick<T: Clone>(&mut self, items: &[T]) -> Option<T> {
        if items.is_empty() {
            None
        } else {
            Some(items[self.below(items.len())].clone())
        }
    }
}

/// How a step answers the name collisions its dialog would list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answers {
    /// No dialog: rule L letters every collision (the `*_auto` forms).
    RuleL,
    /// The preview's suggestions (rule L's letters, the dialog's default).
    Suggested,
    /// The preview's other answers.
    Alternative,
    /// An empty rename list: refused when the step makes a collision.
    Nothing,
}

impl Answers {
    fn draw(rng: &mut Rng) -> Self {
        match rng.below(10) {
            0..=3 => Self::RuleL,
            4..=6 => Self::Suggested,
            7 | 8 => Self::Alternative,
            _ => Self::Nothing,
        }
    }

    /// The renames this answer sends, from the preview's suggestions and
    /// alternatives; `None` for rule L.
    fn renames(
        self,
        suggested: &[LetteredStation],
        alternatives: &[LetteredStation],
    ) -> Option<Vec<(StationId, String)>> {
        let of = |letters: &[LetteredStation]| -> Vec<(StationId, String)> {
            letters.iter().map(|l| (l.station, l.to.clone())).collect()
        };
        match self {
            Self::RuleL => None,
            Self::Suggested => Some(of(suggested)),
            Self::Alternative => Some(of(alternatives)),
            Self::Nothing => Some(Vec::new()),
        }
    }
}

/// A property that failed.
#[derive(Debug, Clone)]
pub(super) struct Failure {
    pub(super) property: &'static str,
    pub(super) detail: String,
}

fn fail(property: &'static str, detail: String) -> Failure {
    Failure { property, detail }
}

/// What a successful step tells the general checks.
#[derive(Debug, Default)]
struct StepCtx {
    /// Identities whose comment the step's preview (or report) names as
    /// becoming hidden.
    hidden_named: BTreeSet<StationId>,
    /// Restores whose comment moved with the node: `(old, new)`.
    moved: Vec<(StationId, StationId)>,
    /// Identities whose comment the step itself set or picked.
    edited: BTreeSet<StationId>,
    /// Nodes whose records the stale-record check leaves out (a Combine's
    /// join stations: RS 4.4 drops the absorbed line, and the kept line
    /// at a shared join, by design).
    skip_record_nodes: BTreeSet<StationId>,
    /// Combine's "Both" pick may join two comments into one.
    joined: bool,
    /// Every letter the step reported.
    lettered: Vec<LetteredStation>,
}

impl StepCtx {
    /// Take what an epilogue reported: its restores (moved and hidden
    /// comments) and letters.
    fn absorb_report(&mut self, report: &IdentityReport, sweep: &SweepReport) {
        self.absorb_restores(report);
        self.lettered.extend(report.lettered.iter().cloned());
        self.absorb_sweep(sweep);
    }

    fn absorb_restores(&mut self, report: &IdentityReport) {
        for restored in &report.restored {
            if restored.comment_moved {
                self.moved.push((restored.old, restored.new));
            }
            if restored.comment_hidden.is_some() {
                self.hidden_named.insert(restored.old);
            }
        }
        self.absorb_hidden(&report.hidden);
    }

    /// The comments a preview or report names as hidden (RS 12).
    fn absorb_hidden(&mut self, hidden: &[HiddenComment]) {
        self.hidden_named.extend(hidden.iter().map(|h| h.station));
    }

    /// A promotion archives its other end (RS 5.4); the step's report names
    /// it, and so does every delete preview (`promoted_closures`).
    fn absorb_sweep(&mut self, sweep: &SweepReport) {
        self.lettered.extend(sweep.lettered.iter().cloned());
        for promotion in &sweep.promoted {
            self.hidden_named.insert(promotion.archived);
        }
    }
}

/// One seed's run.
struct Fuzz {
    p: Project,
    rng: Rng,
    uuid: u64,
    seed: u32,
    step: usize,
    trace: bool,
    /// How many times each property was actually checked (a vacuous pass
    /// would hide a broken generator).
    checked: BTreeMap<&'static str, usize>,
    log: Vec<String>,
}

impl Fuzz {
    fn new(seed: u32, trace: bool) -> Self {
        let mut fuzz = Self {
            p: project(),
            rng: Rng(u64::from(seed).wrapping_mul(0xA24B_AED4_963E_E407) ^ 0x5253),
            uuid: 0,
            seed,
            step: 0,
            trace,
            checked: BTreeMap::new(),
            log: Vec::new(),
        };
        fuzz.build();
        fuzz
    }

    fn count(&mut self, property: &'static str) {
        *self.checked.entry(property).or_default() += 1;
    }

    fn shots(&mut self, count: usize) -> Vec<ShotData> {
        (0..count)
            .map(|_| {
                let depth = self.rng.range(0.0, 20.0);
                let mut data = shot(
                    self.rng.range(0.0, 360.0),
                    self.rng.range(2.0, 15.0),
                    depth,
                    depth + self.rng.range(-1.0, 1.0),
                );
                self.uuid += 1;
                data.uuid = Some(format!("rs{}-{}", self.seed, self.uuid));
                data
            })
            .collect()
    }

    /// A line of its own, placed somewhere in a 600 m square.
    fn free_line(&mut self) -> Result<LineId, EditError> {
        let count = 1 + self.rng.below(5);
        let data = self.shots(count);
        let at = [self.rng.range(-300.0, 300.0), self.rng.range(-300.0, 300.0)];
        let name = format!("F{}", self.p.lines().len());
        self.p
            .create_line(meta(&name), StartAt::New, data, Some(at))
            .map(|e| e.value)
    }

    fn branch(&mut self, from: StationId) -> Result<LineId, EditError> {
        let count = 1 + self.rng.below(3);
        let data = self.shots(count);
        let name = format!("B{}", self.p.lines().len());
        self.p
            .create_line(meta(&name), StartAt::Station(from), data, None)
            .map(|e| e.value)
    }

    /// The starting survey: three to five free lines, a branch or two, a
    /// comment on about a third of the stations, a loop closure.
    fn build(&mut self) {
        for _ in 0..3 + self.rng.below(3) {
            let _ = self.free_line();
        }
        for _ in 0..=self.rng.below(2) {
            if let Some(from) = self.rng.pick(self.p.topology().stations()) {
                let _ = self.branch(from);
            }
        }
        let stations = self.p.topology().stations().to_vec();
        for station in stations {
            if self.rng.chance(35) {
                let text = format!("note-{}-{station}", self.seed);
                if let Some(s) = self.p.station_mut(station) {
                    s.comment = Some(text);
                }
            }
        }
        let violations = self.p.validate();
        assert!(violations.is_empty(), "seed {}: {violations:?}", self.seed);
    }

    // ----- picks ------------------------------------------------------------

    fn stations(&self) -> Vec<StationId> {
        self.p.topology().stations().to_vec()
    }

    fn lines(&self) -> Vec<LineId> {
        self.p.lines().iter().map(|l| l.id).collect()
    }

    fn slots(&self) -> Vec<StationRef> {
        self.p
            .lines()
            .iter()
            .flat_map(|l| (0..l.station_count()).map(move |i| StationRef::new(l.id, i as u32)))
            .collect()
    }

    fn junctions(&self) -> Vec<StationId> {
        let topology = self.p.topology();
        topology
            .stations()
            .iter()
            .copied()
            .filter(|&s| topology.is_junction(s))
            .collect()
    }

    /// A random shot of a random line: `(line, index, session id)`.
    fn some_shot(&mut self) -> Option<(LineId, usize, crate::ShotId)> {
        let line = self.rng.pick(&self.lines())?;
        let shots = &self.p.line(line)?.shots;
        if shots.is_empty() {
            return None;
        }
        let index = self.rng.below(shots.len());
        Some((line, index, shots[index].id))
    }

    // ----- one step ---------------------------------------------------------

    /// Run one random step and every check; `Err` names the first failed
    /// property.
    fn step(&mut self) -> Result<(), Failure> {
        let before = self.p.clone();
        let records = record_slots(&before);
        let shown_before = shown_comments(&before);
        let uuids_before: BTreeSet<String> = before
            .lines()
            .iter()
            .flat_map(|l| l.shots.iter().map(|s| s.uuid.clone()))
            .collect();
        let (label, outcome) = self.run_step();
        if self.trace {
            println!("rs seed {} step {}: {label}", self.seed, self.step);
        }
        self.log.push(label.clone());
        let ctx = match outcome {
            Err(failure) => return Err(failure),
            Ok(None) => {
                if self.p != before {
                    return Err(fail(
                        "rs-refusal",
                        format!("{label}: a refused step changed the project"),
                    ));
                }
                return Ok(());
            }
            Ok(Some(ctx)) => ctx,
        };
        let violations = self.p.validate();
        if !violations.is_empty() {
            return Err(fail(
                "rs-valid",
                format!("{label}: invalid after the step: {violations:?}"),
            ));
        }
        self.count("rs-stale-record");
        check_records(&records, &uuids_before, &self.p, &ctx.skip_record_nodes)
            .map_err(|f| prefixed(f, &label))?;
        self.count("rs-comment");
        check_comments(&shown_before, &self.p, &ctx).map_err(|f| prefixed(f, &label))?;
        if !ctx.lettered.is_empty() {
            self.count("rs-rule-l");
        }
        check_rule_l(&self.p, &ctx.lettered).map_err(|f| prefixed(f, &label))?;
        Ok(())
    }

    /// Draw and run one step: its label, and `Ok(None)` for a refusal,
    /// `Ok(Some(ctx))` for a success, `Err` for a failed step property.
    fn run_step(&mut self) -> (String, Result<Option<StepCtx>, Failure>) {
        match self.rng.below(40) {
            0 | 1 => self.step_free_line(),
            2..=4 => self.step_branch(),
            5..=11 => self.step_merge(),
            12..=16 => self.step_detach(),
            17..=21 => self.step_closure(),
            22 | 23 => self.step_remove_closure(),
            24..=26 => self.step_delete_shot(),
            27 => self.step_delete_range(),
            28 => self.step_delete_line(),
            29 | 30 => self.step_split_line(),
            31 => self.step_insert_station(),
            32..=34 => self.step_combine(),
            35 => self.step_reverse(),
            36 | 37 => self.step_comment(),
            38 => self.step_datum(),
            _ => self.step_append(),
        }
    }

    fn step_free_line(&mut self) -> (String, Result<Option<StepCtx>, Failure>) {
        let result = self.free_line();
        (
            "free line".to_string(),
            Ok(result.ok().map(|_| StepCtx::default())),
        )
    }

    fn step_branch(&mut self) -> (String, Result<Option<StepCtx>, Failure>) {
        let Some(from) = self.rng.pick(&self.stations()) else {
            return ("branch: none".to_string(), Ok(None));
        };
        let result = self.branch(from);
        (
            format!("branch from {from}"),
            Ok(result.ok().map(|_| StepCtx::default())),
        )
    }

    /// Merge two stations of different pieces with a random keep and
    /// answers; also the either-keep and merge-then-detach properties.
    fn step_merge(&mut self) -> (String, Result<Option<StepCtx>, Failure>) {
        let stations = self.stations();
        let Some(a) = self.rng.pick(&stations) else {
            return ("merge: none".to_string(), Ok(None));
        };
        let topology = self.p.topology_arc();
        let others: Vec<StationId> = stations
            .iter()
            .copied()
            .filter(|&s| s != a && !topology.connected(a, s))
            .collect();
        let Some(b) = self.rng.pick(&others) else {
            return ("merge: none".to_string(), Ok(None));
        };
        let keep = match self.rng.below(5) {
            0 => self.p.merge_default_keep(a, b),
            1 | 2 => a,
            _ => b,
        };
        let answers = Answers::draw(&mut self.rng);
        let label = format!("merge {a} {b} keep {keep} answers {answers:?}");
        let before = self.p.clone();
        if let Err(f) = self.either_keep(&before, a, b) {
            return (label, Err(f));
        }
        let preview = before.merge_preview(a, b, keep);
        let renames = preview.as_ref().and_then(|m| {
            let suggested: Vec<LetteredStation> =
                m.collisions.iter().map(|c| c.suggestion.clone()).collect();
            answers.renames(&suggested, &m.alternatives)
        });
        let result = match &renames {
            None => self.p.merge_stations_auto(a, b, keep),
            Some(renames) => self.p.merge_stations(a, b, keep, renames),
        };
        let collisions = preview.as_ref().map_or(0, |m| m.collisions.len());
        if let Err(f) = answers_verdict(answers, collisions, result.as_ref().err(), &label) {
            return (label, Err(f));
        }
        match result {
            Err(_) => {
                if preview.is_some() && answers == Answers::RuleL {
                    return (
                        label.clone(),
                        Err(fail(
                            "rs-preview",
                            format!("{label}: the preview went through, the merge was refused"),
                        )),
                    );
                }
                (label, Ok(None))
            }
            Ok(out) => {
                let Some(preview) = preview else {
                    return (
                        label.clone(),
                        Err(fail(
                            "rs-preview",
                            format!("{label}: the merge went through, its preview was refused"),
                        )),
                    );
                };
                let mut ctx = StepCtx::default();
                // Strict: only what the Merge dialog names is hidden: the
                // archived station, and the comments a promotion the merge's
                // sweep makes or a loop end it moves hides.
                ctx.hidden_named.insert(preview.hidden.0);
                ctx.absorb_hidden(&preview.comments_hidden);
                ctx.lettered.extend(out.identities.lettered.iter().cloned());
                let no_renames = renames.as_ref().is_none_or(Vec::is_empty);
                match merge_then_detach(&before, &self.p, &out, no_renames) {
                    Ok(true) => self.count("rs-merge-detach"),
                    Ok(false) => {}
                    Err(f) => return (label.clone(), Err(prefixed(f, &label))),
                }
                (label, Ok(Some(ctx)))
            }
        }
    }

    /// RS 4.1 step 3, M-P1-1, M-P2-9: the merge keeping `a` and keeping `b`
    /// (both answered by rule L) give the same raw layout bit for bit and the
    /// same Computed View within [`COMPUTED_TOL_M`], and are refused alike.
    fn either_keep(&mut self, before: &Project, a: StationId, b: StationId) -> Result<(), Failure> {
        let (mut ka, mut kb) = (before.clone(), before.clone());
        let (ra, rb) = (
            ka.merge_stations_auto(a, b, a),
            kb.merge_stations_auto(a, b, b),
        );
        match (&ra, &rb) {
            (Ok(_), Ok(_)) => {}
            (Err(_), Err(_)) => return Ok(()),
            _ => {
                return Err(fail(
                    "rs-keep-refusal",
                    format!(
                        "merge {a} {b}: keeping {a} gives {:?}, keeping {b} gives {:?}",
                        ra.as_ref().err(),
                        rb.as_ref().err()
                    ),
                ))
            }
        }
        self.count("rs-keep-layout");
        let (sa, sb) = (slot_positions(&ka, false), slot_positions(&kb, false));
        if let Some((slot, x, y)) = first_difference(&sa, &sb, 0.0) {
            return Err(fail(
                "rs-keep-layout",
                format!(
                    "merge {a} {b}: the raw layout at {slot:?} is {x:?} keeping {a} and {y:?} \
                     keeping {b}"
                ),
            ));
        }
        let (ca, cb) = (slot_positions(&ka, true), slot_positions(&kb, true));
        if let Some((slot, x, y)) = first_difference(&ca, &cb, COMPUTED_TOL_M) {
            return Err(fail(
                "rs-keep-computed",
                format!(
                    "merge {a} {b}: the Computed View at {slot:?} is {x:?} keeping {a} and \
                     {y:?} keeping {b}"
                ),
            ));
        }
        Ok(())
    }

    /// Detach a line from a junction, with a random `rest_keep` and answers.
    fn step_detach(&mut self) -> (String, Result<Option<StepCtx>, Failure>) {
        let Some(station) = self.rng.pick(&self.junctions()) else {
            return ("detach: none".to_string(), Ok(None));
        };
        let lines = self.p.topology().lines_of(station);
        let Some(line) = self.rng.pick(&lines) else {
            return ("detach: none".to_string(), Ok(None));
        };
        let naming = self.p.detach_naming(station, line, None);
        let rest_keep = match &naming {
            Ok(n) if !n.rest_choices.is_empty() && self.rng.chance(70) => {
                self.rng.pick(&n.rest_choices)
            }
            _ => None,
        };
        let naming = if rest_keep.is_some() {
            self.p.detach_naming(station, line, rest_keep)
        } else {
            naming
        };
        let answers = Answers::draw(&mut self.rng);
        let label = format!(
            "detach line {line} from {station} rest_keep {rest_keep:?} answers {answers:?}"
        );
        let renames = naming
            .as_ref()
            .ok()
            .and_then(|n| answers.renames(&n.collisions, &n.alternatives));
        let result = match &renames {
            None => self.p.detach_line_auto(station, line, rest_keep),
            Some(renames) => self.p.detach_line(station, line, renames, rest_keep),
        };
        let collisions = naming.as_ref().map_or(0, |n| n.collisions.len());
        if let Err(f) = answers_verdict(answers, collisions, result.as_ref().err(), &label) {
            return (label, Err(f));
        }
        match (result, naming) {
            (Err(_), Ok(_)) if answers == Answers::RuleL => (
                label.clone(),
                Err(fail(
                    "rs-preview",
                    format!("{label}: the preview went through, the detach was refused"),
                )),
            ),
            (Err(_), _) => (label, Ok(None)),
            (Ok(_), Err(e)) => (
                label.clone(),
                Err(fail(
                    "rs-preview",
                    format!("{label}: the detach went through, its preview was refused ({e})"),
                )),
            ),
            (Ok(out), Ok(naming)) => {
                // Strict, as for a merge: only what the Detach dialog names
                // (the comments its sweep's promotions or a moved loop end
                // hide) may become hidden.
                let mut ctx = StepCtx::default();
                ctx.absorb_hidden(&naming.comments_hidden);
                ctx.lettered.extend(out.identities.lettered.iter().cloned());
                (label, Ok(Some(ctx)))
            }
        }
    }

    /// Create a closure between two slots with a random keep (allowed,
    /// "Keep both names", or one C1 or C2 refuses) and answers; also the
    /// create-then-remove property.
    fn step_closure(&mut self) -> (String, Result<Option<StepCtx>, Failure>) {
        let slots = self.slots();
        let Some(sa) = self.rng.pick(&slots) else {
            return ("closure: none".to_string(), Ok(None));
        };
        let topology = self.p.topology_arc();
        let piece = self.p.station_at(sa).and_then(|s| topology.component_of(s));
        let same: Vec<StationRef> = slots
            .iter()
            .copied()
            .filter(|s| self.p.station_at(*s).and_then(|x| topology.component_of(x)) == piece)
            .collect();
        let pool = if self.rng.chance(75) { &same } else { &slots };
        let Some(sb) = self.rng.pick(pool) else {
            return ("closure: none".to_string(), Ok(None));
        };
        let (Some(a), Some(b)) = (self.p.station_at(sa), self.p.station_at(sb)) else {
            return ("closure: none".to_string(), Ok(None));
        };
        let choices = self.p.closure_keep_choices(a, b);
        let keep = match self.rng.below(10) {
            0..=4 => self.rng.pick(&choices.allowed),
            5 | 6 => None,
            7 => choices.default,
            _ => Some(if self.rng.chance(50) { a } else { b }),
        };
        let strategy = [
            ClosureStrategy::Batch,
            ClosureStrategy::Move,
            ClosureStrategy::LeastSquaresWeighted,
        ][self.rng.below(3)];
        let answers = Answers::draw(&mut self.rng);
        let label = format!("closure {sa:?} {sb:?} keep {keep:?} answers {answers:?}");
        let ends = ((a, Some(sa.line_id)), (b, Some(sb.line_id)));
        let naming = self.p.closure_naming(ends.0, ends.1, strategy, keep);
        let renames = naming
            .as_ref()
            .ok()
            .and_then(|n| answers.renames(&n.collisions, &n.alternatives));
        let before = self.p.clone();
        let result = match &renames {
            None => {
                self.p
                    .create_closure_auto(a, Some(sa.line_id), b, Some(sb.line_id), strategy, keep)
            }
            Some(renames) => self.p.create_closure(
                a,
                Some(sa.line_id),
                b,
                Some(sb.line_id),
                strategy,
                keep,
                renames,
            ),
        };
        let collisions = naming.as_ref().map_or(0, |n| n.collisions.len());
        if let Err(f) = answers_verdict(answers, collisions, result.as_ref().err(), &label) {
            return (label, Err(f));
        }
        if let (Some(k), Ok(_)) = (keep, &result) {
            if !choices.allowed.contains(&k) {
                return (
                    label.clone(),
                    Err(fail(
                        "rs-preview",
                        format!("{label}: a keep C1 or C2 refuses went through"),
                    )),
                );
            }
        }
        match (result, naming) {
            (Err(_), Ok(_)) if answers == Answers::RuleL => (
                label.clone(),
                Err(fail(
                    "rs-preview",
                    format!("{label}: the preview went through, the closure was refused"),
                )),
            ),
            (Err(_), _) => (label, Ok(None)),
            (Ok(_), Err(e)) => (
                label.clone(),
                Err(fail(
                    "rs-preview",
                    format!("{label}: the closure went through, its preview was refused ({e})"),
                )),
            ),
            (Ok(out), Ok(naming)) => {
                let mut ctx = StepCtx::default();
                if let Some((id, _, _)) = naming.identity {
                    ctx.hidden_named.insert(id);
                }
                ctx.absorb_report(&out.identities, &out.sweep);
                let no_renames = renames.as_ref().is_none_or(Vec::is_empty);
                match create_then_remove(&before, &self.p, &out, no_renames) {
                    Ok(true) => self.count("rs-closure-remove"),
                    Ok(false) => {}
                    Err(f) => return (label.clone(), Err(prefixed(f, &label))),
                }
                (label, Ok(Some(ctx)))
            }
        }
    }

    fn step_remove_closure(&mut self) -> (String, Result<Option<StepCtx>, Failure>) {
        let ids: Vec<ClosureId> = self.p.closures.iter().map(|c| c.id).collect();
        let Some(id) = self.rng.pick(&ids) else {
            return ("remove closure: none".to_string(), Ok(None));
        };
        let answers = Answers::draw(&mut self.rng);
        let label = format!("remove closure {id} answers {answers:?}");
        let naming = self.p.closure_removal(id);
        let renames = naming
            .as_ref()
            .ok()
            .and_then(|n| answers.renames(&n.collisions, &n.alternatives));
        let result = match &renames {
            None => self.p.remove_closure_auto(id),
            Some(renames) => self.p.remove_closure(id, renames),
        };
        let collisions = naming.as_ref().map_or(0, |n| n.collisions.len());
        if let Err(f) = answers_verdict(answers, collisions, result.as_ref().err(), &label) {
            return (label, Err(f));
        }
        match result {
            Err(_) => (label, Ok(None)),
            Ok(out) => {
                let mut ctx = StepCtx::default();
                ctx.absorb_report(&out.identities, &out.sweep);
                (label, Ok(Some(ctx)))
            }
        }
    }

    /// Delete one shot; its preview names what becomes hidden.
    fn step_delete_shot(&mut self) -> (String, Result<Option<StepCtx>, Failure>) {
        let Some((line, index, shot)) = self.some_shot() else {
            return ("delete shot: none".to_string(), Ok(None));
        };
        let label = format!("delete shot {line}/{index}");
        let preview = self.p.preview_delete_shot(line, shot);
        match self.p.delete_shot(line, shot) {
            Err(_) => (label, Ok(None)),
            Ok(edited) => {
                let mut ctx = StepCtx::default();
                match preview {
                    Ok(preview) => {
                        ctx.absorb_restores(&preview.identities);
                        for promotion in &preview.promoted_closures {
                            ctx.hidden_named.insert(promotion.archived);
                        }
                    }
                    Err(e) => {
                        return (
                            label.clone(),
                            Err(fail(
                                "rs-preview",
                                format!(
                                "{label}: the delete went through, its preview was refused ({e})"
                            ),
                            )),
                        )
                    }
                }
                ctx.lettered
                    .extend(edited.report.identities.lettered.iter().cloned());
                ctx.lettered
                    .extend(edited.report.sweep.lettered.iter().cloned());
                (label, Ok(Some(ctx)))
            }
        }
    }

    /// Delete a range of one or two shots, gap-close or split.
    fn step_delete_range(&mut self) -> (String, Result<Option<StepCtx>, Failure>) {
        let Some((line, index, first)) = self.some_shot() else {
            return ("delete range: none".to_string(), Ok(None));
        };
        let shots = self.p.line(line).map_or(0, |l| l.shots.len());
        let last_index = (index + self.rng.below(2)).min(shots.saturating_sub(1));
        let Some(last) = self.p.line(line).map(|l| l.shots[last_index].id) else {
            return ("delete range: none".to_string(), Ok(None));
        };
        let step = self.step;
        let mode = if self.rng.chance(50) {
            RangeDeleteMode::Shift
        } else {
            RangeDeleteMode::Split {
                name_a: format!("RA{step}"),
                name_b: format!("RB{step}"),
            }
        };
        let label = format!("delete range {line}/{index}..={last_index} {mode:?}");
        let preview = self.p.preview_delete_shot_range(line, first, last);
        let split = matches!(mode, RangeDeleteMode::Split { .. });
        match self.p.delete_shot_range(line, first, last, &mode) {
            Err(_) => (label, Ok(None)),
            Ok(edited) => {
                let mut ctx = StepCtx::default();
                match preview {
                    // The dialog's Split choice of a range whose gap-close is
                    // offered: what the split would do (RS 12).
                    Ok(preview) if split && preview.split_weld.is_some() => {
                        if let Some(weld) = &preview.split_weld {
                            ctx.absorb_restores(&weld.identities);
                        }
                    }
                    Ok(preview) => {
                        ctx.absorb_restores(&preview.identities);
                        for promotion in &preview.promoted_closures {
                            ctx.hidden_named.insert(promotion.archived);
                        }
                    }
                    Err(e) => {
                        return (
                            label.clone(),
                            Err(fail(
                                "rs-preview",
                                format!(
                                "{label}: the delete went through, its preview was refused ({e})"
                            ),
                            )),
                        )
                    }
                }
                ctx.lettered
                    .extend(edited.report.identities.lettered.iter().cloned());
                ctx.lettered
                    .extend(edited.report.sweep.lettered.iter().cloned());
                (label, Ok(Some(ctx)))
            }
        }
    }

    fn step_delete_line(&mut self) -> (String, Result<Option<StepCtx>, Failure>) {
        if self.p.lines().len() < 3 {
            return ("delete line: too few".to_string(), Ok(None));
        }
        let Some(line) = self.rng.pick(&self.lines()) else {
            return ("delete line: none".to_string(), Ok(None));
        };
        let label = format!("delete line {line}");
        match self.p.delete_line(line) {
            Err(_) => (label, Ok(None)),
            Ok(edited) => {
                let mut ctx = StepCtx::default();
                ctx.absorb_report(&edited.report.identities, &edited.report.sweep);
                (label, Ok(Some(ctx)))
            }
        }
    }

    fn step_split_line(&mut self) -> (String, Result<Option<StepCtx>, Failure>) {
        let Some(line) = self.rng.pick(&self.lines()) else {
            return ("split line: none".to_string(), Ok(None));
        };
        let count = self.p.line(line).map_or(0, |l| l.shots.len());
        if count < 2 {
            return ("split line: too short".to_string(), Ok(None));
        }
        let index = 1 + self.rng.below(count - 1);
        let step = self.step;
        let options = SplitOptions {
            name_a: format!("SA{step}"),
            name_b: format!("SB{step}"),
            color_a: "#112233".to_string(),
            color_b: "#445566".to_string(),
        };
        let label = format!("split line {line} at {index}");
        let at = SplitAt::Station {
            station_index: index as u32,
        };
        match self.p.split_line(line, at, &options) {
            Err(_) => (label, Ok(None)),
            Ok(edited) => {
                let mut ctx = StepCtx::default();
                ctx.absorb_report(&edited.report.identities, &edited.report.sweep);
                (label, Ok(Some(ctx)))
            }
        }
    }

    fn step_insert_station(&mut self) -> (String, Result<Option<StepCtx>, Failure>) {
        let Some((line, index, shot)) = self.some_shot() else {
            return ("insert station: none".to_string(), Ok(None));
        };
        let length = self.p.line(line).map_or(0.0, |l| l.shots[index].length);
        let distance = length * self.rng.range(0.2, 0.8);
        self.uuid += 1;
        let uuid = format!("rs{}-{}", self.seed, self.uuid);
        let label = format!("insert station {line}/{index} at {distance}");
        match self
            .p
            .split_shot_at_distance(line, shot, distance, Some(uuid))
        {
            Err(_) => (label, Ok(None)),
            Ok(edited) => {
                let mut ctx = StepCtx::default();
                ctx.absorb_report(&edited.report.identities, &edited.report.sweep);
                (label, Ok(Some(ctx)))
            }
        }
    }

    /// Combine two lines at a line end, keeping either, with a random comment
    /// pick.
    fn step_combine(&mut self) -> (String, Result<Option<StepCtx>, Failure>) {
        let Some(line) = self.rng.pick(&self.lines()) else {
            return ("combine: none".to_string(), Ok(None));
        };
        let count = self.p.line(line).map_or(0, |l| l.shots.len());
        let end = if self.rng.chance(50) { 0 } else { count };
        let offered: Vec<_> = self
            .p
            .combine_candidates(StationRef::new(line, end as u32))
            .into_iter()
            .filter(|c| c.blocked_reason.is_none())
            .collect();
        let Some(candidate) = self.rng.pick(&offered) else {
            return ("combine: none".to_string(), Ok(None));
        };
        let keep_a = self.rng.chance(50);
        let (kept, other, join) = if keep_a {
            (
                candidate.line_a,
                candidate.line_b,
                CombineJoinEnds {
                    kept: candidate.end_a,
                    other: candidate.end_b,
                },
            )
        } else {
            (
                candidate.line_b,
                candidate.line_a,
                CombineJoinEnds {
                    kept: candidate.end_b,
                    other: candidate.end_a,
                },
            )
        };
        let pick = [CombinePick::Kept, CombinePick::Other, CombinePick::Both][self.rng.below(3)];
        let step = self.step;
        let options = CombineLinesOptions {
            name: format!("C{step}"),
            color: "#123456".to_string(),
            swap_lr: self.rng.chance(50),
            comment_pick: pick,
            name_pick: CombinePick::Kept,
            join: Some(join),
        };
        let joins = [candidate.station_a, candidate.station_b];
        let label = format!("combine keep {kept} other {other} {join:?} pick {pick:?}");
        match self.p.combine_lines(kept, other, &options) {
            Err(_) => (label, Ok(None)),
            Ok(edited) => {
                let mut ctx = StepCtx::default();
                ctx.skip_record_nodes.extend(joins);
                // The user picks the joined station's comment (RS 4.4) when
                // the join station not kept becomes history; when it stays
                // archived for a third line the pick does not apply, and its
                // hidden comment must be in the report (model review
                // finding 3).
                if edited.value.join_archived.is_none() {
                    ctx.edited.extend(joins);
                    ctx.joined = pick == CombinePick::Both;
                }
                ctx.absorb_report(&edited.value.identities, &edited.report.sweep);
                ctx.absorb_report(&edited.report.identities, &SweepReport::default());
                (label, Ok(Some(ctx)))
            }
        }
    }

    fn step_reverse(&mut self) -> (String, Result<Option<StepCtx>, Failure>) {
        let Some(line) = self.rng.pick(&self.lines()) else {
            return ("reverse: none".to_string(), Ok(None));
        };
        let swap = self.rng.chance(50);
        let label = format!("reverse {line}");
        match self.p.reverse_line(line, swap) {
            Err(_) => (label, Ok(None)),
            Ok(edited) => {
                let mut ctx = StepCtx::default();
                ctx.absorb_report(&edited.report.identities, &edited.report.sweep);
                (label, Ok(Some(ctx)))
            }
        }
    }

    /// Set or clear a comment through a slot: it edits the identity the slot
    /// shows (RS 5.4).
    fn step_comment(&mut self) -> (String, Result<Option<StepCtx>, Failure>) {
        let Some(slot) = self.rng.pick(&self.slots()) else {
            return ("comment: none".to_string(), Ok(None));
        };
        let Some(owner) = self.p.slot_owner(slot) else {
            return ("comment: none".to_string(), Ok(None));
        };
        let text = self
            .rng
            .chance(75)
            .then(|| format!("set-{}-{}", self.seed, self.step));
        let label = format!("comment through {slot:?} on {}: {text:?}", owner.shown);
        self.p.set_shown_comment(owner.node, text);
        let mut ctx = StepCtx::default();
        ctx.edited.insert(owner.shown);
        (label, Ok(Some(ctx)))
    }

    fn step_datum(&mut self) -> (String, Result<Option<StepCtx>, Failure>) {
        let Some(slot) = self.rng.pick(&self.slots()) else {
            return ("datum: none".to_string(), Ok(None));
        };
        let Some(station) = self.p.station_at(slot) else {
            return ("datum: none".to_string(), Ok(None));
        };
        let gps = gps_at(
            self.rng.range(-2000.0, 2000.0),
            self.rng.range(-2000.0, 2000.0),
        );
        let label = format!("datum at {slot:?}");
        match self.p.set_datum(station, Some(slot.line_id), gps, None) {
            Err(_) => (label, Ok(None)),
            Ok(out) => {
                let mut ctx = StepCtx::default();
                ctx.absorb_report(&out.identities, &out.sweep);
                (label, Ok(Some(ctx)))
            }
        }
    }

    /// Add .stalx: a small source with a junction, a comment and, sometimes,
    /// a merge record and a closure archive of its own.
    fn step_append(&mut self) -> (String, Result<Option<StepCtx>, Failure>) {
        let mut source = Fuzz {
            p: project(),
            rng: Rng(self.rng.next()),
            uuid: self.uuid,
            seed: self.seed,
            step: 0,
            trace: false,
            checked: BTreeMap::new(),
            log: Vec::new(),
        };
        source.build();
        for _ in 0..3 {
            let _ = source.step_merge();
            let _ = source.step_closure();
        }
        self.uuid = source.uuid + 1;
        if !source.p.validate().is_empty() {
            return ("append: invalid source".to_string(), Ok(None));
        }
        let label = format!(
            "append {} lines, {} records",
            source.p.lines().len(),
            source
                .p
                .stations
                .values()
                .map(|s| s.archived.len())
                .sum::<usize>()
        );
        match self.p.append_project(&source.p) {
            Err(_) => (label, Ok(None)),
            Ok(out) => {
                let mut ctx = StepCtx::default();
                ctx.absorb_report(&out.identities, &out.sweep);
                (label, Ok(Some(ctx)))
            }
        }
    }
}

fn prefixed(f: Failure, label: &str) -> Failure {
    if f.detail.starts_with(label) {
        f
    } else {
        fail(f.property, format!("{label}: {}", f.detail))
    }
}

/// `rs-answers`: the preview's suggested renames are accepted, and a step
/// given no answers is refused for a collision only when its preview listed
/// one.
fn answers_verdict(
    answers: Answers,
    collisions: usize,
    error: Option<&OpError>,
    label: &str,
) -> Result<(), Failure> {
    match (answers, error) {
        (Answers::Suggested, Some(e @ OpError::NameTaken { .. })) => Err(fail(
            "rs-answers",
            format!("{label}: the preview's suggested renames were refused: {e}"),
        )),
        (Answers::Nothing, Some(e @ OpError::NameTaken { .. })) if collisions == 0 => Err(fail(
            "rs-answers",
            format!("{label}: refused for a collision its preview did not list: {e}"),
        )),
        _ => Ok(()),
    }
}

// ----- the properties ---------------------------------------------------------

/// Every slot's position, line by line in chain order: the raw layout, or
/// with `computed` the Computed View.
fn slot_positions(p: &Project, computed: bool) -> Vec<(StationRef, [f64; 3])> {
    let raw = p.raw_positions();
    let adjusted =
        computed.then(|| p.adjusted_positions_from(&raw, &ClosureAdjustmentConfig::default()));
    let mut out = Vec::new();
    for line in p.lines() {
        for (index, station) in line.stations().enumerate() {
            let at = match &adjusted {
                Some(view) => view.xy(station).zip(view.z(station)),
                None => raw.xy(station).zip(raw.z(station)),
            };
            let [x, y] = at.map_or([f64::NAN, f64::NAN], |(xy, _)| xy);
            let z = at.map_or(f64::NAN, |(_, z)| z);
            out.push((StationRef::new(line.id, index as u32), [x, y, z]));
        }
    }
    out
}

/// The first slot where two layouts differ by more than `tol` (bit for bit
/// when `tol` is zero).
fn first_difference(
    a: &[(StationRef, [f64; 3])],
    b: &[(StationRef, [f64; 3])],
    tol: f64,
) -> Option<(StationRef, [f64; 3], [f64; 3])> {
    if a.len() != b.len() {
        let slot = a
            .first()
            .or(b.first())
            .map_or(StationRef::new(0, 0), |s| s.0);
        return Some((slot, [a.len() as f64; 3], [b.len() as f64; 3]));
    }
    a.iter().zip(b).find_map(|((sa, x), (sb, y))| {
        let differs = sa != sb
            || x.iter().zip(y).any(|(u, v)| {
                if tol == 0.0 {
                    u.to_bits() != v.to_bits()
                } else {
                    (u - v).abs() > tol || u.is_nan() != v.is_nan()
                }
            });
        differs.then_some((*sa, *x, *y))
    })
}

/// A record's slot before a step: the record, its node, the line it names,
/// and the shot next to that slot (`to_end`: the slot is that shot's end,
/// else its start).
#[derive(Debug, Clone)]
struct RecordAt {
    record: StationId,
    name: String,
    node: StationId,
    line: LineId,
    shot: String,
    to_end: bool,
}

fn record_slots(p: &Project) -> Vec<RecordAt> {
    let mut out = Vec::new();
    for station in p.stations.values() {
        for record in &station.archived {
            for &line in &record.lines {
                let Some(l) = p.line(line) else { continue };
                let Some(index) = l.stations().position(|s| s == station.id) else {
                    continue;
                };
                let (shot, to_end) = if index > 0 {
                    (l.shots[index - 1].uuid.clone(), true)
                } else if let Some(first) = l.shots.first() {
                    (first.uuid.clone(), false)
                } else {
                    continue;
                };
                out.push(RecordAt {
                    record: record.id,
                    name: record.name.clone(),
                    node: station.id,
                    line,
                    shot,
                    to_end,
                });
            }
        }
    }
    out
}

/// `rs-stale-record` (RS 4.3, U-P1-4, M-P1-4c): every record that named a
/// line at a node before the step, whose shot next to that slot is still on
/// some line, is the node at that slot now, or a record of it naming that
/// line, or (a tear of a line through one junction twice) a node or record
/// elsewhere on that line; never history. The slot is found by the shot
/// (wherever a split, a range move or a combine took it): its two ends, and
/// past them any shots the step made (Insert Station, a reversal), are the
/// candidates.
fn check_records(
    records: &[RecordAt],
    uuids_before: &BTreeSet<String>,
    after: &Project,
    skip: &BTreeSet<StationId>,
) -> Result<(), Failure> {
    let mut where_is: BTreeMap<&str, (LineId, usize)> = BTreeMap::new();
    for line in after.lines() {
        for (index, shot) in line.shots.iter().enumerate() {
            where_is.insert(shot.uuid.as_str(), (line.id, index));
        }
    }
    for r in records {
        if skip.contains(&r.node) {
            continue;
        }
        let Some(&(line, index)) = where_is.get(r.shot.as_str()) else {
            continue;
        };
        let Some(l) = after.line(line) else { continue };
        let chain: Vec<StationId> = l.stations().collect();
        let mut candidates = vec![chain[index], chain[index + 1]];
        let mut k = index;
        while k > 0 && !uuids_before.contains(&l.shots[k - 1].uuid) {
            k -= 1;
            candidates.push(chain[k]);
        }
        let mut k = index;
        while k + 1 < l.shots.len() && !uuids_before.contains(&l.shots[k + 1].uuid) {
            k += 1;
            candidates.push(chain[k + 1]);
        }
        let holds = |node: StationId| {
            node == r.record
                || after
                    .record_naming(node, line)
                    .is_some_and(|rec| rec.id == r.record)
        };
        // A tear of a junction a line passes twice gives the record a node of
        // its own at the other slot of that line (deviation RS-M1, "a tear
        // with a several-line record"): it is not history, so it passes.
        let elsewhere_on_line = after.topology().lines_of(r.record).contains(&line)
            || after.stations.values().any(|s| {
                s.archived
                    .iter()
                    .any(|rec| rec.id == r.record && rec.lines.contains(&line))
            });
        // A tear of one line of a several-line record (deviation RS-M1, "a
        // tear with a several-line record", kept by RS-FX1): the record went
        // with the torn line onto its fresh node, which the tear closure ties
        // to the junction, so the record's other lines show the junction
        // natively there. An id is one node or one record (A1), and the tear
        // must separate the lines (they would close the loop again), so one
        // side has to do without it; the record is not history.
        let torn_off = after.stations.contains_key(&r.record)
            && after.closures.iter().any(|c| {
                (c.a == r.record && candidates.contains(&c.b))
                    || (c.b == r.record && candidates.contains(&c.a))
            });
        if !candidates.iter().any(|&c| holds(c)) && !elsewhere_on_line && !torn_off {
            let now = if after.stations.contains_key(&r.record) {
                format!("a node on {:?}", after.topology().lines_of(r.record))
            } else {
                after
                    .stations
                    .values()
                    .find_map(|s| {
                        s.archived
                            .iter()
                            .find(|rec| rec.id == r.record)
                            .map(|rec| format!("a record of {} naming {:?}", s.id, rec.lines))
                    })
                    .unwrap_or_else(|| "gone".to_string())
            };
            return Err(fail(
                "rs-stale-record",
                format!(
                    "record {} ({:?}) of node {} named line {} at the {} of shot {}; that shot is \
                     now on line {line} between {candidates:?}, none of which is the record or \
                     holds it for line {line} (it is {now})",
                    r.record,
                    r.name,
                    r.node,
                    r.line,
                    if r.to_end { "end" } else { "start" },
                    r.shot
                ),
            ));
        }
    }
    Ok(())
}

/// The comment each shown identity shows (identities shown on at least one
/// slot, with a comment that has text).
fn shown_comments(p: &Project) -> BTreeMap<StationId, String> {
    let index = p.shown_index();
    let mut out = BTreeMap::new();
    for &node in p.topology().stations() {
        let shown = index.shown(node);
        if let Some(text) = p
            .stations
            .get(&shown)
            .and_then(|s| s.comment.clone())
            .filter(|c| !c.is_empty())
        {
            out.insert(shown, text);
        }
    }
    out
}

/// Whether `id` is still a station or a record somewhere.
fn id_exists(p: &Project, id: StationId) -> bool {
    p.stations.contains_key(&id) || p.is_archived_id(id)
}

/// `rs-comment` (RS 12, U-P1-3): every comment shown before the step is
/// shown after it, or its station is gone with its slots, or the step's
/// preview (its report, for a step without one) names it.
fn check_comments(
    before: &BTreeMap<StationId, String>,
    after: &Project,
    ctx: &StepCtx,
) -> Result<(), Failure> {
    let now = shown_comments(after);
    for (&id, text) in before {
        if now.get(&id) == Some(text)
            || !id_exists(after, id)
            || ctx.edited.contains(&id)
            || ctx.hidden_named.contains(&id)
            || ctx
                .moved
                .iter()
                .any(|&(old, new)| old == id && now.get(&new) == Some(text))
            || (ctx.joined && now.values().any(|c| c.contains(text.as_str())))
        {
            continue;
        }
        let shown_as = now.iter().find(|(_, c)| *c == text).map(|(id, _)| *id);
        return Err(fail(
            "rs-comment",
            format!(
                "the comment {text:?} of {id}, shown before, is not shown after (now {:?}; the \
                 text is shown by {shown_as:?}) and nothing names it hidden",
                now.get(&id)
            ),
        ));
    }
    Ok(())
}

/// `rs-rule-l` (RS 2): no reported letter went to an identity shown on more
/// lines than the identity it collided with.
fn check_rule_l(p: &Project, lettered: &[LetteredStation]) -> Result<(), Failure> {
    for letter in lettered {
        let (mine, theirs) = (
            p.lines_shown_on(letter.station).len(),
            p.lines_shown_on(letter.other).len(),
        );
        if mine > theirs {
            return Err(fail(
                "rs-rule-l",
                format!(
                    "{} ({:?} -> {:?}) was lettered on line {} though it is shown on {mine} \
                     lines and {} only on {theirs}",
                    letter.station, letter.from, letter.to, letter.line, letter.other
                ),
            ));
        }
    }
    Ok(())
}

/// Every slot's node, line by line.
fn chains(p: &Project) -> Vec<(LineId, Vec<StationId>)> {
    p.lines()
        .iter()
        .map(|l| (l.id, l.stations().collect()))
        .collect()
}

/// `rs-merge-detach` (RS 12): detaching the archived station's line right
/// after a merge restores every shown identity and record, byte for byte,
/// when the merge made nothing else happen (no sweep, restore, letter or
/// renames): the slots, and every station with its records and their
/// owners. A junction the merge archived comes back whole, with the records
/// it held still inside it (RS-FX4; the flat-record exception RS-T made
/// before records had owners is gone). `Ok(true)` when it was checked.
fn merge_then_detach(
    before: &Project,
    merged: &Project,
    out: &OpOutcome<super::MergeReport>,
    no_renames: bool,
) -> Result<bool, Failure> {
    if !no_renames || !out.sweep.is_empty() || !out.identities.is_empty() {
        return Ok(false);
    }
    let (x, node) = (out.value.archived, out.value.kept);
    let Some(record) = merged
        .stations
        .get(&node)
        .and_then(|s| s.archived.iter().find(|r| r.id == x))
        .cloned()
    else {
        return Err(fail(
            "rs-merge-detach",
            format!("the merge kept {node} but holds no record {x}"),
        ));
    };
    if record.into.is_some() {
        return Err(fail(
            "rs-merge-detach",
            format!(
                "the merge archived {x} under {:?}, not under {node}",
                record.into
            ),
        ));
    }
    let Some(&line) = record.lines.first() else {
        return Err(fail(
            "rs-merge-detach",
            format!("the archived station {x} names no line"),
        ));
    };
    let mut copy = merged.clone();
    let detached = copy.detach_line_auto(node, line, None).map_err(|e| {
        fail(
            "rs-merge-detach",
            format!("detaching line {line} of the archived {x} from {node} was refused: {e}"),
        )
    })?;
    if !detached.sweep.is_empty() || !detached.identities.is_empty() {
        return Ok(false);
    }
    if !before.stations.contains_key(&x) {
        return Ok(false);
    }
    if chains(&copy) != chains(before) {
        return Err(fail(
            "rs-merge-detach",
            format!("after the merge and the detach of {x}'s line {line} the slots differ"),
        ));
    }
    if copy.stations != before.stations {
        let differs = before
            .stations
            .iter()
            .find(|(id, s)| copy.stations.get(id) != Some(s))
            .map(|(id, s)| (id, s, copy.stations.get(id)));
        return Err(fail(
            "rs-merge-detach",
            format!(
                "after the merge and the detach of {x}'s line {line} the stations differ: \
                 {differs:?}"
            ),
        ));
    }
    Ok(true)
}

/// `rs-closure-remove` (RS 12): removing a closure right after creating it
/// restores every shown identity and record, when the creation lettered and
/// renamed nothing. `Ok(true)` when it was checked.
fn create_then_remove(
    before: &Project,
    created: &Project,
    out: &OpOutcome<super::ClosureCreated>,
    no_renames: bool,
) -> Result<bool, Failure> {
    if !no_renames || !out.identities.is_empty() {
        return Ok(false);
    }
    let id = out.value.closure.id;
    let mut copy = created.clone();
    let removed = copy.remove_closure_auto(id).map_err(|e| {
        fail(
            "rs-closure-remove",
            format!("removing the closure {id} just created was refused: {e}"),
        )
    })?;
    if !removed.identities.is_empty() {
        return Err(fail(
            "rs-closure-remove",
            format!(
                "removing the closure {id} just created changed identities: {:?}",
                removed.identities
            ),
        ));
    }
    if copy.stations != before.stations || copy.closures != before.closures {
        return Err(fail(
            "rs-closure-remove",
            format!("removing the closure {id} just created does not restore the stations"),
        ));
    }
    Ok(true)
}

// ----- runs -------------------------------------------------------------------

/// One seed: `Ok` with what it checked, or the failure with the step log.
fn run_seed(
    seed: u32,
    steps: usize,
    trace: bool,
) -> Result<BTreeMap<&'static str, usize>, (usize, Failure, Vec<String>)> {
    let mut fuzz = Fuzz::new(seed, trace);
    for step in 0..steps {
        fuzz.step = step;
        if let Err(failure) = fuzz.step() {
            return Err((step, failure, fuzz.log));
        }
    }
    Ok(fuzz.checked)
}

fn env_u32(name: &str) -> Option<u32> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

#[test]
fn rs_properties_hold_on_the_default_seeds() {
    let mut checked: BTreeMap<&'static str, usize> = BTreeMap::new();
    for seed in 1..=DEFAULT_SEEDS {
        match run_seed(seed, DEFAULT_STEPS, false) {
            Ok(counts) => {
                for (k, v) in counts {
                    *checked.entry(k).or_default() += v;
                }
            }
            Err((step, failure, log)) => panic!(
                "seed {seed} step {step}: {}: {}\nsteps: {log:#?}",
                failure.property, failure.detail
            ),
        }
    }
    println!("rs gate: checked {checked:?}");
    // A property never checked would pass vacuously.
    for property in [
        "rs-stale-record",
        "rs-comment",
        "rs-rule-l",
        "rs-keep-layout",
        "rs-merge-detach",
        "rs-closure-remove",
    ] {
        assert!(
            checked.get(property).copied().unwrap_or(0) > 0,
            "the default seeds never checked {property}: {checked:?}"
        );
    }
}

/// The farm's first round on the RS integration (package RS-FX1): each seed
/// failed with the signature in its comment, at the steps the farm ran it
/// with (`rs-props` 50, `rs-props-long` 200). Pinned so they stay fixed;
/// the hand-built tests in `kept_report_tests` pin each mechanism.
const FARM_SEEDS: [(u32, usize); 9] = [
    // 4909b48e2ea4 rs-comment, merge: a loop end merged keeping the other
    // station hid the kept one (model review finding 1).
    (480_000_039, 200),
    // 647783847b78 rs-comment, detach line: a promotion its sweep made.
    (480_000_033, 200),
    // 7a5ed348b262 rs-comment, delete range: the Split choice's promotion.
    (480_000_143, 200),
    // 56ce8638669d rs-comment, delete shot: a promotion kept a loop end's
    // partner and hid it (model review finding 1, sweep path).
    (400_002_693, 50),
    // 4f602aeda8e9 rs-comment, delete line: a restore moved a comment that
    // a promotion then archived.
    (480_000_296, 200),
    // e546f002311b rs-answers, detach line: renames judged before the sweep.
    (400_000_326, 50),
    (400_002_173, 50),
    // c36d3c33f104 rs-rule-l, delete line: the sweep lettered in a cycle.
    (400_010_005, 50),
    // f953c8f1b475 rs-stale-record, combine keep: a tear of a several-line
    // record (the oracle's excuse, deviation RS-M1 kept by RS-FX1).
    (480_000_089, 200),
];

#[test]
fn rs_properties_hold_on_the_farm_seeds() {
    for (seed, steps) in FARM_SEEDS {
        if let Err((step, failure, log)) = run_seed(seed, steps, false) {
            panic!(
                "seed {seed} step {step}: {}: {}\nsteps: {log:#?}",
                failure.property, failure.detail
            );
        }
    }
}

#[test]
fn the_rs_fuzzer_is_deterministic() {
    let a = run_seed(3, 20, false).map(|c| c.len());
    let b = run_seed(3, 20, false).map(|c| c.len());
    assert_eq!(format!("{a:?}"), format!("{b:?}"));
    let (fa, fb) = (Fuzz::new(7, false), Fuzz::new(7, false));
    assert_eq!(fa.p, fb.p);
}

/// The farm's window (module docs). Catches a panic per seed, so one bad
/// seed does not hide the rest of the window.
#[test]
#[ignore = "sweep: STALAGMIGHT_RUST_RS_SEEDS seeds from STALAGMIGHT_RUST_RS_FIRST_SEED"]
fn rs_property_sweep() {
    let seeds = env_u32("STALAGMIGHT_RUST_RS_SEEDS").unwrap_or(200);
    let first = env_u32("STALAGMIGHT_RUST_RS_FIRST_SEED").unwrap_or(1);
    let steps = env_u32("STALAGMIGHT_RUST_RS_STEPS").map_or(DEFAULT_STEPS, |s| s as usize);
    let trace = std::env::var("STALAGMIGHT_RUST_RS_TRACE").is_ok_and(|v| v == "1");
    let mut failed = 0usize;
    let mut checked: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut report = String::new();
    for seed in first..first.saturating_add(seeds) {
        let result = std::panic::catch_unwind(|| run_seed(seed, steps, trace));
        match result {
            Ok(Ok(counts)) => {
                for (k, v) in counts {
                    *checked.entry(k).or_default() += v;
                }
            }
            Ok(Err((step, failure, log))) => {
                failed += 1;
                println!(
                    "seed {seed} step {step}: {}: {}",
                    failure.property, failure.detail
                );
                let _ = writeln!(report, "seed {seed}: {}", log.join(" | "));
            }
            Err(panic) => {
                failed += 1;
                let message = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_string()))
                    .unwrap_or_default();
                println!("seed {seed}: PANICKED rs-panic: {message}");
            }
        }
    }
    if !report.is_empty() {
        println!("steps of the failing seeds:\n{report}");
    }
    println!("rs sweep: checked {checked:?}");
    println!("rs sweep: seeds with a failure ({failed}) of {seeds}");
    assert_eq!(failed, 0, "{failed} of {seeds} seeds failed");
}

// ----- regressions the properties found ----------------------------------------

/// X (0, 1, 2, 3 east) and Y (0, 1, 2) with Y's 1 merged into X's 1, which is
/// kept (a record of Y's 1 names Y), and a loop closure from that junction,
/// via Y, to X's 3: detaching X from the junction gives Y its own 1 back
/// (RS 4.2 case 2) on a piece only the closure joins to X, so the sweep
/// promotes it. The epilogue's work must reach the Detach preview. With
/// `keep`, the closure keeps the junction (X's 3 is archived).
fn detach_into_a_promotion(keep: bool) -> (Project, LineId, LineId, [StationId; 3]) {
    use super::ops_test_helpers::{free_line, ok, run};
    use super::test_helpers::at;
    let mut p = project();
    let x = free_line(&mut p, [0.0, 0.0], run(90.0, 3, 10.0));
    let y = free_line(&mut p, [5.0, 40.0], run(0.0, 2, 6.0));
    let (x1, x3, y1) = (at(&p, x, 1), at(&p, x, 3), at(&p, y, 1));
    ok(p.merge_stations_auto(y1, x1, x1));
    let keep = keep.then_some(x1);
    ok(p.create_closure(x1, Some(y), x3, Some(x), ClosureStrategy::Move, keep, &[]));
    assert!(p.validate().is_empty());
    (p, x, y, [x1, x3, y1])
}

/// Seed 7 of the gate: the detach's sweep promoted the closure that alone
/// joined the restored station's piece, archiving that station, and the
/// Detach preview refused ("Station 25 does not exist") a detach that goes
/// through. The preview now names the station the lines got as the detach
/// left it, a record of the promoted node.
#[test]
fn the_detach_preview_survives_a_promotion_of_the_station_it_restores() {
    let (p, x, y, [x1, x3, y1]) = detach_into_a_promotion(false);
    assert!(p.detach_keeps(x1, x));
    let naming = p.detach_naming(x1, x, None).unwrap();
    assert_eq!(naming.id, y1);
    assert!(naming.restored && naming.kept);
    let mut q = p;
    let out = q.detach_line_auto(x1, x, None).unwrap();
    assert_eq!(out.sweep.promoted.len(), 1, "the closure is promoted");
    assert!(
        !q.stations.contains_key(&y1),
        "the promotion archived Y's 1"
    );
    let on_y = naming.lines.iter().find(|l| l.line == y).unwrap();
    assert_eq!(on_y.station, x3, "Y ends up on X's 3, as the preview says");
}

/// Seed 13 of the gate: a detach whose own collision (the restored station's
/// name already on its line) was lettered by the orphan-closure sweep, which
/// letters every collision standing when its promotions end. The letter was
/// in `SweepReport::lettered` only, so the preview listed no collision, the
/// dialog's answers (none) were refused with `NameTaken`, and no notice
/// named the letter. Every letter is now in the identity report.
#[test]
fn a_collision_the_sweep_letters_is_in_the_preview_and_the_notice() {
    // The closure archives X's 3 (it shows the junction), so the promotion
    // keeps the restored station (its kept end now, RS 5.4).
    let (mut p, x, y, [x1, _, y1]) = detach_into_a_promotion(true);
    let y2 = p.line_stations(y).unwrap()[2];
    p.set_shown_name(x1, "N".to_string());
    p.set_shown_name(y2, "1".to_string());
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    // Detach Y: its own 1 comes back, beside the 2 renamed 1.
    let naming = p.detach_naming(x1, y, None).unwrap();
    assert_eq!((naming.id, naming.restored), (y1, true));
    assert_eq!(naming.collisions.len(), 1, "{naming:?}");
    let renames: Vec<(StationId, String)> = naming
        .collisions
        .iter()
        .map(|l| (l.station, l.to.clone()))
        .collect();
    let mut answered = p.clone();
    answered.detach_line(x1, y, &renames, None).unwrap();
    let mut unanswered = p.clone();
    assert!(matches!(
        unanswered.detach_line(x1, y, &[], None),
        Err(OpError::NameTaken { .. })
    ));
    let mut auto = p.clone();
    let out = auto.detach_line_auto(x1, y, None).unwrap();
    assert!(!out.sweep.promoted.is_empty());
    assert_eq!(out.identities.lettered, out.sweep.lettered);
    let _ = x;
}

/// Farm f953c8f1b475 (seed 480000089, "combine keep"), decided on the
/// oracle's side: junction J of line T holds the record S of a split
/// junction merged away (S names L1 and L2). Combining K (from L1's Q) with
/// L2 joined by the closure from L2's end to Q tears L2 off J (the two join
/// stations are one piece through J). The tear's fresh node takes S (RS
/// 4.4), so L1 shows J natively there: an id is one node or one record
/// (A1), and the tear must separate L1 and L2 (both reach Q, they would
/// close the loop again), so one of them does without S. The property
/// excused only a tear of a line through J twice; it now excuses the
/// record a tear closure ties back to the junction. Injection: drop
/// `torn_off` from `check_records`.
#[test]
fn a_tear_of_a_several_line_record_is_not_a_stale_record() {
    use super::ops_test_helpers::{branch, free_line, ok, run};
    use super::test_helpers::at;
    use crate::{CombineJoinEnds, CombinePick, LineEnd};
    let mut p = project();
    let t = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let l1 = free_line(&mut p, [50.0, 50.0], run(0.0, 2, 10.0));
    let (j, s, q) = (at(&p, t, 2), at(&p, l1, 0), at(&p, l1, 1));
    let l2 = branch(&mut p, s, run(90.0, 3, 10.0));
    ok(p.merge_stations_auto(s, j, j));
    let k = branch(&mut p, q, run(0.0, 2, 10.0));
    let r3 = at(&p, l2, 3);
    for (i, (_, station)) in p.stations.iter_mut().enumerate() {
        station.name = format!("N{i}");
    }
    ok(p.create_closure(
        r3,
        Some(l2),
        q,
        Some(k),
        ClosureStrategy::Batch,
        Some(q),
        &[],
    ));
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    assert_eq!(p.record_naming(j, l1).map(|r| r.id), Some(s));
    assert_eq!(p.record_naming(j, l2).map(|r| r.id), Some(s));
    let records = record_slots(&p);
    let uuids: BTreeSet<String> = p
        .lines()
        .iter()
        .flat_map(|l| l.shots.iter().map(|s| s.uuid.clone()))
        .collect();
    let options = CombineLinesOptions {
        name: "C".to_string(),
        color: "#123456".to_string(),
        swap_lr: false,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Kept,
        join: Some(CombineJoinEnds {
            kept: LineEnd::Start,
            other: LineEnd::End,
        }),
    };
    let edited = p.combine_lines(k, l2, &options).unwrap();
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    assert!(
        edited.value.torn_ties.iter().any(|tie| tie.station == j),
        "L2 is torn off J: {:?}",
        edited.value.torn_ties
    );
    assert!(p.stations.contains_key(&s), "the fresh node takes S");
    assert!(p.is_native(j, l1), "L1 shows J natively");
    let skip: BTreeSet<StationId> = [q, r3].into_iter().collect();
    check_records(&records, &uuids, &p, &skip).unwrap();
}
