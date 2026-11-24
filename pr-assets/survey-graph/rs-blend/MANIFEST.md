# RS-BLEND captures (blended two-tone pills)

The user's request: "for the multi-color pills, can you blend the color transition".

- Build: worktree `rs-blend`, branch `rs/pill-blend`, app built by the harness from this
  tree (`builtFrom.repoRoot` = `.claude/worktrees/rs-blend`, `gitHead` 91cd04e59). Later
  commits on the branch (fdc5a6ce2, 89ebb56b1) touch docs and a code comment only.
- Data: fully synthetic survey (the RS-OWNERSHIP seed): Main (#f6f6f2, near-white),
  Upper Branch (#101010, near-black) merged at Main 12, Lemon (#ffe600) merged at Branch 5,
  Blue Link (#1f4fff, one shot from Main 4 to a loop end on Lemon 3: head blue, from half
  near-white, to half yellow), Annex (#c2185b), plus Blue Spur (#1f4fff) leaving Lemon 5
  (head blue, from half yellow, to half blue).
- Window: 1440x900 and 860x900, dark and light themes.

## Full screenshots (`<surface>-<theme>-<width>.png`, 14 surfaces x 4)

| Surface | What it shows |
|---|---|
| table-stations-two-12s | Stations table of Branch, two 12s (owned station pills, no split) |
| table-shots-branch | Shots table of Branch: `12 (Main) -> 1`, near-black shot with a near-white from half |
| table-shots-lemon | Shots table of Lemon: yellow shot with a near-black from half |
| table-shots-blue | Shots table of Blue Link: blue head, near-white from half, yellow to half |
| table-shots-spur | Shots table of Blue Spur: blue head, yellow from half, blue to half |
| details-foreign | Station details, Next shot is a two-tone link pill |
| details-loop-end | Station details of the loop end, the three-colour Blue Link shot |
| merge, detach | Merge and Detach dialogs (prose pills) |
| warnings | Warnings tab with a two-tone shot link pill |
| warning-resolve-two-tone | Warning message whose prose names a two-tone shot (wraps at 860) |
| delete-shot-branch-first | Delete Shot dialog, prose pill of the Branch shot |
| delete-shot-blue-both-ends | Delete Shot dialog, prose pill of the three-colour Blue Link shot |
| history | History rows with prose pills |

## Zoomed crops (`zoom/`)

- `before-after-light.png`, `before-after-dark.png`: the same pills before (the
  RS-OWNERSHIP captures of this seed, base 8da2fb499: hard seam, two separate prose tints)
  and after (RS-BLEND), magnified: Blue Link in the table and in Station details, the Delete
  Shot prose pill, the Branch shot.
- `near-white-near-black-{light,dark}.png`: Branch shot link pills (table, details) and its
  prose pill, 8x / 4x.
- `yellow-blue-{light,dark}.png`: Blue Spur (blue, yellow, blue) and the Lemon shot, 8x.
- `white-yellow-on-blue-{light,dark}.png`: Blue Link (blue head, near-white, yellow) in
  details and the table, and its prose pill, 8x / 5x.
- `hover-rest-vs-hover-{light,dark}.png`: the three-colour pill at rest (top) and hovered
  (bottom), 6x.
- `wrapped-prose-{light,dark}-860.png`: a two-tone prose pill whose context wraps inside the
  head and at the seam: each line fades at its own ends, the blend only on the seam's line.
- `split-pills-<theme>-<width>.png`: every split pill on the captured surfaces, magnified.

## Measurements (`measurements/measure-own-<theme>-<width>.json`)

| Combination | Compared blocks | Line gaps | Pitch deviation | Two-tone prose pills | Adjacent-line pairs | Overlapping |
|---|---|---|---|---|---|---|
| dark 1440 | 21 | 83 | 0 px | 11 | 48 | 0 |
| light 1440 | 21 | 83 | 0 px | 11 | 48 | 0 |
| dark 860 | 21 | 104 | 0 px | 11 | 67 | 0 |
| light 860 | 21 | 104 | 0 px | 11 | 67 | 0 |

RS-OWNERSHIP's run of the same seed: 0 px deviation, 0 overlapping of 44 / 61 pairs.

Pixel rows through the link pills (spur, loop end, branch; light and dark): across a seam
the fill changes by at most 14 (sum of the RGB steps) between neighbouring pixels and moves
monotonically, so there is no seam line; the border changes by at most 48 over its ~10 px
fade.
