//! METAMORPHIC oracles: two routes to the same place, driven side by side.
//!
//! # What this adds that the roundtrip fuzzer cannot see
//!
//! `fuzz_undo_tests.rs` compares a state against ITSELF at an earlier moment.
//! That catches everything a history move fails to restore, and nothing about
//! whether two DIFFERENT ways of asking for the same edit agree. A defect that
//! is faithfully undone is invisible to it - and the delete/split family is
//! full of places where that could happen, because each entry point is its own
//! hand-written implementation of an operation another entry point also
//! implements.
//!
//! Two of those equivalences are promised in the code, in as many words:
//!
//! * `execute_delete_shot_range_shift` - an ATOMIC range gap-close that agrees
//!   with `n` single `Command::DeleteShot`s for every range that removes no
//!   station a merge is anchored to (`command_delete_split.rs`);
//! * `execute_delete_shot_range_split` - "Generalizes
//!   [`Self::execute_delete_shot_and_split`] from a single shot to
//!   `[start_pos..=end_pos]`" (`command_delete_split.rs:260-267`).
//!
//! A generalization that does not agree with the thing it generalizes on the
//! one-element case is a bug in one of the two, and nothing was checking.
//!
//! The first of those promises used to be broader - "generalizes the single-shot
//! `delete_shot` by deleting the range's shots in forward order" - and this
//! engine's first run caught it disagreeing with itself (issue #413 finding 22).
//! The claim was the defect: the atomic behaviour is the reviewed one (PR #95),
//! so the doc comment was narrowed to what the code does and the pair below
//! ASSERTS the remaining difference on the one shape that has it, rather than
//! classifying it away. See [`Documented`].
//!
//! # The twins
//!
//! Both routes need to start from the SAME state, and "the same" has to mean
//! byte-identical rather than "built the same way": `Shot::uuid` is a
//! `Uuid::new_v4()`, so two independently-built projects differ in every shot's
//! identity and no strict comparison is possible afterwards.
//!
//! So the base project is built ONCE and then loaded into both twins from one
//! `.stalx`. Both sides therefore go through the identical save -> load
//! pipeline, which also settles the save-time heals (`capture_missing_gauge_anchors`,
//! the loader's zero-trust offset re-solve, the orphan-closure conversion) before
//! either route starts, so none of them can be mistaken for a divergence. That
//! the twins really do start equal is ASSERTED rather than assumed
//! (`Twins::build`), which is the check that would catch the loader itself
//! becoming non-deterministic.
//!
//! # Four oracles again, plus one
//!
//! Every comparison runs the same four the roundtrip fuzzer uses - canonical
//! stored project, the app's own integrity verdict, the adjusted geometry, and
//! (implicitly, through the stored project) merge alignment - re-used as free
//! functions from `fuzz_undo_tests` rather than re-implemented, so the two
//! engines cannot drift apart in what they consider a difference.
//!
//! The fifth is the ID ALLOCATORS. The roundtrip oracle drops them deliberately
//! (a snapshot-pair undo restores them verbatim while a targeted undo only ever
//! raises them, so comparing them per depth would fire on every mixed history).
//! Between two twins at the same logical point there is no such asymmetry, and
//! two routes that leave different allocator values hand out different ids
//! afterwards - so here they are compared.
//!
//! # The inverse is a pair too
//!
//! Every pair is checked TWICE: once on the states the two routes produce, and
//! once on the states they unwind to. The second is the interesting one. Undo is
//! per-entry-point special cases all the way down - it is where finding 10's
//! class lives (two of four delete entry points do not convert orphan closures)
//! - and "both routes undo to the same place" is a property no single-route
//!   fuzzer can even express.
//!
//! The inverse check compares the two UNWOUND TWINS TO EACH OTHER, not to the
//! baseline. Comparing to the baseline re-measures the open findings on this
//! issue (3, 5, 1, 2 all leave an unwind short of where it started) and would
//! make this engine a second, worse report of them; comparing the twins to each
//! other asks the question this engine exists for and is silent on a defect that
//! affects both routes identically. The baseline distance is still MEASURED and
//! reported per run, so the fact is on the record without being asserted here.
//!
//! # Data
//!
//! Everything is generated in code. No fixture is read, copied or derived from.
//!
//! `struct_field_names` is expected rather than renamed: `explains_geometry`
//! reads as a sentence about the defect and `geometry` alone would not.

#![expect(
    clippy::indexing_slicing,
    reason = "test module: see docs/linting.md for the per-test-module expect idiom"
)]
#![expect(
    clippy::struct_field_names,
    reason = "`Explains::explains_geometry` reads as a sentence about the defect; `geometry` alone               would not"
)]
#![expect(
    clippy::print_stdout,
    reason = "a fuzzer's per-seed summary IS its output: the pair counts are what a reviewer reads               to see the oracle is still doing work, and `cargo test -- --nocapture` is the only               channel a test has"
)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use tempfile::TempDir;

use super::fuzz_undo_tests::{
    adjusted_diffs, adjusted_geometry_of, build_project, canonical_snapshot, describe_diffs,
    diff_snapshots, integrity_verdict, random_offset, random_shot, read_targets,
    topology_signature, Diff, Rng, Targets, Verdict, COLORS,
};
use super::AppState;
use crate::core::Command;
use crate::models::{Project, SectionId, ShotData, ShotId, ShotUpdates, StationRef};
// CONFIG ISOLATION. Every test in this module opens with
// `let _config = isolated_config();`: `AppState::new()` LOADS the global config,
// and every save reaches `note_recent_project` -> `Config::save()`, which WRITES
// it. Config resolution is per-thread (`crate::test_helpers` documents what that
// does and does not protect, and why it is not opt-in), so the guard is no longer
// what keeps these tests off other files; what it buys is ONE dir for the whole
// test, whose path the test can name and hand to a worker thread
// (`ConfigHandoff`). It is held for the whole test because the saves are.
use crate::test_helpers::isolated_config;

// ===========================================================================
// Comparison
// ===========================================================================

/// What a pair is allowed to differ in before the comparison even starts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mask {
    /// Nothing. Every uuid, every field, both allocators.
    Strict,
    /// Every uuid is replaced by a placeholder.
    ///
    /// Only for pairs where BOTH routes MINT shots or sections. A fresh
    /// `Uuid::new_v4()` cannot agree across two routes, so comparing them would be comparing the random number generator.
    /// Everything else - the field values, the ORDER, the count, whether a uuid
    /// is present at all - is still compared exactly, so "one route lost a uuid"
    /// and "one route has a shot the other does not" both still fail.
    MintedUuids,
}

/// Replace every RFC-3339 wall-clock timestamp with a placeholder.
///
/// Not a weakening, and not optional: the two routes run at two different
/// instants, so a field the app fills from `Utc::now()` cannot agree between
/// them and comparing it is comparing the clock. `StationMerge::merged_at`
/// (models/station.rs:359) is the one that bites - both delete/split routes
/// re-record the merges they carry across - and it bites INTERMITTENTLY,
/// whenever the two calls happen to straddle a tick, which is exactly the shape
/// that makes a gate flaky rather than red.
///
/// The mask is on the VALUE's shape, so a timestamp appearing anywhere new is
/// covered, and the presence or absence of the field is still compared: a route
/// that DROPPED a `merged_at` still fails.
fn mask_timestamps(snapshot: &str) -> String {
    // `2026-08-04T18:50:23.763293057Z` - digits, dashes, `T`, colons, a dot and
    // a trailing `Z`, at least 20 characters. Cheap and specific enough that no
    // survey value can match it.
    let is_timestamp = |text: &str| -> bool {
        text.len() >= 20
            && text.ends_with('Z')
            && text.as_bytes().get(4) == Some(&b'-')
            && text.as_bytes().get(10) == Some(&b'T')
            && text
                .chars()
                .all(|ch| ch.is_ascii_digit() || matches!(ch, '-' | 'T' | ':' | '.' | 'Z'))
    };
    replace_strings(snapshot, &is_timestamp, "<timestamp>")
}

/// Replace every UUID-shaped string with a placeholder.
///
/// Shape-matched rather than field-matched on purpose: a uuid appears as a shot
/// field, as a section's gauge anchor, and INSIDE a `StationKey::AfterShot`
/// anchor that keys merges, closures, datums and station comments. A rule that
/// listed field names would leave the anchors comparing random numbers.
fn mask_uuids(snapshot: &str) -> String {
    let is_uuid = |text: &str| -> bool {
        text.len() == 36
            && text.chars().enumerate().all(|(index, ch)| match index {
                8 | 13 | 18 | 23 => ch == '-',
                _ => ch.is_ascii_hexdigit(),
            })
    };
    replace_strings(snapshot, &is_uuid, "<uuid>")
}

/// Rewrite every string in a JSON document that `matches`, in place.
fn replace_strings(snapshot: &str, matches: &impl Fn(&str) -> bool, replacement: &str) -> String {
    fn walk(value: &mut serde_json::Value, matches: &impl Fn(&str) -> bool, replacement: &str) {
        match value {
            serde_json::Value::String(text) => {
                if matches(text) {
                    *text = replacement.to_string();
                }
            }
            serde_json::Value::Array(items) => {
                for item in items.iter_mut() {
                    walk(item, matches, replacement);
                }
            }
            serde_json::Value::Object(map) => {
                for child in map.values_mut() {
                    walk(child, matches, replacement);
                }
            }
            _ => {}
        }
    }
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(snapshot) else {
        return snapshot.to_string();
    };
    walk(&mut value, matches, replacement);
    value.to_string()
}

/// Every id allocator in the open project, by name. See the module header for
/// why these are compared here and dropped by the roundtrip oracle.
fn allocators(state: &AppState) -> BTreeMap<String, u32> {
    let mut out = BTreeMap::new();
    let Some(project) = state.get_project_state() else {
        return out;
    };
    out.insert("next_section_id".to_string(), project.next_section_id);
    out.insert(
        "next_closure_id".to_string(),
        u32::try_from(project.next_closure_id).unwrap_or(u32::MAX),
    );
    for section in &project.sections {
        out.insert(
            format!("section {}.next_shot_id", section.id),
            section.next_shot_id,
        );
    }
    out
}

/// A snapshot of everything the twins are compared on, taken from one side.
struct Observed {
    stored: String,
    verdict: Verdict,
    adjusted: String,
    allocators: BTreeMap<String, u32>,
}

