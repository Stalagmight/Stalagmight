//! Turning one declared TML section's shot graph into linear runs.
//!
//! An Ariane `<Section>` is a NAME on a set of shots, not a shape: the shots
//! under one name form an arbitrary directed graph, with junctions and with
//! parents that live under a different name entirely. A `models::Section`, by
//! contrast, is a strictly linear polyline - `Section::shots` is consumed
//! positionally and every cross-reference into it is a single `station_index`
//! - so this module has to cut the graph into linear runs, and the count of
//! those runs is what the import wizard shows as fragments (#412).
//!
//! At a junction only ONE successor can continue the run; the rest are
//! deferred into runs of their own. `choose_continuation` decides which,
//! so that the run that continues is the mainline rather than whichever
//! successor happened to be written first.
//!
//! This module also owns the TRIP rule - the (surveyor, date, colour) triple a
//! `models::Section` can carry one of. Ariane records it per shot, so the same
//! triple decides where runs may merge (`join_contiguous_chains`) and where a
//! run has to be cut (`split_runs_at_trip_boundaries`).

use std::collections::HashMap;

use super::{RawShot, TmlSection, TmlShot, TmlShotType};
use crate::models::SurveyCredit;
// Sections whose shot ids all fit the `section_id * FEATURE_ID_STRIDE + seq`
// encode, as a COUNT (42_949). Shared with the import/merge boundary checks
// (#555) so the cut budget below and the mint-time refusals cannot drift apart.
use crate::models::MAX_SECTIONS_IN_ID_SPACE;

/// How many declared names a merged run spells out before it starts counting.
/// A run of eight joined names is not a label a person can read in the wizard's
/// fragment list.
const MAX_JOINED_NAMES: usize = 3;

/// Character cap on a merged name. Section names are file-supplied and
/// unbounded, so three of them concatenated is attacker-controlled length on
/// a label that flows to the UI.
const MAX_JOINED_NAME_LEN: usize = 200;

/// Build the merged run's label from the names it absorbed.
///
/// Only merged names are sanitised and capped: a lone name is returned
/// verbatim, so this cannot change how an unjoined section is presented.
fn joined_name(names: &[String], extra: usize) -> String {
    if names.is_empty() {
        return String::new();
    }
    if let ([only], 0) = (names, extra) {
        return only.clone();
    }
    // Section names come from the file, so they can carry control characters
    // that would corrupt a log line or a UI label once concatenated.
    let mut out = names
        .iter()
        .map(|n| n.chars().filter(|c| !c.is_control()).collect::<String>())
        .collect::<Vec<_>>()
        .join(" + ");
    if extra > 0 {
        out.push_str(&format!(" + {extra} more"));
    }
    if out.chars().count() > MAX_JOINED_NAME_LEN {
        out = out.chars().take(MAX_JOINED_NAME_LEN).collect::<String>() + "...";
    }
    out
}

/// Why a run stopped absorbing, for the aggregated logs.
#[derive(Default)]
struct JoinRefusals {
    uuid_backoff: usize,
    cycle_break: usize,
    cycle_stranded: usize,
}

