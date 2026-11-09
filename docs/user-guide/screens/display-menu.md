---
title: Display menu
description: "The Display menu controls what the survey map shows: color modes, grid and satellite, map elements, comments and labels, and the adjusted view."
order: 15
---

# Display menu

What the map shows. Opened by **Display** in the top right of the
[survey view](survey-view.md). Highlighted options are on.

![Display menu](../images/display-menu.png)

1. **Color**, one at a time:
   - **Section**: each section in its own color.
   - **Depth**: color by depth.
   - **Distance**: color by distance along the cave from the entrance. Click a
     station to measure from there instead.
   - **Survey Issues**: red for errors, yellow for warnings, green for clear.
     See [Errors and warnings](../concepts/errors-and-warnings.md).
2. **Overlays**: **Grid** and **Satellite** imagery.
3. **Map elements**: **Stations**, **Shots**, **Jumps**, **References** and
   **Zero datum** stations.
4. **Comments & labels**: **Section names**, **Station labels** (station IDs),
   **Station comments**, **Shot comments** and **Shot data** labels.
5. **Geometry**:
   - **Raw Data View**: the survey as measured.
   - **Closures Computed View**: corrected by the
     [loop closures](../workflows/closing-a-loop.md). Read-only.

## Details

- **Grid** adds a **Grid spacing** setting.
- **Closures Computed View** is unavailable when there are no loop closures to
  adjust. Edit in the **Raw Data View**.
- The choices are remembered between sessions, except **Distance** and
  **Survey Issues** coloring.