/// Re-sort the two SET-LIKE maps after a mask has rewritten their keys.
///
/// `canonical_snapshot` already sorts `station_offsets` and `station_comments`,
/// because both serialize as a `Vec` of pairs out of a `HashMap` and their order
/// is not stable. It sorts by the pair's whole JSON text - which, for a
/// `StationKey::AfterShot` anchor, INCLUDES the shot uuid. So the sort key of a
/// comment on a freshly minted shot is a random number.
///
/// That is fine until [`mask_uuids`] replaces those uuids with a placeholder:
/// the mask makes the two routes' keys compare equal but cannot undo the order
/// the random uuids already imposed, so two projects holding exactly the same
/// comments can come out of the sort in different orders. Re-sorting AFTER the
/// mask closes it, and it is a no-op for `Mask::Strict` (where no key was
/// rewritten and the first sort is already canonical).
///
/// This is what `observation-split-routes-disagree-on-station-comment-keys` was
/// - see the triage record in `EXPLAINS` below.
fn resort_set_like(snapshot: &str) -> String {
    // Same shape as `replace_strings` above: a snapshot that will not parse is
    // returned unchanged rather than panicked on, so a comparison still happens
    // and the differ - not this helper - is what reports it.
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(snapshot) else {
        return snapshot.to_string();
    };
    if let serde_json::Value::Object(map) = &mut value {
        for key in ["station_offsets", "station_comments"] {
            if let Some(serde_json::Value::Array(items)) = map.get_mut(key) {
                items.sort_by_key(ToString::to_string);
            }
        }
    }
    value.to_string()
}

fn observe(state: &AppState, mask: Mask) -> Observed {
    // The timestamp mask is UNCONDITIONAL, for both mask modes: no pair can
    // control which instant its two routes run at.
    let stored = mask_timestamps(&canonical_snapshot(state, false));
    Observed {
        stored: match mask {
            Mask::Strict => stored,
            Mask::MintedUuids => resort_set_like(&mask_uuids(&stored)),
        },
        verdict: integrity_verdict(state),
        adjusted: adjusted_geometry_of(state),
        allocators: allocators(state),
    }
}

/// What a comparison found: the differences nothing explains, and the ones a
/// KNOWN defect does.
///
/// Same contract as the roundtrip fuzzer's classifiers, and for the same reason:
/// a hit is still a difference and is still counted and printed - classification
/// only stops one open defect from making a new oracle unusable. **Delete an
/// entry when its defect is fixed**, and the gate then fails on the next hit.
#[derive(Default)]
struct Comparison {
    problems: Vec<String>,
    hits: BTreeMap<&'static str, usize>,
    /// SHAPES the run reached, by name. Not defects and not differences: a pair
    /// whose assertions depend on the generator having produced a particular
    /// topology counts it here, so a run in which that arm was never exercised
    /// reads as such instead of as a pass. Printed per seed and totalled by the
    /// sweep.
    shapes: BTreeMap<&'static str, usize>,
}

impl Comparison {
    fn absorb(&mut self, other: Self) {
        self.problems.extend(other.problems);
        for (id, count) in other.hits {
            *self.hits.entry(id).or_insert(0) += count;
        }
        for (id, count) in other.shapes {
            *self.shapes.entry(id).or_insert(0) += count;
        }
    }

    /// Prefix every problem, for the unwind half of a pair.
    fn prefixed(mut self, prefix: &str) -> Self {
        self.problems = self
            .problems
            .into_iter()
            .map(|line| format!("{prefix}{line}"))
            .collect();
        self
    }
}

/// The one place two routes are DOCUMENTED to disagree, and the frame that keeps
/// that allowance honest.
///
/// This is not a classifier and is not a known defect: `EXPLAINS` carries open
/// bugs and is deleted entry by entry as they are fixed, while this carries a
/// difference the code is SUPPOSED to have. Issue #413 finding 22 was reported
/// as the range delete not being the `n` single deletes its doc comment claimed
/// to generalize; the claim was the defect. `execute_delete_shot_range_shift` is
/// atomic on purpose - it defers the geometry cascade to ONE reconcile at the
/// end - so on a range whose removed stations include one a merged partner
/// section is anchored to, it leaves that partner where the surveyor put it,
/// where the single-delete sequence drags the partner along while the merge is
/// still live and then strands it displaced. Both PR #95 tests
/// (`test_delete_range_shift_does_not_displace_free_floating_merged_section`,
/// `test_delete_range_suffix_shift_keeps_head_with_doomed_tail_anchor`) pin the
/// atomic behaviour, `a_doomed_anchor_range_shift_keeps_the_partner_in_place`
/// (open_undo_bugs_tests.rs) pins it on a hand-built shape, and
/// `command_delete_split.rs` now states it.
///
/// So the allowance is narrow and directional. Only the PARTNER sections' own
/// placement may differ; every other path, every other station, both verdicts
/// and both allocators are still compared exactly. And when the two routes do
/// disagree about a partner, the ATOMIC route has to be the one that left it
/// alone - a divergence the other way round is the atomic path breaking its own
/// contract and fails.
struct Documented {
    /// Sections whose placement the two routes may disagree about: the far side
    /// of every merge the delete removes, plus everything merged rigidly to
    /// those without passing through the edited section (a displaced partner
    /// drags its own merge group with it).
    partners: std::collections::BTreeSet<SectionId>,
    /// The adjusted station geometry BEFORE either route ran, keyed
    /// `section:index`. The twins are asserted equal at that point, so one map
    /// is both routes' starting position - which is what makes "the atomic route
    /// is the one that did not move it" a decidable question.
    before: BTreeMap<String, [f64; 3]>,
}

/// The adjusted geometry as a map, from the `section:index|x|y|z` rows
/// [`adjusted_geometry_of`] emits.
fn adjusted_positions(text: &str) -> BTreeMap<String, [f64; 3]> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split('|');
            let key = parts.next()?.to_string();
            let x = parts.next()?.parse().ok()?;
            let y = parts.next()?.parse().ok()?;
            let z = parts.next()?.parse().ok()?;
            Some((key, [x, y, z]))
        })
        .collect()
}

/// The section a `section:index` adjusted-geometry key belongs to.
fn section_of_station_key(key: &str) -> Option<SectionId> {
    key.split(':').next()?.parse().ok()
}

/// The same bound `adjusted_diffs` uses, so "the routes differ" and "the atomic
/// route moved it" are decided on one ruler.
const POSITION_TOLERANCE_M: f64 = 1e-6;

/// Whether two solved positions differ IN THE HORIZONTAL PLANE.
///
/// The plane, not the point, and not a convenience: the compensation a deferred
/// cascade preserves is `section_offsets`, which is `(f64, f64)` - there is no
/// stored z offset for a section. When a delete cuts a partner's merge, the
/// partner's z falls back to its own raw layout on BOTH routes (measured: same
/// z to the last bit), so comparing z here would report a drop neither route
/// chose and that neither could have avoided. PR #95's
/// `test_delete_range_shift_does_not_displace_free_floating_merged_section`
/// asserts the same plane, through `assert_xy_close`. A z difference BETWEEN the
/// two routes is still compared - by the adjusted-geometry oracle, which the
/// allowance only lets through for a partner section.
fn positions_differ_in_plane(left: [f64; 3], right: [f64; 3]) -> bool {
    (0..2)
        .map(|axis| (left[axis] - right[axis]).abs())
        .fold(0.0_f64, f64::max)
        > POSITION_TOLERANCE_M
}

impl Documented {
    /// Whether a stored-project difference is one this allowance covers.
    ///
    /// `section_offsets` is a map keyed by section id, so the diff path NAMES
    /// the section it is about - `section_offsets.7` when a whole entry is
    /// present on one side only, `section_offsets.7[0]` when one component of
    /// the pair differs - and the allowance can be per-section rather than
    /// per-collection. A `section_offsets` entry for the EDITED section, or for
    /// any section the delete did not cut a merge to, is not this and still
    /// fails.
    fn allows_stored(&self, diff: &Diff) -> bool {
        diff.path
            .strip_prefix("section_offsets.")
            .map(|rest| {
                rest.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
            })
            .and_then(|id| id.parse::<SectionId>().ok())
            .is_some_and(|id| self.partners.contains(&id))
    }

    /// Whether an adjusted-geometry difference line is one this allowance
    /// covers. `adjusted_diffs` writes every line as `station <section>:<index>
    /// ...`; a line that does not parse is NOT allowed, so the check fails
    /// closed if that format ever changes.
    fn allows_geometry(&self, line: &str) -> bool {
        line.strip_prefix("station ")
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(section_of_station_key)
            .is_some_and(|id| self.partners.contains(&id))
    }
}

/// A defect that can explain EVERY stored-project difference between two routes.
struct Explains {
    id: &'static str,
    each: fn(&Diff) -> bool,
    /// Whether this defect also accounts for the ADJUSTED-geometry difference in
    /// the same comparison.
    ///
    /// True only where the geometry difference is DOWNSTREAM of the stored one -
    /// a `section_offsets` entry that one route has and the other does not moves
    /// every station of that section in the solve, and reporting it twice would
    /// say there are two defects. False is the default and the safe answer: a
    /// solve that moved while the stored projects agree is exactly what the
    /// adjusted oracle exists to see.
    explains_geometry: bool,
    /// The PAIR this defect is about, when it is about one.
    ///
    /// This is what narrows a classifier that a path rule alone cannot: finding
    /// 22 is a claim about two specific entry points disagreeing, so the same
    /// `section_offsets` difference arising between any OTHER two routes is not
    /// it and still fails. `None` means the defect is not route-specific.
    only_for: Option<&'static str>,
}

