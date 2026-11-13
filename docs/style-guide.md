# UI Style Guide

This guide captures the current UI conventions and design tokens for Stalagmight.

## Design Tokens
- Source of truth: `src/theme/tokens.ts`, applied as CSS variables in `src/index.css`.
- Use token-backed utility classes (examples): `bg-panel`, `bg-background`, `text-text-primary`, `text-text-secondary`, `border-border`, `bg-accent`, `text-accent-on`, `text-warning`, `text-error`, `text-success`.
- Opacity variants are available as token utilities (examples): `bg-accent-10`, `bg-accent-20`, `bg-background-50`.
- Warning prose takes `text-warning-text`, not `text-warning`: `--cc-warning` is tuned as a fill (stripes, the map's hover) and is too pale for text on the light themes. `--cc-warning-text` reads at WCAG AA on the panel, the background and the `bg-warning-10` / `bg-warning-20` washes in every theme (pinned in `tokens.test.ts`).

## Colors
- **No color value ships without a human seeing it rendered.** Measure and propose; do not merge. Covers `--cc-*` tokens on all 10 themes, display defaults in `src/types/config.ts`, section palettes, and renderer/export constants.
- Contrast and CIEDE2000 numbers are evidence for that review, not a substitute: the legibility harness scores endpoint distance, which lightness dominates, so washing a color out *raises* its score.
- If a test rejects an approved color, the test is wrong.
- Depth ramp direction is load-bearing: `getDepthColor` walks the shortest HSL hue path, so a shallow anchor at hue >= 180 inverts the theme. Anchors sit below 180; `SHALLOW_HUE.max` is 178.
- Light canvases hold a 2:1 ramp contrast floor, not 3:1 - the yellow midpoint is the ramp's brightest sample, and 3:1 on near-white reads olive-brown.
- Outstanding measurements: [#380](https://github.com/Stalagmight/Stalagmight/issues/380).

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