/// Append every chain that begins exactly where another chain ends (#412).
///
/// `linearize_section` cuts one chain per (declared section name, linear run),
/// so a passage that carries on under a NEW name arrives as a separate chain
/// whose first shot's parent is the LAST shot of the previous chain. That join
/// is contiguous - the child's station 0 IS the parent's final station - so one
/// linear run holds both, and a `models::Section` polyline can express it
/// losslessly. Every other adjacency in the file attaches MID-chain, where a
/// polyline has nowhere to put it; see the module docs.
///
/// # What it refuses, and why
///
/// * **A seam that is not a station.** Both ends must be a shot the import
///   actually keeps. `convert_shots_to_shot_data` drops CLOSURE shots AND
///   excluded ones with the same filter, and either would make the child's
///   station 0 land on the previous SURVIVING station - translating the whole
///   absorbed leg, and making the output depend on the include-excluded
///   toggle. Keying on `shots.last()`/`shots.first()` rather than "last
///   surviving shot" is deliberate: a chain that merely ENDS in a closure or
///   an excluded shot never joins. The alternative - seam on the last
///   surviving shot - would re-introduce toggle dependence, because which shot
///   that is changes with the toggle.
/// * **Mismatched trip metadata.** A config carries one surveyor, one date and
///   one colour, and a run takes its FIRST shot's (see `linearize_section`), so
///   absorbing a chain surveyed by someone else on another day would silently
///   reattribute it. Only chains that agree on all three merge.
/// * **A shot UUID already in the run.** Ariane shot ids are unique per
///   SECTION, so two sections may legally repeat a `<UUID>` (#423); merging
///   them would turn that into an intra-section duplicate that
///   `ensure_unique_shot_uuids` re-mints. The run stops there and the child
///   starts a fresh run of its own.
/// * **Everything, when the file has duplicate shot ids.** `ends_at` keys on
///   the file's own ids, so with duplicates two chains can end at the same id
///   and an unrelated chain splices onto the wrong parent. The caller passes
///   the parse-time tally; any duplicate at all disables joining.
///
/// The absorbed chain's NAME does not just vanish, because every join here is
/// necessarily cross-name: a chain that continues under the SAME declared
/// section is never a candidate (it would have to attach at a chain end, and
/// the continuation walk always leaves its junction shot mid-chain). So the
/// merged run is named `"A + B"`, capped by `MAX_JOINED_NAMES`.
///
/// Deterministic: roots are taken in ascending index order, where two chains
/// start at the same end shot the LONGER one is appended (ties by lowest
/// index), and runs are emitted in the file order of their first chain - so a
/// refusal moves nothing else, and the same file always yields the same runs.
///
/// Returns the number of joins performed.
pub(super) fn join_contiguous_chains(
    sections: &mut Vec<TmlSection>,
    duplicate_shot_ids: u32,
) -> usize {
    if sections.len() < 2 {
        return 0;
    }
    if duplicate_shot_ids > 0 {
        tracing::info!(
            duplicate_shot_ids,
            "Not joining contiguous TML chains: the file reuses shot ids, so a \
             chain end cannot be matched to a unique parent"
        );
        return 0;
    }

    let n = sections.len();

    // A seam is only a station if the import keeps the shot on both sides of
    // it - same filter `convert_shots_to_shot_data` applies.
    let survives_import = |shot: &TmlShot| shot.shot_type != TmlShotType::Closure && !shot.excluded;
    let joinable_end = |s: &TmlSection| s.shots.last().is_some_and(survives_import);
    let joinable_start = |s: &TmlSection| s.shots.first().is_some_and(survives_import);
    let same_trip = |a: &TmlSection, b: &TmlSection| section_trip(a) == section_trip(b);

    // Which chain ends at a given shot id. Ids are unique here (duplicates bail
    // out above), so a shot ends at most one chain.
    let mut ends_at: HashMap<i64, usize> = HashMap::new();
    for (idx, section) in sections.iter().enumerate() {
        if !joinable_end(section) {
            continue;
        }
        if let Some(last) = section.shots.last() {
            ends_at.insert(last.id, idx);
        }
    }

    // Group the chains that could extend each parent. A chain has ONE
    // start_ariane_id, so it can appear under at most one parent.
    let mut candidates: HashMap<usize, Vec<usize>> = HashMap::new();
    for (idx, section) in sections.iter().enumerate() {
        if !joinable_start(section) {
            continue;
        }
        if let Some(&parent) = ends_at.get(&section.start_ariane_id) {
            if parent != idx && same_trip(&sections[parent], section) {
                candidates.entry(parent).or_default().push(idx);
            }
        }
    }

    // At most one child per parent: appending a second would branch the run.
    let mut child_of: Vec<Option<usize>> = vec![None; n];
    let mut parent_of: Vec<Option<usize>> = vec![None; n];
    let mut parents: Vec<usize> = candidates.keys().copied().collect();
    parents.sort_unstable();
    for parent in parents {
        let Some(&best) = candidates[&parent]
            .iter()
            .max_by_key(|&&i| (sections[i].shots.len(), std::cmp::Reverse(i)))
        else {
            continue;
        };
        child_of[parent] = Some(best);
        parent_of[best] = Some(parent);
    }

    // Resolve the runs before touching any shots, so a refusal can hand the
    // child back as a fresh root instead of stranding it at the end of the
    // output. `group_of[i]` is the root of the run that owns chain `i`.
    let mut group_of: Vec<Option<usize>> = vec![None; n];
    let mut members_of: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut refusals = JoinRefusals::default();

    while let Some(root) = (0..n).find(|&i| group_of[i].is_none() && parent_of[i].is_none()) {
        let mut members = vec![root];
        group_of[root] = Some(root);
        let mut uuids: std::collections::HashSet<&str> = sections[root]
            .shots
            .iter()
            .filter_map(|s| s.uuid.as_deref())
            .collect();
        let mut cursor = root;
        while let Some(child) = child_of[cursor] {
            // Already grouped means we walked back into this run: a cycle. Each
            // chain has at most one parent and one child, so it cannot belong
            // to a different run.
            if group_of[child].is_some() {
                refusals.cycle_break += 1;
                child_of[cursor] = None;
                break;
            }
            if sections[child]
                .shots
                .iter()
                .filter_map(|s| s.uuid.as_deref())
                .any(|u| uuids.contains(u))
            {
                refusals.uuid_backoff += 1;
                child_of[cursor] = None;
                // Hand the child back as a root so it is emitted in file order
                // rather than stranded after everything else.
                parent_of[child] = None;
                break;
            }
            uuids.extend(
                sections[child]
                    .shots
                    .iter()
                    .filter_map(|s| s.uuid.as_deref()),
            );
            group_of[child] = Some(root);
            members.push(child);
            cursor = child;
        }
        members_of.insert(root, members);
    }

    // A pure cycle has no root at all, so the scan above never reached it.
    // Emit each member unmerged rather than dropping shots on the floor.
    for idx in 0..n {
        if group_of[idx].is_none() {
            refusals.cycle_stranded += 1;
            group_of[idx] = Some(idx);
            members_of.insert(idx, vec![idx]);
        }
    }

    // Emit in the file order of each run's first chain.
    let mut roots: Vec<usize> = members_of.keys().copied().collect();
    roots.sort_unstable();

    let mut slots: Vec<Option<TmlSection>> = sections.drain(..).map(Some).collect();
    let mut out: Vec<TmlSection> = Vec::with_capacity(roots.len());
    let mut joins = 0;

    for root in roots {
        let members = members_of.remove(&root).unwrap_or_default();
        let mut iter = members.into_iter();
        let Some(first) = iter.next().and_then(|i| slots[i].take()) else {
            continue;
        };
        let mut run = first;
        let mut seen_names: std::collections::HashSet<String> =
            std::collections::HashSet::from([run.name.clone()]);
        let mut names: Vec<String> = vec![run.name.clone()];
        let mut extra_names = 0usize;

        for member in iter {
            let Some(tail) = slots[member].take() else {
                continue;
            };
            // Bounded: a HashSet probe, and at most MAX_JOINED_NAMES stored, so
            // a file with thousands of one-shot sections cannot make this
            // quadratic in the number of joins.
            if seen_names.insert(tail.name.clone()) {
                if names.len() < MAX_JOINED_NAMES {
                    names.push(tail.name.clone());
                } else {
                    extra_names += 1;
                }
            }
            run.shots.extend(tail.shots);
            run.end_ariane_id = tail.end_ariane_id;
            run.is_branch_split = run.is_branch_split || tail.is_branch_split;
            joins += 1;
        }
        run.name = joined_name(&names, extra_names);
        out.push(run);
    }

    // One line per refusal CLASS, never one per chain - the policy
    // `push_parse_anomaly_warnings` already applies to file-level anomalies.
    if refusals.uuid_backoff > 0 {
        tracing::warn!(
            chains = refusals.uuid_backoff,
            "Stopped joining TML chains that would repeat a shot UUID inside one section"
        );
    }
    if refusals.cycle_break > 0 || refusals.cycle_stranded > 0 {
        tracing::warn!(
            broken = refusals.cycle_break,
            stranded = refusals.cycle_stranded,
            "Cycle among TML chain joins; the chains involved were emitted unmerged"
        );
    }

    *sections = out;
    // `debug_assert!` still type-checks its arguments in a release build, so the
    // call has to be behind `cfg` as well as the helper it calls.
    #[cfg(debug_assertions)]
    assert!(
        first_linearity_violation(sections).is_none(),
        "join produced a non-linear run: {:?}",
        first_linearity_violation(sections)
    );
    joins
}

