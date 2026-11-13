# Split Section Here and Combine Sections

Status: implemented on `feat/merge-split-sections` (not yet a PR).

Split Section Here (#335) is ported from #710 without #709's per-shot credit;
the branch sits on the per-shot people model (`2026-11-12_survey-people.md`).
Combine Sections is new: the two chosen sections meet end to end at exactly one
point and become one; other sections there stay attached as branches. It is
the inverse of a split, at a junction too. Both are one undo step.

## What

- **Split Section Here.** Right-click a mid-section station, or a point along a
  shot, then choose Split Section Here. Nothing is deleted. The first half keeps
  the section id. The second half starts on a co-located copy of the split
  station. The behaviour is #710's (see `project/split_section.rs`): merges,
  name and offset go to both copies, the loop closures stay on the first half,
  and the user picks the half for the comment and the zero datum.
- **Combine Sections.** Right-click an end station where another section ends
  and choose **Merge Section With <name>...** (or, at a junction, a pair from
  **Merge Sections...**), or select exactly two sections in the Sections panel
  and choose Merge 2 Sections. The dialog shows a summary, a kept-section swap, the name (`X-A` +
  `X-B` gives `X`) and the colour. It warns that the other section will be
  reversed, with a swap-L/R checkbox. It asks about the join comment and name
  only when they conflict, notes the direction, and summarises the combined
  survey credit (each person with their shot count, and the date range),
  computed from the shots. The map previews both sections and rings the join.

## Combine rules

The rule: **the two chosen sections meet end to end; other sections there stay
attached as branches.** A candidate is an end station (index 0 or `len`) of two
different sections at the same point (a section with no shots has one station,
both of its ends). The point is the same in one of three ways:

- **Merged.** The two stations are in one merge group.
- **Co-located.** The stations are not merged to each other, and their chain
  positions are within 1 mm horizontally. A chain position is the section
  offset plus the shot walk, without manual station offsets. Depth is not part
  of the match (see the depth row below). This is the state a fresh split
  leaves.
- **Across a loop closure** (added 2026-11-13). A closure ties the two
  stations, or their merge groups: the survey says they are one point, apart
  by the loop's misclosure. One side moves by it (see "Across loops" below).

Other sections at the point:

- **An end merged there** (to either join station) stays merged to the joined
  station, which becomes a station in the middle of the combined section: a
  branch. Its merges are re-keyed to that station's new index, on an append and
  on a prepend, and a branch merged to both join stations is merged once.
- **An end there that is not merged** is left as it is, a separate section end
  at the same point; the dialog says so (`CombineCandidate::loose_ends`).
- **A station in the middle of a section, merged there,** passes through: it
  stays merged to the joined station, a crossing
  (`CombineCandidate::passing_through`). It blocked until 2026-11-13.

`combine_candidate_for_sections` tries all four pairings; when more than one
touches (a loop of two sections) it picks a merge first, then co-located ends,
then a closure, then the smaller gap, then one that needs no reverse. The
station query (`combine_candidates(station)`) lists every other section with an
end at the point (merged, co-located, or across a closure from it), each joined
at the clicked end. The dialog sends that join (`CombineSectionsOptions::join`)
and the command re-checks exactly it, so the station menu, the Sections panel
and the command always agree. The station menu (2026-11-13, at review: the
picker used to be in the dialog) asks `combine_candidates_at_point`, every pair
at the point: with one other section it is one row, **Merge Section With
<name>...**; with three sections in all, **Merge Sections...** lists every pair;
with four or more it nests "A with..." over the sections A can be merged with.
Each pair says how it meets, or is disabled with its reason; the first-named
section is kept by default. The dialog folds everything informational under
**Details**.
Past `MAX_COMBINE_PARTNERS` (16) other ends the point is a pile (unplaced
sections all start at the origin), not a junction to pick from; the menu points
to the Sections panel, which combines any two.

**Refused**, with the reason shown in the menu row and the dialog:

| Condition | Why |
|---|---|
| Both sections carry a zero datum on one of their own stations | The combined section would hold two datums in one rigid group |
| Sections that meet nowhere (not merged, not within 1 mm, no closure between ends) | Nothing says the ends are one point: merge two of them, or create a loop closure between them, first |
| An end merged only to the middle of another section | No other section end to join |
| More than 99,999 shots, or the kept section has used up its shot ids this session | Shot-id budget (keep the other, or save and reopen) |
| A SpeleoDB project open without its edit lock | `guard_mutation`, like every edit (it checks only that lock) |
| The closure-adjusted view, Plan and Export | The frontend: Plan and Export show no survey menu, the closure view disables its edit items, and both dialogs re-check `isEditingBlockedNow` on confirm |

**Changed instead of refused** (2026-11-13; each listed in the dialog for the
kept section it shows, from `CombinePlan`):

| Condition | What the combine does |
|---|---|
| The two ends are tied by a loop closure, or apart by more than 1 mm (a torn merge, a co-located residual) | The moving side shifts by the gap (see "Across loops") |
| Still rigidly tied once the join is gone (merge or snapped closure, directly or through other sections) | That tie becomes a `Batch` loop closure, the one nearest the moving section. An unaligned closure stays as a loop to adjust inside the section |
| Different stored north references | The other section's azimuths turn by the correction difference and it takes the kept reference, so every station stays put. A declared reference equal to the project default is still converted (by 0): the combined section follows the default from now on |
| Different quality | The kept section's wins |
| Join stations' manual offsets differ | The fixed side's offset wins; the dialog says how far the other end moves |
| Unmerged join stations apart in depth (any amount) | Reported above 5 cm (half the 0.1 m resolution depth gauges record): the joined station takes the average of the shot before and after it. Above 1 m this was refused until 2026-11-13; now the user reads the gap and decides |
| The two sections share a shot identity | The other section's copies get new uuids (derived, like the load heal), and station keys on them follow |
| A section with no shots | Absorbed at its one station; the end named is the one that needs no reverse |

### Across loops (2026-11-13)

`project/combine_plan.rs` decides, per choice of kept section, what has to give.
It leans on the merge forest the app keeps: `merge_stations` refuses a merge
between sections already rigidly connected, so merges never close a loop; a
loop is closed by a closure (snapped ones are rigid, the rest are soft).

- **Join ties** make the two join stations one point: the ties between the two
  join stations' merge groups (for a merged join, every tie inside its one
  group). The combine folds them into the joined station. A tie that reaches a
  station of either section other than its join station is not one: that
  section comes back to the join point, and the tie is a loop.