const EXPLAINS: &[Explains] = &[
    // FINDING 2 (issue #413) was explained here, reached from a direction its
    // own write-up does not mention: the mechanism is a `section_offsets` key
    // that is CREATED by an adjustment and never removed, so every route that
    // adjusted an offset and then adjusted it back - a `MoveSection(+d)` and
    // its `-d`, a `ReverseSection` applied twice - left an inert key its
    // do-nothing twin did not have.
    //
    // FIXED, in two halves, and the entry is gone. A `section_offsets` key
    // present on one route and absent on the other is unexplained from here on,
    // whatever its value.
    //
    // * The EXACT zero. `Project::write_section_offset`
    //   (models/project/offsets.rs) is THE write point for the map and drops
    //   the key when the value is exactly (0, 0). `MoveSection(+d)` and its
    //   `-d` DO land there (0 + d - d == 0 in IEEE for the same d), which is
    //   why `move-out-and-back-is-identity` went clean first.
    // * The float DUST. `ReverseSection` applied twice does NOT cancel: the
    //   compensation is `shot_run_displacement` summed over the shots, and the
    //   second reverse sums the negated vectors in the OPPOSITE order, so the
    //   round trip lands on a residue under `Project::OFFSET_DUST_M` rather
    //   than on zero.
    //   `Project::write_section_offset` normalizes an offset within
    //   `Project::OFFSET_DUST_M` of the origin to absent, and `cascade_bfs`
    //   (models/project/connectivity.rs) declines to "correct" a sub-bound gap
    //   at all, matching the merge-BFS gates in `reconcile_after_edit`.
    //
    // The rule binds EVERY writer, a restore included: a dust-zero
    // `section_offsets` entry is UNREPRESENTABLE (the invariant on
    // `Project::write_section_offset`), which is what closed the second half of
    // this pair - `move-out-and-back-is-identity` failed again the moment a
    // project carrying a legacy all-zero row reached it, because `+d` then `-d`
    // landed the key absent while the do-nothing route still held it.
    // `plant_legacy_zero_offset_rows` below feeds exactly that project to every
    // pair, so the property is now proved on the shape that broke it.
    // `is_inert_offset` went with the entry.
    //
    // FINDING 22 (issue #413) HAD an entry here - "a range shift-delete is not
    // its single deletes" - and it is gone rather than fixed-and-deleted: the
    // divergence is INTENDED (see `Documented` and the `range-shift-is-n-single-
    // deletes` pair, which now asserts it instead of tolerating it; the
    // hand-built version of the same claim is
    // `a_doomed_anchor_range_shift_keeps_the_partner_in_place` in
    // open_undo_bugs_tests.rs). Nothing classifies a range-vs-singles difference
    // any more; one outside the documented doomed-anchor shape is unexplained
    // and fails.
    //
    // `observation-split-routes-disagree-on-station-comment-keys` USED TO BE
    // HERE, and it is gone because it was TRIAGED OUT: it was this engine's own
    // measurement, not a defect in either split route. Kept as a record for the
    // same reason finding 9's is.
    //
    // What it looked like: the two routes disagreed about which SECTION a
    // station comment was keyed to after the split
    // (`station_comments[i][0].section_id` 3 vs 9, with the rest of the list
    // shifted), intermittently, on identical inputs - which read as an
    // iteration-order-dependent re-key.
    //
    // What it was: a PERMUTATION, not a re-key. Every reported instance held the
    // same multiset of `(section_id, comment)` pairs on both sides, with exactly
    // one entry - always the newly minted section B - in a different slot.
    // `canonical_snapshot` sorts `station_comments` by the pair's whole JSON
    // text, which for a `StationKey::AfterShot` anchor includes the shot uuid;
    // both routes MINT section B's shots, so that one sort key was a fresh
    // random uuid on each side, and `Mask::MintedUuids` erases the uuids only
    // AFTER the sort has used them. See `resort_set_like` above, which sorts
    // again once the mask has run.
    //
    // Measured, not argued: 40 seeds x 6 repeats reported 7 divergences over 240
    // built instances with the sort as it was, and 0 over the same 240 with the
    // re-sort, every other pair's counts byte-identical.
    //
    // The earlier lead - the order-dependent `removed_comments` vector in
    // `preview_delete_shot_range` (models/project/shots.rs) - was already refuted as
    // the cause here (it feeds the delete dialog's impact summary, which no oracle
    // observes) and is not resurrected by this. It has since been sorted anyway
    // (#469), so the vector it names is no longer order-dependent at all.
];

/// Every way two observed states differ, as readable lines. Empty means the two
/// routes agree on all five oracles.
///
/// `documented` is the one narrow, per-section allowance a pair may carry for a
/// difference the code is SUPPOSED to have - see [`Documented`]. `None` (every
/// pair but one shape of one pair) means every difference is a problem.
fn differences(
    pair: &'static str,
    left: &Observed,
    right: &Observed,
    documented: Option<&Documented>,
) -> Comparison {
    let mut out = Comparison::default();
    let diffs = diff_snapshots(&left.stored, &right.stored);
    let mut geometry_explained = false;
    if !diffs.is_empty() {
        if let Some(explains) = EXPLAINS.iter().find(|explains| {
            explains.only_for.is_none_or(|only| only == pair)
                && diffs.iter().all(|diff| (explains.each)(diff))
        }) {
            *out.hits.entry(explains.id).or_insert(0) += 1;
            geometry_explained = explains.explains_geometry;
        } else {
            // Per-path rather than all-or-nothing, unlike the classifiers
            // above: the documented divergence names the sections it may
            // touch, so a run that shows it AND something else must still
            // report the something else.
            let unexpected: Vec<Diff> = diffs
                .iter()
                .filter(|diff| !documented.is_some_and(|doc| doc.allows_stored(diff)))
                .cloned()
                .collect();
            if !unexpected.is_empty() {
                out.problems.push(format!(
                    "the stored projects differ in {} path(s):\n{}",
                    unexpected.len(),
                    describe_diffs(&unexpected)
                ));
            }
        }
    }
    if left.verdict != right.verdict {
        out.problems.push(format!(
            "the app's own integrity verdicts differ:\n    left:  {}\n    right: {}",
            left.verdict.describe(),
            right.verdict.describe()
        ));
    }
    let geometry: Vec<String> = adjusted_diffs(&left.adjusted, &right.adjusted)
        .into_iter()
        .filter(|line| !documented.is_some_and(|doc| doc.allows_geometry(line)))
        .collect();
    if !geometry.is_empty() && !geometry_explained {
        out.problems.push(format!(
            "the ADJUSTED geometry differs:\n    {}",
            geometry.join("\n    ")
        ));
    }
    if left.allocators != right.allocators {
        let mut moved = Vec::new();
        for (key, value) in &left.allocators {
            match right.allocators.get(key) {
                Some(other) if other == value => {}
                Some(other) => moved.push(format!("{key}: left {value} vs right {other}")),
                None => moved.push(format!("{key}: only on the left ({value})")),
            }
        }
        for key in right.allocators.keys() {
            if !left.allocators.contains_key(key) {
                moved.push(format!("{key}: only on the right"));
            }
        }
        out.problems.push(format!(
            "the id ALLOCATORS differ, so the two routes hand out different ids from here on:\n    {}",
            moved.join("\n    ")
        ));
    }
    out
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
    /// The state both sides started from, for the measured (not asserted)
    /// baseline distance after an unwind.
    baseline: String,
    /// The topology signature of the state the pair was built on. Reported by
    /// the sweep so the shapes this engine reaches are a number rather than an
    /// assumption - see `fuzz_corpus_sweep` for why that number matters and why
    /// corpus guidance was measured NOT to improve it.
    signature: String,
}

/// Which twin a route drives.
#[derive(Clone, Copy)]
enum Side {
    Left,
    Right,
}

/// Give every section that has no `section_offsets` key one whose value is
/// exactly `(0, 0)`, on the project about to be SAVED.
///
/// This is what a build predating the invariant on
/// `Project::write_section_offset` left in a `.stalx`, and it is planted directly
/// on the map because there is no longer any API that can produce it - every
/// writer drops it. It is planted here, into the file both twins load from, so
/// every pair in this engine runs against the exact input that used to break one
/// of them: `move-out-and-back-is-identity` failed on a project carrying such a
/// row, because `MoveSection(+d)` then `MoveSection(-d)` computes the offset back
/// to the origin and drops the key that the do-nothing route still held.
///
/// It changes nothing about what the twins see, which is the point: the load
/// drops the rows (asserted at the call site), so both sides start from the same
/// project they always did, every seed's trajectory is unchanged, and the pair
/// holds BY CONSTRUCTION rather than by luck of never meeting the input.
///
/// FILL, never overwrite: a section that carries a real offset needs it, and
/// zeroing it would displace the section from its merge partner and make the next
/// reconcile look like an app defect.
fn plant_legacy_zero_offset_rows(state: &AppState) {
    let mut guard = match state.project.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(project) = guard.as_mut() else {
        return;
    };
    let ids: Vec<SectionId> = project.sections.iter().map(|section| section.id).collect();
    for id in ids {
        project.section_offsets.entry(id).or_insert((0.0, 0.0));
    }
}

impl Twins {
    /// Build a project from `seed`, run `prelude` random commands over it, then
    /// load it into both twins from one file.
    ///
    /// `None` when the base project could not be built or saved, which is a
    /// skip rather than a failure: a prelude can leave a project the save path
    /// legitimately refuses (a dangling reference blocks save by design).
    fn build(seed: u32, closures: bool, prelude: usize) -> Option<Self> {
        let source = AppState::new().ok()?;
        let mut rng = Rng::new(seed);
        build_project(&source, &mut rng, closures);
        for _ in 0..prelude {
            prelude_command(&source, &mut rng);
        }
        let temp = TempDir::new().ok()?;
        let path: PathBuf = temp.path().join("twins.stalx");
        plant_legacy_zero_offset_rows(&source);
        if !source.save_project(Some(path.clone())).success {
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
            // The load restores the SAVED undo stacks. Both routes have to start
            // from an empty history or "unwind everything" would unwind the
            // prelude too, and the baseline would not be the state they started
            // from.
            if let Ok(mut stack) = side.undo_stack.lock() {
                stack.clear();
            }
            // The zero rows planted above must not have come back. This is what
            // makes feeding them worth anything: if the boundary ever stops
            // dropping them, every pair below is being run on a project the app
            // is not allowed to hold, and `move-out-and-back-is-identity` is
            // measuring the wrong thing.
            let project = side.get_project_state()?;
            let survivors: Vec<(SectionId, (f64, f64))> = project
                .section_offsets
                .iter()
                .filter(|(_, offset)| {
                    offset.0.abs() <= Project::OFFSET_DUST_M
                        && offset.1.abs() <= Project::OFFSET_DUST_M
                })
                .map(|(id, offset)| (*id, *offset))
                .collect();
            assert!(
                survivors.is_empty(),
                "seed {seed}: a planted all-zero section_offsets row survived the load: \
                 {survivors:?}. A dust-zero entry is unrepresentable (see the invariant on \
                 Project::write_section_offset), so the load boundary has to drop it"
            );
        }

        let baseline = mask_timestamps(&canonical_snapshot(&left, false));
        // The twins REALLY starting equal is the premise of everything below, so
        // it is measured. A failure here is not a metamorphic finding - it is the
        // loader not being a function of the file.
        let mirror = mask_timestamps(&canonical_snapshot(&right, false));
        assert_eq!(
            baseline, mirror,
            "seed {seed}: two loads of ONE .stalx produced different projects, so no \
             path-equivalence below this line means anything"
        );
        let signature = topology_signature(&left);
        Some(Self {
            left,
            right,
            _temp: temp,
            baseline,
            signature,
        })
    }

