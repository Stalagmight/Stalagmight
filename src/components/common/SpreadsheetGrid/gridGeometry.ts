/**
 * Pure selection / navigation / paste / fill math for SpreadsheetGrid.
 *
 * All functions operate on the DATA MODEL (row indices + visible-column
 * indices), never the DOM - this is the keystone constraint: virtualized rows
 * unmount when scrolled off-screen, so selection, copy, paste and fill must be
 * computable for cells that aren't currently rendered. Unit-tested.
 */
import type { CellCoord, GridSelection } from './types';

// Imported from the shared util (also used internally below) and re-exported so
// existing importers (SpreadsheetGrid, tests) keep importing `clamp` from here.
import { clamp } from '../../../utils/clamp';
export { clamp };

/** Inclusive, normalized rectangle in (row, visibleColumnIndex) space. */
export interface Rect {
  r0: number;
  r1: number;
  c0: number;
  c1: number;
}

/** Normalize a selection to an inclusive rect, given a colKey→visible-index map. */
export function selectionRect(
  sel: GridSelection,
  colIndexOf: (colKey: string) => number,
): Rect {
  const ac = colIndexOf(sel.anchor.colKey);
  const fc = colIndexOf(sel.focus.colKey);
  return {
    r0: Math.min(sel.anchor.row, sel.focus.row),
    r1: Math.max(sel.anchor.row, sel.focus.row),
    c0: Math.min(ac, fc),
    c1: Math.max(ac, fc),
  };
}

export function isInRect(rect: Rect, row: number, colIdx: number): boolean {
  return row >= rect.r0 && row <= rect.r1 && colIdx >= rect.c0 && colIdx <= rect.c1;
}

export function rectIsSingleCell(rect: Rect): boolean {
  return rect.r0 === rect.r1 && rect.c0 === rect.c1;
}

/** Prefix-sum of column widths → left offsets. Returns length `widths.length + 1`
 *  so `offsets[i]` is the left edge of column i and `offsets[n]` the right edge. */
export function columnOffsets(widths: number[]): number[] {
  const offsets = [0];
  for (let i = 0; i < widths.length; i++) {
    offsets.push((offsets[i] ?? 0) + (widths[i] ?? 0));
  }
  return offsets;
}

/** Arrow-key target: move one cell, NO wrap, clamped to the grid. Lands on every
 *  visible column including read-only ones. */
export function arrowTarget(
  row: number,
  colIdx: number,
  dRow: number,
  dCol: number,
  rowCount: number,
  colCount: number,
): { row: number; colIdx: number } {
  return {
    row: clamp(row + dRow, 0, Math.max(0, rowCount - 1)),
    colIdx: clamp(colIdx + dCol, 0, Math.max(0, colCount - 1)),
  };
}

/** Tab / Shift+Tab target: move to the next WRITABLE column, wrapping to the
 *  next/prev row's first/last writable column; clamps at the grid extremes.
 *  `writableCols` is the sorted list of visible-column indices that accept edits.
 *  Returns null when there are no writable columns. */
export function tabTarget(
  row: number,
  colIdx: number,
  dir: 1 | -1,
  writableCols: number[],
  rowCount: number,
): { row: number; colIdx: number } | null {
  if (writableCols.length === 0) return null;
  const first = writableCols[0] as number;
  const lastCol = writableCols[writableCols.length - 1] as number;
  if (dir === 1) {
    const next = writableCols.find((c) => c > colIdx);
    if (next !== undefined) return { row, colIdx: next };
    if (row + 1 < rowCount) return { row: row + 1, colIdx: first };
    return { row, colIdx: lastCol }; // clamp at grid end
  }
  const prev = [...writableCols].reverse().find((c) => c < colIdx);
  if (prev !== undefined) return { row, colIdx: prev };
  if (row - 1 >= 0) return { row: row - 1, colIdx: lastCol };
  return { row, colIdx: first }; // clamp at grid start
}