- **The moving side.** With a gap over the 1 mm tolerance, the other section
  M moves, with everything still merged to it, unless that side holds a zero
  datum and the kept side does not; then the kept side moves. With both sides
  anchored, M (whichever does not itself carry a datum) also tears its merges
  towards its datum sides. Only when both sections carry a datum themselves is
  the combine refused.
- **Loop ties.** A merge from M to a section that still reaches the fixed one
  F without M (and without the join) closes a loop through the combined
  section: it becomes a `Batch` closure. Tearing at M, not further round the
  loop, moves the least. A snapped closure between M's side and F's side is
  demoted to `Batch` for the same reason, and with a shift every snapped
  closure from M's side to anything that stays.
- **After.** The new closures, and every closure with one end on the moved
  side, are measured again off the final geometry (each without itself, as
  `create_loop_closure` measures). Nothing the combine leaves is a sole bridge:
  a torn tie's ends stay connected through the combined section, or tie two
  anchored groups (a promotion the loader refuses), so a reload converts
  nothing back. The merge dedupe drops any merge onto the combined section whose
  stations other merges already make one point (a junction's branches merged
  pairwise).

`combine_closure_tests.rs` checks the shapes one by one and then sweeps random
small networks (merged branches, snapped and recorded loops, zero datums, a
second anchored network tied on by a closure), combining everything the station
menu offers, keeping either section: the fixed side never moves, the moving
side moves by exactly the plan's shift, merges stay a forest, nothing snapped is
left inside the combined section, and a reload moves and converts nothing.

**Mutation** (`project/combine_sections.rs`):

0. Shot uuids the other section shares with the kept one are renewed first
   (`renew_shared_shot_uuids`, derived like the load heal). The command runs
   it as a heal before it pins position-keyed ignores and records the undo
   step, so the pins, the held component gauge and the snapshot name each
   shot by the identity it keeps.
1. What has to give (`prepare_combine`): convert the other section's bearings
   to the kept north reference; drop closures across the join point; tear the
   plan's ties into `Batch` closures; shift the moving side.
2. Reverse the other section when the ends match, with `reverse_section`.
3. Settle the join:
   - The join merge and any closure between the two join stations go (either
     would tie the joined station to itself).
   - A comment or name both stations carry is kept once if identical, else per
     the pick: kept, other or both (joined with a newline, or ` / ` for names).
   - The fixed side's offset (or none) wins: the kept station's, unless the
     kept side is the one that moved.
4. Append or prepend the other section's shots. The kept section keeps its shot
   ids. The other section's shots get new ids and keep their uuids and every
   other field. A prepend moves the kept anchor back by the other section's
   span, so the kept stations do not move.
5. Re-key every stored reference by its STORED key (`project/station_rekey.rs`,
   shared with split), with a per-collection injectivity check. Drop closures
   the combine made identical.
6. Keep the kept section's own gauge on the physical station of the lower
   section id's (`project/gauge.rs::gauge_pair_for_station`), and the
   component's least-squares anchor on the station it was held at (see "The
   component's anchor" below).
