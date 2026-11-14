# Color palettes and Levels

When the map colors by **depth** or by **distance**, a cog beside the
bottom-left legend opens the color-scale popover. The same controls appear in
the Export panel under **By depth** and **By distance** (PNG / PDF; distance there
is always the penetration distance from the entrance).
Gas reach in Plan mode keeps its own fixed ramp. **Survey Issues** has its own
cog with a palette and per-error levels; see
[Survey Issues colors](#survey-issues-colors) below.

The popover has two parts, each with a reset-to-default button:

- **Palette** - a searchable picker over every palette, in folders, with a
  gradient preview per row. Hovering or arrowing through it previews on the map;
  Enter or a click picks; Escape or a click outside reverts. The pin on a row
  keeps that palette in a **Pinned** folder at the top. **Reverse** runs the
  palette end to start (typing matplotlib's `_r` suffix, e.g. `viridis_r`, picks
  the reversed palette directly). The default is **Theme**: the active theme's
  own ramp.
- **Range** - a Levels control in the style of an image editor: a histogram of
  the depths (or distances) over the palette bar, with three handles underneath
  and the same three as fields. Values at or below **Min** take the palette's
  start, at or above **Max** its end; **Mid** is a gamma that bends the middle.
  Dragging previews live; the default is the data's full extent, linear.

## Where the settings live

| Setting | Stored | Lifetime |
|---------|--------|----------|
| Palette per kind (depth, distance), reversed flag | `config.toml` `[map_palette]` | Follows you across projects |
| Pinned palettes (one list, up to 64) | `config.toml` `[map_palette].pinned` | Follows you across projects |
| Map range (Min / Mid / Max) per kind | memory (`colorScaleStore`) | Until another project is opened |
| Export range per kind | memory (`colorScaleStore`) | Until another project is opened |
| Survey Issues palette, combine mode (`issues_combine`), switched-off errors (`issues_hidden`) | `config.toml` `[map_palette]` | Follows you across projects |
| Survey Issues levels (Min / Warn / Max) per error | memory (`colorScaleStore.issueLevels`) | Until another project is opened |

None of this is written to the `.stalx` (no `save_display_setting` call; a
store test pins that), so none of it reaches SpeleoDB. The palette is written
only through the partial `update_map_palette` command. The map and the export
share the palette choice, so an export matches the screen; the ranges are
separate because export distance is always measured from the entrance while the
map's can start from any station.

## The Levels mapping

```
t  = (clamp(v, min, max) - min) / (max - min)      // 0..1
t' = t ^ (1 / gamma)                               // gamma 1 = linear
```

The Mid handle is the value whose output is 0.5, which sits `0.5 ^ gamma` of
the way from Min to Max - exactly the grey slider of image-editor Levels. Moving
Min or Max keeps gamma, so Mid keeps its relative position. Gamma is clamped to
0.10 - 9.99. With gamma 1 the code path skips `pow` and computes `t` with the
same operations as `getDepthColor`, which is what keeps the default byte-exact.

## The default is byte-identical

The **Theme** palette over the auto range must reproduce the pre-palette colors
exactly (#380): depth through `getDepthColor`'s shortest-hue HSL walk
(`hslRampAt`), distance through MapLibre's floor-packed RGB lerp. When the
palette and range are both at their defaults the renderer, labels, legends and
export receive no scale at all and run their original code; the Theme palette
itself is also tested to match those paths across every theme. Any change to a
palette color is a color change under the #380 rule: a person looks at it
rendered before it merges.

## Palette sources

- **Theme** - the active theme's depth endpoints (`--cc-depth-shallow` /
  `--cc-depth-deep`), or the fixed distance ramp.
- **matplotlib** - every named colormap of matplotlib 3.10.7, in matplotlib's own
  groups (perceptually uniform, sequential, sequential 2, diverging, cyclic,
  qualitative, miscellaneous). `scripts/gen-palettes.py` samples each at 256
  points and keeps the fewest stops within one 8-bit level, writing
  `src/utils/palettes/catalog.generated.ts`. Qualitative maps are drawn as hard
  bands. Regenerate with `pnpm gen:palettes` (needs `uv`; it pins the
  matplotlib version and refuses another).
- **Gradient folders** - hand-authored ramps (basics, spectrum, blues, greens,
  reds and oranges, purples and pinks, browns and earth, grays, metals) in
  `src/utils/palettes/catalog.curated.ts`.

The licences for the matplotlib-derived data (matplotlib, ColorBrewer, Yorick,
Wistia, turbo) are in `third_party/palettes/`, and `pnpm gen:third-party`
reproduces them in the "Vendored data" section of `THIRD-PARTY-LICENSES.md`. The
licence gate hashes `third_party/`, so editing a notice without regenerating
fails CI.

## Rendering path

- **Exact path** - the color resolvers take a `ColorScaleInput` (resolved
  palette + range) through `DisplayColorContext.depthScale` and the distance
  overlay's `scale`. A change is a global color change, so it takes the
  controller's color-only tier (no geometry rebuild). `SurveyLayer.setColorScales`
  applies it without a `flushUpdate`.
- **Live preview** - while a palette is hovered or a handle dragged,
  `SurveyLayer.previewColorScales` recolors at most once per animation frame
  through `previewResolvers.ts`: the palette baked into a 4096-entry lookup
  table, line colors and station fills written in one flat loop (about 3 ms for
  75k shots), labels and the hover overlay left alone. It is within two 8-bit
  levels of the exact colors, and the controller replaces it exactly when the
  gesture ends - even when the committed scale did not change (hover, then
  Escape). The timing check is opt-in: `STALAGMIGHT_PERF=1 pnpm exec vitest run
  previewResolvers`.
- `MapCanvas` drives the layer by store subscription
  (`useSurveyLayerColorScales`), so a drag never re-renders it.

## Survey Issues colors

The cog beside the Survey Issues legend (`IssueLegend.tsx` ->
`colorScale/IssueScaleEditor.tsx`) has:

- **Palette** - the same picker (`PaletteRow`, kind `issues`). Its theme entry is
  called **Default** and is the original green -> yellow -> red; with it
  unreversed the map gets `palette: null` and runs the original ramp code
  (`issueColor.ts`), byte for byte.
- **Color a shot by** - **Worst error** (the maximum score) or **Average** (the
  mean *palette position* of the shot's errors, so one error at Max and one at
  Min give the warn color). Stations combine the shots touching them, and their
  merge group, the same way. A GPS fix whose gap warning fires (#741) is raised to
  the end color after that, so it is never averaged away.
- **Errors** - one row per error (azimuth in vs out, inclination in vs out,
  inclination vs depth change, length vs depth change, loop closure): an on /
  off switch and a levels slider. **Min**, **Warn** and **Max** map to scores
  0, 1 and 3, i.e. palette positions 0, 0.5 and 1, linearly in between
  (`issueScale.ts::levelScore`). The defaults put Warn on the project threshold
  and Max at three times it, which is exactly the original scoring of the three
  angle checks. Length has no project threshold and was not in the original
  color: it defaults to 1 m / 3 m. Closures default to 1% / 5%, the closure
  line's own color bands. The histogram
  is drawn on a log scale so the few large errors show beside the many small
  ones. A switched-off error leaves the colors, the Errors labels and the
  legend key. Unresolved errors (zero length, length shorter than the depth
  change, ...) still force the end color, and open warnings from checks that
  are not graded here (short / long shot, depth change) still floor at the warn
  color. A graded check's own warning (azimuth, inclination in vs out,
  inclination vs depth) sets no floor, so switching it off or raising its
  levels can take the shot back to the start color.
- In **Average**, inclination vs depth change and length vs depth change both
  come from the same disagreement (the length one restates it in metres), so a
  shot with that one fault counts it twice; switch one of them off for an
  average that counts it once.

The levels never touch the validator: which shots get a warning, and the
thresholds in Project Settings, are unchanged (the popover says so). While
Survey Issues paints the map, loop-closure lines and their labels use the same
palette and the closure levels; otherwise they keep their color bands.

A handle drag or a palette hover previews through
`SurveyLayer.previewIssueScale` (driven by `useSurveyLayerIssuePreview`), which
stands in for the committed scale and recolors through the normal color-only
path - shots, stations, Errors labels and closure lines, exact, at most once per
animation frame. The commit arrives through `MapCanvas` as usual.

## Export

`withColorScales` (`src/utils/exportOptions.ts`) stamps `depth_scale` /
`distance_scale` on the image options for both `export_image` and
`preview_svg_export`: the palette baked into 256 `#rrggbb` entries and the
export's Levels range. Absent fields keep the original export. The Export
panel's histogram comes from `export_value_histogram`, over the export's own
auto range: station depths from the adjusted solve (the range also spans the
shot depth readings) for depth, the entrance distance field for distance. It is
refetched when the kind, the project, the reference-leg setting or the
closure-adjustment settings change.
