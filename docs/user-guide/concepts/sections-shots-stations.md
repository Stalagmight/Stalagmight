---
title: Lines, shots and stations
description: "Stalagmight's survey terms: stations, shots and their kinds (permanent, jump, reference), lines, LRUD, merging, loop closures, zero datums and GPS fixes."
order: 1
---

# Lines, shots and stations

- **Station**: a tie-off.
- **Shot**: the straight leg between two stations. One of three kinds:
  - **Permanent**: along permanent cave line.
  - **Jump** (`J`): temporary line the surveyor lays to connect permanent lines,
    or the main line to the surface.
  - **Reference** (`R`): a shot retaken along a stretch already surveyed, so
    two lines can be joined exactly, without ambiguity. On
    [import](../workflows/importing-from-mnemo.md), Stalagmight uses them to
    suggest where lines join. Survey length leaves references out, so the
    same passage is not counted twice.
- **LRUD**: left, right, up and down distances to the cave walls, i.e. the
  passage size.
- **Line**: a run of shots surveyed in one go, usually along one stretch of
  cave line, with its own name, color, surveyors and date. The **Sections**
  panel on the left lists the lines.
- **Section**: a named, colored group of lines, such as one region of the
  cave. A line is in at most one section. Sections organize the **Sections**
  panel and can be hidden on the map. See
  [Grouping lines into sections](../workflows/grouping-lines-into-sections.md).
- **Merging** two stations connects the lines they are on. That is how
  separate lines become one cave. See
  [Merging stations](../workflows/merging-stations.md).
- **Loop closure**: where survey lines meet up again, completing a loop. It is
  shown as a dashed line and does not move the measured survey. The adjusted
  view (in **Plan**, or **Closures Computed View** under **Display**) spreads
  the error around the loop. See [Closing a loop](../workflows/closing-a-loop.md).
- **Zero datum**: a station tied to a real-world GPS position. The first one
  on a connected piece of the survey places that piece, and its other stations
  are positioned relative to it. A later one on the same piece is a **GPS
  fix**: it moves nothing in the Raw Data View, and the adjusted view bends
  the survey through it. See [Zero datums and GPS fixes](zero-datums-and-gps-fixes.md)
  and [Setting a zero datum](../workflows/setting-a-zero-datum.md).

Add a line with **Add line** and merge stations with **Merge** in the
[survey view](../screens/survey-view.md). The shot kind is the **J/R** column
in the [Import / Create Line dialog](../screens/create-section-dialog.md).
