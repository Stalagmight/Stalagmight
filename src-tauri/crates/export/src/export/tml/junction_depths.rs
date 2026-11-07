//! Junction depths in the Ariane export (#576 TML-DEPTH-D1, D2).
//!
//! A station has ONE depth in Ariane's chain. Ours can have several: the main
//! line arrives at it with one reading, and every branch hung off it starts
//! with its own. Where those readings disagree the file still has to state one
//! number, and the rule is:
//!
//! - The junction is exported at the MEAN of its readings: the depth of the row
//!   that stands for the station (the main line's), plus ONE reading per section
//!   hung there - the `depth_in` of that section's branch start, a block whose
//!   `FromID` resolved to the station through a station merge, however many
//!   merges deep. A section counts once however many pieces it has there. One
//!   merged part-way along leaves the junction in both directions, a forward
//!   block and a reversed one, and its reading is the depth it ARRIVES at the
//!   station with - the reversed block's start - as the main line's is the
//!   depth its row ends at. Where the forward block starts from another depth,
//!   that is a second reading inside the section, and the block's first shot
//!   is refitted to start from the junction's exported depth.
//! - Every other station keeps its TRUE depth. A shot after the first on a
//!   branch states its own absolute depth, so it carries no offset.
//! - A shot whose exported depth change now differs from its measured one - the
//!   shots into and out of an averaged junction, on the main line and on every
//!   branch - gets the length that keeps its measured horizontal run,
//!   `L' = sqrt(h^2 + dz'^2)` with `h` from its own length and depth change. The
//!   plan stays where it is; only the slope changes. `L'` is rounded up with
//!   `length_covering_written_delta`, so `|Depth - parent.Depth| <= Length`
//!   holds on the file's decimals.
//!
//! Example: main-line station A reads 10.0 m and a branch's first shot starts
//! at 12.0 m and ends at 12.5 m. A is exported at 11.0 m, the branch shot runs
//! 11.0 -> 12.5 with the length that keeps its run, the main-line shots into
//! and out of A take up +1.0 / -1.0 m the same way, and every later shot keeps
//! its true depth.
//!
//! How the file says it: every changed shot writes the `DepthIn` "same as the
//! previous shot" sentinel, "I start at my parent's `Depth`". Our importer
//! resolves the sentinel to exactly that (`parsers::tml::resolve_tml_depth_in`,
//! a run's first shot from its parent's `Depth` in `tml::linearize`), and
//! SpeleoDB's `openspeleo_lib` never reads `DepthIn`: it takes every shot's
//! origin from the parent's `Depth` (`geojson.py`,
//! `length_2d(origin_depth=current_shot.depth)`). `Depth` is absolute for
//! both, so a junction's mean and a later shot's true absolute depth need no
//! other spelling, in `.tml` or `.tmlu`.
//!
//! A shot whose OWN drop already exceeds its own tape is broken data
//! (TML-DEPTH-Q2-D1). Both its depths stay true, and it goes out as the
//! vertical shot it can at most be: no run (`h = 0`), so `L' = |dz|`, rounded
//! up the same way. No station moves for it, so the shot after it is
//! untouched. Broken is judged on the values as written, so a vertical shot,
//! whose written drop is its written tape, is no broken data for the binary
//! noise of subtracting its depths, and goes out as measured (#576 RVF-d-7).
//! And an explicit `DepthIn` (a second reading of a station inside one
//! section) is no junction and is never averaged (TML-DEPTH-Q3-D1): it is
//! honoured as long as its shot keeps the bound and neither end moved, and
//! otherwise only that shot is refitted, from its parent's exported depth.

use std::collections::{HashMap, VecDeque};

use super::{as_written, length_covering_written_delta, schema, ExportShot};
use crate::models::SectionId;

/// Readings this close are one reading: the tolerance the same-as-previous flag
/// is computed with (`ExportShot::forward`).
const SAME_DEPTH_M: f64 = 1e-9;

/// The export note for junctions exported at their mean (#576 TML-DEPTH-D2).
/// Its values follow the code as a query, `?count=N&largest=X` (metres), for
/// the receipt line the frontend renders.
const JUNCTION_DEPTHS_AVERAGED_NOTE: &str = "junction-depths-averaged";