/// The first place an emitted run stops being a strictly linear chain, if any.
///
/// Every consumer of a `TmlSection` treats it as a polyline: shot `i` runs from
/// station `i` to station `i + 1`, so `shots[i].from_id` must be `shots[i-1].id`.
/// Nothing else in the pipeline checks it, and a join is exactly the operation
/// that could break it, so this backs a debug-only assertion after the join and
/// is asserted directly by the join tests.
#[cfg(any(debug_assertions, test))]
pub(super) fn first_linearity_violation(sections: &[TmlSection]) -> Option<String> {
    for section in sections {
        for pair in section.shots.windows(2) {
            let (previous, next) = (&pair[0], &pair[1]);
            if next.from_id != previous.id {
                return Some(format!(
                    "section {:?}: shot {} follows shot {} but its FromID is {}",
                    section.name, next.id, previous.id, next.from_id
                ));
            }
        }
    }
    None
}

/// The trip a run is attributed to: its people and date (the shot credit) and
/// its colour. Ariane writes them on EVERY shot row, so a run can change trip
/// halfway down. The credit survives per shot either way; the colour is the
/// one value a `models::Section` holds once.
type Trip<'a> = (Option<&'a SurveyCredit>, &'a str);

fn trip_of(shot: &TmlShot) -> Trip<'_> {
    (shot.credit.as_ref(), shot.color.as_str())
}

/// The trip a section was named after: its head shot's (see [`trip_head`]).
fn section_trip(section: &TmlSection) -> Option<Trip<'_>> {
    trip_head(&section.shots).map(trip_of)
}

/// Whether this shot can be the station on one side of a cut.
///
/// CLOSURE shots never import, and excluded ones import only under the
/// include-excluded toggle - which the parse does not know, because sectioning
/// has to be invariant to it. `join_contiguous_chains` refuses a seam on either
/// for exactly that reason, and a cut needs the same guarantee in the other
/// direction: the child run's station 0 is resolved through the parent's LAST
/// shot (`determine_placement` looks its `from_id` up in the filtered
/// id -> station index), so cutting after a shot the filter may drop would place
/// the child at the origin whenever the toggle drops it.
fn is_station_shot(shot: &TmlShot) -> bool {
    shot.shot_type != TmlShotType::Closure && !shot.excluded
}

/// The shot a run takes its surveyor, date and colour from: the first one that
/// could be a station OF that run.
///
/// Not simply the first shot. A run whose head is EXCLUDED was named after a
/// shot the default import does not keep, so every shot it does keep disagreed
/// with its own section and [`collapsed_trip_report`] reported a loss that had
/// not happened. Falls back to the raw first shot when a run holds no station
/// shot at all - that is what it carried before, and such a run imports as
/// nothing anyway.
fn trip_head(shots: &[TmlShot]) -> Option<&TmlShot> {
    shots
        .iter()
        .find(|shot| is_station_shot(shot))
        .or_else(|| shots.first())
}

/// Most cuts one run may be given.
///
/// Ariane writes the trip on every shot row, so a file whose rows carry a
/// per-ROW date (a data-entry timestamp, or a crafted file) would otherwise
/// mint one section per shot from a single declared name - unbounded
/// amplification of file-controlled input into project objects, each with its
/// own name, placement and undo entry.
///
/// 256 is chosen to sit far above real multi-trip surveys and far below
/// degeneracy: it is about five years of WEEKLY survey days recorded under one
/// declared section name, and the largest real file measured (14 declared
/// names, 17 runs) cuts zero. Past the cap the remaining trips stay on the last
/// part and are named on the receipt by [`collapsed_trip_report`], so the
/// bound costs reporting rather than data.
pub(super) const MAX_TRIP_CUTS_PER_RUN: usize = 256;

