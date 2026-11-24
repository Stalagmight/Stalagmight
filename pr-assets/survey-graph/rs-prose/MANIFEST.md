# RS-PROSE: prose pills in running text

Captured from the real app: the debug build of branch `rs/prose-pills`
(worktree `.claude/worktrees/rs-prose`, `builtFrom.repoRoot` = that
worktree, gitHead 37d7f293a; the Rust and frontend sources of the screenshots
are unchanged at the branch head). It was driven through the agent harness
via tsx, with tauri-driver under `dbus-run-session` on Xvfb `:177`,
`NO_AT_BRIDGE=1`, satellite basemap off and autosave off.

The survey is synthetic, built in the app. It has seven lines:

| Line | Colour |
|---|---|
| "Main Passage of the Northern Sump System" | near-white `#f6f6f2` |
| "Upper Branch towards the Dry Chamber" | near-black `#101010` |
| "Lemon Squeeze Crawlway" | yellow `#ffe600` |
| "Blue Restriction Series" | blue `#1f4fff` |
| "Grey Side Tunnel" | grey `#8a8a8a` |
| "Annex beyond the Terminal Sump" | magenta `#c2185b` |
| "Far Western Extension Pending Survey" | green `#2e7d32` |

It has long station names ("Junction at the Great Collapse", "Chimney Base
below the Dry Chamber"), four foreign junction slots, one loop end and two
loop closures.

The conversion notice is a synthetic `ConversionNoticeView` put into the
store, because no old-format file is involved. Its parts name the survey's
real lines and stations, and it is rendered by the real banner.

## Design chosen: a soft radial tint, not a pill shape

The first design was a bordered fill per line fragment. Measured on 19
surfaces (dark theme, 1440x900):

- An inline fill covers the font's whole content area: 19 px in a 20 px line
  at 14 px text, and 16 px in a 16 px line at 12 px.
- 69 pairs of pills sat on adjacent lines. 54 of them (78%) were 1 px or less
  apart, and 2 touched (0 px).
- 77 of 94 superscript Station IDs rose up to 2.06 px above the fill.

Two lines of pills read as one block, so, as the user asked, the prose pill
now has no fill shape and no border. Instead it is a `radial-gradient` of the
line's tint:

- solid to 45% of its radius, transparent at the edge of each line fragment;
- a translucent overlay, so it works on the panel, accent and warning boxes,
  and toasts;
- its half-alpha band is 13.8 px of a 19 px fragment, so two stacked lines'
  tints stay 6.2 px apart in 20 px lines and 4.4 px apart in 16 px lines.

The dot, the muted context, the home-line dot, the loop mark and the
superscript `#id` are unchanged.

## Line pitch, measured in the running app

Each block holding a pill was measured twice on the same element: once with
pills (Full highlighting) and once as plain text (highlighting off). Line tops
come from `Range.getClientRects` over the text, clustered into lines.

Across 4 combinations (dark and light, at 1440x900 and 860x900) there were
103 blocks and 141 pills. A block counts as compared when it wraps and stayed
the same element both ways.

| Width | Blocks compared | Line gaps | Deviation from line-height |
|---|---|---|---|
| 1440 | 38 | 114 | 0 px |
| 860 | 37 | 124 | 0 px |

- The toast keeps a 20 px pitch in all four combinations.
- 9 blocks at 1440 and 10 at 860 were rebuilt by React when the highlighting
  level changed, so they were not compared.
- The superscript sits at most 0.41 px above the text's content box, inside
  the half-leading.
- The raw numbers are in `measurements/`:
  - `measure-tint-<theme>-<width>.json`: per surface and block, with the
    pitch both ways;
  - `measure-pill-dark-1440.json`: the rejected bordered design;
  - `toast-result.json`.
- `e2e-playwright/prose-line-pitch.spec.ts` repeats the check in CI on the
  Bulk Delete summary and a Warnings tab message, within 0.5 px.

## Shots

Each surface has four files: `<surface>-dark-1440.png`,
`<surface>-light-1440.png`, `<surface>-dark-860.png` and
`<surface>-light-860.png`.

| Surface | What it proves |
|---|---|
| `details-foreign` | Station details of a foreign junction slot. "belongs to" names the station and its home line as prose pills, the "Also on" links stay link pills, and the Closures sentence wraps. |
| `details-loop-end` | Station details of a loop end. "Loop closure end, shown as [Main ... : 10 #10]" wraps across two lines. The superscript and the near-white line's ring dot are in the 12 px Closures sentence. |
| `warnings` | Warnings tab. The junction-depth warning's station (from `messageParts`) is a prose pill after the row's shot pill. Shot warnings keep their shot pill. |
| `merge` | Merge Stations: the renamed lines, "is archived with its comment", "Detach ... brings ... back", name repeats, the move sentence and the depth readings. Pills sit in the accent and warning boxes with no smudge. |
| `closure` | Create Loop Closure, opened through a merge between connected stations: its keep picker and outcome sentences. |
| `detach` | Detach a Line from This Station: the lead with three pills, and "keeps its shots". "Upper Start (Station ID 11)" stays plain: the preview gives it no line. |
| `remove-closure` | Remove Closure: what comes back, and the loop-end sentences. |
| `bulk-delete` | Delete Shots summary (two stations and a line), and the datum and comment sentences. |
| `delete-shot-choice` | Delete Shot choice: the lead names the shot as a shot pill. |
| `delete-line` | Delete Line confirm: the quoted line as a line pill. |
| `split-line-here` | Split Line Here summary. |
| `combine` | Combine Lines: the summary, plan, rename note and closure note, and the wall-swap checkbox label. The "Keep ... instead" button stays plain. |
| `reverse` | Reverse Line: "will run from X to Y" per line. |
| `swap-comments` | Swap Comments: both leads. |
| `line-adjust` | Line Adjust: its existing station pills, shown for completeness. |
| `insert-station` | Insert Station lead: the shot. |
| `shown-identity` | The shown-identity prompt inside Edit Station on a foreign slot. "[Junction ...] belongs to [Main ...]" uses pills; the Rename button stays plain. |
| `history` | The undo dropdown: "Merge [Far Western ...: West Tie-in #46] into [Main ...: 10]" and "Set comment on station [Annex ...: Annex Dive Base #41]", from `descriptionParts`. Rows are clamped at three lines. |
| `conversion-notice` | The conversion banner: three clauses from `change_parts`, wrapping across the banner's lines. |
| `toast-identity` | A real identity notice: the toast "[Upper Branch ...: Upper Start] (Station ID 11) is shown again." after deleting Main (undone afterwards). The live region reads the plain sentence. |

Other files:

- `rejected-bordered-pill-details-dark-1440.png`,
  `rejected-bordered-pill-zoom-14px-lines.png` and
  `rejected-bordered-pill-zoom-12px-lines.png`: the first design. Stacked
  fills touch, and the `#id` sticks out above the fill.
- `final-tint-merge-zoom-dark.png` and `final-tint-detach-zoom-light.png`:
  2x and 3x crops of the chosen design.
