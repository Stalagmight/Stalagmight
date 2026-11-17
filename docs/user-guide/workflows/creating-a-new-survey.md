---
title: Creating a new survey
description: "Create a new survey project in Stalagmight: name it, add a line by hand or import one, place it on the map and check the warnings."
order: 1
---

# Creating a new survey

1. On the [start screen](../screens/start-screen.md), click **New project**.
2. In the [New Project dialog](../screens/new-project-dialog.md), enter a name
   and click **Create Project**.
3. In the [survey view](../screens/survey-view.md), add data:
   [import a file](importing-from-mnemo.md) (for example a `.dmp` from an
   MNemo), or click **+** next to **Sections** to enter it by hand.
4. Entering by hand: in the
   [Import / Create Line dialog](../screens/create-section-dialog.md), name
   the line and set its surveyors, explorers, survey date and color.
5. Enter one row per shot and click **Create Line**.
6. Click the map to [place the line](../screens/placement-mode.md). Rough
   is fine, it can be moved later.
7. Optional: [set a zero datum](setting-a-zero-datum.md) to put the survey at
   its real GPS position.
8. Check the **Warnings** tab and [fix what it finds](fixing-warnings.md).

The project file is written the first time the project is saved, usually by
auto-save shortly after you add data. See
[when the file is written](../screens/new-project-dialog.md#when-the-file-is-written).

Already have the survey in another program? Use **Open / Import** on the start
screen instead.

## Step details

### 1. New project

![Start screen](../images/start-screen.png)

Click **New project** (2) or **Start a new project instead** (5). A dialog asks
for the project details.

### 2. Project details

![New Project dialog](../images/new-project-dialog.png)

- **Project Name** (2) is required. **Location (optional)** (3) is optional.
- **Create Project** (6) opens the new project. No file is written until the
  first save.
- The **Save project file immediately** checkbox (4) in the picture is no
  longer in the dialog.

### 3. Add data

![Survey view](../images/survey-view.png)

- **Import** (2) or **Import data** (13): read data from a file, for example a
  `.dmp` from an MNemo.
- **Add line** (12): start an empty line and enter shots by hand. See
  [Lines, shots and stations](../concepts/sections-shots-stations.md).

### 4. Line details

![Import / Create Line dialog](../images/create-section-dialog.png)

- **Line Name** (2) is optional. So are **Surveyors** and **Explorers**
  (lists of names: type a name and press **Enter**) and **Survey date**, which
  go on every shot you enter.
- **Color** (5) is how the line shows on the map.
- The picture shows an older layout with one **Surveyor** field (3) and a
  **Date** field (4). See the
  [dialog page](../screens/create-section-dialog.md) for the current fields.

### 5. Shots

![Shots table, scrolled down](../images/create-section-shots.png)

- Per shot: **Len** (9), **Az** (10), **Depth In** (11), **Depth Out** (12).
- **J/R** (13): leave blank for permanent line. Click it for `J` (jump) or `R`
  (reference). See
  [shot kinds](../concepts/sections-shots-stations.md).
- **Create Line** (16).
- LRUD, and azimuth or inclination at both ends of a shot, are extra columns
  under the eye icon. See the
  [dialog page](../screens/create-section-dialog.md).

### 6. Place the line

![Placement mode](../images/placement-mode.png)

- The first station (1) follows the pointer. Click to drop the line.
- Drag, scroll or zoom (3) the map to get to the right spot.
- Placement does not need to be accurate. The line can be moved later or
  anchored to a [zero datum](../concepts/sections-shots-stations.md).
- **Esc** (2) goes back to the dialog with what you entered.
