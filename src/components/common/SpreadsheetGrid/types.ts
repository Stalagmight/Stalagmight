import type { ReactNode } from 'react';

/**
 * SpreadsheetGrid - a self-contained, library-free, Excel-like editing grid.
 *
 * Clean-room note: implemented from observable spreadsheet behaviour and
 * commodity algorithms only; no third-party grid library source was referenced.
 *
 * The grid is **controlled**: it renders the `rows` you pass and emits edits via
 * callbacks. It never mutates your data. Selection/active-cell/edit-mode are
 * owned internally (transient, high-frequency state); persisted concerns
 * (hidden/ordered columns, column widths) are lifted to props so the host can
 * store them.
 *
 * Survey-specific behaviour (frozen rows, computed columns, azimuth deltas,
 * locale parsing) plugs in through `GridColumn` predicates and `parse`, so the
 * grid itself stays domain-agnostic and reusable.
 */
export interface GridColumn<Row> {
  /** Stable identity for the column. Selection/copy/paste are keyed by this,
   *  never by position, so hiding/reordering never desyncs a selection. */
  key: string;
  header: string;
  /** Width in px (default {@link DEFAULT_COL_WIDTH}). Fixed widths keep the
   *  selection-overlay geometry exact. */
  width?: number;
  /** Let this column absorb leftover horizontal space so the grid fills its
   *  container instead of leaving an empty gutter on the right (e.g. a comment
   *  column). {@link width} is its minimum; extra space is split across all grow
   *  columns. A manual resize PINS the column: it keeps the dragged width and stops
   *  absorbing leftover, so the grid may then sit narrower than its container.
   *
   *  A pinned width is NOT capped to the current container. A per-window cap was
   *  tried and reverted - see the rationale on `applyWidthOverrides` in
   *  `useColumnWidths.ts`, which is the authority here: capping froze the handle
   *  on a widening drag while the raw dragged value still reached config.toml.
   *  So a wide drag on a big monitor can leave a horizontal scrollbar on a
   *  smaller one; that is bounded by MAX_COL_WIDTH and is recoverable by dragging
   *  back, which the frozen handle was not. */
  grow?: boolean;
  align?: 'left' | 'center' | 'right';

  /** Read the cell's logical value from a row. */
  getValue: (row: Row, rowIndex: number) => unknown;
  /** Format the value for display AND for the clipboard (so a copy round-trips
   *  into a same-locale spreadsheet). Defaults to `String(value ?? '')`. */
  format?: (value: unknown, row: Row) => string;
  /** Custom cell content (badges, dropdowns, icons). Defaults to formatted text. */
  render?: (row: Row, rowIndex: number) => ReactNode;

  /** Whether the column accepts edits at all (default false → read-only). */
  editable?: boolean;
  /** Per-cell override; if present it wins over {@link editable}. Use for frozen
   *  rows / closure-adjusted mode at integration time. */
  isEditable?: (row: Row, rowIndex: number) => boolean;

