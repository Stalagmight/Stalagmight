# UI Style Guide

This guide captures the current UI conventions and design tokens for Stalagmight.

## Design Tokens
- Source of truth: `src/theme/tokens.ts`, applied as CSS variables in `src/index.css`.
- Use token-backed utility classes (examples): `bg-panel`, `bg-background`, `text-text-primary`, `text-text-secondary`, `border-border`, `bg-accent`, `text-accent-on`, `text-warning`, `text-error`, `text-success`.
- Opacity variants are available as token utilities (examples): `bg-accent-10`, `bg-accent-20`, `bg-background-50`.
- Warning prose takes `text-warning-text`, not `text-warning`: `--cc-warning` is tuned as a fill (stripes, the map's hover) and is too pale for text on the light themes. `--cc-warning-text` reads at WCAG AA on the panel, the background and the `bg-warning-10` / `bg-warning-20` washes in every theme (pinned in `tokens.test.ts`).

## Colors
- **No color value ships without a human seeing it rendered.** Measure and propose; do not merge. Covers `--cc-*` tokens on all 10 themes, display defaults in `src/types/config.ts`, line color palettes, and renderer/export constants.
- Contrast and CIEDE2000 numbers are evidence for that review, not a substitute: the legibility harness scores endpoint distance, which lightness dominates, so washing a color out *raises* its score.
- If a test rejects an approved color, the test is wrong.
- Depth ramp direction is load-bearing: `getDepthColor` walks the shortest HSL hue path, so a shallow anchor at hue >= 180 inverts the theme. Anchors sit below 180; `SHALLOW_HUE.max` is 178.
- Light canvases hold a 2:1 ramp contrast floor, not 3:1 - the yellow midpoint is the ramp's brightest sample, and 3:1 on near-white reads olive-brown.
- Outstanding measurements: [#380](https://github.com/Stalagmight/Stalagmight/issues/380).
- Line pills (`DetailLink`, `EntityBlob`, at the Full highlighting level) take every colour from `src/theme/pillColors.ts` and show their line by colour alone: **no dot, no mark** (RS-NODOTS). The fill is a tint of the line's hue in OKLCH (up to 80% of its chroma, capped at 0.08 dark and 0.07 light) with lightness clamped to one band per theme; the **border is the line colour itself**, moved toward the text pole only until it stands 1.8:1 from the panel and from the fill, so a near-white, a grey and a near-black line, which share a neutral fill, differ by their borders; on hover the fill steps and the border moves at most half way to the pole, keeping its hue. Text is the theme's tokens. The sweep in `__tests__/pillColors.test.ts` holds main text at 7:1, the muted context and Station ID at 4.5:1 and the border at 1.8:1 on panel and fill, on every theme, at rest, on hover and for a missing target. Two lines whose colours are close stay close: on nord a mid grey's border lands 1.29:1 from a near-black line's lifted one. Do not colour a pill by hand.
- A pill is in the colour of the line its thing belongs to (RS-OWNERSHIP). A station shown on a line it does not belong to (a foreign slot, a loop end whose kept end lives elsewhere) is a pill of its home line: that line's fill and border, or glow, and its name as the context ("Main: 12" on Side); its tooltip says "Belongs to Main" and its screen-reader words say it too. A shot with an end of another line is **two-tone**: the half holding that end in that line's colours, the rest in the shot's; the end's name carries the tooltip "Belongs to Main" and the words "(Main)". When the from end is the foreign one and the pill has a context, the context keeps the shot's colour as a head of its own, so no half names one line in another's colour. The halves **blend across the arrow** (RS-BLEND, RS-ARROW, `src/theme/pillBlend.ts`): the arrow is a segment of its own whose fill and border (a prose pill's tint) run from the from half's colour to the to half's in OKLab, the midpoint on the arrow's middle, so the arrow sits half in each colour; the halves keep their own colour up to it, and a head blends into the from half over a 0.8em cross-fade at their seam. At rest and on hover, rounded ends intact; a prose pill's glow is one capsule per line whose colour runs across the arrow (`useProseGlow`). The arrow takes the text colour that reads on every colour of its blend, and each half meets the contrast floors on every colour it shows (`twoTonePills.test.ts` sweeps the pairs, three-colour shots and samples along each blend). In prose a line never breaks on either side of the arrow (no-break spaces around it). A **loop end** draws no mark: its tooltip says "Loop end: a loop closure shows this station here" and a screen reader reads "loop end". The data table's Name column keeps its words, `12 (Main)` and `3 (loop end)`.
- Running text that names a station, a shot or a line draws each name as a **prose pill** (`ProsePill`, `src/components/common/Prose.tsx`), never the plain label `Main / 12`: the muted context (`Main:`), the bare name and the superscript Station ID, as the link pill draws them, with no border but a **glow** (RS-NODOTS, `src/theme/proseGlow.ts`): a soft-edged capsule, solid behind the whole name, rounded at the ends of each line's piece, with a halo in the same colour that carries it about 0.4em further and fades. The glow is a wash of the line's hue at alpha 0.5 (`prosePillColors`, `proseTintOf`): lighter than a dark panel, darker than a light one, further off it the nearer the line is to the text pole (so near-white, grey and near-black lines differ), and held so that text reads at 7:1 and muted parts at 4.5:1 on the glow, the panel and the theme's tinted boxes (accent, warning and error at 10%), bare and under the wash. A two-colour shot is one capsule whose colour runs from one line's to the other's across the arrow, mixed in OKLab, with no dip toward the panel; a pale middle is a near-white line's own colour. It never changes the paragraph's line pitch (background, inline padding and box-shadow, a zero line-height inside, a relatively offset superscript); `e2e-playwright/prose-line-pitch.spec.ts` measures it. Glows on adjacent lines touch softly (their cores stay apart; where halos meet the gap is no darker than the cores). A box that clips prose (a `truncate` label) takes `sh-ppill-clip` so the halo keeps its room.
- Word such a sentence with prose tokens (`useProse()`: `prose.station(ref, true)` where plain text has `labels.station(ref, true)`) and render it with `<Prose text>`; a backend text arrives with its parts (`messageParts`, `descriptionParts`, `change_parts`) and renders with `<BackendProse>` / `<ProseParts>`. The en.json strings stay as they are: a token is a string, so it passes through `t()` interpolation and `listNames`. A raw value a sentence holds (a comment, a typed or imported name, a file name) goes through `proseValue` (the i18n `format` hook does it for every `t()` value; call it for a sentence built by hand), so it can never forge or break a pill. The pill keeps the plain words as sr-only text, so a screen reader and a copy read the sentence as plain text wrote it; below the Full level the pill is that plain text.
- Plain-text contexts keep the plain label (`12 (Main)`, `Main / 12 (loop end on Side)`): aria-labels, `title` tooltips, native dialogs, the clipboard, exports, file contents, logs, telemetry, `<option>` labels, input values, dialog titles, button labels and backend error strings. `proseToPlain` turns a token string back into them.

## Typography
- Defaults are inherited from the app shell. Use consistent size tiers:
  - `text-xs` for metadata and hints.
  - `text-sm` for standard body content.
  - `text-lg` and `text-xl` for headers and primary titles.
- Use `font-medium` for section headers and `font-semibold` for key titles.

## Buttons
- `Button` variants (`src/components/common/Button.tsx`); use only these:
  - `primary`: the single main action in a view or dialog, in the theme's accent.
  - `secondary`: default action style. Its fill is the panel mixed 8% toward the text colour (`bg-button-secondary`, 14% on hover), so it reads as a button on the panel in every theme.
  - `ghost`: low-emphasis actions in dense UI or toolbars.
  - `danger`: destructive actions (delete, discard).
- Do not give a `Button` a colour, shadow or fixed height of its own through `className`, and do not add a variant for one feature: a view's buttons look the same as every other panel's and dialog's. Width and margins (`w-full`, `mt-2`) are layout and are fine.
- At most one `primary` per view, and only when one action is clearly the one to take now. A view may have none. The SpeleoDB tab picks it with `speleoPrimaryAction`: Save Changes while there are changes not yet uploaded, Resume editing when the account can edit, the Connect button while signed out, none when the access is view-only or when editing with nothing to save. Every other action there is `secondary`.
- Side panels stack their actions as full-width buttons (the home screen's project details, the shot details, the SpeleoDB tab), at the size the panel uses for all of them.
- A button that opens the browser carries `ExternalLinkIcon` as its `icon`.
- A disabled button says why in its tooltip (`title`).
- A menu row a read-only state turns off says why with `disabledReason` (`AppMenu`) or `reason` (`ContextMenu`): the row is `aria-disabled`, `DisabledReason` shows the reason on hover, and a click shows it as the blocked-edit notice. Not a native `title`: WebKitGTK drew none on a disabled row, and a `disabled` row gets no click to answer. A toolbar button a read-only file turns off (Import, Undo, Redo) does the same with the same component (`placement="below"`).
- `DisabledReason`'s bubble is the opaque panel surface (`bg-panel`, as the menus), not the translucent tooltip one (`bg-panel-90`), and it opens at a menu row's side, vertically centred, flipping left at the window's edge, so it covers neither neighbouring row and nothing shows through its text.
- Labels and confirmations explain the action independently of colour.
- Use `icon` on `Button` for leading icons. When the same kind of action appears in the menus, give its icon the menus' colour with `iconColor` and `MENU_ICON_COLORS` (`src/components/common/menuColors.ts`), so an action wears one colour everywhere: the SpeleoDB tab's Save Changes is the menus' Save, Resume editing is Import, opening a page on the SpeleoDB website is Project Details, Reload Project is Open, Close Project is Close, and Switch to Read Only Mode is the neutral grey. `Button` applies the tint on `secondary` and `ghost` only; a filled `primary` or `danger` button keeps its own text colour on the icon.
- Use `IconButton` for icon-only actions; always include `aria-label` and, when helpful, `tooltip`.

## Dialogs
- Use `DialogWrapper`. The body is text that explains the situation and, when there is a choice, what each button does; then ONE right-aligned footer row (`flex flex-wrap justify-end gap-2 pt-4 border-t border-border`) holds every button: Cancel first, a destructive action (`danger`) next, the `primary` last. `UnsavedChangesDialog` is the reference.
- No buttons in the body, and no hint paragraph under a button: fold the hint into the body text. Size the dialog so the footer fits on one line. The exception is a notice in the body that carries the one action resolving it, such as "Resolve now" in the SpeleoDB upload dialog's survey-errors notice: the footer stays about the dialog's own action.
- A second step (a description to type, a "lost forever" confirmation) keeps the same footer row: Back, then the step's action. Escape and the X cancel the whole dialog.
- Do not signal an error where there is none: un-uploaded SpeleoDB changes are yellow (`text-warning-text`), never a "!" or the error colour.

## Icons
- Use SVG icons from `src/components/common/icons.tsx`.
- Default size is 16px; use 14px in dense toolbars and 20px+ for empty states or emphasis.
- Icons inherit color via `currentColor` and should follow surrounding text color utilities, except where an action takes its menu colour (see Buttons).
- Avoid emoji or text glyphs for icons to keep the system replaceable.

## Panels and Layout
- Use `Panel` for bordered, titled groups.
- Use `ResizablePanel` for split panes and adjustable sidebars.
- Common header spacing: `px-3 py-2` with `border-b border-border`.

## Tables and Lists
- Use `SpreadsheetGrid` for tabular survey data, read-only or editable (the
  in-house grid; see `src/components/common/SpreadsheetGrid/README.md`).
  `ShotTableEditor` is the shot table embedded in dialogs.
- Selection uses a persistent indicator (`selected-cell-indicator`) and a `bg-accent-20` row tint.
- Hover states should be lighter (`bg-accent-10`) than selection.

## Forms
- Inputs should use themed backgrounds and borders (`bg-panel`, `border-border`) and a visible focus ring (`focus:ring-1 focus:ring-accent`).
- Prefer shared components like `Select` and `DatePicker` for consistent behavior.
- A list of people (surveyors, explorers) is a `PeopleInput` (`src/components/common/PeopleInput.tsx`): one pill per person on `bg-accent-10`, a partial one (on some of the selected shots) dashed on `bg-accent-5`, `compact` in a table cell. Identity and labels come from `src/utils/people.ts`.
- A `Select` embedded in a `SpreadsheetGrid` cell takes `variant="cell"`: it drops its box and fills the cell, so only the right-edge chevron marks it as a control (see the Plan Cylinders grid's Role column).

## Accessibility
- Icon-only buttons must include `aria-label`.
- Keep `data-testid` attributes consistent for UI tests.
- A resize divider is a WAI-ARIA window splitter, not a decorative div:
  `role="separator"`, `aria-orientation`, `aria-label`, `aria-valuenow/min/max`
  in the same units the drag clamps to, `tabIndex={0}`, and
  `focus:outline-none focus-visible:ring-1 focus-visible:ring-accent`. Arrow
  keys nudge, Shift+Arrow steps coarsely, Home/End jump to the bounds, and the
  keys persist through the same path the drag's release uses (debounced, so a
  burst of taps is one write). `WizardMapPanel` is the reference
  implementation; the SingleSystemView resizers have not been retrofitted yet
  (#396).
