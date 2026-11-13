/**
 * Pure helpers for moving through find matches: where a match sits in the
 * view, and which match to select after the matches change (an edit, a filter,
 * a replace) so the current match stays put or moves to the nearest one after.
 */
import type { DataColumnKey, DataRow, SearchMatch } from '../types';

/** A spot in a cell's text: the identity of a match. */
export interface MatchSpot {
  rowKey: string;
  colKey: DataColumnKey;
  start: number;
}

export interface MatchOrder {
  /** Display index of a row in the current view (undefined = not shown). */
  rowPos: (rowKey: string) => number | undefined;
  /** Display index of a column. */
  colPos: (colKey: DataColumnKey) => number;
  /** Survey-order position of any row (shown or not), for a spot whose row left the view. */
  surveyOrder: (rowKey: string) => number | undefined;
}

const rowPosCache = new WeakMap<readonly DataRow[], Map<string, number>>();

/** rowKey -> display index for a view's rows (cached per rows array). */
export function rowPositions(rows: readonly DataRow[]): Map<string, number> {
  let map = rowPosCache.get(rows);
  if (!map) {
    map = new Map();
    for (let i = 0; i < rows.length; i++) map.set(rows[i]!.key, i);
    rowPosCache.set(rows, map);
  }
  return map;
}

export function spotOf(m: SearchMatch): MatchSpot {
  return { rowKey: m.rowKey, colKey: m.colKey, start: m.start };
}

/** Index of the match exactly at `spot`, or -1. */
export function exactMatchIndex(matches: readonly SearchMatch[], spot: MatchSpot): number {
  return matches.findIndex((m) => m.rowKey === spot.rowKey && m.colKey === spot.colKey && m.start === spot.start);
}

/**
 * The first match at or after `spot` in display order, wrapping to the first
 * match. -1 only when there are no matches.
 */
export function firstAtOrAfter(matches: readonly SearchMatch[], spot: MatchSpot, order: MatchOrder): number {
  if (matches.length === 0) return -1;
  const p = order.rowPos(spot.rowKey);
  if (p !== undefined) {
    const c = order.colPos(spot.colKey);
    for (let i = 0; i < matches.length; i++) {
      const m = matches[i]!;
      const mp = order.rowPos(m.rowKey) ?? -1;
      if (mp > p) return i;
      if (mp < p) continue;
      const mc = order.colPos(m.colKey);
      if (mc > c || (mc === c && m.start >= spot.start)) return i;
    }
    return 0;
  }
  // The row left the view: continue from where it sat in survey order.
  const o = order.surveyOrder(spot.rowKey);
  if (o !== undefined) {
    for (let i = 0; i < matches.length; i++) {
      const mo = order.surveyOrder(matches[i]!.rowKey);
      if (mo !== undefined && mo > o) return i;
    }
  }
  return 0;
}

/** The first match on or after a row (the active row), else the first match. */
export function firstFromRow(matches: readonly SearchMatch[], rowKey: string | null, order: MatchOrder): number {
  if (matches.length === 0) return -1;
  if (rowKey === null) return 0;
  const p = order.rowPos(rowKey);
  if (p === undefined) return 0;
  for (let i = 0; i < matches.length; i++) {
    if ((order.rowPos(matches[i]!.rowKey) ?? -1) >= p) return i;
  }
  return 0;
}

/**
 * The match to select after `matches` replaced `prev`:
 * - an explicit anchor (set after a replace) wins: the match at or after it;
 * - a new pattern starts from the active row;
 * - otherwise the same match if it still exists, else the nearest one after it.
 */
export function reconcileCurrent(args: {
  matches: readonly SearchMatch[];
  previous: MatchSpot | null;
  anchor: MatchSpot | null;
  patternChanged: boolean;
  activeRowKey: string | null;
  order: MatchOrder;
}): number {
  const { matches, previous, anchor, patternChanged, activeRowKey, order } = args;
  if (matches.length === 0) return -1;
  if (anchor) return firstAtOrAfter(matches, anchor, order);
  if (patternChanged || previous === null) return firstFromRow(matches, activeRowKey, order);
  const exact = exactMatchIndex(matches, previous);
  return exact >= 0 ? exact : firstAtOrAfter(matches, previous, order);
}
