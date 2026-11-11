---
title: Errors and warnings
description: "The difference between errors and warnings in Stalagmight, which checks are errors, and the per-project thresholds that decide when warnings appear."
order: 2
---

# Errors and warnings

- **Error** (red): the data is impossible, for example a shot shorter than its
  depth change. Always fix errors.
- **Warning** (yellow): values that should be close are far apart, for example
  azimuth in and out more than a few degrees apart. The shot may need to be
  retaken more carefully.

Stalagmight still draws the survey as well as it can while errors are open.
Both are listed in the [Warnings panel](../screens/warnings-panel.md).

## Thresholds

When a warning appears is set per project in **Project Settings** (project menu,
or the **Details** tab). **Settings > Validation** holds the defaults for new
projects:

- Minimum shot length, maximum shot length
- Max depth change per shot
- Azimuth in/out difference
- Pitch in/out difference
- Inclination vs depth

## Details

- Errors: zero or negative length, azimuth or inclination out of range,
  negative LRUD, a value that is not a number, length shorter than the depth
  change, more than one zero datum on connected sections.
- Warnings: length shorter or longer than the limits, a large depth change,
  azimuth in and out too far apart, inclination in and out too far apart,
  inclination that disagrees with the depth change.
- Errors can only be fixed. Warnings can also be ignored.

## How far over

**Survey Issues** coloring and the **Errors** labels in the
[Display menu](../screens/display-menu.md) show how far over a threshold a shot
is, not just whether it is. A shot is green when its readings agree, yellow at
the threshold, and red at three times the threshold. An error is always red.
Ignoring a warning takes that difference out of the shot's color and label.