7. Retire the other id (see dive plans below), and measure the plan's closures
   again.

## Deviations from #710 / #709, and why

- **No #709 credit model.** Survey credit (surveyors, explorers, date) lives on
  every shot in the per-shot people model this branch is rebased onto, and a
  split, a reverse and a combine carry it with the shot, so every shot keeps
  its own exactly; nothing is copied, folded or overridden. The split builds its
  second half with no legacy section template (which `Section::new` would give
  to every uncredited shot). The model tests, the command undo test and the
  metamorphic split-then-combine identity compare per-shot credit.
- **The key remap is keyed by stored key.** #710 paired `for_each_station_ref`
  with `map_station_refs` by traversal position, which relies on two HashMap
  iterations agreeing on order. `station_rekey.rs` maps old key to new key, as
  `reverse.rs` does.
- **The gauge is re-captured, not cleared.** #710 cleared a gauge pair the cut
  separates, which moved the gauge to the index fallback. Now the half that
  holds the gauge station keeps or re-captures a pair naming it. A captured
  gauge stays captured; an uncaptured one is captured only when the fallback
  would name a different station. The component's anchor is kept too, even
  when the gauge station lands in the higher-id half while both halves stay in
  one component (see "The component's anchor" below).
- **`section_budget_refusal` is shared.** Split reuses it instead of a copy, and
  its message now reads for any split path. Combine mints no section, so its
  budget check is the per-section SHOT budget instead.
- **Ignored warnings follow moved shots.** A row is keyed by `(section_id,
  shot_uuid)`, so it lapsed when a split, delete-and-split or combine moved its
  shot to another section. Both the read side and the save carry-over now look
  the uuid up project-wide (`warnings.rs::ShotHomes`).
- **`split_shot_at_distance` copies the parent's non-geometry fields** with a
  struct update, so a future per-shot field is not silently dropped.
- **The co-located check includes merged joins.** A merged join whose chain
  positions are more than 1 mm apart (a torn merge) is blocked. The plan only
  required this for unmerged ends. Otherwise the combine would move the other
  section by the tear.
- **Rigid ties through a third section also block** (the safer reading of
  "connected at a second place"). This matches the forest invariant
  `merge_stations` keeps.

## Compared with #713

Borrowed:
- The shared-shot-identity refusal.
- Test shapes:
  - every refusal leaves the project byte-identical, keeping either section;
  - an aligned closure from the merged join into either section;
  - keeping the higher id leaves the closure-adjusted geometry in place;
  - a one-shot lower section's gauge keeps its side;
  - the station query with unknown and out-of-range input;
  - keeping the higher id.
- The dialog line "X will be [reversed and] added before/after Y".

Kept on purpose:
- **Junctions, with a choice.** Like #713, a join with other sections at the
  point is allowed and they stay attached; unlike it, the station menu lists
  every partner for the user to pick, and a section passing through the point
  still blocks.
