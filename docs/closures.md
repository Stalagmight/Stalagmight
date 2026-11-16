# Loop Closures (Current Behavior)

This document summarizes how loop closures work in the current codebase (post-removal of Sequential/KeepRaw/Manual strategies).

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
  the project has no closures eligible for the solve (`Batch` / `LeastSquaresWeighted`); `Move`
  closures are already coincident, so a project with only those shows the option disabled.
- Closure-adjusted coordinates are derived by running the weighted least-squares solve on a
  **throwaway clone** of the project (`Project::compute_adjusted_station_coordinates`) and
  returning only the resulting coordinates - the live project's shots/offsets are untouched.
  Because the solve always starts from intact measured data, adding/removing closures
  re-solves accurately instead of compounding on already-adjusted geometry.
- The adjusted view is **read-only**: geometry-mutating tools are disabled there (edits are
  made in the Raw Data View). Selection, inspection, and measuring remain available.
- The adjusted set is fetched lazily via the `get_adjusted_stations` IPC command, memoized in
  a transient `AppState` cache keyed on `(modified_at, closure-config hash)` and cleared on
  every command/undo/redo. The solve runs on a blocking thread; the map shows a
  "computing closure…" indicator on the first switch for large surveys.
- Headline stats (penetration distance, length, depth) are always computed from **measured**
  geometry.

There is **no** longer an "Adjust Closures" dialog or an `adjust_loop_closures` command - the
old destructive apply path was removed.

> Migration: a project whose shots were already baked by a *previous* destructive adjustment
> cannot recover its original measurements (they were overwritten and never stored). Such a
> project opens looking identical (legacy closures keep `is_aligned = true`) and is never
> rewritten again. That promise is explicit in the alignment re-measure below: it governs
> `Move` closures only, and a baked closure carries a different strategy, so nothing demotes
> it. Its shots were rewritten to make the claim true; the app cannot know what the bake did
> to them, so it leaves the claim as recorded and `check_closure_alignment` reports a torn
> one rather than silently changing co-location and move rigidity underneath the survey.

## Strategy Summary

| Strategy | When used | What it does |
|----------|-----------|--------------|
| `ProportionalByLength` | Deferred | Legacy strategy; create-time distribution is disabled and not exposed in the UI (only revisited if users request it). |
| `EqualDistribution` | Deferred | Legacy strategy; create-time distribution is disabled and not exposed in the UI (only revisited if users request it). |
| `Batch` | Create loop closure | Records the closure only; participates in the closure-adjusted display solve. |
| `LeastSquaresWeighted` | Closures Computed View | Weighted least-squares solve over the connected component (derived, non-destructive). |
| `Move` | Snap-to-merge workflow | Closure created during a move; no extra distribution at create time. |

Removed strategies:
- `Sequential` (no longer available; legacy string maps to `LeastSquaresWeighted`).
- `KeepRaw` / `ManualAdjustment` (legacy strings map to `Batch`).

## Create Loop Closure

Validation rules:
- Both stations must exist.
- Stations cannot already be merged.
- Stations must be in the same connected component (merges + closures),
  **unless** both sections are connected to zero datums via merges (see `section_has_zero_datum`).
  This zero-datum check uses merge-only connectivity (closures are ignored).
- Duplicate closure pairs are rejected.
- A closure must not join the two ends of one shot (#737). The pair is refused,
  for every strategy (`Batch`, `LeastSquaresWeighted` and `Move` alike), when it
  would tie the two ends of ONE shot together - directly (A:1 to A:2), or because
  merges and other closures already tie each station to one end of the same shot -
  with *"This closure would join the two ends of one shot (A:1 to A:2) and squeeze
  that shot to zero length. Pick two stations that are not the ends of a single
  shot."*. That holds even when another route also joins the two stations (a side
  passage merged on at A:1 and closed to A:2, say): the solve would still have to
  put both ends of the shot at one point. It is
  judged against the constraint knots the solve uses (see below), by the one
  shared helper `project/closure_shots.rs`. The importer's
  `create_loop_closures_batch` refuses the same pairs with the same words.
  This is the only refusal it adds: a closure between two stations that other
  merges and closures already tie together is still created, as before. It is
  harmless to the solve (uniting two stations already in one group is a no-op),
  and refusing it would drop such closures from imported files.
- The closure dialog shows that refusal before Create: `check_merge_connectivity`
  returns it in `ConnectivityResult.closure_refusal`, the dialog prints it and
  disables Create (`closure-refusal`). The backend stays the authority.