/// Most cuts one FILE may be given, across every run in it.
///
/// [`MAX_TRIP_CUTS_PER_RUN`] alone bounds the wrong denominator: a file
/// controls how many runs it has as well as how many trips each one holds, so
/// 170 crafted runs of 257 trips would still mint 43k sections while obeying
/// the per-run cap.
///
/// Sections are the numerator of the shot-id encode. Ids are handed out
/// sequentially from 0 (`Project::add_section`), and `Section::new` computes
/// `next_shot_id = id * FEATURE_ID_STRIDE` (100_000) as a plain `u32` multiply,
/// so the encode holds only while `section_id <= 42_948`
/// ([`crate::models::MAX_SECTION_ID`]: 42_948 x 100_000 + 99_999 =
/// 4_294_899_999 <= `u32::MAX`). Past that the multiply panics in debug and,
/// where a site saturates instead, distinct shots ALIAS onto one id - which
/// silently moves station cross-references.
///
/// 4_096 is 16 runs at the per-run cap: under a tenth of that headroom,
/// leaving ~38_850 for the file's declared names and branch splits (both of
/// which predate this cut and are bounded only by the file itself - see
/// `docs/ariane-import.md`). Cuts past the budget are not made, and the trips
/// they would have separated are named on the receipt by
/// [`collapsed_trip_report`] like any other uncut boundary.
pub(super) const MAX_TRIP_CUTS_PER_FILE: usize = 4_096;

/// How many cuts a file with `existing_runs` runs may be given.
///
/// Never more than the id space it has not already spent, so
/// `existing_runs + cut_budget(existing_runs) <= MAX_SECTIONS_IN_ID_SPACE`
/// holds for every input - which is the property that keeps the cut from
/// carrying a file over the encode boundary. Saturating, so a file that is
/// already past it gets no cuts rather than an underflow.
///
/// [`crate::models::MAX_SECTIONS_IN_ID_SPACE`] is the count of sections whose
/// shot ids all fit the encode (42_949; see [`crate::models::MAX_SECTION_ID`]
/// for the arithmetic). The cut spends at most whatever is left of it after the
/// runs the file already declared, so a file cannot use the CUT to cross the
/// boundary - only to approach it. Getting there without the cut, by declaring
/// tens of thousands of sections, is refused at the import boundary
/// (`execute_import_sections_inner`, #555) rather than here: this parser has no
/// view of what the target project has already spent.
pub(super) fn cut_budget(existing_runs: usize) -> usize {
    MAX_TRIP_CUTS_PER_FILE.min(MAX_SECTIONS_IN_ID_SPACE.saturating_sub(existing_runs))
}

/// Cut each run where the trip changes between two ADJACENT stations (#427).
///
/// A run takes its metadata from its first shot (see `linearize_section`), so
/// before this a chain surveyed by two teams over three days imported entirely
/// under the first team and the first date, and the other values were dropped
/// on the floor with nothing said. Cutting gives each trip its own run, which
/// is the only place the model has to put them - `Section` carries one of each
/// and `Shot` carries none.
///
/// This is the SAME rule `join_contiguous_chains` applies from the other side
/// ("only chains that agree on all three merge"), so the two cannot fight over
/// a seam. It runs AFTER the join all the same: see the call site for what
/// offering a cut end to the join costs.
///
/// **Adjacency is the whole rule.** A trip change across a non-station shot is
/// not cut at all: there is no seam there the import could resolve (see
/// `is_station_shot`), and cutting at the next station instead would put a
/// boundary somewhere the file does not have one. What stays uncut - including
/// everything past [`MAX_TRIP_CUTS_PER_RUN`] - is counted and reported by
/// [`collapsed_trip_report`] rather than lost quietly.
///
/// Returns the number of cuts made.
pub(super) fn split_runs_at_trip_boundaries(sections: &mut Vec<TmlSection>) -> usize {
    let mut out: Vec<TmlSection> = Vec::with_capacity(sections.len());
    let mut cuts_made = 0;
    let mut clamped_runs = 0;
    let mut budget = cut_budget(sections.len());

    for section in sections.drain(..) {
        let mut cuts = trip_cut_points(&section.shots);
        // Both caps at once: a file may not buy its way past the file-wide
        // budget by spreading the same trip churn over more runs.
        let allowed = cuts.len().min(MAX_TRIP_CUTS_PER_RUN).min(budget);
        if allowed < cuts.len() {
            clamped_runs += 1;
            cuts.truncate(allowed);
        }
        if cuts.is_empty() {
            out.push(section);
            continue;
        }
        budget -= cuts.len();
        cuts_made += cuts.len();
        out.extend(cut_run(section, &cuts));
    }

    if clamped_runs > 0 {
        // One line per file, not per run: the receipt already carries the
        // per-shot consequence.
        tracing::warn!(
            runs = clamped_runs,
            per_run_cap = MAX_TRIP_CUTS_PER_RUN,
            per_file_cap = MAX_TRIP_CUTS_PER_FILE,
            cuts_made,
            "TML runs changed trip more times than the caps allow; the rest of their \
             per-shot values were left on the last part of each"
        );
    }

    *sections = out;
    cuts_made
}

/// Indices where a new run starts, ascending.
fn trip_cut_points(shots: &[TmlShot]) -> Vec<usize> {
    (1..shots.len())
        .filter(|&i| {
            is_station_shot(&shots[i - 1])
                && is_station_shot(&shots[i])
                && trip_of(&shots[i]) != trip_of(&shots[i - 1])
        })
        .collect()
}

