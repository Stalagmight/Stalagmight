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
   default 5 m. An accuracy of 0 is a hard pin.
3. **The UI keeps the name "Zero Datum".** When the cave already has one, the Edit Station
   dialog explains what this one will do instead of refusing.
4. **Sanity warning.** Warn, without blocking, when the gap between the surveyed position and
   the GPS is larger than **15 m + 2% of the survey distance to the nearest other datum**.
   Example: 1.4 km away gives 43 m.
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
- **Accuracy field** in metres, default 5.
- **Map.** A GPS fix is drawn like a zero datum. Optionally, a thin line from the surveyed
  position to the GPS position, in Raw Data View only.
- **Warnings tab.** One entry per GPS fix whose gap exceeds the threshold.

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
  - with a 5 m accuracy it lands in between, closer to the GPS.
- Merging two placed pieces succeeds, and the later datum becomes a fix. Undo restores both
  placements.
- Unmerging restores the fix to a placement datum, and the detached piece stays where it was.
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
3. **5 m default accuracy.** Depth never takes a GPS fix's elevation into
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
| Solve | weighted fixes in the Closures Computed View | 2 |
| Entrances | Plan mode and Ariane export | 3 |

## Related

- A side finding from the Ariane check: a START with no datum of its own is written with the
  frame datum's GPS. If such a START has a loop back to another START, Ariane collapses that
  loop. Whether the exporter can produce that today has not been checked; it gets its own
  issue if it can.
- #688, the far-datum warning across separate pieces.
