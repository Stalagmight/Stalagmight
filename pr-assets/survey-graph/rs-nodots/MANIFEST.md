# RS-NODOTS captures (no dots or marks; the prose glow is a soft capsule)

The user's requests (2026-11-24): "get rid of the circle and squiggle indicators.. the prose glow is
too weak, it should also cover, it dosent spread far enough. the blue circle as well,"; "when the prose
glow has two colors, they should blend into each other, not blend to white and white to second
color"; "the glow should be much more pill shaped".

- Build: worktree `rs-nodots`, branch `rs/pill-nodots`, the real app (WebKitGTK) built by the agent
  harness from this tree: `builtFrom.repoRoot` = `.claude/worktrees/rs-nodots`, `gitHead` 562df3ec6
  (debug build, `CARGO_TARGET_DIR=~/.cargo_build_rs_a`, `CARGO_PROFILE_DEV_DEBUG=0`), driven by a tsx
  REPL under `dbus-run-session` on Xvfb `:191`, `NO_AT_BRIDGE=1`, satellite off, autosave off.
- Data: the fully synthetic RS-ARROW survey plus a grey line. Main Passage of the Northern Sump
  System (#f6f6f2, near-white), Upper Branch towards the Dry Chamber (#101010, near-black) whose 0
  shows Main's 12 (two 12s on Branch), Lemon Squeeze Crawlway past the Second Restriction (#ffe600)
  whose 0 shows Branch's 5, Blue Link (#1f4fff, one shot from Main's 4 to a loop end showing Lemon's
  3: both ends of other lines), Annex beyond the Terminal Sump (#c2185b), Blue Spur (#1f4fff) leaving
  Lemon's 5, Grey Side Tunnel (#808080) whose 0 shows Main's 8.
- Windows: 1440x900 and 860x900; light, and dark (the app's dark mode, the cave night theme, as the
  RS-ARROW set).
- "Before" is the RS-ARROW set (`../rs-arrow/`), captured at 433188ed0 with the same seed and
  surface openers.

## Full screenshots (`<surface>-<theme>-<width>.png`, 16 surfaces x 4)

| Surface | What it shows |
|---|---|
| table-shots-blue | Data table of Blue Link: the gutter shot 4 -> 3, two-tone (near-white to yellow) with no dots or loop mark |
| table-shots-branch, table-shots-spur, table-shots-grey | Gutter shots 12 -> 1 (near-white to near-black), 5 -> 1 (yellow to blue), 8 -> 1 (near-white to grey) |
| table-stations-blue | Blue Link's stations: 4 in Main's colour, the loop end 3 in Lemon's, no mark |
| table-stations-two-12s | Upper Branch full screen, Name column on: two 12s, the first "12 (Main Passage ...)" |
| details-foreign | Station details of Branch's junction: prose sentences, link pills, the On <line> rows |
| details-loop-end | Station details of the loop end: the box sentence, Loop closures, Previous shot (three colours) |
| warnings | Warnings tab: line pills of near-white and near-black lines, shot pills |
| merge | Merge Stations: the densest prose (wrapping pills on stacked lines, accent and warning boxes) |
| detach | Detach a Line: near-black and near-white prose glows side by side |
| delete-shot-blue-both-ends | Delete Shot: the three-colour prose shot Blue Link: 4 -> 3, link pills of four lines |
| delete-shot-branch-first, delete-shot-spur | Delete Shot: near-black head with a near-white from end; Blue Spur: 5 -> 1 (blue, yellow, blue) |
| history | Undo history rows: prose pills wrapping in a narrow dropdown |
| conversion-notice | The conversion banner (warning wash): station pills and two two-colour shots |

## Zoomed crops (`zoom/`, light and dark)

- `before-after-link-shot-*.png`: the Blue Link shot pill in the data table gutter (6x) and the loop
  end's Station details (Loop closures to Previous shot, 2x), before and after: dot, end dots and loop
  mark gone; the colour shows by the fill and the border.
- `before-after-prose-*.png`: the Delete Shot lead sentence (3x) and the Merge sentences (2x), before
  and after: the faint radial blob with dots becomes a solid, rounded capsule with a soft halo.
- `near-white-near-black-*.png`: link pills of the near-white and near-black lines and the shot
  between them, the two-12s gutter, the Detach prose.
- `yellow-blue-*.png`: link pills of Blue Link, Main and Lemon; Blue Spur: 5 -> 1 in prose; the two
  two-colour shots on the conversion banner.
- `loop-end-*.png`: the loop end's gutter pill and its Station details, no loop mark (the tooltip and
  the words say "loop end").
- `two-colour-prose-*.png`: Blue Link: 4 -> 3 with its words and with the words hidden (one capsule:
  Blue Link's blue, Main's near-white, Lemon's yellow, the colour running across the arrow, no dip
  toward the panel), and Blue Spur: 5 -> 1 with the words hidden.

## Measurements (`measurements/`)

