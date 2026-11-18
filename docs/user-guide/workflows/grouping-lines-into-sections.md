---
title: Grouping lines into sections
description: "Group lines into named, colored sections (regions of the cave) in the Sections panel: create, rename, drag lines in, hide a section on the map, color its lines, and how imports group lines for you."
order: 10
feature: editing
---

# Grouping lines into sections

A **section** is a named group of lines, such as one region of the cave
("Upper level", "Downstream sump"). Each line is in at most one section; the
rest are **Ungrouped**. Sections only organize the **Sections** panel and the
map: they change no survey data, no geometry and no statistics.

## Create a section

1. In the **Sections** panel, click **New Section** (the folder with a plus,
   next to **Add line**).
2. A row appears above the list, asking for the name: **Section 1** (or the
   next free number) is filled in and selected. Type a name and press
   **Enter**. Clicking elsewhere also creates the section, with the name in the
   field.
3. **Escape** cancels: no section is created.

Creating a section is one **Undo** step, already with its name.

Once the project has a section, the panel lists each section with its lines
under it, then **Ungrouped** (left out when every line is in a section). A
section header shows its color, name, how many lines it holds and their total
length.

Names are unique in a project. A name that is already taken, or an empty one,
is refused under the field before anything is created.

## Put lines in a section

- **Drag**: drag a line row onto a section header, or onto any line already in
  that section. The header lights up and a label says where the line goes.
  Drag onto **Ungrouped** to take a line out of its section (while you drag,
  **Ungrouped** is shown even when no line is in it). **Escape** cancels the
  drag.
- **Menu**: right-click a line, choose **Move to Section**, then a section,
  **No Section**, or **New Section...** (the same row asking for a name; the
  new section holds the line).
- **Several at once**: select lines with `Shift`+click or `Ctrl`+click, then
  drag any of them, or right-click one: the menu reads **Move N Lines to
  Section**.
- **New lines**: the [Import / Create Line dialog](../screens/create-section-dialog.md)
  has a **Section** field once the project has sections.

`Shift`+click a section header to select all its lines.

## Hide a section on the map

Click the eye on a section header. Its lines disappear from the map, with
their stations, labels, comments and closures, and can no longer be clicked
there. The panel still lists them, dimmed. Click the eye again to show them.

- The setting is saved with the project, on this computer only. It is not an
  edit: **Undo** does not change it, and it works in read-only views too.
- Selecting a hidden line in the panel shows that line on the map until you
  select something else. The section stays hidden.
- Statistics still count hidden lines.
- **Fit to View** frames the lines you can see, not the hidden ones (unless
  every line is hidden).
- Hidden sections stay hidden in **Plan** and **Export**; show them again from
  the **Sections** panel in **Survey**.
- An exported image leaves hidden sections out too, even a hidden line you
  have selected. Distances in the image (the penetration labels and the
  distance colors) still count the hidden lines.

## Color

- Click the dot on a section header to pick the section's color.
- Right-click the header, **Apply Color to Lines**: every line of the section
  takes the section's color, in one **Undo** step.

## Rename, collapse, delete

Right-click a section header for **Rename**, **Color...**, **Apply Color to
Lines**, **Zoom to Section** and **Delete Section**. Deleting a section keeps
its lines: they become ungrouped, and **Undo** puts the section and its lines
back.

Click the arrow on a header to collapse or expand it. Double-click a header to
zoom the map to the whole section.

## Keyboard

In the Sections panel, `Up` and `Down` move between rows. `Right` opens a
collapsed section, then steps into it; `Left` closes it, or steps from a line up
to its section. `Space` opens or closes a section, `Enter` zooms to it, and `F2`
renames it. `Shift+F10` opens the menu of the row.

## Sections from imports

Imports group lines for you when the file says how they belong together:

- **Ariane**: the lines one Ariane section was split into (`X`,
  `X (branch 2)`, ...) go into a section `X`, when there are two or more.
- **Compass**: the lines of one survey go into a section named after the
  survey, when the survey made two or more lines.
- **Walls**: the lines of a survey in a segment go into a section named after
  the segment.

If the project already has a section of that name, the lines join it.

## Details

- Editing sections, like any edit, works in **Survey** mode, in the **Raw Data
  View**, and not on a SpeleoDB project you are not editing. **Plan** and
  **Export** hide the Sections panel.
- **Add .stalx** and **Merge projects** bring the other project's sections
  along.
