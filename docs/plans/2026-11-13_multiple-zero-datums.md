# Several GPS zero datums in one connected survey

Status: IN PROGRESS. Issue: #741.

## Problem

A user sets a zero datum on a second entrance of a cave that already has one, and gets:

> Cannot set zero datum: "Section B" #44 is connected to "Section A" #0 which already has a zero datum

The station is about 1.4 km of survey from the first datum, at an entrance with its own GPS.
Several GPS-fixed entrances in one connected cave is normal. Today the app refuses it.

The rule is "at most one zero datum per merge-connected component". It is enforced in
`set_zero_datum` and `store_zero_datum_without_repositioning`
(`crates/models/src/project/mutations.rs`), and in the merge guard and the closure-to-merge
conversion (`merge_would_double_anchor`, `crates/models/src/project/connectivity.rs:600`). A file
that breaks the rule gets a warning on load (`over_anchored_merge_components`).

## Why the rule exists

In Raw Data View each section is placed by a translation only, and a merge glues two sections
at a shared station. A merge-connected group is therefore one rigid body. One GPS pin decides
where that body sits. A second pin elsewhere in the body never agrees exactly with the surveyed
vector between the two points, so satisfying both means tearing a merge. That tear was a real
bug, and the rule was the fix: it guards the raw geometry. It was not a decision that a cave
can only have one GPS point.

## How other cave survey tools handle it

The approaches differ, but every one of these tools lets a cave carry more than one GPS fix.

- **Ariane** (measured with a synthetic two-entrance file, 2026-11-13). Every entrance START
  carries its own lat/lon, and each one is a hard fix. When the GPS disagrees with the survey,
  Ariane bends the loop between the fixes to fit and shows the loop error (`Err=20.0%` for a
  100 m disagreement over a 500 m loop). Giving two STARTs the same lat/lon pins them on top of
  each other and collapses the loop between them.
- **Survex / Therion.** Fixed stations can carry a standard error, so a fix can be soft or hard,
  and the network adjustment spreads the misfit. Users debate whether forcing survey legs to
  absorb GPS error is wise. That is an argument for weighting the fix.
- **Compass.** Several fixed stations are allowed, and loop closure absorbs the mismatch.

## Decisions

1. **The survey bends to fit the GPS in Closures Computed View.** Raw Data View stays the
   measurements: the extra datum does not move anything there, and the gap is shown instead.
2. **The fix is weighted.** GPS usually comes from a smartwatch, followed by a direct shot to the
   line, so it is good to a few metres, not to centimetres. Each datum gets a horizontal accuracy,
   default 0, a hard pin: the computed survey goes exactly through the GPS. A soft
   default was measured and rejected: with 5 m, a fix 40 m off over a 500 m traverse
   moved the survey only about 2.4 m, because the solve trusts the survey to about
   a metre over that distance. A user who wants a softer fix types an accuracy.
3. **The UI keeps the name "Zero Datum".** When the cave already has one, the Edit Station
   dialog explains what this one will do instead of refusing.
4. **Sanity warning.** Warn, without blocking, when the gap between the surveyed position and
   the GPS is larger than **15 m + 2% of the survey distance from the placement datum of the
   datum's piece**, measured along shots and merges. The raw position is carried from that
   datum, so that is the survey whose error the allowance covers; another fix nearby, or a
   loop closure back to the datum, does not shorten it. Example: 1.4 km away gives 43 m.
5. **Every datum is an entrance**, for Plan mode's "Start from entrance" and for Ariane export.

## Design

### Model: one list, roles derived

- `zero_datums` stays one list. `ZeroDatum` gains `accuracy_m: Option<f64>` (`None` means the
  default).
- In each merge-connected component, the **placement datum** is the first datum in list order
  that resolves. It works exactly as today: it places the rigid group, and the first one in the
  project defines the frame.
- Every other datum in the component is a **GPS fix**. It never moves the raw geometry. It
  feeds the Computed View solve and the gap check.
- Roles are derived, never stored. Merging two placed pieces, splitting a piece by an unmerge
  or a delete, and undo all re-derive them, with no promote or demote bookkeeping.
