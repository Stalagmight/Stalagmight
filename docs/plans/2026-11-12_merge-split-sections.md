# Split Section Here and Combine Sections

Status: implemented on `feat/merge-split-sections` (not yet a PR).

Split Section Here (#335) is ported from #710 without #709's per-shot credit;
the branch sits on the per-shot people model (`2026-11-12_survey-people.md`).
Combine Sections is new: two sections that meet end to end become one, the
inverse of a split. Both are one undo step.

## What

- **Split Section Here.** Right-click a mid-section station, or a point along a
  shot, then choose Split Section Here. Nothing is deleted. The first half keeps
  the section id. The second half starts on a co-located copy of the split
  station. The behaviour is #710's (see `project/split_section.rs`): merges,
  name and offset go to both copies, the loop closures stay on the first half,
  and the user picks the half for the comment and the zero datum.
- **Combine Sections.** Right-click an end station where another section ends,
  or select exactly two sections in the Sections panel, then choose Combine
  Sections. The dialog shows a summary, a kept-section swap, the name (`X-A` +
  `X-B` gives `X`) and the colour. It warns that the other section will be
  reversed, with a swap-L/R checkbox. It asks about the join comment and name
  only when they conflict, notes the direction, and summarises the combined
  survey credit (each person with their shot count, and the date range),
  computed from the shots. The map previews both sections and rings the join.

## Combine rules

A candidate is an end station (index 0 or `len`) of two different sections,
both with shots, at the same point. The point is the same in one of two ways:

- **Merged.** The two stations are merged to each other, and their merge group
  is exactly those two stations.
- **Co-located.** The stations are unmerged with no merges at all, and their
  chain positions are within 1 mm horizontally. A chain position is the
  section offset plus the shot walk, without manual station offsets. Depth is
  never compared. No third section end may sit at the point. This is the state
  a fresh split leaves.

`combine_candidate_for_sections` tries all four pairings and needs exactly one
to be eligible. `combine_candidate(station)` applies the same rule, so the
station menu, the Sections panel and the command always agree.

**Blocked**, with the reason shown in the menu tooltip and the dialog:

| Condition | Why |
|---|---|
| A third station or section end at the join | A branch, not a linear join |
| Different north reference or quality | Bearings would change meaning; quality weights the adjustment |
| Still rigidly tied once the join is gone (merge or aligned closure, directly or through other sections) | A rigid loop. `merge_stations` refuses the same shape. An unaligned closure stays as a loop to adjust inside the section |
| Both join stations carry a zero datum, or (co-located) both rigid groups are anchored | Two datums in one rigid group |
| Join stations' manual offsets differ by more than 1 mm | The joined station keeps the kept one's offset |
| A merged join more than 1 mm apart | The merge is torn, and the combine would move the other section |
| More than 99,999 shots, sections that do not meet or meet at both ends, a section with no shots | Shot-id budget; nothing or something ambiguous to combine |
| Read-only (closure view, Plan, Export, SpeleoDB lock) | `guard_mutation`, like every edit |

**Mutation** (`project/combine_sections.rs`):

1. Reverse the other section when the ends match, with `reverse_section`.
2. Settle the join:
   - The join merge and a zero-length closure between the two ends go.
   - A comment or name both stations carry is kept once if identical, else per
     the pick: kept, other or both (joined with a newline, or ` / ` for names).
   - The kept station's offset (or none) wins.
3. Append or prepend the other section's shots. The kept section keeps its shot
   ids. The other section's shots get new ids and keep their uuids and every
   other field. A prepend moves the kept anchor back by the other section's
   span, so the kept stations do not move.
4. Re-key every stored reference by its STORED key (`project/station_rekey.rs`,
   shared with split), with a per-collection injectivity check. Drop closures
   the combine made identical.
5. Keep the gauge on the physical station of the lower section id, which is the
   gauge the component used (`project/gauge.rs::gauge_pair_for_station`).
6. Retire the other id (see dive plans below).

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
  would name a different station. One case stays: if the gauge station lands
  in the higher-id half and both halves stay in one component, the component
  uses the first half's gauge, because the gauge is defined by the lowest
  section id.
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

## Dive-plan pins

A plan pin at a section end names one neighbouring shot and a side. The side is
corrected by comparing the parity the pin recorded with its section's
`reverse_parity`. After a combine, the other section's shots live in the kept
section, whose parity is unrelated to theirs, so a pin on the other section's
end stations could land one station off.

Plans live in the `.plnx` beside the project, outside the undo step, so they
cannot be re-keyed. Instead the project keeps `retired_sections: other ->
{ into: kept, parity_flip }`. It is stored in `project_settings` (no schema
bump; readers that do not know the key skip it) and travels with every undo
snapshot. Pin resolution follows that chain and XORs the flip into the parity
comparison, and finds the section by shot uuid (#710's `anchor_section`). A test
pins every station of both sections over every pairing and every parity
combination.

## Follow-ups

- Delete-and-split does not copy `reverse_parity` to its second half (an
  existing gap). Single-neighbour pins and the gauge fallback past such a cut
  read a reset parity.
- Plans in the older station-key anchor format are not repaired past a split or
  a combine. Only the pair format is (as #710 noted).
