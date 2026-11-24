# RS-OWNERSHIP: pills in the colour of the line a station belongs to

Captured from the real app: the debug build of branch `rs/pill-ownership`
(worktree `.claude/worktrees/rs-prose`, `builtFrom.repoRoot` = that
worktree, gitHead b73a05f9e). It was driven through the agent harness via
tsx, with tauri-driver under `dbus-run-session` on Xvfb `:177`,
`NO_AT_BRIDGE=1`, satellite basemap off and autosave off.

The survey is synthetic, built in the app. Every station is named by its
index.

| Line | Colour | Stations |
|---|---|---|
| Main Passage of the Northern Sump System | near-white `#f6f6f2` | 0 to 14 |
| Upper Branch towards the Dry Chamber | near-black `#101010` | 0 to 13 |
| Lemon Squeeze Crawlway past the Second Restriction | yellow `#ffe600` | |
| Blue Link | blue `#1f4fff` | one shot |
| Annex beyond the Terminal Sump | magenta `#c2185b` | |

The junctions and the loop closure:

- **Branch's start** is merged onto Main's 12, and Main's is kept. Branch
  therefore shows two 12s: Main's at its start, and its own 12 further on.
- **Lemon's start** is merged onto Branch's 5.
- **Blue's start** is merged onto Main's 4.
- **Blue's end** closes a loop on Lemon's 3, and Lemon's is kept. Blue's end
  is a loop end of Lemon's station.

This gives these shots:

- Branch 12 → 1: near-white end on a near-black line.
- Lemon 5 → 1: near-black end on a yellow line.
- Blue 4 → 3: both ends foreign (white, then yellow, on a blue line).

## What it shows

**Station pills.** A station pill is in the colour of the line its station
belongs to: fill (link pills) or tint (prose), border and leading dot. The
context names that line too ("Main: 12" on Branch). There is no second dot.
The tooltip reads "Belongs to Main". The words a screen reader reads keep
"(Main)", or "(on Branch)" when the context already names Main.

**Shot pills.** A shot with an end of another line is two-tone. The split
falls before the arrow: the half holding that end is in that line's colours,
with that line's dot after the end's name. When the from end is the foreign
one, the leading dot and the context keep the shot's colour as a head
segment.

**Borders.** A near-white half and a near-black half have nearly the same
fill, because the pill band keeps text legible. So the border now tracks the
line's lightness (`BORDER_TRACK`):

| Theme | Near-white border | Near-black border |
|---|---|---|
| Dark | `#9d9d9d` | `#626262` |
| Light | `#bcbcbc` | `#7e7e7e` |

**Narrow details panel.** A link pill's context gives way first. It
ellipsizes ("Lemon Squeeze Crawlwa…"), and the station name and `#32` stay in
the pill. Line pills in the details rows ellipsize too, instead of running
past the panel.

## Measurements (in the running app)

Each block holding prose pills was measured twice on the same element: once
with pills and once as plain text (highlighting off). Line tops come from
`Range.getClientRects`, clustered into lines.

| Theme and width | Prose pills | Two-tone prose pills | Blocks compared | Line gaps | Largest deviation from line-height |
|---|---|---|---|---|---|
| dark-1440 | 46 | 6 | 18 | 66 | 0 px |
| light-1440 | 46 | 10 | 20 | 78 | 0 px |
| dark-860 | 46 | 10 | 20 | 97 | 0 px |
| light-860 | 46 | 10 | 20 | 97 | 0 px |

The dark-1440 run had fewer prose pills in the panels and dialogs it captured
than the later three runs. Every compared block still matched exactly.

Overlaps between tint fragments (pill tints and two-tone segment tints) on
different lines:

| Theme and width | Fragment pairs | Overlapping |
|---|---|---|
| 1440 | 44 | 0 |
| 860 | 61 | 0 |

At 12 px text in 16 px lines the fragment boxes touch (0 px). Both tints are
transparent at that edge: their half-alpha bands stay 4.4 px apart there, and
6.2 px apart in 20 px lines.

The superscript sits at most 0.41 px above the text box, inside the
half-leading.

The raw data is `measurements/measure-own-<theme>-<width>.json`: per surface
and block, the pitch with pills and as plain text, the overlap census, and
how many link pills are split or owner-coloured.

## Shots

Each surface has four files: `<surface>-dark-1440.png`,
`<surface>-light-1440.png`, `<surface>-dark-860.png` and
`<surface>-light-860.png`.

| Surface | What it proves |
|---|---|
| `table-stations-two-12s` | The data table (full screen, Name column on) with Branch's two 12s. Row 0's gutter pill is Main's colour with Main's `#12` (pale fill and border in light; pale border in dark). Row 12 is Branch's with `#27`. The Name column keeps "12 (Main …)". |
| `table-shots-branch` | Shot gutter, shot 12 → 1: a near-black head, a near-white "12" half with Main's dot, and a near-black "→ 1" half. |
| `table-shots-lemon` | Shot 5 → 1 on the yellow line: a yellow head, a near-black "5" half with Branch's ring dot, and a yellow "→ 1" half. |
| `table-shots-blue` | Shot 4 → 3 with both ends foreign: a blue head, a white "4" half with Main's dot, and a yellow "→ 3" half with Lemon's dot and the loop mark. |
| `details-foreign` | Station details of Branch's start. "12 belongs to Main" has the station pill in Main's colour. The Next shot pill is two-tone. The Line and "Also on" pills ellipsize. |
| `details-loop-end` | Station details of Blue's loop end. "Loop closure end, shown as [Lemon …: 3]". The closure's Direct link pill ellipsizes its long context and keeps "3 #32" (the truncation fix). The Previous shot pill is two-tone with both ends foreign. |
| `merge` | Merge Stations, Annex's 0 onto Main's 2. The keep pills and prose pills are in their owners' colours. The repeat sentence ends "the junction in [Main …]'s colour". |
| `detach` | Detach a line from Main's 12. The kept station is named in Main's colour. |
| `warnings` | The Warnings tab scrolled to Branch's first shot: the row's shot pill is two-tone. |
| `warning-resolve-two-tone` | The resolve view of that warning: the message's shot as a two-tone prose pill (two tints). |
| `delete-shot-branch-first` | Delete Shot lead with a two-tone prose shot. The impact pairs are slots of one station, so each keeps its own line's colour and name ("Branch: 12 <-> Main: 12"), as the "Also on" rows do. |
| `delete-shot-blue-both-ends` | Delete Shot lead "Shot [Blue Link: 4 → 3]" with both ends foreign: three tints. Pairs: "Blue Link: 4 <-> Main …: 4" and "Blue Link: 3 <-> Lemon …: 3". |
| `history` | Undo dropdown rows with prose pills in their owners' colours. |

`zoom/` holds 5x and 6x crops of the gutter pills, in dark and light:

- `shot-near-white-on-near-black-<theme>`
- `shot-near-black-end-on-yellow-<theme>`
- `shot-both-ends-foreign-white-yellow-on-blue-<theme>`
- `two-12s-row-0-<theme>` and `two-12s-row-12-<theme>`