/**
 * {@link tabTarget}, but also skipping cells `accept` refuses (a column that does
 * not apply to that row, a group-header row). Keeps stepping in Tab order until
 * an accepted cell; null when there is none before the grid edge (Tab then stays
 * put rather than landing on a cell it should have skipped). `maxSteps` bounds a
 * pathological run of refused cells.
 */
export function tabTargetWhere(
  row: number,
  colIdx: number,
  dir: 1 | -1,
  writableCols: number[],
  rowCount: number,
  accept: (row: number, colIdx: number) => boolean,
  maxSteps = 200_000,
): { row: number; colIdx: number } | null {
  let cur = { row, colIdx };
  for (let i = 0; i < maxSteps; i++) {
    const t = tabTarget(cur.row, cur.colIdx, dir, writableCols, rowCount);
    if (!t) return null;
    if (accept(t.row, t.colIdx)) return t;
    // Clamped at the grid's first/last writable cell: nowhere left to go.
    if (t.row === cur.row && t.colIdx === cur.colIdx) return null;
    cur = t;
  }
  return null;
}

/**
 * Cycle the active cell WITHIN a fixed selection rectangle (Google-Sheets
 * Tab/Enter-in-selection). `horizontal` = Tab order (row-major, left→right then
 * wrap to next row); `!horizontal` = Enter order (column-major, top→bottom then
 * wrap to next column). Wraps around the whole rectangle. `reverse` for
 * Shift+Tab / Shift+Enter.
 */
export function cycleActiveInRect(
  rect: Rect,
  activeRow: number,
  activeColIdx: number,
  horizontal: boolean,
  reverse: boolean,
): { row: number; colIdx: number } {
  const w = rect.c1 - rect.c0 + 1;
  const h = rect.r1 - rect.r0 + 1;
  const n = w * h;
  const dr = clamp(activeRow, rect.r0, rect.r1) - rect.r0;
  const dc = clamp(activeColIdx, rect.c0, rect.c1) - rect.c0;
  let pos = horizontal ? dr * w + dc : dc * h + dr;
  pos = (pos + (reverse ? -1 : 1) + n) % n;
  return horizontal
    ? { row: rect.r0 + Math.floor(pos / w), colIdx: rect.c0 + (pos % w) }
    : { row: rect.r0 + (pos % h), colIdx: rect.c0 + Math.floor(pos / h) };
}

export interface PasteTarget {
  row: number;
  colIdx: number;
  raw: string;
}

/**
 * Place a parsed clipboard matrix onto the grid at a selection rect.
 *
 * Rules (Google-Sheets semantics):
 *  - 1×1 clipboard onto a multi-cell selection → fill the whole selection.
 *  - Selection is an exact integer multiple of the clipboard (and larger) →
 *    tile/repeat the clipboard across the selection.
 *  - Otherwise → paste the R×C block anchored at the selection's top-left,
 *    auto-growing past the selection.
 *  - Cells past the last row / last visible column are clipped (counted, never
 *    create new rows/columns).
 */
export function placePaste(
  clip: string[][],
  rect: Rect,
  rowCount: number,
  colCount: number,
  /** Emit targets PAST the last row instead of clipping them, for a host that
   *  grows its row set to fit a paste (provisional/draft rows). Columns are
   *  clipped either way - the column set is fixed. */
  growRows = false,
): { targets: PasteTarget[]; clipped: number } {
  const ch = clip.length;
  const cw = clip.reduce((m, r) => Math.max(m, r.length), 0);
  if (ch === 0 || cw === 0) return { targets: [], clipped: 0 };

  const selH = rect.r1 - rect.r0 + 1;
  const selW = rect.c1 - rect.c0 + 1;
  const single = ch === 1 && cw === 1;
  const tiles =
    !single &&
    selH % ch === 0 &&
    selW % cw === 0 &&
    (selH > ch || selW > cw);

  const destRows = single || tiles ? selH : ch;
  const destCols = single || tiles ? selW : cw;

  const targets: PasteTarget[] = [];
  let clipped = 0;
  for (let i = 0; i < destRows; i++) {
    for (let j = 0; j < destCols; j++) {
      const r = rect.r0 + i;
      const c = rect.c0 + j;
      const raw = single ? (clip[0]?.[0] ?? '') : (clip[i % ch]?.[j % cw] ?? '');
      if ((r >= rowCount && !growRows) || c >= colCount) {
        clipped++;
        continue;
      }
      targets.push({ row: r, colIdx: c, raw });
    }
  }
  return { targets, clipped };
}

