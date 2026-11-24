# RS-LEADDOT captures (the leading dot back; a foreign station's superscript home dot)

The user's decisions (2026-11-24): every pill leads with its line's colour dot again (a station pill's
dot is the owning line's colour, a shot pill's the shot's own line's, a line pill's the line's; the
loop-end mark stays removed); and "for foreign stations, instead of dot after, just put a small color
dot in superscript right after the station name, it doesnt need to take any space horizontally".

- Build: worktree `rs-leaddot`, branch `rs/pill-leaddot`, the real app (WebKitGTK) built by the agent
  harness from this tree: `builtFrom.repoRoot` = `.claude/worktrees/rs-leaddot`, `gitHead` ce523e370
  (debug build, `CARGO_TARGET_DIR=~/.cargo_build_rs_a`, `CARGO_PROFILE_DEV_DEBUG=0`), driven by a tsx
  REPL under `dbus-run-session` on Xvfb `:197`, `NO_AT_BRIDGE=1`, a short `TMPDIR`, satellite off,
  autosave off. (ce523e370 differs from the branch head 2939345fc only by the plan's Deviations
  section, a test update and a refactor of `Prose.tsx` that draws the same DOM.)
- Data: the fully synthetic RS-NODOTS survey: Main Passage of the Northern Sump System (#f6f6f2,
  near-white), Upper Branch towards the Dry Chamber (#101010, near-black) whose 0 shows Main's 12,
  Lemon Squeeze Crawlway past the Second Restriction (#ffe600) whose 0 shows Branch's 5, Blue Link
  (#1f4fff, one shot from Main's 4 to a loop end showing Lemon's 3: both ends of other lines), Annex
  beyond the Terminal Sump (#c2185b, magenta), Blue Spur (#1f4fff) leaving Lemon's 5, Grey Side Tunnel
  (#808080) whose 0 shows Main's 8. For the history rows: a comment on Branch's junction slot and on
  Blue Link's shot.
- Window 1440x900; light, and dark (the app's dark mode, the cave theme). "Before" is the RS-NODOTS
  set (`../rs-nodots/`, fbff8ccf5's tree at 562df3ec6), same seed and surface openers.

## Full screenshots (`<surface>-<theme>-1440.png`, 18 surfaces x 2)

| Surface | What it shows |
|---|---|
| table-shots-blue | Data table of Blue Link: gutter shot 4 -> 3, three colours, a home dot on each foreign end |
| table-shots-branch, table-shots-spur, table-shots-grey | Gutter shots 12 -> 1 (near-white end on a near-black shot), 5 -> 1 (yellow end on blue), 8 -> 1 |
| table-shots-lemon | Lemon's gutter 5 -> 1: Branch's near-black home dot, ringed on dark |
| table-stations-blue, table-stations-lemon | Station pills of foreign slots with `#id` (Main's 4, the loop end Lemon's 3, Branch's 5) |
| table-stations-two-12s | Upper Branch full screen, Name column on: its own 12 and Main's 12 (with its home dot) |
| details-foreign | Station details of Branch's junction: "12 #12 belongs to Main ..." and link pills |
| details-loop-end | Station details of the loop end: Previous shot Blue Link 4 -> 3 (three colours) |
| warnings | Warnings tab: link and prose pills, a two-colour shot |
| merge, merge-foreign | Merge Stations: Annex onto Main's 2; Annex onto Branch's 0 (names Main's 12, link and prose) |
| detach | Detach a Line |
| delete-shot-blue-both-ends, delete-shot-branch-first, delete-shot-spur | Delete Shot: the prose lead and link pills |
| history | Undo history: the two comment rows (Blue Link 4 -> 3 with both home dots; Main's 12) |

## Zoomed crops (`zoom/`, light and dark; each crop checked non-blank and located by the pill's rect)

- `before-after-link-shot-*`: gutter Blue Link 4 -> 3 (7x), gutter Upper Branch 12 -> 1 (7x), the loop
  end's Previous shot (5x). After: the shot's leading dot (blue, near-black) and a home dot after each
  foreign end, on its own half.
- `before-after-link-station-*`: gutter station pills of foreign slots with their `#id` (7x): Main's 4
  (near-white) and the loop end Lemon's 3 (yellow): leading dot, home dot raised after the name, the
  `#id` clear of it.
- `before-after-prose-*`: "12 #12 belongs to Main ..." (3x) and the Delete Shot lead Blue Link: 4 -> 3
  (4x).
- `home-dots-more-*` (after only): the near-black home dot ringed on dark, a yellow dot on a yellow
  half, the Merge keep choice and junction sentence, the history row.

## Measurements (`measurements/`)

- Home dot geometry (`homedot-geometry.json`, `homedots-*.json`; dots visible and not under a dialog's
  backdrop): 35 link and 10 prose home dots per theme at 11, 12, 14 and 16 px. Every one 0.36em,
  0.02em after its name's last glyph, its middle 0.65em (link) and 0.64 to 0.67em (prose) above the
  baseline; a following Station ID clears it by 0.256em (link) and 0.22em (prose). Hiding any home dot
  moves no glyph (0 of 90). 16 link and 4 prose dots sit right before an arrow, over its tail.
- Home dot contrast on the actual pixels (`homedot-contrast.json`, `homedot-pixels-*.json`): each dot's
  colour (its ring's when it has one) against every pixel inside its disc in a capture with the home
  dots hidden (`hd/*-hidden.png`): min 3.34:1 (light, 43 of 45 ringed: near-white and yellow) and
  3.33:1 (dark, 2 of 45 ringed: Branch's near-black); none below 3:1, and no ringed dot whose colour
  alone reaches 3.6:1 there.
- Line pitch in WebKit (`pitch-*.json`), every prose block with pills against the same block as plain
  text, light and dark, 1440 and 860: 59 to 62 blocks, 25 to 26 line gaps, 0 px deviation; 93 to 96
  prose pills, each with its leading dot; 10 prose home dots in 6 blocks (8 gaps). Two one-line
  Station details "On <line>:" labels change height (16.97 px) with the level: their `sh-ppill-clip`
  halo room, which only Full has (RS-NODOTS), not a line pitch.

## Adjustments made from looking at the captures

1. At 0.4em and 0.05em after the name the dot met a following `#id` at 0.06em (link); a prose dot
   started 0.15em after its glyph (its ring's padding added to the margin) and overlapped the `#` by
   0.03em: now 0.36em, 0.02em after the name, the `#id` stepping aside about 0.25em / 0.22em.
2. A prose from end's ringed dot touched the arrow's tail below it (light, 16 px): the prose dot now
   sits as high as a link pill's (middle about 0.66em up).
3. Left as asked (zero width), to decide: at the end of a prose pill (a shot's foreign to end) the dot
   hangs into the space before the next word; a ringed one (near-white or yellow on light) comes within
   about a pixel of a tall first letter (`3` then `is` in the Delete Shot lead). Room after such a pill
   (about 0.13em, one CSS rule) would clear it, but would move the words after it.