/// Split one run into `cuts.len() + 1` runs, each carrying its own trip.
///
/// The parent keeps the declared name; every later part is suffixed rather than
/// renamed, so the file's own name is still readable in the wizard's list. The
/// suffix is deliberately NOT "(branch N)": nothing branched, and that label
/// already means something else here.
fn cut_run(section: TmlSection, cuts: &[usize]) -> Vec<TmlSection> {
    let TmlSection {
        name,
        mut shots,
        is_branch_split,
        start_ariane_id,
        start_depth,
        ..
    } = section;

    // Split from the back so each `split_off` leaves the earlier parts intact.
    let mut parts: Vec<Vec<TmlShot>> = Vec::with_capacity(cuts.len() + 1);
    for &cut in cuts.iter().rev() {
        parts.push(shots.split_off(cut));
    }
    parts.push(shots);
    parts.reverse();

    let mut out: Vec<TmlSection> = Vec::with_capacity(parts.len());
    // The station a cut part hangs off: the previous part's last shot, whose
    // cumulative depth is what `resolve_tml_depth_in` rebases the part's first
    // `DepthIn` against. Both are the values `linearize_section` would have
    // computed had the file declared these shots under their own section name.
    let mut previous_end_depth = start_depth;

    for (index, part) in parts.into_iter().enumerate() {
        let (Some(first), Some(tail)) = (part.first(), part.last()) else {
            continue;
        };
        // Metadata from the part's first STATION shot, exactly as
        // `linearize_section` names a run; the start id is the first shot's
        // whatever it is, because that is where the part attaches.
        let head = trip_head(&part).unwrap_or(first);
        let (surveyor, date, color) = (head.explorer.clone(), head.date, head.color.clone());
        // The run is linear (`first_linearity_violation`), so a cut part's first
        // `from_id` IS the previous part's last shot id.
        let (start_ariane_id, start_depth) = if index == 0 {
            (start_ariane_id, start_depth)
        } else {
            (first.from_id, previous_end_depth)
        };
        let end_ariane_id = tail.id;
        previous_end_depth = tail.depth;

        out.push(TmlSection {
            name: if index == 0 {
                name.clone()
            } else {
                format!("{name} (trip {})", index + 1)
            },
            shots: part,
            surveyor,
            date,
            color,
            // A cut part of a branch is still that branch; a cut of the mainline
            // is not a branch, and must not pick up the wizard's branch marker.
            is_branch_split,
            start_ariane_id,
            end_ariane_id,
            start_depth,
        });
    }

    out
}

/// One line naming the per-shot trip values that a section could not carry, or
/// `None` when nothing was dropped (#427).
///
/// After [`split_runs_at_trip_boundaries`] the only shots left disagreeing with
/// their own section are the ones whose trip changed where there was no seam to
/// cut. Their surveyors, explorers and date import with them - the credit is
/// per shot - so the one value that collapses is the COLOUR, which a section
/// holds once. This is the receipt's record of that.
///
/// Counts against the toggle the import will actually run with: a shot the
/// filter drops takes its metadata out of the project with it, and reporting
/// that as a loss would name a shot the user is not importing.
pub(super) fn collapsed_trip_report(
    sections: &[TmlSection],
    include_excluded: bool,
) -> Option<String> {
    let mut shots = 0usize;
    let mut changes = 0usize;
    let mut sections_hit = 0usize;

    for section in sections {
        let mut hit = false;
        // The colour of the previous IMPORTABLE shot, while it was one this
        // section could not carry. A thousand shots on one collapsed colour are
        // one place the file lost a value, not a thousand.
        let mut collapsed_run: Option<&str> = None;
        for shot in &section.shots {
            if shot.shot_type == TmlShotType::Closure || (!include_excluded && shot.excluded) {
                continue;
            }
            if shot.color == section.color {
                collapsed_run = None;
                continue;
            }
            shots += 1;
            hit = true;
            if collapsed_run != Some(shot.color.as_str()) {
                changes += 1;
                collapsed_run = Some(shot.color.as_str());
            }
        }
        sections_hit += usize::from(hit);
    }

    if shots == 0 {
        return None;
    }

    Some(format!(
        "{shots} shot{} at {changes} trip change{} in {sections_hit} section{} recorded a \
         different colour from the section importing them; Ariane records it per shot, a \
         section holds one, and there was no station boundary to split on",
        if shots == 1 { "" } else { "s" },
        if changes == 1 { "" } else { "s" },
        if sections_hit == 1 { "" } else { "s" },
    ))
}

