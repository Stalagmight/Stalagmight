---
title: Data view
description: "The Data workspace: every section's stations and shots in one editable table, with links, colours, find and replace, filters and a map pane."
order: 16
---

# Data view

One spreadsheet with all the survey data of the open project. Choose **Data** in
the top bar (between **Survey** and **Plan**). You edit here exactly as in the
[Survey view](survey-view.md): what you change is the same project, and **Undo**
and **Redo** work as usual.

## The table

- Each section is listed in survey order: station 0, shot 1, station 1, shot 2,
  and so on to the last station, then the next section.
- Each section starts with a header row: its colour, name and number, its
  stations, shots and length, and where a branch section continues from. Click
  the arrow to collapse the section. Once the header has scrolled out of view,
  the section's colour and name stay in the top-left corner of the table.
- A row is named by its ID, as everywhere in the app: `0:5` is station 5 of
  section 0, `0:6` its shot 6. A station's name follows its ID; a shot shows
  the stations it runs between.
- Beside the names, the **link rail** draws a line through the section's
  stations and shots, a ring where a station is merged with another section,
  an arrow where a loop closes, and where a branch section starts from another
  one.
- A station's merges, closures and zero datum appear as chips across the shot
  columns of its row (a station has no shot values). The linked station's name
  is bold; click a chip to go there. With no shot column shown, a link count
  beside the row name lists them instead.
- An error or warning icon at the end of a row name marks a survey issue; on a
  shot, click it to resolve the issue in the Survey view. A lock marks a frozen
  shot.
- Hover a row name for its details. For a station this lists its merges and
  closures.
- The map button on a row shows that station or shot on the map.
- Rows are tinted like the map: by section colour, by survey issue (errors and
  warnings), by depth, by distance or by row type.
- Edit a cell as in the data table of the Survey view. **Delete** clears the
  selected cells.

## Finding and changing data

- **Find** and **Replace** search the table. Replace one match at a time or all
  at once; matches are highlighted. Find also looks at the row IDs, the section
  names, the link chips and the issue texts, which Replace leaves alone.
- Search only the rows and columns you see, or all of them: hidden matches are
  then shown for as long as you need them.
- Filter by row type, by section, by quick filters (issues, comments, links and
  more) or by your own conditions. Active filters show as chips you can remove.
- Show or hide columns, and section header rows. The station depth is hidden at
  first.
- Drag a column header to move a column. The shot columns stay together: other
  columns, such as Comment or the station columns, go before or after them.
- Click a column header to sort. While sorted, a bar says the survey order and
  the link rail are off; **Back to survey order** restores them.

## The map

- Open the **map pane** to see the map beside the table. It follows the row you
  are on. Drag the divider, or focus it and use the arrow keys, to resize it;
  double-click it to reset its width.
- The map tools and the [Display menu](display-menu.md) appear in the top bar
  while the map pane is open.
- **Open in Survey** switches to the Survey view at that station or shot. A
  **Back to Data** chip there takes you back to the same row.
- In the Survey view, **Open in Data** on the data table opens this view
  filtered to that section, at the same row.

## Details

- The Closures Computed View, or a SpeleoDB project you are not editing, makes
  the table read-only, as it does the map. Switch back to the Raw Data View in
  the Display menu to edit.
- Switching between Survey and Data keeps your Display settings and the view
  you were on.
- **Export** opens from the menu (or Ctrl+E); **Close export** returns here.
