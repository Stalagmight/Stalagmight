---
title: Station context menu
description: "The menu that opens when you right-click a station: edit it, merge, add shots, set a zero datum, combine or split sections, or delete."
order: 6
---

# Station context menu

Right-click a station on the map in Survey mode.

![Station context menu](../images/station-context-menu.png)

1. The station you right-clicked.
2. **Edit Station**: opens the [Edit Station dialog](edit-station-dialog.md).
3. **Merge With...**: then click the station to merge with. See
   [Merging stations](../workflows/merging-stations.md) and
   [Closing a loop](../workflows/closing-a-loop.md).
4. **Add Shot Before**: first station only. The last station has **Add Shot
   After** instead.
5. **Set Zero Datum**: tie this station to a GPS position. See
   [Setting a zero datum](../workflows/setting-a-zero-datum.md).
6. **Edit Section**
7. **Reverse Section**
8. **Delete Section**
9. **Delete Station**

The picture predates the combine and split entries. The menu now also has, just
above **Edit Section**:

- **Combine Sections...** (first or last station of a section): join two
  sections that meet end to end at this station. With exactly one other section
  ending there it reads **Combine Section With** and that section's name; with
  more, it opens a list of pairs. Under the label it says how the two meet, or
  why they cannot be combined (for example when no other section ends there).
- **Split Section Here** (any station between the first and the last): cut the
  section in two at this station.

See [Combining and splitting sections](../workflows/combining-and-splitting-sections.md).

## All entries

In menu order, with when each one shows:

| Entry | Shows |
|---|---|
| **Edit Station** | Always |
| **Merge With...** | Always |
| **Unmerge Stations** | Station is merged with another |
| **Remove Closure** | Station is on a loop closure |
| **Add Shot Before** | First station of the section |
| **Add Shot After** | Last station of the section |
| **Set Zero Datum** | Always |
| **Combine Sections...** | First or last station of the section |
| **Split Section Here** | Any other station |
| **Edit Section** | Always |
| **Reverse Section** | Always |
| **Reverse 2 Sections** (and so on) | Several sections selected, this one among them |
| **Delete Section** | Always |
| **Delete Station** | Always |

## Details

- **Unmerge Stations**: see
  [Unmerging stations](../workflows/merging-stations.md#unmerging-stations).
  **Remove Closure**: see
  [Removing a closure](../workflows/closing-a-loop.md#removing-a-closure).
- Both deletes ask for confirmation first.
- In the Closures Computed View, or on a SpeleoDB project you are not editing,
  the entries are shown but disabled.
- Right-clicking a point along a shot gives the shot menu, which also has
  **Split Section Here**: the split is made at that point.
- In Plan mode a right-click on a station gives the route menu instead:
  **Set as start point**, **Set as way point**, **Set as endpoint**,
  **Set as stage drop** (on the planned route only; **Set as stage 1 drop**,
  **Set as stage 2 drop** and so on with several stages) and
  **Set marked point**.
  See [Planning a dive](../workflows/planning-a-dive.md). Export mode has no
  station menu.