/// What the junction pass did, for the export receipt.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct JunctionDepths {
    /// Junctions whose readings disagreed, exported at their mean.
    pub(super) averaged: usize,
    /// The widest disagreement among them, highest reading minus lowest (m).
    pub(super) largest_spread: f64,
}

impl JunctionDepths {
    /// The receipt's note, or `None` when every junction agreed.
    pub(super) fn note(&self) -> Option<String> {
        (self.averaged > 0).then(|| {
            format!(
                "{JUNCTION_DEPTHS_AVERAGED_NOTE}?count={}&largest={}",
                self.averaged,
                receipt_metres(self.largest_spread)
            )
        })
    }
}

/// A spread as the receipt states it: to the centimetre a depth gauge
/// resolves, and a smaller one as such rather than as "0.00".
fn receipt_metres(metres: f64) -> String {
    if metres >= 0.005 {
        format!("{metres:.2}")
    } else {
        "<0.01".to_string()
    }
}

/// Settle every reachable station's exported depth, and every shot's start and
/// length against it, by the rule in the module docs. Returns the receipt and
/// which rows a walk down from the roots reaches: a `from_id` cycle is never
/// reached and is left exactly as it is.
pub(super) fn settle(
    shots: &mut [ExportShot],
    index_by_id: &HashMap<i64, usize>,
) -> (JunctionDepths, Vec<bool>) {
    let n = shots.len();
    // Adjacency by INDEX, so the walk can borrow it while mutating `shots`.
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut queue: VecDeque<usize> = VecDeque::new();
    for (i, shot) in shots.iter().enumerate() {
        match index_by_id.get(&shot.from_id) {
            Some(&parent) if shot.from_id != -1 => children[parent].push(i),
            // A root, or a dangling parent id: nothing to measure against.
            _ => queue.push_back(i),
        }
    }
    // Parent before child, so every shot is settled against its parent's
    // exported depth.
    let mut reached = vec![false; n];
    let mut parent: Vec<Option<usize>> = vec![None; n];
    let mut order = Vec::with_capacity(n);
    while let Some(i) = queue.pop_front() {
        if reached[i] {
            continue;
        }
        reached[i] = true;
        order.push(i);
        for &k in &children[i] {
            if !reached[k] {
                parent[k] = Some(i);
                queue.push_back(k);
            }
        }
    }

    // Every station's exported depth: its true one, or its junction's mean. A
    // CLOSURE ties two stations and is no reading of either.
    let mut exported: Vec<f64> = shots.iter().map(|s| s.depth).collect();
    let mut moved = vec![false; n];
    let mut junctions = JunctionDepths::default();
    for &p in &order {
        if shots[p].shot_type == schema::TYPE_CLOSURE {
            continue;
        }
        // One reading per section hung there: the start of its reversed block
        // where it has one (the depth it arrives at the station with), else
        // of its first block.
        let mut readings = vec![shots[p].depth];
        // Per reading after the main line's: its section, and whether it is
        // that section's arriving one.
        let mut sections: Vec<(SectionId, bool)> = Vec::new();
        for k in children[p].iter().map(|&k| &shots[k]) {
            let Some(section) = k.branch_of else {
                continue;
            };
            match sections.iter().position(|&(s, _)| s == section) {
                None => {
                    sections.push((section, k.reversed));
                    readings.push(k.depth_in);
                }
                Some(at) if k.reversed && !sections[at].1 => {
                    sections[at].1 = true;
                    readings[at + 1] = k.depth_in;
                }
                Some(_) => {}
            }
        }
        let (low, high) = readings
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), &r| {
                (low.min(r), high.max(r))
            });
        if high - low <= SAME_DEPTH_M {
            continue;
        }
        exported[p] = readings.iter().sum::<f64>() / readings.len() as f64;
        moved[p] = true;
        junctions.averaged += 1;
        junctions.largest_spread = junctions.largest_spread.max(high - low);
    }

    let mut refitted = 0usize;
    let mut broken = 0usize;
    for &k in &order {
        let end = exported[k];
        let Some(p) = parent[k] else {
            // A root starts from nothing in the file. A START stands on its
            // station, so both its depths are that station's.
            if moved[k] {
                let shot = &mut shots[k];
                let keep_flag = shot.depth_in_same_as_prev;
                let start = if shot.shot_type == schema::TYPE_START {
                    end
                } else {
                    shot.depth_in
                };
                refit(shot, start, end);
                shot.depth_in_same_as_prev = keep_flag;
                refitted += 1;
            }
            continue;
        };
        let start = exported[p];
        let shot = &mut shots[k];
        if shot.shot_type == schema::TYPE_CLOSURE {
            // A tie, not a measurement: it follows whichever end moved, and its
            // length stays the vertical part of the closure error it ties.
            let to = index_by_id
                .get(&shot.closure_to_id)
                .copied()
                .filter(|&t| reached[t]);
            if moved[p] || to.is_some_and(|t| moved[t]) {
                let tie_end = to.map_or(shot.depth, |t| exported[t]);
                shot.depth_in = start;
                shot.depth = tie_end;
                shot.length = covering_length((tie_end - start).abs(), start, tie_end);
            }
            continue;
        }
        if shot.depth_in_same_as_prev {
            if (start - shot.depth_in).abs() < SAME_DEPTH_M && !moved[k] {
                // On the values as written (#576 RVF-d-7): a vertical shot's
                // drop is its tape there, where in binary 10.1 -> 13.3 is
                // 3.200000000000001, a hair over a 3.2 m tape.
                if written_change(shot.depth_in, shot.depth).abs() <= as_written(shot.length) {
                    continue;
                }
                // Broken data: its own drop is longer than its tape. Both
                // depths stay true; it goes out as the vertical shot it can at
                // most be, no run (h = 0) and its drop as its length.
                refit(shot, start, end);
                broken += 1;
                continue;
            }
        } else if !moved[p] && !moved[k] && (shot.depth - start).abs() <= shot.length {
            continue;
        }
        refit(shot, start, end);
        refitted += 1;
    }

    if junctions.averaged > 0 {
        tracing::warn!(
            junctions = junctions.averaged,
            largest_spread_m = junctions.largest_spread,
            shots = refitted,
            "Ariane export: junctions whose depth readings disagree were exported at \
             their mean; the shots touching them keep their plan and take a new length"
        );
    } else if refitted > 0 {
        tracing::warn!(
            shots = refitted,
            "Ariane export: shots whose start no longer matched their parent's depth \
             took a new length that keeps their plan"
        );
    }
    if broken > 0 {
        tracing::warn!(
            shots = broken,
            "Ariane export: shots whose own depth change exceeds their length (broken \
             data) were exported vertical, with that depth change as their length"
        );
    }
    (junctions, reached)
}

