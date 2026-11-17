---
title: Zero datums and GPS fixes
description: "Several GPS points in one survey: the placement datum that places each connected piece, GPS fixes that bend the computed survey, accuracy and entrances."
order: 3
---

# Zero datums and GPS fixes

A **zero datum** ties a station to a real-world GPS position. A survey can
have several, for example one at each entrance. What a zero datum does
depends on its role, which Stalagmight works out for you from the order you
set them in.

- **Placement datum**: the first zero datum on a connected piece of the
  survey (sections joined by merges). It places that piece: the station sits
  on its GPS and every other station of the piece follows from the shots.
  Pieces that are not merged to each other each have their own placement
  datum.
- **GPS fix**: every later zero datum on the same piece, such as a second
  entrance or a radiolocation point. It moves nothing in the Raw Data View.
  The Closures Computed View, Plan and Export bend the survey so that it
  passes through the fix's GPS.

The [Edit Station dialog](../screens/edit-station-dialog.md) and the
station's details show which role a zero datum has. On the map a GPS fix is
drawn as a hollow ring.

## What a GPS fix does

| Where | Effect |
|-------|--------|
| Raw Data View | Nothing moves. The station stays where the shots put it. |
| Closures Computed View | The survey is bent to pass through the fix's GPS, together with any [loop closures](../workflows/closing-a-loop.md). |
| Plan and map image export | They use the Closures Computed View, so they bend too. |
| Ariane export | The fix is written as a START at its GPS, and Ariane bends the survey to it. |

A GPS fix on its own is enough to make the Closures Computed View available
in the [Display menu](../screens/display-menu.md), with no loop closure.

## Accuracy

**Accuracy (m)**, from 0 to 1000, is set on GPS fixes only.

- **0**, the default, is a hard pin: the computed survey passes exactly
  through the GPS.
- A larger number trusts the GPS less, and the survey takes up most of the
  gap. Set it for a reading you trust less than the survey, such as a phone
  or smartwatch GPS (roughly 5 m) or a radiolocation point with a known
  uncertainty.

## Entrance

A zero datum marked **Entrance** is a way into the cave. Entrances are where:

- the **Distance** color in the Display menu measures from;
- Plan mode starts a dive from the entrance and measures penetration. See
  [Planning a dive](../workflows/planning-a-dive.md).

A placement datum is always an entrance. A GPS fix is an entrance by default
when it is on the first or last station of a section, and not when it is in
the middle of one (a radiolocation point, for example). You can change it for
a GPS fix in the Edit Station dialog.

## The far GPS fix warning

When a GPS fix is further from where the survey puts its station than
expected, Stalagmight warns you. Expected is 15 m plus 2% of the survey
length, along the shots, between the fix and the placement datum of its
piece. A fix 1400 m of survey from its placement datum may be up to 43 m
off.

The warning shows:

- in the Edit Station dialog as you type the GPS (you can still save);
- in the station's details;
- in the [Warnings tab](../screens/warnings-panel.md), on a shot next to the
  fix, with **Go to station**, **Edit zero datum** and **Ignore (keep value,
  don't warn again)**. Ignoring it holds for that GPS reading only: a new
  reading brings the warning back;
- above the map in the Closures Computed View, with **Show warnings**.

## How roles change

Roles follow the survey, so they can change when you edit it.

- **Removing the placement datum**: the next zero datum on that piece, in
  the order they were set, becomes its placement datum, and the piece moves
  so that station sits on its GPS.
- **Unmerging, or deleting part of a piece**: when a piece falls apart, the
  first zero datum of each new piece places it.
- **Merging two placed pieces**: the piece whose zero datum was set later
  moves to meet the other, and its zero datum becomes a GPS fix. The
  [Merge Stations dialog](../screens/merge-stations-dialog.md) says which
  piece moves and by how much.
- **Editing a GPS**: for any placement datum, its piece moves to the new GPS.
  For the first zero datum of the project, every other placed piece stays on
  its own GPS. For a GPS fix, nothing moves in the
  Raw Data View. Changing only **Accuracy (m)** or **Entrance** moves nothing.

## Details

- **Export to Ariane**: every zero datum, placement datum and GPS fix alike,
  is written as a START with its own GPS, also a fix that is not an entrance.
  A fix part-way along a section gets a START of its own at its GPS, tied in
  to the fix station. Ariane holds every START exactly, whatever its
  accuracy. See [Exporting](../workflows/exporting.md).
- **Older versions of Stalagmight** keep only the first zero datum of each
  piece, and drop the GPS fixes and accuracies when they save the file.
  Saving a project that holds any shows a reminder once per session.
- **Files from older versions**: where a merge joined two pieces each placed
  by its own zero datum and the merged stations sit apart, opening the file
  turns that merge into a loop closure, and each piece keeps its own
  placement. Merge them again to join them; the later zero datum then becomes
  a GPS fix.
