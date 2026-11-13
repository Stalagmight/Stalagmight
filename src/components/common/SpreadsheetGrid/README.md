# SpreadsheetGrid

A self-contained, **library-free**, Excel-like editing grid for issue #61.

> **Clean-room provenance:** implemented from observable spreadsheet behaviour
> (Excel / Google Sheets) and commodity algorithms only. No third-party grid
> library source was read or referenced.

It backs the survey data table (`DataTable.tsx`), the raw table view
(`RawTableView.tsx`), the dialog-embedded shot table (`ShotTableEditor.tsx`,
#401), Plan mode's gas tables (`SegmentsTable.tsx` / `CylindersTable.tsx`,
#403) and the full-project Data workspace (`src/components/DataWorkspace/`).

Module map: `SpreadsheetGrid.tsx` (state, keyboard, clipboard, drags),
`GridRow.tsx` (memoized row / group band), `GridHeaderRow.tsx` +
`GridSortButton.tsx`, `GridOverlays.tsx` (selection, marquees, editor, drop
lines), `GridContextMenu.tsx`, `useGridScrollRequests.ts` (`selectedRow` /
`scrollTo`), and the pure helpers `gridGeometry`, `gridClipboard`, `gridClear`,
`gridApplicable`, `gridMarks`, `gridAutoFit`, `gridActivate`.

## Features

- Roving **active cell** + keyboard nav: arrows (no wrap), Tab/Shift+Tab (skips
  read-only columns, wraps rows), Enter/Shift+Enter, Home/End, Ctrl+Home/End,
  PageUp/PageDown.
- **Type-to-edit** (printable key seeds the editor), **F2** / double-click to
  edit in place, **Esc** cancels, **Enter** commits + advances.
- **Rectangular selection**: click, Shift+click, Shift+Arrow, mouse drag,
  Ctrl/Cmd+A. The selection rectangle is drawn as a single **overlay** computed
  from row height + column-width offsets, so it survives row virtualization
  (off-screen rows unmount).
- **Copy / cut / paste** single cells and ranges, including **paste from external
  spreadsheets** (TSV on the clipboard) with locale-aware number parsing,
  1×1-fills-selection, integer-multiple tiling, and clip-to-bounds. A cut clears
  its source on **paste** (Esc cancels it); pasting the grid's own copy moves the
  raw values, so full numeric precision survives the display formatting.
- **Fill** down (Ctrl/Cmd+D), right (Ctrl/Cmd+R), and via the drag fill-handle -
  copy-fill only (the selection block is tiled across the extension; no numeric
  auto-series, deliberate for survey data).
- **Delete/Backspace** clears the selection (never writes 0 into a required
  numeric column).
- **Column hiding** (controlled `hiddenColumns`, read-only to the grid - hosts
  own the Columns menu) and **reordering** (controlled `columnOrder`, header
  drag).
- **Host sync props**: `selectedRow` (moves the active cell to an externally
  selected row, e.g. from the map; never loops back through
  `onActiveCellChange`), `hoveredRow` (tints a row with `sg-row-hover`),
  `rowClassName(row, i)` (extra per-row class, e.g. a frozen-row tint), and
  `onRowHover`. The active row carries `data-active="true"`.
- **Gutter customization**: `rowLabel(row, i)` renders a custom gutter label
  (e.g. a domain-id pill) instead of the 1-based row number; pair with
  `gutterWidth` when the label needs more than the default 48px. The gutter
  participates in row hover/selection like any cell.
- **Chrome embedding**: the grid renders no chrome of its own - every host folds
  it into its own header. Subscribe to `onSelectionChange` (selection dims +
  active cell) to render a name-box indicator there.
- **Mixed row kinds**: `GridColumn.isApplicable(row, i)` blanks a column on rows
  it does not apply to (a plain blank `sg-cell-na`, never edited, copied as '', skipped
  by Tab); `groupRowContent` turns a row into one full-width header band.
- **Header sorting** (host-owned order): `sortable` columns get a chevron button;
  `sort` draws the direction + `aria-sort`, `onSortChange` gets the next state.
- **Find highlighting** (`cellMarks`), **row tints** (`rowStyle` +
  `--sg-row-tint`), host **scroll requests** (`scrollTo`) and an index-state
  reset for re-sorted / re-filtered rows (`rowsVersion`).
- **Memoized rows** (`GridRow.tsx`): a selection, hover or edit move re-renders
  only the rows whose own state changed, not every visible row.

All edits are **coerced** (locale-aware) and emitted via callbacks; the grid never
mutates your data. Multi-cell paste/fill/clear go through `onBatchEdit` so the
host can record them as **one undo unit**.

## Domain hooks (for integration)

Survey-specific behaviour plugs in through `GridColumn`, keeping the grid generic:

- `editable` / `isEditable(row, i)` - read-only vs editable; use `isEditable` for
  frozen rows (`isShotFrozen`) and closure-adjusted (`geometryReadOnly`) mode.
- `kind: 'number' | 'number-nullable' | 'text'` - default coercion + type-to-edit
  gating.
- `parse(raw, row)` - custom coercion (e.g. the `_azimuth_avg` delta field);
  return `undefined` to **skip** a write.
- `format(value, row)` / `render(row, i)` - display + clipboard text, and rich
  cells (badges, dropdowns, icons).

## Mixed-row / workspace props (exact semantics)

All optional; a host that passes none of them gets the grid as before. Every
per-row/per-cell callback runs during render for each VISIBLE row/cell (and the
applicability ones also once per cell in bulk paths), so keep them O(1).

- **`GridColumn.isApplicable(row, i)`** (default true). `false` = not applicable
  (N/A): the cell renders blank with class `sg-cell-na` and NO `getValue` /
  `format` / `render` / `cellClassName` / `cellMarks` call. It never takes an
  edit (type-to-edit, F2, double-click, Space/activate, `openEditor`, paste -
  counted as `skipped` - fill, clear, cut-move). An EXTERNAL paste passes over a
  row where NONE of the target columns applies: that row consumes no clipboard
  line, so a column of values copied from another spreadsheet lands on
  consecutive shots rather than every other value falling into a station gap
  (`placePaste`'s `rowTakes`). A row where some target columns apply still takes
  a line (positional within the row; its N/A cells count as `skipped`). A 1x1
  fill and a tile cover only the selection rows that take a column, and the
  tiling multiple counts only those rows; gap rows are not reported. The grid's
  own copy is pasted 1:1, its gaps included. It copies as ''. Tab / Shift+Tab
  skip it per row (`tabTargetWhere`); arrows and clicks still land on it.
  Ctrl+Arrow and the fill-handle double-click pass OVER N/A cells (neither data
  nor a gap). An N/A source never writes: fill skips it, an internal paste skips
  it (the stash marks it `NOT_APPLICABLE`), a cut-move carries the gap along and
  aborts only when a real value would land on an unwritable cell. The locked
  look (`sg-cell-locked`) is now only for applicable-but-not-editable cells.
- **`GridColumn.sortable`** + **`sort`** + **`onSortChange(colKey, dir | null)`**:
  with `onSortChange` set, a sortable header gets a button
  (`${prefix}-sort-${colKey}`, `data-sort-dir` = asc|desc|none) AFTER the label
  span, shown on header hover / focus and always on the sorted column. Each
  click asks for the next state of asc -> desc -> none. The grid never reorders
  rows. The sorted header gets `aria-sort` and `sg-header-sorted`; without
  `onSortChange` a sorted column shows a static indicator. The button swallows
  mousedown / dblclick / keydown, so it never selects, drags or autofits the
  column. Labels: `GridLabels.sortAscending / sortDescending / sortClear`.
- **`GridColumn.headerClassName` / `headerTitle`**: extra class / `title` on
  the header cell.
- **`cellMarks(row, i, colKey)`** -> `{ ranges, current? }`: half-open
  `[start, end)` offsets into the cell's displayed text (`format(getValue())`).
  Unsorted / overlapping / out-of-range ranges are tolerated. A plain-text
  column draws `<mark class="sg-mark">` segments, the `current` range as
  `sg-mark sg-mark-current`. The cell gets `sg-cell-match` (+
  `sg-cell-match-current` when `current` is set); custom-`render` columns get
  those classes plus `sg-cell-match-outline`, a 2px inset outline in the mark
  colour (the solid current-mark colour when focused), since no `<mark>` can
  show the match inside their content.
- **`rowStyle(row, i)`**: inline style on the row element (the grid's own
  `top` / `height` / `width` win). Set `--sg-row-tint: <css colour>` to tint the
  row (the row then gets `sg-row-tinted`); `--sg-row-tint-strength` (default
  `14%`) sets how much of it is mixed in. The tint is a translucent layer over
  every cell state (normal, N/A, locked, frozen, hover) and under the find-match
  layer and the selection fill; group bands take it too.
- **`scrollTo: { row, colKey?, align?: 'auto' | 'center', nonce, focus? }`**:
  acts once per new `nonce` (re-send the same row with a new nonce to re-fire).
  Scrolls the row into view with `align`, the column too when `colKey` names a
  visible column (otherwise the active column is kept), and makes that one cell
  the active cell and the whole selection. Never calls `onActiveCellChange`. An
  out-of-range row is ignored (the nonce is still consumed). DOM focus moves to
  the grid only when `focus: true`, or (default) when focus is already in the
  grid or on `<body>`; `focus: false` never moves it. A host input (find box)
  therefore keeps focus by default.
- **`rowsVersion`**: any change (`Object.is`) clears the copy/cut marquee and
  stash, cancels an open editor WITHOUT committing, aborts a fill / cell-move /
  row-move drag, and collapses the selection to the active cell, clamped. Bump
  it whenever the same row count now means different rows (sort, filter).
- **`groupRowContent(row, i)`**: non-null / non-false content turns the row into
  one band (`${prefix}-group-${i}`, class `sg-group-band`, row class
  `sg-row-group`) spanning the scroll width, its content sticky just right of
  the gutter. The gutter still renders `rowLabel`. A click (mousedown) on the
  band selects the row exactly like a gutter click (and notifies
  `onActiveCellChange(i, firstColKey)`); interactive children must stop
  `mousedown` + `click` propagation. Every cell of the row is N/A.
- **`GridExtraMenuItem`**: `testId` (default `${prefix}-context-extra-${i}`),
  `disabled` (rendered, not clickable), `separatorBefore`; duplicate labels are
  fine.
- **PageUp / PageDown** page by the measured viewport (scroller height minus the
  header), keeping one row of overlap. Header and gutter MOVE drags arm only
  after 4 px of travel.

### Cell class contract

State colours are `background-color` only (never the `background` shorthand,
which would wipe the layers): `sg-cell-locked`, `sg-cell-missing`, and the row classes `sg-row-frozen`, `sg-row-hover`,
`sg-row-draft`. Overlays are `background-image` layers driven by variables,
bottom to top: `--sg-l-tint` (row tint), `--sg-l-match` (`sg-cell-match`), and
`--sg-l-sel` (`sg-cell-sel`, every selected cell but the active one). `<mark>`
elements are content, so they paint above all of it. `sg-cell-na` has no look
of its own: a not-applicable cell is an ordinary blank cell under the same
layers.

## Usage sketch (future DataTable integration)

```tsx
import { SpreadsheetGrid, type GridColumn, type CellEdit } from '@/components/common/SpreadsheetGrid';
import { useProjectStore } from '@/store/projectStore';
import { getCurrentLocale } from '@/i18n/format';

const columns: GridColumn<Shot>[] = [
  { key: 'id', header: '#', width: 56, getValue: (s) => s.id, render: (s) => <ShotIdBlob shot={s} /> },
  { key: 'length', header: 'Length', width: 90, align: 'right', editable: true, kind: 'number',
    getValue: (s) => s.length, format: (v) => formatFixed(v as number, undefined, precision) },
  { key: 'comment', header: 'Comment', width: 240, editable: true, kind: 'text',
    getValue: (s) => s.comment ?? '' },
  // … azimuth (parse: delta), depth, etc.
];

<SpreadsheetGrid
  columns={columns}
  rows={shots}
  rowKey={(s) => s.id}
  locale={getCurrentLocale()}
  onEdit={(rowIndex, colKey, value) =>
    updateShot(sectionId, shots[rowIndex].id, { [colKey]: value })}
  onBatchEdit={(edits: CellEdit[]) =>
    executeBatch(toBatchCommands(edits))}      // one undo unit
/>
```

> **Note for the integrator:** the `BatchCommand` TS union
> (`src/types/commands.ts`) needs `update_shot` and `set_station_comment`
> variants added (the Rust `Command` enum already supports them inside `Batch`)
> so `onBatchEdit` can dispatch a single-undo batch. No Rust change / IPC regen
> needed.

## Tests

Pure logic is unit-tested (Vitest, node env):
- `__tests__/gridClipboard.test.ts` - locale parsing, TSV serialize/parse round-trips.
- `__tests__/gridGeometry.test.ts` - selection math, nav (incl. read-only skip),
  paste placement (fill/tile/clip, gap rows passed over), fill targets.
- `__tests__/gridMarksAndApplicable.test.ts` - mark segmentation, applicability,
  N/A-skipping Tab order, fill extent, the sort cycle.

Component tests (jsdom, `@tanstack/react-virtual` mocked to render every row):
- `__tests__/spreadsheetGridApplicable.test.tsx` - N/A cells and group rows on
  every read and write path, plus `isEditable` gating of paste / fill / clear.
- `__tests__/spreadsheetGridHostProps.test.tsx` - sort, header props, marks,
  row tints, `scrollTo`, `rowsVersion`, extra menu items, paging, memoized rows,
  the move-drag threshold.
- the other `spreadsheetGrid*.test.tsx` files - commit, open-editor, delete
  rows, long press, column widths, test ids.

Playwright covers the grid through its hosts (`grid-editing.spec.ts`,
`draft-row.spec.ts`, `column-prefs.spec.ts`, ...).