Metrics are computed from current station coordinates:
- Misclosure distance and horizontal/vertical components are computed from current raw station coordinates.
- Path length is computed via `calculate_path_length_between`, which treats merges and **existing closures** as zero-distance edges. This can shorten the path length if other closures already exist.
- Error percentage = misclosure / path length * 100 (0 if path length is 0).

Create-time distribution strategies (`ProportionalByLength`, `EqualDistribution`) are **deferred** and not available in the UI (only revisited if users request them). Loop closures are currently recorded only (`Batch`), unless created by the Move workflow.

## When a closure becomes a merge (promotion)

A closure exists only for a TRUE loop. When its two endpoints stop being connected
through the survey itself (shots + merges) the closure is their *sole* bridge, which
is a merge - so it is converted. This happens on load
(`redrive_offsets_after_load`), in Repair, in `execute()`'s epilogue for any command
classified `can_orphan_closures`, and inside the shot-removal paths themselves
(`Project::convert_closures_orphaned_since`, scoped to what that removal orphaned so
an inverse does no more than invert).

**A closure between merged stations is removed** by the same sweep: one whose two
stations are one point - the same station (a closure from a station to itself, which
used to be kept), or one merge group (`are_stations_merged`). It closes no loop. The
solve can only skip it, its stored misclosure is a gap the geometry no longer has, and
it would still count as a loop in the project stats and keep the Closures Computed
View on. Converting it would add nothing, so it goes.

Which edits can make one:

- **Not** creating a closure: Create Loop Closure and the import's batch refuse a
  merged pair, and so does the shot-removal rewire.
- **Not** a merge: `merge_stations` refuses a merge inside one rigid component, and a
  closure between two components is either a sole bridge (which every sweep promotes)
  or ties two zero-datum-anchored components, which a merge may not join. Tests try
  every merge of a closure's two ends, and of their merge groups, on random networks
  and after every step of the combine/split/reverse chain fuzzer
  (`check_no_merge_folds_a_closure`): all are refused.
- **A promotion**: two closures that tie a station to the same point record one loop
  twice (the Merge tool and Combine Sections both allow that), and once the sweep
  promotes one, the other ties that point to itself (#738).
- A file, or an undo history, written before #738.

One forward pass is enough: the sweep keeps a station-level merge union-find, extended
on every promotion, and asks each closure "one point?" before "orphan?"; a promotion
only joins components that were not connected, so it cannot merge the ends of a
closure the pass has already kept. Every fate is decided in place and the list is
compacted once, so a file holding 50 000 such closures (the most the stalx-reader
accepts) opens in linear time. `Project::sweep_orphan_closures` reports what it
removed and where each one sat; it logs nothing itself, since the split preview runs
it on a copy - the callers that keep the result log one line with the count.

- **On load** it is silent, exactly as the promotion is: the project opens without
  the closure and the log says how many went. The file settles on the next save: an
  explicit Save writes it, while autosave only runs after an edit.
- **Inside a command** it is part of that command's undo unit: Split Section Here,
  the delete-and-split commands, the deletes, unmerges and Remove Zero Datum through
  `execute()`'s epilogue, Batch and Repair all record a before/after snapshot pair
  around it, so one Undo brings the closure back. Each of those commands except Batch
  and Repair also reports it: a `CLOSURE_REMOVED_MERGED_ENDS` warning per closure,
  named by the stations it had BEFORE the edit (`Project::closures_before_sweep`,
  matched by position, else a unique id, else id and stored ends, since a file can
  repeat an id), which the frontend shows in the closure-removal toast: "Removed loop
  closure 1:0 to 0:3: another loop closure became a merge, which made its two
  stations one point." (or "...: its two stations were already one point." for one
  the edit did not cause). Split Section Here says it in its dialog instead.
- **The scoped sweep** an inverse runs (the shot-removal paths) snapshots
  `Project::swept_closure_ids` first - orphans and closures already between merged
  stations - and leaves those as recorded; it removes only what its own promotions
  make redundant.

The two statements are not equivalent, and the difference is the whole of issue #464:

| | loop closure | station merge |
|---|---|---|
| endpoints | may be metres apart - the misclosure is the point | are ONE point, within `MERGE_ALIGNMENT_INVARIANT_M` (1 cm) |
| enforced by | nothing; it is a record | `debug_assert_merge_alignment`, the offset cascade, integrity |

`debug_assert_merge_alignment` only *panics* at that 1 cm invariant in test builds
(the assert is `#[cfg(test)]`, the whole body `#[cfg(debug_assertions)]`), so outside
tests the signal is the `realign_merges` self-heal plus tolerance-level reporting -
an `error!` past the looser 5 cm `MERGE_ALIGNMENT_TOLERANCE_M` in debug builds, and
`check_merge_alignment` feeding the integrity report in any build.

