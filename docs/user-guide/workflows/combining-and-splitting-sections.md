---
title: Combining and splitting lines
description: "Join two lines that meet end to end into one with Combine Lines, or cut one in two with Split Line Here, and see what moves before you confirm."
order: 9
feature: editing
---

# Combining and splitting lines

**Combine Lines** joins two lines that meet end to end into one.
**Split Line Here** cuts one line into two without deleting anything.
Both work in **Survey** mode, in the **Raw Data View**, and one **Undo** puts
everything back.

## Combining two lines

1. In the [survey view](../screens/survey-view.md), right-click the station
   where the two lines meet. It must be the first or last station of a line.
2. Pick **Combine Line With** *name*... (for example **Combine Line With
   Side Passage...**). Where three or more lines end at the station, pick
   **Combine Lines...** and then pick the pair from the submenu.
3. Check the **Combine Lines** dialog and click **Combine**.

Or select two lines in the **Sections** panel (Shift-click the second),
right-click one and pick **Combine 2 Lines...**.

The note under the menu item says how the two ends meet: **Merged here**,
**Side by side here** (less than 1 mm apart, not merged; they are combined as
one point) or **Across a loop closure**. See the
[station context menu](../screens/station-context-menu.md).

### The Combine Lines dialog

Top of the dialog, what you decide:

- Which line is combined with which, at which station, and whether the
  other line is added before or after the kept one (reversed first when the
  two run against each other).
- Across a loop closure: how far apart the survey puts the two ends.
- How many connections the combine removes, if any (see below).
- **Keep** *name*: the combined line continues this one, in its direction.
  A button keeps the other line instead.
- **Combined line name** and its colour. The kept line's name is the
  default; the two halves of a split, `Main A` and `Main B`, give back `Main`.
  The name cannot be empty or the name of another line; the reason shows
  under the field and **Combine** stays off.
- When the other line is reversed: **Swap** *name*'s **left and right
  walls** (on by default).
- When both join stations have a comment: which one to keep.

**Details**, folded, lists everything else the combine changes:

- **Which side moves, and by how much.** Two ends tied by a loop closure are
  one point to the survey, so one side moves by the loop's misclosure to close
  it. Normally the other line moves, with every line merged to it. The
  kept line moves instead when a zero datum holds the other side, or when
  the other side holds the station the **Closures Computed View** is held at.
  The dialog says which and why.
- **Connections that become loop closures.** A merge or a snapped closure that
  would tie the combined line to itself (the two lines also meet
  somewhere else, or are part of a loop) becomes an ordinary
  [loop closure](closing-a-loop.md), and the loop's misclosure moves there.
  One into a part a zero datum holds becomes a loop closure too, and that part
  stays where it is.
- **Connections removed.** A merge, snapped closure or loop closure that would
  end up joining the two ends of one shot is removed: as a closure it would
  squeeze that shot to zero length. A warning names each one after the
  combine.
- **Closures replaced.** Two loop closures on the same two stations would count
  one loop twice; the combine keeps the one the **Closures Computed View**
  adjusts.
- Other lines merged at the join stay attached to it. The other line's
  bearings are converted to the kept line's north reference. The kept line's
  quality and direction win, and the joined station takes the position of the
  side that stays put.
- The joined station takes the average depth of the two ends; Details say
  so when they differ by more than 5 cm.
- Who surveyed the combined line, and when. Every shot keeps its own
  surveyors, explorers and date.

### Station names after a combine

The joined station keeps the kept line's name and Station ID. Every other
station keeps its Station ID, and its name too unless two names would clash
in the combined line: when a station of the other line has the name of a
station of the kept line, Stalagmight puts a letter in front of every name of
both lines:

- the kept line takes the first free letter, usually `A`: `3` becomes `A3`;
- the other line takes the next one, usually `B`: `1` becomes `B1`.

So `0 1 2 3` (kept) combined with `0 1 2`, joined at the kept `3` and the
other `0`, gives `A0 A1 A2 A3 B1 B2`. A line that is already lettered (every
name a capital letter and a number, such as `A3` or `B12a`) keeps its letters,
and the other line takes the next free one: combining that line with `0 1`
adds `C1`. Two lines with no names in common, neither lettered, keep every
name: `0 1 2 3` and `3 4 5` joined at `3` give `0 1 2 3 4 5`.

Rename stations afterwards if your notes use other names; **Undo** puts the
old names back with the rest of the combine. See
[Names and Station IDs](../concepts/sections-shots-stations.md#names-and-station-ids).

### When Combine is not offered

The menu item is greyed out, with the reason under it, when:

- no other line ends at the station, or the two selected lines do not
  meet: merge two of their ends, or [close a loop](closing-a-loop.md) between
  them, first;
- both lines are placed by their own zero datum: [merge](merging-stations.md)
  them first, so the later datum becomes a GPS fix (see
  [zero datums and GPS fixes](../concepts/zero-datums-and-gps-fixes.md));
- both join stations hold a zero datum: remove one first;
- the station is merged to the middle of another line, not to its end;
- rarer limits, such as a combined line of too many shots.

## Splitting a line

1. Right-click a station in the middle of a line (not its first or last
   station) and pick **Split Line Here**. Or right-click a point along a
   shot on the map and pick **Split Line Here**: a new station goes in
   there first, named with a letter after the shot's first station (`4a` on
   `4 → 5`). A point within 1 cm of a shot end splits at that end.
2. Name the halves in **Line A name (first half)** and **Line B name
   (second half)**, and pick their colours. The defaults are `Main A` and
   `Main B` (`Main A (2)` when another line already has that name). Each
   needs a name no other line has, and the two must differ; the reason shows
   under the field and **Split** stays off.
3. Click **Split**.

The two halves start side by side but are not connected. Merge the two cut
stations if routes and gas planning should continue across them.

What happens at the split station:

- Both halves have it, with the same name. The first half keeps its Station
  ID; the copy that starts the second half gets a new one.
- A merge there ties both halves.
- A loop closure there stays with the first half.
- A comment: choose **Keep the station comment on** A, B or **Both**.
- A zero datum: choose **Keep the zero datum on** A or B.

The dialog warns, before you split:

- When a loop closure across the split is all that would tie the halves, it
  becomes a merge, and the dialog says which half moves and how far.
- A loop closure whose two stations become one point is removed, and listed:
  "Loop closure Side / 0 to Main / 3 is removed: its two stations become one
  point."

A point inside a shot that is part of an adjusted survey network cannot be
split; the dialog says so.

## Deleting a shot and splitting

Deleting a shot in the middle of a line (one with at least three shots)
opens the **Delete Shot** dialog. **Split Line** there opens **Delete Shot &
Split Line**: name the two halves and click **Delete & Split**. **Shift
Geometry** instead deletes the shot and shifts the rest of the line after
it. Either way, one **Undo** brings the shot back.