/// Linearize a group of shots belonging to one section name.
/// Splits into multiple sub-sections if branching is detected.
pub(super) fn linearize_section(
    section_name: &str,
    shots: &[&RawShot],
    successors: &HashMap<i64, Vec<i64>>,
    all_shots: &HashMap<i64, &RawShot>,
) -> Vec<TmlSection> {
    if shots.is_empty() {
        return Vec::new();
    }

    let shot_ids: std::collections::HashSet<i64> = shots.iter().map(|s| s.id).collect();
    let shot_by_id: HashMap<i64, &RawShot> = shots.iter().map(|s| (s.id, *s)).collect();

    // Preserve deterministic chain selection by ranking shots in encounter order.
    let order_rank: HashMap<i64, usize> =
        shots.iter().enumerate().map(|(i, s)| (s.id, i)).collect();
    let rank_key = |id: &i64| (order_rank.get(id).copied().unwrap_or(usize::MAX), *id);

    // Successors constrained to this section, sorted deterministically.
    let mut in_section_successors: HashMap<i64, Vec<i64>> = HashMap::new();
    for shot in shots {
        let mut succs: Vec<i64> = successors
            .get(&shot.id)
            .into_iter()
            .flatten()
            .copied()
            .filter(|id| shot_ids.contains(id))
            .collect();
        succs.sort_by_key(&rank_key);
        succs.dedup();
        in_section_successors.insert(shot.id, succs);
    }

    let downstream = downstream_lengths(shots, &shot_by_id, &in_section_successors);

    // Find chain roots: shots whose from_id is not in this section's shot set.
    //
    // Ordered by downstream length, longest first, because the FIRST chain
    // emitted keeps the section's own name and every later one becomes
    // "(branch N)". Walking roots in file order instead let an entrance stub
    // written before the mainline take the section's name and demote the real
    // passage - the same file-order bias `choose_continuation` removes at
    // junctions, one level up. Ties fall back to encounter order then id, so
    // the result is still fixed for a given file.
    let mut roots: Vec<i64> = shots
        .iter()
        .filter(|s| !shot_ids.contains(&s.from_id))
        .map(|s| s.id)
        .collect();
    roots.sort_by(|a, b| {
        let length_of = |id: &i64| downstream.get(id).copied().unwrap_or(0.0);
        length_of(b)
            .total_cmp(&length_of(a))
            .then_with(|| rank_key(a).cmp(&rank_key(b)))
    });

    let needs_split = roots.len() > 1 || has_branching_within(shots, successors);
    let mut visited: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut pending_starts = std::collections::VecDeque::new();
    let mut chains: Vec<Vec<i64>> = Vec::new();

    // Walk natural roots first.
    for root_id in roots {
        let chain = collect_linear_chain(
            root_id,
            &shot_ids,
            &in_section_successors,
            &downstream,
            &shot_by_id,
            &mut visited,
            &mut pending_starts,
        );
        if !chain.is_empty() {
            chains.push(chain);
        }
    }

    // Then walk branch starts discovered while traversing roots.
    while let Some(start_id) = pending_starts.pop_front() {
        let chain = collect_linear_chain(
            start_id,
            &shot_ids,
            &in_section_successors,
            &downstream,
            &shot_by_id,
            &mut visited,
            &mut pending_starts,
        );
        if !chain.is_empty() {
            chains.push(chain);
        }
    }

    // Guard pass for malformed/disconnected graphs:
    // backtrack through in-section parents to find an upstream start.
    let mut unvisited_ids: Vec<i64> = shots
        .iter()
        .map(|s| s.id)
        .filter(|id| !visited.contains(id))
        .collect();
    unvisited_ids.sort_by_key(&rank_key);

    for shot_id in unvisited_ids {
        if visited.contains(&shot_id) {
            continue;
        }

        let mut start = shot_id;
        let mut seen_backtrack = std::collections::HashSet::new();
        loop {
            if !seen_backtrack.insert(start) {
                tracing::warn!(
                    section = section_name,
                    shot_id = start,
                    "Cycle detected while finding branch start; using cycle member as start"
                );
                break;
            }

            let Some(current) = shot_by_id.get(&start) else {
                tracing::warn!(
                    section = section_name,
                    shot_id = start,
                    "Shot missing while finding branch start; using current node as start"
                );
                break;
            };

            let parent_id = current.from_id;
            if !shot_ids.contains(&parent_id) || visited.contains(&parent_id) {
                break;
            }
            start = parent_id;
        }

        let chain = collect_linear_chain(
            start,
            &shot_ids,
            &in_section_successors,
            &downstream,
            &shot_by_id,
            &mut visited,
            &mut pending_starts,
        );
        if !chain.is_empty() {
            chains.push(chain);
        }

        while let Some(extra_start) = pending_starts.pop_front() {
            let extra_chain = collect_linear_chain(
                extra_start,
                &shot_ids,
                &in_section_successors,
                &downstream,
                &shot_by_id,
                &mut visited,
                &mut pending_starts,
            );
            if !extra_chain.is_empty() {
                chains.push(extra_chain);
            }
        }
    }

    // Last-resort guard: ensure every shot is emitted exactly once.
    let mut still_unvisited: Vec<i64> = shots
        .iter()
        .map(|s| s.id)
        .filter(|id| !visited.contains(id))
        .collect();
    still_unvisited.sort_by_key(&rank_key);
    for shot_id in still_unvisited {
        let chain = collect_linear_chain(
            shot_id,
            &shot_ids,
            &in_section_successors,
            &downstream,
            &shot_by_id,
            &mut visited,
            &mut pending_starts,
        );
        if !chain.is_empty() {
            tracing::warn!(
                section = section_name,
                shot_id,
                "Recovered unvisited shot via final guard pass"
            );
            chains.push(chain);
        }
    }

    let mut result = Vec::new();
    for chain_ids in chains {
        let mut chain = Vec::with_capacity(chain_ids.len());
        for shot_id in chain_ids {
            if let Some(shot) = shot_by_id.get(&shot_id) {
                chain.push((*shot).clone());
            } else {
                tracing::warn!(
                    section = section_name,
                    shot_id,
                    "Skipping missing shot while building linearized section"
                );
            }
        }

        if chain.is_empty() {
            continue;
        }

        // The first chain is the mainline (the continuation rule in
        // `choose_continuation` keeps walking it through every junction), so
        // it keeps the section's own name; only the deferred side chains get
        // "(branch N)" suffixes, numbered as if the mainline were branch 1.
        let has_prior_chains = !result.is_empty();
        let branch_number = result.len() + 1;
        let name = if has_prior_chains {
            format!("{section_name} (branch {branch_number})")
        } else {
            section_name.to_string()
        };
        let is_branch_split = needs_split || has_prior_chains;

        let first_shot = &chain[0];
        let last_shot = &chain[chain.len() - 1];

        let tml_shots: Vec<TmlShot> = chain
            .iter()
            .map(|s| TmlShot {
                id: s.id,
                uuid: s.uuid.clone(),
                from_id: s.from_id,
                shot_type: s.shot_type,
                azimuth: s.azimuth,
                length: s.length,
                depth: s.depth,
                depth_in: s.depth_in,
                inclination: s.inclination,
                left: s.left,
                right: s.right,
                up: s.up,
                down: s.down,
                comment: s.comment.clone(),
                explorer: super::people::people_line(&s.explorers, &s.surveyors),
                date: s.date,
                credit: Some(SurveyCredit {
                    surveyors: s.surveyors.clone(),
                    explorers: s.explorers.clone(),
                    date: s.date,
                })
                .filter(|c| !c.is_empty()),
                color: s.color.clone(),
                excluded: s.excluded,
                latitude: s.latitude,
                longitude: s.longitude,
                closure_to_id: s.closure_to_id,
                section: s.section.clone(),
            })
            .collect();

        // Look up the parent shot's cumulative depth for correct rebasing.
        // If from_id points to a shot (START or REAL/VIRTUAL), use its depth.
        let start_depth = all_shots
            .get(&first_shot.from_id)
            .map_or(0.0, |parent| parent.depth);

        // The trip comes from the first shot that can be a STATION of this run,
        // not simply the first shot: see `trip_head`.
        let head = trip_head(&tml_shots);
        let surveyor = head.and_then(|shot| shot.explorer.clone());
        let date = head.and_then(|shot| shot.date);
        let color = head.map(|shot| shot.color.clone()).unwrap_or_default();

        result.push(TmlSection {
            name,
            start_ariane_id: first_shot.from_id,
            end_ariane_id: last_shot.id,
            shots: tml_shots,
            surveyor,
            date,
            color,
            is_branch_split,
            start_depth,
        });
    }

    result
}