So a promotion **owes the coincidence it declares**, and cannot delegate that to
whatever cascades next: `cascade_bfs` never traverses an edge into an already
authoritative section, so a merge whose two ends are both authoritative (both
datum-bearing, or one of them the edited section the cascade was seeded with) is a
gap no cascade, realign or Repair will close.

Two rules, both in `models/project/offsets.rs`:

- **`plan_merge_promotion`** decides whether the promotion may happen at all, and
  which side moves. The one refusal is over-anchoring - an edge fusing two
  independently zero-datum-anchored groups puts two GPS anchors in one rigid
  component. A refused closure STAYS a closure (re-anchored, if the deletion moved
  its endpoint), which is legitimate between two anchored groups and is what the
  over-anchoring detector reports. **Distance is never a refusal**: the loader
  converts unconditionally, so declining on geometry would leave memory and disk
  disagreeing about what the relationship is.
- **`settle_minted_merges`** translates the movable side's whole rigid group onto
  the anchored one, exactly as `merge_stations` does interactively (the datum side
  anchors; with neither anchored, `station_a`'s group moves). It runs after the
  minting command's own geometry is final - a shot removal re-places its section's
  stations and the gap-close compensation slides the whole section - so the residual
  is measured off the geometry that will actually be asserted on.

A **replacement merge** (the arbitrary pair a deletion invents to keep a torn merge
group connected) is the opposite case and is governed by
`replacement_merge_allowed`: nothing is translated, and a pair that is not already
coincident is simply not stitched. The pair is arbitrary rather than surveyed, so
moving a group onto it would invent geometry.

## Closures across a Split Section Here or a Combine Sections

None of these edits moves the closure-adjusted survey of a component they keep
in one piece: the station the adjustment held the component at stays the one it
holds, even when the section that anchors the component afterwards (its lowest
id) does not hold it (`Project::held_gauges`; plans/2026-11-12_merge-split-sections.md,
"The component's anchor").

- **Split Section Here** leaves a closure on the split station with the FIRST
  half only; it is not copied to the second half's co-located start. A copy
  would count the same physical loop twice, and when it is the second half's
  only tie the promotion rule above would turn it into a merge and move the
  second half by the loop's misclosure.
- A closure that CROSSES the cut (from a station before it to one past it, with
  nothing merged at the split station) is promoted by that rule when it is the
  halves' only tie, and one half moves by the misclosure. The split is not
  refused and the promotion is not skipped: keeping a sole-tie closure would
  only last until the next load or edit converts it, and refusing would leave
  the user deleting the closure, splitting and merging by hand, with the same
  move. Instead the Split dialog shows which half moves and how far before the
  user confirms (`AppState::split_section_preview`, the split run on a copy).
  When a second closure tied that half to the same point, the merge leaves it
  between merged stations and the split removes it (#738); the dialog lists
  each one by the station ids it was created with ("Loop closure 1:0 to 0:3 is
  removed: its two stations become one point."), read off the same sweep on
  the copy, so it names exactly what the split removes. A closure that already
  tied a point to itself before the split (an undo can restore one from a
  history recorded before #738) is removed by the same sweep and listed apart
  ("...: its two stations are already one point."); it tied nothing, so it does
  not change how the dialog describes the halves.
- **Combine Sections** re-keys a closure on either join station onto the one
  joined station, drops any closure across the join point (whatever its
  length, it would tie the joined station to itself), and drops a closure the
  combine made identical to another one. An UNALIGNED closure between the two
  sections elsewhere stays: it becomes a closure inside one section, which the
  least-squares solve adjusts like any other.
- **Of closures made identical, the one the solve takes stays (#749).**
  Once the two join stations are one, two closures can tie the same two
  stations: a tie the combine tears (a merge becomes a `Batch` closure,
  appended last) can land on a legacy closure's stations, or a snapped one's.
  `dedupe_closures_in` keeps the first closure of each set the solve takes
  (`Batch` / `LeastSquaresWeighted`), or the first one when it takes none,
  and drops the others by position, the way the plan counts them. That is what
  the plan's model of the joined survey (`joined_sweep`) assumes: it counts
  every eligible closure before the dedupe, so with the eligible one kept
  the stations the solve joins are the same before and after it, and the
  anchor rule's verdict and the across-one-shot removal still hold. Keeping
  the first one instead let a legacy closure the solve ignores push out the
  torn merge's closure: the plan had moved the kept side for a solved
  survey, the Closures Computed View then showed it unadjusted, and the
  dialog's "This connection becomes a loop closure" was false. A dropped
  snap takes its rigidity and its depth co-location with it: its two
  stations stay on one point horizontally (a snap the combine does not tear
  has both ends on the side that moves, or both on the side that stays), but
  each now reads its own depth in the measured survey instead of their
  average, and a later edit's offset cascade no longer carries one with the
  other. Keeping the first closure did the same whenever the ordinary one
  came first; with the snap first it dropped the ordinary closure instead,
  which the plan had counted. The kept closure holds the two on one point in
  the Closures Computed View, which the snap alone did not. The dialog lists,
  in Details, every closure the solve does not take that the dedupe drops for
  one it does (`CombinePlan.closures_replaced`), with what takes its place:
  the plan runs the same `closure_dedupe` rule on its model of the combined
  closure list (`closure_dedupe.rs`), and the combine reports what it really
  replaced, a difference being a `debug_assert!` failure (an error in the log
  in a release build). Duplicates the solve treats alike go silently: nothing
  changes for the view. A replacement keeps the connection, so there is no
  summary line outside Details and no toast, unlike a removal across one
  shot. The combine finds closures by id (its join's own, the ties
  `tear_ties` removes, the names of what it replaced) where the plan goes by
  position, so the two agree only while ids are unique. A file keeps a
  closure's id in `loop_closures.closure_id`, which nothing keeps unique, so
  the load gives the later of two closures with one id a fresh id
  (`assign_closure_ids`, as it does an id of 0), and a debug build asserts
  unique ids when a combine starts.
- **A loop closure is a join.** Two section ends tied by a closure are, by the
  survey's own statement, one point, apart by the misclosure. Combining there
  moves one side by the misclosure (the other section and what is merged to
  it, or the kept side when a zero datum holds the other, or when the other
  side holds the station this view is anchored at, so the closure-adjusted
  survey does not shift) and drops the closure. The rigid ties that would then tie the combined section to itself,
  or hold the moving side, become `Batch` closures between the same stations
  (`project/combine_plan.rs`): merges form a forest, so on each path at most
  one merge tears, the one nearest the moving section; a snapped (aligned)
  closure across the two sides, or from a side that moves, is demoted to
  `Batch` the same way. The loop's misclosure moves to the torn tie. Nothing
  the combine makes is a sole bridge (the torn tie's ends stay connected
  through the combined section, or tie two anchored groups, whose promotion is
  refused), so a reload converts nothing back; the randomised sweep in
  `combine_closure_tests.rs` checks exactly that.
- **A side-by-side residual moves a side too (#740).** Two ends side by
  side (unmerged, within `COMBINE_TOLERANCE_M`, 1 mm) are apart by a
  residual, and so are merged ends whose join station was moved by hand (a
  manual station offset puts their chain positions apart). The combined
  section hangs the other section's stations off the kept join station, so
  one side moves by the residual whatever the plan says. The plan used to
  move nothing under the tolerance: the other section alone moved, the
  sections merged to it stayed, merged stations ended up as much as 1 mm
  apart, and the reopen's offset solve (`redrive_offsets_after_load`, seeded
  at each component's datum or lowest id) moved the combined section to close
  them; where the combine tore a merge into a closure the solve takes, this
  view on the other side moved by the residual. Now any residual past float
  dust (`Project::OFFSET_DUST_M` in each axis, the bound under which the
  offset solve corrects nothing) takes the shifting path of `plan_ties`: the
  anchor rule picks the side, what is merged to it moves along, a snapped
  closure from it is demoted, and with both sides held the moving section's
  merges to its datum sides tear, exactly as for a shift. Dust (truly
  coincident or merged ends) joins as before. Re-solving the offsets after the
  combine instead would seed each component at its datum or lowest id, not
  at the side the anchor rule keeps, and would heal drift anywhere else in
  the project too: the plan could not say what moves. The dialog lists a
  move under 1 mm only when the plan tears a tie (`combineMoveShown`): a
  snap from the moving side made ordinary, or the moving section's merge
  torn with both sides held by datums. Their listed reason is the move, so
  the move is listed beside them, to 0.1 mm ("moveSideBySide"); otherwise
  "moves 0.00m" next to "Side by side here" would be noise. The
  `COMBINE_SIDE_BY_SIDE_M` copy of the tolerance in `combineSections.ts` is
  pinned to `COMBINE_TOLERANCE_M` by a tripwire test. The sweeps check
  the plan against the result to 1e-6 (a plan that moves nothing, to dust),
  merged stations on one point and a reload that moves nothing, and
  `side_by_side_network` plants such residuals, which the other generators,
  placing every section exactly, never reach.
- **Datums on both sides of one point (#754).** Two surveys from one
  entrance, each with its own zero datum on the same GPS position, put the
  two join stations on one point: D carries a datum at its start with K
  merged there, and O's datum puts O's start exactly on D's, side by side
  with K's. Combining K with O is allowed (neither carries both datums), the
  gap is float dust, so nothing moves. The both-held rule used to run only
  when a side moved, and the joined station then united the two sides' merge
  groups: two datums in one, which the offset solve treats as two fixed
  points (`multiple_datums_per_merge_component`, an error in the Warnings).
  Now `plan_ties` runs it with nothing to move too (`tear_datum_sides`, the
  half of `pick_moving_side` that a shift uses): the merges from the other
  section (the kept one, when the other carries a datum itself) to its datum
  sides become `Batch` closures with the zero-datum reason, the same ties a
  shift or a residual would tear, so the rule does not depend on the gap.
  So does a snapped closure from the combined section's side to a part such
  a merge is torn from (`snaps_to_datum_parts`): the offset cascade solves
  through a snap as through a merge, so it would keep the two datums in one
  rigid group (and the per-command alignment sweep would then demote it
  without a word). A shift tears that snap anyway, because its side moves.
  The dialog lists them before the confirm ("The merge of K and D", "D is
  held by a zero datum and stays where it is."), the held station named by
  the plan (`TornTie.held`), and no move. In a project the app keeps sound
  only a side-by-side join reaches this: a closure join is over 1 mm apart
  (it shifts), and merged ends, or two sections already tied by merges
  elsewhere, have both sides in one merge group, which `merge_stations` and
  `set_zero_datum` keep to one datum. A file can still hold two datums in
  one merge group (the integrity check reports it); a merged join there
  reaches the rule too (both sides held once the join's own merges are left
  out), and its tear leaves the two datums in two groups, a repair.
  Where a closure the combine leaves puts two datums in one solve group (the
  issue's shape: D's datum at D 0 and O's on the joined station, the torn
  merge between them; or an ordinary closure already tying D 0 to K 0), the
  solve below cannot adjust that part ("Multiple zero datums in a single
  constraint group"): it shows measured (the same geometry, since the two
  stations are one point) with the advisory notice and
  `CLOSURE_ADJUSTMENT_FAILED` warning, until one datum is removed. A shift
  tears the same merge with the same result (master did so for any residual
  past dust), so this is no new state; refusing such a combine instead would
  have to refuse it at every gap, and per kept section (which section tears
  depends on it), and was left as a choice for later. The plan says so,
  from the joined survey's sweep (`ties_between_datums` on its knots, the
  solve's own groups): `TornTie.between_datums` on the tie ("Its two
  stations are then held by two zero datums, so the Closures Computed View
  will show this part of the survey as measured until one of them is
  removed."), and `CombinePlan.adjustment_blocked_by_datums` for the part,
  which a note says when no torn tie does, and which drops the claim that
  the view adjusts the new closure like any other loop from the paragraph
  under the torn ties. The sweeps check that no two datums share a merge
  group or a rigid group after a combine that did not before
  (`check_datums_stay_apart`), and the plan's two marks against the real
  solve after every combine (`check_datum_refusal_planned`);
  `side_by_side_network` plants a shared entrance on top of its side-by-side
  pair in one seed of three (`plant_entrance`).
- **Nothing is left across one shot (#746).** Once the two join stations are
  one, a tie the combine tears (a merge, or a snap made ordinary) can join the
  two ends of one shot: a one-shot passage X merged to both join stations,
  say, whose merge at the moving end would become a closure across X's shot.
  So can an ordinary closure the join alone puts there. The solve would only
  skip such a closure, so the combine removes it instead, inside its one
  project snapshot pair (one Undo restores it with the rest), and reports each
  as a `CLOSURE_REMOVED_ACROSS_ONE_SHOT` warning ("Removed the merge of X 1:1
  and O 2:0: combining the sections left its two stations at the two ends of
  one shot.", each station with its section's name before the combine, since
  the other section is gone afterwards), shown in the shared removed-closure
  toast. The plan names them before the confirm
  (`CombinePlan.closures_removed_across_one_shot`, each by its stations before
  the combine and as what it was). The dialog says how many outside the folded
  Details, so a user who never opens them is still told, and lists each under
  Details, apart from the ties that become closures. The
  dialog and the combine read the one plan: `Project::joined_across_one_shot`
  runs the joined survey's across-one-shot sweep (`joined_sweep`, the model
  the anchor rule's `joined_survey_is_solved` also reads) over the plan's torn
  ties, and `tear_ties` removes those ties instead of tearing them. The anchor
  verdict does not change: a closure that collapses a shot never made the
  joined survey solved. A closure that already joined the two ends of one shot
  before the combine is left alone (it has its own warning), as Delete with
  shift leaves one. After the join, `closures_newly_across_one_shot` is the
  backstop: anything the plan missed is a `debug_assert!` failure in a debug
  build (every test, fuzzer and debug app build); a release build removes it
  too, logs an error and names it by the tie of this combine it came from, or
  by its own stations before the combine.
  Unlike a split, a combine runs no orphan-closure sweep: it joins two sections
  and never strands one on a lone closure.

## Deferred: Distribution at Create Time (Proportional/Equal)

These algorithms remain in the codebase for possible future use, but are **disabled** today.

The adjustment path uses `shortest_path_shots_merges`:
- Edges are **shots + merges only** (closures are not used for this path).
- Co-located groups are based on merges + loop closures (current behavior treats all closures as co-locating).
- When building targets, steps that stay inside the same co-located group are skipped.

Key behaviors:
- If one endpoint’s co-located group contains a zero datum, that side is treated as the anchor and the path is oriented from anchored -> unanchored.
- If a zero-datum station lies **on** the loop path (via merges) **and is not an endpoint**,
  it becomes the anchor and the correction is split across both legs (anchor -> A, anchor -> B).
- If the zero datum is an endpoint, the correction is distributed along the path from that endpoint to the other.
- If **multiple** zero-datum stations appear on the loop path, the operation fails with an error.
- If no zero datum is on the path, the correction is applied along the chosen loop path between A and B.
- If **no loop path exists** (e.g., two zero-datum sections that are not connected), the algorithm falls back to
  `shortest_path_to_any_zero_datum()` for each endpoint and distributes the correction along those two legs.
- Corrections are distributed per-shot either proportionally by length or equally.
- Targets are computed per co-located group, and the same delta is applied to all members of a group.
- Zero-datum stations are never moved; if a computed delta would move one, the operation fails.
- After application, the endpoints are checked for residual misclosure; if still misaligned, the non-anchored side is moved as a final fix (errors if both sides are anchored).

Implementation note:
- The final deltas are applied via `apply_station_move_internal`, which adjusts adjacent shots to absorb the move. This means
  branch shots connected to moved stations may have their length/azimuth/pitch updated to preserve endpoint positions.

Common error conditions:
- `Inconsistent target coordinates ...` when a co-located group is reached by different paths with conflicting targets.
- `Multiple zero datum stations in closure path` when the loop path crosses more than one datum.
- `No distributable shots found ...` if the path collapses to merges/co-located edges (total length or shot count = 0).
- `Closure distribution would move a zero datum station` if a computed delta touches a datum station.
- `Closure distribution failed to align endpoints` if residual misclosure remains and both sides are anchored.

## Closures Computed Solve (LeastSquaresWeighted)

The Closures Computed View solves with **only** `LeastSquaresWeighted`. It runs
non-destructively on a throwaway clone (`Project::compute_adjusted_station_coordinates`),
once per connected component that has eligible closures, anchored on a zero datum when present
and otherwise on a deterministic default anchor (the lexicographically smallest
`(section_id, station_index)` station in the component, via `default_anchor_for_component`).
Eligible closures are `Batch` + `LeastSquaresWeighted` (`Move` closures are already coincident).

Algorithm overview (unchanged math; the only change is *where* it writes - a clone, not the
live project):
- Build constraint groups where stations connected via merges **or closures** are in the same group.
  A closure whose two stations would end up in one group as the two ends of one
  shot - a *closure across one shot* - is skipped first (#737): the solve keeps
  such a shot at its measured vector, so it could not close, and it used to fail
  the whole component with "inconsistent section translation".
  `Project::closures_across_one_shot` decides it with a union-find over every
  station: all merges first, then the eligible closures in `loop_closures` order;
  a closure that would put both ends of some shot into one group that were not in
  one before it is NOT united and is reported with that shot. A skipped closure
  unites nothing, so it never changes what a later closure sees. Uniting walks only
  the smaller group, so the sweep is linear in the common case and deterministic.
  `adjust_loop_closures` drops these closures before the solve, so the groups, the
  spur detection and the `is_aligned` marking all see the same set; a component
  whose only closures are skipped keeps its measured geometry and is not an error.
- If a group contains a zero datum, it is fixed; multiple zero datums in a single group is an error.
- If no zero datum exists in the component, the anchor station's group is fixed.
- For each shot, build weighted constraints in X/Y using section quality and base sigmas from config:
  - `base_sigma_length_m`
  - `base_sigma_azimuth_deg`
  - `base_sigma_depth_m` (depth is not adjusted - the solve is horizontal-only)
- Solve independent least-squares systems for X/Y using a sparse Jacobi-preconditioned
  Conjugate Gradient solver (see [performance.md](performance.md) for algorithm details).
- Update the clone's shot vectors from solved group coordinates and apply section-level
  translations to keep merged stations aligned, then compute coordinates from the clone.
- Zero datum positions are verified to remain fixed; any movement is an error.

A component whose solve still fails (for example a closure that ties two zero
datums into one group: "Multiple zero datums in a single constraint group", which
creation allows because both sections have fixed coordinates) is rolled back to its
measured geometry, as before - but it is no longer silent (#737).
`Project::into_adjusted_stations` returns each failure (anchor, sections, the
solver's error), `AppState` keeps them beside the memoized adjusted stations, and:

- the `get_adjustment_report` IPC returns `{ skipped_closures, failures }`: the
  closures skipped above (computed on request) and the failures of the cached solve
  of the CURRENT project version only, each with a plain-language reason (the raw
  solver text stays in the log). `get_adjusted_stations` is unchanged.
- the Warnings tab lists one `CLOSURE_ACROSS_ONE_SHOT` **error** per skipped
  closure, on the shot it spans (Resolve: **Remove this loop closure**, or go to its
  station). It is a pure function of the saved data, so it gates like any error and
  cannot be ignored.
- it also lists one `CLOSURE_ADJUSTMENT_FAILED` **warning** per failed part, read
  from the same cached solve: the warnings pass never runs the solve, so the entry
  appears once the Closures Computed View, Plan or Export has solved the current
  version (the frontend refreshes warnings once the report names a failure, or
  stops naming one) and an edit drops it until the next solve. That is why it is
  advisory, and ignorable: as an error, the SpeleoDB upload gate and the map's
  issue colouring would depend on which view was open last, and the measured data
  it is about is fine - only the derived adjusted view is degraded. Resolve: **Go
  to station**, or Ignore.
- the Closures Computed View (and Plan / Export, which draw the adjusted geometry)
  shows a notice stacked under the read-only pill - "1 loop closure skipped: it
  spans a single shot." and/or "Part of the survey could not be adjusted and is
  shown as measured." - with **Show warnings**, which opens the Warnings tab. It is
  the primary surface: it is drawn with the adjusted view itself, so it is always
  current.

## Deleting shots around a closure

A delete with shift can shorten the path between a closure's two stations. The
single **Delete shot** refuses to shift a shot that is part of any loop, closure
loops included; the range gap-close (**Delete shots** with shift) refuses only
merge loops, so closure A:1-A:4 with shots 1..2 gap-closed becomes A:1-A:2 - one
shot, no loop. Both delete commands therefore take
`Project::closures_before_edit` (the closures already across one shot, and every
eligible closure's stations as they resolve before the delete), and afterwards
remove every closure `closures_newly_across_one_shot` reports, inside the same
project snapshot pair, so one Undo restores the shots and the closure together.
Each removal is reported in `CommandResult.warnings`
(`CLOSURE_REMOVED_ACROSS_ONE_SHOT`, "Removed loop closure A:1 to A:4: closing the
gap left its two stations at the two ends of one shot.") and shown as one warning toast, named by the stations BEFORE the delete:
the gap-close renumbered A:4 to A:2, but the user created A:1-A:4.

The Delete Shots dialog says so before the user confirms. The range preview
(`preview_delete_shot_range`) carries `closures_removed_across_one_shot`, which
`Project::preview_gap_close_closure_removals` computes by running the very
gap-close the command runs (`Project::gap_close_shot_range`, shared by both) on a
clone and taking the same `closures_newly_across_one_shot`; the dialog lists them
under "Close Gap & Delete removes these loop closures (they would span a single
shot)" (the heading names the gap-close button, "Delete Shots" for a prefix or
suffix range, since Split removes none). It is absent when no
gap-close is offered (the whole section, or a blocked shift). The single-shot
preview has no such list: that delete never shortens a closure to one shot, and
`preview_delete_shot` runs inside every per-shot removal, so a simulation there
would recurse. A closure that already spanned one shot before the delete is left alone (it
has its own warning). This lives in the commands, not in
`remove_shot_with_rewire*`, which is also the inverse of `CreateShot` (#413).

## Co-location and Move Tools

Closures are treated as co-location relationships **only after alignment** (like merges):
- Dragging a station moves all stations in its merge set plus any **aligned** closures.
- Before batch adjustment, closure endpoints move independently (not co-located).
- Immovable stations (zero datums and their merge/aligned-closure neighbors) are computed via BFS over merges + aligned closures.
- There is no longer a KeepRaw exception; closure participation is gated by alignment.

## `is_aligned` is a MEASUREMENT (#469)

An aligned closure is a **rigid edge**: `get_rigid_connected_section_ids` follows it
like a merge, the offset cascade solves through it, co-location averages station depth
across it, and `compute_frozen_shots` freezes the shots of the loop it closes. So the
flag is a claim about geometry, and it is measured rather than asserted. One predicate,
`Project::closure_alignment_stamp`, with three terms:

| term | why |
|---|---|
| strategy is `Move` | only the snap-to-merge workflow claims coincidence. A `Batch` / `LeastSquaresWeighted` closure RECORDS a misclosure - it is the soft edge the Closures Computed solve relaxes, and rigidifying it would fight what it exists for |
| measured horizontal end gap `< MERGE_ALIGNMENT_INVARIANT_M` (1 cm) | the merge INVARIANT, not the 5 cm reporting tolerance: the cascade treats the edge as a merge, so it is held to what a merge is held to. It also makes a fresh stamp unable to trip `check_closure_alignment`, since `INVARIANT <= TOLERANCE` is enforced |
| the rigid edge would not double-anchor | `merge_would_double_anchor`, the refusal `plan_merge_promotion` makes: an edge fusing two independently zero-datum-anchored merge components is the over-constraint of #464, which no cascade, realign or Repair can satisfy. Unlike a promotion an alignment translates nothing, so refusing costs no geometry - the pair simply stays a soft closure |

Three sites apply it, and they are the same predicate so memory and disk cannot
disagree about what a closure is: `create_loop_closure`, `assign_closure_ids` (which
used to FORCE `is_aligned = true` on every `Move` closure on every open - fixing only
the creation site would have made a closure honestly saved as `0` come straight back
as `1`), and `Project::remeasure_closure_alignment`, a derived idempotent sweep run
from `AppState::reconcile_geometry`, from `redrive_offsets_after_load` and from
Repair. Each of those runs it AFTER its own re-solve, so the claims are measured off
the geometry that was settled on rather than the geometry that was found; the
per-command one also runs it BEFORE, because an aligned closure is an edge the
cascade solves through and a stale claim would otherwise drag a section onto a
coincidence that is about to be retracted.

The user-facing snap-to-merge path is unchanged: `ClosureDialog` sends `Move` only
behind the move commands that made the two stations one point, so the measurement says
exactly what the assertion used to. What changes is every other route - IPC, the agent
harness, the fuzzers - which could stamp an arbitrary pair metres apart as closed, and
a closure TORN LATER by an edit that moves one endpoint. Neither can persist a lie now:
the claim follows the geometry in both directions, including across an undo (see
[undo-redo.md](undo-redo.md) for why re-deriving it on an inverse is still a restore).

The sweep measures the GAP only. The stored misclosure metrics stay as recorded -
re-measuring them is a Dijkstra per closure, which does not belong in a per-command
epilogue, and `error_percentage` already means "as measured when recorded".

## Legacy Strategy Mapping

For backward compatibility with stored projects:
- `KeepRaw` -> `Batch`
- `ManualAdjustment` -> `Batch`
- `Sequential` -> `LeastSquaresWeighted`

These mappings are handled in `ClosureStrategy::from_str_lossy` and via serde aliases.

## Files of Interest

- `src-tauri/crates/models/src/station.rs` (ClosureStrategy enum + legacy mapping)
- `src-tauri/crates/models/src/project/` (`mutations.rs`: create logic; `closure.rs`: the
  `adjust_loop_closures` solver kept as the internal derivation helper; `coordinates.rs`:
  `compute_adjusted_station_coordinates`, `default_anchor_for_component`)
- `src-tauri/src/commands/project.rs` (`get_adjusted_stations` IPC command)
- `src-tauri/crates/core/src/core/state/app_state.rs` (transient closure-adjusted cache)
- `src/components/Dialogs/ClosureDialog.tsx` (create closure UI)
- `src/components/SingleSystemView/DisplayMenu.tsx` (Geometry mode toggle)
- `src/store/geometrySelectors.ts` (active-geometry selectors)
- `docs/move-tools.md` (move tool behavior + co-location rules)