- **A promoted datum places its piece on its GPS** (project owner's decision). When a command
  makes a datum the placement datum of its piece that was not one before (a fix promoted by
  removing the placement datum, an unmerge, or a delete that splits the piece), the piece is
  translated so that datum sits at `gps_to_local(datum.gps, frame.gps)`, exactly as
  `set_zero_datum` places a subsequent datum. The arithmetic uses the frame in effect before the
  command; when the command also changed the frame, the usual re-frame follows. Every command
  epilogue does this through one call, `Project::settle_datum_layout`
  (`crates/models/src/project/datum_placement.rs`), and the undo of every command that can
  promote restores a whole-project snapshot.

### Persistence: older readers keep a safe view

- On save, placement datums go to `zero_datums` with its columns unchanged. GPS fixes, and every
  datum's accuracy, go to a **new table**. `SCHEMA_VERSION` stays at v18, because the outside
  `.stalx` reader pins it.
- On load, the two are combined, keeping the list order.
- Older builds and the outside reader see at most one datum per component, so nothing tears for
  them. The cost: they drop the fixes on save, the same as other tables added since v18.

### Raw Data View

- Unchanged geometry. The guards that refuse today become "the later datum is a GPS fix":
  - `set_zero_datum` and `store_zero_datum_without_repositioning`;
  - the merge guard (`offsets.rs:355`);
  - the closure-to-merge conversion (`offsets.rs:509`);
  - the integrity check (`integrity.rs:674`).
- `over_anchored_merge_components` and its load warning are retired, or reduced to the
  sanity-warning check.
- **Opening a file moves nothing** (review D6). An older file whose merged group holds two
  datums and a merge torn between them (each half pinned by its own datum) has that merge
  turned back into an unaligned loop closure when it opens, and by Repair, before the offset
  re-solve: each piece keeps its own datum as its placement datum and nothing moves. Left a
  merge, the re-solve would move the later piece off its GPS by the whole tear. Joining the
  pieces is then an explicit merge, which moves the later piece and can be undone. Chosen
  over a new "move onto GPS" Repair because it is the old Repair's behaviour, needs no new
  action, and never moves a piece the user did not ask to move. A legacy group with two
  datums and no torn merge loads unchanged; the later datum is a fix and reports its gap.
- **A station holds at most one datum** (review D2). Setting a datum on a station another
  datum already sits on (under another key) is refused; re-setting a datum updates it in
  place. Combine refuses a join whose two join stations each hold a datum, since the
  combined station would hold both: the save keeps one, and Remove Zero Datum clears both.
- **An accuracy-only edit moves nothing** (review D5). Re-setting a datum with the GPS it
  already holds only updates its accuracy, whatever its role; a promoted fix is placed on
  its GPS by `settle_datum_layout`, so after a remove, an unmerge, a delete or a Repair no
  placement datum is left off its GPS.
- Each GPS fix gets a **gap**: the horizontal distance and bearing from its surveyed position
  to `gps_to_local(fix.gps, frame.gps)`.

### Closures Computed View

- The solve (`crates/models/src/project/lsq.rs`) already grounds several fixed groups per
  component (`fixed_groups`, around line 378). Today it pins each datum at its RAW coordinates.
- Change: the placement datum stays fixed. Each GPS fix adds a weighted observation pulling its
  group toward its GPS position, with weight `1 / accuracy^2`; an accuracy of 0 means a fixed
  group at that position. In the Laplacian this is one diagonal term and one right-hand-side
  term per fix.
- The solve currently runs only for components that have eligible closures. It must also run
  for any component with a GPS fix.
- Horizontal only, like the rest of the solve. Depths are untouched.

### Entrances

- Plan mode "Start from entrance" lists every datum.
- Ariane export: every datum's station gets a START carrying its own GPS. Ariane hard-fixes
  each START and bends the loop, which is close to our Computed View; Ariane is hard where ours
  is weighted.
  - A fix in the middle of a section that is merged into the cave has no START today.
  - Writing one needs a shape Ariane accepts: for example a zero-length START at the GPS with a
    CLOSURE to the fix station.
  - Render that shape in Ariane before shipping it.

### UI

- **Edit Station dialog.** When the station's piece is already placed, the checkbox stays "Set
  as Zero Datum". The dialog then says this datum will bend the computed survey to fit its GPS,
  and shows the gap and the warning (decision 4) as soon as GPS is entered.
- **Accuracy field** in metres, default 0 (goes exactly through the GPS).
- **Map.** A GPS fix is drawn like a zero datum. Optionally, a thin line from the surveyed
  position to the GPS position, in Raw Data View only.
- **Warnings tab.** One entry per GPS fix whose gap exceeds the threshold. It hangs off the shot
  that arrives at the fix station (`datum_gps_gap`); a fix on a section's first station uses
  shot 0 under its own code (`datum_gps_gap_start`), so fixes on stations 0 and 1 are ignored
  independently. A fix on a section with no shots has no row (it is logged). An Ignore is
  stored under the code plus the datum's GPS (`datum_gap_ignore_code`), so it covers that
  GPS reading only: a new GPS brings the warning back, and undoing the change brings the
  Ignore back with the old GPS (review U2). Keyed rather than cleared, because the ignores
  live in the file's `resolved_warnings` table, outside the undo history.

## Consumers to audit

Each of these reads `zero_datums` assuming at most one per component:

- `frame_datum`, `renormalize_frame`;
- `component_datum_refs`, `section_has_zero_datum`, `find_connected_zero_datum`;
- `cascade_connection_offsets` (datum sections are authoritative);
- the destructive closure distribution ("Multiple zero datum stations in closure path");
- the immovable-station search used by the move tools;
- `place_by_datum` (Merge projects, Add .stalx);
- the far-datum detection proposed in #688;
- the TML START GPS pass (`export/tml.rs`, around line 1690);
- Plan mode's entrance list.

Most of them should read "placement datums" through one accessor. Only the solve, the gap
check, export and Plan mode read the fixes.

## Phases (one PR each)

1. **Model, persistence and Raw Data View.** Derived roles, the new table, relaxed guards, the
   gap, the dialog and the warning. Users can set the datum and see the disagreement.
2. **Computed View solve** with weighted fixes.
3. **Entrances.** Plan mode, and Ariane export with a render check.

## Tests (synthetic data only)

- Two entrances joined by a 500 m traverse, the GPS 40 m off:
  - Raw Data View is unchanged;
  - the gap is 40 m;
  - with a hard fix the Computed View puts the fix station on its GPS;
  - with a 5 m accuracy it lands in between, at the closed-form position.
- Merging two placed pieces succeeds, and the later datum becomes a fix. Undo restores both
  placements.
- Unmerging restores the fix to a placement datum, and the detached piece moves onto its GPS.
  Removing the frame datum of a piece that holds a fix puts the fix on the origin and every other
  separately placed piece on its own GPS. Undo restores both.
- Save and load round trip. An older-format file without the new table loads unchanged. The
  `zero_datums` rows written never hold two per component.
- Sanity warning boundary: just under and just over 15 m + 2%.
- Injection checks: drop the fix observation, and the Computed View tests go red; read fixes as
  placement datums in the raw view, and the merge-tear test goes red.

## Open questions, settled by the project owner

The geometry owner had not answered by 2026-11-14, so the project owner settled
them and implementation started:

1. **Derived roles in one list.** Not a separate list of GPS fixes.
2. **The destructive closure distribution strategies ignore fixes.** Only
   placement datums anchor them.
3. **Default accuracy 0, a hard pin** (changed from 5 m after the solve was
   measured; see decision 2). Depth never takes a GPS fix's elevation into
   account; the solve stays horizontal.

## Implementation tracks

Work runs in parallel on top of one foundation commit, which adds
`ZeroDatum.accuracy_m`, `DatumRole`, the derived roles
(`crates/models/src/project/datum_roles.rs`), `DatumGap`, `ZeroDatumPreview`
and the wire fields. `component_datum_refs` returns placement datums only.

| Track | Scope | PR |
|---|---|---|
| Model | guards, merge and unmerge, cascade, closure distribution, move tools, integrity, warnings, preview command | 1 |
| Persistence | the new table, load and save | 1 |
| Frontend | Edit Station dialog, accuracy field, gap, warnings tab, map | 1 |
| Solve | weighted fixes in the Closures Computed View | 1 |
| Entrances | Plan mode and Ariane export | 2 |

The solve was folded into PR 1: without it, PR 1 alone would pin a fix at its
surveyed position in the Computed View.

## Related

- A side finding from the Ariane check: a START with no datum of its own is written with the
  frame datum's GPS. If such a START has a loop back to another START, Ariane collapses that
  loop. Whether the exporter can produce that today has not been checked; it gets its own
  issue if it can.
- #688, the far-datum warning across separate pieces.
