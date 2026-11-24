# RS-LEADDOT follow-up: a quieter home dot

The user (2026-11-24): "make the suprescript dot more subtle" (the small raised owner-colour dot after a
foreign station's name).

- Build: worktree `rs-subtle`, branch `rs/dot-subtle`, the real app (WebKitGTK) built by the agent
  harness from this tree: `builtFrom.repoRoot` = `.claude/worktrees/rs-subtle`, `gitHead` 018050114
  (debug build, `CARGO_TARGET_DIR=~/.cargo_build_rs_a`, `CARGO_PROFILE_DEV_DEBUG=0`), a tsx REPL under
  `dbus-run-session` on Xvfb `:197`, `NO_AT_BRIDGE=1`, a short `TMPDIR`, satellite and autosave off.
- Data: the synthetic RS-NODOTS survey (near-white Main, near-black Upper Branch, yellow Lemon, blue
  Blue Link and Blue Spur, magenta Annex, grey Grey Side Tunnel; foreign slots, a loop end, two- and
  three-colour shots) plus two one-shot grey stubs whose start shows Blue Spur's 2 (blue) and Annex's
  2 (magenta), so every colour has a foreign station.
- 1440x900, light and dark (cave). "Before" is 2939345fc: the `../rs-leaddot/` captures (built at
  ce523e370, which draws the same DOM), same seed and surface openers. The stubs are new, so blue and
  magenta are after only.

## Chosen variant

0.26em (was 0.36em), 0.02em after the name, its middle 0.65em up as before; full line colour; a 1px
ring only where the colour alone falls under a new floor of 2:1 on what it sits on (was 3:1), the
gentlest ring that clears it. A Station ID after it now steps aside 0.22em clear (it moves less).

Compared live on one build (`zoom/variants-{link,prose}-{light,dark}.png`, rows top to bottom):
0.36em; 0.28em; 0.26em; 0.28em with a 0.6px ring; 0.28em at 75% saturation; 0.28em at 80% opacity.
The thin ring broke up and all but lost the near-white dot on light and the near-black one on dark.
Desaturating turned yellow olive; fading sank blue and magenta into their dark fills. Size did the
quieting; the 2:1 floor lightens the rings that remain (on the light link fills: near-white `#9e9e9e`
where it was `#838383`, yellow `#ada03c` where it was `#908214`). At 0.26em yellow, blue, magenta, near-white (ringed) and
near-black (ringed on dark) still read as their colours in both themes.

## Full screenshots (`<surface>-<theme>-1440.png`, 17 surfaces x 2)

Data table gutters (shots: Blue Link, Upper Branch, Blue Spur, Grey Side Tunnel, Lemon, the blue and
magenta stubs; stations: Blue Link, Lemon, both stubs), Station details (Branch's junction, the loop
end, the blue stub), Merge Stations naming Main's 12, the Delete Shot lead, the undo history.

## Zoomed crops (`zoom/`; each crop checked non-blank and located by its pill's rect)

- `before-after-link-shot-*`: gutter Blue Link 4 -> 3 (near-white and yellow ends, 7x); the loop end's
  Previous shot (5x).
- `before-after-link-station-*`: foreign stations with `#id`, 7x: near-white (Main's 4), yellow (the
  loop end, Lemon's 3), near-black (Branch's 5).
- `before-after-prose-*`: "12 #12 belongs to Main ..." (3x); the Delete Shot lead Blue Link: 4 -> 3
  and the next word (4x).
- `after-blue-magenta-*`: blue and magenta foreign stations, a blue from end, a blue prose sentence.
- `end-of-pill-{before,after}-light.png` (12x): the yellow to end's ringed dot and the next word "is".
- `variants-*`: the comparison above.

## Measurements (`measurements/`)

- Geometry (`homedot-geometry.json`): 39 link and 6 prose home dots per theme at 11 to 16 px: 0.26em,
  0.02em after the name, the middle 0.65em (link) and 0.64 to 0.67em (prose) up; a Station ID 0.22em
  clear. Hiding a dot moves no glyph beyond WebKit's 1/64 px layout rounding (0.0156 px, on two prose
  lines).
- Contrast on the actual pixels (`homedot-contrast.json`, `homedot-pixels-*.json`): each dot's colour
  (its ring's when ringed) against the median pixel under its own disc in a capture with the dots
  hidden: min 2.35:1 (light, 30 of 45 ringed: near-white and yellow) and 2.33:1 (dark, 5 of 45:
  near-black); even the worst single pixel under a dot is at least 2:1; no ring where the dot alone
  reaches 2.6:1. (A disc 1px wider than the dot, as the RS-LEADDOT run used, now reaches the pill's
  border, the line colour itself, so it is not a backdrop for a dot this small.)
- The end of a prose pill: the dot now ends 0.03em before the next word's advance box (it overlapped
  it by about 0.07em at 0.36em); in the Delete Shot lead (16 px, light) a clear pixel column now
  separates the yellow dot's ring from the "i", which it touched before; on dark (no ring) about 2 px.
