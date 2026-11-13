/**
 * The status bar's selection summary: cell count, and for a selection inside
 * one numeric column its sum / average / min / max with the column's unit.
 *
 * The grid reports only the selection's size and active cell
 * (`onSelectionChange`), so the rectangle is read back from the rendered
 * cells' `data-in-selection` marks. Every column renders, so the selected
 * columns are exact; rows are virtualized, so a selection taller than the
 * viewport is extended from its visible end by the reported row count.
 */
import type { DataColumnDef, DataColumnKey, DataModelCtx, DataRow } from './types';
import { isEntityRow } from './model/rows';

export interface GridSelectionInfo {
  rows: number;
  cols: number;
  activeRow: number;
  activeColKey: string;
}

export interface SelectionRect {
  r0: number;
  r1: number;
  colKeys: string[];
}

export function selectionRectFromDom(container: HTMLElement | null, prefix: string, sel: GridSelectionInfo): SelectionRect {
  const fallback: SelectionRect = { r0: sel.activeRow, r1: sel.activeRow + sel.rows - 1, colKeys: [sel.activeColKey] };
  if (!container) return fallback;
  const cellRe = new RegExp(`^${prefix}-cell-(\\d+)-(.+)$`);
  let rMin = Infinity;
  let rMax = -Infinity;
  const cols = new Set<string>();
  for (const el of container.querySelectorAll<HTMLElement>('[data-in-selection="true"]')) {
    const m = cellRe.exec(el.dataset.testid ?? '');
    if (!m) continue;
    const r = Number(m[1]);
    rMin = Math.min(rMin, r);
    rMax = Math.max(rMax, r);
    cols.add(m[2]!);
  }
  if (!Number.isFinite(rMin)) return fallback;
  const colKeys = [...cols];
  if (rMax - rMin + 1 >= sel.rows) return { r0: rMin, r1: rMax, colKeys };
  // Part of the selection is scrolled away: grow it from the visible end that
  // is NOT at the edge of the rendered rows.
  let first = Infinity;
  let last = -Infinity;
  const gutterRe = new RegExp(`^${prefix}-gutter-(\\d+)$`);
  for (const el of container.querySelectorAll<HTMLElement>(`[data-testid^="${prefix}-gutter-"]`)) {
    const m = gutterRe.exec(el.dataset.testid ?? '');
    if (!m) continue;
    first = Math.min(first, Number(m[1]));
    last = Math.max(last, Number(m[1]));
  }
  if (rMin > first) return { r0: rMin, r1: rMin + sel.rows - 1, colKeys };
  if (rMax < last) return { r0: rMax - sel.rows + 1, r1: rMax, colKeys };
  return { r0: rMin, r1: rMin + sel.rows - 1, colKeys };
}

export interface SelectionSummary {
  cells: number;
  /** Present when the numeric cells all sit in one column. */
  numeric: { colKey: DataColumnKey; count: number; sum: number; avg: number; min: number; max: number } | null;
}

/** Selections bigger than this report only their size (no per-cell reads). */
const MAX_SUMMARY_CELLS = 250_000;

export function summarizeSelection(
  rows: readonly DataRow[],
  rect: SelectionRect,
  columnByKey: ReadonlyMap<DataColumnKey, DataColumnDef>,
  ctx: DataModelCtx,
  cells: number,
): SelectionSummary {
  const numericCols = rect.colKeys.filter((k) => columnByKey.get(k as DataColumnKey)?.valueType === 'number');
  if (numericCols.length !== 1 || cells > MAX_SUMMARY_CELLS) return { cells, numeric: null };
  const colKey = numericCols[0] as DataColumnKey;
  const def = columnByKey.get(colKey)!;
  let count = 0;
  let sum = 0;
  let min = Infinity;
  let max = -Infinity;
  const from = Math.max(0, rect.r0);
  const to = Math.min(rows.length - 1, rect.r1);
  for (let i = from; i <= to; i++) {
    const row = rows[i];
    if (!isEntityRow(row)) continue;
    const v = def.value(row, ctx);
    if (typeof v !== 'number' || !Number.isFinite(v)) continue;
    count++;
    sum += v;
    if (v < min) min = v;
    if (v > max) max = v;
  }
  if (count === 0) return { cells, numeric: null };
  return { cells, numeric: { colKey, count, sum, avg: sum / count, min, max } };
}

/** Display unit of a numeric column ('' for none). */
export function columnUnit(key: DataColumnKey): string {
  switch (key) {
    case 'azimuth':
    case 'azimuth_in':
    case 'azimuth_out':
    case 'inclination':
    case 'pitch_in':
    case 'pitch_out':
    case 'lat':
    case 'long':
      return '°';
    case 'temperature':
      return '';
    default:
      return 'm';
  }
}