/**
 * Fill targets: copy the leading edge of the rect across the rest.
 * Copy-fill only (NO numeric auto-series - deliberate for survey data).
 * `down` fills each column's top row downward; otherwise fills each row's left
 * column rightward. Returns (destRow,destCol)→(srcRow,srcCol) pairs, excluding
 * the source edge itself.
 */
export function fillTargets(
  rect: Rect,
  down: boolean,
): { row: number; colIdx: number; srcRow: number; srcColIdx: number }[] {
  const out: { row: number; colIdx: number; srcRow: number; srcColIdx: number }[] = [];
  if (down) {
    for (let c = rect.c0; c <= rect.c1; c++) {
      for (let r = rect.r0 + 1; r <= rect.r1; r++) {
        out.push({ row: r, colIdx: c, srcRow: rect.r0, srcColIdx: c });
      }
    }
  } else {
    for (let r = rect.r0; r <= rect.r1; r++) {
      for (let c = rect.c0 + 1; c <= rect.c1; c++) {
        out.push({ row: r, colIdx: c, srcRow: r, srcColIdx: rect.c0 });
      }
    }
  }
  return out;
}

/**
 * Where the cursor lands after deleting a run starting at `fromRow`, as a
 * collapsed single-cell selection. The run's END does not matter: the cursor
 * goes above the run either way.
 *
 * The rows below a delete shift up, so a surviving row-range selection stops
 * meaning what it did: after a run at the END it lands on whatever is now last,
 * after one at the START on the next N rows - highlighting rows nobody picked,
 * with Delete still armed on them. The grid's reconciliation only clamps a range
 * into bounds, which is what made the wrong selection look plausible rather than
 * dropping it.
 *
 * Just ABOVE the run, not at its start: after deleting a trailing run that is the
 * last surviving row rather than any blank invitation at the end, and at the top
 * of the table there is nowhere above to go anyway.
 */
export function collapseAfterRowDelete(
  prev: { active: { colKey: string } },
  fromRow: number,
): { anchor: CellCoord; focus: CellCoord; active: CellCoord } {
  const cell = { row: Math.max(0, fromRow - 1), colKey: prev.active.colKey };
  return { anchor: cell, focus: cell, active: cell };
}

/**
 * The selection with every coordinate pulled back into range after the host's
 * rows or columns changed: rows clamped, a dropped column key re-mapped to a
 * surviving column. Returns the SAME object when nothing had to move, so callers
 * can skip a re-render.
 */
export function reconcileSelection<S extends { anchor: CellCoord; focus: CellCoord; active: CellCoord }>(
  prev: S,
  rowCount: number,
  colKeys: readonly string[],
): S | { anchor: CellCoord; focus: CellCoord; active: CellCoord } {
  const fix = (c: CellCoord): CellCoord => {
    const row = clamp(c.row, 0, Math.max(0, rowCount - 1));
    const colKey = colKeys.includes(c.colKey) ? c.colKey : (colKeys[0] ?? c.colKey);
    return row === c.row && colKey === c.colKey ? c : { row, colKey };
  };
  const anchor = fix(prev.anchor);
  const focus = fix(prev.focus);
  const active = fix(prev.active);
  return anchor === prev.anchor && focus === prev.focus && active === prev.active
    ? prev
    : { anchor, focus, active };
}