  /** Coercion hint used by the default parser and for type-to-edit gating. */
  kind?: 'text' | 'number' | 'number-nullable';
  /** Open a larger multi-line editor (comments etc.) instead of the
   *  cell-sized one. */
  multiline?: boolean;
  /** Suppress the inline cell editor - no double-click, F2 or type-to-edit -
   *  while leaving the column WRITABLE by paste, fill and clear.
   *
   *  For a column whose {@link render} is itself the control (a cycling badge, a
   *  toggle): typing a value into it is a second, worse way to do what a click
   *  already does, and opening a text box over a one-glyph cell reads as a bug.
   *  Distinct from clearing {@link editable}, which would also cut off the bulk
   *  paths - stamping one value down a run with the fill handle stays the point
   *  of having the column in the grid at all. */
  noInlineEdit?: boolean;
  /** Keyboard equivalent of the click that {@link render}'s own control handles,
   *  for a {@link noInlineEdit} column. Without it that flag removes EVERY
   *  keyboard path - F2, Space and type-to-edit all route through the cell
   *  editor - leaving a cell a keyboard-only user can focus but never change,
   *  while every other column in the grid stays editable. `dir` mirrors the
   *  click's Shift modifier, so Shift+Space cycles backwards. */
  activate?: (row: Row, rowIdx: number, dir: 1 | -1) => void;
  /** The cell's editor is the host's own popover (a people list, a date): every
   *  way into an edit - Enter, F2, Space, a double click anywhere in the cell,
   *  a typed character - calls this instead, with the typed character as
   *  `seed` ('' for the others). The grid then never opens its text editor over
   *  the cell and never commits a typed character through {@link parse}, which
   *  stays for paste, fill and clear. Implies {@link noInlineEdit}. */
  openEditor?: (row: Row, rowIdx: number, seed: string) => void;
  /** Parse a raw typed/pasted string into the value to commit. Return
   *  `undefined` to SKIP the write (e.g. a non-finite number, or an empty string
   *  into a non-nullable numeric - never silently write 0). Defaults to a
   *  locale-aware parser based on {@link kind}. */
  parse?: (raw: string, row: Row) => unknown;
  /** Validate a parsed value before it is committed. Return an error message to
   *  REJECT it, or `null` to accept. Interactive edits flash the cell red and
   *  keep the editor open so the value can be fixed; bulk paste/fill/move skip a
   *  rejected cell. Clears are never validated. */
  validate?: (value: unknown, row: Row) => string | null;
  /** Does this column apply to this row at all? `false` renders a blank
   *  "not applicable" cell (lighter than the locked look, no not-allowed
   *  cursor), never takes an edit (type/paste/fill/clear/move skip it), copies
   *  as '' and is skipped by Tab. Defaults to applicable. For grids that mix
   *  row kinds (the Data workspace's station / shot rows). */
  isApplicable?: (row: Row, rowIdx: number) => boolean;
  /** Can the header be clicked to sort (the grid shows the sort affordance;
   *  the host owns the order through `sort` / `onSortChange`). */
  sortable?: boolean;
  /** Extra class on this column's header cell (e.g. "revealed by search"). */
  headerClassName?: string;
  /** Tooltip on this column's header cell. */
  headerTitle?: string;
}

/** Substring highlights for one cell's formatted text (`format(getValue())`),
 *  e.g. find matches. `current` is the focused match, drawn stronger. */
export interface CellMarks {
  ranges: readonly (readonly [number, number])[];
  current?: readonly [number, number];
}

/** Host request to bring a cell into view and make it active. A new `nonce`
 *  re-fires the same target. */
export interface GridScrollRequest {
  row: number;
  colKey?: string;
  /** 'auto' = minimum scroll (default); 'center' = centre the row. */
  align?: 'auto' | 'center';
  nonce: number;
}

/** A host entry appended to the grid's right-click menu. */
export interface GridExtraMenuItem {
  label: string;
  onClick: () => void;
  testId?: string;
  disabled?: boolean;
  /** Draw a divider above this item. */
  separatorBefore?: boolean;
}

/** A cell identified by data-row index + column key (never DOM/position). */
export interface CellCoord {
  row: number;
  colKey: string;
}

/** A selection rectangle plus the active cell.
 *  - `anchor` / `focus` are the rectangle corners (`focus` is the moving corner
 *    while extending).
 *  - `active` is where typing/editing happens and the unfilled "white" cell.
 *    During a range extension it stays put (Google-Sheets behaviour); Tab/Enter
 *    cycle it within the rectangle. For a single-cell selection all three are
 *    equal. */
export interface GridSelection {
  anchor: CellCoord;
  focus: CellCoord;
  active: CellCoord;
}

/** A single resolved edit emitted to the host. `value` is already coerced. */
export interface CellEdit {
  row: number;
  colKey: string;
  value: unknown;
}

export const DEFAULT_COL_WIDTH = 120;
export const ROW_HEIGHT = 33;