/// Walk a single linear chain, queueing extra branch starts when a node has
/// multiple unvisited successors within the same section.
fn collect_linear_chain(
    start_id: i64,
    shot_ids: &std::collections::HashSet<i64>,
    in_section_successors: &HashMap<i64, Vec<i64>>,
    downstream: &HashMap<i64, f64>,
    shot_by_id: &HashMap<i64, &RawShot>,
    visited: &mut std::collections::HashSet<i64>,
    pending_starts: &mut std::collections::VecDeque<i64>,
) -> Vec<i64> {
    if !shot_ids.contains(&start_id) || visited.contains(&start_id) {
        return Vec::new();
    }

    let mut chain = Vec::new();
    let mut current = start_id;

    loop {
        if !visited.insert(current) {
            break;
        }
        chain.push(current);

        let Some(successors) = in_section_successors.get(&current) else {
            break;
        };

        let unvisited_successors = successors
            .iter()
            .copied()
            .filter(|id| !visited.contains(id))
            .collect::<Vec<_>>();

        if unvisited_successors.is_empty() {
            break;
        }

        let next_id = choose_continuation(current, &unvisited_successors, downstream, shot_by_id);
        for extra in unvisited_successors {
            if extra != next_id {
                pending_starts.push_back(extra);
            }
        }

        current = next_id;
    }

    chain
}

/// Fraction of the longest candidate's downstream length a rival must reach
/// to still count as "comparable", handing the decision to azimuth continuity.
const COMPARABLE_LENGTH_RATIO: f64 = 0.8;

/// Decide which successor continues the current chain at a branch point (#412).
///
/// The continuation should be the mainline; every other successor defers into
/// its own "(branch N)" section. The rule is elimination, THEN azimuth - length
/// never decides on its own:
/// 1. Eliminate: a candidate below `COMPARABLE_LENGTH_RATIO` (80%) of the
///    longest downstream length is out. Being longest is not itself a win, so
///    a candidate 1% longer than its rival does not take the junction on that
///    alone.
/// 2. Choose among the survivors by azimuth continuity: smallest angular
///    distance from the current shot's azimuth - a mainline tends to hold its
///    heading through a junction, while a side passage leaves at an angle. A
///    survivor that is much shorter than the longest cannot reach this step,
///    which is what keeps a stub from stealing a straight-ahead junction.
/// 3. Break ties on ascending shot id.
///
/// A CLOSURE successor is considered only when there is no real one. Closures
/// are loop markers the import drops unconditionally, so a closure that won a
/// junction left the run stopping dead there while the real continuation was
/// demoted to a branch. When a closure IS the only successor it still
/// continues the chain, which keeps it inline instead of spawning a chain that
/// would be dropped as empty at import.
///
/// Deterministic by construction: `candidates` arrives in the section's
/// sorted encounter order (never HashMap iteration order), the partition below
/// preserves that order, the downstream lengths are fixed-order IEEE 754 sums
/// of parsed values (see `downstream_lengths`), and step 2 compares with
/// `f64::total_cmp`, which is a total order on every run and platform.
fn choose_continuation(
    current_id: i64,
    candidates: &[i64],
    downstream: &HashMap<i64, f64>,
    shot_by_id: &HashMap<i64, &RawShot>,
) -> i64 {
    if candidates.len() == 1 {
        return candidates[0];
    }

    let is_closure = |id: i64| {
        shot_by_id
            .get(&id)
            .is_some_and(|s| s.shot_type == TmlShotType::Closure)
    };
    let real: Vec<i64> = candidates
        .iter()
        .copied()
        .filter(|&id| !is_closure(id))
        .collect();
    // Real survey always outranks a loop marker; fall back to the closures
    // only when the junction offers nothing else.
    let candidates: &[i64] = if real.is_empty() { candidates } else { &real };
    if candidates.len() == 1 {
        return candidates[0];
    }

    let length_of = |id: i64| downstream.get(&id).copied().unwrap_or(0.0);
    let longest = candidates
        .iter()
        .map(|&id| length_of(id))
        .fold(0.0_f64, f64::max);
    let threshold = longest * COMPARABLE_LENGTH_RATIO;

    let current_azimuth = shot_by_id.get(&current_id).map(|s| s.azimuth);
    let turn_of = |id: i64| -> f64 {
        match (current_azimuth, shot_by_id.get(&id)) {
            (Some(azimuth_in), Some(shot)) => {
                let diff = (shot.azimuth - azimuth_in).rem_euclid(360.0);
                diff.min(360.0 - diff)
            }
            _ => f64::MAX,
        }
    };

    let mut best: Option<(i64, f64)> = None;
    for &candidate in candidates {
        if length_of(candidate) < threshold {
            continue;
        }
        // A turn that is not a real angle sorts LAST rather than winning by
        // default. With `partial_cmp` a NaN first candidate became `best` and
        // then nothing could beat it, because every later comparison returned
        // `None` and fell through to "not better" - an accidental win for the
        // one candidate we know least about.
        let turn = turn_of(candidate);
        let turn = if turn.is_nan() { f64::INFINITY } else { turn };
        let better = match best {
            None => true,
            Some((best_id, best_turn)) => match turn.total_cmp(&best_turn) {
                std::cmp::Ordering::Less => true,
                std::cmp::Ordering::Equal => candidate < best_id,
                std::cmp::Ordering::Greater => false,
            },
        };
        if better {
            best = Some((candidate, turn));
        }
    }
    best.map_or(candidates[0], |(id, _)| id)
}

