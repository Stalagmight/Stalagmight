---
title: Lines, shots and stations
description: "Stalagmight's survey terms: stations and their names and IDs, shots and their kinds, lines, LRUD, merging, loop closures, zero datums and GPS fixes."
order: 1
---

# Lines, shots and stations

- **Station**: a tie-off. It has a **name**, such as `17`, `4a` or `A3`,
  unique within its line, and a **Station ID**, a number unique in the
  project. See [Names and Station IDs](#names-and-station-ids).
- **Shot**: the straight leg between two stations, named by them: `4 → 5`.
  One of three kinds:
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
  cave line, with its own name, color, surveyors and date. Every line has a
  name, and no two lines share one. The **Sections** panel on the left lists
  the lines.
- **Section**: a named, colored group of lines, such as one region of the
  cave. A line is in at most one section. Sections organize the **Sections**
  panel and can be hidden on the map. See
  [Grouping lines into sections](../workflows/grouping-lines-into-sections.md).
- **Merging** two stations connects the lines they are on. That is how
  separate lines become one cave. The merged station is the one you keep,
  shown on every line through it; the other is archived with its name and
  comment, and **Detach Line from Station** brings it back. See
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

## Names and Station IDs

Stalagmight names things the way you read them in your notes: a line by its
name, a station by its name, a shot by its two stations.

| What | Shown as | Unique within |
|---|---|---|
| Line | `Main Line` | the project |
| Station | `17`, or with its line `Main Line / 17` | its line |
| Shot | `4 → 5`, or with its line `Main Line: 4 → 5` | its line |
| Station ID | `512` | the project |

- **Station name**: two lines can each have a station `3`, so where the line
  is not clear from the context, the station is shown with it:
  `Main Line / 3`. A station keeps its name when shots are added, deleted or
  reversed around it, and when its line is split or combined (except for the
  letters a combine can add, see
  [Combining and splitting lines](../workflows/combining-and-splitting-sections.md#station-names-after-a-combine)).
- **Renaming a station**: in the
  [Edit Station dialog](../screens/edit-station-dialog.md), or in the
  **Name** column of the **Data** table's **Stations** view (hidden at first:
  show it with **Choose visible columns**). The name cannot be empty or one
  another station of the line already has. **Undo** puts the
  old name back.
- **Station ID**: a number Stalagmight gives every station when it is
  created, unique in the project. Renaming the station keeps it. It cannot be
  edited. **Details**, the Edit Station dialog and the **Station
  ID** column of the **Data** table show it, and the **Station IDs** label in
  the [Display menu](../screens/display-menu.md) draws it on the map.
- **Line name**: required and unique in the project. **Add line** suggests
  the next free `Line 1`, `Line 2`, ... A dialog that names a line refuses an
  empty name or one another line has, and says why under the field. An import,
  or adding another project, renames a line whose name is taken: `Main`,
  `Main (2)`, `Main (3)`.

### Names for new stations

A station you add is named after its neighbours, and you can rename it
afterwards:

- **At the end of a line** (**Add Shot After**): the previous name plus one,
  `5` -> `6`, `A3` -> `A4`, skipping names the line already has.
- **In the middle** (**Insert Station...**, a shot inserted before or after
  another in a shot table, **Split Line Here** inside a shot): the previous
  name plus the first free letter. After `2` that is `2a`, then `2b`; **Insert
  Station...** on `4 → 5` gives `4a`. A name with no digit takes the letter
  after a hyphen: after `Junction` comes `Junction-a`, then `Junction-b`.
- **At the start** (**Add Shot Before**): the next number the line has not
  used: on a line `0` to `6`, the new first station is `7`; on `A0` to `A3`,
  it is `A4`.
- **New and imported lines**: a line entered by hand is numbered `0`, `1`,
  `2`, ... from its first station. An import keeps the station names in the
  file (an [MNemo import](../workflows/importing-from-mnemo.md) uses the MNemo
  station numbers) and names any others by the rules above. A name the line
  already has gets a letter: a second `5` becomes `5a`, a second `Sump`
  becomes `Sump-a`. A station a line shows in another line's colour (a
  merged station kept on its own line) or with the loop mark (a loop
  closure end) does not take up its name on that line: the line may have a
  station of its own with that name too.

### Labels at merged stations

A merged station is one station: the one the merge kept, with its name and
Station ID on every line through it. Where a side line was merged onto
station `Junction` of the main line and `Junction` was kept, the side line
shows `Junction` too, its pill in the main line's colour and named by it
("Main: Junction"), which says the station belongs to another line (hover
it: "Belongs to Main"; the data table's Name column reads `Junction (Main)`).
A shot from that station is drawn in two colours, split at the arrow: the
half with `Junction` in the main line's colour, the rest in the side line's. The side line's own `0` is archived: it is
listed under **Archived here** in the station's **Details**, its name is free
to use again on the side line, and **Detach Line from Station** brings it
back. The **Station IDs** and **Station names** labels draw the kept station
once.

A loop closure can show its two ends as one station in the same way: the
hidden end keeps its own point on the map, but gets no label, and its slot
carries a small loop mark. **Remove Closure** brings its name back. Its
**Details** show the station it is shown as, then its own point's position;
a Plan pin on it names it by its own station first: `Loop / L8 (loop end,
shown as Main / 8)`.
