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
- A closure needs a loop to close (#737). The pair is refused, for every strategy
  (`Batch`, `LeastSquaresWeighted` and `Move` alike), when it would tie the two
  ends of ONE shot together - directly (A:1 to A:2), or because merges and other
  closures already tie each station to one end of the same shot - with *"This
  closure would tie together the two ends of one shot (A:1 to A:2), so there is no
  loop to close. Pick stations joined by at least two separate paths."*. It is
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
- **Combine Sections** re-keys a closure on either join station onto the one
  joined station, drops any closure across the join point (whatever its
  length, it would tie the joined station to itself), and drops a closure the
  combine made identical to another one. An UNALIGNED closure between the two
  sections elsewhere stays: it becomes a closure inside one section, which the
  least-squares solve adjusts like any other.
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
- the Warnings tab lists one `CLOSURE_ACROSS_ONE_SHOT` error per skipped closure, on
  the shot it spans (Resolve: **Remove this loop closure**, or go to its station),
  and one `CLOSURE_ADJUSTMENT_FAILED` error per failed part, read from the same
  cached solve (the warnings pass never runs the solve; the frontend refreshes
  warnings once the report names a failure). Both are errors, so neither can be
  ignored.
- the Closures Computed View (and Plan / Export, which draw the adjusted geometry)
  shows a notice beside the read-only pill - "1 loop closure skipped: it spans a
  single shot." and/or "Part of the survey could not be adjusted and is shown as
  measured." - with **Show warnings**, which opens the Warnings tab.

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
gap left its two stations at the two ends of one shot, so it had no loop left to
close.") and shown as one warning toast, named by the stations BEFORE the delete:
the gap-close renumbered A:4 to A:2, but the user created A:1-A:4.

The Delete Shots dialog says so before the user confirms. The range preview
(`preview_delete_shot_range`) carries `closures_removed_across_one_shot`, which
`Project::preview_gap_close_closure_removals` computes by running the very
gap-close the command runs (`Project::gap_close_shot_range`, shared by both) on a
clone and taking the same `closures_newly_across_one_shot`; the dialog lists them
under "Loop closures removed (they would span a single shot)". It is absent when no
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