    const fn state(&self, side: Side) -> &AppState {
        match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
        }
    }

    /// How the two twins differ right now.
    fn compare(&self, pair: &'static str, mask: Mask) -> Comparison {
        self.compare_allowing(pair, mask, None)
    }

    /// How the two twins differ right now, minus the one difference this shape
    /// is DOCUMENTED to have. See [`Documented`] - `None` for every pair that
    /// promises plain equality.
    fn compare_allowing(
        &self,
        pair: &'static str,
        mask: Mask,
        documented: Option<&Documented>,
    ) -> Comparison {
        differences(
            pair,
            &observe(&self.left, mask),
            &observe(&self.right, mask),
            documented,
        )
    }

    /// Undo every entry on both sides, then compare the two UNWOUND states to
    /// each other. See the module header for why the comparison is twin-to-twin
    /// rather than twin-to-baseline.
    ///
    /// Returns `(differences, baseline_distance)`, where the second is how many
    /// paths each side still differs from its own starting state in - measured
    /// and reported, never asserted.
    fn unwind_and_compare(&self, pair: &'static str, mask: Mask) -> (Comparison, (usize, usize)) {
        self.unwind_and_compare_allowing(pair, mask, None)
    }

    /// [`Self::unwind_and_compare`], carrying the same documented allowance the
    /// forward half used: a difference the two routes are supposed to have is
    /// still supposed to be there after both of them are undone, and everything
    /// else is still a problem.
    fn unwind_and_compare_allowing(
        &self,
        pair: &'static str,
        mask: Mask,
        documented: Option<&Documented>,
    ) -> (Comparison, (usize, usize)) {
        let mut refused = Vec::new();
        for (name, side) in [("left", Side::Left), ("right", Side::Right)] {
            let state = self.state(side);
            let mut guard = 0;
            while !state.get_undo_stack().is_empty() {
                guard += 1;
                if guard > 512 {
                    refused.push(format!("{name} never emptied its undo stack"));
                    break;
                }
                let result = state.undo();
                if !result.success {
                    // A wedged history is finding 6 or 13, both open and both
                    // already pinned; it is not a metamorphic result and the
                    // unwind comparison is meaningless from here.
                    refused.push(format!(
                        "{name} could not unwind: {}",
                        result.error.unwrap_or_default()
                    ));
                    break;
                }
            }
        }
        if !refused.is_empty() {
            return (Comparison::default(), (0, 0));
        }
        let distance = |state: &AppState| -> usize {
            diff_snapshots(
                &self.baseline,
                &mask_timestamps(&canonical_snapshot(state, false)),
            )
            .len()
        };
        (
            self.compare_allowing(pair, mask, documented),
            (distance(&self.left), distance(&self.right)),
        )
    }
}

// ===========================================================================
// The prelude
// ===========================================================================

/// One random command against the base project, before the twins are split.
///
/// Deliberately a NARROWER generator than the roundtrip fuzzer's: the prelude
/// exists to give the pairs varied topology to work on, not to hunt bugs, and a
/// prelude that reached a known open defect would poison every pair built on it
/// with a difference neither route caused. So no deletes, no splits, no
/// closures, and nothing that has an open finding against it.
fn prelude_command(state: &AppState, rng: &mut Rng) {
    let targets = read_targets(state);
    if targets.sections.is_empty() {
        return;
    }
    match rng.below(6) {
        0 => {
            let count = 1 + rng.below(3);
            let shots: Vec<ShotData> = (0..count).map(|_| random_shot(rng)).collect();
            state.execute(Command::CreateSection {
                name: Some(format!("pre-{}", rng.range(1000, 9999))),
                surveyor: None,
                date: None,
                color: (*rng.pick(COLORS)).to_string(),
                direction: None,
                shots,
            });
        }
        1 => {
            let (section_id, shots) = targets.sections[rng.below(targets.sections.len())].clone();
            let after =
                (!shots.is_empty() && rng.chance(50)).then(|| shots[rng.below(shots.len())]);
            state.execute(Command::CreateShot {
                section_id,
                shot: random_shot(rng),
                after_shot_id: after,
                before_shot_id: None,
            });
        }
        2 => {
            let Some((section_id, shot_id)) = pick_shot(&targets, rng) else {
                return;
            };
            state.execute(Command::UpdateShot {
                section_id,
                shot_id,
                updates: ShotUpdates {
                    length: Some(rng.float(1.0, 15.0)),
                    ..ShotUpdates::default()
                },
            });
        }
        3 => {
            let section_id = targets.sections[rng.below(targets.sections.len())].0;
            state.execute(Command::MoveSection {
                section_id,
                offset: random_offset(rng),
            });
        }
        4 => {
            if targets.stations.is_empty() {
                return;
            }
            let (section_id, index) = targets.stations[rng.below(targets.stations.len())];
            state.execute(Command::SetStationComment {
                station_ref: StationRef::new(section_id, index),
                comment: Some(format!("pre-note-{}", rng.range(0, 9999))),
            });
        }
        _ => {
            if targets.sections.len() < 2 || targets.stations.len() < 2 {
                return;
            }
            let a = targets.stations[rng.below(targets.stations.len())];
            let candidates: Vec<(SectionId, u32)> = targets
                .stations
                .iter()
                .copied()
                .filter(|station| station.0 != a.0)
                .collect();
            if candidates.is_empty() {
                return;
            }
            let b = candidates[rng.below(candidates.len())];
            state.execute(Command::MergeStations {
                station_a: StationRef::new(a.0, a.1),
                station_b: StationRef::new(b.0, b.1),
            });
        }
    }
}

/// A shot from a section that has one.
fn pick_shot(targets: &Targets, rng: &mut Rng) -> Option<(SectionId, ShotId)> {
    let populated: Vec<&(SectionId, Vec<ShotId>)> = targets
        .sections
        .iter()
        .filter(|(_, shots)| !shots.is_empty())
        .collect();
    if populated.is_empty() {
        return None;
    }
    let (section_id, shots) = populated[rng.below(populated.len())];
    Some((*section_id, shots[rng.below(shots.len())]))
}

