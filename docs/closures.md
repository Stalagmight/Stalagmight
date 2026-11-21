# Loop Closures (Current Behavior)

How loop closures work on the station-graph model
([survey-model.md](survey-model.md), plan sections 3.6, 3.7 and 5.5). A closure
records that two stations are the same physical point reached by two survey
paths; it stores only its two ends (stations, each with a `via` line hint),
its id and its strategy. Its misclosure figures, whether it is aligned, and
what it does in the solve are derived every time.

## Raw Data View vs Closures Computed View (non-destructive display mode)

Closure adjustment is a **non-destructive, toggleable display mode**, not a destructive
command. The measured survey geometry (raw shots) is always the source of truth and is
**never rewritten** by adjustment.

(Terminology: below, "adjusted" / "closure-adjusted" is shorthand for the Closures Computed
View, and "measured" / "raw" for the Raw Data View. The technical prose keeps those words
because they name the stored enum values - `measured` / `closureAdjusted` - and the solve's
own vocabulary; only the two UI labels were renamed.)

- The **Display** dropdown has a *Geometry* control with two modes: **Raw Data View** (raw,
  open padlock) and **Closures Computed View** (loop-closure-corrected, closed padlock). The
  padlock glyphs render only at syntactic-highlighting level `icons` and above; at
  `toolbar_only` / `off` the two pills are label-only. The mode is persisted per the display
  settings ("remember last used"); default is the Raw Data View. (The stored enum values stay
  `measured` / `closureAdjusted`.) The adjusted option is disabled when
  the project has neither a closure eligible for the solve (`Batch` / `LeastSquaresWeighted`,
  and a migrated `Tie`, which the wire sends as an aligned `Batch`) nor a GPS fix (a later
  zero datum in an already placed piece, #741, which the solve pulls the survey toward);
  `Move` closures are already coincident, so a project with only those shows the option
  disabled (where the view does adjust, it holds a snapped one on one point, see
  "Snapped closures" below). One helper decides this everywhere - the Display option, the auto-revert
  to the Raw Data View, and whether the adjusted geometry is fetched at all:
  `hasAdjustmentInputs` (`src/utils/closures.ts`).
- Closure-adjusted positions are a pure function of the model
  (`Project::adjusted_positions`, `survey/adjust.rs`): the solve reads the survey and
  writes nothing back. Because it always starts from the measured data, adding or removing
  closures re-solves accurately instead of compounding on adjusted geometry.
- The adjusted view is **read-only**: geometry-mutating tools are disabled there (edits are
  made in the Raw Data View). Selection, inspection, and measuring remain available.
- The adjusted set is fetched lazily via the `get_adjusted_stations` IPC command and
  memoised per project revision and closure-adjustment config
  (`AppState::adjusted_positions`): every edit, undo, redo or load bumps the revision, and
  the solve runs on a clone outside the project lock. The map shows a
  "computing closure…" indicator on the first switch for large surveys.
- Headline stats (penetration distance, length, depth) are always computed from **measured**
  geometry.

There is no "Adjust Closures" dialog or `adjust_loop_closures` command: the old
destructive apply path was removed before 0.5.0. A project whose shots such an apply
rewrote cannot recover its original measurements; it keeps those closures as legacy
aligned closures, which the migration turns into `Tie`s (below), so it opens looking
identical and is never rewritten again.

## Strategy Summary

| Strategy | When used | What it does |
|----------|-----------|--------------|
| `Batch` | Create loop closure | Records the closure only; eligible for the Closures Computed solve. |
| `LeastSquaresWeighted` | Closures Computed View | Eligible like `Batch` (the solve is the weighted least squares either way). |
| `Move` | Snap-to-merge workflow | Closure created during a move. While aligned (ends within 1 cm), the solve ties its two stations as one point, like a junction (#736); it is not eligible on its own. |
| `Tie` | Migrated files only | The legacy rigid tie: a 0.6.0 merge the migration could not make a junction (one that closed a cycle, or joined two stations of one line), or a legacy aligned closure of a non-eligible strategy. Always aligned, eligible, never created by a command. On the wire it is an aligned `Batch`, so the frontend's types and eligibility table are unchanged (W3). |
| `ProportionalByLength`, `EqualDistribution` | Deferred | Legacy create-time distribution strategies; refused at creation and not exposed in the UI (only revisited if users request it). |

Removed strategies (legacy strings still read): `Sequential` maps to
`LeastSquaresWeighted`; `KeepRaw` and `ManualAdjustment` map to `Batch`
(`ClosureStrategy::from_str_lossy` and serde aliases). An unknown strategy in a
version 2 file opens the project read-only with the fail-closed fallback `batch`
(file format F7).

## Create Loop Closure

`create_closure_at(slot, slot, strategy)` (`survey/ops_closures.rs`) refuses, in
this order and in 0.6.0's words:

- an end that does not exist ("One or both stations do not exist");
- two ends that are one station: two slots of one junction, or the same slot;
- ends in different pieces, **unless** both pieces are placed by a zero datum
  (a closure may tie two placed pieces; it may not be the only connection of a
  free one, which would be a merge);
- a second closure between the same two stations;
- a closure that would join the two ends of one shot (#737): directly (A:1 to
  A:2), or because other closures and snaps already tie each station to one
  end of the same shot - *"This closure would join the two ends of one shot
  (A:1 to A:2) and squeeze that shot to zero length. Pick two stations that
  are not the ends of a single shot."*. It is judged against the knots the
  solve uses (below). A closure between two stations other closures already
  tie together is still created: harmless to the solve, and refusing it would
  drop such closures from imported files;
- the deferred strategies, and `Tie`.

The closure dialog shows the #737 refusal before Create:
`check_merge_connectivity` returns it in `ConnectivityResult.closure_refusal`,
the dialog prints it and disables Create (`closure-refusal`). The backend stays
the authority. Creating or removing a closure moves no station and never
orphans one, so neither runs the sweep.

## Metrics are derived

`closure_metrics` / `closures_metrics` (`survey/metrics.rs`) recompute every
figure from the raw layout, every time it is asked; nothing is stored:

- **Misclosure** (`misclosure_distance`, `horizontal_misclosure`,
  `vertical_misclosure`): between the two ends' raw positions, each end's depth
  read on its `via` line.
- **Path length**: the shortest path **along shots** between the two stations.
  Junctions are free (one station); closures are not traversable. The shot
  graph is a forest, so this is the unique tree path, each shot weighing its
  length. Two ends in different pieces (two datum-held pieces) have no path:
  `path_length` is then the summed shot length of the first end's piece, as
  0.6.0's fallback did, so the error percentage is a real figure (D14).
- **Error percentage** = misclosure / path length * 100 (0 when the path
  length is 0).

Declared change (plan 12.2): 0.6.0 stored these figures when the closure was
created, counted earlier closures and merges as zero-length edges in the path
length, and never refreshed them. A closure's figures now follow every edit,
and a closure in a survey with other loops reports the longer, shots-only
path: values on old files can change. A created closure reports its figures on
the slots that were picked (`closure_metrics_via`, slot depth on the `via`
line), which is what its wire entry then shows.

## When a closure becomes a merge (promotion)

A closure exists only for a TRUE loop. The **orphan-closure sweep**
(`sweep_orphan_closures`, `survey/sweep.rs`; plan 5.5) runs after the topology
edits that can break one - Merge, Unmerge / Detach, Remove Zero Datum and every
delete - and never on load. One pass over the closures in list order, so a
later closure sees what an earlier one did:

- **A closure whose two ends are one station is removed** (#738). It closes no
  loop. This happens when a merge folds a closure's two ends into one station
  (merging two placed pieces at a closure's two ends, #741), or when a
  promotion merges a point another closure also ties to (two closures that
  recorded one loop twice).
- **A closure that is the sole connection between its ends' pieces is
  promoted**: the two stations are merged (plan 5.1), the movable piece moving
  onto the other (end `a`'s piece moves unless it is placed and `b`'s is not;
  a delete names the end that moves when it left that end's junction). Between
  two placed pieces a promotion would double-anchor, so the closure is kept:
  a closure may tie two placed pieces.

A delete that splits a line inside a loop (Delete Shot + Split, a range split)
can leave the loop's closure the only connection between the two halves'
pieces: the closure is promoted, it becomes a junction, and the stranded piece
moves onto it by the loop's misclosure. 0.6.0's Delete Shot + Split did the
same (its sweep turned the closure into a merge, issue #8; the port is checked
slot for slot against the frozen 0.6.0 model in
`migrate_tests::delete_shot_and_split_welds_a_stranded_loop_closure_as_0_6_0_did`).
The Delete Shot dialog and the Delete & Split dialog say so before the edit,
from a dry run of the split in the delete preview (`DeleteShotPreview::split_weld`,
on the wire `DeleteShotImpact.split_closure_to_merge` and `split_max_move_m`):
"the loop closure A / 3 - B / 5 becomes a junction; the stations past it move
by up to 2.2 m" (review finding UX-5).

A promotion only joins pieces that were apart, so a closure the pass kept as a
loop can never become one station later in the pass, and one pass settles
everything. Each promotion runs the anchor algorithm on its own, so the next
closure is judged on the geometry the previous one left.

Inside a command the sweep is part of that command's undo step: one Undo brings
the closure back. Each removal is reported in `CommandResult.warnings` as
`CLOSURE_REMOVED_MERGED_ENDS`, named by the stations it had BEFORE the edit
(`station_text` on its `via` lines, looked up by closure id), which the
frontend shows in the closure-removal toast: "Removed loop closure A / 0 to B /
3: the merge made its two stations one point." ("...: another loop closure
became a merge, which made its two stations one point." after an unmerge or a
datum removal). Promotions are logged, not reported.

A file cannot hold a closure from a station to itself (invariant 7), so there is
nothing to clean up on open; a version 2 file that breaks it is refused with the
repair offer.

## Closures across a Split Line Here or a Combine Lines

- **Split Line Here** makes the split station a junction of the two halves:
  the halves stay connected, so the topology does not change, nothing moves,
  and no closure becomes an orphan. A closure on the split station stays on
  the one station; a closure across the cut stays a loop closure. The split
  dialog has nothing to warn about (0.6.0 promoted a closure that was the
  halves' only tie and moved one half by its misclosure).
- **Combine Lines** (`survey/edit/combine.rs`; the full rules are its module
  doc and [SPEC 7.11](SPEC.md#711-splitting-and-combining-lines)). Two line ends
  tied by a closure are a join: one side moves by the misclosure and the
  closure is dropped. When the two join stations were already connected,
  the moving line is detached from the junction on the path between them
  nearest the moving side and a `Batch` closure ties the two stations of that
  torn junction, carrying the loop's misclosure; with zero datums on both
  sides, the moving line is detached from the junctions towards the datum
  sides the same way. Snaps the move tears, and snaps across the two sides,
  become `Batch`; a station both lines pass besides the join is torn into a
  `Batch` closure (the combined line may not pass a station twice). Then:
  - closures the combine leaves across one shot are removed (#746) and
    reported as `CLOSURE_REMOVED_ACROSS_ONE_SHOT`; one that already joined the
    two ends of one shot is left alone;
  - of two closures left on the same two stations, the one the solve takes
    stays (#749), and the dialog lists in Details what is replaced;
  - the combine keeps the Closures Computed View still: the moving side is
    the other line's, unless a datum holds it and not the kept side, or (no
    datum deciding) it holds the station the view is held at
    (`Project::solve_hold`).

  Nothing the combine makes is a sole bridge, so nothing it leaves is
  promoted later. The dialog and the command read the same `CombineReport`,
  run on a copy.

## Deferred: Distribution at Create Time (Proportional/Equal)

`ProportionalByLength` and `EqualDistribution` distributed a closure's
misclosure into the shots at creation, rewriting them. They are refused at
creation and not offered in the UI; the code that did it went with the old
model. A legacy closure with one of these strategies keeps it (read, not
re-run); an aligned one was migrated to a `Tie`.

## Closures Computed Solve (LeastSquaresWeighted)

`Project::adjusted_positions(&config)` (`survey/adjust.rs`, linear algebra in
`survey/lsq.rs`; plan 3.7) is 0.6.0's weighted least squares on stations
instead of slots:

- **Solve components**: the shot pieces joined by any closure. A component is
  solved when it holds an eligible closure the across-one-shot rule does not
  skip, or a GPS fix; otherwise it keeps its raw layout.
- **Unknowns**: one per constraint group - stations tied by the closures the
  solve ties (eligible ones, `Tie`s and aligned snaps, minus the skip set). A
  junction is one station, so 0.6.0's merge groups are built in. Groups are
  numbered by their lowest Station ID.
- **Observations**: every shot between two groups, with today's per-axis weights
  from `base_sigma_length_m * quality` and `base_sigma_azimuth_deg * quality`
  (`base_sigma_depth_m` is read but the solve is horizontal-only), each axis's
  sigma floored at `sigma_len x sigma_az` (below); soft GPS fixes add a
  diagonal term (below).
- **Held**: each placement datum's group at its raw position (two placement
  datums in one group fail the component), hard GPS fixes at their GPS, else
  the **gauge**: the component's `Placement` station at its raw position. The
  gauge replaces 0.6.0's held-gauge bookkeeping: it is stable across every edit
  that does not delete that station.
- **Spurs** (leaf-pruned; datums, fixes and the gauge pinned) keep their
  measured shots.
- Depths are the raw ones, except that the ends of every closure a successful
  solve adjusted share their tie group's mean depth (D12).
- Solved per axis for each group's correction from its raw position (exact
  sparse elimination, a conjugate gradient on whatever dense core is left, and
  two refinement passes; see [performance.md](performance.md)), never for
  absolute coordinates: with weights 1e14 apart (a zero-length connector next
  to a 100 km shot) the absolute solve's rounding moved a part of the survey
  held by one weak shot hundreds of metres (plan Deviations D21). Everything
  iterates in sorted order, so reordering lines or closures changes no bit;
  results match 0.6.0's within 1e-6 m (D13), except where 0.6.0's own solve
  was degenerate or imprecise (the sigma floor below, and kilometre-scale
  shots; plan Deviations G9, G11).

**The sigma floor (G9).** A shot's per-axis variance is first order:
`(sin(az) * sigma_len)^2 + (len * cos(az) * sigma_az)^2` across x (cos and sin
swapped for y). On a grid axis the axis across the shot keeps only the
length-times-azimuth term, which vanishes as the shot gets short: a
zero-length "0 0 0" connector at azimuth 0 had no weight on x at all, so the
solve left the loop's misclosure in it and the connector opened by the whole
misclosure; a 1 mm shot weighed 3e9 against tens to hundreds, and the
ill-conditioned solve could move stations by kilometres. The first-order
formula drops the product of the length and azimuth errors, which has
variance `sigma_len^2 * sigma_az^2` on the axis across the shot, so each
axis's sigma is now at least `sigma_len * sigma_az` (8.7e-4 m with the
defaults; `ShotSigmas` in `survey/adjust.rs`). It binds only for shots shorter
than `sigma_len` (5 cm) within about `sigma_az` (1 degree) of a grid axis;
every other shot keeps 0.6.0's weight bit for bit. A file such a shot sits in
can show its Computed View differently from 0.6.0 there, where 0.6.0 was
wrong; the migration reports it as `LegacyWeightsDegenerate`.

**Across one shot (#737).** `closures_across_one_shot` (`survey/closures.rs`)
builds knots with a union-find over stations, taking every closure that ties
in the solve (eligible closures, `Tie`s and aligned snaps) in list order: a
closure whose union would put both ends of some shot into one knot is skipped,
reported with that shot, and never united, so it changes nothing a later
closure sees. A `Tie` goes through the same check (D11): a demoted same-line
merge across one shot is skipped and reported instead of failing its
component.

### Snapped closures (#736)

A *snap* is an aligned `Move` closure: the one Move Station makes when it drops
a station onto one it is already connected to. The Raw Data View holds it rigid
(depth co-location, the move tools' co-located sets, frozen shots), and the
solve ties its two stations as ONE point, exactly as it ties a junction: they
share a constraint group, and the snap is an attachment point for a line that
is all spur.

- **A snap is nothing to solve.** It never makes a component adjusted: a
  component whose only closures are snaps is shown as measured.
- **Its gap** (under 1 cm, or it is not aligned) is closed like any misclosure,
  by the shots around the loop.
- **Across one shot.** A snap whose two knots hold the two ends of one shot is
  skipped and reported like any closure across one shot; Create Loop Closure
  refuses a `Batch` closure that, through a snap, would squeeze a shot.
- **Datums and fixes.** A placement datum on either end holds both. Two
  placement datums tied into one group through a snap fail the component with
  "Multiple placement zero datums in a single constraint group". A GPS fix on
  either end pulls (or, hard, holds) both.

### GPS fixes

A GPS fix pulls its station toward `gps_to_local(fix.gps, frame.gps)`, the GPS position in the
frame's local metres. It is the observation `p = gps` on each horizontal axis with weight
`1 / accuracy^2` (read as a per-axis 1-sigma; the default is 0, a hard fix, below):
one diagonal term and one right-hand-side term on its group's unknown. Shots use the same units,
so the two compare directly. A shot's per-axis variance is
`(sin(az) * sigma_len)^2 + (len * cos(az) * sigma_az)^2` across x (cos and sin swapped for y),
with `sigma_len = base_sigma_length_m * quality` and `sigma_az = base_sigma_azimuth_deg * quality`
(at least `(sigma_len * sigma_az)^2`, the floor above), and variances add along a traverse.

What a soft fix means at the default config (0.05 m, 1 deg), taking an accuracy of 5 m as the
example: a level 500 m traverse of 10 m shots is trusted to about 0.35 m along its length and
1.23 m across it, far tighter than a 5 m GPS. A fix 40 m off across that traverse moves its
station only about 2.3 m toward the GPS; the rest of the gap stays visible. That is why the
default is a hard fix: only a hard fix, or larger base sigmas, puts the station on its GPS.

- **Accuracy 0 is a hard fix** (anything under 1 mm counts), and it is the default: its group is
  fixed AT the GPS position, the way a placement datum fixes its group at its raw one.
- **A fixed group ignores a fix.** A fix in the placement datum's own group is the same point
  and cannot hold both positions; the placement wins. Between two hard fixes in one group, the
  first in list order wins.
- **Fixes count as pinned for the spur prune.** A fix at the end of a passage is a second
  observation of it, so its shots take the solved positions instead of being skipped as a spur.
- **Horizontal only.** Depths never move, and the fix's elevation (`depth_reference`) is never
  read.
- **No closure needed.** A component whose only reason to adjust is a GPS fix is solved the
  same way.

### Hand moves (legacy, #759)

A hand move shows ONE station on ONE line away from where its shots put it
(`Line::hand_moves`). Only a migrated file carries one: no command creates
one, and Move Station folds a station's hand moves into its shots. The Raw
Data View shows a slot at its chain position plus its line's hand move there.
The Closures Computed View keeps that meaning: it adjusts the measured shots,
never a hand move, observing each shot as its measured vector plus the
difference of its two stations' hand moves on its line, so a hand move on the
held station does not shift the whole adjusted survey and a hand move on a
loop station does not vanish into its shots.

### Failures are reported

A component whose solve fails (for example a closure that ties two placement
zero datums into one group, which creation allows because both pieces are
placed) keeps its measured geometry and is reported:

- the `get_adjustment_report` IPC returns `{ skipped_closures, failures }`: the
  closures skipped above and the failures of the memoised solve of the CURRENT
  revision only (never solving to answer), each with a plain-language reason;
- the Warnings tab lists one `CLOSURE_ACROSS_ONE_SHOT` **error** per skipped
  closure, on the shot it spans (Resolve: **Remove this loop closure**, or go
  to its station). It is a pure function of the saved data, so it gates like
  any error and cannot be ignored;
- it also lists one `CLOSURE_ADJUSTMENT_FAILED` **warning** per failed part,
  read from the same memoised solve: the warnings pass never runs the solve, so
  the entry appears once the Closures Computed View, Plan or Export has solved
  the current revision, and an edit drops it until the next solve. That is why
  it is advisory, and ignorable;
- the Closures Computed View (and Plan / Export) shows a notice under the
  read-only pill - "1 loop closure skipped: it spans a single shot." and/or
  "Part of the survey could not be adjusted and is shown as measured." - with
  **Show warnings**.

## Deleting shots around a closure

Shots are edges and stations are entities, so a delete never re-anchors a
closure: a closure on a junction the line leaves stays on the junction
(#776 is impossible by construction), and a closure on a station that leaves
with its line is dropped with it. The sweep (above) then runs.

A delete with shift (a single shot's Shift Geometry, or a range gap-close) can
shorten the path between a closure's two stations to one shot: closure A:1-A:4
with shots 1..2 gap-closed ties the two ends of one shot. Delete shot and the
shift forms of the range delete remove every closure the edit newly left across
one shot (`remove_closures_newly_across_one_shot`), inside the same undo step,
and report each in `CommandResult.warnings` (`CLOSURE_REMOVED_ACROSS_ONE_SHOT`,
"Removed loop closure A / 1 to A / 4: closing the gap left its two stations at
the two ends of one shot."), named by the stations BEFORE the delete; the
frontend shows one warning toast. A closure that already spanned one shot before
the delete is left alone (it has its own warning).

The Delete Shots dialog says so before the user confirms: the range preview
(`preview_delete_shot_range`) runs the very gap-close on a copy and carries
`closures_removed_across_one_shot`, listed under "Close Gap & Delete removes
these loop closures (they would span a single shot)". At a junction the line's
slot carried closure ends follow 0.6.0's member rules (plan Deviation S13,
`survey/edit/junction_leave.rs`).

## Co-location and Move Tools

Closures are treated as co-location relationships **only while aligned** (snaps
and `Tie`s):

- Dragging a station moves the station (every slot of a junction is one
  station) plus the stations tied to it by aligned closures (its tie group).
- Move Segment moves the rigid group: stations joined by shots and aligned
  closures.
- Immovable stations are every datum station and the stations tied to one by
  aligned closures, transitively (D15).

See [move-tools.md](move-tools.md).

## `is_aligned` is derived

An aligned closure is a **rigid edge**: rigid groups follow it, depth
co-location averages station depth across it, the frozen shots freeze the
shots of the loop it closes, and the Closures Computed solve ties its two
stations as one point. So the flag is a claim about geometry, and it is
measured every time it is asked (`closure_alignment`, `survey/closures.rs`),
never stored:

| strategy | aligned when |
|---|---|
| `Move` | the horizontal gap between its ends is under 1 cm AND the tie would not join two datum-held pieces (a rigid edge between two independently placed pieces is the over-constraint of #464) |
| `Tie` | always (the legacy claim, trusted as 0.6.0 trusted it) |
| any other | never: a `Batch` / `LeastSquaresWeighted` closure RECORDS a misclosure, the soft edge the solve relaxes |

The claim follows the geometry in both directions: an edit that pulls a snap
apart makes it a soft closure, one that brings it back within 1 cm makes it a
snap again, and an undo restores exactly what the geometry says.

## Files of Interest

- `src-tauri/crates/models/src/survey/types.rs` (`LoopClosure`, `ClosureStrategy` with `Tie`)
- `src-tauri/crates/models/src/station.rs` (the shared `ClosureStrategy` and its legacy mapping)
- `src-tauri/crates/models/src/survey/ops_closures.rs` (create, remove, the dialog's figures and refusal)
- `src-tauri/crates/models/src/survey/closures.rs` (alignment, the across-one-shot skip set, tie groups)
- `src-tauri/crates/models/src/survey/metrics.rs` (misclosure and path length)
- `src-tauri/crates/models/src/survey/sweep.rs` (the orphan-closure sweep)
- `src-tauri/crates/models/src/survey/adjust.rs`, `lsq.rs` (the Closures Computed View)
- `src-tauri/crates/core/src/core/state/app_state.rs` (the per-revision cache)
- `src-tauri/crates/core/src/core/state/command_station.rs` (the closure commands and their sweep warnings)
- `src/components/Dialogs/ClosureDialog.tsx` (create closure UI)
- `src/components/SingleSystemView/DisplayMenu.tsx` (Geometry mode toggle)
- `src/store/geometrySelectors.ts` (active-geometry selectors)
- `docs/move-tools.md` (move tool behavior + co-location rules)
