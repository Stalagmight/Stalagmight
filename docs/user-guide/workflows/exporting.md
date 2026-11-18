---
title: Exporting
description: "Export a map image (PNG, SVG or PDF) or the survey data (Ariane TML or TMLU, CSV, JSON), and what an Ariane file keeps of your zero datums."
order: 11
feature: import-export
---

# Exporting

1. Click **Export** in the top bar (next to **Survey** and **Plan**) to switch
   to Export mode. Or open **Menu** > **Export** (`Ctrl+E`, `Cmd+E` on a Mac)
   to get the same options in a dialog.
2. Under **Export Type**, pick **Map Image** or **Survey Data**.
3. Pick the **Format** and the options, click **Export** and choose where to
   save the file.

Exporting never changes your project. If an export fails, a file you were
replacing stays as it was.

## Map image

- **Format**: **PNG**, **SVG** or **PDF**. For PNG, **Resolution** goes from
  **Low (72 DPI)** to **Print (600 DPI)**; the default is **High (300 DPI)**.
- The preview is exactly what will be written: large in the map area in Export
  mode, small under **Preview** in the dialog.
- The image always shows the whole survey, north up, with a small margin on a
  white background. It has no scale bar or north arrow. Sections hidden on the
  map are drawn too (see
  [Grouping lines into sections](grouping-lines-into-sections.md)).
- The survey is drawn as the Closures Computed View shows it (loops closed,
  bent through GPS fixes), whichever view you are in. A project with nothing to
  adjust is drawn as surveyed.
- **Line-weight threshold** (0 to 10, default 2): passages tied to at least
  this many others through reference shots are drawn thick, the rest thin.
  Reference shots themselves are not drawn.
- **Comments & Labels Options** (folded away at first): **Line names**,
  **Station IDs** (each station's Station ID), **Station names** (each
  station's name, instead of the IDs), **Station comments**, **Shot
  comments**, **Shot data** and **Penetration-distance labels** (on by
  default: the distance through the cave from the entrance, at the far end of
  each thick passage). See
  [Names and Station IDs](../concepts/sections-shots-stations.md#names-and-station-ids).
- **Color**: **None (monochrome)**, **By line**, **By depth** or **By
  distance**. Color is for PNG and PDF; SVG is always monochrome. Depth and
  distance use the palette chosen on the map, and a range control (**Depth
  colors** or **Distance colors**) sets where the colors start, end and bend.

## Survey data

- **TML (Ariane)**: a file for Ariane. In the save dialog, pick **TML (ZIP)**
  for a `.tml` file or **TMLU (XML)** for a `.tmlu` file. A name without an
  extension saves as `.tml`. See below for what it keeps.
- **CSV**: one row per shot: the line's name (`line`), the names of the
  shot's two stations (`from`, `to`) and their Station IDs (`from_id`,
  `to_id`), then length, bearings in and out, depths in and out, left, right,
  up, down, kind and comment.
- **JSON**: the project name and location, then every line with its name,
  people, dates, color and shots. Each shot has the same `from`, `to`,
  `from_id` and `to_id` as the CSV.
- CSV and JSON hold the shot readings only: no zero datums, merges or loop
  closures.

## What the Ariane file keeps

- Your measurements as surveyed (lengths, bearings, depths) and your loop
  closures, which Ariane adjusts itself. The Raw Data View or Closures Computed
  View on screen makes no difference to the file.
- Every zero datum is written as an Ariane START at its own GPS position, GPS
  fixes included. Ariane holds every START exactly where its GPS says and bends
  the survey between them to fit.
- A GPS fix part-way along a line gets a START of its own at its GPS, tied
  to its station, so Ariane holds the survey there too. This includes a fix
  with **Entrance** unticked: Ariane shows it as a START.
- A fix's **Accuracy** is not carried: Ariane pins every START exactly. See
  [Zero datums and GPS fixes](../concepts/zero-datums-and-gps-fixes.md).

### Lines without a zero datum

If some lines are not connected to a zero datum, a notice lists them before
the export: **Some lines aren't connected to a zero datum**. No GPS reading
places them, so they export at the spot the map shows. Nothing is left out.
Click **Continue** to export, or **Cancel** and
[set a zero datum](setting-a-zero-datum.md) on one of their stations.

With no zero datum at all, the notice is **No line is connected to a zero
datum**: the file is not georeferenced, so Ariane does not know where in the
world the cave is.

### Notes after the export

The file is written; a note says what it could not carry.

- **Some zero datums could not be written to the file**: for example two zero
  datums on the same point (one point holds one GPS position), or a zero datum
  with a GPS position that is not valid. The rest of the file is complete.
- **Bearings rotated to true north for Ariane**: your lines use different
  north references or declinations, and an Ariane file has only one, so every
  bearing is written as true north. Your project keeps its readings.
- **Some lines have no declination - Ariane will re-correct all**: the
  bearings are written as measured and Ariane applies its own declination.

### Importing the file back

[Importing the file into Stalagmight](importing-from-ariane.md) brings back
the stations and shots, but not the GPS fixes as zero datums: their stations
come back as ordinary stations. The [import report](../screens/import-report.md)
may also say that some loop closures were skipped and some lines dropped:
these are usually the ties to the GPS fixes, not your survey.

## Details

- Export mode is read-only: the right panel holds the export options, and the
  **Data** panel and **Display** menu are hidden. Click **Survey** to edit again.
- Export mode keeps your options while the project is open. The dialog starts
  from the defaults each time.
- An export cannot overwrite the open project file.
- Sending a project to SpeleoDB uploads the project itself, not an export.