/// Total downstream traverse length (own length plus all in-section
/// descendants) for every shot in the section.
///
/// Memoized via an iterative post-order walk, so the whole section costs
/// O(V + E) - one linear pass even on 10^5-shot files, never a per-branch
/// BFS. Children are summed in the pre-sorted successor order, so the f64
/// accumulation order is fixed and the result is identical across runs and
/// platforms (IEEE 754 arithmetic is deterministic given a fixed order).
///
/// EXCLUDED shots count toward the length: segmentation must reflect the
/// topology the file declares, and the excluded-shots import toggle (applied
/// downstream of section building) must not change how sections split.
///
/// CLOSURE shots contribute 0. That is not the same call: their removal is
/// unconditional, not toggle-controlled, so zero-weighting them cannot make
/// splitting depend on an import option. Counting them made a junction's
/// ancestor look longer than the survey it actually holds, and could hand the
/// mainline to an edge that then disappeared.
///
/// This sums over PATHS, not over distinct shots. Where the section's graph
/// rejoins - a diamond, i.e. a loop closed inside one section - the shared
/// tail is added once for every path into it, so in `A -> {B, C} -> D` the
/// length of D lands in A's total twice. The result is therefore an upper
/// bound on physical passage length, not a measurement of it.
///
/// That is deliberate rather than merely tolerated: the only consumer,
/// `choose_continuation`, ranks ONE junction's successors against each other,
/// and a shared tail contributes to every rival at that junction, so it
/// cancels out of the comparison. Making it exact would need a reachable-set
/// union per node, which is precisely the cost the O(V + E) memoization
/// exists to avoid. A back-edge in a malformed cycle contributes 0 instead,
/// because that node is still in progress and has no memoized length yet.
fn downstream_lengths(
    shots: &[&RawShot],
    shot_by_id: &HashMap<i64, &RawShot>,
    in_section_successors: &HashMap<i64, Vec<i64>>,
) -> HashMap<i64, f64> {
    let empty: Vec<i64> = Vec::new();
    let mut lengths: HashMap<i64, f64> = HashMap::with_capacity(shots.len());
    let mut in_progress: std::collections::HashSet<i64> = std::collections::HashSet::new();

    for shot in shots {
        if lengths.contains_key(&shot.id) {
            continue;
        }
        in_progress.insert(shot.id);
        let mut stack: Vec<(i64, usize)> = vec![(shot.id, 0)];
        while let Some(&(id, next_child)) = stack.last() {
            let succs = in_section_successors.get(&id).unwrap_or(&empty);
            if let Some(&child) = succs.get(next_child) {
                if let Some(frame) = stack.last_mut() {
                    frame.1 = next_child + 1;
                }
                if !lengths.contains_key(&child) && in_progress.insert(child) {
                    stack.push((child, 0));
                }
            } else {
                // A CLOSURE shot is a loop marker, not passage: it is filtered
                // out of the import unconditionally, so its tape reading must
                // not make an ancestor look longer than the survey it holds.
                // (Our own exporter writes the vertical gap between the two
                // nodes it ties, so this is routinely non-zero.)
                let own = shot_by_id.get(&id).map_or(0.0, |s| {
                    if s.shot_type == TmlShotType::Closure {
                        0.0
                    } else {
                        s.length
                    }
                });
                let total = own
                    + succs
                        .iter()
                        .map(|c| lengths.get(c).copied().unwrap_or(0.0))
                        .sum::<f64>();
                lengths.insert(id, total);
                in_progress.remove(&id);
                stack.pop();
            }
        }
    }

    lengths
}

/// Check if any shot in the group has multiple successors within the same group.
fn has_branching_within(shots: &[&RawShot], successors: &HashMap<i64, Vec<i64>>) -> bool {
    let shot_ids: std::collections::HashSet<i64> = shots.iter().map(|s| s.id).collect();
    for shot in shots {
        if let Some(succs) = successors.get(&shot.id) {
            let in_section = succs.iter().filter(|id| shot_ids.contains(id)).count();
            if in_section > 1 {
                return true;
            }
        }
    }
    false
}
