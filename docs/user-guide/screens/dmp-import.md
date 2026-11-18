---
title: DMP import
description: "Every step of importing an MNemo DMP file: line selection, zero datum, suggested connections from reference shots, and placing the rest by hand."
order: 9
---

# DMP import

Imports an MNemo `.dmp` file into the open project, in steps. Opened by
**Import** in the [survey view](survey-view.md), or by **Import...** after
**Import from MNemo...** reads a device over USB. With no project open, the
[Create Project for Import dialog](create-project-for-import.md) comes first.

Each station is named after its MNemo station number, as in your notes, and
the wizard already shows stations by those names, with their line where
needed: `AC1 / 13`. A station of a row you add in the wizard has no MNemo
number and is named by the
[usual rules](../concepts/sections-shots-stations.md#names-for-new-stations).

## Step 1: pick lines

Each section recorded on the MNemo becomes one line in Stalagmight.

![DMP import, step 1](../images/dmp-import-step1.png)

1. **Close**: nothing imported.
2. **Surveyors** and **Explorers**, side by side: the people for every imported
   line. Type a name and press Enter to add it. They start with the people
   you used last in this session. (The screenshot shows an older single
   Surveyor field.)
3. **Date Override**: one date for every line, instead of the dates in the
   file.
4. **Select All**, **Select None**, **Invert**
5. An empty line. It has no shots and can't be picked.
6. Tick the lines to import.
7. **Cancel**: same as 1.
8. **Continue to Line Setup**

### Details

- Lines with two or more shots start ticked. A one-shot line starts
  unticked but can be ticked.
- Lines already imported into this project are marked **(already imported)**
  and left unticked. A whole file imported before gets a warning at the top.
- The box at the top shows the file, and the number of shots, total length and
  depth range of the ticked lines.
- **Direction**: whether the line was surveyed going in or out.

## Step 2: zero datum (optional)

Only shown when the project has no
[zero datum](../concepts/zero-datums-and-gps-fixes.md) yet.

![DMP import, zero datum line](../images/dmp-import-zero-datum.png)

1. **Close**: nothing imported.
2. Help on zero datums.
3. Click the line that should carry the zero datum. Next you pick its
   station and GPS position.
4. **Cancel**: same as 1.
5. **Back**: to step 1.
6. **Skip Zero Datum**: import without one. It can be
   [set later](../workflows/setting-a-zero-datum.md).

## Step 3: zero datum station and position

After picking a line in step 2.

![DMP import, configure zero datum](../images/dmp-import-zero-datum-config.png)

1. **Close**: nothing imported.
2. Preview. The ring marks the zero datum station. Click a station to pick it.
3. **Zero datum station**, by name.
4. **GPS coordinates**, in any [GPS format](edit-station-dialog.md#gps-formats).
   The badge shows the format it was read as.
5. The position as read from 4.
6. **Depth below surface**: optional, in meters, positive below the surface.
7. **Name**
8. **Surveyors** and **Explorers**, side by side. They start with the people
   from step 1. (The screenshot shows an older single Surveyor field.)
9. **Reverse numbering**: run the line from the other end (ticked here), so
   its last station comes first. The cave does not move, and every station
   keeps its MNemo name.
10. **Cancel**: same as 1.
11. **Back**: to step 2.
12. **Confirm Zero Datum**: disabled until 4 can be read and 6 is empty or a
    number.

Scrolled down, the line's details and shots:

![DMP import, configure zero datum, shots](../images/dmp-import-zero-datum-shots.png)

13. **Survey date** (labelled Date in the screenshot)
14. **Dir**: surveyed going in or out.
15. **Color**
16. **Shot Data**: every shot of the line. Values can be changed here, or
    any time later.
17. A ⚠ in this column marks a shot with a warning. Hover it to read it.
18. Choose visible columns.
19. Enlarge the table.

### Details

- MGRS and What3words positions are recognised but can't be converted: enter
  the position in another format.
- A note appears under 5 when latitude and longitude look swapped.

## Step 4: connection wizard

Stalagmight matches [reference shots](../concepts/sections-shots-stations.md)
between lines and suggests where each line joins, one at a time.

![DMP import, connection wizard](../images/dmp-import-suggestion.png)

1. **Close**
2. Lines placed so far.
3. Preview: grey is already placed, red is the line being placed, the gold
   dot is where they join.
4. The suggested connection: which station joins which, each with its line.
5. Confidence (high, medium or low) and how many shots matched. The arrow shows
   whether they run the same way (→) or opposite ways (←).
6. **Merge at**: pick another station pair, when there is more than one.
7. **Overlap**: what to do with shots that were surveyed twice, see below.
8. **Line Settings**, folded: click it for the line's name, people,
   **Reverse numbering**, survey date, direction and color.
9. **Cancel**: asks whether to keep or remove lines already imported.
10. **Back**: starts the import over. Lines this wizard already imported are
    removed first, which can be undone.
11. **Skip**: leave this line for now and place it by hand at the end.
12. **Confirm**: accept the connection.

**Overlap**:

- **Interpolate**: keep one copy, averaging the two.
- **Use import data** (default): keep one copy, with the new values.
- **Use existing data**: keep one copy, with the values already in the project.
- **Keep both (reference)**: keep both copies. The new ones become reference
  shots.

### Details

- When other matches were found, a link such as **Show 2 alternatives** lists
  them. Click one to use it instead.
- Scroll down for the line's shot table. Edit it here if needed, or later.
- **Interpolate** and **Use import data** are greyed out when the existing shots
  are pinned on both sides, by a zero datum, a merge or a closure.
- With no shots surveyed twice, **Overlap** says so instead of offering a choice.
- The overlap choice is remembered for later imports.

## Step 5: place the rest by hand

Lines with no suggested connection, and skipped ones, are placed by clicking
the map in [placement mode](placement-mode.md), which starts on its own.
Lines joined to each other move together.

It does not need to be accurate. Move them later, or connect them once the
survey has the data to do it.

![DMP import, placing lines by hand](../images/dmp-import-manual-placement.png)

1. The station under the pointer. Click the map to drop the lines there.
2. The lines being placed, dashed.
3. Lines already placed, solid.
4. **Esc**: back to the wizard, for
   [other ways to place them](#step-6-other-placement-options).

A plain drag does not move the map here: pan with Ctrl+drag or a middle-button
drag, and zoom with the scroll wheel.

## Step 6: other placement options

**Esc** in step 5 opens the options for the segment, a group of joined lines
still to place.

![DMP import, placement options](../images/dmp-import-placement-options.png)

1. **Close**
2. Lines placed so far.
3. Preview of the segment. The ring is the anchor station, the one that lands
   where you click. Click another station to change it.
4. Which segment, and its lines.
5. **From existing station**: join it to a station already in the project.
   Pick the segment's station, then the target **Line** and **Station**.
6. **Manual placement**: pick the anchor station and click **Place on map**,
   as in step 5.
7. **Do not place**: leave this segment out of the import.
8. **Line Settings**: name and other details.
9. **Cancel**
10. **Back**
11. **Next** when **Do not place** is picked, as here. Otherwise
    **Place segment**.

### Details

- **Suggested connection** is offered too, when a suggestion for this segment
  was skipped.
- **Skip (place later)** appears when more than one segment is waiting.
- After the import, **Connect Orphaned Lines** may open. It suggests where
  lines that are not tied to a zero datum join ones that are. The
  [import report](import-report.md) may follow.
