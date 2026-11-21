/**
 * The two host-driven "go to this row" props, `selectedRow` and `scrollTo`.
 *
 * Both move the grid's selection and scroll it into view, and neither calls
 * `onActiveCellChange`: the host already knows where it sent the grid, and
 * echoing it back would loop a map/table selection sync.
 *
 *  - `selectedRow` reacts when the number CHANGES (minimum scroll, active
 *    column kept), so it can't fight the grid's own navigation.
 *  - `scrollTo` reacts to a new `nonce` (so the same target can be re-fired),
 *    can name a column and centre the row, and makes that one cell the whole
 *    selection. It moves DOM focus to the grid only when nothing else is
 *    typing: `focus: true` forces it, `false` never, and by default only when
 *    focus is already in the grid or on <body> - a host find box keeps focus.
 *
 * Out-of-range rows are ignored (a `scrollTo` nonce is still consumed).
 */
import { useEffect, useRef, type Dispatch, type RefObject, type SetStateAction } from 'react';
import type { GridScrollRequest, GridSelection } from './types';

export interface GridScrollRequestDeps {
  selectedRow: number | null | undefined;
  scrollTo: GridScrollRequest | null | undefined;
  rowCount: number;
  colKeys: string[];
  activeColKey: string;
  colIndexOf: (key: string) => number;
  setSelection: Dispatch<SetStateAction<GridSelection>>;
  /** Drop the "selected via the gutter" intent (Delete = clear, not delete rows). */
  clearRowIntent: () => void;
  scrollRowIntoView: (row: number, align: 'auto' | 'center') => void;
  scrollColIntoView: (colIdx: number) => void;
  scrollRef: RefObject<HTMLDivElement | null>;
}

export function useGridScrollRequests(deps: GridScrollRequestDeps): void {
  const {
    selectedRow, scrollTo, rowCount, colKeys, activeColKey, colIndexOf,
    setSelection, clearRowIntent, scrollRowIntoView, scrollColIntoView, scrollRef,
  } = deps;

  // External row selection: react only when the prop CHANGES (prev ref), so the
  // grid's own navigation isn't fought.
  const prevSelectedRow = useRef<number | null | undefined>(undefined);
  useEffect(() => {
    const prev = prevSelectedRow.current;
    prevSelectedRow.current = selectedRow;
    if (selectedRow == null || selectedRow === prev) return;
    if (selectedRow < 0 || selectedRow >= rowCount) return;
    setSelection((prevSel) => {
      if (prevSel.active.row === selectedRow) return prevSel;
      const colKey = colKeys.includes(prevSel.active.colKey) ? prevSel.active.colKey : colKeys[0];
      if (colKey === undefined) return prevSel;
      clearRowIntent();
      const cell = { row: selectedRow, colKey };
      return { anchor: cell, focus: cell, active: cell };
    });
    scrollRowIntoView(selectedRow, 'auto');
    // Follow horizontally too, on the column the updater above lands on (the
    // active column, or the first one when it no longer exists).
    const key = colKeys.includes(activeColKey) ? activeColKey : colKeys[0];
    if (key !== undefined) scrollColIntoView(colIndexOf(key));
  }, [selectedRow, rowCount, colKeys, activeColKey, colIndexOf, scrollColIntoView, scrollRowIntoView, setSelection, clearRowIntent]);

  const lastNonce = useRef<number | undefined>(undefined);
  useEffect(() => {
    if (!scrollTo || scrollTo.nonce === lastNonce.current) return;
    lastNonce.current = scrollTo.nonce;
    const { row } = scrollTo;
    if (!Number.isInteger(row) || row < 0 || row >= rowCount) return;
    const named = scrollTo.colKey !== undefined && colKeys.includes(scrollTo.colKey);
    const colKey = named ? scrollTo.colKey : colKeys.includes(activeColKey) ? activeColKey : colKeys[0];
    if (colKey === undefined) return;
    clearRowIntent();
    const cell = { row, colKey };
    setSelection({ anchor: cell, focus: cell, active: cell });
    scrollRowIntoView(row, scrollTo.align === 'center' ? 'center' : 'auto');
    if (named) scrollColIntoView(colIndexOf(colKey));
    const el = scrollRef.current;
    if (!el) return;
    const ae = document.activeElement;
    const focus = scrollTo.focus ?? (!ae || ae === document.body || el.contains(ae));
    if (focus && ae !== el) el.focus({ preventScroll: true });
    // Deliberately keyed on the request only: a later rows/columns change must
    // not replay an old request.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scrollTo]);
}