`measure-<theme>-<width>.json` (per surface: blocks, line tops with pills and as plain text, glow
cores and stacked pairs, every visible prose word's colour and box), `analysis.json`,
`contrast.json`, `rects.json`; `glow/` holds the text-hidden captures the pixel numbers come from
(each compared against a capture with the glows hidden too: the true background of every pixel).

| Combination | Prose pills (two-colour) | Pitch: blocks / gaps compared | Max pitch deviation vs plain text | Stacked pairs on surfaces | Core overlaps | Core gap min / median | Gap fill median (p95) | Overshoot L median / p95 / max | Main text min contrast body / descender | Muted min contrast body / descender |
|---|---|---|---|---|---|---|---|---|---|---|
| light-1440 | 77 (5) | 15 / 23 | 0.0 px | 63 on 12 | 0 | 2.81 / 4.63 px | 0.8 (0.9) | 0.41 / 0.61 / 1.03 | 10.53 / 10.49 | 4.57 / 4.52 |
| light-860 | 86 (5) | 9 / 17 | 0.0 px | 81 on 16 | 0 | 2.81 / 4.63 px | 0.82 (0.95) | 0.41 / 0.95 / 1.03 | 10.53 / 10.49 | 4.57 / 4.54 |
| dark-1440 | 83 (5) | 11 / 17 | 0.0 px | 73 on 16 | 0 | 2.81 / 4.63 px | 0.74 (0.94) | 0.46 / 0.98 / 1.78 | 7.24 / 7.2 | 4.79 / 4.79 |
| dark-860 | 86 (5) | 9 / 17 | 0.0 px | 81 on 16 | 0 | 2.81 / 4.63 px | 0.76 (0.94) | 0.5 / 0.98 / 1.86 | 7.25 / 7.2 | 4.79 / 4.79 |

- Line pitch: every block holding prose pills was measured with pills and again as plain text
  (highlighting off, same element): line tops from the text's client rects. Every compared gap equals
  the plain one (0 px, tolerance 0.5 px) and the line-height. History rows re-render on the level
  change and were measured with pills only (every gap equal to the 20 px line-height).
- Overlap: a "stacked pair" is two glow cores on adjacent lines overlapping horizontally. Every pair's
  halos meet (cores 2.8 to 4.6 px apart, the halo fades over about 6 px at 14 px), no two cores
  overlap. Where they meet, the gap between lines stays filled to a median 74 to 82% of the weaker
  glow (they touch softly), and the strongest pixel there exceeds the stronger core by a median 0.4 to
  0.5 OKLab lightness points, at most 1.0 (light) and 1.9 (dark), on 1 to 2 px rows: no darkened band
  (a lightness step under 1 point is below notice; the glows themselves are 9.3 (light) and 15
  (dark) points off their background, medians).
- Contrast: each visible prose word's colour against the actual pixels under its glyph zone in the
  text-hidden capture (the glow, a neighbour's halo, a tinted box), split into the body (baseline to
  0.72em above) and the descender rows. Main text holds 7:1 and muted 4.5:1 everywhere, descenders
  under a neighbour's halo included. Words cut by an ellipsis are measured up to where another element
  starts.

## Colours of the hard lines

Link pills: the fill carries the line's hue (up to 80% of its chroma, capped) in the legible band;
the border is the line colour itself, moved toward the text pole only until it stands 1.8:1 from the
panel and the fill. Prose glow: a wash at alpha 0.5; its colour over the panel, its lightness further
from the panel the nearer the line is to the text pole.

light (panel #ffffff)

| Line | Link fill | Link border (vs panel, vs fill) | Prose glow (vs panel) | Prose text, muted on the glow |
|---|---|---|---|---|
| near-white `#f6f6f2` | `#f5f5f5` | `#b1b1ae` (2.15, 1.97) | `#e0e0e0` (1.32) | 14.01, 5.17 |
| grey `#808080` | `#f1f1f1` | `#808080` (3.95, 3.50) | `#d3d3d3` (1.50) | 12.36, 5.12 |
| near-black `#101010` | `#ededed` | `#101010` (19.03, 16.25) | `#c4c4c4` (1.74) | 10.61, 5.23 |
| yellow `#ffe600` | `#fef7c0` | `#cab600` (2.06, 1.89) | `#ece192` (1.33) | 13.87, 5.12 |
| blue `#1f4fff` | `#eaf1ff` | `#1f4fff` (5.80, 5.12) | `#bad0ff` (1.55) | 11.94, 5.18 |
| magenta `#c2185b` | `#ffebee` | `#c2185b` (5.87, 5.14) | `#ffbbc9` (1.59) | 11.63, 5.28 |

dark, cave (panel #171615)

| Line | Link fill | Link border (vs panel, vs fill) | Prose glow (vs panel) | Prose text, muted on the glow |
|---|---|---|---|---|
| near-white `#f6f6f2` | `#0f0f0f` | `#f6f6f2` (16.68, 17.69) | `#474747` (1.95) | 8.10, 5.33 |
| grey `#808080` | `#0d0d0d` | `#808080` (4.58, 4.92) | `#3c3c3c` (1.64) | 8.09, 5.38 |
| near-black `#101010` | `#0a0a0a` | `#444444` (1.86, 2.03) | `#303030` (1.37) | 8.61, 5.27 |
| yellow `#ffe600` | `#120f00` | `#ffe600` (14.26, 15.15) | `#4f460b` (1.90) | 8.12, 5.29 |
| blue `#1f4fff` | `#01062e` | `#1f4fff` (3.12, 3.39) | `#20366e` (1.56) | 8.00, 5.18 |
| magenta `#c2185b` | `#200009` | `#c2185b` (3.08, 3.36) | `#631e33` (1.52) | 7.99, 5.20 |

## Adjustments made from looking at the captures

1. Seams between the word boxes of a two-colour glow (one SVG per box at fractional positions):
   each line's piece is now painted by its first box, stretched over the piece.
2. A near-white glow on light (#e7e7e7) and a near-black one on cave (1.29:1) were the faintest, and
   the near-white middle of Blue Link: 4 -> 3 read as a fade to white: the band starts further from
   the panel (0.09, was 0.07).
3. On the conversion banner and the Station details box the muted context measured 4.21:1 and
   4.30:1 (a wash moves a tinted box further than the panel): the floors now hold over the theme's
   tinted boxes too, and the line lightness is spread over the room the text leaves.
4. Station details' truncating "On <line>:" label cut the halo flat at its top and bottom: it keeps
   room for the halo.
