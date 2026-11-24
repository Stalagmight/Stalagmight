# RS-ARROW captures (loop-closure mark, blend across the arrow)

The user's request, looking at `rs-blend/table-shots-blue-light-1440.png`: "why does
table-shots-blue-link have two dots at the end (yellow and black). Also the color should blend
over in the middle of the -> arrow, not before it".

- Build: worktree `rs-arrow`, branch `rs/pill-arrow`, app built by the harness from this tree
  (`builtFrom.repoRoot` = `.claude/worktrees/rs-arrow`, `gitHead` 433188ed0).
- Data: the fully synthetic RS-BLEND survey. Main (#f6f6f2, near-white), Upper Branch
  (#101010, near-black) merged at Main 12, Lemon (#ffe600) merged at Branch 5, Blue Link
  (#1f4fff, one shot from Main 4 to a loop end on Lemon 3: blue head, near-white from half,
  yellow to half), Annex (#c2185b), Blue Spur (#1f4fff) leaving Lemon 5 (blue, yellow, blue).
- Window: 1440x900 and 860x900, dark and light themes.

## Full screenshots (`<surface>-<theme>-<width>.png`, 17 surfaces x 4)

| Surface | What it shows |
|---|---|
| table-shots-blue | The Blue Link table row the user asked about: blend across the arrow, loop mark `∽` |
| table-shots-branch, table-shots-lemon, table-shots-spur | Other two-tone shot rows (near-black/near-white, yellow/near-black, blue/yellow/blue) |
| table-stations-blue | Blue Link stations: 4 in Main's colour, the loop end 3 in Lemon's colour with `∽` |
| table-stations-lemon | Lemon's own 3, home colour, no mark |
| table-stations-two-12s | Branch stations, two 12s |
| details-foreign | Station details, Next shot (two-tone) |
| details-loop-end | Station details of the loop end: the three-colour Blue Link shot |
| warnings, warning-resolve-two-tone | Warnings tab shot pill, and a message whose prose names a two-tone shot |
| delete-shot-branch-first, delete-shot-blue-both-ends | Delete Shot dialog prose with two-tone shots |
| delete-shot-main | Delete Shot dialog prose with a one-colour shot |
| merge, detach, history | Dialog and history prose |

## Zoomed crops (`zoom/`)

- `before-after-blue-link-{light,dark}.png`: the Blue Link pill in the data table, in Station
  details and in Delete Shot prose, before (the RS-BLEND captures of the same seed and surface,
  89ebb56b1: ring loop mark, blend before the arrow) and after.
- `near-white-near-black-{light,dark}.png`, `yellow-blue-{light,dark}.png`,
  `white-yellow-on-blue-{light,dark}.png`: magnified link and prose pills of those pairs.
- `loop-end-station-vs-home-{light,dark}.png`: the loop-end station pill (3 with `∽`) under a
  foreign-slot pill, and Lemon's own home-coloured 3.
- `hover-rest-vs-hover-{light,dark}.png`: the three-colour pill at rest and hovered.
- `wrapped-prose-{light,dark}-860.png`: a two-tone prose pill wrapping inside its context.
- `split-pills-<theme>-<width>.png`: every split pill on the captured surfaces, magnified.

## Measurements

`measurements/measure-own-<theme>-<width>.json`:

| Combination | Compared blocks | Line gaps | Pitch deviation | Two-tone prose pills | Adjacent-line pairs | Overlapping |
|---|---|---|---|---|---|---|
| dark 1440 | 26 | 109 | 0 px | 14 | 65 | 0 |
| light 1440 | 26 | 109 | 0 px | 14 | 65 | 0 |
| dark 860 | 26 | 136 | 0 px | 14 | 90 | 0 |
| light 860 | 26 | 136 | 0 px | 14 | 90 | 0 |

- Arrow on the blend: the arrow glyph's centre lies within 0.29 px of its blend's midpoint on all
  10 link and prose arrows measured.
- Wrap: every prose shot pill of three Delete Shot dialogs (two with two-tone shots, one with a
  one-colour shot) was cloned into a box narrowed from 40 to 460 px in 2 px steps: 1266 layouts,
  971 of them wrapped, and the arrow was always on the line of the from end's last word and the to
  end's first word.
- Prose marks: the home-line dot and the loop mark sit 0.406em above the baseline (the
  inline-block dot they replace sat at 0.41em).