/// Export `shot` from `start` to `end` with the length that keeps its measured
/// horizontal run: `L' = sqrt(h^2 + dz'^2)`, rounded up so the bound holds as
/// written. It now starts where its parent ends, which the `DepthIn` sentinel
/// says.
fn refit(shot: &mut ExportShot, start: f64, end: f64) {
    let run = horizontal_run(shot.length, shot.depth - shot.depth_in);
    shot.length = covering_length(run.hypot(end - start), start, end);
    shot.depth_in = start;
    shot.depth = end;
    shot.depth_in_same_as_prev = true;
}

/// The depth change from `start` to `end` as the file states it: the two depths
/// as written, and their difference to the written decimals. Two six-decimal
/// values differ by a six-decimal value, so that rounding takes off only the
/// noise of subtracting them in binary (#576 RVF-d-7).
fn written_change(start: f64, end: f64) -> f64 {
    as_written(as_written(end) - as_written(start))
}

/// `length`, rounded up to the next written micrometre - and never below the
/// change between `start` and `end` AS WRITTEN, which can exceed the exact
/// change by a micrometre once both depths are rounded to six decimals (a tie
/// has no run to spare). Then the bound holds on the file's own decimals in
/// both profiles.
fn covering_length(length: f64, start: f64, end: f64) -> f64 {
    let written_change = (as_written(end) - as_written(start)).abs();
    length_covering_written_delta(length.max(written_change))
}

/// The horizontal run of a shot of `length` whose depth changes by `dz`: zero
/// where the drop is the whole tape or more (a vertical shot, or broken data),
/// as the model's `horizontal_length_from_slope` has it. Taken as a ratio so an
/// absurd tape cannot overflow a square.
fn horizontal_run(length: f64, dz: f64) -> f64 {
    let drop = dz.abs();
    if length <= drop {
        return 0.0;
    }
    let ratio = drop / length;
    length * ((1.0 - ratio) * (1.0 + ratio)).sqrt()
}