- **The offset block instead of a kept/other choice.** A manual offset moves one
  station, not the chain beyond it. Joining the two chains therefore places
  every other station of the other section from the KEPT end's chain position.
  When the two ends' offsets differ by `d`, those stations move by `d` whichever
  offset the joined station keeps (#713's module doc says so). That is a
  geometry change of arbitrary size. It also tears any merge from the other
  section's middle into an anchored group, which no cascade then closes. So
  combining stays blocked above 1 mm, which bounds the move by the same
  tolerance as the co-location test.
- **A torn merged join (more than 1 mm apart) is blocked.** #713 joins merged
  ends however far apart they are drawn, which moves the other section by the gap.

## The component's anchor

A datum-less component is held fixed in the least-squares adjustment at the
gauge station of its LOWEST-ID section (`default_anchor_for_component`). These
edits change the component's sections: a combine retires the other section's
id, a split or delete-and-split mints one. Keeping each section's own gauge on
its station is not enough. Two shapes moved the closure-adjusted survey:

- Combine with a third section whose id lies between the two (O lowest, T,
  then K; keep K, or K lowest and keep O): O's id is retired, T anchors, and T
  does not hold the station O did.
- Split or delete-and-split whose gauge station goes to the new (higher-id)
  half while the halves stay one component: the first half keeps the id and
  anchors, and it no longer holds the station.

Each edit brackets itself with `Project::hold_component_gauges` (before: the
physical station each touched component is held at, named by its shots) and
`restore_component_gauges` (after). For every component that still holds one
of those stations, the station goes back on the gauge of the section that
anchors it (its lowest id), when that section holds it. When it does not, or
no gauge pair can name the station there, the station is recorded in
`Project::held_gauges`, which `default_anchor_for_component` reads before the
lowest-id rule.

Why a record, and not ids: the alternatives were to give the combined section
the lower of the two ids, and to let the split half that holds the station keep
the section's id. Both change station labels (`5:3`) the user reads, depending
on a gauge they cannot see. A held gauge names its station by its section and
shot uuids, like a station key: one probe of the shot uuid index finds it, and
the load's rename of a repeated uuid follows it (`apply_shot_uuid_remap`) to
the copy it meant. Every edit that moves shots to another section re-records
the held gauge of each component it touches, so it follows the station through
later reverses, splits and combines. It is part of the project (undo snapshots
carry it), persisted in `project_settings` (no schema bump), and carried with
renamed uuids by Add .stalx and Merge projects. An older build ignores it and
uses the lowest-id rule, which only moves the adjusted view.

A file is untrusted, so the load bounds the list before anything reads it:
entries whose section does not hold their shot and duplicates are dropped, the
rest is cut to the section count, and after the uuid heal one entry per
component is kept (`normalize_held_gauges`, a union-find over merges and
closures). Add and Merge projects normalise the same way.

Recorded whether or not the component has a closure or a datum today, so a
later adjustment holds the same station either way.

Residuals:
- A delete-and-split that deletes the anchor station itself (it was between
  the two halves) has nothing to hold: the component falls back to the
  lowest-id rule.
- When an edit joins two components (a combine at co-located, unmerged ends),
  the joined one keeps the station of the one whose lowest id was lower; the
  other's adjustment was a separate solve and changes with the join anyway.
- A station whose shots have no uuid (a pre-v6 file before its first save)
  cannot be named and falls back to the lowest-id rule.

Tests: the two shapes from the #716 review, a matrix over every pairing,
parity, creation order (so every section can be the anchor), kept side and
captured or uncaptured gauge for combine (384 cases), every cut, gauge station,
orientation and creation order for split (72 cases), the delete-and-split
analogue in both paths, and undo, redo and a reopen through `AppState`. All
compare the closure-adjusted position of every station, matched by measured
position, before and after.

## Dive-plan pins

A plan pin at a section end names one neighbouring shot and a side. The side is
corrected by comparing the parity the pin recorded with its section's
`reverse_parity`. After a combine, the other section's shots live in the kept
section, whose parity is unrelated to theirs, so a pin on the other section's
end stations could land one station off.

Plans live in the `.plnx` beside the project, outside the undo step, so they
cannot be re-keyed. The first answer here, a map of retired section ids with a
parity relation, broke under undo. The plan store recaptures every anchor
whenever a pin moves, so pins captured AFTER a combine were written against the
kept section's parity. The undo then removed the map entry with the rest of the
snapshot, and those pins landed one station off.

So the orientation is a property of each shot RECORD, not of a section. A
shot's record parity is its section's `reverse_parity`, flipped when its uuid
is in `Project::counter_parity_shots`. A reverse flips the section's flag and
every record together, so their relation is unchanged. A combine toggles the
other section's shots in the set whenever the two parities differ. Every
shot's record parity therefore changes only when that record is reversed.

- **Pins.** A pin records its neighbours' record parities (`parity`, and
  `parity_b` for the second neighbour, which differs across a combine join) and
  compares each with that shot's record parity wherever the shot lives now.
  Plans written before `parity_b` existed read `parity` for both, as before.
- **Undo.** The set is part of the project, so every undo snapshot restores it
  with the shots.
- **Storage.** It is persisted in `project_settings` (no schema bump), pruned
  to the shots the project holds on save and on load. The load heal of a
  project's own repeated uuids carries an entry onto the renamed copy too (both
  copies read it before the heal); a copy Add .stalx or Merge projects renames
  takes its entry from its own source, never from the target shot it yielded
  the uuid to.
- **Section.** The pin's section is found by shot uuid (#710's
  `anchor_section`).
- **Tests.** One test checks every station of both sections across a combine.
  Another checks every station of the combined section across its undo and
  redo. Both run over every pairing and every parity combination.

## Frontend positional refs

Split and Combine move shots between sections, which no index transform (the
reverse's `i -> N - i`) describes. The distance-gradient origin is therefore
anchored like a plan pin around every survey edit (`beforeSurveyEdit` /
`afterSurveyEdit` in `utils/reverseAftermath.ts`): `plan_anchors_for_refs`
before, `plan_refs_for_anchors` after, so it follows its station into the
second half of a split, past a prepending combine, and back through the undo.
An origin that places nowhere is cleared with an info notice (the ramp starts
from the entrance again) instead of re-rooting silently. An edit that moved
shots between sections (a section vanished, or one appeared while another
changed its shot count) also clears the selection, the merge staging, the
move-comment pickup and the comment popover, as a reverse does.

## Follow-ups

- Delete-and-split now copies `reverse_parity` and the gauge to its second half
  (`Project::settle_cut_halves`, shared with Split Section Here).
- Plans in the older station-key anchor format are not repaired past a split or
  a combine. Only the pair format is (as #710 noted).