/// Live shot ids of one section, in positional order, on one twin.
fn shot_ids(state: &AppState, section_id: SectionId) -> Vec<ShotId> {
    state
        .get_project_state()
        .map(|project| {
            project
                .sections
                .iter()
                .find(|section| section.id == section_id)
                .map(|section| section.shots.iter().map(|shot| shot.id).collect())
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// A section with at least `count` shots, picked deterministically.
fn section_with_shots(state: &AppState, rng: &mut Rng, count: usize) -> Option<SectionId> {
    let targets = read_targets(state);
    let candidates: Vec<SectionId> = targets
        .sections
        .iter()
        .filter(|(_, shots)| shots.len() >= count)
        .map(|(id, _)| *id)
        .collect();
    if candidates.is_empty() {
        return None;
    }
    Some(candidates[rng.below(candidates.len())])
}

// ===========================================================================
// The pairs
// ===========================================================================

/// One equivalence, and what a difference in it MEANS.
struct Pair {
    id: &'static str,
    /// Where the promise comes from. A pair with no promise anywhere is not in
    /// this table: an oracle that asserts something nobody claimed is a
    /// generator of false findings.
    promise: &'static str,
    /// `None` when the shape is not buildable in this state - a SKIP, counted
    /// separately from a pass so a run that built nothing cannot read as green.
    run: fn(&Twins, &mut Rng, &'static str) -> Option<Comparison>,
}

/// What one pair did.
struct PairOutcome {
    built: usize,
    skipped: usize,
    problems: Vec<String>,
    /// Differences a KNOWN open defect explains, by id. Counted and printed,
    /// never asserted - see `EXPLAINS`.
    hits: BTreeMap<&'static str, usize>,
    /// Generated shapes a pair's assertions depend on, by name - see
    /// `Comparison::shapes`. The gate asserts on one of these, because an arm
    /// the generator stopped reaching passes without asserting anything.
    shapes: BTreeMap<&'static str, usize>,
    /// Topology signatures the pair was built on, for coverage reporting.
    signatures: std::collections::BTreeSet<String>,
    /// Paths each unwound twin still differs from its own baseline in, summed.
    /// Measured, never asserted - see the module header.
    baseline_distance: usize,
}

const PAIRS: &[Pair] = &[
    // ---------------------------------------------------------------------
    Pair {
        id: "combine-sections-then-undo-is-identity",
        promise: "command_combine_sections.rs execute_combine_sections - Combine Sections is ONE \
                  undo entry (including the reverse of the other section when the two meet end \
                  to end): undoing it restores every section, shot, reference and allocator",
        run: pair_combine_sections_then_undo_is_identity,
    },
    // ---------------------------------------------------------------------
    Pair {
        id: "split-then-combine-at-the-join-is-identity",
        promise: "combine_sections.rs module doc - Combine Sections is the inverse of Split \
                  Section Here: combining a fresh split at its join, with the original name and \
                  colour, gives back the section, every shot's survey credit included (shot ids \
                  and the id allocators aside)",
        run: pair_split_then_combine_at_the_join_is_identity,
    },
    // ---------------------------------------------------------------------
    Pair {
        id: "combine-end-to-end-is-reverse-then-combine",
        promise: "combine_sections.rs module doc - when two sections meet end to end the other \
                  section is reversed first with reverse_section, so combining them equals \
                  reversing it yourself and combining end to start",
        run: pair_combine_end_to_end_is_reverse_then_combine,
    },
    // ---------------------------------------------------------------------
    Pair {
        id: "split-section-then-undo-is-identity",
        promise: "command_split_section.rs execute_split_section - Split Section Here is ONE \
                  undo entry: undoing it restores every section, shot, merge, closure, datum, \
                  comment and per-shot survey credit exactly (#335)",
        run: pair_split_section_then_undo_is_identity,
    },
    // ---------------------------------------------------------------------
    Pair {
        id: "range-shift-is-n-single-deletes",
        promise: "command_delete_split.rs execute_delete_shot_range_shift - an ATOMIC range \
                  gap-close: identical to the n single deletes on every range that removes no \
                  merge, and on a range that removes one it keeps the partner section in place \
                  where the singles drag it (issue #413 finding 22; PR #95 tests)",
        run: pair_range_shift_vs_singles,
    },
    // ---------------------------------------------------------------------
    Pair {
        id: "one-shot-range-split-is-delete-and-split",
        promise:
            "command_delete_split.rs:260-267 - \"Generalizes \
                  Self::execute_delete_shot_and_split from a single shot to [start_pos..=end_pos]\"",
        run: pair_range_split_of_one_vs_delete_and_split,
    },
    // ---------------------------------------------------------------------
    Pair {
        id: "batch-is-its-commands-in-order",
        promise: "commands.rs execute_batch - a batch is all-or-nothing over the same commands, \
                  committed as ONE undo entry",
        run: pair_batch_vs_sequential,
    },
    // ---------------------------------------------------------------------
    Pair {
        id: "a-failed-batch-is-a-no-op",
        promise: "commands.rs execute_batch - on any sub-command failure the batch rolls back to \
                  its before-snapshot and pushes nothing, where the same commands run singly \
                  apply up to the failure",
        run: pair_poisoned_batch_is_a_no_op,
    },
    // ---------------------------------------------------------------------
    Pair {
        id: "reverse-twice-is-identity",
        promise: "ReverseSection reverses the shot order and the per-shot in/out fields; applying \
                  it twice returns each to itself",
        run: pair_reverse_twice_is_identity,
    },
    // ---------------------------------------------------------------------
    Pair {
        id: "move-out-and-back-is-identity",
        promise:
            "MoveSection translates a rigid group by an offset; the negated offset translates \
                  it back",
        run: pair_move_out_and_back_is_identity,
    },
    // ---------------------------------------------------------------------
    Pair {
        id: "create-section-with-shots-is-create-then-append",
        promise: "CreateSection { shots } and CreateSection { shots: [] } + N CreateShot appends \
                  both end in Section::add_shot with the same ShotData",
        run: pair_create_with_shots_vs_appends,
    },
];

/// PAIR 1. `execute_delete_shot_range_shift(a..=b)` against the same shots
/// deleted one at a time through the user-facing `Command::DeleteShot`.
///
/// The range path deletes forward from the start position, so the equivalent
/// single-step route deletes the shot AT the start position, `n` times - the id
/// is re-read each round because the ids do not shift but the positions do.
///
/// # Two arms, because the two routes are equal on all shapes but one
///
/// The range path is ATOMIC: one project lock, one undo unit, and the geometry
/// cascade deferred to a single reconcile at the end. On a range that removes no
/// merge that alone changes nothing, and the two routes are compared for exact
/// equality on all five oracles - measured over prefix and interior ranges, they
/// agree to the last bit.
///
/// On a range whose removed stations include one a merged partner section is
/// anchored to, they are supposed to differ, and this asserts the difference
/// rather than tolerating it (issue #413 finding 22, resolved as intended
/// behaviour - the doc comment that claimed a composition was the defect). The
/// singles route re-anchors the partner from `execute`'s per-command epilogue
/// while the merge is still live, drags it as the section shrinks and then
/// deletes the merge, stranding it displaced; the atomic route leaves it where
/// the surveyor put it. So on that shape three things are asserted: every
/// difference is confined to the PARTNER sections' placement, the atomic route
/// is the one that left the partner alone, and a partner the delete cut loose
/// entirely is exactly where it was. See [`Documented`] and the PR #95 tests it
/// names.
///
/// A single delete that the guards refuse is a SKIP, not a failure: `delete_shot`
/// has three guards the range path does not consult (the loop/datum pair and the
/// frozen check, issue #413 finding 6), and a route that was refused has not
/// produced a state to compare.
fn pair_range_shift_vs_singles(
    twins: &Twins,
    rng: &mut Rng,
    pair: &'static str,
) -> Option<Comparison> {
    let section_id = section_with_shots(&twins.left, rng, 3)?;
    let ids = shot_ids(&twins.left, section_id);
    // An INTERIOR range, so the gap-close has a head and a tail and the section
    // cannot be emptied - the two conditions the range path refuses on.
    let start = 1 + rng.below(ids.len().saturating_sub(2).max(1));
    let count = 1 + rng.below(2);
    let end = (start + count - 1).min(ids.len().saturating_sub(2));
    if start > end {
        return None;
    }
    // Read BEFORE either route runs, off the left twin - the twins are asserted
    // equal at this point, so this is both routes' starting position.
    let partners = doomed_anchor_partners(&twins.left, section_id, ids[start], ids[end]);
    let before = adjusted_positions(&adjusted_geometry_of(&twins.left));

    let ranged = twins
        .left
        .execute_delete_shot_range_shift(section_id, ids[start], ids[end]);
    if !ranged.success {
        return None;
    }
    for _ in start..=end {
        let live = shot_ids(&twins.right, section_id);
        let &target = live.get(start)?;
        let single = twins.right.execute(Command::DeleteShot {
            section_id,
            shot_id: target,
        });
        if !single.success {
            return None;
        }
    }

    let documented = partners.map(|partners| Documented { partners, before });
    let mut found = twins.compare_allowing(pair, Mask::Strict, documented.as_ref());
    if let Some(documented) = documented.as_ref() {
        // Counted, so a generator that stopped producing the doomed-anchor shape
        // shows up as an arm that asserted nothing instead of as a green run -
        // the gate asserts this counter is non-zero.
        *found.shapes.entry(DOOMED_ANCHOR_SHAPE).or_insert(0) += 1;
        found
            .problems
            .extend(documented_divergence_problems(twins, documented));
    }
    let (unwound, _) = twins.unwind_and_compare_allowing(pair, Mask::Strict, documented.as_ref());
    found.absorb(unwound.prefixed("AFTER UNWIND: "));
    Some(found)
}

/// The name the doomed-anchor arm is counted under. See `Comparison::shapes`.
const DOOMED_ANCHOR_SHAPE: &str = "doomed-anchor-range";

/// The sections a gap-close of `[start_shot..=end_shot]` may move by cutting a
/// merge, or `None` when it cuts none - which is the shape where the two delete
/// routes have to agree exactly.
///
/// The removed set comes from the app's own `preview_delete_shot_range` rather
/// than from a second copy of the "which stations does a gap-close remove" rule,
/// so a change to that rule cannot leave this oracle allowing the wrong sections.
/// It is then closed over the merge graph WITHOUT passing through the edited
/// section: a partner that the delete cuts loose drags whatever is merged to it.
fn doomed_anchor_partners(
    state: &AppState,
    section_id: SectionId,
    start_shot: ShotId,
    end_shot: ShotId,
) -> Option<std::collections::BTreeSet<SectionId>> {
    let project = state.get_project_state()?;
    let impact = project
        .preview_delete_shot_range(section_id, start_shot, end_shot)
        .ok()?;
    if impact.removed_merges.is_empty() {
        return None;
    }
    let mut partners: std::collections::BTreeSet<SectionId> = impact
        .removed_merges
        .iter()
        .flat_map(|merge| [merge.station_a.section_id, merge.station_b.section_id])
        .filter(|id| *id != section_id)
        .collect();
    // A merge wholly inside the edited section (a loop) removes no partner, and
    // is not the documented shape: nothing is allowed to differ.
    if partners.is_empty() {
        return None;
    }
    let merges = read_targets(state).merges;
    loop {
        let mut grew = false;
        for (a, b) in &merges {
            for (near, far) in [(a, b), (b, a)] {
                if partners.contains(&near.0) && far.0 != section_id && partners.insert(far.0) {
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }
    Some(partners)
}

/// The two positive assertions the documented divergence carries, on top of the
/// confinement [`Documented`] applies to the five oracles.
///
/// 1. DIRECTION. Where the two routes disagree about a partner station, the
///    ATOMIC route (left) must be the one holding it at its pre-delete position.
///    The divergence the other way round - the atomic path being the one that
///    moved the partner - is that path breaking the contract this pair now
///    documents, and is reported.
/// 2. THE CUT-LOOSE PARTNER. A partner section the delete left with no merge and
///    no datum has nothing to hold it anywhere; the atomic route must have left
///    it exactly where it was. This is the PR #95 property
///    (`test_delete_range_shift_does_not_displace_free_floating_merged_section`)
///    asserted on random topology, and it fires whether or not the singles route
///    diverged, so the arm is never vacuous.
fn documented_divergence_problems(twins: &Twins, documented: &Documented) -> Vec<String> {
    let atomic = adjusted_positions(&adjusted_geometry_of(&twins.left));
    let singles = adjusted_positions(&adjusted_geometry_of(&twins.right));
    let anchors = read_targets(&twins.left);
    let mut problems = Vec::new();
    for (key, was) in &documented.before {
        let Some(section_id) = section_of_station_key(key) else {
            continue;
        };
        if !documented.partners.contains(&section_id) {
            continue;
        }
        let (Some(&now), Some(&other)) = (atomic.get(key), singles.get(key)) else {
            // A partner section is not the one being edited, so none of its
            // stations may disappear on either side.
            problems.push(format!(
                "partner station {key} is missing from one route's solve: atomic {:?}, singles {:?}",
                atomic.get(key),
                singles.get(key)
            ));
            continue;
        };
        if positions_differ_in_plane(now, other) && positions_differ_in_plane(now, *was) {
            problems.push(format!(
                "the two routes disagree about partner station {key} AND the ATOMIC route is the \
                 one that moved it: was {was:?}, atomic {now:?}, singles {other:?} - the \
                 documented divergence is the single deletes dragging the partner, not the range \
                 delete"
            ));
        }
        let held = anchors
            .merges
            .iter()
            .any(|(a, b)| a.0 == section_id || b.0 == section_id)
            || anchors.datums.iter().any(|(id, _)| *id == section_id);
        if !held && positions_differ_in_plane(now, *was) {
            problems.push(format!(
                "the range delete cut section {section_id} loose (no merge, no datum) and MOVED \
                 it: station {key} was {was:?}, atomic {now:?} - a deferred cascade must leave a \
                 free-floating partner where it was (PR #95, \
                 test_delete_range_shift_does_not_displace_free_floating_merged_section)"
            ));
        }
    }
    problems
}

/// PAIR 2. A ONE-SHOT range split against the single-shot split it generalizes.
///
/// The one-element case is the whole point: a generalization that disagrees with
/// its own base case on the base case is a defect in one of the two, and both
/// are hand-written implementations of the same operation.
///
/// The names are supplied identically on both sides so a name difference cannot
/// be mistaken for a structural one.
fn pair_range_split_of_one_vs_delete_and_split(
    twins: &Twins,
    rng: &mut Rng,
    pair: &'static str,
) -> Option<Comparison> {
    let section_id = section_with_shots(&twins.left, rng, 3)?;
    let ids = shot_ids(&twins.left, section_id);
    // Strictly interior: both paths refuse a prefix or suffix range.
    let position = 1 + rng.below(ids.len() - 2);
    let target = ids[position];
    let ranged = twins.left.execute_delete_shot_range_split(
        section_id,
        target,
        target,
        "A".into(),
        "B".into(),
    );
    if !ranged.success {
        return None;
    }
    let single =
        twins
            .right
            .execute_delete_shot_and_split(section_id, target, "A".into(), "B".into());
    if !single.success {
        return Some(Comparison {
            problems: vec![format!(
                "the range path accepted a one-shot range that the single-shot path it \
                 generalizes refused: {}",
                single.error.unwrap_or_default()
            )],
            hits: BTreeMap::new(),
            shapes: BTreeMap::new(),
        });
    }
    // Both routes MINT a section, so the new section's uuids cannot agree and
    // are masked. Everything else - the split point, which shots went to which
    // side, the offsets that keep B in place, the allocators - is strict.
    let mut found = twins.compare(pair, Mask::MintedUuids);
    let (unwound, _) = twins.unwind_and_compare(pair, Mask::MintedUuids);
    found.absorb(unwound.prefixed("AFTER UNWIND: "));
    Some(found)
}

/// PAIR 3. `Command::Batch` against the same commands executed one at a time.
///
/// The two are NOT the same on the undo stack and are not claimed to be - a
/// batch is one entry, the sequence is `n` - so the stacks are deliberately not
/// compared. What is compared is the STATE, and a difference there means the
/// geometry epilogue is path-dependent: `execute` runs `reconcile_geometry` after
/// each geometry command, and `execute_batch` runs it once at the end
/// (`commands.rs:789`). If reconciling twice is not reconciling once, a batch
/// and a sequence of the same edits land the survey in different places.
fn pair_batch_vs_sequential(
    twins: &Twins,
    rng: &mut Rng,
    pair: &'static str,
) -> Option<Comparison> {
    let commands = batch_commands(&twins.left, rng)?;
    let batched = twins.left.execute(Command::Batch {
        commands: commands.clone(),
    });
    if !batched.success {
        return None;
    }
    for command in commands {
        if !twins.right.execute(command).success {
            // The batch succeeded and a single step did not: the two disagree
            // about what is legal, which is a real difference but a different
            // claim than this pair's. Reported rather than skipped.
            return Some(Comparison {
                problems: vec![
                    "a command the batch accepted was refused when run on its own".to_string(),
                ],
                hits: BTreeMap::new(),
                shapes: BTreeMap::new(),
            });
        }
    }
    let mut found = twins.compare(pair, Mask::Strict);
    let (unwound, _) = twins.unwind_and_compare(pair, Mask::Strict);
    found.absorb(unwound.prefixed("AFTER UNWIND: "));
    Some(found)
}

/// PAIR 4. The FAILURE contract, which is the one place the two routes are
/// promised to differ - so this asserts the difference rather than equality.
///
/// A batch whose last sub-command cannot succeed must roll back to its
/// before-snapshot and push nothing; the same commands run singly apply every
/// one that succeeded. So: the left twin must be EXACTLY as it started with an
/// empty undo stack, and the right twin must have moved.
fn pair_poisoned_batch_is_a_no_op(
    twins: &Twins,
    rng: &mut Rng,
    _pair: &'static str,
) -> Option<Comparison> {
    let mut commands = batch_commands(&twins.left, rng)?;
    let before = mask_timestamps(&canonical_snapshot(&twins.left, false));
    // A section id that cannot exist. Placed LAST so everything before it
    // succeeds and there is something real to roll back.
    commands.push(Command::MoveSection {
        section_id: 999_999,
        offset: crate::models::Coordinates::new(1.0, 1.0, 1.0),
    });
    let batched = twins.left.execute(Command::Batch {
        commands: commands.clone(),
    });
    let mut problems: Vec<String> = Vec::new();
    if batched.success {
        return Some(Comparison {
            problems: vec!["a batch containing an impossible command reported success".to_string()],
            hits: BTreeMap::new(),
            shapes: BTreeMap::new(),
        });
    }
    let after = mask_timestamps(&canonical_snapshot(&twins.left, false));
    if before != after {
        let diffs = diff_snapshots(&before, &after);
        problems.push(format!(
            "a FAILED batch left the project changed in {} path(s), so its rollback is not a \
             rollback:\n{}",
            diffs.len(),
            describe_diffs(&diffs)
        ));
    }
    if !twins.left.get_undo_stack().is_empty() {
        problems.push(format!(
            "a FAILED batch pushed {} undo entry(ies); a no-op must push none",
            twins.left.get_undo_stack().len()
        ));
    }
    // The sequential route, for the other half of the contract: it partially
    // applies. Anything it does is legitimate; what would NOT be is the sequence
    // also being a no-op, which would mean the commands were vacuous and this
    // pair proved nothing.
    let mut applied = 0;
    for command in commands {
        if twins.right.execute(command).success {
            applied += 1;
        }
    }
    if applied == 0 {
        return None;
    }
    if mask_timestamps(&canonical_snapshot(&twins.right, false)) == before {
        problems.push(format!(
            "{applied} command(s) ran singly and changed nothing, so the rollback assertion above \
             was vacuous"
        ));
    }
    Some(Comparison {
        problems,
        hits: BTreeMap::new(),
        shapes: BTreeMap::new(),
    })
}

/// The sub-commands both batch pairs use: geometry edits only, so the epilogue
/// they are about actually runs.
fn batch_commands(state: &AppState, rng: &mut Rng) -> Option<Vec<Command>> {
    let targets = read_targets(state);
    if targets.sections.is_empty() {
        return None;
    }
    let mut commands = Vec::new();
    for _ in 0..rng.range(2, 4) {
        let built = match rng.below(3) {
            0 => pick_shot(&targets, rng).map(|(section_id, shot_id)| Command::UpdateShot {
                section_id,
                shot_id,
                updates: ShotUpdates {
                    length: Some(rng.float(1.0, 15.0)),
                    ..ShotUpdates::default()
                },
            }),
            1 => Some(Command::MoveSection {
                section_id: targets.sections[rng.below(targets.sections.len())].0,
                offset: random_offset(rng),
            }),
            _ => Some(Command::ReverseSection {
                section_id: targets.sections[rng.below(targets.sections.len())].0,
                swap_left_right: rng.chance(50),
            }),
        };
        if let Some(command) = built {
            commands.push(command);
        }
    }
    (!commands.is_empty()).then_some(commands)
}

/// PAIR 5. `ReverseSection` twice, against doing nothing at all.
///
/// The left twin is untouched and the right twin reverses the same section
/// twice, so the comparison is literally "is this an involution". Both flag
/// values are tried in the same run because `swap_left_right` reverses a
/// SECOND, independent thing (the L/R wall distances) and an involution in one
/// says nothing about the other.
fn pair_reverse_twice_is_identity(
    twins: &Twins,
    rng: &mut Rng,
    pair: &'static str,
) -> Option<Comparison> {
    let section_id = section_with_shots(&twins.left, rng, 2)?;
    let swap = rng.chance(50);
    for _ in 0..2 {
        let reversed = twins.right.execute(Command::ReverseSection {
            section_id,
            swap_left_right: swap,
        });
        if !reversed.success {
            return None;
        }
    }
    let mut found = twins.compare(pair, Mask::Strict);
    let (unwound, _) = twins.unwind_and_compare(pair, Mask::Strict);
    found.absorb(unwound.prefixed("AFTER UNWIND: "));
    Some(found)
}

/// Split Section Here (#335) at a random mid-section station, then undo it,
/// against doing nothing. Every comment/datum side is drawn, so the copies the
/// split makes onto the second half's start all have to vanish on undo.
fn pair_split_section_then_undo_is_identity(
    twins: &Twins,
    rng: &mut Rng,
    pair: &'static str,
) -> Option<Comparison> {
    let section_id = section_with_shots(&twins.left, rng, 2)?;
    let shot_count = twins
        .left
        .get_project_state()?
        .get_section(section_id)?
        .shots
        .len();
    let station_index = 1 + rng.below(shot_count - 1) as u32;
    let side = |n: usize| match n {
        0 => crate::models::SplitSide::A,
        1 => crate::models::SplitSide::B,
        _ => crate::models::SplitSide::Both,
    };
    let split = twins.right.execute_split_section(
        section_id,
        crate::models::SplitAt::Station { station_index },
        crate::models::SplitSectionOptions {
            name_a: "split-a".to_string(),
            name_b: "split-b".to_string(),
            color_a: "#111111".to_string(),
            color_b: "#222222".to_string(),
            comment_side: side(rng.below(3)),
            datum_side: side(rng.below(2)),
        },
    );
    if !split.success {
        return None;
    }
    let undo = twins.right.undo();
    assert!(
        undo.success,
        "{pair}: the split of section {section_id} at station {station_index} succeeded but \
         its undo failed: {:?}",
        undo.error
    );
    let mut found = twins.compare(pair, Mask::Strict);
    let (unwound, _) = twins.unwind_and_compare(pair, Mask::Strict);
    found.absorb(unwound.prefixed("AFTER UNWIND: "));
    Some(found)
}

fn split_options(name: &str, color: &str) -> crate::models::SplitSectionOptions {
    crate::models::SplitSectionOptions {
        name_a: name.to_string(),
        name_b: format!("{name}-B"),
        color_a: color.to_string(),
        color_b: color.to_string(),
        comment_side: crate::models::SplitSide::A,
        datum_side: crate::models::SplitSide::A,
    }
}

fn combine_options(
    name: &str,
    color: &str,
    swap_lr: bool,
) -> crate::models::CombineSectionsOptions {
    crate::models::CombineSectionsOptions {
        name: name.to_string(),
        color: color.to_string(),
        swap_lr,
        comment_pick: crate::models::CombinePick::Kept,
        name_pick: crate::models::CombinePick::Kept,
    }
}

/// Split a random section at a mid station that no merge or closure touches
/// (a merge there makes the join a branch, which Combine refuses), on each of
/// `sides`. Returns the two halves and the section's original name and colour.
/// `None` when no such station exists, the split is refused, or it changed the
/// merges (the orphan-closure sweep promoted a closure, which no combine
/// undoes).
fn split_at_a_free_station(
    twins: &Twins,
    rng: &mut Rng,
    sides: &[Side],
) -> Option<(SectionId, SectionId, String, String)> {
    let section_id = section_with_shots(&twins.left, rng, 2)?;
    credit_shots_distinctly(twins, rng, section_id);
    let project = twins.left.get_project_state()?;
    let section = project.get_section(section_id)?;
    let (name, color) = (
        section.name.clone().unwrap_or_default(),
        section.color.clone(),
    );
    let touched = |at: StationRef| {
        let hit = |key: &crate::models::StationKey| project.resolve_key(key) == Some(at);
        project
            .station_merges
            .iter()
            .any(|m| hit(&m.station_a) || hit(&m.station_b))
            || project
                .loop_closures
                .iter()
                .any(|c| hit(&c.station_a) || hit(&c.station_b))
    };
    let free: Vec<u32> = (1..section.shots.len() as u32)
        .filter(|&i| !touched(StationRef::new(section_id, i)))
        .collect();
    if free.is_empty() {
        return None;
    }
    let station_index = free[rng.below(free.len())];
    let merges = project.station_merges.len();
    let mut second = None;
    for &side in sides {
        let state = twins.state(side);
        let split = state.execute_split_section(
            section_id,
            crate::models::SplitAt::Station { station_index },
            split_options(&name, &color),
        );
        if !split.success || state.get_project_state()?.station_merges.len() != merges {
            return None;
        }
        second = split.section_id;
    }
    Some((section_id, second?, name, color))
}

/// Give every shot of `section_id` its own survey credit, the same on both
/// twins (one shot in three left uncredited), so a pair that splits and
/// combines it compares PER-SHOT credit rather than one template every shot
/// shares. Written straight onto both projects: it is the twins' common
/// starting state, not a step of either route.
fn credit_shots_distinctly(twins: &Twins, rng: &mut Rng, section_id: SectionId) {
    let Some(count) = twins
        .left
        .get_project_state()
        .and_then(|p| Some(p.get_section(section_id)?.shots.len()))
    else {
        return;
    };
    let credits: Vec<Option<crate::models::SurveyCredit>> = (0..count)
        .map(|i| {
            (i % 3 != 2).then(|| crate::models::SurveyCredit {
                surveyors: crate::models::Person::named(&format!("Surveyor {}", rng.below(4)))
                    .into_iter()
                    .collect(),
                explorers: crate::models::Person::named(&format!("Explorer {}", rng.below(3)))
                    .into_iter()
                    .collect(),
                date: chrono::NaiveDate::from_ymd_opt(2026, 1 + rng.below(12) as u32, 1),
            })
        })
        .collect();
    for side in [&twins.left, &twins.right] {
        let mut guard = match side.project.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(section) = guard.as_mut().and_then(|p| p.get_section_mut(section_id)) {
            for (shot, credit) in section.shots.iter_mut().zip(&credits) {
                shot.set_credit(credit.clone().unwrap_or_default());
            }
        }
    }
}

/// Two sections both twins can combine: a pair the project already holds, or
/// one made by splitting a section on both twins.
fn combinable_pair(twins: &Twins, rng: &mut Rng) -> Option<(SectionId, SectionId)> {
    let project = twins.left.get_project_state()?;
    let ids: Vec<SectionId> = project.sections.iter().map(|s| s.id).collect();
    let mut natural = Vec::new();
    for (i, &a) in ids.iter().enumerate() {
        for &b in &ids[i + 1..] {
            if project
                .combine_candidate_for_sections(a, b)
                .is_some_and(|c| c.blocked_reason.is_none())
            {
                natural.push((a, b));
            }
        }
    }
    if !natural.is_empty() {
        return Some(natural[rng.below(natural.len())]);
    }
    let (first, second, _, _) = split_at_a_free_station(twins, rng, &[Side::Left, Side::Right])?;
    twins
        .left
        .get_project_state()?
        .combine_candidate_for_sections(first, second)?
        .blocked_reason
        .is_none()
        .then_some((first, second))
}

/// Combine Sections, then undo it, against doing nothing.
fn pair_combine_sections_then_undo_is_identity(
    twins: &Twins,
    rng: &mut Rng,
    pair: &'static str,
) -> Option<Comparison> {
    let (a, b) = combinable_pair(twins, rng)?;
    let (kept, other) = if rng.chance(50) { (a, b) } else { (b, a) };
    let combined = twins.right.execute_combine_sections(
        kept,
        other,
        combine_options("c", "#333333", rng.chance(50)),
    );
    assert!(
        combined.success,
        "{pair}: the candidate said {kept} and {other} combine, but the combine failed: {:?}",
        combined.error
    );
    let undo = twins.right.undo();
    assert!(
        undo.success,
        "{pair}: the combine's undo failed: {:?}",
        undo.error
    );
    let mut found = twins.compare(pair, Mask::Strict);
    let (unwound, _) = twins.unwind_and_compare(pair, Mask::Strict);
    found.absorb(unwound.prefixed("AFTER UNWIND: "));
    Some(found)
}

/// The stored project minus what a split-then-combine is allowed to change:
/// the id allocators, the shot ids (positions instead), and the gauge pair's
/// SPELLING (the gauge station itself is compared
/// separately, through the solver's own anchor).
fn survey_content(state: &AppState) -> String {
    let snapshot = mask_timestamps(&canonical_snapshot(state, true));
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&snapshot) else {
        return snapshot;
    };
    if let serde_json::Value::Object(map) = &mut value {
        map.remove("next_section_id");
        if let Some(serde_json::Value::Array(sections)) = map.get_mut("sections") {
            for section in sections {
                if let serde_json::Value::Object(section) = section {
                    for key in ["next_shot_id", "gauge_shot_uuid", "gauge_neighbour_uuid"] {
                        section.remove(key);
                    }
                }
            }
        }
    }
    value.to_string()
}

/// Split Section Here at a free mid station, then Combine Sections at the join
/// with the original name and colour, against doing nothing.
fn pair_split_then_combine_at_the_join_is_identity(
    twins: &Twins,
    rng: &mut Rng,
    pair: &'static str,
) -> Option<Comparison> {
    let (first, second, name, color) = split_at_a_free_station(twins, rng, &[Side::Right])?;
    let candidate = twins
        .right
        .get_project_state()?
        .combine_candidate_for_sections(first, second)?;
    if candidate.blocked_reason.is_some() {
        // A section passes through the cut, the halves are tied elsewhere,
        // or the cut's two copies sit more than 1 m apart in depth: Combine
        // refuses it.
        return None;
    }
    let combined =
        twins
            .right
            .execute_combine_sections(first, second, combine_options(&name, &color, false));
    assert!(combined.success, "{pair}: {:?}", combined.error);

    let mut found = Comparison::default();
    let diffs = diff_snapshots(&survey_content(&twins.left), &survey_content(&twins.right));
    if !diffs.is_empty() {
        found.problems.push(format!(
            "the stored projects differ in {} path(s):\n{}",
            diffs.len(),
            describe_diffs(&diffs)
        ));
    }
    let geometry = adjusted_diffs(
        &adjusted_geometry_of(&twins.left),
        &adjusted_geometry_of(&twins.right),
    );
    if !geometry.is_empty() {
        found.problems.push(format!(
            "the ADJUSTED geometry differs:\n    {}",
            geometry.join("\n    ")
        ));
    }
    let (left, right) = (
        twins.left.get_project_state()?,
        twins.right.get_project_state()?,
    );
    for section in &left.sections {
        let (l, r) = (
            left.adjustment_anchor_for_tests(section.id),
            right.adjustment_anchor_for_tests(section.id),
        );
        if l != r {
            found.problems.push(format!(
                "section {}'s gauge station moved: {l:?} -> {r:?}",
                section.id
            ));
        }
    }
    if integrity_verdict(&twins.left) != integrity_verdict(&twins.right) {
        found
            .problems
            .push("the app's own integrity verdicts differ".to_string());
    }
    Some(found)
}

/// Two sections that meet END to END: combine them directly (the combine
/// reverses the other one), against reversing it first and combining end to
/// start.
fn pair_combine_end_to_end_is_reverse_then_combine(
    twins: &Twins,
    rng: &mut Rng,
    pair: &'static str,
) -> Option<Comparison> {
    let (first, second, _, _) = split_at_a_free_station(twins, rng, &[Side::Left, Side::Right])?;
    // Reverse the second half on both twins: the halves now meet end to end.
    for side in [&twins.left, &twins.right] {
        let reversed = side.execute(Command::ReverseSection {
            section_id: second,
            swap_left_right: false,
        });
        if !reversed.success {
            return None;
        }
    }
    let candidate = twins
        .left
        .get_project_state()?
        .combine_candidate_for_sections(first, second)?;
    if candidate.blocked_reason.is_some() || !candidate.reverses {
        return None;
    }
    let swap_lr = rng.chance(50);
    let options = combine_options("c", "#333333", swap_lr);
    let direct = twins
        .left
        .execute_combine_sections(first, second, options.clone());
    assert!(direct.success, "{pair}: {:?}", direct.error);
    let reversed = twins.right.execute(Command::ReverseSection {
        section_id: second,
        swap_left_right: swap_lr,
    });
    assert!(reversed.success, "{pair}: {:?}", reversed.error);
    let combined = twins.right.execute_combine_sections(first, second, options);
    assert!(combined.success, "{pair}: {:?}", combined.error);
    Some(twins.compare(pair, Mask::Strict))
}

/// PAIR 6. `MoveSection(+d)` then `MoveSection(-d)`, against doing nothing.
///
/// An inverse pair rather than an involution, and the arithmetic is exact: the
/// offsets are quantized to 1/1000 by the generator, so `+d` and `-d` cancel in
/// binary floating point without rounding.
fn pair_move_out_and_back_is_identity(
    twins: &Twins,
    rng: &mut Rng,
    pair: &'static str,
) -> Option<Comparison> {
    let targets = read_targets(&twins.left);
    if targets.sections.is_empty() {
        return None;
    }
    let section_id = targets.sections[rng.below(targets.sections.len())].0;
    let offset = random_offset(rng);
    let out = twins
        .right
        .execute(Command::MoveSection { section_id, offset });
    if !out.success {
        return None;
    }
    let back = twins.right.execute(Command::MoveSection {
        section_id,
        offset: crate::models::Coordinates::new(-offset.x, -offset.y, -offset.z),
    });
    if !back.success {
        return None;
    }
    let mut found = twins.compare(pair, Mask::Strict);
    let (unwound, _) = twins.unwind_and_compare(pair, Mask::Strict);
    found.absorb(unwound.prefixed("AFTER UNWIND: "));
    Some(found)
}

/// PAIR 7. `CreateSection { shots }` against `CreateSection { shots: [] }`
/// followed by one `CreateShot` append per shot.
///
/// Both routes MINT everything, so uuids are masked - but the section's shape is
/// not: same shot count, same field values in the same order, same
/// `next_shot_id`, same absence or presence of a `section_offsets` key. The last
/// is the one worth having: the append path is offset-neutral by design (it is
/// half of issue #413's finding 3) and the bulk constructor takes a different
/// route into `Section::new`.
fn pair_create_with_shots_vs_appends(
    twins: &Twins,
    rng: &mut Rng,
    pair: &'static str,
) -> Option<Comparison> {
    let count = 1 + rng.below(3);
    let shots: Vec<ShotData> = (0..count).map(|_| random_shot(rng)).collect();
    let name = format!("pair-{}", rng.range(1000, 9999));
    let color = (*rng.pick(COLORS)).to_string();
    let bulk = twins.left.execute(Command::CreateSection {
        name: Some(name.clone()),
        surveyor: None,
        date: None,
        color: color.clone(),
        direction: None,
        shots: shots.clone(),
    });
    if !bulk.success {
        return None;
    }
    let empty = twins.right.execute(Command::CreateSection {
        name: Some(name),
        surveyor: None,
        date: None,
        color,
        direction: None,
        shots: Vec::new(),
    });
    let section_id = empty.section_id?;
    for shot in shots {
        let appended = twins.right.execute(Command::CreateShot {
            section_id,
            shot,
            after_shot_id: None,
            before_shot_id: None,
        });
        if !appended.success {
            return None;
        }
    }
    let mut found = twins.compare(pair, Mask::MintedUuids);
    let (unwound, _) = twins.unwind_and_compare(pair, Mask::MintedUuids);
    found.absorb(unwound.prefixed("AFTER UNWIND: "));
    Some(found)
}

// ===========================================================================
// The driver
// ===========================================================================

/// Run every pair once per `(seed, repeat)`, each on FRESH twins.
///
/// Fresh twins per pair rather than one pair of states walked through all of
/// them: a pair that diverges leaves the two sides different, and every later
/// pair on the same twins would then re-report that one divergence. The cost is
/// a save and two loads per pair, which is what makes this a seeded sweep rather
/// than something the gate runs thousands of.
fn run_pairs(
    seed: u32,
    repeats: usize,
    closures: bool,
    prelude: usize,
) -> BTreeMap<&'static str, PairOutcome> {
    let mut out: BTreeMap<&'static str, PairOutcome> = BTreeMap::new();
    for pair in PAIRS {
        let entry = out.entry(pair.id).or_insert_with(|| PairOutcome {
            built: 0,
            skipped: 0,
            problems: Vec::new(),
            hits: BTreeMap::new(),
            shapes: BTreeMap::new(),
            signatures: std::collections::BTreeSet::new(),
            baseline_distance: 0,
        });
        for repeat in 0..repeats {
            // The RNG is seeded per (seed, pair, repeat) so one pair's draw
            // count cannot move another pair's stream - which is what keeps a
            // pinned seed pinned when a pair is added or changed.
            let mut rng = Rng::new(
                seed.wrapping_mul(1_000_003)
                    .wrapping_add((repeat as u32).wrapping_mul(7919)),
            );
            let Some(twins) = Twins::build(seed, closures, prelude) else {
                entry.skipped += 1;
                continue;
            };
            entry.signatures.insert(twins.signature.clone());
            match (pair.run)(&twins, &mut rng, pair.id) {
                None => entry.skipped += 1,
                Some(found) => {
                    entry.built += 1;
                    let (_, distance) = twins.unwind_and_compare(pair.id, Mask::Strict);
                    entry.baseline_distance += distance.0 + distance.1;
                    for (id, count) in found.hits {
                        *entry.hits.entry(id).or_insert(0) += count;
                    }
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
    let mut parts = Vec::new();
    for (id, outcome) in outcomes {
        let join = |counts: &BTreeMap<&'static str, usize>| -> String {
            counts
                .iter()
                .map(|(id, count)| format!("{id}={count}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let hits = join(&outcome.hits);
        let shapes = join(&outcome.shapes);
        parts.push(format!(
            "{id}: {} built, {} skipped, {} problem(s), baseline-distance {}{}{}",
            outcome.built,
            outcome.skipped,
            outcome.problems.len(),
            outcome.baseline_distance,
            if hits.is_empty() {
                String::new()
            } else {
                format!("; classified: {hits}")
            },
            if shapes.is_empty() {
                String::new()
            } else {
                format!("; shapes: {shapes}")
            }
        ));
    }
    format!("metamorphic seed={seed}\n  {}", parts.join("\n  "))
}

// ===========================================================================
// The gate
// ===========================================================================

/// Seeds the gate runs. PINNED, and pinned to seeds that are clean TODAY - see
/// `metamorphic_fuzz` for how the list was chosen and how to re-choose it.
const GATE_SEEDS: &[u32] = &[1, 2, 3, 4, 5, 6, 7, 8];
/// Prelude commands before the twins split. Enough to give the pairs varied
/// topology (merged groups, sections of several lengths, an offset or two)
/// without spending the run in the generator.
const GATE_PRELUDE: usize = 12;
/// Times each pair is built per seed, each on fresh twins.
const GATE_REPEATS: usize = 3;

/// THE METAMORPHIC GATE. Not `#[ignore]`d, so it runs on every `cargo test`.
///
/// # Scope, stated honestly
///
/// It asserts SEVEN equivalences, every one of which is promised somewhere - two
/// of them in the code's own doc comments, in the word "generalizes". It does
/// NOT assert that either route is correct: two routes that are wrong in the
/// same way agree, and this says nothing about them. That is the roundtrip
/// fuzzer's job and the reason both exist.
///
/// It also does not assert that an unwind returns to where it started. Findings
/// 1, 2, 3 and 5 are all "an unwind does not", they are open, and re-asserting
/// them here would make this a second and worse report of them. The distance is
/// MEASURED and printed instead, so a regression in it is visible in the diff of
/// the summary line without being a false failure today.
#[test]
fn metamorphic_gate() {
    let _config = isolated_config();
    let mut problems = Vec::new();
    let mut built = 0;
    let mut total: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut shapes: BTreeMap<&'static str, usize> = BTreeMap::new();
    for &seed in GATE_SEEDS {
        let outcomes = run_pairs(seed, GATE_REPEATS, false, GATE_PRELUDE);
        println!("{}", summarize(seed, &outcomes));
        for outcome in outcomes.values() {
            built += outcome.built;
            problems.extend(outcome.problems.iter().cloned());
            for (id, count) in &outcome.hits {
                *total.entry(id).or_insert(0) += count;
            }
            for (id, count) in &outcome.shapes {
                *shapes.entry(id).or_insert(0) += count;
            }
        }
    }
    println!("metamorphic gate classified totals: {total:?}");
    println!("metamorphic gate shape totals: {shapes:?}");
    // A run that built nothing passes vacuously, which is the failure mode a
    // skip-heavy generator invites. Every pair has to have been reached.
    assert!(
        built >= GATE_SEEDS.len() * PAIRS.len(),
        "the metamorphic gate built only {built} pair instance(s) over {} seed(s) x {} pair(s): \
         the generators are not reaching the shapes the pairs need",
        GATE_SEEDS.len(),
        PAIRS.len()
    );
    // Same argument, one level down. `range-shift-is-n-single-deletes` asserts
    // one thing on a range that removes a merge and a different thing on a range
    // that does not; a run that only ever drew the second half never exercised
    // the documented divergence at all, and would go green while saying nothing
    // about the property this pair exists for.
    assert!(
        shapes.get(DOOMED_ANCHOR_SHAPE).copied().unwrap_or(0) > 0,
        "no gate seed produced a range whose removed stations carry a merge, so the doomed-anchor \
         arm of range-shift-is-n-single-deletes asserted nothing: re-choose GATE_SEEDS or widen \
         the prelude until the shape is reached again"
    );
    assert!(
        problems.is_empty(),
        "{} metamorphic divergence(s):\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

/// Determinism, for the same reason the roundtrip fuzzer pins it: without it a
/// failing seed is not a repro.
///
/// The shape includes the CLASSIFIED hits, not just the counts, so a
/// classification that starts firing (or stops) is a replay difference rather
/// than something only a reviewer would notice.
///
/// It covers EVERY pair. `one-shot-range-split-is-delete-and-split` used to
/// be filtered out of the shape because it carried
/// `observation-split-routes-disagree-on-station-comment-keys`, which was
/// intermittent on identical inputs; the observation was triaged out as this
/// engine's own sort-before-mask (see `resort_set_like` and the record in
/// `EXPLAINS`), so the pair replays deterministically and the filter is gone.
#[test]
fn the_metamorphic_engine_is_deterministic() {
    let _config = isolated_config();
    for &seed in &GATE_SEEDS[..3] {
        let first = run_pairs(seed, 2, false, GATE_PRELUDE);
        let second = run_pairs(seed, 2, false, GATE_PRELUDE);
        let shape = |outcomes: &BTreeMap<&'static str, PairOutcome>| -> Vec<(
            String,
            usize,
            usize,
            usize,
            BTreeMap<&'static str, usize>,
            BTreeMap<&'static str, usize>,
        )> {
            outcomes
                .iter()
                .map(|(id, outcome)| {
                    (
                        (*id).to_string(),
                        outcome.built,
                        outcome.skipped,
                        outcome.problems.len(),
                        outcome.hits.clone(),
                        // The generated SHAPES too: an arm that stops being
                        // reached is a replay difference, and it is the one that
                        // would otherwise be silent.
                        outcome.shapes.clone(),
                    )
                })
                .collect()
        };
        assert_eq!(
            shape(&first),
            shape(&second),
            "seed {seed} did not replay identically"
        );
    }
}

/// The SWEEP: the same pairs at a scale a gate cannot afford, and the tool for
/// re-choosing `GATE_SEEDS`.
///
/// ```text
/// STALAGMIGHT_RUST_META_SEEDS=60 STALAGMIGHT_RUST_META_REPEATS=6 \
///   STALAGMIGHT_RUST_META_CLOSURES=1 \
///   TELEMETRY_ENDPOINT=http://localhost/dummy cargo test \
///   --manifest-path src-tauri/Cargo.toml -p stalagmight-core \
///   --lib metamorphic_fuzz -- --ignored --nocapture
/// ```
///
/// It catches each seed's problems instead of stopping, so one divergence does
/// not hide the rest, and prints a per-pair total at the end - which is what a
/// triage read wants.
#[test]
#[ignore = "sweep: prints per-seed status, used to choose the metamorphic GATE_SEEDS"]
fn metamorphic_fuzz() {
    let _config = isolated_config();
    let seeds: u32 = std::env::var("STALAGMIGHT_RUST_META_SEEDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(40);
    let repeats: usize = std::env::var("STALAGMIGHT_RUST_META_REPEATS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(4);
    let prelude: usize = std::env::var("STALAGMIGHT_RUST_META_PRELUDE")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(GATE_PRELUDE);
    let closures = std::env::var("STALAGMIGHT_RUST_META_CLOSURES").is_ok();
    let verbose = std::env::var("STALAGMIGHT_RUST_META_VERBOSE").is_ok();

    let mut totals: BTreeMap<&'static str, (usize, usize, usize, usize)> = BTreeMap::new();
    let mut classified: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut shapes: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut signatures: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut dirty = Vec::new();
    for seed in 1..=seeds {
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
            let entry = totals.entry(id).or_insert((0, 0, 0, 0));
            entry.0 += outcome.built;
            entry.1 += outcome.skipped;
            entry.2 += outcome.problems.len();
            entry.3 += outcome.baseline_distance;
            seed_problems += outcome.problems.len();
            for (hit, count) in &outcome.hits {
                *classified.entry(hit).or_insert(0) += count;
            }
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
    for (id, (built, skipped, problems, distance)) in &totals {
        println!(
            "  {id}: {built} built, {skipped} skipped, {problems} problem(s), \
             baseline-distance {distance}"
        );
    }
    println!(
        "topology coverage: {} distinct signature(s) over {seeds} seed(s)",
        signatures.len()
    );
    println!("classified totals: {classified:?}");
    println!("generated shape totals: {shapes:?}");
    println!(
        "seeds with at least one UNEXPLAINED divergence ({}): {dirty:?}",
        dirty.len()
    );
}
