# SpreadsheetGrid

A self-contained, **library-free**, Excel-like editing grid for issue #61.

> **Clean-room provenance:** implemented from observable spreadsheet behaviour
> (Excel / Google Sheets) and commodity algorithms only. No third-party grid
> library source was read or referenced.

It backs the survey data table (`DataTable.tsx`), the raw table view
(`RawTableView.tsx`), the dialog-embedded shot table (`ShotTableEditor.tsx`,
#401) and Plan mode's gas tables (`SegmentsTable.tsx` / `CylindersTable.tsx`,
#403).

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
  paste placement (fill/tile/clip), fill targets.

The React layer is thin glue over these helpers; full interaction coverage
(Playwright) is added when the grid is integrated into `DataTable` and reachable
in the running app.
